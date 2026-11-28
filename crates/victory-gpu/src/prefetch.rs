//! The refreshes of a list's host copies, brought up to date by a thread of
//! ours once the list is done (`VICTORY_REFRESH_PREFETCH=2`), ahead of the
//! submission that runs the list.
//!
//! As the list closes its refreshes are moved out of it, and the list is done
//! at once: the queue takes it as it would without this, and never waits for
//! what follows. The thread (`victory-prefetch`, below normal priority) then
//! copies what the title wrote into those ranges, in pieces of [`PIECE`] under
//! each copy's `refreshing` lock, stamped with the write-watch asking it made
//! for their run ([`crate::watch::ask_prefetch`]: its own, held over the run
//! only). The submission asks again and copies what the title wrote since.
//!
//! The thread starts no piece of a copy the Lane is refreshing or declaring
//! writes on ([`crate::shadow::Shadow::lane_claimed`]): it does the others, and
//! that copy once the Lane is off it. A list the queue takes stops here: a
//! piece under way is finished, nothing after it is started.
//!
//! A list submitted into an idle line is not given to the thread: the queue
//! takes it as soon as it is done, so there is no time to win.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::Duration;

use crate::shadow::Shadow;

/// A refresh of a host copy: the copy, and the title memory it brings up to
/// date (address, bytes).
pub(crate) type Refresh = (Arc<Shadow>, usize, usize);

/// Bytes copied under a copy's lock at a time at most.
const PIECE: usize = 256 << 10;
/// Bytes one write-watch asking covers at most, held meanwhile.
const RUN: usize = 2 << 20;
/// Bytes between two pieces that one asking covers across.
const GAP: usize = 256 << 10;

const PREFETCHED: usize = 0;
const DROPPED: usize = 1;
const SKIPPED: usize = 2;

/// Lists by outcome ([`PREFETCHED`], [`DROPPED`], [`SKIPPED`]), since the last
/// frame-rate line.
#[cfg(feature = "diagnostics")]
static LISTS: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

static LINE: Mutex<VecDeque<Arc<Job>>> = Mutex::new(VecDeque::new());
static MORE: Condvar = Condvar::new();

/// `VICTORY_REFRESH_PREFETCH`: `2` (the thread of ours), any other value that
/// is set (`1`: the thread that translates a list does the refreshes before the
/// list is done, as [`crate::list::prefetch`]), or off.
pub(crate) fn mode() -> u8 {
    static MODE: LazyLock<u8> =
        LazyLock::new(|| match std::env::var("VICTORY_REFRESH_PREFETCH").ok().as_deref().map(str::trim) {
            None | Some("0") => 0,
            Some("2") => 2,
            Some(_) => 1,
        });
    *MODE
}

/// The refreshes of one closed list, per host list of the list.
pub(crate) struct Job {
    parts: Vec<Vec<Refresh>>,
    /// The queue took the list: nothing more of it is started.
    taken: AtomicBool,
    /// Counted in [`LISTS`] (once).
    counted: AtomicBool,
}

impl Job {
    fn of(parts: Vec<Vec<Refresh>>) -> Job {
        Job { parts, taken: AtomicBool::new(false), counted: AtomicBool::new(false) }
    }

    /// The job of a closed list's refreshes, if they are to be done by the thread.
    pub(crate) fn new(parts: Vec<Vec<Refresh>>) -> Option<Arc<Job>> {
        (mode() == 2 && parts.iter().any(|part| !part.is_empty())).then(|| Arc::new(Job::of(parts)))
    }

    /// The queue takes the list, or it is let go. Never waits for the thread.
    pub(crate) fn take(&self) {
        self.taken.store(true, Ordering::Release);
    }

    fn is_taken(&self) -> bool {
        self.taken.load(Ordering::Acquire)
    }
}

/// The job goes to the thread, unless the queue takes the list first, or the list was submitted into an idle line
/// (`behind` false): the queue takes such a list as soon as it is done, so there is no time to win.
pub(crate) fn submit(job: Arc<Job>, behind: bool) {
    if job.is_taken() {
        count(&job, DROPPED);
    } else if !behind {
        job.take();
        count(&job, SKIPPED);
    } else if !start() {
        job.take();
        count(&job, DROPPED);
    } else {
        LINE.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
        MORE.notify_one();
    }
}

/// A recording's prefetch: its job once it is closed, and whether the line had anything ahead of its submission. The
/// job is decided once the recording is both done and submitted ([`Pending::done`], [`Pending::submitted`]).
#[derive(Default)]
pub(crate) struct Pending {
    job: Option<Arc<Job>>,
    done: bool,
    behind: Option<bool>,
    decided: bool,
}

impl Pending {
    /// The recording is closed: its job, if it has one.
    pub(crate) fn closed(&mut self, job: Option<Arc<Job>>) {
        self.job = job;
    }

    /// The queue takes the list: its job stops, and nothing of it is started any more.
    pub(crate) fn taken(&self) {
        if let Some(job) = &self.job {
            job.take();
        }
    }

    /// The recording is submitted, and `behind` says whether the line had anything ahead of the submission.
    pub(crate) fn submitted(&mut self, behind: bool) -> Option<(Arc<Job>, bool)> {
        self.behind = Some(behind);
        self.decide()
    }

    /// The recording is done.
    pub(crate) fn done(&mut self) -> Option<(Arc<Job>, bool)> {
        self.done = true;
        self.decide()
    }

    /// The recording is let go: its job, if it was never decided, is taken and counted dropped.
    pub(crate) fn dropped(&mut self) {
        if let Some(job) = self.job.take() {
            job.take();
            if !std::mem::replace(&mut self.decided, true) {
                count(&job, DROPPED);
            }
        }
    }

    fn decide(&mut self) -> Option<(Arc<Job>, bool)> {
        if self.decided || !self.done {
            return None;
        }
        let behind = self.behind?;
        let job = self.job.clone()?;
        self.decided = true;
        Some((job, behind))
    }
}

fn count(job: &Job, what: usize) {
    if job.counted.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(feature = "diagnostics")]
    LISTS[what].fetch_add(1, Ordering::Relaxed);
    #[cfg(not(feature = "diagnostics"))]
    let _ = what;
}

/// Since the last frame-rate line, per frame: lists prefetched, dropped (taken
/// before their refreshes were done) and skipped (nothing ahead of them).
#[cfg(feature = "diagnostics")]
pub(crate) fn lists(frames: f64) -> [f64; 3] {
    [0, 1, 2].map(|i| LISTS[i].swap(0, Ordering::Relaxed) as f64 / frames)
}

/// Starts the thread, once: whether there is one.
fn start() -> bool {
    static STARTED: LazyLock<bool> = LazyLock::new(|| {
        let worker = || {
            use windows_sys::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
            };
            // SAFETY: this thread's own priority.
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
            let _asker = crate::watch::asker(crate::watch::Asker::Refresh);
            crate::cores::keep_helper_on_fast_cores("victory-prefetch");
            loop {
                let job = {
                    let mut line = LINE.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        match line.pop_front() {
                            Some(job) => break job,
                            None => line = MORE.wait(line).unwrap_or_else(|e| e.into_inner()),
                        }
                    }
                };
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&job))).unwrap_or(false);
                count(&job, if done { PREFETCHED } else { DROPPED });
            }
        };
        std::thread::Builder::new().name("victory-prefetch".into()).spawn(worker).is_ok()
    });
    *STARTED
}

/// Brings the job's copies up to date. `false` if the queue took the list first.
pub(crate) fn run(job: &Job) -> bool {
    let mut todo = plan(&job.parts);
    loop {
        let mut i = 0;
        while i < todo.len() {
            match drive(&mut todo[i], job) {
                Drive::Done => {
                    todo.swap_remove(i);
                }
                Drive::Claimed => i += 1,
                Drive::Taken => return false,
            }
        }
        if todo.is_empty() {
            return true;
        }
        // What is left is what the Lane is on: looked at again shortly.
        std::thread::sleep(Duration::from_micros(200));
    }
}

/// A copy's runs still to do.
struct Todo {
    shadow: Arc<Shadow>,
    runs: VecDeque<Run>,
}

/// Pieces of one copy, asked about together: `[start, end)`.
struct Run {
    start: usize,
    end: usize,
    pieces: VecDeque<(usize, usize)>,
}

enum Drive {
    Done,
    /// The Lane is on the copy: its rest waits.
    Claimed,
    Taken,
}

fn plan(parts: &[Vec<Refresh>]) -> Vec<Todo> {
    let mut index: crate::host::ByCopy<usize> = Default::default();
    let mut copies: Vec<(Arc<Shadow>, Vec<(usize, usize)>)> = Vec::new();
    for (shadow, at, len) in parts.iter().flatten() {
        if *len == 0 || !shadow.prefetchable() {
            continue;
        }
        let i = *index.entry(Arc::as_ptr(shadow) as usize).or_insert_with(|| {
            copies.push((shadow.clone(), Vec::new()));
            copies.len() - 1
        });
        copies[i].1.push((*at, *at + *len));
    }
    copies.into_iter().map(|(shadow, ranges)| Todo { shadow, runs: runs_of(merged(ranges)) }).collect()
}

/// `ranges`, in order, joined where they overlap or touch.
fn merged(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (from, to) in ranges {
        match out.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    out
}

/// `ranges` cut into pieces at multiples of [`PIECE`], gathered into runs.
fn runs_of(ranges: Vec<(usize, usize)>) -> VecDeque<Run> {
    let mut runs: VecDeque<Run> = VecDeque::new();
    for (from, to) in ranges {
        let mut at = from;
        while at < to {
            let next = ((at / PIECE + 1) * PIECE).min(to);
            match runs.back_mut() {
                Some(run) if at <= run.end + GAP && next - run.start <= RUN => {
                    run.end = next;
                    run.pieces.push_back((at, next));
                }
                _ => runs.push_back(Run { start: at, end: next, pieces: VecDeque::from([(at, next)]) }),
            }
            at = next;
        }
    }
    runs
}

fn drive(todo: &mut Todo, job: &Job) -> Drive {
    loop {
        if job.is_taken() {
            return Drive::Taken;
        }
        let Some(run) = todo.runs.front() else {
            return Drive::Done;
        };
        let (start, end) = (run.start, run.end);
        if todo.shadow.lane_claimed() {
            return Drive::Claimed;
        }
        let Some(now) = crate::watch::ask_prefetch(start, end) else {
            // The kernel refuses the range: the Lane refreshes it.
            todo.runs.pop_front();
            continue;
        };
        loop {
            let Some(&(from, to)) = todo.runs.front().and_then(|run| run.pieces.front()) else {
                break;
            };
            if job.is_taken() {
                return Drive::Taken;
            }
            if todo.shadow.lane_claimed() {
                return Drive::Claimed;
            }
            todo.shadow.prefetch_asked(from, to - from, now);
            if let Some(run) = todo.runs.front_mut() {
                run.pieces.pop_front();
            }
        }
        todo.runs.pop_front();
    }
}

#[cfg(test)]
pub(crate) fn job_for_test(parts: Vec<Vec<Refresh>>) -> Arc<Job> {
    Arc::new(Job::of(parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_decided_once_it_is_done_and_submitted() {
        let mut pending = Pending::default();
        pending.closed(Some(job_for_test(Vec::new())));
        assert!(pending.submitted(true).is_none(), "submitted before it is done: not given to the thread yet");
        assert!(matches!(pending.done(), Some((_, true))), "done, and submitted behind work: given to the thread");
        assert!(pending.done().is_none(), "and decided once");
    }

    #[test]
    fn a_list_submitted_into_an_idle_line_is_decided_not_behind() {
        let mut pending = Pending::default();
        pending.closed(Some(job_for_test(Vec::new())));
        assert!(pending.done().is_none(), "done, not yet submitted: nothing is known of the line");
        assert!(matches!(pending.submitted(false), Some((_, false))), "submitted into an idle line: decided, not behind");
    }
}
