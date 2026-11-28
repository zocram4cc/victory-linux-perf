//! Retry dynamic descriptor allocation as flights give their slots back.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::{FINISHED, LINE_LANDED, Line};

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SLOT_WAIT").is_some_and(|v| v == "1")) && super::relay()
}

// Completed allocation waits and total caller time since the last frame-rate line.
static WAITS: Mutex<(u64, Duration)> = Mutex::new((0, Duration::ZERO));

pub(super) fn log(frames: f64) {
    let (count, elapsed) = std::mem::take(&mut *WAITS.lock().unwrap_or_else(|e| e.into_inner()));
    if count != 0 || enabled() {
        let mode = if enabled() { "relay landings" } else { "settle" };
        tracing::info!(
            "dynamic descriptor slot waits per frame ({mode}): {:.2} waits, {:.2} ms",
            count as f64 / frames, elapsed.as_secs_f64() * 1000.0 / frames
        );
    }
}

/// Called by list translation, outside a submission, once its first try for
/// slots has failed: the settle of old, or (`VICTORY_SLOT_WAIT=1` with the
/// relay) waiting for landings. A successful allocation never gets here, so
/// it does not touch the relay, the clock or the counters.
pub(crate) fn after_miss(allocate: impl FnMut() -> Option<u32>) -> Option<u32> {
    let start = Instant::now();
    let by_landing = enabled();
    let mut last = FINISHED.load(Ordering::Acquire);
    let result = retry(allocate, by_landing, super::settle_for_fence, || {
        next_landing(&mut last, super::line(), &LINE_LANDED, &FINISHED)
    });
    let elapsed = start.elapsed();
    let mut waits = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    waits.0 += 1;
    waits.1 += elapsed;
    result
}

/// `last` is sampled before trying the allocator. A flight releases its
/// slots before publishing FINISHED, then updates the line and notifies.
/// A flight already removed from the line may still be landing.
fn next_landing(last: &mut u64, line: MutexGuard<'_, Line>, landed: &Condvar, finished: &AtomicU64) -> bool {
    let _line = landed.wait_while(line, |q| {
        (q.settling || !q.line.is_empty()) && finished.load(Ordering::Acquire) == *last
    }).unwrap_or_else(|e| e.into_inner());
    let now = finished.load(Ordering::Acquire);
    let advanced = now != *last;
    *last = now;
    advanced
}

pub(crate) fn retry(
    mut allocate: impl FnMut() -> Option<u32>,
    by_landing: bool,
    settle: impl FnOnce(),
    mut next: impl FnMut() -> bool,
) -> Option<u32> {
    if by_landing {
        let _span = crate::timeline::span("waiting for landings: dynamic descriptor slots");
        loop {
            if let Some(slot) = allocate() {
                return Some(slot);
            }
            if !next() {
                break;
            }
        }
    } else {
        settle();
    }
    // One final attempt after observing an empty line (or a full settle).
    allocate()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_queue_can_still_have_a_landing_in_progress() {
        let line = Mutex::new(Line { settling: true, ..Default::default() });
        let finished = AtomicU64::new(0);
        let landed = Condvar::new();
        let (ready, waiter_ready) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                let guard = line.lock().unwrap();
                ready.send(()).unwrap();
                let mut last = 0;
                assert!(next_landing(&mut last, guard, &landed, &finished));
                assert_eq!(last, 1);
            });
            waiter_ready.recv().unwrap();
            let mut guard = line.lock().unwrap();
            finished.store(1, Ordering::Release);
            guard.settling = false;
            drop(guard);
            landed.notify_all();
            waiter.join().unwrap();
        });
        // A landing between the failed allocation and taking the line's
        // lock is observed even if the notification has already happened.
        let mut last = 0;
        assert!(next_landing(&mut last, line.lock().unwrap(), &landed, &finished));
        assert!(!next_landing(&mut last, line.lock().unwrap(), &landed, &finished));
    }
}
