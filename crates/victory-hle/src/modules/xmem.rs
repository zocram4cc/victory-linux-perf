//! `xmem.dll`: console memory, served from host memory. The GPU side works
//! from CPU addresses (unified memory), so memory types and bandwidth
//! classes change nothing here; what constrains addresses (alignment,
//! 2 MiB pages, address-bit limits) is honoured.
#![allow(non_snake_case)]
// Title code calls these through its import table, passing pointers the real
// API would trust.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::alloc::{Layout, alloc, dealloc};
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_COMMITMENT_LIMIT, GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MEM_ADDRESS_REQUIREMENTS, MEM_COMMIT, MEM_EXTENDED_PARAMETER,
    MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEM_REPLACE_PLACEHOLDER, MEM_RESERVE,
    MEM_RESERVE_PLACEHOLDER, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3,
    MemExtendedParameterAddressRequirements, PAGE_NOACCESS, PAGE_READWRITE, SEC_COMMIT,
    SEC_RESERVE, UnmapViewOfFile2, VirtualAlloc, VirtualAlloc2, VirtualFree, VirtualQueryEx,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetPerformanceInfo, K32GetProcessMemoryInfo, PERFORMANCE_INFORMATION,
    PROCESS_MEMORY_COUNTERS,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::BOOL;

use crate::{HleModule, hle_export};

const MIB: usize = 1 << 20;

/// Whether we run under Wine (its ntdll exports `wine_get_version`).
pub(crate) fn on_wine() -> bool {
    static WINE: LazyLock<bool> = LazyLock::new(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        // SAFETY: plain lookups in ntdll, always loaded.
        unsafe {
            let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            !ntdll.is_null() && GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()).is_some()
        }
    });
    *WINE
}

/// Standard memory the title has handed to the system.
static TRANSFERRED: AtomicUsize = AtomicUsize::new(0);

// Console additions to the host's flags.
/// 2 MiB pages; the host reads this bit as large pages, which need a
/// privilege.
const PAGES_2M: u32 = 0x2000_0000;
/// With `PAGES_2M`: 64 KiB pages; the host reads it as AWE memory.
const PAGES_64K_BIT: u32 = 0x40_0000;
const PAGES_64K: u32 = PAGES_2M | PAGES_64K_BIT;
/// The host's protection constants and `PAGE_GUARD`. Above them the console
/// adds GPU access bits (0x800-0x40000). Write-combining and uncached are
/// dropped too: our GPU side reads this memory through the CPU.
const HOST_PROTECTION: u32 = 0x1ff;
/// XMem flags of memory both processors reach: all of ours.
const GRAPHICS: u64 = 3;
/// XMem flag bit of 32-bit graphics memory.
const ADDRESS_32BIT: u64 = 4;
/// The host's write watching (not in windows-sys' memory module).
const MEM_WRITE_WATCH: u32 = 0x20_0000;

/// The addresses XMem `flags` allow, first and last: the low 4 GiB for
/// 32-bit memory; else from 4 GiB (kept for 32-bit requests, which the
/// title's big pools would otherwise crowd out) up to 46 down to 40 bits
/// from the field at bit 12, or 40 bits without one: the console keeps
/// title memory there, and its ray-tracing structures must be (
/// the title's BVH builder refuses one above; the title asks for its GPU heap
/// with no field). Decision 0001.
fn address_range(flags: u64) -> (usize, usize) {
    let bits = match (flags & ADDRESS_32BIT, flags >> 12 & 7) {
        (0, 0) => 40,
        (0, field) => 47 - field,
        _ => return (0, u32::MAX as usize),
    };
    (1 << 32, (1 << bits) - 1)
}

pub extern "system" fn XMemVirtualAlloc(
    base: *mut c_void,
    size: usize,
    kind: u32,
    flags: u64,
    protect: u32,
) -> *mut c_void {
    let (lowest, highest) = address_range(flags);
    let mut requirements = MEM_ADDRESS_REQUIREMENTS {
        LowestStartingAddress: lowest as *mut c_void,
        HighestEndingAddress: highest as *mut c_void,
        Alignment: if kind & (PAGES_2M | PAGES_64K_BIT) == PAGES_2M {
            2 * MIB
        } else {
            0
        },
    };
    let asked = protect;
    let mut protect = match protect & HOST_PROTECTION {
        0 => PAGE_NOACCESS,
        p => p,
    };
    // A reservation of 64 KiB pages takes physical pages later
    // (`XMemMapPhysicalPages`); on the host only a placeholder can.
    // shortcut: committing part of such a reservation later would fail
    // (the title's 23 calls never do); replace that part of the
    // placeholder then.
    let placeholder = kind & PAGES_64K == PAGES_64K && kind & MEM_COMMIT == 0;
    if placeholder {
        if kind & MEM_REPLACE_PLACEHOLDER != 0 {
            // Part of a placeholder the title split: it stays one.
            return base;
        }
        protect = PAGE_NOACCESS;
    }
    let mut kind = kind & !(PAGES_2M | PAGES_64K_BIT);
    if placeholder {
        kind |= MEM_RESERVE | MEM_RESERVE_PLACEHOLDER;
    }
    let mut parameter = MEM_EXTENDED_PARAMETER::default();
    parameter.Anonymous1._bitfield = MemExtendedParameterAddressRequirements as u64;
    parameter.Anonymous2.Pointer = (&raw mut requirements).cast();
    // Address requirements apply to reservations at an address of our choice.
    let constrained = base.is_null();
    if constrained {
        kind |= MEM_RESERVE;
        // The GPU side copies only the pages the title wrote since its last
        // copy (`victory_gpu`, decision 0007); placeholders cannot be
        // watched (their physical-page views are copied whole).
        if kind & MEM_RESERVE_PLACEHOLDER == 0 {
            kind |= MEM_WRITE_WATCH;
        }
    }
    // SAFETY: the title's own arguments, cleaned of console bits; the
    // parameter points at `requirements`, which outlives the call.
    let at = unsafe {
        VirtualAlloc2(
            GetCurrentProcess(),
            base,
            size,
            kind,
            protect,
            if constrained {
                &mut parameter
            } else {
                std::ptr::null_mut()
            },
            constrained.into(),
        )
    };
    if at.is_null() {
        let error = std::io::Error::last_os_error();
        if kind & MEM_COMMIT != 0 {
            commit_refused(size, "virtual memory", error);
        } else {
            tracing::warn!(
                size,
                kind = format_args!("{kind:#x}"),
                flags = format_args!("{flags:#x}"),
                %error,
                "XMemVirtualAlloc refused"
            );
        }
    }
    if base.is_null() {
        tracing::debug!(
            size,
            kind = format_args!("{kind:#x}"),
            flags = format_args!("{flags:#x}"),
            at = format_args!("{at:p}"),
            "XMemVirtualAlloc"
        );
    }
    if !at.is_null() && !placeholder {
        victory_gpu::title_memory_allocated(at as usize, size);
    }
    // A reservation only views of physical pages come into: the GPU side
    // knows its holes by this word where the host cannot say (Wine).
    if !at.is_null() && placeholder {
        victory_gpu::title_memory_placeholder(at as usize, size);
    }
    note_protection(asked, kind, flags, at as usize, size);
    at
}

/// What the title asks its memory to be (the console's protection, with the
/// bits that say how each processor reaches it: 0x200 uncached, 0x400
/// write-combined, 0x800 and on the GPU's): said for the 1st, 2nd, 4th.. time of
/// each, with how much so far. (For placing it: memory the CPU reaches
/// uncached or write-combined is memory it does not read.)
fn note_protection(protect: u32, kind: u32, flags: u64, at: usize, size: usize) {
    type Seen = std::collections::HashMap<(u32, u32, u64), (u64, usize)>;
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    let (n, bytes) = {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let e = seen.get_or_insert_with(Default::default).entry((protect, kind, flags)).or_default();
        *e = (e.0 + 1, e.1 + size);
        *e
    };
    if n.is_power_of_two() {
        tracing::info!(
            "title memory asked for as protection {protect:#x}, kind {kind:#x}, flags {flags:#x}: {n} times, {} MB, last {at:#x}+{size:#x}",
            bytes >> 20
        );
    }
}

/// `MEMORY_BASIC_INFORMATION` (the console's first 0x30 bytes are the
/// same), then the XMem flags and tag.
#[repr(C)]
pub struct BasicInformation {
    host: MEMORY_BASIC_INFORMATION,
    flags: u64,
    tag_code: u32,
    tag: [u8; 4],
}

// shortcut: every committed page reports graphics memory and no tag, since
// all of it is GPU-reachable here (the title's BVH builder, the
// only caller, wants graphics memory); remember the flags of
// `XMemVirtualAlloc` regions if a title tells CPU-only memory apart.
pub extern "system" fn XMemVirtualQuery(
    process: HANDLE,
    address: *const c_void,
    out: *mut BasicInformation,
) -> BOOL {
    let size = size_of::<MEMORY_BASIC_INFORMATION>();
    // SAFETY: the title passes room for the whole structure; the host
    // fails on a null one.
    unsafe {
        if VirtualQueryEx(process, address, out.cast(), size) == 0 {
            return 0;
        }
        let info = &mut *out;
        info.flags = if info.host.State == MEM_COMMIT {
            GRAPHICS
        } else {
            0
        };
        info.tag_code = 0;
        info.tag = [0; 4];
    }
    1
}

// `XMemAlloc` attributes: alignment as log2 (5 bits at 24, 4 bytes and up
// from 2), then the memory type, then bit 33: a 32-bit address.
const ALIGNMENT_SHIFT: u64 = 24;
const ALLOC_32BIT: u64 = 1 << 33;
/// Our block header: the block's size and alignment, just below the
/// pointer handed out.
const HEADER: usize = 2 * size_of::<usize>();

fn layout(size: usize, attributes: u64) -> Option<Layout> {
    // An alignment of at least 16 leaves room for the header.
    let align = 1usize << (attributes >> ALIGNMENT_SHIFT & 0x1f).max(4);
    Layout::from_size_align(size.checked_add(align)?, align).ok()
}

type AllocRoutine = extern "system" fn(usize, u64) -> *mut c_void;
type FreeRoutine = extern "system" fn(*mut c_void, u64);

/// The allocation and free routines the title's exe exports as
/// `__XMemAllocationHooks__` (the title does): `XMemAlloc` and
/// `XMemFree` call them, for the title's own calls and its middleware's;
/// the `…Default` exports do not.
fn hooks() -> [usize; 2] {
    static AT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let at = match AT.get() {
        Some(&at) => at,
        // Looked up once the title is mapped, which is before it runs.
        None => match crate::runtime::main_image() {
            Some(base) => *AT.get_or_init(|| {
                use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
                // SAFETY: a mapped image and a NUL-terminated name.
                let at = unsafe {
                    GetProcAddress(base as _, c"__XMemAllocationHooks__".as_ptr().cast())
                }
                .map_or(0, |f| f as usize);
                tracing::info!(found = at != 0, "xmem allocation hooks");
                at
            }),
            None => 0,
        },
    };
    if at == 0 {
        return [0; 2];
    }
    // SAFETY: the export is two routine pointers.
    unsafe { (at as *const [usize; 2]).read() }
}

/// The title's general allocator, for middleware too: its hook if it has
/// one, else [`XMemAllocDefault`].
pub extern "system" fn XMemAlloc(size: usize, attributes: u64) -> *mut c_void {
    match hooks()[0] {
        0 => XMemAllocDefault(size, attributes),
        hook => {
            // SAFETY: the title's own routine of this type.
            let alloc: AllocRoutine = unsafe { std::mem::transmute(hook) };
            alloc(size, attributes)
        }
    }
}

/// See [`XMemAlloc`].
pub extern "system" fn XMemFree(address: *mut c_void, attributes: u64) {
    match hooks()[1] {
        0 => XMemFreeDefault(address, attributes),
        hook => {
            // SAFETY: the title's own routine of this type.
            let free: FreeRoutine = unsafe { std::mem::transmute(hook) };
            free(address, attributes)
        }
    }
}

/// The system allocator behind [`XMemAlloc`].
pub extern "system" fn XMemAllocDefault(size: usize, attributes: u64) -> *mut c_void {
    if attributes & ALLOC_32BIT != 0 {
        return XMemVirtualAlloc(
            std::ptr::null_mut(),
            size,
            MEM_RESERVE | MEM_COMMIT,
            GRAPHICS | ADDRESS_32BIT,
            PAGE_READWRITE,
        );
    }
    let Some(layout) = layout(size, attributes) else {
        return std::ptr::null_mut();
    };
    // shortcut: the host heap's addresses lie above 2^40, unlike the
    // console's; nothing seen builds ray-tracing structures in this memory
    // (the title uses its GPU heap). Sub-allocate from a
    // reservation below 2^40 if one does.
    // SAFETY: the layout has a non-zero size.
    let block = unsafe { alloc(layout) };
    if block.is_null() {
        tracing::warn!(size, "XMemAllocDefault: the host heap is out of memory");
        return block.cast();
    }
    // SAFETY: the block holds the alignment (at least the header) and then
    // `size` bytes.
    unsafe {
        let user = block.add(layout.align());
        user.sub(HEADER)
            .cast::<[usize; 2]>()
            .write([layout.size(), layout.align()]);
        user.cast()
    }
}

/// Frees what [`XMemAllocDefault`] returned, given the same attributes.
pub extern "system" fn XMemFreeDefault(address: *mut c_void, attributes: u64) {
    if address.is_null() {
        return;
    }
    // SAFETY: the address came from `XMemAlloc` with these attributes: a
    // whole reservation, or a header-prefixed block.
    unsafe {
        if attributes & ALLOC_32BIT != 0 {
            VirtualFree(address, 0, MEM_RELEASE);
            return;
        }
        let user = address.cast::<u8>();
        let [size, align] = user.sub(HEADER).cast::<[usize; 2]>().read();
        dealloc(
            user.sub(align),
            Layout::from_size_align_unchecked(size, align),
        );
    }
}

/// A retail console has no tools memory.
pub extern "system" fn XMemIsToolsMemAvailable() -> BOOL {
    0
}

// Physical pages: on the console, 64 KiB frames a title maps into its own
// reservations, at more than one address if it likes. Here each allocation
// is a pagefile-backed section, its pages are 64 KiB slices of it, and a
// mapping is a view of consecutive slices in place of a host placeholder.

const PAGE: usize = 64 << 10;

struct Section {
    handle: HANDLE,
    pages: usize,
    /// A view of the whole section, kept ([`Section::window`]); 0 if the
    /// host gave none.
    window: std::sync::OnceLock<usize>,
    /// A bit per page: committed, as far as [`Section::window`] asked.
    committed: Box<[std::sync::atomic::AtomicU64]>,
}

impl Section {
    /// Where bytes of `n` pages from page `index` on can be written,
    /// whether or not the pages are mapped anywhere, committed.
    ///
    /// Every DirectStorage write mapped a view of its pages, committed it,
    /// faulted each of its 4 KiB pages in and unmapped it again, all of it
    /// under the process's one address-space lock, with 24 workers, the
    /// title's own mapping calls (about 100,000 views of one page) and our
    /// guards asking for the same lock. One view of the section, kept,
    /// costs that once per page.
    /// `VICTORY_SECTION_WINDOW=0`: a view for each write, as before.
    fn window(&self, index: usize, n: usize) -> Option<usize> {
        use std::sync::atomic::Ordering;
        use windows_sys::Win32::System::Memory::{FILE_MAP_WRITE, MapViewOfFile};
        static ON: LazyLock<bool> =
            LazyLock::new(|| std::env::var_os("VICTORY_SECTION_WINDOW").is_none_or(|v| v != "0"));
        if !*ON || index + n > self.pages {
            return None;
        }
        let base = *self.window.get_or_init(|| {
            // SAFETY: the whole of a section we own.
            unsafe { MapViewOfFile(self.handle, FILE_MAP_WRITE, 0, 0, self.pages * PAGE) }.Value as usize
        });
        if base == 0 {
            return None;
        }
        let missing = |page: usize| self.committed[page / 64].load(Ordering::Acquire) >> (page % 64) & 1 == 0;
        let mut page = index;
        while page < index + n {
            if !missing(page) {
                page += 1;
                continue;
            }
            let run = (page..index + n).take_while(|&p| missing(p)).count();
            if run == 0 {
                continue; // another worker committed it meanwhile
            }
            if !commit(base + page * PAGE, run * PAGE) {
                return None;
            }
            for p in page..page + run {
                self.committed[p / 64].fetch_or(1 << (p % 64), Ordering::AcqRel);
            }
            page += run;
        }
        count(4, 1, 0);
        Some(base + index * PAGE)
    }
}

impl Section {
    /// Commits the pages from `index` on that a view at `base` shows, unless
    /// all of them are ([`lean`]): committed for one view is for all.
    fn commit_shown(&self, index: usize, base: usize, len: usize) -> bool {
        use std::sync::atomic::Ordering;
        let pages = index..(index + len / PAGE).min(self.pages);
        let bit = |p: usize| (&self.committed[p / 64], 1 << (p % 64));
        let done = || {
            pages.clone().all(|p| {
                let (word, bit) = bit(p);
                word.load(Ordering::Acquire) & bit != 0
            })
        };
        if lean() && done() {
            return true;
        }
        if !counted(Call::Commit, || commit(base, len)) {
            return false;
        }
        for p in pages {
            let (word, bit) = bit(p);
            word.fetch_or(bit, Ordering::AcqRel);
        }
        true
    }
}

// SAFETY: a section handle may be used from any thread.
unsafe impl Send for Section {}
// SAFETY: as above.
unsafe impl Sync for Section {}

impl Drop for Section {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::UnmapViewOfFile;
        if let Some(&base) = self.window.get()
            && base != 0
        {
            // SAFETY: our view; whoever writes through it holds the section.
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: base as *mut c_void,
                })
            };
        }
        victory_gpu::title_memory_section_gone(self.handle as usize);
        // SAFETY: our handle; views keep the section alive on their own.
        unsafe { CloseHandle(self.handle) };
    }
}

/// `VirtualFree`'s flag that joins adjacent placeholders into one (the
/// bindings do not name it).
const MEM_COALESCE_PLACEHOLDERS: u32 = 0x1;

/// Mapping calls since the start, for [`count`]'s line: views the title
/// mapped (`XMemMapPhysicalPages`), pages the GPU's page-mapping copies
/// asked for ([`map_alias`]) and the views they were mapped as, views of
/// our own for a write ([`View::map`]) and writes served from a section's
/// whole view instead ([`Section::window`]), views unmapped, views mapped
/// again around an unmapped part, views mapped as one with a neighbour
/// ([`join_views`]), and the entries of `XMemMapPhysicalPagesScatter` calls
/// and the runs they were mapped or unmapped as ([`scatter_runs`]).
static MAPPED: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];

/// What the mapping calls asked of the host and what they cost, since the
/// start, for [`count`]'s line. By kind of host call, how many and their
/// nanoseconds: views unmapped, placeholders split, placeholders joined,
/// views mapped, pages committed. Under Wine a view mapped is two requests
/// to wineserver (the section's description, then the view) and a view
/// unmapped one, each a walk of the process's views there; splitting and
/// joining placeholders ask the server nothing (they take the process's
/// address-space lock, as does every protection change), and nothing is
/// committed (sections are committed whole). Then: views the host refused
/// (each a request for nothing); calls that changed nothing because every
/// page showed what it was to show already ([`map_same`]) and the pages
/// left as they were by that; pages of the GPU's page-mapping copies that
/// went into one view with the page before them without a join
/// ([`alias_batch`]); and the nanoseconds of the title's calls in all, of
/// their waiting for the views' lock, of their waiting for readers in place
/// to let go (`victory_gpu::title_memory_remapping`), and inside what tells
/// the GPU side ([`told`]). The rest of a call's time is our own tables.
/// Last, the times a thread's priority was asked ([`lift_thread`]: a
/// request each under Wine), and the views unmapped that the GPU side was
/// told of together with the view before them ([`unmap_runs`]).
static COST: [AtomicU64; 20] = [const { AtomicU64::new(0) }; 20];

/// The slots of [`COST`].
mod cost {
    pub(super) const UNMAP: usize = 0;
    pub(super) const SPLIT: usize = 2;
    pub(super) const JOIN: usize = 4;
    pub(super) const MAP: usize = 6;
    pub(super) const COMMIT: usize = 8;
    pub(super) const REFUSED: usize = 10;
    pub(super) const SAME_CALLS: usize = 11;
    pub(super) const SAME_PAGES: usize = 12;
    pub(super) const MERGED: usize = 13;
    pub(super) const CALLS_NS: usize = 14;
    pub(super) const LOCK_NS: usize = 15;
    pub(super) const HOLD_NS: usize = 16;
    pub(super) const TOLD_NS: usize = 17;
    pub(super) const LIFT_ASKS: usize = 18;
    pub(super) const TOLD_ONCE: usize = 19;
}

/// Adds `n` to slot `slot` of [`COST`].
fn cost(slot: usize, n: u64) {
    COST[slot].fetch_add(n, Ordering::Relaxed);
}

/// Counts `n` mapping calls of kind `slot` ([`MAPPED`]); every 16,384th
/// count of all kinds is a log line with the views alive (`views`, where
/// the caller knows). Under Wine each view mapped is two wineserver
/// requests and each view unmapped one that walks the list of the
/// process's views, and the server is one thread: with the world streaming
/// in it was the frame's whole time (82 % of a core for 4,000 requests a
/// second, the title's threads asleep behind it).
fn count(slot: usize, n: u64, views: usize) {
    MAPPED[slot].fetch_add(n, Ordering::Relaxed);
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static ALIVE: AtomicUsize = AtomicUsize::new(0);
    if views != 0 {
        ALIVE.store(views, Ordering::Relaxed);
    }
    // Also by the clock (`VICTORY_MAP_LINE_SECONDS`, 5; 0: by the count alone,
    // as before): a line for every block of the frame lines while the title
    // maps anything, so that the two can be read side by side. By the count
    // alone a quiet minute had no line and a loading screen eight a second.
    let by_count = CALLS.fetch_add(1, Ordering::Relaxed) % 16384 == 16383;
    let now = line_clock_ms();
    let said = SAID_MS.load(Ordering::Relaxed);
    let every = line_every_ms();
    let by_clock = every != 0
        && now >= said.saturating_add(every)
        && SAID_MS.compare_exchange(said, now, Ordering::Relaxed, Ordering::Relaxed).is_ok();
    if by_count || by_clock {
        if by_count {
            SAID_MS.store(now, Ordering::Relaxed);
        }
        let m: Vec<u64> = MAPPED.iter().map(|c| c.load(Ordering::Relaxed)).collect();
        let c: Vec<u64> = COST.iter().map(|c| c.load(Ordering::Relaxed)).collect();
        since_the_last_line(now, &c, ALIVE.load(Ordering::Relaxed));
        #[cfg(feature = "diagnostics")]
        CENSUS_DUE.store(true, Ordering::Relaxed);
        let ms = |ns: u64| ns / 1_000_000;
        let host = c[cost::UNMAP + 1] + c[cost::SPLIT + 1] + c[cost::JOIN + 1] + c[cost::MAP + 1] + c[cost::COMMIT + 1];
        let waited = c[cost::LOCK_NS] + c[cost::HOLD_NS];
        tracing::info!(
            title_views = m[0],
            alias_pages = m[1],
            alias_views = m[2],
            own_views = m[3],
            own_from_whole = m[4],
            unmapped = m[5],
            mapped_again = m[6],
            joined = m[7],
            scatter_entries = m[8],
            scatter_runs = m[9],
            views_alive = ALIVE.load(Ordering::Relaxed),
            faults_retried = victory_gpu::remap_retries(),
            host_maps = c[cost::MAP],
            host_map_ms = ms(c[cost::MAP + 1]),
            host_unmaps = c[cost::UNMAP],
            host_unmap_ms = ms(c[cost::UNMAP + 1]),
            host_splits = c[cost::SPLIT],
            host_joins = c[cost::JOIN],
            host_split_join_ms = ms(c[cost::SPLIT + 1] + c[cost::JOIN + 1]),
            host_commits = c[cost::COMMIT],
            maps_refused = c[cost::REFUSED],
            priority_asks = c[cost::LIFT_ASKS],
            unmaps_told_with_another = c[cost::TOLD_ONCE],
            same_calls = c[cost::SAME_CALLS],
            same_pages = c[cost::SAME_PAGES],
            alias_merged = c[cost::MERGED],
            calls_ms = ms(c[cost::CALLS_NS]),
            lock_wait_ms = ms(c[cost::LOCK_NS]),
            hold_wait_ms = ms(c[cost::HOLD_NS]),
            told_gpu_ms = ms(c[cost::TOLD_NS]),
            own_ms = ms(c[cost::CALLS_NS].saturating_sub(host + waited + c[cost::TOLD_NS])),
            "title memory mapping calls so far"
        );
    }
}

/// Milliseconds since the first mapping call was counted.
fn line_clock_ms() -> u64 {
    static BEGAN: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
    BEGAN.elapsed().as_millis() as u64
}

/// [`line_clock_ms`] of [`count`]'s last line.
static SAID_MS: AtomicU64 = AtomicU64::new(0);

/// The milliseconds after which [`count`] writes its line whatever the count
/// (`VICTORY_MAP_LINE_SECONDS`, 5 unless set; 0: never by the clock).
fn line_every_ms() -> u64 {
    static MS: LazyLock<u64> = LazyLock::new(|| {
        let seconds = std::env::var("VICTORY_MAP_LINE_SECONDS").ok().and_then(|v| v.trim().parse::<u64>().ok());
        seconds.unwrap_or(5).saturating_mul(1000)
    });
    *MS
}

/// With [`count`]'s line, what the mapping calls cost since the line before:
/// the host's calls a second and the microseconds of one, and the share of
/// the clock that went into the title's mapping calls (of one thread's time:
/// the title streams through one). `cost`: [`COST`] now.
///
/// What a host call costs under Wine goes with the views alive (wineserver
/// walks its list of them for every view mapped and unmapped: 13 us with
/// 1,000 alive, 130 to 280 with 18,000 to 27,000 in the opening scene), and
/// while such a call runs the process's memory lock is held: every asking
/// of the write watch, every change of protection and every write fault of
/// the title's waits for it. In a 56-minute play (play-prologue) a frame
/// took 73 ms at the median while nothing streamed and 284 ms while it did,
/// with the same draws, and one asking of the write watch 74 us against
/// 517. The totals say none of that for a stretch of the run; this line
/// does.
fn since_the_last_line(now_ms: u64, cost: &[u64], views_alive: usize) {
    // The clock and the costs at the line before.
    static BEFORE: Mutex<Option<(u64, Vec<u64>)>> = Mutex::new(None);
    let mut before = BEFORE.lock().unwrap_or_else(|e| e.into_inner());
    let Some((then, was)) = before.replace((now_ms, cost.to_vec())) else {
        return;
    };
    drop(before);
    let ms = now_ms.saturating_sub(then).max(1);
    let since = |slot: usize| cost[slot].saturating_sub(was[slot]);
    let each = |slot: usize| since(slot + 1) / 1000 / since(slot).max(1);
    let a_second = |n: u64| n * 1000 / ms;
    let share = |ns: u64| ns / 10_000 / ms;
    let host = since(cost::UNMAP + 1) + since(cost::SPLIT + 1) + since(cost::JOIN + 1) + since(cost::MAP + 1) + since(cost::COMMIT + 1);
    tracing::info!(
        "title memory mapping since the line before ({:.1} s): the host mapped {} views a second ({} us each) and \
         unmapped {} ({} us each), with {} views alive; the title's mapping calls took {} % of the time ({} % in \
         the host's calls, {} % telling the GPU side, {} % waiting for our locks; of one thread's time)",
        ms as f64 / 1000.0,
        a_second(since(cost::MAP)),
        each(cost::MAP),
        a_second(since(cost::UNMAP)),
        each(cost::UNMAP),
        views_alive,
        share(since(cost::CALLS_NS)),
        share(host),
        share(since(cost::TOLD_NS)),
        share(since(cost::LOCK_NS) + since(cost::HOLD_NS)),
    );
}

/// [`count`] wrote its line: the next call that holds the views counts them
/// ([`census`]).
#[cfg(feature = "diagnostics")]
static CENSUS_DUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Diagnostics, with [`count`]'s line (`VICTORY_VIEW_CENSUS=0`: not): what
/// the views alive are, since what a mapping call costs under Wine goes with
/// their number. By their pages; the views that show the section's slices
/// right after the view before them at the address right after it, and so
/// could be one view with it (inside one [`JOIN_WINDOW`], where
/// [`join_views`] joins, and across two); and the views of one page whose
/// page is shown at more than eight addresses (what the title shows where it
/// has nothing: as many views as addresses, never one view) with the pages
/// they are of. Called with the views held; some milliseconds for 60,000.
#[cfg(feature = "diagnostics")]
fn census(physical: &Physical) {
    if !CENSUS_DUE.swap(false, Ordering::Relaxed) {
        return;
    }
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("VICTORY_VIEW_CENSUS").is_none_or(|v| v != "0"));
    if !*ON {
        return;
    }
    let began = std::time::Instant::now();
    // Views of 1, 2 to 4, 5 to 16 and more pages, and their bytes in all.
    let (mut sized, mut bytes) = ([0u64; 4], 0u64);
    // Views that could be one with the view before: in its window, not.
    let mut beside = [0u64; 2];
    // Views of one page shown at more than eight addresses; those pages.
    let mut many = 0u64;
    let mut many_pages = std::collections::HashSet::new();
    let shown = SHOWN.read().unwrap_or_else(|e| e.into_inner());
    let mut last: Option<(usize, usize, &Slice)> = None;
    for (&base, (len, slice)) in &physical.views {
        let pages = len / PAGE;
        sized[match pages {
            0 | 1 => 0,
            2..=4 => 1,
            5..=16 => 2,
            _ => 3,
        }] += 1;
        bytes += *len as u64;
        if let Some((at, size, before)) = last
            && at + size == base
            && Arc::ptr_eq(&before.section, &slice.section)
            && before.index + size / PAGE == slice.index
        {
            let window = |at: usize| at & !(JOIN_WINDOW - 1);
            beside[usize::from(window(at) != window(base + len - 1))] += 1;
        }
        last = Some((base, *len, slice));
        let key = (Arc::as_ptr(&slice.section) as usize, slice.index);
        if pages == 1 && shown.pages.get(&key).is_some_and(|at| at.len() > 8) {
            many += 1;
            many_pages.insert(key);
        }
    }
    drop(shown);
    tracing::info!(
        "title views alive: {} ({} MB); of 1 page {}, of 2 to 4 {}, of 5 to 16 {}, of more {}; {} show the slices \
         right after their neighbour's and lie in its window, {} lie in the next; {} are views of one page that \
         is shown at more than 8 addresses ({} such pages); counted in {:.1} ms (VICTORY_VIEW_CENSUS=0: not)",
        physical.views.len(),
        bytes >> 20,
        sized[0],
        sized[1],
        sized[2],
        sized[3],
        beside[0],
        beside[1],
        many,
        many_pages.len(),
        began.elapsed().as_secs_f64() * 1000.0,
    );
}

#[cfg(not(feature = "diagnostics"))]
fn census(_: &Physical) {}

#[cfg(test)]
thread_local! {
    /// What [`wine_switch`] answers for this thread's calls, whatever the
    /// host and the environment.
    static SWITCHES_IN_TEST: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether `VICTORY_<name>` switches something on: `1` and `0` say so, and
/// unset it is on under Wine only when `VICTORY_XMEM_RUNS=1` turns all of
/// them on at once: the views joined, the runs of scatter, alias and unmap
/// calls, the pages left that show the same already (zeex64 502e905's
/// mapping port). Off unless asked for: upstream measured it on a stock
/// wineserver only, not on ours with the view tree, its author had six
/// title crashes in some 28 runs on a view-tree wineserver of their own, and
/// a join leaves a moment in which a write of another title thread to the
/// neighbour's pages is not seen ([`victory_gpu::title_memory_regrouped`]).
fn wine_switch(name: &str) -> bool {
    match std::env::var(name).ok().as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => on_wine() && runs_asked(),
    }
}

/// `VICTORY_XMEM_RUNS=1` ([`wine_switch`]).
fn runs_asked() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("VICTORY_XMEM_RUNS").is_some_and(|v| v == "1"));
    *ON
}

/// A [`wine_switch`], asked once.
fn switched(on: &LazyLock<bool>) -> bool {
    #[cfg(test)]
    if let Some(forced) = SWITCHES_IN_TEST.get() {
        return forced;
    }
    **on
}

/// Whether a view mapped beside one that shows the section's slices just
/// before or after its own is made one view with it (`VICTORY_JOIN_VIEWS`,
/// on under Wine with XMEM_RUNS=1), within windows of [`JOIN_WINDOW`].
///
/// The title maps its memory a page at a time: 122,880 of the 132,870
/// views it had mapped when a first world appeared were one page beside
/// such a neighbour, and 125,000 views were alive. wineserver keeps a
/// process's views in a list and walks all of it for every view mapped
/// (looking for an overlap) and up to all of it for every view unmapped,
/// so each of the 4,000 mapping requests a second while a world streams in
/// cost a walk of that list, on the server's one thread: the title's
/// threads slept behind it and a frame took 2-4 s, 0.3 s of it ours.
///
/// A join unmaps the neighbour and maps both as one, so for that moment
/// the neighbour's pages are not there ([`victory_gpu::title_memory_remapping`]
/// is held, and a thread of the title's that touches them then is tried
/// again); what the GPU side knows of the neighbour stays
/// ([`victory_gpu::title_memory_regrouped`]). So too for what stays of a
/// view unmapped in part ([`unmap`]), which with views joined is the usual
/// way a page goes.
fn join_views() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_JOIN_VIEWS"));
    switched(&ON)
}

/// Views are joined only within one aligned window of this size: what a
/// join unmaps for a moment and maps again (and the page-table entries
/// the host then fills in again) is at most this.
const JOIN_WINDOW: usize = 4 * MIB;

// A joined view's pages are noted page by page ([`Shown::pages`]), as its
// parts were: joining and cutting it changes none of those notes.
const _: () = assert!(JOIN_WINDOW / PAGE <= SHORT);

/// Whether the entries of one `XMemMapPhysicalPagesScatter` call at
/// addresses that follow one another are mapped (or unmapped) as one call
/// instead of one call per page (`VICTORY_SCATTER_RUNS`, on under Wine with XMEM_RUNS=1;
/// then no pages are mapped side by side, [`side_by_side`]: the threads
/// would stand in line for wineserver's one).
fn scatter_runs() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_SCATTER_RUNS"));
    switched(&ON)
}

/// Whether the GPU's page-mapping copies map each run of consecutive
/// pages as one view instead of one view per page (`VICTORY_ALIAS_RUNS`,
/// on under Wine with XMEM_RUNS=1; then each range is mapped by the caller alone, as with
/// `VICTORY_MAP_THREADS=1`).
fn alias_runs() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_ALIAS_RUNS"));
    switched(&ON)
}

/// Whether pages that show what a call asks them to show already are left
/// as they are (`VICTORY_MAP_SAME`, on under Wine with XMEM_RUNS=1; `0`: every page of a call
/// is unmapped and mapped again, as before). The pages at the two ends of a
/// call's range that are the same stay; what lies between the first and the
/// last page that change is unmapped and mapped as before (a page that is
/// the same between two that change would otherwise cut the range into more
/// views than one call makes of it). A call whose pages are all the same
/// asks the host nothing and tells the GPU side nothing: the bytes are the
/// ones they were, at the addresses they were at, and what guards them
/// stands.
fn map_same() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_MAP_SAME"));
    switched(&ON)
}

/// Whether the entries of one page-mapping copy of the GPU's
/// ([`map_aliases`]) at addresses that follow one another are mapped as one
/// call (`VICTORY_ALIAS_BATCH`, on under Wine with XMEM_RUNS=1, with [`alias_runs`]; `0`: an
/// entry after the other, as before). The title has a page mapped per entry;
/// entry by entry, each page that shows the section's next slice is joined
/// with the view before it ([`join_views`]), which unmaps that view and maps
/// both again: three requests to wineserver a page, and the guards of the
/// view's blocks set again each time. As one call, the pages are mapped as
/// the runs of slices they are, each run once.
fn alias_batch() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_ALIAS_BATCH"));
    switched(&ON)
}

/// Whether the views a call unmaps whole that stand side by side are told
/// to the GPU side as one range, not one after the other
/// (`VICTORY_UNMAP_RUNS`, on under Wine with XMEM_RUNS=1; `0`: each by itself before its own
/// unmapping, as before). What is told is the same; it is told in one taking
/// of the guards' lock (which every refresh and every write fault of the
/// title's waits for) instead of one for each view.
fn unmap_runs() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_UNMAP_RUNS"));
    switched(&ON)
}

/// Whether a join ([`map_joined`]) splits the new range off its placeholder
/// only where our own note of the placeholders does not say it is the
/// range's own already (`VICTORY_MAP_KNOWN`, on under Wine with XMEM_RUNS=1; `0`: split every
/// time, as before, the call failing harmlessly where there is nothing to
/// split).
fn known() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_MAP_KNOWN"));
    switched(&ON)
}

/// Where a page lives, or what a view starts with: a section and a page
/// index in it.
#[derive(Clone)]
struct Slice {
    section: Arc<Section>,
    index: usize,
}

#[derive(Default)]
struct Physical {
    /// Mapped views by address: their length and what they show.
    views: BTreeMap<usize, (usize, Slice)>,
    /// Placeholders that are some view's own, left by its unmapping: address
    /// and length. The title may have joined one with others since.
    holes: HashMap<usize, usize>,
}

impl Physical {
    /// Whether the page at `at` shows `slice`.
    fn shows(&self, at: usize, slice: &Slice) -> bool {
        self.views.range(..=at).next_back().is_some_and(|(base, (size, s))| {
            at < base + size
                && (at - base).is_multiple_of(PAGE)
                && Arc::ptr_eq(&s.section, &slice.section)
                && s.index + (at - base) / PAGE == slice.index
        })
    }

    /// What the page at `at` shows.
    fn slice_at(&self, at: usize) -> Option<Slice> {
        self.views
            .range(..=at)
            .next_back()
            .filter(|(base, (size, _))| at < *base + size)
            .map(|(base, (_, s))| Slice {
                index: s.index + (at - base) / PAGE,
                section: s.section.clone(),
            })
    }
}

/// The title's pages by number.
struct Pages {
    next_page: usize,
    pages: HashMap<usize, Slice>,
}

/// Where the title sees its pages: [`Physical::views`] the other way.
/// Locked apart from them, and only for a moment ([`PAGES`]).
#[derive(Default)]
struct Shown {
    /// The addresses that show a page, by the page's section and index.
    /// Views of more than [`SHORT`] pages are in `long`.
    pages: HashMap<(usize, usize), Vec<usize>>,
    /// Address, bytes, section, first page.
    long: Vec<(usize, usize, usize, usize)>,
}

/// Pages of a view noted page by page in [`Shown::pages`].
const SHORT: usize = 64;

/// Taken after [`PHYSICAL`] where both are.
static SHOWN: LazyLock<RwLock<Shown>> = LazyLock::new(Default::default);

fn shown_mut() -> RwLockWriteGuard<'static, Shown> {
    SHOWN.write().unwrap_or_else(|e| e.into_inner())
}

impl Shown {
    fn note(&mut self, base: usize, len: usize, slice: &Slice) {
        let section = Arc::as_ptr(&slice.section) as usize;
        if len / PAGE > SHORT {
            self.long.push((base, len, section, slice.index));
            return;
        }
        for k in 0..len / PAGE {
            self.pages.entry((section, slice.index + k)).or_default().push(base + k * PAGE);
        }
    }

    fn forget(&mut self, base: usize, len: usize, slice: &Slice) {
        if len / PAGE > SHORT {
            self.long.retain(|view| view.0 != base);
            return;
        }
        let section = Arc::as_ptr(&slice.section) as usize;
        for k in 0..len / PAGE {
            let key = (section, slice.index + k);
            if let Some(at) = self.pages.get_mut(&key) {
                at.retain(|&at| at != base + k * PAGE);
                if at.is_empty() {
                    self.pages.remove(&key);
                }
            }
        }
    }

    /// Where the title sees the page at `slice`: addresses and bytes.
    fn at(&self, slice: &Slice) -> Vec<(usize, usize)> {
        let section = Arc::as_ptr(&slice.section) as usize;
        let mut out: Vec<(usize, usize)> =
            self.pages.get(&(section, slice.index)).into_iter().flatten().map(|&at| (at, PAGE)).collect();
        for &(base, len, of, index) in &self.long {
            if of == section && (index..index + len / PAGE).contains(&slice.index) {
                out.push((base + (slice.index - index) * PAGE, PAGE));
            }
        }
        out
    }
}

/// See [`pages_written`].
pub(crate) fn stream_notice() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("VICTORY_STREAM_NOTICE").is_none_or(|v| v != "0"));
    *ON
}

/// The views, changed by the title's mapping calls; held for as long as
/// those take (the host maps, commits and unmaps under it).
static PHYSICAL: LazyLock<Mutex<Physical>> = LazyLock::new(Default::default);

fn physical() -> MutexGuard<'static, Physical> {
    PHYSICAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// [`physical`], the wait for it counted ([`COST`]).
fn physical_counted() -> MutexGuard<'static, Physical> {
    let started = std::time::Instant::now();
    let physical = physical();
    cost(cost::LOCK_NS, started.elapsed().as_nanos() as u64);
    physical
}

/// `victory_gpu::title_memory_remapping`, the wait for it counted
/// ([`COST`]): it is had when no read of title memory in place is under way.
fn remapping() -> impl Drop {
    let started = std::time::Instant::now();
    let changing = victory_gpu::title_memory_remapping();
    cost(cost::HOLD_NS, started.elapsed().as_nanos() as u64);
    changing
}

/// Runs `f`, which tells the GPU side of a change, and counts its time
/// ([`COST`]).
fn told<T>(f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let done = f();
    cost(cost::TOLD_NS, started.elapsed().as_nanos() as u64);
    done
}

/// The title's pages. Apart from the views: DirectStorage looks pages up at
/// every request, from 24 workers, and used to wait there for the title's
/// mapping calls (12,000 waits of 1 to 170 ms in a load of 90 s). Written
/// only where the title allocates or frees pages. Taken after [`PHYSICAL`]
/// where both are.
static PAGES: LazyLock<RwLock<Pages>> = LazyLock::new(|| {
    RwLock::new(Pages {
        // Page numbers are opaque to titles; 0 stays unused.
        next_page: 0x10_0000,
        pages: HashMap::new(),
    })
});

fn page_table() -> RwLockReadGuard<'static, Pages> {
    PAGES.read().unwrap_or_else(|e| e.into_inner())
}

fn page_table_mut() -> RwLockWriteGuard<'static, Pages> {
    PAGES.write().unwrap_or_else(|e| e.into_inner())
}

/// `count` is in and out (pages wanted, pages given); all or nothing here.
pub extern "system" fn XMemAllocatePhysicalPages(
    flags: u64,
    count: *mut usize,
    pages: *mut usize,
) -> BOOL {
    if count.is_null() || pages.is_null() {
        return 0;
    }
    // SAFETY: checked non-null.
    let n = unsafe { *count };
    let Some(size) = n.checked_mul(PAGE).filter(|&s| s > 0) else {
        return 0;
    };
    // A pagefile-backed section of `size` bytes, its pages committed when
    // first mapped or written (`commit`): the title asks for 6.1
    // GiB at once, which a busy host often cannot commit (error 1450).
    // SAFETY: plain section creation.
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            // Under Wine a committed section is a sparse file, free until
            // touched, and committing a reserved one page by page costs a
            // wineserver request each that scans every range committed so
            // far: commit it whole there.
            PAGE_READWRITE | if on_wine() { SEC_COMMIT } else { SEC_RESERVE },
            (size as u64 >> 32) as u32,
            size as u32,
            std::ptr::null(),
        )
    };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        tracing::warn!(n, %error, "XMemAllocatePhysicalPages failed");
        // SAFETY: checked non-null.
        unsafe { count.write(0) };
        return 0;
    }
    tracing::debug!(
        n,
        flags = format_args!("{flags:#x}"),
        "XMemAllocatePhysicalPages"
    );
    // For the GPU side's in-place census: only a committed section can be
    // opened as a host heap.
    victory_gpu::title_memory_section(handle as usize, size as u64, on_wine());
    let section = Arc::new(Section {
        handle,
        pages: n,
        window: std::sync::OnceLock::new(),
        committed: (0..n.div_ceil(64)).map(|_| std::sync::atomic::AtomicU64::new(0)).collect(),
    });
    let mut table = page_table_mut();
    let first = table.next_page;
    table.next_page += n * FRAMES;
    for index in 0..n {
        let slice = Slice {
            section: section.clone(),
            index,
        };
        table.pages.insert(first + index * FRAMES, slice);
    }
    // Title memory is touched without our locks: a fault there may wait for
    // a mapping call, which takes them.
    drop(table);
    for index in 0..n {
        // SAFETY: the title passes room for `n` page numbers.
        unsafe { pages.add(index).write(first + index * FRAMES) };
    }
    1
}

/// Page numbers count 4 KiB frames, so a 64 KiB page's is a multiple of
/// 16: the title keeps them as 21-bit `number >> 4` and rebuilds
/// them with `<< 4` (128 GiB of pages at most).
const FRAMES: usize = PAGE >> 12;

/// Pages still mapped stay readable through their views until unmapped.
pub extern "system" fn XMemFreePhysicalPages(n: usize, pages: *const usize) -> BOOL {
    if pages.is_null() {
        return 0;
    }
    // SAFETY: the title passes `n` page numbers.
    let numbers = unsafe { std::slice::from_raw_parts(pages, n) }.to_vec();
    let mut table = page_table_mut();
    // Dropped without the lock: the last page of a section unmaps its window.
    let gone: Vec<Slice> = numbers.iter().filter_map(|number| table.pages.remove(number)).collect();
    drop(table);
    drop(gone);
    1
}

/// A view of our own on consecutive slices of one section, committed and
/// writable, unmapped when dropped.
struct View {
    at: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

impl View {
    /// Maps `n` slices from `slice` on.
    fn map(slice: &Slice, n: usize) -> Option<View> {
        use windows_sys::Win32::System::Memory::{FILE_MAP_WRITE, MapViewOfFile};
        let (offset, len) = ((slice.index * PAGE) as u64, n * PAGE);
        count(3, 1, 0);
        // SAFETY: slices of a section we own; the view is ours until dropped.
        let at = unsafe {
            MapViewOfFile(
                slice.section.handle,
                FILE_MAP_WRITE,
                (offset >> 32) as u32,
                offset as u32,
                len,
            )
        };
        if at.Value.is_null() {
            tracing::warn!(error = %std::io::Error::last_os_error(), n, "physical pages not mapped for a write");
            return None;
        }
        let view = View { at, len };
        if !commit(at.Value as usize, len) {
            tracing::warn!(error = %std::io::Error::last_os_error(), n, "physical pages not committed for a write");
            return None;
        }
        Some(view)
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the view holds `len` committed read-write bytes, ours
        // alone while `self` lives (other views of the slices only alias
        // them, as the console's do).
        unsafe { std::slice::from_raw_parts_mut(self.at.Value.cast(), self.len) }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::UnmapViewOfFile;
        // SAFETY: our view.
        unsafe { UnmapViewOfFile(self.at) };
    }
}

/// The pages behind bytes `[offset, offset + len)` of `pages` (64 KiB
/// each), as runs of consecutive slices of one section: each run's first
/// slice and its length in pages. One look at the page table. `None`
/// if a page is unknown or `pages` ends early.
fn runs(pages: &[usize], offset: usize, len: usize) -> Option<Vec<(Slice, usize)>> {
    let end = offset.checked_add(len)?;
    let pages = pages.get(offset / PAGE..end.div_ceil(PAGE))?;
    let table = page_table();
    let mut runs: Vec<(Slice, usize)> = Vec::new();
    for page in pages {
        let Some(slice) = table.pages.get(page) else {
            tracing::warn!(page, offset, len, "a physical page the title does not own (any more)");
            return None;
        };
        match runs.last_mut() {
            Some((first, n))
                if Arc::ptr_eq(&first.section, &slice.section)
                    && slice.index == first.index + *n =>
            {
                *n += 1;
            }
            _ => runs.push((slice.clone(), 1)),
        }
    }
    Some(runs)
}

/// Whether the title owns every page under bytes `[offset, offset + len)`.
pub(crate) fn pages_known(pages: &[usize], offset: usize, len: usize) -> bool {
    let table = page_table();
    pages
        .get(offset / PAGE..(offset + len).div_ceil(PAGE))
        .is_some_and(|pages| pages.iter().all(|page| table.pages.contains_key(page)))
}

/// Where the title sees the pages under bytes `[offset, offset + len)` now,
/// in order (for the streaming trace: whether that changed while a request
/// wrote them).
pub(crate) fn pages_shown(pages: &[usize], offset: usize, len: usize) -> Vec<(usize, usize)> {
    let numbers = pages.get(offset / PAGE..(offset + len).div_ceil(PAGE)).unwrap_or_default();
    let slices: Vec<Slice> = {
        let table = page_table();
        numbers.iter().filter_map(|n| table.pages.get(n)).cloned().collect()
    };
    let shown = SHOWN.read().unwrap_or_else(|e| e.into_inner());
    let mut at: Vec<(usize, usize)> = slices.iter().flat_map(|slice| shown.at(slice)).collect();
    at.sort_unstable();
    at
}

/// Copies `data` into physical pages, from `offset` bytes into the first,
/// whether or not they are mapped anywhere (DirectStorage destinations).
/// One view per run of consecutive pages, not one per page: each view
/// costs a map, a commit and an unmap (which flushes every core's TLB).
/// False if a page is unknown or the host fails.
pub(crate) fn write_physical(pages: &[usize], offset: usize, data: &[u8]) -> bool {
    let Some(runs) = runs(pages, offset, data.len()) else {
        return false;
    };
    let (mut within, mut done) = (offset % PAGE, 0);
    for (slice, n) in &runs {
        let k = (n * PAGE - within).min(data.len() - done);
        if let Some(at) = slice.section.window(slice.index, *n) {
            // SAFETY: committed bytes of the section's view, which `runs`
            // holds; the title's other uses of them race as on the console.
            unsafe { std::ptr::copy_nonoverlapping(data[done..].as_ptr(), (at + within) as *mut u8, k) };
        } else {
            let Some(mut view) = View::map(slice, *n) else {
                return false;
            };
            view.bytes()[within..within + k].copy_from_slice(&data[done..done + k]);
        }
        (within, done) = (0, done + k);
    }
    true
}

/// Bytes `[offset, offset + len)` of physical pages, writable in place
/// until dropped: DirectStorage decodes into it instead of a buffer it
/// would then copy.
pub(crate) struct PageWindow {
    view: Window,
    within: usize,
    len: usize,
}

enum Window {
    /// In the section's own view, which the section holds.
    Kept(usize, #[expect(dead_code, reason = "held for the view")] Arc<Section>),
    Own(View),
}

impl PageWindow {
    pub(crate) fn bytes(&mut self) -> &mut [u8] {
        let (within, len) = (self.within, self.len);
        match &mut self.view {
            // SAFETY: committed bytes of the section's view, ours alone as
            // far as the runtime goes (other views alias them, as the
            // console's do).
            Window::Kept(at, _) => unsafe { std::slice::from_raw_parts_mut((*at + within) as *mut u8, len) },
            Window::Own(view) => &mut view.bytes()[within..within + len],
        }
    }
}

/// A window on bytes `[offset, offset + len)` of `pages` when they are
/// consecutive pages of one allocation (how titles usually hand them out);
/// `None` otherwise (pages of several allocations, out of order, or
/// unknown, or a host failure), which `write_physical` then handles or
/// reports.
pub(crate) fn page_window(pages: &[usize], offset: usize, len: usize) -> Option<PageWindow> {
    let runs = runs(pages, offset, len)?;
    let [(slice, n)] = runs.as_slice() else {
        return None;
    };
    let view = match slice.section.window(slice.index, *n) {
        Some(at) => Window::Kept(at, slice.section.clone()),
        None => Window::Own(View::map(slice, *n)?),
    };
    Some(PageWindow {
        view,
        within: offset % PAGE,
        len,
    })
}

/// The title addresses that show bytes `[offset, offset + len)` of `pages`
/// now, byte for byte, as runs (address, bytes) in address order: a write
/// to the pages through a view of our own ([`page_window`],
/// [`write_physical`]) lands there too, and nothing that watches those
/// addresses sees it. Empty if no view shows them. Byte for byte, not by
/// whole pages: the title packs several small textures into one page and
/// fills each with a request of its own.
pub(crate) fn shown_at(pages: &[usize], offset: usize, len: usize) -> Vec<(usize, usize)> {
    let Some(end) = offset.checked_add(len) else {
        return Vec::new();
    };
    let first = offset / PAGE;
    let Some(touched) = pages.get(first..end.div_ceil(PAGE)) else {
        return Vec::new();
    };
    let slices: Vec<Option<Slice>> = {
        let table = page_table();
        touched.iter().map(|page| table.pages.get(page).cloned()).collect()
    };
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    {
        let shown = SHOWN.read().unwrap_or_else(|e| e.into_inner());
        for (i, slice) in slices.iter().enumerate() {
            let Some(slice) = slice else { continue };
            // The bytes of this page that the write covers.
            let lo = if i == 0 { offset % PAGE } else { 0 };
            let hi = (end - (first + i) * PAGE).min(PAGE);
            pieces.extend(shown.at(slice).into_iter().map(|(at, _)| (at + lo, hi - lo)));
        }
    }
    pieces.sort_unstable();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (at, bytes) in pieces {
        match runs.last_mut() {
            Some((start, n)) if *start + *n == at => *n += bytes,
            _ => runs.push((at, bytes)),
        }
    }
    runs
}

/// DirectStorage has put bytes `[offset, offset + len)` of `pages` into
/// memory, through a view of our own: the GPU side is told where the title
/// sees them, so that what it copied there before they arrived (a texture
/// uploaded when its view was written holds whatever the pages held then)
/// is copied again. Call when the bytes are there.
/// `VICTORY_STREAM_NOTICE=0`: not told; what is then done is
/// `VICTORY_PAGE_WRITES`' to say (`victory_gpu::title_memory_written`).
pub(crate) fn pages_written(pages: &[usize], offset: usize, len: usize) {
    if len == 0 {
        return;
    }
    for (at, bytes) in shown_at(pages, offset, len) {
        if stream_notice() {
            victory_gpu::kernel_will_write(at, bytes);
        } else {
            victory_gpu::title_memory_written(at, bytes, true);
        }
    }
}

/// What the title's mapping calls cost since the last asking, for the
/// streaming log, counted only while that log is on: calls, pages,
/// milliseconds in all, the longest one's and waiting for the lock; the same
/// of the GPU's page-mapping copies ([`map_alias`]; calls and milliseconds);
/// and by system call (unmap, split, map, commit) how many and their
/// milliseconds.
static MAPPING: [std::sync::atomic::AtomicU64; 15] = [const { std::sync::atomic::AtomicU64::new(0) }; 15];

pub(crate) fn map_costs() -> Option<String> {
    let n: [u64; 15] = std::array::from_fn(|i| MAPPING[i].swap(0, Ordering::Relaxed));
    let time = |i: usize| matches!(i, 2 | 3 | 4 | 6 | 8 | 10 | 12 | 14);
    (n[0] + n[5] > 0).then(|| {
        let all: Vec<String> = n
            .iter()
            .enumerate()
            .map(|(i, &n)| if time(i) { n / 1_000_000 } else { n })
            .map(|n| n.to_string())
            .collect();
        all.join("\t")
    })
}

/// The system calls that change a mapping, as [`MAPPING`] counts them
/// (`Join`: as [`COST`] alone does).
#[derive(Clone, Copy)]
enum Call {
    Unmap = 7,
    Split = 9,
    Map = 11,
    Commit = 13,
    Join = 15,
}

impl Call {
    /// The call's slot in [`COST`].
    fn cost(self) -> usize {
        match self {
            Call::Unmap => cost::UNMAP,
            Call::Split => cost::SPLIT,
            Call::Join => cost::JOIN,
            Call::Map => cost::MAP,
            Call::Commit => cost::COMMIT,
        }
    }
}

/// Runs `f`, a system call, and counts it: always for [`count`]'s line
/// ([`COST`]), and for the streaming log while that is on.
fn counted<T>(call: Call, f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let done = f();
    let ns = started.elapsed().as_nanos() as u64;
    cost(call.cost(), 1);
    cost(call.cost() + 1, ns);
    if (call as usize) < MAPPING.len() && super::dstorage::stream_log::on() {
        MAPPING[call as usize].fetch_add(1, Ordering::Relaxed);
        MAPPING[call as usize + 1].fetch_add(ns, Ordering::Relaxed);
    }
    done
}

/// Counts a mapping call when dropped.
struct Costed {
    started: std::time::Instant,
    /// `None`: a copy of mappings.
    pages: Option<usize>,
    waited: u64,
}

impl Costed {
    fn begin(pages: Option<usize>) -> Option<Costed> {
        super::dstorage::stream_log::on().then(|| Costed { started: std::time::Instant::now(), pages, waited: 0 })
    }
}

impl Drop for Costed {
    fn drop(&mut self) {
        let ns = self.started.elapsed().as_nanos() as u64;
        let Some(pages) = self.pages else {
            MAPPING[5].fetch_add(1, Ordering::Relaxed);
            MAPPING[6].fetch_add(ns, Ordering::Relaxed);
            return;
        };
        for (i, n) in [1, pages as u64, ns].into_iter().enumerate() {
            MAPPING[i].fetch_add(n, Ordering::Relaxed);
        }
        MAPPING[3].fetch_max(ns, Ordering::Relaxed);
        MAPPING[4].fetch_add(self.waited, Ordering::Relaxed);
    }
}

/// Whether a mapping change asks the host only for what it needs: no split
/// of a placeholder that is already the view's own, no commit of section
/// pages that are committed (`VICTORY_MAP_LEAN=0`: both every time, as
/// before).
///
/// All of the title's streaming goes through one thread, which maps the
/// pages of a batch and then submits it. Each call waits for the process's
/// one address-space lock, behind the guards' and the driver's: 200 to 400
/// us a page in the world against 10 in a menu, 2 to 4 s of every 5 s of
/// that thread (runs stream-1 and -2; all six stacks taken of it in the
/// opening were in these calls), so that models came as fast as their pages
/// could be mapped. A page took up to four calls (unmap, split, map,
/// commit), twice: the title maps a page into its pool and then has the GPU
/// map it into its tiled heap.
fn lean() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("VICTORY_MAP_LEAN").is_none_or(|v| v != "0"));
    *ON
}

/// The title's streaming thread runs below normal priority, as do sixteen
/// workers of ours when pipelines are made; on the console nothing else
/// wants its core. Called where that thread maps and submits: lifts a
/// thread that is below normal to above it (`VICTORY_STREAM_PRIORITY=0`:
/// left as the title set it).
///
/// Asking a thread's priority is a request to wineserver under Wine, one
/// for every scatter call and every page-mapping copy: a thread is asked
/// about once a second at most there (`VICTORY_LIFT_SELDOM`, on under Wine
/// with XMEM_RUNS=1; `0`: at every call, as before).
pub(crate) fn lift_thread() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, GetThreadPriority, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("VICTORY_STREAM_PRIORITY").is_none_or(|v| v != "0"));
    if !*ON {
        return;
    }
    static SELDOM: LazyLock<bool> = LazyLock::new(|| wine_switch("VICTORY_LIFT_SELDOM"));
    thread_local! {
        /// When this thread's priority was last asked.
        static ASKED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    }
    if switched(&SELDOM) {
        let now = std::time::Instant::now();
        if ASKED.get().is_some_and(|asked| now.duration_since(asked).as_secs() < 1) {
            return;
        }
        ASKED.set(Some(now));
    }
    cost(cost::LIFT_ASKS, 1);
    // SAFETY: plain calls on this thread.
    unsafe {
        let thread = GetCurrentThread();
        if matches!(GetThreadPriority(thread), -2..0) {
            SetThreadPriority(thread, THREAD_PRIORITY_ABOVE_NORMAL);
        }
    }
}

/// Maps `n` pages at `address`, in place of what is mapped there; null
/// `pages` unmaps. Returns `address`, or null if a page or the host fails.
pub extern "system" fn XMemMapPhysicalPages(
    address: *mut c_void,
    n: usize,
    pages: *const usize,
) -> *mut c_void {
    let base = address as usize;
    // Read before any lock is taken: a fault in title memory may wait for
    // the mapping call that holds them.
    // SAFETY: the title passes `n` page numbers, or null.
    let pages = (!pages.is_null()).then(|| unsafe { std::slice::from_raw_parts(pages, n) }.to_vec());
    let mut costed = Costed::begin(Some(n));
    let _called = victory_gpu::timeline::span("the title's XMemMapPhysicalPages");
    let _busy = Busy::begin();
    let mut physical = physical_counted();
    if let Some(costed) = &mut costed {
        costed.waited = costed.started.elapsed().as_nanos() as u64;
    }
    let slices: Option<Vec<Option<Slice>>> = pages.as_ref().map(|pages| {
        let table = page_table();
        pages.iter().map(|page| table.pages.get(page).cloned()).collect()
    });
    // The pages `[lo, hi)` change; those before and after show what they
    // are to show already and stay as they are ([`map_same`]; not where a
    // page is unknown: that call fails as it did).
    let (mut lo, mut hi) = (0, n);
    if let Some(slices) = &slices
        && n != 0
        && map_same()
        && slices.iter().all(Option::is_some)
    {
        let same = |i: usize| slices[i].as_ref().is_some_and(|slice| physical.shows(base + i * PAGE, slice));
        while lo < hi && same(lo) {
            lo += 1;
        }
        while hi > lo && same(hi - 1) {
            hi -= 1;
        }
        cost(cost::SAME_PAGES, (n - (hi - lo)) as u64);
        if lo == hi {
            cost(cost::SAME_CALLS, 1);
            return address;
        }
    }
    // Until the pages are mapped again: `unmap` takes whole views away and
    // maps what stays of them again, and a mapped view is a reservation
    // until it is committed. A read of title memory in place must not fall
    // into that gap (`victory_gpu::title_memory_remapping`).
    let _changing = remapping();
    unmap(&mut physical, base + lo * PAGE, (hi - lo) * PAGE);
    let (Some(pages), Some(slices)) = (pages, slices) else {
        return address;
    };
    let mut i = lo;
    while i < hi {
        let Some(first) = slices[i].clone() else {
            tracing::warn!(page = pages[i], "XMemMapPhysicalPages: unknown page");
            return std::ptr::null_mut();
        };
        // One view per run of consecutive slices of one section.
        let run = 1 + slices[i + 1..hi]
            .iter()
            .zip(1..)
            .take_while(|&(slice, k)| {
                slice.as_ref().is_some_and(|s| {
                    Arc::ptr_eq(&s.section, &first.section) && s.index == first.index + k
                })
            })
            .count();
        if !map(&mut physical, base + i * PAGE, run * PAGE, first) {
            return std::ptr::null_mut();
        }
        count(0, 1, physical.views.len());
        census(&physical);
        i += run;
    }
    address
}

/// One page at each address; null `pages`, or a page number 0, unmaps
/// (the title passes 0 for addresses to clear).
pub extern "system" fn XMemMapPhysicalPagesScatter(
    addresses: *const *mut c_void,
    n: usize,
    pages: *const usize,
) -> BOOL {
    if addresses.is_null() {
        return 0;
    }
    lift_thread();
    if scatter_runs() {
        // Read before any lock is taken, as in `XMemMapPhysicalPages`.
        let wanted: Vec<(usize, usize)> = (0..n)
            // SAFETY: the title passes `n` addresses and, unless null, `n` pages.
            .map(|i| unsafe { (*addresses.add(i) as usize, if pages.is_null() { 0 } else { *pages.add(i) }) })
            .collect();
        // The entries at addresses that follow one another, all mapped or
        // all unmapped, are one call: its pages are mapped as the runs of
        // slices they are (and joined with their neighbours, `join_views`),
        // and what was there is unmapped view by view, not page by page (a
        // view of many pages unmapped a page at a time is unmapped and
        // mapped again around each).
        let mut i = 0;
        while i < n {
            let unmaps = wanted[i].1 == 0;
            let mut run = 1;
            while i + run < n
                && wanted[i + run].0 == wanted[i].0.wrapping_add(run * PAGE)
                && (wanted[i + run].1 == 0) == unmaps
            {
                run += 1;
            }
            let list: Vec<usize> = wanted[i..i + run].iter().map(|w| w.1).collect();
            count(8, run as u64, 0);
            count(9, 1, 0);
            let list = if unmaps { std::ptr::null() } else { list.as_ptr() };
            if XMemMapPhysicalPages(wanted[i].0 as *mut c_void, run, list).is_null() {
                return 0;
            }
            i += run;
        }
        return 1;
    }
    if side_by_side() > 1 && n > 1 {
        // Read before any lock is taken, as in `XMemMapPhysicalPages`.
        let wanted: Vec<(usize, usize)> = (0..n)
            // SAFETY: the title passes `n` addresses and, unless null, `n` pages.
            .map(|i| unsafe { (*addresses.add(i) as usize, if pages.is_null() { 0 } else { *pages.add(i) }) })
            .collect();
        if wanted.iter().all(|w| w.0 % PAGE == 0) {
            return scatter(&wanted).into();
        }
    }
    for i in 0..n {
        // SAFETY: the title passes `n` addresses and, unless null, `n` pages.
        let (address, page) = unsafe {
            (
                *addresses.add(i),
                if pages.is_null() || *pages.add(i) == 0 {
                    std::ptr::null()
                } else {
                    pages.add(i)
                },
            )
        };
        if XMemMapPhysicalPages(address, 1, page).is_null() {
            return 0;
        }
    }
    1
}

/// Maps the physical pages shown at `[from, from + len)` at `to` as well,
/// in place of what `to` shows; `from` 0 unmaps. The GPU's page-mapping
/// copies (`victory_gpu`, queue `CopyPageMappingsBatchX`):
/// the title maps pages into a pool view, then has the GPU map them into its tiled
/// heap. Page by page, or with [`alias_runs`] a view for each run of pages
/// that are a section's consecutive slices; false if a source page is not
/// mapped physical memory.
pub fn map_alias(to: u64, from: u64, len: u64) -> bool {
    let (mut to, mut from, mut len) = (to as usize, from as usize, len as usize);
    let _costed = Costed::begin(None);
    let _busy = Busy::begin();
    let mut physical = physical_counted();
    // The pages at the range's two ends that show what they are to show
    // already stay as they are ([`map_same`]). Not where the range is mapped
    // from a part of itself, nor where a source page is not mapped: those
    // calls go as they did.
    if from != 0
        && len != 0
        && map_same()
        && [to, from, len].iter().all(|v| v.is_multiple_of(PAGE))
        && (from + len <= to || to + len <= from)
        && (0..len).step_by(PAGE).all(|k| physical.slice_at(from + k).is_some())
    {
        let same = |k: usize| physical.slice_at(from + k).is_some_and(|slice| physical.shows(to + k, &slice));
        let (mut head, mut tail) = (0, len);
        while head < tail && same(head) {
            head += PAGE;
        }
        while tail > head && same(tail - PAGE) {
            tail -= PAGE;
        }
        cost(cost::SAME_PAGES, ((len - (tail - head)) / PAGE) as u64);
        if head == tail {
            cost(cost::SAME_CALLS, 1);
            return true;
        }
        (to, from, len) = (to + head, from + head, tail - head);
    }
    // As in `XMemMapPhysicalPages`: no read in place meets the gap.
    let _changing = remapping();
    unmap(&mut physical, to, len);
    if from == 0 {
        return true;
    }
    let runs = alias_runs();
    let mut k = 0;
    while k < len {
        let Some(slice) = physical.slice_at(from + k) else {
            return false;
        };
        // The pages after it that are the section's next slices: one view
        // for the run. A page that is not mapped ends the run here and
        // fails the call at its own turn, as page by page.
        let mut run = 1;
        while runs && k + run * PAGE < len {
            match physical.slice_at(from + k + run * PAGE) {
                Some(next) if Arc::ptr_eq(&next.section, &slice.section) && next.index == slice.index + run => run += 1,
                _ => break,
            }
        }
        if !map_alias_run(&mut physical, to + k, slice, run) {
            return false;
        }
        k += run * PAGE;
    }
    true
}

/// Maps `run` pages from `slice` on at `base` for a page-mapping copy of the
/// GPU's: as one view, or page by page where the host will not have them as
/// one. False if the host refuses a page.
fn map_alias_run(physical: &mut Physical, base: usize, slice: Slice, run: usize) -> bool {
    count(1, run as u64, 0);
    if run > 1 {
        if map_quietly(physical, base, run * PAGE, slice.clone(), true) {
            count(2, 1, physical.views.len());
            census(physical);
            return true;
        }
        // The host would not have the run as one view: page by page.
        static REFUSED: std::sync::Once = std::sync::Once::new();
        REFUSED.call_once(|| tracing::warn!(pages = run, "a run of page mappings was refused as one view; mapped page by page"));
    }
    for i in 0..run {
        let page = Slice { index: slice.index + i, section: slice.section.clone() };
        if !map(physical, base + i * PAGE, PAGE, page) {
            return false;
        }
        count(2, 1, physical.views.len());
    }
    true
}

/// Views mapped under one hold of the change of mapping by [`map_alias_group`]:
/// a read of title memory in place waits for as long.
const ALIAS_HOLD: usize = 8;

/// [`map_alias`] of `entries`, an entry after the other, where their
/// addresses follow one another from `to` on within one window of
/// [`JOIN_WINDOW`], all are whole pages with a source, and no source lies in
/// what they map over ([`alias_batch`]). What each page is to show is looked
/// up first (nothing the entries map changes it), and the pages are then
/// mapped as the runs of slices they are, a few views under each hold of the
/// change, what was there unmapped just before.
///
/// As an entry after the other, a source page that is not mapped fails the
/// call there: the entries before it are mapped, of its own entry the pages
/// before it, and that entry's range is unmapped whole.
fn map_alias_group(to: usize, entries: &[(u64, u64, u64)]) -> bool {
    let _costed = Costed::begin(None);
    let _busy = Busy::begin();
    let mut physical = physical_counted();
    let mut wanted: Vec<Slice> = Vec::new();
    let (mut bytes, mut ok) = (0, true);
    'entries: for &(_, from, len) in entries {
        bytes += len as usize;
        for k in (0..len as usize).step_by(PAGE) {
            let Some(slice) = physical.slice_at(from as usize + k) else {
                ok = false;
                break 'entries;
            };
            wanted.push(slice);
        }
    }
    // The pages `[lo, hi)` change ([`map_same`], as in `map_alias`).
    let (mut lo, mut hi) = (0, wanted.len());
    if ok && map_same() {
        while lo < hi && physical.shows(to + lo * PAGE, &wanted[lo]) {
            lo += 1;
        }
        while hi > lo && physical.shows(to + (hi - 1) * PAGE, &wanted[hi - 1]) {
            hi -= 1;
        }
        cost(cost::SAME_PAGES, (wanted.len() - (hi - lo)) as u64);
        if lo == hi {
            cost(cost::SAME_CALLS, 1);
            return true;
        }
    }
    // Where what is unmapped ends: with the last page that changes, or with
    // the entry that fails.
    let end = if ok { hi * PAGE } else { bytes };
    let mut i = lo;
    loop {
        // The next runs of slices, as many as one hold maps.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut next = i;
        while next < hi && runs.len() < ALIAS_HOLD {
            let first = &wanted[next];
            let run = 1 + wanted[next + 1..hi]
                .iter()
                .zip(1..)
                .take_while(|&(slice, k)| Arc::ptr_eq(&slice.section, &first.section) && slice.index == first.index + k)
                .count();
            runs.push((next, run));
            next += run;
        }
        let until = if next >= hi { end } else { next * PAGE };
        // As in `XMemMapPhysicalPages`: no read in place meets the gap.
        let _changing = remapping();
        unmap(&mut physical, to + i * PAGE, until - i * PAGE);
        for (at, run) in runs {
            cost(cost::MERGED, run as u64 - 1);
            if !map_alias_run(&mut physical, to + at * PAGE, wanted[at].clone(), run) {
                return false;
            }
        }
        i = next;
        if i >= hi {
            return ok;
        }
    }
}

/// [`map_alias`] of many ranges, in order.
pub fn map_aliases(ranges: &[(u64, u64, u64)]) -> bool {
    let _costed = Costed::begin(None);
    lift_thread();
    // (With views joined or runs mapped as one, a range after the other:
    // a page mapped by itself, side by side with others, is never one view
    // with its neighbours.)
    if side_by_side() <= 1 || alias_runs() || join_views() {
        if !(alias_batch() && alias_runs()) {
            return ranges.iter().all(|&(to, from, len)| map_alias(to, from, len));
        }
        // The entries at addresses that follow one another within a window,
        // all mapped or all unmapped, as one call ([`alias_batch`]): not an
        // entry that maps over a source of its own group, or whose source
        // the group maps over (it is to show what the entries before it left
        // there).
        let whole = |&(to, from, len): &(u64, u64, u64)| {
            len != 0 && [to, from, len].iter().all(|v| v.is_multiple_of(PAGE as u64))
        };
        let window = |to: u64| (to & !(JOIN_WINDOW as u64 - 1)).saturating_add(JOIN_WINDOW as u64);
        let mut i = 0;
        while i < ranges.len() {
            let (to, from, len) = ranges[i];
            let mut end = to.saturating_add(len);
            let mut next = i + 1;
            while whole(&ranges[i]) && end <= window(to) && next < ranges.len() && next - i < GROUP {
                let (at, source, bytes) = ranges[next];
                let grown = at.saturating_add(bytes);
                if !whole(&ranges[next]) || at != end || (source == 0) != (from == 0) || grown > window(to) {
                    break;
                }
                let crossed = ranges[i..=next].iter().any(|&(_, source, bytes)| source != 0 && source < grown && to < source.saturating_add(bytes));
                if crossed {
                    break;
                }
                (end, next) = (grown, next + 1);
            }
            let done = match next - i {
                1 => map_alias(to, from, len),
                _ if from == 0 => map_alias(to, 0, end - to),
                _ => map_alias_group(to as usize, &ranges[i..next]),
            };
            if !done {
                return false;
            }
            i = next;
        }
        return true;
    }
    // By the page: the address, and the one whose page it is to show.
    let pages: Vec<(usize, Option<usize>)> = ranges
        .iter()
        .flat_map(|&(to, from, len)| {
            (0..len as usize).step_by(PAGE).map(move |k| (to as usize + k, (from != 0).then_some(from as usize + k)))
        })
        .collect();
    if pages.iter().any(|p| p.0 % PAGE != 0 || p.1.is_some_and(|from| from % PAGE != 0)) {
        return ranges.iter().all(|&(to, from, len)| map_alias(to, from, len));
    }
    let mut ok = true;
    let mut at = 0;
    while at < pages.len() {
        let _busy = Busy::begin();
        let mut physical = physical();
        let mut ops: Vec<Op> = Vec::new();
        let mut touched = std::collections::HashSet::new();
        for &(to, from) in &pages[at..] {
            // A page the group changes already, or one it would be mapped
            // from: after it.
            if ops.len() == GROUP || touched.contains(&to) || from.is_some_and(|from| touched.contains(&from)) {
                break;
            }
            let slice = from.map(|from| physical.slice_at(from));
            ok &= !matches!(slice, Some(None));
            touched.insert(to);
            ops.push(Op::new(to, slice.flatten()));
            at += 1;
        }
        let _changing = victory_gpu::title_memory_remapping();
        ok &= change(&mut physical, ops);
    }
    ok
}

/// Maps (page number not 0) or unmaps each address; false if a page is not
/// the title's or the host refuses one.
fn scatter(wanted: &[(usize, usize)]) -> bool {
    let mut ok = true;
    let mut at = 0;
    while at < wanted.len() {
        let _busy = Busy::begin();
        let mut costed = Costed::begin(None);
        let mut ops: Vec<Op> = Vec::new();
        let mut physical = physical();
        if let Some(costed) = &mut costed {
            costed.waited = costed.started.elapsed().as_nanos() as u64;
        }
        {
            let table = page_table();
            let mut touched = std::collections::HashSet::new();
            for &(to, page) in &wanted[at..] {
                if ops.len() == GROUP || !touched.insert(to) {
                    break;
                }
                let slice = table.pages.get(&page).cloned();
                if page != 0 && slice.is_none() {
                    tracing::warn!(page, "XMemMapPhysicalPagesScatter: unknown page");
                    ok = false;
                }
                ops.push(Op::new(to, slice));
                at += 1;
            }
        }
        if let Some(costed) = &mut costed {
            costed.pages = Some(ops.len());
        }
        let _changing = victory_gpu::title_memory_remapping();
        ok &= change(&mut physical, ops);
    }
    ok
}

/// Pages changed under the locks at once.
const GROUP: usize = 64;

/// How many threads map a group of pages side by side (`VICTORY_MAP_THREADS`;
/// 8 unless set; 1: the caller alone, a page after the other; `auto`: 1 while
/// mapping takes little of the time, [`WIDE`] while the title streams as fast
/// as its pages are mapped, [`Busy`]).
///
/// shortcut: `auto` is not the default. The intro's diner was there in time
/// with it (one run, textures-d3), but in the dorm, where each call takes 0.3
/// ms of something that only one at a time gets (until a step of the mission,
/// after which it takes 0.01), 16 threads map no more pages than one, and the
/// groups hold the mapping lock: 260 ms a frame against 125 (textures-d4, -d2).
///
/// Mapping and unmapping a view wait for a lock that every protection change
/// of a view takes too, and our guards make 100,000 of those a second, from
/// many threads, in the world: 0.2 to 4 ms a page. One thread stands in that
/// queue once at a time. In a bench (60,000 views, five threads changing
/// protections) one thread mapped 1,500 pages a second, eight 30,000.
fn side_by_side() -> usize {
    // Diagnostics: 1 and 8 by turns.
    #[cfg(all(windows, feature = "diagnostics"))]
    if let Some(on) = victory_gpu::switched("VICTORY_MAP_THREADS") {
        return if on { 8 } else { 1 };
    }
    match asked_threads() {
        Some(n) => n,
        None if BUSY.2.load(Ordering::Relaxed) => WIDE,
        None => 1,
    }
}

/// Threads while the title streams fast: they stand in the host's queue side
/// by side, each for a millisecond or two a page in the world's first seconds.
const WIDE: usize = 16;

/// `VICTORY_MAP_THREADS` as a number; `None`: by how much there is to map.
fn asked_threads() -> Option<usize> {
    static ASKED: LazyLock<Option<usize>> = LazyLock::new(|| {
        match std::env::var("VICTORY_MAP_THREADS") {
            Ok(v) if v.trim() == "auto" => None,
            v => Some(v.ok().and_then(|v| v.trim().parse().ok()).unwrap_or(8).clamp(1, 32)),
        }
    });
    *ASKED
}

/// Of the window under way: when it began, the nanoseconds spent mapping in
/// it; and whether the last one was busy.
static BUSY: (Mutex<Option<std::time::Instant>>, AtomicU64, std::sync::atomic::AtomicBool) =
    (Mutex::new(None), AtomicU64::new(0), std::sync::atomic::AtomicBool::new(false));

/// Counts the time a mapping call of the title's takes, as long as it lives.
/// More than a quarter of a window, and the calls are what loading waits for
/// (one thread of the title's makes them); less than a twentieth, they are
/// not any more.
struct Busy(std::time::Instant);

impl Busy {
    fn begin() -> Busy {
        Busy(std::time::Instant::now())
    }
}

/// Whether mapping is busy after a window of `window` with `spent` in it.
fn busy_after(was: bool, spent: std::time::Duration, window: std::time::Duration) -> bool {
    spent * if was { 20 } else { 4 } > window
}

impl Drop for Busy {
    fn drop(&mut self) {
        const WINDOW: std::time::Duration = std::time::Duration::from_millis(250);
        let now = std::time::Instant::now();
        cost(cost::CALLS_NS, (now - self.0).as_nanos() as u64);
        let all = BUSY.1.fetch_add((now - self.0).as_nanos() as u64, Ordering::Relaxed) + (now - self.0).as_nanos() as u64;
        let mut began = BUSY.0.lock().unwrap_or_else(|e| e.into_inner());
        let window = now - *began.get_or_insert(self.0);
        if window < WINDOW {
            return;
        }
        *began = Some(now);
        BUSY.1.store(0, Ordering::Relaxed);
        let was = BUSY.2.load(Ordering::Relaxed);
        let is = busy_after(was, std::time::Duration::from_nanos(all), window);
        if was != is {
            BUSY.2.store(is, Ordering::Relaxed);
            static SAID: AtomicU64 = AtomicU64::new(0);
            if asked_threads().is_none() && SAID.fetch_add(1, Ordering::Relaxed) < 200 {
                tracing::info!(
                    "title pages are mapped by {} (VICTORY_MAP_THREADS): {:.0} ms of the last {:.0} ms went into mapping calls",
                    if is { "several threads" } else { "the caller alone" },
                    all as f64 / 1e6,
                    window.as_secs_f64() * 1e3
                );
            }
        }
    }
}

/// A page of address space to unmap and, with a slice, map anew.
struct Op {
    at: usize,
    slice: Option<Slice>,
    /// A view of this page alone is there, to be unmapped.
    had: bool,
    /// The placeholder is the page's own (or will be, after the unmapping).
    own: bool,
}

impl Op {
    fn new(at: usize, slice: Option<Slice>) -> Op {
        Op { at, slice, had: false, own: false }
    }

    /// The system calls; whether the host did as asked.
    fn run(&self) -> bool {
        if self.had {
            // SAFETY: a view we mapped; it becomes a placeholder again.
            let gone = counted(Call::Unmap, || unsafe {
                UnmapViewOfFile2(
                    GetCurrentProcess(),
                    MEMORY_MAPPED_VIEW_ADDRESS { Value: self.at as *mut c_void },
                    MEM_PRESERVE_PLACEHOLDER,
                )
            });
            if gone == 0 {
                return false;
            }
        }
        let Some(slice) = &self.slice else {
            return true;
        };
        show(self.at, PAGE, slice, self.own, false)
    }
}

/// Does `ops`, each of another page, side by side: first what is noted of
/// the pages goes, then the system calls, then what is there now is noted.
/// With [`PHYSICAL`] and the change of mapping (`title_memory_remapping`)
/// held.
fn change(physical: &mut Physical, mut ops: Vec<Op>) -> bool {
    for op in &mut ops {
        let there = physical
            .views
            .range(..=op.at)
            .next_back()
            .filter(|(base, view)| op.at < **base + view.0)
            .map(|(base, view)| (*base, view.0));
        match there {
            Some((base, PAGE)) if base == op.at => {
                if let Some((size, slice)) = physical.views.remove(&op.at) {
                    shown_mut().forget(op.at, size, &slice);
                }
                told(|| victory_gpu::title_memory_unmapping(op.at, PAGE));
                physical.holes.remove(&op.at);
                (op.had, op.own) = (true, lean());
            }
            // Part of a larger view.
            Some(_) => {
                unmap(physical, op.at, PAGE);
            }
            None => {}
        }
        if op.had {
            continue;
        }
        op.own = physical.holes.remove(&op.at) == Some(PAGE) && lean();
        if op.slice.is_some() && !op.own {
            // Cut out of a larger placeholder here, not side by side: a
            // thread that maps a piece while others cut the same one can
            // fail.
            split(op.at, PAGE);
            op.own = true;
        }
    }
    let done = pool::all(ops);
    let mut ok = true;
    for (op, done) in done {
        match op.slice {
            Some(slice) if done => {
                // As in `map`.
                shown_mut().note(op.at, PAGE, &slice);
                told(|| {
                    victory_gpu::title_memory_remapped(op.at, PAGE);
                    victory_gpu::title_memory_view(op.at, PAGE, slice.section.handle as usize, (slice.index * PAGE) as u64);
                });
                physical.views.insert(op.at, (PAGE, slice));
            }
            None if op.had && done => {
                physical.holes.insert(op.at, PAGE);
            }
            _ => ok &= done,
        }
    }
    ok
}

/// The threads that do the system calls of [`change`].
mod pool {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, OnceLock};

    use super::{Op, side_by_side};

    struct Batch {
        ops: Vec<(Op, AtomicBool)>,
        next: AtomicUsize,
        /// Ops not done yet.
        left: AtomicUsize,
        over: (Mutex<bool>, Condvar),
    }

    impl Batch {
        /// Does ops until none is left to begin.
        fn work(&self) {
            loop {
                let i = self.next.fetch_add(1, Ordering::Relaxed);
                let Some((op, done)) = self.ops.get(i) else {
                    return;
                };
                done.store(op.run(), Ordering::Release);
                if self.left.fetch_sub(1, Ordering::AcqRel) == 1 {
                    *self.over.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
                    self.over.1.notify_all();
                }
            }
        }
    }

    struct Pool {
        batches: Mutex<VecDeque<Arc<Batch>>>,
        more: Condvar,
    }

    fn pool() -> &'static Pool {
        static POOL: OnceLock<Pool> = OnceLock::new();
        static STARTED: std::sync::Once = std::sync::Once::new();
        let pool = POOL.get_or_init(|| Pool { batches: Mutex::default(), more: Condvar::new() });
        STARTED.call_once(|| {
            let started = (1..super::asked_threads().unwrap_or(super::WIDE).max(side_by_side()))
                .filter(|i| std::thread::Builder::new().name(format!("mapping {i}")).spawn(|| serve(pool)).is_ok())
                .count();
            tracing::info!(started, "threads that map pages side by side (VICTORY_MAP_THREADS=1: none)");
        });
        pool
    }

    fn serve(pool: &Pool) {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
        };
        // SAFETY: a plain call on our own thread.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) };
        let mut batches = pool.batches.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let Some(batch) = batches.front().cloned() else {
                batches = pool.more.wait(batches).unwrap_or_else(|e| e.into_inner());
                continue;
            };
            if batch.next.load(Ordering::Relaxed) >= batch.ops.len() {
                // All begun: nobody needs to see it any more.
                if batches.front().is_some_and(|b| Arc::ptr_eq(b, &batch)) {
                    batches.pop_front();
                }
                continue;
            }
            drop(batches);
            batch.work();
            batches = pool.batches.lock().unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Runs `ops`; each with whether the host did it.
    pub(super) fn all(ops: Vec<Op>) -> Vec<(Op, bool)> {
        // Waking the others takes longer than a few quick calls.
        if ops.len() < 3 {
            return ops
                .into_iter()
                .map(|op| {
                    let done = op.run();
                    (op, done)
                })
                .collect();
        }
        let n = ops.len();
        let batch = Arc::new(Batch {
            ops: ops.into_iter().map(|op| (op, AtomicBool::new(false))).collect(),
            next: AtomicUsize::new(0),
            left: AtomicUsize::new(n),
            over: Default::default(),
        });
        let pool = pool();
        pool.batches.lock().unwrap_or_else(|e| e.into_inner()).push_back(batch.clone());
        if n > side_by_side() {
            pool.more.notify_all();
        } else {
            (1..n).for_each(|_| pool.more.notify_one());
        }
        batch.work();
        let mut over = batch.over.0.lock().unwrap_or_else(|e| e.into_inner());
        while !*over {
            over = batch.over.1.wait(over).unwrap_or_else(|e| e.into_inner());
        }
        drop(over);
        // No worker changes it any more (one may still hold it for a moment).
        batch
            .ops
            .iter()
            .map(|(op, done)| {
                let op = Op { at: op.at, slice: op.slice.clone(), had: op.had, own: op.own };
                (op, done.load(Ordering::Acquire))
            })
            .collect()
    }
}

/// Maps `slice` and the slices after it over `[base, base + len)`, which
/// must lie in one placeholder.
fn map(physical: &mut Physical, base: usize, len: usize, slice: Slice) -> bool {
    map_quietly(physical, base, len, slice, false)
}

/// [`map`]; `quiet` for a caller that has another way if the host refuses.
fn map_quietly(physical: &mut Physical, base: usize, len: usize, slice: Slice, quiet: bool) -> bool {
    if join_views()
        && let Some(done) = map_joined(physical, base, len, &slice)
    {
        return done;
    }
    let own = physical.holes.remove(&base) == Some(len) && lean();
    if !show(base, len, &slice, own, quiet) {
        return false;
    }
    // Noted before the GPU side is told: bytes DirectStorage put in earlier,
    // without notice at these addresses ([`pages_written`]), are there by
    // the time the copies of them go.
    shown_mut().note(base, len, &slice);
    told(|| {
        // New contents, read-write: the GPU side's copies no longer guard it.
        victory_gpu::title_memory_remapped(base, len);
        // What the range shows now, for the GPU side's in-place census.
        victory_gpu::title_memory_view(base, len, slice.section.handle as usize, (slice.index * PAGE) as u64);
    });
    physical.views.insert(base, (len, slice));
    true
}

/// Cuts `[base, base + len)` out of the placeholder it is in (which fails
/// harmlessly when the range is the whole placeholder).
fn split(base: usize, len: usize) {
    // SAFETY: acts only on a placeholder.
    counted(Call::Split, || unsafe {
        VirtualFree(
            base as *mut c_void,
            len,
            MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
        )
    });
}

/// [`map`] as one view with the view that ends at `base` and the view that
/// begins at `base + len`, where those show the section's slices just before
/// and just after and all lies in one window ([`join_views`]); `None` if
/// there is no such neighbour (nothing was done) or the host would not have
/// them as one (the neighbours are as they were), and the new view is the
/// caller's to map by itself.
fn map_joined(physical: &mut Physical, base: usize, len: usize, slice: &Slice) -> Option<bool> {
    let window = base & !(JOIN_WINDOW - 1);
    let end = base + len;
    if len == 0 || end > window + JOIN_WINDOW {
        return None;
    }
    let left = physical
        .views
        .range(..base)
        .next_back()
        .filter(|&(&at, (size, s))| {
            at >= window && at + size == base && Arc::ptr_eq(&s.section, &slice.section) && s.index + size / PAGE == slice.index
        })
        .map(|(&at, (size, s))| (at, *size, s.clone()));
    let right = physical
        .views
        .get(&end)
        .filter(|(size, s)| {
            end + size <= window + JOIN_WINDOW && Arc::ptr_eq(&s.section, &slice.section) && s.index == slice.index + len / PAGE
        })
        .map(|(size, s)| (end, *size, s.clone()));
    if left.is_none() && right.is_none() {
        return None;
    }
    let (from, first) = left.as_ref().map_or((base, slice.clone()), |(at, _, s)| (*at, s.clone()));
    let to = right.as_ref().map_or(end, |(at, size, _)| at + size);
    let neighbours: Vec<(usize, usize, Slice)> = left.into_iter().chain(right).collect();
    let section = slice.section.handle as usize;
    // What the GPU side knows of the neighbours stays, and is left alone
    // until they are there again. Not while the kernel writes into one (a
    // file read of the title's): the new view by itself, then.
    if !told(|| neighbours.iter().all(|(at, size, _)| victory_gpu::title_memory_regrouping(*at, *size))) {
        return None;
    }
    // The new range is split off its placeholder while the neighbours still
    // stand; they become placeholders; the three (or two) are then exactly
    // `[from, to)` and are made one. Only placeholders and views of ours are
    // acted on. (No split where the placeholder is the range's own, left by
    // the view unmapped there: [`known`].)
    let own = known() && physical.holes.get(&base) == Some(&len);
    if !own {
        split(base, len);
    }
    for (at, _, _) in &neighbours {
        // SAFETY: a view we mapped; it becomes a placeholder again.
        counted(Call::Unmap, || unsafe {
            UnmapViewOfFile2(
                GetCurrentProcess(),
                MEMORY_MAPPED_VIEW_ADDRESS { Value: *at as *mut c_void },
                MEM_PRESERVE_PLACEHOLDER,
            )
        });
        physical.views.remove(at);
        count(5, 1, 0);
    }
    physical.holes.remove(&base);
    // SAFETY: joins placeholders only.
    let join = || counted(Call::Join, || unsafe { VirtualFree(from as *mut c_void, to - from, MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS) });
    join();
    let mut shown = show(from, to - from, &first, true, true);
    if !shown && own {
        // The placeholder was not the range's own after all (the title has
        // joined it with others since): split off now, as before.
        split(base, len);
        join();
        shown = show(from, to - from, &first, true, true);
    }
    if !shown {
        // The host would not have them as one: the neighbours as they were
        // (their pages are what they were; one that will not map is gone),
        // and the new view by itself.
        static REFUSED: std::sync::Once = std::sync::Once::new();
        REFUSED.call_once(|| {
            tracing::warn!(
                address = format_args!("{from:#x}"),
                len = to - from,
                error = %std::io::Error::last_os_error(),
                "views were refused as one; mapped apart"
            );
        });
        for (at, size, s) in neighbours {
            if show(at, size, &s, false, false) {
                told(|| victory_gpu::title_memory_regrouped(at, size, section, (s.index * PAGE) as u64, at, 0));
                physical.views.insert(at, (size, s));
            } else {
                shown_mut().forget(at, size, &s);
                told(|| victory_gpu::title_memory_remapped(at, size));
            }
        }
        return None;
    }
    // The neighbours' pages are noted where they were ([`Shown::pages`]);
    // the new ones before the GPU side is told, as in `map`.
    shown_mut().note(base, len, slice);
    told(|| victory_gpu::title_memory_regrouped(from, to - from, section, (first.index * PAGE) as u64, base, len));
    physical.views.insert(from, (to - from, first));
    count(7, 1, physical.views.len());
    Some(true)
}

/// Makes `[base, base + len)` one placeholder where it is several (views
/// unmapped one at a time each leave their own, and a view replaces exactly
/// one), without asking what is there (under Wine each question walks the
/// process's views): the first and last page are split off whatever
/// placeholder reaches past the range, then all within are made one; each
/// call fails harmlessly where there is nothing for it to do. True if the
/// range is one placeholder afterwards as far as the host says.
fn join_unasked(base: usize, len: usize) -> bool {
    // SAFETY: these flags only split and join placeholders.
    unsafe {
        counted(Call::Split, || VirtualFree(base as *mut c_void, PAGE, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER));
        counted(Call::Split, || VirtualFree((base + len - PAGE) as *mut c_void, PAGE, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER));
        counted(Call::Join, || VirtualFree(base as *mut c_void, len, MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS)) != 0
    }
}

/// Whether the title's views are mapped with the host kernel's write
/// tracking (`VICTORY_VIEW_WATCH=1`, and a host that has it: a Wine with
/// `views-writewatch.patch`, where `MapViewOfFile3` takes `MEM_WRITE_WATCH`
/// for a view and `GetWriteWatch` then answers for it). Looked for once, on
/// a view of a section of our own: Windows refuses the mapping, an
/// unpatched Wine maps it and refuses the asking. The GPU side is told, and
/// asks the kernel about such views in place of making them read-only
/// (`victory_gpu::title_memory_views_tracked`).
fn view_watch() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| {
        if std::env::var_os("VICTORY_VIEW_WATCH").is_none_or(|v| v != "1") {
            return false;
        }
        use windows_sys::Win32::System::Memory::GetWriteWatch;
        // SAFETY: a section and a view of our own, let go again below; the
        // asking has room for the view's sixteen pages.
        let tracked = unsafe {
            let section =
                CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE | SEC_COMMIT, 0, PAGE as u32, std::ptr::null());
            if section.is_null() {
                return false;
            }
            let view = MapViewOfFile3(
                section,
                GetCurrentProcess(),
                std::ptr::null(),
                0,
                PAGE,
                MEM_WRITE_WATCH,
                PAGE_READWRITE,
                std::ptr::null_mut(),
                0,
            );
            let mut tracked = false;
            if !view.Value.is_null() {
                view.Value.cast::<u8>().add(0x1000).write_volatile(1);
                let mut found = [std::ptr::null_mut::<c_void>(); 16];
                let (mut count, mut granularity) = (found.len(), 0u32);
                tracked = GetWriteWatch(0, view.Value, PAGE, found.as_mut_ptr(), &mut count, &mut granularity) == 0
                    && count == 1
                    && found[0] as usize == view.Value as usize + 0x1000;
                UnmapViewOfFile2(GetCurrentProcess(), view, 0);
            }
            CloseHandle(section);
            tracked
        };
        if tracked {
            victory_gpu::title_memory_views_tracked();
        } else {
            tracing::warn!("VICTORY_VIEW_WATCH=1, but this host does not track writes to mapped views: the guards as before");
        }
        tracked
    });
    *ON
}

/// The system calls of [`map`]; `own`: the placeholder at `base` is `len`
/// long already; `quiet`: a refusal is the caller's to tell.
fn show(base: usize, len: usize, slice: &Slice, own: bool, quiet: bool) -> bool {
    let watched = if view_watch() { MEM_WRITE_WATCH } else { 0 };
    let map_view = || {
        // SAFETY: maps the section over a placeholder; acts only on one.
        counted(Call::Map, || unsafe {
            MapViewOfFile3(
                slice.section.handle,
                GetCurrentProcess(),
                base as *const c_void,
                (slice.index * PAGE) as u64,
                len,
                MEM_REPLACE_PLACEHOLDER | watched,
                PAGE_READWRITE,
                std::ptr::null_mut(),
                0,
            )
        })
    };
    let mut view = if own { map_view() } else { MEMORY_MAPPED_VIEW_ADDRESS { Value: std::ptr::null_mut() } };
    if view.Value as usize != base {
        cost(cost::REFUSED, own.into());
        split(base, len);
        view = map_view();
    }
    if view.Value as usize != base {
        cost(cost::REFUSED, 1);
    }
    // (Only with the runs of [`wine_switch`], which map runs over pages
    // unmapped singly: with them off, a refusal fails as before.)
    if view.Value as usize != base && len > PAGE && (join_views() || scatter_runs() || alias_runs()) {
        // The range may be several placeholders (a run of pages mapped over
        // pages unmapped singly was refused).
        // SAFETY: reads and restores this thread's error.
        let refused = unsafe { GetLastError() };
        if join_unasked(base, len) {
            view = map_view();
            cost(cost::REFUSED, (view.Value as usize != base).into());
        } else {
            // SAFETY: as above.
            unsafe { SetLastError(refused) };
        }
    }
    if view.Value as usize != base {
        if !quiet {
            tracing::warn!(
                address = format_args!("{base:#x}"),
                len,
                error = %std::io::Error::last_os_error(),
                "mapping physical pages failed (not a placeholder?)"
            );
        }
        return false;
    }
    if !slice.section.commit_shown(slice.index, base, len) {
        // SAFETY: the view just mapped, back to a placeholder.
        unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, MEM_PRESERVE_PLACEHOLDER) };
        return false;
    }
    true
}

/// Commits the section pages a view shows (once per page, whichever view).
fn commit(base: usize, len: usize) -> bool {
    if on_wine() {
        return true; // sections are committed whole there
    }
    // SAFETY: `[base, base + len)` is a view of a reserved section.
    if !unsafe { VirtualAlloc(base as *const c_void, len, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
        return true;
    }
    commit_refused(len, "physical pages", std::io::Error::last_os_error());
    false
}

/// Reports memory the host would not give: at the Windows commit limit to
/// `crashes.log`, since the title usually faults soon after on the null it
/// gets; otherwise as a warning.
fn commit_refused(len: usize, what: &str, error: std::io::Error) {
    if error.raw_os_error() == Some(ERROR_COMMITMENT_LIMIT as i32) {
        let mut info = PERFORMANCE_INFORMATION {
            cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
            ..Default::default()
        };
        // SAFETY: the buffer and size describe a writable PERFORMANCE_INFORMATION.
        let queried = unsafe { K32GetPerformanceInfo(&mut info, info.cb) } != 0;
        victory_loader::log_crash(&commit_limit_message(
            len,
            what,
            &error,
            queried.then_some(&info),
        ));
    } else {
        tracing::warn!(len, %error, "committing {what} failed");
    }
}

fn commit_limit_message(
    len: usize,
    what: &str,
    error: &std::io::Error,
    info: Option<&PERFORMANCE_INFORMATION>,
) -> String {
    let mut message = format!(
        "VIctory: Windows commit limit reached while committing {len} bytes of {what}: {error}.\n"
    );
    if let Some(info) = info {
        use std::fmt::Write;

        let committed = info.CommitTotal * info.PageSize / MIB;
        let limit = info.CommitLimit * info.PageSize / MIB;
        let _ = writeln!(
            message,
            "System commit: {committed} MiB of {limit} MiB in use."
        );
    }
    message.push_str(
        "Close other applications or increase the Windows paging file size, then retry.\n",
    );
    message
}

/// Returns `[base, base + len)` to placeholders; views reaching past it
/// are mapped again outside it.
fn unmap(physical: &mut Physical, base: usize, len: usize) {
    // No bytes: no view to return. The search below would still find the
    // view holding `base` and cut it in two there, and its range of views
    // starting inside would run backwards (which `BTreeMap` panics on).
    if len == 0 {
        return;
    }
    let end = base + len;
    // Views do not overlap: the one holding `base`, then those starting
    // inside (the GPU's page mappings make one view per page).
    let first = physical
        .views
        .range(..=base)
        .next_back()
        .filter(|(at, (size, _))| *at + size > base)
        .map(|(at, _)| *at);
    let hit: Vec<usize> = first
        .into_iter()
        .chain(physical.views.range(base + 1..end).map(|(at, _)| *at))
        .collect();
    // The views that go whole and stand side by side are told to the GPU
    // side as one range before the first is unmapped ([`unmap_runs`]): until
    // the change is done nothing is guarded there, whichever of them still
    // stands.
    let mut told_before: Vec<usize> = Vec::new();
    if hit.len() > 1 && unmap_runs() {
        let whole: Vec<(usize, usize)> = hit
            .iter()
            .filter_map(|at| physical.views.get(at).map(|view| (*at, view.0)))
            .filter(|&(at, size)| at >= base && at + size <= end)
            .collect();
        for run in whole.chunk_by(|a, b| a.0 + a.1 == b.0).filter(|run| run.len() > 1) {
            {
                let mut shown = shown_mut();
                for &(at, size) in run {
                    if let Some((_, slice)) = physical.views.get(&at) {
                        shown.forget(at, size, slice);
                    }
                }
            }
            let (from, to) = (run[0].0, run[run.len() - 1].0 + run[run.len() - 1].1);
            told(|| victory_gpu::title_memory_unmapping(from, to - from));
            cost(cost::TOLD_ONCE, run.len() as u64 - 1);
            told_before.extend(run.iter().map(|view| view.0));
        }
    }
    for at in hit {
        let Some((size, slice)) = physical.views.remove(&at) else {
            continue;
        };
        // What goes of the view, and what stays before and after it.
        let (cut, cut_end) = (at.max(base), (at + size).min(end));
        let stays = [(at, cut - at, slice.index), (cut_end, at + size - cut_end, slice.index + (cut_end - at) / PAGE)];
        let whole = cut == at && cut_end == at + size;
        // With views joined, a page unmapped is most often one of a view of
        // many: what stays of it shows what it showed, and the GPU side
        // keeps what it knows of that (`title_memory_cut`, then
        // `title_memory_regrouped`), where otherwise every copy of it is
        // taken again.
        let keep = !whole && join_views();
        if keep {
            {
                // In one step: who asks where a page is shown
                // ([`pages_written`]) finds what stays all the while.
                let mut shown = shown_mut();
                if size / PAGE > SHORT {
                    shown.forget(at, size, &slice);
                    for (part, bytes, index) in stays {
                        if bytes != 0 {
                            shown.note(part, bytes, &Slice { index, section: slice.section.clone() });
                        }
                    }
                } else {
                    let gone = Slice { index: slice.index + (cut - at) / PAGE, section: slice.section.clone() };
                    shown.forget(cut, cut_end - cut, &gone);
                }
            }
            told(|| {
                victory_gpu::title_memory_cut(
                    at,
                    size,
                    slice.section.handle as usize,
                    (slice.index * PAGE) as u64,
                    cut,
                    cut_end - cut,
                )
            });
        } else if told_before.binary_search(&at).is_err() {
            shown_mut().forget(at, size, &slice);
            told(|| {
                if !whole {
                    // Parts of it stay, and are mapped again below.
                    victory_gpu::title_memory_splitting(at, size);
                }
                victory_gpu::title_memory_unmapping(at, size);
            });
        }
        // SAFETY: a view we mapped; it becomes a placeholder again.
        let gone = counted(Call::Unmap, || unsafe {
            UnmapViewOfFile2(
                GetCurrentProcess(),
                MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: at as *mut c_void,
                },
                MEM_PRESERVE_PLACEHOLDER,
            )
        });
        count(5, 1, physical.views.len());
        census(physical);
        if gone != 0 && whole {
            physical.holes.insert(at, size);
        }
        let mut back = gone != 0;
        for (part, bytes, index) in stays {
            if bytes == 0 {
                continue;
            }
            let part_slice = Slice { index, section: slice.section.clone() };
            if !keep {
                map(physical, part, bytes, part_slice);
            } else if show(part, bytes, &part_slice, false, false) {
                // (By itself, not joined with what is beside it: it shows
                // what it showed.)
                told(|| {
                    victory_gpu::title_memory_regrouped(
                        part,
                        bytes,
                        part_slice.section.handle as usize,
                        (index * PAGE) as u64,
                        part,
                        0,
                    )
                });
                physical.views.insert(part, (bytes, part_slice));
            } else {
                back = false;
                shown_mut().forget(part, bytes, &part_slice);
                told(|| victory_gpu::title_memory_remapped(part, bytes));
            }
            count(6, 1, physical.views.len());
        }
        if keep && back {
            // What stays was cut off the view's placeholder: the rest is
            // the unmapped part's own.
            physical.holes.insert(cut, cut_end - cut);
        }
    }
}

/// The oldest layout we know; newer titles pass other flag values (Minecraft
/// 0x20000001) and read the same leading fields.
#[repr(C)]
#[derive(Debug, Default)]
pub struct WorkingSet {
    pub title_limit: usize,
    pub title_used: usize,
    pub tools_limit: usize,
    pub tools_used: usize,
    pub stack_used: u32,
    pub physical_pages: u32,
    pub gpu_optimal_limit: usize,
    pub gpu_optimal_used: usize,
    pub tools_gpu_optimal_limit: usize,
    pub tools_gpu_optimal_used: usize,
    pub kernel_pool_used: u32,
    pub kernel_pool_limit: u32,
}

/// The title's budget and our process's commit as what it uses. No tools
/// memory: a retail console.
// shortcut: all fields are filled whatever the flags ask for, and use is
// not split by bandwidth class (GPU-optimal use reads 0); track
// allocations per class if a title budgets from that field.
pub extern "system" fn XMemGetWorkingSetStatistics(_which: u32, out: *mut WorkingSet) -> BOOL {
    if out.is_null() {
        return 0;
    }
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    let (gpu_optimal, standard) = victory_gpu::console::current().title_memory();
    let stats = WorkingSet {
        title_limit: gpu_optimal + standard - TRANSFERRED.load(Ordering::Relaxed),
        title_used: counters.PagefileUsage,
        gpu_optimal_limit: gpu_optimal,
        ..Default::default()
    };
    // What the title is told, logged at the first asking and at every
    // 1,024th: "used" is all the process has committed, the layer's own
    // memory and the card's included (27 GB in run 60's gameplay against a
    // limit of 13.5 GB). A title that budgets its streaming from these two
    // figures believes it is far over its limit.
    // shortcut: the figure is not yet the title's own memory alone (its
    // heaps and the pages it mapped); count those if this line shows the
    // title asking.
    static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let asked = ASKED.fetch_add(1, Ordering::Relaxed);
    if asked.is_multiple_of(1024) {
        tracing::info!(
            asked = asked + 1,
            which = _which,
            limit_mb = stats.title_limit >> 20,
            used_mb = stats.title_used >> 20,
            "the title asks how much memory it uses"
        );
    }
    // SAFETY: checked non-null; the title passes room for the structure.
    unsafe { out.write(stats) };
    1
}

/// Hands title memory (standard, in 2 MiB pages) to the system. The other
/// transfers involve tools memory, which a retail console has none of.
pub extern "system" fn XMemTransferMemory(kind: u32, size: usize) -> BOOL {
    const TITLE_TO_SYSTEM: u32 = 0;
    tracing::info!(kind, size, "XMemTransferMemory");
    if size == 0 {
        return 1;
    }
    let Some(size) = size.checked_next_multiple_of(2 * MIB) else {
        return 0;
    };
    let standard = victory_gpu::console::current().title_memory().1;
    let moved = kind == TITLE_TO_SYSTEM
        && TRANSFERRED
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                t.checked_add(size).filter(|&t| t <= standard)
            })
            .is_ok();
    moved.into()
}

/// The title's own `VirtualFree`. What the GPU side noted of the memory goes
/// first: the address may be other memory's the moment it is free.
extern "system" fn title_virtual_free(address: *mut c_void, size: usize, kind: u32) -> BOOL {
    // Splitting and joining placeholders changes nothing they show.
    if kind & (MEM_PRESERVE_PLACEHOLDER | MEM_COALESCE_PLACEHOLDERS) == 0 {
        victory_gpu::title_memory_freed(address as usize, size);
    }
    // SAFETY: the title's own arguments.
    unsafe { VirtualFree(address, size, kind) }
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "xmem",
        dlls: &["xmem.dll", "kernel32.dll", "kernelbase.dll", "api-ms-win-core-memory-*"],
        exports: vec![
            crate::HleExport { name: "VirtualFree", address: title_virtual_free as *const () as usize },
            hle_export!(XMemAlloc),
            hle_export!(XMemFree),
            hle_export!(XMemAllocDefault),
            hle_export!(XMemFreeDefault),
            hle_export!(XMemIsToolsMemAvailable),
            hle_export!(XMemAllocatePhysicalPages),
            hle_export!(XMemFreePhysicalPages),
            hle_export!(XMemMapPhysicalPages),
            hle_export!(XMemMapPhysicalPagesScatter),
            hle_export!(XMemVirtualAlloc),
            hle_export!(XMemVirtualQuery),
            hle_export!(XMemGetWorkingSetStatistics),
            hle_export!(XMemTransferMemory),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(address: *const c_void) -> BasicInformation {
        // SAFETY: plain data; zero is a valid value.
        let mut info: BasicInformation = unsafe { std::mem::zeroed() };
        // SAFETY: a pseudo handle needs no closing.
        let process = unsafe { GetCurrentProcess() };
        assert_eq!(XMemVirtualQuery(process, address, &mut info), 1);
        info
    }

    #[test]
    fn alloc_honours_alignment_and_frees() {
        for (field, align) in [(0, 16), (4, 16), (0xc, 4096), (0x10, 64 * 1024)] {
            let attributes = field << ALIGNMENT_SHIFT | 1 << 29; // a graphics type
            let p = XMemAlloc(100, attributes);
            assert!(!p.is_null());
            assert_eq!(p as usize % align, 0, "field {field:#x}");
            // SAFETY: 100 bytes were allocated.
            unsafe { p.cast::<u8>().write_bytes(0xab, 100) };
            XMemFree(p, attributes);
        }
        let p = XMemAlloc(4096, ALLOC_32BIT);
        assert!(!p.is_null() && (p as usize) < 1 << 32);
        XMemFree(p, ALLOC_32BIT);
    }

    #[test]
    fn virtual_alloc_meets_console_constraints() {
        const GPU_READ_WRITE: u32 = 0x2000;
        let p = XMemVirtualAlloc(
            std::ptr::null_mut(),
            4 * MIB,
            PAGES_2M | MEM_RESERVE | MEM_COMMIT,
            GRAPHICS | 7 << 12, // 40-bit addresses
            PAGE_READWRITE | GPU_READ_WRITE,
        );
        assert!(!p.is_null());
        assert_eq!(p as usize % (2 * MIB), 0);
        assert!((p as usize) < 1 << 40);
        let info = query(p);
        assert_eq!(info.host.State, MEM_COMMIT);
        assert_eq!(info.host.Protect, PAGE_READWRITE);
        assert_eq!(info.flags & GRAPHICS, GRAPHICS);
        // Write-watched, for the GPU side's copies.
        let (mut found, mut count, mut granularity) = ([0usize; 4], 4, 0);
        // SAFETY: room for `count` addresses.
        let watched = unsafe {
            windows_sys::Win32::System::Memory::GetWriteWatch(
                0,
                p,
                4 * MIB,
                found.as_mut_ptr().cast(),
                &mut count,
                &mut granularity,
            )
        };
        assert_eq!(watched, 0);
        // SAFETY: our own reservation.
        assert_ne!(unsafe { VirtualFree(p, 0, MEM_RELEASE) }, 0);
        assert_eq!(query(p).flags, 0);
        // No address field: above the 32-bit range and below 2^40, where
        // the console keeps it.
        let p = XMemVirtualAlloc(
            std::ptr::null_mut(),
            16 << 30,
            MEM_RESERVE,
            GRAPHICS,
            PAGE_NOACCESS,
        );
        assert!(!p.is_null() && (p as usize) >= 1 << 32, "{p:p}");
        assert!((p as usize) + (16 << 30) <= 1 << 40, "{p:p}");
        // SAFETY: our own reservation.
        assert_ne!(unsafe { VirtualFree(p, 0, MEM_RELEASE) }, 0);
    }

    #[test]
    fn transfers_shrink_the_title_budget() {
        let mut before = WorkingSet::default();
        assert_eq!(XMemGetWorkingSetStatistics(0x2000_0001, &mut before), 1);
        assert!(before.title_used > 0);
        assert_eq!(XMemTransferMemory(0, 128 * MIB - 1), 1);
        assert_eq!(XMemTransferMemory(2, MIB), 0, "no tools memory");
        let standard = victory_gpu::console::current().title_memory().1;
        assert_eq!(XMemTransferMemory(0, standard), 0, "over budget");
        let mut after = WorkingSet::default();
        XMemGetWorkingSetStatistics(0x2000_0001, &mut after);
        assert_eq!(before.title_limit - after.title_limit, 128 * MIB);
    }

    #[test]
    fn statistics_have_the_console_layout() {
        assert_eq!(size_of::<WorkingSet>(), 0x50);
        assert_eq!(size_of::<BasicInformation>(), 0x40);
        assert_eq!(std::mem::offset_of!(WorkingSet, gpu_optimal_limit), 0x28);
    }

    #[test]
    fn commit_limit_report_includes_request_and_system_commit() {
        let error = std::io::Error::from_raw_os_error(ERROR_COMMITMENT_LIMIT as i32);
        let info = PERFORMANCE_INFORMATION {
            CommitTotal: 512,
            CommitLimit: 768,
            PageSize: 4096,
            ..Default::default()
        };
        let report = commit_limit_message(4 * MIB, "physical pages", &error, Some(&info));
        assert!(report.contains("4194304 bytes of physical pages"));
        assert!(report.contains("2 MiB of 3 MiB in use"));
        let fallback = commit_limit_message(4 * MIB, "virtual memory", &error, None);
        assert!(fallback.contains("4194304 bytes of virtual memory"));
        for report in [&report, &fallback] {
            assert!(report.contains("os error 1455"));
            assert!(report.contains("Windows paging file size"));
        }
        assert!(!fallback.contains("System commit:"));
    }

    #[test]
    fn physical_pages_map_alias_and_unmap() {
        let mut pages = [0usize; 3];
        let mut n = 3;
        assert_eq!(XMemAllocatePhysicalPages(0, &mut n, pages.as_mut_ptr()), 1);
        assert_eq!(n, 3);
        // Packed into 21 bits as `number >> 4`, as the title keeps them.
        for p in pages {
            assert_eq!((p >> 4 & 0x1f_ffff) << 4, p);
        }
        // Reservations of 64 KiB pages, as titles make them for mapping.
        let reserve = || {
            XMemVirtualAlloc(
                std::ptr::null_mut(),
                3 * PAGE,
                MEM_RESERVE | PAGES_64K,
                0,
                PAGE_READWRITE,
            )
        };
        let (a, b) = (reserve(), reserve());
        assert!(!a.is_null() && !b.is_null());
        let at = |p: *mut c_void, page: usize| p.wrapping_byte_add(page * PAGE);
        assert_eq!(XMemMapPhysicalPages(a, 3, pages.as_ptr()), a);

        // The same page at a second address shows the same bytes.
        assert_eq!(
            XMemMapPhysicalPages(at(b, 1), 1, pages[2..].as_ptr()),
            at(b, 1)
        );
        // SAFETY: both pages are mapped read-write.
        unsafe {
            at(a, 2).cast::<u8>().write(7);
            assert_eq!(at(b, 1).cast::<u8>().read(), 7);
            a.cast::<u8>().write(1);
        }
        // Writes into pages (DirectStorage's) show through their views.
        assert!(write_physical(&pages[2..], 1, &[9]));
        // SAFETY: mapped read-write.
        assert_eq!(unsafe { at(b, 1).cast::<u8>().add(1).read() }, 9);

        // Unmapping the middle page keeps its neighbours.
        assert_eq!(
            XMemMapPhysicalPages(at(a, 1), 1, std::ptr::null()),
            at(a, 1)
        );
        assert_eq!(query(at(a, 1)).host.State, MEM_RESERVE);
        assert_eq!(query(a).host.State, MEM_COMMIT);
        assert_eq!(query(at(a, 2)).host.State, MEM_COMMIT);
        // SAFETY: still mapped.
        assert_eq!(unsafe { at(a, 2).cast::<u8>().read() }, 7);

        // A scattered page lands in the hole.
        let addresses = [at(a, 1)];
        assert_eq!(
            XMemMapPhysicalPagesScatter(addresses.as_ptr(), 1, pages.as_ptr()),
            1
        );
        // SAFETY: page 0 is now mapped at both `a` and `a + PAGE`.
        assert_eq!(unsafe { at(a, 1).cast::<u8>().read() }, 1);
        // Page number 0 unmaps.
        assert_eq!(
            XMemMapPhysicalPagesScatter(addresses.as_ptr(), 1, [0usize].as_ptr()),
            1
        );
        assert_eq!(query(at(a, 1)).host.State, MEM_RESERVE);
        // The GPU's page-mapping copy: what `a + 2 pages` shows, at `b`.
        assert!(map_alias(b as u64, at(a, 2) as u64, PAGE as u64));
        // SAFETY: mapped read-write.
        assert_eq!(unsafe { b.cast::<u8>().read() }, 7);
        assert!(map_alias(b as u64, 0, PAGE as u64));
        assert_eq!(query(b).host.State, MEM_RESERVE);
        assert!(!map_alias(b as u64, at(a, 1) as u64, PAGE as u64));
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
        assert_eq!(XMemIsToolsMemAvailable(), 0);
    }

    /// A thread that reads a mapped page in place, as the GPU side reads a
    /// texture, never meets the moment in which the runtime has taken the
    /// page's view away to map it again: mapping the same pages at the same
    /// address unmaps the view and maps a new one, and without the lock the
    /// reader faults on a page "reserved, never committed" (the crashes of
    /// runs 71 and 74).
    #[test]
    fn a_read_in_place_never_meets_the_gap_of_a_remapping() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut pages = [0usize; 3];
        let mut n = 3;
        assert_eq!(XMemAllocatePhysicalPages(0, &mut n, pages.as_mut_ptr()), 1);
        let a = XMemVirtualAlloc(
            std::ptr::null_mut(),
            3 * PAGE,
            MEM_RESERVE | PAGES_64K,
            0,
            PAGE_READWRITE,
        );
        assert!(!a.is_null());
        assert_eq!(XMemMapPhysicalPages(a, 3, pages.as_ptr()), a);
        // SAFETY: mapped read-write.
        unsafe { a.cast::<u8>().write(1) };
        let first = a as usize;
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let reader = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut reads = 0u64;
                // (Once at least: the mapping thread may be done before this
                // one's first turn.)
                while reads == 0 || !stop.load(Ordering::Relaxed) {
                    let _reading = victory_gpu::title_memory_reading();
                    // SAFETY: the page is mapped whenever no change of
                    // mapping is under way, and none is while `_reading`
                    // is held.
                    assert_eq!(unsafe { (first as *const u8).read_volatile() }, 1);
                    reads += 1;
                }
                reads
            })
        };
        for _ in 0..2000 {
            // The same pages at the same address: the view goes and comes
            // back, with the same bytes.
            assert_eq!(XMemMapPhysicalPages(a, 3, pages.as_ptr()), a);
        }
        stop.store(true, Ordering::Relaxed);
        assert!(reader.join().expect("the reader never faulted") > 0);
        assert_eq!(XMemMapPhysicalPages(a, 3, std::ptr::null()), a);
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
    }

    /// A title thread that reads and writes a page of a view while other
    /// pages of the view are unmapped and mapped again, as the console lets
    /// it: the page was never given up.
    #[test]
    fn a_title_thread_goes_on_using_the_rest_of_a_view() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let pages = allocate(4);
        for _ in 0..200 {
            // A new reservation each time: the pieces of a view cut up stay
            // cut up.
            let a = XMemVirtualAlloc(
                std::ptr::null_mut(),
                4 * PAGE,
                MEM_RESERVE | PAGES_64K,
                0,
                PAGE_READWRITE,
            );
            assert!(!a.is_null());
            assert_eq!(XMemMapPhysicalPages(a, 4, pages.as_ptr()), a);
            let (first, stop) = (a as usize, AtomicBool::new(false));
            // SAFETY: mapped read-write.
            unsafe { (first as *mut u64).write_volatile(0) };
            std::thread::scope(|s| {
                let title = s.spawn(|| {
                    let mut n = 0u64;
                    // SAFETY (the block): the first page is the title's all
                    // the time; a fault would end the process.
                    unsafe {
                        while !stop.load(Ordering::Relaxed) {
                            assert_eq!((first as *const u64).read_volatile(), n);
                            n += 1;
                            (first as *mut u64).write_volatile(n);
                        }
                    }
                    n
                });
                // SAFETY: as above.
                while !title.is_finished() && unsafe { (first as *const u64).read_volatile() } < 100 {}
                for page in [3, 2, 1] {
                    let at = a.wrapping_byte_add(page * PAGE);
                    assert_eq!(XMemMapPhysicalPages(at, 1, std::ptr::null()), at);
                    assert_eq!(XMemMapPhysicalPages(at, 1, pages[page..].as_ptr()), at);
                }
                stop.store(true, Ordering::Relaxed);
                assert!(title.join().expect("the title's thread never faulted") > 0);
            });
            assert_eq!(XMemMapPhysicalPages(a, 4, std::ptr::null()), a);
        }
        assert_eq!(XMemFreePhysicalPages(4, pages.as_ptr()), 1);
    }

    /// Many pages in one call: mapped, moved over each other's addresses,
    /// given twice in one list, copied by the GPU's mappings and unmapped.
    #[test]
    fn a_scatter_of_many_pages_is_what_one_after_the_other_is() {
        const N: usize = 200;
        let pages = allocate(N);
        let a = XMemVirtualAlloc(std::ptr::null_mut(), 2 * N * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE) as usize;
        let at = |k: usize| (a + k * PAGE) as *mut c_void;
        let first = |k: usize| at(k).cast::<u64>();
        for round in 0..4_usize {
            // Every address shows another page each round.
            let to: Vec<*mut c_void> = (0..N).map(|i| at((i + 7 * round) % N)).collect();
            assert_eq!(XMemMapPhysicalPagesScatter(to.as_ptr(), N, pages.as_ptr()), 1);
            for i in 0..N {
                // SAFETY: mapped read-write.
                unsafe {
                    if round > 0 {
                        assert_eq!(first((i + 7 * round) % N).read(), (1000 * (round - 1) + i) as u64);
                    }
                    first((i + 7 * round) % N).write((1000 * round + i) as u64);
                }
            }
        }
        // The GPU's copies of the mappings, in ranges and one by one; a page
        // mapped from one the same call has mapped shows the same.
        let half = (N / 2 * PAGE) as u64;
        let (from, to) = (a as u64, at(N) as u64);
        assert!(map_aliases(&[(to, from, half), (to + half, to, half)]));
        for k in 0..N / 2 {
            // SAFETY: mapped read-write.
            unsafe {
                assert_eq!(first(N + k).read(), first(k).read());
                assert_eq!(first(N + N / 2 + k).read(), first(k).read());
                first(N + k).write(5);
                assert_eq!(first(k).read(), 5);
            }
        }
        // An address twice: the last one counts; page 0 unmaps.
        let to = [at(0), at(1), at(0), at(2), at(3), at(0)];
        let what = [pages[5], pages[6], 0, 0, pages[7], pages[8]];
        assert_eq!(XMemMapPhysicalPagesScatter(to.as_ptr(), 6, what.as_ptr()), 1);
        assert_eq!(query(at(2)).host.State, MEM_RESERVE);
        // SAFETY: mapped read-write; the same page at both.
        unsafe {
            first(0).write(77);
            assert_eq!(XMemMapPhysicalPagesScatter([at(2), at(4), at(6)].as_ptr(), 3, [pages[8]; 3].as_ptr()), 1);
            assert_eq!((first(2).read(), first(4).read(), first(6).read()), (77, 77, 77));
        }
        // A page the title does not own.
        assert_eq!(XMemMapPhysicalPagesScatter([at(0), at(1)].as_ptr(), 2, [pages[0], 1].as_ptr()), 0);
        // All gone, also parts of a larger view.
        assert_eq!(XMemMapPhysicalPages(at(0), 4, std::ptr::null()), at(0));
        // SAFETY: placeholders of ours, nothing mapped in them.
        unsafe { VirtualFree(at(0), 4 * PAGE, MEM_RELEASE | 1 /* MEM_COALESCE_PLACEHOLDERS */) };
        assert_eq!(XMemMapPhysicalPages(at(0), 4, pages.as_ptr()), at(0));
        assert!(map_aliases(&[(a as u64, 0, (2 * N * PAGE) as u64)]));
        assert!((0..2 * N).all(|k| query(at(k)).host.State == MEM_RESERVE));
        assert!(physical().views.range(a..a + 2 * N * PAGE).next().is_none());
        assert_eq!(XMemFreePhysicalPages(N, pages.as_ptr()), 1);
    }

    /// Pages keep what they hold however often they are mapped, moved and
    /// unmapped without a commit or a split of their own.
    #[test]
    fn pages_mapped_again_hold_what_they_held() {
        let pages = allocate(3);
        let a = XMemVirtualAlloc(std::ptr::null_mut(), 8 * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE);
        assert!(!a.is_null());
        let at = |k: usize| a.wrapping_byte_add(k * PAGE);
        for round in 0..6_usize {
            for (k, page) in pages.iter().enumerate() {
                let to = at((round + 2 * k) % 8);
                assert_eq!(XMemMapPhysicalPages(to, 1, page), to);
                let first = to.cast::<u64>();
                // SAFETY: mapped read-write; the far end of the page too.
                unsafe {
                    if round > 0 {
                        assert_eq!(first.read(), (round * 16 + k - 16) as u64);
                        assert_eq!(first.add(PAGE / 8 - 1).read(), k as u64);
                    }
                    first.write((round * 16 + k) as u64);
                    first.add(PAGE / 8 - 1).write(k as u64);
                }
            }
            for k in 0..pages.len() {
                let to = at((round + 2 * k) % 8);
                assert_eq!(XMemMapPhysicalPages(to, 1, std::ptr::null()), to);
                assert_eq!(query(to).host.State, MEM_RESERVE);
            }
            // The title joins what it unmapped: the next view splits it anew.
            if round == 3 {
                // SAFETY: placeholders of ours, nothing mapped in them.
                unsafe {
                    VirtualFree(a, 8 * PAGE, MEM_RELEASE | 1 /* MEM_COALESCE_PLACEHOLDERS */)
                };
            }
        }
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
    }

    #[test]
    fn mapping_no_pages_changes_nothing() {
        let pages = allocate(2);
        let a = XMemVirtualAlloc(
            std::ptr::null_mut(),
            2 * PAGE,
            MEM_RESERVE | PAGES_64K,
            0,
            PAGE_READWRITE,
        );
        assert!(!a.is_null());
        assert_eq!(XMemMapPhysicalPages(a, 2, pages.as_ptr()), a);
        // SAFETY: both pages are mapped read-write.
        unsafe { a.cast::<u8>().write(5) };
        let second = a.wrapping_byte_add(PAGE);

        // A count of 0 at the start of a view, inside it and past it: with
        // pages, without (an unmap), and as the GPU's page-mapping copy.
        for at in [a, second, a.wrapping_byte_add(2 * PAGE)] {
            assert_eq!(XMemMapPhysicalPages(at, 0, pages.as_ptr()), at);
            assert_eq!(XMemMapPhysicalPages(at, 0, std::ptr::null()), at);
            assert!(map_alias(at as u64, a as u64, 0));
            assert!(map_alias(at as u64, 0, 0));
        }

        // The view is still the one view it was.
        let view = physical().views.get(&(a as usize)).map(|v| v.0);
        assert_eq!(view, Some(2 * PAGE));
        assert_eq!(query(a).host.State, MEM_COMMIT);
        assert_eq!(query(second).host.State, MEM_COMMIT);
        // SAFETY: still mapped.
        assert_eq!(unsafe { a.cast::<u8>().read() }, 5);

        // A count of 1 still unmaps that page alone.
        assert_eq!(XMemMapPhysicalPages(second, 1, std::ptr::null()), second);
        assert_eq!(query(second).host.State, MEM_RESERVE);
        assert_eq!(query(a).host.State, MEM_COMMIT);
        assert_eq!(XMemMapPhysicalPages(a, 1, std::ptr::null()), a);
        assert_eq!(XMemFreePhysicalPages(2, pages.as_ptr()), 1);
    }

    /// Pages mapped one at a time at addresses that follow one another keep
    /// their bytes whatever views they are joined into (`join_views`), a
    /// run unmapped from among them leaves the others, a run is mapped over
    /// pages that were unmapped one at a time, and the GPU's page-mapping
    /// copies show a run and one page at every page of a range. With the
    /// switches Wine has on, and without.
    #[test]
    fn pages_mapped_one_at_a_time_keep_their_bytes() {
        const N: usize = 8;
        for joined in [false, true] {
            SWITCHES_IN_TEST.set(Some(joined));
            let pages = allocate(N);
            let reserve = |pages: usize| {
                XMemVirtualAlloc(std::ptr::null_mut(), pages * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE)
            };
            let a = reserve(N);
            assert!(!a.is_null());
            let at = |page: usize| a.wrapping_byte_add(page * PAGE);
            // SAFETY (all reads and writes below): of pages mapped read-write.
            let byte = |page: usize| unsafe { at(page).cast::<u8>().read() };
            // Out of order, so that a page meets a neighbour before it,
            // after it, and on both sides.
            for i in [0usize, 2, 1, 5, 3, 4, 7, 6] {
                assert_eq!(XMemMapPhysicalPages(at(i), 1, pages[i..].as_ptr()), at(i));
                unsafe { at(i).cast::<u8>().write(i as u8 + 1) };
            }
            for i in 0..N {
                assert_eq!(byte(i), i as u8 + 1);
            }
            // Every byte is in a view, and no view is told twice.
            {
                let physical = physical();
                let views: Vec<(usize, usize)> =
                    physical.views.range(a as usize..a as usize + N * PAGE).map(|(at, view)| (*at, view.0)).collect();
                assert_eq!(views.iter().map(|view| view.1).sum::<usize>(), N * PAGE);
                assert!(views.windows(2).all(|pair| pair[0].0 + pair[0].1 == pair[1].0));
                if !joined {
                    assert_eq!(views.len(), N);
                }
            }
            // A run unmapped from among them in one call.
            let addresses = [at(2), at(3), at(4)];
            assert_eq!(XMemMapPhysicalPagesScatter(addresses.as_ptr(), 3, std::ptr::null()), 1);
            for i in [2, 3, 4] {
                assert_eq!(query(at(i)).host.State, MEM_RESERVE);
            }
            for i in [0, 1, 5, 6, 7] {
                assert_eq!(query(at(i)).host.State, MEM_COMMIT);
                assert_eq!(byte(i), i as u8 + 1);
            }
            // And mapped again in one call: the pages are what they were.
            assert_eq!(XMemMapPhysicalPagesScatter(addresses.as_ptr(), 3, pages[2..].as_ptr()), 1);
            for i in 0..N {
                assert_eq!(byte(i), i as u8 + 1);
            }
            // A write through the pages (DirectStorage's) shows in a joined
            // view, and the title is seen to have the page there.
            assert!(write_physical(&pages[3..], 1, &[9]));
            assert_eq!(unsafe { at(3).cast::<u8>().add(1).read() }, 9);
            assert_eq!(shown_at(&pages[3..], 0, PAGE), vec![(at(3) as usize, PAGE)]);
            // A run mapped over pages that were unmapped one at a time (each
            // left a placeholder of its own).
            for i in [6, 4, 5] {
                assert_eq!(XMemMapPhysicalPages(at(i), 1, std::ptr::null()), at(i));
                assert_eq!(query(at(i)).host.State, MEM_RESERVE);
                assert!(shown_at(&pages[i..], 0, PAGE).is_empty());
            }
            assert_eq!(XMemMapPhysicalPages(at(4), 3, pages[4..].as_ptr()), at(4));
            for i in 0..N {
                assert_eq!(byte(i), i as u8 + 1);
                assert_eq!(shown_at(&pages[i..], 0, PAGE), vec![(at(i) as usize, PAGE)]);
            }
            // The GPU's page-mapping copies: a run of pages, then one page
            // at every page of a range, then the range unmapped.
            let b = reserve(3);
            assert!(!b.is_null());
            let other = |k: usize| b.wrapping_byte_add(k * PAGE);
            assert!(map_aliases(&[(b as u64, at(1) as u64, 3 * PAGE as u64)]));
            for k in 0..3 {
                assert_eq!(unsafe { other(k).cast::<u8>().read() }, k as u8 + 2);
            }
            let one: Vec<(u64, u64, u64)> = (0..3).map(|k| (other(k) as u64, at(6) as u64, PAGE as u64)).collect();
            assert!(map_aliases(&one));
            for k in 0..3 {
                assert_eq!(unsafe { other(k).cast::<u8>().read() }, 7);
            }
            assert!(map_alias(b as u64, 0, 3 * PAGE as u64));
            assert_eq!(query(b).host.State, MEM_RESERVE);
            assert_eq!(XMemMapPhysicalPages(a, N, std::ptr::null()), a);
            assert!(physical().views.range(a as usize..a as usize + N * PAGE).next().is_none());
            assert_eq!(XMemFreePhysicalPages(N, pages.as_ptr()), 1);
        }
        SWITCHES_IN_TEST.set(None);
    }

    /// Pages mapped again where they are stay as they are (`map_same`): the
    /// view is the view it was, and only the pages that change are mapped
    /// anew, with the bytes of all of them what they were.
    #[test]
    fn pages_that_show_what_they_are_to_show_stay() {
        const N: usize = 6;
        SWITCHES_IN_TEST.set(Some(true));
        let pages = allocate(N);
        let reserve = |pages: usize| {
            XMemVirtualAlloc(std::ptr::null_mut(), pages * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE)
        };
        let a = reserve(N);
        assert!(!a.is_null());
        let at = |page: usize| a.wrapping_byte_add(page * PAGE);
        // SAFETY (all reads and writes below): of pages mapped read-write.
        let byte = |page: usize| unsafe { at(page).cast::<u8>().read() };
        assert_eq!(XMemMapPhysicalPages(a, N, pages.as_ptr()), a);
        for i in 0..N {
            unsafe { at(i).cast::<u8>().write(i as u8 + 1) };
        }
        let views = || -> Vec<(usize, usize)> {
            physical().views.range(a as usize..a as usize + N * PAGE).map(|(at, view)| (*at, view.0)).collect()
        };
        assert_eq!(views(), vec![(a as usize, N * PAGE)]);
        // The same pages: nothing is asked of the host.
        let same = COST[cost::SAME_CALLS].load(Ordering::Relaxed);
        assert_eq!(XMemMapPhysicalPages(a, N, pages.as_ptr()), a);
        assert_eq!(XMemMapPhysicalPages(at(2), 3, pages[2..].as_ptr()), at(2));
        assert!(COST[cost::SAME_CALLS].load(Ordering::Relaxed) >= same + 2);
        assert_eq!(views(), vec![(a as usize, N * PAGE)]);
        for i in 0..N {
            assert_eq!(byte(i), i as u8 + 1);
        }
        // The last two the other way round: the first four stay.
        let mut turned = pages.clone();
        turned.swap(4, 5);
        assert_eq!(XMemMapPhysicalPages(a, N, turned.as_ptr()), a);
        assert_eq!(views(), vec![(a as usize, 4 * PAGE), (at(4) as usize, PAGE), (at(5) as usize, PAGE)]);
        for i in 0..4 {
            assert_eq!(byte(i), i as u8 + 1);
        }
        assert_eq!((byte(4), byte(5)), (6, 5));
        // The GPU's page-mapping copy of the same pages, twice.
        let b = reserve(2);
        assert!(!b.is_null());
        for _ in 0..2 {
            assert!(map_alias(b as u64, at(1) as u64, 2 * PAGE as u64));
            // SAFETY: mapped read-write.
            unsafe {
                assert_eq!(b.cast::<u8>().read(), 2);
                assert_eq!(b.wrapping_byte_add(PAGE).cast::<u8>().read(), 3);
            }
            assert_eq!(physical().views.get(&(b as usize)).map(|view| view.0), Some(2 * PAGE));
        }
        // A range mapped from a part of itself goes as it did: what it was
        // mapped from is gone by then.
        assert!(!map_alias(at(1) as u64, at(1) as u64, PAGE as u64));
        assert_eq!(query(at(1)).host.State, MEM_RESERVE);
        assert!(map_alias(b as u64, 0, 2 * PAGE as u64));
        assert_eq!(XMemMapPhysicalPages(a, N, std::ptr::null()), a);
        assert_eq!(XMemFreePhysicalPages(N, pages.as_ptr()), 1);
        SWITCHES_IN_TEST.set(None);
    }

    /// A page-mapping copy of the GPU's whose entries follow one another
    /// shows what its entries show one after the other, as one call
    /// (`alias_batch`) and without: pages in a row as one view, an entry
    /// mapped from what an entry before it mapped, an entry whose source is
    /// not mapped, and entries that unmap.
    #[test]
    fn a_page_mapping_copy_in_a_row_is_what_its_entries_are() {
        const N: usize = 8;
        for batched in [false, true] {
            SWITCHES_IN_TEST.set(Some(batched));
            let pages = allocate(N);
            let reserve = |pages: usize| {
                XMemVirtualAlloc(std::ptr::null_mut(), pages * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE)
            };
            // (Twice the pages, so that one window of joined views holds
            // all of `b` whatever its address.)
            let (a, b, hole) = (reserve(N), reserve(2 * N), reserve(1));
            assert!(!a.is_null() && !b.is_null() && !hole.is_null());
            let b = b.wrapping_byte_add((b as usize).next_multiple_of(N * PAGE) - b as usize);
            let (from, to) = (|page: usize| a as u64 + (page * PAGE) as u64, |page: usize| b as u64 + (page * PAGE) as u64);
            // SAFETY (all reads and writes below): of pages mapped read-write.
            let byte = |page: usize| unsafe { (to(page) as *const u8).read() };
            assert_eq!(XMemMapPhysicalPages(a, N, pages.as_ptr()), a);
            for i in 0..N {
                unsafe { (from(i) as *mut u8).write(i as u8 + 1) };
            }
            let order = [0usize, 1, 2, 5, 4, 7, 7, 3];
            let entries: Vec<(u64, u64, u64)> = order.iter().enumerate().map(|(k, &page)| (to(k), from(page), PAGE as u64)).collect();
            assert!(map_aliases(&entries));
            for (k, &page) in order.iter().enumerate() {
                assert_eq!(byte(k), page as u8 + 1);
            }
            let views = || -> Vec<usize> {
                physical().views.range(b as usize..b as usize + N * PAGE).map(|(_, view)| view.0 / PAGE).collect()
            };
            if batched {
                assert_eq!(views(), vec![3, 1, 1, 1, 1, 1]);
            }
            // The same again changes nothing.
            assert!(map_aliases(&entries));
            for (k, &page) in order.iter().enumerate() {
                assert_eq!(byte(k), page as u8 + 1);
            }
            // An entry mapped from what the entry before it mapped.
            assert!(map_aliases(&[(to(0), from(4), PAGE as u64), (to(1), to(0), PAGE as u64)]));
            assert_eq!((byte(0), byte(1), byte(2)), (5, 5, 3));
            // A source that is not mapped: the call fails there.
            let missing = [
                (to(0), from(0), PAGE as u64),
                (to(1), from(1), PAGE as u64),
                (to(2), hole as u64, PAGE as u64),
                (to(3), from(3), PAGE as u64),
            ];
            assert!(!map_aliases(&missing));
            if batched {
                assert_eq!((byte(0), byte(1)), (1, 2));
                assert_eq!(query(to(2) as *const c_void).host.State, MEM_RESERVE);
                assert_eq!(byte(3), 6);
            }
            // Entries that unmap, in a row.
            let unmaps: Vec<(u64, u64, u64)> = (0..N).map(|k| (to(k), 0, PAGE as u64)).collect();
            assert!(map_aliases(&unmaps));
            assert!((0..N).all(|k| query(to(k) as *const c_void).host.State == MEM_RESERVE));
            assert!(views().is_empty());
            for i in 0..N {
                // SAFETY: mapped read-write.
                assert_eq!(unsafe { (from(i) as *const u8).read() }, i as u8 + 1);
            }
            assert_eq!(XMemMapPhysicalPages(a, N, std::ptr::null()), a);
            assert_eq!(XMemFreePhysicalPages(N, pages.as_ptr()), 1);
        }
        SWITCHES_IN_TEST.set(None);
    }

    /// What a title that maps and unmaps pages all the time costs the
    /// workers that write others: 24 threads look pages up, fill one and
    /// tell of it, as a request does, while one maps.
    /// `cargo test --release -p victory-hle --lib map_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "a benchmark"]
    fn map_bench() {
        use std::sync::atomic::AtomicU64;
        use std::time::{Duration, Instant};
        let pages = allocate(1024);
        let a = XMemVirtualAlloc(std::ptr::null_mut(), 1024 * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE) as usize;
        for mapping in [false, true] {
            let (done, waited, worst, maps) =
                (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));
            let until = Instant::now() + Duration::from_secs(3);
            std::thread::scope(|s| {
                if mapping {
                    s.spawn(|| {
                        let at: Vec<*mut c_void> = (0..256).map(|i| (a + i * PAGE) as *mut c_void).collect();
                        while Instant::now() < until {
                            assert_eq!(XMemMapPhysicalPagesScatter(at.as_ptr(), 256, pages.as_ptr()), 1);
                            assert_eq!(XMemMapPhysicalPagesScatter(at.as_ptr(), 256, std::ptr::null()), 1);
                            maps.fetch_add(512, Ordering::Relaxed);
                        }
                    });
                }
                for t in 0..24 {
                    let (pages, done, waited, worst) = (&pages, &done, &waited, &worst);
                    s.spawn(move || {
                        let mut i = 256 + t * 16;
                        while Instant::now() < until {
                            let page = &pages[i..=i];
                            i = if i == 256 + t * 16 + 15 { 256 + t * 16 } else { i + 1 };
                            let began = Instant::now();
                            assert!(pages_known(page, 0, PAGE));
                            let mut window = page_window(page, 0, PAGE).unwrap();
                            let looked = began.elapsed();
                            window.bytes().fill(t as u8);
                            let began = Instant::now();
                            pages_written(page, 0, PAGE);
                            let took = (looked + began.elapsed()).as_nanos() as u64;
                            waited.fetch_add(took, Ordering::Relaxed);
                            worst.fetch_max(took, Ordering::Relaxed);
                            done.fetch_add(1, Ordering::Relaxed);
                        }
                    });
                }
            });
            let n = done.load(Ordering::Relaxed).max(1);
            println!(
                "mapping {mapping}: {:.0} requests/s, {:.1} us of each on lookups and notices (worst {:.1} ms); {:.0} maps/s",
                n as f64 / 3.0,
                waited.load(Ordering::Relaxed) as f64 / n as f64 / 1e3,
                worst.load(Ordering::Relaxed) as f64 / 1e6,
                maps.load(Ordering::Relaxed) as f64 / 3.0
            );
        }
        assert_eq!(XMemFreePhysicalPages(1024, pages.as_ptr()), 1);
    }

    /// `n` pages of one new allocation.
    fn allocate(n: usize) -> Vec<usize> {
        let (mut pages, mut given) = (vec![0; n], n);
        assert_eq!(
            XMemAllocatePhysicalPages(0, &mut given, pages.as_mut_ptr()),
            1
        );
        assert_eq!(given, n);
        pages
    }

    /// The bytes of one page, through a view of our own.
    fn page_bytes(page: usize) -> Vec<u8> {
        let slice = page_table().pages.get(&page).cloned().expect("known page");
        View::map(&slice, 1).expect("mapped").bytes().to_vec()
    }

    #[test]
    fn runs_group_consecutive_pages_of_one_allocation() {
        let (a, b) = (allocate(3), allocate(1));
        let lengths = |pages: &[usize], offset, len| {
            runs(pages, offset, len).map(|r| r.iter().map(|&(_, n)| n).collect::<Vec<_>>())
        };
        assert_eq!(lengths(&a, 0, 3 * PAGE), Some(vec![3]));
        // Only the pages the bytes reach.
        assert_eq!(lengths(&a, PAGE + 5, 10), Some(vec![1]));
        assert_eq!(lengths(&a, PAGE - 1, 2), Some(vec![2]));
        assert_eq!(lengths(&a, 0, 0), Some(vec![]));
        // Out of order, or across allocations: one run each.
        assert_eq!(
            lengths(&[a[0], a[2], a[1]], 0, 3 * PAGE),
            Some(vec![1, 1, 1])
        );
        assert_eq!(lengths(&[a[1], a[2], b[0]], 0, 3 * PAGE), Some(vec![2, 1]));
        // A slice that follows on in another section starts a run.
        assert_eq!(lengths(&[b[0], a[1]], 0, 2 * PAGE), Some(vec![1, 1]));
        // Too few pages for the bytes, or an unknown one.
        assert_eq!(lengths(&a, 0, 3 * PAGE + 1), None);
        assert_eq!(lengths(&[a[0], 0x10], 0, 2 * PAGE), None);
        assert_eq!(lengths(&a, usize::MAX, 2), None);
        assert_eq!(XMemFreePhysicalPages(3, a.as_ptr()), 1);
        assert_eq!(XMemFreePhysicalPages(1, b.as_ptr()), 1);
    }

    #[test]
    fn writes_cross_runs_and_allocations() {
        let (a, b) = (allocate(2), allocate(1));
        let pages = [a[0], a[1], b[0]];
        let data: Vec<u8> = (0..2 * PAGE + 64).map(|i| (i * 7 % 251) as u8).collect();
        let offset = PAGE - 32;
        // Past the third page: the second run is too short.
        assert!(!write_physical(&pages, offset, &data));
        assert!(write_physical(&pages, offset, &data[..2 * PAGE]));
        let seen: Vec<u8> = pages.iter().flat_map(|&p| page_bytes(p)).collect();
        assert!(seen[..offset].iter().all(|&b| b == 0), "before the offset");
        assert_eq!(seen[offset..offset + 2 * PAGE], data[..2 * PAGE]);
        assert!(seen[offset + 2 * PAGE..].iter().all(|&b| b == 0), "after");
        assert!(
            !write_physical(&[a[0], 0x10], 0, &data[..PAGE + 1]),
            "unknown page"
        );
        assert_eq!(XMemFreePhysicalPages(2, a.as_ptr()), 1);
        assert_eq!(XMemFreePhysicalPages(1, b.as_ptr()), 1);
    }

    /// Pages say where the title has them mapped, byte for byte (after
    /// fork PR #571's test; its runs are whole pages).
    #[test]
    fn pages_say_where_they_are_mapped() {
        let pages = allocate(3);
        let reserve = |n: usize| {
            let at = XMemVirtualAlloc(std::ptr::null_mut(), n * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE);
            assert!(!at.is_null());
            at
        };
        // `b` keeps a page unmapped at either end, so that no run at `b`
        // ever touches one at `a`, wherever the host puts the two.
        let (a, b) = (reserve(3), reserve(4));
        assert!(shown_at(&pages, 0, 3 * PAGE).is_empty(), "mapped nowhere");
        // All three at `a`; the last two at `b`, from its second page on.
        assert_eq!(XMemMapPhysicalPages(a, 3, pages.as_ptr()), a);
        let second = b.wrapping_byte_add(PAGE);
        assert_eq!(XMemMapPhysicalPages(second, 2, pages[1..].as_ptr()), second);
        let (a, second) = (a as usize, second as usize);
        let sorted = |mut runs: Vec<(usize, usize)>| {
            runs.sort_unstable();
            runs
        };
        assert_eq!(shown_at(&pages, 0, PAGE), vec![(a, PAGE)]);
        // A request for part of a page names those bytes alone.
        assert_eq!(shown_at(&pages, 0x4000, 0x3000), vec![(a + 0x4000, 0x3000)]);
        // Across two pages, shown at two addresses each: one run at each.
        assert_eq!(
            shown_at(&pages, PAGE + 1, PAGE),
            sorted(vec![(a + PAGE + 1, PAGE), (second + 1, PAGE)])
        );
        // A page unmapped in the middle of a view: the rest still shows.
        let middle = (a + PAGE) as *mut c_void;
        assert_eq!(XMemMapPhysicalPages(middle, 1, std::ptr::null()), middle);
        assert_eq!(
            shown_at(&pages, 0, 3 * PAGE),
            sorted(vec![(a, PAGE), (a + 2 * PAGE, PAGE), (second, 2 * PAGE)])
        );
        // Too few pages for the bytes: nothing (the write fails too).
        assert!(shown_at(&pages, 0, 3 * PAGE + 1).is_empty());
        for (at, n) in [(a, 3), (second, 2)] {
            let at = at as *mut c_void;
            assert_eq!(XMemMapPhysicalPages(at, n, std::ptr::null()), at);
        }
        assert!(shown_at(&pages, 0, 3 * PAGE).is_empty());
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
    }

    #[test]
    fn mapping_is_busy_from_a_quarter_of_the_time_to_a_twentieth() {
        let ms = std::time::Duration::from_millis;
        assert!(!busy_after(false, ms(60), ms(250)) && busy_after(false, ms(70), ms(250)));
        assert!(busy_after(true, ms(15), ms(250)) && !busy_after(true, ms(12), ms(250)));
        // A window without a call, then one short one.
        assert!(!busy_after(true, ms(5), ms(4000)));
    }

    #[test]
    fn page_windows_cover_one_run_in_place() {
        let (a, b) = (allocate(3), allocate(1));
        let mut window = page_window(&a, PAGE + 3, PAGE).expect("one run");
        assert_eq!(window.bytes().len(), PAGE);
        window.bytes().fill(0x5a);
        drop(window);
        let (second, third) = (page_bytes(a[1]), page_bytes(a[2]));
        assert!(second[..3].iter().all(|&b| b == 0));
        assert!(second[3..].iter().all(|&b| b == 0x5a));
        assert!(third[..3].iter().all(|&b| b == 0x5a));
        assert!(third[3..].iter().all(|&b| b == 0));
        // Several runs, or an unknown page: no window.
        assert!(page_window(&[a[0], b[0]], 0, PAGE + 1).is_none());
        assert!(page_window(&[a[1], a[0]], 0, PAGE + 1).is_none());
        assert!(page_window(&[0x10], 0, 1).is_none());
        // One page of several is still one run.
        assert!(page_window(&[a[0], b[0]], 0, PAGE).is_some());
        assert_eq!(XMemFreePhysicalPages(3, a.as_ptr()), 1);
        assert_eq!(XMemFreePhysicalPages(1, b.as_ptr()), 1);
    }
}
