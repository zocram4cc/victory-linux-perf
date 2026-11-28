//! The recorder: what the host layer makes and records with D3D12, seen at
//! the D3D12 boundary (`super::hook`), and one frame of it written down
//! (`super::snap`). On with `VICTORY_RECORD=<folder>` only; without it
//! nothing is hooked.
//!
//! From the device's creation on it keeps what objects were made from and
//! what each descriptor slot holds, and follows the states of textures.
//! Lists are recorded only while it is armed: a few frames ahead of the one
//! captured, since the title records lists before they run.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, PCWSTR};

use super::hook::{self, method, noted_entry, orig};
use super::states::{self, Change, States};
use super::wire::{self, ALL, Barrier, Cmd, Loc, NONE, Part, Place, Slot, Va, Wire, slot};
use super::{gpu, pso, snap};

type Ptr = *mut c_void;
type Out = *mut *mut c_void;

/// What to record.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Captures go to `capture-<n>` in it.
    pub folder: PathBuf,
    pub at: At,
    /// Frames to capture.
    pub frames: u32,
    /// Record lists from the start on, not only once a capture is near: for
    /// a title that submits lists it recorded long before.
    pub always: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum At {
    /// Seconds after the device was made.
    Seconds(f64),
    /// A number of presents.
    Frame(u64),
    /// When a file named `go` appears in the folder (this works in the
    /// other cases too).
    Trigger,
}

impl Settings {
    /// From `VICTORY_RECORD`, `VICTORY_RECORD_AT` (seconds, or `f<frame>`),
    /// `VICTORY_RECORD_FRAMES` and `VICTORY_RECORD_ALWAYS=1`.
    pub fn from_env() -> Option<Settings> {
        let folder = PathBuf::from(std::env::var_os("VICTORY_RECORD").filter(|v| !v.is_empty())?);
        let at = parse_at(&std::env::var("VICTORY_RECORD_AT").unwrap_or_default());
        let frames = std::env::var("VICTORY_RECORD_FRAMES")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(1);
        let always = std::env::var_os("VICTORY_RECORD_ALWAYS").is_some_and(|v| v == "1");
        Some(Settings {
            folder,
            at,
            frames: std::cmp::max(frames, 1),
            always,
        })
    }
}

fn parse_at(text: &str) -> At {
    let text = text.trim();
    if let Some(frame) = text.strip_prefix('f').and_then(|n| n.parse().ok()) {
        return At::Frame(frame);
    }
    match text.parse::<f64>() {
        Ok(s) => At::Seconds(s),
        Err(_) => At::Trigger,
    }
}

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn read<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

fn write<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

/// A resource that lives.
pub(super) struct Res {
    pub(super) id: u32,
    pub(super) ptr: usize,
    pub(super) place: Place,
    pub(super) desc: wire::Desc,
    pub(super) made_in: i32,
    pub(super) clear: Option<[u32; 5]>,
    pub(super) subs: u32,
    pub(super) seen: gpu::Seen,
    /// Made while a frame was being captured.
    pub(super) late: bool,
}

impl Res {
    pub(super) fn is_buffer(&self) -> bool {
        self.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0
    }

    /// The object, without a reference of its own.
    pub(super) fn object(&self) -> ManuallyDrop<ID3D12Resource> {
        borrow(self.ptr as Ptr)
    }
}

/// A COM object named by its address, without a reference.
pub(super) fn borrow<T: Interface>(p: Ptr) -> ManuallyDrop<T> {
    // SAFETY: the caller's live object; never released here.
    ManuallyDrop::new(unsafe { T::from_raw(p) })
}

/// A descriptor heap: where it lies and what its slots hold.
pub(super) struct Heap {
    pub(super) id: u32,
    pub(super) kind: i32,
    pub(super) count: u32,
    pub(super) flags: i32,
    cpu: usize,
    gpu: u64,
    step: usize,
    slots: *mut Slot,
}

// SAFETY: slots are written by the thread that writes the descriptor (the
// same rule as the descriptors': one writer at a time) and read as plain
// data.
unsafe impl Send for Heap {}
// SAFETY: as above.
unsafe impl Sync for Heap {}

impl Heap {
    fn new(id: u32, desc: &D3D12_DESCRIPTOR_HEAP_DESC, cpu: usize, gpu: u64, step: usize) -> Heap {
        let layout = Self::layout(desc.NumDescriptors);
        // SAFETY: a layout of at least one slot; a zeroed slot is an empty
        // one.
        let slots = unsafe { std::alloc::alloc_zeroed(layout) }.cast::<Slot>();
        if slots.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Heap {
            id,
            kind: desc.Type.0,
            count: desc.NumDescriptors,
            flags: desc.Flags.0,
            cpu,
            gpu,
            step,
            slots,
        }
    }

    fn layout(count: u32) -> std::alloc::Layout {
        std::alloc::Layout::array::<Slot>(count.max(1) as usize)
            .unwrap_or(std::alloc::Layout::new::<Slot>())
    }

    pub(super) fn visible(&self) -> bool {
        self.flags & D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE.0 != 0
    }

    pub(super) fn get(&self, i: u32) -> Slot {
        if i >= self.count {
            return Slot::EMPTY;
        }
        // SAFETY: a slot of ours.
        unsafe { self.slots.add(i as usize).read_volatile() }
    }

    fn set(&self, i: u32, s: &Slot) {
        if i < self.count {
            // SAFETY: a slot of ours.
            unsafe { self.slots.add(i as usize).write_volatile(*s) };
        }
    }

    /// All slots, as they are now.
    pub(super) fn all(&self) -> Vec<Slot> {
        let mut out = Vec::with_capacity(self.count as usize);
        // SAFETY: `count` slots of ours.
        unsafe {
            std::ptr::copy_nonoverlapping(self.slots, out.as_mut_ptr(), self.count as usize);
            out.set_len(self.count as usize);
        }
        out
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // SAFETY: as allocated.
        unsafe { std::alloc::dealloc(self.slots.cast(), Self::layout(self.count)) };
    }
}

#[derive(Default)]
pub(super) struct Heaps {
    by_cpu: BTreeMap<usize, Arc<Heap>>,
    by_gpu: BTreeMap<u64, Arc<Heap>>,
}

impl Heaps {
    fn add(&mut self, heap: Arc<Heap>) {
        let end = heap.cpu + heap.count as usize * heap.step;
        let gone: Vec<usize> = self
            .by_cpu
            .range(..end)
            .rev()
            .take_while(|(_, h)| h.cpu + h.count as usize * h.step > heap.cpu)
            .map(|(&k, _)| k)
            .collect();
        for k in gone {
            self.by_cpu.remove(&k);
        }
        if heap.visible() {
            self.by_gpu.insert(heap.gpu, heap.clone());
        }
        self.by_cpu.insert(heap.cpu, heap);
    }

    fn remove(&mut self, id: u32) {
        self.by_cpu.retain(|_, h| h.id != id);
        self.by_gpu.retain(|_, h| h.id != id);
    }

    fn cpu(&self, at: usize) -> Option<(&Arc<Heap>, u32)> {
        let (_, h) = self.by_cpu.range(..=at).next_back()?;
        let i = (at - h.cpu) / h.step;
        (i < h.count as usize).then_some((h, i as u32))
    }

    fn gpu(&self, at: u64) -> Option<(&Arc<Heap>, u32)> {
        let (_, h) = self.by_gpu.range(..=at).next_back()?;
        let i = (at - h.gpu) / h.step as u64;
        (i < u64::from(h.count)).then_some((h, i as u32))
    }

    pub(super) fn visible(&self) -> impl Iterator<Item = &Arc<Heap>> {
        self.by_gpu.values()
    }
}

/// A command list's recording.
pub(super) struct ListState {
    pub(super) kind: i32,
    inner: Mutex<ListInner>,
}

#[derive(Default)]
struct ListInner {
    /// Recorded from its reset on.
    whole: bool,
    bytes: Vec<u8>,
    changes: Vec<Change>,
    closed: Option<(Option<Arc<Vec<u8>>>, Arc<Vec<Change>>)>,
}

impl ListState {
    fn push(&self, cmd: &Cmd) {
        let mut inner = lock(&self.inner);
        if inner.whole {
            cmd.put(&mut inner.bytes);
        }
    }

    /// The recording closed last: its commands if it was recorded whole,
    /// and the states it changes. None if it was not seen closed since its
    /// reset.
    pub(super) fn closed(&self) -> Option<(Option<Arc<Vec<u8>>>, Arc<Vec<Change>>)> {
        lock(&self.inner).closed.clone()
    }
}

struct QueueState {
    kind: i32,
}

/// Shader code, written to `shaders.bin` as it is met, once each.
pub(super) struct Codes {
    file: Option<std::fs::File>,
    at: u64,
    index: HashMap<[u8; 32], u32>,
    pub(super) all: Vec<wire::Code>,
}

/// Objects by their address: ids.
#[derive(Default)]
struct Ids([RwLock<HashMap<usize, u32>>; 32]);

impl Ids {
    fn shard(&self, p: usize) -> &RwLock<HashMap<usize, u32>> {
        &self.0[(p >> 5 ^ p >> 12) % 32]
    }
    fn get(&self, p: usize) -> Option<u32> {
        read(self.shard(p)).get(&p).copied()
    }
    fn set(&self, p: usize, id: u32) {
        write(self.shard(p)).insert(p, id);
    }
    fn remove(&self, p: usize, id: u32) {
        let mut map = write(self.shard(p));
        if map.get(&p) == Some(&id) {
            map.remove(&p);
        }
    }
}

pub(super) struct Recorder {
    pub(super) settings: Settings,
    /// Where this process' capture goes.
    pub(super) folder: PathBuf,
    pub(super) began: std::time::Instant,
    next: AtomicU32,
    ids: Ids,
    pub(super) resources: RwLock<HashMap<u32, Arc<Res>>>,
    /// Buffers by their GPU address: size and id.
    addresses: RwLock<BTreeMap<u64, (u64, u32)>>,
    pub(super) descriptors: RwLock<Heaps>,
    lists: RwLock<HashMap<usize, Arc<ListState>>>,
    pub(super) heaps: Mutex<HashMap<u32, wire::Heap>>,
    pub(super) roots: Mutex<HashMap<u32, Arc<Vec<u8>>>>,
    pub(super) pipelines: Mutex<HashMap<u32, Arc<Vec<Part>>>>,
    pub(super) signatures: Mutex<HashMap<u32, wire::Signature>>,
    pub(super) queries: Mutex<HashMap<u32, wire::QueryHeap>>,
    pub(super) names: Mutex<HashMap<u32, String>>,
    pub(super) codes: Mutex<Codes>,
    pub(super) states: Mutex<States>,
    pub(super) queues: Mutex<Vec<(ID3D12CommandQueue, i32)>>,
    device: Mutex<Option<ID3D12Device>>,
    /// Lists are recorded.
    armed: AtomicBool,
    /// A frame is being captured: descriptor writes are noted, resources
    /// made are kept alive.
    pub(super) watching: AtomicBool,
    pub(super) writes: Mutex<Vec<(u32, u32, u32)>>,
    /// Resources kept alive for a capture, by id. One that dies all the same
    /// (its last reference was going when ours was taken) is taken out: see
    /// `Recorder::gone`.
    pub(super) held: Mutex<HashMap<u32, ID3D12Resource>>,
    /// What could not be recorded, with how often.
    problems: Mutex<BTreeMap<String, u64>>,
    /// Submissions and presents, one at a time.
    pub(super) submits: Mutex<snap::Phase>,
}

static RECORDER: OnceLock<Recorder> = OnceLock::new();

/// The recorder, if it is on.
pub(super) fn recorder() -> Option<&'static Recorder> {
    RECORDER.get()
}

/// Whether the recorder is on.
pub fn on() -> bool {
    RECORDER.get().is_some()
}

thread_local! {
    static OURS: Cell<u32> = const { Cell::new(0) };
}

/// While it lives, this thread's D3D12 calls are our own: not recorded.
pub(super) struct Ours;

impl Ours {
    pub(super) fn new() -> Ours {
        OURS.with(|o| o.set(o.get() + 1));
        Ours
    }
}

impl Drop for Ours {
    fn drop(&mut self) {
        OURS.with(|o| o.set(o.get() - 1));
    }
}

/// While what this returns lives, this thread's D3D12 calls are not
/// recorded.
pub(crate) fn unrecorded() -> impl Drop {
    Ours::new()
}

fn ours() -> bool {
    OURS.with(|o| o.get() > 0)
}

impl Recorder {
    fn new(settings: Settings) -> std::io::Result<Recorder> {
        std::fs::create_dir_all(&settings.folder)?;
        let mut n = 1;
        let folder = loop {
            let folder = settings.folder.join(format!("capture-{n}"));
            match std::fs::create_dir(&folder) {
                Ok(()) => break folder,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e),
            }
        };
        let file = std::fs::File::create(folder.join("shaders.bin"))?;
        Ok(Recorder {
            settings,
            folder,
            began: std::time::Instant::now(),
            next: AtomicU32::new(1),
            ids: Ids::default(),
            resources: RwLock::default(),
            addresses: RwLock::default(),
            descriptors: RwLock::default(),
            lists: RwLock::default(),
            heaps: Mutex::default(),
            roots: Mutex::default(),
            pipelines: Mutex::default(),
            signatures: Mutex::default(),
            queries: Mutex::default(),
            names: Mutex::default(),
            codes: Mutex::new(Codes {
                file: Some(file),
                at: 0,
                index: HashMap::new(),
                all: Vec::new(),
            }),
            states: Mutex::default(),
            queues: Mutex::default(),
            device: Mutex::default(),
            armed: AtomicBool::new(false),
            watching: AtomicBool::new(false),
            writes: Mutex::default(),
            held: Mutex::default(),
            problems: Mutex::default(),
            submits: Mutex::new(snap::Phase::default()),
        })
    }

    fn id(&self) -> u32 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Notes something that is not in the capture.
    pub(super) fn problem(&self, what: &str) {
        self.problem_times(what, 1);
    }

    pub(super) fn problem_times(&self, what: &str, n: u64) {
        let mut problems = lock(&self.problems);
        let seen = problems.entry(what.to_owned()).or_insert(0);
        if *seen == 0 {
            tracing::warn!("frame recorder: {what}");
        }
        *seen += n;
    }

    /// The problems so far.
    pub(super) fn problems(&self) -> Vec<String> {
        lock(&self.problems)
            .iter()
            .map(|(what, n)| {
                if *n == 1 {
                    what.clone()
                } else {
                    format!("{what} ({n} times)")
                }
            })
            .collect()
    }

    pub(super) fn arm(&self, on: bool) {
        self.armed.store(on, Ordering::SeqCst);
    }

    pub(super) fn device(&self) -> Option<ID3D12Device> {
        lock(&self.device).clone()
    }

    /// Whether a list was made through the hooked device (the swap chain may
    /// have others).
    pub(super) fn knows(&self, p: Ptr) -> bool {
        read(&self.lists).contains_key(&(p as usize))
    }

    /// The state of a list, if its methods are still ours.
    pub(super) fn list(&self, p: Ptr) -> Option<Arc<ListState>> {
        let state = read(&self.lists).get(&(p as usize)).cloned()?;
        // SAFETY: a live list: it is being submitted.
        if !unsafe { hook::ours(p) } {
            self.problem("a list lost its hooks");
            return None;
        }
        Some(state)
    }

    /// The buffer an address lies in.
    pub(super) fn va(&self, address: u64) -> Va {
        if address == 0 {
            return Va::Null {};
        }
        match read(&self.addresses).range(..=address).next_back() {
            Some((&start, &(size, res))) if address - start < size => Va::At {
                res,
                offset: address - start,
            },
            _ => Va::Unknown { address },
        }
    }

    /// The id of an object, [`NONE`] for null; an unknown one is a problem.
    fn object(&self, p: Ptr, what: &str) -> u32 {
        if p.is_null() {
            return NONE;
        }
        self.ids.get(p as usize).unwrap_or_else(|| {
            self.problem(&format!("{what} was used that was not seen made"));
            NONE
        })
    }

    /// The id of a resource; one not seen made (a swap chain's buffer) is
    /// taken in now.
    pub(super) fn resource(&self, p: Ptr) -> u32 {
        if p.is_null() {
            return NONE;
        }
        if let Some(id) = self.ids.get(p as usize) {
            return id;
        }
        self.add(p, Place::BackBuffer {}, 0, None)
    }

    /// Takes a new resource in.
    fn add(&self, p: Ptr, place: Place, made_in: i32, clear: Option<[u32; 5]>) -> u32 {
        let object = borrow::<ID3D12Resource>(p);
        // SAFETY: a live resource.
        let d = unsafe { object.GetDesc() };
        let desc = gpu::from_desc(&d);
        let id = self.id();
        let seen = match &place {
            Place::Committed { props, .. } => gpu::seen(props),
            Place::Placed { heap, .. } => lock(&self.heaps)
                .get(heap)
                .map_or(gpu::Seen::No, |h| gpu::seen(&h.props)),
            Place::BackBuffer {} => gpu::Seen::No,
        };
        let subs = gpu::subresources(&desc);
        let is_buffer = desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0;
        // SAFETY: a live buffer.
        let va = if is_buffer {
            unsafe { object.GetGPUVirtualAddress() }
        } else {
            0
        };
        let placed = matches!(place, Place::Placed { .. });
        let late = self.watching.load(Ordering::SeqCst);
        {
            let mut states = lock(&self.states);
            if !is_buffer {
                let decays = d.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS.0 != 0;
                states.add(id, subs, made_in as u32, decays);
            }
            if placed {
                states.in_use(id);
            }
        }
        if va != 0 {
            write(&self.addresses).insert(va, (desc.width, id));
        }
        let res = Arc::new(Res {
            id,
            ptr: p as usize,
            place,
            desc,
            made_in,
            clear,
            subs,
            seen,
            late,
        });
        write(&self.resources).insert(id, res);
        self.ids.set(p as usize, id);
        if late {
            lock(&self.held).insert(id, ID3D12Resource::clone(&object));
        }
        self.tell_gone(
            &*object,
            Gone::Resource {
                id,
                ptr: p as usize,
                va,
            },
        );
        id
    }

    fn gone(&self, what: &Gone) {
        match *what {
            Gone::Resource { id, ptr, va } => {
                // The memory goes whatever we hold: letting go of it later
                // would be a call into what is there then.
                if let Some(dead) = lock(&self.held).remove(&id) {
                    std::mem::forget(dead);
                    self.problem(
                        "a resource died as the frame started: its contents may be missing",
                    );
                }
                self.ids.remove(ptr, id);
                write(&self.resources).remove(&id);
                if va != 0 {
                    let mut all = write(&self.addresses);
                    if all.get(&va).is_some_and(|&(_, i)| i == id) {
                        all.remove(&va);
                    }
                }
                lock(&self.states).remove(id);
                lock(&self.names).remove(&id);
            }
            Gone::List { ptr } => {
                write(&self.lists).remove(&ptr);
            }
            Gone::Heap { id, ptr } => {
                self.ids.remove(ptr, id);
                write(&self.descriptors).remove(id);
            }
        }
    }

    /// Has us told when an object dies.
    fn tell_gone(&self, object: &impl Interface, what: Gone) {
        const KEY: GUID = GUID::from_u128(0x3d0d5c1e_7a1b_4a53_9a3e_5f2b1e6b9c41);
        let notice = Box::into_raw(Box::new(Notice {
            methods: &NOTICE,
            references: AtomicU32::new(1),
            what,
        }));
        // SAFETY: a COM object of ours with one reference, which this drops
        // at the end; the object takes its own.
        unsafe {
            let notice = IUnknown::from_raw(notice.cast());
            let set = object
                .cast::<ID3D12Object>()
                .and_then(|o| o.SetPrivateDataInterface(&KEY, &notice));
            if set.is_err() {
                self.problem("an object's end cannot be followed");
                std::mem::forget(notice);
            }
        }
    }

    fn view(&self, at: usize) -> Slot {
        match read(&self.descriptors).cpu(at) {
            Some((h, i)) => h.get(i),
            None => {
                self.problem("a descriptor handle in no heap seen made");
                Slot::EMPTY
            }
        }
    }

    fn set_view(&self, at: usize, s: &Slot) {
        let all = read(&self.descriptors);
        let Some((h, i)) = all.cpu(at) else {
            drop(all);
            self.problem("a view was made in no descriptor heap seen made");
            return;
        };
        h.set(i, s);
        self.wrote(h, i, 1);
    }

    /// After slots were written: noted if a frame is being captured.
    fn wrote(&self, h: &Heap, first: u32, n: u32) {
        if h.visible() && self.watching.load(Ordering::SeqCst) {
            lock(&self.writes).push((h.id, first, n));
        }
    }

    fn copy_views(&self, to: usize, from: usize, n: u32) {
        let all = read(&self.descriptors);
        let (Some((a, i)), Some((b, j))) = (all.cpu(to), all.cpu(from)) else {
            drop(all);
            self.problem("descriptors were copied in no heap seen made");
            return;
        };
        for k in 0..n {
            a.set(i + k, &b.get(j + k));
        }
        self.wrote(a, i, n);
    }

    fn code(&self, bytes: &[u8]) -> u32 {
        use std::io::Write;
        let hash = *blake3::hash(bytes).as_bytes();
        let mut codes = lock(&self.codes);
        if let Some(&i) = codes.index.get(&hash) {
            return i;
        }
        let i = codes.all.len() as u32;
        let at = codes.at;
        let wrote = codes
            .file
            .as_mut()
            .is_some_and(|f| f.write_all(bytes).is_ok());
        if !wrote {
            drop(codes);
            self.problem("shader code could not be written");
            return u32::MAX;
        }
        codes.at += bytes.len() as u64;
        codes.all.push(wire::Code {
            at,
            len: bytes.len() as u64,
            hash,
        });
        codes.index.insert(hash, i);
        i
    }

    fn pipeline(&self, p: Ptr, parts: Vec<Part>) {
        let id = self.id();
        lock(&self.pipelines).insert(id, Arc::new(parts));
        self.ids.set(p as usize, id);
    }

    /// `name` is an object's, for reports.
    fn name(&self, p: Ptr, name: &str) {
        if let Some(id) = self.ids.get(p as usize) {
            lock(&self.names).insert(id, name.to_owned());
        }
    }
}

struct Keeper<'a>(&'a Recorder);

impl pso::Keep for Keeper<'_> {
    fn root(&mut self, root: Ptr) -> u32 {
        self.0.object(root, "a root signature")
    }
    fn code(&mut self, code: &[u8]) -> u32 {
        self.0.code(code)
    }
    fn lost(&mut self, what: &str) {
        self.0.problem(what);
    }
}

/// Which object died.
enum Gone {
    Resource { id: u32, ptr: usize, va: u64 },
    List { ptr: usize },
    Heap { id: u32, ptr: usize },
}

/// A COM object an object of D3D12's holds and lets go of when it dies.
#[repr(C)]
struct Notice {
    methods: *const NoticeTable,
    references: AtomicU32,
    what: Gone,
}

#[repr(C)]
struct NoticeTable {
    query: unsafe extern "system" fn(Ptr, *const GUID, Out) -> HRESULT,
    add: unsafe extern "system" fn(Ptr) -> u32,
    release: unsafe extern "system" fn(Ptr) -> u32,
}
static NOTICE: NoticeTable = NoticeTable {
    query: notice_query,
    add: notice_add,
    release: notice_release,
};

unsafe extern "system" fn notice_query(this: Ptr, iid: *const GUID, out: Out) -> HRESULT {
    // SAFETY: COM's rules: a valid id and place for the answer.
    unsafe {
        if *iid == IUnknown::IID {
            notice_add(this);
            *out = this;
            return HRESULT(0);
        }
        *out = std::ptr::null_mut();
    }
    HRESULT(0x8000_4002_u32 as i32)
}

unsafe extern "system" fn notice_add(this: Ptr) -> u32 {
    // SAFETY: one of ours, alive while a reference is.
    unsafe {
        (*this.cast::<Notice>())
            .references
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}

unsafe extern "system" fn notice_release(this: Ptr) -> u32 {
    // SAFETY: as above; the last reference frees the box it was made as.
    unsafe {
        let left = (*this.cast::<Notice>())
            .references
            .fetch_sub(1, Ordering::AcqRel)
            - 1;
        if left == 0 {
            let notice = Box::from_raw(this.cast::<Notice>());
            if let Some(r) = recorder() {
                r.gone(&notice.what);
            }
        }
        left
    }
}

/// Starts the recorder if the environment asks for it, and hooks `device`.
/// Called right after the device was made.
pub(crate) fn from_env(device: &ID3D12Device) {
    let Some(settings) = Settings::from_env() else {
        return;
    };
    match start(settings, device) {
        Ok(folder) => tracing::info!(folder = %folder.display(), "frame recorder on"),
        Err(e) => tracing::warn!(error = %e, "frame recorder not started"),
    }
}

/// Starts the recorder (once a process) and hooks `device`, which must have
/// made nothing the frame will use. Where the capture will be.
pub fn start(settings: Settings, device: &ID3D12Device) -> std::io::Result<PathBuf> {
    let r = Recorder::new(settings)?;
    let folder = r.folder.clone();
    if RECORDER.set(r).is_err() {
        return Err(std::io::Error::other("the recorder is already on"));
    }
    let r = recorder().ok_or_else(|| std::io::Error::other("no recorder"))?;
    *lock(&r.device) = Some(device.clone());
    r.arm(r.settings.always);
    // The newer interfaces may be other pointers (they are the same one
    // without the debug layer).
    let mut all = vec![device.as_raw()];
    let mut more: Vec<IUnknown> = Vec::new();
    for iid in [
        ID3D12Device1::IID,
        ID3D12Device2::IID,
        ID3D12Device3::IID,
        ID3D12Device4::IID,
        ID3D12Device5::IID,
        ID3D12Device8::IID,
        ID3D12Device10::IID,
        ID3D12Device11::IID,
        ID3D12Device13::IID,
    ] {
        if let Some(p) = query(device.as_raw(), &iid) {
            all.push(p.as_raw());
            more.push(p);
        }
    }
    for p in all {
        // SAFETY: a live device; the entries below are of their methods'
        // types (the compiler checks them).
        unsafe { hook::install(p, std::ptr::null(), device_table) };
    }
    Ok(folder)
}

/// Unhooks `device` (tests).
pub fn unhook(device: &ID3D12Device) {
    // SAFETY: a live device, used by no other thread (the caller's word).
    unsafe { hook::remove(device.as_raw()) };
}

/// The object's interface `iid`.
fn query(p: Ptr, iid: &GUID) -> Option<IUnknown> {
    let object = borrow::<IUnknown>(p);
    let mut out = std::ptr::null_mut();
    // SAFETY: a live COM object.
    unsafe {
        object.query(iid, &mut out).ok().ok()?;
        (!out.is_null()).then(|| IUnknown::from_raw(out))
    }
}

/// Names an object for reports.
pub fn name(object: &impl Interface, text: &str) {
    if let Some(r) = recorder() {
        r.name(object.as_raw(), text);
    }
}

/// At the end of a frame, before the swap chain's present: `texture`, in
/// `state`, is shown.
pub fn present(texture: &ID3D12Resource, state: D3D12_RESOURCE_STATES) {
    if let Some(r) = recorder() {
        snap::present(r, texture, state.0 as u32);
    }
}

/// Where the capture is, once it is done.
pub fn finished() -> Option<PathBuf> {
    let r = recorder()?;
    lock(&r.submits).done().then(|| r.folder.clone())
}

/// Sets a method of a table to a function of its type.
macro_rules! set {
    ($t:ident, $vtbl:ident . $name:ident = $f:expr) => {{
        // The compiler checks the function's type here.
        let _ = |v: &mut $vtbl| v.$name = $f;
        $t[method!($vtbl.$name)] = $f as *const () as usize;
    }};
}

/// The object's own method.
macro_rules! own {
    ($this:ident, $vtbl:ident . $name:ident) => {
        orig::<$vtbl>($this).$name
    };
}

/// Entries for methods that are not recorded: counted, and named in the
/// capture's notes if they were called.
macro_rules! not_recorded {
    ($t:ident, $class:ident, $names:ident: $($vtbl:ident . $name:ident),* $(,)?) => {
        const $names: &[(usize, &str)] = &[$((method!($vtbl.$name), stringify!($name))),*];
        $($t[method!($vtbl.$name)] = noted_entry::<{ hook::$class * hook::SLOTS + method!($vtbl.$name) }>();)*
    };
}

fn device_table(t: &mut [usize; hook::SLOTS]) {
    set!(t, ID3D12Device_Vtbl.CreateCommandQueue = create_queue);
    set!(
        t,
        ID3D12Device_Vtbl.CreateGraphicsPipelineState = create_graphics
    );
    set!(
        t,
        ID3D12Device_Vtbl.CreateComputePipelineState = create_compute
    );
    set!(t, ID3D12Device_Vtbl.CreateCommandList = create_list);
    set!(
        t,
        ID3D12Device_Vtbl.CreateDescriptorHeap = create_descriptors
    );
    set!(t, ID3D12Device_Vtbl.CreateRootSignature = create_root);
    set!(t, ID3D12Device_Vtbl.CreateConstantBufferView = create_cbv);
    set!(t, ID3D12Device_Vtbl.CreateShaderResourceView = create_srv);
    set!(t, ID3D12Device_Vtbl.CreateUnorderedAccessView = create_uav);
    set!(t, ID3D12Device_Vtbl.CreateRenderTargetView = create_rtv);
    set!(t, ID3D12Device_Vtbl.CreateDepthStencilView = create_dsv);
    set!(t, ID3D12Device_Vtbl.CreateSampler = create_sampler);
    set!(t, ID3D12Device_Vtbl.CopyDescriptors = copy_descriptors);
    set!(
        t,
        ID3D12Device_Vtbl.CopyDescriptorsSimple = copy_descriptors_simple
    );
    set!(
        t,
        ID3D12Device_Vtbl.CreateCommittedResource = create_committed
    );
    set!(t, ID3D12Device_Vtbl.CreateHeap = create_heap);
    set!(t, ID3D12Device_Vtbl.CreatePlacedResource = create_placed);
    set!(t, ID3D12Device_Vtbl.CreateQueryHeap = create_queries);
    set!(
        t,
        ID3D12Device_Vtbl.CreateCommandSignature = create_signature
    );
    set!(t, ID3D12Device1_Vtbl.CreatePipelineLibrary = create_library);
    set!(t, ID3D12Device2_Vtbl.CreatePipelineState = create_stream);
    let _ = device_rest(t);
}

fn device_rest(t: &mut [usize; hook::SLOTS]) -> &'static [(usize, &'static str)] {
    not_recorded!(t, DEVICE, NAMES:
        ID3D12Device_Vtbl.CreateReservedResource,
        ID3D12Device_Vtbl.OpenSharedHandle,
        ID3D12Device4_Vtbl.CreateCommandList1,
        ID3D12Device4_Vtbl.CreateCommittedResource1,
        ID3D12Device4_Vtbl.CreateHeap1,
        ID3D12Device4_Vtbl.CreateReservedResource1,
        ID3D12Device5_Vtbl.CreateStateObject,
        ID3D12Device8_Vtbl.CreateCommittedResource2,
        ID3D12Device8_Vtbl.CreatePlacedResource1,
        ID3D12Device8_Vtbl.CreateSamplerFeedbackUnorderedAccessView,
        ID3D12Device10_Vtbl.CreateCommittedResource3,
        ID3D12Device10_Vtbl.CreatePlacedResource2,
        ID3D12Device10_Vtbl.CreateReservedResource2,
        ID3D12Device11_Vtbl.CreateSampler2,
    );
    NAMES
}

fn queue_table(t: &mut [usize; hook::SLOTS]) {
    set!(t, ID3D12CommandQueue_Vtbl.ExecuteCommandLists = execute);
    let _ = queue_rest(t);
}

fn queue_rest(t: &mut [usize; hook::SLOTS]) -> &'static [(usize, &'static str)] {
    not_recorded!(t, QUEUE, NAMES:
        ID3D12CommandQueue_Vtbl.UpdateTileMappings,
        ID3D12CommandQueue_Vtbl.CopyTileMappings,
        ID3D12CommandQueue_Vtbl.Wait,
    );
    NAMES
}

fn library_table(t: &mut [usize; hook::SLOTS]) {
    set!(
        t,
        ID3D12PipelineLibrary_Vtbl.LoadGraphicsPipeline = load_graphics
    );
    set!(
        t,
        ID3D12PipelineLibrary_Vtbl.LoadComputePipeline = load_compute
    );
    set!(t, ID3D12PipelineLibrary1_Vtbl.LoadPipeline = load_stream);
}

fn list_table(t: &mut [usize; hook::SLOTS]) {
    set!(t, ID3D12GraphicsCommandList_Vtbl.Close = list_close);
    set!(t, ID3D12GraphicsCommandList_Vtbl.Reset = list_reset);
    set!(t, ID3D12GraphicsCommandList_Vtbl.DrawInstanced = draw);
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.DrawIndexedInstanced = draw_indexed
    );
    set!(t, ID3D12GraphicsCommandList_Vtbl.Dispatch = dispatch);
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.CopyBufferRegion = copy_buffer
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.CopyTextureRegion = copy_texture
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.CopyResource = copy_resource
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.ResolveSubresource = resolve
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.IASetPrimitiveTopology = topology
    );
    set!(t, ID3D12GraphicsCommandList_Vtbl.RSSetViewports = viewports);
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.RSSetScissorRects = scissors
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.OMSetBlendFactor = blend_factor
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.OMSetStencilRef = stencil_ref
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetPipelineState = set_pipeline
    );
    set!(t, ID3D12GraphicsCommandList_Vtbl.ResourceBarrier = barrier);
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetDescriptorHeaps = set_heaps
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetComputeRootSignature = root_compute
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetGraphicsRootSignature = root_graphics
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetComputeRootDescriptorTable = table_compute
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetGraphicsRootDescriptorTable = table_graphics
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetComputeRoot32BitConstant = constant_compute
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetGraphicsRoot32BitConstant = constant_graphics
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetComputeRoot32BitConstants = constants_compute
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetGraphicsRoot32BitConstants = constants_graphics
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetComputeRootConstantBufferView = cbv_compute
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetGraphicsRootConstantBufferView = cbv_graphics
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetComputeRootShaderResourceView = srv_compute
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetGraphicsRootShaderResourceView = srv_graphics
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetComputeRootUnorderedAccessView = uav_compute
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.SetGraphicsRootUnorderedAccessView = uav_graphics
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.IASetIndexBuffer = index_buffer
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.IASetVertexBuffers = vertex_buffers
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.OMSetRenderTargets = targets
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.ClearDepthStencilView = clear_depth
    );
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.ClearRenderTargetView = clear_target
    );
    set!(t, ID3D12GraphicsCommandList_Vtbl.DiscardResource = discard);
    set!(t, ID3D12GraphicsCommandList_Vtbl.BeginQuery = begin_query);
    set!(t, ID3D12GraphicsCommandList_Vtbl.EndQuery = end_query);
    set!(
        t,
        ID3D12GraphicsCommandList_Vtbl.ResolveQueryData = resolve_query
    );
    set!(t, ID3D12GraphicsCommandList_Vtbl.ExecuteIndirect = indirect);
    set!(
        t,
        ID3D12GraphicsCommandList1_Vtbl.ResolveSubresourceRegion = resolve_region
    );
    set!(
        t,
        ID3D12GraphicsCommandList1_Vtbl.OMSetDepthBounds = depth_bounds
    );
    set!(
        t,
        ID3D12GraphicsCommandList2_Vtbl.WriteBufferImmediate = write_immediate
    );
    set!(
        t,
        ID3D12GraphicsCommandList6_Vtbl.DispatchMesh = dispatch_mesh
    );
    let _ = list_rest(t);
}

fn list_rest(t: &mut [usize; hook::SLOTS]) -> &'static [(usize, &'static str)] {
    not_recorded!(t, LIST, NAMES:
        ID3D12GraphicsCommandList_Vtbl.ClearState,
        ID3D12GraphicsCommandList_Vtbl.CopyTiles,
        ID3D12GraphicsCommandList_Vtbl.ExecuteBundle,
        ID3D12GraphicsCommandList_Vtbl.SOSetTargets,
        ID3D12GraphicsCommandList_Vtbl.ClearUnorderedAccessViewUint,
        ID3D12GraphicsCommandList_Vtbl.ClearUnorderedAccessViewFloat,
        ID3D12GraphicsCommandList_Vtbl.SetPredication,
        ID3D12GraphicsCommandList1_Vtbl.AtomicCopyBufferUINT,
        ID3D12GraphicsCommandList1_Vtbl.AtomicCopyBufferUINT64,
        ID3D12GraphicsCommandList1_Vtbl.SetSamplePositions,
        ID3D12GraphicsCommandList1_Vtbl.SetViewInstanceMask,
        ID3D12GraphicsCommandList3_Vtbl.SetProtectedResourceSession,
        ID3D12GraphicsCommandList4_Vtbl.BeginRenderPass,
        ID3D12GraphicsCommandList4_Vtbl.EndRenderPass,
        ID3D12GraphicsCommandList4_Vtbl.InitializeMetaCommand,
        ID3D12GraphicsCommandList4_Vtbl.ExecuteMetaCommand,
        ID3D12GraphicsCommandList4_Vtbl.BuildRaytracingAccelerationStructure,
        ID3D12GraphicsCommandList4_Vtbl.EmitRaytracingAccelerationStructurePostbuildInfo,
        ID3D12GraphicsCommandList4_Vtbl.CopyRaytracingAccelerationStructure,
        ID3D12GraphicsCommandList4_Vtbl.SetPipelineState1,
        ID3D12GraphicsCommandList4_Vtbl.DispatchRays,
        ID3D12GraphicsCommandList5_Vtbl.RSSetShadingRate,
        ID3D12GraphicsCommandList5_Vtbl.RSSetShadingRateImage,
        ID3D12GraphicsCommandList7_Vtbl.Barrier,
        ID3D12GraphicsCommandList8_Vtbl.OMSetFrontAndBackStencilRef,
        ID3D12GraphicsCommandList9_Vtbl.RSSetDepthBias,
        ID3D12GraphicsCommandList9_Vtbl.IASetIndexBufferStripCutValue,
        ID3D12GraphicsCommandList10_Vtbl.SetProgram,
        ID3D12GraphicsCommandList10_Vtbl.DispatchGraph,
    );
    NAMES
}

/// How often each method that is not recorded was called so far.
pub(super) fn not_recorded_calls() -> Vec<(String, u64)> {
    let mut t = [0; hook::SLOTS];
    let mut all = Vec::new();
    for (class, of, names) in [
        (hook::LIST, "command list", list_rest(&mut t)),
        (hook::DEVICE, "device", device_rest(&mut t)),
        (hook::QUEUE, "queue", queue_rest(&mut t)),
    ] {
        for &(slot, name) in names {
            let what = match (class, name) {
                // Submissions are in order anyway: the replay has one queue.
                (hook::QUEUE, "Wait") => {
                    "waits of the drawing queue (the swap chain makes some) are not replayed"
                        .to_owned()
                }
                _ => format!("{of} method {name} is not recorded, and was called"),
            };
            all.push((what, hook::count(class, slot)));
        }
    }
    all
}

/// The recorder, for a call that is the host layer's and not our own, which
/// succeeded.
fn made(hr: HRESULT, out: Out) -> Option<(&'static Recorder, Ptr)> {
    if hr.is_err() || out.is_null() || ours() {
        return None;
    }
    // SAFETY: the call's answer.
    let p = unsafe { *out };
    (!p.is_null()).then_some(())?;
    Some((recorder()?, p))
}

unsafe extern "system" fn create_queue(
    this: Ptr,
    desc: *const D3D12_COMMAND_QUEUE_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: the caller's arguments, passed on; then a new queue no other
    // thread knows.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateCommandQueue)(this, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            let kind = (*desc).Type.0;
            let state = Box::leak(Box::new(QueueState { kind }));
            hook::install(p, (&raw const *state).cast(), queue_table);
            lock(&r.queues).push((ID3D12CommandQueue::clone(&borrow(p)), kind));
        }
        hr
    }
}

unsafe extern "system" fn create_list(
    this: Ptr,
    nodes: u32,
    kind: D3D12_COMMAND_LIST_TYPE,
    allocator: Ptr,
    pso: Ptr,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateCommandList)(
            this, nodes, kind, allocator, pso, iid, out,
        );
        if let Some((r, p)) = made(hr, out) {
            let state = Arc::new(ListState {
                kind: kind.0,
                inner: Mutex::default(),
            });
            // Every interface of it we hook, and the one it is submitted as.
            static ONE: OnceLock<bool> = OnceLock::new();
            let iids = [
                ID3D12CommandList::IID,
                ID3D12GraphicsCommandList1::IID,
                ID3D12GraphicsCommandList2::IID,
                ID3D12GraphicsCommandList4::IID,
                ID3D12GraphicsCommandList6::IID,
            ];
            let same = *ONE.get_or_init(|| {
                iids.iter()
                    .all(|iid| query(p, iid).is_none_or(|q| q.as_raw() == p))
            });
            let mut all = vec![p];
            if !same {
                all.extend(
                    iids.iter()
                        .filter_map(|iid| query(p, iid))
                        .map(|q| q.as_raw()),
                );
            }
            {
                let mut inner = lock(&state.inner);
                inner.whole =
                    r.armed.load(Ordering::SeqCst) && kind == D3D12_COMMAND_LIST_TYPE_DIRECT;
                begin(&mut inner, r, pso);
            }
            for q in all {
                // The state's address is kept for good, as the table is.
                hook::install(q, Arc::into_raw(state.clone()).cast(), list_table);
                write(&r.lists).insert(q as usize, state.clone());
                r.tell_gone(
                    &*borrow::<ID3D12CommandList>(q),
                    Gone::List { ptr: q as usize },
                );
            }
        }
        hr
    }
}

/// A list is open again, with pipeline `pso`.
fn begin(inner: &mut ListInner, r: &Recorder, pso: Ptr) {
    inner.bytes = Vec::new();
    inner.changes.clear();
    if inner.whole && !pso.is_null() {
        Cmd::Pipeline {
            pso: r.object(pso, "a pipeline"),
        }
        .put(&mut inner.bytes);
    }
}

unsafe extern "system" fn create_library(
    this: Ptr,
    blob: *const c_void,
    size: usize,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device1_Vtbl.CreatePipelineLibrary)(this, blob, size, iid, out);
        if let Some((_, p)) = made(hr, out) {
            let mut all = vec![p];
            let newer = query(p, &ID3D12PipelineLibrary1::IID);
            all.extend(newer.as_ref().map(|q| q.as_raw()));
            for q in all {
                hook::install(q, std::ptr::null(), library_table);
            }
        }
        hr
    }
}

unsafe extern "system" fn create_graphics(
    this: Ptr,
    desc: *const D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above; the description is valid, as it was accepted.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateGraphicsPipelineState)(this, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            r.pipeline(p, pso::graphics(&*desc, &mut Keeper(r)));
        }
        hr
    }
}

unsafe extern "system" fn create_compute(
    this: Ptr,
    desc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateComputePipelineState)(this, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            r.pipeline(p, pso::compute(&*desc, &mut Keeper(r)));
        }
        hr
    }
}

unsafe extern "system" fn create_stream(
    this: Ptr,
    desc: *const D3D12_PIPELINE_STATE_STREAM_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device2_Vtbl.CreatePipelineState)(this, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            r.pipeline(p, pso::stream(&*desc, &mut Keeper(r)));
        }
        hr
    }
}

unsafe extern "system" fn load_graphics(
    this: Ptr,
    name: PCWSTR,
    desc: *const D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr =
            own!(this, ID3D12PipelineLibrary_Vtbl.LoadGraphicsPipeline)(this, name, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            r.pipeline(p, pso::graphics(&*desc, &mut Keeper(r)));
        }
        hr
    }
}

unsafe extern "system" fn load_compute(
    this: Ptr,
    name: PCWSTR,
    desc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr =
            own!(this, ID3D12PipelineLibrary_Vtbl.LoadComputePipeline)(this, name, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            r.pipeline(p, pso::compute(&*desc, &mut Keeper(r)));
        }
        hr
    }
}

unsafe extern "system" fn load_stream(
    this: Ptr,
    name: PCWSTR,
    desc: *const D3D12_PIPELINE_STATE_STREAM_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12PipelineLibrary1_Vtbl.LoadPipeline)(this, name, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            r.pipeline(p, pso::stream(&*desc, &mut Keeper(r)));
        }
        hr
    }
}

unsafe extern "system" fn create_root(
    this: Ptr,
    nodes: u32,
    blob: *const c_void,
    size: usize,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr =
            own!(this, ID3D12Device_Vtbl.CreateRootSignature)(this, nodes, blob, size, iid, out);
        if let Some((r, p)) = made(hr, out) {
            // D3D12 may answer with an object it made before.
            let known = r
                .ids
                .get(p as usize)
                .filter(|id| lock(&r.roots).contains_key(id));
            let bytes = std::slice::from_raw_parts(blob.cast::<u8>(), size).to_vec();
            let id = known.unwrap_or_else(|| r.id());
            lock(&r.roots).insert(id, Arc::new(bytes));
            r.ids.set(p as usize, id);
        }
        hr
    }
}

unsafe extern "system" fn create_signature(
    this: Ptr,
    desc: *const D3D12_COMMAND_SIGNATURE_DESC,
    root: Ptr,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateCommandSignature)(this, desc, root, iid, out);
        if let Some((r, p)) = made(hr, out) {
            let id = r.id();
            let d = &*desc;
            let args: Vec<[u32; 4]> = std::slice::from_raw_parts(
                d.pArgumentDescs.cast::<[u32; 4]>(),
                d.NumArgumentDescs as usize,
            )
            .to_vec();
            // Arguments that are GPU addresses lie in buffers; they are not
            // rewritten for a replay.
            let addresses = [
                D3D12_INDIRECT_ARGUMENT_TYPE_VERTEX_BUFFER_VIEW,
                D3D12_INDIRECT_ARGUMENT_TYPE_INDEX_BUFFER_VIEW,
                D3D12_INDIRECT_ARGUMENT_TYPE_CONSTANT_BUFFER_VIEW,
                D3D12_INDIRECT_ARGUMENT_TYPE_SHADER_RESOURCE_VIEW,
                D3D12_INDIRECT_ARGUMENT_TYPE_UNORDERED_ACCESS_VIEW,
            ];
            if args
                .iter()
                .any(|a| addresses.iter().any(|t| t.0 as u32 == a[0]))
            {
                r.problem("a command signature with GPU addresses among its arguments: a replay reads the recorded addresses");
            }
            let root = if root.is_null() {
                NONE
            } else {
                r.object(root, "a root signature")
            };
            lock(&r.signatures).insert(
                id,
                wire::Signature {
                    id,
                    stride: d.ByteStride,
                    args,
                    root,
                },
            );
            r.ids.set(p as usize, id);
        }
        hr
    }
}

unsafe extern "system" fn create_queries(
    this: Ptr,
    desc: *const D3D12_QUERY_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateQueryHeap)(this, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            let id = r.id();
            lock(&r.queries).insert(
                id,
                wire::QueryHeap {
                    id,
                    kind: (*desc).Type.0,
                    count: (*desc).Count,
                },
            );
            r.ids.set(p as usize, id);
        }
        hr
    }
}

unsafe extern "system" fn create_descriptors(
    this: Ptr,
    desc: *const D3D12_DESCRIPTOR_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateDescriptorHeap)(this, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            let heap = borrow::<ID3D12DescriptorHeap>(p);
            let d = &*desc;
            let visible = d.Flags.0 & D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE.0 != 0;
            let cpu = heap.GetCPUDescriptorHandleForHeapStart().ptr;
            let gpu = if visible {
                heap.GetGPUDescriptorHandleForHeapStart().ptr
            } else {
                0
            };
            let step = own!(this, ID3D12Device_Vtbl.GetDescriptorHandleIncrementSize)(this, d.Type)
                as usize;
            let id = r.id();
            write(&r.descriptors).add(Arc::new(Heap::new(id, d, cpu, gpu, step.max(1))));
            r.ids.set(p as usize, id);
            r.tell_gone(
                &*heap,
                Gone::Heap {
                    id,
                    ptr: p as usize,
                },
            );
        }
        hr
    }
}

unsafe extern "system" fn create_heap(
    this: Ptr,
    desc: *const D3D12_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateHeap)(this, desc, iid, out);
        if let Some((r, p)) = made(hr, out) {
            let id = r.id();
            lock(&r.heaps).insert(id, heap_of(id, &*desc));
            r.ids.set(p as usize, id);
        }
        hr
    }
}

fn heap_of(id: u32, d: &D3D12_HEAP_DESC) -> wire::Heap {
    wire::Heap {
        id,
        size: d.SizeInBytes,
        props: gpu::from_props(&d.Properties),
        alignment: d.Alignment,
        flags: d.Flags.0,
    }
}

unsafe fn clear_of(clear: *const D3D12_CLEAR_VALUE) -> Option<[u32; 5]> {
    const _: () = assert!(size_of::<D3D12_CLEAR_VALUE>() == 20);
    // SAFETY: null or a clear value, of 20 bytes.
    (!clear.is_null()).then(|| unsafe { clear.cast::<[u32; 5]>().read() })
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_committed(
    this: Ptr,
    props: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreateCommittedResource)(
            this, props, flags, desc, state, clear, iid, out,
        );
        if let Some((r, p)) = made(hr, out) {
            let place = Place::Committed {
                props: gpu::from_props(&*props),
                flags: flags.0,
            };
            r.add(p, place, state.0, clear_of(clear));
        }
        hr
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_placed(
    this: Ptr,
    heap: Ptr,
    offset: u64,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12Device_Vtbl.CreatePlacedResource)(
            this, heap, offset, desc, state, clear, iid, out,
        );
        if let Some((r, p)) = made(hr, out) {
            // One opened from memory was not seen made.
            let heap = r.ids.get(heap as usize).unwrap_or_else(|| {
                let id = r.id();
                let d = borrow::<ID3D12Heap>(heap).GetDesc();
                r.problem("a heap opened from existing memory: a replay makes one like it");
                lock(&r.heaps).insert(id, heap_of(id, &d));
                r.ids.set(heap as usize, id);
                id
            });
            r.add(p, Place::Placed { heap, offset }, state.0, clear_of(clear));
        }
        hr
    }
}

unsafe extern "system" fn create_cbv(
    this: Ptr,
    desc: *const D3D12_CONSTANT_BUFFER_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CreateConstantBufferView)(this, desc, at);
        if let Some(r) = recorder().filter(|_| !ours()) {
            r.set_view(
                at.ptr,
                &Slot::new(
                    slot::CBV,
                    NONE,
                    NONE,
                    desc.cast(),
                    size_of::<D3D12_CONSTANT_BUFFER_VIEW_DESC>(),
                ),
            );
        }
    }
}

unsafe extern "system" fn create_srv(
    this: Ptr,
    res: Ptr,
    desc: *const D3D12_SHADER_RESOURCE_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CreateShaderResourceView)(this, res, desc, at);
        if let Some(r) = recorder().filter(|_| !ours()) {
            let id = r.resource(res);
            r.set_view(
                at.ptr,
                &Slot::new(
                    slot::SRV,
                    id,
                    NONE,
                    desc.cast(),
                    size_of::<D3D12_SHADER_RESOURCE_VIEW_DESC>(),
                ),
            );
        }
    }
}

unsafe extern "system" fn create_uav(
    this: Ptr,
    res: Ptr,
    counter: Ptr,
    desc: *const D3D12_UNORDERED_ACCESS_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CreateUnorderedAccessView)(this, res, counter, desc, at);
        if let Some(r) = recorder().filter(|_| !ours()) {
            let (id, counter) = (r.resource(res), r.resource(counter));
            r.set_view(
                at.ptr,
                &Slot::new(
                    slot::UAV,
                    id,
                    counter,
                    desc.cast(),
                    size_of::<D3D12_UNORDERED_ACCESS_VIEW_DESC>(),
                ),
            );
        }
    }
}

unsafe extern "system" fn create_rtv(
    this: Ptr,
    res: Ptr,
    desc: *const D3D12_RENDER_TARGET_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CreateRenderTargetView)(this, res, desc, at);
        if let Some(r) = recorder().filter(|_| !ours()) {
            let id = r.resource(res);
            r.set_view(
                at.ptr,
                &Slot::new(
                    slot::RTV,
                    id,
                    NONE,
                    desc.cast(),
                    size_of::<D3D12_RENDER_TARGET_VIEW_DESC>(),
                ),
            );
        }
    }
}

unsafe extern "system" fn create_dsv(
    this: Ptr,
    res: Ptr,
    desc: *const D3D12_DEPTH_STENCIL_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CreateDepthStencilView)(this, res, desc, at);
        if let Some(r) = recorder().filter(|_| !ours()) {
            let id = r.resource(res);
            r.set_view(
                at.ptr,
                &Slot::new(
                    slot::DSV,
                    id,
                    NONE,
                    desc.cast(),
                    size_of::<D3D12_DEPTH_STENCIL_VIEW_DESC>(),
                ),
            );
        }
    }
}

unsafe extern "system" fn create_sampler(
    this: Ptr,
    desc: *const D3D12_SAMPLER_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    const _: () = assert!(size_of::<D3D12_SAMPLER_DESC>() == 52);
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CreateSampler)(this, desc, at);
        if let Some(r) = recorder().filter(|_| !ours()) {
            r.set_view(
                at.ptr,
                &Slot::new(slot::SAMPLER, NONE, NONE, desc.cast(), 52),
            );
        }
    }
}

unsafe extern "system" fn copy_descriptors_simple(
    this: Ptr,
    n: u32,
    to: D3D12_CPU_DESCRIPTOR_HANDLE,
    from: D3D12_CPU_DESCRIPTOR_HANDLE,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CopyDescriptorsSimple)(this, n, to, from, kind);
        if let Some(r) = recorder().filter(|_| !ours()) {
            r.copy_views(to.ptr, from.ptr, n);
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn copy_descriptors(
    this: Ptr,
    n_to: u32,
    to: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    to_sizes: *const u32,
    n_from: u32,
    from: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    from_sizes: *const u32,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
) {
    // SAFETY: as above; the arrays have as many entries as the counts say,
    // and null sizes mean one descriptor each.
    unsafe {
        own!(this, ID3D12Device_Vtbl.CopyDescriptors)(
            this, n_to, to, to_sizes, n_from, from, from_sizes, kind,
        );
        let Some(r) = recorder().filter(|_| !ours()) else {
            return;
        };
        let step =
            own!(this, ID3D12Device_Vtbl.GetDescriptorHandleIncrementSize)(this, kind) as usize;
        let size = |sizes: *const u32, i: u32| {
            if sizes.is_null() {
                1
            } else {
                *sizes.add(i as usize)
            }
        };
        let (mut i, mut j, mut done_to, mut done_from) = (0, 0, 0, 0);
        while i < n_to && j < n_from {
            let n = (size(to_sizes, i) - done_to).min(size(from_sizes, j) - done_from);
            if n > 0 {
                let a = (*to.add(i as usize)).ptr + done_to as usize * step;
                let b = (*from.add(j as usize)).ptr + done_from as usize * step;
                r.copy_views(a, b, n);
            }
            done_to += n;
            done_from += n;
            if done_to >= size(to_sizes, i) {
                (i, done_to) = (i + 1, 0);
            }
            if done_from >= size(from_sizes, j) {
                (j, done_from) = (j + 1, 0);
            }
        }
    }
}

unsafe extern "system" fn execute(this: Ptr, n: u32, lists: *const Ptr) {
    // SAFETY: `n` closed lists, as D3D12 asks; the state is the queue's.
    unsafe {
        let go = || own!(this, ID3D12CommandQueue_Vtbl.ExecuteCommandLists)(this, n, lists);
        let Some(r) = recorder().filter(|_| !ours()) else {
            return go();
        };
        let kind = hook::state::<QueueState>(this).kind;
        if kind != D3D12_COMMAND_LIST_TYPE_DIRECT.0 {
            snap::other_queue(r, n);
            return go();
        }
        let all = std::slice::from_raw_parts(lists, n as usize);
        snap::submit(r, &borrow::<ID3D12CommandQueue>(this), all, go);
    }
}

/// A list's recorder and state, for a call of the host layer's.
unsafe fn of(this: Ptr) -> Option<(&'static Recorder, &'static ListState)> {
    let r = recorder()?;
    if ours() {
        return None;
    }
    // SAFETY: a list with a shim; its state is a list's.
    Some((r, unsafe { hook::state::<ListState>(this) }))
}

/// Records a command, if lists are recorded now.
unsafe fn note(this: Ptr, cmd: impl FnOnce(&Recorder) -> Cmd) {
    // SAFETY: a list with a shim.
    if let Some((r, state)) = unsafe { of(this) }
        && r.armed.load(Ordering::Relaxed)
    {
        state.push(&cmd(r));
    }
}

unsafe extern "system" fn list_close(this: Ptr) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let hr = own!(this, ID3D12GraphicsCommandList_Vtbl.Close)(this);
        if let Some((_, state)) = of(this) {
            let mut inner = lock(&state.inner);
            let bytes = std::mem::take(&mut inner.bytes);
            let changes = Arc::new(std::mem::take(&mut inner.changes));
            inner.closed = Some((inner.whole.then(|| Arc::new(bytes)), changes));
        }
        hr
    }
}

unsafe extern "system" fn list_reset(this: Ptr, allocator: Ptr, pso: Ptr) -> HRESULT {
    // SAFETY: as above.
    unsafe {
        let table = hook::table(this);
        let hr = own!(this, ID3D12GraphicsCommandList_Vtbl.Reset)(this, allocator, pso);
        // A reset sets the list's table pointer anew.
        hook::again(this, table, list_table);
        if let Some((r, state)) = of(this)
            && hr.is_ok()
        {
            let mut inner = lock(&state.inner);
            inner.closed = None;
            inner.whole =
                r.armed.load(Ordering::SeqCst) && state.kind == D3D12_COMMAND_LIST_TYPE_DIRECT.0;
            begin(&mut inner, r, pso);
        }
        hr
    }
}

unsafe extern "system" fn draw(
    this: Ptr,
    vertices: u32,
    instances: u32,
    first: u32,
    first_instance: u32,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.DrawInstanced)(
            this,
            vertices,
            instances,
            first,
            first_instance,
        );
        note(this, |_| Cmd::Draw {
            vertices,
            instances,
            first,
            first_instance,
        });
    }
}

unsafe extern "system" fn draw_indexed(
    this: Ptr,
    indices: u32,
    instances: u32,
    first: u32,
    base: i32,
    first_instance: u32,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.DrawIndexedInstanced)(
            this,
            indices,
            instances,
            first,
            base,
            first_instance,
        );
        note(this, |_| Cmd::DrawIndexed {
            indices,
            instances,
            first,
            base,
            first_instance,
        });
    }
}

unsafe extern "system" fn dispatch(this: Ptr, x: u32, y: u32, z: u32) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.Dispatch)(this, x, y, z);
        note(this, |_| Cmd::Dispatch { groups: [x, y, z] });
    }
}

unsafe extern "system" fn dispatch_mesh(this: Ptr, x: u32, y: u32, z: u32) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList6_Vtbl.DispatchMesh)(this, x, y, z);
        note(this, |_| Cmd::DispatchMesh { groups: [x, y, z] });
    }
}

unsafe extern "system" fn copy_buffer(
    this: Ptr,
    dst: Ptr,
    dst_at: u64,
    src: Ptr,
    src_at: u64,
    len: u64,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.CopyBufferRegion)(
            this, dst, dst_at, src, src_at, len,
        );
        note(this, |r| Cmd::CopyBuffer {
            dst: r.resource(dst),
            dst_at,
            src: r.resource(src),
            src_at,
            len,
        });
    }
}

unsafe fn location(r: &Recorder, l: &D3D12_TEXTURE_COPY_LOCATION) -> Loc {
    // SAFETY: a copy's valid location.
    unsafe {
        let res = r.resource(std::mem::transmute_copy::<_, Ptr>(&l.pResource));
        if l.Type == D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX {
            return Loc::Sub {
                res,
                sub: l.Anonymous.SubresourceIndex,
            };
        }
        let p = l.Anonymous.PlacedFootprint;
        let f = p.Footprint;
        Loc::Placed {
            res,
            offset: p.Offset,
            format: f.Format.0,
            size: [f.Width, f.Height, f.Depth],
            pitch: f.RowPitch,
        }
    }
}

/// A command that changes states whether lists are recorded or not.
unsafe fn note_change(this: Ptr, cmd: impl FnOnce(&Recorder) -> Cmd) {
    // SAFETY: a list with a shim.
    if let Some((r, state)) = unsafe { of(this) } {
        let cmd = cmd(r);
        let mut inner = lock(&state.inner);
        states::changes(&cmd, &mut inner.changes);
        if inner.whole {
            cmd.put(&mut inner.bytes);
        }
    }
}

unsafe extern "system" fn copy_texture(
    this: Ptr,
    dst: *const D3D12_TEXTURE_COPY_LOCATION,
    x: u32,
    y: u32,
    z: u32,
    src: *const D3D12_TEXTURE_COPY_LOCATION,
    part: *const D3D12_BOX,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.CopyTextureRegion)(this, dst, x, y, z, src, part);
        note_change(this, |r| Cmd::CopyTexture {
            dst: location(r, &*dst),
            at: [x, y, z],
            src: location(r, &*src),
            part: part
                .as_ref()
                .map(|b| [b.left, b.top, b.front, b.right, b.bottom, b.back]),
        });
    }
}

unsafe extern "system" fn copy_resource(this: Ptr, dst: Ptr, src: Ptr) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.CopyResource)(this, dst, src);
        note_change(this, |r| Cmd::CopyResource {
            dst: r.resource(dst),
            src: r.resource(src),
        });
    }
}

unsafe extern "system" fn resolve(
    this: Ptr,
    dst: Ptr,
    dst_sub: u32,
    src: Ptr,
    src_sub: u32,
    format: DXGI_FORMAT,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.ResolveSubresource)(
            this, dst, dst_sub, src, src_sub, format,
        );
        note(this, |r| Cmd::Resolve {
            dst: r.resource(dst),
            dst_sub,
            src: r.resource(src),
            src_sub,
            format: format.0,
        });
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn resolve_region(
    this: Ptr,
    dst: Ptr,
    dst_sub: u32,
    x: u32,
    y: u32,
    src: Ptr,
    src_sub: u32,
    part: *const RECT,
    format: DXGI_FORMAT,
    mode: D3D12_RESOLVE_MODE,
) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList1_Vtbl.ResolveSubresourceRegion
        )(this, dst, dst_sub, x, y, src, src_sub, part, format, mode);
        note(this, |r| Cmd::ResolveRegion {
            dst: r.resource(dst),
            dst_sub,
            at: [x, y],
            src: r.resource(src),
            src_sub,
            part: part.as_ref().map(rect),
            format: format.0,
            mode: mode.0,
        });
    }
}

fn rect(r: &RECT) -> [i32; 4] {
    [r.left, r.top, r.right, r.bottom]
}

unsafe fn rects(n: u32, all: *const RECT) -> Vec<[i32; 4]> {
    if all.is_null() || n == 0 {
        return Vec::new();
    }
    // SAFETY: `n` rectangles.
    unsafe { std::slice::from_raw_parts(all, n as usize) }
        .iter()
        .map(rect)
        .collect()
}

unsafe extern "system" fn topology(this: Ptr, kind: D3D_PRIMITIVE_TOPOLOGY) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.IASetPrimitiveTopology)(this, kind);
        note(this, |_| Cmd::Topology { kind: kind.0 });
    }
}

unsafe extern "system" fn viewports(this: Ptr, n: u32, all: *const D3D12_VIEWPORT) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.RSSetViewports)(this, n, all);
        note(this, |_| Cmd::Viewports {
            all: (0..n as usize)
                .map(|i| all.add(i).cast::<[f32; 6]>().read())
                .collect(),
        });
    }
}

unsafe extern "system" fn scissors(this: Ptr, n: u32, all: *const RECT) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.RSSetScissorRects)(this, n, all);
        note(this, |_| Cmd::Scissors { all: rects(n, all) });
    }
}

unsafe extern "system" fn blend_factor(this: Ptr, factor: *const f32) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.OMSetBlendFactor)(this, factor);
        note(this, |_| Cmd::BlendFactor {
            factor: factor.cast::<[f32; 4]>().as_ref().copied(),
        });
    }
}

unsafe extern "system" fn stencil_ref(this: Ptr, value: u32) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.OMSetStencilRef)(this, value);
        note(this, |_| Cmd::StencilRef { value });
    }
}

unsafe extern "system" fn depth_bounds(this: Ptr, min: f32, max: f32) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList1_Vtbl.OMSetDepthBounds)(this, min, max);
        note(this, |_| Cmd::DepthBounds { min, max });
    }
}

unsafe extern "system" fn set_pipeline(this: Ptr, pso: Ptr) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.SetPipelineState)(this, pso);
        note(this, |r| Cmd::Pipeline {
            pso: r.object(pso, "a pipeline"),
        });
    }
}

unsafe extern "system" fn barrier(this: Ptr, n: u32, all: *const D3D12_RESOURCE_BARRIER) {
    // SAFETY: as above; a barrier's type says which of its parts holds.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.ResourceBarrier)(this, n, all);
        note_change(this, |r| {
            let all = std::slice::from_raw_parts(all, n as usize)
                .iter()
                .map(|b| match b.Type {
                    D3D12_RESOURCE_BARRIER_TYPE_ALIASING => {
                        let a = &b.Anonymous.Aliasing;
                        Barrier::Alias {
                            flags: b.Flags.0,
                            before: r
                                .resource(std::mem::transmute_copy::<_, Ptr>(&a.pResourceBefore)),
                            after: r
                                .resource(std::mem::transmute_copy::<_, Ptr>(&a.pResourceAfter)),
                        }
                    }
                    D3D12_RESOURCE_BARRIER_TYPE_UAV => Barrier::Uav {
                        flags: b.Flags.0,
                        res: r.resource(std::mem::transmute_copy::<_, Ptr>(
                            &b.Anonymous.UAV.pResource,
                        )),
                    },
                    _ => {
                        let t = &b.Anonymous.Transition;
                        Barrier::Transition {
                            flags: b.Flags.0,
                            res: r.resource(std::mem::transmute_copy::<_, Ptr>(&t.pResource)),
                            sub: t.Subresource,
                            before: t.StateBefore.0,
                            after: t.StateAfter.0,
                        }
                    }
                })
                .collect();
            Cmd::Barriers { all }
        });
    }
}

unsafe extern "system" fn set_heaps(this: Ptr, n: u32, all: *const Ptr) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.SetDescriptorHeaps)(this, n, all);
        note(this, |r| Cmd::Heaps {
            heaps: (0..n as usize)
                .map(|i| r.object(*all.add(i), "a descriptor heap"))
                .collect(),
        });
    }
}

unsafe extern "system" fn root_compute(this: Ptr, root: Ptr) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.SetComputeRootSignature)(this, root);
        note(this, |r| Cmd::RootSignature {
            graphics: false,
            root: r.object(root, "a root signature"),
        });
    }
}

unsafe extern "system" fn root_graphics(this: Ptr, root: Ptr) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList_Vtbl.SetGraphicsRootSignature
        )(this, root);
        note(this, |r| Cmd::RootSignature {
            graphics: true,
            root: r.object(root, "a root signature"),
        });
    }
}

fn table(r: &Recorder, graphics: bool, param: u32, at: u64) -> Cmd {
    let (heap, slot) = match read(&r.descriptors).gpu(at) {
        Some((h, i)) => (h.id, i),
        None => (NONE, 0),
    };
    if heap == NONE {
        r.problem("a descriptor table in no heap seen made");
    }
    Cmd::Table {
        graphics,
        param,
        heap,
        slot,
    }
}

unsafe extern "system" fn table_compute(this: Ptr, param: u32, at: D3D12_GPU_DESCRIPTOR_HANDLE) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList_Vtbl.SetComputeRootDescriptorTable
        )(this, param, at);
        note(this, |r| table(r, false, param, at.ptr));
    }
}

unsafe extern "system" fn table_graphics(this: Ptr, param: u32, at: D3D12_GPU_DESCRIPTOR_HANDLE) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList_Vtbl.SetGraphicsRootDescriptorTable
        )(this, param, at);
        note(this, |r| table(r, true, param, at.ptr));
    }
}

unsafe extern "system" fn constant_compute(this: Ptr, param: u32, value: u32, offset: u32) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList_Vtbl.SetComputeRoot32BitConstant
        )(this, param, value, offset);
        note(this, |_| Cmd::Constants {
            graphics: false,
            param,
            offset,
            values: vec![value],
        });
    }
}

unsafe extern "system" fn constant_graphics(this: Ptr, param: u32, value: u32, offset: u32) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList_Vtbl.SetGraphicsRoot32BitConstant
        )(this, param, value, offset);
        note(this, |_| Cmd::Constants {
            graphics: true,
            param,
            offset,
            values: vec![value],
        });
    }
}

unsafe fn values(n: u32, data: *const c_void) -> Vec<u32> {
    // SAFETY: `n` values, of any alignment.
    (0..n as usize)
        .map(|i| unsafe { data.cast::<u32>().add(i).read_unaligned() })
        .collect()
}

unsafe extern "system" fn constants_compute(
    this: Ptr,
    param: u32,
    n: u32,
    data: *const c_void,
    offset: u32,
) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList_Vtbl.SetComputeRoot32BitConstants
        )(this, param, n, data, offset);
        note(this, |_| Cmd::Constants {
            graphics: false,
            param,
            offset,
            values: values(n, data),
        });
    }
}

unsafe extern "system" fn constants_graphics(
    this: Ptr,
    param: u32,
    n: u32,
    data: *const c_void,
    offset: u32,
) {
    // SAFETY: as above.
    unsafe {
        own!(
            this,
            ID3D12GraphicsCommandList_Vtbl.SetGraphicsRoot32BitConstants
        )(this, param, n, data, offset);
        note(this, |_| Cmd::Constants {
            graphics: true,
            param,
            offset,
            values: values(n, data),
        });
    }
}

/// A root view's address, reported if it lies in no buffer.
fn address(r: &Recorder, at: u64) -> Va {
    let va = r.va(at);
    if matches!(va, Va::Unknown { .. }) {
        r.problem("a GPU address in no buffer seen made");
    }
    va
}

macro_rules! root_views {
    ($($name:ident, $method:ident, $graphics:literal, $kind:ident;)*) => {$(
        unsafe extern "system" fn $name(this: Ptr, param: u32, at: u64) {
            // SAFETY: as above.
            unsafe {
                own!(this, ID3D12GraphicsCommandList_Vtbl.$method)(this, param, at);
                note(this, |r| Cmd::RootView { graphics: $graphics, kind: slot::$kind, param, va: address(r, at) });
            }
        }
    )*};
}
root_views! {
    cbv_compute, SetComputeRootConstantBufferView, false, CBV;
    cbv_graphics, SetGraphicsRootConstantBufferView, true, CBV;
    srv_compute, SetComputeRootShaderResourceView, false, SRV;
    srv_graphics, SetGraphicsRootShaderResourceView, true, SRV;
    uav_compute, SetComputeRootUnorderedAccessView, false, UAV;
    uav_graphics, SetGraphicsRootUnorderedAccessView, true, UAV;
}

unsafe extern "system" fn index_buffer(this: Ptr, view: *const D3D12_INDEX_BUFFER_VIEW) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.IASetIndexBuffer)(this, view);
        note(this, |r| Cmd::IndexBuffer {
            view: view
                .as_ref()
                .map(|v| (address(r, v.BufferLocation), v.SizeInBytes, v.Format.0)),
        });
    }
}

unsafe extern "system" fn vertex_buffers(
    this: Ptr,
    start: u32,
    n: u32,
    views: *const D3D12_VERTEX_BUFFER_VIEW,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.IASetVertexBuffers)(this, start, n, views);
        note(this, |r| Cmd::VertexBuffers {
            start,
            count: n,
            views: (!views.is_null()).then(|| {
                let all = std::slice::from_raw_parts(views, n as usize);
                all.iter()
                    .map(|v| (address(r, v.BufferLocation), v.SizeInBytes, v.StrideInBytes))
                    .collect()
            }),
        });
    }
}

unsafe extern "system" fn targets(
    this: Ptr,
    n: u32,
    colors: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    range: BOOL,
    depth: *const D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.OMSetRenderTargets)(
            this, n, colors, range, depth,
        );
        note(this, |r| {
            let colors = (0..n as usize)
                .map(|i| {
                    if !range.as_bool() {
                        return r.view((*colors.add(i)).ptr);
                    }
                    let all = read(&r.descriptors);
                    all.cpu((*colors).ptr)
                        .map_or(Slot::EMPTY, |(h, first)| h.get(first + i as u32))
                })
                .collect();
            Cmd::Targets {
                colors,
                depth: depth.as_ref().map(|d| r.view(d.ptr)),
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn clear_depth(
    this: Ptr,
    view: D3D12_CPU_DESCRIPTOR_HANDLE,
    flags: D3D12_CLEAR_FLAGS,
    depth: f32,
    stencil: u8,
    n: u32,
    all: *const RECT,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.ClearDepthStencilView)(
            this, view, flags, depth, stencil, n, all,
        );
        note(this, |r| Cmd::ClearDepth {
            view: r.view(view.ptr),
            flags: flags.0,
            depth,
            stencil,
            rects: rects(n, all),
        });
    }
}

unsafe extern "system" fn clear_target(
    this: Ptr,
    view: D3D12_CPU_DESCRIPTOR_HANDLE,
    color: *const f32,
    n: u32,
    all: *const RECT,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.ClearRenderTargetView)(this, view, color, n, all);
        note(this, |r| Cmd::ClearTarget {
            view: r.view(view.ptr),
            color: color.cast::<[f32; 4]>().read(),
            rects: rects(n, all),
        });
    }
}

unsafe extern "system" fn discard(this: Ptr, res: Ptr, region: *const D3D12_DISCARD_REGION) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.DiscardResource)(this, res, region);
        note(this, |r| Cmd::Discard {
            res: r.resource(res),
            rects: region
                .as_ref()
                .map_or(Vec::new(), |d| rects(d.NumRects, d.pRects)),
            subs: region
                .as_ref()
                .map(|d| [d.FirstSubresource, d.NumSubresources]),
        });
    }
}

unsafe extern "system" fn begin_query(this: Ptr, heap: Ptr, kind: D3D12_QUERY_TYPE, index: u32) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.BeginQuery)(this, heap, kind, index);
        note(this, |r| Cmd::BeginQuery {
            heap: r.object(heap, "a query heap"),
            kind: kind.0,
            index,
        });
    }
}

unsafe extern "system" fn end_query(this: Ptr, heap: Ptr, kind: D3D12_QUERY_TYPE, index: u32) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.EndQuery)(this, heap, kind, index);
        note(this, |r| Cmd::EndQuery {
            heap: r.object(heap, "a query heap"),
            kind: kind.0,
            index,
        });
    }
}

unsafe extern "system" fn resolve_query(
    this: Ptr,
    heap: Ptr,
    kind: D3D12_QUERY_TYPE,
    first: u32,
    count: u32,
    dst: Ptr,
    at: u64,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.ResolveQueryData)(
            this, heap, kind, first, count, dst, at,
        );
        note(this, |r| Cmd::ResolveQuery {
            heap: r.object(heap, "a query heap"),
            kind: kind.0,
            first,
            count,
            dst: r.resource(dst),
            at,
        });
    }
}

unsafe extern "system" fn indirect(
    this: Ptr,
    signature: Ptr,
    most: u32,
    args: Ptr,
    at: u64,
    count: Ptr,
    count_at: u64,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList_Vtbl.ExecuteIndirect)(
            this, signature, most, args, at, count, count_at,
        );
        note(this, |r| Cmd::Indirect {
            signature: r.object(signature, "a command signature"),
            most,
            args: r.resource(args),
            at,
            count: r.resource(count),
            count_at,
        });
    }
}

unsafe extern "system" fn write_immediate(
    this: Ptr,
    n: u32,
    all: *const D3D12_WRITEBUFFERIMMEDIATE_PARAMETER,
    modes: *const D3D12_WRITEBUFFERIMMEDIATE_MODE,
) {
    // SAFETY: as above.
    unsafe {
        own!(this, ID3D12GraphicsCommandList2_Vtbl.WriteBufferImmediate)(this, n, all, modes);
        note(this, |r| Cmd::WriteImmediate {
            all: (0..n as usize)
                .map(|i| {
                    let p = all.add(i).read();
                    let mode = if modes.is_null() {
                        0
                    } else {
                        modes.add(i).read().0
                    };
                    (address(r, p.Dest), p.Value, mode)
                })
                .collect(),
        });
    }
}

/// The first subresources of all: what [`ALL`] means in a barrier.
const _: () = assert!(ALL == D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_to_record() {
        assert_eq!(parse_at("120"), At::Seconds(120.0));
        assert_eq!(parse_at(" 1.5 "), At::Seconds(1.5));
        assert_eq!(parse_at("f900"), At::Frame(900));
        assert_eq!(parse_at(""), At::Trigger);
        assert_eq!(parse_at("later"), At::Trigger);
    }

    #[test]
    fn descriptor_heaps_are_found_by_address() {
        let heap = |id, cpu, gpu, count| {
            let d = D3D12_DESCRIPTOR_HEAP_DESC {
                NumDescriptors: count,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                ..Default::default()
            };
            Arc::new(Heap::new(id, &d, cpu, gpu, 32))
        };
        let mut all = Heaps::default();
        all.add(heap(1, 0x1000, 0x10_0000, 4));
        all.add(heap(2, 0x2000, 0x20_0000, 8));
        let at = |all: &Heaps, p| all.cpu(p).map(|(h, i)| (h.id, i));
        assert_eq!(at(&all, 0x1000), Some((1, 0)));
        assert_eq!(at(&all, 0x1060), Some((1, 3)));
        assert_eq!(at(&all, 0x1080), None, "past its end");
        assert_eq!(at(&all, 0xfff), None);
        assert_eq!(all.gpu(0x20_0040).map(|(h, i)| (h.id, i)), Some((2, 2)));
        // A heap made where a dead one was takes its place.
        all.add(heap(3, 0x1020, 0x30_0000, 4));
        assert_eq!(at(&all, 0x1000), None);
        assert_eq!(at(&all, 0x1020), Some((3, 0)));
        all.remove(2);
        assert_eq!(at(&all, 0x2000), None);
        let h = heap(4, 0, 0, 2);
        let s = Slot {
            kind: slot::SRV,
            res: 5,
            ..Slot::EMPTY
        };
        h.set(1, &s);
        h.set(2, &s);
        assert_eq!(h.all(), [Slot::EMPTY, s]);
        assert_eq!(h.get(9), Slot::EMPTY);
    }
}
