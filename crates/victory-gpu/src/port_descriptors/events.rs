use crate::watch::PAGE;
use std::collections::BTreeSet;
use std::sync::atomic::{
    AtomicU64, AtomicUsize,
    Ordering::{Acquire, Relaxed, Release},
};
use std::sync::{Mutex, OnceLock};

static START: AtomicUsize = AtomicUsize::new(0);
static END: AtomicUsize = AtomicUsize::new(0);
static DIRTY: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());
static ROUNDS: AtomicU64 = AtomicU64::new(0);
static BLOCKS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        super::on()
            && !crate::records::on()
            && crate::watch::port_guard()
            && crate::watch::view_guard()
            && !super::early_audit()
            && std::env::var_os("VICTORY_DESCRIPTOR_EVENTS").is_none_or(|x| x != "0")
    })
}

pub(crate) fn active() -> bool {
    END.load(Acquire) != 0
}

pub(crate) fn bounds() -> Option<(usize, usize)> {
    let end = END.load(Acquire);
    (end != 0).then(|| (START.load(Relaxed), end))
}

pub(crate) fn overlaps(at: usize, len: usize) -> bool {
    bounds().is_some_and(|(lo, hi)| len != 0 && at < hi && at.saturating_add(len) > lo)
}

pub(super) fn attach(at: usize, len: usize) {
    if !enabled() || !at.is_multiple_of(PAGE) || !len.is_multiple_of(PAGE) || len == 0 {
        return;
    }
    let mut dirty = DIRTY.lock().unwrap_or_else(|e| e.into_inner());
    dirty.clear();
    dirty.extend((at..at + len).step_by(PAGE));
    START.store(at, Relaxed);
    END.store(at + len, Release);
    tracing::info!(
        blocks = dirty.len(),
        "descriptor table subscribed to writes"
    );
}

pub(super) fn detach(at: usize) {
    let mut dirty = DIRTY.lock().unwrap_or_else(|e| e.into_inner());
    if START.load(Relaxed) == at {
        END.store(0, Release);
        dirty.clear();
    }
}

pub(crate) fn opened(at: usize, len: usize) {
    if !overlaps(at, len) {
        return;
    }
    let mut dirty = DIRTY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((lo, hi)) = bounds() {
        let start = at.max(lo) & !(PAGE - 1);
        dirty.extend((start..at.saturating_add(len).min(hi)).step_by(PAGE));
    }
}

pub(crate) fn drain(at: usize, ranges: &mut Vec<(usize, usize)>) -> bool {
    let Some((start, end)) = bounds() else {
        return false;
    };
    if at != start {
        return false;
    }
    let dirty = std::mem::take(&mut *DIRTY.lock().unwrap_or_else(|e| e.into_inner()));
    ROUNDS.fetch_add(1, Relaxed);
    if dirty.is_empty() {
        return true;
    }
    let _mapping = crate::watch::title_memory_reading();
    BLOCKS.fetch_add(dirty.len() as u64, Relaxed);
    for page in dirty {
        if let Some((_, hi)) = ranges.last_mut()
            && *hi == page
        {
            *hi += PAGE;
        } else {
            ranges.push((page, page + PAGE));
        }
    }
    if !crate::watch::arm_descriptors(ranges) {
        tracing::warn!(
            start,
            end,
            "descriptor write coverage unavailable; restoring write watch"
        );
        detach(at);
        ranges.clear();
        ranges.push((start, end));
        return false;
    }
    true
}

pub(super) fn line(frames: f64) {
    if active() {
        tracing::info!(
            drains = ROUNDS.swap(0, Relaxed) as f64 / frames,
            invalidated_blocks = BLOCKS.swap(0, Relaxed) as f64 / frames,
            "descriptor write events per frame"
        );
    }
}
