//! How long textures take to be shown as the title asks (always counted; a
//! line every few seconds with the memory lines): from the moment a title
//! slot of a texture picked by index is written (or its memory is mapped or
//! written) to the moment the pass brought it up to date, apart for slots
//! whose memory was there at the first look and those that waited for it
//! (the title's streaming, or ours of its files); and from an upload's
//! recording to the slot that shows it being made from it, for a slot that
//! showed another level of the texture meanwhile (a sharper level arriving)
//! and for one that showed nothing of it (a texture new to the slot).

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// What is timed.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    /// Slot written to brought up to date, its memory there at once.
    Ready = 0,
    /// The same, after waiting for its memory.
    Waited = 1,
    /// Upload recorded to the slot showing its sharper level.
    Sharper = 2,
    /// Upload recorded to the slot showing a texture new to it.
    New = 3,
    /// A texture first viewed from a smaller level to its first view of its
    /// whole level 0 (the title's streaming, ours of its files, and ours),
    /// or to a list that copied its larger levels from the store being sent.
    Full = 4,
    /// The last look that found a slot's memory away to its upload: how
    /// late its memory's arrival was noticed, at most.
    Noticed = 5,
}

const KINDS: usize = 6;
/// Upper bounds of the buckets, in ms (the last is open).
const BOUNDS: [u64; 14] = [16, 33, 66, 100, 166, 250, 500, 1000, 2000, 4000, 8000, 16000, 32000, u64::MAX];

static COUNTS: [[AtomicU64; BOUNDS.len()]; KINDS] =
    [const { [const { AtomicU64::new(0) }; BOUNDS.len()] }; KINDS];
static SUMS: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];

/// One of `kind` that took from `since` (`crate::host::now_ms`) to now.
pub(crate) fn note(kind: Kind, since: u64) {
    if since == 0 {
        return;
    }
    let ms = crate::host::now_ms().saturating_sub(since);
    let bucket = BOUNDS.iter().position(|&b| ms < b).unwrap_or(BOUNDS.len() - 1);
    COUNTS[kind as usize][bucket].fetch_add(1, Relaxed);
    SUMS[kind as usize].fetch_add(ms, Relaxed);
}

/// Textures viewed from a level other than 0 so far: since when, and from
/// which level.
static PARTLY: std::sync::Mutex<Option<std::collections::HashMap<usize, u64>>> = std::sync::Mutex::new(None);

/// A view of placed texture `texture` from level `first` is made.
pub(crate) fn viewed(texture: usize, first: u32) {
    let mut partly = PARTLY.lock().unwrap_or_else(|e| e.into_inner());
    let partly = partly.get_or_insert_with(std::collections::HashMap::new);
    if first == 0 {
        if let Some(since) = partly.remove(&texture) {
            note(Kind::Full, since);
        }
    } else if partly.len() < 1 << 20 {
        partly.entry(texture).or_insert_with(crate::host::now_ms);
    }
}

/// Texture `texture`'s larger levels were copied from the store (the list
/// that did it has been sent): shown whole from now on.
pub(crate) fn whole(texture: usize) {
    let since = PARTLY.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|p| p.remove(&texture));
    if let Some(since) = since {
        note(Kind::Full, since);
    }
}

/// A texture is let go: no longer waited for.
pub(crate) fn gone(texture: usize) {
    if let Some(partly) = PARTLY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        partly.remove(&texture);
    }
}

/// The bucket bound under which `share` of `counts` lie.
fn under(counts: &[u64], share: f64) -> String {
    let all: u64 = counts.iter().sum();
    if all == 0 {
        return "-".into();
    }
    let want = (all as f64 * share).ceil() as u64;
    let mut seen = 0;
    for (i, &c) in counts.iter().enumerate() {
        seen += c;
        if seen >= want {
            return match BOUNDS[i] {
                u64::MAX => ">32 s".into(),
                b => format!("<{b} ms"),
            };
        }
    }
    "-".into()
}

/// The line: per kind, how many since the start, their mean, and under
/// which bound half, nine tenths and 99 in 100 of them were.
pub(crate) fn line() -> String {
    const NAMES: [&str; KINDS] = [
        "written to shown up to date, memory there",
        "the same after waiting for memory",
        "upload to a sharper level shown",
        "upload to a new texture shown",
        "first viewed in part to shown whole",
        "last look that found the memory away to the upload",
    ];
    let mut parts = Vec::new();
    for (k, name) in NAMES.iter().enumerate() {
        let counts: Vec<u64> = COUNTS[k].iter().map(|c| c.load(Relaxed)).collect();
        let n: u64 = counts.iter().sum();
        let mean = SUMS[k].load(Relaxed) as f64 / n.max(1) as f64;
        parts.push(format!(
            "{name}: {n}, mean {mean:.0} ms, half {}, 90% {}, 99% {}",
            under(&counts, 0.5),
            under(&counts, 0.9),
            under(&counts, 0.99)
        ));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_by_bucket() {
        let counts = [0, 5, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        assert_eq!(under(&counts, 0.5), "<33 ms");
        assert_eq!(under(&counts, 0.9), "<100 ms");
        assert_eq!(under(&counts, 0.99), ">32 s");
        assert_eq!(under(&[0; 14], 0.5), "-");
    }
}
