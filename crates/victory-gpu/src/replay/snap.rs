//! One frame written down: what its submissions run, what the card's
//! memory held at its start, and what the CPU put into memory both see
//! before each submission.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use super::gpu::{self, Seen, Worker};
use super::record::{At, Heap, Ours, Recorder, Res, lock, not_recorded_calls, read};
use super::states::{COMMON, COPY_SOURCE, Change, States};
use super::wire::{
    self, ALL, Cmd, Content, Delta, Loc, MISSING, NONE, Place, Slot, Va, ZERO, slot,
};

type Fault = Box<dyn std::error::Error + Send + Sync>;

/// A read-back buffer contents go through, and how many there are.
const STAGING: u64 = 256 << 20;
const STAGINGS: usize = 4;
/// The most of a buffer in one piece.
const PIECE: u64 = 32 << 20;
/// What the CPU changes is kept in units of.
const PAGE: usize = 4096;
/// What one thread compares at a time.
const CHUNK: usize = 1 << 20;
/// Frames waited, at the most, for one whose lists are all recorded.
const PATIENCE: u64 = 100;

#[derive(Default)]
enum Stage {
    #[default]
    Waiting,
    /// Lists are recorded, since this many presents.
    Armed(u64),
    Capturing(Box<Capture>),
    Done,
}

/// Where the recorder is.
#[derive(Default)]
pub(super) struct Phase {
    presents: u64,
    stage: Stage,
    /// Submissions of this frame so far, and lists among them that were not
    /// recorded.
    submits: u32,
    missing: u32,
}

impl Phase {
    pub(super) fn done(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }
}

/// A submission to a queue other than the drawing one: left out.
pub(super) fn other_queue(r: &Recorder, lists: u32) {
    if r.watching.load(Ordering::SeqCst) {
        r.problem_times(
            "lists sent to a queue other than the drawing one are not in the capture",
            u64::from(lists),
        );
    }
}

/// A submission to the drawing queue, made by `go`.
pub(super) fn submit(
    r: &Recorder,
    queue: &ID3D12CommandQueue,
    lists: &[*mut std::ffi::c_void],
    go: impl FnOnce(),
) {
    let mut phase = lock(&r.submits);
    type Closed = (Option<Arc<Vec<u8>>>, Arc<Vec<Change>>);
    let closed: Vec<Option<Closed>> = lists
        .iter()
        .map(|&p| r.list(p).and_then(|l| l.closed()))
        .collect();
    phase.submits += 1;
    let whole = |c: &Option<Closed>| c.as_ref().is_some_and(|c| c.0.is_some());
    phase.missing += lists
        .iter()
        .zip(&closed)
        .filter(|(p, c)| !whole(c) && r.knows(**p))
        .count() as u32;
    if let Stage::Capturing(c) = &mut phase.stage {
        let commands: Vec<Option<Arc<Vec<u8>>>> = closed
            .iter()
            .map(|c| c.as_ref().and_then(|c| c.0.clone()))
            .collect();
        let _ours = Ours::new();
        if let Err(e) = c.submit(r, queue, &commands) {
            stop(r, &mut phase, &*e);
        }
    }
    go();
    let mut states = lock(&r.states);
    for change in closed.iter().flatten().flat_map(|c| c.1.iter()) {
        states.change(change);
    }
    states.decay();
}

/// The capture failed.
fn stop(r: &Recorder, phase: &mut Phase, e: &(dyn std::error::Error + Send + Sync)) {
    tracing::error!("frame recorder: the capture failed: {e}");
    let _ = std::fs::write(r.folder.join("failed.txt"), e.to_string());
    phase.stage = Stage::Done;
    r.watching.store(false, Ordering::SeqCst);
    r.arm(false);
    let held = std::mem::take(&mut *lock(&r.held));
    drop(held);
}

pub(super) fn present(r: &Recorder, texture: &ID3D12Resource, state: u32) {
    let mut phase = lock(&r.submits);
    phase.presents += 1;
    let presents = phase.presents;
    let (submits, missing) = (
        std::mem::take(&mut phase.submits),
        std::mem::take(&mut phase.missing),
    );
    let mut finished = None;
    match &mut phase.stage {
        Stage::Waiting => {
            let due = match r.settings.at {
                At::Seconds(s) => r.began.elapsed().as_secs_f64() >= s,
                At::Frame(f) => presents >= f,
                At::Trigger => false,
            };
            if due || r.settings.folder.join("go").exists() {
                tracing::info!(presents, "frame recorder: lists are recorded from now on");
                r.arm(true);
                phase.stage = Stage::Armed(presents);
            }
        }
        Stage::Armed(since) => {
            let waited = presents - *since;
            let whole = submits > 0 && missing == 0;
            if waited >= 2 && (whole || waited > PATIENCE) {
                if !whole {
                    r.problem("no frame was found whose lists were all recorded");
                }
                tracing::info!(
                    presents,
                    waited,
                    "frame recorder: the next frame is captured"
                );
                match Capture::new(r) {
                    Ok(c) => phase.stage = Stage::Capturing(Box::new(c)),
                    Err(e) => stop(r, &mut phase, &*e),
                }
            }
        }
        Stage::Capturing(c) if c.started => {
            let _ours = Ours::new();
            let done = c.present(r, texture, state).and_then(|()| {
                if c.frame < r.settings.frames {
                    return Ok(false);
                }
                c.finish(r)?;
                Ok(true)
            });
            match done {
                Ok(false) => {}
                Ok(true) => {
                    r.watching.store(false, Ordering::SeqCst);
                    r.arm(false);
                    if let Stage::Capturing(c) = std::mem::replace(&mut phase.stage, Stage::Done) {
                        finished = Some((c, std::mem::take(&mut *lock(&r.held))));
                    }
                }
                Err(e) => stop(r, &mut phase, &*e),
            }
        }
        // A frame without a submission: the next one.
        Stage::Capturing(_) | Stage::Done => {}
    }
    drop(phase);
    // What was kept alive goes without a lock held.
    drop(finished);
}

/// What a submission reads of buffers, by id: parts of them (copied from),
/// or all (read some other way).
#[derive(Default)]
struct Reads {
    all: HashSet<u32>,
    parts: HashMap<u32, Vec<(u64, u64)>>,
}

impl Reads {
    fn note(&mut self, cmds: &[Cmd]) {
        let (all, parts) = (&mut self.all, &mut self.parts);
        let mut whole = |va: &Va| {
            if let Va::At { res, .. } = va {
                all.insert(*res);
            }
        };
        for c in cmds {
            match c {
                Cmd::RootView { va, .. }
                | Cmd::IndexBuffer {
                    view: Some((va, _, _)),
                } => whole(va),
                Cmd::VertexBuffers {
                    views: Some(views), ..
                } => views.iter().for_each(|(va, _, _)| whole(va)),
                Cmd::Indirect { args, count, .. } => {
                    for res in [*args, *count] {
                        whole(&Va::At { res, offset: 0 });
                    }
                }
                Cmd::CopyResource { src, .. } => whole(&Va::At {
                    res: *src,
                    offset: 0,
                }),
                Cmd::CopyBuffer {
                    src, src_at, len, ..
                } => parts.entry(*src).or_default().push((*src_at, src_at + len)),
                Cmd::CopyTexture {
                    src:
                        Loc::Placed {
                            res,
                            offset,
                            size,
                            pitch,
                            ..
                        },
                    ..
                } => {
                    // At the most: compressed formats have fewer rows.
                    let len = u64::from(*pitch) * u64::from(size[1]) * u64::from(size[2].max(1));
                    parts.entry(*res).or_default().push((*offset, offset + len));
                }
                _ => {}
            }
        }
    }

    /// The whole pages read of a buffer of `size` bytes; `viewed` by a
    /// descriptor, all of it is.
    fn pages(&self, id: u32, size: usize, viewed: bool) -> Vec<(usize, usize)> {
        if viewed || self.all.contains(&id) {
            return vec![(0, size)];
        }
        let Some(parts) = self.parts.get(&id) else {
            return Vec::new();
        };
        let mut pages: Vec<(usize, usize)> = parts
            .iter()
            .map(|&(from, to)| {
                (
                    from as usize / PAGE * PAGE,
                    (to as usize).next_multiple_of(PAGE).min(size),
                )
            })
            .filter(|(from, to)| from < to)
            .collect();
        pages.sort_unstable();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (from, to) in pages {
            match out.last_mut() {
                Some(last) if from <= last.1 => last.1 = last.1.max(to),
                _ => out.push((from, to)),
            }
        }
        out
    }
}

/// A buffer the CPU sees.
struct CpuBuffer {
    id: u32,
    /// Held by the recorder, not by this.
    res: std::mem::ManuallyDrop<ID3D12Resource>,
    /// Where it is read: its mapping, or the read-back copy of it.
    live: *const u8,
    /// The read-back buffer and the offset in it, for one that is slow to
    /// read.
    through: Option<(usize, u64)>,
    size: usize,
    /// What it held at the submission before (zeros before the first).
    kept: Vec<u8>,
}

// SAFETY: the mapping is of a buffer the struct holds.
unsafe impl Send for CpuBuffer {}
// SAFETY: as above; the mapping is only read.
unsafe impl Sync for CpuBuffer {}

struct Capture {
    worker: Worker,
    others: Vec<Worker>,
    started: bool,
    frame: u32,
    states: States,
    idle: HashSet<u32>,
    heaps: Vec<(Arc<Heap>, Vec<(u32, u32, Slot)>)>,
    contents: Vec<Content>,
    buffers: Vec<CpuBuffer>,
    known: HashSet<u32>,
    /// Resources descriptors name, or named during the frame.
    viewed: HashSet<u32>,
    /// Read-back buffers for the slow ones: the buffer, its mapping, size,
    /// and how much of it is taken.
    blocks: Vec<(ID3D12Resource, usize, u64, u64)>,
    deltas: File,
    delta_at: AtomicU64,
    lists: Vec<Arc<Vec<u8>>>,
    submits: Vec<wire::Submit>,
    presents: Vec<wire::Present>,
    present_file: File,
    present_at: u64,
    calls: Vec<(String, u64)>,
    began: Instant,
    took: BTreeMap<&'static str, std::time::Duration>,
}

impl Capture {
    fn new(r: &Recorder) -> Result<Capture, Fault> {
        let _ours = Ours::new();
        let device = r.device().ok_or("no device")?;
        let queues = lock(&r.queues).clone();
        let direct = queues
            .iter()
            .find(|q| q.1 == D3D12_COMMAND_LIST_TYPE_DIRECT.0)
            .ok_or("no drawing queue")?;
        let mut others = Vec::new();
        for (queue, _) in queues.iter().filter(|q| q.0 != direct.0) {
            others.push(Worker::new(&device, queue)?);
        }
        Ok(Capture {
            worker: Worker::new(&device, &direct.0)?,
            others,
            started: false,
            frame: 0,
            states: States::default(),
            idle: HashSet::new(),
            heaps: Vec::new(),
            contents: Vec::new(),
            buffers: Vec::new(),
            known: HashSet::new(),
            viewed: HashSet::new(),
            blocks: Vec::new(),
            deltas: File::create(r.folder.join("deltas.bin"))?,
            delta_at: AtomicU64::new(0),
            lists: Vec::new(),
            submits: Vec::new(),
            presents: Vec::new(),
            present_file: File::create(r.folder.join("present.bin"))?,
            present_at: 0,
            calls: not_recorded_calls(),
            began: Instant::now(),
            took: BTreeMap::new(),
        })
    }

    fn timed<T>(
        &mut self,
        what: &'static str,
        f: impl FnOnce(&mut Self) -> Result<T, Fault>,
    ) -> Result<T, Fault> {
        let began = Instant::now();
        let out = f(self);
        *self.took.entry(what).or_default() += began.elapsed();
        out
    }

    /// Before the first submission of the frame: what the card holds.
    fn start(&mut self, r: &Recorder) -> Result<(), Fault> {
        self.began = Instant::now();
        r.watching.store(true, Ordering::SeqCst);
        self.started = true;
        self.worker.idle()?;
        for w in &mut self.others {
            w.idle()?;
        }
        let mut all: Vec<Arc<Res>> = Vec::new();
        {
            let live = read(&r.resources);
            let mut held = lock(&r.held);
            for res in live.values().filter(|res| !res.late) {
                let object = res.object();
                // SAFETY: a live resource, or one whose end waits for the
                // lock held here (`Recorder::gone`): the count tells which.
                let count = unsafe {
                    (Interface::vtable(&*object)
                        .base__
                        .base__
                        .base__
                        .base__
                        .AddRef)(object.as_raw())
                };
                if count <= 1 {
                    // Dying: the reference does not bring it back, and
                    // giving it back would end it a second time.
                    continue;
                }
                // SAFETY: the reference just taken. (The count does not tell
                // all that are dying: `Recorder::gone` takes those out.)
                held.insert(res.id, unsafe { ID3D12Resource::from_raw(object.as_raw()) });
                all.push(res.clone());
            }
        }
        all.sort_by_key(|res| res.id);
        self.states = lock(&r.states).clone();
        self.idle = self.find_idle(&all);
        self.timed("contents of the card's memory", |c| c.snapshot(r, &all))?;
        Ok(())
    }

    /// Of placed resources that overlap, those that are not the one used
    /// last.
    fn find_idle(&self, all: &[Arc<Res>]) -> HashSet<u32> {
        let mut heaps: HashMap<u32, Vec<(u64, u64, u64, u32)>> = HashMap::new();
        for res in all {
            if let Place::Placed { heap, offset } = res.place {
                // SAFETY: a plain query.
                let info = unsafe {
                    self.worker
                        .device
                        .GetResourceAllocationInfo(0, &[gpu::to_desc(&res.desc)])
                };
                let size = if info.SizeInBytes == u64::MAX {
                    res.desc.width
                } else {
                    info.SizeInBytes
                };
                let used = self.states.used.get(&res.id).copied().unwrap_or(0);
                heaps
                    .entry(heap)
                    .or_default()
                    .push((used, offset, offset + size, res.id));
            }
        }
        let mut idle = HashSet::new();
        for (_, mut list) in heaps {
            list.sort_by(|a, b| b.cmp(a));
            let mut taken: BTreeMap<u64, u64> = BTreeMap::new();
            for (_, from, to, id) in list {
                let overlaps = taken
                    .range(..to)
                    .next_back()
                    .is_some_and(|(_, &end)| end > from);
                if overlaps {
                    idle.insert(id);
                } else {
                    taken.insert(from, to);
                }
            }
        }
        idle
    }

    fn snapshot(&mut self, r: &Recorder, all: &[Arc<Res>]) -> Result<(), Fault> {
        let device = self.worker.device.clone();
        let mut pieces: Vec<Piece> = Vec::new();
        for res in all.iter().filter(|res| res.seen == Seen::No) {
            if matches!(res.place, Place::BackBuffer {}) || self.idle.contains(&res.id) {
                continue;
            }
            if res.is_buffer() {
                let mut at = 0;
                while at < res.desc.width {
                    let len = PIECE.min(res.desc.width - at);
                    pieces.push(Piece {
                        res: res.clone(),
                        first: at,
                        count: 1,
                        len,
                        at: 0,
                    });
                    at += len;
                }
                continue;
            }
            if res.desc.samples[0] > 1 {
                r.problem("contents of multisampled textures are not in the capture");
                continue;
            }
            let desc = gpu::to_desc(&res.desc);
            let (_, total) = gpu::footprints(&device, &desc, 0, res.subs);
            if total == u64::MAX {
                r.problem("contents of a texture without a copyable layout are not in the capture");
            } else if total <= STAGING {
                pieces.push(Piece {
                    res: res.clone(),
                    first: 0,
                    count: res.subs,
                    len: total,
                    at: 0,
                });
            } else {
                for sub in 0..res.subs {
                    let (_, len) = gpu::footprints(&device, &desc, sub, 1);
                    if len <= STAGING {
                        pieces.push(Piece {
                            res: res.clone(),
                            first: u64::from(sub),
                            count: 1,
                            len,
                            at: 0,
                        });
                    } else {
                        r.problem(
                            "contents of a texture level of over 256 MB are not in the capture",
                        );
                    }
                }
            }
        }
        // Into batches, one read-back buffer each.
        let mut batches: Vec<Vec<Piece>> = vec![Vec::new()];
        let mut at = 0u64;
        for mut p in pieces {
            at = at.next_multiple_of(512);
            if at + p.len > STAGING {
                batches.push(Vec::new());
                at = 0;
            }
            p.at = at;
            at += p.len;
            if let Some(b) = batches.last_mut() {
                b.push(p);
            }
        }
        let mut stagings = Vec::new();
        for _ in 0..STAGINGS.min(batches.len()) {
            stagings.push(gpu::buffer(
                &device,
                D3D12_HEAP_TYPE_READBACK,
                STAGING,
                D3D12_RESOURCE_FLAG_NONE,
            )?);
        }
        let file = File::create(r.folder.join("contents.bin"))?;
        let end = AtomicU64::new(0);
        let contents: std::sync::Mutex<Vec<Content>> = std::sync::Mutex::default();
        let failed: std::sync::Mutex<Option<std::io::Error>> = std::sync::Mutex::default();
        let (todo, work) = std::sync::mpsc::channel::<(usize, Vec<Piece>)>();
        let work = std::sync::Mutex::new(work);
        let (free, freed) = std::sync::mpsc::channel::<usize>();
        for i in 0..stagings.len() {
            free.send(i)?;
        }
        let maps: Vec<usize> = stagings.iter().map(|s| s.1 as usize).collect();
        std::thread::scope(|scope| -> Result<(), Fault> {
            for _ in 0..stagings.len() {
                let (work, free, maps, file, end, contents, failed) =
                    (&work, free.clone(), &maps, &file, &end, &contents, &failed);
                scope.spawn(move || {
                    loop {
                        let next = lock(work).recv();
                        let Ok((i, pieces)) = next else { break };
                        for p in pieces {
                            // SAFETY: inside the buffer's mapping; the copy
                            // into it has run.
                            let bytes = unsafe {
                                std::slice::from_raw_parts(
                                    (maps[i] + p.at as usize) as *const u8,
                                    p.len as usize,
                                )
                            };
                            let at = if zero(bytes) {
                                ZERO
                            } else {
                                end.fetch_add(p.len, Ordering::Relaxed)
                            };
                            if at != ZERO
                                && let Err(e) = write_at(file, bytes, at)
                            {
                                lock(failed).get_or_insert(e);
                            }
                            lock(contents).push(Content {
                                res: p.res.id,
                                first: p.first,
                                count: p.count,
                                len: p.len,
                                file: at,
                            });
                        }
                        let _ = free.send(i);
                    }
                });
            }
            // Moved in: the writers end when it goes.
            let todo = todo;
            for mut pieces in batches {
                {
                    let held = lock(&r.held);
                    pieces.retain(|p| held.contains_key(&p.res.id));
                }
                if pieces.is_empty() {
                    continue;
                }
                let i = freed.recv()?;
                let value = self
                    .worker
                    .send(|list| copy_out(&device, list, &self.states, &pieces, &stagings[i].0))?;
                self.worker.wait(value)?;
                todo.send((i, pieces))?;
            }
            Ok(())
        })?;
        if let Some(e) = lock(&failed).take() {
            return Err(e.into());
        }
        self.contents = contents.into_inner().unwrap_or_else(|e| e.into_inner());
        Ok(())
    }

    /// Buffers the CPU sees that were not known yet.
    fn new_buffers(&mut self, r: &Recorder) -> Result<(), Fault> {
        let fresh: Vec<Arc<Res>> = read(&r.resources)
            .values()
            .filter(|res| {
                matches!(res.seen, Seen::Cached | Seen::Combined) && !self.known.contains(&res.id)
            })
            .cloned()
            .collect();
        for res in fresh {
            self.known.insert(res.id);
            if !res.is_buffer() {
                r.problem("contents of textures in memory the CPU sees are not in the capture");
                continue;
            }
            // Those alive at the start are held, those made since too; what
            // is not is dying.
            if !lock(&r.held).contains_key(&res.id) {
                continue;
            }
            let object = res.object();
            let size = res.desc.width as usize;
            let (live, through) = if res.seen == Seen::Cached {
                let mut data = std::ptr::null_mut();
                // SAFETY: a live buffer in memory the CPU sees.
                unsafe { object.Map(0, None, Some(&mut data))? };
                (data.cast::<u8>().cast_const(), None)
            } else {
                let room = self.blocks.iter().position(|b| b.3 + size as u64 <= b.2);
                let i = match room {
                    Some(i) => i,
                    None => {
                        let len = STAGING.max(size as u64);
                        let (b, data) = gpu::buffer(
                            &self.worker.device,
                            D3D12_HEAP_TYPE_READBACK,
                            len,
                            D3D12_RESOURCE_FLAG_NONE,
                        )?;
                        self.blocks.push((b, data as usize, len, 0));
                        self.blocks.len() - 1
                    }
                };
                let b = &mut self.blocks[i];
                let at = b.3;
                b.3 = (at + size as u64).next_multiple_of(256);
                ((b.1 + at as usize) as *const u8, Some((i, at)))
            };
            self.buffers.push(CpuBuffer {
                id: res.id,
                res: object,
                live,
                through,
                size,
                kept: vec![0; size],
            });
        }
        Ok(())
    }

    /// What changed in the buffers the CPU sees, of what a submission reads
    /// of them, since they were last looked at (the first time: what is not
    /// zero).
    fn changed(&mut self, r: &Recorder, reads: &Reads) -> Result<Vec<Delta>, Fault> {
        self.new_buffers(r)?;
        {
            let held = lock(&r.held);
            self.buffers.retain(|b| held.contains_key(&b.id));
        }
        let parts: Vec<(usize, usize, usize)> = self
            .buffers
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                reads
                    .pages(b.id, b.size, self.viewed.contains(&b.id))
                    .into_iter()
                    .map(move |(from, to)| (i, from, to))
            })
            .collect();
        if parts.iter().any(|p| self.buffers[p.0].through.is_some()) {
            let (buffers, blocks) = (&self.buffers, &self.blocks);
            self.worker.run(|list| {
                for &(i, from, to) in &parts {
                    let b = &buffers[i];
                    if let Some((block, at)) = b.through {
                        // SAFETY: an open list and live buffers.
                        unsafe {
                            list.CopyBufferRegion(
                                &blocks[block].0,
                                at + from as u64,
                                &*b.res,
                                from as u64,
                                (to - from) as u64,
                            )
                        };
                    }
                }
            })?;
        }
        let chunks: Vec<(usize, usize, usize)> = parts
            .iter()
            .flat_map(|&(i, from, to)| {
                (from..to)
                    .step_by(CHUNK)
                    .map(move |at| (i, at, CHUNK.min(to - at)))
            })
            .collect();
        // Each chunk is one thread's; threads share what is kept by parts.
        struct Kept(*mut u8);
        // SAFETY: parts of it are used by one thread each.
        unsafe impl Sync for Kept {}
        let kept: Vec<Kept> = self
            .buffers
            .iter_mut()
            .map(|b| Kept(b.kept.as_mut_ptr()))
            .collect();
        let next = AtomicUsize::new(0);
        let out: std::sync::Mutex<Vec<Delta>> = std::sync::Mutex::default();
        let failed: std::sync::Mutex<Option<std::io::Error>> = std::sync::Mutex::default();
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get() / 2)
            .clamp(1, 16)
            .min(chunks.len().max(1));
        let (buffers, file, end) = (&self.buffers, &self.deltas, &self.delta_at);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(b, at, len)) = chunks.get(i) else {
                            break;
                        };
                        // SAFETY: a chunk of the buffer's memory (it may
                        // change meanwhile: what is kept is what was read)
                        // and of what is kept, which no other thread has.
                        let runs = unsafe {
                            changed_runs(buffers[b].live.add(at), kept[b].0.add(at), len)
                        };
                        for (from, to) in runs {
                            // SAFETY: as above.
                            let bytes = unsafe {
                                std::slice::from_raw_parts(kept[b].0.add(at + from), to - from)
                            };
                            let file_at = end.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                            if let Err(e) = write_at(file, bytes, file_at) {
                                lock(&failed).get_or_insert(e);
                            }
                            mine.push(Delta {
                                res: buffers[b].id,
                                at: (at + from) as u64,
                                len: bytes.len() as u64,
                                file: file_at,
                            });
                        }
                    }
                    lock(&out).extend(mine);
                });
            }
        });
        if let Some(e) = lock(&failed).take() {
            return Err(e.into());
        }
        let mut out = out.into_inner().unwrap_or_else(|e| e.into_inner());
        out.sort_by_key(|d| (d.res, d.at));
        Ok(out)
    }

    fn submit(
        &mut self,
        r: &Recorder,
        _queue: &ID3D12CommandQueue,
        lists: &[Option<Arc<Vec<u8>>>],
    ) -> Result<(), Fault> {
        let first = !self.started;
        if first {
            self.start(r)?;
        } else {
            self.timed("waiting for the card", |c| Ok(c.worker.idle()?))?;
        }
        // Descriptors first: what they name is looked at below.
        let mut written = std::mem::take(&mut *lock(&r.writes));
        let mut views = Vec::new();
        if first {
            let heaps: Vec<Arc<Heap>> = read(&r.descriptors).visible().cloned().collect();
            for h in heaps {
                let runs = runs_of(r, &h.all());
                self.viewed
                    .extend(runs.iter().flat_map(|(_, _, s)| [s.res, s.counter]));
                self.heaps.push((h, runs));
            }
        } else {
            written.sort_unstable();
            let mut seen = HashSet::new();
            for (heap, at, n) in written {
                let h = match self.heaps.iter().find(|(h, _)| h.id == heap) {
                    Some((h, _)) => h.clone(),
                    None => {
                        let found = read(&r.descriptors)
                            .visible()
                            .find(|h| h.id == heap)
                            .cloned();
                        let Some(h) = found else { continue };
                        self.heaps.push((h.clone(), Vec::new()));
                        h
                    }
                };
                for i in at..at + n {
                    if seen.insert((heap, i)) {
                        let s = resolved(r, h.get(i));
                        self.viewed.extend([s.res, s.counter]);
                        views.push((heap, i, s));
                    }
                }
            }
        }
        let mut reads = Reads::default();
        for l in lists.iter().flatten() {
            reads.note(&Cmd::all(l)?);
        }
        let deltas = self.timed("what the CPU changed", |c| c.changed(r, &reads))?;
        let mut numbers = Vec::new();
        for l in lists {
            numbers.push(match l {
                None => {
                    r.problem(
                        "a list was submitted that was not recorded whole: its work is missing",
                    );
                    MISSING
                }
                Some(commands) => match self
                    .lists
                    .iter()
                    .position(|have| Arc::ptr_eq(have, commands))
                {
                    Some(i) => i as u32,
                    None => {
                        self.lists.push(commands.clone());
                        self.lists.len() as u32 - 1
                    }
                },
            });
        }
        self.submits.push(wire::Submit {
            frame: self.frame,
            lists: numbers,
            deltas,
            views,
        });
        Ok(())
    }

    fn present(&mut self, r: &Recorder, texture: &ID3D12Resource, state: u32) -> Result<(), Fault> {
        self.worker.idle()?;
        // SAFETY: a live texture.
        let desc = unsafe { texture.GetDesc() };
        let (f, total) = gpu::footprints(&self.worker.device, &desc, 0, 1);
        let (back, data) = gpu::buffer(
            &self.worker.device,
            D3D12_HEAP_TYPE_READBACK,
            total,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        self.worker
            .run(|list| read_sub(list, texture, 0, state, &back, f[0]))?;
        // SAFETY: the buffer's mapping; the copy into it has run.
        let bytes = unsafe { std::slice::from_raw_parts(data, total as usize) };
        write_at(&self.present_file, bytes, self.present_at)?;
        let id = r.resource(texture.as_raw());
        let (width, height, pitch) = (desc.Width as u32, desc.Height, f[0].Footprint.RowPitch);
        self.presents.push(wire::Present {
            submits: self.submits.len() as u32,
            res: id,
            width,
            height,
            format: desc.Format.0,
            pitch,
            file: self.present_at,
        });
        self.present_at += total;
        match gpu::rgb(width, height, pitch, desc.Format.0, bytes) {
            Some(rgb) => std::fs::write(
                r.folder.join(format!("frame-{}.png", self.frame)),
                crate::capture::png(width, height, &rgb),
            )?,
            None => r.problem("the presented format is not shown in a PNG"),
        }
        self.frame += 1;
        Ok(())
    }

    /// Writes `frame.bin`.
    fn finish(&mut self, r: &Recorder) -> Result<(), Fault> {
        let mut resources: Vec<wire::Resource> = Vec::new();
        {
            let names = lock(&r.names);
            for res in read(&r.resources).values() {
                let state = match (res.is_buffer(), res.late) {
                    (true, _) => vec![COMMON],
                    (_, true) => vec![res.made_in as u32],
                    _ => self.states.get(res.id).unwrap_or(vec![COMMON]),
                };
                resources.push(wire::Resource {
                    id: res.id,
                    place: res.place.clone(),
                    desc: res.desc.clone(),
                    made_in: res.made_in,
                    clear: res.clear,
                    state,
                    idle: self.idle.contains(&res.id),
                    late: res.late,
                    name: names.get(&res.id).cloned().unwrap_or_default(),
                });
            }
        }
        resources.sort_by_key(|res| res.id);
        let mut heaps: Vec<wire::Heap> = {
            let all = lock(&r.heaps);
            let used: HashSet<u32> = resources
                .iter()
                .filter_map(|res| {
                    if let Place::Placed { heap, .. } = res.place {
                        Some(heap)
                    } else {
                        None
                    }
                })
                .collect();
            used.iter().filter_map(|id| all.get(id).cloned()).collect()
        };
        heaps.sort_by_key(|h| h.id);
        // What the lists name.
        let (mut psos, mut roots, mut signatures, mut queries) = (
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
        );
        for l in &self.lists {
            for c in Cmd::all(l)? {
                match c {
                    Cmd::Pipeline { pso } => _ = psos.insert(pso),
                    Cmd::RootSignature { root, .. } => _ = roots.insert(root),
                    Cmd::Indirect { signature, .. } => _ = signatures.insert(signature),
                    Cmd::BeginQuery { heap, .. }
                    | Cmd::EndQuery { heap, .. }
                    | Cmd::ResolveQuery { heap, .. } => _ = queries.insert(heap),
                    _ => {}
                }
            }
        }
        let pipelines: Vec<wire::Pipeline> = {
            let all = lock(&r.pipelines);
            let names = lock(&r.names);
            let mut out: Vec<wire::Pipeline> = psos
                .iter()
                .filter_map(|id| {
                    let parts = all.get(id)?;
                    Some(wire::Pipeline {
                        id: *id,
                        parts: parts.to_vec(),
                        name: names.get(id).cloned().unwrap_or_default(),
                    })
                })
                .collect();
            out.sort_by_key(|p| p.id);
            out
        };
        roots.extend(pipelines.iter().flat_map(|p| &p.parts).filter_map(|p| {
            if let wire::Part::Root { root } = p {
                Some(*root)
            } else {
                None
            }
        }));
        let signatures: Vec<wire::Signature> = {
            let all = lock(&r.signatures);
            signatures
                .iter()
                .filter_map(|id| all.get(id).cloned())
                .collect()
        };
        roots.extend(signatures.iter().map(|s| s.root));
        roots.remove(&NONE);
        let roots: Vec<(u32, wire::Blob)> = {
            let all = lock(&r.roots);
            roots
                .iter()
                .filter_map(|id| Some((*id, wire::Blob(all.get(id)?.to_vec()))))
                .collect()
        };
        for ((what, before), (_, now)) in self.calls.iter().zip(not_recorded_calls()) {
            if now > *before {
                r.problem_times(what, now - before);
            }
        }
        let (adapter, vendor, device) = gpu::adapter(&self.worker.device);
        let mut notes = r.problems();
        let bytes: u64 = self
            .contents
            .iter()
            .filter(|c| c.file != ZERO)
            .map(|c| c.len)
            .sum();
        notes.push(format!(
            "{} submissions, {} lists, {} resources, {} pipelines; {} MB of the card's memory, {} MB of changes by the CPU; {:.1} s: {}",
            self.submits.len(),
            self.lists.len(),
            resources.len(),
            pipelines.len(),
            bytes >> 20,
            self.delta_at.load(Ordering::Relaxed) >> 20,
            self.began.elapsed().as_secs_f64(),
            self.took.iter().map(|(what, d)| format!("{what} {:.1} s", d.as_secs_f64())).collect::<Vec<_>>().join(", ")
        ));
        for n in &notes {
            tracing::info!("frame recorder: {n}");
        }
        let frame = wire::Frame {
            adapter,
            vendor,
            device,
            notes: notes.clone(),
            heaps,
            resources,
            roots,
            codes: lock(&r.codes).all.clone(),
            pipelines,
            signatures,
            descriptor_heaps: self
                .heaps
                .iter()
                .map(|(h, runs)| wire::DescriptorHeap {
                    id: h.id,
                    kind: h.kind,
                    count: h.count,
                    flags: h.flags,
                    runs: runs.clone(),
                })
                .collect(),
            query_heaps: {
                let all = lock(&r.queries);
                queries
                    .iter()
                    .filter_map(|id| all.get(id).cloned())
                    .collect()
            },
            lists: self
                .lists
                .iter()
                .map(|l| wire::List {
                    kind: D3D12_COMMAND_LIST_TYPE_DIRECT.0,
                    commands: wire::Blob(l.to_vec()),
                })
                .collect(),
            submits: std::mem::take(&mut self.submits),
            contents: std::mem::take(&mut self.contents),
            presents: std::mem::take(&mut self.presents),
        };
        frame.write(&r.folder.join("frame.bin"))?;
        std::fs::write(r.folder.join("notes.txt"), notes.join("\n"))?;
        tracing::info!(folder = %r.folder.display(), "frame recorder: the frame is captured");
        Ok(())
    }
}

/// A part of a resource in a read-back buffer, `at` bytes in.
struct Piece {
    res: Arc<Res>,
    first: u64,
    count: u32,
    len: u64,
    at: u64,
}

/// Records the copies of `pieces` into `to`. Textures are moved to a state
/// they can be copied from and back.
fn copy_out(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    states: &States,
    pieces: &[Piece],
    to: &ID3D12Resource,
) {
    let (mut before, mut after) = (Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    for p in pieces.iter().filter(|p| !p.res.is_buffer()) {
        if !seen.insert(p.res.id) {
            continue;
        }
        let from = states.get(p.res.id).unwrap_or(vec![COMMON]);
        // From the common state a texture is promoted by the copy.
        let to: Vec<u32> = from
            .iter()
            .map(|&s| {
                if s == COMMON || s & COPY_SOURCE != 0 {
                    s
                } else {
                    COPY_SOURCE
                }
            })
            .collect();
        gpu::moves(&p.res.object(), &from, &to, &mut before);
        gpu::moves(&p.res.object(), &to, &from, &mut after);
    }
    // SAFETY: an open list; live resources.
    unsafe {
        if !before.is_empty() {
            list.ResourceBarrier(&before);
        }
        for p in pieces {
            let object = p.res.object();
            if p.res.is_buffer() {
                list.CopyBufferRegion(to, p.at, &*object, p.first, p.len);
                continue;
            }
            let (all, _) =
                gpu::footprints(device, &gpu::to_desc(&p.res.desc), p.first as u32, p.count);
            for (i, mut f) in all.into_iter().enumerate() {
                f.Offset += p.at;
                list.CopyTextureRegion(
                    &gpu::placed_location(to, f),
                    0,
                    0,
                    0,
                    &gpu::sub_location(&object, p.first as u32 + i as u32),
                    None,
                );
            }
        }
        if !after.is_empty() {
            list.ResourceBarrier(&after);
        }
    }
}

/// Records the copy of a texture's subresource, in `state`, into `to`.
pub(super) fn read_sub(
    list: &ID3D12GraphicsCommandList,
    texture: &ID3D12Resource,
    sub: u32,
    state: u32,
    to: &ID3D12Resource,
    f: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) {
    let there = state == COMMON || state & COPY_SOURCE != 0;
    // SAFETY: an open list; live resources.
    unsafe {
        if !there {
            list.ResourceBarrier(&[gpu::transition(texture, ALL, state, COPY_SOURCE)]);
        }
        list.CopyTextureRegion(
            &gpu::placed_location(to, f),
            0,
            0,
            0,
            &gpu::sub_location(texture, sub),
            None,
        );
        if !there {
            list.ResourceBarrier(&[gpu::transition(texture, ALL, COPY_SOURCE, state)]);
        }
    }
}

/// A slot as it goes into the file: a constant buffer view's address as
/// buffer and offset.
fn resolved(r: &Recorder, mut s: Slot) -> Slot {
    let kind = i32::from_le_bytes([s.desc[4], s.desc[5], s.desc[6], s.desc[7]]);
    if s.kind == slot::SRV
        && s.described != 0
        && kind == D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE.0
    {
        r.problem("views of acceleration structures are null in the capture");
        return Slot {
            kind: slot::SRV,
            ..Slot::EMPTY
        };
    }
    if s.kind == slot::CBV && s.described != 0 {
        match r.va(s.address()) {
            Va::At { res, offset } => {
                s.res = res;
                s.set_address(offset);
            }
            Va::Null {} => {}
            Va::Unknown { .. } => {
                // Its buffer is gone: nothing may read it.
                s = Slot {
                    kind: slot::CBV,
                    ..Slot::EMPTY
                };
            }
        }
    }
    s
}

/// The slots as runs of equal ones, empty ones left out.
fn runs_of(r: &Recorder, all: &[Slot]) -> Vec<(u32, u32, Slot)> {
    let mut runs: Vec<(u32, u32, Slot)> = Vec::new();
    for (i, s) in all.iter().enumerate() {
        if s.kind == slot::EMPTY {
            continue;
        }
        match runs.last_mut() {
            Some((first, n, same)) if *first + *n == i as u32 && same == s => *n += 1,
            _ => runs.push((i as u32, 1, *s)),
        }
    }
    for run in &mut runs {
        run.2 = resolved(r, run.2);
    }
    runs
}

/// Whether all bytes are zero.
fn zero(bytes: &[u8]) -> bool {
    // SAFETY: any bytes are a number.
    let (a, b, c) = unsafe { bytes.align_to::<u128>() };
    a.iter().chain(c).all(|&x| x == 0) && b.iter().all(|&x| x == 0)
}

fn write_at(file: &File, mut bytes: &[u8], mut at: u64) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let n = file.seek_write(bytes, at)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[n..];
        at += n as u64;
    }
    Ok(())
}

/// Compares `len` bytes at `live` with those at `kept`, page by page, brings
/// the changed ones in `kept` up to date, and answers their runs.
///
/// # Safety
/// Both are `len` bytes; `kept` is the caller's alone.
unsafe fn changed_runs(live: *const u8, kept: *mut u8, len: usize) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < len {
        let n = PAGE.min(len - at);
        // SAFETY: the caller's.
        unsafe {
            let same = std::slice::from_raw_parts(live.add(at), n)
                == std::slice::from_raw_parts(kept.add(at), n);
            if !same {
                std::ptr::copy_nonoverlapping(live.add(at), kept.add(at), n);
                match runs.last_mut() {
                    Some(last) if last.1 == at => last.1 = at + n,
                    _ => runs.push((at, at + n)),
                }
            }
        }
        at += n;
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_found_by_page() {
        let mut live = vec![0u8; 5 * PAGE + 100];
        let mut kept = live.clone();
        let find = |live: &[u8], kept: &mut [u8]| {
            // SAFETY: two arrays of one length.
            unsafe { changed_runs(live.as_ptr(), kept.as_mut_ptr(), live.len()) }
        };
        assert!(find(&live, &mut kept).is_empty());
        live[PAGE + 5] = 1;
        live[2 * PAGE] = 2;
        live[5 * PAGE + 99] = 3;
        assert_eq!(
            find(&live, &mut kept),
            [(PAGE, 3 * PAGE), (5 * PAGE, 5 * PAGE + 100)]
        );
        assert_eq!(live, kept);
        assert!(find(&live, &mut kept).is_empty());
    }

    #[test]
    fn what_a_submission_reads() {
        let mut reads = Reads::default();
        let part = |from, len| Cmd::CopyBuffer {
            dst: 9,
            dst_at: 0,
            src: 1,
            src_at: from,
            len,
        };
        reads.note(&[
            part(100, 50),
            part(4000, 200),
            part(20_000, 1),
            Cmd::CopyTexture {
                dst: Loc::Sub { res: 9, sub: 0 },
                at: [0; 3],
                src: Loc::Placed {
                    res: 2,
                    offset: 4096,
                    format: 28,
                    size: [16, 16, 1],
                    pitch: 256,
                },
                part: None,
            },
            Cmd::Indirect {
                signature: 1,
                most: 1,
                args: 3,
                at: 8,
                count: 0,
                count_at: 0,
            },
            Cmd::RootView {
                graphics: true,
                kind: slot::CBV,
                param: 0,
                va: Va::At { res: 4, offset: 5 },
            },
        ]);
        assert_eq!(reads.pages(1, 1 << 20, false), [(0, 8192), (16384, 20480)]);
        assert_eq!(
            reads.pages(1, 20_100, false),
            [(0, 8192), (16384, 20_100)],
            "its end"
        );
        assert_eq!(reads.pages(2, 1 << 20, false), [(4096, 8192)]);
        assert_eq!(reads.pages(3, 100, false), [(0, 100)]);
        assert_eq!(reads.pages(4, 100, false), [(0, 100)]);
        assert_eq!(
            reads.pages(5, 100, true),
            [(0, 100)],
            "a descriptor names it"
        );
        assert!(reads.pages(5, 100, false).is_empty() && reads.pages(9, 100, false).is_empty());
    }

    #[test]
    fn zeros() {
        assert!(zero(&[]));
        assert!(zero(&[0; 1001]));
        for i in [0, 7, 500, 1000] {
            let mut b = [0u8; 1001];
            b[i] = 1;
            assert!(!zero(&b));
        }
    }
}
