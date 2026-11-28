//! Steps of a submission that read title memory flights in the air may
//! still write, done on the card in the flights' order instead of on the
//! CPU after a wait for them (`VICTORY_CARD_STEPS`, see [`stage`]).

use std::sync::Arc;

use super::*;
use crate::shadow::Shadow;

/// The stage (`VICTORY_CARD_STEPS`; off by default): `1` constant buffers
/// of descriptor tables (`Step::Fill` with a slot of its own), `2` the
/// title's buffer copies (`Step::Copy`) too, with the steps of the same
/// submission that depend on them. Only with the relay's second stage
/// (`relay_exact`).
///
/// Why: eleven dispatches a frame (the skinning passes, in two early
/// submissions) write through a view of the title's whole 174 MB heap, at
/// places they read out of job records in buffers: no bound of what they
/// write is known before they ran (frame capture replay2: 17af15c4015cb4b6,
/// 55679a0547649d0b, 89d08e561897d4e5, fdd56d97a806bcca and three more,
/// all through one view at offset 5920 of the title's bindless table). The
/// title's ring of constants lies in the same heap. One constant buffer of
/// 480 bytes and one copy of 20 KiB a frame, at a new place of the ring
/// each time, waited for these passes to land: 25-30 ms of a 105 ms frame,
/// the submitting thread standing (run record-1; with `VICTORY_FILL_TITLE=1`
/// in run relaycopy-1 the copy alone took over the wait).
///
/// What is done: a list of ours goes to the card in front of the
/// submission's lists, in the same call. It copies the bytes read from the
/// host copy that the flights write into a buffer of ours, and from there
/// to the destination's host copy (a copy), or the fill's descriptor slot
/// is pointed at that buffer (a fill; the slot is the fill's alone, the
/// root signatures say such descriptors are volatile, and the list that
/// names it has not gone to the card).
///
/// Why that is the value the CPU would have read after the wait. After the
/// wait title memory holds, for every byte read, what the title wrote, or
/// what the last of the flights that wrote it wrote (the landings put it
/// there, in order). The host copy on the card holds the same when our list
/// runs: every flight that may write one of these bytes writes this same
/// host copy ([`holder`]: a write through another host copy, a read-back
/// or a value written behind a flight there, and the old wait is taken),
/// they are all ahead of our list on the queue, and the title's own bytes
/// are brought up to date by a refresh step of our list, which is prepared
/// like any other list's (also under flights' declared writes:
/// `stale_under_flights`, which settles if it cannot). Nothing of this
/// submission writes these bytes earlier than our list: the steps are done
/// in front of all its lists in either case, and among themselves in the
/// order they stand in wherever one depends on another: [`plan`] takes
/// every copy and fill of the submission that writes what a taken one
/// reads or writes, or reads what a taken one writes. Every draw that
/// reads a buffer view of the heap relies on the same refresh and the same
/// order; what is new is only that a constant buffer and a copy do too.
/// Where a page is not mapped the host copy may hold other bytes than the
/// CPU's copy would have left: refused.
///
/// For a copy's destination our list declares a shader write, exactly the
/// bytes copied, after a refresh of them: from there on it is what any
/// shader's write is. The relay compares, and puts what changed into title
/// memory before the title is told anything of this flight (its
/// bottom-of-pipe writes and fences come after, in order); CPU steps of
/// later submissions that read it find it declared and wait, or are done
/// here. Refused: a step of the submission that cannot be taken (a texture,
/// vertices expanded, a fill of memory of another kind) reads the
/// destination behind the copy, or a refresh brings another host copy of it
/// up to date ([`plan`], [`elsewhere`]). While such a copy is in the air a
/// submission that refreshes another host copy of its destination, or
/// reads it in a step that is not checked otherwise, waits for it
/// ([`copied_read`]); with the copy done on the CPU it was there at once.
///
/// What stage 2 does not see: code of ours that reads title memory away
/// from a submission's steps (the pass over textures picked by index, while
/// a list is recorded). With the CPU's copy such a read of the destination
/// found the bytes at once; now it finds them when the flight has landed.
/// The landing tells the guards (`crate::watch::landing`), so a texture
/// made from older bytes is made again. This is why stage 2 is its own.
pub(super) fn stage() -> u32 {
    static STAGE: OnceLock<u32> = OnceLock::new();
    *STAGE.get_or_init(|| match std::env::var("VICTORY_CARD_STEPS").ok().as_deref() {
        Some("1") => 1,
        Some("2") => 2,
        _ => 0,
    })
}

/// `VICTORY_LEARNED_ON_CARD=1` (off by default): fills of learned places
/// ([`late`]) done on the card from the title's memory in place, not waited
/// for (see [`Host::take_steps`]).
fn learned_on_card() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_LEARNED_ON_CARD").is_some_and(|v| v == "1"))
}

/// Whether a copy at its place in a list ([`place`]) is only in the way of
/// what is done on the card in front of the lists if that stands behind it in
/// them (`VICTORY_PLACED_ORDER=1`; off: wherever it stands), and a
/// submission where it does is cut behind the copy's list ([`cut_for`]). Off:
/// the case did not come up once in runs submit-1 to submit-3, so it has only
/// its tests behind it.
///
/// The fills of constant buffers read the title's ring (7,500 a frame in the
/// opening), and one copy of a few hundred bytes a frame writes into it at its
/// place in a list. The whole submission was refused for it, and stood 40 to
/// 60 ms until the flights had landed (run n19).
fn placed_order() -> bool {
    #[cfg(feature = "diagnostics")]
    if let Some(on) = crate::ab::pick("VICTORY_PLACED_ORDER") {
        return on;
    }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_PLACED_ORDER"))
}

/// Why [`plan`] refuses for [`place`].
const BEHIND_PLACED: &str = "a copy at its place in a list writes what it reads, or reads or writes what it writes";

/// The bytes of our buffer between reading and writing: what one submission
/// may do this way (each place read once, however many fills take it).
const BETWEEN: u64 = 4 << 20;

/// The most steps of a submission done this way.
const MOST: usize = 12288;

/// How a fill's bytes get where its list reads them, if the card brings
/// them.
#[derive(Clone)]
pub(super) enum FillTo {
    /// A descriptor slot that shows the fill's memory, and nothing else does
    /// (title heap, slot): it is made to show our buffer instead.
    Slot(usize, u32),
    /// A buffer the card can write, filled from its beginning.
    Buffer(ID3D12Resource),
}

/// Title memory something in the air writes: the host copy it goes through
/// (`None`: none, the bytes reach title memory when the flight lands), from,
/// to, and whether it is the destination of a copy done here.
pub(super) struct Writer {
    shadow: Option<Arc<Shadow>>,
    from: usize,
    to: usize,
    copy: bool,
}

/// What `items` of the line write.
pub(super) fn writers(items: &[Late]) -> Vec<Writer> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Late::Run(flight) => {
                for (shadow, ranges) in &flight.back {
                    out.extend(ranges.iter().map(|&(from, to)| Writer {
                        shadow: Some(shadow.clone()),
                        from,
                        to,
                        copy: false,
                    }));
                }
                for &i in &flight.order {
                    for &(_, at, [row, rows, pitch]) in &flight.lists[i].host().deferred().backs {
                        out.push(Writer {
                            shadow: None,
                            from: at,
                            to: at + rows.saturating_sub(1) * pitch + row,
                            copy: false,
                        });
                    }
                }
                out.extend(flight.texture_back_ranges.iter().map(|&(from, to)| Writer {
                    shadow: None,
                    from,
                    to,
                    copy: false,
                }));
                out.extend(flight.own.iter().flat_map(Taken::copied));
            }
            Late::Write(at, _, bytes) => out.push(Writer {
                shadow: None,
                from: *at,
                to: *at + *bytes,
                copy: false,
            }),
            Late::Signal(..) => {}
        }
    }
    out
}

/// The destinations of copies done here that have not landed.
pub(super) fn copies(line: &Line) -> Vec<Writer> {
    if stage() < 2 {
        return Vec::new();
    }
    let flying = line.line.iter().filter_map(|l| match l {
        Late::Run(flight) => flight.own.as_ref(),
        _ => None,
    });
    let landing = line.landing.iter().filter(|w| w.copy).map(|w| Writer {
        shadow: w.shadow.clone(),
        ..*w
    });
    flying.flat_map(Taken::copied).chain(landing).collect()
}

/// Whether `runs` refresh another host copy of a destination in `copies`,
/// or read it in a step that flights are not waited for otherwise
/// (`Step::Host`): that reads title memory, where the copy is not yet.
pub(super) fn copied_read(copies: &[Writer], runs: &[&HostList]) -> bool {
    if copies.is_empty() {
        return false;
    }
    runs.iter().any(|list| {
        list.deferred().copies.iter().any(|step| match step {
            Step::Refresh(shadow, at, len) => copies
                .iter()
                .any(|w| w.from < *at + *len && *at < w.to && w.shadow.as_ref().is_none_or(|s| !Arc::ptr_eq(s, shadow))),
            Step::Host(_, (from, to)) => copies.iter().any(|w| w.from < *to && *from < w.to),
            _ => false,
        })
    })
}

/// What a step of a submission does to title memory, as the plan sees it.
/// `asked`: flights in the air stop it from being done on the CPU now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Does {
    Copy { dst: usize, src: usize, len: usize, asked: bool },
    /// `to`: its memory can be given the bytes on the card.
    Fill { src: usize, len: usize, asked: bool, to: bool },
    /// Another read on the CPU (a texture, vertices expanded).
    Read { from: usize, to: usize, asked: bool },
    /// A refresh of a host copy (named by its address).
    Refresh { shadow: usize, from: usize, to: usize },
    /// A copy that its list does on the card ([`place`]).
    Placed { dst: usize, src: usize, len: usize },
    /// Anything, anywhere.
    Run,
}

fn meet(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// The bytes `d` reads and writes as a copy or a fill (empty ranges for
/// what it does not).
fn reads_writes(d: &Does) -> Option<((usize, usize), (usize, usize))> {
    match *d {
        Does::Copy { dst, src, len, .. } => Some(((src, src + len), (dst, dst + len))),
        Does::Fill { src, len, .. } => Some(((src, src + len), (0, 0))),
        _ => None,
    }
}

fn touched(ranges: &[(usize, usize)], r: (usize, usize)) -> bool {
    touches(ranges, r.0, r.1)
}

/// The steps of `does` (by place) to be done on the card, in order, each
/// with its level; or why not. All that are asked for, and every copy and
/// fill that is in the way of one that is done there: it writes what that
/// one reads or writes, or reads what it writes, whichever comes first. So
/// what is left on the CPU writes none of their bytes, and reads of them
/// only what a taken copy has not been told to write yet: bytes a step
/// ahead of it reads, in title memory as it is.
///
/// `behind`: for [`BEHIND_PLACED`], the copy at its place and the first step
/// behind it that is in its way.
///
/// Steps of a level are done together, every read before every write; a
/// step that writes what an earlier one reads or writes, or reads what it
/// writes, is of a higher level. Our list does them level by level.
fn plan(does: &[Does], stage: u32, by_order: bool, behind: &mut Option<(usize, usize)>) -> Result<Vec<(usize, u32)>, &'static str> {
    let takes = |d: &Does| match *d {
        Does::Copy { .. } if stage < 2 => Err("a buffer copy (VICTORY_CARD_STEPS=2 does it on the card)"),
        Does::Fill { to: false, .. } => Err("a fill of memory the card cannot be given the bytes of"),
        _ => Ok(()),
    };
    let mut taken = vec![false; does.len()];
    let (mut read, mut written) = (Vec::new(), Vec::new());
    let mut more = Vec::new();
    for (i, d) in does.iter().enumerate() {
        match *d {
            Does::Run => return Err("a step of ours runs"),
            Does::Read { asked: true, .. } => return Err("a texture reads it"),
            Does::Copy { asked: true, .. } | Does::Fill { asked: true, .. } => more.push(i),
            _ => {}
        }
    }
    while !more.is_empty() {
        // (Joined once for the round: one by one the 2,800 fills of a frame
        // sorted the ranges each time, 16 ms of the submitting thread.)
        let (mut reads, mut writes) = (Vec::new(), Vec::new());
        for i in std::mem::take(&mut more) {
            takes(&does[i])?;
            taken[i] = true;
            if let Some((r, w)) = reads_writes(&does[i]) {
                if w.0 < w.1 && meet(r, w) {
                    return Err("a copy onto itself");
                }
                reads.push(r);
                writes.push(w);
            }
        }
        join_ranges(&mut read, reads);
        if writes.iter().any(|w| w.0 < w.1) {
            join_ranges(&mut written, writes);
        }
        if taken.iter().filter(|t| **t).count() > MOST {
            return Err("too many steps depend on it");
        }
        for (i, d) in does.iter().enumerate() {
            // A fill that cannot be taken is looked at below.
            if taken[i] || matches!(d, Does::Fill { to: false, .. }) {
                continue;
            }
            if let Some((r, w)) = reads_writes(d)
                && (touched(&written, r) || touched(&written, w) || touched(&read, w))
            {
                more.push(i);
            }
        }
    }
    // A copy at its place in a list comes after what is done in front of all
    // of them: as it should after the steps ahead of it, but not after those
    // behind it that read or write its bytes.
    for (p, d) in does.iter().enumerate() {
        if let Does::Placed { dst, src, len } = *d
            && (touched(&read, (dst, dst + len)) || touched(&written, (dst, dst + len)) || touched(&written, (src, src + len)))
        {
            let (to, from) = ((dst, dst + len), (src, src + len));
            let in_the_way = |i: usize| {
                reads_writes(&does[i]).is_some_and(|(r, w)| meet(r, to) || w.0 < w.1 && (meet(w, to) || meet(w, from)))
            };
            let found = (p + 1..does.len()).find(|&i| taken[i] && in_the_way(i));
            *behind = found.map(|i| (p, i));
            if !by_order || found.is_some() {
                return Err(BEHIND_PLACED);
            }
        }
    }
    let order: Vec<usize> = (0..does.len()).filter(|&i| taken[i]).collect();
    // What stays on the CPU and reads what an earlier taken copy writes
    // would read it too early.
    let mut behind: Vec<(usize, usize)> = Vec::new();
    for (i, d) in does.iter().enumerate() {
        if taken[i] {
            if let Some((_, w)) = reads_writes(d)
                && w.0 < w.1
            {
                join_ranges(&mut behind, [w]);
            }
            continue;
        }
        match *d {
            Does::Read { from, to, .. } if touched(&behind, (from, to)) => {
                return Err("a texture or expanded vertices read what it writes");
            }
            Does::Fill { src, len, .. } if touched(&behind, (src, src + len)) => {
                return Err("a fill of memory the card cannot be given the bytes of reads what it writes");
            }
            _ => {}
        }
    }
    let mut out: Vec<(usize, u32)> = Vec::with_capacity(order.len());
    // The copies among them, which alone put others behind them or have to
    // stand behind others.
    let mut copies: Vec<usize> = Vec::new();
    for &i in &order {
        let Some((r, w)) = reads_writes(&does[i]) else {
            continue;
        };
        let writes = w.0 < w.1;
        let mut level = 0;
        // Behind what writes what this reads or writes, and, if this
        // writes, behind what reads it.
        let ahead: Box<dyn Iterator<Item = usize>> =
            if writes { Box::new(0..out.len()) } else { Box::new(copies.iter().copied()) };
        for k in ahead {
            let (j, at): (usize, u32) = out[k];
            let Some((jr, jw)) = reads_writes(&does[j]) else {
                continue;
            };
            if at >= level && (meet(jw, r) || meet(jw, w) || meet(jr, w)) {
                level = at + 1;
            }
        }
        if writes {
            copies.push(out.len());
        }
        out.push((i, level));
    }
    Ok(out)
}

/// Whether a refresh of `does` brings a host copy other than `shadow` up to
/// date in `[from, to)`.
fn elsewhere(does: &[Does], shadow: usize, from: usize, to: usize) -> bool {
    does.iter().any(|d| match *d {
        Does::Refresh { shadow: s, from: a, to: b } => s != shadow && meet((a, b), (from, to)),
        _ => false,
    })
}

/// The host copy that holds `[from, to)` on the card once what `writers`
/// write is there: the one they all write it through, or without any the
/// one shaders would be given.
fn holder(writers: &[Writer], from: usize, to: usize) -> Result<Arc<Shadow>, &'static str> {
    let mut found: Option<&Arc<Shadow>> = None;
    for w in writers.iter().filter(|w| meet((w.from, w.to), (from, to))) {
        match (&w.shadow, found) {
            (None, _) => return Err("a read-back or a value behind a flight is written there"),
            (Some(s), Some(f)) if !Arc::ptr_eq(s, f) => return Err("flights write it through two host copies"),
            (Some(s), _) => found = Some(s),
        }
    }
    let shadow = match found {
        Some(s) => s.clone(),
        None => crate::shadow::find(from as u64, (to - from) as u64).ok_or("no host copy of it")?,
    };
    let (start, len) = shadow.span();
    if shadow.in_place() || (from as u64) < start || to as u64 > start + len {
        return Err("no host copy holds all of it");
    }
    Ok(shadow)
}

/// What the steps of `runs` do, and which step each is (its list by address,
/// its place in it). `written`: what flights may write.
fn does_of(written: &[(usize, usize)], runs: &[&HostList]) -> (Vec<Does>, Vec<(usize, usize)>) {
    let mut does = Vec::new();
    // Which step each is. One list's lock at a time: a submission may
    // name a list twice.
    let mut steps = Vec::new();
    for list in runs {
        for (i, step) in list.deferred().copies.iter().enumerate() {
            does.push(match step {
                &Step::Copy(dst, src, len) => Does::Copy {
                    dst,
                    src,
                    len,
                    asked: copy_blocked(written, dst, src, len).is_some(),
                },
                Step::Fill(_, src, len, to) => Does::Fill {
                    src: *src,
                    len: *len,
                    asked: read_blocked(written, *src, *len),
                    to: to.is_some(),
                },
                Step::Texture(staging) => {
                    let (at, len) = staging.memory();
                    Does::Read {
                        from: at,
                        to: at + len,
                        asked: touches(written, at, at + len),
                    }
                }
                &Step::Host(_, (from, to)) if from < to => Does::Read { from, to, asked: false },
                Step::Refresh(shadow, at, len) => Does::Refresh {
                    shadow: Arc::as_ptr(shadow) as usize,
                    from: *at,
                    to: *at + *len,
                },
                Step::Run(_) => Does::Run,
                Step::Placed(p) => Does::Placed { dst: p.dst, src: p.src, len: p.len },
                Step::GpuWrite(..) | Step::Host(..) => continue,
            });
            steps.push((std::ptr::from_ref::<HostList>(list) as usize, i));
        }
    }
    (does, steps)
}

/// With flights in the air: before which of `runs` the submission is cut so
/// that what reads or writes the bytes of a copy at its place in an earlier
/// list ([`BEHIND_PLACED`]) is done on the card in front of a submission of
/// its own, behind that copy.
pub(super) fn cut_for(runs: &[&HostList]) -> Option<usize> {
    if stage() == 0 || !relay_exact() || !placed_order() || runs.len() < 2 {
        return None;
    }
    // (Most submissions have none.)
    let first = runs.iter().position(|l| l.deferred().copies.iter().any(|s| matches!(s, Step::Placed(_))))?;
    if first + 1 == runs.len() {
        return None;
    }
    let written = line().written.clone();
    blocked_by(&written, runs, false)?;
    let (does, steps) = does_of(&written, runs);
    let mut behind = None;
    if plan(&does, stage(), true, &mut behind) != Err(BEHIND_PLACED) {
        return None;
    }
    let (placed, reader) = behind?;
    let place = |i: usize| runs.iter().position(|l| std::ptr::from_ref::<HostList>(l) as usize == steps[i].0);
    let cut = place(placed)? + 1;
    (place(reader)? >= cut).then_some(cut)
}

/// Whether every page of `[from, to)` can be read (through the kernel:
/// asking Windows walks the page tables, milliseconds in the title's heap).
/// `known`: pages that could be, of the same submission: its fills' places
/// lie eight to a page, and each was read whole to find out.
fn mapped(from: usize, to: usize, known: &mut HashMap<usize, ()>) -> bool {
    const PAGE: usize = 4096;
    (from / PAGE..to.div_ceil(PAGE)).all(|page| {
        known.contains_key(&page) || {
            let there = read_into((page * PAGE).max(from) as *const u8, &mut [0]);
            if there {
                known.insert(page, ());
            }
            there
        }
    })
}

/// A table by addresses and numbers of ours.
type HashMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<AddressHash>>;

/// Where the bytes of the fill that is `step` (its list by address, its place
/// in it) go.
fn fill_to(runs: &[&HostList], step: (usize, usize)) -> Option<FillTo> {
    let list = runs.iter().find(|l| std::ptr::from_ref::<HostList>(l) as usize == step.0)?;
    match list.deferred().copies.get(step.1)? {
        Step::Fill(_, _, _, to) => to.clone(),
        _ => None,
    }
}

/// A step done on the card.
struct Op {
    from: (Arc<Shadow>, usize),
    len: usize,
    /// Its place in the buffer between.
    at: u64,
    level: u32,
    to: To,
}

enum To {
    /// A host copy, and the title address.
    Copy(Arc<Shadow>, usize),
    /// A descriptor slot (title heap, slot).
    Slot(usize, u32),
    /// A buffer's beginning.
    Buffer(ID3D12Resource),
}

/// Records a level of what [`stage`] describes into `list` (open): `reads`
/// are copied into `between` (source, offset in it, offset in `between`,
/// bytes), then `writes` out of it (destination, offset in it, offset in
/// `between`, bytes). `between` is left readable as constants (and must be
/// so, unless this is the `first` level), the others in the common state,
/// as the submission's lists expect them.
fn record(
    first: bool,
    list: &ID3D12GraphicsCommandList,
    between: &ID3D12Resource,
    reads: &[(&ID3D12Resource, u64, u64, u64)],
    writes: &[(&ID3D12Resource, u64, u64, u64)],
) {
    // Every range is checked against its buffer and the staging buffer before
    // it is recorded: vkd3d-proton does not validate CopyBufferRegion, and on
    // RADV a range past a buffer's end is a write by the card's command
    // processor to an address of no allocation — a GPU page fault and, when the
    // queue reset fails, a whole-GPU reset that takes the desktop with it
    // (2026-11-26: CPC write at 0xfffffffff000 on vkd3d's compute queue).
    let fits = |&&(buffer, offset, at, len): &&(&ID3D12Resource, u64, u64, u64)| {
        // SAFETY: a plain query of a live buffer.
        let width = unsafe { buffer.GetDesc() }.Width;
        let ok = len != 0
            && offset.checked_add(len).is_some_and(|end| end <= width)
            && at.checked_add(len).is_some_and(|end| end <= BETWEEN);
        if !ok {
            static SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 16 || n.is_power_of_two() {
                tracing::warn!(offset, at, len, width, skipped = n + 1, "card step: a copy out of its buffer's range left out");
            }
        }
        ok
    };
    let reads: Vec<_> = reads.iter().filter(fits).copied().collect();
    let writes: Vec<_> = writes.iter().filter(fits).copied().collect();
    let (reads, writes) = (&reads[..], &writes[..]);
    let common = D3D12_RESOURCE_STATE_COMMON;
    let (source, dest) = (D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
    let mut sources: Vec<&ID3D12Resource> = Vec::new();
    let mut dests: Vec<&ID3D12Resource> = Vec::new();
    for (all, list) in [(reads, &mut sources), (writes, &mut dests)] {
        for &(buffer, ..) in all {
            if !list.iter().any(|b| b.as_raw() == buffer.as_raw()) {
                list.push(buffer);
            }
        }
    }
    let both = |b: &ID3D12Resource| dests.iter().any(|d| d.as_raw() == b.as_raw());
    let constants = source | D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER;
    let mut barriers = vec![transition(between, dest, constants)];
    barriers.extend(sources.iter().map(|b| transition(b, source, if both(b) { dest } else { common })));
    let done: Vec<D3D12_RESOURCE_BARRIER> = dests.iter().map(|b| transition(b, dest, common)).collect();
    // SAFETY: an open list; live buffers, and the callers' ranges lie in
    // them. The buffers start in the common state (the list is the first of
    // its call) and are promoted by their first use.
    unsafe {
        if first {
            // What earlier lists' shaders wrote, first.
            list.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                        pResource: std::mem::ManuallyDrop::new(None),
                    }),
                },
            }]);
        } else {
            list.ResourceBarrier(&[transition(between, constants, dest)]);
        }
        for &(buffer, offset, at, len) in reads {
            list.CopyBufferRegion(between, at, buffer, offset, len);
        }
        list.ResourceBarrier(&barriers);
        for &(buffer, offset, at, len) in writes {
            list.CopyBufferRegion(buffer, offset, between, at, len);
        }
        if !done.is_empty() {
            list.ResourceBarrier(&done);
        }
    }
}

/// The list in front of a submission's, and its buffer ([`BETWEEN`] bytes).
struct Own {
    list: HostList,
    between: ID3D12Resource,
    /// What the buffer between held, read back ([`late`]).
    back: Option<ID3D12Resource>,
}

// SAFETY: D3D12 objects are free-threaded, and the list is touched by one
// thread at a time (the submitting one until it is sent, then the one that
// lands it, then nobody until it is taken out of `POOL`).
unsafe impl Send for Own {}

/// Those that have landed: closed, their steps dropped.
static POOL: Mutex<Vec<Own>> = Mutex::new(Vec::new());

/// Since the last frame-rate line: fills and copies done on the card, their
/// bytes, and submissions that waited instead, refused by a check.
static DONE: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// What [`Host::card_steps`] took off a submission: the list that does it,
/// for the submission to run first, and the steps the CPU leaves out (their
/// list by address, their place in it).
pub(super) struct Taken {
    own: Own,
    steps: std::collections::HashSet<(usize, usize), std::hash::BuildHasherDefault<AddressHash>>,
    copied: Vec<(Arc<Shadow>, usize, usize)>,
    /// The fills' places, lengths and places in the buffer between
    /// ([`late`]).
    sent: Vec<(usize, usize, u64)>,
    /// The fills of the submission done on the CPU: digest of title bytes at
    /// submission, place, length ([`late`]). Hashing from cached title RAM
    /// avoids reading back from uncached write-combined (WC) upload memory.
    cpu: Vec<(u128, usize, usize)>,
    /// The slots it fills, if they are written later ([`Host::card_steps`]).
    slots: Option<Slots>,
}

/// The descriptor slots a submission's steps on the card fill: each is
/// made a view of the bytes the list copies into the buffer between
/// ([`Own`]) for them (heap, index, address, bytes).
pub(super) struct Slots {
    ops: Vec<(usize, u32, u64, u32)>,
    between: ID3D12Resource,
    parts: usize,
}

// SAFETY: D3D12 objects are free-threaded; the slots are written by one
// thread, once.
unsafe impl Send for Slots {}

impl Slots {
    /// (11,000 slots a frame in a street, two descriptors each: on several
    /// threads, a slot always on the same one, in the steps' order.) By the
    /// sender (`later`) on the threads of the heaps' records, which it has
    /// just waited for, not on those the line's thread puts title bytes
    /// under flights' writes with.
    pub(super) fn write(self, later: bool) {
        let Slots { ops, between, parts } = self;
        let each: fn(usize, &(dyn Fn(usize) + Sync)) =
            if later { |n, f| crate::land::each_view(true, n, f) } else { crate::land::each_under };
        each(parts, &|part| {
            for &(heap, index, gpu, len) in &ops {
                if index as usize / 4 % parts == part {
                    // (A heap that is gone: nothing can read the slot.)
                    crate::descriptor::constant_slot(heap, index, &between, gpu, len);
                }
            }
        });
    }
}

/// Places the title writes after the submission that reads them was sent.
///
/// Without card steps a submission behind flights waits for them, and all its
/// fills read title memory after that; on the console the card reads a
/// constant buffer when the draw runs. Taken, the submission is sent at once:
/// the fills done on the card take their bytes then, and so do the ones left
/// to the CPU, which no flight writes and are done while it is prepared. The
/// title counts on the later moment: some constant buffers (at the boat five
/// places of 240 bytes to 32 KiB read on the card, and two of 32 and 544
/// bytes read on the CPU, the same ones frame after frame) are written again
/// by its CPU between sending and landing, and the draws get the bytes of
/// before: the skinned characters and the boat throw vertices off, a few times
/// a second, more the longer a cutscene runs and the more the camera moves
/// (`VICTORY_CARD_STEPS=1` and `2`). A check that read back what the card took
/// found every such fill's bytes changed by no write of the relay, and none
/// changed while the card's host copy was behind; with fills on the card
/// learned and those on the CPU not, the stretching stayed.
///
/// So what each fill of a taken submission read (on the card: read back from
/// the buffer between; on the CPU: the upload memory it filled) is compared
/// with title memory when the submission has landed (the title has not been
/// told: what it wrote meanwhile it wrote before the card was to read it).
/// Pages where they differ are learned, and a submission with any fill there
/// waits as without card steps. The first time is drawn with the old bytes;
/// from then on it is exact. At the boat that is about every submission card
/// steps took: 88 ms a frame against 94 without the check, and 95 without card
/// steps (the frame waits on the card; runs at one place, within their
/// spread). `VICTORY_CARD_STEPS_LEARN=0`: not.
mod late {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    const PAGE: usize = 4096;

    static PAGES: Mutex<Option<std::collections::HashSet<usize>>> = Mutex::new(None);
    /// Fills done on the card compared, found written again, submissions
    /// refused for a learned page; pages learned in all; fills done on the CPU
    /// compared, found written again.
    pub(super) static COUNTS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

    pub(in crate::host) fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("VICTORY_CARD_STEPS_LEARN").is_none_or(|v| v != "0"))
    }

    /// Whether the title was found writing a page of `[from, to)` late.
    pub(super) fn written(from: usize, to: usize) -> bool {
        let mut pages = PAGES.lock().unwrap_or_else(|e| e.into_inner());
        forget(&mut pages);
        pages.as_ref().is_some_and(|p| (from / PAGE..to.div_ceil(PAGE)).any(|page| p.contains(&page)))
    }

    /// `VICTORY_CARD_STEPS_UNLEARN_S=<s>` (off without it): what was learned
    /// is forgotten every `s` seconds, so a page the title wrote late once
    /// (all of them were learned in one burst while the world loaded, runs
    /// c-o-x and c-base-x, 2026-11-27) makes its fills wait only while the
    /// title still does; one that it writes late again is learned again at the
    /// next landing (one submission with the bytes of before, as the first
    /// time). Logged each time with how many pages were forgotten.
    fn forget(pages: &mut Option<std::collections::HashSet<usize>>) {
        static PERIOD: OnceLock<Option<std::time::Duration>> = OnceLock::new();
        let Some(period) = *PERIOD.get_or_init(|| {
            std::env::var("VICTORY_CARD_STEPS_UNLEARN_S")
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|s| *s > 0.0)
                .map(std::time::Duration::from_secs_f64)
        }) else {
            return;
        };
        static SINCE: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        let mut since = SINCE.lock().unwrap_or_else(|e| e.into_inner());
        let start = *since.get_or_insert_with(std::time::Instant::now);
        if start.elapsed() < period {
            return;
        }
        *since = Some(std::time::Instant::now());
        if let Some(p) = pages.as_mut().filter(|p| !p.is_empty()) {
            tracing::info!(pages = p.len(), "card steps: learned pages forgotten (VICTORY_CARD_STEPS_UNLEARN_S)");
            p.clear();
        }
    }

    /// The submission has landed: what the fills of `sent` (place, length,
    /// place in the buffer between) read, in `back`, compared with title
    /// memory now.
    pub(super) fn landed(sent: &[(usize, usize, u64)], back: &windows::Win32::Graphics::Direct3D12::ID3D12Resource) {
        use windows::Win32::Graphics::Direct3D12::D3D12_RANGE;
        let Some(end) = sent.iter().map(|&(_, len, at)| at as usize + len).max() else {
            return;
        };
        let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: a read-back buffer the landed list wrote, `end` bytes of it.
        if unsafe { back.Map(0, Some(&D3D12_RANGE { Begin: 0, End: end }), Some(&mut mapped)) }.is_err() || mapped.is_null() {
            return;
        }
        // SAFETY: mapped, `end` bytes.
        compare(sent, unsafe { std::slice::from_raw_parts(mapped as *const u8, end) });
        // SAFETY: mapped above; nothing written.
        unsafe { back.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 })) };
    }

    /// [`landed`] with what the card read in `card`.
    pub(super) fn compare(sent: &[(usize, usize, u64)], card: &[u8]) {
        let mut now = Vec::new();
        for &(src, len, at) in sent {
            let Some(read) = card.get(at as usize..at as usize + len) else { continue };
            now.clear();
            now.resize(len, 0);
            if !crate::host::read_into(src as *const u8, &mut now) {
                continue;
            }
            COUNTS[0].fetch_add(1, Ordering::Relaxed);
            if now == read {
                continue;
            }
            COUNTS[1].fetch_add(1, Ordering::Relaxed);
            learn(src, src + len, "on the card");
        }
    }

    /// The submission has landed: the fills of it the CPU did when it was
    /// sent (digest at submission, place, length) compared with title memory now.
    pub(super) fn landed_cpu(cpu: &[(u128, usize, usize)]) {
        thread_local! {
            static NOW: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        NOW.with_borrow_mut(|now| {
            for &(was, src, len) in cpu {
                if written(src, src + len) {
                    continue;
                }
                now.resize(len, 0);
                if !crate::host::read_into(src as *const u8, &mut now[..len]) {
                    continue;
                }
                COUNTS[4].fetch_add(1, Ordering::Relaxed);
                if crate::texture::digest(&now[..len]).0 == was {
                    continue;
                }
                COUNTS[5].fetch_add(1, Ordering::Relaxed);
                learn(src, src + len, "on the CPU");
            }
        });
    }

    /// Tests: nothing learned (the pages are one set for the process).
    #[cfg(test)]
    pub(super) fn forget() {
        *PAGES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn learn(from: usize, to: usize, by: &str) {
        let mut new = false;
        {
            let mut pages = PAGES.lock().unwrap_or_else(|e| e.into_inner());
            let pages = pages.get_or_insert_with(Default::default);
            for page in from / PAGE..to.div_ceil(PAGE) {
                if pages.insert(page) {
                    new = true;
                    COUNTS[3].fetch_add(1, Ordering::Relaxed);
                    tracing::info!(
                        page = format_args!("{:#x}", page * PAGE),
                        fill = format_args!("{from:#x}+{:#x}", to - from),
                        by,
                        "card steps: the title wrote a constant buffer after the submission that reads it was sent; fills \
                         there wait from now on"
                    );
                }
            }
        }
        // `VICTORY_LEARNED_IN_PLACE`: the host copies over the place go, and
        // the next ask makes them in place (the card then reads the title's
        // bytes when the draws run: no fill, no wait). After the pages' lock:
        // making a copy asks `written` under the buffers' lock.
        if new {
            let gone = crate::shadow::learned_retire(from as u64, to as u64);
            if gone > 0 {
                tracing::info!(fill = format_args!("{from:#x}+{:#x}", to - from), gone, "card steps: host copies over a learned place let go, to be made in place (VICTORY_LEARNED_IN_PLACE)");
            }
        }
    }
}

/// Whether the title was found writing a page of `[from, to)` after sending
/// ([`late`]), for `crate::host::learned_late`.
pub(super) fn learned(from: usize, to: usize) -> bool {
    late::written(from, to)
}

/// A submission that reads places the title writes after sending ([`late`])
/// sent at once all the same, behind a gate on the queue
/// (`VICTORY_CARD_STEPS_GATE=1`; off by default, see [`gate::on`]: it waits
/// before it is prepared).
///
/// With thirteen pages learned every submission of a frame (four) waited for
/// the flights ahead before it was prepared at all, and the card stood
/// meanwhile: nothing of ours queued 15 ms a frame against 1 to 4 without
/// the learning, 11 ms of them while the submitting thread prepared what it
/// could have prepared during the wait (run cs1 against ab2; 19.5 against
/// 22.2 fps in a cutscene). Only those fills need the later moment: a few
/// constant buffers of 32 bytes to 32 KiB.
///
/// So the submission is prepared and sent as with card steps; its fills of
/// learned pages are done on the CPU into the list's upload memory, which
/// the card reads when the draws run; and the queue is told to wait for a
/// fence of ours in front of the submission's lists. When the flights ahead
/// that write those places have landed (the moment the wait ended) the
/// fills are done again from title memory as it is then, and the fence is
/// signalled. What else the submission reads of the flights' is done on the
/// card as before, or the old wait is taken ([`plan`] refuses).
pub(super) mod gate {
    use super::*;

    /// A fill: upload memory, title memory, bytes.
    pub(in crate::host) type Fill = (usize, usize, usize);

    struct Gate {
        /// Opens when the flight of this number has landed.
        after: u64,
        value: u64,
        fills: Vec<Fill>,
        made: std::time::Instant,
    }

    struct Gates {
        fence: Option<ID3D12Fence>,
        made: u64,
        /// The last gate's `after`: gates open in the order they were made
        /// (a fence passes every wait for a lower value).
        after: u64,
        waiting: std::collections::VecDeque<Gate>,
    }

    // SAFETY: a D3D12 fence (free-threaded) and addresses, under the lock.
    unsafe impl Send for Gates {}

    static GATES: Mutex<Gates> = Mutex::new(Gates { fence: None, made: 0, after: 0, waiting: std::collections::VecDeque::new() });

    /// Submissions sent behind a gate, their fills, and ns from a gate's
    /// making to its opening, since the last frame-rate line.
    pub(super) static COUNTS: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

    /// Off by default: with it the stretching came back in the boat cutscene
    /// (run gt2, the user watching; 21 fps against 19.5 with the wait and
    /// 22.2 without the learning). The fills are done again the moment the
    /// flights ahead have landed, and the title writes those places a little
    /// after it is told of that landing: the wait is late enough only because
    /// preparing the submission takes some 10 ms more. `=1` for experiments.
    pub(in crate::host) fn on() -> bool {
        #[cfg(feature = "diagnostics")]
        if let Some(on) = crate::ab::pick("VICTORY_CARD_STEPS_GATE") {
            return on;
        }
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| crate::switch::on("VICTORY_CARD_STEPS_GATE"))
    }

    /// Whether a flight writes a place one of `fills` reads.
    fn in_the_way(writers: &[Writer], fills: &[Fill]) -> bool {
        writers.iter().any(|w| fills.iter().any(|&(_, src, len)| meet((w.from, w.to), (src, src + len))))
    }

    /// The gate for a submission about to be sent with `fills`: the fence
    /// and the value the queue is to wait for in front of its lists. Under
    /// the submission lock, before the submission is in the line. `None`:
    /// no fence could be made (the fills stay as they were done).
    pub(in crate::host) fn make(host: &Host, fills: Vec<Fill>) -> Option<(ID3D12Fence, u64)> {
        let after = {
            let line = line();
            let mut after = 0;
            for item in &line.line {
                if let Late::Run(flight) = item
                    && in_the_way(&writers(std::slice::from_ref(item)), &fills)
                {
                    after = after.max(flight.number);
                }
            }
            // Those being landed now are out of the line.
            if in_the_way(&line.landing, &fills) {
                after = after.max(DRAINED.load(Ordering::Acquire));
            }
            after
        };
        let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
        if gates.fence.is_none() {
            // SAFETY: a plain creation.
            gates.fence = unsafe { host.device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }.ok();
        }
        let fence = gates.fence.clone()?;
        COUNTS[0].fetch_add(1, Ordering::Relaxed);
        COUNTS[1].fetch_add(fills.len() as u64, Ordering::Relaxed);
        let value = put(&mut gates, after, fills);
        // (Landed while this looked: nobody else opens it.)
        open(&mut gates, FINISHED.load(Ordering::Acquire));
        Some((fence, value))
    }

    /// `VICTORY_CARD_STEPS_GATE_REACH=1` (with the gate; off by default): a
    /// gate opens when the card reaches it, not when the flights in the way
    /// have landed. In front of the submission the queue signals a fence of
    /// ours ("everything before this has run") and then waits for the gate;
    /// a thread of ours waits for that signal, does the fills again from title
    /// memory as it is at that moment, and opens the gate. That is the
    /// console's moment: its card reads a constant buffer when the draw runs.
    /// Nothing waits for a landing (copy-back, telling the title), and the
    /// submitting thread does not wait at all; the card stands for one round
    /// trip to a thread of ours.
    ///
    /// Why not the landing: with gates opened at landings (run c-hold-x,
    /// 2026-11-27, Series X at 1440p) a gate took 48 ms from its making to its
    /// opening (the flights in the way stood behind earlier gates), and the
    /// title wrote 93-97 % of the places behind gates no earlier than 8 ms
    /// after the opening (run d-watch-x): bytes of its next frame, not of the
    /// one the card was drawing.
    pub(in crate::host) fn reach() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| crate::switch::on("VICTORY_CARD_STEPS_GATE_REACH"))
    }

    /// [`make`] for [`reach`]: the fence the queue signals in front of the
    /// submission, the gate's fence it then waits for, and the value of both.
    pub(in crate::host) fn make_reach(host: &Host, fills: Vec<Fill>) -> Option<(ID3D12Fence, ID3D12Fence, u64)> {
        let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
        if gates.fence.is_none() {
            // SAFETY: a plain creation.
            gates.fence = unsafe { host.device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }.ok();
        }
        let gate = gates.fence.clone()?;
        let reached = reaching::fence(host)?;
        COUNTS[0].fetch_add(1, Ordering::Relaxed);
        COUNTS[1].fetch_add(fills.len() as u64, Ordering::Relaxed);
        gates.made += 1;
        let value = gates.made;
        reaching::give(reached.clone(), gate.clone(), value, fills);
        Some((reached, gate, value))
    }

    /// The thread that opens gates when the card reaches them ([`reach`]).
    pub(super) mod reaching {
        use super::Fill;
        use std::collections::VecDeque;
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        use std::sync::{Condvar, Mutex, OnceLock};
        use std::time::Instant;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence};
        use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

        struct Job {
            reached: ID3D12Fence,
            gate: ID3D12Fence,
            value: u64,
            fills: Vec<Fill>,
        }

        // SAFETY: D3D12 fences (free-threaded) and addresses of upload and
        // title memory alive until the gate's lists have run.
        unsafe impl Send for Job {}

        struct Shared(Option<ID3D12Fence>);
        // SAFETY: a D3D12 fence, free-threaded.
        unsafe impl Send for Shared {}

        static FENCE: Mutex<Shared> = Mutex::new(Shared(None));
        static LINE: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
        static MOVED: Condvar = Condvar::new();
        /// Gates opened; µs from the card reaching them to their opening;
        /// places whose bytes changed since the submission was prepared.
        pub(in crate::host) static COUNTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

        pub(super) fn fence(host: &super::Host) -> Option<ID3D12Fence> {
            let mut f = FENCE.lock().unwrap_or_else(|e| e.into_inner());
            if f.0.is_none() {
                // SAFETY: a plain creation.
                f.0 = unsafe { host.device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }.ok();
            }
            f.0.clone()
        }

        pub(super) fn give(reached: ID3D12Fence, gate: ID3D12Fence, value: u64, fills: Vec<Fill>) {
            static STARTED: OnceLock<()> = OnceLock::new();
            STARTED.get_or_init(|| {
                if let Err(e) = std::thread::Builder::new().name("victory-reach".into()).spawn(serve_all) {
                    tracing::error!(error = %e, "no thread to open gates the card reaches");
                }
            });
            LINE.lock().unwrap_or_else(|e| e.into_inner()).push_back(Job { reached, gate, value, fills });
            MOVED.notify_one();
        }

        fn serve_all() {
            crate::cores::keep_helper_on_fast_cores("victory-reach");
            // SAFETY: a plain auto-reset event, ours for the thread's life.
            let event = unsafe { CreateEventW(None, false, false, None) }.unwrap_or(HANDLE::default());
            let mut places: Vec<(usize, usize)> = Vec::new();
            let mut which: Vec<usize> = Vec::new();
            let mut offsets: Vec<(usize, usize)> = Vec::new();
            let mut scratch: Vec<u8> = Vec::new();
            loop {
                let job = {
                    let mut line = LINE.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        match line.pop_front() {
                            Some(j) => break j,
                            None => line = MOVED.wait(line).unwrap_or_else(|e| e.into_inner()),
                        }
                    }
                };
                places.clear();
                which.clear();
                which.extend(job.fills.iter().map(|&(_, src, len)| {
                    match places.iter().position(|&p| p == (src, len)) {
                        Some(k) => k,
                        None => {
                            places.push((src, len));
                            places.len() - 1
                        }
                    }
                }));
                // SAFETY: our fences and event.
                unsafe {
                    if job.reached.GetCompletedValue() < job.value {
                        match job.reached.SetEventOnCompletion(job.value, event) {
                            Ok(()) => {
                                WaitForSingleObject(event, INFINITE);
                            }
                            Err(e) => tracing::error!(error = %e, "a gate's reaching could not be waited for: it opens now"),
                        }
                    }
                }
                let reached = Instant::now();
                // Each place once (some 40 fills a gate read a dozen places),
                // read into our reused scratch buffer, then written to every fill.
                // Upload memory is write-combined: nothing reads it back.
                scratch.clear();
                offsets.clear();
                for &(src, len) in &places {
                    let start = scratch.len();
                    scratch.resize(start + len, 0);
                    if crate::host::read_into(src as *const u8, &mut scratch[start..start + len]) {
                        offsets.push((start, len));
                    } else {
                        scratch.truncate(start);
                        offsets.push((usize::MAX, 0));
                    }
                }
                for (i, &(dst, _, len)) in job.fills.iter().enumerate() {
                    let (start, n) = offsets[which[i]];
                    if start == usize::MAX || n != len {
                        continue;
                    }
                    // SAFETY: the fill's upload memory, alive until its lists
                    // are reset; `len` bytes on each side.
                    unsafe { std::ptr::copy_nonoverlapping(scratch.as_ptr().add(start), dst as *mut u8, len) };
                }
                // SAFETY: our fence; jobs are served in the order the gates
                // were made, so its values only go up.
                if let Err(e) = unsafe { job.gate.Signal(job.value) } {
                    tracing::error!(error = %e, "a gate the card reached could not be opened");
                }
                COUNTS[0].fetch_add(1, Relaxed);
                COUNTS[1].fetch_add(reached.elapsed().as_micros() as u64, Relaxed);
            }
        }
    }

    /// A gate more, to open after the flight `after` and not before the
    /// gates made before it: its value.
    fn put(gates: &mut Gates, after: u64, fills: Vec<Fill>) -> u64 {
        gates.made += 1;
        gates.after = gates.after.max(after);
        gates.waiting.push_back(Gate { after: gates.after, value: gates.made, fills, made: std::time::Instant::now() });
        gates.made
    }

    /// The flights up to number `last` have landed.
    pub(in crate::host) fn landed(last: u64) {
        let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
        if !gates.waiting.is_empty() {
            open(&mut gates, last);
        }
    }

    fn open(gates: &mut Gates, last: u64) {
        while gates.waiting.front().is_some_and(|g| g.after <= last) {
            let Some(gate) = gates.waiting.pop_front() else { break };
            COUNTS[2].fetch_add(gate.made.elapsed().as_nanos() as u64, Ordering::Relaxed);
            #[cfg(feature = "diagnostics")]
            watch::opened(&gate.fills);
            if let (Some(hold), Some(fence)) = (holder::hold(), gates.fence.clone()) {
                holder::give(fence, gate.value, gate.fills, hold);
                continue;
            }
            for &(dst, src, len) in &gate.fills {
                // (As the submission's fills are done; upload memory of lists
                // that have not run, alive until they are reset.)
                copy_mapped(dst as *mut u8, src as *const u8, len, false);
            }
            if let Some(fence) = &gates.fence {
                // SAFETY: our fence; values only go up.
                if let Err(e) = unsafe { fence.Signal(gate.value) } {
                    tracing::error!(error = %e, "a gate in front of a submission could not be opened");
                }
            }
        }
    }

    /// A gate whose flights ahead have landed, held open-to-be by a thread of
    /// its own until the title has written its places again
    /// (`VICTORY_CARD_STEPS_GATE_HOLD_US=<us>`, with the gate; off without it).
    ///
    /// Why: the title writes those places a little after it is told the
    /// flights ahead landed ([`on`]), so a gate that fills at once fills the
    /// bytes of before. The old wait was late enough only because preparing
    /// the submission takes a while. Here the card stands at the gate (the
    /// submitting thread does not): a place is taken when its bytes differ
    /// from what was sent and two looks in a row agree (the title is not in
    /// the middle of writing it); the gate opens when every place is taken,
    /// or `VICTORY_CARD_STEPS_GATE_GRACE_US` (200) after the first one was,
    /// or at the hold's end, with whatever title memory holds then.
    pub(super) mod holder {
        use super::Fill;
        use std::collections::VecDeque;
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        use std::sync::{Condvar, Mutex, OnceLock};
        use std::time::{Duration, Instant};
        use windows::Win32::Graphics::Direct3D12::ID3D12Fence;

        struct Held {
            fence: ID3D12Fence,
            value: u64,
            fills: Vec<Fill>,
            given: Instant,
            hold: Duration,
        }

        // SAFETY: a D3D12 fence (free-threaded) and addresses of upload and
        // title memory that stay alive until the gate's lists have run.
        unsafe impl Send for Held {}

        static LINE: Mutex<VecDeque<Held>> = Mutex::new(VecDeque::new());
        static MOVED: Condvar = Condvar::new();
        /// Gates held; opened with every place taken, by the grace, by the
        /// hold's end; µs from giving to opening.
        pub(in crate::host) static COUNTS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

        fn micros(name: &str, default: Option<u64>) -> Option<u64> {
            match std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()) {
                Some(0) => None,
                Some(us) => Some(us),
                None => default,
            }
        }

        pub(in crate::host) fn hold() -> Option<Duration> {
            static HOLD: OnceLock<Option<Duration>> = OnceLock::new();
            *HOLD.get_or_init(|| micros("VICTORY_CARD_STEPS_GATE_HOLD_US", None).map(Duration::from_micros))
        }

        fn grace() -> Duration {
            static GRACE: OnceLock<Duration> = OnceLock::new();
            *GRACE.get_or_init(|| Duration::from_micros(micros("VICTORY_CARD_STEPS_GATE_GRACE_US", Some(200)).unwrap_or(200)))
        }

        pub(in crate::host) fn give(fence: ID3D12Fence, value: u64, fills: Vec<Fill>, hold: Duration) {
            static STARTED: OnceLock<()> = OnceLock::new();
            STARTED.get_or_init(|| {
                if let Err(e) = std::thread::Builder::new().name("victory-gate".into()).spawn(serve_all) {
                    tracing::error!(error = %e, "no thread for gates: they open at once");
                }
            });
            LINE.lock().unwrap_or_else(|e| e.into_inner()).push_back(Held { fence, value, fills, given: Instant::now(), hold });
            MOVED.notify_one();
        }

        fn serve_all() {
            crate::cores::keep_helper_on_fast_cores("victory-gate");
            loop {
                let held = {
                    let mut line = LINE.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        match line.pop_front() {
                            Some(h) => break h,
                            None => line = MOVED.wait(line).unwrap_or_else(|e| e.into_inner()),
                        }
                    }
                };
                serve(held);
            }
        }

        fn serve(held: Held) {
            let Held { fence, value, fills, given, hold } = held;
            COUNTS[0].fetch_add(1, Relaxed);
            let n = fills.len();
            // Per place: taken; the bytes of the last look.
            let mut taken = vec![false; n];
            let mut last: Vec<Option<Vec<u8>>> = vec![None; n];
            let mut first: Option<Instant> = None;
            let mut now_bytes = Vec::new();
            let why = loop {
                for (i, &(dst, src, len)) in fills.iter().enumerate() {
                    if taken[i] {
                        continue;
                    }
                    now_bytes.clear();
                    now_bytes.resize(len, 0);
                    if !crate::host::read_into(src as *const u8, &mut now_bytes) {
                        continue;
                    }
                    // SAFETY: the fill's upload memory, alive until its lists are reset.
                    let sent = unsafe { std::slice::from_raw_parts(dst as *const u8, len) };
                    if now_bytes[..] == *sent {
                        last[i] = None;
                        continue;
                    }
                    if last[i].as_deref() == Some(&now_bytes[..]) {
                        taken[i] = true;
                        first.get_or_insert_with(Instant::now);
                    } else {
                        last[i] = Some(now_bytes.clone());
                    }
                }
                if taken.iter().all(|&t| t) {
                    break 1;
                }
                let now = Instant::now();
                if first.is_some_and(|f| now.duration_since(f) >= grace()) {
                    break 2;
                }
                if now.duration_since(given) >= hold {
                    break 3;
                }
                for _ in 0..64 {
                    std::hint::spin_loop();
                }
            };
            COUNTS[why].fetch_add(1, Relaxed);
            COUNTS[4].fetch_add(given.elapsed().as_micros() as u64, Relaxed);
            for &(dst, src, len) in &fills {
                super::copy_mapped(dst as *mut u8, src as *const u8, len, false);
            }
            // SAFETY: our fence; gates are served in the order they were made,
            // so values only go up.
            if let Err(e) = unsafe { fence.Signal(value) } {
                tracing::error!(error = %e, "a held gate in front of a submission could not be opened");
            }
        }
    }

    /// Diagnostics (`VICTORY_CARD_STEPS_GATE_WATCH=1`, with the gate): when
    /// the title writes the places a gate's fills read, counted from the
    /// gate's opening. Every eighth gate's places are read again every tenth
    /// of a millisecond for 30 ms by a thread of its own; logged once a
    /// second: how many places were as sent when the gate opened, how many
    /// changed after it, and when each changed for the last time.
    #[cfg(feature = "diagnostics")]
    pub(super) mod watch {
        use super::Fill;
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        use std::sync::{Mutex, OnceLock, mpsc};
        use std::time::{Duration, Instant};

        /// Upper ends of the classes of a place's last change, in tenths of
        /// a millisecond after the opening.
        const CLASSES: [u64; 8] = [2, 5, 10, 20, 40, 80, 160, 300];
        /// Places watched; as sent at the opening; changed after it; then by class.
        static SEEN: [AtomicU64; 11] = [const { AtomicU64::new(0) }; 11];
        static GATES: AtomicU64 = AtomicU64::new(0);

        fn on() -> bool {
            static ON: OnceLock<bool> = OnceLock::new();
            *ON.get_or_init(|| crate::switch::on("VICTORY_CARD_STEPS_GATE_WATCH"))
        }

        type Asked = (Instant, Vec<(usize, Vec<u8>, bool)>);

        fn watcher() -> &'static Mutex<mpsc::Sender<Asked>> {
            static TO: OnceLock<Mutex<mpsc::Sender<Asked>>> = OnceLock::new();
            TO.get_or_init(|| {
                let (to, from) = mpsc::channel::<Asked>();
                std::thread::spawn(move || {
                    for (opened, mut places) in from {
                        let mut last: Vec<Option<Duration>> = vec![None; places.len()];
                        let mut now = Vec::new();
                        while opened.elapsed() < Duration::from_millis(30) {
                            for (i, (src, bytes, _)) in places.iter_mut().enumerate() {
                                now.clear();
                                now.resize(bytes.len(), 0);
                                if crate::host::read_into(*src as *const u8, &mut now) && now != *bytes {
                                    bytes.copy_from_slice(&now);
                                    last[i] = Some(opened.elapsed());
                                }
                            }
                            std::thread::sleep(Duration::from_micros(100));
                        }
                        for (at, (_, _, as_sent)) in last.iter().zip(&places) {
                            SEEN[0].fetch_add(1, Relaxed);
                            SEEN[1].fetch_add(u64::from(*as_sent), Relaxed);
                            if let Some(at) = at {
                                SEEN[2].fetch_add(1, Relaxed);
                                let tenths = at.as_micros() as u64 / 100;
                                let class = CLASSES.iter().position(|&c| tenths < c).unwrap_or(CLASSES.len() - 1);
                                SEEN[3 + class].fetch_add(1, Relaxed);
                            }
                        }
                    }
                });
                Mutex::new(to)
            })
        }

        /// A gate opens now, before its fills are done again.
        pub(super) fn opened(fills: &[Fill]) {
            if !on() || !GATES.fetch_add(1, Relaxed).is_multiple_of(8) {
                return;
            }
            let mut seen = std::collections::HashSet::new();
            let mut places = Vec::new();
            for &(dst, src, len) in fills {
                if !seen.insert((src, len)) {
                    continue;
                }
                let mut bytes = vec![0; len];
                if !crate::host::read_into(src as *const u8, &mut bytes) {
                    continue;
                }
                // SAFETY: the fill's upload memory, alive until its lists are reset.
                let as_sent = unsafe { std::slice::from_raw_parts(dst as *const u8, len) } == &bytes[..];
                places.push((src, bytes, as_sent));
            }
            let _ = watcher().lock().unwrap_or_else(|e| e.into_inner()).send((Instant::now(), places));
        }

        pub(in crate::host::card_steps) fn log() {
            if !on() {
                return;
            }
            let s: Vec<u64> = SEEN.iter().map(|c| c.swap(0, Relaxed)).collect();
            tracing::info!(
                "card steps, the places behind gates watched (VICTORY_CARD_STEPS_GATE_WATCH): {} places, {} as sent when the gate opened, \
                 {} changed after it; their last change under 0.2 ms: {}, 0.5: {}, 1: {}, 2: {}, 4: {}, 8: {}, 16: {}, 30: {}",
                s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], s[10]
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A gate opens when the flight it waits for has landed, with its
        /// fills done again from the memory as it is then, and never before
        /// a gate made earlier.
        #[test]
        fn gates_open_in_order_and_fill_again() {
            let mut gates = Gates { fence: None, made: 0, after: 0, waiting: std::collections::VecDeque::new() };
            let title = [1u8, 2, 3, 4, 5, 6, 7, 8];
            let mut upload = [0u8; 8];
            let (src, dst) = (title.as_ptr() as usize, upload.as_mut_ptr() as usize);
            // The first waits for flight 7, the second for flight 3 only:
            // behind the first all the same.
            assert_eq!(put(&mut gates, 7, vec![(dst, src, 4)]), 1);
            assert_eq!(put(&mut gates, 3, vec![(dst + 4, src + 4, 4)]), 2);
            open(&mut gates, 3);
            assert_eq!(gates.waiting.len(), 2);
            assert_eq!(upload, [0; 8]);
            open(&mut gates, 6);
            assert_eq!(gates.waiting.len(), 2);
            open(&mut gates, 7);
            assert!(gates.waiting.is_empty());
            assert_eq!(upload, title);
            // Nothing ahead writes its places: open at the next look.
            let mut again = [0u8; 2];
            put(&mut gates, 0, vec![(again.as_mut_ptr() as usize, src, 2)]);
            assert_eq!(gates.waiting.front().map(|g| g.after), Some(7), "not before the gates made before it");
            open(&mut gates, 7);
            assert_eq!(again, [1, 2]);
        }
    }
}

/// Why a submission is refused for [`late`].
const WRITTEN_LATE: &str = "a fill reads a place the title was found writing after sending";

impl Taken {
    pub(super) fn list(&self) -> &HostList {
        &self.own.list
    }

    /// The slots left for later, to write before the submission's lists go
    /// to the queue.
    pub(super) fn slots(&mut self) -> Option<Slots> {
        self.slots.take()
    }

    /// Whether step `i` of `list` is done by this.
    pub(super) fn takes(&self, list: &HostList, i: usize) -> bool {
        self.steps.contains(&(std::ptr::from_ref(list) as usize, i))
    }

    fn copied(&self) -> impl Iterator<Item = Writer> {
        self.copied.iter().map(|(shadow, from, to)| Writer {
            shadow: Some(shadow.clone()),
            from: *from,
            to: *to,
            copy: true,
        })
    }

    /// The submission has run and landed: the list and its buffer are free.
    pub(super) fn landed(self) {
        if let Some(back) = &self.own.back {
            late::landed(&self.sent, back);
        }
        late::landed_cpu(&self.cpu);
        // The host copies its steps held go; its commands stay until the
        // next use resets it.
        drop(std::mem::take(&mut *self.own.list.deferred()));
        POOL.lock().unwrap_or_else(|e| e.into_inner()).push(self.own);
    }
}

impl Host {
    /// With flights in the air: if what keeps `runs` from being prepared
    /// now is reads of title memory that can all be done on the card
    /// ([`stage`]), the list that does them. `None` if nothing is in the
    /// way or the old wait is due. Under the submission lock. With `later`
    /// the slots it fills are not written here but kept in it
    /// ([`Taken::slots`]).
    pub(super) fn card_steps(&self, runs: &[&HostList], later: bool) -> (Option<Taken>, Vec<gate::Fill>) {
        if stage() == 0 || !relay_exact() {
            return (None, Vec::new());
        }
        let (written, copies) = {
            let line = line();
            if line.written.is_empty() {
                return (None, Vec::new());
            }
            (line.written.clone(), copies(&line))
        };
        let looked = crate::timeline::span("steps on the card: whether in the way");
        if blocked_by(&written, runs, false).is_none() {
            return (None, Vec::new());
        }
        let read = copied_read(&copies, runs);
        looked.end();
        let taken = if read {
            Err("a copy done on the card is read through another host copy")
        } else {
            self.take_steps(&written, runs, later)
        };
        match taken {
            Ok(taken) => taken,
            Err(why) => {
                DONE[3].fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "diagnostics")]
                refused(why);
                let _ = why;
                (None, Vec::new())
            }
        }
    }

    fn take_steps(&self, written: &[(usize, usize)], runs: &[&HostList], later: bool) -> Result<(Option<Taken>, Vec<gate::Fill>), &'static str> {
        let mut spent = crate::timeline::span("steps on the card: chosen");
        let (mut does, steps) = does_of(written, runs);
        // Every fill of the submission, taken or not: one done on the CPU is
        // done now too, not after the wait ([`late`]). With the gate such a
        // fill stays on the CPU and is done again when the gate opens.
        let mut gated: Vec<gate::Fill> = Vec::new();
        // `VICTORY_LEARNED_ON_CARD=1` (off by default): a fill of a learned
        // place is done on the card all the same, reading the title's memory
        // through a view of ours in place (`crate::shadow::in_place_block`):
        // the copy runs when the card reaches the submission, after the
        // title's late write, as the console's draws read it then. No wait,
        // and only those few hundred bytes cross the bus. Its bytes are still
        // read back and compared at landing (`late`: "found written again").
        let mut learned: std::collections::HashSet<usize> = std::collections::HashSet::new();
        if late::on() {
            let gate = gate::on();
            for (k, d) in does.iter_mut().enumerate() {
                let Does::Fill { src, len, asked, to } = d else { continue };
                if !late::written(*src, *src + *len) {
                    continue;
                }
                if learned_on_card() && *to && crate::shadow::in_place_block(*src as u64, *len as u64).is_some() {
                    learned.insert(k);
                    continue;
                }
                if !gate {
                    late::COUNTS[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Err(WRITTEN_LATE);
                }
                (*asked, *to) = (false, false);
                let (list, i) = steps[k];
                let dst = runs.iter().find(|l| std::ptr::from_ref::<HostList>(l) as usize == list).and_then(|l| {
                    match l.deferred().copies.get(i) {
                        Some(&Step::Fill(dst, ..)) => Some(dst),
                        _ => None,
                    }
                });
                match dst {
                    Some(dst) => gated.push((dst, *src, *len)),
                    None => return Err(WRITTEN_LATE),
                }
            }
        }
        let chosen = match plan(&does, stage(), placed_order(), &mut None) {
            Ok(chosen) => chosen,
            Err(why) => {
                if !gated.is_empty() {
                    late::COUNTS[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                return Err(why);
            }
        };
        if chosen.is_empty() {
            // Nothing for the card: with fills behind the gate the wait is
            // spared only if nothing else is in the flights' way.
            if !gated.is_empty() && blocked_by_but(written, runs, false, &|src, len| late::written(src, src + len)).is_some() {
                late::COUNTS[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                gated.clear();
            }
            return Ok((None, gated));
        }
        spent.next("steps on the card: what holds their bytes");
        let mut writers = {
            let line = line();
            let mut all = writers(&line.line);
            all.extend(line.landing.iter().map(|w| Writer {
                shadow: w.shadow.clone(),
                ..*w
            }));
            all
        };
        let mut ops: Vec<Op> = Vec::with_capacity(chosen.len());
        // The places read (host copy, address, bytes, level): where in the
        // buffer between their bytes are.
        let mut places: HashMap<(usize, usize, usize, u32), u64> = HashMap::default();
        // Some 2,800 fills a frame, of a few hundred places: what holds each
        // is looked for once, as long as no copy is added to the writers.
        let mut held: HashMap<(usize, usize), Arc<Shadow>> = HashMap::default();
        let mut room = HashMap::default();
        let mut end = 0u64;
        for &(i, level) in &chosen {
            let Some(((src, _), (dst, _))) = reads_writes(&does[i]) else {
                continue;
            };
            let (Does::Copy { len, .. } | Does::Fill { len, .. }) = does[i] else {
                continue;
            };
            if len == 0 {
                return Err("nothing to read");
            }
            let from = if learned.contains(&i) {
                // `VICTORY_LEARNED_ON_CARD`: the title's memory itself.
                crate::shadow::in_place_block(src as u64, len as u64).ok_or("no view in place of a learned place")?
            } else {
                match held.get(&(src, len)) {
                    Some(from) => from.clone(),
                    None => {
                        let from = holder(&writers, src, src + len)?;
                        held.insert((src, len), from.clone());
                        from
                    }
                }
            };
            let place = (Arc::as_ptr(&from) as usize, src, len, level);
            let at = match places.get(&place) {
                Some(&at) => at,
                None => {
                    if end + len as u64 > BETWEEN {
                        return Err("too much at once");
                    }
                    if !mapped(src, src + len, &mut room) {
                        return Err("not all of what it reads is mapped");
                    }
                    let at = end;
                    end = (end + len as u64).next_multiple_of(256);
                    places.insert(place, at);
                    at
                }
            };
            let to = match does[i] {
                Does::Copy { .. } => {
                    let to = holder(&writers, dst, dst + len)?;
                    if elsewhere(&does, Arc::as_ptr(&to) as usize, dst, dst + len) {
                        return Err("another host copy of what it writes is read");
                    }
                    if !mapped(dst, dst + len, &mut room) {
                        return Err("not all of what it writes is mapped");
                    }
                    // What the steps behind it read of it is in that host
                    // copy, or nowhere.
                    writers.push(Writer {
                        shadow: Some(to.clone()),
                        from: dst,
                        to: dst + len,
                        copy: true,
                    });
                    held.clear();
                    To::Copy(to, dst)
                }
                _ => match fill_to(runs, steps[i]) {
                    Some(FillTo::Slot(heap, index)) => To::Slot(heap, index),
                    Some(FillTo::Buffer(buffer)) => To::Buffer(buffer),
                    None => continue,
                },
            };
            ops.push(Op {
                from: (from, src),
                len,
                at,
                level,
                to,
            });
        }
        spent.next("steps on the card: the list");
        let Some(own) = self.own_list() else {
            return Err("no host list or buffer for it");
        };
        // First every refresh, then the writes: the bytes compared with are
        // the title's. (One step for each host copy, over all that is read of
        // it: a flush brings a copy up to date from the first range asked for
        // to the last, whatever the steps.)
        let mut spans: ByCopy<(Arc<Shadow>, usize, usize)> = ByCopy::default();
        for op in &ops {
            let to = match &op.to {
                To::Copy(to, dst) => Some((to, *dst)),
                _ => None,
            };
            for (shadow, at) in std::iter::once((&op.from.0, op.from.1)).chain(to) {
                let span = spans.entry(Arc::as_ptr(shadow) as usize).or_insert_with(|| (shadow.clone(), at, at + op.len));
                (span.1, span.2) = (span.1.min(at), span.2.max(at + op.len));
            }
        }
        for (shadow, from, to) in spans.into_values() {
            shadow.refresh(&own.list, from as u64, (to - from) as u64);
        }
        let mut copied = Vec::new();
        for op in &ops {
            if let To::Copy(to, dst) = &op.to {
                to.gpu_writes(&own.list, *dst as u64, op.len as u64);
                copied.push((to.clone(), *dst, *dst + op.len));
            }
        }
        #[cfg(feature = "diagnostics")]
        own.list.note_op(|| format!("{} copies of title memory done on the card, {end} bytes (VICTORY_CARD_STEPS)", ops.len()));
        let levels = ops.iter().map(|op| op.level).max().unwrap_or(0);
        for level in 0..=levels {
            let mut reads = Vec::new();
            let mut writes = Vec::new();
            for op in ops.iter().filter(|op| op.level == level) {
                let len = op.len as u64;
                let read = (&op.from.0.buffer, op.from.0.offset(op.from.1 as u64), op.at, len);
                // (Each place once.)
                if places.remove(&(Arc::as_ptr(&op.from.0) as usize, op.from.1, op.len, level)).is_some() {
                    reads.push(read);
                }
                match &op.to {
                    To::Copy(to, dst) => writes.push((&to.buffer, to.offset(*dst as u64), op.at, len)),
                    To::Buffer(buffer) => writes.push((buffer, 0, op.at, len)),
                    To::Slot(..) => {}
                }
            }
            record(level == 0, &own.list.list, &own.between, &reads, &writes);
        }
        // What the fills read, read back ([`late`]): each place once, and
        // where its bytes are in the buffer between.
        let mut sent: Vec<(usize, usize, u64)> = Vec::new();
        if let Some(back) = own.back.as_ref().filter(|_| late::on() && end > 0) {
            // (The buffer between is readable as constants and as a copy
            // source after the last level.)
            // SAFETY: an open list; both buffers live and `BETWEEN` long.
            unsafe { own.list.list.CopyBufferRegion(back, 0, &own.between, 0, end) };
            let mut seen: HashMap<(usize, usize), ()> = HashMap::default();
            for op in ops.iter().filter(|op| !matches!(op.to, To::Copy(..))) {
                if seen.insert((op.from.1, op.len), ()).is_none() {
                    sent.push((op.from.1, op.len, op.at));
                }
            }
        }
        own.list.close();
        // SAFETY: a live buffer.
        let gpu = unsafe { own.between.GetGPUVirtualAddress() };
        spent.next("steps on the card: the slots");
        let slots = Slots {
            ops: ops
                .iter()
                .filter_map(|op| match op.to {
                    To::Slot(heap, index) => Some((heap, index, gpu + op.at, op.len as u32)),
                    _ => None,
                })
                .collect(),
            between: own.between.clone(),
            parts: if ops.len() < 256 { 1 } else { 8 },
        };
        let slots = if later {
            Some(slots)
        } else {
            slots.write(false);
            None
        };
        let copies = ops.iter().filter(|op| matches!(op.to, To::Copy(..))).count();
        DONE[0].fetch_add((ops.len() - copies) as u64, Ordering::Relaxed);
        DONE[1].fetch_add(copies as u64, Ordering::Relaxed);
        DONE[2].fetch_add(ops.iter().map(|op| op.len as u64).sum(), Ordering::Relaxed);
        #[cfg(feature = "diagnostics")]
        noted(&ops);
        spent.end();
        let steps: std::collections::HashSet<(usize, usize), std::hash::BuildHasherDefault<AddressHash>> =
            chosen.iter().map(|&(i, _)| steps[i]).collect();
        // The fills left to the CPU: snapshot their digest from cached title
        // memory now so `landed_cpu` never reads back from WC upload memory.
        // Skip fills whose pages are already learned (`late::written`).
        let mut cpu = Vec::new();
        if late::on() {
            thread_local! {
                static SCRATCH: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
            }
            SCRATCH.with_borrow_mut(|scratch| {
                for list in runs {
                    let at = std::ptr::from_ref::<HostList>(list) as usize;
                    for (i, step) in list.deferred().copies.iter().enumerate() {
                        if let &Step::Fill(_dst, src, len, _) = step
                            && !steps.contains(&(at, i))
                            && !late::written(src, src + len)
                        {
                            scratch.resize(len, 0);
                            if crate::host::read_into(src as *const u8, &mut scratch[..len]) {
                                cpu.push((crate::texture::digest(&scratch[..len]).0, src, len));
                            }
                        }
                    }
                }
            });
        }
        Ok((
            Some(Taken {
                own,
                steps,
                copied,
                sent,
                cpu,
                slots,
            }),
            gated,
        ))
    }

    /// A list of ours open for recording, and its buffer.
    fn own_list(&self) -> Option<Own> {
        let pooled = POOL.lock().unwrap_or_else(|e| e.into_inner()).pop();
        if let Some(own) = pooled {
            own.list.reset();
            return Some(own);
        }
        let between = self.uav_buffer(BETWEEN)?;
        name(&between, || "copies of title memory on the card".into());
        let list = self.new_list().ok()?;
        let back = if late::on() { self.read_back_buffer(BETWEEN) } else { None };
        Some(Own { list, between, back })
    }
}

/// Diagnostics: the copies and fills done on the card since the last
/// frame-rate line, by bytes, host copy read and host copy written (start
/// and length; zeros for a fill): how many, and the last title addresses.
#[cfg(feature = "diagnostics")]
type Noted = std::collections::HashMap<(usize, (u64, u64), (u64, u64)), (u64, usize, usize)>;
#[cfg(feature = "diagnostics")]
static NOTED: Mutex<Option<Noted>> = Mutex::new(None);

#[cfg(feature = "diagnostics")]
fn noted(ops: &[Op]) {
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    let noted = noted.get_or_insert_with(Default::default);
    for op in ops {
        let (to, dst) = match &op.to {
            To::Copy(to, dst) => (to.span(), *dst),
            To::Slot(..) | To::Buffer(_) => ((0, 0), 0),
        };
        let e = noted.entry((op.len, op.from.0.span(), to)).or_default();
        *e = (e.0 + 1, op.from.1, dst);
    }
}

/// For the frame-rate line.
pub(super) fn log(frames: f64) {
    if stage() == 0 {
        return;
    }
    let d: Vec<f64> = DONE.iter().map(|d| d.swap(0, Ordering::Relaxed) as f64 / frames).collect();
    tracing::info!(
        "steps done on the card instead of waiting for flights, per frame (VICTORY_CARD_STEPS={}): {:.2} fills, {:.2} buffer \
         copies, {:.1} KB; {:.2} submissions refused and waited",
        stage(),
        d[0],
        d[1],
        d[2] / 1024.0,
        d[3],
    );
    if late::on() {
        // (Index 3 counts in all, the others since the last line.)
        let c: Vec<f64> = late::COUNTS
            .iter()
            .enumerate()
            .map(|(i, c)| if i == 3 { 0.0 } else { c.swap(0, Ordering::Relaxed) as f64 / frames })
            .collect();
        tracing::info!(
            "card steps, places the title writes after sending (VICTORY_CARD_STEPS_LEARN), per frame: {:.1} fills on the card \
             and {:.1} on the CPU compared at landing, {:.2} and {:.2} found written again, {:.2} submissions waited for a \
             learned page; {} pages learned",
            c[0],
            c[4],
            c[1],
            c[5],
            c[2],
            late::COUNTS[3].load(Ordering::Relaxed)
        );
        #[cfg(feature = "diagnostics")]
        gate::watch::log();
        #[cfg(feature = "diagnostics")]
        crate::list::wait_kinds::log();
        let g: Vec<f64> = gate::COUNTS.iter().map(|c| c.swap(0, Ordering::Relaxed) as f64 / frames).collect();
        tracing::info!(
            "card steps, submissions sent behind a gate instead of that wait (VICTORY_CARD_STEPS_GATE={}), per frame: {:.2} with \
             {:.1} fills done again when it opened, {:.1} ms from making to opening",
            u8::from(gate::on()),
            g[0],
            g[1],
            g[2] / 1e6
        );
        if gate::reach() {
            let r: Vec<f64> = gate::reaching::COUNTS.iter().map(|c| c.swap(0, Ordering::Relaxed) as f64 / frames).collect();
            tracing::info!(
                "card steps, gates opened when the card reached them (VICTORY_CARD_STEPS_GATE_REACH), per frame: {:.2} opened, \
                 {:.3} ms from the card reaching one to its opening, {:.1} places changed since they were prepared",
                r[0],
                if r[0] > 0.0 { r[1] / 1e3 / r[0] } else { 0.0 },
                r[2]
            );
        }
        if gate::holder::hold().is_some() {
            let h: Vec<f64> = gate::holder::COUNTS.iter().map(|c| c.swap(0, Ordering::Relaxed) as f64 / frames).collect();
            tracing::info!(
                "card steps, gates held until the title wrote their places again (VICTORY_CARD_STEPS_GATE_HOLD_US), per frame: {:.2} held, \
                 opened {:.2} with every place written, {:.2} by the grace, {:.2} at the hold's end; {:.2} ms held in all",
                h[0],
                h[1],
                h[2],
                h[3],
                h[4] / 1e3
            );
        }
    }
    #[cfg(feature = "diagnostics")]
    {
        let r: Vec<f64> = place::READERS.iter().map(|r| r.swap(0, Ordering::Relaxed) as f64 / frames).collect();
        if r.iter().any(|&n| n > 0.0) {
            tracing::info!(
                "what a copy at its place writes, found read on the CPU behind it, per frame (looks, all threads): {:.2} by \
                 another such copy, {:.2} by a buffer copy, {:.2} by a fill, {:.2} by a texture, {:.2} by a step of ours",
                r[0],
                r[1],
                r[2],
                r[3],
                r[4]
            );
        }
        let noted = NOTED.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
        let mut noted: Vec<_> = noted.into_iter().collect();
        noted.sort_unstable_by_key(|(_, e)| std::cmp::Reverse(e.0));
        for ((len, from, to), (n, src, dst)) in noted.into_iter().take(5) {
            let n = n as f64 / frames;
            if to.1 == 0 {
                tracing::info!("done on the card, per frame: {n:.2} fills of {len:#x} bytes from the host copy of {:#x}+{:#x} (last {src:#x})", from.0, from.1);
            } else {
                tracing::info!(
                    "done on the card, per frame: {n:.2} copies of {len:#x} bytes from the host copy of {:#x}+{:#x} (last {src:#x}) \
                     to that of {:#x}+{:#x} (last {dst:#x})",
                    from.0,
                    from.1,
                    to.0,
                    to.1
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The learned pages are the process's ([`late`]): the tests that learn
    /// some, one at a time, each from none (their memory may lie where the
    /// one before learned).
    static LEARNING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn copy(dst: usize, src: usize, len: usize, asked: bool) -> Does {
        Does::Copy { dst, src, len, asked }
    }

    fn fill(src: usize, len: usize, asked: bool) -> Does {
        Does::Fill { src, len, asked, to: true }
    }

    fn stuck(src: usize, len: usize) -> Does {
        Does::Fill { src, len, asked: false, to: false }
    }

    /// What is done on the card, as (place, level), in stage 2.
    fn planned(does: &[Does]) -> Vec<(usize, u32)> {
        plan(does, 2, true, &mut None).unwrap()
    }

    /// A place written between sending and landing is learned, page by page;
    /// one left alone is not.
    #[test]
    fn places_written_after_sending_are_learned() {
        let _one = LEARNING.lock().unwrap_or_else(|e| e.into_inner());
        late::forget();
        // Two pages of memory of our own, page-aligned, as title memory.
        let memory = vec![0u8; 4 * 4096];
        let page = (memory.as_ptr() as usize).next_multiple_of(4096);
        let (kept, changed) = (page, page + 4096 + 64);
        // What the card read: the bytes as they were, at 0 and 256 of the
        // buffer between.
        let sent = [(kept, 32, 0), (changed, 240, 256)];
        let card = vec![0u8; 512];
        // SAFETY: inside `memory`.
        unsafe { *(changed as *mut u8).add(88) = 1 };
        assert!(!late::written(kept, kept + 32) && !late::written(changed, changed + 240));
        late::compare(&sent, &card);
        assert!(!late::written(kept, kept + 32));
        assert!(late::written(changed, changed + 240));
        // The whole page: another constant buffer in it waits too.
        assert!(late::written(page + 4096, page + 4096 + 16));
        drop(memory);
    }

    /// A fill the CPU did into upload memory, whose place the title wrote
    /// again before landing, is learned; one that still matches is not.
    #[test]
    fn fills_done_on_the_cpu_are_learned_too() {
        let _one = LEARNING.lock().unwrap_or_else(|e| e.into_inner());
        late::forget();
        let memory = vec![0u8; 4 * 4096];
        let page = (memory.as_ptr() as usize).next_multiple_of(4096);
        let (kept, changed) = (page + 16, page + 4096 + 0xbd0);
        // The digests of the bytes as they were when sent.
        let zero32 = crate::texture::digest(&[0u8; 32]).0;
        let zero544 = crate::texture::digest(&[0u8; 0x220]).0;
        let cpu = [(zero32, kept, 32), (zero544, changed, 0x220)];
        // SAFETY: inside `memory`.
        unsafe { *(changed as *mut u8).add(0x100) = 7 };
        late::landed_cpu(&cpu);
        assert!(!late::written(kept, kept + 32));
        assert!(late::written(changed, changed + 0x220));
        drop(memory);
    }

    /// Only what is asked for is taken, all of it, and a step that stands
    /// in none of their ways stays where it is.
    #[test]
    fn steps_apart_from_one_another_are_taken() {
        let does = [
            copy(1000, 5000, 100, false),
            fill(2000, 64, true),
            Does::Refresh { shadow: 1, from: 0, to: 9000 },
            copy(3000, 6000, 100, true),
            fill(1000, 64, false),
            Does::Read { from: 3100, to: 4000, asked: false },
            stuck(3100, 5),
        ];
        assert_eq!(planned(&does), [(1, 0), (3, 0)]);
        // Two fills of the same bytes.
        assert_eq!(plan(&[fill(0, 64, true), fill(32, 64, true)], 1, true, &mut None), Ok(vec![(0, 0), (1, 0)]));
        assert_eq!(plan(&[fill(0, 64, false)], 1, true, &mut None), Ok(vec![]));
        // Ends that touch are apart.
        let does = [
            copy(1000, 5000, 100, true),
            copy(900, 9000, 100, false),
            copy(1100, 9000, 5, false),
            copy(4900, 9000, 100, false),
        ];
        assert_eq!(planned(&does), [(0, 0)]);
    }

    /// What depends on a taken step, or it on it, is taken too, on the level
    /// the order of the steps asks for.
    #[test]
    fn steps_that_depend_on_one_another_are_taken_in_order() {
        let taken = copy(1000, 5000, 100, true);
        // Reads what it writes, behind and ahead of it.
        assert_eq!(planned(&[taken, fill(1099, 64, false)]), [(0, 0), (1, 1)]);
        assert_eq!(planned(&[fill(1099, 64, false), taken]), [(0, 0), (1, 1)]);
        assert_eq!(planned(&[taken, copy(7000, 1050, 10, false)]), [(0, 0), (1, 1)]);
        // Writes what it reads, and what it writes.
        assert_eq!(planned(&[copy(5099, 9000, 1, false), taken]), [(0, 0), (1, 1)]);
        assert_eq!(planned(&[taken, copy(5099, 9000, 1, false)]), [(0, 0), (1, 1)]);
        assert_eq!(planned(&[taken, copy(999, 9000, 2, false)]), [(0, 0), (1, 1)]);
        // The same list twice.
        assert_eq!(planned(&[taken, taken]), [(0, 0), (1, 1)]);
        // A chain, with steps apart from it in between, and two fills of one
        // copy on one level.
        let does = [
            copy(8000, 7000, 50, false),
            taken,
            copy(2000, 1010, 20, false),
            fill(90_000, 10, false),
            fill(2000, 20, false),
            fill(2010, 20, true),
            copy(1000, 2005, 5, false),
            fill(1000, 100, false),
        ];
        assert_eq!(planned(&does), [(1, 0), (2, 1), (4, 2), (5, 2), (6, 2), (7, 3)]);
        // A fill is in nobody's way.
        assert_eq!(planned(&[taken, fill(5000, 100, false)]), [(0, 0)]);
        // A copy ahead that a taken one only shares its source with.
        assert_eq!(planned(&[copy(7000, 5000, 10, false), taken]), [(1, 0)]);
    }

    /// A copy at its place in a list is in the way of what reads or writes its
    /// bytes behind it, not of what did ahead of it.
    #[test]
    fn copies_at_their_place_stand_in_the_way_of_what_is_behind_them() {
        let placed = Does::Placed { dst: 2000, src: 3000, len: 64 };
        let behind = |does: &[Does]| {
            let mut found = None;
            let refused = plan(does, 2, true, &mut found);
            assert!(refused.is_ok() || found.is_some() && refused == Err(BEHIND_PLACED));
            assert_eq!(plan(does, 2, false, &mut None).is_err(), does.len() > 1 && !matches!(does, [_, Does::Fill { src: 5000, .. }]));
            found
        };
        assert_eq!(behind(&[fill(2032, 64, true), placed]), None);
        assert_eq!(behind(&[placed, fill(2032, 64, true)]), Some((0, 1)));
        assert_eq!(behind(&[placed, fill(5000, 64, true)]), None);
        assert_eq!(behind(&[fill(2000, 8, true), placed, fill(2000, 8, true)]), Some((1, 2)));
        // One that writes what it copies from, and one that writes over it.
        assert_eq!(behind(&[copy(3000, 5000, 8, true), placed]), None);
        assert_eq!(behind(&[placed, copy(3000, 5000, 8, true)]), Some((0, 1)));
        assert_eq!(behind(&[copy(2060, 5000, 8, true), placed]), None);
        assert_eq!(behind(&[placed, copy(2060, 5000, 8, true)]), Some((0, 1)));
    }

    /// Whatever the steps, done level by level (in a level every read
    /// before every write) they read and leave what they read and leave one
    /// after the other, the CPU's steps ahead of all of them.
    #[test]
    fn levels_do_what_the_order_does() {
        let mut seed = 12345u32;
        let mut random = move |n: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) % n
        };
        let (mut done, mut refused) = (0, 0);
        for _ in 0..4000 {
            let does: Vec<Does> = (0..2 + random(9))
                .map(|_| {
                    let (at, other, len) = (random(60) as usize, random(60) as usize, 1 + random(8) as usize);
                    let asked = random(4) == 0;
                    match random(8) {
                        0..4 => copy(at, other, len, asked),
                        4..6 => fill(at, len, asked),
                        6 => stuck(at, len),
                        _ => Does::Read { from: at, to: at + len, asked: false },
                    }
                })
                .collect();
            let Ok(chosen) = plan(&does, 2, true, &mut None) else {
                refused += 1;
                continue;
            };
            done += 1;
            let start: Vec<u32> = (0..80).collect();
            // One after the other: what each reads, and what is left.
            let mut memory = start.clone();
            let mut wanted = Vec::new();
            for d in &does {
                let (from, to) = match *d {
                    Does::Read { from, to, .. } => (from, to),
                    _ => reads_writes(d).unwrap().0,
                };
                wanted.push(memory[from..to].to_vec());
                if let Does::Copy { dst, src, len, .. } = *d {
                    memory.copy_within(src..src + len, dst);
                }
            }
            // The CPU's, then the card's.
            let mut cpu = start.clone();
            let taken = |i: usize| chosen.iter().any(|c| c.0 == i);
            for (i, d) in does.iter().enumerate().filter(|&(i, _)| !taken(i)) {
                let (from, to) = match *d {
                    Does::Read { from, to, .. } => (from, to),
                    _ => reads_writes(d).unwrap().0,
                };
                assert_eq!(cpu[from..to], wanted[i][..], "step {i} of {does:?}");
                if let Does::Copy { dst, src, len, .. } = *d {
                    cpu.copy_within(src..src + len, dst);
                }
            }
            let mut card = cpu.clone();
            for level in 0..=chosen.iter().map(|c| c.1).max().unwrap_or(0) {
                let of: Vec<usize> = chosen.iter().filter(|c| c.1 == level).map(|c| c.0).collect();
                let before = card.clone();
                for i in of {
                    let ((from, to), (dst, _)) = reads_writes(&does[i]).unwrap();
                    assert_eq!(before[from..to], wanted[i][..], "step {i} of {does:?}: {chosen:?}");
                    if matches!(does[i], Does::Copy { .. }) {
                        card[dst..dst + to - from].copy_from_slice(&before[from..to]);
                    }
                }
            }
            assert_eq!(card, memory, "{does:?}: {chosen:?}");
        }
        assert!(done > 500 && refused > 100, "{done} {refused}");
    }

    /// What cannot be done on the card refuses the lot, where it stands in
    /// the way.
    #[test]
    fn steps_in_the_way_are_refused() {
        let taken = copy(1000, 5000, 100, true);
        assert!(plan(&[taken, Does::Run], 2, true, &mut None).is_err());
        // A texture that itself waits for flights; a fill that cannot be
        // taken but waits.
        assert!(plan(&[taken, Does::Read { from: 1, to: 2, asked: true }], 2, true, &mut None).is_err());
        assert!(plan(&[Does::Fill { src: 1, len: 1, asked: true, to: false }], 2, true, &mut None).is_err());
        // Reads on the CPU of what it writes: behind it, not ahead of it.
        for read in [Does::Read { from: 900, to: 1001, asked: false }, stuck(1050, 3)] {
            assert!(plan(&[taken, read], 2, true, &mut None).is_err(), "{read:?} behind");
            assert_eq!(planned(&[read, taken]), [(1, 0)], "{read:?} ahead");
        }
        // Also of what is taken along.
        assert!(plan(&[taken, copy(3000, 1000, 10, false), stuck(3005, 1)], 2, true, &mut None).is_err());
        // Into itself, and copies without stage 2.
        assert!(plan(&[copy(1000, 1050, 100, true)], 2, true, &mut None).is_err());
        assert!(plan(&[taken], 1, true, &mut None).is_err());
        assert!(plan(&[fill(1000, 4, true), copy(1000, 5000, 100, false)], 1, true, &mut None).is_err());
    }

    #[test]
    fn a_refresh_of_another_host_copy_is_seen() {
        let does = [Does::Refresh { shadow: 7, from: 100, to: 200 }];
        assert!(!elsewhere(&does, 7, 100, 200));
        assert!(elsewhere(&does, 8, 199, 300));
        assert!(!elsewhere(&does, 8, 200, 300));
    }

    /// A value written behind a flight, or a read-back, is in no host copy
    /// before it lands.
    #[test]
    fn what_no_host_copy_holds_is_refused() {
        let writers = [Writer { shadow: None, from: 100, to: 108, copy: false }];
        assert!(holder(&writers, 104, 120).is_err());
    }

    /// Texture copy-back ranges in a flight (`texture_back_ranges`) land in
    /// title memory rather than a host copy, so `writers()` marks them without
    /// a shadow and `holder()` refuses overlapping reads while leaving
    /// disjoint or touching reads unaffected.
    #[test]
    fn texture_copy_back_writers_refuse_overlapping_reads() {
        let flight = Flight {
            number: 1,
            fence: 1,
            lists: Vec::new(),
            order: Vec::new(),
            card: None,
            back: Vec::new(),
            own: None,
            texture_back_ranges: vec![(100, 108)],
        };
        let writers = writers(&[Late::Run(flight)]);
        assert_eq!(writers.len(), 1);
        assert!(writers[0].shadow.is_none());
        assert!(!writers[0].copy);
        assert_eq!((writers[0].from, writers[0].to), (100, 108));
        for (from, to) in [(96, 101), (100, 108), (104, 120)] {
            assert_eq!(
                holder(&writers, from, to).err(),
                Some("a read-back or a value behind a flight is written there"),
                "overlapping range [{from}, {to}) should be refused"
            );
        }
        for (from, to) in [(90, 100), (108, 120)] {
            assert_ne!(
                holder(&writers, from, to).err(),
                Some("a read-back or a value behind a flight is written there"),
                "disjoint range [{from}, {to}) should not hit the texture copy-back writer"
            );
        }
    }

    /// Reads are all before writes, between two buffers and within one, and
    /// the buffers are usable in the lists that follow in the same call.
    #[test]
    fn the_card_copies_what_is_recorded() {
        let Some(host) = host() else {
            return; // no host GPU
        };
        let (a, a_data) = host.shared_buffer(1 << 16).unwrap();
        let (b, b_data) = host.shared_buffer(1 << 16).unwrap();
        // SAFETY: mappings of 64 KiB.
        let (a_bytes, b_bytes) =
            unsafe { (std::slice::from_raw_parts_mut(a_data, 1 << 16), std::slice::from_raw_parts_mut(b_data, 1 << 16)) };
        for (i, (x, y)) in a_bytes.iter_mut().zip(b_bytes.iter_mut()).enumerate() {
            (*x, *y) = (i as u8 ^ (i >> 8) as u8, 0xee);
        }
        let was = a_bytes.to_vec();
        let own = host.own_list().unwrap();
        // Within `a` (there and back), from `a` to `b`, and a read alone.
        record(
            true,
            &own.list.list,
            &own.between,
            &[(&a, 33, 0, 100), (&a, 2000, 256, 100), (&a, 5001, 512, 7), (&a, 9, 1024, 4)],
            &[(&a, 2000, 0, 100), (&a, 33, 256, 100), (&b, 77, 512, 7)],
        );
        // A level behind it, of what it wrote.
        record(false, &own.list.list, &own.between, &[(&b, 77, 2048, 7)], &[(&b, 1077, 2048, 7)]);
        own.list.close();
        // A list behind it that uses the buffers in other ways.
        let next = host.new_list().unwrap();
        let (out, out_data) = host.shared_buffer(1 << 16).unwrap();
        // SAFETY: an open list; live buffers.
        unsafe {
            next.list.CopyBufferRegion(&out, 0, &own.between, 1024, 4);
            next.list.CopyBufferRegion(&b, 4096, &a, 33, 100);
        }
        next.close();
        host.execute(&[&own.list, &next]);
        // With the debug layer (`VICTORY_D3D12_DEBUG=1`, diagnostics).
        assert_eq!(host.log_messages(), None);
        assert_eq!(a_bytes[2000..2100], was[33..133]);
        assert_eq!(a_bytes[33..133], was[2000..2100]);
        assert_eq!(b_bytes[77..84], was[5001..5008]);
        assert_eq!(b_bytes[1077..1084], was[5001..5008]);
        assert_eq!(b_bytes[4096..4196], was[2000..2100]);
        // SAFETY: a mapping of 64 KiB.
        assert_eq!(unsafe { std::slice::from_raw_parts(out_data, 4) }, &was[9..13]);
        assert!(b_bytes[..77].iter().all(|&x| x == 0xee));
        a_bytes[2000..2100].copy_from_slice(&was[2000..2100]);
        a_bytes[33..133].copy_from_slice(&was[33..133]);
        assert_eq!(a_bytes, &was[..]);
        // As the landing does: its next use records anew.
        POOL.lock().unwrap().push(own);
        let own = host.own_list().unwrap();
        own.list.close();
    }

    /// The whole way, in a process of its own with the relay on:
    /// `VICTORY_RELAY=2 VICTORY_CARD_STEPS=2 cargo test -p victory-gpu --lib
    /// card_steps -- --ignored --test-threads=1`. A flight's list writes
    /// part of what the next submission's copy reads, the title wrote the
    /// rest; the copy is done on the card behind the flight, and a second
    /// one reads what the first wrote. Title memory ends as on the CPU.
    #[test]
    #[ignore = "wants VICTORY_RELAY=2 VICTORY_CARD_STEPS=2 and a process of its own"]
    fn a_copy_behind_a_flight_reads_what_the_flight_wrote() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
        const LEN: usize = 4 << 20;
        // Without it (`TEST_UNWATCHED=1`) the title's bytes cannot be put in
        // under a flight's writes: the flights that declared them are waited
        // for.
        let write_watch = if std::env::var_os("TEST_UNWATCHED").is_some() { 0 } else { 0x20_0000 };
        let Some(host) = host() else {
            return;
        };
        assert!(relay_exact() && stage() == 2, "set VICTORY_RELAY=2 VICTORY_CARD_STEPS=2");
        // Two title buffers: in the first a copy within it, from it into the
        // second, and back.
        let (title, start) = {
            // SAFETY: memory of our own, never freed.
            let base = unsafe { VirtualAlloc(std::ptr::null(), LEN, MEM_RESERVE | MEM_COMMIT | write_watch, PAGE_READWRITE) }.cast::<u8>();
            assert!(!base.is_null());
            // SAFETY: `LEN` bytes just committed.
            (unsafe { std::slice::from_raw_parts_mut(base, LEN) }, base as usize)
        };
        const HALF: usize = LEN / 2;
        crate::shadow::register(0x1111, start as u64, HALF as u64);
        crate::shadow::register(0x2222, (start + HALF) as u64, HALF as u64);
        let first = crate::shadow::find(start as u64, 64).unwrap();
        let (slow_from, slow_to) = (host.uav_buffer(64 << 20).unwrap(), host.uav_buffer(64 << 20).unwrap());
        let before = DONE[1].load(Ordering::Relaxed);
        let mut lists = Vec::new();
        for round in 0..8usize {
            // The title writes; then a "shader" of a flight writes 300 bytes
            // of what the copy reads, behind some work.
            let seed = |i: usize| (i * 7 + round * 13 + i / 251) as u8;
            for (i, b) in title[4000..6000].iter_mut().enumerate() {
                *b = seed(i);
            }
            let (upload, data) = host.upload_buffer(4096).unwrap();
            for i in 0..300 {
                // SAFETY: inside the mapping.
                unsafe { data.add(i).write(0x80 | (round * 31 + i) as u8) };
            }
            let a = host.new_list().unwrap();
            first.refresh(&a, start as u64, HALF as u64);
            first.gpu_writes(&a, start as u64, HALF as u64);
            // SAFETY: an open list; live buffers.
            unsafe {
                for _ in 0..24 {
                    a.list.CopyBufferRegion(&slow_to, 0, &slow_from, 0, 64 << 20);
                }
                a.list.CopyBufferRegion(&first.buffer, 4500, &upload, 0, 300);
                a.list
                    .ResourceBarrier(&[transition(&first.buffer, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON)]);
            }
            a.close();
            host.execute_own(&a);
            // With the flight in the air, beside what it writes.
            for (i, b) in title[5300..5400].iter_mut().enumerate() {
                *b = seed(1300 + i) ^ 0x5c;
            }
            // Thousands of runs of bytes of its own.
            for b in title[1_700_000..1_708_000].iter_mut().step_by(2) {
                *b = 1 + round as u8;
            }
            // Into the second buffer, and within the first.
            let b = host.new_list().unwrap();
            b.defer_copy((start + HALF + 100) as *mut u8, (start + 4001) as *const u8, 1500);
            b.defer_copy((start + 900_001) as *mut u8, (start + 4400) as *const u8, 777);
            b.defer_copy((start + HALF + 10_000) as *mut u8, (start + 1_700_000) as *const u8, 8000);
            // What depends on them: a copy of the first copy, upload memory
            // filled from that, and the first one's bytes copied over.
            b.defer_copy((start + 1_500_000) as *mut u8, (start + HALF + 600) as *const u8, 700);
            let (staging, staged) = b.upload(host, 4096).unwrap();
            b.defer_fill_buffer(&staging, staged, (start + 1_500_100) as *const u8, 500);
            b.defer_copy((start + HALF + 300) as *mut u8, (start + 8000) as *const u8, 64);
            b.close();
            host.execute_own(&b);
            // What the first wrote, read by the next submission.
            let c = host.new_list().unwrap();
            c.defer_copy((start + 1_200_000) as *mut u8, (start + HALF + 100) as *const u8, 1500);
            c.close();
            let number = host.execute_own(&c);
            settle_now(WHY_SYNC);
            assert!(FINISHED.load(Ordering::Acquire) >= number);
            let wanted: Vec<u8> = (0..2000)
                .map(|i| match i {
                    500..800 => 0x80 | (round * 31 + i - 500) as u8,
                    1300..1400 => seed(i) ^ 0x5c,
                    _ => seed(i),
                })
                .collect();
            assert!(title[4000..6000] == wanted[..], "what the flight wrote, round {round}");
            // (The copy over the first one's bytes came last.)
            let mut over = wanted[1..1501].to_vec();
            over[200..264].fill(0);
            assert!(title[HALF + 100..HALF + 1600] == over[..], "the copy to the other buffer, round {round}");
            assert!(title[1_500_000..1_500_700] == wanted[501..1201], "the copy behind a copy, round {round}");
            // SAFETY: the mapping, which the flight wrote and has landed.
            let staged = unsafe { std::slice::from_raw_parts(staged, 500) };
            assert!(staged == &wanted[601..1101], "the fill behind it, round {round}");
            assert!(title[900_001..900_778] == wanted[400..1177], "the copy within the buffer, round {round}");
            assert!(title[1_200_000..1_201_500] == over[..], "the copy of the copy, round {round}");
            assert!(title[HALF..HALF + 100].iter().all(|&x| x == 0));
            assert!(title[HALF + 1600..HALF + 10_000].iter().all(|&x| x == 0));
            assert!(title[HALF + 18_000..].iter().all(|&x| x == 0));
            let runs = &title[HALF + 10_000..HALF + 18_000];
            assert!(runs.chunks(2).all(|b| b == [1 + round as u8, 0]), "the runs, round {round}");
            a.keep(&upload);
            lists.extend([a, b, c]);
        }
        assert_eq!(host.log_messages(), None);
        let done = DONE[1].load(Ordering::Relaxed) - before;
        eprintln!("{done} of 48 copies were done on the card, {} submissions waited", DONE[3].load(Ordering::Relaxed));
        eprintln!("settles for title memory under shader writes: {}", LATE_STATS[WHY_STALE].load(Ordering::Relaxed));
        assert!(done >= 20, "only {done} copies were done on the card");
    }
}
