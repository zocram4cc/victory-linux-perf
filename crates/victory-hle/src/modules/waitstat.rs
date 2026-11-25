//! The title's waits, timed by thread and by the title address that called
//! them, logged every 5 s (`VICTORY_MEDIR=1`, phase 0.4 of the README). A
//! wait on an event the title gave `SetEventOnCompletion` is tagged
//! `[fence]`: time the title spends waiting for the GPU.
#![allow(non_snake_case)]
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Instant;

use crate::{HleModule, hle_export};

type Key = (u32, usize, &'static str, bool);
static STATS: Mutex<Option<HashMap<Key, (u64, u64)>>> = Mutex::new(None);
static LAST: Mutex<Option<Instant>> = Mutex::new(None);

fn caller() -> usize {
    let mut frames = [std::ptr::null_mut::<c_void>(); 1];
    // SAFETY: room for one frame; skip ours (and the inlined helper's).
    let n = unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace(2, 1, frames.as_mut_ptr(), std::ptr::null_mut())
    };
    if n == 0 { 0 } else { frames[0] as usize }
}

fn record(at: usize, kind: &'static str, fence: bool, start: Instant) {
    let ns = start.elapsed().as_nanos() as u64;
    let mut stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: no arguments.
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let e = stats.get_or_insert_with(HashMap::new).entry((tid, at, kind, fence)).or_insert((0, 0));
    e.0 += 1;
    e.1 += ns;
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let first = *last.get_or_insert(now);
    let secs = now.duration_since(first).as_secs_f64();
    if secs >= 5.0 {
        *last = Some(now);
        let base = super::kernel::GetModuleHandleW(std::ptr::null()) as usize;
        let all: Vec<_> = stats.take().unwrap_or_default().into_iter().collect();
        let rva = |at: usize| if at >= base && at < base + (1 << 31) { format!("title+{:#x}", at - base) } else { format!("{at:#x}") };
        // Per thread: time spent waiting a second; threads that wait less
        // than all the time are the ones that work, and the frame's chain.
        let mut threads: HashMap<u32, (u64, Vec<(u64, u64, usize, &'static str, bool)>)> = HashMap::new();
        for ((tid, at, kind, fence), (count, ns)) in all {
            let t = threads.entry(tid).or_default();
            t.0 += ns;
            t.1.push((ns, count, at, kind, fence));
        }
        let mut threads: Vec<_> = threads.into_iter().collect();
        threads.sort_by_key(|(_, (ns, _))| *ns);
        tracing::info!("medir: waits by thread ({} threads), the least waiting first:", threads.len());
        for (tid, (ns, mut sites)) in threads.into_iter().take(8) {
            sites.sort_by_key(|s| std::cmp::Reverse(s.0));
            let top: Vec<String> = sites.iter().take(4).map(|(ns, count, at, kind, fence)| format!(
                "{:.0} ms/s {:.0}/s {kind}{} {}", *ns as f64 / 1e6 / secs, *count as f64 / secs,
                if *fence { "[fence]" } else { "" }, rva(*at))).collect();
            tracing::info!("medir:   thread {tid}: waits {:.0} ms/s: {}", ns as f64 / 1e6 / secs, top.join(" | "));
        }
    }
}

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading as T;

pub extern "system" fn WaitForSingleObject(h: HANDLE, ms: u32) -> u32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::WaitForSingleObject(h, ms) };
    record(at, "WaitForSingleObject", victory_gpu::fence::medir_is_fence_event(h as usize), t); r
}
pub extern "system" fn WaitForSingleObjectEx(h: HANDLE, ms: u32, alertable: i32) -> u32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::WaitForSingleObjectEx(h, ms, alertable) };
    record(at, "WaitForSingleObjectEx", victory_gpu::fence::medir_is_fence_event(h as usize), t); r
}
fn any_fence(n: u32, hs: *const HANDLE) -> bool {
    if hs.is_null() { return false; }
    // SAFETY: the title passes n handles.
    unsafe { std::slice::from_raw_parts(hs, n as usize) }.iter().any(|&h| victory_gpu::fence::medir_is_fence_event(h as usize))
}
pub extern "system" fn WaitForMultipleObjects(n: u32, hs: *const HANDLE, all: i32, ms: u32) -> u32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::WaitForMultipleObjects(n, hs, all, ms) };
    record(at, "WaitForMultipleObjects", any_fence(n, hs), t); r
}
pub extern "system" fn WaitForMultipleObjectsEx(n: u32, hs: *const HANDLE, all: i32, ms: u32, alertable: i32) -> u32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::WaitForMultipleObjectsEx(n, hs, all, ms, alertable) };
    record(at, "WaitForMultipleObjectsEx", any_fence(n, hs), t); r
}
pub extern "system" fn SleepEx(ms: u32, alertable: i32) -> u32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::SleepEx(ms, alertable) };
    record(at, if ms == 0 { "SleepEx(0)" } else { "SleepEx" }, false, t); r
}
pub extern "system" fn Sleep(ms: u32) {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    unsafe { T::Sleep(ms) };
    record(at, if ms == 0 { "Sleep(0)" } else { "Sleep" }, false, t);
}
pub extern "system" fn WaitOnAddress(a: *const c_void, c: *const c_void, size: usize, ms: u32) -> i32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::WaitOnAddress(a, c, size, ms) };
    record(at, "WaitOnAddress", false, t); r
}
pub extern "system" fn SleepConditionVariableSRW(cv: *mut T::CONDITION_VARIABLE, l: *mut T::SRWLOCK, ms: u32, f: u32) -> i32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::SleepConditionVariableSRW(cv, l, ms, f) };
    record(at, "SleepConditionVariableSRW", false, t); r
}
pub extern "system" fn SleepConditionVariableCS(cv: *mut T::CONDITION_VARIABLE, cs: *mut T::CRITICAL_SECTION, ms: u32) -> i32 {
    let at = caller(); let t = Instant::now();
    // SAFETY: the title's own arguments.
    let r = unsafe { T::SleepConditionVariableCS(cv, cs, ms) };
    record(at, "SleepConditionVariableCS", false, t); r
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "waitstat",
        dlls: &["kernel32.dll", "kernelbase.dll", "api-ms-win-core-synch-*"],
        exports: vec![
            hle_export!(WaitForSingleObject),
            hle_export!(WaitForSingleObjectEx),
            hle_export!(WaitForMultipleObjects),
            hle_export!(WaitForMultipleObjectsEx),
            hle_export!(SleepEx),
            hle_export!(Sleep),
            hle_export!(WaitOnAddress),
            hle_export!(SleepConditionVariableSRW),
            hle_export!(SleepConditionVariableCS),
        ],
    }
}
