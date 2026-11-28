//! A book of the title's write faults on guarded view memory
//! (`VICTORY_FAULT_BOOK=<file>`; builds with diagnostics): which block, when
//! in the frame, what it waited for, and what else happened to the block
//! (taken by a refresh, asked about), for 40 frames from each of the times
//! `VICTORY_FAULT_BOOK_AT` gives (seconds after the start; 150,210,270 unless
//! given). Written as `<file>.<n>.bin`, records of 40 bytes: ticks of the
//! performance counter, two words, thread, what.
#![cfg(feature = "diagnostics")]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

/// What a record is of.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum What {
    /// A frame ended (a: its number).
    Frame = 0,
    /// A write fault opened block `a`; b: blocks opened with it (the upper
    /// 32 bits), nanoseconds of waiting for the guards' lock; c: nanoseconds
    /// with it.
    Fault = 1,
    /// Blocks `[a, b)` were found written, or never taken, and made read-only.
    Taken = 2,
    /// `[a, b)` was asked about and known unwritten by its bits.
    Known = 3,
    /// `[a, b)` was asked about block by block; c: how many were written.
    Asked = 4,
    /// We opened block `a` ourselves.
    Ours = 5,
    /// Another view of a page taken, block `a`, was made read-only.
    Alias = 6,
    /// A fault on block `a` that someone else had opened; b: nanoseconds.
    Open = 7,
    /// The instruction at `a` made the fault before (block `b`).
    Site = 9,
    /// A store (the instruction at `a`) was done beside the guard of block
    /// `b`.
    Stored = 10,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Record {
    at: i64,
    a: u64,
    b: u64,
    c: u64,
    thread: u32,
    what: u32,
}

static ON: AtomicBool = AtomicBool::new(false);
static BOOK: Mutex<Vec<Record>> = Mutex::new(Vec::new());

fn ticks() -> i64 {
    let mut t = 0;
    // SAFETY: an out pointer of the right size.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut t) };
    t
}

/// Whether records are kept now.
pub(crate) fn on() -> bool {
    ON.load(Relaxed)
}

pub(crate) fn note(what: What, a: usize, b: usize, c: u64) {
    if !on() {
        return;
    }
    // SAFETY: a question about this thread.
    let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let record = Record { at: ticks(), a: a as u64, b: b as u64, c, thread, what: what as u32 };
    BOOK.lock().unwrap_or_else(|e| e.into_inner()).push(record);
}

/// A frame ended.
pub(crate) fn frame() {
    const FRAMES: u64 = 40;
    static SETTINGS: OnceLock<Option<(String, Vec<u64>)>> = OnceLock::new();
    static BEGAN: OnceLock<crate::clock::Instant> = OnceLock::new();
    static LEFT: AtomicU64 = AtomicU64::new(0);
    static MADE: AtomicU64 = AtomicU64::new(0);
    let began = *BEGAN.get_or_init(crate::clock::Instant::now);
    let Some((file, times)) = SETTINGS.get_or_init(|| {
        let file = std::env::var("VICTORY_FAULT_BOOK").ok().filter(|f| !f.is_empty())?;
        let times = std::env::var("VICTORY_FAULT_BOOK_AT").unwrap_or_else(|_| "150,210,270".into());
        Some((file, times.split(',').filter_map(|t| t.trim().parse().ok()).collect()))
    }) else {
        return;
    };
    note(What::Frame, crate::frame::current() as usize, 0, 0);
    if on() {
        if LEFT.fetch_sub(1, Relaxed) > 1 {
            return;
        }
        ON.store(false, Relaxed);
        let records = std::mem::take(&mut *BOOK.lock().unwrap_or_else(|e| e.into_inner()));
        let file = format!("{file}.{}.bin", MADE.fetch_add(1, Relaxed));
        std::thread::spawn(move || {
            // SAFETY: plain data, every byte of it written.
            let bytes = unsafe { std::slice::from_raw_parts(records.as_ptr().cast::<u8>(), size_of_val(&records[..])) };
            match std::fs::write(&file, bytes) {
                Ok(()) => tracing::info!(file, records = records.len(), "book of faults written"),
                Err(e) => tracing::warn!(file, %e, "book of faults not written"),
            }
        });
    } else if times.get(MADE.load(Relaxed) as usize).is_some_and(|&t| began.elapsed().as_secs() >= t) {
        LEFT.store(FRAMES, Relaxed);
        ON.store(true, Relaxed);
    }
}
