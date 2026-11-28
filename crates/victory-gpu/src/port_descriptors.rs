use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};

/// Whether renderer port #2 is on.
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // Read once, here: the switch of renderer port #2.
    *ON.get_or_init(|| std::env::var("VICTORY_PORT_DESCRIPTORS").is_ok_and(|v| v.trim() == "1"))
}

/// What the patched stores read (`victory_loader::descriptor_sites`,
/// `MARKS_LEN` bytes): the heap's first byte, its length (0: none), the map
/// of slots, the map of chunks of [`CHUNK`] slots.
#[repr(C)]
pub struct Marks {
    start: AtomicU64,
    len: AtomicU64,
    slots: AtomicPtr<AtomicU8>,
    chunks: AtomicPtr<AtomicU8>,
}

const _: () = assert!(size_of::<Marks>() == 32);

pub(crate) mod events;

pub static MARKS: Marks = Marks {
    start: AtomicU64::new(0),
    len: AtomicU64::new(0),
    slots: AtomicPtr::new(std::ptr::null_mut()),
    chunks: AtomicPtr::new(std::ptr::null_mut()),
};

/// Slots a byte of the map of chunks stands for (4 KiB of records).
const CHUNK: usize = 128;

/// Where the stores' cells are, for `descriptor_sites::apply`.
pub fn marks_address() -> usize {
    std::ptr::from_ref(&MARKS) as usize
}

/// The title's image (base, size) and how many of its stores were patched.
static IMAGE: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// Told by the launcher: `patched` of the title's record stores in the image
/// at `base` (`size` bytes) mark their slots.
pub fn hooked(base: usize, size: usize, patched: usize) {
    IMAGE[0].store(base, Relaxed);
    IMAGE[1].store(size, Relaxed);
    IMAGE[2].store(patched, Relaxed);
}

/// Whether [`take`] stands for the heap's changed pages: the switch is on and
/// the stores were patched.
fn working() -> bool {
    on() && IMAGE[2].load(Relaxed) > 0
}

/// The heap whose slots are marked: its slots, and the places in the title's
/// code that last faulted on each of its 64 KiB blocks.
struct Adopted {
    start: u64,
    count: usize,
    slots: &'static [AtomicU8],
    chunks: &'static [AtomicU8],
    writers: &'static [AtomicUsize],
    spans: std::sync::Mutex<std::collections::BTreeMap<u32, u32>>,
}

static ADOPTED: std::sync::RwLock<Option<Adopted>> = std::sync::RwLock::new(None);

/// A placed shader-visible view heap of `count` records at `start`: the
/// first such is the one whose slots are marked. Whether it is.
pub(crate) fn adopt(start: u64, count: u32) -> bool {
    working() && adopt_now(start, count)
}

fn adopt_now(start: u64, count: u32) -> bool {
    let mut adopted = ADOPTED.write().unwrap_or_else(|e| e.into_inner());
    if adopted.is_some() {
        tracing::info!(start = format_args!("{start:#x}"), count, "renderer port 2: another placed view heap, looked at by its pages as before");
        return false;
    }
    let count = count as usize;
    let leak = |n: usize| -> &'static [AtomicU8] { Box::leak((0..n).map(|_| AtomicU8::new(0)).collect()) };
    // (Never freed: a store of the title's may be marking while the heap
    // goes; a few MB at most.)
    let slots = leak(count);
    let chunks = leak(count.div_ceil(CHUNK));
    let writers: &'static [AtomicUsize] = Box::leak((0..(count * 32).div_ceil(1 << 16)).map(|_| AtomicUsize::new(0)).collect());
    MARKS.slots.store(slots.as_ptr().cast_mut(), Release);
    MARKS.chunks.store(chunks.as_ptr().cast_mut(), Release);
    MARKS.start.store(start, Release);
    // Last: a store that sees a length sees the rest.
    MARKS.len.store(count as u64 * 32, Release);
    *adopted = Some(Adopted { start, count, slots, chunks, writers, spans: Default::default() });
    tracing::info!(
        start = format_args!("{start:#x}"),
        count,
        stores = IMAGE[2].load(Relaxed),
        "renderer port 2: the title's view heap's slots are told by its record stores and our calls, not looked for on its pages (VICTORY_PORT_DESCRIPTORS)"
    );
    drop(adopted);
    events::attach(start as usize, count * 32);
    true
}

/// The heap at `start` goes: nothing more is marked for it.
pub(crate) fn let_go(start: u64) {
    let mut adopted = ADOPTED.write().unwrap_or_else(|e| e.into_inner());
    if adopted.as_ref().is_some_and(|a| a.start == start) {
        events::detach(start as usize);
        MARKS.len.store(0, Release);
        *adopted = None;
        FALLBACK.lock().unwrap_or_else(|e| e.into_inner()).pages.clear();
        tracing::info!(start = format_args!("{start:#x}"), "renderer port 2: the marked view heap went");
    }
}

/// Whether the heap at `start` is the one whose slots are marked.
pub(crate) fn adopted(start: u64) -> bool {
    ADOPTED.read().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|a| a.start == start)
}

/// Marks the slot at `handle` (a record our own calls wrote), if it is in the
/// marked heap.
#[inline]
pub(crate) fn mark(handle: u64) -> bool {
    let len = MARKS.len.load(Acquire);
    if len == 0 {
        return false;
    }
    let at = handle.wrapping_sub(MARKS.start.load(Relaxed));
    if at >= len {
        return false;
    }
    let slot = (at / 32) as usize;
    // SAFETY: the maps are leaked, as long as the length says.
    unsafe {
        let chunk = &*MARKS.chunks.load(Relaxed).add(slot / CHUNK);
        (*MARKS.slots.load(Relaxed).add(slot)).store(1, Release);
        if chunk.load(Relaxed) == 0 {
            chunk.store(1, Release);
        }
    }
    thread_local! {
        static OURS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }
    OURS.with(|ours| {
        let n = ours.get() + 1;
        if n == 256 {
            MARKED_BY_US.fetch_add(256, Relaxed);
            ours.set(0);
        } else {
            ours.set(n);
        }
    });
    true
}

/// Whether slot `index`'s mark is set (told, not yet taken).
fn marked(a: &Adopted, index: u32) -> bool {
    a.slots.get(index as usize).is_some_and(|s| s.load(Acquire) != 0)
}

/// The marked slots of the heap at `start`, each mark cleared, as runs of
/// slots `(first, count)`; `None` if it is not the marked heap.
pub(crate) fn take(start: u64) -> Option<Vec<(u32, u32)>> {
    let began = crate::clock::Instant::now();
    let adopted = ADOPTED.read().unwrap_or_else(|e| e.into_inner());
    let a = adopted.as_ref().filter(|a| a.start == start)?;
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut n = 0u64;
    for (c, chunk) in a.chunks.iter().enumerate() {
        if chunk.load(Relaxed) == 0 {
            continue;
        }
        // Cleared before its slots are read: a mark set meanwhile sets it again.
        chunk.swap(0, AcqRel);
        let first = c * CHUNK;
        for (i, slot) in a.slots[first..(first + CHUNK).min(a.count)].iter().enumerate() {
            if slot.load(Relaxed) == 0 || slot.swap(0, AcqRel) == 0 {
                continue;
            }
            let index = (first + i) as u32;
            n += 1;
            match runs.last_mut() {
                Some((at, len)) if *at + *len == index => *len += 1,
                _ => runs.push((index, 1)),
            }
        }
    }
    let spans = std::mem::take(&mut *a.spans.lock().unwrap_or_else(|e| e.into_inner()));
    if !spans.is_empty() {
        runs.extend(spans.into_iter().map(|(first, end)| (first, end - first)));
        runs.sort_unstable();
        runs.dedup_by(|next, prev| {
            if next.0 > prev.0 + prev.1 { return false; }
            prev.1 = prev.1.max(next.0 + next.1 - prev.0);
            true
        });
        n = runs.iter().map(|&(_, n)| u64::from(n)).sum();
    }
    TAKEN[0].fetch_add(n, Relaxed);
    TAKEN[1].fetch_add(1, Relaxed);
    TAKEN[2].fetch_add(began.elapsed().as_nanos() as u64, Relaxed);
    Some(runs)
}

/// A write fault of the title's at `at` (from its instruction at `rip`),
/// if in the marked heap: who wrote its block last, for [`missed`].
pub(crate) fn faulted(at: usize, rip: usize) {
    if MARKS.len.load(Relaxed) == 0 {
        return;
    }
    let Ok(adopted) = ADOPTED.try_read() else { return };
    if let Some(a) = adopted.as_ref() {
        let off = (at as u64).wrapping_sub(a.start);
        if let Some(w) = a.writers.get((off >> 16) as usize) {
            w.store(rip, Relaxed);
        }
    }
}

/// Frames between askings of the write watch for the fallback
/// (`VICTORY_PORT_DESCRIPTORS_EVERY=<frames>`, 8 if not given).
fn check_every() -> u64 {
    static N: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *N.get_or_init(|| every_of(std::env::var("VICTORY_PORT_DESCRIPTORS_EVERY").ok().as_deref()))
}

/// Pages the fallback looks at a frame (`VICTORY_PORT_DESCRIPTORS_CHECK=<pages>`,
/// 16 if not given; `all`: every written page at every sync; 0: none).
fn check_budget() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| budget_of(std::env::var("VICTORY_PORT_DESCRIPTORS_CHECK").ok().as_deref()))
}

/// What `VICTORY_PORT_DESCRIPTORS_CHECK` says: 16 pages a frame unless told
/// otherwise; `all` (with `VICTORY_PORT_DESCRIPTORS_EVERY=1`): every page the
/// write watch says was written, at every sync, as the layer looked before the
/// port. With 16 pages, 110-250 view records a frame on them had no mark
/// (run pop-1; 70-175 with `VICTORY_PORT_COPIES=1`, pop-4), so slots of
/// textures picked by index can show an older view until the fallback comes
/// round. Looking at every page made them right but cost 15-25 ms a frame
/// in the boat scenes, and the trees and palms that come and go flickered
/// just the same (runs pop-5, pop-6: POPIN.md), so it is not the default.
fn budget_of(spec: Option<&str>) -> usize {
    match spec.map(str::trim) {
        None => 16,
        Some("all") => usize::MAX,
        Some(v) => v.parse().unwrap_or(16),
    }
}

/// What `VICTORY_PORT_DESCRIPTORS_EVERY` says (unset: 8).
fn every_of(spec: Option<&str>) -> u64 {
    spec.and_then(|v| v.trim().parse().ok()).unwrap_or(8u64).max(1)
}

/// The fallback's pages: those the watch said were written, not looked at yet;
/// the frame it last asked in, and the frame it last looked in.
struct Fallback {
    pages: std::collections::BTreeSet<usize>,
    asked: u64,
    looked: u64,
    /// The page it goes on from (round the heap).
    next: usize,
}

static FALLBACK: std::sync::Mutex<Fallback> = std::sync::Mutex::new(Fallback { pages: std::collections::BTreeSet::new(), asked: 0, looked: 0, next: 0 });

/// Whether the fallback is whole: every written page, at every sync of the
/// heaps (several a frame), as the layer looked before the port.
fn whole() -> bool {
    check_budget() == usize::MAX && check_every() == 1
}

/// Whether to ask the write watch of the marked heap now (whole: at every
/// sync; else once in [`check_every`] frames).
pub(crate) fn ask_due(frame: u64) -> bool {
    if whole() {
        return true;
    }
    let mut f = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if check_budget() == 0 || f.asked != 0 && frame < f.asked + check_every() {
        return false;
    }
    f.asked = frame.max(1);
    true
}

/// What the watch said was written (byte ranges), for later.
pub(crate) fn asked(ranges: &[(usize, usize)]) {
    let mut f = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    for &(from, to) in ranges {
        let mut page = from & !(crate::watch::PAGE - 1);
        while page < to {
            f.pages.insert(page);
            page += crate::watch::PAGE;
        }
    }
}

pub(crate) fn take_pending() -> Vec<usize> {
    let mut f = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut f.pages).into_iter().collect()
}

pub(crate) fn early_audit() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PORT_DESCRIPTORS_CHECK").is_some())
}

/// The pages to look at whole now (whole: all those asked of, at every
/// sync; else once a frame, none after).
pub(crate) fn to_check(frame: u64) -> Vec<usize> {
    let mut f = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if whole() {
        let out: Vec<usize> = std::mem::take(&mut f.pages).into_iter().collect();
        CHECKED[0].fetch_add(out.len() as u64, Relaxed);
        return out;
    }
    if f.looked == frame {
        return Vec::new();
    }
    f.looked = frame;
    let mut out = Vec::new();
    while out.len() < check_budget() {
        let from = f.next;
        let Some(&page) = f.pages.range(from..).next().or_else(|| f.pages.iter().next()) else {
            break;
        };
        f.pages.remove(&page);
        f.next = page + crate::watch::PAGE;
        out.push(page);
    }
    CHECKED[0].fetch_add(out.len() as u64, Relaxed);
    out
}

/// The fallback found slot `index` (at `handle`) with a new record: a view
/// record that no mark told of is counted and said.
pub(crate) fn missed(index: u32, handle: u64, view: bool, tabled: bool) {
    if !view {
        CHECKED[2].fetch_add(1, Relaxed);
        return;
    }
    if tabled {
        // A draw read it through a table lately: the draw makes it anyway.
        CHECKED[5].fetch_add(1, Relaxed);
    }
    let adopted = ADOPTED.read().unwrap_or_else(|e| e.into_inner());
    let Some(a) = adopted.as_ref() else { return };
    if marked(a, index) {
        // Told, not taken yet.
        CHECKED[3].fetch_add(1, Relaxed);
        return;
    }
    CHECKED[1].fetch_add(1, Relaxed);
    if !(MISSED_ALL.fetch_add(1, Relaxed) + 1).is_power_of_two() {
        return;
    }
    let rip = a.writers.get(((handle - a.start) >> 16) as usize).map_or(0, |w| w.load(Relaxed));
    let (base, size) = (IMAGE[0].load(Relaxed), IMAGE[1].load(Relaxed));
    let writer = if rip >= base && rip < base + size { format!("title+{:#x}", rip - base) } else if rip != 0 { format!("{rip:#x} (not the title's)") } else { "not known (no fault on its block)".into() };
    {
        let mut by = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
        *by.entry(writer.clone()).or_default() += 1;
    }
    tracing::debug!(slot = index, %writer, "descriptor recovered before submission");
}

/// The runtime's own `memcpy` and `memmove` ([`import`]).
static REAL: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// Since the last line: the title's copies into the marked heap, records they marked.
static COPIES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// What the title's import of `name` (bound to `address`) is bound to: with
/// the switch, its `memcpy` and `memmove` mark the view records they copy
/// into the heap (the fallback of run port2-2 found ~250 view records a
/// frame on 16 pages that the six stores had not told of, the last writers
/// of their blocks the runtime's `memcpy` among them).
pub fn import(name: &str, address: usize) -> usize {
    let (which, ours) = match name {
        "memcpy" => (0, copy as *const () as usize),
        "memmove" => (1, shift as *const () as usize),
        "memset" => (2, fill as *const () as usize),
        _ => return address,
    };
    if !on() {
        return address;
    }
    REAL[which].store(address, Relaxed);
    ours
}

type Copy = unsafe extern "system-unwind" fn(*mut u8, *const u8, usize) -> *mut u8;

unsafe extern "system-unwind" fn copy(to: *mut u8, from: *const u8, n: usize) -> *mut u8 {
    // SAFETY: the title's copy, by the function it asked for.
    unsafe { copied(to, from, n, REAL[0].load(Relaxed)) }
}

unsafe extern "system-unwind" fn fill(to: *mut u8, byte: i32, len: usize) -> *mut u8 {
    if crate::watch::descriptor_put(to as usize, len, |dst| unsafe { dst.write_bytes(byte as u8, len) }) { return to; }
    let real: unsafe extern "system-unwind" fn(*mut u8, i32, usize) -> *mut u8 = unsafe { std::mem::transmute(REAL[2].load(Relaxed)) };
    let result = unsafe { real(to, byte, len) };
    mark_range(to as u64, len as u64);
    result
}

pub(crate) unsafe fn copy_exact(to: *mut u8, from: *const u8, len: usize) -> bool {
    crate::watch::descriptor_put(to as usize, len, |dst| {
        let mut bytes = [std::mem::MaybeUninit::<u8>::uninit(); 256];
        unsafe {
            std::ptr::copy_nonoverlapping(from, bytes.as_mut_ptr().cast(), len);
            std::ptr::copy_nonoverlapping(bytes.as_ptr().cast(), dst, len);
        }
    })
}

unsafe extern "system-unwind" fn shift(to: *mut u8, from: *const u8, n: usize) -> *mut u8 {
    // SAFETY: as above.
    unsafe { copied(to, from, n, REAL[1].load(Relaxed)) }
}

/// # Safety
/// `real` copies as `memcpy` does; the rest are the title's.
#[inline]
unsafe fn copied(to: *mut u8, from: *const u8, n: usize, real: usize) -> *mut u8 {
    // SAFETY: the host's function, noted by `import` before the title ran.
    if unsafe { copy_exact(to, from, n) } { return to; }
    let real: Copy = unsafe { std::mem::transmute(real) };
    // SAFETY: by the caller.
    let out = unsafe { real(to, from, n) };
    let marked = mark_range(to as u64, n as u64);
    if marked != 0 {
        COPIES[0].fetch_add(1, Relaxed);
        COPIES[1].fetch_add(marked, Relaxed);
    }
    out
}

/// The title's own `memcpy`, whose calls in the frame allocator's two table
/// copiers go to [`title_copy`] (`victory_loader::descriptor_sites::apply_copies`).
static TITLE_MEMCPY: AtomicUsize = AtomicUsize::new(0);

/// Since the last line: the title's table copies into the marked heap through
/// its own `memcpy`, and the slots they marked.
static TITLE_COPIES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// The function the title's patched `memcpy` calls go to, with the title's
/// `memcpy` at `real` behind it.
pub fn title_copy_hook(real: usize) -> usize {
    TITLE_MEMCPY.store(real, Relaxed);
    title_copy as *const () as usize
}

/// The title's copy of a descriptor table (or of anything else) by its own
/// `memcpy`: done by it, then every slot of the marked heap the copy wrote a
/// byte of is marked, view record or not (a slot the title made null by a
/// copy must be made again too).
unsafe extern "system-unwind" fn title_copy(to: *mut u8, from: *const u8, n: usize) -> *mut u8 {
    // SAFETY: the title's `memcpy`, noted before the title ran.
    if unsafe { copy_exact(to, from, n) } { return to; }
    let real: Copy = unsafe { std::mem::transmute(TITLE_MEMCPY.load(Relaxed)) };
    // SAFETY: the title's copy, by the function it asked for.
    let out = unsafe { real(to, from, n) };
    let marked = mark_range(to as u64, n as u64);
    if marked > 0 {
        TITLE_COPIES[0].fetch_add(1, Relaxed);
        TITLE_COPIES[1].fetch_add(marked, Relaxed);
    }
    out
}

/// Marks every slot of the marked heap with a byte in `[to, to + n)`: how many.
pub(crate) fn mark_range(to: u64, n: u64) -> u64 {
    let len = MARKS.len.load(Acquire);
    if len == 0 || n == 0 {
        return 0;
    }
    let start = MARKS.start.load(Relaxed);
    let (lo, hi) = (to.max(start), to.saturating_add(n).min(start + len));
    if lo >= hi {
        return 0;
    }
    let adopted = ADOPTED.read().unwrap_or_else(|e| e.into_inner());
    let Some(a) = adopted.as_ref().filter(|a| a.start == start) else { return 0 };
    let first = ((lo - start) / 32) as usize;
    let end = ((hi - start).div_ceil(32) as usize).min(a.count);
    if first >= end { return 0; }
    if end - first >= CHUNK {
        let (mut lo, mut hi) = (first as u32, end as u32);
        let mut spans = a.spans.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((&start, &stop)) = spans.range(..=lo).next_back() && stop >= lo {
            if stop >= hi { return (end - first) as u64; }
            spans.remove(&start);
            lo = start;
        }
        while let Some((&start, &stop)) = spans.range(lo..=hi).next() {
            spans.remove(&start);
            hi = hi.max(stop);
        }
        spans.insert(lo, hi);
        return (end - first) as u64;
    }
    for chunk in first / CHUNK..=(end - 1) / CHUNK {
        let lo = (chunk * CHUNK).max(first);
        let hi = ((chunk + 1) * CHUNK).min(end);
        for slot in &a.slots[lo..hi] { slot.store(1, Release); }
        a.chunks[chunk].store(1, Release);
    }
    (end - first) as u64
}

/// Our own calls' marks since the last line.
static MARKED_BY_US: AtomicU64 = AtomicU64::new(0);
/// Since the last line: slots taken, takings, nanoseconds taking.
static TAKEN: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
/// Since the last line, of the fallback: pages looked at whole, view records
/// no mark told of, other records changed (not views), view records told and
/// not taken yet; nanoseconds.
/// and of the view records no mark told of, those of draws' tables.
pub(crate) static CHECKED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
/// Since the start: view records the fallback found that no mark told of.
static MISSED_ALL: AtomicU64 = AtomicU64::new(0);
/// Since the start: the missed records by the place that last faulted on their block.
static WRITERS: std::sync::Mutex<std::collections::BTreeMap<String, u64>> = std::sync::Mutex::new(std::collections::BTreeMap::new());
/// Since the start: slots taken, of them marked by our calls; pages checked.
static ALL: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// With the frame line: what port #2 did per frame over `frames`.
pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    events::line(frames);
    let [taken, takings, ns] = [0, 1, 2].map(|i| TAKEN[i].swap(0, Relaxed));
    let ours = MARKED_BY_US.swap(0, Relaxed);
    let [pages, missed, other, pending, check_ns, tabled] = [0, 1, 2, 3, 4, 5].map(|i| CHECKED[i].swap(0, Relaxed));
    let copies = [0, 1].map(|i| COPIES[i].swap(0, Relaxed));
    let title_copies = [0, 1].map(|i| TITLE_COPIES[i].swap(0, Relaxed));
    ALL[0].fetch_add(taken, Relaxed);
    ALL[1].fetch_add(ours, Relaxed);
    ALL[2].fetch_add(pages, Relaxed);
    if !working() {
        tracing::info!("renderer port 2 (VICTORY_PORT_DESCRIPTORS=1): not working, no store of the title's was patched");
        return;
    }
    let writers = {
        let by = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<(&String, &u64)> = by.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        v.iter().take(6).map(|(w, n)| format!("{w} {n}")).collect::<Vec<_>>().join(", ")
    };
    tracing::info!(
        "renderer port 2 (VICTORY_PORT_DESCRIPTORS=1) per frame: {:.0} slots told ({:.0} individual host marks, {} patched stores active) in {:.1} takings, {:.3} ms taking; \
         of the title's memcpy/memmove calls into the heap {:.0}, {:.0} records they marked; its own memcpy's table copies {:.0}, {:.0} slots they marked; fallback: {:.1} pages looked at whole, {:.2} view records no store told of ({:.2} of them of draws' tables), {:.0} other records changed, {:.1} told and not taken yet, {:.2} ms; \
         since the start: {} slots told ({} by our calls), {} pages checked; optional audit: {} unmarked views recovered, sampled writers: {}",
        taken as f64 / frames,
        ours as f64 / frames,
        IMAGE[2].load(Relaxed),
        takings as f64 / frames,
        ns as f64 / 1e6 / frames,
        copies[0] as f64 / frames,
        copies[1] as f64 / frames,
        title_copies[0] as f64 / frames,
        title_copies[1] as f64 / frames,
        pages as f64 / frames,
        missed as f64 / frames,
        tabled as f64 / frames,
        other as f64 / frames,
        pending as f64 / frames,
        check_ns as f64 / 1e6 / frames,
        ALL[0].load(Relaxed),
        ALL[1].load(Relaxed),
        ALL[2].load(Relaxed),
        MISSED_ALL.load(Relaxed),
        if writers.is_empty() { "none".into() } else { writers }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marked heap is one for the process: its tests one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn marks_layout_is_what_the_stores_read() {
        assert_eq!(size_of::<Marks>(), victory_loader_marks_len());
        assert_eq!(std::mem::offset_of!(Marks, start), 0);
        assert_eq!(std::mem::offset_of!(Marks, len), 8);
        assert_eq!(std::mem::offset_of!(Marks, slots), 16);
        assert_eq!(std::mem::offset_of!(Marks, chunks), 24);
    }

    /// `victory_loader::descriptor_sites::MARKS_LEN` (no dependency on the loader).
    fn victory_loader_marks_len() -> usize {
        32
    }

    /// A copy marks the slots it wrote a byte of in the marked heap, and
    /// nothing outside it.
    #[test]
    fn a_copy_marks_the_slots_it_wrote() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let start = 0x2000_0000u64;
        assert!(adopt_now(start, 1000));
        assert_eq!(mark_range(start + 32 * 10 + 16, 64), 3); // slots 10, 11, 12
        assert_eq!(mark_range(start - 64, 64), 0);
        assert_eq!(mark_range(start - 16, 32), 1); // slot 0
        assert_eq!(mark_range(start + 32 * 999, 4096), 1); // slot 999, the last
        assert_eq!(mark_range(start + 32 * 20, 0), 0);
        assert_eq!(take(start).unwrap(), [(0, 1), (10, 3), (999, 1)]);
        let_go(start);
        assert_eq!(mark_range(start, 32), 0);
    }

    /// Unset, the fallback keeps the port's budget (16 pages, the watch
    /// asked every 8 frames); `all` is every written page.
    #[test]
    fn fallback_keeps_the_ports_budget_unless_told_otherwise() {
        assert_eq!(budget_of(None), 16);
        assert_eq!(budget_of(Some("all")), usize::MAX);
        assert_eq!(budget_of(Some(" 32 ")), 32);
        assert_eq!(budget_of(Some("0")), 0);
        assert_eq!(budget_of(Some("x")), 16);
        assert_eq!(every_of(None), 8);
        assert_eq!(every_of(Some("1")), 1);
        assert_eq!(every_of(Some("0")), 1);
    }

    /// Slots marked as the stores mark them, and by our calls, come out once
    /// each, as runs; outside the heap nothing is marked.
    #[test]
    fn marked_slots_are_taken_once_as_runs() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let start = 0x1000_0000u64;
        assert!(adopt_now(start, 1000));
        assert!(adopted(start) && !adopted(start + 32));
        let store = |slot: usize| {
            let a = ADOPTED.read().unwrap();
            let a = a.as_ref().unwrap();
            a.slots[slot].store(1, Release);
            a.chunks[slot / CHUNK].store(1, Release);
        };
        store(5);
        mark(start + 6 * 32 + 7);
        mark(start + 300 * 32);
        store(999);
        mark(start - 32);
        mark(start + 1000 * 32);
        assert_eq!(take(start).unwrap(), [(5, 2), (300, 1), (999, 1)]);
        assert_eq!(take(start).unwrap(), []);
        assert!(take(start + 32).is_none());
        let_go(start);
        mark(start + 5 * 32);
        assert!(take(start).is_none());
        assert_eq!(MARKS.len.load(Relaxed), 0);
    }
}
