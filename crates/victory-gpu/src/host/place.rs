//! The title's buffer copies, done on the card at their place in the list
//! (`VICTORY_COPIES_IN_PLACE`, on by default; `0`: all at submission, on the
//! CPU, as before).
//!
//! A buffer copy was done in title memory when its list was submitted, before
//! any draw of the submission ran, and the host copies refreshed after it.
//! That is another order than the list's: draws ahead of the copy read what
//! it put; of two copies to the same bytes with a draw behind each, both
//! draws read the second; a source that a dispatch of the same submission
//! writes first was read as it was before it; and a destination the
//! submissions in the air still read changed under them (where its host copy
//! has one side only, `crate::shadow::card_all`).
//!
//! Now the list copies, in the destination's host copy: out of the source's
//! host copy where shaders write that one, else out of upload memory of the
//! list that takes the source's bytes when the list is submitted (a source
//! shaders never wrote is the title's CPU's, or a copy's put there earlier in
//! the submission). What is written there is a shader's write from then on:
//! the bytes are declared, and what the card made of them is put into title
//! memory before the title is told of the submission.
//!
//! Title memory takes the copy at submission as well, as it did: fills of
//! upload memory, textures and host copies that lie over the same bytes read
//! it there. The reference of the destination's host copy takes the same
//! bytes ([`Shadow::placed_copy`]), so what the landing finds is what the card
//! made of them afterwards (a counter copied to zero and counted up; a source
//! that a shader wrote in between), and nothing is sent up over it. Where
//! flights in the air may write the source or the destination this is left to
//! the landing: what they wrote is not in title memory yet, and what they
//! will put there must not stand over this copy's bytes. If a step of the
//! submission reads the destination on the CPU, it waits for them instead
//! ([`Placed::waits`]).
//!
//! Copies to memory that has no host copy yet are done as before: nothing on
//! the card has read or written it. So are those of more than 8 MiB out of
//! memory shaders do not write (streaming).

use super::*;
use crate::shadow::Shadow;

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COPIES_IN_PLACE").is_none_or(|v| v != "0"))
}

/// The most bytes copied out of memory shaders do not write.
const MOST: usize = 8 << 20;

/// Since the last frame-rate line: copies at their place, their bytes, those
/// out of a host copy, and texture copies out of one.
pub(super) static STATS: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// Since the last frame-rate line: constant buffers copied on the card out of
/// the host copy a copy at its place wrote ([`HostList::place_constant`]).
pub(super) static CONSTANTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether a constant buffer whose bytes a copy at its place wrote earlier in
/// the same list ([`HostList::place_copy`]) is copied on the card, at its
/// place, out of that copy's destination (on by default with the copies at
/// their place; `VICTORY_PLACED_CONSTANTS=0`: filled on the CPU at submission,
/// as before). Read once. From trustno1 PR #34.
///
/// Filled at submission it read title memory, where the copy's bytes are only
/// if no flight in the air kept them out (else the submission waited for the
/// flights, [`Placed::waits`]); of two copies to the same bytes it read the
/// second; and under `VICTORY_CARD_STEPS` the fill behind the copy had the
/// whole submission refused (`card_steps::BEHIND_PLACED`).
fn placed_constants() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| on() && std::env::var_os("VICTORY_PLACED_CONSTANTS").is_none_or(|v| v != "0"))
}

/// What the CPU does at submission for a copy at its place in a list.
pub(super) struct Placed {
    pub(super) dst: usize,
    pub(super) src: usize,
    pub(super) len: usize,
    /// The destination's host copy.
    pub(super) shadow: std::sync::Arc<Shadow>,
    /// Upload memory the list copies from, to be given the source's bytes.
    from: Option<usize>,
}

impl Placed {
    /// Whether flights that write `written` are waited for.
    pub(super) fn waits(&self, written: &[(usize, usize)]) -> Waits {
        let source = touches(written, self.src, self.src + self.len);
        if source && self.from.is_some() {
            return Waits::Yes;
        }
        match source || touches(written, self.dst, self.dst + self.len) {
            true => Waits::ForReaders,
            false => Waits::No,
        }
    }

    /// At submission, in the order of the steps: the list's upload memory
    /// takes the source's bytes and, if `free` (no flight in the air may write
    /// source or destination), title memory at the destination does. Gives
    /// the bytes if it took them.
    pub(super) fn prepare(&self, free: bool) -> Option<Vec<u8>> {
        if !free && self.from.is_none() {
            return None;
        }
        let mut bytes = vec![0u8; self.len];
        // (What is not there reads as zeros, as any copy's source.)
        copy_mapped(bytes.as_mut_ptr(), self.src as *const u8, self.len, false);
        if let Some(to) = self.from {
            // SAFETY: the list's own upload memory of that many bytes.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), to as *mut u8, self.len) };
        }
        if !free {
            return None;
        }
        copy_mapped(self.dst as *mut u8, bytes.as_ptr(), self.len, true);
        crate::watch::wrote(self.dst, self.len);
        // Title memory that is not all there was left alone.
        let mut there = vec![0u8; self.len];
        (read_into(self.dst as *const u8, &mut there) && there == bytes).then_some(bytes)
    }
}

/// Whether a copy waits for flights before its submission is prepared: not;
/// yes, as it takes a source on the CPU that they may write; or only if
/// another step of the submission reads its destination on the CPU.
pub(super) enum Waits {
    No,
    ForReaders,
    Yes,
}

/// Whether a step of `runs` reads one of `ranges` on the CPU.
pub(super) fn read_by(runs: &[&HostList], mut ranges: Vec<(usize, usize)>) -> bool {
    join_ranges(&mut ranges, []);
    runs.iter().any(|list| {
        list.deferred().copies.iter().any(|step| match step {
            &Step::Copy(_, src, len) | &Step::Fill(_, src, len, _) => touches(&ranges, src, src + len),
            Step::Texture(staging) => {
                let (at, len) = staging.memory();
                touches(&ranges, at, at + len)
            }
            &Step::Plane((at, len), _) => touches(&ranges, at, at.saturating_add(len)),
            Step::Host(_, (from, to)) => touches(&ranges, *from, *to),
            Step::Placed(p) => p.from.is_some() && touches(&ranges, p.src, p.src + p.len),
            _ => false,
        })
    })
}

/// Everything recorded so far is done, and seen, before what follows.
///
/// # Safety
/// An open list.
unsafe fn wait_for_all(host: &Host, list: &ID3D12GraphicsCommandList) {
    static ENHANCED: OnceLock<bool> = OnceLock::new();
    let enhanced = *ENHANCED.get_or_init(|| {
        let mut o = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
        // SAFETY: a feature structure of its size.
        let known = unsafe {
            host.device.CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS12,
                (&raw mut o).cast(),
                size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS12>() as u32,
            )
        };
        known.is_ok() && o.EnhancedBarriersSupported.as_bool()
    });
    // SAFETY: the caller's.
    unsafe {
        if enhanced && let Ok(list) = list.cast::<ID3D12GraphicsCommandList7>() {
            let all = [D3D12_GLOBAL_BARRIER {
                SyncBefore: D3D12_BARRIER_SYNC_ALL,
                SyncAfter: D3D12_BARRIER_SYNC_ALL,
                AccessBefore: D3D12_BARRIER_ACCESS_COMMON,
                AccessAfter: D3D12_BARRIER_ACCESS_COMMON,
            }];
            list.Barrier(&[D3D12_BARRIER_GROUP {
                Type: D3D12_BARRIER_TYPE_GLOBAL,
                NumBarriers: 1,
                Anonymous: D3D12_BARRIER_GROUP_0 {
                    pGlobalBarriers: all.as_ptr(),
                },
            }]);
            return;
        }
        let alias = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Aliasing: std::mem::ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                    pResourceBefore: std::mem::ManuallyDrop::new(None),
                    pResourceAfter: std::mem::ManuallyDrop::new(None),
                }),
            },
        };
        let uav = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: std::mem::ManuallyDrop::new(None),
                }),
            },
        };
        list.ResourceBarrier(&[alias, uav]);
    }
}

/// The host copy of `[src, src + len)` if shaders write it: what it holds at
/// a place in a list is not what title memory holds at submission.
fn shaders_own(src: usize, len: usize) -> Option<std::sync::Arc<Shadow>> {
    crate::shadow::peek(src as u64, len as u64).filter(|s| s.written() && !s.in_place())
}

impl HostList {
    /// Copies `len` bytes of title memory at `src` to `dst`, at this place of
    /// the list (see the file). `false` if it is to be done at submission.
    pub(crate) fn place_copy(&self, dst: usize, src: usize, len: usize) -> bool {
        if !on() {
            return false;
        }
        let Some(host) = host() else {
            return false;
        };
        let Some(to) = crate::shadow::peek(dst as u64, len as u64).filter(|s| !s.in_place()) else {
            return false;
        };
        let own = shaders_own(src, len);
        let scratch = match &own {
            Some(_) => None,
            None if len <= MOST => match self.scratch_at(len as u64) {
                Some(scratch) => Some(scratch),
                None => return false,
            },
            None => return false,
        };
        let at = to.offset(dst as u64);
        // What the title's CPU wrote around and under it so far goes first.
        to.refresh(self, dst as u64, len as u64);
        // SAFETY (both): an open list; live buffers (the list holds the
        // copies and its scratch memory), ranges inside them.
        unsafe { wait_for_all(host, &self.list) };
        match (&own, &scratch) {
            (Some(from), _) => {
                from.refresh(self, src as u64, len as u64);
                let offset = from.offset(src as u64);
                if from.buffer.as_raw() == to.buffer.as_raw() {
                    let Some(between) = self.between(host, len as u64) else {
                        return false;
                    };
                    unsafe {
                        self.list.CopyBufferRegion(&between, 0, &from.buffer, offset, len as u64);
                        self.list.ResourceBarrier(&[transition(&between, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                        wait_for_all(host, &self.list);
                        self.list.CopyBufferRegion(&to.buffer, at, &between, 0, len as u64);
                    }
                } else {
                    unsafe { self.list.CopyBufferRegion(&to.buffer, at, &from.buffer, offset, len as u64) };
                }
                STATS[2].fetch_add(1, Ordering::Relaxed);
            }
            (None, Some((from, offset, _))) => unsafe {
                self.list.CopyBufferRegion(&to.buffer, at, from, *offset, len as u64);
            },
            (None, None) => return false,
        }
        unsafe { wait_for_all(host, &self.list) };
        STATS[0].fetch_add(1, Ordering::Relaxed);
        STATS[1].fetch_add(len as u64, Ordering::Relaxed);
        to.gpu_writes(self, dst as u64, len as u64);
        let mut deferred = self.deferred();
        deferred.placed_written.push((dst, dst + len, to.clone()));
        deferred.copies.push(Step::Placed(Placed {
            dst,
            src,
            len,
            shadow: to,
            from: scratch.map(|s| s.2 as usize),
        }));
        deferred.title_written();
        self.has_placed.store(true, Ordering::Relaxed);
        self.changed_something();
        true
    }

    /// For a constant buffer of title memory `[src, src + len)`: if the last
    /// thing this recording did to those bytes is a [`HostList::place_copy`]
    /// that covers all of them, a copy of them made on the card at this place
    /// of the list, out of that copy's destination: the buffer and the
    /// address in it (on 256 bytes, readable as constants). `None`: filled as
    /// any other ([`placed_constants`]).
    ///
    /// No step is recorded for it: nothing of it is read on the CPU. So the
    /// steps done on the card in front of the lists (`card_steps`) do not see
    /// it, and neither does their check for places the title writes after
    /// sending (`card_steps::late`), on purpose: these bytes are what the
    /// list's own copy put there when it ran, as on the console, whatever
    /// the title's CPU writes at that address before the flight lands. What
    /// the title may still write late is the copy's source, which was no
    /// fill before this either.
    pub(crate) fn place_constant(&self, src: usize, len: usize) -> Option<(ID3D12Resource, u64)> {
        if !placed_constants() || len == 0 || !self.has_placed.load(Ordering::Relaxed) {
            return None;
        }
        let end = src.checked_add(len)?;
        let host = host()?;
        let alloc = (len as u64).next_multiple_of(256);
        let (from, mut cbv_slot) = {
            let mut deferred = self.deferred();
            let mut matched = None;
            for (p_dst, p_end, shadow) in deferred.placed_written.iter().rev() {
                if *p_end <= src || *p_dst >= end {
                    continue;
                }
                if *p_dst <= src && end <= *p_end {
                    let (start, shadow_len) = shadow.span();
                    if !shadow.in_place() && (src as u64) >= start && (end as u64) <= start + shadow_len {
                        matched = Some(shadow.clone());
                    }
                }
                break;
            }
            let from = matched?;
            let slot = match deferred.placed_cbv.as_mut() {
                Some((buf, cap, used)) if *used + alloc <= *cap => {
                    let at = *used;
                    *used += alloc;
                    Some((buf.clone(), at, true))
                }
                _ => None,
            };
            (from, slot)
        };
        if cbv_slot.is_none() {
            let cap = alloc.max(64 << 10);
            let buf = host.uav_buffer(cap)?;
            name(&buf, || "placed constants on the card".into());
            let old = {
                let mut deferred = self.deferred();
                match deferred.placed_cbv.as_mut() {
                    Some((existing, existing_cap, used)) if *used + alloc <= *existing_cap => {
                        let at = *used;
                        *used += alloc;
                        cbv_slot = Some((existing.clone(), at, true));
                        None
                    }
                    _ => {
                        cbv_slot = Some((buf.clone(), 0, false));
                        deferred.placed_cbv.replace((buf, cap, alloc)).map(|(b, _, _)| b)
                    }
                }
            };
            if let Some(old) = old {
                self.keep(&old);
            }
        }
        let (cbv_buf, at, was_used) = cbv_slot?;
        // SAFETY: `self.list` is open; `cbv_buf` is kept alive in `Deferred`
        // until reset, `from` is a live host copy covering `[src, end)`, and
        // `at..at + len` fits in `cbv_buf`.
        unsafe {
            if was_used {
                self.list.ResourceBarrier(&[transition(
                    &cbv_buf,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )]);
            }
            wait_for_all(host, &self.list);
            self.list.CopyBufferRegion(&cbv_buf, at, &from.buffer, from.offset(src as u64), len as u64);
            self.list.ResourceBarrier(&[transition(
                &cbv_buf,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_GENERIC_READ,
            )]);
            let gpu = cbv_buf.GetGPUVirtualAddress() + at;
            CONSTANTS.fetch_add(1, Ordering::Relaxed);
            Some((cbv_buf, gpu))
        }
    }

    /// For a copy of `[src, src + len)` into a texture: a buffer (readable by
    /// copies) that holds what shaders wrote there, at this place of the list.
    /// `None` where shaders do not write it.
    pub(crate) fn place_source(&self, src: usize, len: usize) -> Option<ID3D12Resource> {
        if !on() {
            return None;
        }
        let host = host()?;
        let from = shaders_own(src, len)?;
        let between = self.between(host, len as u64)?;
        from.refresh(self, src as u64, len as u64);
        STATS[3].fetch_add(1, Ordering::Relaxed);
        // SAFETY: an open list; live buffers, ranges inside them.
        unsafe {
            wait_for_all(host, &self.list);
            self.list.CopyBufferRegion(&between, 0, &from.buffer, from.offset(src as u64), len as u64);
            self.list.ResourceBarrier(&[transition(&between, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            wait_for_all(host, &self.list);
        }
        Some(between)
    }

    /// A buffer on the card for this recording alone.
    fn between(&self, host: &Host, len: u64) -> Option<ID3D12Resource> {
        let buffer = host.uav_buffer(len.next_multiple_of(1 << 16))?;
        name(&buffer, || "bytes between copies".into());
        self.keep(&buffer);
        Some(buffer)
    }
}

#[cfg(test)]
mod tests {
    /// What copies with two sides cost to make (ignored: a measurement for
    /// `VICTORY_CARD_ALL`): buffers of 64 KiB on the card and in the PC's
    /// memory.
    #[test]
    #[ignore]
    fn small_buffers_cost() {
        let Some(host) = super::host() else {
            return;
        };
        const N: usize = 1000;
        let used = || host.video_memory().map_or(0, |m| m[1] >> 20);
        let before = used();
        let t = std::time::Instant::now();
        let card: Vec<_> = (0..N).map(|_| host.uav_buffer(1 << 16)).collect();
        let card_us = t.elapsed().as_micros() as f64 / N as f64;
        let after = used();
        let t = std::time::Instant::now();
        let pc: Vec<_> = (0..N).map(|_| host.shared_buffer(1 << 16)).collect();
        let pc_us = t.elapsed().as_micros() as f64 / N as f64;
        println!(
            "{N} buffers of 64 KiB: on the card {card_us:.0} us each, {} MB of its memory; in the PC's memory {pc_us:.0} us each",
            after.saturating_sub(before)
        );
        drop((card, pc));
    }

    #[test]
    fn place_constant_reads_what_place_copy_wrote() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree};
        let Some(host) = super::host() else {
            return;
        };
        const LEN: usize = 64 << 10;
        // SAFETY: fresh 64 KiB page-aligned allocation for the test.
        let base = unsafe { VirtualAlloc(std::ptr::null(), LEN, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        assert!(base != 0);
        // Initialize source bytes outside the destination range.
        let src = base + 4096;
        let dst = base + 8192;
        // SAFETY: `base..base + LEN` is committed read-write memory.
        unsafe {
            for i in 0..512 {
                ((src + i) as *mut u8).write((i.wrapping_mul(37) ^ 0xa5) as u8);
            }
        }
        crate::shadow::register(0x501a_ce01, base as u64, LEN as u64);
        let _shadow = crate::shadow::find(dst as u64, 512).expect("shadow for registered range");
        let list = host.new_list().unwrap();
        assert!(list.place_copy(dst, src, 512));
        // Partial overlap or disjoint ranges must bail out cleanly.
        assert!(list.place_constant(dst - 16, 64).is_none());
        assert!(list.place_constant(dst + 480, 64).is_none());
        assert!(list.place_constant(base + 16384, 64).is_none());
        let (buf1, gpu1) = list.place_constant(dst, 256).expect("first slice inside placed copy");
        let (buf2, gpu2) = list.place_constant(dst + 256, 256).expect("second slice inside placed copy");
        assert_eq!(gpu1 % 256, 0);
        assert_eq!(gpu2 % 256, 0);
        assert!(!list.deferred().copies.iter().any(|s| matches!(s, super::Step::Fill(..))));

        let (out, out_ptr) = host.shared_buffer(512).unwrap();
        // SAFETY: `list` is open; offsets match `gpu1`/`gpu2` inside `buf1`/`buf2`.
        unsafe {
            let off1 = gpu1 - buf1.GetGPUVirtualAddress();
            let off2 = gpu2 - buf2.GetGPUVirtualAddress();
            list.list.CopyBufferRegion(&out, 0, &buf1, off1, 256);
            list.list.CopyBufferRegion(&out, 256, &buf2, off2, 256);
        }
        list.close();
        host.execute(&[&list]);
        assert_eq!(host.log_messages(), None);
        // SAFETY: `out_ptr` is a 512-byte shared buffer mapping, and `src` is inside `base..base + LEN`.
        let got = unsafe { std::slice::from_raw_parts(out_ptr, 512) };
        let expected = unsafe { std::slice::from_raw_parts(src as *const u8, 512) };
        assert_eq!(got, expected);
        crate::shadow::unregister(0x501a_ce01, base as u64);
        // SAFETY: `base` was allocated with `VirtualAlloc` and `list` has finished.
        unsafe {
            VirtualFree(base as *mut _, 0, MEM_RELEASE);
        }
    }
}
