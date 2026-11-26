//! The read census of roadmap task 1.3 (`VICTORY_BACK_CENSUS=1`, off by
//! default): how much of what the copy-back writes into title memory does
//! the title's CPU touch before the next submission? See
//! docs/design/p1.3-copyback-on-demand.md.
//!
//! The copy-back's writes (`crate::host::write_mapped`) are noted by page.
//! When a submission's copy-back and its bottom-of-pipe writes are done
//! ([`arm`]), each noted page of private memory gets `PAGE_GUARD` on top of
//! its protection. A guard fault is one-shot: the system takes the guard
//! away and our handler notes the page as touched by the title (read or
//! write) and lets the thread go on. At the start of the next submission
//! ([`lift`]) the guards still there are taken off, before the layer reads
//! title memory again, and the frame line says how many pages were armed
//! and how many the title touched.
//!
//! Our own reads and writes of title memory go through `crate::fastcopy`'s
//! copy instruction; a census guard met there is not the title's: its
//! handler asks [`ours`], which drops the page and lets the copy go on
//! (the fault already took the guard away). Kernel reads (the fallback when
//! a page is missing) would fail on a guarded page, so the census needs the
//! fast reads (`VICTORY_F1_FAST_READ`, on by default) and is off without.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const PAGE: usize = 0x1000;

/// Whether the census runs (`VICTORY_BACK_CENSUS=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_BACK_CENSUS").is_some_and(|v| v == "1" || v == "2")
            && crate::fastcopy::on()
            && !crate::host::relay();
        if on {
            install();
            tracing::info!("read census of the copy-back on (VICTORY_BACK_CENSUS=1)");
        }
        on
    })
}

/// `VICTORY_BACK_CENSUS=2`: view (mapped) memory that is read-write is
/// guarded too. Its aliases are not: a read through another view of the
/// same pages goes unseen, so this counts a lower bound there.
fn views() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_BACK_CENSUS").is_some_and(|v| v == "2"))
}

struct State {
    /// Pages written back since the last arming: page number -> bytes.
    noted: BTreeMap<usize, u32>,
    /// Pages guarded now: page number -> the protection found.
    armed: BTreeMap<usize, u32>,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static S: Mutex<State> = Mutex::new(State { noted: BTreeMap::new(), armed: BTreeMap::new() });
    S.lock().unwrap_or_else(|e| e.into_inner())
}

/// Since the last frame line: pages armed, pages the title read first,
/// pages it wrote first, pages our own copies met, pages skipped (not
/// private, already guarded, or not committed read-write), bytes written
/// back on the armed pages, runs of touched pages, pages lifted untouched.
pub(crate) static STATS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
// [8]: of the skipped, mapped (view) pages that are read-write; [9]: of the
// skipped, pages read-only or otherwise protected (`crate::watch`'s guards).

/// Largest runs of touched pages since the last frame line: (pages, first
/// address), for the log.
static RUNS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Pages touched since the last lift, by page number (to make runs).
static TOUCHED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// The copy-back wrote `[at, at + len)` of title memory.
pub(crate) fn note(at: usize, len: usize) {
    if len == 0 || !on() {
        return;
    }
    let mut s = state();
    let (first, last) = (at / PAGE, (at + len - 1) / PAGE);
    for page in first..=last {
        let lo = (page * PAGE).max(at);
        let hi = ((page + 1) * PAGE).min(at + len);
        *s.noted.entry(page).or_insert(0) += (hi - lo) as u32;
    }
}

/// After a submission's copy-back and bottom-of-pipe writes: guards the
/// pages written back.
pub(crate) fn arm() {
    if !on() {
        return;
    }
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_MAPPED, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_READWRITE, VirtualProtect, VirtualQuery,
    };
    let mut s = state();
    let noted = std::mem::take(&mut s.noted);
    let pages: Vec<(usize, u32)> = noted.into_iter().collect();
    let mut i = 0;
    while i < pages.len() {
        // One query per region: a run of pages with the same state.
        let (page, _) = pages[i];
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { VirtualQuery((page * PAGE) as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            STATS[4].fetch_add(1, Ordering::Relaxed);
            i += 1;
            continue;
        }
        let end = (info.BaseAddress as usize + info.RegionSize) / PAGE;
        let mut j = i;
        while j < pages.len() && pages[j].0 < end {
            j += 1;
        }
        // Private read-write memory only: view memory has our own write
        // guards (`crate::watch`), and a page the title guards or made
        // read-only is left as it is.
        let views = views();
        let fit = info.State == MEM_COMMIT
            && (info.Type == MEM_PRIVATE || (views && info.Type == MEM_MAPPED))
            && info.Protect == PAGE_READWRITE;
        if !fit {
            STATS[4].fetch_add((j - i) as u64, Ordering::Relaxed);
            let reason = if info.State == MEM_COMMIT && info.Type == MEM_MAPPED && info.Protect == PAGE_READWRITE { 8 } else { 9 };
            STATS[reason].fetch_add((j - i) as u64, Ordering::Relaxed);
            i = j;
            continue;
        }
        // Contiguous runs inside the region, one protect each.
        let mut k = i;
        while k < j {
            let mut l = k + 1;
            while l < j && pages[l].0 == pages[l - 1].0 + 1 {
                l += 1;
            }
            let (from, n) = (pages[k].0, pages[l - 1].0 + 1 - pages[k].0);
            // Noted as armed before the guard goes on, so a thread that
            // faults at once finds it.
            for &(p, bytes) in &pages[k..l] {
                s.armed.insert(p, PAGE_READWRITE);
                STATS[5].fetch_add(u64::from(bytes), Ordering::Relaxed);
            }
            let mut old = 0;
            // SAFETY: committed private read-write pages (queried above);
            // only the guard bit is added.
            if unsafe { VirtualProtect((from * PAGE) as *const _, n * PAGE, PAGE_READWRITE | PAGE_GUARD, &mut old) } == 0 {
                for p in from..from + n {
                    s.armed.remove(&p);
                }
                STATS[4].fetch_add(n as u64, Ordering::Relaxed);
            } else {
                STATS[0].fetch_add(n as u64, Ordering::Relaxed);
            }
            k = l;
        }
        i = j;
    }
}

/// At the start of a submission, before the layer reads title memory:
/// takes off the census guards still there.
pub(crate) fn lift() {
    if !on() {
        return;
    }
    use windows_sys::Win32::System::Memory::{PAGE_GUARD, VirtualProtect};
    let mut s = state();
    s.noted.clear();
    // Left in `armed` until their guards are off: a title thread that
    // meets a guard meanwhile must still find its page there, or its guard
    // fault goes on to the title's handlers and ends the run (it did, in
    // all three of the census's first runs: 0x80000001 in title code).
    let pages: Vec<(usize, u32)> = s.armed.iter().map(|(&p, &v)| (p, v)).collect();
    drop(s);
    let mut k = 0;
    while k < pages.len() {
        let mut l = k + 1;
        while l < pages.len() && pages[l].0 == pages[l - 1].0 + 1 && pages[l].1 == pages[k].1 {
            l += 1;
        }
        let (from, n) = (pages[k].0, pages[l - 1].0 + 1 - pages[k].0);
        // Only where our guard is still what the page has: something else
        // (the title, or `crate::watch`) may have changed it since.
        for (at, len) in still_guarded(from, n, pages[k].1 | PAGE_GUARD) {
            let mut old = 0;
            // SAFETY: pages we guarded, still as we left them; their
            // protection goes back to what was found.
            unsafe { VirtualProtect((at * PAGE) as *const _, len * PAGE, pages[k].1, &mut old) };
            STATS[7].fetch_add(len as u64, Ordering::Relaxed);
        }
        k = l;
    }
    // The guards are off: the pages are armed no more.
    let mut s = state();
    for (page, _) in &pages {
        s.armed.remove(page);
    }
    drop(s);
    // Runs of touched pages, for the frame line.
    let mut touched = std::mem::take(&mut *TOUCHED.lock().unwrap_or_else(|e| e.into_inner()));
    touched.sort_unstable();
    touched.dedup();
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let mut k = 0;
    while k < touched.len() {
        let mut l = k + 1;
        while l < touched.len() && touched[l] == touched[l - 1] + 1 {
            l += 1;
        }
        runs.push((l - k, touched[k] * PAGE));
        STATS[6].fetch_add(1, Ordering::Relaxed);
        k = l;
    }
    runs.sort_unstable_by(|a, b| b.cmp(a));
    runs.truncate(6);
}

/// The parts of pages `[from, from + n)` whose protection is `want` now.
fn still_guarded(from: usize, n: usize, want: u32) -> Vec<(usize, usize)> {
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut out = Vec::new();
    let mut page = from;
    while page < from + n {
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { VirtualQuery((page * PAGE) as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        let end = ((info.BaseAddress as usize + info.RegionSize) / PAGE).min(from + n).max(page + 1);
        if info.Protect == want {
            out.push((page, end - page));
        }
        page = end;
    }
    out
}

/// Called by `crate::fastcopy`'s handler for a guard fault at its copy
/// instruction: whether the page was a census guard (dropped then; the
/// copy goes on, the fault took the guard away).
pub(crate) fn ours(at: usize) -> bool {
    if !on() {
        return false;
    }
    if state().armed.remove(&(at / PAGE)).is_some() {
        STATS[3].fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// The address range of our own image (victory.exe), whose code is not
/// the title's.
fn own_image() -> (usize, usize) {
    static RANGE: OnceLock<(usize, usize)> = OnceLock::new();
    *RANGE.get_or_init(|| {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        // SAFETY: our own loaded image; its headers are mapped.
        unsafe {
            let base = GetModuleHandleW(std::ptr::null()) as usize;
            if base == 0 {
                return (0, 0);
            }
            let pe = base + *((base + 0x3c) as *const u32) as usize;
            // SizeOfImage: optional header offset 56, after the 24-byte
            // PE signature and file header.
            let size = *((pe + 24 + 56) as *const u32) as usize;
            (base, base + size)
        }
    })
}

fn install() {
    own_image();
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };
    const STATUS_GUARD_PAGE_VIOLATION: i32 = 0x8000_0001_u32 as i32;
    unsafe extern "system" fn on_guard(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        let r = unsafe { &*(*info).ExceptionRecord };
        if r.ExceptionCode != STATUS_GUARD_PAGE_VIOLATION || r.NumberParameters < 2 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // Our own copies are fastcopy's handler's to settle (`ours`).
        if crate::fastcopy::at_copy(unsafe { (*(*info).ContextRecord).Rip } as usize) {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let at = r.ExceptionInformation[1];
        if state().armed.remove(&(at / PAGE)).is_none() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // A read of the layer's own code (it parses title memory in
        // place): not the title's.
        let rip = unsafe { (*(*info).ContextRecord).Rip } as usize;
        let (lo, hi) = own_image();
        if (lo..hi).contains(&rip) {
            STATS[3].fetch_add(1, Ordering::Relaxed);
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        let write = r.ExceptionInformation[0] == 1;
        STATS[if write { 2 } else { 1 }].fetch_add(1, Ordering::Relaxed);
        TOUCHED.lock().unwrap_or_else(|e| e.into_inner()).push(at / PAGE);
        // The guard is gone: the access is made again and goes through.
        EXCEPTION_CONTINUE_EXECUTION
    }
    // SAFETY: the handler only settles guard faults on pages it armed.
    unsafe {
        AddVectoredExceptionHandler(1, Some(on_guard));
    }
}

/// The frame line: per frame, pages armed and touched.
pub(crate) fn census_line(frames: f64) {
    if !on() {
        return;
    }
    let s: Vec<u64> = STATS.iter().map(|c| c.swap(0, Ordering::Relaxed)).collect();
    let runs = std::mem::take(&mut *RUNS.lock().unwrap_or_else(|e| e.into_inner()));
    let f = |n: u64| n as f64 / frames;
    let touched = s[1] + s[2];
    tracing::info!(
        "read census per frame: {:.0} pages written back and guarded ({:.1} MB written on them), {:.0} skipped ({:.0} view pages read-write, {:.0} protected or other); \
         the title touched {:.1} before the next submission ({:.1} read first, {:.1} written first; {:.1} % of the guarded), \
         in {:.1} runs; the layer's own reads met {:.1}; the largest runs (pages at address): {:x?}",
        f(s[0]),
        f(s[5]) / 1e6,
        f(s[4]),
        f(s[8]),
        f(s[9]),
        f(touched),
        f(s[1]),
        f(s[2]),
        if s[0] > 0 { touched as f64 * 100.0 / s[0] as f64 } else { 0.0 },
        f(s[6]),
        f(s[3]),
        runs,
    );
}
