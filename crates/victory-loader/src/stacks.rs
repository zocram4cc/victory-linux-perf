//! Where every thread is: `VICTORY_STACKS=<seconds>[,<seconds>...]` prints
//! each thread's unwound stack as `module+rva` that many seconds after
//! [`dump_stacks_on_schedule`], to find what a title waits on.

use std::ffi::c_void;

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationThread, ThreadBasicInformation, ThreadQuerySetWin32StartAddress,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_FULL_AMD64, GetThreadContext,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, OpenThread, ResumeThread,
    SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

use crate::crash::{MAX_FRAMES, locate, unwind_into};

/// Reads `VICTORY_STACKS` and, if set, starts a thread that prints every
/// thread's stack at each listed time (seconds from now).
pub fn dump_stacks_on_schedule() {
    sample_on_schedule();
    let Ok(list) = std::env::var("VICTORY_STACKS") else {
        return;
    };
    let times = schedule(&list);
    let start = std::time::Instant::now();
    std::thread::spawn(move || {
        for at in times {
            std::thread::sleep(at.saturating_sub(start.elapsed()));
            dump_stacks();
        }
    });
}

/// How many return addresses a sample keeps, and how many words of the stack
/// above the stack pointer are looked through for them.
const SAMPLE_DEPTH: usize = 10;
const SAMPLE_WORDS: usize = 8192;

/// `VICTORY_SAMPLE=<start s>,<seconds>,<interval ms>,<name>[|<name>...]`: a
/// sampling profile of the threads whose names begin with one of the names
/// (`*`: of every thread),
/// printed when the time is over. [`dump_stacks`] unwinds every thread with
/// `RtlVirtualUnwind`, which under Wine faulted inside ntdll on the first
/// dump of a running title; this reads only the instruction pointer and the
/// words of the stack that are return addresses into this executable (a call
/// instruction ends right before them), so it calls nothing while a thread
/// is suspended. Words a frame has not written yet may be return addresses
/// of earlier calls: the chains are approximate, the instruction pointers
/// exact. A line is `sample <count> <thread> <where> <- <rva> <rva> ...`,
/// the rvas of this executable, innermost first.
fn sample_on_schedule() {
    let Ok(spec) = std::env::var("VICTORY_SAMPLE") else {
        return;
    };
    let mut parts = spec.splitn(4, ',');
    let mut number = || parts.next().and_then(|t| t.trim().parse::<f64>().ok()).filter(|t| t.is_finite() && *t >= 0.0);
    let (Some(start), Some(length), Some(every)) = (number(), number(), number()) else {
        return;
    };
    let Some(names) = parts.next() else {
        return;
    };
    let names: Vec<String> = names.split('|').map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).collect();
    let _ = std::thread::Builder::new().name("victory-sampler".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs_f64(start));
        sample(
            std::time::Duration::from_secs_f64(length),
            std::time::Duration::from_secs_f64(every / 1000.0),
            &names,
        );
    });
}

/// Whether a call instruction ends at `at` (inside this executable): a
/// relative call, or an indirect one of two to seven bytes.
///
/// # Safety
/// `at - 7 .. at` must be readable.
unsafe fn after_a_call(at: usize) -> bool {
    // SAFETY: the caller's.
    let byte = |back: usize| unsafe { ((at - back) as *const u8).read() };
    if byte(5) == 0xe8 {
        return true;
    }
    [2usize, 3, 4, 6, 7].iter().any(|&back| byte(back) == 0xff && byte(back - 1) & 0x38 == 0x10)
}

fn sample(length: std::time::Duration, every: std::time::Duration, names: &[String]) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    // SAFETY: this executable's own headers: `e_lfanew` at 0x3c, and in the
    // PE32+ optional header `SizeOfImage` 0x50 past the signature.
    let (base, size) = unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as usize;
        let nt = base + ((base + 0x3c) as *const u32).read() as usize;
        (base, ((nt + 0x50) as *const u32).read() as usize)
    };
    // The threads asked for: name, handle, the high end of the stack.
    let mut threads = Vec::new();
    for id in other_threads() {
        // SAFETY: opens a thread of our own process.
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, id) };
        if thread.is_null() {
            continue;
        }
        let name = name_of(thread);
        let mut info = BasicInfo {
            exit_status: 0,
            teb: std::ptr::null(),
            client_id: [0; 2],
            affinity: 0,
            priority: 0,
            base_priority: 0,
        };
        // SAFETY: writes `info`, whose layout and size are the system's.
        let status = unsafe {
            NtQueryInformationThread(
                thread,
                ThreadBasicInformation,
                (&raw mut info).cast(),
                size_of::<BasicInfo>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status != 0 || info.teb.is_null() || !names.iter().any(|n| n == "*" || name.starts_with(n.as_str())) {
            // SAFETY: we opened it.
            unsafe { CloseHandle(thread) };
            continue;
        }
        // SAFETY: the TEB of a thread of this process that lives as long as
        // the process (the threads sampled are the layer's own workers).
        let high = unsafe { info.teb.add(1).read() };
        threads.push((name, thread, high));
    }
    eprintln!(
        "VIctory: sampling {} threads every {:.1} ms for {:.0} s (executable at {base:#x}+{size:#x})",
        threads.len(),
        every.as_secs_f64() * 1000.0,
        length.as_secs_f64()
    );
    let mut counts: std::collections::HashMap<(usize, u64, [u32; SAMPLE_DEPTH]), u32> = std::collections::HashMap::new();
    let mut words = vec![0usize; SAMPLE_WORDS];
    // SAFETY: zeroed is a valid CONTEXT.
    let mut context = Aligned(unsafe { std::mem::zeroed() });
    let (mut taken, mut missed) = (0u64, 0u64);
    let end = std::time::Instant::now() + length;
    while std::time::Instant::now() < end {
        for (i, &(_, thread, high)) in threads.iter().enumerate() {
            // SAFETY: a thread of this process that is not this one.
            if unsafe { SuspendThread(thread) } == u32::MAX {
                missed += 1;
                continue;
            }
            context.0.ContextFlags = CONTEXT_FULL_AMD64;
            // Nothing between the suspension and the resume allocates or
            // takes a lock: the words go into a buffer made beforehand.
            // SAFETY: the context is ours; the stack between the suspended
            // thread's stack pointer and its high end is committed.
            let (ok, n) = unsafe {
                let ok = GetThreadContext(thread, &mut context.0) != 0;
                let rsp = context.0.Rsp as usize;
                // Not the thread's own stack end: work run on a stack of
                // its own (a large one for pipelines) is outside it. The
                // query takes no lock a suspended thread can hold.
                let mut region: windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                let asked = ok
                    && rsp != 0
                    && rsp % 8 == 0
                    && windows_sys::Win32::System::Memory::VirtualQuery(rsp as *const c_void, &mut region, size_of_val(&region)) != 0
                    && region.State == windows_sys::Win32::System::Memory::MEM_COMMIT
                    && region.Protect & (windows_sys::Win32::System::Memory::PAGE_READWRITE | windows_sys::Win32::System::Memory::PAGE_READONLY) != 0
                    && region.Protect & windows_sys::Win32::System::Memory::PAGE_GUARD == 0;
                let _ = high;
                let n = if asked {
                    ((region.BaseAddress as usize + region.RegionSize).saturating_sub(rsp) / 8).min(SAMPLE_WORDS)
                } else {
                    0
                };
                std::ptr::copy_nonoverlapping(rsp as *const usize, words.as_mut_ptr(), n);
                ResumeThread(thread);
                (ok, n)
            };
            if !ok {
                missed += 1;
                continue;
            }
            let mut key = [0u32; SAMPLE_DEPTH];
            let mut k = 0;
            for &word in &words[..n] {
                // SAFETY: inside this executable, past its headers.
                if word >= base + 0x1000 && word < base + size && unsafe { after_a_call(word) } {
                    key[k] = (word - base) as u32;
                    k += 1;
                    if k == SAMPLE_DEPTH {
                        break;
                    }
                }
            }
            *counts.entry((i, context.0.Rip, key)).or_default() += 1;
            taken += 1;
        }
        std::thread::sleep(every);
    }
    let mut lines: Vec<_> = counts.into_iter().collect();
    lines.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("VIctory: {taken} samples taken, {missed} missed, {} different", lines.len());
    for ((i, rip, key), n) in lines {
        let chain: Vec<String> = key.iter().take_while(|&&rva| rva != 0).map(|rva| format!("{rva:#x}")).collect();
        let name = if threads[i].0.is_empty() { "unnamed" } else { threads[i].0.as_str() };
        eprintln!("  sample {n} {name}#{i} {} <- {}", locate(rip as usize), chain.join(" "));
    }
    for (_, thread, _) in threads {
        // SAFETY: we opened it.
        unsafe { CloseHandle(thread) };
    }
}

/// The times in a `VICTORY_STACKS` list (seconds, comma-separated), in
/// order. Negative times are now; times no `Duration` holds (`inf`, `NaN`,
/// `1e20`) are left out instead of panicking the dump thread.
fn schedule(list: &str) -> Vec<std::time::Duration> {
    let mut times: Vec<_> = list
        .split(',')
        .filter_map(|t| t.trim().parse::<f64>().ok())
        .filter(|t| !t.is_nan())
        .filter_map(|t| std::time::Duration::try_from_secs_f64(t.max(0.0)).ok())
        .collect();
    times.sort();
    times
}

/// `THREAD_BASIC_INFORMATION` (ntddk).
#[repr(C)]
struct BasicInfo {
    exit_status: i32,
    teb: *const usize,
    client_id: [usize; 2],
    affinity: usize,
    priority: i32,
    base_priority: i32,
}

/// `GetThreadContext` wants a 16-byte aligned context.
#[repr(C, align(16))]
struct Aligned(CONTEXT);

/// The ids of this process's threads but the caller's.
pub(crate) fn other_threads() -> Vec<u32> {
    // SAFETY: plain queries and a snapshot of this process's threads.
    let (me, process, snapshot) = unsafe {
        (
            GetCurrentThreadId(),
            GetCurrentProcessId(),
            CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0),
        )
    };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut ids = Vec::new();
    // SAFETY: zeroed is a valid THREADENTRY32; its size is set before use.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    // SAFETY: `snapshot` is open; `entry` is sized.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == process && entry.th32ThreadID != me {
            ids.push(entry.th32ThreadID);
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: we opened it.
    unsafe { CloseHandle(snapshot) };
    ids
}

/// Prints every other thread's stack to stderr. Each thread is suspended
/// only while its context is read and unwound into a buffer allocated
/// beforehand (it may hold the heap lock).
pub fn dump_stacks() {
    let ids = other_threads();
    eprintln!("VIctory: stacks of {} threads", ids.len());
    for id in ids {
        // SAFETY: opens a thread of our own process.
        let thread = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                id,
            )
        };
        if thread.is_null() {
            continue;
        }
        let (frames, (priority, base, affinity)) = frames_of(thread);
        eprintln!(
            "  thread {id} \"{}\" start {} priority {priority} (base {base}) affinity {affinity:#x}",
            name_of(thread),
            locate(start_of(thread))
        );
        for (i, pc) in frames.into_iter().enumerate() {
            eprintln!("    #{i:<2} {}", locate(pc as usize));
        }
        // SAFETY: we opened it.
        unsafe { CloseHandle(thread) };
    }
}

/// The thread's frames and its current and base scheduling priority (0-31).
pub(crate) fn frames_of(thread: HANDLE) -> (Vec<u64>, (i32, i32, usize)) {
    let mut frames = Vec::with_capacity(MAX_FRAMES);
    let mut info: BasicInfo = BasicInfo {
        exit_status: 0,
        teb: std::ptr::null(),
        client_id: [0; 2],
        affinity: 0,
        priority: 0,
        base_priority: 0,
    };
    // Suspended first: a running thread may exit and free its TEB between
    // the query and the read below.
    // SAFETY: a thread of this process that is not this one.
    if unsafe { SuspendThread(thread) } == u32::MAX {
        return (frames, (0, 0, 0));
    }
    // SAFETY: writes `info`, whose layout and size are the system's.
    let status = unsafe {
        NtQueryInformationThread(
            thread,
            ThreadBasicInformation,
            (&raw mut info).cast(),
            size_of::<BasicInfo>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status != 0 || info.teb.is_null() {
        // SAFETY: suspended above.
        unsafe { ResumeThread(thread) };
        return (frames, (0, 0, 0));
    }
    // SAFETY: the TEB of a suspended thread of this process: its NT_TIB
    // holds the stack base at +8 and the stack limit at +0x10.
    let (high, low) = unsafe { (info.teb.add(1).read(), info.teb.add(2).read()) };
    // SAFETY: zeroed is a valid CONTEXT.
    let mut context = Aligned(unsafe { std::mem::zeroed() });
    context.0.ContextFlags = CONTEXT_FULL_AMD64;
    // SAFETY: reads and resumes the thread suspended above; nothing until
    // the resume allocates or takes a lock the thread could hold besides
    // the unwinder's own.
    unsafe {
        if GetThreadContext(thread, &mut context.0) != 0 {
            unwind_into(context.0, low, high, &mut frames);
        }
        ResumeThread(thread);
    }
    (frames, (info.priority, info.base_priority, info.affinity))
}

pub(crate) fn start_of(thread: HANDLE) -> usize {
    let mut start = 0usize;
    // SAFETY: writes one pointer-sized start address.
    unsafe {
        NtQueryInformationThread(
            thread,
            ThreadQuerySetWin32StartAddress,
            (&raw mut start).cast(),
            size_of::<usize>() as u32,
            std::ptr::null_mut(),
        )
    };
    start
}

pub(crate) fn name_of(thread: HANDLE) -> String {
    let mut name = std::ptr::null_mut();
    // SAFETY: the system allocates the string; we free it with LocalFree.
    unsafe {
        if GetThreadDescription(thread, &mut name) < 0 || name.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| *name.add(i) != 0).count();
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(name, len));
        LocalFree(name.cast::<c_void>());
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_threads_stack_unwinds_to_its_start() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let parked = std::thread::spawn(move || {
            // SAFETY: plain query.
            tx.send(unsafe { GetCurrentThreadId() }).unwrap();
            stop_rx.recv().ok();
        });
        let id = rx.recv().unwrap();
        // SAFETY: a thread of this process.
        let thread = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                id,
            )
        };
        assert!(!thread.is_null());
        let (frames, _) = frames_of(thread);
        // Waiting in the kernel, several frames down from the thread start.
        assert!(frames.len() >= 3, "{frames:?}");
        assert!(start_of(thread) != 0);
        // SAFETY: opened above.
        unsafe { CloseHandle(thread) };
        dump_stacks();
        stop_tx.send(()).unwrap();
        parked.join().unwrap();
    }

    /// Times no `Duration` holds are left out instead of panicking the dump
    /// thread (`from_secs_f64` panicked on `inf`); the rest come in order.
    #[test]
    fn schedules_skip_times_out_of_range() {
        let secs = |list| {
            schedule(list)
                .iter()
                .map(|d| d.as_secs_f64())
                .collect::<Vec<_>>()
        };
        assert_eq!(secs("30, 5,x,-2,inf,NaN,1e20"), [0.0, 5.0, 30.0]);
        assert!(schedule("").is_empty());
    }
}
