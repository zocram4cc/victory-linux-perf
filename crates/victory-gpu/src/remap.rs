//! Title memory remaps, counted per frame (`VICTORY_REMAP_STATS=1`): the
//! calls, time and bytes of each kind of change to what title memory shows (a
//! page shown, a page taken away, a placeholder split, a GPU page-mapping
//! copy), and the wait for the remapping lock ([`crate::watch`]) and its hold.
//! Off, nothing is counted and [`Timed::begin`] is `None`.
//!
//! The frame meter takes the counts once a second ([`take`]): one info line,
//! and the `map_*`, `unmap_*`, `split_*`, `alias_*` and `lock_*` columns of the
//! frametime dump. A copy's maps and unmaps are counted under their own kinds
//! too, so `alias` overlaps them, and the lock is held around all of it.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// What is counted; [`take`] and the counters are in this order.
#[derive(Clone, Copy)]
pub enum Kind {
    /// A page shown: a view or a host mapping of a section's file, committed.
    Map,
    /// A page taken away.
    Unmap,
    /// A placeholder cut in two so that a view fits.
    Split,
    /// A GPU page-mapping copy: one call of `map_aliases` or `map_alias`.
    Alias,
    /// The wait for the remapping lock, per change.
    LockWait,
    /// The hold of the remapping lock, per change.
    LockHold,
}

const KINDS: usize = 6;

static CALLS: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
static NANOS: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
static BYTES: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];

#[cfg(test)]
thread_local! {
    /// What [`on`] answers for this thread's calls (tests only).
    static FORCED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether the remap counters are on (`VICTORY_REMAP_STATS=1`).
pub fn on() -> bool {
    #[cfg(test)]
    if let Some(forced) = FORCED.get() {
        return forced;
    }
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var("VICTORY_REMAP_STATS").is_ok_and(|v| v.trim() == "1"));
    *ON
}

/// One kind's figures over a summary interval, per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Figures {
    pub calls: f64,
    pub ms: f64,
    /// Megabytes (10^6 bytes).
    pub mb: f64,
}

impl std::fmt::Display for Figures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:.1} calls {:.2} ms {:.2} MB",
            self.calls, self.ms, self.mb
        )
    }
}

/// Every kind's figures over a summary interval, per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub map: Figures,
    pub unmap: Figures,
    pub split: Figures,
    pub alias: Figures,
    pub lock_wait: Figures,
    pub lock_hold: Figures,
}

/// Counts one change of `kind` over `bytes` from [`Timed::begin`] until dropped.
pub struct Timed {
    kind: Kind,
    bytes: usize,
    started: Instant,
}

impl Timed {
    /// Starts counting a change of `kind` over `bytes`; `None` unless on.
    pub fn begin(kind: Kind, bytes: usize) -> Option<Timed> {
        on().then(|| Timed {
            kind,
            bytes,
            started: Instant::now(),
        })
    }
}

impl Drop for Timed {
    fn drop(&mut self) {
        record(
            self.kind,
            self.bytes,
            self.started.elapsed().as_nanos() as u64,
        );
    }
}

fn record(kind: Kind, bytes: usize, nanos: u64) {
    let i = kind as usize;
    CALLS[i].fetch_add(1, Ordering::Relaxed);
    NANOS[i].fetch_add(nanos, Ordering::Relaxed);
    BYTES[i].fetch_add(bytes as u64, Ordering::Relaxed);
}

/// Takes every counter as figures per frame of `frames` frames, and resets them.
pub fn take(frames: f64) -> Frame {
    let [map, unmap, split, alias, lock_wait, lock_hold]: [Figures; KINDS] =
        std::array::from_fn(|i| Figures {
            calls: CALLS[i].swap(0, Ordering::Relaxed) as f64 / frames,
            ms: NANOS[i].swap(0, Ordering::Relaxed) as f64 / 1e6 / frames,
            mb: BYTES[i].swap(0, Ordering::Relaxed) as f64 / 1e6 / frames,
        });
    Frame {
        map,
        unmap,
        split,
        alias,
        lock_wait,
        lock_hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the counters: one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn figures_are_per_frame_and_taking_resets_them() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        take(1.0);
        record(Kind::Map, 500_000, 2_000_000);
        record(Kind::Map, 500_000, 4_000_000);
        record(Kind::Unmap, 0, 1_000_000);
        let f = take(2.0);
        assert_eq!(
            f.map,
            Figures {
                calls: 1.0,
                ms: 3.0,
                mb: 0.5
            }
        );
        assert_eq!(
            f.unmap,
            Figures {
                calls: 0.5,
                ms: 0.5,
                mb: 0.0
            }
        );
        assert_eq!(take(1.0), Frame::default());
    }

    #[test]
    fn a_timed_change_is_counted_only_while_on() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        FORCED.set(Some(false));
        assert!(Timed::begin(Kind::Split, 4_000_000).is_none());
        take(1.0);
        FORCED.set(Some(true));
        drop(Timed::begin(Kind::Split, 4_000_000));
        let f = take(1.0);
        FORCED.set(None);
        assert_eq!((f.split.calls, f.split.mb), (1.0, 4.0));
    }
}
