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

/// Whether title memory is sorted by kind: for the stats, or for
/// [`no_back`], which needs to know it.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WC_STATS").is_some_and(|v| v == "1") || no_back())
}

/// `VICTORY_WC_NO_BACK=1` (off by default): what shaders write into title
/// memory the title asked for as write-combined is not copied back into it.
/// The card's side keeps it, as a PC title keeps what its GPU writes: the
/// console's CPU does not read such memory back (belorx_8: all 46 MB a frame
/// of the copy-back went there). Such a range is not declared as written at
/// all (no snapshot, no compare, no copy back); it is noted as the card's
/// ([`crate::shadow::Shadow::card_owns`]) so that a refresh of a copy whose
/// writes are not watched does not put the title's old bytes over it.
/// Where the copy is watched, pages the CPU writes are still sent up: its
/// newer bytes win, as they do today.
///
/// Ranges under `VICTORY_WC_NO_BACK_MIN_KB` (64 by default) are copied back
/// as before: the few results the title's CPU does read (the walls'
/// culling, small read-outs) are small.
pub(crate) fn no_back() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WC_NO_BACK").is_some_and(|v| v == "1"))
}

fn no_back_min() -> usize {
    static MIN: OnceLock<usize> = OnceLock::new();
    *MIN.get_or_init(|| {
        std::env::var("VICTORY_WC_NO_BACK_MIN_KB").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(64) << 10
    })
}

/// Declared writes left to the card ([`skip_back`]) and small ones kept,
/// in declared bytes (not bytes shaders changed), since the last line.
static SKIPPED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Whether shaders' writes to `[address, address + len)` stay on the card
/// ([`no_back`]): the whole range lies in write-combined title memory and
/// is not small.
pub(crate) fn skip_back(address: usize, len: usize) -> bool {
    if !no_back() || len == 0 {
        return false;
    }
    let wc = kind(address) == Kind::WriteCombined && kind(address + len - 1) == Kind::WriteCombined;
    if !wc {
        return false;
    }
    if len < no_back_min() {
        SKIPPED[2].fetch_add(1, Ordering::Relaxed);
        SKIPPED[3].fetch_add(len as u64, Ordering::Relaxed);
        return false;
    }
    SKIPPED[0].fetch_add(1, Ordering::Relaxed);
    SKIPPED[1].fetch_add(len as u64, Ordering::Relaxed);
    true
}

/// A set of disjoint `[start, end)` ranges, joined where they touch.
#[derive(Default)]
pub(crate) struct Ranges(BTreeMap<usize, usize>);

impl Ranges {
    pub(crate) fn add(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let (mut from, mut to) = (from, to);
        // One that starts before and reaches `from` is joined.
        if let Some((&s, &e)) = self.0.range(..=from).next_back()
            && e >= from
        {
            if e >= to {
                return;
            }
            from = s;
        }
        let inside: Vec<usize> = self.0.range(from..=to).map(|(&s, _)| s).collect();
        for s in inside {
            if let Some(e) = self.0.remove(&s) {
                to = to.max(e);
            }
        }
        self.0.insert(from, to);
    }

    /// The ranges that overlap `[from, to)`.
    pub(crate) fn overlapping(&self, from: usize, to: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        let first = self.0.range(..from).next_back().filter(|(_, e)| **e > from).map(|(&s, &e)| (s, e));
        first.into_iter().chain(self.0.range(from..to).map(|(&s, &e)| (s, e)))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
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

/// MB per frame, by kind, copied up and copied back.
pub(crate) type Sample = Option<([f64; KINDS], [f64; KINDS])>;

/// Takes the counters as the frame-rate line takes its own (every second,
/// over the same frames), so that the line printed beside it matches it.
pub(crate) fn take(frames: f64) -> Sample {
    if !on() {
        return None;
    }
    let take = |a: &[AtomicU64; KINDS]| -> [f64; KINDS] {
        std::array::from_fn(|i| a[i].swap(0, Ordering::Relaxed) as f64 / 1e6 / frames.max(1.0))
    };
    Some((take(&UP), take(&BACK)))
}

/// With the frame-rate line: what [`take`] took, and the title memory
/// noted of each kind.
pub(crate) fn log(sample: Sample) {
    let Some((up, back)) = sample else {
        return;
    };
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
    if no_back() {
        let [n, bytes, small, small_bytes] = std::array::from_fn(|i| SKIPPED[i].swap(0, Ordering::Relaxed));
        tracing::info!(
            "shaders' writes into write-combined title memory left on the card (VICTORY_WC_NO_BACK, under {} KiB kept), \
             since the last line: {n} ranges declared, {} MB; kept and copied back as before: {small} ranges, {:.1} MB",
            no_back_min() >> 10,
            bytes >> 20,
            small_bytes as f64 / 1e6
        );
    }
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
    fn ranges_join_and_answer_overlaps() {
        let mut r = Ranges::default();
        r.add(10, 20);
        r.add(30, 40);
        r.add(20, 25);
        r.add(12, 15);
        assert_eq!(r.0.iter().map(|(&s, &e)| (s, e)).collect::<Vec<_>>(), [(10, 25), (30, 40)]);
        assert_eq!(r.overlapping(0, 11).collect::<Vec<_>>(), [(10, 25)]);
        assert_eq!(r.overlapping(25, 30).collect::<Vec<_>>(), Vec::<(usize, usize)>::new());
        assert_eq!(r.overlapping(24, 31).collect::<Vec<_>>(), [(10, 25), (30, 40)]);
        r.add(5, 50);
        assert_eq!(r.0.iter().map(|(&s, &e)| (s, e)).collect::<Vec<_>>(), [(5, 50)]);
    }

    #[test]
    fn protection_bits_pick_the_kind() {
        assert_eq!(kind_of_protection(0x2404), Kind::WriteCombined);
        assert_eq!(kind_of_protection(0x2204), Kind::Uncached);
        assert_eq!(kind_of_protection(0x22004), Kind::Cached);
    }
}
