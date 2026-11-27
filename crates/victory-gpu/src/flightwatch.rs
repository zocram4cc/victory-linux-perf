//! Diagnostics (`VICTORY_FLIGHT_WRITE_WATCH=<title address>[,<address>...]`;
//! off unless set; builds with diagnostics): who writes title memory of a
//! copy in the card's memory while a flight that writes it is in the air, and
//! when.
//!
//! Each address names a copy by the title address it starts at. When a
//! submission declares shader writes to such a copy, the pages of the
//! declared ranges are made read-only right after the submission's refreshes
//! took title memory. The watch lasts until the copy's next refresh: a write
//! is placed before the lists were sent, in the air, or after the flight's
//! first landing (then: whether its block is one the landing wrote into).
//! Pages made read-only here are opened when the landing begins (it writes
//! them); after it, only the guards' blocks a landing left read-only tell.
//! Pages the write watch's guards hold (memory the kernel cannot write-watch:
//! a copy leaves each 64 KiB block it took read-only, and their handler takes
//! the first write) are not touched: their faults are looked at instead, a
//! block at a time ([`guard_fault`]). The line of each flight says how many
//! pages were made read-only, how many blocks were left to the guards, and
//! why any others were left (not committed, another protection, held by
//! another flight): a watch that watches nothing says so.
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
    /// When it came, by our events.
    window: Window,
    /// Of a write after the landing: whether its 64 KiB block was one the
    /// flight just landed wrote into.
    in_landed: bool,
    /// The 16 bytes after, read when the flight landed (for a write before
    /// that: what the title had put there, before any landing's bytes).
    after: Option<[u8; 16]>,
    /// The address the title wrote through (another view of the same page
    /// than the copy's `at`, or `at`).
    through: usize,
}

/// Since the start: the guards' faults on title writes while a flight was
/// watched, and those that were a watched block's (at any of its views).
static GUARD_FAULTS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// When a write came, by our events.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Window {
    /// Before the submission's lists went to the card.
    BeforeSent,
    /// While the flight was in the air: sent, not landed.
    InAir,
    /// After the flight's first landing, before the next refresh of the copy.
    AfterLanding,
}

/// The window of a write, by whether the lists were sent and the flight
/// landed.
pub(crate) fn window_of(sent: bool, landed: bool) -> Window {
    match (sent, landed) {
        (_, true) => Window::AfterLanding,
        (true, false) => Window::InAir,
        (false, false) => Window::BeforeSent,
    }
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
    /// The 64 KiB blocks of the declared ranges the guards of `crate::watch`
    /// hold: their faults tell the title's first write after a refresh took
    /// the block ([`guard_fault`]). Each block is told once.
    guarded: std::collections::BTreeSet<usize>,
    /// Pages left alone, and why.
    left: Left,
    caught: Vec<Caught>,
    /// When the flight's first landing began, and the 64 KiB blocks its
    /// landings wrote into.
    landed: Option<std::time::Instant>,
    landed_blocks: std::collections::BTreeSet<usize>,
    /// As armed: pages made read-only here, blocks left to the guards.
    armed: (usize, usize),
}

/// What a page of a declared range is to the watch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PageIs {
    /// Read-write and not the guards': made read-only here.
    Ours,
    /// The guards': their fault is looked at.
    Guarded,
    /// Not committed.
    Absent,
    /// Committed with another protection than read-write, and not the
    /// guards' (a protection of someone else's).
    Other,
}

/// Pages left alone, by why.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct Left {
    pub(crate) absent: usize,
    pub(crate) other: usize,
    /// Held by another flight of the copy already.
    pub(crate) held: usize,
}

/// The pages of `ranges` (title addresses) the watch makes read-only, the
/// 64 KiB blocks it looks at the guards' faults for, and the pages left, by
/// `is`, which says what a page is.
pub(crate) fn plan(
    ranges: &[(usize, usize)],
    is: impl Fn(usize) -> PageIs,
) -> (std::collections::BTreeSet<usize>, std::collections::BTreeSet<usize>, Left) {
    let (mut ours, mut guarded, mut left) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new(), Left::default());
    for &(from, to) in ranges {
        let mut at = from / PAGE * PAGE;
        while at < to {
            match is(at) {
                PageIs::Ours => drop(ours.insert(at)),
                PageIs::Guarded => drop(guarded.insert(at / crate::watch::PAGE)),
                PageIs::Absent => left.absent += 1,
                PageIs::Other => left.other += 1,
            }
            at += PAGE;
        }
    }
    (ours, guarded, left)
}

/// What the page at `at` is now.
fn page_is(at: usize) -> PageIs {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualQuery};
    if crate::watch::guards_any(at, PAGE) {
        return PageIs::Guarded;
    }
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
    if n == 0 || info.State != MEM_COMMIT {
        PageIs::Absent
    } else if info.Protect == PAGE_READWRITE {
        PageIs::Ours
    } else {
        PageIs::Other
    }
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

/// After a submission's refreshes, before its lists go to the card: of the
/// copies watched, the declared ranges (title addresses) are made read-only.
pub(crate) fn arm(copy: u64, ranges: &[(usize, usize)], submission: u64) {
    if !watched().contains(&copy) {
        return;
    }
    install();
    let (mut pages, guarded, mut left) = plan(ranges, page_is);
    let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
    // A flight of the copy that never landed (its shaders changed nothing)
    // gives its pages back after two seconds.
    let (stale, keep): (Vec<Armed>, Vec<Armed>) = std::mem::take(&mut *armed)
        .into_iter()
        .partition(|a| a.copy == copy && (a.landed.is_some() || a.since.elapsed() > std::time::Duration::from_secs(2)));
    *armed = keep;
    for a in &stale {
        for &page in &a.pages {
            protect(page, PAGE, false);
        }
    }
    // Pages another flight of the copy holds already stay with it.
    for a in armed.iter().filter(|a| a.copy == copy) {
        let before = pages.len();
        pages.retain(|p| !a.pages.contains(p));
        left.held += before - pages.len();
    }
    pages.retain(|&p| protect(p, PAGE, true));
    armed.push(Armed {
        copy,
        submission,
        since: std::time::Instant::now(),
        fences: FENCES.load(Ordering::Relaxed),
        sent: false,
        pages,
        guarded,
        left,
        caught: Vec::new(),
        landed: None,
        landed_blocks: std::collections::BTreeSet::new(),
        armed: (0, 0),
    });
    if let Some(a) = armed.last_mut() {
        a.armed = (a.pages.len(), a.guarded.len());
    }
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

/// A landing of `copy` begins, into title memory `[from, to)`: the pages
/// made read-only here are made writable again (the landing writes them),
/// the flight is marked landed, and from here until the copy's next refresh
/// ([`refreshed`]) what is caught counts as after the landing. Blocks the
/// guards hold stay theirs: a landing put beside the guards leaves them
/// read-only and the title's next write faults; one put through a fault
/// opens them, and later writes there are not seen.
pub(crate) fn landing(copy: u64, from: usize, to: usize) {
    if !ANY.load(Ordering::Acquire) {
        return;
    }
    let now = std::time::Instant::now();
    let mut opened = Vec::new();
    {
        let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
        // The landings of one flight come in a burst: a flight of the copy
        // that began landing in the last 50 ms takes these too; else the
        // oldest flight of the copy not landed yet lands now.
        let recent = armed
            .iter()
            .rposition(|a| a.copy == copy && a.landed.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(50)));
        let at = recent.or_else(|| armed.iter().position(|a| a.copy == copy && a.landed.is_none()));
        let Some(at) = at else {
            return;
        };
        let a = &mut armed[at];
        if a.landed.is_none() {
            a.landed = Some(now);
            opened.extend(std::mem::take(&mut a.pages));
            for c in a.caught.iter_mut().filter(|c| c.after.is_none()) {
                let mut after = [0u8; 16];
                if crate::host::read_into(c.at as *const u8, &mut after) {
                    c.after = Some(after);
                }
            }
        }
        if from < to {
            let (first, last) = (from / crate::watch::PAGE, (to - 1) / crate::watch::PAGE);
            a.landed_blocks.extend(first..=last);
        }
    }
    for page in opened {
        protect(page, PAGE, false);
    }
}

/// A refresh of `copy` begins: the flights of it that landed are done, and
/// what was caught is said.
pub(crate) fn refreshed(copy: u64) {
    if !ANY.load(Ordering::Acquire) {
        return;
    }
    let done: Vec<Armed> = {
        let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
        let (done, keep): (Vec<Armed>, Vec<Armed>) =
            std::mem::take(&mut *armed).into_iter().partition(|a| a.copy == copy && a.landed.is_some());
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
    let count = |w: Window| a.caught.iter().filter(|c| c.window == w).count();
    let after_fence = a.caught.iter().filter(|c| c.fences > 0).count();
    let after_landing: Vec<&Caught> = a.caught.iter().filter(|c| c.window == Window::AfterLanding).collect();
    let read_after = |c: &Caught| {
        c.after.or_else(|| {
            let mut b = [0u8; 16];
            crate::host::read_into(c.at as *const u8, &mut b).then_some(b)
        })
    };
    let landing_pattern = after_landing.iter().filter(|c| read_after(c).is_some_and(|b| has_pattern(&b))).count();
    let landing_in_landed = after_landing.iter().filter(|c| c.in_landed).count();
    tracing::info!(
        copy = format_args!("{:#x}", a.copy),
        submission = a.submission,
        watching = if a.armed == (0, 0) { "NOTHING: every page left" } else { "yes" },
        pages_read_only_here = a.armed.0,
        blocks_by_the_guards = a.armed.1,
        left_not_committed = a.left.absent,
        left_other_protection = a.left.other,
        left_held_by_another_flight = a.left.held,
        caught = a.caught.len(),
        before_lists_sent = count(Window::BeforeSent),
        in_the_air = count(Window::InAir),
        after_landing_before_refresh = count(Window::AfterLanding),
        after_landing_with_record = landing_pattern,
        after_landing_in_blocks_it_landed = landing_in_landed,
        after_a_fence_set = after_fence,
        landed = a.landed.is_some(),
        blocks_landed = a.landed_blocks.len(),
        armed_ms = a.since.elapsed().as_millis() as u64,
        guard_faults_while_watched = GUARD_FAULTS[0].load(Ordering::Relaxed),
        of_them_a_watched_blocks = GUARD_FAULTS[1].load(Ordering::Relaxed),
        lines_left_out = left_out,
        "flight write watch: title writes into a copy's pages from a refresh to the next, around a flight that writes them"
    );
    // A few of each window.
    let shown: Vec<&Caught> = [Window::BeforeSent, Window::InAir, Window::AfterLanding]
        .into_iter()
        .flat_map(|w| a.caught.iter().filter(move |c| c.window == w).take(3))
        .collect();
    for c in shown {
        let Some(left_out) = lines.may(now) else {
            return;
        };
        let after = read_after(c).unwrap_or_default();
        let pattern = has_pattern(&after);
        tracing::info!(
            copy = format_args!("{:#x}", a.copy),
            thread = c.thread,
            code = %code_at(c.rip),
            at = format_args!("{:#x}", c.at),
            through = format_args!("{:#x}", c.through),
            offset = format_args!("{:#x}", c.at as u64 - a.copy),
            before = %hex(&c.before),
            after = %hex(&after),
            pattern,
            window = ?c.window,
            in_blocks_landed = c.in_landed,
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
            window: window_of(a.sent, a.landed.is_some()),
            in_landed: a.landed_blocks.contains(&(at / crate::watch::PAGE)),
            after: None,
            through: at,
        });
    }
    true
}

/// The guards of `crate::watch` took a write of the title's at `at` from
/// code at `rip` to a block they held, and opened it (the write has not
/// happened yet). Noted if a flight of a watched copy declared that block:
/// once a block and flight.
pub(crate) fn guard_fault(at: usize, rip: usize) {
    if !ANY.load(Ordering::Acquire) {
        return;
    }
    GUARD_FAULTS[0].fetch_add(1, Ordering::Relaxed);
    let block = crate::watch::PAGE;
    // The title shows a page at several addresses and may write it through
    // another than the copy's: each of them is looked at.
    let within = at % block;
    let mut at_views = vec![at];
    at_views.extend(
        crate::inplace::pages_of(at / block * block).into_iter().map(|view| view + within).filter(|&v| v != at),
    );
    let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
    let found = armed.iter_mut().find_map(|a| {
        let view = at_views.iter().copied().find(|v| a.guarded.contains(&(v / block)))?;
        Some((a, view))
    });
    let Some((a, view)) = found else {
        return;
    };
    GUARD_FAULTS[1].fetch_add(1, Ordering::Relaxed);
    a.guarded.remove(&(view / block));
    let page = view / PAGE * PAGE;
    let from = view.min(page + PAGE - 16);
    let mut before = [0u8; 16];
    crate::host::read_into(from as *const u8, &mut before);
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
            window: window_of(a.sent, a.landed.is_some()),
            in_landed: a.landed_blocks.contains(&(view / block)),
            after: None,
            through: at,
        });
    }
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

    /// A copy the write watch's guards hold whole is still watched: through
    /// their faults, a block at a time (the first run of the watch left
    /// every page of 0xbffa40000 alone and caught nothing).
    #[test]
    fn a_copy_the_guards_hold_is_watched_through_their_faults() {
        let block = crate::watch::PAGE;
        let from = 0xbffa40000usize;
        let to = from + 0x1900000;
        let (ours, guarded, left) = plan(&[(from, to)], |_| PageIs::Guarded);
        assert!(ours.is_empty());
        assert_eq!(guarded.len(), 0x1900000 / block, "every block of the range");
        assert_eq!(left, Left::default());
        assert!(!guarded.is_empty(), "a dead watch: nothing armed on a copy the guards hold");
        // Mixed: ours, the guards', absent, another protection.
        let (ours, guarded, left) = plan(&[(from + 100, from + 4 * PAGE)], |at| match (at - from) / PAGE {
            0 => PageIs::Ours,
            1 => PageIs::Guarded,
            2 => PageIs::Absent,
            _ => PageIs::Other,
        });
        assert_eq!(ours.into_iter().collect::<Vec<_>>(), [from]);
        assert_eq!(guarded.into_iter().collect::<Vec<_>>(), [(from + PAGE) / block]);
        assert_eq!((left.absent, left.other), (1, 1));
    }

    #[test]
    fn a_write_falls_in_the_window_of_our_events() {
        assert_eq!(window_of(false, false), Window::BeforeSent);
        assert_eq!(window_of(true, false), Window::InAir);
        assert_eq!(window_of(true, true), Window::AfterLanding);
        // A landing with the lists not marked sent (another queue's) is a landing all the same.
        assert_eq!(window_of(false, true), Window::AfterLanding);
    }

    #[test]
    fn the_record_is_found_in_any_of_its_three_places() {
        let mut b = [0u8; 16];
        assert!(!has_pattern(&b));
        b[4..8].copy_from_slice(&0xfffe03ffu32.to_le_bytes());
        assert!(has_pattern(&b));
    }
}
