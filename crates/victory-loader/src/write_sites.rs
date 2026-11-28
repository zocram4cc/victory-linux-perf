//! Places in the title's code that write what is nearly always there already, changed at load to
//! look first (`VICTORY_WRITE_SITES=<file>`; off without it): flags that are set, registers and
//! constants moved to memory, 16 and 32 bytes of the vector registers.
//!
//! The title's threads raise flags in their objects at every use ("drawn this frame"): `or byte
//! [object+n], bit`. The objects lie in memory that copies for the card are kept of, and each such
//! write was a fault on a guarded page, a copy of its block and a guard again, for a byte that
//! stayed as it was (a fifth of a frame's write faults at one place, run faults-3). Here the
//! instruction becomes a jump to code of ours that reads the byte, leaves the flags the instruction
//! leaves, and runs the instruction only if the byte would change.
//!
//! The places are not ours to know: `victory-gpu` (`titlewrite`) finds them while the title runs,
//! by their faults, and writes them into that file, a line each: the image file's name, the place's
//! offset in it (hexadecimal). Nothing of the title's code is kept. What is not such an instruction
//! is left alone.

use std::path::Path;

/// An `or byte [m], constant` at the start of `code`: its length, the bytes that name its address
/// (prefix or 0, ModRM without its register field, what follows up to the constant), the constant.
fn or_byte(code: &[u8]) -> Option<(usize, u8, u8, &[u8], u8)> {
    let mut at = 0;
    let mut rex = 0;
    if code.first()? & 0xf0 == 0x40 {
        rex = code[0];
        at = 1;
    }
    let (&op, &modrm) = (code.get(at)?, code.get(at + 1)?);
    if op != 0x80 || modrm >> 3 & 7 != 1 {
        return None;
    }
    let (mode, rm) = (modrm >> 6, modrm & 7);
    // Not a register, nor the instruction pointer's neighbourhood.
    if mode == 3 || mode == 0 && rm == 5 {
        return None;
    }
    let rest = at + 2;
    let mut end = rest;
    if rm == 4 {
        let sib = *code.get(end)?;
        // Not the stack, whose pointer is another in our code; a plain address is left too.
        if sib & 7 == 4 && rex & 1 == 0 || sib & 7 == 5 && mode == 0 {
            return None;
        }
        end += 1;
    }
    end += match mode {
        1 => 1,
        2 => 4,
        _ => 0,
    };
    let constant = *code.get(end)?;
    Some((end + 1, rex, modrm, &code[rest..end], constant))
}

/// How many bytes behind a ModRM byte name an address, `None` for a register, for the instruction
/// pointer's neighbourhood, the stack (whose pointer is another in our code) and a plain address.
/// `b`: the prefix's bit for the base.
fn address(code: &[u8], modrm: u8, b: bool) -> Option<usize> {
    let (mode, rm) = (modrm >> 6, modrm & 7);
    if mode == 3 || mode == 0 && rm == 5 {
        return None;
    }
    let mut len = 0;
    if rm == 4 {
        let sib = *code.first()?;
        if sib & 7 == 4 && !b || sib & 7 == 5 && mode == 0 {
            return None;
        }
        len = 1;
    }
    len += match mode {
        1 => 1,
        2 => 4,
        _ => 0,
    };
    Some(len)
}

/// The end of a stub that has nothing in rcx if the write is left out, and the title's rcx on the
/// stack: the write (`code[..len]`, at `at`) or not, and back. `wide`: all of rcx counts.
fn write_or_not(out: &mut Vec<u8>, stub: usize, code: &[u8], at: usize, len: usize, wide: bool) -> Option<()> {
    if !wide {
        out.push(0x67);
    }
    out.extend([0xe3, u8::try_from(1 + len + 5).ok()?]); // jrcxz (jecxz) past the write
    out.push(0x59); // pop rcx
    out.extend(&code[..len]);
    out.extend(crate::console_insn::jump(stub + out.len(), at + len)?);
    out.push(0x59);
    out.extend(crate::console_insn::jump(stub + out.len(), at + len)?);
    Some(())
}

/// [`stub`] for a vector register's store. No flag is touched.
fn vector_stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    let (reg_high, xb, third, op_at) = match *code.first()? {
        0xc5 => {
            let b = *code.get(1)?;
            (b & 0x80 == 0, 0x60, b, 2)
        }
        0xc4 => {
            let b = *code.get(1)?;
            if b & 0x1f != 1 {
                return None;
            }
            (b & 0x80 == 0, b & 0x60, *code.get(2)?, 3)
        }
        _ => return None,
    };
    let (op, modrm) = (*code.get(op_at)?, *code.get(op_at + 1)?);
    if !matches!((op, third & 3), (0x11 | 0x29, 0 | 1) | (0x7f, 1 | 2)) || third >> 3 & 15 != 15 {
        return None;
    }
    let long = third & 4;
    if !std::is_x86_feature_detected!("avx2") {
        return None;
    }
    let rest = address(code.get(op_at + 2..)?, modrm, xb & 0x20 == 0)?;
    let len = op_at + 2 + rest;
    if len < 5 {
        return None;
    }
    let reg = modrm >> 3 & 7 | if reg_high { 8 } else { 0 };
    let spare = if reg == 1 { 2u8 } else { 1 };
    let mut out = vec![0x51]; // push rcx
    out.extend([0x48, 0x8d, 0x64, 0x24, 0xe0]); // lea rsp, [rsp-32]
    out.extend([0xc5, 0xfe, 0x7f, spare << 3 | 4, 0x24]); // vmovdqu [rsp], spare (32 bytes)
    // vpcmpeqb spare, reg, [m]
    out.extend([0xc4, 0x80 | xb | 1, (!reg & 15) << 3 | long | 1, 0x74, modrm & 0xc7 | spare << 3]);
    out.extend(&code[op_at + 2..len]);
    out.extend([0xc5, 0xf9 | long, 0xd7, 0xc8 | spare]); // vpmovmskb ecx, spare
    out.extend([0xc5, 0xfe, 0x6f, spare << 3 | 4, 0x24]); // vmovdqu spare, [rsp]
    out.extend([0x48, 0x8d, 0x64, 0x24, 0x20]); // lea rsp, [rsp+32]
    out.extend([0xf7, 0xd1]); // not ecx
    if long == 0 {
        out.extend([0x0f, 0xb7, 0xc9]); // movzx ecx, cx
    }
    write_or_not(&mut out, stub, code, at, len, false)?;
    Some((len, out))
}

/// [`stub`] for a `mov` of a register or a constant, 4 or 8 bytes. No flag is touched.
fn move_stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    let mut at_op = 0;
    let mut rex = 0x40;
    if code.first()? & 0xf0 == 0x40 {
        rex = code[0];
        at_op = 1;
    }
    let (op, modrm) = (*code.get(at_op)?, *code.get(at_op + 1)?);
    let wide = rex & 8 != 0;
    let reg = modrm >> 3 & 7 | (rex & 4) << 1;
    let rest = address(code.get(at_op + 2..)?, modrm, rex & 1 != 0)?;
    let end = at_op + 2 + rest;
    let (len, constant) = match op {
        0x89 if reg != 1 && reg != 4 => (end, None),
        0xc7 if reg & 7 == 0 => {
            let bytes = code.get(end..end + 4)?.try_into().ok()?;
            (end + 4, Some(i32::from_le_bytes(bytes).checked_add(1)?))
        }
        _ => return None,
    };
    if len < 5 {
        return None;
    }
    let mut out = vec![0x51]; // push rcx
    out.extend([rex & 0x4b, 0x8b, modrm & 0xc7 | 1 << 3]); // mov rcx, [m]
    out.extend(&code[at_op + 2..end]);
    let w = 0x40 | rex & 8;
    out.extend([w, 0xf7, 0xd1]); // not rcx
    match constant {
        Some(next) => {
            out.extend([w, 0x8d, 0x89]); // lea rcx, [rcx+constant+1]
            out.extend(next.to_le_bytes());
        }
        // lea rcx, [rcx+reg+1]
        None => out.extend([w | (reg & 8) >> 2, 0x8d, 0x4c, (reg & 7) << 3 | 1, 1]),
    }
    write_or_not(&mut out, stub, code, at, len, wide)?;
    Some((len, out))
}

/// The code at `stub` for the instruction at `at` (`code` from there).
fn stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    vector_stub(code, at, stub).or_else(|| move_stub(code, at, stub)).or_else(|| or_stub(code, at, stub))
}

/// [`stub`] for the `or` of a byte with a constant.
fn or_stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    let (len, rex, modrm, rest, constant) = or_byte(code)?;
    if len < 5 {
        return None;
    }
    let mut out = vec![0x50, 0x51]; // push rax, push rcx
    // movzx eax, byte [m] (the prefix's register field is none of it)
    if rex & 0x43 != 0 {
        out.push(rex & 0x43);
    }
    out.extend([0x0f, 0xb6, modrm & 0xc7]);
    out.extend(rest);
    out.extend([0x89, 0xc1]); // mov ecx, eax
    out.extend([0xf7, 0xd1]); // not ecx
    out.extend([0x81, 0xe1, constant, 0, 0, 0]); // and ecx, constant: the bits to be set
    out.extend([0x0c, constant]); // or al, constant: the flags
    let back = |out: &mut Vec<u8>| {
        let from = stub + out.len();
        out.extend(crate::console_insn::jump(from, at + len)?);
        Some(())
    };
    let mut write = vec![0x59, 0x58]; // pop rcx, pop rax
    write.extend(&code[..len]);
    out.extend([0x67, 0xe3, u8::try_from(write.len() + 5).ok()?]); // jecxz past it
    out.extend(write);
    back(&mut out)?;
    out.extend([0x59, 0x58]);
    back(&mut out)?;
    Some((len, out))
}

/// Changes the places of `list` that are in the image called `name`, mapped at `base` with `size`
/// bytes: how many.
///
/// # Safety
/// The image stays mapped, and no thread runs its code yet.
pub unsafe fn apply(name: &str, base: usize, size: usize, list: &str) -> usize {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut room: Option<(usize, usize)> = None;
    let mut done = 0;
    let mut seen = std::collections::BTreeSet::new();
    for line in list.lines() {
        let mut words = line.split_whitespace();
        let (Some(image), Some(offset)) = (words.next(), words.next()) else {
            continue;
        };
        let Ok(offset) = usize::from_str_radix(offset.trim_start_matches("0x"), 16) else {
            continue;
        };
        if !image.eq_ignore_ascii_case(name) || offset + 16 > size || !seen.insert(offset) {
            continue;
        }
        let at = base + offset;
        if room.is_none() {
            room = crate::console_insn::near(at).map(|place| (place, 0));
        }
        let Some((place, used)) = room.as_mut() else {
            return done;
        };
        // SAFETY: sixteen bytes inside the image; what is not mapped in it is not named.
        let code: [u8; 16] = unsafe { std::ptr::read_unaligned(at as *const _) };
        let to = *place + *used;
        let Some((len, body)) = stub(&code, at, to) else {
            tracing::warn!(image, offset = format_args!("{offset:#x}"), "not a write of the kind that can be looked at first");
            continue;
        };
        let Some(jump) = crate::console_insn::jump(at, to) else {
            continue;
        };
        if *used + body.len() > crate::console_insn::STUBS_LEN {
            return done;
        }
        let mut old = 0;
        // SAFETY: our own code memory, which nothing runs yet; then the instruction, whose page is
        // made writable for it and put back, and which nothing runs either.
        unsafe {
            std::ptr::copy_nonoverlapping(body.as_ptr(), to as *mut u8, body.len());
            if VirtualProtect(at as *const _, len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                continue;
            }
            std::ptr::write_bytes(at as *mut u8, 0x90, len);
            std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, jump.len());
            VirtualProtect(at as *const _, len, old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), at as *const _, len);
        }
        *used += body.len().next_multiple_of(16);
        done += 1;
    }
    done
}

/// [`apply`] with the file `VICTORY_WRITE_SITES` names.
///
/// # Safety
/// As [`apply`].
pub unsafe fn apply_asked(name: &str, base: usize, size: usize) {
    let Some(file) = std::env::var_os("VICTORY_WRITE_SITES") else {
        return;
    };
    let Ok(list) = std::fs::read_to_string(Path::new(&file)) else {
        return;
    };
    // SAFETY: by the caller.
    let places = unsafe { apply(name, base, size, &list) };
    tracing::info!(image = name, places, "writes of the title's that look first (VICTORY_WRITE_SITES)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_can_be_looked_at_first() {
        // or byte [r9+9eh], 1
        let (len, rex, modrm, rest, constant) = or_byte(&[0x41, 0x80, 0x89, 0x9e, 0, 0, 0, 1, 0, 0]).unwrap();
        assert_eq!((len, rex, modrm, rest, constant), (8, 0x41, 0x89, &[0x9e, 0, 0, 0][..], 1));
        // or byte [r9+31h], 80h; [rax+rcx*8+5]; [r12+5] (a base of its own)
        assert_eq!(or_byte(&[0x41, 0x80, 0x49, 0x31, 0x80]).unwrap().0, 5);
        assert_eq!(or_byte(&[0x80, 0x4c, 0xc8, 5, 1]).unwrap().0, 5);
        assert_eq!(or_byte(&[0x41, 0x80, 0x4c, 0x24, 5, 1]).unwrap().0, 6);
        // Of the stack, beside the instruction, a register's, another operation.
        assert!(or_byte(&[0x80, 0x4c, 0x24, 5, 1]).is_none());
        assert!(or_byte(&[0x80, 0x0d, 1, 0, 0, 0, 1]).is_none());
        assert!(or_byte(&[0x80, 0xc9, 1]).is_none());
        assert!(or_byte(&[0x80, 0x61, 5, 1, 0]).is_none());
    }

    /// The instruction in code of a test's, changed, run: the byte, the registers and the flags
    /// are what it makes of them unchanged.
    #[test]
    fn a_changed_write_does_what_it_did() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};
        type F = unsafe extern "system" fn(*mut u8, *mut [u64; 3]);
        for (constant, with) in [(0x80u8, 0x49u8), (1, 0x49), (0x7f, 0x49), (0xff, 0x48), (0x10, 0x49)] {
            // mov rax, rcx ; mov r9, rcx ; mov rcx, 1234h ; xor r10, r10 ; or byte [r9+31h | rax+31h], constant ;
            // pushfq ; pop r10 ; mov [rdx], r10 ; mov [rdx+8], rax ; mov [rdx+16], rcx ; ret
            let mut code = vec![0x48, 0x89, 0xc8, 0x49, 0x89, 0xc9, 0x48, 0xc7, 0xc1, 0x34, 0x12, 0, 0, 0x4d, 0x31, 0xd2];
            let at = code.len();
            if with == 0x49 {
                code.extend([0x41, 0x80, 0x49, 0x31, constant]);
            } else {
                // (With a long address, for the five bytes.)
                code.extend([0x80, 0x88, 0x31, 0, 0, 0, constant]);
            }
            code.extend([0x9c, 0x41, 0x5a, 0x4c, 0x89, 0x12, 0x48, 0x89, 0x42, 0x08, 0x48, 0x89, 0x4a, 0x10, 0xc3]);
            // SAFETY: fresh memory for code, twice, one of them changed before it runs.
            unsafe {
                let place = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
                assert_ne!(place, 0);
                std::ptr::copy_nonoverlapping(code.as_ptr(), place as *mut u8, code.len());
                let second = place + 0x1000;
                std::ptr::copy_nonoverlapping(code.as_ptr(), second as *mut u8, code.len());
                let mut old = 0;
                VirtualProtect(second as *const _, 0x1000, PAGE_EXECUTE_READ, &mut old);
                let list = format!("other.dll {:x}\nT.dll 0x{at:x}\nt.dll {at:x}\nt.dll 5\nt.dll\n", at);
                assert_eq!(apply("t.dll", second, 0x1000, &list), 1);
                assert_eq!(std::ptr::read((second + at) as *const u8), 0xe9);
                let (plain, changed): (F, F) = (std::mem::transmute::<usize, F>(place), std::mem::transmute::<usize, F>(second));
                for was in 0..=255u8 {
                    let mut bytes = [[0u8; 0x40]; 2];
                    bytes[0][0x31] = was;
                    bytes[1][0x31] = was;
                    let mut out = [[0u64; 3]; 2];
                    plain(bytes[0].as_mut_ptr(), &mut out[0]);
                    changed(bytes[1].as_mut_ptr(), &mut out[1]);
                    assert_eq!(bytes[0], bytes[1], "{was:x} | {constant:x}");
                    out[0][1] = out[0][1].wrapping_sub(bytes[0].as_ptr() as u64);
                    out[1][1] = out[1][1].wrapping_sub(bytes[1].as_ptr() as u64);
                    // (AF is left as it likes.)
                    out[0][0] &= 0x8c5;
                    out[1][0] &= 0x8c5;
                    assert_eq!(out[0], out[1], "{was:x} | {constant:x}");
                }
            }
        }
    }

    /// Memory a vector store may ask to lie on its boundary.
    #[repr(C, align(64))]
    #[derive(Clone, Copy)]
    struct Memory([u8; 0x80]);

    /// Moves of registers and constants, and vector stores, in code of a test's: changed, they
    /// store what they did, leave registers and flags, and write nothing that is there already.
    #[test]
    fn changed_moves_and_vector_stores_do_what_they_did() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, PAGE_READONLY, PAGE_READWRITE, VirtualAlloc, VirtualProtect,
        };
        // Each stores at r9+40h (r10 is 0): rdx, r13 or a vector register 0, 1, 9.
        let stores: &[(&[u8], usize)] = &[
            (&[0x49, 0x89, 0x51, 0x40], 0),                         // (four bytes: left alone)
            (&[0x4b, 0x89, 0x54, 0xd1, 0x40], 8),                   // mov [r9+r10*8+40h], rdx
            (&[0x43, 0x89, 0x54, 0xd1, 0x40], 4),                   // mov [r9+r10*8+40h], edx
            (&[0x4d, 0x89, 0xa9, 0x40, 0, 0, 0], 8),                // mov [r9+40h], r13
            (&[0x45, 0x89, 0xa9, 0x40, 0, 0, 0], 4),                // mov [r9+40h], r13d
            (&[0x41, 0xc7, 0x41, 0x40, 0x78, 0x56, 0x34, 0x92], 4), // mov dword [r9+40h], 92345678h
            (&[0x49, 0xc7, 0x41, 0x40, 0x78, 0x56, 0x34, 0x92], 8), // mov qword [r9+40h], the same
            (&[0xc4, 0xc1, 0x7a, 0x7f, 0x41, 0x40], 16),            // vmovdqu [r9+40h], xmm0
            (&[0xc4, 0xc1, 0x7a, 0x7f, 0x49, 0x40], 16),            // vmovdqu [r9+40h], xmm1
            (&[0xc4, 0x41, 0x78, 0x29, 0x49, 0x40], 16),            // vmovaps [r9+40h], xmm9
            (&[0xc4, 0x81, 0x78, 0x11, 0x44, 0xd1, 0x40], 16),      // vmovups [r9+r10*8+40h], xmm0
            (&[0xc4, 0xc1, 0x7c, 0x11, 0x41, 0x40], 32),            // vmovups [r9+40h], ymm0
            (&[0xc4, 0xc1, 0x7e, 0x7f, 0x49, 0x40], 32),            // vmovdqu [r9+40h], ymm1
        ];
        type F = unsafe extern "system" fn(*mut u8, u64, *mut u8);
        for &(store, size) in stores {
            let mut code = vec![0x41, 0x55, 0x49, 0x89, 0xc9, 0x4d, 0x31, 0xd2, 0x49, 0x89, 0xd5]; // push r13 ; r9 = rcx ; r10 = 0 ; r13 = rdx
            code.extend([0x48, 0xc7, 0xc1, 0x34, 0x12, 0, 0]); // mov rcx, 1234h
            code.extend([0x48, 0x83, 0xec, 0x40, 0xc5, 0x7a, 0x7f, 0x0c, 0x24]); // sub rsp, 40h ; xmm9 kept
            code.extend([0xc4, 0xc1, 0x7e, 0x6f, 0x00]); // vmovdqu ymm0, [r8]
            code.extend([0xc4, 0xc1, 0x7e, 0x6f, 0x48, 0x20]); // vmovdqu ymm1, [r8+32]
            code.extend([0xc4, 0x41, 0x7a, 0x6f, 0x48, 0x10]); // vmovdqu xmm9, [r8+16]
            code.extend([0x48, 0x39, 0xc0]); // cmp rax, rax
            let at = code.len();
            code.extend(store);
            code.extend([0x9c, 0x41, 0x5b]); // pushfq ; pop r11
            code.extend([0xc5, 0xfe, 0x7f, 0x4c, 0x24, 0x10]); // vmovdqu [rsp+10h], ymm1
            code.extend([0x48, 0x8b, 0x44, 0x24, 0x18, 0x48, 0x03, 0x44, 0x24, 0x28]); // rax = two of its parts
            code.extend([0xc5, 0x7a, 0x6f, 0x0c, 0x24, 0x48, 0x83, 0xc4, 0x40, 0x41, 0x5d]); // xmm9 back ; rsp ; pop r13
            // Flags, rcx and rax behind the source ; vzeroupper ; ret
            code.extend([0x4d, 0x89, 0x58, 0x40, 0x49, 0x89, 0x48, 0x48, 0x49, 0x89, 0x40, 0x50, 0xc5, 0xf8, 0x77, 0xc3]);
            // SAFETY: fresh memory for code, twice, one of them changed before it runs; data of ours.
            unsafe {
                let place = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
                assert_ne!(place, 0);
                let second = place + 0x2000;
                std::ptr::copy_nonoverlapping(code.as_ptr(), place as *mut u8, code.len());
                std::ptr::copy_nonoverlapping(code.as_ptr(), second as *mut u8, code.len());
                assert_eq!(apply("t.dll", second, 0x2000, &format!("t.dll {at:x}")), usize::from(size != 0), "{store:x?}");
                if size == 0 {
                    continue;
                }
                let (plain, changed): (F, F) = (std::mem::transmute::<usize, F>(place), std::mem::transmute::<usize, F>(second));
                let data = VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) as *mut u8;
                for seed in 0..200u64 {
                    let value = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    let mut source = [[0u8; 0x60]; 2];
                    for (k, byte) in source[0][..0x40].iter_mut().enumerate() {
                        *byte = (value >> (k % 8 * 8)) as u8 ^ k as u8;
                    }
                    source[1] = source[0];
                    let was = Memory(std::array::from_fn(|k| (seed as usize * 7 + k) as u8));
                    let mut memory = [was; 2];
                    plain(memory[0].0.as_mut_ptr(), value, source[0].as_mut_ptr());
                    changed(memory[1].0.as_mut_ptr(), value, source[1].as_mut_ptr());
                    assert!(memory[0].0 == memory[1].0, "{store:x?}");
                    assert!(memory[0].0[..0x40] == was.0[..0x40] && memory[0].0[0x40 + size..] == was.0[0x40 + size..]);
                    assert_eq!(source[0], source[1], "{store:x?}: registers and flags");
                    // One bit different; and nothing different, in read-only memory.
                    let mut off = memory[0];
                    off.0[0x40 + seed as usize % size] ^= 1 << (seed % 8);
                    changed(off.0.as_mut_ptr(), value, source[1].as_mut_ptr());
                    assert!(off.0 == memory[0].0, "{store:x?}");
                    std::ptr::copy_nonoverlapping(memory[0].0.as_ptr(), data, 0x80);
                    let mut old = 0;
                    VirtualProtect(data.cast(), 4096, PAGE_READONLY, &mut old);
                    changed(data, value, source[1].as_mut_ptr());
                    assert_eq!(source[0], source[1], "{store:x?}: registers and flags, nothing written");
                    VirtualProtect(data.cast(), 4096, PAGE_READWRITE, &mut old);
                }
            }
        }
    }

    /// A byte that would not change is not written.
    #[test]
    fn a_byte_that_stays_is_not_written() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, PAGE_READONLY, VirtualAlloc, VirtualProtect};
        // mov r9, rcx ; or byte [r9+31h], 80h ; ret
        let code = [0x49, 0x89, 0xc9, 0x41, 0x80, 0x49, 0x31, 0x80, 0xc3];
        // SAFETY: fresh memory for code and for a byte, which is read-only when the code runs.
        unsafe {
            let place = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
            std::ptr::copy_nonoverlapping(code.as_ptr(), place as *mut u8, code.len());
            assert_eq!(apply("t.dll", place, 0x10000, "t.dll 3"), 1);
            let data = VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
            data.add(0x31).write(0x83);
            let mut old = 0;
            VirtualProtect(data.cast(), 4096, PAGE_READONLY, &mut old);
            let f: unsafe extern "system" fn(*mut u8) = std::mem::transmute(place);
            f(data);
            assert_eq!(data.add(0x31).read(), 0x83);
        }
    }
}
