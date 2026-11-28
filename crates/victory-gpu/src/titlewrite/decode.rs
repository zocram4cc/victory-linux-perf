//! The stores of x86-64 code that [`super::store`] does in the title's place:
//! moves from a register or a constant, arithmetic of those with memory
//! (locked or not), exchanges, and the 4, 8, 16 and 32 byte stores of the
//! vector registers. Everything else is `None`.

use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering::SeqCst};
use windows_sys::Win32::System::Diagnostics::Debug::CONTEXT;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Alu {
    Add,
    Or,
    And,
    Sub,
    Xor,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    /// These bytes (the low `size` of them).
    Put([u8; 32]),
    /// With a value; `keeps` carry (`inc`, `dec`).
    Alu { alu: Alu, with: u64, keeps: bool },
    /// With a register, which gets what was there.
    Xchg(Register),
    Xadd(Register),
    Cmpxchg(Register),
}

/// A general register, or (`high`) the second byte of one of the first four.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Register {
    number: usize,
    high: bool,
}

pub(super) struct Store {
    /// Where it stores, and how many bytes.
    pub(super) at: usize,
    pub(super) size: usize,
    /// The instruction's length.
    pub(super) len: usize,
    op: Op,
    locked: bool,
}

fn general(c: &CONTEXT, number: usize) -> u64 {
    match number {
        0 => c.Rax,
        1 => c.Rcx,
        2 => c.Rdx,
        3 => c.Rbx,
        4 => c.Rsp,
        5 => c.Rbp,
        6 => c.Rsi,
        7 => c.Rdi,
        8 => c.R8,
        9 => c.R9,
        10 => c.R10,
        11 => c.R11,
        12 => c.R12,
        13 => c.R13,
        14 => c.R14,
        _ => c.R15,
    }
}

fn general_mut(c: &mut CONTEXT, number: usize) -> &mut u64 {
    match number {
        0 => &mut c.Rax,
        1 => &mut c.Rcx,
        2 => &mut c.Rdx,
        3 => &mut c.Rbx,
        4 => &mut c.Rsp,
        5 => &mut c.Rbp,
        6 => &mut c.Rsi,
        7 => &mut c.Rdi,
        8 => &mut c.R8,
        9 => &mut c.R9,
        10 => &mut c.R10,
        11 => &mut c.R11,
        12 => &mut c.R12,
        13 => &mut c.R13,
        14 => &mut c.R14,
        _ => &mut c.R15,
    }
}

impl Register {
    /// Operand `number` of `size` bytes, with or without a REX prefix.
    fn of(number: usize, size: usize, rex: bool) -> Register {
        if size == 1 && !rex && (4..8).contains(&number) {
            Register { number: number - 4, high: true }
        } else {
            Register { number, high: false }
        }
    }

    fn get(self, c: &CONTEXT) -> u64 {
        general(c, self.number) >> if self.high { 8 } else { 0 }
    }

    /// As an instruction writes it: 32 bits clear the rest, fewer leave it.
    fn put(self, c: &mut CONTEXT, value: u64, size: usize) {
        let register = general_mut(c, self.number);
        let shift = if self.high { 8 } else { 0 };
        *register = match size {
            8 => value,
            4 => value & 0xffff_ffff,
            _ => *register & !(mask(size) << shift) | (value & mask(size)) << shift,
        };
    }
}

fn mask(size: usize) -> u64 {
    if size >= 8 { u64::MAX } else { (1 << (size * 8)) - 1 }
}

/// The upper half of vector register `reg`, where the context has it; nothing
/// in it if the thread used none.
fn upper(c: &CONTEXT, reg: usize) -> Option<[u8; 16]> {
    use windows_sys::Win32::System::Diagnostics::Debug::{GetXStateFeaturesMask, LocateXStateFeature};
    const AVX: u32 = 2;
    let mut mask = 0;
    // SAFETY: a context as the system passes it to a handler (the calls
    // change nothing of it).
    unsafe {
        if GetXStateFeaturesMask(c, &mut mask) == 0 {
            return None;
        }
        if mask >> AVX & 1 == 0 {
            return Some([0; 16]);
        }
        let mut len = 0;
        let at = LocateXStateFeature(c, AVX, &mut len) as *const [u8; 16];
        (!at.is_null() && len as usize >= 16 * 16).then(|| at.add(reg).read_unaligned())
    }
}

/// The store at the start of `code`, which the registers are those of.
pub(super) fn store(code: &[u8], c: &CONTEXT) -> Option<Store> {
    let mut at = 0;
    let mut next = || {
        let byte = code.get(at).copied();
        at += 1;
        byte
    };
    let (mut locked, mut narrow, mut repeat, mut long) = (false, false, 0u8, false);
    let mut byte = next()?;
    loop {
        match byte {
            0xf0 => locked = true,
            0x66 => narrow = true,
            0xf2 | 0xf3 => repeat = byte,
            // (Of data and stack segments nothing is used; branch hints.)
            0x2e | 0x36 | 0x3e | 0x26 => {}
            _ => break,
        }
        byte = next()?;
    }
    let mut rex = 0u8;
    if byte & 0xf0 == 0x40 {
        rex = byte;
        byte = next()?;
    }
    // The vector prefixes, with their own forms of the others.
    let vector = match byte {
        0xc5 if rex == 0 => {
            let b = next()?;
            rex = if b & 0x80 == 0 { 0x44 } else { 0x40 };
            Some(b)
        }
        0xc4 if rex == 0 => {
            let (b, d) = (next()?, next()?);
            if b & 0x1f != 1 {
                return None;
            }
            rex = 0x40 | (!b >> 5 & 7) | (d >> 4 & 8);
            Some(d)
        }
        _ => None,
    };
    if let Some(b) = vector {
        if narrow || repeat != 0 || locked {
            return None;
        }
        long = b & 4 != 0;
        // (A store has no third register.)
        if b >> 3 & 15 != 15 {
            return None;
        }
        (narrow, repeat) = match b & 3 {
            0 => (false, 0),
            1 => (true, 0),
            2 => (false, 0xf3),
            _ => (false, 0xf2),
        };
        byte = next()?;
    }
    let two = vector.is_some() || byte == 0x0f;
    if two && vector.is_none() {
        byte = next()?;
    }
    let wide = rex & 8 != 0;
    let modrm = next()?;
    let (mode, reg, rm) = (modrm >> 6, (modrm >> 3 & 7) as usize | (rex as usize & 4) << 1, modrm & 7);
    if mode == 3 {
        return None;
    }
    // The address, all but what hangs on the instruction's length.
    let mut address = 0u64;
    let mut on_rip = false;
    let mut base = Some(rm as usize | (rex as usize & 1) << 3);
    if rm == 4 {
        let sib = next()?;
        let index = (sib >> 3 & 7) as usize | (rex as usize & 2) << 2;
        if index != 4 {
            address = general(c, index) << (sib >> 6);
        }
        base = Some((sib & 7) as usize | (rex as usize & 1) << 3);
        if sib & 7 == 5 && mode == 0 {
            base = None;
        }
    } else if rm == 5 && mode == 0 {
        base = None;
        on_rip = true;
    }
    if let Some(base) = base {
        address = address.wrapping_add(general(c, base));
    }
    let displacement = match mode {
        1 => i64::from(next()? as i8),
        2 => i64::from(i32::from_le_bytes([next()?, next()?, next()?, next()?])),
        _ if base.is_none() => i64::from(i32::from_le_bytes([next()?, next()?, next()?, next()?])),
        _ => 0,
    };
    address = address.wrapping_add(displacement as u64);

    let words = if wide {
        8
    } else if narrow {
        2
    } else {
        4
    };
    let put = |value: u64| {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&value.to_le_bytes());
        Op::Put(bytes)
    };
    let of = |size: usize| Register::of(reg, size, rex != 0);
    let vector_bytes = || {
        // SAFETY: the registers, whichever way they are named.
        let x = unsafe { &c.Anonymous.FltSave }.XmmRegisters[reg];
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&x.Low.to_le_bytes());
        bytes[8..16].copy_from_slice(&x.High.to_le_bytes());
        bytes
    };
    let alu_of = |kind: u8| match kind {
        0 => Some(Alu::Add),
        1 => Some(Alu::Or),
        4 => Some(Alu::And),
        5 => Some(Alu::Sub),
        6 => Some(Alu::Xor),
        _ => None,
    };
    let (op, size) = if long {
        if !matches!((byte, narrow, repeat), (0x11 | 0x29, _, 0) | (0x7f, true, 0) | (0x7f, false, 0xf3) | (0xe7, true, 0)) {
            return None;
        }
        let mut bytes = vector_bytes();
        bytes[16..].copy_from_slice(&upper(c, reg)?);
        (Op::Put(bytes), 32)
    } else if two {
        match (byte, narrow, repeat) {
            (0x11, _, 0) | (0x29, _, 0) | (0x7f, true, 0) | (0x7f, false, 0xf3) | (0xe7, true, 0) => (Op::Put(vector_bytes()), 16),
            (0x11, false, 0xf3) => (Op::Put(vector_bytes()), 4),
            (0x11, false, 0xf2) | (0x13, _, 0) | (0xd6, true, 0) => (Op::Put(vector_bytes()), 8),
            (0x17, _, 0) => {
                let mut bytes = vector_bytes();
                bytes.copy_within(8..16, 0);
                (Op::Put(bytes), 8)
            }
            (0x7e, true, 0) => (Op::Put(vector_bytes()), if wide { 8 } else { 4 }),
            _ if vector.is_some() || repeat != 0 => return None,
            (0xc3, false, _) => (put(of(words).get(c)), words),
            (0xb0 | 0xb1, _, _) => {
                let size = if byte & 1 == 0 { 1 } else { words };
                (Op::Cmpxchg(of(size)), size)
            }
            (0xc0 | 0xc1, _, _) => {
                let size = if byte & 1 == 0 { 1 } else { words };
                (Op::Xadd(of(size)), size)
            }
            _ => return None,
        }
    } else {
        if repeat != 0 {
            return None;
        }
        let size = if byte & 1 == 0 { 1 } else { words };
        let mut constant = |bytes: usize| -> Option<u64> {
            Some(match bytes {
                1 => next()? as i8 as i64 as u64,
                2 => i64::from(i16::from_le_bytes([next()?, next()?])) as u64,
                _ => i64::from(i32::from_le_bytes([next()?, next()?, next()?, next()?])) as u64,
            })
        };
        match byte {
            0x88 | 0x89 => (put(of(size).get(c)), size),
            0xc6 | 0xc7 if reg & 7 == 0 => (put(constant(size)?), size),
            0x00 | 0x01 | 0x08 | 0x09 | 0x20 | 0x21 | 0x28 | 0x29 | 0x30 | 0x31 => {
                (Op::Alu { alu: alu_of(byte >> 3)?, with: of(size).get(c), keeps: false }, size)
            }
            0x80 | 0x81 | 0x83 => {
                let alu = alu_of(reg as u8 & 7)?;
                (Op::Alu { alu, with: constant(if byte == 0x81 { size } else { 1 })?, keeps: false }, size)
            }
            0x86 | 0x87 => {
                locked = true;
                (Op::Xchg(of(size)), size)
            }
            0xfe | 0xff if reg & 7 < 2 => {
                (Op::Alu { alu: if reg & 7 == 0 { Alu::Add } else { Alu::Sub }, with: 1, keeps: true }, size)
            }
            _ => return None,
        }
    };
    if locked && matches!(op, Op::Put(_)) {
        return None;
    }
    // What is read of memory decides nothing but what is stored: no
    // register but the operand is written.
    if on_rip {
        address = address.wrapping_add(c.Rip).wrapping_add(at as u64);
    }
    Some(Store { at: address as usize, size, len: at, op, locked })
}

const CF: u32 = 1;
const PF: u32 = 4;
const AF: u32 = 0x10;
const ZF: u32 = 0x40;
const SF: u32 = 0x80;
const OF: u32 = 0x800;

/// The result of an operation on `size` bytes and the flags it leaves.
fn arithmetic(alu: Alu, a: u64, b: u64, size: usize, keeps: bool, flags: u32) -> (u64, u32) {
    let m = mask(size);
    let (a, b) = (a & m, b & m);
    let top = |v: u64| v >> (size * 8 - 1) & 1 != 0;
    let r = match alu {
        Alu::Add => a.wrapping_add(b),
        Alu::Or => a | b,
        Alu::And => a & b,
        Alu::Sub => a.wrapping_sub(b),
        Alu::Xor => a ^ b,
    } & m;
    let mut out = flags & !(CF | PF | AF | ZF | SF | OF);
    let mut flag = |set: bool, flag: u32| out |= if set { flag } else { 0 };
    flag(r == 0, ZF);
    flag(top(r), SF);
    flag((r as u8).count_ones().is_multiple_of(2), PF);
    match alu {
        Alu::Add => {
            flag(r < a, CF);
            flag(top((a ^ r) & (b ^ r)), OF);
            flag((a ^ b ^ r) & 16 != 0, AF);
        }
        Alu::Sub => {
            flag(a < b, CF);
            flag(top((a ^ b) & (a ^ r)), OF);
            flag((a ^ b ^ r) & 16 != 0, AF);
        }
        _ => {}
    }
    if keeps {
        out = out & !CF | flags & CF;
    }
    (r, out)
}

/// A value of `size` bytes in memory, changed by `f` as one if `locked`: what
/// was there (`None`: not aligned).
///
/// # Safety
/// `size` bytes can be read and written at `at`.
unsafe fn change(at: usize, size: usize, locked: bool, f: impl Fn(u64) -> u64) -> Option<u64> {
    if !locked {
        let mut bytes = [0u8; 8];
        // SAFETY (both): by the caller.
        unsafe { std::ptr::copy_nonoverlapping(at as *const u8, bytes.as_mut_ptr(), size) };
        let was = u64::from_le_bytes(bytes);
        unsafe { std::ptr::copy_nonoverlapping(f(was).to_le_bytes().as_ptr(), at as *mut u8, size) };
        return Some(was);
    }
    if !at.is_multiple_of(size) {
        return None;
    }
    // SAFETY (all): by the caller, and aligned.
    Some(unsafe {
        match size {
            1 => u64::from(AtomicU8::from_ptr(at as *mut _).fetch_update(SeqCst, SeqCst, |v| Some(f(v.into()) as u8)).ok()?),
            2 => u64::from(AtomicU16::from_ptr(at as *mut _).fetch_update(SeqCst, SeqCst, |v| Some(f(v.into()) as u16)).ok()?),
            4 => u64::from(AtomicU32::from_ptr(at as *mut _).fetch_update(SeqCst, SeqCst, |v| Some(f(v.into()) as u32)).ok()?),
            _ => AtomicU64::from_ptr(at as *mut _).fetch_update(SeqCst, SeqCst, |v| Some(f(v))).ok()?,
        }
    })
}

impl Store {
    /// Whether it leaves the memory as it is, and what it leaves in the flags
    /// then; `None` for those of which that is not asked (locked ones,
    /// exchanges).
    ///
    /// # Safety
    /// [`Store::size`] bytes can be read at `at`.
    pub(super) unsafe fn same(&self, c: &CONTEXT) -> Option<(bool, u32)> {
        if self.locked {
            return None;
        }
        let mut there = [0u8; 32];
        // SAFETY: by the caller (32 bytes at the most).
        unsafe { std::ptr::copy_nonoverlapping(self.at as *const u8, there.as_mut_ptr(), self.size) };
        match self.op {
            Op::Put(bytes) => Some((there[..self.size] == bytes[..self.size], c.EFlags)),
            Op::Alu { alu, with, keeps } => {
                let was = u64::from_le_bytes(there[..8].try_into().expect("eight bytes")) & mask(self.size);
                let (value, flags) = arithmetic(alu, was, with, self.size, keeps, c.EFlags);
                Some((value == was, flags))
            }
            _ => None,
        }
    }

    /// Whether the loader may make it look before it writes
    /// (`victory_loader::write_sites`, which looks itself): an `or` of a byte
    /// with a constant, a move of 4 bytes or more; of the five bytes a jump
    /// needs.
    pub(super) fn can_look(&self, code: &[u8]) -> bool {
        let at = usize::from(code[0] & 0xf0 == 0x40);
        self.len >= 5
            && match self.op {
                Op::Alu { alu: Alu::Or, .. } => code[at] == 0x80,
                Op::Put(_) => matches!(code[0], 0xc4 | 0xc5) && self.size >= 16 || matches!(code[at], 0x89 | 0xc7),
                _ => false,
            }
    }

    /// Does it, with `to` for the address, and gives the registers what the
    /// instruction leaves in them (but the instruction pointer). `false`:
    /// nothing was done.
    ///
    /// # Safety
    /// [`Store::size`] bytes can be read and written at `to`.
    pub(super) unsafe fn run(&self, c: &mut CONTEXT, to: usize) -> bool {
        let (size, locked, m) = (self.size, self.locked, mask(self.size));
        // SAFETY (all `change`s): by the caller.
        match self.op {
            Op::Put(bytes) => {
                // As one, for those the processor stores as one.
                // SAFETY (all): by the caller, aligned where it must be.
                unsafe {
                    let low = u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]);
                    match size {
                        1 => (to as *mut u8).write_volatile(bytes[0]),
                        2 if to.is_multiple_of(2) => (to as *mut u16).write_volatile(low as u16),
                        4 if to.is_multiple_of(4) => (to as *mut u32).write_volatile(low as u32),
                        8 if to.is_multiple_of(8) => (to as *mut u64).write_volatile(low),
                        _ => std::ptr::copy_nonoverlapping(bytes.as_ptr(), to as *mut u8, size),
                    }
                }
            }
            Op::Alu { alu, with, keeps } => {
                let flags = c.EFlags;
                let Some(was) = (unsafe { change(to, size, locked, |v| arithmetic(alu, v, with, size, keeps, flags).0) }) else {
                    return false;
                };
                c.EFlags = arithmetic(alu, was, with, size, keeps, flags).1;
            }
            Op::Xchg(register) => {
                let value = register.get(c) & m;
                let Some(was) = (unsafe { change(to, size, true, |_| value) }) else {
                    return false;
                };
                register.put(c, was & m, size);
            }
            Op::Xadd(register) => {
                let value = register.get(c);
                let Some(was) = (unsafe { change(to, size, locked, |v| v.wrapping_add(value) & m) }) else {
                    return false;
                };
                c.EFlags = arithmetic(Alu::Add, was, value, size, false, c.EFlags).1;
                register.put(c, was & m, size);
            }
            Op::Cmpxchg(register) => {
                let (value, expected) = (register.get(c) & m, c.Rax & m);
                let Some(was) = (unsafe { change(to, size, locked, |v| if v & m == expected { value } else { v }) }) else {
                    return false;
                };
                let was = was & m;
                c.EFlags = arithmetic(Alu::Sub, expected, was, size, false, c.EFlags).1;
                if was != expected {
                    Register { number: 0, high: false }.put(c, was, size);
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> CONTEXT {
        // SAFETY: plain data.
        unsafe { std::mem::zeroed() }
    }

    /// The instruction's address, size and length, with every register its
    /// number times 0x10000.
    fn decoded(code: &[u8]) -> Option<(usize, usize, usize)> {
        let mut c = context();
        for n in 0..16 {
            *general_mut(&mut c, n) = n as u64 * 0x10000;
        }
        c.Rip = 0x7000_0000;
        let mut padded = code.to_vec();
        padded.resize(15, 0xcc);
        store(&padded, &c).map(|s| (s.at, s.size, s.len))
    }

    #[test]
    fn stores_are_told_by_their_bytes() {
        // mov [rcx+rax+48h], rbx
        assert_eq!(decoded(&[0x48, 0x89, 0x5c, 0x01, 0x48]), Some((0x10048, 8, 5)));
        // mov [rbx], rdx
        assert_eq!(decoded(&[0x48, 0x89, 0x13]), Some((0x30000, 8, 3)));
        // or byte [r9+9eh], 1
        assert_eq!(decoded(&[0x41, 0x80, 0x89, 0x9e, 0, 0, 0, 1]), Some((0x9009e, 1, 8)));
        // lock dec dword [rcx+10h]
        assert_eq!(decoded(&[0xf0, 0xff, 0x49, 0x10]), Some((0x10010, 4, 4)));
        // lock xadd [r10+r12*8-1], eax
        assert_eq!(decoded(&[0xf0, 0x43, 0x0f, 0xc1, 0x44, 0xe2, 0xff]), Some((0xa0000 + 0xc0000 * 8 - 1, 4, 7)));
        // mov word [rsp], 1234h
        assert_eq!(decoded(&[0x66, 0xc7, 0x04, 0x24, 0x34, 0x12]), Some((0x40000, 2, 6)));
        // mov [rip+10h], al; mov byte [rip+10h], 5 (counted from its end)
        assert_eq!(decoded(&[0x88, 0x05, 0x10, 0, 0, 0]), Some((0x7000_0016, 1, 6)));
        assert_eq!(decoded(&[0xc6, 0x05, 0x10, 0, 0, 0, 5]), Some((0x7000_0017, 1, 7)));
        // mov [12345678h], eax (no base)
        assert_eq!(decoded(&[0x89, 0x04, 0x25, 0x78, 0x56, 0x34, 0x12]), Some((0x1234_5678, 4, 7)));
        // vmovaps [rsi+10h], xmm4; vmovaps [r12], xmm3; movups [rax], xmm9
        assert_eq!(decoded(&[0xc5, 0xf8, 0x29, 0x66, 0x10]), Some((0x60010, 16, 5)));
        assert_eq!(decoded(&[0xc4, 0xc1, 0x78, 0x29, 0x1c, 0x24]), Some((0xc0000, 16, 6)));
        assert_eq!(decoded(&[0x44, 0x0f, 0x11, 0x08]), Some((0, 16, 4)));
        // vmovss [rax], xmm0; movlps, movhps, vmovq, movd
        assert_eq!(decoded(&[0xc5, 0xfa, 0x11, 0x00]), Some((0, 4, 4)));
        assert_eq!(decoded(&[0x0f, 0x13, 0x00]), Some((0, 8, 3)));
        assert_eq!(decoded(&[0xc5, 0xf9, 0xd6, 0x00]), Some((0, 8, 4)));
        assert_eq!(decoded(&[0x66, 0x0f, 0x7e, 0x00]), Some((0, 4, 4)));
        // Not stores of these: a load, a register's, a string instruction, a
        // thread's own memory, a compare, adc.
        assert_eq!(decoded(&[0x48, 0x8b, 0x13]), None);
        assert_eq!(decoded(&[0x48, 0x89, 0xd3]), None);
        assert_eq!(decoded(&[0xf3, 0xa4]), None);
        assert_eq!(decoded(&[0x65, 0x48, 0x89, 0x13]), None);
        assert_eq!(decoded(&[0x83, 0x38, 1]), None);
        assert_eq!(decoded(&[0x11, 0x00]), None);
        // blend by a mask (vmaskmov is of another map), vmovss with a third
        // register.
        assert_eq!(decoded(&[0xc4, 0xe2, 0x79, 0x2e, 0x00]), None);
    }

    /// What the processor makes of `code` with rax as the address, rcx and
    /// rdx as operands; against [`Store::run`].
    fn against_the_processor(code: &[u8], seed: u64) {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
        static PLACE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: fresh memory for code.
        let place = *PLACE.get_or_init(|| unsafe {
            VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize
        });
        let spread = |n: u64| {
            let v = (seed + n).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ seed << 7;
            // Small, edge and large ones.
            match v >> 61 {
                0 => 0,
                1 => v & 1,
                2 => u64::MAX,
                3 => 0x8000_0000_0000_0000u64 | 0x8000 | 0x80 | 0x8000_0000,
                _ => v,
            }
        };
        // f(address, operand, compared, flags, out):
        // mov r11, rcx ; the address (its instruction takes [r11])
        // mov rax, r8 ; mov r10, r9 ; push r10 ; popfq ; <code> ; pushfq ; pop r10
        // mov rcx, [rsp+28h] ; mov [rcx], r10 ; mov [rcx+8], rdx ; mov [rcx+16], rax ; ret
        let mut f = vec![0x49, 0x89, 0xcb, 0x4c, 0x89, 0xc0, 0x41, 0x51, 0x9d];
        f.extend(code);
        f.extend([0x9c, 0x41, 0x5a, 0x48, 0x8b, 0x4c, 0x24, 0x28]);
        f.extend([0x4c, 0x89, 0x11, 0x48, 0x89, 0x51, 0x08, 0x48, 0x89, 0x41, 0x10, 0xc3]);
        type F = unsafe extern "system" fn(*mut u64, u64, u64, u64, *mut [u64; 3]);
        let (there, operand, compared) = (spread(1), spread(2), if seed.is_multiple_of(3) { spread(1) } else { spread(3) });
        let flags = 0x202 | (spread(4) & 0x8d5) as u32;
        let (mut memory, mut out) = (there, [0; 3]);
        // SAFETY: the code above, which keeps to the calling convention.
        unsafe {
            std::ptr::copy_nonoverlapping(f.as_ptr(), place as *mut u8, f.len());
            let f: F = std::mem::transmute(place);
            f(&mut memory, operand, compared, u64::from(flags), &mut out);
        }
        let mut c = context();
        let mut ours = there;
        (c.R11, c.Rdx, c.Rax, c.EFlags) = (&raw mut ours as u64, operand, compared, flags);
        let mut padded = code.to_vec();
        padded.resize(15, 0xcc);
        let store = store(&padded, &c).unwrap_or_else(|| panic!("{code:x?}"));
        assert_eq!((store.at, store.len), (&raw mut ours as usize, code.len()), "{code:x?}");
        // SAFETY: our word.
        assert!(unsafe { store.run(&mut c, &raw mut ours as usize) });
        assert_eq!(ours, memory, "{code:x?} of {there:x} with {operand:x}");
        assert_eq!((c.Rdx, c.Rax), (out[1], out[2]), "{code:x?} of {there:x} with {operand:x}, {compared:x}");
        // (`and`, `or` and `xor` leave AF as they like.)
        const SEEN: u32 = CF | PF | ZF | SF | OF;
        assert_eq!(c.EFlags & SEEN, out[0] as u32 & SEEN, "{code:x?} of {there:x} with {operand:x}");
    }

    #[test]
    fn stores_do_what_the_processor_does() {
        let mut codes: Vec<Vec<u8>> = Vec::new();
        // op [r11], rdx in all sizes; op [r11], constant; inc, dec; xchg, xadd, cmpxchg.
        for op in [0x00, 0x08, 0x20, 0x28, 0x30, 0x88, 0x86] {
            codes.push(vec![0x41, op, 0x13]);
            codes.push(vec![0x66, 0x41, op | 1, 0x13]);
            codes.push(vec![0x41, op | 1, 0x13]);
            codes.push(vec![0x49, op | 1, 0x13]);
            if op < 0x80 {
                codes.push(vec![0xf0, 0x41, op | 1, 0x13]);
                codes.push(vec![0xf0, 0x49, op | 1, 0x13]);
            }
        }
        for kind in [0u8, 1, 4, 5, 6] {
            let modrm = 0x03 | kind << 3;
            for constant in [1, 0x7f, 0x80, 0xff] {
                codes.push(vec![0x41, 0x80, modrm, constant]);
                codes.push(vec![0x41, 0x83, modrm, constant]);
                codes.push(vec![0x49, 0x83, modrm, constant]);
                codes.push(vec![0x66, 0x41, 0x83, modrm, constant]);
                codes.push(vec![0xf0, 0x41, 0x83, modrm, constant]);
                codes.push(vec![0x49, 0x81, modrm, 3, 0, 0, constant]);
                codes.push(vec![0x66, 0x41, 0x81, modrm, 3, constant]);
            }
        }
        for modrm in [0x03, 0x0b] {
            codes.push(vec![0x41, 0xfe, modrm]);
            codes.push(vec![0x41, 0xff, modrm]);
            codes.push(vec![0x49, 0xff, modrm]);
            codes.push(vec![0xf0, 0x41, 0xff, modrm]);
            codes.push(vec![0x66, 0x41, 0xff, modrm]);
        }
        for op in [0xb0, 0xc0] {
            codes.push(vec![0x41, 0x0f, op, 0x13]);
            codes.push(vec![0xf0, 0x41, 0x0f, op, 0x13]);
            codes.push(vec![0xf0, 0x66, 0x41, 0x0f, op | 1, 0x13]);
            codes.push(vec![0xf0, 0x41, 0x0f, op | 1, 0x13]);
            codes.push(vec![0xf0, 0x49, 0x0f, op | 1, 0x13]);
        }
        codes.push(vec![0x41, 0xc6, 0x03, 0x99]);
        codes.push(vec![0x66, 0x41, 0xc7, 0x03, 0x99, 0x88]);
        codes.push(vec![0x41, 0xc7, 0x03, 1, 2, 3, 0x84]);
        codes.push(vec![0x49, 0xc7, 0x03, 1, 2, 3, 0x84]);
        for code in &codes {
            for seed in 0..200 {
                against_the_processor(code, seed);
            }
        }
    }

    /// The second byte of a register, and 32 bits that clear the rest.
    #[test]
    fn registers_are_read_and_written_as_by_the_processor() {
        let mut c = context();
        c.Rax = 0x1122_3344_5566_7788;
        assert_eq!(Register::of(4, 1, false).get(&c) & 0xff, 0x77, "ah");
        assert_eq!(Register::of(4, 1, true), Register { number: 4, high: false }, "spl");
        Register { number: 0, high: true }.put(&mut c, 0xff, 1);
        assert_eq!(c.Rax, 0x1122_3344_5566_ff88);
        Register { number: 0, high: false }.put(&mut c, 0xabcd, 2);
        assert_eq!(c.Rax, 0x1122_3344_5566_abcd);
        Register { number: 0, high: false }.put(&mut c, u64::MAX, 4);
        assert_eq!(c.Rax, 0xffff_ffff);
    }
}
