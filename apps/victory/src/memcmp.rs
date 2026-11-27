//! `memcmp` of our own, in place of the C runtime's, for every comparison of
//! bytes in this program (slices compared with `==` among them).
//!
//! Under Wine the C runtime's `memcmp` compares eight bytes at a step (its
//! `ucrtbase.dll`), where Windows' compares 32: the threads that compare host
//! copies of title memory with what the title wrote spent 6.7 % of the
//! process's processor time in that loop (RTX 5060 prologue, run prof3).
//! Defined here, in the executable, the linker takes it before the import
//! library's; other modules keep their own. Sixteen bytes at a step with
//! SSE2, which every x86-64 processor has, four steps at a time.
//!
//! On Windows itself the C runtime's own (`vcruntime140.dll`, 32 bytes a
//! step with AVX2) is called instead, found once: what runs there is what
//! ran before this file. Ours is used under Wine, or if that one is not found.

#[cfg(all(windows, target_arch = "x86_64"))]
type Compare = unsafe extern "C" fn(*const core::ffi::c_void, *const core::ffi::c_void, usize) -> i32;

/// The `memcmp` to call: 0 not decided yet, 1 ours ([`sse2`]), else the
/// address of the C runtime's.
#[cfg(all(windows, target_arch = "x86_64"))]
static CHOSEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(all(windows, target_arch = "x86_64"))]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut core::ffi::c_void;
    fn LoadLibraryA(name: *const u8) -> *mut core::ffi::c_void;
    fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> usize;
}

/// Under Wine (its ntdll exports `wine_get_version`) ours; on Windows the C
/// runtime's. Neither lookup compares bytes with this program's `memcmp`.
#[cfg(all(windows, target_arch = "x86_64"))]
#[cold]
fn choose() -> usize {
    // SAFETY: plain module and export lookups with NUL-terminated names.
    let runtime = unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        if !ntdll.is_null() && GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()) != 0 {
            0
        } else {
            let mut module = GetModuleHandleA(c"vcruntime140.dll".as_ptr().cast());
            if module.is_null() {
                module = LoadLibraryA(c"vcruntime140.dll".as_ptr().cast());
            }
            if module.is_null() { 0 } else { GetProcAddress(module, c"memcmp".as_ptr().cast()) }
        }
    };
    let chosen = if runtime > 1 { runtime } else { 1 };
    CHOSEN.store(chosen, core::sync::atomic::Ordering::Relaxed);
    chosen
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const core::ffi::c_void, b: *const core::ffi::c_void, n: usize) -> i32 {
    let chosen = match CHOSEN.load(core::sync::atomic::Ordering::Relaxed) {
        0 => choose(),
        c => c,
    };
    if chosen == 1 {
        // SAFETY: as the caller promised.
        return unsafe { sse2(a, b, n) };
    }
    // SAFETY: the C runtime's `memcmp`, with what the caller passed.
    unsafe { core::mem::transmute::<usize, Compare>(chosen)(a, b, n) }
}

/// Ours: sixteen bytes a step with SSE2.
#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn sse2(a: *const core::ffi::c_void, b: *const core::ffi::c_void, n: usize) -> i32 {
    let (a, b) = (a.cast::<u8>(), b.cast::<u8>());
    use core::arch::x86_64::{__m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8};
    // SAFETY: the caller passes `n` readable bytes at `a` and at `b`, as for
    // the C function; unaligned loads read only inside them.
    unsafe {
        let mut i = 0usize;
        // The first differing byte of a 16-byte step whose mask is not full.
        let first = |at: usize, mask: i32| -> i32 {
            let k = at + (!(mask as u32) & 0xFFFF).trailing_zeros() as usize;
            i32::from(*a.add(k)) - i32::from(*b.add(k))
        };
        while i + 64 <= n {
            let eq = |k: usize| {
                _mm_cmpeq_epi8(
                    _mm_loadu_si128(a.add(i + k).cast::<__m128i>()),
                    _mm_loadu_si128(b.add(i + k).cast::<__m128i>()),
                )
            };
            let (e0, e1, e2, e3) = (eq(0), eq(16), eq(32), eq(48));
            let all = _mm_and_si128(_mm_and_si128(e0, e1), _mm_and_si128(e2, e3));
            if _mm_movemask_epi8(all) != 0xFFFF {
                for (k, e) in [(0, e0), (16, e1), (32, e2), (48, e3)] {
                    let mask = _mm_movemask_epi8(e);
                    if mask != 0xFFFF {
                        return first(i + k, mask);
                    }
                }
            }
            i += 64;
        }
        while i + 16 <= n {
            let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(
                _mm_loadu_si128(a.add(i).cast::<__m128i>()),
                _mm_loadu_si128(b.add(i).cast::<__m128i>()),
            ));
            if mask != 0xFFFF {
                return first(i, mask);
            }
            i += 16;
        }
        // The last bytes one at a time, read so that the compiler does not
        // make the loop a call of `memcmp` (this one).
        while i < n {
            let (x, y) = (core::ptr::read_volatile(a.add(i)), core::ptr::read_volatile(b.add(i)));
            if x != y {
                return i32::from(x) - i32::from(y);
            }
            i += 1;
        }
        0
    }
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    /// The sign the C function gives, by the first differing byte (unsigned).
    fn reference(a: &[u8], b: &[u8]) -> i32 {
        for (x, y) in a.iter().zip(b) {
            if x != y {
                return i32::from(*x) - i32::from(*y);
            }
        }
        0
    }

    #[test]
    fn agrees_with_a_byte_at_a_time_at_every_length_and_place() {
        let base: Vec<u8> = (0..300u32).map(|i| (i * 37 % 251) as u8).collect();
        for len in 0..200 {
            for offset in 0..3 {
                let a = &base[offset..offset + len];
                // SAFETY: both slices hold `len` bytes.
                assert_eq!(unsafe { super::sse2(a.as_ptr().cast(), a.as_ptr().cast(), len) }, 0);
                for at in 0..len {
                    for delta in [1u8, 0x80, 0xFF] {
                        let mut b = a.to_vec();
                        b[at] = b[at].wrapping_add(delta);
                        // SAFETY: as above.
                        let got = unsafe { super::sse2(a.as_ptr().cast(), b.as_ptr().cast(), len) };
                        assert_eq!(got.signum(), reference(a, &b).signum(), "len {len} at {at}");
                        // SAFETY: as above.
                        let chosen = unsafe { super::memcmp(a.as_ptr().cast(), b.as_ptr().cast(), len) };
                        assert_eq!(chosen.signum(), reference(a, &b).signum(), "chosen, len {len} at {at}");
                    }
                }
            }
        }
    }
}
