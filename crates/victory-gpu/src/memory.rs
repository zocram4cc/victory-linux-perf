//! Title memory by address, for translated shaders that read and write
//! through raw 40-bit addresses (`victory_shader::emit::SLOT_MEMORY`: the
//! console's ray-tracing structure builder and the programs that walk its
//! structures; the structures themselves lie in plain title memory, in no
//! buffer object).
//!
//! Title memory below 128 GiB is mirrored into reserved 2 GiB GPU window
//! buffers (`CreateReservedResource` + `UpdateTileMappings`), so a shader
//! reaches any 40-bit address directly as `(window_index, offset_in_window)`
//! without a per-access directory lookup:
//!
//! - per host view heap, a fixed run of `1 + 2 * MEMORY_WINDOWS` (129) raw
//!   UAV slots starting at `base` (`USER_MEMORY_BASE`): slot `base` is a null
//!   raw view, `base + 1 ..= base + 64` are the 64 read windows of 2 GiB, and
//!   `base + 65 ..= base + 128` are the 64 write windows of 2 GiB;
//! - active 64 KiB title pages get physical 64 KiB tiles from default-heap
//!   buffer pools (`ID3D12Heap` with `ALLOW_ONLY_BUFFERS`), mapped into the
//!   corresponding read window and (when writable by address) into the
//!   corresponding write window at the same physical tile;
//! - closed title buffers (not yet given to the builder as a UAV) are mapped
//!   only in the read windows and left unmapped in the write windows, so stray
//!   builder stores are dropped in hardware;
//! - `SLOT_MEMORY` binds a 256 KiB residency bitmap (1 bit per 64 KiB page)
//!   used only on cold all-zero / builder paths to distinguish resident zero
//!   bytes from unmapped pages and report missing page numbers in
//!   `SLOT_FAULTS`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use victory_shader::emit::{MEMORY_FAULTS, MEMORY_PAGES, MEMORY_WINDOW_BYTES, MEMORY_WINDOWS};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::core::Interface;

use crate::descriptor::host::HostHeap;
use crate::host::{HostList, host};
use crate::shadow::Shadow;

/// A 64 KiB page / D3D12 buffer tile.
const PAGE: u64 = 1 << 16;
/// Bytes per physical `ID3D12Heap` in the tile pool (64 MiB = 1,024 tiles).
const ARENA: u64 = 64 << 20;
/// Tiles per physical `ID3D12Heap`.
const HEAP_TILES: u16 = (ARENA / PAGE) as u16;
/// Bytes per reserved window buffer (2 GiB = 32,768 tiles).
const WINDOW_BYTES: u64 = MEMORY_WINDOW_BYTES;
/// Number of 2 GiB windows covering `< 128 GiB`.
const WINDOW_COUNT: usize = MEMORY_WINDOWS as usize;
/// View-heap slots reserved per `HostHeap`:
/// `[null_srv, null_uav, read_srv[0..64], read_uav[0..64], write_uav[0..64]]`.
const HEAP_SLOTS: u32 = 2 + 3 * MEMORY_WINDOWS;
/// `u32` words in the 256 KiB residency bitmap (`SLOT_MEMORY`).
const BITMAP_WORDS: usize = (MEMORY_PAGES as usize) / 32;
/// Bytes in the 256 KiB residency bitmap.
const BITMAP_BYTES: u64 = (BITMAP_WORDS * 4) as u64;
/// Bytes per staging upload buffer chunk (16 MiB).
const STAGE_CHUNK: u64 = 16 << 20;
/// Number of 64 KiB slots in the small-buffer readback staging buffer (4 MiB).
const SMALL_SLOTS: usize = 64;
/// Frames a freed packed-structure tile waits before reuse.
const RETIRE_FRAMES: u64 = 8;
/// What is compared at a time on the CPU.
const PIECE: usize = 4096;
/// Pages held across arenas and mirrored title buffers, at most (4 GiB).
const MOST: usize = 4 << 14;
/// Pages of title memory with small title buffers in them, at most.
const WINDOWS: usize = 512;
/// The biggest packed structure taken for one.
const PACKED: u64 = 64 << 20;
/// For so many frames what a page holds may still be on its way.
const YOUNG: u64 = 4;
/// Free pages there are, at least, when a list is recorded.
const SPARE: usize = 1024;
/// A packed structure not named for so many frames is another one.
const AGAIN: u64 = 8;
/// From a root view; from one that is written (a structure being built).
const VIEW: u64 = 256 << 10;
const BUILT: u64 = 4 << 20;
/// Maximum initial span mapped at once for a single title buffer.
const BUFFER_INIT_CAP: u64 = 64 << 20;
/// The biggest title buffer whose builder results come back.
const BACK: u64 = 256 << 10;
/// Pages taken from the faults, of one frame and in all, at most.
const FAULTS: usize = 128;
const FAULTS_ALL: usize = 2048;
/// Frames over which every page is looked at once.
const SWEEP: usize = 128;

/// What a draw or dispatch does with title memory by address.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Use {
    Reads,
    /// Stores by address, as the structure builder; `back`: its results in
    /// small title buffers come back.
    Builds { back: bool },
}

/// Physical 64 MiB tile heap on the GPU plus CPU reference bytes of its tiles.
struct TileHeap {
    heap: ID3D12Heap,
    cpu: Box<[u8]>,
}

/// Arena-specific ownership tracking for a page outside title buffers.
#[derive(Clone, Copy)]
struct ArenaMeta {
    owners: u32,
    theirs: bool,
}

/// A 64 KiB title page mirrored into the reserved windows.
struct Resident {
    heap: u16,
    tile: u16,
    writable: bool,
    /// Write-watch sequence after which CPU bytes were last merged (0: never).
    taken: u64,
    /// Frame in which `bring` last checked this page during recording.
    looked: u64,
    /// Frame in which this page was mapped.
    born: u64,
    /// Present for arena pages outside title buffers (and not small windows).
    arena: Option<ArenaMeta>,
    /// Index in `Memory::buffers` if this page lies inside a title buffer.
    buffer: Option<u32>,
}

/// Key in [`HostList::used`] marking a list that reads or builds title memory by address.
pub(crate) const READ_KEY: usize = usize::MAX - 0x6d;

/// A registered title buffer that has pages mirrored in the reserved windows.
struct TitleBuffer {
    shadow: Arc<Shadow>,
    open: bool,
    pages: Vec<u64>,
    refreshed_frame: u64,
}

/// A packed structure the builder came to.
struct Packed {
    frame: u64,
    len: u64,
    header: u64,
}

/// Pending `UpdateTileMappings` operation on one 64 KiB tile of a window.
#[derive(Clone, Copy)]
struct TileMapOp {
    win: u8,
    write: bool,
    win_tile: u16,
    target: Option<(u16, u16)>,
}

/// Pending `CopyBufferRegion` from an active staging chunk into a read window.
#[derive(Clone, Copy)]
struct StagedCopy {
    win: u8,
    dst_offset: u64,
    chunk_idx: usize,
    src_offset: u64,
    len: u64,
}

/// Reusable GPU upload / readback transport owned by `Memory`.
struct Transport {
    fence: ID3D12Fence,
    ticket: u64,
    free_chunks: Vec<(ID3D12Resource, *mut u8)>,
    in_flight_chunks: VecDeque<(u64, Vec<(ID3D12Resource, *mut u8)>)>,
    in_flight_kept: VecDeque<(u64, Vec<ID3D12Resource>)>,
    cur_chunks: Vec<(ID3D12Resource, *mut u8, usize)>,
    copies: Vec<StagedCopy>,
    free_lists: Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)>,
    in_flight_lists: VecDeque<(u64, ID3D12CommandAllocator, ID3D12GraphicsCommandList)>,
    /// Shared readback buffer (`SMALL_SLOTS * PAGE` bytes) for `small` results.
    small_buf: Option<(ID3D12Resource, *mut u8)>,
    small_free: Vec<usize>,
    /// `(fence_or_sub, is_host_sub, slot_idx, page)`
    small_in_flight: VecDeque<(u64, bool, usize, u64)>,
}

impl Transport {
    fn new(device: &ID3D12Device) -> Option<Self> {
        // SAFETY: plain D3D12 fence creation.
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
        Some(Self {
            fence,
            ticket: 0,
            free_chunks: Vec::new(),
            in_flight_chunks: VecDeque::new(),
            in_flight_kept: VecDeque::new(),
            cur_chunks: Vec::new(),
            copies: Vec::new(),
            free_lists: Vec::new(),
            in_flight_lists: VecDeque::new(),
            small_buf: None,
            small_free: (0..SMALL_SLOTS).rev().collect(),
            small_in_flight: VecDeque::new(),
        })
    }

    fn reclaim(&mut self) {
        // SAFETY: plain fence query on live fence.
        let completed = unsafe { self.fence.GetCompletedValue() };
        while self.in_flight_chunks.front().is_some_and(|(t, _)| *t <= completed) {
            if let Some((_, chunks)) = self.in_flight_chunks.pop_front() {
                if self.free_chunks.len() < 4 {
                    self.free_chunks.extend(chunks);
                }
            }
        }
        while self.in_flight_kept.front().is_some_and(|(t, _)| *t <= completed) {
            self.in_flight_kept.pop_front();
        }
        while self.in_flight_lists.front().is_some_and(|(t, _, _)| *t <= completed) {
            if let Some((_, alloc, list)) = self.in_flight_lists.pop_front() {
                self.free_lists.push((alloc, list));
            }
        }
    }

    fn alloc_stage(&mut self, bytes: &[u8]) -> Option<(usize, u64)> {
        if bytes.is_empty() {
            return None;
        }
        let need = bytes.len().next_multiple_of(4);
        if need as u64 > STAGE_CHUNK {
            return None;
        }
        let fits = self
            .cur_chunks
            .last()
            .is_some_and(|(_, _, used)| *used + need <= STAGE_CHUNK as usize);
        if !fits {
            self.reclaim();
            let (res, ptr) = match self.free_chunks.pop() {
                Some(pair) => pair,
                None => {
                    let host = host()?;
                    let (res, ptr) = host.shared_buffer(STAGE_CHUNK)?;
                    crate::host::name(&res, || "title memory by address: upload chunk".into());
                    (res, ptr)
                }
            };
            self.cur_chunks.push((res, ptr, 0));
        }
        let idx = self.cur_chunks.len() - 1;
        let (_, ptr, used) = &mut self.cur_chunks[idx];
        let offset = *used;
        // SAFETY: `offset + bytes.len() <= offset + need <= STAGE_CHUNK` inside our mapped buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.add(offset), bytes.len());
        }
        *used += need;
        Some((idx, offset as u64))
    }

    fn stage_window_write(&mut self, page: u64, offset_in_page: usize, bytes: &[u8]) {
        let Some((chunk_idx, src_offset)) = self.alloc_stage(bytes) else {
            return;
        };
        let win = (page >> 15) as u8;
        let dst_offset = (page & 0x7FFF) * PAGE + offset_in_page as u64;
        let len = bytes.len() as u64;
        if let Some(last) = self.copies.last_mut()
            && last.win == win
            && last.chunk_idx == chunk_idx
            && last.dst_offset + last.len == dst_offset
            && last.src_offset + last.len == src_offset
        {
            last.len += len;
            return;
        }
        self.copies.push(StagedCopy {
            win,
            dst_offset,
            chunk_idx,
            src_offset,
            len,
        });
    }

    fn take_list(&mut self, device: &ID3D12Device) -> Option<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)> {
        self.reclaim();
        if let Some((alloc, list)) = self.free_lists.pop() {
            // SAFETY: fence for this allocator/list has completed (`reclaim`).
            let ok = unsafe { alloc.Reset().and_then(|()| list.Reset(&alloc, None)) };
            if ok.is_ok() {
                return Some((alloc, list));
            }
        }
        // SAFETY: plain command allocator and command list creation on live device.
        unsafe {
            let alloc: ID3D12CommandAllocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .ok()?;
            let list: ID3D12GraphicsCommandList = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None)
                .ok()?;
            crate::host::name(&list, || "title memory by address: sync list".into());
            Some((alloc, list))
        }
    }

    fn ensure_small_buf(&mut self) -> Option<(ID3D12Resource, *mut u8)> {
        if let Some((res, ptr)) = &self.small_buf {
            return Some((res.clone(), *ptr));
        }
        let host = host()?;
        let (res, ptr) = host.shared_buffer((SMALL_SLOTS as u64) * PAGE)?;
        crate::host::name(&res, || "title memory by address: small readback".into());
        self.small_buf = Some((res.clone(), ptr));
        Some((res, ptr))
    }
}

struct Memory {
    /// 256 KiB residency bitmap (`SLOT_MEMORY`): CPU words, GPU buffer, GPU VA, dirty word span.
    bitmap_cpu: Box<[u32]>,
    bitmap_gpu: ID3D12Resource,
    bitmap_va: u64,
    bitmap_dirty: Option<(usize, usize)>,
    faults: (ID3D12Resource, *mut u32, u64),
    /// Reserved 2 GiB read and write window buffers, created on first page in that window.
    read_windows: [Option<ID3D12Resource>; WINDOW_COUNT],
    write_windows: [Option<ID3D12Resource>; WINDOW_COUNT],
    read_mask: u64,
    write_mask: u64,
    /// Physical 64 MiB default heaps and their CPU reference mirrors.
    tile_heaps: Vec<TileHeap>,
    /// Free `(heap_idx, tile_idx)` pairs ready to assign.
    free: Vec<(u16, u16)>,
    /// Freed tiles waiting a few frames before reuse: `(frame_freed, heap_idx, tile_idx)`.
    retired_tiles: VecDeque<(u64, u16, u16)>,
    /// Pending `UpdateTileMappings` operations flushed before submissions execute.
    pending_maps: Vec<TileMapOp>,
    /// All mapped 64 KiB title pages.
    resident: HashMap<u64, Resident>,
    /// Arena page numbers (subset of `resident` with `arena.is_some()`), for `SWEEP` and stats.
    arena_pages: usize,
    order: Vec<u64>,
    sweep: usize,
    /// Registered title buffers mirrored into windows, and map from `Shadow` pointer to index.
    buffers: Vec<TitleBuffer>,
    buffer_numbers: HashMap<usize, u32>,
    buffers_checked_sub: u64,
    /// Small-buffer pages (`<= BACK`) written by the builder and awaiting GPU -> CPU readback:
    /// `page -> submission_when_marked`.
    small_dirty: BTreeMap<u64, u64>,
    /// Count of small-buffer window pages outside full buffer copies.
    windows: usize,
    /// Packed structures the builder came to.
    packed: HashMap<u64, Packed>,
    packed_order: Vec<u64>,
    packed_sweep: usize,
    /// Pages taken from faults.
    faulted: usize,
    /// Per host view heap (by address): `(base_slot, bound_read_mask, bound_write_mask)`.
    heaps: HashMap<usize, (u32, u64, u64)>,
    /// Active submission-time `land(at)` queues per open `HostList` (cleared when flushed).
    list_lands: HashMap<usize, Arc<Mutex<Vec<u64>>>>,
    transport: Transport,
    frame: u64,
    refused: HashSet<u64>,
    stats: Stats,
    #[cfg(feature = "diagnostics")]
    dump: dump::Dump,
    #[cfg(feature = "diagnostics")]
    trace: trace::Trace,
}

/// Since the last report.
#[derive(Default)]
struct Stats {
    from: Option<(std::time::Instant, u64)>,
    uses: [u64; 2],
    faults: u64,
    unknown: u64,
    back: u64,
    looked: u64,
    changed: u64,
    packed: u64,
    packed_bytes: u64,
    gone: u64,
    came: [u64; 4],
    micros: u64,
}

// SAFETY: raw pointers inside `Memory` belong to D3D12 buffers owned by `Memory`
// and are only accessed while holding `MEMORY`'s mutex.
unsafe impl Send for Memory {}

static MEMORY: Mutex<Option<Memory>> = Mutex::new(None);

/// What a draw binds.
pub(crate) struct Ready {
    pub(crate) directory: u64,
    pub(crate) faults: u64,
    /// The view heap slot of the null view; read windows are `1..=64` further,
    /// write windows `65..=128` further.
    pub(crate) base: u32,
}

fn make_reserved_window(device: &ID3D12Device, w: usize, write: bool) -> Option<ID3D12Resource> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: WINDOW_BYTES,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: valid reserved buffer description; out pointer is ours.
    if let Err(e) = unsafe {
        device.CreateReservedResource(&desc, D3D12_RESOURCE_STATE_COMMON, None, &mut made)
    } {
        tracing::warn!(error = %e, window = w, write, "title memory by address: no reserved window buffer");
        return None;
    }
    let res = made?;
    crate::host::name(&res, || {
        format!(
            "title memory by address: window {w} ({})",
            if write { "write" } else { "read" }
        )
    });
    Some(res)
}

fn transition_barrier(
    buffer: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: borrowed raw interface pointer used only for the immediate `ResourceBarrier` call.
                pResource: unsafe { std::mem::transmute_copy(buffer) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

impl Memory {
    fn new() -> Option<Memory> {
        let host = host()?;
        let bitmap_cpu = vec![0u32; BITMAP_WORDS].into_boxed_slice();
        let bitmap_gpu = match host.uav_buffer(BITMAP_BYTES) {
            Some(buf) => buf,
            None => host.shared_buffer(BITMAP_BYTES)?.0,
        };
        crate::host::name(&bitmap_gpu, || "title memory by address: residency bitmap".into());
        // SAFETY: live D3D12 buffer.
        let bitmap_va = unsafe { bitmap_gpu.GetGPUVirtualAddress() };
        let (faults, data) = host.shared_buffer(u64::from(MEMORY_FAULTS) * 4)?;
        crate::host::name(&faults, || "title memory by address: faults".into());
        // SAFETY: freshly mapped buffer of `MEMORY_FAULTS * 4` bytes.
        unsafe { std::ptr::write_bytes(data, 0, MEMORY_FAULTS as usize * 4) };
        // SAFETY: live D3D12 buffer.
        let faults_va = unsafe { faults.GetGPUVirtualAddress() };
        let transport = Transport::new(&host.device)?;
        tracing::info!(
            windows = WINDOW_COUNT,
            window_mb = WINDOW_BYTES >> 20,
            "title memory by address: reserved 2 GiB windows active"
        );
        Some(Memory {
            bitmap_cpu,
            bitmap_gpu,
            bitmap_va,
            bitmap_dirty: Some((0, BITMAP_WORDS)),
            faults: (faults, data.cast::<u32>(), faults_va),
            read_windows: std::array::from_fn(|_| None),
            write_windows: std::array::from_fn(|_| None),
            read_mask: 0,
            write_mask: 0,
            tile_heaps: Vec::new(),
            free: Vec::new(),
            retired_tiles: VecDeque::new(),
            pending_maps: Vec::new(),
            resident: HashMap::new(),
            arena_pages: 0,
            order: Vec::new(),
            sweep: 0,
            buffers: Vec::new(),
            buffer_numbers: HashMap::new(),
            buffers_checked_sub: 0,
            small_dirty: BTreeMap::new(),
            windows: 0,
            packed: HashMap::new(),
            packed_order: Vec::new(),
            packed_sweep: 0,
            faulted: 0,
            heaps: HashMap::new(),
            list_lands: HashMap::new(),
            transport,
            frame: u64::MAX,
            refused: HashSet::new(),
            stats: Stats::default(),
            #[cfg(feature = "diagnostics")]
            dump: Default::default(),
            #[cfg(feature = "diagnostics")]
            trace: Default::default(),
        })
    }

    fn set_bitmap(&mut self, page: u64, on: bool) {
        if page >= u64::from(MEMORY_PAGES) {
            return;
        }
        let word = (page >> 5) as usize;
        let mask = 1u32 << (page & 31);
        let prev = self.bitmap_cpu[word];
        let next = if on { prev | mask } else { prev & !mask };
        if next != prev {
            self.bitmap_cpu[word] = next;
            self.bitmap_dirty = Some(match self.bitmap_dirty {
                Some((lo, hi)) => (lo.min(word), hi.max(word + 1)),
                None => (word, word + 1),
            });
        }
    }

    fn ensure_window(&mut self, w: usize, write: bool) -> bool {
        if w >= WINDOW_COUNT {
            return false;
        }
        let slot = if write {
            &mut self.write_windows[w]
        } else {
            &mut self.read_windows[w]
        };
        if slot.is_some() {
            return true;
        }
        let Some(host) = host() else {
            return false;
        };
        let Some(res) = make_reserved_window(&host.device, w, write) else {
            return false;
        };
        *slot = Some(res);
        if write {
            self.write_mask |= 1u64 << w;
        } else {
            self.read_mask |= 1u64 << w;
        }
        true
    }

    fn queue_map(&mut self, page: u64, write: bool, target: Option<(u16, u16)>) -> bool {
        let w = (page >> 15) as usize;
        if target.is_some() && !self.ensure_window(w, write) {
            return false;
        }
        if target.is_none() {
            let exists = if write {
                self.write_windows[w].is_some()
            } else {
                self.read_windows[w].is_some()
            };
            if !exists {
                return true;
            }
        }
        self.pending_maps.push(TileMapOp {
            win: w as u8,
            write,
            win_tile: (page & 0x7FFF) as u16,
            target,
        });
        true
    }

    fn reclaim_retired_tiles(&mut self) {
        while self
            .retired_tiles
            .front()
            .is_some_and(|&(freed_at, _, _)| self.frame >= freed_at.saturating_add(RETIRE_FRAMES))
        {
            if let Some((_, heap, tile)) = self.retired_tiles.pop_front() {
                self.free.push((heap, tile));
            }
        }
    }

    /// Allocates another 64 MiB default-memory `ID3D12Heap` of buffer tiles.
    fn another(&mut self) -> bool {
        if self.tile_heaps.len() * (HEAP_TILES as usize) >= MOST {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                tracing::warn!(MOST, "title memory by address: no more pages are taken; others read zeros")
            });
            return false;
        }
        let Some(host) = host() else {
            return false;
        };
        if let Some([budget, used, ..]) = host.video_memory()
            && used + ARENA >= budget / 100 * 97
        {
            return false;
        }
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: ARENA,
            Properties: D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            },
            Alignment: 0,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS | crate::host::not_resident(),
        };
        let mut made: Option<ID3D12Heap> = None;
        // SAFETY: valid heap description; out pointer is ours.
        if let Err(e) = unsafe { host.device.CreateHeap::<ID3D12Heap>(&desc, &mut made) } {
            tracing::warn!(error = %e, "title memory by address: CreateHeap failed");
            return false;
        }
        let Some(heap) = made else {
            return false;
        };
        if let Err(e) =
            crate::host::resident(&host.device, &heap).and_then(|()| crate::host::mapped(&host.device))
        {
            tracing::warn!(error = %e, "title memory by address: tile heap residency failed");
            return false;
        }
        let Ok(heap_idx) = u16::try_from(self.tile_heaps.len()) else {
            return false;
        };
        let cpu = vec![0u8; ARENA as usize].into_boxed_slice();
        self.tile_heaps.push(TileHeap { heap, cpu });
        self.free
            .extend((0..HEAP_TILES).rev().map(|t| (heap_idx, t)));
        true
    }

    fn take_tile(&mut self) -> Option<(u16, u16)> {
        self.reclaim_retired_tiles();
        if let Some(pair) = self.free.pop() {
            return Some(pair);
        }
        if !self.another() {
            return None;
        }
        self.free.pop()
    }

    fn possible(&mut self, page: u64) -> bool {
        if page < 0x1_0000 || page >= u64::from(MEMORY_PAGES) - 1 || self.refused.contains(&page) {
            return false;
        }
        let at = (page * PAGE) as usize;
        let ours = crate::texture::committed(at as *const u8, PAGE as usize)
            && crate::watch::writes(at, at + PAGE as usize, true).is_some();
        if !ours {
            self.refused.insert(page);
            self.stats.unknown += 1;
        }
        ours
    }

    fn register_buffer(&mut self, shadow: Arc<Shadow>) -> Option<(u32, bool)> {
        let key = Arc::as_ptr(&shadow) as usize;
        if let Some(&idx) = self.buffer_numbers.get(&key) {
            return Some((idx, false));
        }
        // SAFETY: live D3D12 resource.
        let flags = unsafe { shadow.buffer.GetDesc() }.Flags;
        if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
            return None;
        }
        let idx = u32::try_from(self.buffers.len()).ok()?;
        shadow.set_in_memory();
        self.buffers.push(TitleBuffer {
            shadow,
            open: false,
            pages: Vec::new(),
            refreshed_frame: 0,
        });
        self.buffer_numbers.insert(key, idx);
        Some((idx, true))
    }

    fn insert_resident(
        &mut self,
        page: u64,
        writable: bool,
        arena: Option<ArenaMeta>,
        buffer: Option<u32>,
    ) -> bool {
        if self.resident.contains_key(&page) {
            return true;
        }
        let Some((heap, tile)) = self.take_tile() else {
            return false;
        };
        if !self.queue_map(page, false, Some((heap, tile))) {
            self.free.push((heap, tile));
            return false;
        }
        if writable && !self.queue_map(page, true, Some((heap, tile))) {
            let _ = self.queue_map(page, false, None);
            self.free.push((heap, tile));
            return false;
        }
        let off = (tile as usize) * (PAGE as usize);
        self.tile_heaps[heap as usize].cpu[off..off + PAGE as usize].fill(0);
        self.set_bitmap(page, true);
        if arena.is_some() {
            self.arena_pages += 1;
            self.order.push(page);
        }
        if let Some(b) = buffer {
            let buf = &mut self.buffers[b as usize];
            buf.pages.push(page);
            if !buf.open && buf.shadow.written() {
                let (bs, _) = buf.shadow.span();
                let off = (page * PAGE).saturating_sub(bs) as usize;
                buf.shadow.note_mirror_dirty(off, off + PAGE as usize);
            }
        }
        self.resident.insert(
            page,
            Resident {
                heap,
                tile,
                writable,
                taken: 0,
                looked: u64::MAX,
                born: self.frame,
                arena,
                buffer,
            },
        );
        true
    }

    /// Gives `page` a physical tile in the reserved windows if it has none and
    /// lies in committed title GPU memory.
    fn map(&mut self, page: u64) -> bool {
        if self.resident.contains_key(&page) {
            return true;
        }
        if !self.possible(page) {
            return false;
        }
        let start = page * PAGE;
        if crate::shadow::in_buffer(start, PAGE).is_some() {
            let Some(shadow) = crate::shadow::find(start, PAGE) else {
                return false;
            };
            let Some((buf_idx, fresh)) = self.register_buffer(shadow.clone()) else {
                return false;
            };
            let open = self.buffers[buf_idx as usize].open;
            if fresh {
                let (first, len) = shadow.span();
                let (span_from, span_to) = if len <= BUFFER_INIT_CAP {
                    (first.next_multiple_of(PAGE), first + len)
                } else {
                    (
                        start.saturating_sub(16 << 20).max(first.next_multiple_of(PAGE)),
                        (start + BUFFER_INIT_CAP).min(first + len),
                    )
                };
                let mut at = span_from;
                while at + PAGE <= span_to && at / PAGE < u64::from(MEMORY_PAGES) - 1 {
                    let p = at / PAGE;
                    if !self.resident.contains_key(&p)
                        && crate::texture::committed(at as *const u8, PAGE as usize)
                    {
                        let _ = self.insert_resident(p, open, None, Some(buf_idx));
                    }
                    at += PAGE;
                }
            }
            if !self.resident.contains_key(&page) {
                return self.insert_resident(page, open, None, Some(buf_idx));
            }
            return true;
        }
        if self.windows < WINDOWS && crate::shadow::small_buffer_in(start, PAGE, BACK) {
            if self.insert_resident(page, true, None, None) {
                self.windows += 1;
                return true;
            }
            return false;
        }
        self.insert_resident(
            page,
            true,
            Some(ArenaMeta {
                owners: 0,
                theirs: false,
            }),
            None,
        )
    }

    /// Releases pages held solely for the departed packed structure at `address`.
    fn gone(&mut self, address: u64) {
        let Some(packed) = self.packed.remove(&address) else {
            return;
        };
        for page in address / PAGE..=(address + packed.len - 1) / PAGE {
            let free_it = match self.resident.get_mut(&page).and_then(|r| r.arena.as_mut()) {
                Some(meta) => {
                    meta.owners = meta.owners.saturating_sub(1);
                    meta.owners == 0 && meta.theirs
                }
                None => false,
            };
            if free_it && let Some(removed) = self.resident.remove(&page) {
                self.arena_pages = self.arena_pages.saturating_sub(1);
                self.set_bitmap(page, false);
                let _ = self.queue_map(page, false, None);
                if removed.writable {
                    let _ = self.queue_map(page, true, None);
                }
                self.retired_tiles
                    .push_back((self.frame, removed.heap, removed.tile));
            }
        }
        self.stats.gone += 1;
    }

    fn there(&self, address: u64) -> bool {
        let mut header = [0u8; 8];
        crate::texture::committed(address as *const u8, 8) && {
            crate::host::copy_mapped(header.as_mut_ptr(), address as *const u8, 8, false);
            self.packed
                .get(&address)
                .is_some_and(|p| p.header == u64::from_le_bytes(header))
        }
    }

    fn look_again(&mut self) {
        if !self.order.is_empty() {
            for _ in 0..self.order.len().div_ceil(SWEEP) {
                self.sweep = (self.sweep + 1) % self.order.len();
                let page = self.order[self.sweep];
                if self.resident.get(&page).is_some_and(|r| r.arena.is_some()) {
                    self.bring(page, page + 1, false);
                } else {
                    self.order.swap_remove(self.sweep);
                    if self.order.is_empty() {
                        break;
                    }
                }
            }
        }
        if !self.packed_order.is_empty() {
            for _ in 0..self.packed_order.len().div_ceil(SWEEP) {
                self.packed_sweep = (self.packed_sweep + 1) % self.packed_order.len();
                let address = self.packed_order[self.packed_sweep];
                if !self.packed.contains_key(&address) {
                    self.packed_order.swap_remove(self.packed_sweep);
                    if self.packed_order.is_empty() {
                        break;
                    }
                } else if !self.there(address) {
                    self.gone(address);
                }
            }
        }
    }

    /// Opens a title buffer so builder stores by address reach its tiles in the
    /// write windows too.
    fn opens(&mut self, address: u64) {
        let page = address / PAGE;
        if !self.map(page) {
            return;
        }
        let Some(buf_idx) = self.resident.get(&page).and_then(|r| r.buffer) else {
            return;
        };
        if self.buffers[buf_idx as usize].open {
            return;
        }
        self.buffers[buf_idx as usize].open = true;
        self.buffers[buf_idx as usize].shadow.keep_stores();
        let (first, len) = self.buffers[buf_idx as usize].shadow.span();
        tracing::info!(
            start = format_args!("{first:#x}"),
            len,
            "title memory by address: the structure builder may store in a title buffer"
        );
        let pages = self.buffers[buf_idx as usize].pages.clone();
        for p in pages {
            if let Some(&Resident {
                heap,
                tile,
                writable: false,
                ..
            }) = self.resident.get(&p)
                && self.queue_map(p, true, Some((heap, tile)))
                && let Some(r) = self.resident.get_mut(&p)
            {
                r.writable = true;
            }
        }
    }

    fn ready(&mut self, address: u64, len: u64) {
        let first = address / PAGE;
        let mut end = first;
        while end * PAGE < address + len && self.map(end) {
            end += 1;
        }
        self.bring(first, end, false);
    }

    /// Merges CPU writes on `[first, end)` into their tiles and stages only
    /// changed dwords (or the full 64 KiB page on first upload).
    fn bring(&mut self, first: u64, end: u64, force_recheck: bool) {
        if first >= end {
            return;
        }
        let frame = self.frame;
        if !force_recheck
            && !(first..end).any(|page| self.resident.get(&page).is_some_and(|r| r.looked != frame))
        {
            return;
        }
        let started = std::time::Instant::now();
        let answer = crate::watch::writes((first * PAGE) as usize, (end * PAGE) as usize, true);
        let mut title = [0u8; PIECE];
        for page in first..end {
            let (heap, tile, never) = {
                let Some(res) = self.resident.get_mut(&page) else {
                    continue;
                };
                if !force_recheck && res.looked == frame {
                    continue;
                }
                res.looked = frame;
                self.stats.looked += 1;
                let wrote = answer.as_ref().map(|(_, w)| {
                    w.binary_search_by_key(&(page as usize), |w| w.0)
                        .map_or(0, |i| w[i].1)
                });
                let now = answer.as_ref().map_or(1, |a| a.0);
                let stale = res.taken == 0 || wrote.is_none_or(|w| w > res.taken);
                if !stale {
                    continue;
                }
                let never = std::mem::replace(&mut res.taken, now.max(1)) == 0;
                (res.heap, res.tile, never)
            };
            let base_off = (tile as usize) * (PAGE as usize);
            if never {
                for piece in (0..PAGE as usize).step_by(PIECE) {
                    let from = (page * PAGE) as usize + piece;
                    crate::host::copy_mapped(title.as_mut_ptr(), from as *const u8, PIECE, false);
                    let ours = &mut self.tile_heaps[heap as usize].cpu[base_off + piece..base_off + piece + PIECE];
                    ours.copy_from_slice(&title);
                }
                self.stats.changed += PAGE;
                let full_start = base_off;
                let bytes = self.tile_heaps[heap as usize].cpu[full_start..full_start + PAGE as usize].to_vec();
                self.transport.stage_window_write(page, 0, &bytes);
            } else {
                for piece in (0..PAGE as usize).step_by(PIECE) {
                    let from = (page * PAGE) as usize + piece;
                    crate::host::copy_mapped(title.as_mut_ptr(), from as *const u8, PIECE, false);
                    let span = {
                        let ours =
                            &mut self.tile_heaps[heap as usize].cpu[base_off + piece..base_off + piece + PIECE];
                        merge_span(&title, ours)
                    };
                    if let Some((lo, hi, dwords)) = span {
                        self.stats.changed += (dwords as u64) * 4;
                        let slice = self.tile_heaps[heap as usize].cpu
                            [base_off + piece + lo..base_off + piece + hi]
                            .to_vec();
                        self.transport.stage_window_write(page, piece + lo, &slice);
                    }
                }
            }
        }
        self.stats.micros += started.elapsed().as_micros() as u64;
    }

    /// Ensures all mapped title-buffer pages pick up any CPU writes made right
    /// up to submission time (including multi-threaded `Upload::new` writes).
    fn bring_buffers(&mut self) {
        let sub = crate::host::submissions().0;
        if sub != 0 && self.buffers_checked_sub == sub {
            return;
        }
        self.buffers_checked_sub = sub;
        let mut pages: Vec<u64> = Vec::new();
        for buf in &self.buffers {
            if buf.pages.is_empty() || (!buf.open && buf.shadow.written()) {
                continue;
            }
            pages.extend_from_slice(&buf.pages);
        }
        if pages.is_empty() {
            return;
        }
        pages.sort_unstable();
        pages.dedup();
        let first = pages[0];
        let end = pages[pages.len() - 1] + 1;
        let Some((now, wrote_list)) =
            crate::watch::writes((first * PAGE) as usize, (end * PAGE) as usize, true)
        else {
            return;
        };
        let mut stale_pages: Vec<u64> = Vec::new();
        for page in pages {
            let Some(res) = self.resident.get(&page) else {
                continue;
            };
            let wrote = wrote_list
                .binary_search_by_key(&(page as usize), |w| w.0)
                .map_or(0, |i| wrote_list[i].1);
            if res.taken == 0 || wrote > res.taken {
                stale_pages.push(page);
            }
        }
        let _ = now;
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        for p in stale_pages {
            match ranges.last_mut() {
                Some(last) if last.1 == p => last.1 = p + 1,
                _ => ranges.push((p, p + 1)),
            }
        }
        for (a, b) in ranges {
            self.bring(a, b, true);
        }
    }

    fn packed_at(&self, address: u64) -> Option<(u64, u64)> {
        if address & 0xFF != 0 {
            return None;
        }
        let mut header = [0u8; 8];
        crate::host::copy_mapped(header.as_mut_ptr(), address as *const u8, 8, false);
        let header = u64::from_le_bytes(header);
        Some((packed_len(header)?, header))
    }

    fn land(&mut self, address: u64) {
        let page = address / PAGE;
        if !crate::texture::committed((page * PAGE) as *const u8, PAGE as usize) {
            return;
        }
        let Some((len, header)) = self.packed_at(address) else {
            for p in page..=(address + VIEW - 1) / PAGE {
                if self.resident.get(&p).is_some_and(|r| r.born + YOUNG >= self.frame) {
                    self.take(p, 0, PAGE as usize, false);
                }
            }
            return;
        };
        if let Some(known) = self.packed.get_mut(&address) {
            let soon = known.frame + AGAIN >= self.frame;
            known.frame = self.frame;
            if known.header == header && soon {
                return;
            }
            if known.header != header {
                self.gone(address);
            }
        }
        let last = (address + len - 1) / PAGE;
        if !self.packed.contains_key(&address) {
            self.packed.insert(
                address,
                Packed {
                    frame: self.frame,
                    len,
                    header,
                },
            );
            self.packed_order.push(address);
            for p in page..=last {
                let new = !self.resident.contains_key(&p);
                if new && !self.place(p) {
                    continue;
                }
                if let Some(meta) = self.resident.get_mut(&p).and_then(|r| r.arena.as_mut()) {
                    meta.owners += 1;
                    meta.theirs |= new;
                }
            }
            self.stats.packed_bytes += len;
        }
        for p in page..=last {
            let from = address.max(p * PAGE) - p * PAGE;
            let to = (address + len).min((p + 1) * PAGE) - p * PAGE;
            self.take(p, from as usize, to as usize, true);
        }
        self.stats.packed += 1;
    }

    /// Maps an arena page at submission time using already-allocated free tiles
    /// (or a newly grown default heap) and uploads its initial bytes.
    fn place(&mut self, page: u64) -> bool {
        if page < 0x1_0000
            || page >= u64::from(MEMORY_PAGES) - 1
            || !crate::texture::committed((page * PAGE) as *const u8, PAGE as usize)
            || crate::shadow::in_buffer(page * PAGE, PAGE).is_some()
        {
            return false;
        }
        if !self.insert_resident(
            page,
            true,
            Some(ArenaMeta {
                owners: 0,
                theirs: false,
            }),
            None,
        ) {
            return false;
        }
        self.take(page, 0, PAGE as usize, true);
        true
    }

    fn take(&mut self, page: u64, from: usize, to: usize, whole: bool) {
        let Some(&Resident { heap, tile, .. }) = self.resident.get(&page) else {
            return;
        };
        let from = from & !3;
        let to = to.next_multiple_of(4).min(PAGE as usize);
        if from >= to {
            return;
        }
        let base_off = (tile as usize) * (PAGE as usize);
        let mut title = [0u8; PIECE];
        let mut at = from;
        let mut dirty_range: Option<(usize, usize)> = if whole { Some((from, to)) } else { None };
        while at < to {
            let n = (PIECE - at % PIECE).min(to - at);
            crate::host::copy_mapped(
                title.as_mut_ptr(),
                (page as usize * PAGE as usize + at) as *const u8,
                n,
                false,
            );
            let ours = &mut self.tile_heaps[heap as usize].cpu[base_off + at..base_off + at + n];
            if *ours != title[..n] {
                ours.copy_from_slice(&title[..n]);
                dirty_range = Some(match dirty_range {
                    Some((lo, hi)) => (lo.min(at), hi.max(at + n)),
                    None => (at, at + n),
                });
            }
            at += n;
        }
        if let Some((lo, hi)) = dirty_range {
            let bytes = self.tile_heaps[heap as usize].cpu[base_off + lo..base_off + hi].to_vec();
            self.transport.stage_window_write(page, lo, &bytes);
        }
    }

    fn land_small_results(&mut self) {
        // SAFETY: plain fence query on live fence.
        let fence_done = unsafe { self.transport.fence.GetCompletedValue() };
        let (_, sub_done) = crate::host::submissions();
        let Some((_, base_ptr)) = self.transport.small_buf else {
            return;
        };
        let mut remaining = VecDeque::new();
        while let Some((target, is_sub, slot_idx, page)) = self.transport.small_in_flight.pop_front() {
            let ready = if is_sub {
                sub_done >= target
            } else {
                fence_done >= target
            };
            if !ready {
                remaining.push_back((target, is_sub, slot_idx, page));
                continue;
            }
            if let Some(&Resident { heap, tile, .. }) = self.resident.get(&page) {
                let base_off = (tile as usize) * (PAGE as usize);
                // SAFETY: `slot_idx < SMALL_SLOTS`, GPU copy to this slot has completed.
                let gpu_bytes = unsafe {
                    std::slice::from_raw_parts(
                        base_ptr.add(slot_idx * (PAGE as usize)).cast_const(),
                        PAGE as usize,
                    )
                };
                let ours = &mut self.tile_heaps[heap as usize].cpu[base_off..base_off + PAGE as usize];
                let page_addr = (page * PAGE) as usize;
                for (i, (g, o)) in gpu_bytes.chunks_exact(4).zip(ours.chunks_exact_mut(4)).enumerate() {
                    if g != o {
                        o.copy_from_slice(g);
                        let dst = page_addr + i * 4;
                        crate::watch::landing(dst, 4);
                        crate::host::write_mapped(dst as *mut u8, g.as_ptr(), 4);
                    }
                }
            }
            self.transport.small_free.push(slot_idx);
        }
        self.transport.small_in_flight = remaining;
    }

    fn flush_tile_mappings(&mut self, queue: &ID3D12CommandQueue) {
        if self.pending_maps.is_empty() {
            return;
        }
        // Keep only the latest mapping per `(win, write, win_tile)`.
        let mut latest: BTreeMap<(u8, bool, u16), Option<(u16, u16)>> = BTreeMap::new();
        for op in self.pending_maps.drain(..) {
            latest.insert((op.win, op.write, op.win_tile), op.target);
        }
        let mut by_heap: BTreeMap<(u8, bool, u16), Vec<(u32, u32, u32)>> = BTreeMap::new();
        let mut nulls: BTreeMap<(u8, bool), Vec<(u32, u32)>> = BTreeMap::new();
        for ((win, write, win_tile), target) in latest {
            let wt = u32::from(win_tile);
            match target {
                Some((heap_idx, heap_tile)) => {
                    let ht = u32::from(heap_tile);
                    let runs = by_heap.entry((win, write, heap_idx)).or_default();
                    match runs.last_mut() {
                        Some(last) if last.0 + last.2 == wt && last.1 + last.2 == ht => last.2 += 1,
                        _ => runs.push((wt, ht, 1)),
                    }
                }
                None => {
                    let runs = nulls.entry((win, write)).or_default();
                    match runs.last_mut() {
                        Some(last) if last.0 + last.1 == wt => last.1 += 1,
                        _ => runs.push((wt, 1)),
                    }
                }
            }
        }
        for ((win, write, heap_idx), runs) in by_heap {
            let res = if write {
                self.write_windows[win as usize].as_ref()
            } else {
                self.read_windows[win as usize].as_ref()
            };
            let (Some(res), Some(th)) = (res, self.tile_heaps.get(heap_idx as usize)) else {
                continue;
            };
            let coords: Vec<D3D12_TILED_RESOURCE_COORDINATE> = runs
                .iter()
                .map(|&(wt, _, _)| D3D12_TILED_RESOURCE_COORDINATE {
                    X: wt,
                    Y: 0,
                    Z: 0,
                    Subresource: 0,
                })
                .collect();
            let sizes: Vec<D3D12_TILE_REGION_SIZE> = runs
                .iter()
                .map(|&(_, _, count)| D3D12_TILE_REGION_SIZE {
                    NumTiles: count,
                    ..Default::default()
                })
                .collect();
            let flags = vec![D3D12_TILE_RANGE_FLAG_NONE; runs.len()];
            let starts: Vec<u32> = runs.iter().map(|&(_, ht, _)| ht).collect();
            let counts: Vec<u32> = runs.iter().map(|&(_, _, count)| count).collect();
            // SAFETY: live reserved window buffer and live default buffer heap;
            // matching array lengths `runs.len()`.
            unsafe {
                queue.UpdateTileMappings(
                    res,
                    runs.len() as u32,
                    Some(coords.as_ptr()),
                    Some(sizes.as_ptr()),
                    &th.heap,
                    runs.len() as u32,
                    Some(flags.as_ptr()),
                    Some(starts.as_ptr()),
                    Some(counts.as_ptr()),
                    D3D12_TILE_MAPPING_FLAG_NONE,
                );
            }
        }
        for ((win, write), runs) in nulls {
            let res = if write {
                self.write_windows[win as usize].as_ref()
            } else {
                self.read_windows[win as usize].as_ref()
            };
            let Some(res) = res else {
                continue;
            };
            let coords: Vec<D3D12_TILED_RESOURCE_COORDINATE> = runs
                .iter()
                .map(|&(wt, _)| D3D12_TILED_RESOURCE_COORDINATE {
                    X: wt,
                    Y: 0,
                    Z: 0,
                    Subresource: 0,
                })
                .collect();
            let sizes: Vec<D3D12_TILE_REGION_SIZE> = runs
                .iter()
                .map(|&(_, count)| D3D12_TILE_REGION_SIZE {
                    NumTiles: count,
                    ..Default::default()
                })
                .collect();
            let flags = vec![D3D12_TILE_RANGE_FLAG_NULL; runs.len()];
            let counts: Vec<u32> = runs.iter().map(|&(_, count)| count).collect();
            // SAFETY: live reserved window buffer; NULL range flags unmap tiles without a heap.
            unsafe {
                queue.UpdateTileMappings(
                    res,
                    runs.len() as u32,
                    Some(coords.as_ptr()),
                    Some(sizes.as_ptr()),
                    None,
                    runs.len() as u32,
                    Some(flags.as_ptr()),
                    None,
                    Some(counts.as_ptr()),
                    D3D12_TILE_MAPPING_FLAG_NONE,
                );
            }
        }
    }

    /// Flushes pending `UpdateTileMappings`, residency bitmap updates, staged
    /// page uploads, and any fallback small-buffer readbacks onto `host.queue()`
    /// ahead of the current submission's command lists.
    fn flush_gpu(&mut self) {
        self.land_small_results();
        let Some(host) = host() else {
            return;
        };
        let queue = host.queue();
        self.flush_tile_mappings(queue);

        let cur_sub = crate::host::submissions().0;
        let bitmap_stage = self.bitmap_dirty.take().and_then(|(lo_w, hi_w)| {
            let byte_off = lo_w * 4;
            let byte_len = (hi_w - lo_w) * 4;
            // SAFETY: `lo_w..hi_w` lies within `self.bitmap_cpu`.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    self.bitmap_cpu.as_ptr().add(lo_w).cast::<u8>(),
                    byte_len,
                )
            };
            self.transport
                .alloc_stage(bytes)
                .map(|(c_idx, src_off)| (byte_off as u64, c_idx, src_off, byte_len as u64))
        });

        let mut small_copies: Vec<(usize, u64)> = Vec::new();
        let ready_small: Vec<u64> = self
            .small_dirty
            .iter()
            .filter(|&(_, &marked_sub)| marked_sub < cur_sub)
            .map(|(&p, _)| p)
            .collect();
        if !ready_small.is_empty() && self.transport.ensure_small_buf().is_some() {
            for p in ready_small {
                let Some(slot_idx) = self.transport.small_free.pop() else {
                    break;
                };
                self.small_dirty.remove(&p);
                small_copies.push((slot_idx, p));
            }
        }

        let mut dirty_buf_copies: Vec<(ID3D12Resource, Vec<(usize, u64, u64, u64)>)> = Vec::new();
        for buf in &self.buffers {
            if buf.open {
                let _ = buf.shadow.take_mirror_dirty();
                continue;
            }
            if !buf.shadow.written() || buf.pages.is_empty() {
                continue;
            }
            let Some((lo_off, hi_off)) = buf.shadow.take_mirror_dirty() else {
                continue;
            };
            let (buf_start, buf_len) = buf.shadow.span();
            let lo_addr = buf_start.saturating_add(lo_off as u64);
            let hi_addr = buf_start.saturating_add(hi_off as u64).min(buf_start + buf_len);
            if lo_addr >= hi_addr {
                continue;
            }
            let mut sorted: Vec<u64> = buf
                .pages
                .iter()
                .copied()
                .filter(|&p| {
                    let addr = p * PAGE;
                    addr >= buf_start
                        && addr + PAGE <= buf_start + buf_len
                        && addr < hi_addr
                        && addr + PAGE > lo_addr
                })
                .collect();
            if sorted.is_empty() {
                continue;
            }
            sorted.sort_unstable();
            let mut runs: Vec<(usize, u64, u64, u64)> = Vec::new();
            for p in sorted {
                let win = (p >> 15) as usize;
                if self.read_windows[win].is_none() {
                    continue;
                }
                let win_off = (p & 0x7FFF) * PAGE;
                let buf_off = p * PAGE - buf_start;
                match runs.last_mut() {
                    Some(last)
                        if last.0 == win && last.1 + last.3 == win_off && last.2 + last.3 == buf_off =>
                    {
                        last.3 += PAGE;
                    }
                    _ => runs.push((win, win_off, buf_off, PAGE)),
                }
            }
            if !runs.is_empty() {
                dirty_buf_copies.push((buf.shadow.buffer.clone(), runs));
            }
        }

        if bitmap_stage.is_none()
            && self.transport.copies.is_empty()
            && small_copies.is_empty()
            && dirty_buf_copies.is_empty()
        {
            return;
        }

        let Some((alloc, list)) = self.transport.take_list(&host.device) else {
            return;
        };
        if !dirty_buf_copies.is_empty() {
            // SAFETY: global UAV barrier ensures prior shader writes to `Shadow::buffer` are visible.
            unsafe {
                list.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                    Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                            pResource: std::mem::ManuallyDrop::new(None),
                        }),
                    },
                }]);
            }
        }
        let mut post_barriers: Vec<D3D12_RESOURCE_BARRIER> = Vec::new();
        if let Some((dst_off, c_idx, src_off, len)) = bitmap_stage {
            let (src_res, _, _) = &self.transport.cur_chunks[c_idx];
            // SAFETY: open command list, valid buffer regions.
            unsafe {
                list.CopyBufferRegion(&self.bitmap_gpu, dst_off, src_res, src_off, len);
            }
            post_barriers.push(transition_barrier(
                &self.bitmap_gpu,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            ));
        }
        let copies = std::mem::take(&mut self.transport.copies);
        let mut copied_wins = 0u64;
        for copy in copies {
            let Some(dst_res) = self.read_windows[copy.win as usize].as_ref() else {
                continue;
            };
            let (src_res, _, _) = &self.transport.cur_chunks[copy.chunk_idx];
            // SAFETY: open command list, valid buffer regions within 2 GiB window and chunk.
            unsafe {
                list.CopyBufferRegion(dst_res, copy.dst_offset, src_res, copy.src_offset, copy.len);
            }
            copied_wins |= 1u64 << copy.win;
        }
        let mut kept_resources: Vec<ID3D12Resource> = Vec::with_capacity(dirty_buf_copies.len());
        for (src_res, runs) in dirty_buf_copies {
            for (win, win_off, buf_off, len) in runs {
                let Some(dst_res) = self.read_windows[win].as_ref() else {
                    continue;
                };
                // SAFETY: open command list, valid buffer regions within 2 GiB window and `Shadow::buffer`.
                unsafe {
                    list.CopyBufferRegion(dst_res, win_off, &src_res, buf_off, len);
                }
                copied_wins |= 1u64 << win;
            }
            post_barriers.push(transition_barrier(
                &src_res,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            ));
            kept_resources.push(src_res);
        }
        while copied_wins != 0 {
            let w = copied_wins.trailing_zeros() as usize;
            if let Some(dst_res) = self.read_windows[w].as_ref() {
                post_barriers.push(transition_barrier(
                    dst_res,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ));
            }
            copied_wins &= copied_wins - 1;
        }
        if !post_barriers.is_empty() {
            // SAFETY: open command list; resources owned by `Memory`.
            unsafe {
                list.ResourceBarrier(&post_barriers);
            }
        }
        let next_ticket = self.transport.ticket + 1;
        if !small_copies.is_empty()
            && let Some((small_res, _)) = &self.transport.small_buf
        {
            // SAFETY: global UAV barrier ensures prior builder stores on the queue are visible to copy.
            unsafe {
                list.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                    Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                            pResource: std::mem::ManuallyDrop::new(None),
                        }),
                    },
                }]);
            }
            for &(slot_idx, page) in &small_copies {
                let win = (page >> 15) as usize;
                if let Some(src_res) = self.read_windows[win].as_ref() {
                    let src_off = (page & 0x7FFF) * PAGE;
                    let dst_off = (slot_idx as u64) * PAGE;
                    // SAFETY: open command list, valid buffer regions.
                    unsafe {
                        list.CopyBufferRegion(small_res, dst_off, src_res, src_off, PAGE);
                    }
                    self.transport
                        .small_in_flight
                        .push_back((next_ticket, false, slot_idx, page));
                } else {
                    self.transport.small_free.push(slot_idx);
                }
            }
        }
        // SAFETY: closing and executing our command list on the host 3D queue, then signalling our fence.
        let sent = unsafe {
            list.Close().and_then(|()| {
                let cmd: ID3D12CommandList = list.cast()?;
                queue.ExecuteCommandLists(&[Some(cmd)]);
                self.transport.ticket = next_ticket;
                queue.Signal(&self.transport.fence, next_ticket)
            })
        };
        if let Err(e) = sent {
            tracing::warn!(error = %e, "title memory by address: sync command list execution failed");
        }
        let used_chunks: Vec<(ID3D12Resource, *mut u8)> = self
            .transport
            .cur_chunks
            .drain(..)
            .map(|(r, p, _)| (r, p))
            .collect();
        if !used_chunks.is_empty() {
            self.transport
                .in_flight_chunks
                .push_back((self.transport.ticket, used_chunks));
        }
        if !kept_resources.is_empty() {
            self.transport
                .in_flight_kept
                .push_back((self.transport.ticket, kept_resources));
        }
        self.transport
            .in_flight_lists
            .push_back((self.transport.ticket, alloc, list));
    }

    /// Records same-submission GPU -> staging copies for any `small` pages into
    /// a reading command list (before `read_windows` are used as UAVs on it).
    fn record_small_on_read_list(&mut self, list: &HostList) {
        if self.small_dirty.is_empty() {
            return;
        }
        let Some((small_res, _)) = self.transport.ensure_small_buf() else {
            return;
        };
        let pages: Vec<u64> = self.small_dirty.keys().copied().collect();
        let target_sub = crate::host::submissions().0 + crate::queue::queued() + 1;
        let mut copied_wins: BTreeSet<usize> = BTreeSet::new();
        for page in pages {
            let win = (page >> 15) as usize;
            let Some(src_res) = self.read_windows[win].as_ref() else {
                continue;
            };
            let Some(slot_idx) = self.transport.small_free.pop() else {
                break;
            };
            self.small_dirty.remove(&page);
            let src_off = (page & 0x7FFF) * PAGE;
            let dst_off = (slot_idx as u64) * PAGE;
            // SAFETY: open command list at `first_read` (before any UAV use of `read_windows` on `list`);
            // `src_res` promotes from `COMMON` to `COPY_SOURCE`, then transitions to `UNORDERED_ACCESS`.
            unsafe {
                list.list
                    .CopyBufferRegion(&small_res, dst_off, src_res, src_off, PAGE);
            }
            copied_wins.insert(win);
            self.transport
                .small_in_flight
                .push_back((target_sub, true, slot_idx, page));
        }
        if !copied_wins.is_empty() {
            list.keep(&small_res);
            let barriers: Vec<D3D12_RESOURCE_BARRIER> = copied_wins
                .into_iter()
                .filter_map(|w| self.read_windows[w].as_ref())
                .map(|res| {
                    transition_barrier(
                        res,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    )
                })
                .collect();
            // SAFETY: open command list; resources live in `MEMORY`.
            unsafe {
                list.list.ResourceBarrier(&barriers);
            }
        }
    }

    /// Mirrors intra-list GPU-written closed title buffers (`[lo_addr, hi_addr)`)
    /// from `Shadow::buffer` into `read_windows` on `list` before the builder reads them.
    fn sync_gpu_written_buffer(&self, list: &HostList, buf_idx: u32, lo_addr: u64, hi_addr: u64) {
        let Some(buf) = self.buffers.get(buf_idx as usize) else {
            return;
        };
        if buf.open || !buf.shadow.written() || buf.pages.is_empty() || lo_addr >= hi_addr {
            return;
        }
        let (buf_start, buf_len) = buf.shadow.span();
        let mut sorted: Vec<u64> = buf
            .pages
            .iter()
            .copied()
            .filter(|&p| {
                let addr = p * PAGE;
                addr >= buf_start
                    && addr + PAGE <= buf_start + buf_len
                    && addr < hi_addr
                    && addr + PAGE > lo_addr
            })
            .collect();
        if sorted.is_empty() {
            return;
        }
        sorted.sort_unstable();
        // Join contiguous runs within the same 2 GiB window.
        let mut runs: Vec<(usize, u64, u64, u64)> = Vec::new();
        for p in sorted {
            let win = (p >> 15) as usize;
            let win_off = (p & 0x7FFF) * PAGE;
            let buf_off = p * PAGE - buf_start;
            match runs.last_mut() {
                Some(last) if last.0 == win && last.1 + last.3 == win_off && last.2 + last.3 == buf_off => {
                    last.3 += PAGE;
                }
                _ => runs.push((win, win_off, buf_off, PAGE)),
            }
        }
        if runs.is_empty() {
            return;
        }
        list.keep(&buf.shadow.buffer);
        let mut touched_wins: BTreeSet<usize> = BTreeSet::new();
        for &(win, _, _, _) in &runs {
            if self.read_windows[win].is_some() {
                touched_wins.insert(win);
            }
        }
        if touched_wins.is_empty() {
            return;
        }
        let mut pre = vec![transition_barrier(
            &buf.shadow.buffer,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )];
        for &win in &touched_wins {
            let win_key = usize::MAX - (7 << 33) - win;
            if !list.first_use(win_key)
                && let Some(res) = &self.read_windows[win]
            {
                pre.push(transition_barrier(
                    res,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                ));
            }
        }
        // SAFETY: open command list; live buffers.
        unsafe {
            list.list.ResourceBarrier(&pre);
            for (win, win_off, buf_off, len) in runs {
                if let Some(dst) = &self.read_windows[win] {
                    list.list
                        .CopyBufferRegion(dst, win_off, &buf.shadow.buffer, buf_off, len);
                }
            }
            let mut post = vec![transition_barrier(
                &buf.shadow.buffer,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )];
            for win in touched_wins {
                if let Some(res) = &self.read_windows[win] {
                    post.push(transition_barrier(
                        res,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    ));
                }
            }
            list.list.ResourceBarrier(&post);
        }
    }

    fn take_faults(&mut self) {
        self.land_small_results();
        let mut pages = Vec::new();
        for i in 1..MEMORY_FAULTS as usize {
            // SAFETY: inside `faults` shared mapping.
            let at = unsafe { self.faults.1.add(i) };
            // SAFETY: as above.
            let page = unsafe { at.read_volatile() };
            if page != 0 {
                pages.push(u64::from(page));
                // SAFETY: as above.
                unsafe { at.write_volatile(0) };
            }
        }
        if pages.is_empty() {
            return;
        }
        pages.sort_unstable();
        pages.dedup();
        let mut made = 0;
        for &page in &pages {
            if made < FAULTS
                && self.faulted < FAULTS_ALL
                && !self.resident.contains_key(&page)
                && self.map(page)
            {
                made += 1;
                self.faulted += 1;
                self.bring(page, page + 1, false);
            }
        }
        self.stats.faults += pages.len() as u64;
        static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let said = SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if said < 40 || said.is_multiple_of(100) {
            let some: Vec<String> = pages
                .iter()
                .step_by(pages.len().div_ceil(12))
                .map(|&p| format!("{:#x}", p * PAGE))
                .collect();
            tracing::warn!(
                frame = self.frame,
                pages = pages.len(),
                made,
                held = self.resident.len(),
                ?some,
                "title memory by address: shaders touched pages that had no copy (those accesses read 0 or were dropped); those that are the title's are there now"
            );
        }
    }

    fn named_in_upload(&mut self, address: u64) {
        let Some((from, len)) = crate::upload::block(address as usize) else {
            return;
        };
        // SAFETY: `upload::block` stays mapped and was written before the dispatch was recorded.
        let words = unsafe { std::slice::from_raw_parts(from as *const u64, len / 8) };
        let mut seen = HashSet::new();
        for &word in words {
            if word & 0xFF == 0
                && (1 << 32..1 << 37).contains(&word)
                && seen.insert(word / PAGE)
                && self.map(word / PAGE)
            {
                let page = word / PAGE;
                self.bring(page, page + 1, false);
            }
        }
    }

    fn small(&mut self, start: u64, len: u64) {
        let cur_sub = crate::host::submissions().0 + crate::queue::queued();
        let mut at = start;
        while at < start + len {
            let page = at / PAGE;
            let n = ((page + 1) * PAGE).min(start + len) - at;
            if self.map(page) && self.resident.get(&page).is_some_and(|r| r.arena.is_none()) {
                self.opens(at);
                self.small_dirty.entry(page).or_insert(cur_sub);
                self.stats.back += 1;
            }
            at += n;
        }
    }

    fn report(&mut self) {
        let now = std::time::Instant::now();
        let (since, frame) = *self.stats.from.get_or_insert((now, self.frame));
        if now.duration_since(since).as_secs_f64() < 10.0 {
            return;
        }
        let frames = self.frame.wrapping_sub(frame).max(1) as f64;
        let s = std::mem::take(&mut self.stats);
        tracing::info!(
            "title memory by address: {} pages ({} MB, {} arena) in {} heaps, {} buffers, {}/{} windows; per frame {:.0} dispatches that read, {:.0} that build, \
             {:.1} pages that had no copy, {:.1} pages turned down, {:.1} results brought back; {:.0} pages looked at, {:.1} KB changed, \
             {:.1} packed structures ({} there), {:.2} ms; {} small windows; since the last line {} KB of packed structures came and {} of them went, \
             and pages for constants {}, views written {}, others {}, upload memory {}, faults in all {}",
            self.resident.len(),
            (self.resident.len() as u64 * PAGE) >> 20,
            self.arena_pages,
            self.tile_heaps.len(),
            self.buffers.len(),
            self.read_mask.count_ones(),
            self.write_mask.count_ones(),
            s.uses[0] as f64 / frames,
            s.uses[1] as f64 / frames,
            s.faults as f64 / frames,
            s.unknown as f64 / frames,
            s.back as f64 / frames,
            s.looked as f64 / frames,
            s.changed as f64 / 1024.0 / frames,
            s.packed as f64 / frames,
            self.packed.len(),
            s.micros as f64 / 1000.0 / frames,
            self.windows,
            s.packed_bytes >> 10,
            s.gone,
            s.came[0],
            s.came[1],
            s.came[2],
            s.came[3],
            self.faulted
        );
        self.stats.from = Some((now, self.frame));
    }
}

/// A list with this key ([`HostList::used`]) asks the builder for sizes.
const SIZES: usize = usize::MAX - (9 << 33);

fn sizes_wait() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_BVH_SIZES_WAIT").is_some_and(|v| v == "1"))
}

fn asks_size(written: &[u64], constants: &[u32], threads: Option<[u32; 3]>) -> bool {
    threads == Some([1, 1, 1])
        && matches!(*constants, [_, hi, 1..=3, ..] if (1..0x20).contains(&hi))
        && written
            .first()
            .is_some_and(|&at| crate::shadow::in_buffer(at, 1).is_some_and(|b| b.1 <= BACK))
}

/// Whether one of `lists` asks for sizes.
pub(crate) fn asked_sizes<'a>(lists: impl Iterator<Item = &'a HostList>) -> bool {
    let mut lists = lists;
    let asked = sizes_wait() && lists.any(|l| l.used(SIZES));
    if asked {
        static TIMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = TIMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n.is_power_of_two() || n.is_multiple_of(100) {
            tracing::info!(
                n,
                frame = crate::frame::current(),
                "submissions that ask for structures' sizes, waited for"
            );
        }
    }
    asked
}

fn packed_len(header: u64) -> Option<u64> {
    let (version, nodes) = (header as u32, header >> 32);
    let marks = version >> 16;
    let len = (nodes + 1) * 64;
    (version & 0xFFFF == 6 && marks & 0x8100 == 0x8100 && nodes > 0 && len <= PACKED)
        .then_some(len)
}

/// Updates `ours` with any differing dwords in `title` and returns `(lo_byte, hi_byte, changed_dwords)`.
fn merge_span(title: &[u8; PIECE], ours: &mut [u8]) -> Option<(usize, usize, usize)> {
    if ours == title {
        return None;
    }
    let mut changed = 0usize;
    let mut first = usize::MAX;
    let mut last = 0usize;
    for (idx, (t, o)) in title.chunks_exact(4).zip(ours.chunks_exact_mut(4)).enumerate() {
        if t != o {
            o.copy_from_slice(t);
            changed += 1;
            let byte = idx * 4;
            if first == usize::MAX {
                first = byte;
            }
            last = byte + 4;
        }
    }
    (changed > 0).then_some((first, last, changed))
}

#[cfg(test)]
fn merge(title: &[u8; PIECE], ours: &mut [u8], never: bool) -> usize {
    if never {
        ours.copy_from_slice(title);
        return PIECE / 4;
    }
    merge_span(title, ours).map_or(0, |(_, _, dwords)| dwords)
}

pub(crate) fn ready(
    list: &HostList,
    heap: &HostHeap,
    addresses: &[u64],
    written: &[u64],
    constants: &[u32],
    use_: Use,
    threads: Option<[u32; 3]>,
) -> Option<Ready> {
    let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Memory::new();
    }
    let m = guard.as_mut()?;
    let frame = crate::frame::current();
    if m.frame != frame {
        m.frame = frame;
        m.refused.clear();
        m.take_faults();
        m.look_again();
        m.report();
        #[cfg(feature = "diagnostics")]
        m.trace.look(frame);
    }
    #[cfg(feature = "diagnostics")]
    if use_ != Use::Reads {
        m.trace
            .dispatch(frame, list, addresses, written, constants, threads);
    }
    let builds = use_ != Use::Reads;
    if builds && sizes_wait() && asks_size(written, constants, threads) {
        list.first_use(SIZES);
    }
    if builds {
        m.reclaim_retired_tiles();
        while m.free.len() < SPARE
            && m.tile_heaps.len() * (HEAP_TILES as usize) < MOST
            && m.another()
        {}
    }
    m.stats.uses[usize::from(builds)] += 1;

    let first_read = list.first_use(READ_KEY);
    let list_key = std::ptr::from_ref(list) as usize;
    if first_read {
        let queue_vec = Arc::new(Mutex::new(Vec::<u64>::new()));
        m.list_lands.insert(list_key, queue_vec.clone());
        let to_run = queue_vec;
        list.defer_host(move || {
            let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
            let Some(m) = guard.as_mut() else {
                return;
            };
            m.list_lands.remove(&list_key);
            let addrs = std::mem::take(&mut *to_run.lock().unwrap_or_else(|e| e.into_inner()));
            for at in addrs {
                m.land(at);
            }
            m.bring_buffers();
        });
    }

    let named = |kind: usize, at: u64| list.first_use(usize::MAX - (kind << 26) - (at / PAGE) as usize);
    let held = |m: &Memory| m.arena_pages as u64;
    let before = held(m);
    for (i, &c) in constants.iter().enumerate() {
        if let Some(&high) = constants.get(i + 1)
            && (1..=0x1F).contains(&high)
            && c >= 0x1_0000
        {
            let at = u64::from(c) | u64::from(high) << 32;
            if named(1, at) {
                m.ready(at, 1);
            }
        }
        if builds && (0x0100_0000..0x2000_0000).contains(&c) {
            let at = u64::from(c) << 8;
            if named(1, at) {
                m.ready(at, 1);
            }
        }
    }
    let mut came = [held(m) - before, 0, 0, 0];
    for &at in addresses.iter().filter(|&&a| a >= 1 << 32) {
        let mut packed = false;
        if builds
            && list.first_use(usize::MAX - (3 << 33) - (at >> 8) as usize)
            && crate::shadow::in_buffer(at, 1).is_none()
            && m.possible(at / PAGE)
        {
            if let Some((len, _)) = m.packed_at(at) {
                packed = true;
                for page in at / PAGE..=(at + len - 1) / PAGE {
                    let new = !m.resident.contains_key(&page);
                    if m.map(page)
                        && new
                        && let Some(meta) = m.resident.get_mut(&page).and_then(|r| r.arena.as_mut())
                    {
                        meta.theirs = true;
                    }
                }
                m.bring(at / PAGE, (at + len - 1) / PAGE + 1, false);
            }
            if let Some(q) = m.list_lands.get(&list_key) {
                q.lock().unwrap_or_else(|e| e.into_inner()).push(at);
            }
        }
        if builds
            && written.contains(&at)
            && list.first_use(usize::MAX - (4 << 33) - (at >> 2 & 0x7FFF_FFFF) as usize)
        {
            if let Some((start, len)) = crate::shadow::in_buffer(at, 1)
                && len <= BACK
                && use_ == (Use::Builds { back: true })
                && list.first_use(usize::MAX - (5 << 33) - (start >> 2 & 0x7FFF_FFFF) as usize)
            {
                m.small(start, len);
            }
            m.opens(at);
            if !packed && crate::shadow::in_buffer(at, 1).is_none() {
                #[cfg(feature = "diagnostics")]
                {
                    let built = m.dump.built.entry(at).or_insert((frame, frame, 0));
                    (built.1, built.2) = (frame, built.2 + 1);
                }
                let before = held(m);
                m.ready(at, BUILT);
                came[1] += held(m).saturating_sub(before);
            }
        }
        if packed || !named(2, at) {
            continue;
        }
        let before = held(m);
        match crate::shadow::in_buffer(at, 1) {
            Some(_) => {
                m.ready(at, 1);
                m.named_in_upload(at);
                if let Some(buf_idx) = m.resident.get(&(at / PAGE)).and_then(|r| r.buffer) {
                    let (needs_refresh, shadow) = {
                        let buf = &mut m.buffers[buf_idx as usize];
                        let nr = !buf.open && buf.shadow.written() && buf.refreshed_frame != frame;
                        if nr {
                            buf.refreshed_frame = frame;
                        }
                        (nr, buf.shadow.clone())
                    };
                    if needs_refresh {
                        let (bs, bl) = shadow.span();
                        shadow.refresh(list, bs, bl);
                    }
                    if let Some((lo_addr, hi_addr)) = list.gpu_written_span(&shadow)
                        && list.first_use(usize::MAX - (6 << 33) - buf_idx as usize)
                    {
                        m.sync_gpu_written_buffer(list, buf_idx, lo_addr, hi_addr);
                    }
                }
                came[3] += held(m).saturating_sub(before);
            }
            None => {
                m.ready(at, if builds { VIEW } else { PAGE });
                came[2] += held(m).saturating_sub(before);
            }
        }
    }
    for (sum, n) in m.stats.came.iter_mut().zip(came) {
        *sum += n;
    }

    let key = std::ptr::from_ref(heap) as usize;
    let (base, mut bound_read, mut bound_write) = match m.heaps.get(&key) {
        Some(&entry) => entry,
        None => {
            let base = heap.dynamic(HEAP_SLOTS)?;
            heap.null_raw_at(base);
            heap.null_raw_uav_at(base + 1);
            for n in 0..MEMORY_WINDOWS {
                heap.null_raw_at(base + 2 + n);
            }
            for n in 0..2 * MEMORY_WINDOWS {
                heap.null_raw_uav_at(base + 2 + MEMORY_WINDOWS + n);
            }
            (base, 0, 0)
        }
    };
    let mut missing_r = m.read_mask & !bound_read;
    while missing_r != 0 {
        let w = missing_r.trailing_zeros() as usize;
        if let Some(win) = &m.read_windows[w] {
            heap.raw_at(base + 2 + w as u32, win, 0, WINDOW_BYTES);
            heap.raw_uav_at(base + 2 + MEMORY_WINDOWS + w as u32, win, 0, WINDOW_BYTES);
            bound_read |= 1u64 << w;
        }
        missing_r &= missing_r - 1;
    }
    let mut missing_w = m.write_mask & !bound_write;
    while missing_w != 0 {
        let w = missing_w.trailing_zeros() as usize;
        if let Some(win) = &m.write_windows[w] {
            heap.raw_uav_at(base + 2 + 2 * MEMORY_WINDOWS + w as u32, win, 0, WINDOW_BYTES);
            bound_write |= 1u64 << w;
        }
        missing_w &= missing_w - 1;
    }
    m.heaps.insert(key, (base, bound_read, bound_write));

    if first_read {
        if use_ == Use::Reads {
            m.record_small_on_read_list(list);
        }
        list.keep(&m.bitmap_gpu);
        list.keep(&m.faults.0);
        #[cfg(feature = "diagnostics")]
        m.dump(list, addresses);
    }
    #[cfg(feature = "diagnostics")]
    if use_ == Use::Reads {
        m.dump.walked(addresses);
    }
    Some(Ready {
        directory: m.bitmap_va,
        faults: m.faults.2,
        base,
    })
}

/// Flushes all pending `Memory` tile mappings, residency bitmap updates, staged
/// arena uploads, and cross-submission dirty `Shadow::buffer -> read_windows`
/// copies in a single GPU command list right before `runs` executes.
pub(crate) fn before_execute(runs: &[&HostList]) {
    if !runs.iter().any(|l| l.used(READ_KEY)) {
        return;
    }
    let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = guard.as_mut() {
        m.flush_gpu();
    }
}

/// `VICTORY_BVH_TRACE=1`: traces structure compacted-size requests and copies.
#[cfg(feature = "diagnostics")]
mod trace {
    use super::*;

    #[derive(Default)]
    pub(super) struct Trace {
        asked: Vec<(u64, u64, u64, u64)>,
        sizes: HashMap<u64, (u64, u64)>,
        said: u32,
    }

    fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("VICTORY_BVH_TRACE").is_some_and(|v| v == "1"))
    }

    fn value(at: u64) -> u64 {
        let mut b = [0u8; 8];
        crate::host::copy_mapped(b.as_mut_ptr(), at as *const u8, 8, false);
        u64::from_le_bytes(b)
    }

    impl Trace {
        fn say(&mut self, text: String) {
            self.said += 1;
            if self.said < 6000 {
                tracing::info!("bvh trace: {text}");
            }
        }

        pub(super) fn look(&mut self, frame: u64) {
            if !on() {
                return;
            }
            let mut gone = Vec::new();
            for i in 0..self.asked.len() {
                let (then, at, structure, was) = self.asked[i];
                let now = value(at);
                if now != was {
                    self.sizes.insert(structure, (now, frame));
                    gone.push(i);
                    self.say(format!(
                        "frame {frame}: size of {structure:#x} at {at:#x} asked in {then}: {was:#x} -> {now:#x}"
                    ));
                } else if frame > then + 60 {
                    gone.push(i);
                    self.say(format!(
                        "frame {frame}: size of {structure:#x} at {at:#x} asked in {then}: still {was:#x}"
                    ));
                }
            }
            for i in gone.into_iter().rev() {
                self.asked.swap_remove(i);
            }
        }

        pub(super) fn dispatch(
            &mut self,
            frame: u64,
            list: &HostList,
            addresses: &[u64],
            written: &[u64],
            constants: &[u32],
            threads: Option<[u32; 3]>,
        ) {
            if !on() {
                return;
            }
            let _ = addresses;
            let list = std::ptr::from_ref(list) as usize & 0xFFFF_FFFF;
            if threads == Some([1, 1, 1])
                && let [lo, hi, mode, ..] = *constants
                && (1..=3).contains(&mode)
                && hi < 0x20
                && let Some(&at) = written.first()
                && crate::shadow::in_buffer(at, 1).is_some_and(|b| b.1 <= BACK)
            {
                let structure = u64::from(lo) | u64::from(hi) << 32;
                let was = value(at);
                self.say(format!(
                    "frame {frame} list {list:x}: asks mode {mode} of {structure:#x} to {at:#x}, there {was:#x}"
                ));
                if mode == 1 {
                    self.asked.push((frame, at, structure, was));
                }
            } else if threads == Some([32, 1, 1])
                && let [from, to] = *written
                && [from, to]
                    .iter()
                    .all(|&a| a >= 1 << 32 && crate::shadow::in_buffer(a, 1).is_none())
            {
                let known = self.sizes.get(&from).copied();
                self.say(format!(
                    "frame {frame} list {list:x}: copies {from:#x} to {to:#x} constants {:x?}, its size was {known:x?}",
                    &constants[..constants.len().min(4)]
                ));
            }
        }
    }
}

/// `VICTORY_MEMORY_DUMP=<folder>`: offline arena dump when `<folder>/go` exists.
#[cfg(feature = "diagnostics")]
mod dump {
    use super::*;

    #[derive(Default)]
    pub(super) struct Dump {
        walked: BTreeSet<u64>,
        pub(super) built: BTreeMap<u64, (u64, u64, u64)>,
        due: Option<(u64, Vec<(ID3D12Resource, usize)>)>,
        done: bool,
    }

    impl Dump {
        pub(super) fn walked(&mut self, addresses: &[u64]) {
            if !self.done && self.walked.len() < 64 {
                self.walked.extend(addresses);
            }
        }
    }

    fn folder() -> Option<&'static std::path::PathBuf> {
        static FOLDER: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
        FOLDER
            .get_or_init(|| std::env::var_os("VICTORY_MEMORY_DUMP").map(Into::into))
            .as_ref()
    }

    impl Memory {
        pub(super) fn dump(&mut self, list: &HostList, _: &[u64]) {
            let Some(folder) = folder() else { return };
            if let Some((due, _)) = &self.dump.due {
                if self.frame < *due {
                    return;
                }
                let copies = self.dump.due.take().map(|d| d.1).unwrap_or_default();
                let mut text = String::new();
                for a in &self.dump.walked {
                    text += &format!("walked {a:#x}\n");
                }
                for (a, (first, last, n)) in &self.dump.built {
                    text += &format!("built {a:#x} {first} {last} {n}\n");
                }
                for (a, p) in &self.packed {
                    text += &format!("packed {a:#x} {} {}\n", p.len, p.frame);
                }
                text += &format!("frame {}\n", self.frame);
                for (&page, res) in &self.resident {
                    if res.arena.is_some() {
                        let at = u64::from(res.tile) * PAGE;
                        text += &format!("{:#x} {} {at:#x}\n", page * PAGE, res.heap);
                    }
                }
                let _ = std::fs::write(folder.join("pages.txt"), text);
                for (n, (_, data)) in copies.iter().enumerate() {
                    // SAFETY: mapped `ARENA`-byte readback buffer; GPU copies finished frames ago.
                    let bytes = unsafe { std::slice::from_raw_parts(*data as *const u8, ARENA as usize) };
                    let _ = std::fs::write(folder.join(format!("arena-{n}.bin")), bytes);
                }
                tracing::info!(arenas = copies.len(), "title memory by address: the arenas are written out");
                return;
            }
            if self.dump.done || !folder.join("go").exists() {
                return;
            }
            self.dump.done = true;
            let Some(host) = host() else { return };
            let mut copies = Vec::new();
            let mut touched: BTreeSet<usize> = BTreeSet::new();
            for _ in 0..self.tile_heaps.len() {
                let Some((to, data)) = host.shared_buffer(ARENA) else {
                    break;
                };
                // SAFETY: freshly mapped buffer of `ARENA` bytes.
                unsafe { std::ptr::write_bytes(data, 0, ARENA as usize) };
                list.keep(&to);
                copies.push((to, data as usize));
            }
            for (&page, res) in &self.resident {
                if res.arena.is_none() || (res.heap as usize) >= copies.len() {
                    continue;
                }
                let win = (page >> 15) as usize;
                if let Some(src) = &self.read_windows[win] {
                    if touched.insert(win) {
                        // SAFETY: open list; live window resource.
                        unsafe {
                            list.list.ResourceBarrier(&[transition_barrier(
                                src,
                                D3D12_RESOURCE_STATE_COMMON,
                                D3D12_RESOURCE_STATE_COPY_SOURCE,
                            )]);
                        }
                    }
                    let src_off = (page & 0x7FFF) * PAGE;
                    let dst_off = u64::from(res.tile) * PAGE;
                    // SAFETY: open list; valid tile offsets.
                    unsafe {
                        list.list
                            .CopyBufferRegion(&copies[res.heap as usize].0, dst_off, src, src_off, PAGE);
                    }
                }
            }
            for win in touched {
                if let Some(src) = &self.read_windows[win] {
                    // SAFETY: open list; live window resource.
                    unsafe {
                        list.list.ResourceBarrier(&[transition_barrier(
                            src,
                            D3D12_RESOURCE_STATE_COPY_SOURCE,
                            D3D12_RESOURCE_STATE_COMMON,
                        )]);
                    }
                }
            }
            self.dump.due = Some((self.frame + 12, copies));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piece_takes_what_changed() {
        let mut title = [7u8; PIECE];
        let mut ours = vec![0u8; PIECE];
        assert_eq!(merge(&title, &mut ours, true), PIECE / 4);
        assert_eq!(ours, title);
        assert_eq!(merge(&title, &mut ours, false), 0);
        title[41] = 8;
        assert_eq!(merge(&title, &mut ours, false), 1);
        assert_eq!(ours, title);
    }

    #[test]
    fn a_packed_structure_is_known_by_its_header() {
        let header = |version: u32, nodes: u32| u64::from(version) | u64::from(nodes) << 32;
        assert_eq!(packed_len(header(0x8102_0006, 0x9a1a)), Some(0x9a1b * 64));
        assert_eq!(packed_len(header(0x8002_0006, 0x9a1a)), None);
        assert_eq!(packed_len(header(0x0000_0006, 5)), None);
        assert_eq!(packed_len(header(0x8102_0007, 5)), None);
        assert_eq!(packed_len(header(0x8102_0006, 0)), None);
        assert_eq!(packed_len(header(0x8102_0006, 1 << 21)), None);
    }

    #[test]
    fn reserved_windows_alias_tiles_and_drop_unmapped_writes_on_card() {
        let Some(host) = host() else {
            return;
        };
        let Some(read_win) = make_reserved_window(&host.device, 0, false) else {
            return;
        };
        let Some(write_win) = make_reserved_window(&host.device, 0, true) else {
            return;
        };
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: ARENA,
            Properties: D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            },
            Alignment: 0,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS | crate::host::not_resident(),
        };
        let mut made: Option<ID3D12Heap> = None;
        // SAFETY: valid default buffer heap creation.
        unsafe { host.device.CreateHeap::<ID3D12Heap>(&desc, &mut made) }.expect("CreateHeap");
        let heap = made.expect("heap");
        crate::host::resident(&host.device, &heap).expect("resident");
        crate::host::mapped(&host.device).expect("mapped");

        let queue = host.queue();
        let map_one = |res: &ID3D12Resource, win_tile: u32, heap_tile: u32| {
            let coord = D3D12_TILED_RESOURCE_COORDINATE {
                X: win_tile,
                Y: 0,
                Z: 0,
                Subresource: 0,
            };
            let size = D3D12_TILE_REGION_SIZE {
                NumTiles: 1,
                ..Default::default()
            };
            let flag = D3D12_TILE_RANGE_FLAG_NONE;
            let count = 1u32;
            // SAFETY: single-tile mapping on live reserved buffer and heap.
            unsafe {
                queue.UpdateTileMappings(
                    res,
                    1,
                    Some(&raw const coord),
                    Some(&raw const size),
                    &heap,
                    1,
                    Some(&raw const flag),
                    Some(&raw const heap_tile),
                    Some(&raw const count),
                    D3D12_TILE_MAPPING_FLAG_NONE,
                );
            }
        };
        // Tile 0: read-only (mapped in read_win -> heap tile 0, unmapped in write_win).
        map_one(&read_win, 0, 0);
        // Tile 32767 (last tile of the 2 GiB window): read-write (mapped in both -> heap tile 1).
        map_one(&read_win, 32767, 1);
        map_one(&write_win, 32767, 1);

        let (stage, stage_ptr) = host.shared_buffer(4096).expect("stage");
        let (back, back_ptr) = host.shared_buffer(4096).expect("back");
        // SAFETY: mapped 4096-byte shared buffers.
        unsafe {
            std::ptr::write_bytes(stage_ptr, 0, 4096);
            std::ptr::write_bytes(back_ptr, 0xCC, 4096);
            stage_ptr.cast::<u32>().add(0).write_volatile(0x1122_3344);
            stage_ptr.cast::<u32>().add(1).write_volatile(0x5566_7788);
            stage_ptr.cast::<u32>().add(2).write_volatile(0x99AA_BBCC);
            stage_ptr.cast::<u32>().add(3).write_volatile(0xDEAD_BEEF);
        }

        // SAFETY: dedicated command allocator, list, and fence on live device/queue.
        unsafe {
            let alloc: ID3D12CommandAllocator = host
                .device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .unwrap();
            let list: ID3D12GraphicsCommandList = host
                .device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None)
                .unwrap();
            let fence: ID3D12Fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();

            // Seed tile 0 and tile 32767+0xFFC0 via read_win.
            list.CopyBufferRegion(&read_win, 0, &stage, 0, 4);
            list.CopyBufferRegion(&read_win, 32767 * PAGE + 0xFFC0, &stage, 4, 4);
            list.Close().unwrap();
            let cmd: ID3D12CommandList = list.cast().unwrap();
            queue.ExecuteCommandLists(&[Some(cmd)]);
            queue.Signal(&fence, 1).unwrap();
            while fence.GetCompletedValue() < 1 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            // Write via write_win: tile 0 (unmapped -> must drop), tile 32767+0xFFC0 (aliased -> must update),
            // and tile 100 (unmapped -> must drop).
            alloc.Reset().unwrap();
            list.Reset(&alloc, None).unwrap();
            list.CopyBufferRegion(&write_win, 0, &stage, 12, 4);
            list.CopyBufferRegion(&write_win, 32767 * PAGE + 0xFFC0, &stage, 8, 4);
            list.CopyBufferRegion(&write_win, 100 * PAGE, &stage, 12, 4);
            list.Close().unwrap();
            let cmd: ID3D12CommandList = list.cast().unwrap();
            queue.ExecuteCommandLists(&[Some(cmd)]);
            queue.Signal(&fence, 2).unwrap();
            while fence.GetCompletedValue() < 2 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            // Read all three back from read_win.
            alloc.Reset().unwrap();
            list.Reset(&alloc, None).unwrap();
            list.CopyBufferRegion(&back, 0, &read_win, 0, 4);
            list.CopyBufferRegion(&back, 4, &read_win, 32767 * PAGE + 0xFFC0, 4);
            list.CopyBufferRegion(&back, 8, &read_win, 100 * PAGE, 4);
            list.Close().unwrap();
            let cmd: ID3D12CommandList = list.cast().unwrap();
            queue.ExecuteCommandLists(&[Some(cmd)]);
            queue.Signal(&fence, 3).unwrap();
            while fence.GetCompletedValue() < 3 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            let w0 = back_ptr.cast::<u32>().add(0).read_volatile();
            let w1 = back_ptr.cast::<u32>().add(1).read_volatile();
            let w2 = back_ptr.cast::<u32>().add(2).read_volatile();
            assert_eq!(w0, 0x1122_3344, "closed read-only tile must ignore write_win writes");
            assert_eq!(w1, 0x99AA_BBCC, "aliased tile at end of 2 GiB window must reflect write_win writes");
            assert_eq!(w2, 0, "unmapped tile in 2 GiB window must read zeros");
        }
    }
}
