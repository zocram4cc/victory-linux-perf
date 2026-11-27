//! Diagnostics (`VICTORY_WC_STATS=1`): how much of what is copied up to the
//! card and back into title memory lies in memory the title asked for as
//! write-combined or uncached, which the console's CPU writes but does not
//! read back (as PC titles keep their per-frame rings in memory they only
//! write). XMem reports every protection the title asks for
//! ([`title_memory_protection`]); copies are counted by the title address
//! they start at, against the newest protection asked for there.
//!
//! Off, every call returns at once. On, a copy costs a read lock and a
//! map lookup.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

/// Kinds of title memory, by what the title asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum Kind {
    /// No protection was ever noted there (host memory, or memory the
    /// runtime gave the title another way).
    Unknown = 0,
    /// Cached for the CPU (no write-combining or uncached bit).
    Cached = 1,
    /// The console's write-combined bit (0x400).
    WriteCombined = 2,
    /// The console's uncached bit (0x200).
    Uncached = 3,
}

const KINDS: usize = 4;
const NAMES: [&str; KINDS] = ["unknown", "cached", "write-combined", "uncached"];

/// The console's protection bits for how the CPU reaches memory.
const UNCACHED: u32 = 0x200;
const WRITE_COMBINED: u32 = 0x400;

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WC_STATS").is_some_and(|v| v == "1"))
}

/// Start of a range -> (end, kind); ranges never overlap.
fn ranges() -> &'static RwLock<BTreeMap<usize, (usize, Kind)>> {
    static RANGES: OnceLock<RwLock<BTreeMap<usize, (usize, Kind)>>> = OnceLock::new();
    RANGES.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn kind_of_protection(protect: u32) -> Kind {
    if protect & WRITE_COMBINED != 0 {
        Kind::WriteCombined
    } else if protect & UNCACHED != 0 {
        Kind::Uncached
    } else {
        Kind::Cached
    }
}

/// The title asked for `[at, at + len)` with the console's `protect`
/// (reserved, committed or allocated): the newest word on that range.
pub fn title_memory_protection(at: usize, len: usize, protect: u32) {
    if !on() || at == 0 || len == 0 {
        return;
    }
    insert(&mut ranges().write().unwrap_or_else(|e| e.into_inner()), at, at.saturating_add(len), kind_of_protection(protect));
}

/// Puts `[from, to)` as `kind`, cutting what it overlaps.
fn insert(map: &mut BTreeMap<usize, (usize, Kind)>, from: usize, to: usize, kind: Kind) {
    // A range that starts before `from` and reaches into it keeps its head
    // (and its tail past `to`, if it reaches past).
    if let Some((&start, &(end, k))) = map.range(..from).next_back()
        && end > from
    {
        map.insert(start, (from, k));
        if end > to {
            map.insert(to, (end, k));
        }
    }
    // Ranges that start inside go; the last may keep its tail.
    let inside: Vec<usize> = map.range(from..to).map(|(&s, _)| s).collect();
    for start in inside {
        if let Some((end, k)) = map.remove(&start)
            && end > to
        {
            map.insert(to, (end, k));
        }
    }
    map.insert(from, (to, kind));
}

pub(crate) fn kind(address: usize) -> Kind {
    let map = ranges().read().unwrap_or_else(|e| e.into_inner());
    match map.range(..=address).next_back() {
        Some((_, &(end, k))) if address < end => k,
        _ => Kind::Unknown,
    }
}

/// Bytes copied up to the card (refreshes, copies, fills), by kind, and
/// what of them were counted.
static UP: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
/// Bytes put back into title memory from what shaders wrote, by kind.
static BACK: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];

/// `bytes` of title memory at `address` were copied up.
pub(crate) fn up(address: usize, bytes: u64) {
    if on() && bytes != 0 {
        UP[kind(address) as usize].fetch_add(bytes, Ordering::Relaxed);
    }
}

/// `bytes` were put back into title memory at `address`.
pub(crate) fn back(address: usize, bytes: u64) {
    if on() && bytes != 0 {
        BACK[kind(address) as usize].fetch_add(bytes, Ordering::Relaxed);
    }
}

/// With the frame-rate line: MB per frame by kind since the last, and the
/// title memory noted of each kind.
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let take = |a: &[AtomicU64; KINDS]| -> [f64; KINDS] {
        std::array::from_fn(|i| a[i].swap(0, Ordering::Relaxed) as f64 / 1e6 / frames.max(1.0))
    };
    let (up, back) = (take(&UP), take(&BACK));
    let mut noted = [0u64; KINDS];
    for (start, (end, k)) in ranges().read().unwrap_or_else(|e| e.into_inner()).iter() {
        noted[*k as usize] += (end - start) as u64;
    }
    let part = |v: &[f64; KINDS]| {
        (0..KINDS).map(|i| format!("{} {:.1}", NAMES[i], v[i])).collect::<Vec<_>>().join(", ")
    };
    tracing::info!(
        "title memory by what the title asked for (VICTORY_WC_STATS), MB per frame: copied up {}; copied back {}; \
         noted so far: {}",
        part(&up),
        part(&back),
        (0..KINDS).skip(1).map(|i| format!("{} {} MB", NAMES[i], noted[i] >> 20)).collect::<Vec<_>>().join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_protection_wins_and_cuts_older_ranges() {
        let mut map = BTreeMap::new();
        insert(&mut map, 0x1000, 0x9000, Kind::Cached);
        insert(&mut map, 0x3000, 0x5000, Kind::WriteCombined);
        let got: Vec<_> = map.iter().map(|(&s, &(e, k))| (s, e, k)).collect();
        assert_eq!(
            got,
            [(0x1000, 0x3000, Kind::Cached), (0x3000, 0x5000, Kind::WriteCombined), (0x5000, 0x9000, Kind::Cached)]
        );
        insert(&mut map, 0x2000, 0x6000, Kind::Uncached);
        let got: Vec<_> = map.iter().map(|(&s, &(e, k))| (s, e, k)).collect();
        assert_eq!(got, [(0x1000, 0x2000, Kind::Cached), (0x2000, 0x6000, Kind::Uncached), (0x6000, 0x9000, Kind::Cached)]);
    }

    #[test]
    fn protection_bits_pick_the_kind() {
        assert_eq!(kind_of_protection(0x2404), Kind::WriteCombined);
        assert_eq!(kind_of_protection(0x2204), Kind::Uncached);
        assert_eq!(kind_of_protection(0x22004), Kind::Cached);
    }
}
