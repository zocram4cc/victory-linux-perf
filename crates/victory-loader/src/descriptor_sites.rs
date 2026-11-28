//! Renderer port #2 (`VICTORY_PORT_DESCRIPTORS=1`, read in `victory-gpu`'s
//! `port_descriptors`): the title's own code that copies descriptor records
//! into its shader-visible heap as bytes tells the layer which slots it wrote.
//!
//! What the title does (game_scarlett_master.exe): each draw's and each
//! dispatch's descriptor tables are allocated from its frame allocator
//! (image +0x4d9f400; the allocator's memory is the placed shader-visible
//! view heap) and filled by two functions, +0x4d9efd0 (graphics) and
//! +0x4eec8e0 (compute), three loops each, one record at a time:
//! `and r, 7FE0h` (the slot's byte offset in the table), `vmovups ymm0,
//! [record]`, `vmovups [table + r], ymm0`. Found with `dumpbin /disasm` from
//! the write faults of run faults-6 (book records "the instruction at a made
//! the fault"); a search of all of `.text` for the same instruction shape
//! finds these six stores and no other.
//!
//! Each store becomes a jump to code of ours that does the store, then, if
//! the address lies in the heap `victory-gpu` names in [`Marks`], sets the
//! slot's byte in a map of slots and its 4 KiB chunk's byte in a map of
//! chunks, and jumps back. The flags are clobbered: at every site the next
//! instructions are a `movzx` and a `cmp`, which sets them before any use
//! (checked as part of the original bytes). Every register is put back.
//!
//! Each site's original bytes (from the `and` to the `cmp`) are compared
//! before anything is written; a site whose bytes differ is left alone and
//! said so (another build of the title).

/// The cells the stubs read, `#[repr(C)]` in `victory-gpu`: the heap's first
/// byte, its length in bytes (0: none, nothing is marked), the map of slots
/// (a byte each), the map of chunks (a byte for 128 slots).
pub const MARKS_LEN: usize = 32;

/// A store of the title's to hook: the window's offset in the image, the
/// store's offset in the window and its length, and the window's bytes.
pub struct Site {
    pub window: usize,
    pub store: usize,
    pub len: usize,
    pub bytes: &'static [u8],
}

/// The six record stores of the table fillers (game_scarlett_master.exe
/// 1.0.2.1, image offsets).
pub const SITES: [Site; 6] = [
    // +0x4d9efd0, loop 1: [rcx+rsi]
    Site { window: 0x4d9f04f, store: 10, len: 5, bytes: &[0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc5, 0xfc, 0x11, 0x04, 0x31, 0x0f, 0xb6, 0x47, 0x0b, 0x39, 0xc5] },
    // loop 2: [rcx+rsi]
    Site { window: 0x4d9f10d, store: 10, len: 5, bytes: &[0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc5, 0xfc, 0x11, 0x04, 0x31, 0x0f, 0xb6, 0x47, 0x0f, 0x41, 0x39, 0xc5] },
    // loop 3: [r9+rsi]
    Site { window: 0x4d9f19c, store: 12, len: 6, bytes: &[0x41, 0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc4, 0xc1, 0x7c, 0x10, 0x02, 0xc4, 0xc1, 0x7c, 0x11, 0x04, 0x31, 0x44, 0x0f, 0xb6, 0x4f, 0x0d, 0x44, 0x39, 0xc8] },
    // +0x4eec8e0, loop 1: [r9+r8]
    Site { window: 0x4eec95d, store: 11, len: 6, bytes: &[0x41, 0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc4, 0x81, 0x7c, 0x11, 0x04, 0x01, 0x0f, 0xb6, 0x42, 0x0b, 0x39, 0xc5] },
    // loop 2: [rdi+r8]
    Site { window: 0x4eeca08, store: 10, len: 6, bytes: &[0x81, 0xe7, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x06, 0xc4, 0xa1, 0x7c, 0x11, 0x04, 0x07, 0x0f, 0xb6, 0x72, 0x0d, 0x39, 0xf0] },
    // loop 3: [r10+r8]
    Site { window: 0x4eeca82, store: 11, len: 6, bytes: &[0x41, 0x81, 0xe2, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc4, 0x81, 0x7c, 0x11, 0x04, 0x02, 0x0f, 0xb6, 0x42, 0x0f, 0x41, 0x39, 0xc6] },
];

/// `lea rax, [base+index]` for a `vmovups [base+index], ymm0` (VEX, ModRM
/// 04h with a SIB byte, no displacement, the stack pointer not its base):
/// `None` for another form.
fn lea_of(store: &[u8]) -> Option<[u8; 4]> {
    let (x, b, op_at) = match *store.first()? {
        0xc5 => (false, false, 2),
        0xc4 => {
            let p = *store.get(1)?;
            if p & 0x1f != 1 {
                return None;
            }
            (p & 0x40 == 0, p & 0x20 == 0, 3)
        }
        _ => return None,
    };
    let (op, modrm, sib) = (*store.get(op_at)?, *store.get(op_at + 1)?, *store.get(op_at + 2)?);
    if op != 0x11 || modrm != 0x04 || store.len() != op_at + 3 {
        return None;
    }
    // No base (mode 0, base 101), or the stack pointer as base, or rsp as index (none).
    if sib & 7 == 5 || (sib & 7 == 4 && !b) || (sib >> 3 & 7 == 4 && !x) {
        return None;
    }
    Some([0x48 | u8::from(x) << 1 | u8::from(b), 0x8d, 0x04, sib])
}

/// The code at `to` for the store `store` at `at`: the store, the marks, back
/// to `at + store.len()`.
fn stub(store: &[u8], at: usize, to: usize, marks: usize) -> Option<Vec<u8>> {
    let lea = lea_of(store)?;
    let mut out = store.to_vec();
    out.push(0x50); // push rax
    out.extend(lea); // lea rax, [base+index]
    out.extend([0x52, 0x51]); // push rdx ; push rcx
    out.extend([0x48, 0xba]); // mov rdx, marks
    out.extend((marks as u64).to_le_bytes());
    out.extend([0x48, 0x2b, 0x02]); // sub rax, [rdx]
    out.extend([0x48, 0x3b, 0x42, 0x08]); // cmp rax, [rdx+8]
    out.extend([0x73, 0x18]); // jae past the marks
    out.extend([0x48, 0xc1, 0xe8, 0x05]); // shr rax, 5: the slot
    out.extend([0x48, 0x8b, 0x4a, 0x10]); // mov rcx, [rdx+16]
    out.extend([0xc6, 0x04, 0x01, 0x01]); // mov byte [rcx+rax], 1
    out.extend([0x48, 0xc1, 0xe8, 0x07]); // shr rax, 7: the chunk
    out.extend([0x48, 0x8b, 0x4a, 0x18]); // mov rcx, [rdx+24]
    out.extend([0xc6, 0x04, 0x01, 0x01]); // mov byte [rcx+rax], 1
    out.extend([0x59, 0x5a, 0x58]); // pop rcx ; pop rdx ; pop rax
    let back = crate::console_insn::jump(to + out.len(), at + store.len())?;
    out.extend(back);
    Some(out)
}

/// What became of a site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hooked {
    Done,
    /// Its bytes are not the ones expected: left alone.
    Differs,
    /// Outside the image, no room for code near it, or Windows refused.
    Failed(&'static str),
}

/// Hooks `sites` in the image of `size` bytes at `base`; the stubs read the
/// cells at `marks` ([`MARKS_LEN`] bytes). What became of each, said in the
/// log too.
///
/// # Safety
/// The image stays mapped and no thread runs its code yet; `marks` stays
/// valid for as long as the image runs.
pub unsafe fn apply(base: usize, size: usize, marks: usize, sites: &[Site]) -> Vec<Hooked> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut room: Option<(usize, usize)> = None;
    let mut out = Vec::new();
    for site in sites {
        let done = (|| {
            if site.window.checked_add(site.bytes.len()).is_none_or(|end| end > size) || site.store + site.len > site.bytes.len() {
                return Hooked::Failed("outside the image");
            }
            let window = base + site.window;
            // SAFETY: inside the mapped image (checked above).
            let there = unsafe { std::slice::from_raw_parts(window as *const u8, site.bytes.len()) };
            if there != site.bytes {
                return Hooked::Differs;
            }
            let at = window + site.store;
            if room.is_none() {
                room = crate::console_insn::near(at).map(|place| (place, 0));
            }
            let Some((place, used)) = room.as_mut() else {
                return Hooked::Failed("no room for code near the title's");
            };
            let to = *place + *used;
            let store = &site.bytes[site.store..site.store + site.len];
            let (Some(body), Some(jump)) = (stub(store, at, to, marks), crate::console_insn::jump(at, to)) else {
                return Hooked::Failed("not a store of the expected form, or out of reach");
            };
            if *used + body.len() > crate::console_insn::STUBS_LEN {
                return Hooked::Failed("no room for code near the title's");
            }
            let mut old = 0;
            // SAFETY: our own code memory, which nothing runs yet; then the
            // store, whose page is made writable for it and put back, and
            // which nothing runs either.
            unsafe {
                std::ptr::copy_nonoverlapping(body.as_ptr(), to as *mut u8, body.len());
                if VirtualProtect(at as *const _, site.len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                    return Hooked::Failed("Windows would not make it writable");
                }
                std::ptr::write_bytes(at as *mut u8, 0x90, site.len);
                std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, jump.len());
                VirtualProtect(at as *const _, site.len, old, &mut old);
                FlushInstructionCache(GetCurrentProcess(), at as *const _, site.len);
            }
            *used += body.len().next_multiple_of(16);
            Hooked::Done
        })();
        let offset = format_args!("{:#x}", site.window + site.store);
        match done {
            Hooked::Done => tracing::info!(site = %offset, "renderer port 2: the title's descriptor record store at title+site patched to tell the layer its slot (VICTORY_PORT_DESCRIPTORS)"),
            Hooked::Differs => tracing::warn!(site = %offset, "renderer port 2: not patched, the title's bytes there are not the ones expected (another build?)"),
            Hooked::Failed(why) => tracing::warn!(site = %offset, why, "renderer port 2: not patched"),
        }
        out.push(done);
    }
    out
}

/// The title's own `memcpy` (game_scarlett_master.exe 1.0.2.1, image offset).
pub const TITLE_MEMCPY: usize = 0x5f4_0560;

/// The other writers of descriptor records into the heap, which the six
/// stores do not cover (the fallback of runs port2-2 and pop-1 found 150-300
/// view records a frame no store had told of, on 16 of the ~290 pages
/// written): the frame allocator's two copiers, +0x4d9f200 (a table of
/// exactly 128 bytes: four `vmovups` stores of `ymm3..ymm0` to
/// `[rbx+60h..rbx]`, else `memcpy(rbx, r12, r14)`) and +0x4d9f540
/// (`memcpy(rax, r15, rbx)`). Found from PORT-PATCH-2.md's suspects with the
/// image's bytes (POPIN.md).
///
/// A call: the window's offset, the `call rel32`'s offset in it, the bytes
/// (the call's own five included: its target must be [`TITLE_MEMCPY`]).
pub struct CallSite {
    pub window: usize,
    pub call: usize,
    pub bytes: &'static [u8],
}

pub const COPY_CALLS: [CallSite; 2] = [
    // +0x4d9f200, tables of another size: mov rcx, rbx ; mov rdx, r12 ; mov r8, r14 ; call memcpy
    CallSite { window: 0x4d9_f37d, call: 9, bytes: &[0x48, 0x89, 0xd9, 0x4c, 0x89, 0xe2, 0x4d, 0x89, 0xf0, 0xe8, 0xd5, 0x11, 0x1a, 0x01] },
    // +0x4d9f540: mov rcx, rax ; mov rdx, r15 ; mov r8, rbx ; mov r14, rax ; call memcpy
    CallSite {
        window: 0x4d9_f540,
        call: 12,
        bytes: &[0x48, 0x89, 0xc1, 0x4c, 0x89, 0xfa, 0x49, 0x89, 0xd8, 0x49, 0x89, 0xc6, 0xe8, 0x0f, 0x10, 0x1a, 0x01],
    },
];

/// The 128-byte table copy of +0x4d9f200: the window (its four stores, then
/// `mov rcx, r15 ; mov rdx, rsi ; mov r8, rbx ; xor r9d, r9d`, which sets the
/// flags before any use), and where its last two stores (9 bytes) are
/// replaced by a jump. Not the `mov rcx, r15` after them: the `memcpy` path
/// for other sizes jumps back to it (+0x4d9f38b `jmp +0x4d9f272`; run pop-3
/// ran into the jump's last byte there and stopped).
pub const COPY_128: Site = Site {
    window: 0x4d9_f25f,
    store: 10,
    len: 9,
    bytes: &[
        0xc5, 0xfc, 0x11, 0x5b, 0x60, 0xc5, 0xfc, 0x11, 0x53, 0x40, 0xc5, 0xfc, 0x11, 0x4b, 0x20, 0xc5, 0xfc, 0x11, 0x03, 0x4c, 0x89,
        0xf9, 0x48, 0x89, 0xf2, 0x49, 0x89, 0xd8, 0x45, 0x31, 0xc9,
    ],
};

/// The code at `to` for [`COPY_128`] at `at` (its 9 bytes replaced): the last
/// two stores, the four slots of `[rbx..rbx+128)` marked if they lie in the
/// heap, back to the `mov rcx, r15`.
fn copy_128_stub(at: usize, to: usize, marks: usize, len: usize) -> Option<Vec<u8>> {
    let mut out = vec![0xc5, 0xfc, 0x11, 0x4b, 0x20]; // vmovups [rbx+20h], ymm1
    out.extend([0xc5, 0xfc, 0x11, 0x03]); // vmovups [rbx], ymm0
    out.extend([0x50, 0x52, 0x51]); // push rax ; push rdx ; push rcx
    out.extend([0x48, 0x89, 0xd8]); // mov rax, rbx
    out.extend([0x48, 0xba]); // mov rdx, marks
    out.extend((marks as u64).to_le_bytes());
    out.extend([0x48, 0x2b, 0x02]); // sub rax, [rdx]
    out.extend([0x48, 0x3b, 0x42, 0x08]); // cmp rax, [rdx+8]
    let skip_a = out.len();
    out.extend([0x73, 0x00]); // jae past the marks
    out.extend([0x48, 0x8d, 0x88, 0x80, 0x00, 0x00, 0x00]); // lea rcx, [rax+80h]
    out.extend([0x48, 0x3b, 0x4a, 0x08]); // cmp rcx, [rdx+8]
    let skip_b = out.len();
    out.extend([0x77, 0x00]); // ja past the marks
    out.extend([0x48, 0xc1, 0xe8, 0x05]); // shr rax, 5: the first slot
    out.extend([0x48, 0x8b, 0x4a, 0x10]); // mov rcx, [rdx+16]
    out.extend([0xc7, 0x04, 0x01, 0x01, 0x01, 0x01, 0x01]); // mov dword [rcx+rax], 01010101h: four slots
    out.extend([0x48, 0x8b, 0x4a, 0x18]); // mov rcx, [rdx+24]
    out.extend([0x48, 0x89, 0xc2]); // mov rdx, rax
    out.extend([0x48, 0xc1, 0xea, 0x07]); // shr rdx, 7: the first slot's chunk
    out.extend([0xc6, 0x04, 0x11, 0x01]); // mov byte [rcx+rdx], 1
    out.extend([0x48, 0x83, 0xc0, 0x03]); // add rax, 3
    out.extend([0x48, 0xc1, 0xe8, 0x07]); // shr rax, 7: the last slot's chunk
    out.extend([0xc6, 0x04, 0x01, 0x01]); // mov byte [rcx+rax], 1
    let past = out.len();
    out[skip_a + 1] = u8::try_from(past - (skip_a + 2)).ok()?;
    out[skip_b + 1] = u8::try_from(past - (skip_b + 2)).ok()?;
    out.extend([0x59, 0x5a, 0x58]); // pop rcx ; pop rdx ; pop rax
    let back = crate::console_insn::jump(to + out.len(), at + len)?;
    out.extend(back);
    Some(out)
}

/// Copies `bytes` over `len` bytes of the title's code at `at` (the rest
/// `nop`s).
///
/// # Safety
/// `at..at + len` is the title's code, which nothing runs yet.
unsafe fn write_code(at: usize, bytes: &[u8], len: usize) -> bool {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut old = 0;
    // SAFETY: by the caller; the page is made writable and put back.
    unsafe {
        if VirtualProtect(at as *const _, len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        std::ptr::write_bytes(at as *mut u8, 0x90, len);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const _, len, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), at as *const _, len);
    }
    true
}

/// Room for code near `at`, `room` the place found before and what is used of
/// it: `body` is put there; where.
fn place_for(at: usize, body: &[u8], room: &mut Option<(usize, usize)>) -> Option<usize> {
    if room.is_none() {
        *room = crate::console_insn::near(at).map(|place| (place, 0));
    }
    let (place, used) = room.as_mut()?;
    if *used + body.len() > crate::console_insn::STUBS_LEN {
        return None;
    }
    let to = *place + *used;
    // SAFETY: our own code memory, which nothing runs yet.
    unsafe { std::ptr::copy_nonoverlapping(body.as_ptr(), to as *mut u8, body.len()) };
    *used += body.len().next_multiple_of(16);
    Some(to)
}

/// Hooks [`COPY_CALLS`] (each call then goes to `hook`, a function with
/// `memcpy`'s arguments that copies with the title's own and marks the slots
/// it wrote) and [`COPY_128`] (marks its four slots in the cells at `marks`).
/// What became of each, in that order, said in the log too.
///
/// # Safety
/// As [`apply`]; `hook` is a function as said, alive as long as the image runs.
pub unsafe fn apply_copies(base: usize, size: usize, marks: usize, hook: usize) -> Vec<Hooked> {
    let mut room: Option<(usize, usize)> = None;
    let mut out = Vec::new();
    for site in &COPY_CALLS {
        let done = (|| {
            if site.window.checked_add(site.bytes.len()).is_none_or(|end| end > size) {
                return Hooked::Failed("outside the image");
            }
            let window = base + site.window;
            // SAFETY: inside the mapped image (checked above).
            let there = unsafe { std::slice::from_raw_parts(window as *const u8, site.bytes.len()) };
            let at = window + site.call;
            let rel = i32::from_le_bytes(site.bytes[site.call + 1..site.call + 5].try_into().unwrap_or_default());
            if there != site.bytes || (at as i64 + 5 + i64::from(rel)) as usize != base + TITLE_MEMCPY {
                return Hooked::Differs;
            }
            let mut body = vec![0x48, 0xb8]; // mov rax, hook
            body.extend((hook as u64).to_le_bytes());
            body.extend([0xff, 0xe0]); // jmp rax
            let Some(to) = place_for(at, &body, &mut room) else {
                return Hooked::Failed("no room for code near the title's");
            };
            let Some(mut call) = crate::console_insn::jump(at, to) else {
                return Hooked::Failed("out of reach");
            };
            call[0] = 0xe8;
            // SAFETY: the title's call, which nothing runs yet.
            if !unsafe { write_code(at, &call, 5) } {
                return Hooked::Failed("Windows would not make it writable");
            }
            Hooked::Done
        })();
        say(site.window + site.call, done, "the title's memcpy of descriptor tables");
        out.push(done);
    }
    let done = copy_128(base, size, marks, &COPY_128, &mut room);
    say(COPY_128.window + COPY_128.store, done, "the title's 128-byte copy of a descriptor table");
    out.push(done);
    out
}

/// Hooks `site`, the window of [`COPY_128`] somewhere in the image at `base`.
fn copy_128(base: usize, size: usize, marks: usize, site: &Site, room: &mut Option<(usize, usize)>) -> Hooked {
    (|| {
        if site.window.checked_add(site.bytes.len()).is_none_or(|end| end > size) {
            return Hooked::Failed("outside the image");
        }
        let window = base + site.window;
        // SAFETY: inside the mapped image (checked above).
        let there = unsafe { std::slice::from_raw_parts(window as *const u8, site.bytes.len()) };
        if there != site.bytes {
            return Hooked::Differs;
        }
        let at = window + site.store;
        if room.is_none() {
            *room = crate::console_insn::near(at).map(|place| (place, 0));
        }
        let Some((place, used)) = *room else {
            return Hooked::Failed("no room for code near the title's");
        };
        let Some(body) = copy_128_stub(at, place + used, marks, site.len) else {
            return Hooked::Failed("out of reach");
        };
        let Some(to) = place_for(at, &body, room) else {
            return Hooked::Failed("no room for code near the title's");
        };
        let Some(jump) = crate::console_insn::jump(at, to) else {
            return Hooked::Failed("out of reach");
        };
        // SAFETY: the title's store, which nothing runs yet.
        if !unsafe { write_code(at, &jump, site.len) } {
            return Hooked::Failed("Windows would not make it writable");
        }
        Hooked::Done
    })()
}

fn say(offset: usize, done: Hooked, what: &str) {
    let offset = format_args!("{offset:#x}");
    match done {
        Hooked::Done => tracing::info!(site = %offset, what, "renderer port 2: patched to tell the layer the slots it writes (VICTORY_PORT_DESCRIPTORS)"),
        Hooked::Differs => tracing::warn!(site = %offset, what, "renderer port 2: not patched, the title's bytes there are not the ones expected (another build?)"),
        Hooked::Failed(why) => tracing::warn!(site = %offset, what, why, "renderer port 2: not patched"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sites_lea_their_stores_address() {
        let leas: Vec<[u8; 4]> = SITES.iter().map(|s| lea_of(&s.bytes[s.store..s.store + s.len]).unwrap()).collect();
        assert_eq!(leas[0], [0x48, 0x8d, 0x04, 0x31]); // lea rax, [rcx+rsi]
        assert_eq!(leas[1], [0x48, 0x8d, 0x04, 0x31]);
        assert_eq!(leas[2], [0x49, 0x8d, 0x04, 0x31]); // [r9+rsi]
        assert_eq!(leas[3], [0x4b, 0x8d, 0x04, 0x01]); // [r9+r8]
        assert_eq!(leas[4], [0x4a, 0x8d, 0x04, 0x07]); // [rdi+r8]
        assert_eq!(leas[5], [0x4b, 0x8d, 0x04, 0x02]); // [r10+r8]
        // Each window: the store follows the record's load and is followed
        // by a movzx and the cmp that sets the flags.
        for s in &SITES {
            let after = &s.bytes[s.store + s.len..];
            let movzx = if after[0] == 0x0f { 0 } else { 1 };
            assert_eq!(&after[movzx..movzx + 2], &[0x0f, 0xb6]);
            assert_eq!(after[after.len() - 2], 0x39, "cmp");
        }
        // A store with a displacement, of the stack, of another kind: not taken.
        assert!(lea_of(&[0xc5, 0xfc, 0x11, 0x44, 0x31, 0x08]).is_none());
        assert!(lea_of(&[0xc5, 0xfc, 0x11, 0x04, 0x24]).is_none());
        assert!(lea_of(&[0xc5, 0xfc, 0x10, 0x04, 0x31]).is_none());
    }

    /// The marks `victory-gpu` keeps, for a test.
    #[repr(C)]
    struct Marks {
        start: u64,
        len: u64,
        slots: *mut u8,
        chunks: *mut u8,
    }

    /// Each site's window in code of a test's, twice, one copy hooked, both
    /// run: the record is stored as before, the registers the window leaves
    /// are the same, and the slot and its chunk are marked when the table is
    /// in the heap, not otherwise.
    #[test]
    fn a_hooked_site_stores_and_marks_its_slot() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
        // mov to, from (64 bits)
        let mov = |to: u8, from: u8| [0x48 | (from >> 3) << 2 | (to >> 3), 0x89, 0xc0 | (from & 7) << 3 | (to & 7)];
        for site in &SITES {
            let store = &site.bytes[site.store..site.store + site.len];
            let lea = lea_of(store).unwrap();
            let base = lea[3] & 7 | (lea[0] & 1) << 3;
            let index = lea[3] >> 3 & 7 | (lea[0] & 2) << 2;
            let load_at = site.store - if site.bytes[site.store - 5] == 0xc4 { 5 } else { 4 };
            let load = &site.bytes[load_at..site.store];
            let load_reg = if load[0] == 0xc4 { load[4] & 7 | 8 } else { load[3] & 7 };
            // fn(offset: rcx, table: rdx, record: r8, scratch: r9) -> rax:
            // the store's address as the window leaves its registers.
            let mut code: Vec<u8> = vec![0x56, 0x57, 0x53]; // push rsi ; push rdi ; push rbx
            code.extend(mov(11, 1)); // r11 = offset
            code.extend(mov(3, 2)); // rbx = table
            code.extend(mov(6, 8)); // rsi = record
            for scratch in [7, 10, 2] {
                code.extend(mov(scratch, 9)); // rdi, r10, rdx: what the movzx reads
            }
            // The window's `and` masks the offset's register; the table is in
            // the other. (The record first: rsi may be one of them.)
            let rex = usize::from(site.bytes[0] == 0x41);
            let offset = site.bytes[rex + 1] & 7 | if rex == 1 { 8 } else { 0 };
            assert!(offset == base || offset == index);
            let table = if offset == base { index } else { base };
            code.extend(mov(load_reg, 6));
            code.extend(mov(table, 3));
            code.extend(mov(offset, 11));
            let at = code.len();
            code.extend(site.bytes);
            code.extend(lea); // lea rax, [base+index]
            code.extend([0x5b, 0x5f, 0x5e]); // pop rbx ; pop rdi ; pop rsi
            code.extend([0xc5, 0xf8, 0x77, 0xc3]); // vzeroupper ; ret
            type F = unsafe extern "system" fn(u64, *mut u8, *const u8, *mut u8) -> u64;
            // SAFETY: fresh memory for code and data, one copy changed before it runs.
            unsafe {
                let place = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
                assert_ne!(place, 0);
                let (plain, hooked) = (place + 0x100 - at, place + 0x1100 - at);
                std::ptr::copy_nonoverlapping(code.as_ptr(), plain as *mut u8, code.len());
                std::ptr::copy_nonoverlapping(code.as_ptr(), hooked as *mut u8, code.len());
                let heap = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
                let scratch = [0u8; 64];
                let (mut slots, mut chunks) = (vec![0u8; 0x8000 / 32], vec![0u8; 0x8000 / 32 / 128]);
                let marks = Box::new(Marks { start: heap as u64 + 0x2000, len: 0x8000, slots: slots.as_mut_ptr(), chunks: chunks.as_mut_ptr() });
                let mine = [Site { window: 0x1100, ..*site }];
                assert_eq!(apply(place, 0x10000, &raw const *marks as usize, &mine), [Hooked::Done]);
                assert_eq!(*((place + 0x1100 + site.store) as *const u8), 0xe9);
                let (plain, hooked): (F, F) = (std::mem::transmute(plain), std::mem::transmute(hooked));
                let record: [u8; 32] = std::array::from_fn(|k| k as u8 ^ 0x5a);
                // In the heap: slot 0x40 + 3 of its start.
                let table = heap.add(0x2000 + 0x40 * 32);
                let first = plain(0x60 | 0x8000_0000, table.add(0x4000), record.as_ptr(), scratch.as_ptr().cast_mut());
                let second = hooked(0x60 | 0x8000_0000, table, record.as_ptr(), scratch.as_ptr().cast_mut());
                assert_eq!(first.wrapping_sub(0x4000), second, "the registers the window leaves");
                assert_eq!(std::slice::from_raw_parts(table.add(0x60), 32), &record);
                assert_eq!(std::slice::from_raw_parts(table.add(0x4060), 32), &record);
                let slot = 0x40 + 3;
                assert_eq!(slots[slot], 1);
                assert_eq!(slots.iter().filter(|&&b| b != 0).count(), 1);
                assert_eq!(chunks[slot / 128], 1);
                // Before the heap: stored, not marked.
                slots.fill(0);
                chunks.fill(0);
                let below = heap.add(0x1000);
                hooked(0x20, below, record.as_ptr(), scratch.as_ptr().cast_mut());
                assert_eq!(std::slice::from_raw_parts(below.add(0x20), 32), &record);
                assert!(slots.iter().chain(&chunks).all(|&b| b == 0));
                drop(marks);
            }
        }
    }

    /// The 128-byte copy's window in code of a test's, twice, one copy
    /// hooked: the four records are stored as before, `rcx` is what the
    /// window leaves, and the four slots and their chunks are marked when the
    /// table is in the heap, nothing otherwise.
    #[test]
    fn the_128_byte_copy_stores_and_marks_its_four_slots() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
        // fn(table: rcx, records: rdx, r15: r8) -> rcx as the window leaves it.
        let mut code: Vec<u8> = vec![0x41, 0x57, 0x53]; // push r15 ; push rbx
        code.extend([0x48, 0x89, 0xcb]); // mov rbx, rcx
        code.extend([0x4d, 0x89, 0xc7]); // mov r15, r8
        code.extend([0xc5, 0xfc, 0x10, 0x02]); // vmovups ymm0, [rdx]
        code.extend([0xc5, 0xfc, 0x10, 0x4a, 0x20]); // vmovups ymm1, [rdx+20h]
        code.extend([0xc5, 0xfc, 0x10, 0x52, 0x40]); // vmovups ymm2, [rdx+40h]
        code.extend([0xc5, 0xfc, 0x10, 0x5a, 0x60]); // vmovups ymm3, [rdx+60h]
        let at = code.len();
        // (Its last three: mov rdx, rsi ; mov r8, rbx ; xor r9d, r9d.)
        code.extend(COPY_128.bytes);
        code.extend([0x48, 0x89, 0xc8]); // mov rax, rcx
        code.extend([0x5b, 0x41, 0x5f]); // pop rbx ; pop r15
        code.extend([0xc5, 0xf8, 0x77, 0xc3]); // vzeroupper ; ret
        type F = unsafe extern "system" fn(*mut u8, *const u8, u64) -> u64;
        // SAFETY: fresh memory for code and data, one copy changed before it runs.
        unsafe {
            let place = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
            assert_ne!(place, 0);
            let (plain, hooked) = (place + 0x100 - at, place + 0x1100 - at);
            std::ptr::copy_nonoverlapping(code.as_ptr(), plain as *mut u8, code.len());
            std::ptr::copy_nonoverlapping(code.as_ptr(), hooked as *mut u8, code.len());
            let heap = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
            let (mut slots, mut chunks) = (vec![0u8; 0x8000 / 32], vec![0u8; 0x8000 / 32 / 128]);
            let marks = Box::new(Marks { start: heap as u64 + 0x2000, len: 0x8000, slots: slots.as_mut_ptr(), chunks: chunks.as_mut_ptr() });
            let site = Site { window: 0x1100, ..COPY_128 };
            let mut room = None;
            assert_eq!(copy_128(place, 0x10000, &raw const *marks as usize, &site, &mut room), Hooked::Done);
            assert_eq!(*((place + 0x1100 + COPY_128.store) as *const u8), 0xe9);
            // The `mov rcx, r15` that another path jumps to is left as it was.
            let after = COPY_128.store + COPY_128.len;
            assert_eq!(&COPY_128.bytes[after..after + 3], &[0x4c, 0x89, 0xf9]);
            assert_eq!(std::slice::from_raw_parts((place + 0x1100 + after) as *const u8, COPY_128.bytes.len() - after), &COPY_128.bytes[after..]);
            let (plain, hooked): (F, F) = (std::mem::transmute(plain), std::mem::transmute(hooked));
            let records: [u8; 128] = std::array::from_fn(|k| (k as u8).wrapping_mul(7) ^ 0x3c);
            // Slots 126..130 of the heap: over the end of chunk 0.
            let table = heap.add(0x2000 + 126 * 32);
            assert_eq!(plain(table.add(0x4000), records.as_ptr(), 0x5a5a), 0x5a5a);
            assert_eq!(hooked(table, records.as_ptr(), 0x5a5a), 0x5a5a, "rcx as the window leaves it");
            assert_eq!(std::slice::from_raw_parts(table, 128), &records);
            assert_eq!(std::slice::from_raw_parts(table.add(0x4000), 128), &records);
            assert_eq!(&slots[125..131], &[0, 1, 1, 1, 1, 0]);
            assert_eq!(slots.iter().filter(|&&b| b != 0).count(), 4);
            assert_eq!(&chunks[..2], &[1, 1]);
            // Before the heap, and over its end: stored, not marked.
            slots.fill(0);
            chunks.fill(0);
            for outside in [heap.add(0x1000), heap.add(0x2000 + 0x8000 - 64)] {
                hooked(outside, records.as_ptr(), 1);
                assert_eq!(std::slice::from_raw_parts(outside, 128), &records);
            }
            assert!(slots.iter().chain(&chunks).all(|&b| b == 0));
            drop(marks);
        }
    }

    #[test]
    fn other_bytes_are_left_alone() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
        // SAFETY: fresh memory of our own.
        unsafe {
            let place = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
            let mut bytes = SITES[0].bytes.to_vec();
            bytes[3] = 0x7e; // and ecx, 7EE0h
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), place.add(0x100), bytes.len());
            let mine = [Site { window: 0x100, ..SITES[0] }];
            assert_eq!(apply(place as usize, 0x10000, 0, &mine), [Hooked::Differs]);
            assert_eq!(std::slice::from_raw_parts(place.add(0x100), bytes.len()), &bytes[..]);
            let outside = [Site { window: 0xfff0, ..SITES[0] }];
            assert!(matches!(apply(place as usize, 0x10000, 0, &outside)[..], [Hooked::Failed(_)]));
        }
    }
}
