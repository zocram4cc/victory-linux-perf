//! What making host pipelines costs, and what it costs the picture, said in
//! the log while pipelines are being made (always on: counters and a line
//! every five seconds at most, nothing that changes what is made or drawn).
//!
//! Before this the log said how many pipelines were loaded and made in all
//! ("host pipelines all made") and how many draws went without one, but not
//! how long a pipeline took, where that time went, or whether a dropped draw
//! waited for a pipeline at all: the frame line's "dropped N without a host
//! pipeline" counts the ray-tracing structure builder's dispatches too (off
//! without `VICTORY_BVH_BUILDER=1`), and the diagnostics' "being made" counted
//! pipelines left out on purpose. Runs 20261127-210643 and -200336 (warm):
//! no pipeline queued from 30 s on, and still 2,300-2,600 a frame "dropped"
//! in the opening cutscene, all of them the builder's.
//!
//! The times (`Times`): a pipeline's making on a worker as a whole; the
//! library loads that gave a pipeline (vkd3d-proton's conversion of the
//! code and the driver's compile, or the driver's own cache on disk when it
//! has it: a few ms then); the loads that found none; the pipelines made
//! anew after such a miss; and the wait in `HostPipeline::new` for the
//! shaders' translations (our translator, or our cache of it).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Durations of one kind: since the last line, and since the start.
/// Per array: count, sum (µs), longest (µs), then how many took under
/// 10 ms, 10-100 ms, 0.1-1 s, and 1 s or more.
pub(crate) struct Times {
    now: [AtomicU64; 7],
    all: [AtomicU64; 7],
}

/// The upper ends of the buckets after the first three slots, in µs.
const BUCKETS: [u64; 3] = [10_000, 100_000, 1_000_000];

impl Times {
    pub(crate) const fn new() -> Self {
        Times { now: [const { AtomicU64::new(0) }; 7], all: [const { AtomicU64::new(0) }; 7] }
    }

    pub(crate) fn add(&self, took: Duration) {
        let us = took.as_micros().min(u128::from(u64::MAX)) as u64;
        let bucket = 3 + BUCKETS.iter().take_while(|&&end| us >= end).count();
        for side in [&self.now, &self.all] {
            side[0].fetch_add(1, Ordering::Relaxed);
            side[1].fetch_add(us, Ordering::Relaxed);
            side[2].fetch_max(us, Ordering::Relaxed);
            side[bucket].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Since the last call, which starts the next interval.
    pub(crate) fn take(&self) -> [u64; 7] {
        self.now.each_ref().map(|a| a.swap(0, Ordering::Relaxed))
    }

    pub(crate) fn total(&self) -> [u64; 7] {
        self.all.each_ref().map(|a| a.load(Ordering::Relaxed))
    }
}

/// `t` ([`Times::take`] or [`Times::total`]) in words.
pub(crate) fn text(t: [u64; 7]) -> String {
    if t[0] == 0 {
        return "none".to_owned();
    }
    format!(
        "{} in {} ms (under 10 ms {}, 10-100 ms {}, 0.1-1 s {}, 1 s or more {}; longest {} ms)",
        t[0],
        t[1] / 1000,
        t[3],
        t[4],
        t[5],
        t[6],
        t[2] / 1000
    )
}

/// A worker's whole making of a pipeline ([`super::make`]).
pub(crate) static WHOLE: Times = Times::new();
/// Library loads that gave the pipeline.
pub(crate) static LOADED: Times = Times::new();
/// Library loads that found none (or no library).
pub(crate) static LOAD_MISSED: Times = Times::new();
/// `CreatePipelineState` after such a miss.
pub(crate) static CREATED: Times = Times::new();
/// Waiting for the shaders' translations at a pipeline's making.
pub(crate) static TRANSLATING: Times = Times::new();
/// From a draw's or dispatch's first ask to the pipeline made, for those
/// asked for before they were made.
pub(crate) static LATE: Times = Times::new();

/// Draws, dispatches and indirect executions dropped because their
/// pipeline was still being made (queued, or on a worker): since the start.
/// Not those of pipelines left out on purpose, refused or given up.
pub(crate) static LATE_DROPS: AtomicU64 = AtomicU64::new(0);
/// [`LATE_DROPS`] at the last line.
pub(crate) static LATE_DROPS_SAID: AtomicU64 = AtomicU64::new(0);

/// Draws dropped for a late pipeline since the start, for the frames CSV.
pub(crate) fn late_drops() -> u64 {
    LATE_DROPS.load(Ordering::Relaxed)
}

/// When the next line may come (`crate::host::now_ms`).
static NEXT_MS: AtomicU64 = AtomicU64::new(0);

/// How often the line comes at most.
pub(crate) const EVERY_MS: u64 = 5000;

/// Whether this caller says the line now: its time has come, and no other
/// caller took it.
pub(crate) fn due(now_ms: u64) -> bool {
    let next = NEXT_MS.load(Ordering::Relaxed);
    now_ms >= next
        && NEXT_MS.compare_exchange(next, now_ms + EVERY_MS, Ordering::Relaxed, Ordering::Relaxed).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_go_into_their_buckets_and_the_interval_starts_again() {
        let t = Times::new();
        for ms in [1, 9, 10, 99, 100, 999, 1000, 5000] {
            t.add(Duration::from_millis(ms));
        }
        let now = t.take();
        assert_eq!(now[0], 8);
        assert_eq!(now[1], (1 + 9 + 10 + 99 + 100 + 999 + 1000 + 5000) * 1000);
        assert_eq!(now[2], 5_000_000);
        assert_eq!(&now[3..], &[2, 2, 2, 2]);
        assert_eq!(t.take()[0], 0, "taken: the next interval is empty");
        assert_eq!(t.total()[0], 8, "the totals stay");
        assert_eq!(
            text(t.total()),
            "8 in 7218 ms (under 10 ms 2, 10-100 ms 2, 0.1-1 s 2, 1 s or more 2; longest 5000 ms)"
        );
        assert_eq!(text(t.take()), "none");
    }

    #[test]
    fn one_caller_has_the_line_each_interval() {
        // (The line's clock here is a test's own: well past any real one.)
        let base = u64::MAX / 2;
        assert!(due(base));
        assert!(!due(base + 1));
        assert!(!due(base + EVERY_MS - 1));
        assert!(due(base + EVERY_MS));
    }
}
