//! Write-combining title memory left on the card (`VICTORY_SKIP_WC_BACK`, on by default here).
//!
//! Memory the title reserves write-combined (console protection bit 0x400) is memory its CPU
//! writes and does not read back: what shaders write there need not be fetched back from the
//! card's copy. Buffers under `VICTORY_SKIP_WC_MIN_KB` (1024 by default) are still fetched:
//! skipping the small ones left rooms dark and frames black.
//!
//! Port of fxceless/victory-fork#868 by Chipsraine (reservation fix by iGoldenNinja), with the
//! 1 MiB gate from Proudnoob4's fxceless/victory-fork#1268, as in PastiiJay/klbf-lab
//! 0b779d5 + 6488e18 (here without the census lines and the compare skip).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Title reservations asked write-combined: base -> length.
static WC: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());

static SKIPPED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

fn setting(names: [&str; 2]) -> Option<String> {
    names.iter().find_map(|n| std::env::var(n).ok()).map(|v| v.trim().to_string())
}

/// `VICTORY_SKIP_WC_BACK` / `KLBF_SKIP_WC_BACK`: on unless `0`.
pub(crate) fn skip_wc_back() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = setting(["KLBF_SKIP_WC_BACK", "VICTORY_SKIP_WC_BACK"]).is_none_or(|v| v != "0");
        if on {
            tracing::info!("write-combining title memory of 1 MiB and over is not fetched back from the card (VICTORY_SKIP_WC_BACK=0 to fetch it)");
        }
        on
    })
}

/// Whether a buffer of `len` bytes is at least `VICTORY_SKIP_WC_MIN_KB` (1024 KiB by default).
fn big_enough(len: u64) -> bool {
    static MIN: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let min = *MIN.get_or_init(|| {
        setting(["KLBF_SKIP_WC_MIN_KB", "VICTORY_SKIP_WC_MIN_KB"])
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1024)
            .saturating_mul(1024)
    });
    len >= min
}

/// The title reserved `[at, at + len)` with console protection `protect`.
pub fn title_memory_reserved(at: usize, len: usize, protect: u32) {
    if protect & 0x400 == 0 || at == 0 || len == 0 || !skip_wc_back() {
        return;
    }
    WC.lock().unwrap_or_else(|e| e.into_inner()).insert(at, len);
}

fn in_wc(map: &BTreeMap<usize, usize>, (from, to): (usize, usize)) -> bool {
    map.range(..=from).next_back().is_some_and(|(&base, &len)| base.checked_add(len).is_some_and(|end| to <= end))
}

/// Whether a piece of `bytes` of a copy (of a `buffer_len`-byte title buffer) whose shaders
/// declared `declared` written is left on the card: all of `declared` is write-combining
/// memory and the buffer is big enough.
pub(crate) fn skip_back(buffer_len: u64, declared: (usize, usize), bytes: u64) -> bool {
    if !skip_wc_back() || !big_enough(buffer_len) {
        return false;
    }
    let wc = in_wc(&WC.lock().unwrap_or_else(|e| e.into_inner()), declared);
    if wc {
        let n = SKIPPED[0].fetch_add(1, Ordering::Relaxed) + 1;
        let b = SKIPPED[1].fetch_add(bytes, Ordering::Relaxed) + bytes;
        if n.is_power_of_two() {
            tracing::info!("VICTORY_SKIP_WC_BACK: {n} pieces ({} MB) of write-combining memory left on the card so far", b >> 20);
        }
    }
    wc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_write_combining_only_inside_one_reservation() {
        let map = BTreeMap::from([(0x1000, 0x1000), (0x4000, 0x2000)]);
        assert!(in_wc(&map, (0x1000, 0x2000)));
        assert!(in_wc(&map, (0x4800, 0x5000)));
        assert!(!in_wc(&map, (0x1800, 0x2800)), "past its end");
        assert!(!in_wc(&map, (0x800, 0x1800)), "before its start");
        assert!(!in_wc(&map, (0x3000, 0x3800)), "between two");
    }
}
