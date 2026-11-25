//! Measuring counters that cost nothing unless asked for (`VICTORY_MEDIR=1`,
//! fase 0.4 of the README). Other crates add to them; the frametime dump
//! (`VICTORY_FRAMETIME_DUMP`) takes them per summary interval, so
//! `scripts/bench.sh` sees them beside the frame's other buckets.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Whether `VICTORY_MEDIR=1` is set.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MEDIR").is_some_and(|v| v == "1"))
}

/// Changes of the title's memory mapping (`victory-hle` xmem): unmaps,
/// their µs, kernel maps, their µs, µs of the bookkeeping after a map. All
/// of it is time under the remapping lock that texture refreshes wait for.
pub static REMAP: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
/// Index of each [`REMAP`] counter.
pub const UNMAPS: usize = 0;
pub const UNMAP_US: usize = 1;
pub const MAPS: usize = 2;
pub const MAP_US: usize = 3;
pub const BOOKS_US: usize = 4;

/// Adds `n` to [`REMAP`]`[k]` when measuring.
pub fn add_remap(k: usize, n: u64) {
    if on() {
        REMAP[k].fetch_add(n, Relaxed);
    }
}

/// The counters since the previous call, and zeroes them.
pub(crate) fn take_remap() -> [u64; 5] {
    [0, 1, 2, 3, 4].map(|i| REMAP[i].swap(0, Relaxed))
}
