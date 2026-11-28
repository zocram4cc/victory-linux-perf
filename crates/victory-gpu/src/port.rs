//! Renderer port #1: the title's 32-bit graphics heap as the card's own
//! memory (`VICTORY_PORT_CONSTANTS`, off by default).
//!
//! What the title does (game_scarlett_master.exe, the routine at image
//! +0x1e86440, reached at its start): one `XMemVirtualAlloc` of the five
//! sizes it is given together (kind 0x42000: a reservation of 64 KiB
//! pages, flags 7: graphics memory below 4 GiB, 185 MB at 0x34310000 in
//! every run so far), split into five placeholders (`VirtualFree` with
//! `MEM_PRESERVE_PLACEHOLDER`), each turned into graphics memory of its own
//! protection (`XMemVirtualAlloc`, kind 0x20406000, flags 0x27); then ONE
//! `XMemAllocatePhysicalPages` for all their pages and five
//! `XMemMapPhysicalPages`, in order, each from the next page of that one
//! array. `xmem` makes one section of all the pages, so the whole
//! reservation is one stretch of one section: views that follow one
//! another in memory and in the section alike. The title's 174 MB buffer at
//! 0x34e10000 (where its ring of constants lies, and what the skinning
//! passes write through one view of all of it) is in it; it is never
//! tile-mapped (`CopyPageMappings` maps the GPU heap's pages, not these).
//!
//! What the layer did with it: a host copy in the card's memory, the title's
//! writes found and sent up (17 MB a frame in ~600 pieces), what shaders
//! wrote fetched back and merged, and every constant buffer read out of it
//! (~6,000 fills a frame) either waited for flights or done on the card
//! from the copy.
//!
//! What is done here instead: the stretch is mapped once more, at an
//! address of ours (a view of the same section: the same physical pages),
//! and opened as a host heap there (`OpenExistingHeapFromAddress1`); title
//! buffers and windows of title memory in it are buffers placed in that
//! heap ([`crate::shadow::Shadow`]'s in-place form): the card reads and
//! writes the title's bytes where the title wrote them. Nothing is tracked,
//! sent up, fetched back or compared for it, and no compare or hash pass
//! of ours ever runs over it (they are for copies in the card's memory:
//! one such pass over memory shared with the PC removed the device on
//! another tree). No tile mapping is ever made into this heap (that removes
//! the device).
//!
//! Why our own view: a heap opened from an address holds the pages it was
//! opened on: the title could not unmap, decommit or protect them while it
//! stands. Opened on our view, the title's own views stay free to change.
//! When the title maps other pages at such an address, what is in place
//! there is let go (`crate::inplace::title_memory_freeing`) and the region
//! forgotten; the view of ours stays (shortcut: it is never unmapped, a
//! few hundred MB of address space at most).
//!
//! Under Wine (vkd3d-proton on RADV, #163) a heap opened on a view is
//! accepted but is memory of vkd3d-proton's own: the kernel takes only
//! anonymous memory as a user pointer of the card, a section's views have a
//! file behind them, and vkd3d-proton falls back without a word. The card
//! then read zeros and the world was black. So under Wine a region is
//! opened only if a heap on a view of a section of our own shows the card
//! its bytes ([`crate::inplace::section_views_seen`]); else a copy as
//! before, counted as refused by the host.
//!
//! What it assumes: the CPU steps of ours that read this memory at
//! submission (fills of constant buffers, the title's buffer copies done on
//! the CPU) read what the title's CPU wrote, not what a submission still in
//! the air writes there: in place nothing tells the flights' writes, so
//! nothing waits for them. The title's constants are its CPU's; how many
//! such steps there are is on the frame-rate line ([`log`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows_sys::Win32::System::Memory as mem;

use crate::inplace::{Placed, cover, stretch};

const K64: u64 = 64 << 10;

/// The ported memory lies below this: the title's graphics memory for
/// 32-bit addresses, which only that heap is.
const LOW: u64 = 1 << 32;

/// The most bytes of one region: its pages are locked into the PC's memory
/// while the heap stands.
const MOST: u64 = 512 << 20;

/// `VICTORY_PORT_CONSTANTS`: `1` turns renderer port #1 on (title buffers
/// and windows of title memory in the title's 32-bit graphics memory, view
/// memory that is one stretch of a section, used in place); unset or
/// anything else: off, everything as before.
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // Read once, here: the switch of renderer port #1 ([`constants_switch`]).
    *ON.get_or_init(constants_switch_here)
}

/// Whether renderer port #1 is on, from `VICTORY_PORT_CONSTANTS` (`setting`),
/// `VICTORY_PORT_CONSTANTS_WINE` (`wine_setting`) and whether we run under
/// Wine: `1` turns it on, but under Wine only with `VICTORY_PORT_CONSTANTS_WINE=1`
/// as well. Under Wine 11.22, RADV and vkd3d-proton (RX 7900 XT) the port
/// draws the HUD over a black world (same-binary control on
/// avatarmanz/VIctory-7900XTX#380): the GPU reads zeros from the opened
/// heap whose bytes a second CPU mapping sees. On Windows the world is drawn
/// with it (RX 9070 XT, alone and with port #7), and the same heap fixture
/// passes. So it stays off under Wine. Both halves read it here:
/// this crate's [`on`] and `victory_hle`'s xmem. `home` (`VICTORY_PORT_HOME=1`
/// with `VICTORY_WINE_MAP=1`) counts as `VICTORY_PORT_CONSTANTS_WINE=1`: that
/// path opens the heap on the title's own private pages, which is what the
/// black world lacked.
pub fn constants_switch(setting: Option<&str>, wine_setting: Option<&str>, wine: bool, home: bool) -> bool {
    let one = |v: Option<&str>| v.is_some_and(|v| v.trim() == "1");
    one(setting) && (!wine || home || one(wine_setting))
}

/// [`constants_switch`] from the environment and the platform.
pub fn constants_switch_here() -> bool {
    let setting = std::env::var("VICTORY_PORT_CONSTANTS").ok();
    let wine_setting = std::env::var("VICTORY_PORT_CONSTANTS_WINE").ok();
    let asked = |name: &str| std::env::var(name).is_ok_and(|v| v.trim() == "1");
    let home = asked("VICTORY_PORT_HOME") && asked("VICTORY_WINE_MAP");
    let wine = crate::wine::on_wine();
    let on = constants_switch(setting.as_deref(), wine_setting.as_deref(), wine, home);
    if wine && setting.as_deref().is_some_and(|v| v.trim() == "1") && !on {
        tracing::warn!(
            "VICTORY_PORT_CONSTANTS=1 is left off under Wine (a black world was reported there); \
             VICTORY_PORT_CONSTANTS_WINE=1 turns it on anyway"
        );
    }
    on
}

/// A stretch of title memory opened as a heap through a view of ours.
struct Region {
    start: u64,
    end: u64,
    heap: ID3D12Heap,
    /// Bytes from the heap's first to `start`'s (a heap opened by handle holds
    /// all of the home, [`home_heap`]).
    offset: u64,
}

// SAFETY: D3D12 heaps are free-threaded.
unsafe impl Send for Region {}

static REGIONS: Mutex<Vec<Region>> = Mutex::new(Vec::new());

/// The lowest start and highest end of every region (both 0: none), for a
/// quick look on the paths that run thousands of times a frame.
static BOUNDS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Since the start: regions opened, their bytes; title buffers and windows
/// placed; refused because the memory is not one stretch of a section,
/// not committed, refused by the host; regions forgotten because the title
/// mapped other pages there. Then, since the last line: fills read out of
/// ported memory on the CPU, their bytes, buffer copies on the CPU that
/// touch it, their bytes.
static STATS: [AtomicU64; 13] = [const { AtomicU64::new(0) }; 13];

/// Whether `[at, at + len)` may be ported: in the title's 32-bit graphics
/// memory.
pub(crate) fn wanted(at: u64, len: u64) -> bool {
    on() && at.checked_add(len).is_some_and(|end| end <= LOW && len > 0)
}

/// Title memory `[start, start + len)` in place, if [`wanted`] and the
/// memory is one stretch of a section: a buffer placed in the region's
/// heap. `None` leaves the caller to do what it did before.
pub(crate) fn place_for(start: u64, len: u64, window: bool) -> Option<Placed> {
    if !wanted(start, len) {
        return None;
    }
    let host = crate::host::host()?;
    match place_in(&host.device, start, len) {
        Ok(placed) => {
            crate::inplace::counted(window, len);
            let n = STATS[2].fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 32 || n.is_power_of_two() {
                tracing::info!(
                    start = format_args!("{start:#x}"),
                    len,
                    window,
                    "renderer port 1: title memory in place, read and written by the card where the title has it"
                );
            }
            Some(placed)
        }
        Err((at, why)) => {
            let n = STATS[at].fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 8 || n.is_power_of_two() {
                tracing::warn!(start = format_args!("{start:#x}"), len, window, why, "renderer port 1: title memory not in place, a copy as before");
            }
            None
        }
    }
}

/// [`place_for`]'s work: the region holding the pages of `[start, start +
/// len)`, opened if none does, and a buffer placed in it. `Err` with the
/// statistic to count and why.
pub(crate) fn place_in(device: &ID3D12Device, start: u64, len: u64) -> Result<Placed, (usize, String)> {
    let at = start & !(K64 - 1);
    let end = (start + len.max(1)).next_multiple_of(K64);
    let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    let i = match regions.iter().position(|r| r.start <= at && end <= r.end) {
        Some(i) => i,
        None => {
            let region = open_region(device, at, end)?;
            regions.push(region);
            let n = regions.len() - 1;
            let r = &regions[n];
            STATS[0].fetch_add(1, Ordering::Relaxed);
            STATS[1].fetch_add(r.end - r.start, Ordering::Relaxed);
            bounds(&regions);
            n
        }
    };
    let region = &regions[i];
    let buffer = crate::inplace::place(device, &region.heap, region.offset + at - region.start, end - at).map_err(|e| (6, format!("no buffer placed in the region: {e}")))?;
    // SAFETY: a live buffer.
    let gpu = unsafe { buffer.GetGPUVirtualAddress() } + (start - at);
    Ok(Placed { buffer, heap: region.heap.clone(), gpu, bias: start - at })
}

/// Whether we run under Wine (its ntdll exports `wine_get_version`), as
/// `xmem` asks it.
fn on_wine() -> bool {
    static WINE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WINE.get_or_init(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        // SAFETY: plain lookups in ntdll, always loaded.
        unsafe {
            let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            !ntdll.is_null() && GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()).is_some()
        }
    })
}

/// Opens the stretch around `[at, end)` (64 KiB aligned) as a heap through
/// a view of ours of the same pages.
fn open_region(device: &ID3D12Device, at: u64, end: u64) -> Result<Region, (usize, String)> {
    // Under Wine (vkd3d-proton on RADV) such a heap is accepted but holds
    // memory of the host's own, not the view's pages: the card would read
    // zeros (#163). Asked once on a section of our own; a copy as before
    // unless the card read the bytes there. (Where the runtime shows pages as
    // memory of their own, `VICTORY_PORT_HOME=1`, a heap is opened there and
    // not on a view: told below, once the stretch is known.)
    let homes = crate::inplace::homes_any();
    if on_wine() && !homes && !crate::inplace::section_views_seen(device) {
        let why = "the host does not show the card a view's pages (a heap of its own memory)";
        return Err((5, why.to_owned()));
    }
    let Some((first, bytes, section, offset)) = stretch(at as usize, (end - at) as usize) else {
        return Err((3, format!("not one stretch of a section: {:?}", cover(at as usize, (end - at) as usize))));
    };
    // The whole stretch, kept to what may be ported: the next buffer in it
    // finds the region made.
    let (mut first, mut bytes, mut offset) = (first as u64, bytes as u64, offset);
    if first < LOW && first + bytes > LOW {
        bytes = LOW.saturating_sub(first);
    }
    if bytes > MOST {
        // Only what was asked for.
        offset += at - first;
        (first, bytes) = (at, end - at);
    }
    if bytes > MOST {
        return Err((3, format!("{bytes:#x} bytes, over the most of a region")));
    }
    if first > at || first + bytes < end || offset % K64 != 0 {
        return Err((3, format!("the stretch {first:#x}+{bytes:#x} does not hold it")));
    }
    if homes {
        // The heap is opened where the title has the pages, if the runtime
        // shows them as memory of their own (`crate::inplace::title_memory_home`,
        // `xmem`'s `home_on`); a stretch that is a view is refused where the
        // card would read zeros.
        if crate::inplace::at_home(first as usize, bytes as usize) {
            return open_at_home(device, first, bytes);
        }
        if !crate::inplace::section_views_seen(device) {
            return Err((5, "the host opens a heap on a view with memory of its own, and these pages are not shown as memory of their own (VICTORY_PORT_HOME)".to_owned()));
        }
    }
    // SAFETY: the section of a view the title has; the runtime keeps its
    // handle open while a view of it is there, and the range is inside it.
    let view = victory_core::mapping::changing(|| unsafe { mem::MapViewOfFile(section as _, mem::FILE_MAP_WRITE, (offset >> 32) as u32, offset as u32, bytes as usize) }).Value as usize;
    if view == 0 {
        return Err((5, format!("no view of the stretch: {}", std::io::Error::last_os_error())));
    }
    let unmap = || {
        // SAFETY: our own view, nothing opened on it.
        victory_core::mapping::changing(|| unsafe { mem::UnmapViewOfFile(mem::MEMORY_MAPPED_VIEW_ADDRESS { Value: view as *mut _ }) });
    };
    if !committed(view, bytes as usize) {
        unmap();
        return Err((4, "pages of the stretch are not committed".to_owned()));
    }
    match crate::inplace::open(device, crate::inplace::From::Part(view, bytes as usize)) {
        Ok(heap) => {
            tracing::info!(
                start = format_args!("{first:#x}"),
                bytes,
                section = format_args!("{section:#x}"),
                offset = format_args!("{offset:#x}"),
                view = format_args!("{view:#x}"),
                "renderer port 1: a stretch of the title's graphics memory opened as a host heap through a view of ours"
            );
            // shortcut: the view stays mapped for good (the heap holds its
            // pages while anything names it).
            Ok(Region { start: first, end: first + bytes, heap, offset: 0 })
        }
        Err(e) => {
            unmap();
            Err((5, format!("refused by the host: {e}")))
        }
    }
}

/// Under Wine, with `VICTORY_PORT_HOME=1`: opens `[first, first + bytes)`,
/// pages the runtime shows as memory of their own, as a heap right there
/// ([`open_region`]). No view of ours: while the heap stands these pages must
/// stay as they are (the runtime tells before it takes them away,
/// `crate::inplace::title_memory_home`, and the guards leave them alone).
///
/// The host says yes also where it gave the heap memory of its own, so the
/// card is asked what it reads there. It read zeros in 3 launches of 13
/// (runs 42, 44 and 49 of upstream's docs/linux-wine.md): the kernel takes
/// memory by address only as one mapping, and the pieces the runtime
/// committed one by one were several (`xmem`'s `whole_home`, which now makes
/// the stretch one mapping first). Should the card read zeros all the same,
/// the heap is let go and opened again, a few times, and then the stretch is
/// given up for good (`crate::inplace::home_given_up`): copied as before, its
/// pages guarded as views are.
fn open_at_home(device: &ID3D12Device, first: u64, bytes: u64) -> Result<Region, (usize, String)> {
    const TRIES: u32 = 4;
    if !committed(first as usize, bytes as usize) {
        return Err((4, "pages of the stretch are not committed".to_owned()));
    }
    for attempt in 1..=TRIES {
        let (heap, offset) = home_heap(device, first, bytes)?;
        let shows = crate::inplace::heap_shows(device, &heap, offset, first as usize, bytes as usize);
        // Diagnostics (`VICTORY_PORT_HOME_FAIL=1`, read here): as if the card
        // had read zeros every time, to see what a stretch given up costs.
        #[cfg(feature = "diagnostics")]
        let shows = if std::env::var_os("VICTORY_PORT_HOME_FAIL").is_some_and(|v| v == "1") { Some(false) } else { shows };
        if shows != Some(false) {
            tracing::info!(
                start = format_args!("{first:#x}"),
                bytes,
                card_reads_it = ?shows,
                attempt,
                "renderer port 1: a stretch of the title's graphics memory opened as a host heap where the title has it (pages shown as memory of their own, under Wine)"
            );
            return Ok(Region { start: first, end: first + bytes, heap, offset });
        }
        tracing::warn!(
            start = format_args!("{first:#x}"),
            bytes,
            attempt,
            "renderer port 1: the card reads zeros in the heap the host opened on the title's pages (the host gave it memory of its own: the pages are not one mapping of the process's own to the kernel)"
        );
        drop(heap);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    crate::inplace::home_given_up(first as usize, bytes as usize);
    Err((5, "the card reads zeros in the heap the host opened on the title's pages".to_owned()))
}

/// [`open_at_home`]'s heap on `[first, first + bytes)`, and the bytes from its
/// first to `first`: by the handle the runtime gave for those pages
/// (`VICTORY_HOME_BO=1`, `crate::inplace::title_memory_home_handle`; the heap is
/// then all of their memory); else, or if the host refuses the handle, on their
/// address as before.
fn home_heap(device: &ID3D12Device, first: u64, bytes: u64) -> Result<(ID3D12Heap, u64), (usize, String)> {
    if let Some((handle, offset)) = crate::inplace::home_handle(first as usize, bytes as usize) {
        match crate::inplace::open(device, crate::inplace::From::Shared(handle)) {
            Ok(heap) => return Ok((heap, offset)),
            Err(e) => {
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| {
                    tracing::warn!(error = %e, "renderer port 1 under Wine: the host opens no heap by the handle to the pages at home (VICTORY_HOME_BO=1): opened on their address as before");
                });
            }
        }
    }
    let heap = crate::inplace::open(device, crate::inplace::From::Part(first as usize, bytes as usize)).map_err(|e| (5, format!("refused by the host: {e}")))?;
    Ok((heap, 0))
}

/// Whether every page of `[at, at + len)` is committed.
fn committed(at: usize, len: usize) -> bool {
    let mut here = at;
    while here < at + len {
        // SAFETY: plain data filled in by the query.
        let mut info: mem::MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer of the right size.
        if unsafe { mem::VirtualQuery(here as *const _, &mut info, size_of_val(&info)) } == 0 || info.State != mem::MEM_COMMIT {
            return false;
        }
        here = (info.BaseAddress as usize).saturating_add(info.RegionSize);
    }
    true
}

fn bounds(regions: &[Region]) {
    let lo = regions.iter().map(|r| r.start).min().unwrap_or(0);
    let hi = regions.iter().map(|r| r.end).max().unwrap_or(0);
    BOUNDS[0].store(lo, Ordering::Relaxed);
    BOUNDS[1].store(hi, Ordering::Relaxed);
}

/// The title maps other pages at `[at, at + len)`: the regions over it are
/// forgotten (what is placed in them is let go by the caller).
pub(crate) fn forget(at: usize, len: usize) {
    let (at, end) = (at as u64, (at as u64).saturating_add(len as u64));
    if BOUNDS[1].load(Ordering::Relaxed) <= at || BOUNDS[0].load(Ordering::Relaxed) >= end {
        return;
    }
    let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    let before = regions.len();
    regions.retain(|r| r.end <= at || r.start >= end);
    STATS[7].fetch_add((before - regions.len()) as u64, Ordering::Relaxed);
    bounds(&regions);
}

/// Whether `[at, at + len)` touches ported memory (a quick look).
fn touches(at: usize, len: usize) -> bool {
    let hi = BOUNDS[1].load(Ordering::Relaxed);
    hi != 0 && (at as u64) < hi && (at as u64).saturating_add(len as u64) > BOUNDS[0].load(Ordering::Relaxed)
}

/// A fill of upload memory read `len` bytes at `src` on the CPU.
pub(crate) fn filled(src: usize, len: usize) {
    if touches(src, len) {
        STATS[9].fetch_add(1, Ordering::Relaxed);
        STATS[10].fetch_add(len as u64, Ordering::Relaxed);
    }
}

/// A title buffer copy was done on the CPU.
pub(crate) fn copied(dst: usize, src: usize, len: usize) {
    if touches(dst, len) || touches(src, len) {
        STATS[11].fetch_add(1, Ordering::Relaxed);
        STATS[12].fetch_add(len as u64, Ordering::Relaxed);
    }
}

/// For the frame-rate line: what is in place, and what of ours read or
/// wrote it on the CPU per frame.
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let s: Vec<u64> = STATS.iter().map(|s| s.load(Ordering::Relaxed)).collect();
    let per: Vec<f64> = STATS[9..].iter().map(|s| s.swap(0, Ordering::Relaxed) as f64 / frames).collect();
    tracing::info!(
        "renderer port 1 (VICTORY_PORT_CONSTANTS=1): {} regions of the title's graphics memory in place ({} MB), {} title buffers and windows placed in them \
         (no host copy, nothing sent up or fetched back for them); per frame on the CPU: {:.0} fills read out of them ({:.1} KB), {:.1} buffer copies \
         touching them ({:.1} KB); since the start refused: {} not one stretch of a section, {} not committed, {} by the host, {} no buffer; {} regions \
         forgotten (other pages mapped there)",
        s[0],
        s[1] >> 20,
        s[2],
        per[0],
        per[1] / 1024.0,
        per[2],
        per[3] / 1024.0,
        s[3],
        s[4],
        s[5],
        s[6],
        s[7],
    );
}

/// For `victory-hle`'s tests: what the card reads of title memory `[at, at +
/// len)` through the buffer [`place_in`] places there on the host device.
#[doc(hidden)]
pub fn card_reads(at: usize, len: usize) -> Result<Vec<u8>, String> {
    let host = crate::host::host().ok_or("no host device")?;
    let placed = place_in(&host.device, at as u64, len as u64).map_err(|(_, why)| why)?;
    let mut rig = crate::inplace::Rig::new(&host.device).map_err(|e| e.to_string())?;
    rig.fetch(&placed.buffer, placed.bias, len).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With `VICTORY_PORT_CONSTANTS=1` the title's constants heap (the 174 MB
    /// buffer at 0x34e10000 where its ring of constants lies) is used in
    /// place instead of as a host copy that is tracked, sent up and fetched
    /// back; memory above 4 GiB is not. The switch is read once per process,
    /// so this runs on its own: `cargo test -p victory-gpu --lib --
    /// --ignored the_constants_heap` with and without the variable.
    #[test]
    #[ignore = "needs VICTORY_PORT_CONSTANTS=1 in the environment"]
    fn the_constants_heap_is_used_in_place_with_the_switch() {
        assert!(wanted(0x34e1_0000, 0xae0_0000), "the title's constants heap is not used in place (a host copy is tracked)");
        assert!(!wanted(0xe_1c80_0000, 0x1000), "memory above 4 GiB was taken in place");
    }

    /// `VICTORY_PORT_CONSTANTS=1` turns the port on on Windows; under Wine it
    /// also needs `VICTORY_PORT_CONSTANTS_WINE=1` (a black world was reported
    /// there) or the home path (`VICTORY_PORT_HOME=1` with `VICTORY_WINE_MAP=1`);
    /// anything but `1` leaves it off everywhere.
    #[test]
    fn the_constants_port_stays_off_under_wine_unless_asked_for_there() {
        assert!(constants_switch(Some("1"), None, false, false), "off on Windows with the switch");
        assert!(!constants_switch(Some("1"), None, true, false), "on under Wine without VICTORY_PORT_CONSTANTS_WINE=1");
        assert!(constants_switch(Some(" 1 "), Some("1"), true, false), "off under Wine though both were asked for");
        assert!(!constants_switch(Some("1"), Some("0"), true, false));
        assert!(!constants_switch(None, Some("1"), true, false));
        assert!(!constants_switch(Some("0"), None, false, false));
        assert!(!constants_switch(None, None, false, false));
        assert!(constants_switch(Some("1"), None, true, true), "off under Wine with the home path asked for");
        assert!(!constants_switch(None, None, true, true), "the home path alone turned the port on");
    }

    /// Under Wine, on the card (nothing is tried elsewhere): a stretch whose
    /// pages are memory of their own is opened where it is, and the card
    /// reads and writes the bytes at that address; a stretch that is a view
    /// is opened through a view of ours only where the card then reads the
    /// view's bytes, and is refused where the host gives such a heap memory
    /// of its own (vkd3d-proton with RADV).
    #[test]
    fn under_wine_a_stretch_at_home_is_opened_where_it_is_and_a_view_only_if_shared() {
        use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
        if !on_wine() {
            return;
        }
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: plain device creation on the default adapter.
        let made = unsafe { D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device) };
        let Some(device) = device.filter(|_| made.is_ok()) else {
            eprintln!("not tried: no device");
            return;
        };
        let k64 = K64 as usize;
        let bytes = 8 * k64;
        let bytes_at = |at: usize, len: usize| -> &'static [u8] {
            // SAFETY: committed memory of the test's own, never freed before the last look.
            unsafe { std::slice::from_raw_parts(at as *const u8, len) }
        };
        // SAFETY: memory, a section and a view of the test's own.
        unsafe {
            // As the runtime shows pages at home: private memory, told as two
            // views of a section that follow one another.
            let home = mem::VirtualAlloc(std::ptr::null(), bytes, mem::MEM_RESERVE | mem::MEM_COMMIT, mem::PAGE_READWRITE) as usize;
            let invalid = windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            let section = mem::CreateFileMappingW(invalid, std::ptr::null(), mem::PAGE_READWRITE | mem::SEC_COMMIT, 0, bytes as u32, std::ptr::null()) as usize;
            assert!(home != 0 && section != 0);
            crate::inplace::title_memory_section(section, bytes as u64, true);
            crate::inplace::title_memory_view(home, bytes / 2, section, 0);
            crate::inplace::title_memory_view(home + bytes / 2, bytes / 2, section, (bytes / 2) as u64);
            crate::inplace::title_memory_home(home, bytes, true);
            for i in 0..bytes {
                ((home + i) as *mut u8).write((i as u8).wrapping_mul(7).wrapping_add(3));
            }
            let region = open_region(&device, home as u64 + K64, home as u64 + 2 * K64).unwrap_or_else(|e| panic!("not opened: {e:?}"));
            assert_eq!((region.start, region.end), (home as u64, (home + bytes) as u64), "the whole stretch, where the title has it");
            let buffer = crate::inplace::place(&device, &region.heap, K64, K64).unwrap();
            let mut rig = crate::inplace::Rig::new(&device).unwrap();
            assert!(rig.fetch(&buffer, 0, k64).unwrap() == bytes_at(home + k64, k64), "the card reads the bytes at the title's address");
            ((home + k64 + 5) as *mut u8).write(0x77);
            assert!(rig.fetch(&buffer, 0, k64).unwrap() == bytes_at(home + k64, k64), "and those written afterwards");
            rig.store(&buffer, 16, &[0xa5; 64]).unwrap();
            assert_eq!(bytes_at(home + k64 + 16, 64), [0xa5; 64], "the card's write is at the title's address");
            // A stretch that is a view of the section.
            let view = mem::MapViewOfFile(section as _, mem::FILE_MAP_WRITE, 0, 0, bytes).Value as usize;
            assert!(view != 0);
            crate::inplace::title_memory_view(view, bytes, section, 0);
            let shared = crate::inplace::section_views_seen(&device);
            eprintln!("a heap opened on a view is the view's memory to the card: {shared}");
            let opened = open_region(&device, view as u64, view as u64 + K64);
            if shared {
                assert!(opened.is_ok());
            } else {
                assert!(matches!(opened, Err((5, _))), "refused: the card would read zeros");
            }
            drop((opened, buffer, region));
            crate::inplace::title_memory_home(home, bytes, false);
            crate::inplace::views_gone(home, bytes);
            crate::inplace::views_gone(view, bytes);
            assert!(!crate::inplace::at_home(home, 1));
            mem::UnmapViewOfFile(mem::MEMORY_MAPPED_VIEW_ADDRESS { Value: view as *mut _ });
            crate::inplace::title_memory_section_gone(section);
            windows_sys::Win32::Foundation::CloseHandle(section as _);
        }
    }

    /// Under Wine, on the card (nothing is tried elsewhere): a stretch at home
    /// with a handle the host opens no heap by (here an event's; to a host
    /// without `VICTORY_HOME_BO=1`'s vkd3d-proton and Wine every handle is
    /// one) is opened where it is, as without a handle, and the card reads
    /// the bytes at the title's address.
    #[test]
    fn under_wine_a_stretch_at_home_whose_handle_opens_no_heap_is_opened_where_it_is() {
        use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
        if !on_wine() {
            return;
        }
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: plain device creation on the default adapter.
        let made = unsafe { D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device) };
        let Some(device) = device.filter(|_| made.is_ok()) else {
            eprintln!("not tried: no device");
            return;
        };
        let k64 = K64 as usize;
        let bytes = 4 * k64;
        // SAFETY: memory, a section and an event of the test's own.
        unsafe {
            let home = mem::VirtualAlloc(std::ptr::null(), bytes, mem::MEM_RESERVE | mem::MEM_COMMIT, mem::PAGE_READWRITE) as usize;
            let invalid = windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            let section = mem::CreateFileMappingW(invalid, std::ptr::null(), mem::PAGE_READWRITE | mem::SEC_COMMIT, 0, bytes as u32, std::ptr::null()) as usize;
            let event = windows::Win32::System::Threading::CreateEventW(None, false, false, None).unwrap().0 as usize;
            assert!(home != 0 && section != 0);
            crate::inplace::title_memory_section(section, bytes as u64, true);
            crate::inplace::title_memory_view(home, bytes, section, 0);
            crate::inplace::title_memory_home(home, bytes, true);
            crate::inplace::title_memory_home_handle(home, bytes, Some(event));
            for (i, b) in std::slice::from_raw_parts_mut(home as *mut u8, bytes).iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(13).wrapping_add(1);
            }
            let region = open_region(&device, home as u64 + K64, home as u64 + 2 * K64).unwrap_or_else(|e| panic!("not opened: {e:?}"));
            assert_eq!((region.start, region.end, region.offset), (home as u64, (home + bytes) as u64, 0), "the whole stretch, on its address");
            let buffer = crate::inplace::place(&device, &region.heap, K64, K64).unwrap();
            let got = crate::inplace::Rig::new(&device).unwrap().fetch(&buffer, 0, k64).unwrap();
            assert!(got == std::slice::from_raw_parts((home + k64) as *const u8, k64), "the card reads the bytes at the title's address");
            drop((buffer, region));
            crate::inplace::title_memory_home_handle(home, bytes, None);
            crate::inplace::title_memory_home(home, bytes, false);
            crate::inplace::views_gone(home, bytes);
            crate::inplace::title_memory_section_gone(section);
            windows_sys::Win32::Foundation::CloseHandle(section as _);
            windows_sys::Win32::Foundation::CloseHandle(event as _);
            mem::VirtualFree(home as _, 0, mem::MEM_RELEASE);
        }
    }

    /// Only the title's 32-bit memory is asked for.
    #[test]
    fn only_memory_below_4_gib_is_wanted_with_the_switch() {
        // The switch is read once from the environment; tests do not set
        // it, so nothing is wanted.
        if !on() {
            assert!(!wanted(0x3431_0000, 0x1000));
        }
        let low = |at: u64, len: u64| at.checked_add(len).is_some_and(|end| end <= LOW && len > 0);
        assert!(low(0x34e1_0000, 0xae0_0000));
        assert!(!low(0xffff_0000, 0x2_0000));
        assert!(!low(0xe_1c80_0000, 0x1000));
        assert!(!low(u64::MAX, 2));
    }

    /// A stretch of a reserved section, mapped as `xmem` maps physical
    /// pages (views of consecutive pages in turn, committed), is opened
    /// through a view of ours: the card reads the bytes the title's views
    /// show, at the buffer's offset, and its writes show in them; a second
    /// buffer in the stretch is placed in the same region.
    #[test]
    fn warp_a_stretch_of_a_reserved_section_is_used_in_place_through_a_view_of_ours() {
        let _turn = crate::indirect::tests::warp_turn();
        let Some(device) = crate::inplace::tests::warp() else { return };
        if windows::core::Interface::cast::<ID3D12Device13>(&device).is_err() {
            return;
        }
        const K: usize = 64 << 10;
        // SAFETY (the whole test): the section, placeholder and views made
        // here.
        unsafe {
            let process = windows_sys::Win32::System::Threading::GetCurrentProcess();
            let section = mem::CreateFileMappingW(
                windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
                std::ptr::null(),
                mem::PAGE_READWRITE | mem::SEC_RESERVE,
                0,
                8 * K as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            let hole = mem::VirtualAlloc2(process, std::ptr::null(), 8 * K, mem::MEM_RESERVE | mem::MEM_RESERVE_PLACEHOLDER, mem::PAGE_NOACCESS, std::ptr::null_mut(), 0)
                .cast::<u8>();
            assert!(!hole.is_null());
            // Two views, pages 0..3 and 3..8 of the section, in turn.
            mem::VirtualFree(hole.cast(), 3 * K, mem::MEM_RELEASE | mem::MEM_PRESERVE_PLACEHOLDER);
            let mut views = Vec::new();
            for (page, n) in [(0usize, 3usize), (3, 5)] {
                let view = mem::MapViewOfFile3(section, process, hole.add(page * K).cast(), (page * K) as u64, n * K, mem::MEM_REPLACE_PLACEHOLDER, mem::PAGE_READWRITE, std::ptr::null_mut(), 0);
                assert!(!view.Value.is_null(), "{}", std::io::Error::last_os_error());
                assert!(!mem::VirtualAlloc(view.Value, n * K, mem::MEM_COMMIT, mem::PAGE_READWRITE).is_null());
                crate::inplace::title_memory_view(view.Value as usize, n * K, section as usize, (page * K) as u64);
                views.push(view);
            }
            let base = hole as u64;
            let (start, len) = (base + K as u64 + 0x40, 5 * K as u64);
            let placed = place_in(&device, start, len).unwrap_or_else(|e| panic!("not placed: {e:?}"));
            assert_eq!(placed.bias, 0x40);
            let mut rig = crate::inplace::Rig::new(&device).unwrap();
            for (i, b) in std::slice::from_raw_parts_mut(start as *mut u8, len as usize).iter_mut().enumerate() {
                *b = (i * 7 + 3) as u8;
            }
            let got = rig.fetch(&placed.buffer, placed.bias, len as usize).unwrap();
            assert!(got == std::slice::from_raw_parts(start as *const u8, len as usize), "the card reads the title's bytes, across both views");
            rig.store(&placed.buffer, placed.bias + 3 * K as u64, &[0x5a; 256]).unwrap();
            assert!(std::slice::from_raw_parts((start + 3 * K as u64) as *const u8, 256).iter().all(|&b| b == 0x5a), "the card's write shows in the title's view");
            // Another buffer in the same stretch: the same region.
            let regions = REGIONS.lock().unwrap().len();
            let second = place_in(&device, base + 6 * K as u64, 100).unwrap_or_else(|e| panic!("not placed: {e:?}"));
            assert_eq!(REGIONS.lock().unwrap().len(), regions, "one region for the stretch");
            assert_eq!(second.bias, 0);
            // The title's views may change under the heap.
            drop((placed, second));
            forget(base as usize, 8 * K);
            for view in views {
                assert!(mem::UnmapViewOfFile2(process, view, mem::MEM_PRESERVE_PLACEHOLDER) != 0, "the title's own view can be unmapped");
            }
            crate::inplace::views_gone(base as usize, 8 * K);
            mem::VirtualFree(hole.cast(), 0, mem::MEM_RELEASE);
            mem::VirtualFree(hole.add(3 * K).cast(), 0, mem::MEM_RELEASE);
            windows_sys::Win32::Foundation::CloseHandle(section);
        }
    }
}
