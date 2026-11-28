use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

use crate::descriptor::host::HostHeap;
use crate::descriptor::{Heap, VIEW_TAG, View};

pub(crate) const SLOTS: u32 = 1 << 20;
pub(crate) const RESERVED: u32 = 2 * victory_shader::emit::RECORDS_CHUNKS + 1;
pub(crate) const BUFFER: u32 = 1 << 31;
pub(crate) const UNCOVERED: u32 = 1 << 30;
pub(crate) const SLOT: u32 = victory_shader::emit::RECORD_SLOTS;
const ROW: usize = 16;
const ROWS: usize = (victory_shader::emit::RECORDS_ENTRIES + victory_shader::emit::RECORDS_SHAPES) as usize;
const SHARDS: usize = 64;
const AUDIT: u32 = 8192;
const LONG: u32 = 64;
const BUSY: u32 = 1 << 30;
const SIDE: usize = 64 << 20;
const MOST: u64 = 64 << 10;

type Shard = Mutex<HashMap<u64, u32, std::hash::BuildHasherDefault<crate::host::AddressHash>>>;

pub(crate) struct Canon {
    heap: &'static Heap,
    pub(crate) host: &'static HostHeap,
    start: u64,
    keys: Box<[AtomicU64]>,
    refs: Box<[AtomicU32]>,
    named: Box<[AtomicU32]>,
    shards: Box<[Shard]>,
    free: Mutex<Vec<u32>>,
    next: AtomicU32,
    cursor: AtomicU32,
}

unsafe impl Send for Canon {}
unsafe impl Sync for Canon {}

static STATS: [AtomicU64; 22] = [const { AtomicU64::new(0) }; 22];
static STEP: AtomicU32 = AtomicU32::new(1);
static REST: AtomicU32 = AtomicU32::new(u32::MAX);

fn step() -> u32 {
    STEP.load(Relaxed)
}
const MADE: usize = 0;
const FREED: usize = 1;
const TOLD: usize = 2;
const MOVED: usize = 3;
const STALE: usize = 4;
const RESTED: usize = 16;
pub(crate) const SCANNED: usize = 5;
pub(crate) const SLOW: usize = 6;
const COPIED: usize = 7;
const COPIED_BYTES: usize = 8;
const LEFT: usize = 9;
const STOOD: usize = 10;
const FULL: usize = 11;
pub(crate) const FRESH: usize = 12;
pub(crate) const WRITTEN: usize = 13;
pub(crate) const AGAIN: usize = 14;
pub(crate) const FIRST: usize = 15;
pub(crate) const UNPLANNED: usize = 17;
pub(crate) const APART: usize = 20;
const UNMADE: usize = 21;

pub(crate) fn count(what: usize, n: u64) {
    STATS[what].fetch_add(n, Relaxed);
}

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RECORDS").is_none_or(|v| v != "0") && crate::um::on())
}

pub(crate) fn apart(address: u64, len: u64, format: u32, unordered: bool) {
    if !on() {
        return;
    }
    if STATS[APART].fetch_add(1, Relaxed) < 24 {
        tracing::warn!(address = format_args!("{address:#x}"), len, format, unordered, covered = covered(address, len), "records: a buffer view read by number is made on a copy apart from title memory");
    }
}

pub(crate) fn unmade(view: &View) -> bool {
    if !on() {
        return false;
    }
    let Some((start, len)) = range(view).filter(|&(start, len)| start != 0 && len != 0) else {
        return false;
    };
    static SAID: AtomicU32 = AtomicU32::new(0);
    STATS[UNMADE].fetch_add(1, Relaxed);
    if SAID.fetch_add(1, Relaxed) < 16 {
        tracing::warn!(start = format_args!("{start:#x}"), len, covered = covered(start, len), word = format_args!("{:#x}", crate::descriptor::host::slot_word(view)), "records: a buffer view read by number could not be made yet; it is tried again when it is next named");
    }
    true
}

pub(crate) fn snapshots() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RECORDS_SNAPSHOT").is_some_and(|v| v == "1"))
}

pub(crate) fn canon() -> Option<&'static Canon> {
    static CANON: OnceLock<Option<Canon>> = OnceLock::new();
    if !on() {
        return None;
    }
    let mut fresh = false;
    let canon = CANON
        .get_or_init(|| {
            fresh = true;
            Canon::new()
        })
        .as_ref();
    if fresh {
        match canon {
            Some(c) => {
                READY.store(std::ptr::from_ref(c) as usize, Release);
                for (chunk, buffer, bytes) in crate::um::mirrors() {
                    c.show(chunk, &buffer, bytes);
                }
                match table() {
                    Some(t) => c.host.raw_at(victory_shader::emit::records() + 2 * victory_shader::emit::RECORDS_CHUNKS, &t.buffer, t.offset, (ROWS * ROW) as u64),
                    None => tracing::error!("records: no table of the views' places; buffers read by their records read nothing"),
                }
                tracing::info!(slots = SLOTS, first = victory_shader::emit::records(), "records: the card reads the title's view records and its memory by address");
            }
            None => tracing::error!("records: no heap of views by record; shaders translated for it read nothing"),
        }
    }
    canon
}

static READY: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn mirrored(chunk: usize, buffer: &ID3D12Resource, bytes: u64) {
    let at = READY.load(Acquire);
    if at != 0 {
        unsafe { &*(at as *const Canon) }.show(chunk, buffer, bytes);
    }
}

pub(crate) fn before_send() {
    if on() {
        crate::um::mirror_wanted();
    }
}

fn shard_of(key: u64) -> usize {
    (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58) as usize % SHARDS
}

fn key_of(i: u32, generation: u32) -> u64 {
    u64::from(generation) << 32 | u64::from(i)
}

fn read(handle: u64) -> [u32; 4] {
    unsafe { (handle as *const [u32; 4]).read_volatile() }
}

struct Table {
    buffer: ID3D12Resource,
    offset: u64,
    data: usize,
}

unsafe impl Send for Table {}
unsafe impl Sync for Table {}

fn table() -> Option<&'static Table> {
    static TABLE: OnceLock<Option<Table>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let bytes = ROWS * ROW;
            let wanted = std::env::var_os("VICTORY_RECORDS_CARD").is_none_or(|v| v != "0");
            let card = wanted.then(|| crate::um::own(bytes, true)).flatten().and_then(|at| {
                let (buffer, offset) = crate::um::mirror_at(at as u64, bytes as u64)?;
                Some(Table { buffer, offset, data: at })
            });
            let table = match card {
                Some(table) => table,
                None => {
                    let (buffer, data) = crate::host::host()?.upload_buffer(bytes as u64)?;
                    crate::host::name(&buffer, || "where the views of title memory lie, by record".to_owned());
                    Table { buffer, offset: 0, data: data as usize }
                }
            };
            unsafe { std::ptr::write_bytes(table.data as *mut u8, 0, bytes) };
            tracing::info!(
                at = format_args!("{:#x}", table.data),
                mb = bytes >> 20,
                card = crate::um::dedicated_vram(table.data, bytes),
                "records: the table of the views' places"
            );
            Some(table)
        })
        .as_ref()
}

fn row(at: usize, view: Option<&View>) {
    let Some(t) = table() else {
        return;
    };
    if at >= ROWS {
        return;
    }
    let words = match view.map(|view| (kind(view), view)) {
        Some((Kind::Buffer, view)) => match range(view) {
            Some((start, len)) => {
                crate::um::cpu_pointer(start, len, "raw address in record");
                [start as u32, (start >> 32) as u32, len.min(u64::from(u32::MAX)) as u32, crate::descriptor::host::slot_word(view)]
            },
            None => [0; 4],
        },
        Some((Kind::Structure, view)) => {
            let at = crate::descriptor::host::view_address(view).unwrap_or(0);
            crate::um::cpu_pointer(at, 1, "acceleration structure");
            [at as u32, (at >> 32) as u32, u32::MAX, 0]
        }
        _ => [0; 4],
    };
    let to = (t.data + at * ROW) as *mut u32;
    for (n, word) in words.into_iter().enumerate() {
        unsafe { to.add(n).write_volatile(word) };
    }
}

#[cfg(feature = "diagnostics")]
pub(crate) fn seen(words: &[u32; 4]) -> Option<(u64, u32, u32)> {
    use victory_shader::emit::{RECORDS_ENTRIES, RECORD_TAG};
    let t = table()?;
    let [tag, first, second, _] = *words;
    let kind = second >> 30;
    let row = match kind {
        0 => first.min(RECORDS_ENTRIES - 2),
        2 => RECORDS_ENTRIES + (second >> 8 & 0x3F_FFFF),
        _ => RECORDS_ENTRIES - 1,
    };
    let at = (t.data + row as usize * ROW) as *const [u32; 4];
    let [off_lo, off_hi, table_len, word] = unsafe { at.read_volatile() };
    let at_lo = if kind == 0 { 0 } else { first };
    let at_hi = match kind {
        2 => second & 0xFF,
        1 => second & 0x7FFF,
        _ => 0,
    };
    let (lo, carry) = at_lo.overflowing_add(off_lo);
    let hi = at_hi.wrapping_add(off_hi).wrapping_add(u32::from(carry));
    let code = second >> 15 & 0x7FFF;
    let constant_len = if code < 0x4000 { code } else { (code & 0x3FFF) << 8 };
    let len = if kind == 1 { constant_len } else { table_len };
    let len = if tag == RECORD_TAG { len } else { 0 };
    Some((u64::from(hi) << 32 | u64::from(lo), len, word))
}

#[cfg(feature = "diagnostics")]
pub(crate) fn expected(view: &View) -> Option<(u64, u64, u32, bool, bool)> {
    let (start, len) = range(view)?;
    Some((start, len, crate::descriptor::host::slot_word(view), typed(view), start == 0 || len == 0 || covered(start, len)))
}

pub(crate) fn entry_made(i: u32, view: Option<&View>) {
    if on() && i < victory_shader::emit::RECORDS_ENTRIES - 1 {
        row(i as usize, view);
    }
}

pub(crate) fn shape_made(n: u32, view: &View) {
    if on() {
        row(victory_shader::emit::RECORDS_ENTRIES as usize + n as usize, Some(view));
    }
}

impl Canon {
    fn new() -> Option<Canon> {
        crate::descriptor::host::probe_view_limit();
        if victory_shader::emit::records() == 0 {
            return None;
        }
        let heap = crate::descriptor::canon_heap(SLOTS)?;
        let host = heap.host()?;
        let host: &'static HostHeap = unsafe { &*std::ptr::from_ref(host) };
        let cells = |n: u32| (0..n).map(|_| AtomicU32::new(0)).collect::<Box<[_]>>();
        Some(Canon {
            heap,
            host,
            start: heap.start,
            keys: (0..SLOTS).map(|_| AtomicU64::new(0)).collect(),
            refs: cells(SLOTS),
            named: cells(SLOTS),
            shards: (0..SHARDS).map(|_| Shard::default()).collect(),
            free: Mutex::new(Vec::new()),
            next: AtomicU32::new(1),
            cursor: AtomicU32::new(1),
        })
    }

    fn show(&self, chunk: usize, buffer: &ID3D12Resource, bytes: u64) {
        let first = victory_shader::emit::records();
        let chunk = chunk as u32;
        if first == 0 || chunk >= victory_shader::emit::RECORDS_CHUNKS {
            return;
        }
        self.host.raw_at(first + chunk, buffer, 0, bytes);
        self.host.raw_uav_at(first + victory_shader::emit::RECORDS_CHUNKS + chunk, buffer, 0, bytes);
    }

    pub(crate) fn heap(&self) -> &'static Heap {
        self.heap
    }

    pub(crate) fn handle(&self, slot: u32) -> u64 {
        self.start + u64::from(slot) * 32
    }

    pub(crate) fn holds(&self, slot: u32, key: u64) -> bool {
        slot != 0 && self.keys.get(slot as usize).is_some_and(|k| k.load(Acquire) == key)
    }

    pub(crate) fn touch(&self, slot: u32) {
        let now = step();
        if let Some(named) = self.named.get(slot as usize)
            && named.load(Relaxed) != now
        {
            named.store(now, Relaxed);
        }
    }

    fn take(&self) -> u32 {
        if let Some(slot) = self.free.lock().unwrap_or_else(|e| e.into_inner()).pop() {
            return slot;
        }
        let slot = self.next.fetch_add(1, AcqRel);
        if slot < SLOTS {
            return slot;
        }
        self.next.store(SLOTS, Release);
        0
    }

    pub(crate) fn slot(&self, key: u64, view: &View, now: bool) -> u32 {
        let frame = step();
        let shard = &self.shards[shard_of(key)];
        let slot = {
            let mut map = shard.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(&slot) = map.get(&key) {
                self.named[slot as usize].store(frame, Relaxed);
                return slot;
            }
            let mut slot = self.take();
            if slot == 0 {
                drop(map);
                let rest = REST.load(Relaxed);
                self.reclaim(SLOTS, rest, rest);
                map = shard.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(&slot) = map.get(&key) {
                    return slot;
                }
                slot = self.take();
                if slot == 0 {
                    if STATS[FULL].fetch_add(1, Relaxed) == 0 {
                        tracing::error!("records: every view slot is held; new views read nothing");
                    }
                    return 0;
                }
            }
            self.named[slot as usize].store(frame, Relaxed);
            self.keys[slot as usize].store(key, Release);
            let words = [VIEW_TAG, key as u32, (key >> 32) as u32, slot];
            unsafe { (self.handle(slot) as *mut [u32; 4]).write_volatile(words) };
            map.insert(key, slot);
            slot
        };
        STATS[MADE].fetch_add(1, Relaxed);
        let _ = view;
        if now {
            self.host.mirror(slot, self.handle(slot), true);
        }
        slot
    }

    fn hold(&self, slot: u32, key: u64) -> bool {
        let Some(refs) = self.refs.get(slot as usize) else {
            return false;
        };
        if refs.fetch_add(1, AcqRel) < BUSY && self.keys[slot as usize].load(Acquire) == key {
            return true;
        }
        refs.fetch_sub(1, AcqRel);
        false
    }

    fn release(&self, slot: u32) {
        if let Some(refs) = self.refs.get(slot as usize) {
            self.named[slot as usize].store(step(), Relaxed);
            refs.fetch_sub(1, AcqRel);
        }
    }

    fn reclaim(&self, most: u32, rest: u32, long: u32) {
        let end = self.next.load(Acquire).min(SLOTS);
        if end <= 1 || rest == u32::MAX {
            return;
        }
        let aged = |slot: u32| step().saturating_sub(self.named[slot as usize].load(Relaxed));
        for _ in 0..most.min(end - 1) {
            let mut slot = self.cursor.fetch_add(1, Relaxed);
            if slot >= end {
                self.cursor.store(2, Relaxed);
                slot = 1;
            }
            let key = self.keys[slot as usize].load(Acquire);
            if key == 0 || self.refs[slot as usize].load(Acquire) != 0 || aged(slot) < rest {
                continue;
            }
            if aged(slot) < long {
                if self.host.rests(slot) {
                    continue;
                }
                if let Some(view) = crate::descriptor::view_of(key as u32, (key >> 32) as u32) {
                    self.host.rest(slot);
                    STATS[RESTED].fetch_add(1, Relaxed);
                    if self.refs[slot as usize].load(Acquire) != 0 || aged(slot) < rest {
                        self.host.mirror(slot, self.handle(slot), matches!(kind(&view), Kind::Buffer));
                    }
                    continue;
                }
            }
            {
                let mut map = self.shards[shard_of(key)].lock().unwrap_or_else(|e| e.into_inner());
                if aged(slot) < rest || self.refs[slot as usize].compare_exchange(0, BUSY, AcqRel, Acquire).is_err() {
                    continue;
                }
                map.remove(&key);
                self.keys[slot as usize].store(0, Release);
                unsafe { (self.handle(slot) as *mut [u32; 4]).write_volatile([0; 4]) };
            }
            self.host.mirror(slot, self.handle(slot), false);
            self.refs[slot as usize].store(0, Release);
            self.free.lock().unwrap_or_else(|e| e.into_inner()).push(slot);
            STATS[FREED].fetch_add(1, Relaxed);
        }
    }

    fn long(&self, rest: u32) -> u32 {
        let crowded = self.next.load(Relaxed) > SLOTS / 2 && self.free.lock().unwrap_or_else(|e| e.into_inner()).len() < (SLOTS / 16) as usize;
        if crowded { rest.saturating_mul(LONG) } else { u32::MAX }
    }
}

fn typed(view: &View) -> bool {
    matches!(crate::descriptor::host::view_elements(view), Some((0, false, _)))
}

fn range(view: &View) -> Option<(u64, u64)> {
    match view {
        View::Constant(address, size) => Some((*address, u64::from(*size))),
        View::Shader(r, None) | View::Unordered(r, _, None) => {
            let (data, len) = crate::resource::buffer_bytes(*r as crate::object::This)?;
            Some((data as u64, len))
        }
        _ => crate::descriptor::host::buffer_range(view),
    }
}

fn covered(start: u64, len: u64) -> bool {
    const PAGE: u64 = 1 << 16;
    let len = len.max(1);
    if len <= 64 * PAGE {
        return crate::um::covers(start, len);
    }
    let last = start + len - 1;
    (0..=(last - start) / (16 * PAGE)).all(|step| crate::um::covers(start + step * 16 * PAGE, 1)) && crate::um::covers(last, 1)
}

enum Kind {
    Buffer,
    Structure,
    Texture,
    Other,
}

fn kind(view: &View) -> Kind {
    const STRUCTURE: u32 = windows::Win32::Graphics::Direct3D12::D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE.0 as u32;
    let object = |r: &usize| crate::resource::buffer_bytes(*r as crate::object::This).is_some();
    match view {
        View::Constant(..) => Kind::Buffer,
        View::Placed(_, resource, _) if resource[0] == 1 => Kind::Buffer,
        View::Placed(..) => Kind::Texture,
        View::Shader(_, Some(desc)) if desc[1] == STRUCTURE => Kind::Structure,
        View::Shader(r, _) | View::Unordered(r, _, _) if object(r) => Kind::Buffer,
        View::Shader(..) | View::Unordered(..) => Kind::Texture,
        View::Sampler(_) | View::ConsoleSampler(_) => Kind::Other,
    }
}

pub(crate) fn describe(view: &View, i: u32, generation: u32) -> u32 {
    match kind(view) {
        Kind::Buffer => {
            let mut word = BUFFER;
            if let Some((start, bytes)) = range(view)
                && start != 0
                && bytes != 0
                && !covered(start, bytes)
            {
                word |= UNCOVERED;
            }
            if typed(view)
                && let Some(c) = canon()
            {
                word |= c.slot(key_of(i, generation), view, true);
            }
            word
        }
        Kind::Structure => BUFFER,
        Kind::Texture => canon().map_or(0, |c| c.slot(key_of(i, generation), view, false)),
        Kind::Other => 0,
    }
}

pub(crate) fn viewed(words: &[u32; 4]) -> bool {
    words[0] == VIEW_TAG && (words[3] & BUFFER == 0 || words[3] & SLOT != 0)
}

pub(crate) fn resolve(c: &Canon, handle: u64, words: &[u32; 4]) -> u32 {
    let key = key_of(words[1], words[2]);
    let named = words[3] & SLOT;
    if c.holds(named, key) {
        return named;
    }
    STATS[STALE].fetch_add(1, Relaxed);
    let slot = match crate::descriptor::view_of(words[1], words[2]) {
        Some(view) if !matches!(kind(&view), Kind::Other) => c.slot(key, &view, words[3] & BUFFER != 0),
        _ => 0,
    };
    if slot != named {
        crate::descriptor::put_record_view(handle, words[3] & !SLOT | slot);
    }
    slot
}

pub(crate) fn slot_at(handle: u64) -> Option<u32> {
    let c = canon()?;
    let words = read(handle);
    if !viewed(&words) {
        return None;
    }
    Some(resolve(c, handle, &words)).filter(|&slot| slot != 0)
}

pub(crate) fn place(handle: u64) -> Option<(u64, u64)> {
    range(&crate::descriptor::view(handle)?)
}

pub(crate) fn reached(handle: u64) -> bool {
    place(handle).is_none_or(|(start, len)| start == 0 || len == 0 || covered(start, len))
}

pub(crate) fn told(heap: &Heap, index: u32, handle: u64) {
    if let Some(c) = canon()
        && let Some(cell) = heap.named.get(index as usize)
    {
        told_at(c, cell, handle);
    }
}

fn told_at(c: &Canon, cell: &AtomicU32, handle: u64) {
    let words = read(handle);
    let viewed = viewed(&words);
    if !viewed && cell.load(Relaxed) == 0 {
        return;
    }
    let mut slot = 0;
    if viewed {
        let key = key_of(words[1], words[2]);
        slot = resolve(c, handle, &words);
        if slot == cell.load(Relaxed) {
            c.touch(slot);
            return;
        }
        let mut tries = 0;
        while slot != 0 && !c.hold(slot, key) {
            tries += 1;
            slot = match crate::descriptor::view_of(words[1], words[2]) {
                Some(view) if tries < 4 => c.slot(key, &view, words[3] & BUFFER != 0),
                _ => 0,
            };
            crate::descriptor::put_record_view(handle, words[3] & !SLOT | slot);
        }
    }
    let old = cell.swap(slot, AcqRel);
    STATS[MOVED].fetch_add(1, Relaxed);
    if old != 0 {
        c.release(old);
    }
    if slot != 0 && c.host.quick_key(slot) != key_of(words[1], words[2]) {
        c.host.mirror(slot, c.handle(slot), words[3] & BUFFER != 0);
    }
    if slot != 0 && old != 0 && slot != old && words[3] & BUFFER == 0 && c.host.stand_in(slot, old) {
        STATS[STOOD].fetch_add(1, Relaxed);
    }
}

pub(crate) fn forgotten(i: u32, generation: u32) {
    if !on() {
        return;
    }
    let Some(c) = canon() else {
        return;
    };
    let key = key_of(i, generation);
    let slot = c.shards[shard_of(key)].lock().unwrap_or_else(|e| e.into_inner()).get(&key).copied();
    if let Some(slot) = slot {
        c.host.mirror(slot, c.handle(slot), false);
    }
}

pub(crate) fn round(required: bool) {
    let Some(c) = canon() else {
        return;
    };
    let mut all = 0u64;
    let mut sweep = 0u32;
    crate::descriptor::title_view_heaps(|heap| {
        let (start, count) = (heap.start, heap.count());
        let end = start + u64::from(count) * 32;
        let port = crate::port_descriptors::adopted(start);
        let marks = if port { crate::port_descriptors::take(start).unwrap_or_default() } else { Vec::new() };
        let mut pages = Vec::new();
        let events = port && required && crate::port_descriptors::events::drain(start as usize, &mut pages);
        if events {
            pages.append(&mut heap.retried());
        } else if required || !port {
            pages = heap.changed(start as usize, end as usize);
        }
        let mut runs: Vec<(u32, u32)> = marks;
        if required && count != 0 {
            let first = heap.audited.load(Relaxed) % count;
            let len = AUDIT.min(count - first);
            heap.audited.store((first + len) % count, Relaxed);
            runs.push((first, len));
            sweep += count.div_ceil(AUDIT) + 1;
        }
        for (from, to) in pages {
            let first = ((from as u64).saturating_sub(start) / 32) as u32;
            let last = (((to as u64).min(end).saturating_sub(start)).div_ceil(32) as u32).min(count);
            if first < last {
                runs.push((first, last - first));
            }
        }
        all += runs.iter().map(|&(_, n)| u64::from(n)).sum::<u64>();
        const A_JOB: usize = 64;
        let cells: &[AtomicU32] = &heap.named;
        crate::land::each_view(true, runs.len().div_ceil(A_JOB), &|n| {
            for &(first, len) in runs.chunks(A_JOB).nth(n).unwrap_or_default() {
                for index in first..(first + len).min(count) {
                    if let Some(cell) = cells.get(index as usize) {
                        told_at(c, cell, start + u64::from(index) * 32);
                    }
                }
            }
        });
    });
    STATS[TOLD].fetch_add(all, Relaxed);
    if required && sweep != 0 {
        let rest = 2 * sweep + 2;
        REST.store(rest, Relaxed);
        STEP.fetch_add(1, Relaxed);
        c.reclaim(AUDIT, rest, c.long(rest));
    }
}

struct Side {
    at: usize,
    next: AtomicUsize,
}

fn side() -> Option<&'static Side> {
    static SIDE_SPACE: OnceLock<Option<Side>> = OnceLock::new();
    SIDE_SPACE
        .get_or_init(|| {
            let at = crate::um::own(SIDE, false);
            match at {
                Some(at) => tracing::info!(at = format_args!("{at:#x}"), mb = SIDE >> 20, "records: memory of ours among the title's, for what it keeps outside the card's reach"),
                None => tracing::warn!("records: no memory of ours among the title's; what it keeps outside the card's reach reads nothing"),
            }
            at.map(|at| Side { at, next: AtomicUsize::new(0) })
        })
        .as_ref()
}

pub(crate) fn aside(bytes: usize) -> Option<usize> {
    let side = side()?;
    let bytes = bytes.next_multiple_of(32);
    if bytes > SIDE / 4 {
        return None;
    }
    loop {
        let at = side.next.load(Relaxed);
        let (from, to) = if at + bytes > SIDE { (0, bytes) } else { (at, at + bytes) };
        if side.next.compare_exchange(at, to, AcqRel, Relaxed).is_ok() {
            return Some(side.at + from);
        }
    }
}

pub(crate) fn copy_aside(list: &crate::host::HostList, from: u64, len: u64) -> Option<u64> {
    if len > MOST {
        if STATS[LEFT].fetch_add(1, Relaxed) < 16 {
            tracing::warn!(from = format_args!("{from:#x}"), len, "records: title memory outside the card's reach, too long to copy for a draw; it reads nothing");
        }
        return None;
    }
    let len = len.max(4) as usize;
    let to = aside(len)?;
    STATS[COPIED].fetch_add(1, Relaxed);
    STATS[COPIED_BYTES].fetch_add(len as u64, Relaxed);
    list.defer_host(move || {
        let into = unsafe { std::slice::from_raw_parts_mut(to as *mut u8, len) };
        crate::host::read_there(from as *const u8, into);
    });
    Some(to as u64)
}

pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    let s: Vec<f64> = STATS.iter().map(|s| s.swap(0, Relaxed) as f64 / frames).collect();
    let (held, free) = canon().map_or((0, 0), |c| (c.next.load(Relaxed).min(SLOTS), c.free.lock().unwrap_or_else(|e| e.into_inner()).len()));
    tracing::info!(
        "records per frame: {:.0} title slots told, {:.0} changed what they name, {:.1} named a view slot given away; {:.1} view slots made, {:.1} put to rest, {:.1} freed ({held} taken so far, {free} free again), {:.1} stood in for by what their slot showed; {:.0} tables looked at by draws, {:.0} of their views brought up to date ({:.0} not made yet, {:.0} written by the shader, {:.0} after the list changed, {:.0} first in the list); outside the card's reach: {:.0} copied for a draw ({:.1} KB), {:.0} left; {:.2} buffer views by number made on a copy apart from title memory; {:.0} tables' bases given outside the tables looked at; {:.1} views by number not made for want of their memory",
        s[TOLD], s[MOVED], s[STALE], s[MADE], s[RESTED], s[FREED], s[STOOD], s[SCANNED], s[SLOW], s[FRESH], s[WRITTEN], s[AGAIN], s[FIRST], s[COPIED], s[COPIED_BYTES] / 1024.0, s[LEFT], s[APART], s[UNPLANNED], s[UNMADE]
    );
}
