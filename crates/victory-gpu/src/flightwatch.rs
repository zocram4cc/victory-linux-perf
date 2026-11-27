//! Diagnostics (`VICTORY_FLIGHT_WRITE_WATCH=<title address>[,<address>...]`;
//! off unless set; builds with diagnostics): who writes title memory of a
//! copy in the card's memory while a flight that writes it is in the air, and
//! when.
//!
//! Each address names a copy by the title address it starts at. When a
//! submission declares shader writes to such a copy, the pages of the
//! declared ranges are made read-only right after the submission's refreshes
//! took title memory, and stay so until the first landing of the copy begins.
//! The first write of the title's into each such page faults here: the thread,
//! the code address (module and offset), the address written and the 16 bytes
//! there before are noted, the page is made writable again, and the thread goes
//! on. The bytes after are read when the landing begins. Each write is placed
//! among our events: whether the submission's lists had been sent to the card
//! (ExecuteCommandLists), how many fences were set for the title since the
//! pages were made read-only, and how long after that.
//!
//! What it costs: a change of protection per declared range at submission and
//! at the landing (some 5 us each), and a fault per page the title writes in
//! the air (some 10-20 us each, one per page and flight). It can make the
//! writing thread slower while it writes those pages, and so move its writes
//! a little later; it does not change what is written, what goes up or what
//! lands: pages are writable again before any landing writes them. Memory
//! the guards of `crate::watch` hold is left alone (a protection there is
//! theirs).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const PAGE: usize = 4096;

/// The title addresses of the copies to watch, from the variable's text:
/// hexadecimal, with or without `0x`, separated by commas, spaces or
/// semicolons. What does not read as a number is left out.
pub(crate) fn parse(text: &str) -> Vec<u64> {
    let mut out: Vec<u64> = text
        .split([',', ' ', ';'])
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let s = s.trim();
            let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
            u64::from_str_radix(s, 16).ok()
        })
        .filter(|&a| a != 0)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn watched() -> &'static [u64] {
    static WATCHED: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    WATCHED.get_or_init(|| std::env::var("VICTORY_FLIGHT_WRITE_WATCH").map(|v| parse(&v)).unwrap_or_default())
}

/// Whether any copy is watched.
pub(crate) fn on() -> bool {
    !watched().is_empty()
}

/// At most `most` lines per `per`; how many were left out since the last one
/// said.
pub(crate) struct Limit {
    most: u32,
    per: std::time::Duration,
    window: Option<std::time::Instant>,
    said: u32,
    left_out: u64,
}

impl Limit {
    pub(crate) const fn new(most: u32, per: std::time::Duration) -> Self {
        Limit { most, per, window: None, said: 0, left_out: 0 }
    }

    /// Whether a line may be said at `now`; with it, the lines left out
    /// before it.
    pub(crate) fn may(&mut self, now: std::time::Instant) -> Option<u64> {
        match self.window {
            Some(at) if now.duration_since(at) < self.per => {}
            _ => {
                self.window = Some(now);
                self.said = 0;
            }
        }
        if self.said >= self.most {
            self.left_out += 1;
            return None;
        }
        self.said += 1;
        Some(std::mem::take(&mut self.left_out))
    }
}

/// A write caught.
struct Caught {
    thread: u32,
    rip: usize,
    at: usize,
    before: [u8; 16],
    /// Whether the submission's lists had gone to the card.
    sent: bool,
    /// Fences set for the title since the pages were made read-only.
    fences: u64,
    after_us: u64,
}

/// Pages read-only for a copy (title addresses of 4 KiB pages), since when,
/// for which submission, whether its lists were sent, and what was caught.
struct Armed {
    copy: u64,
    submission: u64,
    since: std::time::Instant,
    fences: u64,
    sent: bool,
    pages: std::collections::BTreeSet<usize>,
    /// Pages left writable: another protection than read-write was found.
    skipped: usize,
    caught: Vec<Caught>,
}

static ARMED: Mutex<Vec<Armed>> = Mutex::new(Vec::new());
/// Whether anything is armed (looked at before the lock).
static ANY: AtomicBool = AtomicBool::new(false);
/// Fences set for the title, since the start.
static FENCES: AtomicU64 = AtomicU64::new(0);

/// A fence of the title's was set (`crate::fence::set`).
pub(crate) fn fence_set() {
    if on() {
        FENCES.fetch_add(1, Ordering::Relaxed);
    }
}

fn protect(at: usize, len: usize, readonly: bool) -> bool {
    use windows_sys::Win32::System::Memory::{PAGE_READONLY, PAGE_READWRITE, VirtualProtect};
    let mut old = 0;
    // SAFETY: title memory, committed (checked by the caller); only its
    // protection changes.
    unsafe { VirtualProtect(at as *const _, len, if readonly { PAGE_READONLY } else { PAGE_READWRITE }, &mut old) != 0 }
}

fn read_write(at: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualQuery};
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
    n != 0 && info.State == MEM_COMMIT && info.Protect == PAGE_READWRITE
}

/// After a submission's refreshes, before its lists go to the card: of the
/// copies watched, the declared ranges (title addresses) are made read-only.
pub(crate) fn arm(copy: u64, ranges: &[(usize, usize)], submission: u64) {
    if !watched().contains(&copy) {
        return;
    }
    install();
    let mut pages = std::collections::BTreeSet::new();
    let mut skipped = 0;
    for &(from, to) in ranges {
        let mut at = from / PAGE * PAGE;
        while at < to {
            if crate::watch::guards_any(at, PAGE) || !read_write(at) {
                skipped += 1;
            } else if protect(at, PAGE, true) {
                pages.insert(at);
            }
            at += PAGE;
        }
    }
    let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
    // A flight of the copy that never landed (its shaders changed nothing)
    // gives its pages back after two seconds.
    let (stale, keep): (Vec<Armed>, Vec<Armed>) = std::mem::take(&mut *armed)
        .into_iter()
        .partition(|a| a.copy == copy && a.since.elapsed() > std::time::Duration::from_secs(2));
    *armed = keep;
    for a in &stale {
        for &page in &a.pages {
            protect(page, PAGE, false);
        }
    }
    // Pages another flight of the copy holds already stay with it.
    for a in armed.iter().filter(|a| a.copy == copy) {
        pages.retain(|p| !a.pages.contains(p));
    }
    armed.push(Armed {
        copy,
        submission,
        since: std::time::Instant::now(),
        fences: FENCES.load(Ordering::Relaxed),
        sent: false,
        pages,
        skipped,
        caught: Vec::new(),
    });
    ANY.store(true, Ordering::Release);
    drop(armed);
    for a in stale {
        say(a);
    }
}

/// A submission's lists went to the card.
pub(crate) fn sent() {
    if !ANY.load(Ordering::Acquire) {
        return;
    }
    for a in ARMED.lock().unwrap_or_else(|e| e.into_inner()).iter_mut() {
        a.sent = true;
    }
}

/// A landing of `copy` begins: its pages are made writable again, and what
/// was caught is said.
pub(crate) fn landing(copy: u64) {
    if !ANY.load(Ordering::Acquire) {
        return;
    }
    let done: Vec<Armed> = {
        let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
        let (done, keep): (Vec<Armed>, Vec<Armed>) = std::mem::take(&mut *armed).into_iter().partition(|a| a.copy == copy);
        *armed = keep;
        ANY.store(!armed.is_empty(), Ordering::Release);
        done
    };
    for a in &done {
        for &page in &a.pages {
            protect(page, PAGE, false);
        }
    }
    for a in done {
        say(a);
    }
}

static LINES: Mutex<Limit> = Mutex::new(Limit::new(40, std::time::Duration::from_secs(5)));

fn say(a: Armed) {
    let now = std::time::Instant::now();
    let mut lines = LINES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(left_out) = lines.may(now) else {
        return;
    };
    let before_sent = a.caught.iter().filter(|c| !c.sent).count();
    let after_fence = a.caught.iter().filter(|c| c.fences > 0).count();
    tracing::info!(
        copy = format_args!("{:#x}", a.copy),
        submission = a.submission,
        pages_read_only = a.pages.len(),
        pages_left = a.skipped,
        caught = a.caught.len(),
        before_lists_sent = before_sent,
        after_a_fence_set = after_fence,
        in_air_ms = a.since.elapsed().as_millis() as u64,
        lines_left_out = left_out,
        "flight write watch: title writes into a copy's pages while a flight that writes them is in the air"
    );
    for c in a.caught.iter().take(6) {
        let Some(left_out) = lines.may(now) else {
            return;
        };
        let mut after = [0u8; 16];
        let pattern = crate::host::read_into(c.at as *const u8, &mut after) && has_pattern(&after);
        tracing::info!(
            copy = format_args!("{:#x}", a.copy),
            thread = c.thread,
            code = %code_at(c.rip),
            at = format_args!("{:#x}", c.at),
            offset = format_args!("{:#x}", c.at as u64 - a.copy),
            before = %hex(&c.before),
            after = %hex(&after),
            pattern,
            lists_sent = c.sent,
            fences_set_since = c.fences,
            us_after_read_only = c.after_us,
            lines_left_out = left_out,
            "  flight write watch: a write"
        );
    }
}

/// Whether the 12-byte record of the corridor's stray sheets lies in `b`.
fn has_pattern(b: &[u8]) -> bool {
    const P: [u8; 12] = [0x00, 0x3c, 0x00, 0xf8, 0xff, 0x03, 0xfe, 0xff, 0x00, 0xf8, 0xff, 0x0f];
    (0..3).any(|k| b.windows(4).any(|w| w == &P[k * 4..k * 4 + 4]))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The module and offset of a code address, or the address.
fn code_at(rip: usize) -> String {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameA,
        GetModuleHandleExA,
    };
    let mut module = std::ptr::null_mut();
    // SAFETY: an address and an out pointer; the count is left as it is.
    let found = unsafe {
        GetModuleHandleExA(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            rip as *const u8,
            &mut module,
        )
    } != 0;
    if !found {
        return format!("{rip:#x}");
    }
    let mut name = [0u8; 260];
    // SAFETY: a module handle and a buffer of its size.
    let n = unsafe { GetModuleFileNameA(module, name.as_mut_ptr(), name.len() as u32) } as usize;
    let name = String::from_utf8_lossy(&name[..n]);
    let short = name.rsplit(['\\', '/']).next().unwrap_or("?");
    format!("{short}+{:#x}", rip - module as usize)
}

/// A write fault at `at` from code at `rip`: a page armed here is noted and
/// made writable again. Whether it was ours.
fn caught(at: usize, rip: usize) -> bool {
    let page = at / PAGE * PAGE;
    // (Not `lock`: a thread of ours that holds the lock does not write
    // title memory; another may be waiting for it, and so may this one.)
    let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(a) = armed.iter_mut().find(|a| a.pages.contains(&page)) else {
        return false;
    };
    let mut before = [0u8; 16];
    let from = at.min(page + PAGE - 16);
    // SAFETY: a page of title memory, readable (read-only by us).
    unsafe { std::ptr::copy_nonoverlapping(from as *const u8, before.as_mut_ptr(), 16) };
    protect(page, PAGE, false);
    a.pages.remove(&page);
    if a.caught.len() < 256 {
        a.caught.push(Caught {
            // SAFETY: no arguments.
            thread: unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
            rip,
            at: from,
            before,
            sent: a.sent,
            fences: FENCES.load(Ordering::Relaxed) - a.fences,
            after_us: a.since.elapsed().as_micros() as u64,
        });
    }
    true
}

fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_write(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        let r = unsafe { &*(*info).ExceptionRecord };
        if !ANY.load(Ordering::Acquire)
            || r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS
            || r.NumberParameters < 2
            || r.ExceptionInformation[0] != 1
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        if caught(r.ExceptionInformation[1], r.ExceptionAddress as usize) {
            EXCEPTION_CONTINUE_EXECUTION
        } else {
            EXCEPTION_CONTINUE_SEARCH
        }
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: the handler touches only the armed pages' protection and its
    // own state. Added after `crate::watch`'s, first: it is asked first.
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_write));
    });
}

/// The ranges watched copies' flights declare, per copy (for [`arm`]).
pub(crate) fn of_declared<'a>(declared: impl Iterator<Item = (u64, &'a [(usize, usize)])>) -> BTreeMap<u64, Vec<(usize, usize)>> {
    let mut out: BTreeMap<u64, Vec<(usize, usize)>> = BTreeMap::new();
    for (copy, ranges) in declared {
        if watched().contains(&copy) {
            out.entry(copy).or_default().extend_from_slice(ranges);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_read_as_hexadecimal_in_any_separation() {
        assert_eq!(parse("0xbffa40000"), [0xbffa40000]);
        assert_eq!(parse("0xbfc840000, BFFA40000;0xbffa40000 junk 0x0"), [0xbfc840000, 0xbffa40000]);
        assert_eq!(parse(""), Vec::<u64>::new());
        assert_eq!(parse("1"), [1]);
    }

    #[test]
    fn lines_are_limited_per_window_and_say_what_was_left_out() {
        let mut l = Limit::new(2, std::time::Duration::from_secs(5));
        let t = std::time::Instant::now();
        assert_eq!(l.may(t), Some(0));
        assert_eq!(l.may(t), Some(0));
        assert_eq!(l.may(t), None);
        assert_eq!(l.may(t + std::time::Duration::from_secs(1)), None);
        // A new window: the first line says how many were left out.
        assert_eq!(l.may(t + std::time::Duration::from_secs(6)), Some(2));
        assert_eq!(l.may(t + std::time::Duration::from_secs(6)), Some(0));
    }

    #[test]
    fn the_record_is_found_in_any_of_its_three_places() {
        let mut b = [0u8; 16];
        assert!(!has_pattern(&b));
        b[4..8].copy_from_slice(&0xfffe03ffu32.to_le_bytes());
        assert!(has_pattern(&b));
    }
}
