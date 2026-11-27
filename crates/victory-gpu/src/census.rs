//! Census of what the title touches of the memory shaders' writes were
//! left in on the card (`VICTORY_WC_CENSUS=1`, with `VICTORY_WC_NO_BACK=1`).
//!
//! [`crate::wc::skip_back`] leaves shaders' writes into write-combined title
//! memory on the card: title memory there keeps older bytes. That is only
//! right if the title's processor does not read them. This counts what it
//! does touch, after fxceless/victory-fork issue #1169's read census: once
//! a second the 4 KiB pages of those ranges ([`note`]) get `PAGE_GUARD`,
//! and a guard hit there is counted, once per guard, by who touched the
//! page, then let go on (the system took the guard off with the fault).
//!
//! - Reads and writes of the layer's own paths that name themselves
//!   ([`tag`]: copy refreshes, texture refreshes, our copies) count as the
//!   layer's.
//! - Any other touch is counted by the instruction that made it, read or
//!   write; the line names the most frequent ones by module (none: the
//!   title's code; `victory.exe`: our code on a thread no tag names, such as
//!   a call of the title's into the HLE).
//! - A read is a byte the title would have needed copied back (an upper
//!   bound: a whole page counts, and part of a page may lie outside the
//!   ranges, counted apart). A write is the title's own new bytes (the
//!   write watch still sends them up).
//!
//! Guards are set holding the write guards' lock alone, and never on a
//! block those guards keep read-only or one the kernel is writing for the
//! title ([`crate::watch::census_arm`]); the kernel's writes for the title
//! take them off first ([`disarm`]), since the kernel fails on a guard
//! instead of faulting. The handler is put first, after the write guards':
//! a copy of ours that hits a guard goes on, counted as ours.
//!
//! shortcut: a protection change of the title's own (or of the in-place
//! views) between the query and the guard is undone by the guard. Our
//! copies through the kernel (`VICTORY_SAFE_COPY=0`) would fail on a guard:
//! the census then stays off. Measurement only: never on by default.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{OnceLock, RwLock};

const PAGE: usize = 4096;
/// Pages of the 40-bit title address space.
const PAGES: usize = 1 << 28;

/// Whether the census runs.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let asked = std::env::var_os("VICTORY_WC_CENSUS").is_some_and(|v| v == "1");
        if asked && !crate::wc::no_back() {
            tracing::warn!("VICTORY_WC_CENSUS needs VICTORY_WC_NO_BACK=1: off");
            return false;
        }
        if asked && !crate::safecopy::on() {
            tracing::warn!("VICTORY_WC_CENSUS needs our own copies (not VICTORY_SAFE_COPY=0): off");
            return false;
        }
        if asked {
            tracing::warn!(
                "VICTORY_WC_CENSUS: pages of what shaders' writes were left in on the card are guarded to count what the title touches (slower)"
            );
        }
        asked
    })
}

/// Title memory whose shaders' writes are on the card, of all copies.
fn owned() -> &'static RwLock<crate::wc::Ranges> {
    static OWNED: OnceLock<RwLock<crate::wc::Ranges>> = OnceLock::new();
    OWNED.get_or_init(Default::default)
}

/// `[from, to)` of title memory holds older bytes than the card's.
pub(crate) fn note(from: usize, to: usize) {
    if !on() || from >= to {
        return;
    }
    if owned().read().unwrap_or_else(|e| e.into_inner()).covers(from, to) {
        return;
    }
    owned().write().unwrap_or_else(|e| e.into_inner()).add(from, to);
}

/// A copy that held `[from, to)` is gone: not counted there any more, and
/// its guards come off.
pub(crate) fn forget(from: usize, to: usize) {
    if !on() || from >= to {
        return;
    }
    owned().write().unwrap_or_else(|e| e.into_inner()).remove(from, to);
    for page in from / PAGE..to.div_ceil(PAGE) {
        if is_ours(page) {
            set_guard(page * PAGE, PAGE, false);
        }
    }
}

/// One bit per page the census ever guarded: a guard hit there is the
/// census's (never cleared, so that a hit just after a guard came off is
/// still known as ours and let go on).
fn ours() -> &'static [AtomicU64] {
    static OURS: OnceLock<Box<[AtomicU64]>> = OnceLock::new();
    OURS.get_or_init(|| (0..PAGES / 64).map(|_| AtomicU64::new(0)).collect())
}

fn is_ours(page: usize) -> bool {
    page < PAGES && ours()[page / 64].load(Relaxed) & (1u64 << (page % 64)) != 0
}

// Whether this thread's touches of title memory are the layer's ([`tag`]).
thread_local! {
    static TAGGED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// This thread's touches of title memory are the layer's until the result
/// is dropped.
pub(crate) fn tag() -> Tag {
    Tag(TAGGED.with(|t| t.replace(true)))
}

pub(crate) struct Tag(bool);
impl Drop for Tag {
    fn drop(&mut self) {
        TAGGED.with(|t| t.set(self.0));
    }
}

/// Per-second counts, in pages: guards set; touched by a tagged path of
/// ours; untagged reads and writes inside the ranges; untagged touches of a
/// guarded page outside them (its other part); guards the kernel's writes
/// for the title took off.
static COUNT: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
const ARMED: usize = 0;
const LAYER: usize = 1;
const READ: usize = 2;
const WRITE: usize = 3;
const EDGE: usize = 4;
const KERNEL: usize = 5;
/// Since the start: guard passes and the time they took (µs).
static PASSES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// The instructions of untagged touches inside the ranges, with their reads
/// and writes since the start (an open table: a slot is taken once).
const SLOTS: usize = 256;
static RIPS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static RIP_COUNT: [[AtomicU64; 2]; SLOTS] = [const { [const { AtomicU64::new(0) }; 2] }; SLOTS];
/// Touches whose instruction found no slot.
static RIPS_FULL: AtomicU64 = AtomicU64::new(0);

fn count_rip(rip: usize, write: bool) {
    let mut i = rip.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56;
    for _ in 0..SLOTS {
        let slot = i % SLOTS;
        let at = RIPS[slot].load(Relaxed);
        if at == rip || (at == 0 && RIPS[slot].compare_exchange(0, rip, Relaxed, Relaxed).map_or_else(|now| now == rip, |_| true)) {
            RIP_COUNT[slot][write as usize].fetch_add(1, Relaxed);
            return;
        }
        i += 1;
    }
    RIPS_FULL.fetch_add(1, Relaxed);
}

unsafe extern "system" fn on_guard(p: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
    const GUARD: i32 = 0x8000_0001_u32 as i32;
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    // SAFETY: the system passes a valid record.
    let r = unsafe { &*(*p).ExceptionRecord };
    if r.ExceptionCode != GUARD || r.NumberParameters < 2 {
        return CONTINUE_SEARCH;
    }
    let address = r.ExceptionInformation[1];
    if !is_ours(address / PAGE) {
        return CONTINUE_SEARCH;
    }
    // The guard went with the fault: the access goes on when retried.
    if TAGGED.with(std::cell::Cell::get) {
        COUNT[LAYER].fetch_add(1, Relaxed);
        return CONTINUE_EXECUTION;
    }
    // (Not waited for: a writer holds the lock only to add or take out.)
    let inside = owned().try_read().map_or(true, |o| o.contains(address));
    if !inside {
        COUNT[EDGE].fetch_add(1, Relaxed);
        return CONTINUE_EXECUTION;
    }
    let write = r.ExceptionInformation[0] == 1;
    COUNT[if write { WRITE } else { READ }].fetch_add(1, Relaxed);
    count_rip(r.ExceptionAddress as usize, write);
    CONTINUE_EXECUTION
}

/// Before the kernel writes `[at, at + len)` for the title (a file read;
/// called holding the write guards' lock): the census's guards there come
/// off, and none is set again until it is done ([`crate::watch::census_arm`]
/// looks at the kernel's writes).
pub(crate) fn disarm(at: usize, len: usize) {
    if !on() || len == 0 {
        return;
    }
    for page in at / PAGE..at.saturating_add(len).div_ceil(PAGE) {
        if is_ours(page) && set_guard(page * PAGE, PAGE, false) {
            COUNT[KERNEL].fetch_add(1, Relaxed);
        }
    }
}

/// Once a second: guards every page of the ranges that has none now.
pub(crate) fn arm() {
    if !on() {
        return;
    }
    static HANDLER: std::sync::Once = std::sync::Once::new();
    // SAFETY: a handler that lives forever, put first (after the write
    // guards' own, put at startup).
    HANDLER.call_once(|| unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(1, Some(on_guard));
    });
    let began = std::time::Instant::now();
    let ranges: Vec<(usize, usize)> = owned().read().unwrap_or_else(|e| e.into_inner()).iter().collect();
    let mut armed = 0u64;
    crate::watch::census_arm(|busy| {
        for (from, to) in ranges {
            if crate::wc::kind(from) != crate::wc::Kind::WriteCombined {
                continue;
            }
            let (first, end) = (from / PAGE, to.div_ceil(PAGE).min(PAGES));
            let mut page = first;
            while page < end {
                // A run of pages free to guard, cut where the guards are busy.
                while page < end && busy(page * PAGE) {
                    page += 1;
                }
                let start = page;
                while page < end && !busy(page * PAGE) {
                    page += 1;
                }
                if start < page {
                    armed += guard_run(start, page);
                }
            }
        }
    });
    COUNT[ARMED].fetch_add(armed, Relaxed);
    PASSES[0].fetch_add(1, Relaxed);
    PASSES[1].fetch_add(began.elapsed().as_micros() as u64, Relaxed);
}

/// Guards pages `[first, end)` region by region of one protection, where
/// committed, readable and not guarded already. Returns the pages guarded.
fn guard_run(first: usize, end: usize) -> u64 {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualProtect, VirtualQuery,
    };
    let mut armed = 0;
    let mut at = first * PAGE;
    let end = end * PAGE;
    while at < end {
        let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a query of any address; the out struct is ours.
        if unsafe { VirtualQuery(at as _, &mut m, std::mem::size_of_val(&m)) } == 0 {
            return armed;
        }
        let region_end = (m.BaseAddress as usize + m.RegionSize).min(end).max(at + PAGE);
        if m.State == MEM_COMMIT && m.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && m.Protect != 0 {
            let pages = (region_end - at) / PAGE;
            // The bits before the guard: a hit at once is then the census's.
            for page in at / PAGE..region_end / PAGE {
                ours()[page / 64].fetch_or(1u64 << (page % 64), Relaxed);
            }
            let mut old = 0;
            // SAFETY: committed title pages; only their guard bit changes.
            if unsafe { VirtualProtect(at as _, region_end - at, m.Protect | PAGE_GUARD, &mut old) } != 0 {
                armed += pages as u64;
            }
        }
        at = region_end;
    }
    armed
}

/// Puts on or takes off `PAGE_GUARD` at `[at, at + len)`, one region of
/// one protection, keeping the rest of its protection. True if changed.
fn set_guard(at: usize, len: usize, guard: bool) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, VirtualProtect, VirtualQuery,
    };
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a query of any address; the out struct is ours.
    if unsafe { VirtualQuery(at as _, &mut m, std::mem::size_of_val(&m)) } == 0
        || m.State != MEM_COMMIT
        || (m.Protect & PAGE_GUARD != 0) == guard
    {
        return false;
    }
    let len = len.min(m.BaseAddress as usize + m.RegionSize - at);
    let protect = if guard { m.Protect | PAGE_GUARD } else { m.Protect & !PAGE_GUARD };
    let mut old = 0;
    // SAFETY: committed title pages; only their guard bit changes.
    unsafe { VirtualProtect(at as _, len, protect, &mut old) != 0 }
}

/// The module holding `rip` and the offset in it ("title" if none).
fn module_of(rip: usize) -> String {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    // SAFETY: a lookup by address; the handle is not kept counted.
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            rip as *const u16,
            &mut module,
        )
    } != 0;
    if !found {
        return format!("title {rip:#x}");
    }
    let mut name = [0u16; 260];
    // SAFETY: the buffer is ours, its length given.
    let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&name[..n]);
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
    format!("{file}+{:#x}", rip - module as usize)
}

/// With the line of [`crate::wc::log`]: MB a second since the last line,
/// and the instructions that touched the most since the start.
pub(crate) fn log() {
    if !on() {
        return;
    }
    static LAST: OnceLock<std::sync::Mutex<std::time::Instant>> = OnceLock::new();
    let last = LAST.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
    let secs = {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        let secs = last.elapsed().as_secs_f64().max(0.001);
        *last = std::time::Instant::now();
        secs
    };
    let c: Vec<f64> = COUNT.iter().map(|c| c.swap(0, Relaxed) as f64 * PAGE as f64 / 1e6 / secs).collect();
    let ranges = owned().read().unwrap_or_else(|e| e.into_inner()).iter().map(|(s, e)| e - s).sum::<usize>();
    let passes = PASSES[0].load(Relaxed).max(1);
    tracing::info!(
        "census of what shaders' writes were left in on the card (VICTORY_WC_CENSUS), MB a second: {:.1} guarded anew; \
         touched by the title: {:.3} read, {:.3} written; by our tagged paths {:.2}; beside the ranges in their pages {:.3}; \
         guards taken off for the kernel's writes {:.3}; ranges now {} MB, a guard pass {:.1} ms",
        c[ARMED],
        c[READ],
        c[WRITE],
        c[LAYER],
        c[EDGE],
        c[KERNEL],
        ranges >> 20,
        PASSES[1].load(Relaxed) as f64 / passes as f64 / 1000.0
    );
    let mut rips: Vec<(usize, u64, u64)> = (0..SLOTS)
        .filter_map(|i| {
            let rip = RIPS[i].load(Relaxed);
            (rip != 0).then(|| (rip, RIP_COUNT[i][0].load(Relaxed), RIP_COUNT[i][1].load(Relaxed)))
        })
        .collect();
    if rips.is_empty() {
        return;
    }
    rips.sort_by_key(|&(_, r, w)| std::cmp::Reverse(r + w));
    let total_reads: u64 = rips.iter().map(|r| r.1).sum();
    let named: Vec<String> =
        rips.iter().take(12).map(|&(rip, r, w)| format!("{} ({r} read, {w} written)", module_of(rip))).collect();
    tracing::info!(
        "census, since the start: {} instructions touched the ranges ({} page reads in all, {} with no slot); the most: {}",
        rips.len(),
        total_reads,
        RIPS_FULL.load(Relaxed),
        named.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_pages_count_each_touch_once_and_let_it_go_on() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree};
        // SAFETY: fresh pages of our own, freed at the end.
        let base = unsafe { VirtualAlloc(std::ptr::null(), 4 * PAGE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) } as usize;
        assert_ne!(base, 0);
        if base / PAGE + 4 > PAGES {
            // SAFETY: as above.
            unsafe { VirtualFree(base as _, 0, MEM_RELEASE) };
            return;
        }
        // SAFETY: inside the pages.
        unsafe { (base as *mut u8).write_volatile(7) };
        // Pages 0-2 are the ranges' (page 2 only in part); page 3 is not.
        owned().write().unwrap().add(base, base + 2 * PAGE + 100);
        // SAFETY: a handler that lives forever.
        unsafe { windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(1, Some(on_guard)) };
        let before: Vec<u64> = COUNT.iter().map(|c| c.load(Relaxed)).collect();
        assert_eq!(guard_run(base / PAGE, base / PAGE + 3), 3);
        // SAFETY: inside the pages; each access faults once at most.
        unsafe {
            assert_eq!((base as *const u8).read_volatile(), 7);
            assert_eq!((base as *const u8).read_volatile(), 7);
            ((base + PAGE) as *mut u8).write_volatile(9);
            assert_eq!(((base + PAGE) as *const u8).read_volatile(), 9);
            let _tag = tag();
            ((base + 2 * PAGE + 200) as *const u8).read_volatile();
        }
        let after: Vec<u64> = COUNT.iter().map(|c| c.load(Relaxed)).collect();
        let d = |i: usize| after[i] - before[i];
        assert_eq!((d(READ), d(WRITE), d(LAYER), d(EDGE)), (1, 1, 1, 0));
        // A page out of the ranges' bytes, untagged, counts beside them.
        assert_eq!(guard_run(base / PAGE + 2, base / PAGE + 3), 1);
        // SAFETY: as above.
        unsafe { ((base + 2 * PAGE + 200) as *const u8).read_volatile() };
        assert_eq!(COUNT[EDGE].load(Relaxed) - after[EDGE], 1);
        owned().write().unwrap().remove(base, base + 4 * PAGE);
        // SAFETY: as above.
        unsafe { VirtualFree(base as _, 0, MEM_RELEASE) };
    }
}
