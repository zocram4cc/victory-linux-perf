//! The title's writes into guarded view memory that do not go through a
//! fault that opens the block (`crate::watch`): its bytes are put in through
//! our own view of the same pages (`crate::inplace`), which no guard covers,
//! and the block is told as written (`crate::watch::put_beside`). The block
//! stays read-only, so neither this write nor the block's next taking changes
//! a protection.
//!
//! A change of protection is some 5 us in the game's process, one at a time
//! whoever asks; a frame had 590 write faults, and made as many blocks
//! read-only again (run faults-1). What faults is, by the time spent in it,
//! for a third the title's `memcpy`, and for most of the rest single stores
//! into objects that lie in memory some buffer's copy covers: a flag, a
//! reference count, the link of a free list.
//!
//! - [`import`]: the title's `memcpy`, `memmove` and `memset` are ours
//!   (`VICTORY_COPY_BESIDE=1`; off unless asked for). They look at the bits
//!   of the blocks they write and go to the runtime's own, but for the parts
//!   that lie in guarded blocks.
//! - [`store`]: a store that faults is done here, and the thread goes on
//!   behind it (`VICTORY_STORE_BESIDE=1`; off unless asked for). Nothing of
//!   the title's code is known ahead: the instruction is read where it
//!   faulted, and one that is not understood opens the block as before. So
//!   does a block's fifth store since it was taken, and one at a place in the
//!   code most of whose blocks come to that ([`SITES`]): code that fills a
//!   block would fault at every store.

mod decode;

use crate::watch::Beside;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

/// Diagnostics, since the start: copies of the title's that put bytes beside
/// the guards, blocks they did so in, blocks they opened instead; stores done
/// beside the guards, faulting stores not understood, and blocks opened
/// because too many came; stores and parts of copies that left the memory as
/// it was, and were not made; descriptors copied onto themselves.
pub(crate) static DONE: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];

fn add(what: usize) {
    DONE[what].fetch_add(1, Relaxed);
}

/// A switch that is off unless its variable is `1` (or `VICTORY_AB` names it:
/// on and off while the title runs).
macro_rules! asked_for {
    ($name:literal) => {{
        static ASKED: std::sync::OnceLock<Result<bool, u32>> = std::sync::OnceLock::new();
        let asked = ASKED.get_or_init(|| {
            #[cfg(feature = "diagnostics")]
            if let Some(bit) = crate::ab::bit($name) {
                return Err(bit);
            }
            Ok(std::env::var_os($name).is_some_and(|v| v == "1"))
        });
        match *asked {
            Ok(on) => on,
            #[cfg(feature = "diagnostics")]
            Err(bit) => crate::ab::on(bit),
            #[cfg(not(feature = "diagnostics"))]
            Err(_) => false,
        }
    }};
}

fn copies_on() -> bool {
    #[cfg(test)]
    if ON_IN_TEST.get() {
        return true;
    }
    asked_for!("VICTORY_COPY_BESIDE")
}

fn stores_on() -> bool {
    #[cfg(test)]
    if ON_IN_TEST.get() {
        return true;
    }
    asked_for!("VICTORY_STORE_BESIDE")
}

#[cfg(test)]
thread_local! {
    /// Both, for this thread's writes.
    pub(crate) static ON_IN_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The runtime's own `memcpy`, `memmove` and `memset`.
static REAL: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

type Copy = unsafe extern "system-unwind" fn(*mut u8, *const u8, usize) -> *mut u8;
type Set = unsafe extern "system-unwind" fn(*mut u8, i32, usize) -> *mut u8;

/// What the title's import of `name`, which the host has at `address`, is
/// bound to.
pub fn import(name: &str, address: usize) -> usize {
    let (which, ours) = match name {
        "memcpy" => (0, copy as *const () as usize),
        "memmove" => (1, shift as *const () as usize),
        "memset" => (2, set as *const () as usize),
        _ => return address,
    };
    #[cfg(feature = "diagnostics")]
    let on = crate::ab::bit("VICTORY_COPY_BESIDE").is_some() || copies_on();
    #[cfg(not(feature = "diagnostics"))]
    let on = copies_on();
    if !on {
        return address;
    }
    REAL[which].store(address, Relaxed);
    ours
}

#[cfg(test)]
pub(crate) fn copy_for_test(real: usize) -> usize {
    REAL[0].store(real, Relaxed);
    copy as *const () as usize
}

/// The most a block takes of copies beside its guard before it is opened; a
/// store counts as [`A_STORE`].
pub(crate) const MOST: u32 = 32;
const A_STORE: u32 = 8;

unsafe extern "system-unwind" fn copy(to: *mut u8, from: *const u8, n: usize) -> *mut u8 {
    // SAFETY: the title's copy, as it asked for it, by the function it asked.
    unsafe { copied(to, from, n, REAL[0].load(Relaxed)) }
}

unsafe extern "system-unwind" fn shift(to: *mut u8, from: *const u8, n: usize) -> *mut u8 {
    // SAFETY: as above.
    unsafe { copied(to, from, n, REAL[1].load(Relaxed)) }
}

/// # Safety
/// `real` copies as `memcpy` (or `memmove`) does, `to`, `from` and `n` are
/// the title's.
#[inline]
unsafe fn copied(to: *mut u8, from: *const u8, n: usize, real: usize) -> *mut u8 {
    // SAFETY: the host's function, noted by `import` before the title ran.
    let real: Copy = unsafe { std::mem::transmute(real) };
    if n == 0 || !crate::watch::guards_any(to as usize, n) || !copies_on() || overlap(to as usize, from as usize, n) {
        // SAFETY: by the caller.
        return unsafe { real(to, from, n) };
    }
    let same = |at: usize, offset: usize, len: usize| {
        // SAFETY: both are the title's, the one read-only by our hand, the other read by it next.
        unsafe { std::slice::from_raw_parts(at as *const u8, len) == std::slice::from_raw_parts(from.add(offset), len) }
    };
    in_pieces(to as usize, n, same, |at, offset, len| {
        // SAFETY: a part of what the title asked for; or the same bytes at
        // our view of the pages.
        unsafe { real(at as *mut u8, from.add(offset), len) };
    });
    to
}

unsafe extern "system-unwind" fn set(to: *mut u8, byte: i32, n: usize) -> *mut u8 {
    // SAFETY: as in `copied`.
    let real: Set = unsafe { std::mem::transmute(REAL[2].load(Relaxed)) };
    if n == 0 || !crate::watch::guards_any(to as usize, n) || !copies_on() {
        // SAFETY: the title's, as it asked for it.
        return unsafe { real(to, byte, n) };
    }
    let same = |at: usize, _: usize, len: usize| {
        // SAFETY: the title's, read-only by our hand.
        unsafe { std::slice::from_raw_parts(at as *const u8, len) }.iter().all(|&there| there == byte as u8)
    };
    in_pieces(to as usize, n, same, |at, _, len| {
        // SAFETY: as in `copied`.
        unsafe { real(at as *mut u8, byte, len) };
    });
    to
}

/// Bytes of the title's that we put into its memory for it (a descriptor it
/// has copied): as a copy of its own, but what is there already is never
/// written (`VICTORY_SAME_RECORDS=0`: always).
///
/// # Safety
/// `[from, from + n)` and `[to, to + n)` are the title's.
pub(crate) unsafe fn put(to: *mut u8, from: *const u8, n: usize) {
    // SAFETY: by the caller.
    unsafe {
        let there = || std::slice::from_raw_parts(to.cast_const(), n) == std::slice::from_raw_parts(from, n);
        if crate::toggled!("VICTORY_SAME_RECORDS") && there() {
            add(8);
            return;
        }
        if n == 0 || !copies_on() || !crate::watch::guards_any(to as usize, n) || overlap(to as usize, from as usize, n) {
            std::ptr::copy(from, to, n);
            return;
        }
        in_pieces(to as usize, n, |_, _, _| false, |at, offset, len| std::ptr::copy_nonoverlapping(from.add(offset), at as *mut u8, len));
    }
}

/// Whether the two ranges meet, if only in a block (of which only one might
/// be written at another address).
fn overlap(to: usize, from: usize, n: usize) -> bool {
    const BLOCK: usize = crate::watch::PAGE;
    let (first, last) = (to / BLOCK * BLOCK, to.saturating_add(n).next_multiple_of(BLOCK));
    from < last && first < from.saturating_add(n)
}

/// Does `[to, to + n)` block by block: `put(at, offset, len)` with the
/// address to write at, the title's or ours.
///
/// What is there already (`same`, of a part in one guarded block) is not
/// written.
fn in_pieces(to: usize, n: usize, same: impl Fn(usize, usize, usize) -> bool, put: impl Fn(usize, usize, usize)) {
    const BLOCK: usize = crate::watch::PAGE;
    add(0);
    let mut offset = 0;
    while offset < n {
        let at = to + offset;
        let len = (BLOCK - at % BLOCK).min(n - offset);
        if crate::watch::guards_any(at, len) {
            if same(at, offset, len) {
                add(7);
                offset += len;
                continue;
            }
            match crate::watch::put_beside(at / BLOCK, 1, |delta| put(at.wrapping_add(delta), offset, len)) {
                Beside::Done => {
                    add(1);
                    offset += len;
                    continue;
                }
                // Opened with no fault: the next copies here are plain ones.
                _ => crate::watch::opened_for_writing(at),
            }
            add(2);
        }
        put(at, offset, len);
        offset += len;
    }
}

/// Places of the title's code whose stores faulted, by a hash of the address:
/// the stores done beside the guards (the low half) and the blocks that had
/// too many of them (the high half).
static SITES: [AtomicU64; 1 << 10] = [const { AtomicU64::new(0) }; 1 << 10];

fn site(rip: usize) -> &'static AtomicU64 {
    &SITES[(rip as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize >> 54 & 1023]
}

/// Whether a site's stores come one to a block, as far as is known.
fn sparse(counts: u64) -> bool {
    let (done, full) = (counts & 0xffff_ffff, counts >> 32);
    full < 4 || full * 12 < done
}

/// Faulting stores by their place in the title's code: the place, and how
/// many left the memory as it was (the low half) or did not (the high one).
static LEARNED: [(AtomicUsize, AtomicU64); 1 << 9] = [const { (AtomicUsize::new(0), AtomicU64::new(0)) }; 1 << 9];

/// A store at `rip` would leave the memory as it is, or not. With
/// `VICTORY_WRITE_SITES=<file>`, a place that can be made to look first
/// (`can_look`) and mostly left it is written into that file, for the next
/// start (`victory_loader::write_sites`).
fn learn(rip: usize, same: bool, can_look: bool) {
    let slot = &LEARNED[(rip as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize >> 55 & 511];
    let there = slot.0.compare_exchange(0, rip, Relaxed, Relaxed).unwrap_or_else(|there| there);
    if there != 0 && there != rip {
        return;
    }
    let counts = slot.1.fetch_add(if same { 1 } else { 1 << 32 }, Relaxed);
    let (stayed, changed) = (counts & 0xffff_ffff, counts >> 32);
    if !same || !can_look || stayed != 256 || changed * 4 > stayed {
        return;
    }
    // (Nothing more once the file has been refused.)
    if std::env::var_os("VICTORY_WRITE_SITES").is_none()
        || SITES_FILE.get().is_some_and(Option::is_none)
    {
        return;
    }
    // (Not here, under a fault of the title's.)
    std::thread::spawn(move || {
        let Some(file) = SITES_FILE
            .get_or_init(|| outside_repo(std::env::var_os("VICTORY_WRITE_SITES")?.into()))
        else {
            return;
        };
        let Some((image, offset)) = place(rip) else {
            return;
        };
        use std::io::Write;
        let line = format!("{image} {offset:x}\n");
        let known = std::fs::read_to_string(file).unwrap_or_default();
        if known.lines().any(|l| l.starts_with(line.trim_end())) {
            return;
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(file) {
            let _ = file.write_all(line.as_bytes());
        }
        tracing::info!(image, offset = format_args!("{offset:#x}"), stayed, changed, "a write of the title's that mostly changes nothing, noted");
    });
}

/// The file `VICTORY_WRITE_SITES` names, once looked at: `None` if it may not
/// be written.
static SITES_FILE: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// `file`, if it is outside every source tree: its lines are places in the
/// title's code. Said once if not.
fn outside_repo(file: std::path::PathBuf) -> Option<std::path::PathBuf> {
    match victory_core::ensure_outside_repo(&file) {
        Ok(()) => Some(file),
        Err(error) => {
            tracing::warn!(%error, "VICTORY_WRITE_SITES: not written");
            None
        }
    }
}

/// The file name of the image `address` is in, and where in it.
fn place(address: usize) -> Option<(String, usize)> {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    };
    const FLAGS: u32 = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    let mut module = std::ptr::null_mut();
    let mut name = [0u16; 1024];
    // SAFETY: an address, and room for the answers.
    let n = unsafe {
        if GetModuleHandleExW(FLAGS, address as *const u16, &mut module) == 0 {
            return None;
        }
        GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32)
    } as usize;
    let path = String::from_utf16_lossy(&name[..n]);
    let file = path.rsplit(['\\', '/']).next()?.to_owned();
    (!file.is_empty()).then(|| (file, address - module as usize))
}

/// The places with the most faulting stores, for the log: offset in its
/// image, how many left the memory as it was, how many did not.
#[cfg(feature = "diagnostics")]
pub(crate) fn busiest() -> String {
    let mut all: Vec<(u64, u64, usize)> = LEARNED
        .iter()
        .map(|(rip, counts)| (counts.load(Relaxed), rip.load(Relaxed)))
        .filter(|s| s.1 != 0)
        .map(|(counts, rip)| (counts & 0xffff_ffff, counts >> 32, rip))
        .collect();
    all.sort_unstable_by_key(|s| std::cmp::Reverse(s.0 + s.1));
    let of = |&(stayed, changed, rip): &(u64, u64, usize)| {
        let (image, offset) = place(rip).unwrap_or_default();
        format!("{}+{offset:x} {stayed}/{changed}", image.chars().skip(14).take(4).collect::<String>())
    };
    all.iter().take(16).map(of).collect::<Vec<_>>().join(", ")
}

/// Does the store that faulted at `at`, a byte of a guarded block, beside the
/// guards; `false` if it is to open the block (and nothing was done).
pub(crate) fn store(context: &mut windows_sys::Win32::System::Diagnostics::Debug::CONTEXT, at: usize) -> bool {
    if !stores_on() {
        return false;
    }
    let rip = context.Rip as usize;
    let site = site(rip);
    let counts = site.load(Relaxed);
    let mut code = [0u8; 15];
    // (To the end of the page if the next is not there.)
    let left = 4096 - rip % 4096;
    let there = crate::host::read_into(rip as *const u8, &mut code) || {
        let n = left.min(15);
        n < 15 && crate::host::read_into(rip as *const u8, &mut code[..n])
    };
    let Some(store) = there.then(|| decode::store(&code, context)).flatten() else {
        add(4);
        return false;
    };
    const BLOCK: usize = crate::watch::PAGE;
    let end = store.at.wrapping_add(store.size);
    if !(store.at..end).contains(&at) || store.at / BLOCK != (end - 1) / BLOCK {
        add(4);
        return false;
    }
    // SAFETY: a guarded block's, which can be read.
    if let Some((same, flags)) = unsafe { store.same(context) } {
        learn(rip, same, store.can_look(&code));
        if same && crate::watch::nothing_put(store.at / BLOCK) {
            context.EFlags = flags;
            context.Rip += store.len as u64;
            add(6);
            return true;
        }
    }
    if !sparse(counts) {
        // Looked at anew some time.
        if site.fetch_add(1 << 32, Relaxed) >> 32 > 4096 {
            site.store(0, Relaxed);
        }
        return false;
    }
    let mut done = false;
    let put = crate::watch::put_beside(store.at / BLOCK, A_STORE, |delta| {
        // SAFETY: the bytes the title is storing to, at our view of them.
        done = unsafe { store.run(context, store.at.wrapping_add(delta)) };
    });
    if put != Beside::Done {
        if put == Beside::Full {
            site.fetch_add(1 << 32, Relaxed);
            add(5);
        }
        return false;
    }
    if !done {
        // (A locked one that is not aligned: told as written for nothing.)
        add(4);
        return false;
    }
    context.Rip += store.len as u64;
    site.fetch_add(1, Relaxed);
    add(3);
    #[cfg(feature = "diagnostics")]
    crate::faultbook::note(crate::faultbook::What::Stored, rip, at / BLOCK, 0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_that_fill_blocks_are_told_from_others() {
        assert!(sparse(0) && sparse(1000) && sparse(3 << 32));
        assert!(!sparse(4 << 32 | 16) && sparse(4 << 32 | 60));
    }

    #[test]
    fn copies_that_meet_are_left_to_the_runtime() {
        const B: usize = crate::watch::PAGE;
        assert!(overlap(10 * B + 5, 10 * B + 900, 8), "in one block");
        assert!(!overlap(10 * B + 5, 12 * B, 8) && !overlap(10 * B, 9 * B, B));
        assert!(overlap(10 * B, 9 * B, B + 1));
    }

    #[test]
    fn sites_are_not_written_into_the_source_tree() {
        let inside = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("victory.txt");
        assert_eq!(outside_repo(inside), None);
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("victory.txt");
        assert_eq!(outside_repo(outside.clone()), Some(outside));
    }
}
