//! `QueryPerformanceCounter` for our own code without a kernel call.
//!
//! Under Proton Experimental 11 Wine's `RtlQueryPerformanceCounter` is
//! always `NtQueryPerformanceCounter`: a syscall through
//! `__wine_syscall_dispatcher` (the whole CPU state saved and restored) for a
//! `clock_gettime`. Every `std::time::Instant::now` of ours is one: in a CPU
//! profile of the list threads (28 Nov, standing) the dispatcher, the clock
//! and their kernel entries were about 7 % of the threads' time, the
//! submitting thread's about 6 %.
//!
//! [`install`] points victory.exe's own import of
//! `kernel32!QueryPerformanceCounter` (std's `Instant` and every other caller
//! in this executable go through it; the title's imports are bound
//! elsewhere and keep the host's) at [`counter`]: the processor's time stamp
//! counter scaled to the host counter's ticks, anchored to a real reading at
//! most every 100 ms so the two never drift apart by more than a microsecond
//! or so. Only with an invariant TSC (CPUID 0x80000007 EDX bit 8: constant
//! rate, kept in every power state, the same on every core); else nothing
//! changes. `VICTORY_FAST_CLOCK=0`: the host's counter, as before.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};

/// The host's `QueryPerformanceCounter`, as the import held it.
static HOST: AtomicUsize = AtomicUsize::new(0);
/// Host counter ticks per TSC tick, in 2^-40 units.
static SCALE: AtomicU64 = AtomicU64::new(0);
/// The anchor: a host reading and the TSC at it; a sequence number that is
/// odd while they change.
static ANCHOR_QPC: AtomicU64 = AtomicU64::new(0);
static ANCHOR_TSC: AtomicU64 = AtomicU64::new(0);
static SEQ: AtomicU64 = AtomicU64::new(0);
/// The first anchor, for the scale measured over everything since.
static FIRST: (AtomicU64, AtomicU64) = (AtomicU64::new(0), AtomicU64::new(0));
static MOVING: AtomicBool = AtomicBool::new(false);

/// TSC ticks between anchors (100 ms at the scale found at start).
static EVERY: AtomicU64 = AtomicU64::new(u64::MAX);

type Qpc = unsafe extern "system" fn(*mut i64) -> i32;

fn host_counter() -> u64 {
    let f = HOST.load(Relaxed);
    let mut v = 0i64;
    if f == 0 {
        // SAFETY: a valid out pointer.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut v) };
    } else {
        // SAFETY: the host's function, as the import held it.
        unsafe { std::mem::transmute::<usize, Qpc>(f)(&mut v) };
    }
    v as u64
}

fn tsc() -> u64 {
    // SAFETY: RDTSC has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Whether the switch has the fast path on (read at each call so an in-run
/// A/B can flip it).
fn on() -> bool {
    crate::toggled!("VICTORY_FAST_CLOCK")
}

/// The replacement for `QueryPerformanceCounter`.
unsafe extern "system" fn counter(out: *mut i64) -> i32 {
    if out.is_null() {
        return 0;
    }
    if !on() {
        // SAFETY: the caller's out pointer.
        unsafe { *out = host_counter() as i64 };
        return 1;
    }
    let now = tsc();
    let scale = SCALE.load(Relaxed);
    loop {
        let s1 = SEQ.load(Acquire);
        if s1 & 1 == 1 {
            std::hint::spin_loop();
            continue;
        }
        let (q, t) = (ANCHOR_QPC.load(Relaxed), ANCHOR_TSC.load(Relaxed));
        if SEQ.load(Acquire) != s1 {
            continue;
        }
        // Signed: a reading taken on this thread just before another thread
        // moved the anchor (or on a core whose TSC is a few cycles behind)
        // lies before it. Taken unsigned, that was 2^64 ticks in the future
        // (28 Nov: durations of 10^12 ms in the log, and the title crashed).
        let since = now.wrapping_sub(t) as i64;
        if since >= EVERY.load(Relaxed) as i64 && !MOVING.swap(true, Acquire) {
            move_anchor();
            MOVING.store(false, Release);
            continue;
        }
        let ticks = ((u128::from(since.unsigned_abs()) * u128::from(scale)) >> 40) as u64;
        let v = if since >= 0 { q.wrapping_add(ticks) } else { q.wrapping_sub(ticks) };
        // SAFETY: the caller's out pointer.
        unsafe { *out = v as i64 };
        return 1;
    }
}

/// Takes a new anchor from a real reading and refines the scale over all the
/// time since the first one.
fn move_anchor() {
    let t0 = tsc();
    let q = host_counter();
    let t1 = tsc();
    let t = t0 / 2 + t1 / 2;
    let (fq, ft) = (FIRST.0.load(Relaxed), FIRST.1.load(Relaxed));
    if t > ft && q > fq {
        let scale = ((u128::from(q - fq) << 40) / u128::from(t - ft)) as u64;
        if scale > 0 {
            SCALE.store(scale, Relaxed);
        }
    }
    SEQ.fetch_add(1, Release);
    ANCHOR_QPC.store(q, Relaxed);
    ANCHOR_TSC.store(t, Relaxed);
    SEQ.fetch_add(1, Release);
}

/// Whether the processor's TSC is invariant.
fn invariant_tsc() -> bool {
    let max = core::arch::x86_64::__cpuid(0x8000_0000).eax;
    max >= 0x8000_0007 && core::arch::x86_64::__cpuid(0x8000_0007).edx & (1 << 8) != 0
}

/// Points victory.exe's import of `QueryPerformanceCounter` at [`counter`]
/// (see the module's comment). Call once, early, before other threads.
pub fn install() {
    if std::env::var_os("VICTORY_FAST_CLOCK").is_some_and(|v| v == "0") || !invariant_tsc() {
        return;
    }
    // The scale: the host counter against the TSC over 20 ms.
    let (q0, t0) = (host_counter(), tsc());
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(20) {
        std::hint::spin_loop();
    }
    let (q1, t1) = (host_counter(), tsc());
    if t1 <= t0 || q1 <= q0 {
        return;
    }
    let scale = ((u128::from(q1 - q0) << 40) / u128::from(t1 - t0)) as u64;
    // 100 ms in TSC ticks: host ticks a second (10 MHz under Wine) / 10.
    let mut freq = 0i64;
    // SAFETY: a valid out pointer.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut freq) };
    if scale == 0 || freq <= 0 {
        return;
    }
    let every = ((u128::from(freq as u64 / 10) << 40) / u128::from(scale)) as u64;
    SCALE.store(scale, Relaxed);
    FIRST.0.store(q0, Relaxed);
    FIRST.1.store(t0, Relaxed);
    ANCHOR_QPC.store(q1, Relaxed);
    ANCHOR_TSC.store(t1, Relaxed);
    EVERY.store(every.max(1), Relaxed);
    // SAFETY: our own image's import table, read and written as the PE
    // format lays it out; the slot is made writable for the write.
    let host = unsafe { patch_import(c"kernel32.dll", c"QueryPerformanceCounter", counter as Qpc as usize) };
    match host {
        Some(h) => {
            HOST.store(h, Relaxed);
            tracing::info!(
                ticks_per_second = freq,
                tsc_mhz = (u128::from(freq as u64) << 40) / u128::from(scale) / 1_000_000,
                "QueryPerformanceCounter of victory.exe from the TSC (VICTORY_FAST_CLOCK=0: the host's)"
            );
        }
        None => tracing::info!("victory.exe's import of QueryPerformanceCounter not found: the host's counter stays"),
    }
}

/// Replaces the import `dll!name` of this executable with `with`; the
/// function it held.
///
/// # Safety
/// Writes the image's import address table; call before other threads use
/// the import.
unsafe fn patch_import(dll: &std::ffi::CStr, name: &std::ffi::CStr, with: usize) -> Option<usize> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    // SAFETY: our own module.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if base == 0 {
        return None;
    }
    // SAFETY (all reads below): the loaded image's headers and import
    // directory, valid for as long as the image is loaded.
    unsafe {
        let rd32 = |at: usize| (at as *const u32).read_unaligned();
        let nt = base + rd32(base + 0x3C) as usize;
        // PE32+: the data directories begin 0x88 into the NT headers; the
        // import directory is the second.
        let imports = rd32(nt + 0x88 + 8) as usize;
        if imports == 0 {
            return None;
        }
        let mut desc = base + imports;
        loop {
            let (original, name_rva, first) = (rd32(desc), rd32(desc + 12), rd32(desc + 16));
            if name_rva == 0 {
                return None;
            }
            let dll_name = std::ffi::CStr::from_ptr((base + name_rva as usize) as *const std::ffi::c_char);
            if dll_name.to_bytes().eq_ignore_ascii_case(dll.to_bytes()) {
                let names = base + if original != 0 { original } else { first } as usize;
                let slots = base + first as usize;
                for i in 0.. {
                    let entry = ((names + i * 8) as *const u64).read_unaligned();
                    if entry == 0 {
                        break;
                    }
                    if entry >> 63 != 0 {
                        continue;
                    }
                    let by_name = base + (entry & 0x7FFF_FFFF) as usize + 2;
                    if std::ffi::CStr::from_ptr(by_name as *const std::ffi::c_char) == name {
                        let slot = (slots + i * 8) as *mut usize;
                        let mut old = 0;
                        if VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old) == 0 {
                            return None;
                        }
                        let host = slot.read_volatile();
                        slot.write_volatile(with);
                        VirtualProtect(slot.cast(), 8, old, &mut old);
                        return Some(host);
                    }
                }
                return None;
            }
            desc += 20;
        }
    }
}
