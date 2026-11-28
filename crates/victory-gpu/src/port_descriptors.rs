//! Renderer port #2: the title's writes of descriptor records into its
//! shader-visible heap say which slots they wrote, and the per-frame look at
//! every changed heap page goes (`VICTORY_PORT_DESCRIPTORS=1`, off by
//! default; read in [`on`]).
//!
//! What the title does (game_scarlett_master.exe; `victory-loader`'s
//! `descriptor_sites` has the code): its placed shader-visible view heap is
//! the memory of its frame allocator (image +0x4d9f400), which hands out the
//! draws' and dispatches' descriptor tables and much else (constants, list
//! nodes). The tables are filled one 32-byte record at a time by six stores
//! in two functions (+0x4d9efd0, +0x4eec8e0); views made by our calls
//! (`Create*View`, `CopyDescriptors*`) are mirrored where they are made
//! (`descriptor::mirror`). What the layer did before: the write watch's
//! changed 64 KiB pages of the heap, every slot on them read and compared
//! with the one last seen (~290 pages, ~600,000 slots, 15-16 ms a frame of
//! the heap-sync thread, 5-7 of it asking the watch, run port1-3).
//!
//! What is done here: the six stores are patched (`descriptor_sites`) to set
//! a byte per slot and per 128 slots in maps of ours ([`MARKS`]), and our own
//! mirroring marks too; `descriptor::sync_heaps` takes the marked slots
//! ([`take`]) in place of the changed pages, and compares and makes only
//! those, as before.
//!
//! Correctness first, the fallback: once a frame a few of the pages the
//! write watch says were written ([`check_budget`], every
//! [`check_every`] frames asked) are looked at whole, as before; a slot found
//! with a new view record that no mark told of is made (so it is right from
//! then on) and counted, with the place in the title's code that last
//! faulted on its 64 KiB block when the guards saw one ([`faulted`]): a writer
//! the patches miss shows in the log. Records that are not views (the
//! allocator's other data) are made as before and counted apart.
//!
//! Assumption: a slot whose view record the title overwrote with other data
//! (not a view) and no mark told of keeps showing its old view on the host
//! until the fallback looks at its page; nothing reads such a slot as a
//! descriptor (the title wrote no descriptor there).

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
    *adopted = Some(Adopted { start, count, slots, chunks, writers });
    tracing::info!(
        start = format_args!("{start:#x}"),
        count,
        stores = IMAGE[2].load(Relaxed),
        "renderer port 2: the title's view heap's slots are told by its record stores and our calls, not looked for on its pages (VICTORY_PORT_DESCRIPTORS)"
    );
    true
}

/// The heap at `start` goes: nothing more is marked for it.
pub(crate) fn let_go(start: u64) {
    let mut adopted = ADOPTED.write().unwrap_or_else(|e| e.into_inner());
    if adopted.as_ref().is_some_and(|a| a.start == start) {
        MARKS.len.store(0, Release);
        *adopted = None;
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
pub(crate) fn mark(handle: u64) {
    let len = MARKS.len.load(Acquire);
    if len == 0 {
        return;
    }
    let at = handle.wrapping_sub(MARKS.start.load(Relaxed));
    if at >= len {
        return;
    }
    let slot = (at / 32) as usize;
    // SAFETY: the maps are leaked, as long as the length says.
    unsafe {
        (*MARKS.slots.load(Relaxed).add(slot)).store(1, Release);
        (*MARKS.chunks.load(Relaxed).add(slot / CHUNK)).store(1, Release);
    }
    MARKED_BY_US.fetch_add(1, Relaxed);
}

/// Whether slot `index`'s mark is set (told, not yet taken).
fn marked(a: &Adopted, index: u32) -> bool {
    a.slots.get(index as usize).is_some_and(|s| s.load(Acquire) != 0)
}

/// The marked slots of the heap at `start`, each mark cleared, as runs of
/// slots `(first, count)`; `None` if it is not the marked heap.
pub(crate) fn take(start: u64) -> Option<Vec<(u32, u32)>> {
    let began = std::time::Instant::now();
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
    *N.get_or_init(|| std::env::var("VICTORY_PORT_DESCRIPTORS_EVERY").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(8u64).max(1))
}

/// Pages the fallback looks at a frame (`VICTORY_PORT_DESCRIPTORS_CHECK=<pages>`,
/// 16 if not given; 0: none).
fn check_budget() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("VICTORY_PORT_DESCRIPTORS_CHECK").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(16usize))
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

/// Whether to ask the write watch of the marked heap this frame.
pub(crate) fn ask_due(frame: u64) -> bool {
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

/// The pages to look at whole this frame (once a frame; none after).
pub(crate) fn to_check(frame: u64) -> Vec<usize> {
    let mut f = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
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

/// Builds disjoint `(from, to, check)` ranges for `descriptor::sync_heaps`
/// from the sorted, disjoint mark runs `(first, count)` and the fallback
/// `pages`, trimming or dropping any marked run that overlaps a fallback page.
/// Also returns the sorted, disjoint slot runs `(first, count)` that were
/// taken by [`take`] and fall inside a fallback page so [`missed`] does not
/// count them as unreported.
pub(crate) fn plan_parts(
    start: u64,
    end: u64,
    step: u64,
    mirrored_count: u32,
    runs: &[(u32, u32)],
    pages: &[usize],
    page_size: usize,
) -> (Vec<(usize, usize, bool)>, Vec<(u32, u32)>) {
    let start_usize = start as usize;
    let end_usize = end as usize;
    let step_usize = step as usize;
    let mirrored = u64::from(mirrored_count);

    let mut check_parts: Vec<(usize, usize)> = Vec::with_capacity(pages.len());
    let mut check_slots: Vec<(u64, u64)> = Vec::with_capacity(pages.len());
    for &page in pages {
        let p_from = page.max(start_usize);
        let p_to = page.saturating_add(page_size).min(end_usize);
        if p_from >= p_to {
            continue;
        }
        check_parts.push((p_from, p_to));
        let c_first = ((p_from - start_usize) as u64).div_ceil(step);
        let c_end = ((p_to - start_usize) as u64 / step).min(mirrored);
        if c_first < c_end {
            check_slots.push((c_first, c_end));
        }
    }
    check_slots.sort_unstable_by_key(|&(first, _)| first);
    let mut merged_checks: Vec<(u64, u64)> = Vec::with_capacity(check_slots.len());
    for (c_first, c_end) in check_slots {
        match merged_checks.last_mut() {
            Some((_, prev_end)) if c_first <= *prev_end => {
                *prev_end = (*prev_end).max(c_end);
            }
            _ => merged_checks.push((c_first, c_end)),
        }
    }

    let mut parts: Vec<(usize, usize, bool)> = Vec::with_capacity(runs.len() + check_parts.len());
    let mut taken_in_check: Vec<(u32, u32)> = Vec::new();
    let mut ci = 0usize;
    for &(first, n) in runs {
        if n == 0 {
            continue;
        }
        let mut cur = u64::from(first);
        let r_end = cur + u64::from(n);
        while ci < merged_checks.len() && merged_checks[ci].1 <= cur {
            ci += 1;
        }
        while ci < merged_checks.len() && merged_checks[ci].0 < r_end {
            let (c_first, c_end) = merged_checks[ci];
            if cur < c_first {
                let len = c_first - cur;
                let from = start_usize + cur as usize * step_usize;
                parts.push((from, from + len as usize * step_usize, false));
            }
            let ov_start = cur.max(c_first);
            let ov_end = r_end.min(c_end);
            if ov_start < ov_end {
                let ov_first = ov_start as u32;
                let ov_len = (ov_end - ov_start) as u32;
                match taken_in_check.last_mut() {
                    Some((prev_first, prev_len)) if u64::from(*prev_first) + u64::from(*prev_len) == ov_start => {
                        *prev_len += ov_len;
                    }
                    _ => taken_in_check.push((ov_first, ov_len)),
                }
            }
            cur = ov_end;
            if c_end <= r_end {
                ci += 1;
            } else {
                break;
            }
        }
        if cur < r_end {
            let len = r_end - cur;
            let from = start_usize + cur as usize * step_usize;
            parts.push((from, from + len as usize * step_usize, false));
        }
    }
    for (p_from, p_to) in check_parts {
        parts.push((p_from, p_to, true));
    }
    (parts, taken_in_check)
}

/// Whether `index` lies in `taken_in_check` (sorted, disjoint `(first, count)` runs).
pub(crate) fn was_taken_in_check(taken_in_check: &[(u32, u32)], index: u32) -> bool {
    let i = taken_in_check.partition_point(|&(first, _)| first <= index);
    i > 0 && {
        let (first, count) = taken_in_check[i - 1];
        u64::from(index) < u64::from(first) + u64::from(count)
    }
}

/// Whether the fallback warning for the `n`-th (0-based) missed view record
/// since the start should be logged.
#[inline]
fn should_warn_missed(n: u64) -> bool {
    n < 20 || n.is_power_of_two()
}

/// The fallback found slot `index` (at `handle`) with a new record: a view
/// record that no mark told of is counted and said.
pub(crate) fn missed(index: u32, handle: u64, view: bool, tabled: bool) {
    if !view {
        CHECKED[2].fetch_add(1, Relaxed);
        return;
    }
    let adopted = ADOPTED.read().unwrap_or_else(|e| e.into_inner());
    let Some(a) = adopted.as_ref() else { return };
    if marked(a, index) {
        // Told, not taken yet.
        CHECKED[3].fetch_add(1, Relaxed);
        return;
    }
    CHECKED[1].fetch_add(1, Relaxed);
    if tabled {
        // A draw read it through a table lately: the draw makes it anyway.
        CHECKED[5].fetch_add(1, Relaxed);
    }
    let n = MISSED_ALL.fetch_add(1, Relaxed);
    let rip = a.writers.get(((handle - a.start) >> 16) as usize).map_or(0, |w| w.load(Relaxed));
    let (base, size) = (IMAGE[0].load(Relaxed), IMAGE[1].load(Relaxed));
    let writer = if rip >= base && rip < base + size { format!("title+{:#x}", rip - base) } else if rip != 0 { format!("{rip:#x} (not the title's)") } else { "not known (no fault on its block)".into() };
    {
        let mut by = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
        *by.entry(writer.clone()).or_default() += 1;
    }
    if should_warn_missed(n) {
        tracing::warn!(
            slot = index,
            handle = format_args!("{handle:#x}"),
            %writer,
            "renderer port 2: a view record in the title's heap that no store told of (found by the fallback; made now)"
        );
    }
}

/// The runtime's own `memcpy` and `memmove` ([`import`]).
static REAL: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

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

unsafe extern "system-unwind" fn shift(to: *mut u8, from: *const u8, n: usize) -> *mut u8 {
    // SAFETY: as above.
    unsafe { copied(to, from, n, REAL[1].load(Relaxed)) }
}

/// # Safety
/// `real` copies as `memcpy` does; the rest are the title's.
#[inline]
unsafe fn copied(to: *mut u8, from: *const u8, n: usize, real: usize) -> *mut u8 {
    // SAFETY: the host's function, noted by `import` before the title ran.
    let real: Copy = unsafe { std::mem::transmute(real) };
    // SAFETY: by the caller.
    let out = unsafe { real(to, from, n) };
    let len = MARKS.len.load(Acquire);
    if len != 0 && n >= 4 {
        let start = MARKS.start.load(Relaxed);
        let (lo, hi) = ((to as u64).max(start), (to as u64 + n as u64).min(start + len));
        if lo < hi {
            COPIES[0].fetch_add(1, Relaxed);
            // Each slot whose first word now lies in the copy and is a view's.
            let mut slot = (lo - start).div_ceil(32);
            let mut marked = 0;
            while start + slot * 32 + 4 <= hi {
                let at = start + slot * 32;
                // SAFETY: just written by the copy, inside the heap.
                if unsafe { (at as *const u32).read_unaligned() } == crate::descriptor::VIEW_TAG {
                    mark_slot(slot as usize);
                    marked += 1;
                }
                slot += 1;
            }
            COPIES[1].fetch_add(marked, Relaxed);
        }
    }
    out
}

/// Marks slot `slot` of the marked heap (in it: checked by the caller).
#[inline]
fn mark_slot(slot: usize) {
    // SAFETY: the maps are leaked, as long as the length says.
    unsafe {
        (*MARKS.slots.load(Relaxed).add(slot)).store(1, Release);
        (*MARKS.chunks.load(Relaxed).add(slot / CHUNK)).store(1, Release);
    }
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
    let [taken, takings, ns] = [0, 1, 2].map(|i| TAKEN[i].swap(0, Relaxed));
    let ours = MARKED_BY_US.swap(0, Relaxed);
    let [pages, missed, other, pending, check_ns, tabled] = [0, 1, 2, 3, 4, 5].map(|i| CHECKED[i].swap(0, Relaxed));
    let copies = [0, 1].map(|i| COPIES[i].swap(0, Relaxed));
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
        "renderer port 2 (VICTORY_PORT_DESCRIPTORS=1) per frame: {:.0} slots told (of them {:.0} marked by our calls, the rest by the title's {} patched stores) in {:.1} takings, {:.3} ms taking; \
         of the title's memcpy/memmove calls into the heap {:.0}, {:.0} records they marked; fallback: {:.1} pages looked at whole, {:.2} view records no store told of ({:.2} of them of draws' tables), {:.0} other records changed, {:.1} told and not taken yet, {:.2} ms; \
         since the start: {} slots told ({} by our calls), {} pages looked at, {} view records missed; their writers: {}",
        taken as f64 / frames,
        ours as f64 / frames,
        IMAGE[2].load(Relaxed),
        takings as f64 / frames,
        ns as f64 / 1e6 / frames,
        copies[0] as f64 / frames,
        copies[1] as f64 / frames,
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

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    /// Slots marked as the stores mark them, and by our calls, come out once
    /// each, as runs; outside the heap nothing is marked.
    #[test]
    fn marked_slots_are_taken_once_as_runs() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn plan_parts_keeps_marked_runs_and_check_pages_disjoint() {
        let start = 0x2000_0000u64;
        let step = 32u64;
        let page_size = 65536usize; // 2048 slots per page
        let count = 8192u32;
        let end = start + u64::from(count) * step;
        let start_usize = start as usize;

        // Pass fallback pages in wrap-around order (page 2, then page 1).
        let pages = [start_usize + 2 * page_size, start_usize + page_size];
        let runs = [
            (100, 10),   // wholly before page 1
            (2040, 15),  // straddles start of page 1 (2040..2048 kept, 2048..2055 in page 1)
            (2060, 10),  // wholly inside page 1
            (4090, 12),  // straddles page 1 / page 2 boundary (4090..4102 wholly inside merged checks)
            (6140, 10),  // straddles end of page 2 (6140..6144 in page 2, 6144..6150 kept)
        ];

        let (parts, taken_in_check) = plan_parts(start, end, step, count, &runs, &pages, page_size);

        let false_parts: Vec<(usize, usize)> = parts.iter().filter(|&&(_, _, check)| !check).map(|&(f, t, _)| (f, t)).collect();
        let true_parts: Vec<(usize, usize)> = parts.iter().filter(|&&(_, _, check)| check).map(|&(f, t, _)| (f, t)).collect();

        assert_eq!(
            false_parts,
            vec![
                (start_usize + 100 * 32, start_usize + 110 * 32),
                (start_usize + 2040 * 32, start_usize + 2048 * 32),
                (start_usize + 6144 * 32, start_usize + 6150 * 32),
            ]
        );
        assert_eq!(
            true_parts,
            vec![
                (start_usize + 2 * page_size, start_usize + 3 * page_size),
                (start_usize + page_size, start_usize + 2 * page_size),
            ]
        );

        // Verify all parts are pairwise disjoint.
        let mut all_ranges: Vec<(usize, usize)> = parts.iter().map(|&(f, t, _)| (f, t)).collect();
        all_ranges.sort_unstable();
        for w in all_ranges.windows(2) {
            assert!(w[0].1 <= w[1].0, "overlapping parts: {:?} and {:?}", w[0], w[1]);
        }

        // Verify taken_in_check covers exact overlapping slots.
        assert_eq!(taken_in_check, vec![(2048, 7), (2060, 10), (4090, 12), (6140, 4)]);
        assert!(!was_taken_in_check(&taken_in_check, 2047));
        assert!(was_taken_in_check(&taken_in_check, 2048));
        assert!(was_taken_in_check(&taken_in_check, 2054));
        assert!(!was_taken_in_check(&taken_in_check, 2055));
        assert!(was_taken_in_check(&taken_in_check, 4096));
        assert!(was_taken_in_check(&taken_in_check, 6143));
        assert!(!was_taken_in_check(&taken_in_check, 6144));
    }

    #[test]
    fn missed_counts_tabled_after_marked_and_keeps_global_warn_counter() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let start = 0x3000_0000u64;
        assert!(adopt_now(start, 256));
        for c in &CHECKED {
            c.store(0, Relaxed);
        }
        MISSED_ALL.store(0, Relaxed);
        WRITERS.lock().unwrap().clear();

        // Slot 5 is marked: pending (CHECKED[3]) increments, tabled (CHECKED[5]) and missed (CHECKED[1], MISSED_ALL) do not.
        mark(start + 5 * 32);
        missed(5, start + 5 * 32, true, true);
        assert_eq!(CHECKED[3].load(Relaxed), 1);
        assert_eq!(CHECKED[1].load(Relaxed), 0);
        assert_eq!(CHECKED[5].load(Relaxed), 0);
        assert_eq!(MISSED_ALL.load(Relaxed), 0);

        // Unmarked tabled view record increments both missed and tabled, plus MISSED_ALL.
        missed(6, start + 6 * 32, true, true);
        assert_eq!(CHECKED[1].load(Relaxed), 1);
        assert_eq!(CHECKED[5].load(Relaxed), 1);
        assert_eq!(MISSED_ALL.load(Relaxed), 1);

        // Even after line() resets CHECKED[1], the global warn sequence continues past 20 without resetting.
        for i in 7..28 {
            missed(i, start + u64::from(i) * 32, true, false);
        }
        CHECKED[1].store(0, Relaxed);
        let n_before = MISSED_ALL.load(Relaxed);
        assert_eq!(n_before, 22);
        assert!(!should_warn_missed(n_before));
        missed(28, start + 28 * 32, true, false);
        assert_eq!(CHECKED[1].load(Relaxed), 1);
        assert_eq!(MISSED_ALL.load(Relaxed), 23);
        assert!(should_warn_missed(32));
        assert!(!should_warn_missed(33));

        let_go(start);
        WRITERS.lock().unwrap().clear();
    }
}
