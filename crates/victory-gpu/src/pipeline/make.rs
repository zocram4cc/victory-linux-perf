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
//!
//! If asked for (`VICTORY_COMPUTE_WAIT_MS`), the first use of a compute
//! pass waits for its host pipeline by a budget of its own
//! ([`compute_budget`]), not within the frame's budget: the table a
//! character's face is deformed by is filled once on the card when the
//! character is made; dropped, it is not run again, and without it the
//! face's vertices all lie in one point (docs/linux-wine.md, "People
//! without a head"). Upstream's switch; here off by default (our
//! [`compute_waits`] mechanism is the default), and it takes the pass out
//! of the frame's budget and of that mechanism's bookkeeping.

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
    /// What identifies it across runs (shader containers and state), and
    /// whether a draw or dispatch has requested it yet.
    key: String,
    used: AtomicBool,
    /// A pass over the screen ([`GraphicsState::screen_pass`]).
    screen: bool,
    /// A compute pipeline whose dispatches wait for it ([`compute_waits`]),
    /// whether a dropped dispatch of it was logged, and whether one that
    /// gave up waiting for it was.
    compute: bool,
    compute_dropped: AtomicBool,
    compute_given_up: AtomicBool,
    /// When a worker began making it.
    making_since: OnceLock<Instant>,
    /// A first use that went without it was written down (diagnostics).
    #[cfg(feature = "diagnostics")]
    noted: AtomicBool,
    /// The worker that was making it ended in the middle ([`watch`]): it
    /// will never be made, and nothing waits for it.
    lost: AtomicBool,
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
        Pending {
            screen: graphics.as_ref().is_some_and(|g| g.screen_pass()),
            compute: graphics.is_none() && shaders.first().is_some_and(|s| s.stage == "compute") && compute_waits(),
            compute_dropped: AtomicBool::new(false),
            compute_given_up: AtomicBool::new(false),
            making_since: OnceLock::new(),
            #[cfg(feature = "diagnostics")]
            noted: AtomicBool::new(false),
            shaders,
            graphics,
            queue_state: AtomicU8::new(NOT_QUEUED),
            was_slow: AtomicBool::new(false),
            made: OnceLock::new(),
            key: key.finalize().to_hex()[..32].to_owned(),
            used: AtomicBool::new(false),
            lost: AtomicBool::new(false),
        }
    }

    pub(super) fn key(&self) -> &str {
        &self.key
    }

    /// A compute pass where those wait apart from the frame's budget
    /// (`VICTORY_COMPUTE_WAIT_MS` asked for one; a pass of one shader, of
    /// that stage).
    fn waits_alone(&self) -> bool {
        matches!(self.shaders.as_slice(), [cs] if cs.stage == "compute") && !compute_budget().is_zero()
    }

    fn make(&self) {
        self.make_with(|| {
            let _building = crate::timeline::span("pipeline: build a host pipeline");
            let began = Instant::now();
            let made = HostPipeline::new(&self.shaders, self.graphics.as_deref());
            super::library::note_time(&self.key, began.elapsed().as_millis() as u64);
            made
        });
    }

    /// Makes it with `build`, outside the `OnceLock` (no other caller waits
    /// inside it on the driver's compiler, nor forever if the thread making
    /// it ends), and a panic in `build` refuses the pipeline instead of
    /// ending the worker (Filip Michalski, fxceless/victory-fork#1353, as
    /// adapted in rorodev6/victory-megafork-amd af875f26).
    fn make_with(&self, build: impl FnOnce() -> Option<HostPipeline>) {
        self.making_since.get_or_init(Instant::now);
        if self.made.get().is_none() {
            let made = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
                Ok(pipeline) => pipeline,
                Err(_) => {
                    tracing::warn!(key = self.key, shaders = ?self.shaders, "host pipeline build panicked: the title goes without it");
                    None
                }
            };
            // A second maker that finished first keeps its pipeline; ours is let go.
            let _ = self.made.set(made);
        }
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    }
}

/// Why a pipeline has no host pipeline (diagnostics): its shaders that do
/// not translate, else "refused" or "being made"; `None` if it has one.
#[cfg(feature = "diagnostics")]
pub(crate) fn missing(p: &Pending) -> Option<String> {
    match p.made.get() {
        Some(Some(_)) => None,
        None if p.lost.load(Ordering::Relaxed) => Some(format!("lost, its making ended the worker {:?}", p.shaders)),
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

/// Pipelines queued and not made yet.
static WAITING: AtomicU64 = AtomicU64::new(0);

/// Pipelines queued and not made yet (for the frame line).
pub(crate) fn waiting() -> u64 {
    WAITING.load(Ordering::Relaxed)
}

/// The host pipeline if it is made (or is made within the frame's budget);
/// queues it on first use.
pub(crate) fn ready(p: &Arc<Pending>) -> Option<&HostPipeline> {
    // A still-pending pipeline was requested by the title too. Prewarm it
    // on the next launch, even if this run never got to use it.
    if !p.used.load(Ordering::Relaxed) && !p.used.swap(true, Ordering::Relaxed) {
        super::library::note_used(&p.key);
    }
    if p.made.get().is_none() {
        if p.lost.load(Ordering::Relaxed) {
            return None;
        }
        start(p, REQUESTED);
        wait(p, crate::frame::current());
        // (Which first uses go without: once a pipeline.)
        #[cfg(feature = "diagnostics")]
        if p.made.get().is_none() && !p.lost.load(Ordering::Relaxed) && !p.noted.swap(true, Ordering::Relaxed) {
            tracing::info!(
                shaders = ?p.shaders,
                frame = crate::frame::current(),
                "a first use goes without its host pipeline, which is still being made: dropped"
            );
        }
    }
    p.made.get()?.as_ref()
}

/// Makes `p` on this thread if it is not made yet (waiting for a worker
/// that is making it); whether it has a host pipeline.
pub(crate) fn now(p: &Arc<Pending>) -> bool {
    if p.lost.load(Ordering::Relaxed) {
        return false;
    }
    p.make();
    p.made.get().is_some_and(|made| made.is_some())
}

/// At the title's creation of a pipeline: starts it at once if an earlier
/// run drew with it (during loading, so the world's first frames find it).
pub(crate) fn early(p: &Arc<Pending>) {
    if !p.shaders.is_empty() && super::library::wanted_early(&p.key) {
        super::library::EARLY.fetch_add(1, Ordering::Relaxed);
        start(p, EARLY);
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

/// Under Wine, a dispatch of a compute pipeline not made yet waits for it
/// outside the frame's budget, within [`COMPUTE_BUDGET`]
/// (`VICTORY_COMPUTE_WAIT`: unset, on under Wine only; `0` never; `1` on
/// any host). A dropped draw loses one frame's picture; a dropped dispatch
/// the title issues once loses its output for the session. With a cold
/// driver shader cache the head blend-shape map (61e54e90adccaf6a) was
/// made 1 to 21 s after its one dispatch, behind hundreds of graphics
/// pipelines, and every head of the session had no skin (runs r2 and r6,
/// vkd3d-proton 3.2.0 and 71cae09 alike; a warm cache had skin). A timing
/// race, not a Wine one (the fork's issue #75 saw heads without skin on
/// Windows), but on Windows it is untried, so it is off there by default.
fn compute_waits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = crate::wine::on_under_wine("VICTORY_COMPUTE_WAIT");
        if on {
            tracing::info!(secs = COMPUTE_BUDGET.as_secs(), "compute dispatches wait for their pipelines");
        }
        on
    })
}

/// What compute dispatches ([`compute_waits`]) may hold one frame, from
/// its first such wait, and wait for one pipeline over all frames. Bounded
/// per frame: with no bound the title's first world frame waited for 670
/// compute pipelines one after another on a cold driver cache, 33 s, and
/// the title's own watchdog (about 30 s) ended the run (run r5). Bounded
/// per pipeline: a dispatch waits only while its pipeline is queued or a
/// worker has been making it for less than this, so one that is never made
/// (six were, with vkd3d-proton 3.2.0) holds one frame, not every frame
/// that dispatches it (as the fork's #73 asks of every wait). With the
/// screen passes' 10 s and the ordinary half second, a frame stays below
/// the watchdog.
const COMPUTE_BUDGET: Duration = Duration::from_secs(10);

/// What a compute dispatch may wait now: what is left of the frame's
/// [`COMPUTE_BUDGET`] (`since` the frame's first compute wait began) and
/// of the pipeline's (`making` since a worker began it).
fn compute_left(since: Duration, making: Duration) -> Duration {
    COMPUTE_BUDGET.saturating_sub(since).min(COMPUTE_BUDGET.saturating_sub(making))
}

/// What the first use of a compute pass may wait for its pipeline, apart
/// from the frame's budget and whatever else waits in the frame
/// (`VICTORY_COMPUTE_WAIT_MS`, read here; set, on under Wine or Windows, to
/// the milliseconds; unset: not used, [`compute_waits`]; `0`: as before
/// under Wine too).
///
/// Upstream (PR #440, `d9ee5f2f`) defaults this to 10 s under Wine and
/// replaces the frame's budget for compute passes with it; here it is an
/// opt-in beside [`compute_waits`], whose per-pipeline cap of 10 s of
/// making and its per-frame bound it joins: a pass that is not made within
/// the budget is dropped by the caller, as the frame's budget drops.
fn compute_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| match std::env::var("VICTORY_COMPUTE_WAIT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(millis) if millis != 0 => {
            tracing::info!(millis, "compute passes wait for their host pipeline by a budget of their own (VICTORY_COMPUTE_WAIT_MS)");
            Duration::from_millis(millis)
        }
        _ => Duration::ZERO,
    })
}

/// Waits for a compute pass's pipeline ([`compute_budget`]) until it is
/// made or `budget` after the frame's first such wait, whichever thread
/// that was: all of a frame's waits end together, and none takes another's
/// time (a thread that came while another waited found nothing left of a
/// budget counted as [`wait`] counts it, and dropped its pass at once).
fn wait_compute(p: &Pending, frame: u64, budget: Duration) {
    // The frame the waiting belongs to and when it ends.
    static UNTIL: Mutex<(u64, Option<Instant>)> = Mutex::new((u64::MAX, None));
    let until = {
        let mut until = UNTIL.lock().unwrap_or_else(|e| e.into_inner());
        if until.0 != frame {
            *until = (frame, None);
        }
        *until.1.get_or_insert_with(|| Instant::now() + budget)
    };
    let waiting = crate::timeline::span("pipeline: first use waits for a host pipeline");
    let started = Instant::now();
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while p.made.get().is_none() && !p.lost.load(Ordering::Relaxed) {
        let rest = until.saturating_duration_since(Instant::now());
        if rest.is_zero() {
            break;
        }
        held = done.wait_timeout(held, rest).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    let waited = started.elapsed();
    waiting.end();
    if waited > Duration::from_millis(100) {
        tracing::info!(
            ms = waited.as_millis() as u64,
            made = p.made.get().is_some(),
            shaders = ?p.shaders,
            frame,
            "a compute pass was waited for (VICTORY_COMPUTE_WAIT_MS: dropped past its own budget)"
        );
    }
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(crate::copystat::C::PipelineWaitUs, waited.as_micros() as u64);
}

/// Waits for `p` while frame `frame` has budget left.
fn wait(p: &Pending, frame: u64) {
    if p.waits_alone() {
        return wait_compute(p, frame, compute_budget());
    }
    let screen = p.screen && screens_first();
    // A compute dispatch ([`compute_waits`]) waits within its own budget
    // while its pipeline is being made for less than that; then as any other.
    let making = p.making_since.get().map_or(Duration::ZERO, Instant::elapsed);
    let compute = p.compute && making < COMPUTE_BUDGET;
    let budget = if screen {
        screen_budget()
    } else if compute {
        COMPUTE_BUDGET
    } else {
        wait_budget()
    };
    if budget.is_zero() {
        return;
    }
    // The frame the budget belongs to, and what was waited of it (or is
    // taken by a thread waiting now); of the passes over the screen too.
    static SPENT: [Mutex<(u64, Duration)>; 2] = [const { Mutex::new((u64::MAX, Duration::ZERO)) }; 2];
    let spent_of = &SPENT[usize::from(screen)];
    let shared = shared_budget();
    let left = if compute {
        // Compute waits end at one time a frame, the budget after the
        // frame's first one began: threads that wait together all wait (a
        // second thread that found the budget taken dropped its dispatch,
        // 91b10a73e48b2192, 18 ms before its pipeline was made), and the
        // frame is held at most the budget however many wait.
        static FIRST: Mutex<(u64, Option<Instant>)> = Mutex::new((u64::MAX, None));
        let mut first = FIRST.lock().unwrap_or_else(|e| e.into_inner());
        // (A thread that read the frame before a newer one began counts in
        // the newer one: it must not give that frame a second budget.)
        if first.0 == u64::MAX || frame > first.0 {
            *first = (frame, Some(Instant::now()));
        }
        compute_left(first.1.map_or(Duration::ZERO, |at| at.elapsed()), making)
    } else {
        let mut spent = spent_of.lock().unwrap_or_else(|e| e.into_inner());
        if spent.0 != frame {
            *spent = (frame, Duration::ZERO);
        }
        let left = budget.saturating_sub(spent.1);
        if shared {
            spent.1 += left;
        }
        left
    };
    if left.is_zero() {
        if compute && !p.compute_dropped.swap(true, Ordering::Relaxed) {
            tracing::info!(
                shaders = %hashes(p),
                frame,
                "a compute dispatch was dropped: its pipeline is not made and this frame's compute waits took 10 s"
            );
        }
        return;
    }
    let waiting = crate::timeline::span("pipeline: first use waits for a host pipeline");
    let started = Instant::now();
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while p.made.get().is_none() && !p.lost.load(Ordering::Relaxed) {
        let rest = left.saturating_sub(started.elapsed());
        if rest.is_zero() {
            break;
        }
        held = done.wait_timeout(held, rest).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    let waited = started.elapsed().min(left);
    waiting.end();
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
    // A dispatch that waited to the end of the frame's budget is dropped by
    // the caller: said once a pipeline, as one found with nothing left is.
    if compute && p.made.get().is_none() && !p.compute_dropped.swap(true, Ordering::Relaxed) {
        tracing::info!(
            shaders = %hashes(p),
            frame,
            waited_ms = waited.as_millis() as u64,
            "a compute dispatch was dropped: its pipeline was not made within this frame's compute waits"
        );
    }
    if compute
        && p.made.get().is_none()
        && p.making_since.get().is_some_and(|at| at.elapsed() >= COMPUTE_BUDGET)
        && !p.compute_given_up.swap(true, Ordering::Relaxed)
    {
        tracing::warn!(
            shaders = %hashes(p),
            "a compute pipeline was not made in 10 s: its dispatches wait within the frame's budget from now on"
        );
    }
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(
        crate::copystat::C::PipelineWaitUs,
        started.elapsed().as_micros() as u64,
    );
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
    /// Requested compute pipelines whose dispatches wait
    /// ([`compute_waits`]): next, before the requested graphics.
    compute: VecDeque<Arc<Pending>>,
    early: VecDeque<Arc<Pending>>,
    /// Those of the early ones known to take long, and how many of them are
    /// being made ([`slow_limit`]).
    slow: VecDeque<Arc<Pending>>,
    slow_now: usize,
    requested: VecDeque<Arc<Pending>>,
}

impl Queue {
    const fn new() -> Self {
        Self {
            screen: VecDeque::new(),
            compute: VecDeque::new(),
            early: VecDeque::new(),
            slow: VecDeque::new(),
            slow_now: 0,
            requested: VecDeque::new(),
        }
    }

    /// Queues an unrequested pipeline or promotes one queued at creation.
    /// Returns whether it is new to the queue, or `None` if nothing changed.
    fn push(&mut self, p: &Arc<Pending>, priority: u8) -> Option<bool> {
        self.push_as(p, priority, slow_limit() != usize::MAX && super::library::slow(&p.key))
    }

    fn push_as(&mut self, p: &Arc<Pending>, priority: u8, slow: bool) -> Option<bool> {
        let prev = p.queue_state.load(Ordering::Relaxed);
        if prev >= priority {
            return None;
        }
        p.queue_state.store(priority, Ordering::Relaxed);
        if priority == REQUESTED && p.screen && screens_first() {
            self.screen.push_back(p.clone());
        } else if priority == REQUESTED && p.compute {
            // A compute dispatch that waits ([`compute_waits`]) goes before
            // the requested graphics: behind them the head blend-shape map
            // was made 21 s after its dispatch (run r2), past any bound. Not
            // with the passes over the screen: a cold world frame asks for
            // hundreds of compute pipelines, and those passes wait too.
            self.compute.push_back(p.clone());
        } else if priority == REQUESTED {
            self.requested.push_back(p.clone());
        } else if slow {
            self.slow.push_back(p.clone());
        } else {
            self.early.push_back(p.clone());
        }
        Some(prev == NOT_QUEUED)
    }

    fn pop(&mut self, screens_only: bool, slow_limit: usize) -> Option<Arc<Pending>> {
        // An early entry can have been promoted and claimed already.
        let unclaimed = |list: &mut VecDeque<Arc<Pending>>| {
            std::iter::from_fn(|| list.pop_front()).find(|p| p.queue_state.load(Ordering::Relaxed) == EARLY)
        };
        let mut p = self.screen.pop_front();
        if p.is_none() && !screens_only {
            p = self.compute.pop_front().or_else(|| self.requested.pop_front()).or_else(|| unclaimed(&mut self.early));
            if p.is_none() && self.slow_now < slow_limit {
                p = unclaimed(&mut self.slow);
                if let Some(p) = &p {
                    p.was_slow.store(true, Ordering::Relaxed);
                    self.slow_now += 1;
                }
            }
        }
        let p = p?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
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
    match asked {
        Some(n) => n.clamp(1, 64),
        None => (processors * 2 / 3).clamp(2, 16),
    }
}

/// The stack of a worker: `VICTORY_PIPELINE_STACK_MB` (1 to 1024), else
/// `victory_shader::emit::PIPELINE_STACK`. Reserved; used as far as it is
/// needed.
///
/// Upstream's default is the standard library's 2 MiB; here every worker has
/// had 256 MiB since 64373d81. Its runs found the six programs of the first
/// scene that end their worker under Wine do so with 64 MiB and with 1 GiB as
/// with 2 MiB (runs 16 and 18, `docs/linux-wine.md`), so the size is not the
/// cure, and [`watch`] is for what is left.
fn worker_stack() -> usize {
    match std::env::var("VICTORY_PIPELINE_STACK_MB").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        Some(megabytes) => {
            tracing::info!(megabytes, "pipeline workers' stacks (VICTORY_PIPELINE_STACK_MB)");
            megabytes.clamp(1, 1024) << 20
        }
        None => victory_shader::emit::PIPELINE_STACK,
    }
}

/// What each worker is making now, by its number.
static NOW: Mutex<Vec<Option<Arc<Pending>>>> = Mutex::new(Vec::new());

/// Worker `i`: takes pipelines from the queue and makes them, for as long as
/// the process runs.
fn spawn_worker(i: usize, screens_only: bool, stack: usize) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(format!("victory-pipelines-{i}")).stack_size(stack).spawn(move || {
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
        let making = |p: Option<Arc<Pending>>| {
            let mut now = NOW.lock().unwrap_or_else(|e| e.into_inner());
            if now.len() <= i {
                now.resize(i + 1, None);
            }
            now[i] = p;
        };
        loop {
            let p = next(screens_only);
            making(Some(p.clone()));
            p.make();
            making(None);
            if p.was_slow.load(Ordering::Relaxed) {
                let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
                queue.slow_now -= 1;
                AVAILABLE.notify_all();
            }
            if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
                drained();
            } else {
                super::library::save(false);
            }
        }
    })
}

/// Under Wine: watches the workers, and when one ends, which only the host
/// can have done, gives up the pipeline it was making and starts another
/// worker in its place (`VICTORY_PIPELINE_WATCH=0`: not watched, as before).
///
/// Wine ends a thread whose stack has run out with no word (unless its `err`
/// channel is on: "virtual_setup_exception stack overflow"), where Windows
/// ends the process. The host's `CreatePipelineState` is vkd3d-proton's
/// there, which turns the shader into the host's form on the calling thread,
/// and for six compute programs of the first scene that runs out of any
/// stack. The pipeline then stayed "being made" for good: every frame that
/// used it waited its whole budget for it ([`wait`]), half a second, 250 of
/// 271 ms a frame (run 13), and the six workers were gone. Given up, its
/// draws and dispatches are dropped at once, as those of a pipeline the host
/// refuses are.
///
/// This covers the pipeline workers alone. A thread of the title's or ours
/// that runs out of stack is not watched.
fn watch(mut workers: Vec<(usize, bool, std::thread::JoinHandle<()>)>, stack: usize) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{INFINITE, WaitForMultipleObjects};
    let watching = std::thread::Builder::new().name("victory-pipelines-watch".into()).spawn(move || {
        while !workers.is_empty() {
            let handles: Vec<HANDLE> = workers.iter().map(|w| HANDLE(w.2.as_raw_handle())).collect();
            // SAFETY: the handles of threads we hold, which stay open while we do.
            let ended = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
            let k = ended.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            if k >= workers.len() {
                tracing::warn!(?ended, "the pipeline workers are watched no more");
                return;
            }
            let (i, screens_only, _gone) = workers.swap_remove(k);
            let was = NOW.lock().unwrap_or_else(|e| e.into_inner()).get_mut(i).and_then(Option::take);
            if let Some(p) = was {
                p.lost.store(true, Ordering::Relaxed);
                if p.was_slow.load(Ordering::Relaxed) {
                    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
                    queue.slow_now -= 1;
                    AVAILABLE.notify_all();
                }
                WAITING.fetch_sub(1, Ordering::Relaxed);
                tracing::warn!(
                    worker = i,
                    shaders = ?p.shaders,
                    "a host pipeline's making ended its worker (under Wine: the host ran out of stack?): the title goes without this pipeline"
                );
                let (lock, done) = &DONE;
                let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
                done.notify_all();
            } else {
                tracing::warn!(worker = i, "a pipeline worker ended with no pipeline in the making");
            }
            match spawn_worker(i, screens_only, stack) {
                Ok(worker) => workers.push((i, screens_only, worker)),
                Err(e) => tracing::warn!(error = %e, worker = i, "no pipeline worker in its place"),
            }
        }
    });
    if let Err(e) = watching {
        tracing::warn!(error = %e, "the pipeline workers are not watched");
    }
}

/// Starts the workers ([`worker_count`]) on first use.
fn workers() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        let mut running = false;
        let workers = worker_count(
            std::env::var("VICTORY_PIPELINE_WORKERS")
                .ok()
                .and_then(|v| v.trim().parse().ok()),
            std::thread::available_parallelism().map_or(2, |n| n.get()),
        );
        tracing::info!(workers, "pipeline workers");
        let screens = if screens_first() { SCREEN_WORKERS } else { 0 };
        let stack = worker_stack();
        let mut started = Vec::new();
        for i in 0..workers + screens {
            let screens_only = i >= workers;
            match spawn_worker(i, screens_only, stack) {
                Ok(worker) => {
                    started.push((i, screens_only, worker));
                    running = true;
                }
                Err(e) => tracing::warn!(error = %e, "no pipeline worker"),
            }
        }
        if crate::wine::on_wine() && std::env::var_os("VICTORY_PIPELINE_WATCH").is_none_or(|v| v != "0") {
            watch(started, stack);
        }
        running
    })
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
    }
    super::library::save(false);
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
        assert!(ready(&p).is_none());
        assert!(p.made.get().is_none());
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
        for p in [&long, &longer, &asked] {
            assert_eq!(queue.push_as(p, EARLY, true), Some(true));
        }
        assert_eq!(queue.push_as(&quick, EARLY, false), Some(true));
        // The quick one first, then one long one at a time; one a draw asks
        // for waits for nobody.
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &quick));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &long));
        assert!(queue.pop(false, 1).is_none());
        assert_eq!(queue.push_as(&asked, REQUESTED, true), Some(false));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &asked));
        assert!(!asked.was_slow.load(Ordering::Relaxed));
        assert!(queue.pop(false, 1).is_none());
        queue.slow_now -= 1;
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &longer));
        assert!(queue.pop(false, 1).is_none());
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

    /// A compute dispatch that waits goes before requested graphics, after
    /// passes over the screen, and is not taken by their own workers.
    #[test]
    fn waiting_compute_dispatches_go_first() {
        let of = |compute, screen| {
            let mut p = Pending::new(Vec::new(), None);
            p.compute = compute;
            p.screen = screen;
            Arc::new(p)
        };
        let (world, compute, screen) = (of(false, false), of(true, false), of(false, true));
        let mut queue = Queue::new();
        assert_eq!(queue.push(&world, REQUESTED), Some(true));
        assert_eq!(queue.push(&compute, REQUESTED), Some(true));
        assert_eq!(queue.push(&screen, REQUESTED), Some(true));
        assert!(queue.pop(true, 1).is_some_and(|p| Arc::ptr_eq(&p, &screen)));
        assert!(queue.pop(true, 1).is_none());
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &compute));
        assert!(Arc::ptr_eq(&queue.pop(false, 1).unwrap(), &world));
    }

    /// Compute waits are bounded by the frame's and the pipeline's budget.
    #[test]
    fn compute_waits_are_bounded_per_frame_and_per_pipeline() {
        let s = Duration::from_secs;
        assert_eq!(compute_left(s(0), s(0)), COMPUTE_BUDGET);
        assert_eq!(compute_left(COMPUTE_BUDGET, s(0)), Duration::ZERO);
        assert_eq!(compute_left(s(0), COMPUTE_BUDGET), Duration::ZERO);
        assert_eq!(compute_left(s(3), s(8)), COMPUTE_BUDGET - s(8));
        assert_eq!(compute_left(s(8), s(3)), COMPUTE_BUDGET - s(8));
    }

    /// A dispatch waits for a compute pipeline that is not made until a
    /// worker has been making it for the budget, and then no longer than for
    /// the others.
    #[test]
    fn a_compute_pipeline_not_made_stops_waiting_long() {
        let mut p = Pending::new(Vec::new(), None);
        p.compute = true;
        // A worker has had it for nearly the budget and does not finish.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let began = Instant::now() - (COMPUTE_BUDGET - Duration::from_millis(100));
        p.making_since.set(began).unwrap();
        let p = Arc::new(p);
        let started = Instant::now();
        assert!(ready(&p).is_none());
        let first = started.elapsed();
        assert!(first >= Duration::from_millis(90) && first < Duration::from_secs(2), "{first:?}");
        assert!(p.compute_given_up.load(Ordering::Relaxed));
        // The next dispatch waits within the ordinary budget, at most that.
        let started = Instant::now();
        assert!(ready(&p).is_none());
        assert!(started.elapsed() <= wait_budget() + Duration::from_millis(500));
    }

    /// (Filip Michalski, fxceless/victory-fork#1353) A build that panics is
    /// refused, the thread making it goes on to the next, and a refused
    /// pipeline is not built again.
    #[test]
    fn a_panicking_pipeline_is_refused_and_the_worker_continues() {
        let failed = Arc::new(Pending::new(Vec::new(), None));
        let next = Arc::new(Pending::new(Vec::new(), None));
        let (failed_job, next_job) = (failed.clone(), next.clone());
        std::thread::spawn(move || {
            failed_job.make_with(|| panic!("synthetic pipeline build panic"));
            next_job.make_with(|| None);
        })
        .join()
        .expect("the pipeline worker should survive");
        assert!(matches!(failed.made.get(), Some(None)));
        assert!(matches!(next.made.get(), Some(None)));
        failed.make_with(|| panic!("a completed pipeline should not be rebuilt"));
    }
}
