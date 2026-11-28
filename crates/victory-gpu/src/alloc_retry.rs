//! Allocations the system refuses are tried again for a while before the
//! process gives up. A player's 48 GB PC ended with "memory allocation of
//! 3076541 bytes failed" (28 Nov 2026): Rust aborts on the first refusal,
//! although the sweep lets go of spare memory once a second and the bury
//! worker drops what waited every 40 ms. A refusal is counted
//! ([`crate::residency::ALLOCATIONS_REFUSED`]: the next sweep then keeps
//! spare memory short and says so in the log), and the allocation is tried
//! again every 25 ms for up to 4 s. Nothing here allocates or takes a lock.
//! `VICTORY_ALLOC_RETRY=0`: not tried again (as before).
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, Ordering};

const TRIES: u32 = 160;
const WAIT_MS: u64 = 25;

/// 0 unknown, 1 on, 2 off. Read from the environment without allocating.
static STATE: AtomicU8 = AtomicU8::new(0);

fn on() -> bool {
    match STATE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let off = env_is_zero();
            STATE.store(if off { 2 } else { 1 }, Ordering::Relaxed);
            !off
        }
    }
}

/// Whether `VICTORY_ALLOC_RETRY` is "0", read with the system's call into a
/// buffer on the stack (`std::env` would allocate inside the allocator).
fn env_is_zero() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetEnvironmentVariableW(name: *const u16, buffer: *mut u16, size: u32) -> u32;
    }
    const NAME: [u16; 20] = {
        let bytes = *b"VICTORY_ALLOC_RETRY\0";
        let mut wide = [0u16; 20];
        let mut i = 0;
        while i < 20 {
            wide[i] = bytes[i] as u16;
            i += 1;
        }
        wide
    };
    let mut value = [0u16; 8];
    // SAFETY: a nul-terminated name and a buffer of the length given.
    let n = unsafe { GetEnvironmentVariableW(NAME.as_ptr(), value.as_mut_ptr(), value.len() as u32) };
    n == 1 && value[0] == u16::from(b'0')
}

/// `again` until it gives memory or the tries run out (null).
#[inline(never)]
#[cold]
pub(crate) fn retry(mut again: impl FnMut() -> *mut u8) -> *mut u8 {
    if !on() {
        return std::ptr::null_mut();
    }
    crate::residency::ALLOCATIONS_REFUSED.fetch_add(1, Ordering::Relaxed);
    for _ in 0..TRIES {
        std::thread::sleep(std::time::Duration::from_millis(WAIT_MS));
        let at = again();
        if !at.is_null() {
            return at;
        }
    }
    std::ptr::null_mut()
}

/// The system's allocator, with [`retry`] on a refusal (release builds; the
/// diagnostics builds' counting allocator does the same).
#[cfg_attr(feature = "diagnostics", allow(dead_code))]
pub(crate) struct Retrying;

// SAFETY: every call goes to the system's allocator unchanged; a refused one
// is only asked again.
unsafe impl GlobalAlloc for Retrying {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let at = unsafe { System.alloc(layout) };
        if at.is_null() { retry(|| unsafe { System.alloc(layout) }) } else { at }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let at = unsafe { System.alloc_zeroed(layout) };
        if at.is_null() { retry(|| unsafe { System.alloc_zeroed(layout) }) } else { at }
    }

    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(at, layout) };
    }

    unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on; a refused realloc leaves
        // the block as it was, so asking again is sound.
        let moved = unsafe { System.realloc(at, layout, size) };
        if moved.is_null() { retry(|| unsafe { System.realloc(at, layout, size) }) } else { moved }
    }
}

#[cfg(not(feature = "diagnostics"))]
#[global_allocator]
static HEAP: Retrying = Retrying;
