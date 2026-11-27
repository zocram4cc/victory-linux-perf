//! Write-combined title memory left on the card (`VICTORY_SKIP_WC_BACK=1`, off by default).
//!
//! Memory the title reserves write-combined (console protection bit 0x400) is memory its CPU
//! writes and does not read back: what shaders write there need not be fetched back from the
//! card's copy. Buffers under `VICTORY_SKIP_WC_MIN_KB` (1024 by default) are still fetched:
//! skipping the small ones left rooms dark and frames black (in the forks it comes from).
//! Diagnostics builds can turn it on and off while the title runs
//! (`VICTORY_AB=<s>,VICTORY_SKIP_WC_BACK`).
//!
//! Port of fxceless/victory-fork#868 by Chipsraine (reservation fix by iGoldenNinja), with the
//! 1 MiB gate from Proudnoob4's fxceless/victory-fork#1268, as in PastiiJay/klbf-lab e0a91ea.
//! Here also: a later reservation that is not write-combined takes the place of those it
//! overlaps, so reused addresses are not taken for write-combined ones.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Title reservations asked write-combined: base -> length.
static WC: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());

/// Pieces left on the card, and their bytes.
static SKIPPED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// `VICTORY_SKIP_WC_BACK=1` (or the in-run A/B's choice in a diagnostics build).
fn on() -> bool {
    #[cfg(feature = "diagnostics")]
    {
        static BIT: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
        if let Some(bit) = *BIT.get_or_init(|| crate::ab::bit("VICTORY_SKIP_WC_BACK")) {
            return crate::ab::on(bit);
        }
    }
    asked()
}

/// Whether the reservations are kept at all: asked for, or under the in-run A/B.
fn kept() -> bool {
    #[cfg(feature = "diagnostics")]
    if crate::ab::bit("VICTORY_SKIP_WC_BACK").is_some() {
        return true;
    }
    asked()
}

fn asked() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("VICTORY_SKIP_WC_BACK").is_ok_and(|v| v.trim() == "1");
        if on {
            tracing::info!(
                "write-combined title memory of {} KiB and over is not fetched back from the card (VICTORY_SKIP_WC_BACK=1)",
                min_bytes() >> 10
            );
        }
        on
    })
}

/// `VICTORY_SKIP_WC_MIN_KB` (1024 KiB by default), in bytes.
fn min_bytes() -> u64 {
    static MIN: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MIN.get_or_init(|| {
        std::env::var("VICTORY_SKIP_WC_MIN_KB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(1024)
            .saturating_mul(1024)
    })
}

/// The title reserved `[at, at + len)` with console protection `protect`.
pub(crate) fn reserved(at: usize, len: usize, protect: u32) {
    if at == 0 || len == 0 || !kept() {
        return;
    }
    let mut map = WC.lock().unwrap_or_else(|e| e.into_inner());
    note(&mut map, at, len, protect & 0x400 != 0);
}

/// `[at, at + len)` is reserved now, write-combined or not: the reservations it overlaps are
/// gone.
fn note(map: &mut BTreeMap<usize, usize>, at: usize, len: usize, wc: bool) {
    let end = at.saturating_add(len);
    let gone: Vec<usize> = map
        .range(..end)
        .rev()
        .take_while(|&(&base, &n)| base.saturating_add(n) > at || base >= at)
        .filter(|&(&base, &n)| base.saturating_add(n) > at)
        .map(|(&base, _)| base)
        .collect();
    for base in gone {
        map.remove(&base);
    }
    if wc {
        map.insert(at, len);
    }
}

fn in_wc(map: &BTreeMap<usize, usize>, (from, to): (usize, usize)) -> bool {
    map.range(..=from)
        .next_back()
        .is_some_and(|(&base, &len)| base.checked_add(len).is_some_and(|end| to <= end))
}

/// Whether a piece of `bytes` of a copy (of a `buffer_len`-byte title buffer) whose shaders
/// declared `declared` written is left on the card: all of `declared` is write-combined
/// memory and the buffer is big enough.
pub(crate) fn skip_back(buffer_len: u64, declared: (usize, usize), bytes: u64) -> bool {
    if buffer_len < min_bytes() || !on() {
        return false;
    }
    let wc = in_wc(&WC.lock().unwrap_or_else(|e| e.into_inner()), declared);
    if wc {
        let n = SKIPPED[0].fetch_add(1, Ordering::Relaxed) + 1;
        let b = SKIPPED[1].fetch_add(bytes, Ordering::Relaxed) + bytes;
        if n.is_power_of_two() {
            tracing::info!(
                "VICTORY_SKIP_WC_BACK: {n} pieces ({} MB) of write-combined memory left on the card so far",
                b >> 20
            );
        }
    }
    wc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_write_combined_only_inside_one_reservation() {
        let map = BTreeMap::from([(0x1000, 0x1000), (0x4000, 0x2000)]);
        assert!(in_wc(&map, (0x1000, 0x2000)));
        assert!(in_wc(&map, (0x4800, 0x5000)));
        assert!(!in_wc(&map, (0x1800, 0x2800)), "past its end");
        assert!(!in_wc(&map, (0x800, 0x1800)), "before its start");
        assert!(!in_wc(&map, (0x3000, 0x3800)), "between two");
    }

    #[test]
    fn a_later_reservation_takes_the_place_of_those_it_overlaps() {
        let mut map = BTreeMap::new();
        note(&mut map, 0x1000, 0x1000, true);
        note(&mut map, 0x4000, 0x2000, true);
        note(&mut map, 0x8000, 0x1000, true);
        note(&mut map, 0x1800, 0x3000, false);
        assert!(!in_wc(&map, (0x1000, 0x1100)), "overlapped at its end");
        assert!(!in_wc(&map, (0x4800, 0x5000)), "overlapped at its start");
        assert!(in_wc(&map, (0x8000, 0x9000)), "not overlapped");
        note(&mut map, 0x8000, 0x1000, false);
        assert!(map.is_empty());
        note(&mut map, 0x2000, 0x1000, true);
        note(&mut map, 0x3000, 0x1000, false);
        assert!(in_wc(&map, (0x2000, 0x3000)), "next to it, not over it");
    }
}
