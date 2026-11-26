//! A stage of a submission's preparing done by a thread of ours while the
//! submitting thread goes on with the next one.
//!
//! The frame is as long as that one thread works (`crate::queue::behind`:
//! "that thread worked 50.4 ms" of a 50 ms frame, run av5), and within a
//! submission it does one thing after the other that have nothing to do with
//! one another: the write watch is asked while nothing is planned for the
//! card, the title's bytes go under flights' writes while no copy is
//! refreshed, the snapshots are taken while nothing is sent up to the copies
//! in the card's memory. Each pair's second half is [`begin`]'s here, and is
//! done by the time [`Begun::end`] returns: by the kept thread if that was
//! free in time, else then and there by the caller, as before. The callers
//! say why their two halves do not meet.

use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

fn lock<T>(of: &Mutex<T>) -> MutexGuard<'_, T> {
    of.lock().unwrap_or_else(|e| e.into_inner())
}

enum State<B> {
    /// Nobody has begun it.
    Waiting(Box<dyn FnOnce() -> B + Send>),
    Running,
    Done(std::thread::Result<B>),
    /// Its result was taken, or it was let go.
    Taken,
}

struct Cell<B> {
    state: Mutex<State<B>>,
    done: Condvar,
}

trait Run: Send + Sync {
    /// Does the work, if nobody has begun it: whether this call did.
    fn run(&self) -> bool;
}

impl<B: Send> Run for Cell<B> {
    fn run(&self) -> bool {
        let work = {
            let mut state = lock(&self.state);
            match std::mem::replace(&mut *state, State::Running) {
                State::Waiting(work) => work,
                other => {
                    *state = other;
                    return false;
                }
            }
        };
        // (However it ends, who waits for it goes on, and hears of it.)
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        *lock(&self.state) = State::Done(out);
        self.done.notify_all();
        true
    }
}

/// What the kept thread is to look at, and the word that there is something.
struct Kept {
    jobs: Mutex<VecDeque<Arc<dyn Run>>>,
    more: Condvar,
    /// How many, for [`spin`].
    there: AtomicUsize,
}

static KEPT: Kept = Kept {
    jobs: Mutex::new(VecDeque::new()),
    more: Condvar::new(),
    there: AtomicUsize::new(0),
};

/// Since the last frame-rate line: pieces of work begun, and those the kept
/// thread did (the others the caller did itself, at their end).
pub(super) static COUNTS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Waits awake until `there` says so, `VICTORY_BESIDE_SPIN` microseconds at
/// most (500 if not given; `0`: not at all). A submission has three pieces
/// of work for the thread, each over within some 300 microseconds, and a
/// thread asleep is woken in 50 and more: the next piece comes while it
/// would still be on its way back to sleep.
fn spin(there: impl Fn() -> bool) {
    static MOST: OnceLock<u64> = OnceLock::new();
    let most = *MOST.get_or_init(|| std::env::var("VICTORY_BESIDE_SPIN").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(500));
    if most == 0 {
        return;
    }
    let began = std::time::Instant::now();
    while !there() && (began.elapsed().as_micros() as u64) < most {
        for _ in 0..16 {
            std::hint::spin_loop();
        }
    }
}

fn serve() {
    crate::cores::keep_on_fast_cores("victory-beside");
    loop {
        let job = {
            spin(|| KEPT.there.load(Ordering::Acquire) > 0);
            let mut jobs = lock(&KEPT.jobs);
            loop {
                if let Some(job) = jobs.pop_front() {
                    KEPT.there.fetch_sub(1, Ordering::AcqRel);
                    break job;
                }
                jobs = KEPT.more.wait(jobs).unwrap_or_else(|e| e.into_inner());
            }
        };
        let _working = crate::timeline::span("beside the submitting thread");
        if job.run() {
            COUNTS[1].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Whether the kept thread is there (made at the first call).
fn started() -> bool {
    static STARTED: OnceLock<bool> = OnceLock::new();
    *STARTED.get_or_init(|| std::thread::Builder::new().name("victory-beside".into()).spawn(serve).is_ok())
}

/// What is lent to the kept thread for the length of a piece of work.
struct Lent<F>(F);

// SAFETY: `begin`'s callers answer for what the work touches.
unsafe impl<F> Send for Lent<F> {}

impl<F> Lent<F> {
    fn call<B>(self) -> B
    where
        F: FnOnce() -> B,
    {
        (self.0)()
    }
}

/// Work begun by [`begin`]: done when [`Begun::end`] returns, and not running
/// any more when this is dropped without it.
pub(super) struct Begun<'a, B: Send + 'static> {
    cell: Arc<Cell<B>>,
    /// What the work borrows.
    _lent: PhantomData<&'a mut ()>,
}

/// `work` begins on the kept thread as soon as that is free; the caller goes
/// on, and takes the result with [`Begun::end`].
///
/// # Safety
/// `work` may run on another thread, at any time until what comes back is
/// ended or dropped (never forgotten): what it touches is safe to touch from
/// there meanwhile, and lives that long.
pub(super) unsafe fn begin<'a, B: Send + 'static>(work: impl FnOnce() -> B + 'a) -> Begun<'a, B> {
    let lent = Lent(work);
    let work: Box<dyn FnOnce() -> B + Send + 'a> = Box::new(move || lent.call());
    // SAFETY: the caller's: the work is done or let go before `'a` ends.
    let work = unsafe { std::mem::transmute::<Box<dyn FnOnce() -> B + Send + 'a>, Box<dyn FnOnce() -> B + Send + 'static>>(work) };
    let cell = Arc::new(Cell {
        state: Mutex::new(State::Waiting(work)),
        done: Condvar::new(),
    });
    COUNTS[0].fetch_add(1, Ordering::Relaxed);
    if started() {
        lock(&KEPT.jobs).push_back(cell.clone());
        KEPT.there.fetch_add(1, Ordering::AcqRel);
        KEPT.more.notify_one();
    }
    Begun { cell, _lent: PhantomData }
}

impl<B: Send + 'static> Begun<'_, B> {
    /// What the work gave: done here and now if the kept thread has not
    /// begun it, waited for if it is at it. A panic of the work goes on here.
    pub(super) fn end(self) -> B {
        self.cell.run();
        let mut state = lock(&self.cell.state);
        loop {
            match std::mem::replace(&mut *state, State::Taken) {
                State::Done(out) => {
                    drop(state);
                    return out.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                }
                other => {
                    *state = other;
                    state = self.cell.done.wait(state).unwrap_or_else(|e| e.into_inner());
                }
            }
        }
    }
}

impl<B: Send + 'static> Drop for Begun<'_, B> {
    fn drop(&mut self) {
        let mut state = lock(&self.cell.state);
        loop {
            match &*state {
                State::Running => state = self.cell.done.wait(state).unwrap_or_else(|e| e.into_inner()),
                // Not begun: it never is (what it borrows goes with it, here).
                State::Waiting(_) => {
                    *state = State::Taken;
                    return;
                }
                State::Done(_) | State::Taken => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_begun_is_done_at_its_end_whoever_did_it() {
        let mut sum = 0u64;
        for round in 0..200u64 {
            let mut here = 0u64;
            // SAFETY: ended below, before `here` goes.
            let begun = unsafe {
                begin(|| {
                    here = round;
                    round * 2
                })
            };
            if round % 3 == 0 {
                std::thread::yield_now();
            }
            sum += begun.end();
            assert_eq!(here, round);
        }
        assert_eq!(sum, 199 * 200);
    }

    #[test]
    fn work_let_go_is_not_running_afterwards() {
        let touched = std::sync::atomic::AtomicBool::new(false);
        for _ in 0..200 {
            // SAFETY: dropped at once, before `touched` goes.
            drop(unsafe { begin(|| touched.store(true, Ordering::Relaxed)) });
        }
        // (Begun or not, each is over: nothing writes `touched` from here on.)
        let was = touched.load(Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(touched.load(Ordering::Relaxed), was);
    }

    #[test]
    fn a_panic_of_the_work_comes_out_at_its_end() {
        // SAFETY: ended at once; nothing is borrowed.
        let begun = unsafe { begin(|| -> u32 { panic!("a panic of the work") }) };
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| begun.end())).is_err());
    }
}
