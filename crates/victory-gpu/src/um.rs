use std::collections::{BTreeMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory as mem;

use crate::inplace::Placed;

mod cpu_reservation;
pub(crate) use cpu_reservation::{cpu_ask, cpu_pointer};

const K64: usize = 64 << 10;

pub const CELL: usize = 4 << 20;
pub const SPACE: (usize, usize) = (CELL, 1 << 37);

const APPLICATION_INFO: u32 = 0;
const INSTANCE_CREATE_INFO: u32 = 1;
const DEVICE_QUEUE_CREATE_INFO: u32 = 2;
const DEVICE_CREATE_INFO: u32 = 3;
const MEMORY_ALLOCATE_INFO: u32 = 5;
const EXPORT_MEMORY_ALLOCATE_INFO: u32 = 1_000_072_002;
const MEMORY_MAP_INFO: u32 = 1_000_271_000;
const MEMORY_UNMAP_INFO: u32 = 1_000_271_001;
const MEMORY_ALIAS_INFO_RADV: u32 = 1_000_999_001;
const API_1_4: u32 = (1 << 22) | (4 << 12);
const OPAQUE_WIN32: u32 = 2;
const DEVICE_LOCAL: u32 = 1;
const HOST_VISIBLE: u32 = 2;
const HOST_COHERENT: u32 = 4;
const HOST_CACHED: u32 = 8;
const READ_WRITE: u32 = 3;
const ALIAS_SPACE: u32 = 0x8000_0000;
const STRIDE: usize = 2 << 30;
const CHUNK: usize = 0xf000_0000;

#[repr(C)]
struct ApplicationInfo {
    kind: u32,
    next: *const c_void,
    name: *const u8,
    version: u32,
    engine: *const u8,
    engine_version: u32,
    api: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    application: *const ApplicationInfo,
    layers: u32,
    layer_names: *const *const u8,
    extensions: u32,
    extension_names: *const *const u8,
}

#[repr(C)]
struct DeviceProperties {
    api: u32,
    driver: u32,
    vendor: u32,
    device: u32,
    kind: u32,
    name: [u8; 256],
    rest: [u64; 128],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryType {
    flags: u32,
    heap: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryHeap {
    size: u64,
    flags: u32,
}

#[repr(C)]
struct MemoryProperties {
    types: u32,
    kinds: [MemoryType; 32],
    heaps: u32,
    heap: [MemoryHeap; 16],
}

#[repr(C)]
struct QueueCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    family: u32,
    count: u32,
    priorities: *const f32,
}

#[repr(C)]
struct DeviceCreateInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    queues: u32,
    queue_infos: *const QueueCreateInfo,
    layers: u32,
    layer_names: *const *const u8,
    extensions: u32,
    extension_names: *const *const u8,
    features: *const c_void,
}

#[repr(C)]
struct ExportInfo {
    kind: u32,
    next: *const c_void,
    handles: u32,
}

#[repr(C)]
struct AllocateInfo {
    kind: u32,
    next: *const c_void,
    size: u64,
    memory_type: u32,
}

#[repr(C)]
struct AliasInfo {
    kind: u32,
    next: *const c_void,
    address: *mut c_void,
    size: u64,
    protection: u32,
}

#[repr(C)]
struct MapInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    memory: u64,
    offset: u64,
    size: u64,
}

#[repr(C)]
struct UnmapInfo {
    kind: u32,
    next: *const c_void,
    flags: u32,
    memory: u64,
}

type CreateInstance = unsafe extern "system" fn(*const InstanceCreateInfo, *const c_void, *mut usize) -> i32;
type EnumerateDevices = unsafe extern "system" fn(usize, *mut u32, *mut usize) -> i32;
type GetProperties = unsafe extern "system" fn(usize, *mut DeviceProperties);
type GetMemoryProperties = unsafe extern "system" fn(usize, *mut MemoryProperties);
type CreateDevice = unsafe extern "system" fn(usize, *const DeviceCreateInfo, *const c_void, *mut usize) -> i32;
type Allocate = unsafe extern "system" fn(usize, *const AllocateInfo, *const c_void, *mut u64) -> i32;
type Free = unsafe extern "system" fn(usize, u64, *const c_void);
type Map = unsafe extern "system" fn(usize, u64, u64, u64, u32, *mut *mut c_void) -> i32;
type Map2 = unsafe extern "system" fn(usize, *const MapInfo, *mut *mut c_void) -> i32;
type Unmap2 = unsafe extern "system" fn(usize, *const UnmapInfo) -> i32;

pub(crate) static VRAM_READ: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

static VRAM_LOW: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
static VRAM_HIGH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct Pool {
    vram: bool,
    memory: u64,
    bytes: u64,
    views: u64,
    dead: bool,
    places: Vec<(usize, usize)>,
}

struct State {
    pools: Vec<Option<Pool>>,
    views: BTreeMap<usize, (usize, u32)>,
    spans: BTreeMap<usize, usize>,
    cpu_spans: BTreeMap<usize, usize>,
    mirrors: BTreeMap<usize, (ID3D12Heap, ID3D12Resource, u64)>,
    foreign: BTreeMap<usize, usize>,
    phases: BTreeMap<(usize, u64), ID3D12Resource>,
    refused: bool,
    seq: u64,
    changed: VecDeque<(u64, usize, usize)>,
}

const CHANGES: usize = 1 << 16;

impl State {
    fn touched(&mut self, at: usize, len: usize) {
        self.seq += 1;
        if self.changed.len() == CHANGES {
            self.changed.pop_front();
        }
        self.changed.push_back((self.seq, at, len));
    }
}

struct Um {
    device: usize,
    allocate: Allocate,
    free: Free,
    map: Map,
    map2: Map2,
    unmap2: Unmap2,
    ram: u32,
    vram: Option<u32>,
    anchor: u64,
    state: Mutex<State>,
}

unsafe impl Send for Um {}
unsafe impl Sync for Um {}

static STATS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

const PAGES: usize = 1 << 21;
static COVER: [AtomicU64; PAGES / 64] = [const { AtomicU64::new(0) }; PAGES / 64];
static WANTED: AtomicU64 = AtomicU64::new(0);
static MIRRORED: AtomicU64 = AtomicU64::new(0);
static MIRROR_GPU: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
static MIRROR_BUFFER: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static MIRROR_HEAP: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static SHOWN: [AtomicU64; PAGES / 64] = [const { AtomicU64::new(0) }; PAGES / 64];
static CARD: [AtomicU64; PAGES / 64] = [const { AtomicU64::new(0) }; PAGES / 64];
// The alias ABI installs 4 KiB pages, including partial console (64 KiB)
// pages. Counting the enclosing console page would misattribute its RAM
// neighbours to the BAR and would not prove a complete GPU mapping.
const PTE: usize = 4096;
static VRAM_PTES: [AtomicU64; SPACE.1 / (64 * PTE)] = [const { AtomicU64::new(0) }; SPACE.1 / (64 * PTE)];

fn pte_words(a: usize, b: usize) -> impl Iterator<Item = (usize, u64)> {
    let (first, end) = (a / PTE, b.div_ceil(PTE));
    (first / 64..end.div_ceil(64)).map(move |i| {
        let lo = first.saturating_sub(i * 64);
        let hi = (end - i * 64).min(64);
        (i, (u64::MAX << lo) & (u64::MAX >> (64 - hi)))
    })
}

fn mark_vram(at: usize, len: usize, set: bool) {
    let (a, b) = (at.max(SPACE.0), at.saturating_add(len).min(SPACE.1));
    if a >= b { return; }
    for (i, mask) in pte_words(a, b) {
        if mask == u64::MAX {
            VRAM_PTES[i].store(if set { mask } else { 0 }, Ordering::Release);
        } else if set {
            VRAM_PTES[i].fetch_or(mask, Ordering::Release);
        } else {
            VRAM_PTES[i].fetch_and(!mask, Ordering::Release);
        }
    }
}

fn on_card(at: usize) -> bool {
    let page = at / PTE;
    VRAM_PTES[page / 64].load(Ordering::Acquire) & (1u64 << (page % 64)) != 0
}

static REFUSED: AtomicBool = AtomicBool::new(false);

fn pages(at: usize, len: usize) -> std::ops::Range<usize> {
    (at >> 16)..(at.saturating_add(len).saturating_add(K64 - 1) >> 16).min(PAGES)
}

fn mark(map: &[AtomicU64], at: usize, len: usize, on: bool) {
    for page in pages(at, len) {
        let bit = 1u64 << (page % 64);
        if on {
            map[page / 64].fetch_or(bit, Ordering::Release);
        } else {
            map[page / 64].fetch_and(!bit, Ordering::Release);
        }
    }
}

fn marked(map: &[AtomicU64], at: usize, len: usize) -> impl Iterator<Item = bool> + '_ {
    pages(at, len.max(1)).map(|page| map[page / 64].load(Ordering::Acquire) & 1 << (page % 64) != 0)
}

fn inside(at: usize, len: usize) -> bool {
    at >= SPACE.0 && at.checked_add(len.max(1)).is_some_and(|end| end <= SPACE.1)
}

fn card(pool: &Pool) {
    let alone = pool.vram && !pool.dead && pool.places.len() == 1;
    for &(at, len) in &pool.places {
        mark(&CARD, at, len, alone);
    }
}

fn recover(state: &State, at: usize, len: usize) {
    let first = at >> 16;
    let last = (at.saturating_add(len).saturating_add(K64 - 1) >> 16).min(PAGES);
    for page in first..last {
        let bit = 1u64 << (page % 64);
        if covered(state, page << 16, (page + 1) << 16) {
            COVER[page / 64].fetch_or(bit, Ordering::Release);
        } else {
            COVER[page / 64].fetch_and(!bit, Ordering::Release);
        }
    }
}

fn want(at: usize, len: usize) {
    if at < SPACE.0 || len == 0 {
        return;
    }
    let first = (at - SPACE.0) / STRIDE;
    let last = (at + len - 1 - SPACE.0) / STRIDE;
    for chunk in first..=last.min(63) {
        WANTED.fetch_or(1 << chunk, Ordering::Release);
    }
}

pub(crate) fn covers(at: u64, len: u64) -> bool {
    let end = at.saturating_add(len.max(1));
    if at < SPACE.0 as u64 || end > SPACE.1 as u64 {
        return false;
    }
    let (first, last) = ((at >> 16) as usize, ((end - 1) >> 16) as usize);
    (first..=last).all(|page| COVER[page / 64].load(Ordering::Acquire) & 1 << (page % 64) != 0)
}

pub(crate) fn gpu_at(at: u64, len: u64) -> Option<u64> {
    if !covers(at, len) {
        return None;
    }
    let chunk = (at as usize - SPACE.0) / STRIDE;
    let base = (SPACE.0 + chunk * STRIDE) as u64;
    if at - base + len > CHUNK as u64 {
        return None;
    }
    match MIRROR_GPU.get(chunk)?.load(Ordering::Acquire) {
        0 => None,
        gpu => Some(gpu + (at - base)),
    }
}

pub(crate) fn mirror_at(at: u64, len: u64) -> Option<(ID3D12Resource, u64)> {
    if !covers(at, len) {
        return None;
    }
    let chunk = (at as usize - SPACE.0) / STRIDE;
    let base = (SPACE.0 + chunk * STRIDE) as u64;
    if at - base + len > CHUNK as u64 {
        return None;
    }
    if let Some((_, buffer, _)) = mirrored(chunk) {
        return Some((buffer, at - base));
    }
    let um = get()?;
    let mut state = um.state();
    if !state.mirrors.contains_key(&chunk) {
        if state.refused {
            return None;
        }
        mirror(&mut state, chunk)?;
    }
    let (_, buffer, _) = state.mirrors.get(&chunk)?;
    Some((buffer.clone(), at - base))
}

pub(crate) fn mirrors() -> Vec<(usize, ID3D12Resource, u64)> {
    let Some(um) = get() else {
        return Vec::new();
    };
    let state = um.state();
    state.mirrors.iter().map(|(&chunk, (_, buffer, _))| (chunk, buffer.clone(), CHUNK.min(SPACE.1 - SPACE.0 - chunk * STRIDE) as u64)).collect()
}

pub(crate) fn mirror_wanted() {
    let missing = WANTED.load(Ordering::Acquire) & !MIRRORED.load(Ordering::Acquire);
    if missing == 0 {
        return;
    }
    let Some(um) = get() else {
        return;
    };
    let mut state = um.state();
    if state.refused {
        return;
    }
    for chunk in 0..64 {
        if missing & 1 << chunk != 0 && mirror(&mut state, chunk).is_none() {
            return;
        }
    }
}

pub(crate) fn own(bytes: usize, card: bool) -> Option<usize> {
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    const REACH: usize = 0xE000_0000;
    get()?;
    let (lowest, highest) = if card { (REACH, SPACE.1) } else { (SPACE.0, REACH) };
    let mut requirements = mem::MEM_ADDRESS_REQUIREMENTS {
        LowestStartingAddress: lowest as *mut c_void,
        HighestEndingAddress: (highest - 1) as *mut c_void,
        Alignment: CELL,
    };
    let mut parameter = mem::MEM_EXTENDED_PARAMETER::default();
    parameter.Anonymous1._bitfield = mem::MemExtendedParameterAddressRequirements as u64;
    parameter.Anonymous2.Pointer = (&raw mut requirements).cast();
    let bytes = bytes.next_multiple_of(CELL);
    let at = unsafe {
        mem::VirtualAlloc2(GetCurrentProcess(), std::ptr::null_mut(), bytes, mem::MEM_RESERVE | mem::MEM_COMMIT, mem::PAGE_READWRITE, &mut parameter, 1)
    } as usize;
    if at == 0 {
        return None;
    }
    let (id, _) = pool(bytes as u64, card)?;
    if !alias(id, 0, bytes, at) {
        pool_gone(id);
        return None;
    }
    span(at, bytes);
    Some(at)
}

fn symbol<T: Copy>(library: *mut c_void, name: &std::ffi::CStr) -> Option<T> {
    assert_eq!(size_of::<T>(), size_of::<usize>());
    let at = unsafe { GetProcAddress(library, name.as_ptr().cast()) }?;
    Some(unsafe { std::mem::transmute_copy(&at) })
}

fn card_memory(memory: &MemoryProperties) -> Option<u32> {
    if !std::env::var("VICTORY_UNIFIED_VRAM").is_ok_and(|v| v.trim() == "1") {
        return None;
    }
    let kinds = &memory.kinds[..memory.types as usize];
    let kind = kinds.iter().position(|k| k.flags == DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT)?;
    let reached = memory.heap[kinds[kind].heap as usize].size;
    if reached < 4 << 30 {
        tracing::warn!(
            mb = reached >> 20,
            "unified memory: the CPU reaches too little of the card's own memory (resizable BAR off); title memory stays in RAM"
        );
        return None;
    }
    Some(kind as u32)
}

fn load() -> Option<Um> {
    if std::env::var("VICTORY_UNIFIED").is_ok_and(|v| v.trim() == "0") {
        return None;
    }
    let library = unsafe { LoadLibraryA(c"vulkan-1.dll".as_ptr().cast()) };
    if library.is_null() {
        return None;
    }
    let create_instance: CreateInstance = symbol(library, c"vkCreateInstance")?;
    let enumerate: EnumerateDevices = symbol(library, c"vkEnumeratePhysicalDevices")?;
    let properties: GetProperties = symbol(library, c"vkGetPhysicalDeviceProperties")?;
    let memory_properties: GetMemoryProperties = symbol(library, c"vkGetPhysicalDeviceMemoryProperties")?;
    let create_device: CreateDevice = symbol(library, c"vkCreateDevice")?;
    let allocate: Allocate = symbol(library, c"vkAllocateMemory")?;
    let free: Free = symbol(library, c"vkFreeMemory")?;
    let map: Map = symbol(library, c"vkMapMemory")?;
    let map2: Map2 = symbol(library, c"vkMapMemory2")?;
    let unmap2: Unmap2 = symbol(library, c"vkUnmapMemory2")?;

    let application = ApplicationInfo {
        kind: APPLICATION_INFO,
        next: std::ptr::null(),
        name: c"victory".as_ptr().cast(),
        version: 0,
        engine: c"victory".as_ptr().cast(),
        engine_version: 0,
        api: API_1_4,
    };
    let instance_info = InstanceCreateInfo {
        kind: INSTANCE_CREATE_INFO,
        next: std::ptr::null(),
        flags: 0,
        application: &application,
        layers: 0,
        layer_names: std::ptr::null(),
        extensions: 0,
        extension_names: std::ptr::null(),
    };
    let mut instance = 0usize;
    if unsafe { create_instance(&instance_info, std::ptr::null(), &mut instance) } != 0 {
        return None;
    }
    let mut count = 8u32;
    let mut devices = [0usize; 8];
    unsafe { enumerate(instance, &mut count, devices.as_mut_ptr()) };
    let mut picked = None;
    for &physical in &devices[..count as usize] {
        let mut p: DeviceProperties = unsafe { std::mem::zeroed() };
        unsafe { properties(physical, &mut p) };
        if p.vendor == 0x1002 && p.kind == 2 {
            picked = Some(physical);
            break;
        }
    }
    let physical = picked?;
    let mut memory: MemoryProperties = unsafe { std::mem::zeroed() };
    unsafe { memory_properties(physical, &mut memory) };
    let kinds = &memory.kinds[..memory.types as usize];
    let ram = kinds.iter().position(|k| k.flags == HOST_VISIBLE | HOST_COHERENT | HOST_CACHED)? as u32;
    let vram = card_memory(&memory);

    let priority = 1.0f32;
    let queue = QueueCreateInfo {
        kind: DEVICE_QUEUE_CREATE_INFO,
        next: std::ptr::null(),
        flags: 0,
        family: 0,
        count: 1,
        priorities: &priority,
    };
    let extension = c"VK_KHR_external_memory_win32".as_ptr().cast::<u8>();
    let device_info = DeviceCreateInfo {
        kind: DEVICE_CREATE_INFO,
        next: std::ptr::null(),
        flags: 0,
        queues: 1,
        queue_infos: &queue,
        layers: 0,
        layer_names: std::ptr::null(),
        extensions: 1,
        extension_names: &extension,
        features: std::ptr::null(),
    };
    let mut device = 0usize;
    if unsafe { create_device(physical, &device_info, std::ptr::null(), &mut device) } != 0 {
        return None;
    }
    let mut um = Um {
        device,
        allocate,
        free,
        map,
        map2,
        unmap2,
        ram,
        vram,
        anchor: 0,
        state: Mutex::new(State {
            pools: Vec::new(),
            views: BTreeMap::new(),
            spans: BTreeMap::new(),
            cpu_spans: BTreeMap::new(),
            mirrors: BTreeMap::new(),
            foreign: BTreeMap::new(),
            phases: BTreeMap::new(),
            refused: false,
            seq: 0,
            changed: VecDeque::new(),
        }),
    };
    let (anchor, window, _) = um.memory(K64 as u64, false)?;
    um.anchor = anchor;
    let probe = unsafe { mem::VirtualAlloc(std::ptr::null(), K64, mem::MEM_RESERVE | mem::MEM_COMMIT, mem::PAGE_READWRITE) };
    if probe.is_null() {
        return None;
    }
    let shown = um.show(anchor, 0, K64, probe as usize) && unsafe {
        (window as *mut u64).write_volatile(0x756e_6966_6965_6421);
        (probe as *const u64).read_volatile() == 0x756e_6966_6965_6421
    };
    um.hide(probe as usize, K64);
    unsafe { mem::VirtualFree(probe, 0, mem::MEM_RELEASE) };
    if !shown {
        tracing::info!("unified memory: the driver does not show card memory at an address of ours; copies as before");
        return None;
    }
    if !um.declare(SPACE.0, SPACE.1 - SPACE.0) {
        tracing::info!("unified memory: the driver does not take the title's address space; copies as before");
        return None;
    }
    tracing::info!(ram, ?vram, "unified memory: title graphics memory is the card driver's own, shown to the title where it asks");
    Some(um)
}

fn get() -> Option<&'static Um> {
    static UM: OnceLock<Option<Um>> = OnceLock::new();
    UM.get_or_init(load).as_ref()
}

impl Um {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn memory(&self, bytes: u64, combined: bool) -> Option<(u64, usize, bool)> {
        if combined
            && let Some(kind) = self.vram
            && let Some((memory, window)) = self.memory_of(bytes, kind)
        {
            return Some((memory, window, true));
        }
        self.memory_of(bytes, self.ram).map(|(memory, window)| (memory, window, false))
    }

    fn memory_of(&self, bytes: u64, memory_type: u32) -> Option<(u64, usize)> {
        let export = ExportInfo { kind: EXPORT_MEMORY_ALLOCATE_INFO, next: std::ptr::null(), handles: OPAQUE_WIN32 };
        let info = AllocateInfo {
            kind: MEMORY_ALLOCATE_INFO,
            next: (&raw const export).cast(),
            size: bytes,
            memory_type,
        };
        let mut memory = 0u64;
        let allocated = unsafe { (self.allocate)(self.device, &info, std::ptr::null(), &mut memory) };
        if allocated != 0 {
            tracing::warn!(mb = bytes >> 20, memory_type, result = allocated, "unified memory: the driver gave no memory");
            return None;
        }
        let mut window = std::ptr::null_mut();
        let mapped = unsafe { (self.map)(self.device, memory, 0, u64::MAX, 0, &mut window) };
        if mapped != 0 || window.is_null() {
            tracing::warn!(mb = bytes >> 20, memory_type, result = mapped, "unified memory: the driver's memory not mapped");
            unsafe { (self.free)(self.device, memory, std::ptr::null()) };
            return None;
        }
        Some((memory, window as usize))
    }

    fn show(&self, memory: u64, offset: u64, len: usize, at: usize) -> bool {
        let alias = AliasInfo {
            kind: MEMORY_ALIAS_INFO_RADV,
            next: std::ptr::null(),
            address: at as *mut c_void,
            size: 0,
            protection: READ_WRITE,
        };
        let info = MapInfo { kind: MEMORY_MAP_INFO, next: (&raw const alias).cast(), flags: 0, memory, offset, size: len as u64 };
        let mut out = std::ptr::null_mut();
        unsafe { (self.map2)(self.device, &info, &mut out) == 0 && out as usize == at }
    }

    fn declare(&self, at: usize, len: usize) -> bool {
        let alias = AliasInfo {
            kind: MEMORY_ALIAS_INFO_RADV,
            next: std::ptr::null(),
            address: at as *mut c_void,
            size: 0,
            protection: ALIAS_SPACE,
        };
        let info = MapInfo { kind: MEMORY_MAP_INFO, next: (&raw const alias).cast(), flags: 0, memory: self.anchor, offset: 0, size: len as u64 };
        let mut out = std::ptr::null_mut();
        unsafe { (self.map2)(self.device, &info, &mut out) == 0 }
    }

    fn hide(&self, at: usize, len: usize) -> bool {
        let alias = AliasInfo {
            kind: MEMORY_ALIAS_INFO_RADV,
            next: std::ptr::null(),
            address: at as *mut c_void,
            size: len as u64,
            protection: 0,
        };
        let info = UnmapInfo { kind: MEMORY_UNMAP_INFO, next: (&raw const alias).cast(), flags: 0, memory: self.anchor };
        unsafe { (self.unmap2)(self.device, &info) == 0 }
    }
}

pub fn on() -> bool {
    get().is_some()
}

pub fn pool(bytes: u64, combined: bool) -> Option<(u32, usize)> {
    let um = get()?;
    let (memory, window, vram) = um.memory(bytes, combined)?;
    let mut state = um.state();
    let id = state.pools.iter().position(|p| p.is_none()).unwrap_or(state.pools.len());
    let pool = Some(Pool { memory, bytes, vram, views: 0, dead: false, places: Vec::new() });
    if id == state.pools.len() {
        state.pools.push(pool);
    } else {
        state.pools[id] = pool;
    }
    STATS[if vram { 1 } else { 0 }].fetch_add(bytes, Ordering::Relaxed);
    tracing::debug!(id, kb = bytes >> 10, vram, "unified memory: a pool of title memory in the card driver's own");
    Some((id as u32, window))
}

pub fn pool_gone(id: u32) {
    let Some(um) = get() else {
        return;
    };
    let mut state = um.state();
    let Some(Some(pool)) = state.pools.get_mut(id as usize) else {
        return;
    };
    pool.dead = true;
    card(pool);
    if pool.views == 0 {
        let memory = pool.memory;
        state.pools[id as usize] = None;
        drop(state);
        unsafe { (um.free)(um.device, memory, std::ptr::null()) };
    }
}

pub fn alias(id: u32, offset: u64, len: usize, at: usize) -> bool {
    let Some(um) = get() else {
        return false;
    };
    let mut state = um.state();
    let Some(Some(pool)) = state.pools.get(id as usize) else {
        return false;
    };
    if offset + len as u64 > pool.bytes {
        return false;
    }
    let memory = pool.memory;
    let vram = pool.vram;
    forget(um, &mut state, at, len);
    state.touched(at, len);
    if !um.show(memory, offset, len, at) {
        return false;
    }
    if vram {
        VRAM_LOW.fetch_min(at, Ordering::Relaxed);
        VRAM_HIGH.fetch_max(at + len, Ordering::Relaxed);
        // The winsys accepted this mapping with aligned address, offset and size.
        mark_vram(at, len, true);
        if crate::texture::action_memory::on() { tracing::info!(start=at, bytes=len, physical=id, "IBS card map"); }
    }
    want_visible(&state, at, len);
    let first = state.views.is_empty();
    state.views.insert(at, (len, id));
    if first {
        let spans: Vec<(usize, usize)> = state.spans.iter().map(|(&at, &len)| (at, len)).collect();
        for (at, len) in spans {
            recover(&state, at, len);
        }
    }
    mark(&SHOWN, at, len, true);
    if let Some(Some(pool)) = state.pools.get_mut(id as usize) {
        pool.views += 1;
        if pool.vram {
            pool.places.push((at, len));
            card(pool);
        }
    }
    STATS[2].fetch_add(1, Ordering::Relaxed);
    true
}

fn forget(um: &Um, state: &mut State, at: usize, len: usize) {
    let end = at + len;
    let first = state.views.range(..=at).next_back().filter(|(base, view)| *base + view.0 > at).map(|(base, _)| *base);
    let hit: Vec<usize> = first.into_iter().chain(state.views.range(at + 1..end).map(|(base, _)| *base)).collect();
    for base in hit {
        let Some((bytes, id)) = state.views.remove(&base) else {
            continue;
        };
        let mut pieces = 0;
        let mut left: [Option<(usize, usize)>; 2] = [None; 2];
        if base < at {
            state.views.insert(base, (at - base, id));
            left[0] = Some((base, at - base));
            pieces += 1;
        }
        if base + bytes > end {
            state.views.insert(end, (base + bytes - end, id));
            left[1] = Some((end, base + bytes - end));
            pieces += 1;
        }
        let (from, to) = (base.max(at), (base + bytes).min(end));
        mark(&SHOWN, from, to - from, false);
        let Some(Some(pool)) = state.pools.get_mut(id as usize) else {
            continue;
        };
        if pool.vram {
            mark(&CARD, from, to - from, false);
            mark_vram(from, to - from, false);
            if crate::texture::action_memory::on() { tracing::info!(start=from, bytes=to-from, "IBS card unmap"); }
            pool.places.retain(|&(place, _)| place != base);
            pool.places.extend(left.into_iter().flatten());
            card(pool);
        }
        pool.views = pool.views + pieces - 1;
        if pool.dead && pool.views == 0 {
            let memory = pool.memory;
            state.pools[id as usize] = None;
            unsafe { (um.free)(um.device, memory, std::ptr::null()) };
        }
    }
}

pub fn unalias(at: usize, len: usize) -> bool {
    let Some(um) = get() else {
        return false;
    };
    let mut state = um.state();
    state.touched(at, len);
    let done = um.hide(at, len);
    forget(um, &mut state, at, len);
    STATS[3].fetch_add(1, Ordering::Relaxed);
    done
}

fn want_visible(state: &State, at: usize, len: usize) {
    if state.cpu_spans.is_empty() {
        want(at, len);
        return;
    }
    let end = at.saturating_add(len);
    let mut p = at;
    if let Some((&base, &bytes)) = state.cpu_spans.range(..=at).next_back() {
        p = p.max(base.saturating_add(bytes).min(end));
    }
    if p >= end { return; }
    for (&base, &bytes) in state.cpu_spans.range(p..end) {
        if base > p { want(p, base - p); }
        p = p.max(base.saturating_add(bytes));
    }
    if p < end { want(p, end - p); }
}

pub fn reservation(at: usize, len: usize, cpu: bool) {
    static ONLY: OnceLock<bool> = OnceLock::new();
    if !cpu || !*ONLY.get_or_init(|| crate::switch::on("VICTORY_CPU_RESERVATIONS")) {
        span(at, len);
        return;
    }
    let Some(um) = get() else { return; };
    let mut state = um.state();
    state.cpu_spans.insert(at, len);
    cpu_reservation::registered(at, len, true);
    tracing::info!(start = format_args!("{at:#x}"), bytes = len, "CPU-only physical page reservation does not need a graphics mirror");
}

pub fn span(at: usize, len: usize) {
    let Some(um) = get() else {
        return;
    };
    let mut state = um.state();
    state.spans.insert(at, len);
    recover(&state, at, len);
    drop(state);
    tracing::debug!(at = format_args!("{at:#x}"), kb = len >> 10, "unified memory: a stretch of title addresses");
}

pub fn span_gone(at: usize) {
    let Some(um) = get() else {
        return;
    };
    let mut state = um.state();
    if let Some(len) = state.cpu_spans.remove(&at) { cpu_reservation::registered(at, len, false); }
    if let Some(len) = state.spans.remove(&at) {
        recover(&state, at, len);
    }
}

pub fn holds(at: usize, len: usize) -> bool {
    inside(at, len) && marked(&SHOWN, at, len).all(|shown| shown)
}

pub(crate) fn measure_reads() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CARD_READERS").is_some_and(|v| v == "1"))
}

type Site = std::panic::Location<'static>;
static SITES: [std::sync::atomic::AtomicPtr<Site>; 512] = [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 512];
static SITE_BYTES: [AtomicU64; 512] = [const { AtomicU64::new(0) }; 512];

pub(crate) fn count_read(from: &'static Site, at: usize, len: usize) {
    if !measure_reads() { return; }
    let a = at.max(VRAM_LOW.load(Ordering::Relaxed)).max(SPACE.0);
    let b = at.saturating_add(len).min(VRAM_HIGH.load(Ordering::Relaxed)).min(SPACE.1);
    if a >= b { return; }
    let n = if a / PTE == (b-1) / PTE {
        if !on_card(a) { return; }
        b-a
    } else {
        let leading = (PTE - a%PTE)%PTE;
        let trailing = b%PTE;
        let (lo, hi) = (a+leading, b-trailing);
        let whole = if lo < hi {
            pte_words(lo, hi).map(|(i,mask)| (VRAM_PTES[i].load(Ordering::Acquire)&mask).count_ones() as usize * PTE).sum::<usize>()
        } else { 0 };
        whole + usize::from(leading != 0 && on_card(a)) * leading + usize::from(trailing != 0 && on_card(b-1)) * trailing
    };
    if n == 0 { return; }
    let addr = std::ptr::from_ref(from).cast_mut();
    for j in 0..SITES.len() {
        let i = (((addr as usize) >> 4) + j) % SITES.len();
        let old = SITES[i].load(Ordering::Acquire);
        if old == addr || (old.is_null() && SITES[i].compare_exchange(old, addr, Ordering::AcqRel, Ordering::Acquire).is_ok()) {
            SITE_BYTES[i].fetch_add(n as u64,Ordering::Relaxed);
            return;
        }
    }
}

fn read_counts(now: u64) {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let prev = LAST.load(Ordering::Relaxed);
    if now < prev+1000 || LAST.compare_exchange(prev,now,Ordering::AcqRel,Ordering::Relaxed).is_err() { return; }
    let mut callers = String::new();
    use std::fmt::Write;
    for (site,n) in SITES.iter().zip(&SITE_BYTES) {
        let ptr = site.load(Ordering::Acquire);
        let count = n.swap(0,Ordering::Relaxed);
        if !ptr.is_null() && count != 0 {
            let loc = unsafe { &*ptr };
            let _ = write!(callers," {}:{}={count}",loc.file().rsplit("src/").next().unwrap_or(loc.file()),loc.line());
        }
    }
    tracing::info!(frame=crate::frame::current(),"CPU bytes read from card by caller:{callers}");
}

pub(crate) fn resident(at: usize,len: usize) -> bool {
    let end = at.saturating_add(len);
    len != 0 && at >= VRAM_LOW.load(Ordering::Relaxed) && end <= VRAM_HIGH.load(Ordering::Relaxed)
        && inside(at,len) && pte_words(at,end).all(|(i,mask)| VRAM_PTES[i].load(Ordering::Acquire)&mask == mask)
}

/// Unlike `dedicated_vram`, includes pages shared through another title alias.
/// A span crossing pools can still be used directly when its whole GPU mirror exists.
pub(crate) fn touches_card(at: usize, len: usize) -> bool {
    let a = at.max(VRAM_LOW.load(Ordering::Relaxed)).max(SPACE.0);
    let b = at.saturating_add(len).min(VRAM_HIGH.load(Ordering::Relaxed)).min(SPACE.1);
    a < b && pte_words(a,b).any(|(i,mask)| VRAM_PTES[i].load(Ordering::Acquire)&mask != 0)
}

pub(crate) fn card_pages(at: usize, len: usize) -> usize {
    if len == 0 || !inside(at, len) { return 0; }
    marked(&CARD, at, len).filter(|&card| card).count()
}

pub(crate) fn dedicated_vram(at: usize, len: usize) -> bool {
    let Some(end) = at.checked_add(len) else { return false };
    if at < VRAM_LOW.load(Ordering::Relaxed) || end > VRAM_HIGH.load(Ordering::Relaxed) || len == 0 {
        return false;
    }
    inside(at, len) && marked(&CARD, at, len).all(|card| card)
}

pub(crate) fn blind(start: usize, end: usize) -> bool {
    let (start, end) = (start.max(SPACE.0), end.min(SPACE.1));
    start < end && marked(&SHOWN, start, end - start).any(|shown| shown)
}

pub fn foreign(at: usize, len: usize) {
    let Some(um) = get() else {
        return;
    };
    let mut state = um.state();
    foreign_gone_in(&mut state, at, len);
    state.foreign.insert(at, len);
    recover(&state, at, len);
}

pub fn foreign_gone(at: usize, len: usize) {
    let Some(um) = get() else {
        return;
    };
    let mut state = um.state();
    foreign_gone_in(&mut state, at, len);
    recover(&state, at, len);
}

fn foreign_gone_in(state: &mut State, at: usize, len: usize) {
    let end = at + len;
    let first = state.foreign.range(..=at).next_back().filter(|(base, bytes)| *base + **bytes > at).map(|(base, _)| *base);
    let hit: Vec<usize> = first.into_iter().chain(state.foreign.range(at + 1..end).map(|(base, _)| *base)).collect();
    for base in hit {
        let Some(bytes) = state.foreign.remove(&base) else {
            continue;
        };
        if base < at {
            state.foreign.insert(base, at - base);
        }
        if base + bytes > end {
            state.foreign.insert(end, base + bytes - end);
        }
    }
}

fn covered(state: &State, start: usize, end: usize) -> bool {
    if start < SPACE.0 || end > SPACE.1 || state.views.is_empty() {
        return false;
    }
    if state.foreign.range(..end).next_back().is_some_and(|(base, bytes)| base + bytes > start) {
        return false;
    }
    let mut at = start;
    while at < end {
        match state.spans.range(..=at).next_back() {
            Some((&base, &len)) if at < base + len => at = base + len,
            _ => return false,
        }
    }
    true
}

pub(crate) fn reach(at: u64) -> u64 {
    let Some(um) = get() else {
        return 0;
    };
    let state = um.state();
    if state.mirrors.is_empty() {
        return 0;
    }
    let mut end = at as usize;
    while let Some((&base, &len)) = state.spans.range(..=end).next_back() {
        if end >= base + len {
            break;
        }
        end = base + len;
    }
    if state.foreign.range(..=at as usize).next_back().is_some_and(|(base, bytes)| base + bytes > at as usize) {
        return 0;
    }
    if let Some((&first, _)) = state.foreign.range(at as usize..end).next() {
        end = first;
    }
    if end as u64 > at { end as u64 } else { 0 }
}

pub fn refuse() {
    let Some(um) = get() else {
        return;
    };
    let mut state = um.state();
    REFUSED.store(true, Ordering::Release);
    if !state.refused {
        state.refused = true;
        tracing::warn!("unified memory: title memory the driver could not back; what is asked for from here on is copied as before");
    }
}

pub(crate) fn piece_end(start: u64) -> u64 {
    let base = SPACE.0 + (start as usize).saturating_sub(SPACE.0) / STRIDE * STRIDE;
    (base + CHUNK).min(SPACE.1) as u64
}

fn mirror(state: &mut State, chunk: usize) -> Option<()> {
    static ARENA: OnceLock<bool> = OnceLock::new();
    if *ARENA.get_or_init(|| crate::switch::on("VICTORY_MIRROR_ARENA")) {
        let first = state.mirrors.last_key_value().map_or(0, |(&n, _)| n + 1);
        for n in first..=chunk {
            mirror_piece(state, n)?;
        }
        return state.mirrors.contains_key(&chunk).then_some(());
    }
    mirror_piece(state, chunk)
}

fn mirror_piece(state: &mut State, chunk: usize) -> Option<()> {
    use windows::core::Interface;
    if state.mirrors.contains_key(&chunk) {
        return Some(());
    }
    let base = SPACE.0 + chunk * STRIDE;
    let bytes = CHUNK.min(SPACE.1 - base);
    let host = crate::host::host()?;
    let opened = crate::inplace::open(&host.device, crate::inplace::From::Part(base, bytes))
        .and_then(|heap| crate::inplace::place(&host.device, &heap, 0, bytes as u64).map(|buffer| (heap, buffer)));
    match opened {
        Ok((heap, buffer)) => {
            let gpu = unsafe { buffer.GetGPUVirtualAddress() };
            crate::host::name(&buffer, || format!("title memory unified {base:#x}+{bytes:#x}"));
            tracing::info!(
                base = format_args!("{base:#x}"),
                mb = bytes >> 20,
                gpu = format_args!("{gpu:#x}"),
                "unified memory: the card reads and writes title memory where the title has it"
            );
            STATS[4].fetch_add(1, Ordering::Relaxed);
            if let Some(shown) = MIRROR_GPU.get(chunk) {
                MIRROR_HEAP[chunk].store(heap.as_raw() as usize, Ordering::Release);
                MIRROR_BUFFER[chunk].store(buffer.as_raw() as usize, Ordering::Release);
                shown.store(gpu, Ordering::Release);
                MIRRORED.fetch_or(1 << chunk, Ordering::Release);
            }
            crate::records::mirrored(chunk, &buffer, bytes as u64);
            state.mirrors.insert(chunk, (heap, buffer, gpu));
            Some(())
        }
        Err(error) => {
            tracing::warn!(%error, base = format_args!("{base:#x}"), "unified memory: the title's addresses not opened on the card; copies as before");
            STATS[5].fetch_add(1, Ordering::Relaxed);
            state.refused = true;
            REFUSED.store(true, Ordering::Release);
            None
        }
    }
}

pub(crate) fn phase_buffer(heap: &ID3D12Heap, steps: u64) -> Option<ID3D12Resource> {
    use windows::core::Interface;
    let um = get()?;
    let mut state = um.state();
    let key = (heap.as_raw() as usize, steps);
    if let Some(buffer) = state.phases.get(&key) {
        return Some(buffer.clone());
    }
    let bytes = unsafe { heap.GetDesc() }.SizeInBytes;
    let offset = steps * K64 as u64;
    if offset >= bytes {
        return None;
    }
    let host = crate::host::host()?;
    match crate::inplace::place(&host.device, heap, offset, bytes - offset) {
        Ok(buffer) => {
            crate::host::name(&buffer, || format!("title memory unified, {steps} steps in"));
            state.phases.insert(key, buffer.clone());
            Some(buffer)
        }
        Err(error) => {
            tracing::warn!(%error, steps, "unified memory: no buffer further into the title's addresses on the card; a copy of the view's bytes as before");
            None
        }
    }
}

pub(crate) fn placeable(start: u64, len: u64) -> bool {
    !cpu_ask(start, len, "placed copy range") && !REFUSED.load(Ordering::Acquire) && covers(start, len)
}

fn mirrored(chunk: usize) -> Option<(ID3D12Heap, ID3D12Resource, u64)> {
    use windows::core::Interface;
    let gpu = MIRROR_GPU.get(chunk)?.load(Ordering::Acquire);
    let heap = MIRROR_HEAP[chunk].load(Ordering::Acquire) as *mut c_void;
    let buffer = MIRROR_BUFFER[chunk].load(Ordering::Acquire) as *mut c_void;
    if gpu == 0 || heap.is_null() || buffer.is_null() {
        return None;
    }
    let heap = unsafe { ID3D12Heap::from_raw_borrowed(&heap) }?.clone();
    let buffer = unsafe { ID3D12Resource::from_raw_borrowed(&buffer) }?.clone();
    Some((heap, buffer, gpu))
}

pub(crate) fn place_for(start: u64, len: u64, window: bool) -> Option<Placed> {
    if cpu_ask(start, len, "placed buffer") { return None; }
    if !REFUSED.load(Ordering::Acquire) && covers(start, len) {
        let chunk = (start as usize - SPACE.0) / STRIDE;
        let base = SPACE.0 + chunk * STRIDE;
        if start + len.max(1) <= (base + CHUNK.min(SPACE.1 - base)) as u64
            && let Some((heap, buffer, gpu)) = mirrored(chunk)
        {
            let bias = start - base as u64;
            crate::inplace::counted(window, len);
            STATS[6].fetch_add(1, Ordering::Relaxed);
            return Some(Placed { buffer, heap, gpu: gpu + bias, bias });
        }
    }
    let um = get()?;
    let mut state = um.state();
    if state.refused {
        return None;
    }
    if !covered(&state, start as usize, (start + len.max(1)) as usize) {
        if len >= K64 as u64 {
            let n = STATS[7].fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 32 || n.is_power_of_two() {
                let mut info: mem::MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
                unsafe { mem::VirtualQuery(start as *const c_void, &mut info, size_of_val(&info)) };
                tracing::info!(
                    start = format_args!("{start:#x}"),
                    len,
                    window,
                    allocation = format_args!("{:p}", info.AllocationBase),
                    region = format_args!("{:p}+{:#x}", info.BaseAddress, info.RegionSize),
                    kind = format_args!("{:#x}/{:#x}/{:#x}", info.State, info.Type, info.AllocationProtect),
                    "unified memory: title memory outside the unified stretches; a copy as before"
                );
            }
        }
        return None;
    }
    let chunk = (start as usize - SPACE.0) / STRIDE;
    let base = SPACE.0 + chunk * STRIDE;
    let bytes = CHUNK.min(SPACE.1 - base);
    if start + len.max(1) > (base + bytes) as u64 {
        let n = STATS[7].fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 32 || n.is_power_of_two() {
            tracing::info!(start = format_args!("{start:#x}"), len, window, "unified memory: a range longer than the card takes in one piece; a copy as before");
        }
        return None;
    }
    mirror(&mut state, chunk)?;
    let (heap, buffer, gpu) = state.mirrors.get(&chunk)?;
    let bias = start - base as u64;
    crate::inplace::counted(window, len);
    STATS[6].fetch_add(1, Ordering::Relaxed);
    Some(Placed { buffer: buffer.clone(), heap: heap.clone(), gpu: gpu + bias, bias })
}

pub(crate) fn checking() -> usize {
    static MB: OnceLock<usize> = OnceLock::new();
    *MB.get_or_init(|| std::env::var("VICTORY_UNIFIED_CHECK").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0))
}

const PIECE: usize = 1 << 20;
static CHECKED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

pub(crate) fn check(host: &crate::host::Host) {
    let budget = checking() << 20;
    let Some(um) = get() else {
        return;
    };
    if budget == 0 {
        return;
    }
    static LAST: AtomicU64 = AtomicU64::new(0);
    static CURSOR: AtomicU64 = AtomicU64::new(0);
    let now = crate::host::now_ms();
    if now < LAST.load(Ordering::Relaxed) + 1000 {
        return;
    }
    let mut pieces: Vec<(usize, usize, ID3D12Resource, u64, u32)> = Vec::new();
    let began;
    {
        let mut state = um.state();
        if state.refused {
            return;
        }
        began = state.seq;
        let cursor = CURSOR.load(Ordering::Relaxed) as usize;
        let inside = state.views.range(..cursor).next_back().filter(|(at, view)| **at + view.0 > cursor).map(|(&at, &(len, id))| (cursor, at + len - cursor, id));
        let views: Vec<(usize, usize, u32)> = inside
            .into_iter()
            .chain(state.views.range(cursor..).map(|(&at, &(len, id))| (at, len, id)))
            .chain(state.views.range(..cursor).map(|(&at, &(len, id))| (at, len.min(cursor - at), id)))
            .collect();
        let mut total = 0;
        let mut next = None;
        let mut wrapped = false;
        'views: for (at, len, id) in views {
            wrapped |= at < cursor;
            let mut from = at;
            while from < at + len {
                let chunk = (from - SPACE.0) / STRIDE;
                let base = SPACE.0 + chunk * STRIDE;
                let n = PIECE.min(at + len - from).min(base + CHUNK.min(SPACE.1 - base) - from);
                if total + n > budget {
                    next = Some(from);
                    break 'views;
                }
                if mirror(&mut state, chunk).is_none() {
                    return;
                }
                let Some((_, buffer, _)) = state.mirrors.get(&chunk) else {
                    return;
                };
                pieces.push((from, n, buffer.clone(), (from - base) as u64, id));
                total += n;
                from += n;
            }
        }
        CURSOR.store(next.unwrap_or(cursor) as u64, Ordering::Relaxed);
        if wrapped || next.is_none() {
            CHECKED[4].fetch_add(1, Ordering::Relaxed);
        }
    }
    if pieces.is_empty() {
        return;
    }
    let total: usize = pieces.iter().map(|p| p.1).sum();
    let mut before = vec![0u8; total];
    let mut readable = vec![false; pieces.len()];
    let mut out = 0;
    for (i, &(at, n, ..)) in pieces.iter().enumerate() {
        readable[i] = crate::host::read_into(at as *const u8, &mut before[out..out + n]);
        out += n;
    }
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total as u64,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut back: Option<ID3D12Resource> = None;
    let made = unsafe {
        host.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut back)
    };
    let Some(back) = back.filter(|_| made.is_ok()) else {
        tracing::warn!(mb = total >> 20, "unified memory check: no buffer to read the card's side into");
        return;
    };
    let ran = host.run_own(|list| {
        let mut out = 0u64;
        for (_, n, buffer, offset, _) in &pieces {
            unsafe { list.CopyBufferRegion(&back, out, buffer, *offset, *n as u64) };
            out += *n as u64;
        }
    });
    if ran.is_none() {
        tracing::warn!("unified memory check: the card's side could not be read");
        return;
    }
    let mut data = std::ptr::null_mut();
    if unsafe { back.Map(0, None, Some(&mut data)) }.is_err() || data.is_null() {
        tracing::warn!("unified memory check: the card's side could not be mapped");
        return;
    }
    let card = unsafe { std::slice::from_raw_parts(data as *const u8, total) };
    let mut after = vec![0u8; PIECE];
    let changed: Option<Vec<(usize, usize)>> = {
        let state = um.state();
        let all = state.changed.front().is_none_or(|&(seq, ..)| seq <= began + 1) || state.seq == began;
        all.then(|| state.changed.iter().filter(|&&(seq, ..)| seq > began).map(|&(_, at, len)| (at, len)).collect())
    };
    let (mut bytes, mut skipped, mut passing) = (0u64, 0u64, 0u64);
    let mut flagged: Vec<(usize, u32, ID3D12Resource, u64, Vec<u8>, Vec<u8>)> = Vec::new();
    let mut out = 0;
    for (i, (at, n, buffer, offset, id)) in pieces.iter().enumerate() {
        let (at, n) = (*at, *n);
        let (was, got) = (&before[out..out + n], &card[out..out + n]);
        out += n;
        let moved = changed.as_ref().is_none_or(|c| c.iter().any(|&(a, l)| a < at + n && a + l > at));
        if !readable[i] || moved || !crate::host::read_into(at as *const u8, &mut after[..n]) {
            skipped += n as u64;
            continue;
        }
        bytes += n as u64;
        if was == got {
            continue;
        }
        for page in (0..n).step_by(4096) {
            let end = (page + 4096).min(n);
            let (w, g, a) = (&was[page..end], &got[page..end], &after[page..end]);
            if (0..end - page).any(|k| w[k] == a[k] && g[k] != w[k]) {
                passing += 1;
                if flagged.len() < 1024 {
                    flagged.push((at + page, *id, buffer.clone(), offset + page as u64, w.to_vec(), g.to_vec()));
                }
            }
        }
    }
    unsafe { back.Unmap(0, None) };
    let (mut pages, mut wrong) = (0u64, 0u64);
    if !flagged.is_empty() {
        let mut second = vec![0u8; 4096];
        let mut third = vec![0u8; 4096];
        let seen: Vec<Option<Vec<u8>>> = flagged
            .iter()
            .map(|(at, _, _, _, w, _)| crate::host::read_into(*at as *const u8, &mut second[..w.len()]).then(|| second[..w.len()].to_vec()))
            .collect();
        let ran = host.run_own(|list| {
            for (i, (_, _, buffer, offset, w, _)) in flagged.iter().enumerate() {
                unsafe { list.CopyBufferRegion(&back, (i * 4096) as u64, buffer, *offset, w.len() as u64) };
            }
        });
        let mut data = std::ptr::null_mut();
        if ran.is_some() && unsafe { back.Map(0, None, Some(&mut data)) }.is_ok() && !data.is_null() {
            let card = unsafe { std::slice::from_raw_parts(data as *const u8, flagged.len() * 4096) };
            let moved: Vec<(usize, usize)> = {
                let state = um.state();
                state.changed.iter().filter(|&&(seq, ..)| seq > began).map(|&(_, at, len)| (at, len)).collect()
            };
            let mut shown = 0;
            for (i, (at, id, _, _, w, g)) in flagged.iter().enumerate() {
                let n = w.len();
                let Some(c2) = &seen[i] else {
                    continue;
                };
                if moved.iter().any(|&(a, l)| a < at + n && a + l > *at) || !crate::host::read_into(*at as *const u8, &mut third[..n]) {
                    continue;
                }
                let g2 = &card[i * 4096..i * 4096 + n];
                let stable = |k: usize| g[k] == g2[k] && w[k] == c2[k] && c2[k] == third[k] && g[k] != w[k];
                let differing = (0..n).filter(|&k| stable(k)).count();
                if differing == 0 {
                    continue;
                }
                pages += 1;
                wrong += differing as u64;
                if shown < 12 {
                    shown += 1;
                    let first = (0..n).find(|&k| stable(k)).unwrap_or(0) & !7;
                    let word = |b: &[u8]| u64::from_le_bytes(b[first..first + 8].try_into().unwrap_or([0; 8]));
                    tracing::warn!(
                        at = format_args!("{at:#x}"),
                        pool = id,
                        bytes = differing,
                        offset = first,
                        title = format_args!("{:#018x}", word(w)),
                        card = format_args!("{:#018x}", word(g)),
                        card_zero = g.iter().all(|&b| b == 0),
                        title_zero = w.iter().all(|&b| b == 0),
                        "unified memory check: the card does not read what the title has there"
                    );
                }
            }
            unsafe { back.Unmap(0, None) };
        }
    }
    CHECKED[0].fetch_add(bytes, Ordering::Relaxed);
    CHECKED[1].fetch_add(skipped, Ordering::Relaxed);
    CHECKED[2].fetch_add(pages, Ordering::Relaxed);
    CHECKED[3].fetch_add(wrong, Ordering::Relaxed);
    CHECKED[5].fetch_add(passing, Ordering::Relaxed);
    tracing::info!(
        "unified memory check: {} MB compared now ({} MB left out: remapped or not readable meanwhile), {} pages differ ({} more only while the title wrote them); so far {} MB, {} pages ({} bytes) differ, {} only in passing, {} times round all of it; {} ms",
        bytes >> 20,
        skipped >> 20,
        pages,
        passing - pages.min(passing),
        CHECKED[0].load(Ordering::Relaxed) >> 20,
        CHECKED[2].load(Ordering::Relaxed),
        CHECKED[3].load(Ordering::Relaxed),
        CHECKED[5].load(Ordering::Relaxed),
        CHECKED[4].load(Ordering::Relaxed),
        crate::host::now_ms() - now
    );
    LAST.store(crate::host::now_ms(), Ordering::Relaxed);
}

pub(crate) fn line() -> Option<String> {
    get()?;
    if measure_reads() { read_counts(crate::host::now_ms()); }
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::host::now_ms();
    let last = LAST.load(Ordering::Relaxed);
    if now < last + 10_000 || LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return None;
    }
    cpu_reservation::report();
    let s: Vec<u64> = STATS.iter().map(|s| s.load(Ordering::Relaxed)).collect();
    let (pools, views) = {
        let state = get()?.state();
        (state.pools.iter().flatten().count(), state.views.len())
    };
    let [skipped, partial, misses, read] = crate::texture::version_counts();
    Some(format!(
        "unified memory: {} MB in RAM, {} MB in VRAM in {pools} pools shown at {views} places; {} maps, {} unmaps; on the card {} ({} refused); {} title buffers and windows in place, {} of 64 KiB or more left to copies; {} MiB streamed from VRAM, {} MiB of duplicate texture checks skipped; texture versions: {} MiB skipped, {} MiB read, {partial} partial updates, {misses} audit misses",
        s[0] >> 20,
        s[1] >> 20,
        s[2],
        s[3],
        s[4],
        s[5],
        s[6],
        s[7],
        VRAM_READ[0].load(Ordering::Relaxed) >> 20,
        VRAM_READ[1].load(Ordering::Relaxed) >> 20, skipped >> 20, read >> 20
    ))
}
