//! Host pipelines are made by workers, not by the title thread that first
//! records a draw with them. Translating a shader and the host driver's
//! compile take from milliseconds to seconds each (structurisation of the
//! biggest compute programs ~0.4 s, some driver compiles 3.4 s), and
//! the title's first world frame wants over 1,200 new pipelines: made
//! on its recording threads, that frame took more than 30 s and the
//! title's GPU watchdog ended the run (decision 0007, "Pipelines made off
//! the title's threads").
//!
//! A first use queues the pipeline and waits for it within a budget per
//! frame, so the cheap ones (menus, one-off passes such as a colour-grading
//! bake) are there at once as before; past the budget a draw or dispatch
//! whose pipeline is not ready is dropped, as an untranslated one is.
//!
//! The default budget is half a second per title frame. A pass the title
//! runs only once and whose pipeline takes longer is lost. Passes over the
//! screen (no vertex input, no depth) have workers and a longer budget of
//! their own ([`screen_budget`]): the title fills its lighting tables with
//! such draws once, at the world's first frame, and behind a world's worth of
//! pipelines they were dropped; that run had no lamps and no diffuse light to
//! its end (run textures-d2, the first after a new translation; Belobug
//! 18e57db8 found the same).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::Shader;
use super::host::{GraphicsState, HostPipeline};

/// What a host pipeline is made from, and the host pipeline once made.
pub(crate) struct Pending {
    shaders: Vec<Arc<Shader>>,
    pub(crate) graphics: Option<Box<GraphicsState>>,
    queue_state: AtomicU8,
    /// Taken by a worker from [`Queue::slow`].
    was_slow: AtomicBool,
    made: OnceLock<Option<HostPipeline>>,
    /// A thread is making it ([`Pending::make_with`]); others wait for
    /// `made` or `failed` instead of making it too.
    building: AtomicBool,
    /// Its making ended the thread making it, and the thread that made it
    /// again ([`reap`], [`rescue`]): `made` is then never set, and no one
    /// waits for it.
    failed: AtomicBool,
    /// Made again on a thread with a larger stack after its worker ended
    /// ([`rescue`]).
    rescued: AtomicBool,
    /// What identifies it across runs (shader containers and state), and
    /// whether a draw or dispatch has requested it yet.
    key: String,
    used: AtomicBool,
    /// When a worker took it to make (unset until then), and whether
    /// draws have been told to stop waiting for that worker ([`stall_bound`]).
    claimed: OnceLock<Instant>,
    stall_noted: AtomicBool,
    /// How long draws have waited for it so far, in microseconds
    /// ([`wait_once`]).
    waited_us: AtomicU64,
    /// A pass over the screen ([`GraphicsState::screen_pass`]) or a confirmed
    /// opening-scene / bake producer that must never sit behind cold RT shaders.
    screen: bool,
    /// Confirmed first-scene / bake producer prepared early even on a cold cache.
    scene_input: bool,
    /// Why this pipeline is intentionally skipped (e.g. RT resolve without BVH).
    left_out: Option<&'static str>,
    /// A compute pipeline whose dispatches have a wait of their own
    /// ([`compute_budget`]), the reasons ([`Lost`], bits) a dropped dispatch
    /// of it was logged for, whether a dispatch has had a full wait for it
    /// ([`COMPUTE_CHANCE`]; its only one with [`compute_wait_once`]), and
    /// when a dispatch first waited for it ([`Facts::unclaimed`]).
    compute: bool,
    compute_said: AtomicU8,
    compute_waited: AtomicBool,
    compute_asked: OnceLock<Instant>,
    /// When a draw or dispatch first asked for it, how many were dropped
    /// while it was being made, and whether one has been drawn with it
    /// since it was made after such drops (the account, [`account_line`]).
    asked: OnceLock<Instant>,
    dropped: AtomicU64,
    drawn_after: AtomicBool,
}

fn named(shaders: &[Arc<Shader>], name: &[u8]) -> bool {
    shaders
        .iter()
        .any(|s| s.stage == "compute" && s.container.windows(name.len()).any(|w| w == name))
}

fn left_out(shaders: &[Arc<Shader>]) -> Option<&'static str> {
    static SKIP_RT_RESOLVE: OnceLock<bool> = OnceLock::new();
    let skip_rt = *SKIP_RT_RESOLVE.get_or_init(|| {
        let builder = std::env::var_os("VICTORY_BVH_BUILDER").is_some_and(|v| v == "1");
        if builder {
            return false;
        }
        // Without BVH, ~1,000 CS_RaytracingResolve shaders shade no hits while
        // clogging the cold compiler queue; skip unless VICTORY_RT_RESOLVE=1.
        std::env::var_os("VICTORY_RT_RESOLVE").is_none_or(|v| v == "0")
    });
    if skip_rt && named(shaders, b"CS_RaytracingResolve") {
        return Some("shades ray-tracing hits without BVH, left out");
    }
    static SKIP_ACTIVATE_RAYS: OnceLock<bool> = OnceLock::new();
    let skip_activate = *SKIP_ACTIVATE_RAYS.get_or_init(|| {
        let builder = std::env::var_os("VICTORY_BVH_BUILDER").is_some_and(|v| v == "1");
        if builder {
            return std::env::var_os("VICTORY_ACTIVATE_RAYS").is_some_and(|v| v == "0");
        }
        std::env::var_os("VICTORY_ACTIVATE_RAYS").is_none_or(|v| v == "0")
    });
    if skip_activate && named(shaders, b"CS_ScreenProbes_ActivateRays") {
        return Some("ray activation shader without BVH, left out");
    }
    None
}

/// Confirmed first-scene producers. Prepare these at creation even on a cold
/// launch so cutscene lighting, tiling, and bakes never miss their first frame.
fn scene_input(shaders: &[Arc<Shader>]) -> bool {
    shaders.iter().any(|s| {
        matches!(
            s.hash.as_str(),
            "970c4fbb3b017c0e"
                | "068c90badc86f1fd"
                | "e4a7e088c7780ce8"
                | "ebe4e53f031f5249"
                | "2cc4c197ee29e291"
                | "369cbe906ea72764"
        ) || crate::draw::BAKE_PIXEL_SHADERS.contains(&s.hash.as_str())
    })
}

// SAFETY: D3D12 objects are free-threaded; the graphics state (whose input
// elements point into its own names) is only read after creation.
unsafe impl Send for Pending {}
// SAFETY: as above.
unsafe impl Sync for Pending {}

impl Pending {
    pub(crate) fn new(shaders: Vec<Arc<Shader>>, graphics: Option<Box<GraphicsState>>) -> Pending {
        let mut key = blake3::Hasher::new();
        for s in &shaders {
            key.update(s.stage.as_bytes());
            key.update(s.hash.as_bytes());
        }
        key.update(graphics.as_ref().map(|g| g.describe()).unwrap_or_default().as_bytes());
        let scene_input = scene_input(&shaders);
        let screen = (graphics.as_ref().is_some_and(|g| g.screen_pass())
            && shaders.iter().all(|s| s.stage != "mesh"))
            || scene_input;
        let left_out = left_out(&shaders);
        Pending {
            compute: graphics.is_none()
                && shaders.first().is_some_and(|s| s.stage == "compute")
                && !compute_budget().is_zero(),
            compute_said: AtomicU8::new(0),
            compute_waited: AtomicBool::new(false),
            compute_asked: OnceLock::new(),
            screen,
            scene_input,
            left_out,
            shaders,
            graphics,
            queue_state: AtomicU8::new(NOT_QUEUED),
            was_slow: AtomicBool::new(false),
            made: OnceLock::new(),
            building: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            rescued: AtomicBool::new(false),
            key: key.finalize().to_hex()[..32].to_owned(),
            used: AtomicBool::new(false),
            claimed: OnceLock::new(),
            stall_noted: AtomicBool::new(false),
            waited_us: AtomicU64::new(0),
            asked: OnceLock::new(),
            dropped: AtomicU64::new(0),
            drawn_after: AtomicBool::new(false),
        }
    }

    pub(super) fn key(&self) -> &str {
        &self.key
    }

    /// Whether a worker has held it for `bound` or longer at `now`, longer
    /// than any compile should take ([`stall_bound`]). Said once. The worker
    /// is left to finish: a pipeline it makes later is used as any other.
    fn stalled(&self, now: Instant, bound: Option<Duration>) -> bool {
        let (Some(bound), Some(&claimed)) = (bound, self.claimed.get()) else {
            return false;
        };
        let held = now.saturating_duration_since(claimed);
        if held < bound || self.made.get().is_some() {
            return false;
        }
        if !self.stall_noted.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                shaders = ?self.shaders,
                key = %self.key,
                held_s = held.as_secs(),
                "a host pipeline worker has not returned from making a pipeline: its draws and dispatches no longer wait for it (VICTORY_PIPELINE_STALL_S, 0: waited for as any other)"
            );
        }
        true
    }

    /// The host pipeline, made now, on a stack that holds its making
    /// ([`on_a_large_stack`]).
    fn build(&self) -> Option<HostPipeline> {
        let began = Instant::now();
        let made = on_a_large_stack(|| self.build_here());
        super::library::note_time(&self.key, began.elapsed().as_millis() as u64);
        made
    }

    fn build_here(&self) -> Option<HostPipeline> {
        HostPipeline::new(&self.shaders, self.graphics.as_deref())
    }
}

thread_local! {
    /// Whether this thread has a stack made for pipelines (a worker's, the
    /// rescue's, one of [`on_a_large_stack`]).
    static LARGE_STACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether what goes deep is run on a stack made for it when the thread
/// that asks has none ([`on_a_large_stack`]). On by default;
/// `VICTORY_LARGE_STACK=0`: on the asking thread's own stack, as before.
fn large_stacks() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_LARGE_STACK").is_none_or(|v| v != "0"))
}

/// Runs `make` (a host pipeline's making, a shader's translation) on a
/// stack large enough for it: here if this thread has one (a worker) or
/// [`large_stacks`] says no, else on a thread of its own with a worker's
/// stack ([`worker_stack`]), waited for.
///
/// The host converts a pipeline's code on the thread that asks, deep into
/// its control flow, and the translator goes as deep. A thread of the
/// title's that makes a pipeline itself ([`now`], a first use that finds no
/// worker) or translates a shader at its first use has the stack the title
/// gave it, 1 MiB by default. Under Wine the overflow does not end the
/// process: the thread stands still (zeex64 502e905: a world's loading
/// stood so at 59 s), or a fault taken on a nearly full stack faults again
/// inside the handler and the process ends with no exception for anyone to
/// log. (A larger default for every thread is no way, upstream found: at
/// 64 MiB the title stops at its start.)
pub(crate) fn on_a_large_stack<T: Send>(make: impl FnOnce() -> T + Send) -> T {
    let Some(stack) = stack_now().filter(|_| large_stacks() && !LARGE_STACK.with(|l| l.get())) else {
        return make();
    };
    let mut make = Some(make);
    let made = std::thread::scope(|s| {
        let spawned = std::thread::Builder::new().name("victory-pipeline-now".into()).stack_size(stack).spawn_scoped(s, || {
            LARGE_STACK.with(|l| l.set(true));
            (make.take().expect("taken once"))()
        });
        spawned.ok().map(|thread| thread.join())
    });
    match made {
        Some(Ok(made)) => made,
        // What it threw is the caller's to see, as if made here.
        Some(Err(thrown)) => std::panic::resume_unwind(thrown),
        // No thread to be had: here after all.
        None => (make.take().expect("not run on a thread"))(),
    }
}

impl Pending {
    fn make(&self) {
        self.make_with(|| self.build());
    }

    /// Makes it with `build` unless made, or waits for the thread making it
    /// until it is made or given up ([`reap`]). Not inside `made`'s
    /// initialisation: a thread that ends while making a pipeline (a stack
    /// overflow in vkd3d-proton under Wine ends it without unwinding) would
    /// hold that for good, and every later caller ([`now`], on the title's
    /// own thread) would wait for ever (DoubleGhosty 5e7faab).
    fn make_with(&self, build: impl FnOnce() -> Option<HostPipeline>) {
        if self.made.get().is_none() {
            if self.building.swap(true, Ordering::AcqRel) {
                let (lock, done) = &DONE;
                let mut asked = Instant::now();
                while self.made.get().is_none() && !self.failed.load(Ordering::Acquire) {
                    {
                        let held = lock.lock().unwrap_or_else(|e| e.into_inner());
                        if self.made.get().is_some() || self.failed.load(Ordering::Acquire) {
                            break;
                        }
                        // (Timed: a notification may come between the test
                        // and the wait.)
                        let _ = done.wait_timeout(held, Duration::from_millis(50));
                    }
                    // Whether the thread making it ended is asked here too:
                    // nobody else may be waiting who would ask. Outside
                    // `DONE`, which `give_up` takes (DoubleGhosty 42b95b9).
                    if asked.elapsed() >= Duration::from_secs(1) {
                        reap();
                        asked = Instant::now();
                    }
                }
                return;
            }
            // A build that panics must end this making and wake its waiters.
            let made = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
                Ok(pipeline) => pipeline,
                Err(_) => {
                    tracing::warn!(shaders = ?self.shaders, "host pipeline build panicked");
                    None
                }
            };
            let _ = self.made.set(made);
        }
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    }
}

/// Why a pipeline has no host pipeline (diagnostics): its shaders that do
/// not translate, else "refused", "left out" (and why), "given up",
/// "stalled" or "being made"; `None` if it has one.
///
/// "Being made" is said only of one queued or on a worker. Pipelines left
/// out on purpose are never queued and never made, and were said to be
/// "being made" with the rest: some 590 indirect executions a frame of the
/// ray-tracing resolve, the whole run, while no pipeline was queued at all
/// (runs 20261127-210643 and -200336, the frame line's "(0 being made)").
#[cfg(feature = "diagnostics")]
pub(crate) fn missing(p: &Pending) -> Option<String> {
    if let Some(why) = p.left_out {
        return Some(format!("left out: {why}"));
    }
    match p.made.get() {
        Some(Some(_)) => None,
        None if p.failed.load(Ordering::Relaxed) => {
            Some("given up: making it ended its thread, and again on the larger stack".into())
        }
        None if p.stalled(Instant::now(), stall_bound()) => {
            Some("stalled: its worker has not returned (VICTORY_PIPELINE_STALL_S)".into())
        }
        None => Some("being made".into()),
        Some(None) => {
            let failed: Vec<String> = p
                .shaders
                .iter()
                .filter(|s| matches!(s.translated.get(), Some(None)))
                .map(|s| format!("{} {}", s.stage, s.hash))
                .collect();
            Some(if failed.is_empty() {
                format!("refused {:?}", p.shaders)
            } else {
                failed.join(" + ")
            })
        }
    }
}

/// The hashes of the shaders `p` is made of.
pub(crate) fn hashes(p: &Pending) -> String {
    let hashes: Vec<&str> = p.shaders.iter().map(|s| s.hash.as_str()).collect();
    hashes.join("+")
}

/// Signalled whenever a worker has made a pipeline.
static DONE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

/// Waiting allowed per title frame, over all threads.
fn wait_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = std::env::var("VICTORY_PIPELINE_WAIT_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(500);
        tracing::info!(millis, "host pipeline wait budget per frame");
        Duration::from_millis(millis)
    })
}

/// As [`wait_budget`], for passes over the screen (`VICTORY_SCREEN_WAIT_MS`,
/// 10 s; 0: like the others). Below the title's 30 s watchdog.
fn screen_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = std::env::var("VICTORY_SCREEN_WAIT_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(10_000);
        Duration::from_millis(millis)
    })
}

/// Whether passes over the screen go before the others.
fn screens_first() -> bool {
    !screen_budget().is_zero()
}

/// How long a worker may hold a pipeline before its draws and dispatches
/// stop waiting for it: `VICTORY_PIPELINE_STALL_S` seconds, 15 if unset,
/// well past the longest compile measured (3.4 s); `None` for 0, no such
/// bound. Under Wine a worker can stay inside the host's compile of a
/// compute pipeline for good (Legendary a1058fa: NVIDIA's compiler hangs on
/// one compute shader), and a wait for it is time the frame never gets
/// back. With [`wait_once`] a draw waits for such a pipeline one budget in
/// all anyway; this ends the waits that have no such end, those of
/// `VICTORY_PIPELINE_WAIT_ONCE=0` and those of
/// `VICTORY_PIPELINE_COMPUTE_WAIT=1` begun after the bound, and names the
/// pipeline in the log.
fn stall_bound() -> Option<Duration> {
    static BOUND: OnceLock<Option<Duration>> = OnceLock::new();
    *BOUND.get_or_init(|| {
        let secs = std::env::var("VICTORY_PIPELINE_STALL_S").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(15);
        (secs != 0).then(|| Duration::from_secs(secs))
    })
}

/// Pipelines queued and not made yet.
static WAITING: AtomicU64 = AtomicU64::new(0);

/// Pipelines queued and not made yet (for the frame line).
pub(crate) fn waiting() -> u64 {
    WAITING.load(Ordering::Relaxed)
}

/// The host pipeline if it is made (or is made within the frame's budget);
/// queues it on first use.
pub(crate) fn ready(p: &Arc<Pending>) -> Option<&HostPipeline> {
    if p.left_out.is_some() {
        return None;
    }
    // A still-pending pipeline was requested by the title too. Prewarm it
    // on the next launch, even if this run never got to use it.
    if !p.used.load(Ordering::Relaxed) && !p.used.swap(true, Ordering::Relaxed) {
        let _ = p.asked.set(Instant::now());
        super::library::note_used(&p.key);
    }
    // A compute dispatch's own wait, if it had one ([`compute_budget`]).
    let mut waited = None;
    if p.made.get().is_none() && !p.failed.load(Ordering::Relaxed) {
        start(p, REQUESTED);
        // (A worker that ended is found even when nobody waits to the end
        // of a budget any more.)
        reap_now_and_then();
        if p.stalled(Instant::now(), stall_bound()) {
            // Its worker has not come back: nothing is waited for.
        } else if p.graphics.is_none() && compute_waits() {
            // Made here, or waited for while a worker makes it.
            p.make();
        } else {
            waited = wait(p, crate::frame::current());
        }
    }
    if p.compute {
        // A dispatch still without its pipeline is dropped by the caller:
        // why, counted, and said once a pipeline and reason.
        if let Some(waited) = waited {
            compute_note_wait(waited, p.made.get().is_some());
        }
        let lost = match p.made.get() {
            Some(Some(_)) => None,
            Some(None) if Facts::of(p, Instant::now()).untranslated => Some(Lost::Untranslated),
            Some(None) => Some(Lost::Refused),
            None => Some(Facts::of(p, Instant::now()).hopeless().unwrap_or(if waited.is_some() {
                Lost::NotYet
            } else {
                Lost::FrameSpent
            })),
        };
        if let Some(lost) = lost {
            compute_dropped(p, lost, crate::frame::current());
        }
        // (Not for a dispatch whose pipeline was there: nothing to count.)
        if lost.is_some() || waited.is_some() {
            compute_line(Instant::now());
        }
    }
    let made = p.made.get();
    match made {
        // Dropped by the caller for want of it while it is being made.
        None if !p.failed.load(Ordering::Relaxed) => {
            p.dropped.fetch_add(1, Ordering::Relaxed);
            super::account::LATE_DROPS.fetch_add(1, Ordering::Relaxed);
            account_now_and_then();
        }
        Some(Some(_)) if p.dropped.load(Ordering::Relaxed) > 0 && !p.drawn_after.load(Ordering::Relaxed) => {
            p.drawn_after.store(true, Ordering::Relaxed);
        }
        _ => {}
    }
    made?.as_ref()
}

/// Why a dispatch of a compute pipeline was dropped, its pipeline not made
/// ([`ready`]). The first two leave it its waits in later frames; the
/// others end them, for the pipeline can no longer come in time (waiting
/// half a second a frame for one that never comes would be a picture at 2
/// frames a second for good).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Lost {
    /// Waited to the frame's deadline, still being made: waits again.
    NotYet,
    /// The frame's compute wait was spent by other dispatches: waits again.
    FrameSpent,
    /// Its worker has held it past `VICTORY_PIPELINE_STALL_S`.
    Stalled,
    /// Making it ended its worker, and the rescue too (or there was none).
    Failed,
    /// One of its shaders does not translate.
    Untranslated,
    /// Made, and the host gave no pipeline for it.
    Refused,
    /// No worker took it within `VICTORY_PIPELINE_STALL_S` of its first wait.
    NoWorker,
    /// It had its one wait (`VICTORY_COMPUTE_WAIT_ONCE=1`).
    HadItsWait,
}

impl Lost {
    const ALL: [Lost; 8] = [
        Lost::NotYet,
        Lost::FrameSpent,
        Lost::Stalled,
        Lost::Failed,
        Lost::Untranslated,
        Lost::Refused,
        Lost::NoWorker,
        Lost::HadItsWait,
    ];

    fn says(self) -> &'static str {
        match self {
            Lost::NotYet => "still being made at the frame's deadline: waits again in the next frame",
            Lost::FrameSpent => "the frame's compute wait was spent by other dispatches: waits again in the next frame",
            Lost::Stalled => {
                "its worker has not returned from the host's compile (VICTORY_PIPELINE_STALL_S): not waited for again"
            }
            Lost::Failed => "making it ended its worker, and the rescue's too: not waited for again",
            Lost::Untranslated => "one of its shaders does not translate: not waited for again",
            Lost::Refused => "the host gave no pipeline for it: not waited for again",
            Lost::NoWorker => {
                "no worker took it within VICTORY_PIPELINE_STALL_S of its first wait: not waited for again until one does"
            }
            Lost::HadItsWait => "it had its one wait (VICTORY_COMPUTE_WAIT_ONCE=1): not waited for again",
        }
    }

    /// Whether it ends the pipeline's waits (said once a pipeline).
    fn ends(self) -> bool {
        !matches!(self, Lost::NotYet | Lost::FrameSpent)
    }
}

/// What decides whether a dispatch waits for a compute pipeline not made.
#[derive(Clone, Copy, Debug, Default)]
struct Facts {
    failed: bool,
    stalled: bool,
    untranslated: bool,
    /// Waited for since [`stall_bound`] or longer, and no worker has taken
    /// it (all of them in compiles that do not return).
    unclaimed: bool,
    /// A full wait ended without it ([`COMPUTE_CHANCE`]).
    had_its_wait: bool,
    /// `VICTORY_COMPUTE_WAIT_ONCE=1`.
    once: bool,
}

impl Facts {
    fn of(p: &Pending, now: Instant) -> Facts {
        let bound = stall_bound();
        let past = |since: Option<&Instant>| {
            since.zip(bound).is_some_and(|(&since, bound)| now.saturating_duration_since(since) >= bound)
        };
        Facts {
            failed: p.failed.load(Ordering::Relaxed),
            stalled: p.made.get().is_none() && past(p.claimed.get()),
            untranslated: p.shaders.iter().any(|s| matches!(s.translated.get(), Some(None))),
            unclaimed: p.claimed.get().is_none() && past(p.compute_asked.get()),
            had_its_wait: p.compute_waited.load(Ordering::Relaxed),
            once: compute_wait_once(),
        }
    }

    /// Why no dispatch waits for the pipeline any more, if none does: it
    /// cannot come (or, with the switch, it had its wait). Otherwise a
    /// dispatch waits again in every frame, within the frame's deadline,
    /// for as long as the pipeline is being made.
    fn hopeless(self) -> Option<Lost> {
        if self.failed {
            Some(Lost::Failed)
        } else if self.untranslated {
            Some(Lost::Untranslated)
        } else if self.stalled {
            Some(Lost::Stalled)
        } else if self.unclaimed {
            Some(Lost::NoWorker)
        } else if self.once && self.had_its_wait {
            Some(Lost::HadItsWait)
        } else {
            None
        }
    }
}

/// `VICTORY_COMPUTE_WAIT_ONCE=1`: a compute pipeline has one wait in all,
/// as before (cebe78e): its later dispatches wait as draws do and are
/// dropped. Unset: a dispatch waits again in later frames while its
/// pipeline is being made ([`Facts::hopeless`]).
fn compute_wait_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COMPUTE_WAIT_ONCE").is_some_and(|v| v == "1"))
}

/// Since the last [`compute_line`]: dispatches that had a wait of their
/// own, its microseconds, how many got their pipeline in it; and the
/// dispatches dropped, by [`Lost`].
static COMPUTE_WAITS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static COMPUTE_LOST: [AtomicU64; Lost::ALL.len()] = [const { AtomicU64::new(0) }; Lost::ALL.len()];

fn compute_note_wait(waited: Duration, made: bool) {
    COMPUTE_WAITS[0].fetch_add(1, Ordering::Relaxed);
    COMPUTE_WAITS[1].fetch_add(waited.as_micros() as u64, Ordering::Relaxed);
    COMPUTE_WAITS[2].fetch_add(u64::from(made), Ordering::Relaxed);
}

/// A dispatch of `p` is dropped for `lost`: counted, and said once a
/// pipeline and reason where that ends its waits.
fn compute_dropped(p: &Pending, lost: Lost, frame: u64) {
    COMPUTE_LOST[lost as usize].fetch_add(1, Ordering::Relaxed);
    let bit = 1u8 << (lost as u8);
    if lost.ends() && p.compute_said.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        tracing::info!(
            shaders = %hashes(p),
            key = %p.key,
            frame,
            why = lost.says(),
            "a compute dispatch is dropped, its pipeline not made (VICTORY_COMPUTE_WAIT_MS)"
        );
    }
}

/// Every five seconds at most, when there is something to say: how many
/// dispatches waited for their compute pipeline, how long in all, and how
/// many were dropped and why.
fn compute_line(now: Instant) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let Ok(mut last) = LAST.try_lock() else { return };
        match *last {
            Some(t) if now.saturating_duration_since(t) < Duration::from_secs(5) => return,
            _ => *last = Some(now),
        }
    }
    let [waits, us, made] = [0, 1, 2].map(|i| COMPUTE_WAITS[i].swap(0, Ordering::Relaxed));
    let lost = Lost::ALL.map(|l| COMPUTE_LOST[l as usize].swap(0, Ordering::Relaxed));
    if waits == 0 && lost.iter().all(|&n| n == 0) {
        return;
    }
    tracing::info!(
        waits,
        waited_ms = us / 1000,
        made_in_wait = made,
        dropped = %dropped_text(&lost),
        "compute dispatches whose pipeline was not made, in about five seconds (VICTORY_COMPUTE_WAIT_MS a frame; \
         NotYet and FrameSpent wait again in later frames, the others no more; VICTORY_COMPUTE_WAIT_ONCE=1: one wait a pipeline)"
    );
}

/// The dropped dispatches by reason, for [`compute_line`].
fn dropped_text(lost: &[u64; Lost::ALL.len()]) -> String {
    let parts: Vec<String> =
        Lost::ALL.iter().zip(lost).filter(|(_, n)| **n > 0).map(|(l, n)| format!("{n} {l:?}")).collect();
    if parts.is_empty() { "none".to_owned() } else { parts.join(", ") }
}

/// What compute dispatches may hold one frame, over all threads, from the
/// frame's first such wait: `VICTORY_COMPUTE_WAIT_MS` milliseconds, 500 if
/// unset, at most 1,000; 0: no wait of their own, a dispatch waits within
/// the frame's budget as a draw does ([`wait_budget`]), as before.
///
/// A dropped draw loses one frame's picture; a dropped dispatch the title
/// issues once loses its output for the session. With a cold driver cache
/// the head blend-shape map (61e54e90adccaf6a) was made 1 to 21 s after its
/// one dispatch, behind hundreds of graphics pipelines whose draws had
/// spent the frame's budget, and every head of the session had no skin
/// (avatarmanz PR #166, beba8da and ef32cab).
///
/// The wait is bounded three ways, so the title's watchdog (about 30 s
/// without a frame) cannot come near:
///
/// - a frame: all compute waits of a frame end together, this budget after
///   the first of them began ([`FrameWaits`]), however many threads wait
///   and however many pipelines they wait for;
/// - a dispatch: so no dispatch waits longer than that either;
/// - a pipeline: its dispatches wait again in later frames only while it
///   can still come ([`Facts::hopeless`]): not once its worker stalled
///   (`VICTORY_PIPELINE_STALL_S`), its making failed for good, a shader of
///   it does not translate, or no worker took it within the stall bound.
///   Waiting half a second in every frame for one that never comes would
///   be a picture at two frames a second for good. With
///   `VICTORY_COMPUTE_WAIT_ONCE=1`, one wait a pipeline ([`COMPUTE_CHANCE`])
///   as before: in runs on a loaded machine that dropped 68 to 153
///   dispatches at world load, each a one-off's output lost.
///
/// With the passes over the screen (10 s, once a pipeline) and the ordinary
/// half second, a frame is held 11 s at the very most. The wait without a
/// bound (fxceless PR #1312, `VICTORY_PIPELINE_COMPUTE_WAIT=1`, off) held
/// the title's thread until the watchdog ended the run (5752ad3); upstream's
/// first bound of 10 s a frame left gaps of 1 to 10 s between frames on a
/// cold start. Waiting does not make a compile faster: what a one-off
/// dispatch needs is its own pipeline made next ([`Queue::hurry`]) on a
/// worker kept free for it ([`Queue::reserved`]); the head map is made 3 ms
/// after a worker takes it.
fn compute_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = compute_millis(std::env::var("VICTORY_COMPUTE_WAIT_MS").ok().and_then(|v| v.trim().parse().ok()));
        if millis != 0 {
            tracing::info!(
                millis,
                once = compute_wait_once(),
                "compute dispatches wait for their pipelines, within this a frame (VICTORY_COMPUTE_WAIT_MS), in every frame \
                 while a pipeline is being made (VICTORY_COMPUTE_WAIT_ONCE=1: once a pipeline)"
            );
        }
        Duration::from_millis(millis)
    })
}

/// [`compute_budget`] in milliseconds for what the switch says.
fn compute_millis(asked: Option<u64>) -> u64 {
    asked.unwrap_or(500).min(1000)
}

/// The end of the frame's compute waits ([`compute_budget`]).
struct FrameWaits {
    frame: u64,
    until: Option<Instant>,
}

impl FrameWaits {
    const fn new() -> Self {
        FrameWaits { frame: u64::MAX, until: None }
    }

    /// A compute wait of frame `frame` begins at `now`: how long it may
    /// wait. Threads that wait together end together (upstream, a second
    /// thread that found the budget taken dropped its dispatch 18 ms before
    /// its pipeline was made), and a frame is held at most the budget
    /// however many wait. (A thread that read the frame before a newer one
    /// began counts in the newer one: it must not give that frame a second
    /// budget.)
    fn begin(&mut self, now: Instant, frame: u64, budget: Duration) -> Duration {
        if self.frame == u64::MAX || frame > self.frame {
            *self = FrameWaits { frame, until: Some(now + budget) };
        }
        self.until.map_or(Duration::ZERO, |until| until.saturating_duration_since(now))
    }
}

/// A wait at least this long without its pipeline made is a full one; with
/// `VICTORY_COMPUTE_WAIT_ONCE=1` the pipeline's only one: one that is never
/// made holds one frame once, not every frame that dispatches it. A
/// shorter one (the frame's budget nearly spent) leaves it another.
const COMPUTE_CHANCE: Duration = Duration::from_millis(100);

/// Whether a dispatch waits for its compute pipeline until it is made,
/// whatever the frame's budget ([`wait_budget`]); a draw still waits within
/// the budget and is dropped while its pipeline is not there (fxceless
/// a42b627 and 4c60466, PR #1312; DoubleGhosty 2a1965f is the same).
///
/// A dropped draw is drawn again in the next frame. A dropped dispatch may
/// not be: the title runs some compute passes once, when a scene changes
/// (a buffer reset or cleared), and what they leave is read for as long as
/// the scene lasts. Upstream, a compute program met for the first time took
/// 4.4 s to make; its one dispatch was dropped after the half second of
/// budget, and the room before stayed blended over the whole picture for
/// minutes. A wait of seconds once (the driver's cache has it on the next
/// run) against a picture wrong until the scene ends.
/// Off by default here: under Proton on NVIDIA the wait held the title's
/// thread until its render loop stopped at about 30 s of the world, and the
/// title's watchdog ended it with a write to 0 another 35 s later (runs
/// 20261127-005324 and -005721; with it off, -010017 played on).
/// `VICTORY_PIPELINE_COMPUTE_WAIT=1` waits as upstream does. What is on by
/// default is the bounded wait of [`compute_budget`].
fn compute_waits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_COMPUTE_WAIT").is_some_and(|v| v == "1"))
}

/// Makes `p` on this thread if it is not made yet (waiting for a worker
/// that is making it); whether it has a host pipeline.
pub(crate) fn now(p: &Arc<Pending>) -> bool {
    if p.left_out.is_some() || p.failed.load(Ordering::Relaxed) {
        return false;
    }
    p.make();
    p.made.get().is_some_and(|made| made.is_some())
}

/// At the title's creation of a pipeline: starts it at once if an earlier
/// run drew with it (during loading, so the world's first frames find it),
/// or if it is a confirmed opening-scene / bake producer.
pub(crate) fn early(p: &Arc<Pending>) {
    if p.left_out.is_some() {
        return;
    }
    if !p.shaders.is_empty() && (p.scene_input || super::library::wanted_early(&p.key)) {
        super::library::EARLY.fetch_add(1, Ordering::Relaxed);
        start(p, if p.scene_input { REQUESTED } else { EARLY });
    }
}

/// Whether the frame's budget is one for all threads together: a thread
/// that waits takes what is left of it and gives back what it did not
/// use. Counted only when a wait had ended, the budget was one per thread:
/// each of the title's five recording threads began its first wait of a
/// frame with nothing counted yet and waited the whole half second, 2.5 s
/// of waiting in every frame in which pipelines were being made (runs
/// 34-49: "2504 ms waiting for pipelines" in each such frame).
/// On by default; `VICTORY_PIPELINE_WAIT_SHARED=0` counts as before.
fn shared_budget() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_WAIT_SHARED").is_none_or(|v| v != "0"))
}

/// Whether one pipeline is waited for at most one budget over the whole
/// run, not one budget every frame (trustno1's fix for fxceless issue
/// 1181). A pipeline whose make takes longer than the budget (a driver
/// compile of tens of seconds, or one that never ends) otherwise cost the
/// whole budget in every frame in which a draw asked for it: the picture
/// stood still while the thing drawn with it was in view, and moved again
/// when the camera turned away. After its budget its draws are dropped
/// until it is made, as with any pipeline past the frame's budget. On by
/// default; `VICTORY_PIPELINE_WAIT_ONCE=0` waits as before.
fn wait_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_WAIT_ONCE").is_none_or(|v| v != "0"))
}

/// Waits for `p` while frame `frame` has budget left; how long, if it was
/// a compute dispatch's wait of its own ([`compute_budget`]).
fn wait(p: &Arc<Pending>, frame: u64) -> Option<Duration> {
    let screen = p.screen && screens_first();
    // A compute dispatch of a pipeline that can still come ([`Facts`])
    // waits within the frame's compute budget ([`compute_budget`]), again
    // in every frame until the pipeline is made; with that budget spent, or
    // once the pipeline cannot come, as any other. The pipeline it waits
    // for is made next.
    let allowed = if p.compute && !screen && Facts::of(p, Instant::now()).hopeless().is_none() {
        static WAITS: Mutex<FrameWaits> = Mutex::new(FrameWaits::new());
        WAITS.lock().unwrap_or_else(|e| e.into_inner()).begin(Instant::now(), frame, compute_budget())
    } else {
        Duration::ZERO
    };
    let compute = !allowed.is_zero();
    if compute {
        let _ = p.compute_asked.set(Instant::now());
        QUEUE.lock().unwrap_or_else(|e| e.into_inner()).hurry(p);
    }
    let (draw_budget, starting) = draw_wait_at(crate::host::now_ms(), start_window(), wait_budget());
    let budget = if screen {
        screen_budget()
    } else if compute {
        allowed
    } else {
        draw_budget
    };
    if budget.is_zero() {
        return None;
    }
    // What is left of this pipeline's own budget ([`wait_once`]); a compute
    // wait has its own bounds ([`Facts::hopeless`], the frame's).
    let own = if compute || starting {
        Duration::MAX
    } else if wait_once() {
        let own = budget.saturating_sub(Duration::from_micros(p.waited_us.load(Ordering::Relaxed)));
        if own.is_zero() {
            return None;
        }
        own
    } else {
        Duration::MAX
    };
    // The frame the budget belongs to, and what was waited of it (or is
    // taken by a thread waiting now); of the passes over the screen too.
    static SPENT: [Mutex<(u64, Duration)>; 2] = [const { Mutex::new((u64::MAX, Duration::ZERO)) }; 2];
    let spent_of = &SPENT[usize::from(screen)];
    let shared = shared_budget();
    let left = if compute {
        allowed
    } else {
        let mut spent = spent_of.lock().unwrap_or_else(|e| e.into_inner());
        if spent.0 != frame {
            *spent = (frame, Duration::ZERO);
        }
        let left = budget.saturating_sub(spent.1).min(own);
        if shared {
            spent.1 += left;
        }
        left
    };
    if left.is_zero() {
        return None;
    }
    let started = Instant::now();
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while p.made.get().is_none() && !p.failed.load(Ordering::Relaxed) {
        let rest = left.saturating_sub(started.elapsed());
        if rest.is_zero() {
            break;
        }
        held = done.wait_timeout(held, rest).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    let waited = started.elapsed().min(left);
    if p.made.get().is_none() && waited >= left {
        // Waited to the end for nothing: did its worker end?
        reap();
    }
    if compute {
        if p.made.get().is_none() && waited >= COMPUTE_CHANCE.min(compute_budget() / 2) {
            p.compute_waited.store(true, Ordering::Relaxed);
        }
    } else if wait_once() && !starting && p.made.get().is_none() {
        let before = p.waited_us.fetch_add(waited.as_micros() as u64, Ordering::Relaxed);
        if Duration::from_micros(before) + waited >= budget {
            tracing::info!(
                shaders = ?p.shaders,
                ms = budget.as_millis() as u64,
                "a pipeline not made within its wait: its draws are dropped until it is (VICTORY_PIPELINE_WAIT_ONCE=0: waited for every frame)"
            );
        }
    }
    if screen && waited > Duration::from_millis(500) {
        tracing::info!(
            ms = waited.as_millis() as u64,
            made = p.made.get().is_some(),
            shaders = ?p.shaders,
            "a pass over the screen was waited for (VICTORY_SCREEN_WAIT_MS=0: dropped past half a second a frame)"
        );
    }
    if !compute {
        let mut spent = spent_of.lock().unwrap_or_else(|e| e.into_inner());
        // A frame that began meanwhile has its own budget.
        if spent.0 == frame {
            if shared {
                spent.1 = spent.1.saturating_sub(left - waited);
            } else {
                spent.1 += waited;
            }
        }
    }
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(
        crate::copystat::C::PipelineWaitUs,
        started.elapsed().as_micros() as u64,
    );
    compute.then_some(waited)
}

/// The opening's wait for draws: until `VICTORY_PIPELINE_WAIT_START_S`
/// seconds into the run (`crate::host::now_ms`), a frame's draws wait for
/// their pipelines up to `VICTORY_PIPELINE_WAIT_START_MS` (1,000 if unset,
/// at most 5,000) together, and a pipeline is waited for in every frame
/// that draws with it ([`wait_once`] does not hold in that time). After
/// it, as before. Off (unset or 0): today's half second a frame, once a
/// pipeline.
///
/// A dropped draw is a piece missing from one frame; a wait is a frame
/// held. On a warm driver cache nothing is being made in the opening and
/// this waits for nothing (runs 20261127-210643, -200336: no pipeline
/// queued from 30 s on). On a cold one, some 1,700 pipelines were queued
/// as the cutscene began and the workers made about 15 a second (run
/// 20261127-210258): with this the cutscene's frames come complete, but
/// as slowly as the pipelines they need come, up to this many ms each,
/// for as long as they keep coming. Every wait is bounded: a frame is held
/// at most this budget (and the passes over the screen' own), far below
/// the title's 30 s watchdog.
fn start_window() -> Option<(u64, Duration)> {
    static WINDOW: OnceLock<Option<(u64, Duration)>> = OnceLock::new();
    *WINDOW.get_or_init(|| {
        let number = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok());
        let window = start_window_of(number("VICTORY_PIPELINE_WAIT_START_S"), number("VICTORY_PIPELINE_WAIT_START_MS"));
        if let Some((until_ms, budget)) = window {
            tracing::info!(
                until_s = until_ms / 1000,
                millis = budget.as_millis() as u64,
                "draws wait for their pipelines up to this a frame, in every frame, until this far into the run \
                 (VICTORY_PIPELINE_WAIT_START_S, _MS); then as VICTORY_PIPELINE_WAIT_MS says"
            );
        }
        window
    })
}

/// [`start_window`] for what the switches say: seconds, milliseconds.
fn start_window_of(seconds: Option<u64>, millis: Option<u64>) -> Option<(u64, Duration)> {
    let seconds = seconds.filter(|&s| s > 0)?;
    let millis = millis.unwrap_or(1000).min(5000);
    (millis > 0).then(|| (seconds.saturating_mul(1000), Duration::from_millis(millis)))
}

/// The budget a frame's draws wait within at `now_ms`, and whether that is
/// the opening's ([`start_window`]): `normal` otherwise.
fn draw_wait_at(now_ms: u64, window: Option<(u64, Duration)>, normal: Duration) -> (Duration, bool) {
    match window {
        Some((until_ms, budget)) if now_ms < until_ms => (budget, true),
        _ => (normal, false),
    }
}

const NOT_QUEUED: u8 = 0;
const EARLY: u8 = 1;
const REQUESTED: u8 = 2;
const MAKING: u8 = 3;

/// How many workers may make pipelines at once that are known to take long
/// and that no draw has asked for: a quarter of them. On a fresh cache (a
/// change of the translator or the driver) the early list begins with
/// programs that take tens of seconds each: all 16 workers were in them
/// through the whole of the title's loading, 97 pipelines made in 20 s, and
/// the first minute and a half of the world ran at 0.5 to 1.6 s a frame,
/// dropping up to 2,900 draws a frame (run shaders-1).
/// `VICTORY_PIPELINE_SLOW=0`: in the order the title creates them, as before.
fn slow_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if std::env::var_os("VICTORY_PIPELINE_SLOW").is_some_and(|v| v == "0") {
            usize::MAX
        } else {
            (worker_count(None, std::thread::available_parallelism().map_or(2, |n| n.get())) / 4).max(1)
        }
    })
}

struct Queue {
    /// Requested passes over the screen: before all others, and the only ones
    /// [`SCREEN_WORKERS`] take.
    screen: VecDeque<Arc<Pending>>,
    /// Compute pipelines a dispatch waits for now ([`Queue::hurry`]), and
    /// the other requested ones whose dispatches have a wait of their own
    /// ([`compute_budget`]): next, before the requested graphics, the newest
    /// first.
    hurried: VecDeque<Arc<Pending>>,
    compute: VecDeque<Arc<Pending>>,
    /// Started at creation ([`early`]), by when an earlier run first asked
    /// for each ([`super::library::first_use`]; unknown last), then in the
    /// order they came.
    early: std::collections::BTreeMap<(u32, u64), Arc<Pending>>,
    early_seq: u64,
    /// Those of the early ones known to take long, and how many of them are
    /// being made ([`slow_limit`]).
    slow: VecDeque<Arc<Pending>>,
    slow_now: usize,
    requested: VecDeque<Arc<Pending>>,
    /// What the workers other than the screen passes' have taken and not
    /// made yet, and how many of those workers there are
    /// ([`Queue::reserved`]; `None`: as many as were started, [`WORKERS`]).
    taken: Vec<Arc<Pending>>,
    workers: Option<usize>,
}

/// How many workers take any pipeline ([`workers`]; 0 until they start).
static WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Queue {
    const fn new() -> Self {
        Self {
            screen: VecDeque::new(),
            hurried: VecDeque::new(),
            compute: VecDeque::new(),
            early: std::collections::BTreeMap::new(),
            early_seq: 0,
            slow: VecDeque::new(),
            slow_now: 0,
            requested: VecDeque::new(),
            taken: Vec::new(),
            workers: None,
        }
    }

    /// `p`, requested and not taken by a worker yet, goes before every
    /// other compute pipeline: a dispatch waits for it now. Compute
    /// pipelines are made newest first anyway: first in, first out, a short
    /// wait would be spent on the hundreds dropped before it (a cold start's
    /// first world frame dispatches some 670), and a pipeline the title
    /// dispatches once would sit behind those it dispatches every frame.
    fn hurry(&mut self, p: &Arc<Pending>) {
        if p.queue_state.load(Ordering::Relaxed) == REQUESTED && !self.hurried.iter().any(|q| Arc::ptr_eq(q, p)) {
            self.hurried.push_front(p.clone());
            // (The worker kept for it may be asleep.)
            AVAILABLE.notify_all();
        }
    }

    /// Whether the last worker free is kept for pipelines a dispatch waits
    /// for now ([`Queue::hurry`]) and passes over the screen. Upstream the
    /// head map compiled in 1 to 3 ms, but on a cold start its dispatch
    /// waited 280, 289, 444 and 1,132 ms for a worker to come free: the last
    /// was past the budget, and the heads had no skin. Only with
    /// [`compute_budget`] and four workers or more. A worker in a compile
    /// that never returns counts as taken: the next one is kept.
    fn reserved(&mut self) -> bool {
        if compute_budget().is_zero() {
            return false;
        }
        let workers = self.workers.unwrap_or_else(|| WORKERS.load(Ordering::Relaxed));
        self.taken.retain(|p| p.made.get().is_none());
        workers >= 4 && self.taken.len() + 1 >= workers
    }

    /// Queues an unrequested pipeline or promotes one queued at creation.
    /// Returns whether it is new to the queue, or `None` if nothing changed.
    fn push(&mut self, p: &Arc<Pending>, priority: u8) -> Option<bool> {
        let slow = slow_limit() != usize::MAX && super::library::slow(&p.key);
        let order = if priority == EARLY { super::library::first_use(&p.key) } else { None };
        self.push_as(p, priority, slow, order.unwrap_or(u32::MAX))
    }

    /// As [`Queue::push`]; `order` places an early one ([`Queue::early`]).
    fn push_as(&mut self, p: &Arc<Pending>, priority: u8, slow: bool, order: u32) -> Option<bool> {
        let prev = p.queue_state.load(Ordering::Relaxed);
        if prev >= priority {
            return None;
        }
        p.queue_state.store(priority, Ordering::Relaxed);
        if priority == REQUESTED && p.screen && screens_first() {
            self.screen.push_back(p.clone());
        } else if priority == REQUESTED && p.compute {
            // Before the requested graphics: a dropped draw is drawn again
            // the next frame, a dispatch the title issues once is not
            // (upstream the head map was made 21 s after its dispatch,
            // behind a cold world's graphics). Not with the passes over the
            // screen: a cold world frame asks for hundreds of compute
            // pipelines, and those passes wait too.
            self.compute.push_front(p.clone());
        } else if priority == REQUESTED {
            self.requested.push_back(p.clone());
        } else if slow {
            self.slow.push_back(p.clone());
        } else {
            self.early.insert((order, self.early_seq), p.clone());
            self.early_seq += 1;
        }
        Some(prev == NOT_QUEUED)
    }

    fn pop(&mut self, screens_only: bool, slow_limit: usize) -> Option<Arc<Pending>> {
        // An early entry can have been promoted and claimed already.
        let unclaimed = |list: &mut VecDeque<Arc<Pending>>| {
            std::iter::from_fn(|| list.pop_front()).find(|p| p.queue_state.load(Ordering::Relaxed) == EARLY)
        };
        // A compute pipeline can be in both of its lines.
        let waiting = |list: &mut VecDeque<Arc<Pending>>| {
            std::iter::from_fn(|| list.pop_front()).find(|p| p.queue_state.load(Ordering::Relaxed) == REQUESTED)
        };
        let mut p = self.screen.pop_front();
        if !screens_only {
            p = p.or_else(|| waiting(&mut self.hurried));
            if p.is_none() && !self.reserved() {
                p = waiting(&mut self.compute)
                    .or_else(|| self.requested.pop_front())
                    .or_else(|| {
                        std::iter::from_fn(|| self.early.pop_first().map(|(_, p)| p))
                            .find(|p| p.queue_state.load(Ordering::Relaxed) == EARLY)
                    });
                if p.is_none() && self.slow_now < slow_limit {
                    p = unclaimed(&mut self.slow);
                    if let Some(p) = &p {
                        p.was_slow.store(true, Ordering::Relaxed);
                        self.slow_now += 1;
                    }
                }
            }
            if let Some(p) = &p
                && !compute_budget().is_zero()
            {
                self.taken.push(p.clone());
            }
        }
        let p = p?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let _ = p.claimed.set(Instant::now());
        Some(p)
    }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static AVAILABLE: Condvar = Condvar::new();

/// Start or promote work for a draw without waiting behind the entire early
/// list. Queue changes and worker claims happen under the same lock.
fn start(p: &Arc<Pending>, priority: u8) {
    if p.queue_state.load(Ordering::Relaxed) >= priority {
        return;
    }
    if !workers() {
        if p.queue_state.swap(MAKING, Ordering::Relaxed) == NOT_QUEUED {
            p.make();
        }
        return;
    }
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(new) = queue.push(p, priority) {
        if new {
            PEAK.fetch_max(WAITING.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        // (One woken could be a worker that takes passes over the screen
        // alone: it would leave the pipeline to nobody.)
        AVAILABLE.notify_all();
    }
}

/// Workers that make passes over the screen alone, besides the others.
const SCREEN_WORKERS: usize = 2;

fn next(screens_only: bool) -> Arc<Pending> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(p) = queue.pop(screens_only, slow_limit()) {
            return p;
        }
        queue = AVAILABLE.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// How many workers make pipelines: `asked` (`VICTORY_PIPELINE_WORKERS`, 1 to
/// 64), else two thirds of the logical processors, 2 to 16. They run below the
/// title's own threads, and a cold cache queues hundreds at once.
fn worker_count(asked: Option<usize>, processors: usize) -> usize {
    worker_count_for(asked, processors, false)
}

fn worker_count_for(asked: Option<usize>, processors: usize, limited_ram: bool) -> usize {
    match asked {
        Some(n) => n.clamp(1, 64),
        None if limited_ram => (processors * 3 / 4).clamp(2, 8),
        None => (processors * 2 / 3).clamp(2, 16),
    }
}

/// The stack a worker is given, in bytes: `asked` MiB
/// (`VICTORY_PIPELINE_STACK_MB`, at most 1,024), else 64 MiB; `None` for 0,
/// the default stack as before (2 MiB). Reserved, not committed. The host
/// driver compiles on the worker's thread, and under Wine vkd3d-proton's
/// conversion of DXIL to SPIR-V recurses over the program's control flow:
/// past the default stack the thread ends without a word and its pipeline
/// is never made (avatarmanz 14709c5; Belobug PR #421 gives 64 MiB too,
/// Legendary a1058fa 8 MiB).
fn worker_stack(asked: Option<usize>) -> Option<usize> {
    match asked {
        Some(0) => None,
        Some(mib) => Some(mib.min(1024) << 20),
        None => Some(64 << 20),
    }
}

/// Starts the workers ([`worker_count`]) on first use.
fn workers() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        let mut running = false;
        let workers = worker_count_for(
            std::env::var("VICTORY_PIPELINE_WORKERS")
                .ok()
                .and_then(|v| v.trim().parse().ok()),
            std::thread::available_parallelism().map_or(2, |n| n.get()),
            crate::residency::limited_host_ram(),
        );
        tracing::info!(workers, stack_mib = stack_now().map_or(0, |s| s >> 20), "pipeline workers");
        let screens = if screens_first() { SCREEN_WORKERS } else { 0 };
        WORKERS.store(workers, Ordering::Relaxed);
        for i in 0..workers + screens {
            running |= spawn_worker(i);
        }
        running
    })
}

/// [`worker_stack`] of this run.
fn stack_now() -> Option<usize> {
    static STACK: OnceLock<Option<usize>> = OnceLock::new();
    *STACK.get_or_init(|| worker_stack(std::env::var("VICTORY_PIPELINE_STACK_MB").ok().and_then(|v| v.trim().parse().ok())))
}

/// Starts worker `i` (its slot in [`NOW_MAKING`]; from [`WORKERS`] on, one
/// that makes passes over the screen alone); whether it started.
fn spawn_worker(i: usize) -> bool {
    {
        {
            let screens_only = i >= WORKERS.load(Ordering::Relaxed);
            let mut builder = std::thread::Builder::new().name(format!("victory-pipelines-{i}"));
            if let Some(stack) = stack_now() {
                builder = builder.stack_size(stack);
            }
            let spawned = builder
                .spawn(move || {
                    LARGE_STACK.with(|l| l.set(stack_now().is_some()));
                    // Below the title's own threads (loading, recording),
                    // and the streaming one, which is below normal itself
                    // (`VICTORY_PIPELINE_LOWEST=0`: level with it, as before).
                    let lowest = !screens_only && std::env::var_os("VICTORY_PIPELINE_LOWEST").is_none_or(|v| v != "0");
                    // SAFETY: a plain call on our own thread.
                    unsafe {
                        use windows::Win32::System::Threading::*;
                        let priority = if lowest { THREAD_PRIORITY_LOWEST } else { THREAD_PRIORITY_BELOW_NORMAL };
                        let _ = SetThreadPriority(GetCurrentThread(), priority);
                    }
                    let slot = &NOW_MAKING[i.min(NOW_MAKING.len() - 1)];
                    // SAFETY: a plain query of this thread.
                    let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
                    loop {
                        let p = next(screens_only);
                        let began = Instant::now();
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(Making { pipeline: p.clone(), thread, since: began });
                        p.make();
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        note_made(&p, began);
                        if p.was_slow.load(Ordering::Relaxed) {
                            let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
                            queue.slow_now = queue.slow_now.saturating_sub(1);
                            AVAILABLE.notify_all();
                        }
                        // (Saturating: [`reap`] counts down too, for a
                        // worker that ended, and a count must not wrap.)
                        let before = WAITING
                            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |w| Some(w.saturating_sub(1)))
                            .unwrap_or(0);
                        if before == 1 {
                            drained();
                        } else {
                            super::library::save(false);
                        }
                    }
                });
            match spawned {
                Err(e) => {
                    tracing::warn!(error = %e, "no pipeline worker");
                    false
                }
                Ok(_) => true,
            }
        }
    }
}

/// What a worker is making, on which thread, since when.
struct Making {
    pipeline: Arc<Pending>,
    thread: u32,
    since: Instant,
}

/// What each worker is making ([`reap`]).
static NOW_MAKING: [Mutex<Option<Making>>; 80] = [const { Mutex::new(None) }; 80];

/// Whether the thread with id `tid` has ended (or is no more).
fn ended(tid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeThread, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: plain queries of a thread of this process, its handle closed.
    unsafe {
        let thread = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid);
        if thread.is_null() {
            return true;
        }
        let mut code = 0u32;
        let ok = GetExitCodeThread(thread, &mut code) != 0;
        CloseHandle(thread);
        ok && code != STILL_ACTIVE
    }
}

/// [`reap`], at most once a second over all threads: asked at every use of
/// a pipeline that is not made, where nobody may wait to the end of a
/// budget any more ([`wait_once`]).
fn reap_now_and_then() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let Ok(mut last) = LAST.try_lock() else { return };
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        *last = Some(Instant::now());
    }
    reap();
}

/// Pipelines whose worker ended while making them (a stack overflow in the
/// host's compiler ends a thread under Wine without an exception anyone can
/// catch): made once more on a thread with a larger stack ([`rescue`]), and
/// the worker started again. A pipeline whose second making ends its thread
/// too is failed: draws and dispatches go without it at once and nobody
/// waits for it. Before this the worker was gone for the run with no word
/// in the log (sixteen such pipelines and no worker is left), and its
/// pipeline stayed "being made" for good: upstream `CS_Prop_BatchNoCull`
/// under Proton, its dispatch dropped right before the title's write to 0
/// (DoubleGhosty 5e7faab, its issue #39).
fn reap() {
    for (i, slot) in NOW_MAKING.iter().enumerate() {
        let dead = {
            let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_ref() {
                Some(m) if m.since.elapsed() > Duration::from_secs(2) && ended(m.thread) => slot.take(),
                _ => None,
            }
        };
        let Some(m) = dead else { continue };
        let p = &m.pipeline;
        {
            let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if p.was_slow.load(Ordering::Relaxed) {
                queue.slow_now = queue.slow_now.saturating_sub(1);
            }
            // (Its worker is started again below: not one taken.)
            queue.taken.retain(|q| !Arc::ptr_eq(q, p));
        }
        let _ = WAITING.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |w| Some(w.saturating_sub(1)));
        let again = !p.rescued.swap(true, Ordering::AcqRel) && rescue(p.clone());
        tracing::warn!(
            pipeline = %hashes(p),
            key = %p.key,
            thread = m.thread,
            after_ms = m.since.elapsed().as_millis() as u64,
            again,
            "a pipeline worker ended while making a pipeline (a stack overflow in the host's compiler?): \
             made again on a thread with a larger stack if `again` (VICTORY_PIPELINE_RESCUE_MB, 0: not), \
             and the worker is started again"
        );
        if !again {
            give_up(p);
        }
        spawn_worker(i);
    }
    let gone: Vec<Making> = {
        let mut rescuing = RESCUING.lock().unwrap_or_else(|e| e.into_inner());
        let (gone, alive) =
            std::mem::take(&mut *rescuing).into_iter().partition(|m| m.pipeline.made.get().is_some() || ended(m.thread));
        *rescuing = alive;
        gone
    };
    for m in gone {
        if m.pipeline.made.get().is_none() {
            tracing::warn!(
                pipeline = %hashes(&m.pipeline),
                key = %m.pipeline.key,
                after_ms = m.since.elapsed().as_millis() as u64,
                "making a pipeline ended its thread again, on the larger stack: \
                 its draws and dispatches go without it for this run"
            );
            give_up(&m.pipeline);
        }
    }
}

/// The stack of the thread that makes a pipeline again after its worker
/// ended ([`rescue`]), in bytes: `asked` MiB (`VICTORY_PIPELINE_RESCUE_MB`,
/// at most 4,096), else 1 GiB; `None` for 0, not made again. Reserved only,
/// its pages committed as they are used. Upstream gives 4 GiB; a program
/// whose conversion never ends its recursion (two loop headers, Vice-Tay PR
/// #198) commits all of it before its thread ends, and 1 GiB is sixteen
/// times what a worker has.
fn rescue_stack(asked: Option<usize>) -> Option<usize> {
    match asked {
        Some(0) => None,
        Some(mib) => Some(mib.min(4096) << 20),
        None => Some(1 << 30),
    }
}

/// Pipelines being made again on a thread of their own ([`rescue`]), for [`reap`].
static RESCUING: Mutex<Vec<Making>> = Mutex::new(Vec::new());

/// Makes `p` again on a thread with [`rescue_stack`]; whether that thread
/// started.
fn rescue(p: Arc<Pending>) -> bool {
    static STACK: OnceLock<Option<usize>> = OnceLock::new();
    let stack = *STACK
        .get_or_init(|| rescue_stack(std::env::var("VICTORY_PIPELINE_RESCUE_MB").ok().and_then(|v| v.trim().parse().ok())));
    stack.is_some_and(|stack| rescue_with(p, stack, |p| p.build()))
}

fn rescue_with(p: Arc<Pending>, stack: usize, build: impl FnOnce(&Pending) -> Option<HostPipeline> + Send + 'static) -> bool {
    let (tell, told) = std::sync::mpsc::channel();
    let pipeline = p.clone();
    let started = std::thread::Builder::new().name("victory-pipeline-rescue".to_owned()).stack_size(stack).spawn(move || {
        // SAFETY: a plain query of this thread.
        let _ = tell.send(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() });
        LARGE_STACK.with(|l| l.set(true));
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&pipeline))).unwrap_or(None);
        let _ = pipeline.made.set(made);
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    });
    if started.is_err() {
        return false;
    }
    let Ok(thread) = told.recv_timeout(Duration::from_secs(5)) else { return false };
    RESCUING.lock().unwrap_or_else(|e| e.into_inner()).push(Making { pipeline: p, thread, since: Instant::now() });
    true
}

/// `p` is not made: its draws and dispatches go without it, and no one
/// waits for it.
fn give_up(p: &Pending) {
    p.failed.store(true, Ordering::Release);
    let (lock, done) = &DONE;
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    done.notify_all();
}

/// The most pipelines waiting at once since the workers last ran out.
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The workers ran out of work: logged after a batch of 10 or more, and
/// the cache saved (at most every 20 s).
fn drained() {
    use super::library::{EARLY, LOADED, MADE};
    if PEAK.swap(0, Ordering::Relaxed) >= 10 {
        tracing::info!(
            made = MADE.load(Ordering::Relaxed),
            loaded = LOADED.load(Ordering::Relaxed),
            started_early = EARLY.load(Ordering::Relaxed),
            "host pipelines all made"
        );
        // The burst's last word, whenever the line last came.
        account_line();
    }
    super::library::save(false);
}

/// Pipelines made after draws or dispatches of them were dropped, for the
/// account's "not drawn with since" (at most [`AFTER_MAX`]).
static AFTER: Mutex<Vec<Arc<Pending>>> = Mutex::new(Vec::new());
const AFTER_MAX: usize = 4096;

/// A worker made `p`, begun at `began`: its time, and its lateness if a
/// draw or dispatch had asked for it before.
fn note_made(p: &Arc<Pending>, began: Instant) {
    let done = Instant::now();
    super::account::WHOLE.add(done - began);
    if let Some(&asked) = p.asked.get()
        && asked < done
    {
        super::account::LATE.add(done - asked);
    }
    if p.dropped.load(Ordering::Relaxed) > 0 && matches!(p.made.get(), Some(Some(_))) {
        let mut after = AFTER.lock().unwrap_or_else(|e| e.into_inner());
        if after.len() < AFTER_MAX {
            after.push(p.clone());
        }
    }
    account_now_and_then();
}

/// [`account_line`] if its time has come (cheap otherwise: one load).
fn account_now_and_then() {
    if super::account::due(crate::host::now_ms()) {
        account_line();
    }
}

/// The account of the pipelines made since the last such line (at most
/// every five seconds, when something was made or dropped), and since the
/// start: how long they took and where, the queue, the workers, and what the
/// picture lost. Always on; also at the end of a burst ([`drained`]).
fn account_line() {
    use super::account::{self as a, text};
    let whole = a::WHOLE.take();
    let (loaded, missed, created, translating, late) =
        (a::LOADED.take(), a::LOAD_MISSED.take(), a::CREATED.take(), a::TRANSLATING.take(), a::LATE.take());
    let drops_all = a::LATE_DROPS.load(Ordering::Relaxed);
    let drops = drops_all.saturating_sub(a::LATE_DROPS_SAID.swap(drops_all, Ordering::Relaxed));
    if whole[0] == 0 && loaded[0] == 0 && created[0] == 0 && drops == 0 {
        return;
    }
    // (Not waited for: the queue is the title's to fill and the workers'.)
    let queue = QUEUE.try_lock().ok().map(|q| {
        format!(
            "screen {}, compute waited for {}, compute {}, asked for {}, early {}, early known to be slow {} ({} of them on workers)",
            q.screen.len(),
            q.hurried.len(),
            q.compute.len(),
            q.requested.len(),
            q.early.len(),
            q.slow.len(),
            q.slow_now
        )
    });
    let busy = NOW_MAKING.iter().filter(|s| s.try_lock().map_or(true, |s| s.is_some())).count();
    let (after, lost) = {
        let mut after = AFTER.lock().unwrap_or_else(|e| e.into_inner());
        after.retain(|p| !p.drawn_after.load(Ordering::Relaxed));
        let names: Vec<String> = after.iter().take(8).map(|p| format!("{} ({} dropped)", hashes(p), p.dropped.load(Ordering::Relaxed))).collect();
        (after.len(), names)
    };
    tracing::info!(
        "host pipelines in about {} s: {} made by the workers, {}; library loads that gave one {}; loads that found none {}; \
         made anew {}; waiting for the shaders' translations {}. Queued now {} ({}), workers making {busy} of {}. \
         Asks of draws and dispatches that found their pipeline still being made (dropped; a rectangle draw asks twice): \
         {drops} ({drops_all} since the start). Made after a draw or dispatch had asked: {}",
        a::EVERY_MS / 1000,
        whole[0],
        text(whole),
        text(loaded),
        text(missed),
        text(created),
        text(translating),
        WAITING.load(Ordering::Relaxed),
        queue.as_deref().unwrap_or("busy"),
        WORKERS.load(Ordering::Relaxed) + if screens_first() { SCREEN_WORKERS } else { 0 },
        text(late),
    );
    tracing::info!(
        "host pipelines since the start: made by the workers {}; library loads that gave one {}; made anew {}; \
         made after a draw or dispatch had asked {}; made after some of their draws were dropped and not drawn with \
         since (a once-only pass lost, or only out of view): {after}{}",
        text(a::WHOLE.total()),
        text(a::LOADED.total()),
        text(a::CREATED.total()),
        text(a::LATE.total()),
        if lost.is_empty() { String::new() } else { format!(", first {}", lost.join(", ")) },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_count_follows_the_processors_and_the_switch() {
        assert_eq!(worker_count(None, 1), 2);
        assert_eq!(worker_count(None, 24), 16);
        assert_eq!(worker_count(None, 64), 16);
        assert_eq!(worker_count(Some(0), 24), 1);
        assert_eq!(worker_count(Some(20), 24), 20);
        assert_eq!(worker_count_for(None, 8, true), 6);
        assert_eq!(worker_count_for(None, 16, true), 8);
        assert_eq!(worker_count_for(None, 24, true), 8);
        assert_eq!(worker_count_for(Some(12), 8, true), 12);
    }

    #[test]
    fn workers_get_a_stack_of_64_mib_or_what_the_switch_says() {
        assert_eq!(worker_stack(None), Some(64 << 20));
        assert_eq!(worker_stack(Some(256)), Some(256 << 20));
        assert_eq!(worker_stack(Some(1 << 20)), Some(1 << 30));
        assert_eq!(worker_stack(Some(0)), None);
    }

    /// A thread with a worker's stack takes recursion the default 2 MiB
    /// does not: 16 MiB of frames here.
    #[test]
    fn a_workers_stack_takes_deep_recursion() {
        #[inline(never)]
        fn deep(n: usize) -> usize {
            let frame = std::hint::black_box([n as u8; 4096]);
            if n == 0 { 0 } else { std::hint::black_box(deep(n - 1)) + usize::from(frame[0] == frame[4095]) }
        }
        let made = std::thread::Builder::new()
            .stack_size(worker_stack(None).unwrap())
            .spawn(|| deep(4096))
            .unwrap()
            .join();
        assert_eq!(made.ok(), Some(4096));
    }

    /// What a thread without a stack for it asks is run on a thread with
    /// one, once, and its answer (or what it threw) comes back; a thread
    /// that has such a stack runs it itself.
    #[test]
    fn what_goes_deep_is_run_on_a_large_stack() {
        #[inline(never)]
        fn deep(n: usize) -> usize {
            let frame = std::hint::black_box([n as u8; 4096]);
            if n == 0 { 0 } else { std::hint::black_box(deep(n - 1)) + usize::from(frame[0] == frame[4095]) }
        }
        if !large_stacks() || stack_now().is_none() {
            // (Switched off: run where asked, and no depth to try.)
            let here = std::thread::current().id();
            assert_eq!(on_a_large_stack(|| std::thread::current().id()), here);
            return;
        }
        let here = std::thread::current().id();
        let borrowed = vec![1u8, 2, 3];
        // 8 MiB of frames from a thread with 1 MiB, as the title's are.
        let asked = std::thread::Builder::new().stack_size(1 << 20).spawn(move || {
            let asker = std::thread::current().id();
            let (ran_on, depth, sum) =
                on_a_large_stack(|| (std::thread::current().id(), deep(2048), borrowed.iter().map(|&b| usize::from(b)).sum::<usize>()));
            // Inside such a thread, nothing is started again.
            let inner = on_a_large_stack(|| on_a_large_stack(|| LARGE_STACK.with(|l| l.get())));
            (asker, ran_on, depth, sum, inner)
        });
        let (asker, ran_on, depth, sum, inner) = asked.unwrap().join().unwrap();
        assert_eq!((depth, sum), (2048, 6));
        assert_ne!(asker, here);
        assert_ne!(ran_on, asker);
        assert!(inner);
        let thrown = std::panic::catch_unwind(|| on_a_large_stack(|| -> u32 { panic!("thrown on the large stack") }));
        assert!(thrown.is_err());
    }

    #[test]
    fn the_rescue_stack_is_a_gibibyte_or_what_the_switch_says() {
        assert_eq!(rescue_stack(None), Some(1 << 30));
        assert_eq!(rescue_stack(Some(0)), None);
        assert_eq!(rescue_stack(Some(256)), Some(256 << 20));
        assert_eq!(rescue_stack(Some(1 << 20)), Some(4 << 30));
    }

    /// A thread that ends while making a pipeline without unwinding
    /// (ExitThread: what a stack overflow in vkd3d-proton does under Wine)
    /// leaves it "being made". A second caller waits instead of making it
    /// too or waiting for ever; [`rescue_with`] makes it and wakes it, and a
    /// rescue that ends its thread too has [`reap`] give it up.
    #[test]
    fn a_pipeline_whose_maker_ended_is_made_again_or_given_up() {
        fn ends_its_thread(p: Arc<Pending>) {
            std::thread::spawn(move || {
                p.make_with(|| {
                    // SAFETY: ends this test thread, which holds nothing but `p`'s claim.
                    unsafe { windows_sys::Win32::System::Threading::ExitThread(0) }
                })
            });
        }
        let waits_for = |p: &Arc<Pending>| {
            let (tell, told) = std::sync::mpsc::channel();
            let q = p.clone();
            std::thread::spawn(move || {
                q.make_with(|| panic!("the second caller must not make it too"));
                let _ = tell.send(q.made.get().is_some());
            });
            told
        };
        for rescue_ends_too in [false, true] {
            let mut p = Pending::new(Vec::new(), None);
            p.key = format!("rescue-test-{rescue_ends_too}");
            let p = Arc::new(p);
            ends_its_thread(p.clone());
            let started = Instant::now();
            while !p.building.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(10) {
                std::thread::yield_now();
            }
            let told = waits_for(&p);
            assert!(told.recv_timeout(Duration::from_millis(300)).is_err(), "the second caller waits");
            assert!(rescue_with(p.clone(), 1 << 30, move |_| {
                if rescue_ends_too {
                    // SAFETY: as above, the rescue thread.
                    unsafe { windows_sys::Win32::System::Threading::ExitThread(0) }
                }
                None
            }));
            if rescue_ends_too {
                let started = Instant::now();
                while !p.failed.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(10) {
                    reap();
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(p.failed.load(Ordering::Acquire), "given up once the rescue ended too");
                assert_eq!(told.recv_timeout(Duration::from_secs(10)), Ok(false));
                // Nothing waits for it any more, and nothing makes it.
                let started = Instant::now();
                assert!(ready(&p).is_none() && !now(&p));
                assert!(started.elapsed() < Duration::from_millis(100));
            } else {
                assert_eq!(told.recv_timeout(Duration::from_secs(10)), Ok(true), "made by the rescue");
                assert!(!p.failed.load(Ordering::Acquire));
            }
        }
    }

    /// A worker whose thread ended in a making is found, its pipeline made
    /// again, and the count of what is waiting put right.
    #[test]
    fn a_worker_that_ended_is_found_by_its_thread() {
        // A thread that has ended, and this one, which has not.
        let (tell, told) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // SAFETY: a plain query of this thread.
            let _ = tell.send(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() });
        })
        .join()
        .unwrap();
        let gone = told.recv().unwrap();
        // SAFETY: a plain query of this thread.
        let here = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        assert!(!ended(here));
        let started = Instant::now();
        while !ended(gone) && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(ended(gone));
        // That thread as a worker that was making a pipeline for 3 s: the
        // pipeline is made again (here: no host pipeline for no shaders),
        // or given up with the rescue switched off.
        let Some(since) = Instant::now().checked_sub(Duration::from_secs(3)) else { return };
        let p = Arc::new(Pending::new(Vec::new(), None));
        p.building.store(true, Ordering::Release);
        *NOW_MAKING[NOW_MAKING.len() - 1].lock().unwrap() = Some(Making { pipeline: p.clone(), thread: gone, since });
        reap();
        assert!(NOW_MAKING[NOW_MAKING.len() - 1].lock().unwrap().is_none());
        assert!(p.rescued.load(Ordering::Acquire));
        let started = Instant::now();
        while p.made.get().is_none() && !p.failed.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(matches!(p.made.get(), Some(None)) || p.failed.load(Ordering::Acquire));
        // A second ending of the same pipeline's worker is not rescued again.
        let again = Arc::new(Pending::new(Vec::new(), None));
        again.rescued.store(true, Ordering::Release);
        *NOW_MAKING[NOW_MAKING.len() - 1].lock().unwrap() = Some(Making { pipeline: again.clone(), thread: gone, since });
        reap();
        assert!(again.failed.load(Ordering::Acquire));
    }

    #[test]
    fn a_panicking_build_ends_the_making() {
        let bad = Arc::new(Pending::new(Vec::new(), None));
        bad.make_with(|| panic!("synthetic pipeline build panic"));
        assert!(matches!(bad.made.get(), Some(None)));
        assert!(!now(&bad));
    }

    #[test]
    fn the_compute_wait_is_half_a_second_and_never_more_than_one() {
        assert_eq!(compute_millis(None), 500);
        assert_eq!(compute_millis(Some(0)), 0);
        assert_eq!(compute_millis(Some(250)), 250);
        assert_eq!(compute_millis(Some(30_000)), 1000);
    }

    /// Compute pipelines are made newest first, before requested graphics;
    /// one a dispatch waits for now goes first, once, and not to the screen
    /// passes' workers.
    #[test]
    fn a_pipeline_a_dispatch_waits_for_is_made_next() {
        let of = || {
            let mut p = Pending::new(Vec::new(), None);
            p.compute = true;
            Arc::new(p)
        };
        let (first, second, waited, later) = (of(), of(), of(), of());
        let world = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();
        queue.workers = Some(64);
        for p in [&world, &first, &waited, &second, &later] {
            assert_eq!(queue.push(p, REQUESTED), Some(true));
        }
        // A later dispatch of one already queued does not move it.
        assert_eq!(queue.push(&first, REQUESTED), None);
        queue.hurry(&waited);
        queue.hurry(&waited);
        assert!(queue.pop(true, 1).is_none());
        for p in [&waited, &later, &second, &first, &world] {
            assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), p));
        }
        queue.hurry(&first);
        assert!(queue.pop(false, 1).is_none());
    }

    /// With all workers but one busy, the last takes only a pipeline a
    /// dispatch waits for now (or a pass over the screen); a worker that
    /// finishes gives its place back.
    #[test]
    fn the_last_worker_is_kept_for_a_waited_for_pipeline() {
        if compute_budget().is_zero() {
            return;
        }
        let of = |compute| {
            let mut p = Pending::new(Vec::new(), None);
            p.compute = compute;
            Arc::new(p)
        };
        let mut queue = Queue::new();
        queue.workers = Some(4);
        let (a, b, c, d, waited) = (of(true), of(true), of(true), of(false), of(true));
        for p in [&a, &b, &c, &d] {
            queue.push(p, REQUESTED);
        }
        for p in [&c, &b, &a] {
            assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), p));
        }
        assert!(queue.pop(false, 1).is_none());
        queue.push(&waited, REQUESTED);
        assert!(queue.pop(false, 1).is_none());
        queue.hurry(&waited);
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &waited));
        // One of the first three is made: its worker takes ordinary work again.
        let _ = c.made.set(None);
        let _ = waited.made.set(None);
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &d));
    }

    /// A frame's compute waits end together, the budget after its first
    /// began; a newer frame has its own; a late thread of an older frame
    /// does not open a new one.
    #[test]
    fn compute_waits_end_with_the_frames_budget() {
        let ms = Duration::from_millis;
        let budget = ms(500);
        let t0 = Instant::now();
        let mut w = FrameWaits::new();
        assert_eq!(w.begin(t0, 7, budget), budget);
        assert_eq!(w.begin(t0 + ms(100), 7, budget), budget - ms(100));
        assert_eq!(w.begin(t0 + budget, 7, budget), Duration::ZERO);
        assert_eq!(w.begin(t0 + budget, 8, budget), budget);
        assert_eq!(w.begin(t0 + budget + ms(50), 7, budget), budget - ms(50));
    }

    /// A compute pipeline still being made is waited for again in later
    /// frames, each time within the frame's compute budget; a second such
    /// pipeline in the same frame has nothing left of that budget; once the
    /// pipeline cannot come (here: its making failed for good) no dispatch
    /// waits for it, and that is said once.
    #[test]
    fn a_compute_pipeline_being_made_is_waited_for_in_every_frame() {
        let budget = compute_budget();
        if budget.is_zero() || compute_wait_once() {
            return;
        }
        let stuck = || {
            let mut p = Pending::new(Vec::new(), None);
            p.compute = true;
            p.queue_state.store(MAKING, Ordering::Relaxed);
            Arc::new(p)
        };
        let (p, other) = (stuck(), stuck());
        let late = Duration::from_millis(500);
        // (A frame of its own, whatever the tests before waited in theirs.)
        crate::frame::test_advance();
        let started = Instant::now();
        assert!(ready(&p).is_none());
        let first = started.elapsed();
        assert!(first >= COMPUTE_CHANCE && first < budget + late, "{first:?}");
        assert!(p.compute_waited.load(Ordering::Relaxed));
        assert_eq!(p.compute_said.load(Ordering::Relaxed), 0, "still being made: nothing said");
        assert_eq!(p.waited_us.load(Ordering::Relaxed), 0, "not counted against its ordinary wait");
        // The frame's compute budget is spent: the other waits as a draw
        // does, and has its compute waits in later frames.
        let started = Instant::now();
        assert!(ready(&other).is_none());
        assert!(started.elapsed() <= wait_budget() + late);
        assert!(other.compute_asked.get().is_none());
        // Its next dispatch, in a later frame, waits again, as long.
        crate::frame::test_advance();
        let started = Instant::now();
        assert!(ready(&p).is_none());
        let second = started.elapsed();
        assert!(second >= COMPUTE_CHANCE && second < budget + late, "{second:?}");
        // Its making ended for good: no wait, and it is said.
        give_up(&p);
        crate::frame::test_advance();
        let started = Instant::now();
        assert!(ready(&p).is_none());
        assert!(started.elapsed() < Duration::from_millis(50), "{:?}", started.elapsed());
        assert_eq!(p.compute_said.load(Ordering::Relaxed), 1 << Lost::Failed as u8);
        assert!(ready(&p).is_none());
        assert_eq!(p.compute_said.load(Ordering::Relaxed), 1 << Lost::Failed as u8);
    }

    /// What ends a compute pipeline's waits: what keeps it from coming, or,
    /// with `VICTORY_COMPUTE_WAIT_ONCE=1`, the one wait it had. A pipeline
    /// still being made that had full waits is waited for again.
    #[test]
    fn what_ends_a_compute_pipelines_waits() {
        let made = Facts::default();
        assert_eq!(made.hopeless(), None);
        assert_eq!(Facts { had_its_wait: true, ..made }.hopeless(), None, "waits again");
        assert_eq!(Facts { once: true, ..made }.hopeless(), None, "its first wait");
        assert_eq!(Facts { once: true, had_its_wait: true, ..made }.hopeless(), Some(Lost::HadItsWait));
        assert_eq!(Facts { stalled: true, ..made }.hopeless(), Some(Lost::Stalled));
        assert_eq!(Facts { failed: true, ..made }.hopeless(), Some(Lost::Failed));
        assert_eq!(Facts { untranslated: true, ..made }.hopeless(), Some(Lost::Untranslated));
        assert_eq!(Facts { unclaimed: true, ..made }.hopeless(), Some(Lost::NoWorker));
        let all = Facts { failed: true, stalled: true, untranslated: true, unclaimed: true, had_its_wait: true, once: true };
        assert_eq!(all.hopeless(), Some(Lost::Failed));
        for lost in Lost::ALL {
            assert_eq!(lost.ends(), !matches!(lost, Lost::NotYet | Lost::FrameSpent), "{lost:?}");
            assert_eq!(Lost::ALL[lost as usize], lost);
        }
        let mut counts = [0; Lost::ALL.len()];
        assert_eq!(dropped_text(&counts), "none");
        counts[Lost::NotYet as usize] = 12;
        counts[Lost::Stalled as usize] = 1;
        assert_eq!(dropped_text(&counts), "12 NotYet, 1 Stalled");
    }

    /// The facts of a pipeline: a worker held past the stall bound, no
    /// worker taking it within the bound of its first wait, a shader that
    /// does not translate.
    #[test]
    fn a_compute_pipelines_facts() {
        let Some(bound) = stall_bound() else { return };
        let now = Instant::now();
        let Some(long_ago) = now.checked_sub(bound * 2) else { return };
        let p = Pending::new(Vec::new(), None);
        assert_eq!(Facts::of(&p, now).hopeless(), None);
        p.compute_asked.set(long_ago).unwrap();
        assert!(Facts::of(&p, now).unclaimed);
        assert!(!Facts::of(&p, long_ago).unclaimed, "not yet past the bound");
        p.claimed.set(long_ago).unwrap();
        let f = Facts::of(&p, now);
        assert!(!f.unclaimed && f.stalled);
        let _ = p.made.set(None);
        assert!(!Facts::of(&p, now).stalled, "its worker came back");

        let untranslatable = Arc::new(Shader {
            stage: "compute",
            hash: "0000000000000000".to_owned(),
            container: Vec::new(),
            ambient: None,
            translated: OnceLock::from(None),
        });
        let p = Pending::new(vec![untranslatable], None);
        assert_eq!(Facts::of(&p, now).hopeless(), Some(Lost::Untranslated));
    }

    /// A worker that never returns from the host's compile (seen under
    /// Wine) is no longer waited for once it has held the pipeline past the
    /// bound; it is not given up on, and what it makes is used.
    #[test]
    fn draws_stop_waiting_for_a_worker_that_does_not_return() {
        let bound = Duration::from_secs(15);
        let p = Arc::new(Pending::new(Vec::new(), None));
        let later = Instant::now() + bound + Duration::from_secs(1);
        let mut queue = Queue::new();
        assert_eq!(queue.push(&p, REQUESTED), Some(true));
        assert!(!p.stalled(later, Some(bound)), "queued, not claimed");
        let p = queue.pop(false, 1).unwrap();
        assert!(!p.stalled(Instant::now(), Some(bound)), "a compile under the limit");
        assert!(!p.stalled(later, None), "the switch at 0: no bound");
        assert!(!p.stall_noted.load(Ordering::Relaxed));
        assert!(p.stalled(later, Some(bound)));
        assert!(p.stall_noted.load(Ordering::Relaxed));
        assert!(p.stalled(later, Some(bound)), "said once, still not waited for");
        assert_eq!(p.queue_state.load(Ordering::Relaxed), MAKING);

        // ready() returns without spending the frame's budget on it (timed
        // only with a budget long enough to tell, and the bound on).
        let stuck = Arc::new(Pending::new(Vec::new(), None));
        stuck.queue_state.store(MAKING, Ordering::Relaxed);
        if let (Some(bound), Some(long_ago)) = (stall_bound(), stall_bound().and_then(|b| Instant::now().checked_sub(b * 2))) {
            stuck.claimed.set(long_ago).unwrap();
            assert!(stuck.stalled(Instant::now(), Some(bound)));
            let started = Instant::now();
            assert!(ready(&stuck).is_none());
            let budget = wait_budget();
            if budget >= Duration::from_millis(100) {
                assert!(started.elapsed() < budget / 2, "{:?}", started.elapsed());
            }
            assert_eq!(stuck.waited_us.load(Ordering::Relaxed), 0);
        }

        // The worker returns at last: its answer is the pipeline's, and
        // nothing calls it stalled any more.
        p.make();
        assert!(matches!(p.made.get(), Some(None)));
        assert!(!p.stalled(later, Some(bound)));
    }

    /// A first use queues the pipeline and, within the budget, gets the
    /// worker's answer (here: no host pipeline for no shaders).
    #[test]
    fn first_use_is_made_by_a_worker() {
        let p = Arc::new(Pending::new(Vec::new(), None));
        assert!(ready(&p).is_none());
        assert_ne!(p.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
        let started = Instant::now();
        while p.made.get().is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(matches!(p.made.get(), Some(None)));
    }

    #[test]
    fn pending_pipeline_is_recorded_for_the_next_launch() {
        let mut p = Pending::new(Vec::new(), None);
        p.key = "pending-pipeline-test".to_owned();
        // Simulate a worker that has taken the job but cannot finish it yet.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let p = Arc::new(p);

        assert!(!super::super::library::wanted_early(&p.key));
        // A compute pipeline is made at once when dispatches wait for theirs
        // (fxceless PR #1312, the default; it has no shaders here, so nothing
        // comes of it); either way it is noted for the next launch.
        assert!(ready(&p).is_none());
        assert!(super::super::library::wanted_early(&p.key));
    }

    #[test]
    fn draws_promote_early_pipelines_without_claiming_them_twice() {
        let earlier = Arc::new(Pending::new(Vec::new(), None));
        let promoted = Arc::new(Pending::new(Vec::new(), None));
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();

        assert_eq!(queue.push(&earlier, EARLY), Some(true));
        assert_eq!(queue.push(&promoted, EARLY), Some(true));
        assert_eq!(queue.push(&requested, REQUESTED), Some(true));
        assert_eq!(queue.push(&promoted, REQUESTED), Some(false));
        assert_eq!(queue.push(&promoted, REQUESTED), None);

        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &requested));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &promoted));
        assert_eq!(queue.push(&promoted, REQUESTED), None);
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &earlier));
        assert!(queue.pop(false, 1).is_none());
    }

    #[test]
    fn pipelines_known_to_take_long_leave_workers_for_the_others() {
        let new = || Arc::new(Pending::new(Vec::new(), None));
        let (long, longer, asked, quick) = (new(), new(), new(), new());
        let mut queue = Queue::new();
        queue.workers = Some(64);
        for p in [&long, &longer, &asked] {
            assert_eq!(queue.push_as(p, EARLY, true, u32::MAX), Some(true));
        }
        assert_eq!(queue.push_as(&quick, EARLY, false, u32::MAX), Some(true));
        // The quick one first, then one long one at a time; one a draw asks
        // for waits for nobody.
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &quick));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &long));
        assert!(queue.pop(false, 1).is_none());
        assert_eq!(queue.push_as(&asked, REQUESTED, true, u32::MAX), Some(false));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &asked));
        assert!(!asked.was_slow.load(Ordering::Relaxed));
        assert!(queue.pop(false, 1).is_none());
        queue.slow_now -= 1;
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &longer));
        assert!(queue.pop(false, 1).is_none());
    }

    /// Early ones go by when an earlier run first asked for them, those no
    /// run asked for last, and in the order they came among equals; one a
    /// draw asks for meanwhile is taken from the asked-for line, once.
    #[test]
    fn early_pipelines_go_in_the_order_earlier_runs_asked_for_them() {
        let new = || Arc::new(Pending::new(Vec::new(), None));
        let (never, late, first, also_first, unknown, asked) = (new(), new(), new(), new(), new(), new());
        let mut queue = Queue::new();
        queue.workers = Some(64);
        for (p, order) in [(&never, u32::MAX), (&late, 90), (&first, 3), (&asked, 40), (&also_first, 3), (&unknown, u32::MAX)] {
            assert_eq!(queue.push_as(p, EARLY, false, order), Some(true));
        }
        assert_eq!(queue.push_as(&asked, REQUESTED, false, u32::MAX), Some(false));
        for p in [&asked, &first, &also_first, &late, &never, &unknown] {
            assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), p));
        }
        assert!(queue.pop(false, 1).is_none(), "the asked-for one is not made twice");
    }

    #[test]
    fn the_openings_wait_holds_until_its_time_and_is_bounded() {
        assert_eq!(start_window_of(None, Some(2000)), None);
        assert_eq!(start_window_of(Some(0), None), None);
        assert_eq!(start_window_of(Some(150), None), Some((150_000, Duration::from_millis(1000))));
        assert_eq!(start_window_of(Some(150), Some(60_000)), Some((150_000, Duration::from_millis(5000))));
        assert_eq!(start_window_of(Some(150), Some(0)), None);
        let normal = Duration::from_millis(500);
        let window = start_window_of(Some(150), Some(2000));
        assert_eq!(draw_wait_at(149_999, window, normal), (Duration::from_millis(2000), true));
        assert_eq!(draw_wait_at(150_000, window, normal), (normal, false));
        assert_eq!(draw_wait_at(0, None, normal), (normal, false));
    }

    /// Only a pipeline queued or on a worker is said to be being made.
    #[cfg(feature = "diagnostics")]
    #[test]
    fn a_pipeline_left_out_or_given_up_is_not_said_to_be_being_made() {
        let mut left = Pending::new(Vec::new(), None);
        left.left_out = Some("shades ray-tracing hits without BVH, left out");
        assert_eq!(missing(&left).as_deref(), Some("left out: shades ray-tracing hits without BVH, left out"));
        let gone = Pending::new(Vec::new(), None);
        gone.failed.store(true, Ordering::Relaxed);
        assert!(missing(&gone).is_some_and(|w| w.starts_with("given up")));
        let making = Pending::new(Vec::new(), None);
        assert_eq!(missing(&making).as_deref(), Some("being made"));
        let _ = making.made.set(None);
        assert!(missing(&making).is_some_and(|w| w.starts_with("refused")));
    }

    /// Passes over the screen go before all others, and workers of their own
    /// take nothing else.
    #[test]
    fn passes_over_the_screen_go_first() {
        let of = |screen| {
            let mut p = Pending::new(Vec::new(), None);
            p.screen = screen;
            Arc::new(p)
        };
        let (world, screen) = (of(false), of(true));
        let mut queue = Queue::new();
        assert_eq!(queue.push(&world, REQUESTED), Some(true));
        assert_eq!(queue.push(&screen, REQUESTED), Some(true));
        assert!(Arc::ptr_eq(&queue.pop(true, 1).unwrap(), &screen));
        assert!(queue.pop(true, 1).is_none());
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &world));
    }
}
