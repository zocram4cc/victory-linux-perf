use super::*;
use std::collections::BTreeSet;

static LOW: AtomicU64 = AtomicU64::new(u64::MAX);
static HIGH: AtomicU64 = AtomicU64::new(0);
static BITS: [AtomicU64; SPACE.1 / CELL / 64] = [const { AtomicU64::new(0) }; SPACE.1 / CELL / 64];
static REQUESTS: AtomicU64 = AtomicU64::new(0);
static SEEN: Mutex<BTreeSet<(&'static str, u64, u64)>> = Mutex::new(BTreeSet::new());

pub(super) fn registered(at: usize, len: usize, set: bool) {
    let end = at.saturating_add(len).min(SPACE.1);
    if set {
        LOW.fetch_min(at as u64, Ordering::Release);
        HIGH.fetch_max(end as u64, Ordering::Release);
    }
    for n in at / CELL..end.div_ceil(CELL) {
        let bit = 1 << (n % 64);
        if set { BITS[n / 64].fetch_or(bit, Ordering::Release); } else { BITS[n / 64].fetch_and(!bit, Ordering::Release); }
    }
}

pub(crate) fn cpu_ask(at: u64, len: u64, site: &'static str) -> bool {
    let low = LOW.load(Ordering::Acquire);
    if low == u64::MAX || at == 0 { return false; }
    let end = at.saturating_add(len.max(1)).min(HIGH.load(Ordering::Acquire));
    let base = at.max(low);
    if base >= end || !(base as usize / CELL..(end as usize).div_ceil(CELL)).any(|n| BITS[n / 64].load(Ordering::Acquire) & (1 << (n % 64)) != 0) { return false; }
    let Some(um) = get() else { return false; };
    let state = um.state();
    let found = state.cpu_spans.range(..end as usize).next_back().is_some_and(|(&start,&size)| start.saturating_add(size) as u64 > base);
    drop(state);
    if found {
        REQUESTS.fetch_add(1, Ordering::Relaxed);
        if SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert((site, at, len)) {
            tracing::info!(site, address = format_args!("{at:#x}"), bytes = len, "graphics request inside a CPU-only reservation");
        }
    }
    found
}

pub(crate) fn cpu_pointer(at: u64, len: u64, site: &'static str) {
    if !cpu_ask(at, len, site) { return; }
    let Some(um) = get() else { return; };
    let mut state = um.state();
    let end = at.saturating_add(len.max(1)).min(SPACE.1 as u64) as usize;
    let affected: Vec<_> = state.cpu_spans.range(..end).filter(|(b,n)| b.saturating_add(**n) > at as usize).map(|(&b,&n)| (b,n)).collect();
    for (base, bytes) in affected {
        if state.spans.insert(base, bytes).is_none() { recover(&state, base, bytes); }
        let (start, stop) = ((at as usize).max(base), end.min(base + bytes));
        for chunk in (start - SPACE.0) / STRIDE..=(stop - 1 - SPACE.0) / STRIDE {
            if mirror(&mut state, chunk).is_none() {
                crate::host::fail(format_args!("could not preserve shader address {at:#x} in a CPU-only reservation"));
            }
        }
    }
}

pub(super) fn report() {
    if LOW.load(Ordering::Relaxed) != u64::MAX {
        tracing::info!(requests = REQUESTS.load(Ordering::Relaxed), unique = SEEN.lock().unwrap_or_else(|e| e.into_inner()).len(), "graphics requests inside CPU-only reservations");
    }
}
