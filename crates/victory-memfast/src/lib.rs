//! `memcmp` for victory.exe itself, in place of the C runtime's.
//!
//! Under Wine the C runtime is Wine's own `ucrtbase.dll`, whose `memcmp`
//! compares 8 bytes a step. A perf profile in the open world (26 Nov,
//! 18.5 fps, huge pages on) had 22,000 of 500,000 samples in it, most on
//! the threads of `wide` (refreshes compared page by page, snapshots
//! compared after submissions), which the submitting thread waits for: about
//! a third of those threads' time. Here a step is 64 bytes with AVX2 (32
//! with SSE2 where AVX2 is missing), which the data mostly in the
//! processor's cache can feed.
//!
//! Defined as the symbol `memcmp` (and `bcmp`) of the executable on Windows
//! targets: the linker takes it before the runtime's import, for every
//! caller in victory.exe (our code and Rust's standard library). The title,
//! Wine and the driver keep their own. `VICTORY_MEMFAST` cannot switch it
//! off at run time (the symbol is the symbol); the previous build is the
//! way back.
//!
//! `#![no_builtins]`: the compiler must not turn the loops here back into
//! calls of `memcmp`.
#![no_builtins]
#![allow(clippy::missing_safety_doc)]

use core::sync::atomic::{AtomicU8, Ordering};

/// 0 not asked yet, 1 SSE2 only, 2 AVX2.
static LEVEL: AtomicU8 = AtomicU8::new(0);

#[inline]
fn level() -> u8 {
    let l = LEVEL.load(Ordering::Relaxed);
    if l != 0 {
        return l;
    }
    let l = if avx2_usable() { 2 } else { 1 };
    LEVEL.store(l, Ordering::Relaxed);
    l
}

/// AVX2 there and its registers saved by the system: asked of the processor
/// itself (`cpuid`, `xgetbv`), not of the standard library, whose detection
/// may copy memory and so call [`memcpy`] here before it knows the answer.
#[cfg(target_arch = "x86_64")]
fn avx2_usable() -> bool {
    use core::arch::x86_64::{__cpuid, __cpuid_count};
    // (cpuid is safe to call on x86-64.)
    let (basic, leaf1) = (__cpuid(0), __cpuid(1));
    let osxsave = leaf1.ecx & (1 << 27) != 0;
    let avx = leaf1.ecx & (1 << 28) != 0;
    if basic.eax < 7 || !osxsave || !avx {
        return false;
    }
    // Leaf 7 exists: basic.eax >= 7.
    let leaf7 = __cpuid_count(7, 0);
    if leaf7.ebx & (1 << 5) == 0 {
        return false;
    }
    // XCR0: the system saves XMM (bit 1) and YMM (bit 2) state.
    let (lo, _hi): (u32, u32);
    // SAFETY: xgetbv with ecx = 0 is allowed where OSXSAVE is set.
    unsafe { core::arch::asm!("xgetbv", in("ecx") 0, out("eax") lo, out("edx") _hi, options(nomem, nostack, preserves_flags)) };
    lo & 0b110 == 0b110
}

/// Which form [`compare`] uses: "avx2" or "sse2".
pub fn form() -> &'static str {
    if level() == 2 { "avx2" } else { "sse2" }
}

/// The first `n` bytes at `a` and `b` compared as unsigned bytes, as C's
/// `memcmp`: negative, zero or positive.
///
/// # Safety
/// `a` and `b` are readable for `n` bytes.
#[inline]
pub unsafe fn compare(a: *const u8, b: *const u8, n: usize) -> i32 {
    unsafe {
        if n < 16 {
            return small(a, b, n);
        }
        if level() == 2 { avx2(a, b, n) } else { sse2(a, b, n) }
    }
}

/// The byte where `a` and `b` first differ in `[at, at + 16)` or `32`,
/// given the mask of equal bytes.
#[inline(always)]
unsafe fn diff_at(a: *const u8, b: *const u8, at: usize, equal: u32) -> i32 {
    let k = at + (!equal).trailing_zeros() as usize;
    unsafe { i32::from(*a.add(k)) - i32::from(*b.add(k)) }
}

#[inline(always)]
unsafe fn small(a: *const u8, b: *const u8, n: usize) -> i32 {
    unsafe {
        let mut i = 0;
        // 8 bytes at a time, then single bytes.
        while i + 8 <= n {
            let x = (a.add(i) as *const u64).read_unaligned();
            let y = (b.add(i) as *const u64).read_unaligned();
            if x != y {
                // The first differing byte is the lowest one (little endian).
                let k = i + ((x ^ y).trailing_zeros() / 8) as usize;
                return i32::from(*a.add(k)) - i32::from(*b.add(k));
            }
            i += 8;
        }
        while i < n {
            let (x, y) = (*a.add(i), *b.add(i));
            if x != y {
                return i32::from(x) - i32::from(y);
            }
            i += 1;
        }
        0
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn sse2(a: *const u8, b: *const u8, n: usize) -> i32 {
    use core::arch::x86_64::*;
    unsafe {
        let eq16 = |at: usize| -> u32 {
            let x = _mm_loadu_si128(a.add(at) as *const __m128i);
            let y = _mm_loadu_si128(b.add(at) as *const __m128i);
            _mm_movemask_epi8(_mm_cmpeq_epi8(x, y)) as u32
        };
        let mut i = 0;
        while i + 64 <= n {
            let (m0, m1, m2, m3) = (eq16(i), eq16(i + 16), eq16(i + 32), eq16(i + 48));
            if m0 & m1 & m2 & m3 != 0xFFFF {
                for (k, m) in [m0, m1, m2, m3].into_iter().enumerate() {
                    if m != 0xFFFF {
                        return diff_at(a, b, i + 16 * k, m | !0xFFFF);
                    }
                }
            }
            i += 64;
        }
        while i + 16 <= n {
            let m = eq16(i);
            if m != 0xFFFF {
                return diff_at(a, b, i, m | !0xFFFF);
            }
            i += 16;
        }
        if i < n {
            // The last 16 bytes, overlapping what was compared (n >= 16).
            let at = n - 16;
            let m = eq16(at);
            if m != 0xFFFF {
                return diff_at(a, b, at, m | !0xFFFF);
            }
        }
        0
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2(a: *const u8, b: *const u8, n: usize) -> i32 {
    use core::arch::x86_64::*;
    unsafe {
        if n < 32 {
            return sse2(a, b, n);
        }
        let eq32 = |at: usize| -> u32 {
            let x = _mm256_loadu_si256(a.add(at) as *const __m256i);
            let y = _mm256_loadu_si256(b.add(at) as *const __m256i);
            _mm256_movemask_epi8(_mm256_cmpeq_epi8(x, y)) as u32
        };
        let mut i = 0;
        while i + 64 <= n {
            let (m0, m1) = (eq32(i), eq32(i + 32));
            if m0 & m1 != u32::MAX {
                if m0 != u32::MAX {
                    return diff_at(a, b, i, m0);
                }
                return diff_at(a, b, i + 32, m1);
            }
            i += 64;
        }
        if i + 32 <= n {
            let m = eq32(i);
            if m != u32::MAX {
                return diff_at(a, b, i, m);
            }
            i += 32;
        }
        if i < n {
            let at = n - 32;
            let m = eq32(at);
            if m != u32::MAX {
                return diff_at(a, b, at, m);
            }
        }
        0
    }
}

/// Copies `n` bytes from `src` to `dst`; the ranges may overlap (C's
/// `memmove`, and so also `memcpy`).
///
/// Not the executable's `memcpy`/`memmove` (measured on a 5800X3D: for the
/// 64 KiB to 1 MiB pieces of `wide` as fast as Wine's 16-byte steps and
/// glibc, the memory's bandwidth is the limit; and `rep movsb` was 20 times
/// slower where source and destination lie 4 KiB apart). Kept, tested, for
/// a caller that wants it. Here: up to 64 bytes
/// with loads that overlap at the ends (every load before any store, so an
/// overlap is safe), above that 128 bytes a step with AVX2 (64 with SSE2),
/// forward unless `dst` lies inside the source after its start, and the
/// last (or first) 32 bytes loaded first and stored at the end.
///
/// # Safety
/// `src` readable and `dst` writable for `n` bytes.
#[inline]
pub unsafe fn copy(dst: *mut u8, src: *const u8, n: usize) {
    unsafe {
        if n <= 16 {
            return copy_small(dst, src, n);
        }
        if level() == 2 { copy_avx2(dst, src, n) } else { copy_sse2(dst, src, n) }
    }
}

#[inline(always)]
unsafe fn copy_small(dst: *mut u8, src: *const u8, n: usize) {
    unsafe {
        if n >= 8 {
            let (a, b) = ((src as *const u64).read_unaligned(), (src.add(n - 8) as *const u64).read_unaligned());
            (dst as *mut u64).write_unaligned(a);
            (dst.add(n - 8) as *mut u64).write_unaligned(b);
        } else if n >= 4 {
            let (a, b) = ((src as *const u32).read_unaligned(), (src.add(n - 4) as *const u32).read_unaligned());
            (dst as *mut u32).write_unaligned(a);
            (dst.add(n - 4) as *mut u32).write_unaligned(b);
        } else if n >= 2 {
            let (a, b) = ((src as *const u16).read_unaligned(), (src.add(n - 2) as *const u16).read_unaligned());
            (dst as *mut u16).write_unaligned(a);
            (dst.add(n - 2) as *mut u16).write_unaligned(b);
        } else if n == 1 {
            *dst = *src;
        }
    }
}

/// Whether copying from the front never overwrites source bytes not read
/// yet: `dst` before `src`, or past the source's end.
#[inline(always)]
fn forward_safe(dst: *mut u8, src: *const u8, n: usize) -> bool {
    (dst as usize).wrapping_sub(src as usize) >= n
}

#[cfg(target_arch = "x86_64")]
unsafe fn copy_sse2(dst: *mut u8, src: *const u8, n: usize) {
    use core::arch::x86_64::*;
    unsafe {
        let ld = |p: *const u8| _mm_loadu_si128(p as *const __m128i);
        let st = |p: *mut u8, v: __m128i| _mm_storeu_si128(p as *mut __m128i, v);
        if n <= 32 {
            let (a, b) = (ld(src), ld(src.add(n - 16)));
            st(dst, a);
            st(dst.add(n - 16), b);
            return;
        }
        if n <= 64 {
            let (a, b, c, d) = (ld(src), ld(src.add(16)), ld(src.add(n - 32)), ld(src.add(n - 16)));
            st(dst, a);
            st(dst.add(16), b);
            st(dst.add(n - 32), c);
            st(dst.add(n - 16), d);
            return;
        }
        if forward_safe(dst, src, n) {
            let tail = ld(src.add(n - 16));
            let mut i = 0;
            while i + 64 <= n {
                let (a, b, c, d) = (ld(src.add(i)), ld(src.add(i + 16)), ld(src.add(i + 32)), ld(src.add(i + 48)));
                st(dst.add(i), a);
                st(dst.add(i + 16), b);
                st(dst.add(i + 32), c);
                st(dst.add(i + 48), d);
                i += 64;
            }
            while i + 16 <= n {
                let a = ld(src.add(i));
                st(dst.add(i), a);
                i += 16;
            }
            st(dst.add(n - 16), tail);
        } else {
            let head = ld(src);
            let mut i = n;
            while i >= 64 {
                i -= 64;
                let (a, b, c, d) = (ld(src.add(i)), ld(src.add(i + 16)), ld(src.add(i + 32)), ld(src.add(i + 48)));
                st(dst.add(i), a);
                st(dst.add(i + 16), b);
                st(dst.add(i + 32), c);
                st(dst.add(i + 48), d);
            }
            while i >= 16 {
                i -= 16;
                let a = ld(src.add(i));
                st(dst.add(i), a);
            }
            st(dst, head);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn copy_avx2(dst: *mut u8, src: *const u8, n: usize) {
    use core::arch::x86_64::*;
    unsafe {
        if n <= 32 {
            return copy_sse2(dst, src, n);
        }
        let ld = |p: *const u8| _mm256_loadu_si256(p as *const __m256i);
        let st = |p: *mut u8, v: __m256i| _mm256_storeu_si256(p as *mut __m256i, v);
        if n <= 64 {
            let (a, b) = (ld(src), ld(src.add(n - 32)));
            st(dst, a);
            st(dst.add(n - 32), b);
            return;
        }
        if forward_safe(dst, src, n) {
            let tail = ld(src.add(n - 32));
            let mut i = 0;
            while i + 128 <= n {
                let (a, b, c, d) = (ld(src.add(i)), ld(src.add(i + 32)), ld(src.add(i + 64)), ld(src.add(i + 96)));
                st(dst.add(i), a);
                st(dst.add(i + 32), b);
                st(dst.add(i + 64), c);
                st(dst.add(i + 96), d);
                i += 128;
            }
            while i + 32 <= n {
                let a = ld(src.add(i));
                st(dst.add(i), a);
                i += 32;
            }
            st(dst.add(n - 32), tail);
        } else {
            let head = ld(src);
            let mut i = n;
            while i >= 128 {
                i -= 128;
                let (a, b, c, d) = (ld(src.add(i)), ld(src.add(i + 32)), ld(src.add(i + 64)), ld(src.add(i + 96)));
                st(dst.add(i), a);
                st(dst.add(i + 32), b);
                st(dst.add(i + 64), c);
                st(dst.add(i + 96), d);
            }
            while i >= 32 {
                i -= 32;
                let a = ld(src.add(i));
                st(dst.add(i), a);
            }
            st(dst, head);
        }
    }
}

/// The C runtime's `memcmp`, for victory.exe (see the crate's comment).
#[allow(suspicious_runtime_symbol_definitions)] // the point of the crate; same signature as C's
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    unsafe { compare(a, b, n) }
}

/// `bcmp`: zero when equal (LLVM may call it for equality tests).
#[allow(suspicious_runtime_symbol_definitions)]
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    unsafe { compare(a, b, n) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(a: &[u8], b: &[u8]) -> i32 {
        for (x, y) in a.iter().zip(b) {
            if x != y {
                return i32::from(*x) - i32::from(*y);
            }
        }
        0
    }

    fn sign(x: i32) -> i32 {
        x.signum()
    }

    #[test]
    fn every_length_and_every_differing_place_agrees_with_a_plain_loop() {
        // A small deterministic generator (no dependencies).
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let buf: Vec<u8> = (0..4200).map(|_| next() as u8).collect();
        for &forced in &[0u8, 1, 2] {
            LEVEL.store(forced, Ordering::Relaxed);
            if forced == 2 && !std::arch::is_x86_feature_detected!("avx2") {
                continue;
            }
            for off in [0usize, 1, 7, 31] {
                for n in (0..300).chain([511, 512, 513, 1000, 4096, 4100]) {
                    if off + n > buf.len() {
                        continue;
                    }
                    let a = &buf[off..off + n];
                    let mut b = a.to_vec();
                    assert_eq!(unsafe { compare(a.as_ptr(), b.as_ptr(), n) }, 0, "equal n={n}");
                    // One differing byte at every place (sampled for big n).
                    let step = if n > 600 { 37 } else { 1 };
                    for k in (0..n).step_by(step) {
                        let old = b[k];
                        for delta in [1u8, 0x80, 0xFF] {
                            b[k] = old.wrapping_add(delta);
                            let got = unsafe { compare(a.as_ptr(), b.as_ptr(), n) };
                            assert_eq!(sign(got), sign(reference(a, &b)), "n={n} k={k} level={forced}");
                            assert_eq!(got, reference(a, &b), "value n={n} k={k}");
                        }
                        b[k] = old;
                    }
                }
            }
        }
        LEVEL.store(0, Ordering::Relaxed);
    }

    #[test]
    fn copies_and_overlapping_moves_agree_with_a_plain_copy() {
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let pattern: Vec<u8> = (0..9000).map(|_| next() as u8).collect();
        let sizes: Vec<usize> = (0..=200).chain([255, 256, 257, 511, 1000, 4095, 4096, 4097, 8000]).collect();
        for &forced in &[1u8, 2] {
            if forced == 2 && !avx2_usable() {
                continue;
            }
            LEVEL.store(forced, Ordering::Relaxed);
            for &n in &sizes {
                // Apart, at several alignments of both ends.
                for (so, d_off) in [(0usize, 0usize), (1, 3), (7, 0), (13, 31), (32, 5)] {
                    let mut dst = vec![0xAAu8; n + 64];
                    unsafe { copy(dst.as_mut_ptr().add(d_off), pattern.as_ptr().add(so), n) };
                    assert_eq!(&dst[d_off..d_off + n], &pattern[so..so + n], "apart n={n} level={forced}");
                    assert!(dst[..d_off].iter().all(|&b| b == 0xAA), "before dst n={n}");
                    assert!(dst[d_off + n..].iter().all(|&b| b == 0xAA), "after dst n={n}");
                }
                // Overlapping, the destination shifted either way.
                for shift in [1isize, 2, 3, 7, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, -1, -2, -7, -16, -17, -31, -32, -33, -64, -65, -128, -129] {
                    let base = 200usize;
                    let mut buf = pattern[..n + 400].to_vec();
                    let mut want = buf.clone();
                    let (src, dst) = (base, (base as isize + shift) as usize);
                    want.copy_within(src..src + n, dst);
                    unsafe { copy(buf.as_mut_ptr().add(dst), buf.as_ptr().add(src), n) };
                    assert_eq!(buf, want, "overlap n={n} shift={shift} level={forced}");
                }
            }
        }
        LEVEL.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `cargo test --release -p victory-memfast -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn speed_against_the_platform_copy() {
        for &n in &[256usize, 2048, 4096, 16384, 65536, 262144, 1 << 20, 4 << 20, 16 << 20] {
            let src = vec![7u8; n];
            let mut dst = vec![0u8; n];
            let rounds = (512usize << 20) / n;
            let t = std::time::Instant::now();
            for _ in 0..rounds {
                unsafe { copy(dst.as_mut_ptr(), src.as_ptr(), n) };
                std::hint::black_box(&mut dst);
            }
            let ours = t.elapsed();
            let t = std::time::Instant::now();
            for _ in 0..rounds {
                dst.copy_from_slice(&src);
                std::hint::black_box(&mut dst);
            }
            let glibc = t.elapsed();
            LEVEL.store(1, Ordering::Relaxed);
            let t = std::time::Instant::now();
            for _ in 0..rounds {
                unsafe { copy(dst.as_mut_ptr(), src.as_ptr(), n) };
                std::hint::black_box(&mut dst);
            }
            let sse = t.elapsed();
            LEVEL.store(0, Ordering::Relaxed);
            let t = std::time::Instant::now();
            let mut z = 0;
            for _ in 0..rounds {
                z += unsafe { compare(dst.as_ptr(), src.as_ptr(), n) };
            }
            let cmp = t.elapsed();
            std::hint::black_box(z);
            let gbs = |d: std::time::Duration| (rounds * n) as f64 / d.as_secs_f64() / 1e9;
            println!("{n:>9} B: copy ours {:6.1} GB/s, 16-byte steps {:6.1}, glibc {:6.1}; compare ours {:6.1} GB/s", gbs(ours), gbs(sse), gbs(glibc), gbs(cmp));
        }
    }
}
