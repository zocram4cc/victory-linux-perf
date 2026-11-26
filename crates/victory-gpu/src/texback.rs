//! Textures shaders write that the title's own code then reads on the CPU.
//! On the console the CPU and the GPU share memory, so what a compute pass
//! stores into a texture is in title memory for the CPU to read. Here it stays
//! in the texture's host copy. The title's ocean is one: its wave passes write
//! a 256x256 height map (R32_FLOAT) and a displacement map (R32G32_FLOAT), and
//! its CPU code samples both, which is what boats float on; it read memory
//! nothing had written (run rw1, `VICTORY_READ_WATCH=1`: about 30 places in
//! the title read those two kinds), and boats sank.
//!
//! For textures of the kinds in `VICTORY_TEXTURE_BACK` (comma-separated
//! `<width>x<height>f<format>`; unset or `1`: the four water formats found by
//! the upstream trace; `0`: none), a list in which shaders write one copies it
//! into a read-back buffer of ours when it closes, and once the list has run
//! the texels are tiled into the texture's title memory in the console's
//! layout, as the console's GPU would have left them.
//!
//! shortcut: by kind, not by texture: textures of those kinds the title never
//! reads are copied back too (about a megabyte per such list). The copy shows
//! what the list's last write left; a title read in between sees the frame
//! before, as it would before the console's GPU has run.
//!
//! Under Wine ([`hardened`]) four things more, from the owner's Proton tree
//! after a play crash review (its texback.rs, aec7e29 and c41a7ad):
//! - a mark behind the copies (`VICTORY_FETCH_MARK`, `=0` off): the card may
//!   finish a list's fence on a lost device before the loss is reported, and
//!   then the buffer holds nothing the copies wrote (zeros), which would be
//!   tiled into title memory as the ocean. A landing without its mark writes
//!   nothing.
//! - only the 64-byte pieces that changed are written: bytes the card left as
//!   the title had them are not written back over a title write that lands
//!   between our read of title memory and our write.
//! - only textures of one subresource (and of the title's format) are taken:
//!   an unordered view writes one mip, and the others' card contents (never
//!   uploaded from title memory: a texture shaders write is the host's own)
//!   went over the title's texels; every texture taken in the owner's session
//!   had one mip and one slice. Layouts the formats crate cannot tile are not
//!   taken either.
//! - a line per texture taken, once, and every 10 s the counts (taken,
//!   recorded, landed, unchanged, stale, unmarked, failed, bytes), in every
//!   build: the owner's play sessions are the test.
//!
//! Each changed piece goes in through `crate::host::copy_mapped` as before
//! (it opens the piece's guards, so the guards see it written, and tells the
//! descriptor side), one call per piece.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use windows::Win32::Graphics::Direct3D12::*;

use crate::object::This;

/// The kinds copied back: (width, height, format).
fn kinds() -> &'static [(u64, u32, u32)] {
    static KINDS: OnceLock<Vec<(u64, u32, u32)>> = OnceLock::new();
    KINDS.get_or_init(|| {
        const DEFAULT: &str = "256x256f41,256x256f16,64x64f16,128x128f36";
        let list = match std::env::var("VICTORY_TEXTURE_BACK").as_deref() {
            Ok("1") | Err(_) => DEFAULT.to_owned(),
            Ok(spec) => spec.to_owned(),
        };
        let kinds: Vec<(u64, u32, u32)> = list
            .split(',')
            .filter_map(|k| {
                let (size, format) = k.trim().split_once('f')?;
                let (w, h) = size.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?, format.parse().ok()?))
            })
            .collect();
        if !kinds.is_empty() {
            tracing::info!(?kinds, "textures of these kinds that shaders write are copied back into title memory (VICTORY_TEXTURE_BACK)");
        }
        kinds
    })
}

/// Whether the checks and counts of the module's comment are on: under Wine
/// only, so the read-back on Windows stays as it was (untested there).
fn hardened() -> bool {
    crate::wine::on_wine()
}

/// With [`hardened`]: whether a read-back writes a mark behind its copies and
/// lands only if the mark is there (`VICTORY_FETCH_MARK=0` turns it off).
///
/// shortcut: a copy of the owner's `crate::host::fetch_mark`, which also
/// marks the relay's fetch; one of the two goes when both are in a tree.
fn fetch_mark() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_FETCH_MARK").is_none_or(|v| v != "0"))
}

/// The mark read-back number `sent` writes: never 0, so a buffer the card
/// never wrote does not pass, and never the mark of the one before, so that
/// one's leftover does not pass either.
fn mark_value(sent: u64) -> u32 {
    (sent as u32) | 0x8000_0000
}

/// Whether title texture `resource` is of a kind copied back.
pub(crate) fn wanted(resource: This) -> bool {
    let kinds = kinds();
    !kinds.is_empty()
        && crate::resource::texture_kind(resource).is_some_and(|k| kinds.contains(&k))
}

/// An outstanding write to a specific allocation. Mapping and resource
/// changes invalidate only the writes that could touch the replaced memory.
struct Ticket {
    owner: usize,
    at: usize,
    end: usize,
    valid: AtomicBool,
}

static TICKETS: Mutex<Vec<Weak<Ticket>>> = Mutex::new(Vec::new());
static TRACKING: AtomicBool = AtomicBool::new(false);

fn track(owner: usize, at: usize, end: usize) -> Arc<Ticket> {
    let ticket = Arc::new(Ticket { owner, at, end, valid: AtomicBool::new(true) });
    let mut tickets = TICKETS.lock().unwrap_or_else(|e| e.into_inner());
    tickets.retain(|weak| weak.strong_count() != 0);
    tickets.push(Arc::downgrade(&ticket));
    TRACKING.store(true, Ordering::Relaxed);
    ticket
}

fn invalidate(matches: impl Fn(&Ticket) -> bool) {
    if !TRACKING.load(Ordering::Relaxed) {
        return;
    }
    TICKETS.lock().unwrap_or_else(|e| e.into_inner()).retain(|weak| {
        let Some(ticket) = weak.upgrade() else { return false };
        if matches(&ticket) {
            ticket.valid.store(false, Ordering::Release);
        }
        true
    });
}

pub(crate) fn released(owner: usize) {
    invalidate(|ticket| ticket.owner == owner);
}

pub(crate) fn replaced(at: usize) {
    invalidate(|ticket| ticket.at == at);
}

pub(crate) fn remapped(at: usize, len: usize) {
    let end = at.saturating_add(len);
    invalidate(|ticket| at < ticket.end && ticket.at < end);
}

pub(crate) struct Target {
    pub(crate) copy: Arc<crate::resource::HostTexture>,
    layout: victory_formats::texture::Layout,
    /// The title's format.
    format: u32,
    ticket: Arc<Ticket>,
}

impl Target {
    pub(crate) fn new(resource: This) -> Option<Self> {
        // Keep the host copy independently of the title's resource lifetime.
        let copy = crate::resource::host_copy(resource)?;
        let _mapping = crate::watch::title_memory_reading();
        let (layout, at, size) = crate::resource::title_texture(resource)?;
        let (_, _, format) = crate::resource::texture_kind(resource)?;
        if hardened() {
            first_seen(resource, at, size, &layout);
            // A layout the formats crate cannot tile (the 64 KiB XOR modes:
            // one 256x256 R32_FLOAT texture in SW_64KB_R_X in the owner's run
            // water-2, beside 45 it can) is not copied at all.
            if !layout.addressable() {
                return None;
            }
        }
        let end = at.checked_add(size).filter(|&end| at != 0 && end > at)?;
        let ticket = track(resource as usize, at, end);
        Some(Self { copy, layout, format, ticket })
    }

    pub(crate) fn owner(&self) -> usize { self.ticket.owner }
}

/// Logs each texture the read-back takes, once (by its title memory and
/// kind).
fn first_seen(resource: This, at: usize, size: usize, layout: &victory_formats::texture::Layout) {
    static SEEN: Mutex<Option<std::collections::HashSet<(usize, u64, u32, u32)>>> = Mutex::new(None);
    let Some((width, height, format)) = crate::resource::texture_kind(resource) else {
        return;
    };
    let new = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .insert((at, width, height, format));
    if new {
        TEXTURES.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            width,
            height,
            format,
            memory = format_args!("{at:#x}"),
            bytes = size,
            mips = layout.desc.mips,
            tileable = layout.addressable(),
            "a texture shaders write is copied back into title memory (VICTORY_TEXTURE_BACK)"
        );
    }
}

/// With [`hardened`], since the start: textures taken (distinct), and by
/// index below the copies recorded, landed and so on.
static TEXTURES: AtomicU64 = AtomicU64::new(0);
static STATS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
const RECORDED: usize = 0;
/// Time recording them (us).
const RECORD_US: usize = 1;
const LANDED: usize = 2;
/// Bytes written into title memory.
const BYTES: usize = 3;
/// Copies landed that changed nothing (not written).
const UNCHANGED: usize = 4;
/// Copies whose allocation went meanwhile.
const STALE: usize = 5;
/// Time landing (us).
const LAND_US: usize = 6;
/// Copies landed that could not be: title memory not all readable, the
/// read-back not mapped, or the texels not tiled.
const FAILED: usize = 7;
/// Textures not recorded: the host copy's format is not the title's, or it
/// has more than one subresource.
const OTHER_FORMAT: usize = 8;
/// Copies whose mark was not behind them: not written.
const UNMARKED: usize = 9;

/// Every 10 s while read-backs happen: the counts of the last 10 s.
fn report() {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    static NEXT_MS: AtomicU64 = AtomicU64::new(10_000);
    static LAST: Mutex<[u64; 10]> = Mutex::new([0; 10]);
    let now = START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64;
    let next = NEXT_MS.load(Ordering::Relaxed);
    if now < next || NEXT_MS.compare_exchange(next, now + 10_000, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    let stats: [u64; 10] = std::array::from_fn(|i| STATS[i].load(Ordering::Relaxed));
    let d: [u64; 10] = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let d = std::array::from_fn(|i| stats[i] - last[i]);
        *last = stats;
        d
    };
    tracing::info!(
        "texture read-back (VICTORY_TEXTURE_BACK), last 10 s: {} textures taken so far; {} copies recorded ({} not: \
         host format differs or more than one subresource), {} landed ({} unchanged), {} stale, {} unmarked, {} failed \
         (memory unreadable, not mapped or not tiled); {} KiB written; CPU {:.2} ms recording, {:.2} ms landing",
        TEXTURES.load(Ordering::Relaxed),
        d[RECORDED],
        d[OTHER_FORMAT],
        d[LANDED],
        d[UNCHANGED],
        d[STALE],
        d[UNMARKED],
        d[FAILED],
        d[BYTES] >> 10,
        d[RECORD_US] as f64 / 1e3,
        d[LAND_US] as f64 / 1e3,
    );
}

/// The readback resources and the allocation the pixels belong to.
pub(crate) struct Back {
    buffer: ID3D12Resource,
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    rows: Vec<u32>,
    mips: u32,
    layout: victory_formats::texture::Layout,
    ticket: Arc<Ticket>,
    /// With [`fetch_mark`]: where in `buffer` the list writes its mark
    /// behind the copies, and the mark.
    mark: Option<(u64, u32)>,
}

impl Back {
    pub(crate) fn range(&self) -> (usize, usize) {
        (self.ticket.at, self.ticket.end)
    }
}

/// Records a host copy from a texture already in COPY_SOURCE.
pub(crate) fn record(list: &ID3D12GraphicsCommandList, target: Target) -> Option<Back> {
    if !target.ticket.valid.load(Ordering::Acquire) {
        return None;
    }
    let host = crate::host::host()?;
    let hardened = hardened();
    let started = std::time::Instant::now();
    let texture = &target.copy.texture;
    // SAFETY: a live texture kept through the command list's state tracking.
    let desc = unsafe { texture.GetDesc() };
    let mips = u32::from(desc.MipLevels.max(1));
    let subresources = mips * u32::from(desc.DepthOrArraySize.max(1));
    if hardened && (desc.Format.0 as u32 != target.format || subresources != 1) {
        STATS[OTHER_FORMAT].fetch_add(1, Ordering::Relaxed);
        report();
        return None;
    }
    let n = subresources as usize;
    let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
    let mut rows = vec![0u32; n];
    let mut total = 0u64;
    // SAFETY: out arrays of `n` entries.
    unsafe {
        host.device.GetCopyableFootprints(
            &desc,
            0,
            subresources,
            0,
            Some(footprints.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            None,
            Some(&mut total),
        )
    };
    // The last footprint's advertised extent can omit row-end padding.
    // The tiler reads a pitched slice, so the mapped buffer must include it.
    for (footprint, &rows) in footprints.iter().zip(&rows) {
        let format = footprint.Footprint;
        let pitched = u64::from(format.RowPitch)
            .checked_mul(u64::from(rows))?
            .checked_mul(u64::from(format.Depth))?;
        total = total.max(footprint.Offset.checked_add(pitched)?);
    }
    let mut mark = (hardened && fetch_mark()).then(|| {
        static SENT: AtomicU64 = AtomicU64::new(0);
        (total.next_multiple_of(4), mark_value(SENT.fetch_add(1, Ordering::Relaxed) + 1))
    });
    let buffer = host.read_back_buffer(total + if mark.is_some() { 8 } else { 0 })?;
    for (i, footprint) in footprints.iter().enumerate() {
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: a borrowed pointer the location never releases.
            pResource: unsafe { std::mem::transmute_copy(&buffer) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *footprint },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above.
            pResource: unsafe { std::mem::transmute_copy(texture) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: i as u32 },
        };
        // SAFETY: an open list; the texture is in the copy-source state.
        unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
    }
    if let Some((at, value)) = mark {
        use windows::core::Interface;
        // SAFETY: our buffer, in the copy destination state; the mark lies
        // past every footprint.
        unsafe {
            let write = [D3D12_WRITEBUFFERIMMEDIATE_PARAMETER { Dest: buffer.GetGPUVirtualAddress() + at, Value: value }];
            // Without the interface the copies are recorded all the same,
            // and the buffer must outlive the list: kept, without a mark
            // (returning here would drop a buffer the list names).
            match list.cast::<ID3D12GraphicsCommandList2>() {
                Ok(list) => {
                    list.WriteBufferImmediate(1, write.as_ptr(), Some([D3D12_WRITEBUFFERIMMEDIATE_MODE_MARKER_OUT].as_ptr()))
                }
                Err(_) => mark = None,
            }
        }
    }
    if hardened {
        STATS[RECORDED].fetch_add(1, Ordering::Relaxed);
        STATS[RECORD_US].fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        report();
    }
    Some(Back { buffer, footprints, rows, mips, layout: target.layout, ticket: target.ticket, mark })
}

/// Once the list has run, tile its texels back into the same title allocation.
pub(crate) fn land(back: &Back) {
    if hardened() {
        let started = std::time::Instant::now();
        land_checked(back);
        STATS[LAND_US].fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        report();
        return;
    }
    static COPIED: AtomicU64 = AtomicU64::new(0);
    static SKIPPED: AtomicU64 = AtomicU64::new(0);
    let _mapping = crate::watch::title_memory_reading();
    let _ticket_guard = TICKETS.lock().unwrap_or_else(|e| e.into_inner());
    if !back.ticket.valid.load(Ordering::Acquire) {
        let skipped = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
        if skipped.is_power_of_two() {
            tracing::info!(skipped, "skipping texture readbacks whose title allocation changed");
        }
        return;
    }
    let (at, end) = back.range();
    let size = end - at;
    let mut tiled = vec![0u8; size];
    // Keep title bytes the subresources do not cover.
    if !crate::host::read_into(at as *const u8, &mut tiled) {
        return;
    }
    if tile_into(back, &mut tiled) != Tiled::Done {
        return;
    }
    crate::host::copy_mapped(at as *mut u8, tiled.as_ptr(), size, true);
    let copied = COPIED.fetch_add(1, Ordering::Relaxed) + 1;
    if copied == 1 || copied.is_multiple_of(128) {
        tracing::info!(copied, bytes = size, "texture readback landed");
    }
}

#[derive(PartialEq)]
enum Tiled {
    Done,
    /// The mark was not behind the copies.
    Unmarked,
    /// The buffer could not be mapped or a subresource not tiled.
    Failed,
}

/// Maps `back`'s buffer and tiles its texels into `tiled` (title memory as
/// read; maybe partly written if this fails).
fn tile_into(back: &Back, tiled: &mut [u8]) -> Tiled {
    let mut data = std::ptr::null_mut();
    // SAFETY: our read-back buffer, written by the list that has run.
    unsafe {
        if back.buffer.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
            return Tiled::Failed;
        }
    }
    if let Some((at, value)) = back.mark {
        // SAFETY: inside the mapped buffer (made 8 bytes past the mark).
        let seen = unsafe { std::ptr::read_volatile(data.cast::<u8>().add(at as usize).cast::<u32>()) };
        if seen != value {
            // SAFETY: mapped above.
            unsafe { back.buffer.Unmap(0, None) };
            let n = STATS[UNMARKED].fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                tracing::warn!(n, seen, value, "a texture read-back's mark was not behind its copies: not written (VICTORY_FETCH_MARK)");
            }
            return Tiled::Unmarked;
        }
    }
    let mut tiled_all = Tiled::Done;
    for (i, (footprint, &rows)) in back.footprints.iter().zip(&back.rows).enumerate() {
        let (mip, slice) = (i as u32 % back.mips, i as u32 / back.mips);
        let f = footprint.Footprint;
        let pitch = f.RowPitch as usize;
        let len = pitch * rows as usize * f.Depth as usize;
        // SAFETY: the subresource's part of the mapped buffer.
        let linear = unsafe { std::slice::from_raw_parts(data.cast::<u8>().add(footprint.Offset as usize), len) };
        if let Err(e) = back.layout.tile(tiled, mip, slice, linear, pitch, pitch * rows as usize) {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!(?e, mip, slice, "a texture copied back could not be tiled"));
            tiled_all = Tiled::Failed;
            break;
        }
    }
    // SAFETY: mapped above.
    unsafe { back.buffer.Unmap(0, None) };
    tiled_all
}

/// [`land`] under Wine ([`hardened`]): nothing without the mark, and only
/// the pieces that changed are written.
fn land_checked(back: &Back) {
    let _mapping = crate::watch::title_memory_reading();
    // Held across the write: a release or remap of the allocation waits for
    // it, and one before it makes the ticket stale.
    let _tickets = TICKETS.lock().unwrap_or_else(|e| e.into_inner());
    if !back.ticket.valid.load(Ordering::Acquire) {
        let skipped = STATS[STALE].fetch_add(1, Ordering::Relaxed) + 1;
        if skipped.is_power_of_two() {
            tracing::info!(skipped, "texture read-backs skipped: their title allocation changed");
        }
        return;
    }
    let (at, end) = back.range();
    let size = end - at;
    let mut read = vec![0u8; size];
    // The title's bytes the subresources do not cover are kept.
    if !crate::host::read_into(at as *const u8, &mut read) {
        STATS[FAILED].fetch_add(1, Ordering::Relaxed);
        return;
    }
    let mut tiled = read.clone();
    match tile_into(back, &mut tiled) {
        Tiled::Done => {}
        Tiled::Unmarked => return,
        Tiled::Failed => {
            STATS[FAILED].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    STATS[LANDED].fetch_add(1, Ordering::Relaxed);
    if tiled == read {
        STATS[UNCHANGED].fetch_add(1, Ordering::Relaxed);
        return;
    }
    // Only the pieces that changed are written: bytes the card left as the
    // title had them are not written back over a title write that may land
    // between the read above and the write.
    let mut written = 0;
    for (lo, hi) in changed_runs(&tiled, &read) {
        crate::host::copy_mapped((at + lo) as *mut u8, tiled[lo..hi].as_ptr(), hi - lo, true);
        written += hi - lo;
    }
    STATS[BYTES].fetch_add(written as u64, Ordering::Relaxed);
}

/// The runs of 64-byte pieces where `new` differs from `old` (equal
/// lengths), joined where they touch: (start, end) offsets.
fn changed_runs(new: &[u8], old: &[u8]) -> Vec<(usize, usize)> {
    const PIECE: usize = 64;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, (a, b)) in new.chunks(PIECE).zip(old.chunks(PIECE)).enumerate() {
        if a != b {
            let (lo, hi) = (i * PIECE, i * PIECE + a.len());
            match runs.last_mut() {
                Some(last) if last.1 == lo => last.1 = hi,
                _ => runs.push((lo, hi)),
            }
        }
    }
    runs
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_readbacks_for_changed_allocations_are_invalidated() {
        let first = track(91, 0x1000, 0x2000);
        let next = track(92, 0x2000, 0x3000);
        let far = track(93, 0x4000, 0x5000);

        remapped(0x0f00, 0x200);
        assert!(!first.valid.load(Ordering::Acquire));
        assert!(next.valid.load(Ordering::Acquire));
        replaced(0x2000);
        assert!(!next.valid.load(Ordering::Acquire));
        assert!(far.valid.load(Ordering::Acquire));

        released(93);
        assert!(!far.valid.load(Ordering::Acquire));
        assert!(track(94, 0x2000, 0x3000).valid.load(Ordering::Acquire));
    }

    #[test]
    fn changed_runs_are_whole_pieces_joined() {
        let old = vec![0u8; 300];
        let mut new = old.clone();
        assert!(changed_runs(&new, &old).is_empty());
        new[5] = 1;
        new[70] = 1;
        new[299] = 1;
        assert_eq!(changed_runs(&new, &old), vec![(0, 128), (256, 300)]);
    }

    /// A buffer the card never wrote (zeros) or one holding the mark of the
    /// read-back before it does not pass.
    #[test]
    fn marks_are_never_zero_nor_the_last_one() {
        for sent in [1u64, 2, 0x7fff_ffff, 0x8000_0000, u64::from(u32::MAX), 1 << 40] {
            assert_ne!(mark_value(sent), 0);
            assert_ne!(mark_value(sent), mark_value(sent - 1));
        }
    }
}
