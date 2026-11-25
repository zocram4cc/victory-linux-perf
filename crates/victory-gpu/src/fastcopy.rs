//! Reading title memory without a system call (phase 1.8 of the README).
//!
//! Title memory can have pages with no backing (reserved, never committed,
//! or unmapped by the title's page mapping), so it was always read through
//! the kernel (`ReadProcessMemory`), which reports such a page instead of
//! faulting. Under Wine that call is expensive per call: in scene A it was
//! about a quarter of the submitting thread (`virtual_check_buffer_for_write`
//! walking the destination, a second copy in libc, the syscall dispatch).
//!
//! Here the copy is a plain `rep movsb` in a small assembly routine. If a
//! source page is not there, the access violation is caught by a vectored
//! handler of ours: it sees the fault at that one instruction, moves the
//! thread to the routine's failure exit, and the routine returns false. The
//! caller then reads as before, through the kernel, piece by piece. This is
//! the technique kernels use to copy from user memory.
//!
//! `VICTORY_F1_FAST_READ=0` turns it off. The copy-back's writes into title
//! memory use the same routine (`VICTORY_F1_FAST_WRITE=0` turns that off).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(all(windows, target_arch = "x86_64"))]
core::arch::global_asm!(
    ".globl victory_fast_copy",
    ".globl victory_fast_copy_at",
    ".globl victory_fast_copy_fail",
    // fn victory_fast_copy(dst: rcx, src: rdx, len: r8) -> eax (1 copied, 0 faulted)
    "victory_fast_copy:",
    "    push rdi",
    "    push rsi",
    "    mov rdi, rcx",
    "    mov rsi, rdx",
    "    mov rcx, r8",
    // The one instruction that may fault on a missing source page.
    "victory_fast_copy_at:",
    "    rep movsb",
    "    pop rsi",
    "    pop rdi",
    "    mov eax, 1",
    "    ret",
    // Where the handler sends a faulting copy: the stack is as it was
    // after the two pushes, since `rep movsb` pushes nothing.
    "victory_fast_copy_fail:",
    "    pop rsi",
    "    pop rdi",
    "    xor eax, eax",
    "    ret",
);

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "C" {
    fn victory_fast_copy(dst: *mut u8, src: *const u8, len: usize) -> u32;
    static victory_fast_copy_at: u8;
    static victory_fast_copy_fail: u8;
}

/// Whether reads take the fast path (`VICTORY_F1_FAST_READ`, on unless `0`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_F1_FAST_READ").is_none_or(|v| v != "0"))
}

/// Whether writes into title memory (the copy-back) take the fast path too
/// (`VICTORY_F1_FAST_WRITE`, on unless `0`). A write that faults (a page
/// not there, a guard page, a page the title made read-only) goes to the
/// kernel as before; our own read-only guards on view memory are opened by
/// the caller first (`crate::watch::writing`), or by their handler if one
/// is met, as for a write of the title's.
pub(crate) fn writes_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| on() && std::env::var_os("VICTORY_F1_FAST_WRITE").is_none_or(|v| v != "0"))
}

/// Since the last frame line: fast copies, bytes, and copies that faulted
/// (and went to the kernel instead); reads and writes together.
pub(crate) static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// `STATUS_GUARD_PAGE_VIOLATION`.
#[cfg(all(windows, target_arch = "x86_64"))]
const STATUS_GUARD_PAGE_VIOLATION: i32 = 0x8000_0001_u32 as i32;

/// Puts `PAGE_GUARD` back on the page holding `at`, which a read of ours
/// just took it from.
#[cfg(all(windows, target_arch = "x86_64"))]
fn restore_guard(at: usize) {
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, PAGE_GUARD, VirtualProtect, VirtualQuery};
    // SAFETY: queries and protects one page that is committed (it faulted
    // as a guard page), keeping its protection otherwise.
    unsafe {
        let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        if VirtualQuery(at as *const _, &mut info, size_of_val(&info)) == 0 {
            return;
        }
        let mut old = 0;
        VirtualProtect((at & !0xfff) as *const _, 0x1000, info.Protect | PAGE_GUARD, &mut old);
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_fault(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        unsafe {
            let record = &*(*info).ExceptionRecord;
            let context = &mut *(*info).ContextRecord;
            if context.Rip as usize != &raw const victory_fast_copy_at as usize {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            let code = record.ExceptionCode;
            if code == STATUS_GUARD_PAGE_VIOLATION && record.NumberParameters >= 2 {
                // A guard page (a thread's stack end, or the title's own):
                // the fault took its guard away. It is put back, so the
                // owner sees its guard page as before, and the copy goes
                // through the kernel, which reads around it.
                restore_guard(record.ExceptionInformation[1]);
            } else if code != EXCEPTION_ACCESS_VIOLATION as NTSTATUS {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            context.Rip = &raw const victory_fast_copy_fail as u64;
        }
        STATS[2].fetch_add(1, Ordering::Relaxed);
        EXCEPTION_CONTINUE_EXECUTION
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: the handler only redirects faults at our own copy instruction.
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_fault));
    });
}

/// Copies `len` bytes from `src` to `dst`, one of them title memory
/// (possibly with missing pages), the other ours. False if a page of the
/// title's side cannot be read or written; part of the bytes may have been
/// copied then.
pub(crate) fn copy(dst: *mut u8, src: *const u8, len: usize) -> bool {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        install();
        STATS[0].fetch_add(1, Ordering::Relaxed);
        STATS[1].fetch_add(len as u64, Ordering::Relaxed);
        // SAFETY: the destination is ours and holds `len` bytes; a fault
        // on the source is caught by `on_fault` and reported as false.
        return unsafe { victory_fast_copy(dst, src, len) } != 0;
    }
    #[allow(unreachable_code)]
    {
        let _ = (dst, src, len);
        false
    }
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };

    #[test]
    fn copies_mapped_memory_and_fails_on_missing_pages() {
        // SAFETY: memory of our own, reserved, the first page committed.
        unsafe {
            let base = VirtualAlloc(std::ptr::null(), 0x20000, MEM_RESERVE, PAGE_READWRITE).cast::<u8>();
            assert!(!base.is_null());
            assert!(!VirtualAlloc(base.cast(), 0x1000, MEM_COMMIT, PAGE_READWRITE).is_null());
            for i in 0..0x1000 {
                *base.add(i) = i as u8;
            }
            let mut out = vec![0u8; 0x2000];
            assert!(super::copy(out.as_mut_ptr(), base, 0x1000));
            assert!(out[..0x1000].iter().enumerate().all(|(i, &b)| b == i as u8));
            // Runs into the reserved, uncommitted second page.
            assert!(!super::copy(out.as_mut_ptr(), base, 0x2000));
            // Wholly inside it, and again afterwards: the routine stays usable.
            assert!(!super::copy(out.as_mut_ptr(), base.add(0x1000), 16));
            assert!(super::copy(out.as_mut_ptr(), base.add(8), 16));
            assert_eq!(out[0], 8);
            VirtualFree(base.cast(), 0, MEM_RELEASE);
        }
    }

    #[test]
    fn writes_fail_on_missing_and_read_only_pages() {
        use windows_sys::Win32::System::Memory::PAGE_READONLY;
        // SAFETY: memory of our own: a writable page, a read-only one, and
        // a reserved one.
        unsafe {
            let base = VirtualAlloc(std::ptr::null(), 0x3000, MEM_RESERVE, PAGE_READWRITE).cast::<u8>();
            assert!(!VirtualAlloc(base.cast(), 0x1000, MEM_COMMIT, PAGE_READWRITE).is_null());
            assert!(!VirtualAlloc(base.add(0x1000).cast(), 0x1000, MEM_COMMIT, PAGE_READONLY).is_null());
            let src = vec![7u8; 0x3000];
            assert!(super::copy(base, src.as_ptr(), 0x1000));
            assert_eq!(*base.add(0xfff), 7);
            assert!(!super::copy(base.add(0x1000), src.as_ptr(), 16));
            assert!(!super::copy(base.add(0x2000), src.as_ptr(), 16));
            VirtualFree(base.cast(), 0, MEM_RELEASE);
        }
    }

    #[test]
    fn a_fast_write_is_seen_by_write_watching() {
        use windows_sys::Win32::System::Memory::GetWriteWatch;
        const MEM_WRITE_WATCH: u32 = 0x0020_0000;
        // SAFETY: write-watched memory of our own; the answer buffer holds
        // `count` entries.
        unsafe {
            let base = VirtualAlloc(std::ptr::null(), 0x4000, MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH, PAGE_READWRITE).cast::<u8>();
            assert!(!base.is_null());
            let mut found = [std::ptr::null_mut::<core::ffi::c_void>(); 4];
            let (mut count, mut granularity) = (4usize, 0u32);
            GetWriteWatch(1, base.cast(), 0x4000, found.as_mut_ptr(), &mut count, &mut granularity);
            let src = [9u8; 16];
            assert!(super::copy(base.add(0x2008), src.as_ptr(), 16));
            count = 4;
            GetWriteWatch(1, base.cast(), 0x4000, found.as_mut_ptr(), &mut count, &mut granularity);
            assert_eq!(count, 1);
            assert_eq!(found[0] as usize, base as usize + 0x2000);
            VirtualFree(base.cast(), 0, MEM_RELEASE);
        }
    }

    #[test]
    fn a_guard_page_fails_the_copy_and_keeps_its_guard() {
        use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, PAGE_GUARD, VirtualQuery};
        // SAFETY: two committed pages of our own, the second a guard page.
        unsafe {
            let base = VirtualAlloc(std::ptr::null(), 0x2000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE).cast::<u8>();
            assert!(!base.is_null());
            assert!(!VirtualAlloc(base.add(0x1000).cast(), 0x1000, MEM_COMMIT, PAGE_READWRITE | PAGE_GUARD).is_null());
            let mut out = vec![0u8; 0x2000];
            assert!(!super::copy(out.as_mut_ptr(), base, 0x2000));
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            VirtualQuery(base.add(0x1000).cast(), &mut info, size_of_val(&info));
            assert_ne!(info.Protect & PAGE_GUARD, 0, "the guard was put back");
            // Twice: the guard still fails the copy.
            assert!(!super::copy(out.as_mut_ptr(), base.add(0x1000), 16));
            VirtualFree(base.cast(), 0, MEM_RELEASE);
        }
    }
}
