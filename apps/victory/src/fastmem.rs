//! `memcmp` of our own, for the whole program.
//!
//! The runtime compares memory all the time (title memory against the copies
//! the card has, what shaders wrote against what was there), and every such
//! comparison is a call of the C runtime's `memcmp`. Under Wine (Proton) that
//! is Wine's `ucrtbase.dll`, whose `memcmp` compares eight bytes a turn of a
//! scalar loop: walking, it was the hottest code of the submitting thread and
//! of every worker beside it (2026-11-26, perf). Defined here, the linker
//! takes this one before the imported one, for every crate of the program.
//!
//! Same contract as C's, and the same values as Wine's: 1 or -1 by the first
//! differing byte taken as unsigned, of `a` against `b`; 0 when the `n` bytes
//! are equal.
//!
//! Measured under Proton (diag build): 4 KiB 0.03 against 0.12 us, 64 KiB
//! 0.51 against 1.60, 64 MiB 1.41 against 1.85 ms. Wine's `memcpy` (SSE2,
//! 64 bytes a turn) is as fast as an AVX2 copy where it counts (1.0-1.1x at
//! 64 KiB and more): the program keeps it.

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{__m256i, _mm256_and_si256, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8};

/// # Safety
/// `a` and `b` are readable for `n` bytes.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const core::ffi::c_void, b: *const core::ffi::c_void, n: usize) -> i32 {
    let (a, b) = (a.cast::<u8>(), b.cast::<u8>());
    // SAFETY: forwarded from the caller; AVX2 only where the CPU has it.
    unsafe {
        match chosen() {
            Choice::Ours if avx2() => compare_avx2(a, b, n),
            Choice::Ours | Choice::Deciding => compare_words(a, b, n),
            Choice::Wine(wine) => wine(a.cast(), b.cast(), n),
        }
    }
}

type Memcmp = unsafe extern "C" fn(*const core::ffi::c_void, *const core::ffi::c_void, usize) -> i32;

enum Choice {
    Ours,
    Wine(Memcmp),
    /// While the choice is being made (what makes it compares strings too).
    Deciding,
}

/// Whose `memcmp` runs: ours with `VICTORY_FAST_MEMCMP=1`, or in the phases
/// of a `VICTORY_AB` run that has it on; otherwise the C runtime's
/// (`ucrtbase.dll`), as without this file. Ours ended two runs in the
/// title's code (2026-11-26, before Wine's placeholder fix); it stays off
/// unless asked for until runs show it safe.
fn chosen() -> Choice {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    // 0 not decided, 1 deciding, 2 ours, 3 Wine's, 4 + bit: as the A/B bit.
    static MODE: AtomicU64 = AtomicU64::new(0);
    static WINE: AtomicUsize = AtomicUsize::new(0);
    let wine = || {
        let at = WINE.load(Ordering::Acquire);
        // SAFETY: the C runtime's `memcmp`, found by its name.
        if at == 0 { Choice::Ours } else { Choice::Wine(unsafe { std::mem::transmute::<usize, Memcmp>(at) }) }
    };
    match MODE.load(Ordering::Acquire) {
        2 => return Choice::Ours,
        3 => return wine(),
        1 => return Choice::Deciding,
        0 => {}
        m => {
            #[cfg(feature = "diagnostics")]
            if victory_hle::ab_on((m - 4) as u32) {
                return Choice::Ours;
            }
            let _ = m;
            return wine();
        }
    }
    if MODE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Choice::Deciding;
    }
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetModuleHandleW(name: *const u16) -> *mut core::ffi::c_void;
            fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> usize;
        }
        let name: Vec<u16> = "ucrtbase.dll".encode_utf16().chain([0]).collect();
        // SAFETY: plain lookups of a loaded system library's export.
        let at = unsafe {
            let module = GetModuleHandleW(name.as_ptr());
            if module.is_null() { 0 } else { GetProcAddress(module, c"memcmp".as_ptr().cast()) }
        };
        WINE.store(at, Ordering::Release);
    }
    let asked = std::env::var_os("VICTORY_FAST_MEMCMP").is_some_and(|v| v == "1");
    #[cfg(feature = "diagnostics")]
    let bit = victory_hle::ab_bit("VICTORY_FAST_MEMCMP");
    #[cfg(not(feature = "diagnostics"))]
    let bit: Option<u32> = None;
    let mode = match bit {
        Some(bit) => 4 + u64::from(bit),
        None if asked || WINE.load(Ordering::Acquire) == 0 => 2,
        None => 3,
    };
    MODE.store(mode, Ordering::Release);
    chosen()
}

/// Whether the CPU has AVX2 (asked once).
#[cfg(target_arch = "x86_64")]
fn avx2() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    static HAS: AtomicU8 = AtomicU8::new(0);
    match HAS.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let has = std::arch::is_x86_feature_detected!("avx2");
            HAS.store(if has { 1 } else { 2 }, Ordering::Relaxed);
            has
        }
    }
}

/// The ordering of the bytes at `i`, which differ.
///
/// # Safety
/// Both readable at `i`.
#[inline(always)]
unsafe fn at(a: *const u8, b: *const u8, i: usize) -> i32 {
    // -1 or 1, as Wine's and Microsoft's runtimes give (C asks only for the
    // sign): what we hand back is what callers had before.
    // SAFETY: as the caller says.
    unsafe { if *a.add(i) > *b.add(i) { 1 } else { -1 } }
}

/// From `i` on, eight bytes a turn, then bytes. Written so that it is no
/// call of `memcmp` itself.
///
/// # Safety
/// `a` and `b` are readable for `n` bytes.
#[inline(never)]
unsafe fn compare_words(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY (the whole function): reads below `n`.
    unsafe { tail(a, b, 0, n) }
}

/// # Safety
/// `a` and `b` are readable for `n` bytes, `i <= n`.
#[inline(always)]
unsafe fn tail(a: *const u8, b: *const u8, mut i: usize, n: usize) -> i32 {
    // SAFETY (the whole function): reads below `n`.
    unsafe {
        while i + 8 <= n {
            let (x, y) = (a.add(i).cast::<u64>().read_unaligned(), b.add(i).cast::<u64>().read_unaligned());
            if x != y {
                // Little endian: the first differing byte is the lowest.
                return at(a, b, i + ((x ^ y).trailing_zeros() / 8) as usize);
            }
            i += 8;
        }
        while i < n {
            if *a.add(i) != *b.add(i) {
                return at(a, b, i);
            }
            i += 1;
        }
        0
    }
}

/// # Safety
/// `a` and `b` are readable for `n` bytes; the CPU has AVX2.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn compare_avx2(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY (the whole function): every load lies below `n`.
    unsafe {
        let load = |p: *const u8, i: usize| _mm256_loadu_si256(p.add(i).cast::<__m256i>());
        let equal = |i: usize| _mm256_cmpeq_epi8(load(a, i), load(b, i));
        let first = |mask: u32, i: usize| at(a, b, i + (!mask).trailing_zeros() as usize);
        let mut i = 0;
        // 128 bytes a turn: four compares joined, looked into only where one
        // differs.
        while i + 128 <= n {
            let (e0, e1, e2, e3) = (equal(i), equal(i + 32), equal(i + 64), equal(i + 96));
            let all = _mm256_and_si256(_mm256_and_si256(e0, e1), _mm256_and_si256(e2, e3));
            if _mm256_movemask_epi8(all) as u32 != u32::MAX {
                for (k, e) in [e0, e1, e2, e3].into_iter().enumerate() {
                    let mask = _mm256_movemask_epi8(e) as u32;
                    if mask != u32::MAX {
                        return first(mask, i + 32 * k);
                    }
                }
            }
            i += 128;
        }
        while i + 32 <= n {
            let mask = _mm256_movemask_epi8(equal(i)) as u32;
            if mask != u32::MAX {
                return first(mask, i);
            }
            i += 32;
        }
        tail(a, b, i, n)
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;

    /// Ours, whichever runs as `memcmp` here.
    unsafe fn ours(a: *const core::ffi::c_void, b: *const core::ffi::c_void, n: usize) -> i32 {
        // SAFETY: forwarded.
        unsafe { if avx2() { compare_avx2(a.cast(), b.cast(), n) } else { compare_words(a.cast(), b.cast(), n) } }
    }

    /// Without `VICTORY_FAST_MEMCMP=1` (or an A/B run naming it) the C
    /// runtime's own runs, and gives what ours gives.
    #[cfg(windows)]
    #[test]
    fn the_runtimes_own_runs_unless_asked() {
        if std::env::var_os("VICTORY_FAST_MEMCMP").is_some() || std::env::var_os("VICTORY_AB").is_some() {
            return;
        }
        assert!(matches!(chosen(), Choice::Wine(_)));
        let a = bytes(4096, 5);
        let mut b = a.clone();
        b[1000] ^= 0x80;
        // SAFETY: both hold the bytes.
        unsafe {
            assert_eq!(memcmp(a.as_ptr().cast(), b.as_ptr().cast(), 4096).signum(), ours(a.as_ptr().cast(), b.as_ptr().cast(), 4096).signum());
        }
    }

    /// The reference: byte by byte.
    fn reference(a: &[u8], b: &[u8]) -> i32 {
        for (x, y) in a.iter().zip(b) {
            if x != y {
                return if x > y { 1 } else { -1 };
            }
        }
        0
    }

    fn bytes(n: usize, seed: u64) -> Vec<u8> {
        let mut s = seed | 1;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s as u8
            })
            .collect()
    }

    /// Every length to 520 at every offset to 33, equal and with one byte
    /// different (lower and higher) at every place: both ways agree with the
    /// reference in sign and value.
    #[test]
    fn agrees_with_the_reference_everywhere() {
        let base = bytes(600, 7);
        for offset in 0..33 {
            for n in 0..520usize.min(600 - offset) {
                let a = &base[offset..offset + n];
                let mut b = a.to_vec();
                // SAFETY: both hold `n` bytes.
                unsafe {
                    assert_eq!(compare_words(a.as_ptr(), b.as_ptr(), n), 0);
                    if avx2() {
                        assert_eq!(compare_avx2(a.as_ptr(), b.as_ptr(), n), 0);
                    }
                }
                for place in (0..n).step_by(7).chain(n.checked_sub(1)) {
                    for change in [1u8, 255] {
                        b[place] = a[place].wrapping_add(change);
                        let want = reference(a, &b);
                        // SAFETY: as above.
                        unsafe {
                            assert_eq!(compare_words(a.as_ptr(), b.as_ptr(), n), want, "words n={n} at {place}");
                            if avx2() {
                                assert_eq!(compare_avx2(a.as_ptr(), b.as_ptr(), n), want, "avx2 n={n} at {place}");
                            }
                        }
                        b[place] = a[place];
                    }
                }
            }
        }
    }

    /// Slices compared by Rust go through `memcmp` (this one, in the test
    /// program too): their ordering is unchanged.
    #[cfg(windows)]
    #[test]
    fn slice_ordering_goes_through_it() {
        let a = bytes(4096, 3);
        let mut b = a.clone();
        assert!(a == b);
        b[4000] = b[4000].wrapping_add(1);
        assert_eq!(a.cmp(&b), reference(&a, &b).cmp(&0));
        assert!(a != b);
    }

    /// Against the C runtime's own `memcmp` (Wine's `ucrtbase.dll` under
    /// Proton), equal bytes of three sizes. Printed, not asserted.
    #[cfg(windows)]
    #[test]
    fn faster_than_the_runtimes_own() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryA(name: *const u8) -> *mut core::ffi::c_void;
            fn GetProcAddress(lib: *mut core::ffi::c_void, name: *const u8) -> usize;
        }
        type Cmp = unsafe extern "C" fn(*const u8, *const u8, usize) -> i32;
        // SAFETY: plain lookups of a system library's export.
        let theirs: Option<Cmp> = unsafe {
            let lib = LoadLibraryA(c"ucrtbase.dll".as_ptr().cast());
            let f = if lib.is_null() { 0 } else { GetProcAddress(lib, c"memcmp".as_ptr().cast()) };
            (f != 0).then(|| std::mem::transmute::<usize, Cmp>(f))
        };
        // Sizes the runtime compares: a 4 KiB part, a 64 KiB block (in the
        // cache), 64 MiB (from memory); the same bytes over and over.
        for (size, turns) in [(4usize << 10, 200_000u32), (64 << 10, 20_000), (64 << 20, 10)] {
            let a = bytes(size, 11);
            let b = a.clone();
            let time = |f: &dyn Fn() -> i32| {
                let start = std::time::Instant::now();
                for _ in 0..turns {
                    assert_eq!(f(), 0);
                }
                start.elapsed().as_secs_f64() * 1e6 / f64::from(turns)
            };
            // SAFETY: both hold the bytes.
            let mine = time(&|| unsafe { ours(a.as_ptr().cast(), b.as_ptr().cast(), a.len()) });
            match theirs {
                // SAFETY: as above.
                Some(theirs) => {
                    let wine = time(&|| unsafe { theirs(a.as_ptr(), b.as_ptr(), a.len()) });
                    eprintln!("memcmp of {} KiB: ours {mine:.2} us, ucrtbase's {wine:.2} us ({:.1}x)", size >> 10, wine / mine);
                }
                None => eprintln!("memcmp of {} KiB: ours {mine:.2} us (no ucrtbase)", size >> 10),
            }
        }
    }



    /// The very values Wine's (or Microsoft's) runtime gives, for equal and
    /// differing bytes of many lengths and places.
    #[cfg(windows)]
    #[test]
    fn same_values_as_the_runtimes_own() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryA(name: *const u8) -> *mut core::ffi::c_void;
            fn GetProcAddress(lib: *mut core::ffi::c_void, name: *const u8) -> usize;
        }
        type Cmp = unsafe extern "C" fn(*const u8, *const u8, usize) -> i32;
        // SAFETY: plain lookups of a system library's export.
        let theirs: Cmp = unsafe {
            let lib = LoadLibraryA(c"ucrtbase.dll".as_ptr().cast());
            assert!(!lib.is_null());
            let f = GetProcAddress(lib, c"memcmp".as_ptr().cast());
            assert_ne!(f, 0);
            std::mem::transmute::<usize, Cmp>(f)
        };
        let a = bytes(5000, 21);
        let mut checked = 0;
        for n in (0..5000).step_by(37) {
            let mut b = a[..n].to_vec();
            for place in (0..n).step_by(53).chain(n.checked_sub(1)) {
                for change in [1u8, 128, 255] {
                    b[place] = a[place].wrapping_add(change);
                    // SAFETY: both hold `n` bytes.
                    let (ours, wine) = unsafe { (ours(a.as_ptr().cast(), b.as_ptr().cast(), n), theirs(a.as_ptr(), b.as_ptr(), n)) };
                    assert_eq!(ours, wine, "n={n} at {place}");
                    b[place] = a[place];
                    checked += 1;
                }
            }
            // SAFETY: as above.
            unsafe { assert_eq!(ours(a.as_ptr().cast(), b.as_ptr().cast(), n), theirs(a.as_ptr(), b.as_ptr(), n)) };
        }
        assert!(checked > 1000);
    }
}

/// `memcpy` and `memmove` of our own, `rep movsb` for copies of
/// [`copy::BIG`] bytes and more (`VICTORY_FAST_MEMCPY=1`; off by default:
/// Wine's, as without this).
///
/// Why: Wine's `ucrtbase.dll` copies with SSE2 stores (`sse2_memmove`): 18.5 %
/// of the submitting thread at Series X 1440p (run c-o-nl-x-perf,
/// 2026-11-27), the hottest code of that thread. An AVX2 loop was measured no
/// faster above 64 KiB (above), but both make the processor read every
/// destination line before writing it; `rep movsb` on a processor with ERMS
/// and FSRM (Zen 3 and later) writes whole lines without that read.
/// Smaller copies, and a `memmove` whose destination overlaps the source from
/// behind, stay with Wine's.
#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) mod copy {
    use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering::*};

    type Copy = unsafe extern "C" fn(*mut u8, *const u8, usize) -> *mut u8;

    /// Below this, Wine's (a call there costs about what `rep movsb` starts up in).
    pub(crate) const BIG: usize = 512;
    /// 0 not decided, 1 deciding, 2 ours for big copies, 3 Wine's always.
    static MODE: AtomicU8 = AtomicU8::new(0);
    static WINE_COPY: AtomicUsize = AtomicUsize::new(0);
    static WINE_MOVE: AtomicUsize = AtomicUsize::new(0);

    /// # Safety
    /// `dst` writable and `src` readable for `n` bytes; forward is safe for them.
    #[inline(always)]
    unsafe fn forward(dst: *mut u8, src: *const u8, n: usize) {
        // SAFETY: as the caller says; the direction flag is clear by the ABI.
        unsafe {
            core::arch::asm!("rep movsb", inout("rcx") n => _, inout("rdi") dst => _, inout("rsi") src => _,
                options(nostack, preserves_flags));
        }
    }

    /// # Safety
    /// `dst` writable and `src` readable for `n` bytes (`n > 0`).
    #[inline(always)]
    unsafe fn backward(dst: *mut u8, src: *const u8, n: usize) {
        // SAFETY: as the caller says; the flag is set back before returning.
        unsafe {
            core::arch::asm!("std", "rep movsb", "cld", inout("rcx") n => _, inout("rdi") dst.add(n - 1) => _,
                inout("rsi") src.add(n - 1) => _, options(nostack));
        }
    }

    /// Looked up without any allocation or copy of ours.
    fn decide() -> u8 {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetModuleHandleW(name: *const u16) -> *mut core::ffi::c_void;
            fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> usize;
            fn GetEnvironmentVariableW(name: *const u16, buffer: *mut u16, size: u32) -> u32;
        }
        if MODE.compare_exchange(0, 1, AcqRel, Acquire).is_err() {
            return MODE.load(Acquire);
        }
        const UCRT: [u16; 13] = utf16(b"ucrtbase.dll\0");
        const NAME: [u16; 20] = utf16(b"VICTORY_FAST_MEMCPY\0");
        let mut value = [0u16; 4];
        // SAFETY: plain lookups of a loaded system library's exports and of
        // an environment variable into a buffer of its size.
        let (copy, moving, asked) = unsafe {
            let module = GetModuleHandleW(UCRT.as_ptr());
            let (c, m) = if module.is_null() {
                (0, 0)
            } else {
                (GetProcAddress(module, c"memcpy".as_ptr().cast()), GetProcAddress(module, c"memmove".as_ptr().cast()))
            };
            let len = GetEnvironmentVariableW(NAME.as_ptr(), value.as_mut_ptr(), value.len() as u32);
            (c, m, len == 1 && value[0] == u16::from(b'1'))
        };
        WINE_COPY.store(copy, Release);
        WINE_MOVE.store(moving, Release);
        let mode = if asked || copy == 0 || moving == 0 { 2 } else { 3 };
        MODE.store(mode, Release);
        mode
    }

    const fn utf16<const N: usize>(s: &[u8; N]) -> [u16; N] {
        let mut out = [0u16; N];
        let mut i = 0;
        while i < N {
            out[i] = s[i] as u16;
            i += 1;
        }
        out
    }

    #[inline(always)]
    fn mode() -> u8 {
        match MODE.load(Relaxed) {
            0 => decide(),
            m => m,
        }
    }

    /// Fast inline copy for `n < BIG` (forward/non-overlapping): avoids calling
    /// into Wine's `ucrtbase.dll` `sse2_memmove`.
    #[inline(always)]
    unsafe fn small(dst: *mut u8, src: *const u8, n: usize) {
        unsafe {
            if n == 0 {
                return;
            }
            if n <= 4 {
                if n >= 2 {
                    let a = (src as *const u16).read_unaligned();
                    let b = (src.add(n - 2) as *const u16).read_unaligned();
                    (dst as *mut u16).write_unaligned(a);
                    (dst.add(n - 2) as *mut u16).write_unaligned(b);
                } else {
                    *dst = *src;
                }
                return;
            }
            if n <= 8 {
                let a = (src as *const u32).read_unaligned();
                let b = (src.add(n - 4) as *const u32).read_unaligned();
                (dst as *mut u32).write_unaligned(a);
                (dst.add(n - 4) as *mut u32).write_unaligned(b);
                return;
            }
            if n <= 16 {
                let a = (src as *const u64).read_unaligned();
                let b = (src.add(n - 8) as *const u64).read_unaligned();
                (dst as *mut u64).write_unaligned(a);
                (dst.add(n - 8) as *mut u64).write_unaligned(b);
                return;
            }
            if n <= 32 {
                let a = (src as *const u128).read_unaligned();
                let b = (src.add(n - 16) as *const u128).read_unaligned();
                (dst as *mut u128).write_unaligned(a);
                (dst.add(n - 16) as *mut u128).write_unaligned(b);
                return;
            }
            // 33..512 bytes: FSRM on Zen 3/4/5 handles `rep movsb` fast down to 33B.
            forward(dst, src, n);
        }
    }

    /// # Safety
    /// C's `memcpy`: `dst` writable, `src` readable, `n` bytes, not overlapping.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        // SAFETY: forwarded from the caller.
        unsafe {
            match mode() {
                2 if n >= BIG => forward(dst, src, n),
                2 => small(dst, src, n),
                3 if WINE_COPY.load(Relaxed) != 0 => {
                    return core::mem::transmute::<usize, Copy>(WINE_COPY.load(Relaxed))(dst, src, n);
                }
                _ => forward(dst, src, n),
            }
        }
        dst
    }

    /// # Safety
    /// C's `memmove`: `dst` writable, `src` readable, `n` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        // Forward is right unless the destination begins inside the source.
        let ahead = (dst as usize).wrapping_sub(src as usize) >= n;
        // SAFETY: forwarded from the caller.
        unsafe {
            match mode() {
                2 if n >= BIG && ahead => forward(dst, src, n),
                2 if ahead => small(dst, src, n),
                2 | 3 if WINE_MOVE.load(Relaxed) != 0 => {
                    return core::mem::transmute::<usize, Copy>(WINE_MOVE.load(Relaxed))(dst, src, n);
                }
                _ if ahead => forward(dst, src, n),
                _ if n > 0 => backward(dst, src, n),
                _ => {}
            }
        }
        dst
    }
}
