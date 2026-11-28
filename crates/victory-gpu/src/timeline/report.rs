//! What was recorded, put in order: the Chrome trace, and the text summary
//! with the chain each present came out of.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use super::record::{COPY_QUEUE, DEPTH, Data, Event, Kind, frequency};

/// The process's threads at a moment.
#[derive(Default)]
pub(super) struct Threads {
    /// By number: its processor cycles so far, its name, where it started.
    all: HashMap<u32, (u64, String, usize)>,
    /// The processor's clock and the performance counter then.
    cycles: u64,
    ticks: u64,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: *mut std::ffi::c_void, cycles: *mut u64) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationThread(
        thread: *mut std::ffi::c_void,
        class: i32,
        out: *mut std::ffi::c_void,
        len: u32,
        returned: *mut u32,
    ) -> i32;
}

impl Threads {
    pub(super) fn now() -> Threads {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, LocalFree};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        };
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcessId, GetThreadDescription, OpenThread, THREAD_QUERY_INFORMATION,
        };
        /// `ThreadQuerySetWin32StartAddress`.
        const START: i32 = 9;
        let mut threads = Threads::default();
        // SAFETY: a snapshot of our own, walked with a sized entry; handles
        // opened here are closed; the name is the system's, freed as asked.
        unsafe {
            let own = GetCurrentProcessId();
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return threads;
            }
            let mut entry = THREADENTRY32 {
                dwSize: size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == own {
                    let thread = OpenThread(THREAD_QUERY_INFORMATION, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        let (mut cycles, mut start, mut name) = (0, 0usize, std::ptr::null_mut());
                        QueryThreadCycleTime(thread, &mut cycles);
                        NtQueryInformationThread(thread, START, (&raw mut start).cast(), 8, std::ptr::null_mut());
                        let mut text = String::new();
                        if GetThreadDescription(thread, &mut name) >= 0 && !name.is_null() {
                            let len = (0..).take_while(|&i| *name.add(i) != 0).count();
                            text = String::from_utf16_lossy(std::slice::from_raw_parts(name, len));
                            LocalFree(name.cast());
                        }
                        CloseHandle(thread);
                        threads.all.insert(entry.th32ThreadID, (cycles, text, start));
                    }
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            threads.cycles = std::arch::x86_64::_rdtsc();
        }
        threads.ticks = super::record::ticks();
        threads
    }
}

/// Names of addresses.
pub(super) struct Names {
    known: HashMap<usize, String>,
    symbols: Option<Symbols>,
}

/// The system's symbol library (asked for when a timeline is written, not
/// at start).
struct Symbols {
    from_address: unsafe extern "system" fn(*mut std::ffi::c_void, u64, *mut u64, *mut u8) -> i32,
    process: *mut std::ffi::c_void,
}

impl Symbols {
    fn load() -> Option<Symbols> {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExA};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        type Initialize = unsafe extern "system" fn(*mut std::ffi::c_void, *const u16, i32) -> i32;
        type Options = unsafe extern "system" fn(u32) -> u32;
        // SAFETY: the system's own library and functions, by their names and
        // signatures.
        unsafe {
            let module = LoadLibraryExA(c"dbghelp.dll".as_ptr().cast(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if module.is_null() {
                return None;
            }
            let options: Options = std::mem::transmute(GetProcAddress(module, c"SymSetOptions".as_ptr().cast())?);
            let initialize: Initialize = std::mem::transmute(GetProcAddress(module, c"SymInitializeW".as_ptr().cast())?);
            let from_address = std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(*mut std::ffi::c_void, u64, *mut u64, *mut u8) -> i32,
            >(GetProcAddress(module, c"SymFromAddr".as_ptr().cast())?);
            let process = GetCurrentProcess();
            // Undecorated names, loaded when asked for, no dialogs.
            options(0x2 | 0x4 | 0x200);
            // Our symbols lie beside the program, which names them by their
            // file alone. (It fails if a backtrace was asked for before:
            // then it is there, and looks beside every module.)
            let beside = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.to_path_buf())).unwrap_or_default();
            let beside: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(beside.as_os_str()).chain([0]).collect();
            initialize(process, beside.as_ptr(), 1);
            Some(Symbols { from_address, process })
        }
    }

    fn name(&self, address: usize) -> Option<String> {
        // `SYMBOL_INFO`: 88 bytes with the name's first character at 84.
        const NAME: usize = 84;
        const ROOM: usize = 512;
        let mut info = [0u64; (NAME + ROOM) / 8 + 1];
        let at = info.as_mut_ptr().cast::<u8>();
        let mut off = 0u64;
        // SAFETY: a buffer of the structure's size and the name's, its size
        // fields set as asked.
        unsafe {
            at.cast::<u32>().write(88);
            at.add(80).cast::<u32>().write(ROOM as u32);
            if (self.from_address)(self.process, address as u64 - 1, &mut off, at) == 0 {
                return None;
            }
            let name = std::ffi::CStr::from_ptr(at.add(NAME).cast());
            Some(name.to_string_lossy().into_owned())
        }
    }
}

impl Names {
    pub(super) fn new() -> Names {
        Names {
            known: HashMap::new(),
            symbols: Symbols::load(),
        }
    }

    /// The module `address` lies in, and where in it; whether it is ours.
    fn module(address: usize) -> (String, usize, bool) {
        use windows_sys::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
            GetModuleHandleExW, GetModuleHandleW,
        };
        let mut module = std::ptr::null_mut();
        let mut path = [0u16; 260];
        // SAFETY: an address, no name; plain queries.
        unsafe {
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if GetModuleHandleExW(flags, address as *const u16, &mut module) == 0 {
                return ("?".into(), address, false);
            }
            let len = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize;
            let path = String::from_utf16_lossy(&path[..len]);
            let file = path.rsplit('\\').next().unwrap_or("?").to_string();
            (file, address - module as usize, module == GetModuleHandleW(std::ptr::null()))
        }
    }

    /// A name for `address`: our own functions by their names, others by
    /// their module and place in it.
    fn of(&mut self, address: usize) -> String {
        if let Some(name) = self.known.get(&address) {
            return name.clone();
        }
        let (file, at, ours) = Self::module(address);
        let name = match self.symbols.as_ref().filter(|_| ours).and_then(|s| s.name(address)) {
            Some(name) => name,
            None => format!("{}+{at:#x}", file.strip_suffix(".dll").unwrap_or(&file)),
        };
        self.known.insert(address, name.clone());
        name
    }

    /// Where a stack says the thread was: the innermost functions that are
    /// not the standard library's or this module's.
    fn site(&mut self, stack: &[usize; DEPTH]) -> String {
        let mut parts: Vec<String> = Vec::new();
        for &address in stack.iter().take_while(|a| **a != 0) {
            let name = self.of(address);
            let plain = name.trim_start_matches('<').trim_start_matches("impl$").trim_start_matches(|c: char| c.is_ascii_digit());
            let skipped = ["std::", "core::", "alloc::", "windows_core::", "windows::"].iter().any(|p| plain.starts_with(p))
                || ["timeline::hooks::", "timeline::record::"].iter().any(|p| name.contains(p))
                || ["KERNEL", "ntdll", "kernel"].iter().any(|p| name.starts_with(p));
            if !skipped {
                parts.push(name.replace("victory_gpu::", "").replace("victory_hle::", "hle::"));
            }
            if parts.len() == 3 {
                break;
            }
        }
        parts.join(" < ")
    }
}

/// A part of a chain: what was done, or waited for, from when to when.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Part {
    pub(super) from: u64,
    pub(super) to: u64,
    /// The thread (0: the card).
    pub(super) thread: u32,
    pub(super) cause: String,
}

/// A wait that ended, as the chains see it.
#[derive(Clone, Copy, Debug)]
struct Wait {
    t0: u64,
    t1: u64,
    object: u64,
    fence: u64,
    event: usize,
}

/// A submission: when it went to the card, from which thread, and from when
/// to when it ran there.
#[derive(Clone, Copy, Debug)]
pub(super) struct Run {
    pub(super) fence: u64,
    pub(super) t0: u64,
    pub(super) t1: u64,
    pub(super) thread: u32,
    pub(super) on: (u64, u64),
}

/// What was recorded, by thread and in order.
pub(super) struct Trace {
    pub(super) events: Vec<Event>,
    /// The spans of a thread (places among the events), by their beginning.
    spans: HashMap<u32, Vec<usize>>,
    /// The waits of a thread, by their end.
    waits: HashMap<u32, Vec<Wait>>,
    /// Who signalled an object when.
    wakes: HashMap<u64, Vec<(u64, u32)>>,
    /// The submissions, by the fence's value.
    pub(super) runs: Vec<Run>,
    pub(super) frames: Vec<(u64, u32)>,
}

impl Trace {
    /// `stamps`: the card's times of a submission by its place, as ticks.
    pub(super) fn new(mut events: Vec<Event>, stamps: &[(u64, u64)]) -> Trace {
        events.sort_by_key(|e| (e.t0, std::cmp::Reverse(e.t1)));
        let mut trace = Trace {
            events: Vec::new(),
            spans: HashMap::new(),
            waits: HashMap::new(),
            wakes: HashMap::new(),
            runs: Vec::new(),
            frames: Vec::new(),
        };
        for (i, e) in events.iter().enumerate() {
            match e.kind {
                Kind::Span | Kind::Calls | Kind::Kernel => trace.spans.entry(e.thread).or_default().push(i),
                Kind::Wait => trace.waits.entry(e.thread).or_default().push(Wait {
                    t0: e.t0,
                    t1: e.t1,
                    object: e.a,
                    fence: e.b,
                    event: i,
                }),
                Kind::Wake => trace.wakes.entry(e.a).or_default().push((e.t0, e.thread)),
                Kind::Sent => {
                    let on = (e.b as usize).checked_sub(1).and_then(|at| stamps.get(at)).copied().unwrap_or((0, 0));
                    trace.runs.push(Run {
                        fence: e.a,
                        t0: e.t0,
                        t1: e.t1,
                        thread: e.thread,
                        on,
                    });
                    trace.spans.entry(e.thread).or_default().push(i);
                }
                Kind::Frame => trace.frames.push((e.t1, e.thread)),
                Kind::Told | Kind::Poll | Kind::Sample => {}
            }
        }
        for waits in trace.waits.values_mut() {
            waits.sort_by_key(|w| w.t1);
        }
        trace.runs.sort_by_key(|r| r.fence);
        trace.events = events;
        trace
    }

    /// Who ended the wait `w` of `thread`, and when.
    fn waker(&self, thread: u32, w: &Wait) -> Option<(u32, u64)> {
        let wakes = self.wakes.get(&w.object)?;
        let end = wakes.partition_point(|k| k.0 <= w.t1);
        wakes[..end].iter().rev().take_while(|k| k.0 >= w.t0).find(|k| k.1 != thread).map(|k| (k.1, k.0))
    }

    /// What `thread` was doing in `[from, to)`, the innermost span counting;
    /// the newest part first.
    fn doing(&self, thread: u32, from: u64, to: u64, out: &mut Vec<Part>) {
        if from >= to {
            return;
        }
        let none = Vec::new();
        let spans = self.spans.get(&thread).unwrap_or(&none);
        // The spans that touch it (a span is at most some frames long).
        let end = spans.partition_point(|&i| self.events[i].t0 < to);
        let touching: Vec<&Event> =
            spans[..end].iter().map(|&i| &self.events[i]).filter(|e| e.t1 > from).collect();
        let mut edges: Vec<u64> = vec![from, to];
        for e in &touching {
            edges.extend([e.t0, e.t1].iter().filter(|t| **t > from && **t < to));
        }
        edges.sort_unstable();
        edges.dedup();
        let mut parts: Vec<Part> = Vec::new();
        for pair in edges.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let inner = touching.iter().filter(|e| e.t0 <= a && e.t1 >= b).max_by_key(|e| (e.t0, std::cmp::Reverse(e.t1)));
            let cause = match inner {
                Some(e) if e.kind == Kind::Calls => {
                    // Of a run of list calls, the part that was ours.
                    let ours = (b - a) * e.b.min(e.t1 - e.t0) / (e.t1 - e.t0).max(1);
                    parts.push(Part {
                        from: a,
                        to: a + ours,
                        thread,
                        cause: "recording: inside our list calls".into(),
                    });
                    parts.push(Part {
                        from: a + ours,
                        to: b,
                        thread,
                        cause: "recording: between list calls (the caller's code)".into(),
                    });
                    continue;
                }
                Some(e) if e.kind == Kind::Kernel => format!("kernel: {}", e.name),
                Some(e) => e.name.to_string(),
                None => OUTSIDE.into(),
            };
            match parts.last_mut() {
                Some(last) if last.cause == cause => last.to = b,
                _ => parts.push(Part { from: a, to: b, thread, cause }),
            }
        }
        out.extend(parts.into_iter().rev().filter(|p| p.from < p.to));
    }

    /// The chain that ended at `t` on `thread`, back to `until`: what that
    /// thread did since its last wait ended, then what the thread that ended
    /// the wait had done, or the card, and so on. Newest first, end to end.
    pub(super) fn chain(&self, mut thread: u32, mut t: u64, until: u64) -> Vec<Part> {
        let mut out = Vec::new();
        for _ in 0..200_000 {
            if t <= until {
                break;
            }
            let waits = self.waits.get(&thread).map_or(&[][..], |w| w);
            let last = waits[..waits.partition_point(|w| w.t1 <= t)].last().filter(|w| w.t1 > until).copied();
            let Some(w) = last else {
                self.doing(thread, until, t, &mut out);
                break;
            };
            self.doing(thread, w.t1, t, &mut out);
            let name = self.events[w.event].name;
            if w.fence != 0 && w.fence != COPY_QUEUE {
                match self.card(w.fence, w.t1, until, &mut out) {
                    Some((sender, at)) if at < t => {
                        (thread, t) = (sender, at);
                        continue;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            match self.waker(thread, &w) {
                Some((other, at)) if at < t && w.fence == 0 => {
                    out.push(Part {
                        from: at.max(until),
                        to: w.t1,
                        thread,
                        cause: "woken, not running yet".into(),
                    });
                    (thread, t) = (other, at);
                }
                _ => {
                    let cause = match w.fence {
                        0 => format!("a wait nobody seen here ended ({name})"),
                        COPY_QUEUE => "the card's copy queue".into(),
                        _ => "the card (no times of it)".into(),
                    };
                    out.push(Part {
                        from: w.t0.max(until),
                        to: w.t1,
                        thread,
                        cause,
                    });
                    t = w.t0;
                }
            }
        }
        out.retain(|p| p.from < p.to);
        out
    }

    /// The card's part of a chain: it reached `fence` by `t`. Back through
    /// the submissions that stood in its queue, to the sending of one that
    /// found it idle: by whom, and when; `None` if that goes back before
    /// `until`, or (nothing put in) if the card's times are not known.
    fn card(&self, fence: u64, t: u64, until: u64, out: &mut Vec<Part>) -> Option<(u32, u64)> {
        let mut i = self.runs.partition_point(|r| r.fence < fence);
        let first = self.runs.get(i)?;
        if first.on == (0, 0) || first.t0 >= t {
            return Some((0, u64::MAX));
        }
        let part = |from: u64, to: u64, cause: &str| Part {
            from: from.max(until),
            to: to.max(until),
            thread: 0,
            cause: cause.into(),
        };
        let mut end = first.on.1.min(t);
        out.push(part(end, t, "the card was done, the wait not over yet"));
        loop {
            let run = self.runs[i];
            let begun = run.on.0.clamp(run.t0.min(end), end);
            out.push(part(begun, end, "the card running submissions"));
            if begun <= until {
                return None;
            }
            match i.checked_sub(1).map(|before| self.runs[before]) {
                Some(before) if before.on != (0, 0) && before.on.1 >= run.t0 => {
                    let to = before.on.1.min(begun);
                    out.push(part(to, begun, "the card between submissions"));
                    end = to;
                    i -= 1;
                    if end <= until {
                        return None;
                    }
                }
                _ => {
                    let sent = run.t1.min(begun);
                    out.push(part(sent, begun, "sent, the card not started yet"));
                    return Some((run.thread, sent));
                }
            }
        }
    }
}

/// The cause of what a thread did outside every span.
const OUTSIDE: &str = "outside our spans";

fn ms(ticks: u64) -> f64 {
    ticks as f64 * 1e3 / frequency() as f64
}

/// `parts` by cause, the largest first.
fn by_cause(parts: &[Part], label: &dyn Fn(u32) -> String) -> Vec<(String, u64)> {
    let mut sums: BTreeMap<String, u64> = BTreeMap::new();
    for p in parts {
        let cause = if p.cause == OUTSIDE { format!("{} on {}", p.cause, label(p.thread)) } else { p.cause.clone() };
        *sums.entry(cause).or_default() += p.to - p.from;
    }
    let mut sums: Vec<(String, u64)> = sums.into_iter().collect();
    sums.sort_by_key(|s| std::cmp::Reverse(s.1));
    sums
}

/// The summary.
pub(super) fn summary(trace: &Trace, names: &mut Names, stacks: &[[usize; DEPTH]], before: &Threads, after: &Threads) -> String {
    let mut text = String::new();
    let Some((&(start, _), &(stop, _))) = trace.frames.first().zip(trace.frames.last()) else {
        return "no frames\n".into();
    };
    let frames = (trace.frames.len() - 1).max(1) as f64;
    let mut labels: HashMap<u32, String> = HashMap::new();
    for (&id, (_, name, begun)) in after.all.iter().chain(before.all.iter()) {
        labels.entry(id).or_insert_with(|| match name.is_empty() {
            true => format!("{id} ({})", names.of(*begun + 1)),
            false => format!("{id} \"{name}\""),
        });
    }
    let label = |id: u32| match id {
        0 => "the card".to_string(),
        id => labels.get(&id).cloned().unwrap_or_else(|| id.to_string()),
    };
    let _ = writeln!(text, "{} frames in {:.1} ms ({:.1} ms a frame); {} events", frames, ms(stop - start), ms(stop - start) / frames, trace.events.len());

    // The chains.
    let mut all: Vec<Part> = Vec::new();
    for pair in trace.frames.windows(2) {
        let chain = trace.chain(pair[1].1, pair[1].0, pair[0].0);
        let _ = writeln!(text, "\nframe of {:.1} ms, present on {}: the chain it came out of, by cause", ms(pair[1].0 - pair[0].0), label(pair[1].1));
        for (cause, sum) in by_cause(&chain, &label).into_iter().take(24) {
            let _ = writeln!(text, "  {:7.2} ms  {cause}", ms(sum));
        }
        all.extend(chain);
    }
    let whole = trace.chain(trace.frames.last().map_or(0, |f| f.1), stop, start);
    let _ = writeln!(text, "\nthe chain back through all frames, per frame, by cause");
    for (cause, sum) in by_cause(&whole, &label).into_iter().take(40) {
        let _ = writeln!(text, "  {:7.2} ms  {cause}", ms(sum) / frames);
    }
    let _ = writeln!(text, "\nby thread, per frame");
    let mut by_thread: BTreeMap<u32, u64> = BTreeMap::new();
    for p in &whole {
        *by_thread.entry(p.thread).or_default() += p.to - p.from;
    }
    let mut by_thread: Vec<(u32, u64)> = by_thread.into_iter().collect();
    by_thread.sort_by_key(|t| std::cmp::Reverse(t.1));
    for (thread, sum) in by_thread {
        let _ = writeln!(text, "  {:7.2} ms  {}", ms(sum) / frames, label(thread));
    }
    let _ = writeln!(text, "\nthat chain, oldest first (parts under 0.15 ms left out; ms since the first present)");
    for p in whole.iter().rev().filter(|p| ms(p.to - p.from) >= 0.15) {
        let _ = writeln!(text, "  {:8.2} +{:6.2}  {:32} {}", ms(p.from - start), ms(p.to - p.from), label(p.thread), p.cause);
    }

    // The card.
    let runs: Vec<&Run> = trace.runs.iter().filter(|r| r.on != (0, 0) && r.on.1 > start && r.on.0 < stop).collect();
    let (mut busy, mut reach) = (0, start);
    let mut idle: BTreeMap<String, u64> = BTreeMap::new();
    let mut idle_all = 0;
    for r in &runs {
        let (a, b) = (r.on.0.clamp(start, stop), r.on.1.clamp(start, stop));
        if a > reach {
            idle_all += a - reach;
            if r.t1 > reach {
                for p in trace.chain(r.thread, r.t1.min(a), reach) {
                    let cause = if p.cause == OUTSIDE { format!("{} on {}", p.cause, label(p.thread)) } else { p.cause };
                    *idle.entry(cause).or_default() += p.to - p.from;
                }
            }
        }
        busy += b.saturating_sub(a.max(reach));
        reach = reach.max(b);
    }
    let _ = writeln!(
        text,
        "\nthe card, per frame: {:.2} ms running {:.1} submissions, {:.2} ms idle between them; of the idle time, by what the chain to the \
         next submission was",
        ms(busy) / frames,
        runs.len() as f64 / frames,
        ms(idle_all) / frames
    );
    let mut idle: Vec<(String, u64)> = idle.into_iter().collect();
    idle.sort_by_key(|i| std::cmp::Reverse(i.1));
    for (cause, sum) in idle.into_iter().take(30) {
        let _ = writeln!(text, "  {:7.2} ms  {cause}", ms(sum) / frames);
    }

    // Threads.
    let rate = (after.cycles.saturating_sub(before.cycles)) as f64 / ms(after.ticks.saturating_sub(before.ticks)).max(1e-9);
    let mut threads: Vec<(u32, f64, u64)> = after
        .all
        .iter()
        .map(|(&id, t)| {
            let ran = t.0.saturating_sub(before.all.get(&id).map_or(0, |b| b.0)) as f64 / rate.max(1e-9);
            let waits = trace.waits.get(&id).map_or(&[][..], |w| w);
            let waited = waits.iter().map(|w| w.t1.min(stop).saturating_sub(w.t0.max(start))).sum();
            (id, ran, waited)
        })
        .collect();
    threads.sort_by(|a, b| b.1.total_cmp(&a.1));
    let _ = writeln!(text, "\nthreads, per frame: running, waiting in what is wrapped, outermost spans of ours");
    for (id, ran, waited) in threads.into_iter().filter(|t| t.1 / frames >= 0.2).take(60) {
        let mut sums: BTreeMap<&str, u64> = BTreeMap::new();
        let mut reach = 0;
        for &i in trace.spans.get(&id).map_or(&[][..], |s| s) {
            let e = &trace.events[i];
            if e.t1 > reach {
                *sums.entry(e.name).or_default() += e.t1.min(stop).saturating_sub(e.t0.max(reach).max(start));
                reach = e.t1;
            }
        }
        let mut sums: Vec<(&str, u64)> = sums.into_iter().collect();
        sums.sort_by_key(|s| std::cmp::Reverse(s.1));
        let spans: Vec<String> = sums.iter().take(5).map(|(name, sum)| format!("{name} {:.1}", ms(*sum) / frames)).collect();
        let _ = writeln!(text, "  {:6.1} ran {:6.1} waited  {:36} {}", ran / frames, ms(waited) / frames, label(id), spans.join(", "));
    }

    // Spans, all threads, each by name with what lies in it.
    let mut sums: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
    for e in trace.events.iter().filter(|e| matches!(e.kind, Kind::Span | Kind::Kernel | Kind::Sent) && e.t1 > start && e.t0 < stop) {
        let s = sums.entry(e.name).or_default();
        let took = e.t1.min(stop) - e.t0.max(start);
        *s = (s.0 + took, s.1 + 1, s.2.max(took));
    }
    let mut sums: Vec<_> = sums.into_iter().collect();
    sums.sort_by_key(|s| std::cmp::Reverse(s.1.0));
    let _ = writeln!(text, "\nspans and kernel calls, all threads, per frame (a span counts what lies in it)");
    for (name, (sum, n, most)) in sums.into_iter().take(80) {
        let _ = writeln!(text, "  {:8.2} ms in {:8.1}, longest {:6.2}  {name}", ms(sum) / frames, n as f64 / frames, ms(most));
    }

    // Waits: who, where, for whom.
    let mut groups: HashMap<(u32, String, String), (u64, u64, u64)> = HashMap::new();
    for (&thread, waits) in &trace.waits {
        for w in waits.iter().filter(|w| w.t1 > start && w.t0 < stop && ms(w.t1 - w.t0) >= 0.05) {
            let e = &trace.events[w.event];
            let site = (e.stack as usize).checked_sub(1).and_then(|s| stacks.get(s)).map_or(String::new(), |s| names.site(s));
            let by = match (w.fence, self_waker(trace, thread, w)) {
                (COPY_QUEUE, _) => "the card's copy queue".to_string(),
                (0, Some((other, _))) => label(other),
                (0, None) => "nobody seen".into(),
                _ => "the card".into(),
            };
            let g = groups.entry((thread, format!("{} at {site}", e.name), by)).or_default();
            let took = w.t1.min(stop) - w.t0.max(start);
            *g = (g.0 + took, g.1 + 1, g.2.max(took));
        }
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.1.0));
    let _ = writeln!(text, "\nwaits of 50 us or more, per frame: who, what and where, who ended them");
    for ((thread, what, by), (sum, n, most)) in groups.into_iter().take(150) {
        let _ = writeln!(
            text,
            "  {:7.2} ms in {:7.1}, longest {:6.2}  {} | {what} | ended by {by}",
            ms(sum) / frames,
            n as f64 / frames,
            ms(most),
            label(thread)
        );
    }

    // Long list calls.
    let mut calls: HashMap<String, (u64, u64)> = HashMap::new();
    for e in trace.events.iter().filter(|e| e.name == "a long list call" && e.t1 > start && e.t0 < stop) {
        let site = (e.stack as usize).checked_sub(1).and_then(|s| stacks.get(s)).map_or(String::new(), |s| names.site(s));
        let c = calls.entry(site).or_default();
        *c = (c.0 + (e.t1 - e.t0), c.1 + 1);
    }
    let mut calls: Vec<_> = calls.into_iter().collect();
    calls.sort_by_key(|c| std::cmp::Reverse(c.1.0));
    let _ = writeln!(text, "\nlist calls of 50 us or more, per frame");
    for (site, (sum, n)) in calls.into_iter().take(25) {
        let _ = writeln!(text, "  {:7.2} ms in {:7.1}  {site}", ms(sum) / frames, n as f64 / frames);
    }
    text
}

fn self_waker(trace: &Trace, thread: u32, w: &Wait) -> Option<(u32, u64)> {
    trace.waker(thread, w)
}

fn escaped(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            c if (c as u32) < 32 => vec![' '],
            c => vec![c],
        })
        .collect()
}

/// The Chrome trace.
pub(super) fn chrome(
    trace: &Trace,
    names: &mut Names,
    stacks: &[[usize; DEPTH]],
    threads: &Threads,
    out: &mut impl std::io::Write,
) -> std::io::Result<()> {
    let start = trace.events.first().map_or(0, |e| e.t0);
    let us = |t: u64| t.saturating_sub(start) as f64 * 1e6 / frequency() as f64;
    write!(out, "{{\"traceEvents\":[\n{{\"ph\":\"M\",\"pid\":1,\"tid\":0,\"name\":\"thread_name\",\"args\":{{\"name\":\"the card\"}}}}")?;
    for (id, (_, name, begun)) in &threads.all {
        let name = if name.is_empty() { names.of(*begun + 1) } else { name.clone() };
        write!(
            out,
            ",\n{{\"ph\":\"M\",\"pid\":1,\"tid\":{id},\"name\":\"thread_name\",\"args\":{{\"name\":\"{}\"}}}}",
            escaped(&name)
        )?;
    }
    for e in &trace.events {
        let (ts, dur) = (us(e.t0), us(e.t1) - us(e.t0));
        let (pid, tid, cpu) = (1, e.thread, e.cpu);
        match e.kind {
            Kind::Wake => {}
            Kind::Told => write!(
                out,
                ",\n{{\"ph\":\"i\",\"s\":\"t\",\"pid\":{pid},\"tid\":{tid},\"ts\":{ts:.1},\"cat\":\"told\",\"name\":\"{}\",\"args\":{{\"at\":\"{:#x}\",\"value\":\"{:#x}\"}}}}",
                escaped(e.name),
                e.a,
                e.b
            )?,
            Kind::Sample => {
                let stack = (e.stack as usize).checked_sub(1).and_then(|s| stacks.get(s)).map_or(&[][..], |s| &s[..]);
                let stack: Vec<String> = stack.iter().take_while(|a| **a != 0).map(|a| format!("{a:#x}")).collect();
                write!(
                    out,
                    ",\n{{\"ph\":\"i\",\"s\":\"t\",\"pid\":{pid},\"tid\":{tid},\"ts\":{ts:.1},\"cat\":\"sample\",\"name\":\"{}\",\"args\":{{\"at\":\"{:#x}\",\"stack\":\"{}\"}}}}",
                    escaped(&names.of(e.a as usize)),
                    e.a,
                    stack.join(" ")
                )?;
            }
            Kind::Frame => write!(out, ",\n{{\"ph\":\"i\",\"s\":\"g\",\"pid\":{pid},\"tid\":{tid},\"ts\":{ts:.1},\"name\":\"a frame ends\"}}")?,
            Kind::Wait => {
                let site = (e.stack as usize).checked_sub(1).and_then(|s| stacks.get(s)).map_or(String::new(), |s| names.site(s));
                write!(
                    out,
                    ",\n{{\"ph\":\"X\",\"pid\":{pid},\"tid\":{tid},\"ts\":{ts:.1},\"dur\":{dur:.1},\"cat\":\"wait\",\"cname\":\"grey\",\
                     \"name\":\"wait\",\"args\":{{\"what\":\"{}\",\"object\":\"{:#x}\",\"fence\":\"{}\",\"at\":\"{}\"}}}}",
                    e.name,
                    e.a,
                    e.b,
                    escaped(&site)
                )?;
            }
            _ => write!(
                out,
                ",\n{{\"ph\":\"X\",\"pid\":{pid},\"tid\":{tid},\"ts\":{ts:.1},\"dur\":{dur:.1},\"cat\":\"{:?}\",\"name\":\"{}\",\
                 \"args\":{{\"a\":{},\"b\":{},\"cpu\":{cpu}}}}}",
                e.kind,
                escaped(e.name),
                e.a,
                e.b
            )?,
        }
    }
    for r in trace.runs.iter().filter(|r| r.on != (0, 0)) {
        write!(
            out,
            ",\n{{\"ph\":\"X\",\"pid\":1,\"tid\":0,\"ts\":{:.1},\"dur\":{:.1},\"name\":\"submission {}\",\"args\":{{\"sent by\":{}}}}}",
            us(r.on.0),
            us(r.on.1) - us(r.on.0),
            r.fence,
            r.thread
        )?;
    }
    // Who woke whom.
    let mut id = 0u64;
    for (&thread, waits) in &trace.waits {
        for w in waits.iter().filter(|w| ms(w.t1 - w.t0) >= 0.1) {
            if let Some((other, at)) = trace.waker(thread, w) {
                id += 1;
                write!(
                    out,
                    ",\n{{\"ph\":\"s\",\"pid\":1,\"tid\":{other},\"ts\":{:.1},\"id\":{id},\"cat\":\"wake\",\"name\":\"woke\"}},\
                     \n{{\"ph\":\"f\",\"bp\":\"e\",\"pid\":1,\"tid\":{thread},\"ts\":{:.1},\"id\":{id},\"cat\":\"wake\",\"name\":\"woke\"}}",
                    us(at),
                    us(w.t1)
                )?;
            }
        }
    }
    writeln!(out, "\n]}}")
}

/// Writes both files.
pub(super) fn write(file: &std::path::Path, data: Data, before: &Threads, after: &Threads, stamps: Vec<(u64, u64)>) {
    let mut names = Names::new();
    let (events, lost) = (data.events.len(), data.lost);
    let trace = Trace::new(data.events, &stamps);
    let mut text = summary(&trace, &mut names, &data.stacks, before, after);
    if lost > 0 {
        let _ = writeln!(text, "\n{lost} events were lost: the buffer was full");
    }
    let beside = file.with_extension("txt");
    let written = std::fs::write(&beside, text).and_then(|()| {
        let mut out = std::io::BufWriter::new(std::fs::File::create(file)?);
        chrome(&trace, &mut names, &data.stacks, after, &mut out)?;
        std::io::Write::flush(&mut out)
    });
    match written {
        Ok(()) => tracing::info!(file = %file.display(), summary = %beside.display(), events, lost, "the timeline is written"),
        Err(e) => tracing::warn!(error = %e, file = %file.display(), "the timeline could not be written"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: Kind, name: &'static str, thread: u32, t: (u64, u64), a: u64, b: u64) -> Event {
        Event {
            t0: t.0,
            t1: t.1,
            a,
            b,
            name,
            thread,
            stack: 0,
            cpu: 0,
            kind,
        }
    }

    fn causes(parts: &[Part]) -> Vec<(u32, u64, u64, &str)> {
        parts.iter().map(|p| (p.thread, p.from, p.to, p.cause.as_str())).collect()
    }

    /// The present's thread waited for the title's, which waited for the
    /// relay, which waited for the card, whose submission a third thread
    /// sent: the chain goes back through all of them, end to end.
    #[test]
    fn a_chain_goes_back_through_who_woke_whom_and_the_card() {
        let events = vec![
            event(Kind::Frame, "present", 1, (1000, 1000), 0, 0),
            event(Kind::Frame, "present", 1, (2000, 2000), 0, 0),
            // Thread 1 presents after waiting for object 7.
            event(Kind::Span, "present", 1, (1900, 2000), 0, 0),
            event(Kind::Wait, "WaitForSingleObject", 1, (1100, 1850), 7, 0),
            event(Kind::Wake, "SetEvent", 2, (1840, 1840), 7, 0),
            // (An earlier wake of the same object, before the wait.)
            event(Kind::Wake, "SetEvent", 9, (1050, 1050), 7, 0),
            // Thread 2 landed flights after the card reached 12.
            event(Kind::Span, "landing", 2, (1700, 1845), 0, 0),
            event(Kind::Span, "merging", 2, (1750, 1800), 0, 0),
            event(Kind::Wait, "WaitForSingleObject", 2, (1200, 1690), 99, 12),
            // The card ran 11 and 12, both sent by thread 3; 11 found it idle.
            event(Kind::Sent, "ExecuteCommandLists", 3, (1290, 1300), 11, 1),
            event(Kind::Sent, "ExecuteCommandLists", 3, (1390, 1400), 12, 2),
            event(Kind::Span, "steps", 3, (1100, 1290), 0, 0),
        ];
        let trace = Trace::new(events, &[(1310, 1500), (1500, 1680)]);
        let chain = trace.chain(1, 2000, 1000);
        assert_eq!(
            causes(&chain),
            [
                (1, 1900, 2000, "present"),
                (1, 1850, 1900, OUTSIDE),
                (1, 1840, 1850, "woken, not running yet"),
                (2, 1800, 1840, "landing"),
                (2, 1750, 1800, "merging"),
                (2, 1700, 1750, "landing"),
                (2, 1690, 1700, OUTSIDE),
                (0, 1680, 1690, "the card was done, the wait not over yet"),
                (0, 1500, 1680, "the card running submissions"),
                (0, 1310, 1500, "the card running submissions"),
                (0, 1300, 1310, "sent, the card not started yet"),
                (3, 1290, 1300, "ExecuteCommandLists"),
                (3, 1100, 1290, "steps"),
                (3, 1000, 1100, OUTSIDE),
            ]
        );
        // End to end: no part of the frame is missing or counted twice.
        assert!(chain.windows(2).all(|p| p[0].from == p[1].to));
    }

    /// A wait nobody was seen to end (a timer) is a cause of its own, and
    /// the chain goes on with the thread before it.
    #[test]
    fn a_wait_without_a_waker_stays_with_its_thread() {
        let events = vec![
            event(Kind::Wait, "Sleep", 1, (120, 180), 0, 0),
            event(Kind::Wait, "WaitOnAddress", 1, (20, 60), 5, 0),
            event(Kind::Wake, "WakeByAddressSingle", 1, (50, 50), 5, 0),
        ];
        let trace = Trace::new(events, &[]);
        assert_eq!(
            causes(&trace.chain(1, 200, 0)),
            [
                (1, 180, 200, OUTSIDE),
                (1, 120, 180, "a wait nobody seen here ended (Sleep)"),
                (1, 60, 120, OUTSIDE),
                (1, 20, 60, "a wait nobody seen here ended (WaitOnAddress)"),
                (1, 0, 20, OUTSIDE),
            ]
        );
    }

    /// Real threads, real waits: what the wrappers record names who ended
    /// them, and the chain leads to the thread that worked.
    #[test]
    fn real_waits_are_recorded_with_who_ended_them() {
        use super::super::{hooks, record, span};
        use std::sync::{Arc, Condvar, Mutex};
        hooks::wrap_own();
        record::make_room();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let lock = Arc::new(Mutex::new(()));
        record::begin();
        record::frame();
        let first = super::super::record::ticks();
        let worker = {
            let (gate, lock) = (gate.clone(), lock.clone());
            std::thread::spawn(move || {
                let held = lock.lock().unwrap();
                let working = span("the work of a test");
                std::thread::sleep(std::time::Duration::from_millis(5));
                let spun = crate::clock::Instant::now();
                while spun.elapsed().as_millis() < 30 {
                    std::hint::spin_loop();
                }
                working.end();
                drop(held);
                *gate.0.lock().unwrap() = true;
                gate.1.notify_all();
                record::thread()
            })
        };
        {
            let mut open = gate.0.lock().unwrap();
            while !*open {
                open = gate.1.wait(open).unwrap();
            }
        }
        let last = super::super::record::ticks();
        let me = record::thread();
        let other = worker.join().unwrap();
        // A contended lock.
        let held = lock.lock().unwrap();
        let second = {
            let lock = lock.clone();
            std::thread::spawn(move || {
                drop(lock.lock().unwrap());
                record::thread()
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        drop(held);
        let second = second.join().unwrap();
        let data = record::end();
        assert_eq!(data.lost, 0);
        let trace = Trace::new(data.events, &[]);
        let chain = trace.chain(me, last, first);
        let work: u64 = chain.iter().filter(|p| p.thread == other && p.cause == "the work of a test").map(|p| p.to - p.from).sum();
        assert!(ms(work) >= 20.0, "{chain:?}");
        let contended = trace.waits[&second]
            .iter()
            .filter(|w| ms(w.t1 - w.t0) >= 10.0)
            .find_map(|w| trace.waker(second, w));
        assert_eq!(contended.map(|c| c.0), Some(me));
        let mut names = Names::new();
        let sites: Vec<String> = data.stacks.iter().map(|s| names.site(s)).collect();
        assert!(sites.iter().any(|s| s.contains("real_waits_are_recorded")), "{sites:?}");
        let mut json = Vec::new();
        chrome(&trace, &mut names, &data.stacks, &Threads::now(), &mut json).unwrap();
        assert!(json.starts_with(b"{\"traceEvents\""));
        let text = summary(&trace, &mut names, &data.stacks, &Threads::now(), &Threads::now());
        assert!(text.contains("frames in"));
    }
}
