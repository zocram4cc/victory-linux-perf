//! Host descriptor heaps behind the title's shader-visible heaps (decision
//! 0007, step 3). Translated shaders index the heaps directly at the
//! title's descriptor positions, so each host heap keeps the title heap's
//! order: slot `i` is made from what the title wrote at slot `i`, when it
//! writes it (view creation and descriptor copies) and again when a draw
//! reads it through a table and finds the slot changed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::{VIEW_TAG, View, view_at};
use crate::host::host;

/// Power-of-two stripes for slow-path descriptor slot locks and `kept` storage.
const SLOT_LOCK_STRIPES: usize = 256;
const SLOT_LOCK_SHIFT: usize = 8;
const _: () = assert!(SLOT_LOCK_STRIPES == 1 << SLOT_LOCK_SHIFT);

/// [`HostHeap::slot_flags`]: title slot `index` is in the queue of textures
/// picked by index ([`TextureQueue::slots`]).
const SLOT_IN_TEXTURES: u8 = 1 << 0;
/// [`HostHeap::slot_flags`]: it holds a placed texture view in
/// [`HostHeap::placed`] ([`Spans::by_slot`]).
const SLOT_IN_PLACED: u8 = 1 << 1;
/// [`HostHeap::slot_flags`]: it holds a buffer view in [`HostHeap::buffers`].
const SLOT_IN_BUFFERS: u8 = 1 << 2;
/// [`HostHeap::slot_flags`]: it holds an acceleration structure view in
/// [`HostHeap::acceleration`].
const SLOT_IN_ACCELERATION: u8 = 1 << 3;

#[repr(align(64))]
struct StripeLock<T>(Mutex<T>);

pub(crate) struct HostHeap {
    pub(crate) heap: ID3D12DescriptorHeap,
    cpu: usize,
    step: u32,
    /// Slots mirroring the title's; view heaps have `dynamic` more.
    count: u32,
    /// Dynamic slots past `count`: [`DYNAMIC`] in view heaps, none in
    /// sampler heaps. (Upstream falls back to 2^16 on a capped host heap;
    /// ours keeps 2^18 there: 2^16 and 2^17 both ran out in this title.)
    dynamic: u32,
    sampler: bool,
    /// The record (view index and generation) each slot was last made from.
    made: Box<[AtomicU64]>,
    /// Striped locks guarding slow-path descriptor updates for title slots.
    slot_locks: Box<[StripeLock<()>]>,
    /// A fingerprint of the record each slot held when the pass over
    /// records copied in as bytes last looked at it
    /// ([`HostHeap::mirror_copied`]; 0: never looked at). Made on first use.
    copied: OnceLock<Box<[AtomicU64]>>,
    /// The host resource each slot views, held while the slot does: shaders
    /// index heaps directly, so a slot the title has not rewritten since it
    /// released the resource may still be read, and a freed host resource
    /// read there is a GPU page fault (the console reads stale memory).
    kept: Box<[StripeLock<Vec<Option<ID3D12Resource>>>]>,
    /// Where the search for free dynamic slots starts, and which are taken
    /// (by lists not yet reset).
    next: Mutex<(u32, Vec<bool>)>,
    /// How many dynamic slots are free (changed under `next`'s lock): with
    /// fewer than a table needs, the search is not made. Once the world
    /// had taken them all, every later table of the frame searched every
    /// slot before it was refused.
    free: std::sync::atomic::AtomicU32,
    /// The shortest table that found no run of free slots since slots were
    /// last given back (changed under `next`'s lock; `u32::MAX`: none).
    /// Until something is given back the free runs only shrink, so a table
    /// of that length or longer is refused without the search. Enough
    /// slots were free, but in pieces, and every such table walked the
    /// whole ring before it was refused: 0.33 ms each with 131,072 slots,
    /// 1.0 s of the recording threads in a frame that refused 3,143 (run
    /// 45), under the lock all of them need. (A free run that lies across
    /// the place the search starts from is seen by neither.)
    too_long: std::sync::atomic::AtomicU32,
    /// View heaps: the bytes per element of the buffer view at each title
    /// slot, for shaders that pick a buffer by an index they compute
    /// (`victory_shader::emit::SLOT_STRIDES`): an upload buffer of
    /// `SLOT_STRIDES_COUNT` dwords, its GPU address and its mapping.
    strides: Option<(ID3D12Resource, u64, usize)>,
    /// Title slots holding buffer views. Such a view gets its host slot
    /// (and its memory a host copy) only when something reads it: a draw
    /// through a table, or [`HostHeap::buffer_slots`] for shaders that pick
    /// buffers from the whole heap.
    buffers: Mutex<std::collections::BTreeSet<u32>>,
    /// Guest addresses, in the same slots as the title's shader-visible
    /// views (`victory_shader::emit::SLOT_VIEW_ADDRESSES`): an upload
    /// buffer and its mapping, made when a shader first asks for it (from
    /// dexnom/VIctory 5f4c3df4).
    addresses: std::sync::OnceLock<(ID3D12Resource, usize)>,
    /// Title slots holding a ray-tracing acceleration structure view, and
    /// its guest address.
    acceleration: Mutex<std::collections::BTreeMap<u32, u64>>,
    /// Of those and of the slots that left the set: the ones written since
    /// [`HostHeap::buffer_changes`] last took them.
    changed: Mutex<std::collections::HashSet<u32>>,
    /// Title slots holding a view of a texture the title may fill itself (a
    /// placed texture, a texture object), with the frame from which they
    /// are due: such a texture is uploaded when a draw reads it through a
    /// table; one a shader picks by a computed index is brought up to date
    /// from here ([`HostHeap::texture_slots_due`]).
    textures: Mutex<TextureQueue>,
    /// Per title slot, the frame (plus one; 0: never) in which a draw last
    /// read it through a table, or made a table it lies in (with
    /// [`TABLED_READ`] set if the draw read it). The draw that
    /// reads such a slot brings its texture up to date itself and the slot
    /// then leaves `textures`; a write of a slot that was a table's within
    /// the last [`TABLED_FRAMES`] frames is due only [`TABLED_GRACE`]
    /// frames later, by when its draw has read it. A title that builds a
    /// table per draw writes thousands of texture views a frame into slots
    /// only tables read (the title: some 2,500 a frame in its
    /// opening cutscene, a few hundred textures over and over), and every
    /// one of them stood in the queue of textures picked by index, ahead
    /// of or behind the ones shaders do pick by index.
    tabled: Vec<std::sync::atomic::AtomicU32>,
    /// Per title slot, bits saying which sets the slot may be in without
    /// taking their lock to ask ([`SLOT_IN_TEXTURES`], [`SLOT_IN_PLACED`],
    /// [`SLOT_IN_BUFFERS`], [`SLOT_IN_ACCELERATION`]). The passes write nearly
    /// every slot a frame, and most writes take a slot out of sets it is not
    /// in, which took each set's lock for nothing. A bit that is not set says
    /// the slot is not in the set (the slot may be in it with the bit set,
    /// which only costs the lock). Dynamic slots and sampler heaps have none.
    slot_flags: Vec<std::sync::atomic::AtomicU8>,
    /// The title memory each placed texture view of this heap describes,
    /// by slot and by address ([`Spans`]): which slots a newer view of
    /// other memory there makes stale, and which to look at again when the
    /// runtime maps, unmaps or writes memory ([`remapped`]).
    placed: std::sync::Arc<Mutex<Spans>>,
}

/// The spans of every view heap made ([`HostHeap::placed`]), for the
/// texture code, which has no heap at hand ([`next_start`],
/// [`first_viewed`]).
static PLACED_SPANS: Mutex<Vec<std::sync::Weak<Mutex<Spans>>>> = Mutex::new(Vec::new());

/// Where the next texture placed inside `[start, end)` starts, past the
/// first bytes of the one at `start` ([`HEAD`]): what is there of the
/// texture at `start` ends there, whatever its full size. `None`: no view
/// in a slot shaders pick by index describes memory in that range.
pub(crate) fn next_start(start: u64, end: u64) -> Option<u64> {
    let heaps = PLACED_SPANS.lock().unwrap_or_else(|e| e.into_inner());
    heaps
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .filter_map(|spans| spans.lock().unwrap_or_else(|e| e.into_inner()).next_start(start, end))
        .min()
}

/// The lowest level that a view of the placed texture at `start` shows,
/// over the slots shaders pick by index that hold one now. The title
/// views a texture from the first level it holds. `None`: no such slot.
pub(crate) fn first_viewed(start: u64) -> Option<u32> {
    let heaps = PLACED_SPANS.lock().unwrap_or_else(|e| e.into_inner());
    heaps
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .filter_map(|spans| spans.lock().unwrap_or_else(|e| e.into_inner()).first_viewed(start))
        .min()
}

/// Levels on demand (`crate::tiles`): where the placed textures start
/// whose first level with memory changed since a texture pass last took
/// them ([`HostHeap::texture_views_remade`]).
static RESHOWN: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The first level with memory of the placed texture at `start` changed:
/// its slots are made again at the next texture pass.
pub(crate) fn levels_changed(start: u64) {
    RESHOWN.lock().unwrap_or_else(|e| e.into_inner()).push(start);
}

/// Levels on demand (`crate::tiles`): where the placed textures start
/// whose last upload left a held level without tiles (the card had no
/// room, or no heap could be had), and when (`crate::host::moment`). A
/// texture pass makes their slots due again once the card has room
/// ([`HostHeap::texture_slots_waiting`]).
static WAITING: Mutex<std::collections::BTreeMap<u64, u64>> = Mutex::new(std::collections::BTreeMap::new());

/// The last upload of the placed texture at `start` left a held level
/// without tiles. A texture picked by index is uploaded once and then
/// only when its memory is found changed, and its staging buffer goes
/// with the upload: without this the level stayed empty (the texture
/// blurred, or black if no level had tiles) for as long as the title left
/// its memory alone, where a texture refused for want of room is tried
/// again.
pub(crate) fn levels_waiting(start: u64) {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).insert(start, crate::host::moment());
}

/// Title memory by the placed texture views that describe it: per span of
/// memory (its start, its end at full size) the title slots holding a
/// view of a texture that takes exactly that span. A console title need
/// not clear a descriptor when it streams a texture out, so a newer view
/// that cannot be there together with an older one is all that says the
/// older one is stale ([`Rule`] says when that is). Views of one span with
/// different descriptions (the formats of one family, say) are one
/// texture's.
///
/// Spans may overlap: a texture's smallest levels lie at the start of its
/// memory, and a title that holds only those puts the next texture where
/// the larger ones would be (the title: three 512x256 textures
/// of 0x2c000 bytes each whose starts are 0x2000 and 0x4000 apart, all
/// three read by the same frame's draws, run 188). What is there of a
/// texture ends where the next one starts.
/// No host heap is needed, so it is tested without one.
#[derive(Default)]
pub(crate) struct Spans {
    /// When a newer view makes an older one stale.
    rule: Rule,
    by_start: std::collections::BTreeMap<u64, (u64, std::collections::BTreeSet<u32>)>,
    by_slot: std::collections::HashMap<u32, u64>,
    /// The level each slot's view starts at ([`Spans::shows`]).
    first: std::collections::HashMap<u32, u32>,
    /// The bytes a placed texture takes, by resource description: worked
    /// out once for each (a title writes the views of a few thousand
    /// descriptions, thousands of times a frame).
    sizes: std::collections::HashMap<[u32; 14], Option<u64>>,
}

/// When a view the title writes makes an older view of other memory stale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Rule {
    /// When it starts where the older one starts: at the same address
    /// with another length, or within the older one's first [`HEAD`]
    /// bytes (or the older within its own). No two textures share the
    /// block of their smallest levels. A view that only reaches into the
    /// address range of an older texture's larger levels says nothing.
    #[default]
    Start,
    /// Whenever their spans at full size overlap (the rule until run
    /// 188, where it made null the slots of textures packed as above:
    /// each view written took its neighbours out of the queue and off the
    /// card, for good). `VICTORY_STALE_TEXTURES=overlap`.
    Overlap,
    /// Never; a view that would be stale under [`Rule::Overlap`] is left
    /// out of the spans instead. `VICTORY_STALE_TEXTURES=0`, as before.
    Never,
}

/// The first bytes of a placed texture that are its own under
/// [`Rule::Start`]: the 4 KiB block its smallest levels share. (Of 129
/// pairs of overlapping neighbours in the listings of runs 188 and 189,
/// none starts closer than 0x2000 to the one before it.)
const HEAD: u64 = 4096;

impl Spans {
    /// The bytes of title memory a placed texture with the resource
    /// description `desc` takes (`crate::resource::placed_bytes`).
    fn bytes(&mut self, desc: &[u32; 14]) -> Option<u64> {
        if let Some(&known) = self.sizes.get(desc) {
            return known;
        }
        let bytes = crate::resource::placed_bytes(desc);
        self.sizes.insert(*desc, bytes);
        bytes
    }

    /// The starts of the spans that overlap `[start, end)`, each taken to
    /// end where the next one starts: the last span that starts before
    /// `start`, if it reaches past it at full size, and every span that
    /// starts inside.
    fn overlapping(&self, start: u64, end: u64) -> Vec<u64> {
        let before = self
            .by_start
            .range(..start)
            .next_back()
            .filter(|(_, span)| span.0 > start)
            .map(|(at, _)| *at);
        before.into_iter().chain(self.by_start.range(start..end).map(|(at, _)| *at)).collect()
    }

    /// Slot `index` holds a view of a texture in `[start, end)`. `newer`:
    /// the title wrote it now, so the slots whose views cannot be there
    /// with it ([`Rule`]) are stale: they are returned, and forgotten.
    /// Otherwise (a draw made the slot from what it held), and under
    /// [`Rule::Never`], a view that clashes with other spans is left out.
    fn note(&mut self, index: u32, start: u64, end: u64, newer: bool) -> Vec<u32> {
        let same = |span: Option<&(u64, std::collections::BTreeSet<u32>)>| span.is_some_and(|s| s.0 == end);
        if self.by_slot.get(&index) == Some(&start) && same(self.by_start.get(&start)) {
            return Vec::new();
        }
        self.forget(index);
        if end <= start {
            return Vec::new();
        }
        let rule = self.rule;
        // The spans the view cannot be there with.
        let clashing: Vec<u64> = match rule {
            Rule::Start => self
                .by_start
                .range(start.saturating_sub(HEAD - 1)..start.saturating_add(HEAD))
                .map(|(at, _)| *at)
                .collect(),
            Rule::Overlap | Rule::Never => self.overlapping(start, end),
        };
        let mut stale = Vec::new();
        for at in clashing {
            if at == start && same(self.by_start.get(&at)) {
                continue;
            }
            if !newer || rule == Rule::Never {
                return stale;
            }
            if let Some((old_end, slots)) = self.by_start.remove(&at) {
                // The first of them are said with both spans, to tell a
                // replacement from a neighbour in one launch's log.
                static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 48 {
                    tracing::info!(
                        slot = index,
                        start = format_args!("{start:#x}"),
                        end = format_args!("{end:#x}"),
                        stale_start = format_args!("{at:#x}"),
                        stale_end = format_args!("{old_end:#x}"),
                        stale_slots = slots.len(),
                        rule = ?rule,
                        "a placed texture view makes older views of other memory stale"
                    );
                }
                for slot in slots {
                    self.by_slot.remove(&slot);
                    stale.push(slot);
                }
            }
        }
        self.by_start.entry(start).or_insert_with(|| (end, Default::default())).1.insert(index);
        self.by_slot.insert(index, start);
        stale
    }

    /// The slots (`!= index`) that [`Spans::note`] with `newer == true` would
    /// mark stale for a view in `[start, end)`, without mutating `self`.
    fn clashing_slots(&self, index: u32, start: u64, end: u64) -> Vec<u32> {
        let same = |span: Option<&(u64, std::collections::BTreeSet<u32>)>| {
            span.is_some_and(|s| s.0 == end)
        };
        if (self.by_slot.get(&index) == Some(&start) && same(self.by_start.get(&start)))
            || end <= start
            || self.rule == Rule::Never
        {
            return Vec::new();
        }
        let clashing: Vec<u64> = match self.rule {
            Rule::Start => self
                .by_start
                .range(start.saturating_sub(HEAD - 1)..start.saturating_add(HEAD))
                .map(|(at, _)| *at)
                .collect(),
            Rule::Overlap => self.overlapping(start, end),
            Rule::Never => return Vec::new(),
        };
        let mut stale = Vec::new();
        for at in clashing {
            if at == start && same(self.by_start.get(&at)) {
                continue;
            }
            if let Some((_, slots)) = self.by_start.get(&at) {
                for &slot in slots {
                    if slot != index {
                        stale.push(slot);
                    }
                }
            }
        }
        stale
    }

    /// The view in slot `index` starts at level `first` of its texture.
    fn shows(&mut self, index: u32, first: u32) {
        self.first.insert(index, first);
    }

    /// The lowest level a view of the texture at `start` starts at.
    fn first_viewed(&self, start: u64) -> Option<u32> {
        self.by_start.get(&start)?.1.iter().filter_map(|slot| self.first.get(slot)).min().copied()
    }

    /// The first span that starts inside `[start, end)` past the first
    /// [`HEAD`] bytes of it.
    fn next_start(&self, start: u64, end: u64) -> Option<u64> {
        self.by_start.range(start.saturating_add(HEAD)..end).next().map(|(at, _)| *at)
    }

    /// The slots that hold a view of the texture at `start`.
    fn slots_at(&self, start: u64) -> Vec<u32> {
        self.by_start.get(&start).map(|span| span.1.iter().copied().collect()).unwrap_or_default()
    }

    /// Slot `index` holds no placed texture view any more.
    fn forget(&mut self, index: u32) {
        self.first.remove(&index);
        let Some(start) = self.by_slot.remove(&index) else {
            return;
        };
        if let Some(span) = self.by_start.get_mut(&start) {
            span.1.remove(&index);
            if span.1.is_empty() {
                self.by_start.remove(&start);
            }
        }
    }

    /// The slots whose texture's memory overlaps `[start, end)`.
    fn slots_in(&self, start: u64, end: u64) -> Vec<u32> {
        self.overlapping(start, end)
            .iter()
            .filter_map(|at| self.by_start.get(at))
            .flat_map(|span| span.1.iter().copied())
            .collect()
    }

    /// The slots of the textures that hold bytes of `[start, end)`, for a
    /// write there: as [`Spans::slots_in`], but when a texture starts at
    /// `start` itself the one before it is left out (it ends where that
    /// one starts, and a request that fills a texture begins at its first
    /// byte: `slots_in` would name its neighbour with every such write).
    fn slots_holding(&self, start: u64, end: u64) -> Vec<u32> {
        if end <= start {
            return Vec::new();
        }
        let before = if self.by_start.contains_key(&start) {
            None
        } else {
            self.by_start.range(..start).next_back().filter(|(_, span)| span.0 > start).map(|(_, span)| span)
        };
        before
            .into_iter()
            .chain(self.by_start.range(start..end).map(|(_, span)| span))
            .flat_map(|span| span.1.iter().copied())
            .collect()
    }
}

/// Title memory written behind the guards' back whose bytes are there
/// (DirectStorage's reads into physical pages, at the addresses the title
/// has them mapped; `crate::watch::title_memory_written`), since a texture
/// pass last took the ranges ([`page_written`]). Kept apart from
/// [`REMAPPED`] so that what such writes do is counted by itself.
static PAGE_WRITTEN: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// Since the start: such ranges kept for a pass; ranges dropped because no
/// pass took them; slots of placed textures that held bytes of them when a
/// pass took them; of those, the slots whose texture had been uploaded
/// before the write (out of the queue, or in it after an upload): they are
/// the ones that kept the bytes from before, and the ones made due when
/// `VICTORY_PAGE_WRITES` asks for it.
pub(crate) static PAGE_WRITES: [std::sync::atomic::AtomicU64; 4] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// Frames from the pass that takes a written range to the look at its
/// slots: the levels of one texture arrive in a few requests, and a look
/// two frames on takes those of the next frame with it.
const PAGE_WRITE_AFTER: u64 = 2;

/// The bytes of a write the guards do not see are in title memory `[at,
/// at + len)`: the placed textures there changed with no descriptor
/// written and nothing mapped. Kept for the next texture pass
/// ([`HostHeap::texture_slots_page_written`]), as [`remapped`] keeps its
/// ranges; locks one mutex and calls nothing.
pub(crate) fn page_written(at: usize, len: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    /// Ranges kept until a pass takes them.
    const MOST: usize = 1 << 15;
    let (start, end) = (at as u64, at.saturating_add(len) as u64);
    if end <= SPANNED[0].load(Relaxed) || start >= SPANNED[1].load(Relaxed) {
        return;
    }
    let mut ranges = PAGE_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    PAGE_WRITES[0].fetch_add(1, Relaxed);
    if let Some(last) = ranges.last_mut()
        && last.1 == start
    {
        last.1 = end;
        return;
    }
    if ranges.len() >= MOST {
        PAGE_WRITES[1].fetch_add(ranges.len() as u64, Relaxed);
        ranges.clear();
    }
    ranges.push((start, end));
}

/// Title memory the runtime mapped, unmapped or is about to write since a
/// texture pass last took the ranges ([`remapped`]).
static REMAPPED: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// More ranges came than [`REMAPPED`] keeps: the next texture pass looks
/// at every placed slot once instead ([`HostHeap::texture_slots_remapped`]).
static OVERFLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The lowest start and the highest end of any span of [`Spans`] so far
/// (the start is `u64::MAX` until there is one): what the runtime does to
/// memory outside is none of the texture pass's business.
static SPANNED: [std::sync::atomic::AtomicU64; 2] = [
    std::sync::atomic::AtomicU64::new(u64::MAX),
    std::sync::atomic::AtomicU64::new(0),
];

/// Since the start: slots whose view a newer view of other memory there
/// made stale; ranges of memory noted as mapped, unmapped or written;
/// ranges dropped because no pass took them; slots made due by such
/// ranges; and made slots whose memory was found unmapped (made null).
pub(crate) static STALE: [std::sync::atomic::AtomicU64; 5] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 5];

/// When a view the title writes makes older views of other memory stale
/// ([`Rule`]): where it starts in the block of their smallest levels, by
/// default; `VICTORY_STALE_TEXTURES=0`: never, as before;
/// `VICTORY_STALE_TEXTURES=overlap`: wherever their spans overlap, the rule
/// until run 188.
fn stale_rule() -> Rule {
    static RULE: std::sync::OnceLock<Rule> = std::sync::OnceLock::new();
    *RULE.get_or_init(|| match std::env::var("VICTORY_STALE_TEXTURES").as_deref() {
        Ok("0") => Rule::Never,
        Ok("overlap") => Rule::Overlap,
        _ => Rule::Start,
    })
}

/// The runtime mapped or unmapped title memory `[at, at + len)`, or is
/// about to write it through the kernel (`crate::watch`): the placed
/// textures there may have arrived, gone, or changed with no descriptor
/// written. The range is kept for the next texture pass
/// ([`HostHeap::texture_slots_remapped`]). Called with the runtime's own
/// locks held: this locks one mutex of its own and calls nothing.
pub(crate) fn remapped(at: usize, len: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    /// Ranges kept until a pass takes them.
    const MOST: usize = 1 << 20;
    let (start, end) = (at as u64, at.saturating_add(len) as u64);
    if end <= SPANNED[0].load(Relaxed) || start >= SPANNED[1].load(Relaxed) {
        return;
    }
    let mut ranges = REMAPPED.lock().unwrap_or_else(|e| e.into_inner());
    STALE[1].fetch_add(1, Relaxed);
    // Pages are mapped one after the other.
    if let Some(last) = ranges.last_mut()
        && last.1 == start
    {
        last.1 = end;
        return;
    }
    if ranges.len() >= MOST {
        // No pass runs (no shader that picks textures by index is bound):
        // the ranges go, and the next pass looks at every placed slot. Lost,
        // a texture streamed in there stayed stale or null for the run.
        STALE[2].fetch_add(ranges.len() as u64, Relaxed);
        OVERFLOWED.store(true, Relaxed);
        ranges.clear();
    }
    ranges.push((start, end));
}

/// Frames for which a slot a draw read through a table counts as a table's
/// slot ([`HostHeap::tabled`]).
const TABLED_FRAMES: u64 = 240;

/// Frames a write of a table's slot waits in the queue for the draw that
/// reads it, before the slot is taken for one shaders pick by index.
/// Frames as `crate::frame::current` counts them: in its opening cutscene
/// The title begins two or three of them per frame it presents
/// (435 begun in the 353 s of run 52's, for some 140 presented).
const TABLED_GRACE: u64 = 8;

/// In [`HostHeap::tabled`]: a draw read the slot in that frame.
const TABLED_READ: u32 = 1 << 31;

/// Since the start: writes of texture views into slots that count as a
/// table's (due later), and slots that left the queue because a draw read
/// them through a table.
pub(crate) static TABLED_WRITES: [std::sync::atomic::AtomicU64; 2] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Whether slots draws read through tables leave the queue of textures
/// picked by index; `VICTORY_TABLED_TEXTURES=1` keeps them in it as
/// before.
fn tabled_out() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TABLED_TEXTURES").is_none_or(|v| v != "1"))
}

/// What the queue of textures picked by index knows of a title slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Waiting {
    /// The frame from which the slot is due.
    due: u64,
    /// The frame it was written in.
    written: u64,
    /// Which write of the queue put it here: the pass hands it back with
    /// what it did, so a slot written again meanwhile stays as written.
    write: u64,
    /// Looks in a row that found nothing to upload (its memory not mapped,
    /// or no more of it than at the last upload).
    looks: u8,
    /// Bytes of its memory that were mapped when it was last uploaded
    /// without all of it there; 0: not uploaded yet.
    mapped: u64,
    /// For a slot uploaded whole and looked at again
    /// ([`TextureQueue::recheck`]): where its memory is (address, bytes),
    /// the fingerprint it had at the upload, how many looks found it
    /// unchanged, and the frame of the upload. A fingerprint of 0: none.
    checked: (u64, u64, u64, u8, u64),
}

thread_local! {
    /// This thread is inside [`HostHeap::publish`].
    static PUBLISHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The queue of title slots whose texture view waits to be brought up to
/// date for shaders that pick textures by index: which are due, and in
/// which order (no host heap needed, so it is tested without one).
#[derive(Default)]
pub(crate) struct TextureQueue {
    slots: std::collections::BTreeMap<u32, Waiting>,
    writes: u64,
    /// Slots held back because the card had no room for the host copy
    /// they need ([`TextureQueue::hold`]), each with the write it was
    /// held as.
    held: std::collections::BTreeMap<u32, u64>,
}

impl TextureQueue {
    /// A texture view was written into slot `index` in `frame`, due from
    /// frame `due`. `seen`: it was not written, a draw or the pass made
    /// the host slot from a view the queue may hold already, which then
    /// stays as it is.
    fn written(&mut self, index: u32, frame: u64, due: u64, seen: bool) {
        if seen && self.slots.contains_key(&index) {
            return;
        }
        self.writes += 1;
        let write = self.writes;
        self.slots.insert(index, Waiting { due, written: frame, write, looks: 0, mapped: 0, checked: (0, 0, 0, 0, 0) });
    }

    fn remove(&mut self, index: u32) {
        self.slots.remove(&index);
    }

    /// Up to `max` slots due at `frame`, each with its write number, in
    /// the order to serve them. Alternate retries with first looks so a
    /// backlog of unmapped or partial textures cannot use the whole pass.
    /// Among first looks, take the newest and oldest writes in turns (from
    /// dexnom/VIctory 5f4c3df4: retries first starved new textures).
    fn due(&self, frame: u64, max: usize) -> Vec<(u32, u64)> {
        let mut again = Vec::new();
        let mut fresh = Vec::new();
        for (&index, w) in self.slots.iter().filter(|(_, w)| w.due <= frame) {
            let class = if w.looks > 0 || w.mapped > 0 { &mut again } else { &mut fresh };
            class.push((index, w.write));
        }
        again.sort_unstable_by_key(|&(_, write)| write);
        fresh.sort_unstable_by_key(|&(_, write)| write);
        let count = max.min(again.len() + fresh.len());
        let mut fresh = fresh.into_iter();
        // A time-limited pass may stop after one slot. Rotate its first
        // choice across frames as well as rotating within each pass.
        let mut retry = frame & 1 == 0;
        let mut newest = again.is_empty() || frame & 2 == 0;
        let mut order = Vec::with_capacity(count);
        while order.len() < count {
            if !again.is_empty() && (retry || fresh.len() == 0) {
                order.push(again.pop().unwrap());
            } else {
                let next = if newest { fresh.next_back() } else { fresh.next() };
                order.push(next.unwrap());
                newest = !newest;
            }
            retry = !retry;
        }
        order
    }

    /// The entry of `index` if it is still the write the pass was handed.
    fn same(&mut self, index: u32, write: u64) -> Option<&mut Waiting> {
        self.slots.get_mut(&index).filter(|w| w.write == write)
    }

    /// Slot `index` is up to date: out of the queue until written again.
    fn done(&mut self, index: u32, write: u64) {
        if self.same(index, write).is_some() {
            self.slots.remove(&index);
        }
    }

    /// Nothing to upload at slot `index` now: looked at again after 1, 2,
    /// 4, ... frames, at most `most`.
    fn later(&mut self, index: u32, write: u64, frame: u64, most: u64) {
        if let Some(w) = self.same(index, write) {
            w.due = frame + (1u64 << w.looks.min(16)).min(most.max(1));
            w.looks = w.looks.saturating_add(1);
        }
    }

    /// The card has no room for the host copy slot `index` needs: it is
    /// not handed out again before frame `due`, or before the card has
    /// room again ([`TextureQueue::room`]). Nothing else of it changes: it
    /// is not a slot "waiting for its memory", and when it is due it has
    /// its old place among the slots never looked at. (It stayed due, so
    /// every pass on a full card looked at the same few hundred slots
    /// again and stopped there, and the slots behind them were never
    /// reached: the ones whose host copy exists and only wants bringing up
    /// to date, and the ones whose memory went, which is what lets host
    /// copies go and makes room.)
    fn hold(&mut self, index: u32, write: u64, due: u64) {
        if let Some(w) = self.same(index, write) {
            w.due = w.due.max(due);
            self.held.insert(index, write);
        }
    }

    /// The card has room again at `frame`: the slots held back for lack of
    /// it are due now, as if they had never been held. Returns how many.
    fn room(&mut self, frame: u64) -> usize {
        let held = std::mem::take(&mut self.held);
        let mut back = 0;
        for (index, write) in held {
            if let Some(w) = self.same(index, write) {
                w.due = w.due.min(frame);
                back += 1;
            }
        }
        back
    }

    /// Slot `index` was uploaded with `mapped` bytes of its memory there,
    /// not all of it: looked at again from frame `due`.
    fn partly(&mut self, index: u32, write: u64, due: u64, mapped: u64) {
        if let Some(w) = self.same(index, write) {
            (w.due, w.looks, w.mapped) = (due, 0, mapped.max(1));
        }
    }

    /// Slot `index` was uploaded whole from memory `at` (address, bytes)
    /// with fingerprint `print`: looked at again from frame `due`, for a
    /// title that fills a texture's memory after it wrote the view (the
    /// `looks`-th such look; `uploaded` is the upload's frame).
    #[allow(clippy::too_many_arguments)]
    fn recheck(&mut self, index: u32, write: u64, due: u64, at: (u64, u64), print: u64, looks: u8, uploaded: u64) {
        if let Some(w) = self.same(index, write) {
            (w.due, w.looks, w.mapped) = (due, 1, 0);
            w.checked = (at.0, at.1, print, looks, uploaded);
        }
    }

    /// What [`TextureQueue::recheck`] noted of slot `index`, if it is
    /// waiting for such a look.
    fn checked(&self, index: u32) -> Option<(u64, u64, u64, u8, u64)> {
        self.slots.get(&index).map(|w| w.checked).filter(|c| c.2 != 0)
    }

    /// The memory slot `index`'s texture lies in was mapped, unmapped or
    /// written in `frame`: the slot is looked at from frame `due` as one
    /// written now, whatever was known of it (a pass at work on it does
    /// not take it out).
    fn changed(&mut self, index: u32, frame: u64, due: u64) {
        self.writes += 1;
        let write = self.writes;
        let due = self.slots.get(&index).map_or(due, |w| w.due.min(due));
        self.slots.insert(index, Waiting { due, written: frame, write, looks: 0, mapped: 0, checked: (0, 0, 0, 0, 0) });
    }

    /// Bytes of slot `index`'s memory that were mapped at its last upload
    /// (0: not uploaded, or all of it was).
    fn mapped(&self, index: u32) -> u64 {
        self.slots.get(&index).map_or(0, |w| w.mapped)
    }

    /// For the frame line: slots in the queue; of them due at `frame`;
    /// waiting for their memory (looked at, never uploaded); uploaded in
    /// part; and the frames the oldest due one has waited since its write.
    fn census(&self, frame: u64) -> [u64; 5] {
        let mut c = [self.slots.len() as u64, 0, 0, 0, 0];
        for w in self.slots.values() {
            if w.mapped > 0 {
                c[3] += 1;
            } else if w.looks > 0 {
                c[2] += 1;
            }
            if w.due <= frame {
                c[1] += 1;
                c[4] = c[4].max(frame.saturating_sub(w.written));
            }
        }
        c
    }
}

/// Whether `view` is of a texture the title may fill itself: a texture it
/// placed in memory, or a texture object.
pub(crate) fn is_texture_view(view: &View) -> bool {
    match view {
        View::Placed(false, resource, _) => resource[0] != 1,
        View::Shader(r, _) => crate::resource::buffer_bytes(*r as crate::object::This).is_none(),
        _ => false,
    }
}

/// Buffer views left null because the host would refuse them
/// ([`buffer_view_ok`]), since the start.
pub(crate) static REFUSED_VIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether the host device is gone, and why (the first time it is asked
/// after that): for passes that make many views, to name the one that did
/// it.
pub(crate) fn device_removed() -> Option<String> {
    let host = host()?;
    // SAFETY: a plain query.
    let reason = unsafe { host.device.GetDeviceRemovedReason() };
    reason.err().map(|e| e.to_string())
}

/// Writes of title view-heap slots with a placed texture view and with a
/// view of a texture object, since the start (diagnostic counts).
pub(crate) static PLACED_TEXTURE_SLOTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static OBJECT_TEXTURE_SLOTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bytes per element of a buffer view, as the console's descriptor holds
/// them for indexed reads (structured: the stride; raw: 1, the index is the
/// byte; typed: the format's); `None` if `view` is not a buffer view.
fn view_stride(view: &View) -> Option<u32> {
    const BUFFER: u32 = 1;
    let (words, unordered): (&[u32; 10], bool) = match view {
        View::Shader(_, Some(d)) if d[1] == BUFFER => (d, false),
        View::Unordered(_, _, Some(d)) if d[1] == BUFFER => (d, true),
        View::Placed(unordered, resource, d) if resource[0] == BUFFER => {
            (d[..10].try_into().ok()?, *unordered)
        }
        _ => return None,
    };
    // SAFETY: buffer views' union members.
    let (stride, raw, format) = unsafe {
        if unordered {
            let d = uav_desc(words);
            let b = d.Anonymous.Buffer;
            (b.StructureByteStride, b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0, d.Format)
        } else {
            let d = srv_desc(words);
            let b = d.Anonymous.Buffer;
            (b.StructureByteStride, b.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0, d.Format)
        }
    };
    Some(if stride > 0 {
        stride
    } else if raw {
        1
    } else {
        crate::resource::element(format.0 as u32).map_or(0, |(bytes, _)| bytes as u32)
    })
}

/// Host heaps' GPU ranges (start, bytes, bytes per slot, sampler), for
/// device-removed reports: a shader indexing a heap past its end faults.
static GPU_RANGES: Mutex<Vec<(u64, u64, u64, bool)>> = Mutex::new(Vec::new());

/// For device-removed reports: where host GPU address `va` lies against
/// the host descriptor heaps.
pub(crate) fn near_heaps(va: u64) -> String {
    let ranges = GPU_RANGES.lock().unwrap_or_else(|e| e.into_inner());
    let mut text = String::new();
    for &(start, bytes, step, sampler) in ranges.iter() {
        let kind = if sampler { "sampler" } else { "view" };
        let place = if va < start {
            format!("{:#x} below", start - va)
        } else if va < start + bytes {
            format!("inside, slot {}", (va - start) / step)
        } else {
            format!("{:#x} past its end (slot {})", va - start - bytes, (va - start) / step)
        };
        text.push_str(&format!("; {kind} heap {start:#x}+{bytes:#x}: {place}"));
    }
    text
}

/// The host's limit for shader-visible view heaps on resource binding
/// tiers 1 and 2; tier 3 hardware may take more.
const PORTABLE_VIEWS: u32 = 1_000_000;

/// The size of every host view heap on a host that takes more than
/// [`PORTABLE_VIEWS`] (AMD): the title's heap (a little over 1,000,000
/// views in the retail game) plus [`DYNAMIC`] fits with room to spare, and
/// the last slot is the null slot every view index at or past it reads.
const UNCAPPED_VIEWS: u32 = 1 << 21;

/// Picks, once, the null slot at the end of every host view heap and tells
/// the translator now (`set_view_heap_limit`) instead of when the title's
/// heap is made: a shader keeps the limit it was translated with, and the
/// title makes 25 pipelines (their shaders translated at once) before it
/// makes its view heap. Called before a shader is translated and before a
/// view heap is made.
///
/// Every host clamps. A host that caps view heaps at [`PORTABLE_VIEWS`]
/// (NVIDIA) hung on a view index past the heap; one that does not (AMD
/// RDNA3, RX 7900 XT) took a page fault and removed the device on entering
/// the world: a compute program read view slot 16,777,088 (0xffff80) of a
/// heap of 1,507,328, past its end. Unclamped, that index is undefined on
/// any host.
pub(crate) fn probe_view_limit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(host) = host() else { return };
        let fits = |count: u32| {
            // SAFETY: plain heap creation; the heap is released at once.
            let made: windows::core::Result<ID3D12DescriptorHeap> = unsafe {
                host.device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                        NumDescriptors: count,
                        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                        NodeMask: 0,
                    })
            };
            made.is_ok()
        };
        let views = if fits(UNCAPPED_VIEWS) {
            UNCAPPED_VIEWS
        } else {
            PORTABLE_VIEWS
        };
        tracing::info!(
            views,
            "host view heaps hold {views} views; view indices are clamped to the null slot at the end from the first shader on"
        );
        victory_shader::emit::set_view_heap_limit(views - 1);
    });
}

/// Slots past the title's in a view heap, for tables a draw needs made
/// afresh (constant buffers copied per draw). A list holds the slots it
/// took until it is reset (its submissions have run) or released.
/// 2^16 ran out in the world ("no free dynamic descriptor slots", run 11;
/// upstream PR #336 takes 2^18). On a host heap capped at 1,000,000 views
/// (NVIDIA) these come out of the title's slots, whose highest seen is
/// about 720,000: 2^17 left 868,000 of them, and ran out in the opening
/// cutscene (a frame asks for up to 16,000 tables while the frame before
/// still holds its own; a draw refused its table read slots never made
/// for it, and the card stopped within frames). 2^18 leaves 737,000; a
/// table past them is made afresh in these slots like any other.
pub(crate) const DYNAMIC: u32 = 1 << 18;

impl HostHeap {
    pub(crate) fn new(sampler: bool, count: u32) -> Option<Self> {
        let host = host()?;
        let kind = if sampler {
            D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER
        } else {
            D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
        };
        let make = |count: u32| -> windows::core::Result<ID3D12DescriptorHeap> {
            // SAFETY: plain heap creation.
            unsafe {
                host.device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: kind,
                        NumDescriptors: count,
                        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                        NodeMask: 0,
                    })
            }
        };
        let extra = if sampler { 0 } else { DYNAMIC };
        // A view heap ends at the null slot shaders clamp view indices to
        // (`probe_view_limit`), whatever the title's own count: the title's
        // slots, then the dynamic ones, then null up to and with that slot.
        if !sampler {
            probe_view_limit();
        }
        let null = if sampler {
            0
        } else {
            victory_shader::emit::view_heap_limit()
        };
        let (heap, count, size) = if null == 0 {
            match make(count + extra) {
                Ok(heap) => (heap, count, count + extra),
                Err(e) => {
                    tracing::warn!(error = %e, count, sampler, "no host descriptor heap");
                    return None;
                }
            }
        } else {
            let size = null + 1;
            let fitted = if count + extra <= null {
                count
            } else {
                // shortcut: descriptors past the host's limit are left out; a title that reaches
                // them needs remapping. They read the null slot instead.
                tracing::warn!(count, "host view heap capped at {size}; views past it read a null slot");
                null - extra
            };
            match make(size) {
                Ok(heap) => (heap, fitted, size),
                Err(e) => {
                    tracing::warn!(error = %e, count, size, "no host view heap");
                    return None;
                }
            }
        };
        // SAFETY: a live heap.
        let (cpu, step) = unsafe {
            (
                heap.GetCPUDescriptorHandleForHeapStart().ptr,
                host.device.GetDescriptorHandleIncrementSize(kind),
            )
        };
        tracing::debug!(count, sampler, "host descriptor heap");
        // The last slot stays null: what shaders read for an index past the heap.
        let mut taken = vec![false; DYNAMIC as usize];
        taken[DYNAMIC as usize - 1] = true;
        // SAFETY: a live shader-visible heap.
        let gpu = unsafe { heap.GetGPUDescriptorHandleForHeapStart().ptr };
        GPU_RANGES.lock().unwrap_or_else(|e| e.into_inner()).push((
            gpu,
            u64::from(size) * u64::from(step),
            u64::from(step),
            sampler,
        ));
        let strides = (!sampler)
            .then(|| {
                let bytes = u64::from(victory_shader::emit::SLOT_STRIDES_COUNT) * 4;
                let (buffer, data) = host.upload_buffer(bytes)?;
                // SAFETY: the buffer's whole mapping; a live buffer.
                let gpu = unsafe {
                    std::ptr::write_bytes(data, 0, bytes as usize);
                    buffer.GetGPUVirtualAddress()
                };
                crate::host::name(&buffer, || "element strides by view heap slot".to_owned());
                Some((buffer, gpu, data as usize))
            })
            .flatten();
        let total = (count + extra) as usize;
        let h = HostHeap {
            heap,
            cpu,
            step,
            count,
            dynamic: extra,
            sampler,
            made: (0..count).map(|_| AtomicU64::new(0)).collect(),
            slot_locks: (0..SLOT_LOCK_STRIPES)
                .map(|_| StripeLock(Mutex::new(())))
                .collect(),
            copied: OnceLock::new(),
            kept: (0..SLOT_LOCK_STRIPES)
                .map(|stripe| {
                    let len = total.saturating_sub(stripe).div_ceil(SLOT_LOCK_STRIPES);
                    StripeLock(Mutex::new(vec![None; len]))
                })
                .collect(),
            next: Mutex::new((0, taken)),
            free: std::sync::atomic::AtomicU32::new(DYNAMIC - 1),
            too_long: std::sync::atomic::AtomicU32::new(u32::MAX),
            strides,
            buffers: Mutex::default(),
            addresses: std::sync::OnceLock::new(),
            acceleration: Mutex::default(),
            changed: Mutex::default(),
            textures: Mutex::default(),
            tabled: (0..if sampler { 0 } else { count })
                .map(|_| std::sync::atomic::AtomicU32::new(0))
                .collect(),
            slot_flags: (0..if sampler { 0 } else { count })
                .map(|_| std::sync::atomic::AtomicU8::new(0))
                .collect(),
            placed: {
                let spans = std::sync::Arc::<Mutex<Spans>>::default();
                if !sampler {
                    PLACED_SPANS
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(std::sync::Arc::downgrade(&spans));
                }
                spans
            },
        };
        // A slot never written is undefined: shaders reading one (a slot
        // the title wrote in a way we do not know, or a table entry no
        // used-resource entry names) read a wild address. Every slot starts
        // null instead.
        let started = std::time::Instant::now();
        if let Err(e) = h.fill_null(kind, size) {
            tracing::warn!(error = %e, "host descriptor heap not made null");
        }
        tracing::debug!(ms = started.elapsed().as_millis(), "host descriptor heap made null");
        Some(h)
    }

    /// Makes the first `total` slots null: one null slot in a CPU-side heap,
    /// doubled by copies to a block, the block copied over the heap.
    fn fill_null(&self, kind: D3D12_DESCRIPTOR_HEAP_TYPE, total: u32) -> windows::core::Result<()> {
        const BLOCK: u32 = 1 << 16;
        let Some(host) = host() else { return Ok(()) };
        // SAFETY: plain heap creation and copies inside live heaps.
        unsafe {
            let block: ID3D12DescriptorHeap =
                host.device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: kind,
                    NumDescriptors: BLOCK,
                    ..Default::default()
                })?;
            let start = block.GetCPUDescriptorHandleForHeapStart();
            let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: start.ptr + (i * self.step) as usize,
            };
            if self.sampler {
                null_sampler(start);
            } else {
                null_view(start);
            }
            let mut filled = 1;
            while filled < BLOCK {
                let n = filled.min(BLOCK - filled);
                host.device.CopyDescriptorsSimple(n, at(filled), start, kind);
                filled += n;
            }
            let mut done = 0;
            while done < total {
                let n = BLOCK.min(total - done);
                host.device.CopyDescriptorsSimple(n, self.cpu(done), start, kind);
                done += n;
            }
        }
        Ok(())
    }

    /// A plain point sampler at slot `index`, for sampler slots without a
    /// view.
    fn null_sampler(&self, index: u32) {
        null_sampler(self.cpu(index));
    }
}

/// A plain point sampler at `at`.
fn null_sampler(at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let desc = D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        ..Default::default()
    };
    // SAFETY: a slot of a sampler heap of ours.
    unsafe { host.device.CreateSampler(&desc, at) };
}

impl HostHeap {
    /// `n` consecutive free dynamic slots (view heaps only), taken until
    /// [`HostHeap::release`]: the first's index.
    pub(crate) fn dynamic(&self, n: u32) -> Option<u32> {
        if self.sampler || n == 0 || n > self.dynamic {
            return None;
        }
        #[cfg(feature = "diagnostics")]
        let started = std::time::Instant::now();
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::DynamicAsked, 1);
        let mut guard = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let (next, taken) = &mut *guard;
        let enough = self.free.load(std::sync::atomic::Ordering::Relaxed) >= n
            && n < self.too_long.load(std::sync::atomic::Ordering::Relaxed);
        let n = n as usize;
        // Search from `next` on, then from the start.
        let starts = (*next as usize..self.dynamic as usize).chain(0..*next as usize);
        let mut run = 0;
        let mut found = None;
        let mut last = usize::MAX;
        for at in starts.take(if enough { DYNAMIC as usize } else { 0 }) {
            // A run cannot wrap past the end.
            if at != last.wrapping_add(1) {
                run = 0;
            }
            last = at;
            run = if taken[at] { 0 } else { run + 1 };
            if run == n {
                found = Some(at + 1 - n);
                break;
            }
        }
        let Some(first) = found else {
            if enough {
                self.too_long.fetch_min(n as u32, std::sync::atomic::Ordering::Relaxed);
            }
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!(n, "no free dynamic descriptor slots: a table is not made"));
            #[cfg(feature = "diagnostics")]
            {
                use crate::copystat::{C, add};
                add(C::DynamicRefused, 1);
                add(C::DynamicNs, started.elapsed().as_nanos() as u64);
            }
            return None;
        };
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::DynamicNs, started.elapsed().as_nanos() as u64);
        taken[first..first + n].fill(true);
        self.free.fetch_sub(n as u32, std::sync::atomic::Ordering::Relaxed);
        *next = ((first + n) % DYNAMIC as usize) as u32;
        Some(self.count + first as u32)
    }

    /// The title slots this heap has host slots for; fewer than the title's
    /// when the host caps its size.
    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    /// The GPU address of the table of element strides by title slot (view
    /// heaps).
    pub(crate) fn strides_gpu(&self) -> Option<u64> {
        self.strides.as_ref().map(|s| s.1)
    }

    /// The table's buffer, for a list to hold.
    pub(crate) fn strides_buffer(&self) -> Option<&ID3D12Resource> {
        self.strides.as_ref().map(|s| &s.0)
    }

    /// The table of guest addresses by slot
    /// (`victory_shader::emit::SLOT_VIEW_ADDRESSES`): the slot count, a
    /// null entry, then one 64-bit title address per slot. Made at the
    /// first ask from the views there, under the slot locks that order
    /// later writes (from dexnom/VIctory 5f4c3df4).
    pub(crate) fn address_buffer(&self, heap: &super::Heap) -> Option<&ID3D12Resource> {
        if let Some((buffer, _)) = self.addresses.get() {
            return Some(buffer);
        }
        // Every slot stripe, in ascending order as `mirror` takes them:
        // no slot is written while the table is filled.
        let _slots: Vec<_> =
            self.slot_locks.iter().map(|l| l.0.lock().unwrap_or_else(|e| e.into_inner())).collect();
        if self.addresses.get().is_none() {
            let bytes = (u64::from(self.count) + 2) * 8;
            let (buffer, data) = host()?.upload_buffer(bytes)?;
            // SAFETY: a fresh mapping of the whole table.
            unsafe {
                std::ptr::write_bytes(data, 0, bytes as usize);
                data.cast::<u64>().write(u64::from(self.count));
            }
            let mut slots: Vec<u32> = self.buffers.lock().unwrap_or_else(|e| e.into_inner()).iter().copied().collect();
            slots.extend(self.acceleration.lock().unwrap_or_else(|e| e.into_inner()).keys());
            for slot in slots.into_iter().filter(|&s| s < self.count) {
                let address = heap.handle(slot).and_then(super::view).as_ref().and_then(guest_srv_address).unwrap_or(0);
                // SAFETY: inside the mapping.
                unsafe { data.cast::<u64>().add(slot as usize + 2).write(address) };
            }
            let _ = self.addresses.set((buffer, data as usize));
        }
        self.addresses.get().map(|v| &v.0)
    }

    /// The guest addresses of the acceleration structures the heap's
    /// views name, for the memory a traversal reads to be made ready.
    pub(crate) fn acceleration_addresses(&self) -> Vec<u64> {
        let mut locations: Vec<u64> =
            self.acceleration.lock().unwrap_or_else(|e| e.into_inner()).values().copied().collect();
        locations.sort_unstable();
        locations.dedup();
        locations
    }

    /// Up to `max` title slots whose texture view is due at `frame`, each
    /// with its write number, in the order to serve them
    /// ([`TextureQueue::due`]), and the queue's counts
    /// ([`TextureQueue::census`]).
    pub(crate) fn texture_slots_due(&self, frame: u64, max: usize) -> (Vec<(u32, u64)>, [u64; 5]) {
        let textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        (if max == 0 { Vec::new() } else { textures.due(frame, max) }, textures.census(frame))
    }

    /// Every title slot of the queue that is due at `frame`, and whether
    /// it was uploaded before (in part, or whole and waiting for another
    /// look): for the card accounting line
    /// (`crate::resource::card_accounting`).
    pub(crate) fn texture_slots_all_due(&self, frame: u64) -> Vec<(u32, bool)> {
        let textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        textures
            .slots
            .iter()
            .filter(|(_, w)| w.due <= frame)
            .map(|(&index, w)| (index, w.mapped > 0 || w.checked.2 != 0))
            .collect()
    }

    /// The texture view of title slot `index` (as handed out with `write`)
    /// was brought up to date: due again from frame `again` (uploaded with
    /// that many bytes of its memory there, not all), or not until the
    /// slot is written again.
    pub(crate) fn texture_slot_done(&self, index: u32, write: u64, again: Option<(u64, u64)>) {
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        match again {
            Some((frame, mapped)) => textures.partly(index, write, frame, mapped),
            None => textures.done(index, write),
        }
    }

    /// Title slot `index` was uploaded whole: see [`TextureQueue::recheck`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn texture_slot_recheck(
        &self,
        index: u32,
        write: u64,
        due: u64,
        at: (u64, u64),
        print: u64,
        looks: u8,
        uploaded: u64,
    ) {
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        textures.recheck(index, write, due, at, print, looks, uploaded);
        self.note_slot(index, SLOT_IN_TEXTURES, true);
    }

    /// See [`TextureQueue::checked`].
    pub(crate) fn texture_slot_checked(&self, index: u32) -> Option<(u64, u64, u64, u8, u64)> {
        self.textures.lock().unwrap_or_else(|e| e.into_inner()).checked(index)
    }

    /// Nothing to upload at title slot `index` now (its memory is not
    /// mapped, or no more of it than before): looked at again after 1, 2,
    /// 4, ... frames, at most `most`.
    pub(crate) fn texture_slot_later(&self, index: u32, write: u64, frame: u64, most: u64) {
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        textures.later(index, write, frame, most);
    }

    /// See [`TextureQueue::hold`].
    pub(crate) fn texture_slot_held(&self, index: u32, write: u64, due: u64) {
        self.textures.lock().unwrap_or_else(|e| e.into_inner()).hold(index, write, due);
    }

    /// See [`TextureQueue::room`].
    pub(crate) fn texture_slots_room(&self, frame: u64) -> usize {
        self.textures.lock().unwrap_or_else(|e| e.into_inner()).room(frame)
    }

    /// Bytes of title slot `index`'s memory that were mapped when it was
    /// last uploaded in part (0: not uploaded, or whole).
    pub(crate) fn texture_slot_mapped(&self, index: u32) -> u64 {
        self.textures.lock().unwrap_or_else(|e| e.into_inner()).mapped(index)
    }

    /// Takes the ranges of title memory the runtime mapped, unmapped or
    /// wrote since the last call ([`remapped`]) and makes the slots whose
    /// placed texture lies there due from the next frame (by when a write
    /// under way has landed): memory that arrived is uploaded, memory that
    /// went makes the slot null ([`HostHeap::texture_slot_gone`]), and a
    /// texture streamed into the place of another of the same description
    /// is uploaded although no descriptor says so. Slots tables read are
    /// left to their draws. After ranges were dropped ([`OVERFLOWED`]) it
    /// makes every placed slot due, once. Returns how many slots it made due.
    /// shortcut: the ranges are kept in one place for all heaps; the heap
    /// whose pass runs first takes them (a title has one view heap).
    pub(crate) fn texture_slots_remapped(&self, frame: u64) -> usize {
        use std::sync::atomic::Ordering::Relaxed;
        self.texture_slots_page_written(frame);
        let ranges = std::mem::take(&mut *REMAPPED.lock().unwrap_or_else(|e| e.into_inner()));
        // Taken once: the flag is set again only by another overflow.
        let all = OVERFLOWED.swap(false, Relaxed);
        if ranges.is_empty() && !all {
            return 0;
        }
        let mut slots: Vec<u32> = {
            let placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            if all {
                placed.by_slot.keys().copied().collect()
            } else {
                ranges.iter().flat_map(|&(start, end)| placed.slots_in(start, end)).collect()
            }
        };
        if all {
            tracing::info!("remapped ranges overflowed: full rescan of {} slots", slots.len());
        }
        slots.sort_unstable();
        slots.dedup();
        // A table's slot waits for its draw, as a write of it does: the
        // draw takes it out again. Left out altogether, a slot that shaders
        // also pick by index kept what it showed before.
        let lately: Vec<bool> = slots.iter().map(|&slot| self.tabled_lately(slot, frame)).collect();
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        for (&slot, lately) in slots.iter().zip(lately) {
            textures.changed(slot, frame, frame + if lately { TABLED_GRACE } else { 1 });
            self.note_slot(slot, SLOT_IN_TEXTURES, true);
        }
        STALE[3].fetch_add(slots.len() as u64, Relaxed);
        slots.len()
    }

    /// Takes the ranges of title memory written behind the guards' back
    /// since the last call ([`page_written`]) and finds the slots whose
    /// placed texture holds bytes of them ([`Spans::slots_holding`]). A
    /// slot whose texture was uploaded before the write shows the bytes
    /// from before: with `VICTORY_PAGE_WRITES` (`crate::watch::page_writes`,
    /// bit 2) it is due again [`PAGE_WRITE_AFTER`] frames on, as memory
    /// mapped under it would make it ([`HostHeap::texture_slots_remapped`]);
    /// its host copy exists, so the upload needs no room on the card. A
    /// slot still waiting for its first upload is left as it is: that
    /// upload takes the bytes that are there by then. Slots tables read
    /// are left to their draws. Counted either way; returns how many slots
    /// it made due.
    fn texture_slots_page_written(&self, frame: u64) -> usize {
        let ranges = std::mem::take(&mut *PAGE_WRITTEN.lock().unwrap_or_else(|e| e.into_inner()));
        if ranges.is_empty() {
            return 0;
        }
        self.slots_written_under(&ranges, frame, crate::watch::page_writes() & 2 != 0)[2]
    }

    /// [`HostHeap::texture_slots_page_written`] for `ranges` (start, end):
    /// the slots that held bytes of them, those of them uploaded before,
    /// and those made due (the latter, if `due`).
    fn slots_written_under(&self, ranges: &[(u64, u64)], frame: u64, due: bool) -> [usize; 3] {
        use std::sync::atomic::Ordering::Relaxed;
        let mut slots: Vec<u32> = {
            let placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            ranges.iter().flat_map(|&(start, end)| placed.slots_holding(start, end)).collect()
        };
        slots.sort_unstable();
        slots.dedup();
        slots.retain(|&slot| !self.tabled_lately(slot, frame));
        let held = slots.len();
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        // Uploaded before: out of the queue, or in it after an upload (in
        // part, or whole and being looked at again).
        slots.retain(|slot| textures.slots.get(slot).is_none_or(|w| w.mapped > 0 || w.checked.2 != 0));
        PAGE_WRITES[2].fetch_add(held as u64, Relaxed);
        PAGE_WRITES[3].fetch_add(slots.len() as u64, Relaxed);
        if !due {
            return [held, slots.len(), 0];
        }
        for &slot in &slots {
            textures.changed(slot, frame, frame + PAGE_WRITE_AFTER);
            self.note_slot(slot, SLOT_IN_TEXTURES, true);
        }
        [held, slots.len(), slots.len()]
    }

    /// Whether host slot `index` is made from the very record the title
    /// has at `handle` now. Its host texture then exists (a slot keeps what
    /// it views), so bringing that texture up to date takes no memory of
    /// the card.
    pub(crate) fn texture_slot_made_from(&self, index: u32, handle: u64) -> bool {
        if index >= self.count {
            return false;
        }
        // SAFETY: the handle is a descriptor slot in title memory.
        let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
        let key = u64::from(generation) << 32 | u64::from(i);
        if tag != VIEW_TAG {
            return false;
        }
        // And the slot holds a host resource: a slot is also noted as made
        // from a view the host could not make (`make` gave nothing, the
        // slot is null and keeps nothing). The host copy of such a
        // texture is held by no slot, may have been let go, and would be
        // made anew here, with the card full. `made` first, then `kept`,
        // as `mirror` takes them.
        self.made
            .get(index as usize)
            .is_some_and(|made| {
                let made = made.load(Ordering::Acquire);
                made != 0 && made == key
            })
            && self.has_kept(index)
    }

    /// Levels on demand (`crate::tiles`): makes again the host slots of the
    /// placed textures whose first level with memory changed since the
    /// last call ([`levels_changed`]), each from the view it was made
    /// from, so that it shows the texture from that level ([`make`]). Only
    /// slots that are made; a slot draws read through a table is made by
    /// its draw. Returns how many it made.
    /// shortcut: the starts are kept in one place for all heaps; the heap
    /// whose pass runs first takes them (a title has one view heap).
    /// Levels on demand (`crate::tiles`): the card has room, so the slots
    /// of up to [`Self::WAITING_A_PASS`] placed textures whose upload left
    /// a held level without tiles at least a second ago
    /// ([`levels_waiting`]) are due again from the next frame, as memory
    /// mapped under them would make them
    /// ([`HostHeap::texture_slots_remapped`]): the upload then gives the
    /// level its tiles. To be called only while the card has room: without
    /// it the upload would give nothing again. Slots tables read are left
    /// to their draws (`texture.rs` `upload` asks again for those). Returns
    /// how many slots it made due.
    /// shortcut: kept in one place for all heaps, as the starts below.
    pub(crate) fn texture_slots_waiting(&self, frame: u64) -> usize {
        if self.sampler {
            return 0;
        }
        let starts: Vec<u64> = {
            let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
            if waiting.is_empty() {
                return 0;
            }
            let starts: Vec<u64> = waiting
                .iter()
                .filter(|&(_, &since)| crate::host::long_ago(since, 1000, 2))
                .map(|(&start, _)| start)
                .take(Self::WAITING_A_PASS)
                .collect();
            for start in &starts {
                waiting.remove(start);
            }
            starts
        };
        if starts.is_empty() {
            return 0;
        }
        let mut slots: Vec<u32> = {
            let placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            starts.iter().flat_map(|&start| placed.slots_at(start)).collect()
        };
        slots.sort_unstable();
        slots.dedup();
        slots.retain(|&slot| !self.tabled_lately(slot, frame));
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        for &slot in &slots {
            textures.changed(slot, frame, frame + 1);
            self.note_slot(slot, SLOT_IN_TEXTURES, true);
        }
        crate::tiles::STATS[10].fetch_add(slots.len() as u64, std::sync::atomic::Ordering::Relaxed);
        slots.len()
    }

    /// Textures a pass asks again for their levels
    /// ([`HostHeap::texture_slots_waiting`]): the first ones given tiles
    /// may take the room the others were asked for, and an upload that
    /// gives nothing is a whole texture staged and copied for nothing.
    const WAITING_A_PASS: usize = 128;

    pub(crate) fn texture_views_remade(&self) -> usize {
        if self.sampler {
            return 0;
        }
        let starts = std::mem::take(&mut *RESHOWN.lock().unwrap_or_else(|e| e.into_inner()));
        if starts.is_empty() {
            return 0;
        }
        let slots: Vec<u32> = {
            let placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            starts.iter().flat_map(|&start| placed.slots_at(start)).collect()
        };
        let mut remade = 0;
        let mut displaced = Vec::new();
        for slot in slots {
            if slot >= self.count {
                continue;
            }
            let _slot_guard = self.slot_lock(slot);
            let Some(key) = self
                .made
                .get(slot as usize)
                .map(|m| m.load(Ordering::Acquire))
            else {
                continue;
            };
            if key == 0 {
                continue;
            }
            let Some(view) = view_at(key as u32, (key >> 32) as u32) else {
                continue;
            };
            // Clear `made[slot]` with Release before rewriting the descriptor
            // and replacing `kept`, then restore `key` with Release afterwards
            // so concurrent fast-path readers wait on `_slot_guard` instead of
            // observing a partially updated slot.
            self.made[slot as usize].store(0, Ordering::Release);
            if let Some((kept, _)) = make(&view, self.cpu(slot)) {
                if let Some(old) = self.replace_kept(slot, kept) {
                    displaced.push(old);
                }
                remade += 1;
            }
            self.made[slot as usize].store(key, Ordering::Release);
        }
        Self::retire_all(displaced);
        remade
    }

    /// The memory of the texture title slot `index` views is not mapped
    /// (any more): a host slot made from it becomes null again, so that
    /// the host copy can be let go (`crate::resource::let_go_idle`; a slot
    /// keeps what it views). True if the slot was made.
    pub(crate) fn texture_slot_gone(&self, index: u32) -> bool {
        if index >= self.count {
            return false;
        }
        let displaced = {
            let _slot_guard = self.slot_lock(index);
            if self
                .made
                .get(index as usize)
                .is_none_or(|m| m.load(Ordering::Acquire) == 0)
            {
                return false;
            }
            self.unmake_locked(index)
        };
        if let Some(old) = displaced {
            crate::host::retire(old);
        }
        STALE[4].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        true
    }

    /// Makes host slot `index` null again, as it is before its view is
    /// made: a null view of the kind it held. Caller must hold
    /// `slot_lock(index)`. Returns any displaced resource to retire outside
    /// locks.
    fn unmake_locked(&self, index: u32) -> Option<ID3D12Resource> {
        let slot_made = self.made.get(index as usize)?;
        let key = slot_made.swap(0, Ordering::AcqRel);
        if key == 0 {
            return None;
        }
        match view_at(key as u32, (key >> 32) as u32) {
            Some(view) => null_for(&view, self.cpu(index)),
            None => null_view(self.cpu(index)),
        }
        self.replace_kept(index, None)
    }

    /// Title slot `index` is a table's ([`HostHeap::tabled`]). `read`: a
    /// draw read it through the table and brought what it views up to
    /// date, so the slot leaves the queue of textures picked by index; the
    /// caller makes the host slot in place ([`HostHeap::mirror`]), as the
    /// pass would have, for a shader that picks it by index all the same.
    pub(crate) fn texture_slot_tabled(&self, index: u32, read: bool) {
        self.tabled_in(index, read, crate::frame::current());
    }

    /// [`HostHeap::texture_slot_tabled`] in frame `frame`.
    fn tabled_in(&self, index: u32, read: bool, frame: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        let Some(stamp) = self.tabled.get(index as usize) else {
            return;
        };
        let now = (frame as u32).wrapping_add(1) & !TABLED_READ;
        let old = stamp.load(Relaxed);
        if !read {
            if old & !TABLED_READ != now {
                stamp.store(now, Relaxed);
            }
            return;
        }
        // Once per slot and frame: tables are read by every draw.
        if old != now | TABLED_READ {
            stamp.store(now | TABLED_READ, Relaxed);
            if tabled_out() {
                let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
                if textures.slots.remove(&index).is_some() {
                    TABLED_WRITES[1].fetch_add(1, Relaxed);
                }
            }
        }
    }

    /// Whether title slot `index` was a table's within the last
    /// [`TABLED_FRAMES`] frames before `frame` (never, if such slots are
    /// to be queued as any other).
    fn tabled_lately(&self, index: u32, frame: u64) -> bool {
        let stamp = self.tabled_stamp(index) & !TABLED_READ;
        let now = (frame as u32).wrapping_add(1) & !TABLED_READ;
        tabled_out() && stamp != 0 && u64::from(now.wrapping_sub(stamp)) <= TABLED_FRAMES
    }

    /// What `tabled` holds for title slot `index`; 0: never a table's.
    fn tabled_stamp(&self, index: u32) -> u32 {
        self.tabled
            .get(index as usize)
            .map_or(0, |s| s.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Whether title slot `index` may be in the set `bit` stands for
    /// ([`HostHeap::slot_flags`]): a slot the flags do not cover (a dynamic
    /// one) is taken to be, so it takes the set's lock as before.
    fn slot_may(&self, index: u32, bit: u8) -> bool {
        self.slot_flags
            .get(index as usize)
            .is_none_or(|f| f.load(std::sync::atomic::Ordering::Relaxed) & bit != 0)
    }

    /// Notes in [`HostHeap::slot_flags`] that title slot `index` is (`there`),
    /// or is not, in the set `bit` stands for (nothing for dynamic slots).
    fn note_slot(&self, index: u32, bit: u8, there: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        let Some(flag) = self.slot_flags.get(index as usize) else {
            return;
        };
        if there {
            flag.fetch_or(bit, Relaxed);
        } else {
            flag.fetch_and(!bit, Relaxed);
        }
    }

    /// The queue's part of a write of title slot `index` in `frame`:
    /// `texture`, whether it now holds a view of a texture the title may
    /// fill; `seen` as for [`HostHeap::note_buffer_checked`].
    #[cfg(test)]
    fn queue_texture(&self, index: u32, texture: bool, seen: bool, frame: u64) {
        self.queue_texture_with_stale(index, texture, seen, frame, &[]);
    }

    fn queue_texture_with_stale(
        &self,
        index: u32,
        texture: bool,
        seen: bool,
        frame: u64,
        stale: &[u32],
    ) {
        if PUBLISHING.get() {
            return;
        }
        let stamp = if tabled_out() { self.tabled_stamp(index) } else { 0 };
        let now = (frame as u32).wrapping_add(1) & !TABLED_READ;
        // A table's slot lately: the draw that reads it uploads its
        // texture, so its write waits for that draw.
        let lately = stamp != 0 && u64::from(now.wrapping_sub(stamp & !TABLED_READ)) <= TABLED_FRAMES;
        // Made now by a draw of this frame that read it through a table:
        // up to date.
        let read_now = seen && stamp == now | TABLED_READ;
        if texture && !read_now {
            // Noted before the queue takes it, so that a write which asks the
            // flag first takes the lock.
            self.note_slot(index, SLOT_IN_TEXTURES, true);
        } else if stale.is_empty() && !self.slot_may(index, SLOT_IN_TEXTURES) {
            // Not in the queue, and nothing stale to take out of it.
            return;
        }
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        if texture && !read_now {
            let due = if lately && !seen {
                TABLED_WRITES[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                frame + TABLED_GRACE
            } else {
                0
            };
            textures.written(index, frame, due, seen);
        } else {
            textures.remove(index);
            self.note_slot(index, SLOT_IN_TEXTURES, false);
        }
        for &slot in stale {
            textures.remove(slot);
            self.note_slot(slot, SLOT_IN_TEXTURES, false);
        }
    }

    /// The title slots that hold buffer views.
    pub(crate) fn buffer_slots(&self) -> Vec<u32> {
        let buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        buffers.iter().copied().collect()
    }

    /// The title slots whose buffer view was written, or went, since the
    /// last call.
    pub(crate) fn buffer_changes(&self) -> Vec<u32> {
        let mut changed = self.changed.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *changed).into_iter().collect()
    }

    #[cfg(test)]
    fn note_buffer(&self, index: u32, view: Option<&View>, seen: bool) -> Vec<u32> {
        self.note_buffer_checked(index, view, seen, None, true, None)
            .unwrap_or_default()
    }

    /// Records what title slot `index` now holds for shaders that pick
    /// buffers from the heap: its element stride in the table, and whether
    /// it is a buffer view at all.
    /// `seen`: the slot was not written now, a draw or a pass made its
    /// host slot from what it held.
    /// Returns the title slots whose placed texture view the write made
    /// stale ([`Spans`]): out of the queue of textures already, their host
    /// slots are the caller's to make null (it holds the slot stripe locks).
    /// With `held_stripes = Some(sorted_stripes)`, if a clashing stale slot
    /// belongs to a stripe not in `held_stripes`, returns `Err(missing_stripes)`
    /// before mutating `placed`, `textures`, `strides`, `buffers`, or `changed`.
    /// `record_change`: a buffer view written here is handed to `changed`
    /// ([`HostHeap::buffer_changes`]), or to `batch` when given (a page of
    /// them, [`HostHeap::mirror_copied_page`]); not for the pass that took
    /// it from there ([`HostHeap::mirror_heap_buffer`]).
    fn note_buffer_checked(
        &self,
        index: u32,
        view: Option<&View>,
        seen: bool,
        held_stripes: Option<&[usize]>,
        record_change: bool,
        batch: Option<&mut Vec<u32>>,
    ) -> Result<Vec<u32>, Vec<usize>> {
        if self.sampler {
            return Ok(Vec::new());
        }
        let frame = crate::frame::current();
        // The memory a placed texture view describes. Not for a slot
        // tables read (thousands of writes a frame, each read by its
        // draw): no pass looks at it, and the same texture's view in a
        // slot shaders pick by index says what it says.
        let placed_view = match view {
            Some(View::Placed(false, resource, desc))
                if resource[0] != 1 && !self.tabled_lately(index, frame) =>
            {
                // (And the level the view starts at: word 4 of a texture
                // view's description, after format, dimension, mapping
                // and a pad.)
                Some((resource, u64::from(desc[10]) | u64::from(desc[11]) << 32, desc[4]))
            }
            _ => None,
        };
        let stale = if placed_view.is_some() || self.slot_may(index, SLOT_IN_PLACED) {
            use std::sync::atomic::Ordering::Relaxed;
            let mut placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            let span = placed_view.and_then(|(resource, start, first)| {
                Some((start, start.saturating_add(placed.bytes(resource)?), first))
            });
            match span {
                Some((start, end, first)) => {
                    placed.rule = stale_rule();
                    if !seen && let Some(held) = held_stripes {
                        let missing: Vec<usize> = placed
                            .clashing_slots(index, start, end)
                            .into_iter()
                            .map(Self::stripe)
                            .filter(|s| held.binary_search(s).is_err())
                            .collect();
                        if !missing.is_empty() {
                            return Err(missing);
                        }
                    }
                    SPANNED[0].fetch_min(start, Relaxed);
                    SPANNED[1].fetch_max(end, Relaxed);
                    let stale = placed.note(index, start, end, !seen);
                    placed.shows(index, first);
                    let there = placed.by_slot.contains_key(&index);
                    self.note_slot(index, SLOT_IN_PLACED, there);
                    if let Some(held) = held_stripes {
                        debug_assert!(
                            stale.iter().all(|&st| st == index
                                || held.binary_search(&Self::stripe(st)).is_ok()),
                            "all stale slots must be covered by held stripes"
                        );
                    }
                    stale
                }
                None => {
                    placed.forget(index);
                    self.note_slot(index, SLOT_IN_PLACED, false);
                    Vec::new()
                }
            }
        } else {
            // Not in the spans as far as the flags say: nothing to forget.
            Vec::new()
        };
        // Measured for the texture side of the same gap (textures a shader
        // picks by a computed index are neither made, if placed, nor
        // uploaded): slots written with a placed texture view, and with a
        // view of a texture object.
        let texture = match view {
            Some(View::Placed(false, resource, _)) if resource[0] != 1 => {
                PLACED_TEXTURE_SLOTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                true
            }
            Some(View::Shader(r, _)) if crate::resource::buffer_bytes(*r as crate::object::This).is_none() => {
                OBJECT_TEXTURE_SLOTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                true
            }
            _ => false,
        };
        self.queue_texture_with_stale(index, texture, seen, frame, &stale);
        if !stale.is_empty() {
            STALE[0].fetch_add(stale.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        let stride = view.and_then(view_stride);
        if let Some((_, _, data)) = &self.strides
            && index < victory_shader::emit::SLOT_STRIDES_COUNT
        {
            // SAFETY: inside the mapping; shaders read it while we write (a
            // dword, either value is one the slot had).
            unsafe { (*data as *mut u32).add(index as usize).write_volatile(stride.unwrap_or(0)) };
        }
        if let Some((_, mapping)) = self.addresses.get() {
            let address = view.and_then(guest_srv_address).unwrap_or(0);
            // SAFETY: inside the mapping (a title slot below `count`).
            unsafe { (*mapping as *mut u64).add(index as usize + 2).write_volatile(address) };
        }
        match view.and_then(acceleration_location) {
            Some(location) => {
                self.note_slot(index, SLOT_IN_ACCELERATION, true);
                self.acceleration.lock().unwrap_or_else(|e| e.into_inner()).insert(index, location);
            }
            // Not in the map as far as the flags say: nothing to take out.
            None if self.slot_may(index, SLOT_IN_ACCELERATION) => {
                self.note_slot(index, SLOT_IN_ACCELERATION, false);
                self.acceleration.lock().unwrap_or_else(|e| e.into_inner()).remove(&index);
            }
            None => {}
        }
        // The set is written only here: a slot with its flag set is in it, and
        // its lock is taken only to put a slot in or to take one out.
        let flagged = self.slot_flags.get(index as usize);
        let in_buffers = flagged.is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed) & SLOT_IN_BUFFERS != 0);
        let was = if stride.is_some() {
            if !in_buffers {
                self.note_slot(index, SLOT_IN_BUFFERS, true);
                self.buffers.lock().unwrap_or_else(|e| e.into_inner()).insert(index);
            }
            true
        } else if in_buffers || flagged.is_none() {
            self.note_slot(index, SLOT_IN_BUFFERS, false);
            self.buffers.lock().unwrap_or_else(|e| e.into_inner()).remove(&index)
        } else {
            false
        };
        if was && record_change {
            match batch {
                Some(changed) => changed.push(index),
                None => {
                    self.changed.lock().unwrap_or_else(|e| e.into_inner()).insert(index);
                }
            }
        }
        Ok(stale)
    }

    /// Gives back `n` dynamic slots from `first` (an index [`dynamic`]
    /// returned).
    ///
    /// [`dynamic`]: HostHeap::dynamic
    pub(crate) fn release(&self, first: u32, n: u32) {
        let Some(at) = first.checked_sub(self.count) else {
            return;
        };
        let mut guard = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let end = (at + n).min(self.dynamic) as usize;
        if let Some(slots) = guard.1.get_mut(at as usize..end) {
            let given = slots.iter().filter(|taken| **taken).count() as u32;
            self.free.fetch_add(given, std::sync::atomic::Ordering::Relaxed);
            self.too_long.store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
            slots.fill(false);
        }
    }

    /// Makes slot `index` (a dynamic one) from `view`; a null view if the
    /// host cannot.
    pub(crate) fn make_at(&self, index: u32, view: &View) {
        let made = make(view, self.cpu(index));
        if made.is_none() {
            null_for(view, self.cpu(index));
        }
        self.keep(index, made.and_then(|(kept, _)| kept));
    }

    /// Makes slot `index` read nothing: a null view, or a plain sampler.
    pub(crate) fn make_null(&self, index: u32) {
        if let Some(old) = self.make_null_keeping(index) {
            crate::host::retire(old);
        }
    }

    fn make_null_keeping(&self, index: u32) -> Option<ID3D12Resource> {
        if self.sampler {
            self.null_sampler(index);
        } else {
            null_view(self.cpu(index));
        }
        self.replace_kept(index, None)
    }

    fn stripe(index: u32) -> usize {
        (index as usize) & (SLOT_LOCK_STRIPES - 1)
    }

    fn slot_lock(&self, index: u32) -> std::sync::MutexGuard<'_, ()> {
        self.slot_locks[Self::stripe(index)]
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn kept_slot(&self, index: u32) -> Option<(usize, usize)> {
        let total = self.count.checked_add(self.dynamic)?;
        if index >= total {
            return None;
        }
        Some((Self::stripe(index), (index as usize) >> SLOT_LOCK_SHIFT))
    }

    fn replace_kept(&self, index: u32, resource: Option<ID3D12Resource>) -> Option<ID3D12Resource> {
        let (stripe, local) = self.kept_slot(index)?;
        let mut kept = self.kept[stripe].0.lock().unwrap_or_else(|e| e.into_inner());
        kept.get_mut(local)
            .and_then(|slot| std::mem::replace(slot, resource))
    }

    fn has_kept(&self, index: u32) -> bool {
        let Some((stripe, local)) = self.kept_slot(index) else {
            return false;
        };
        self.kept[stripe]
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(local)
            .is_some_and(Option::is_some)
    }

    fn retire_all(resources: Vec<ID3D12Resource>) {
        for old in resources {
            crate::host::retire(old);
        }
    }

    /// Slot `index` now views `resource` (or nothing of the host's).
    fn keep(&self, index: u32, resource: Option<ID3D12Resource>) {
        if let Some(old) = self.replace_kept(index, resource) {
            crate::host::retire(old);
        }
    }

    /// Makes slot `index` (a dynamic one) a raw buffer view of `len` bytes
    /// of `buffer` from byte `offset` (a multiple of 16): a root SRV with a
    /// size, whose reads past it return zeros instead of reaching whatever
    /// the host placed after the buffer.
    pub(crate) fn raw_at(&self, index: u32, buffer: &ID3D12Resource, offset: u64, len: u64) {
        let Some(host) = host() else { return };
        let elements = (len / 4).clamp(1, u64::from(u32::MAX)) as u32;
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_SRV {
                    FirstElement: offset / 4,
                    NumElements: elements,
                    StructureByteStride: 0,
                    Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                },
            },
        };
        // SAFETY: a slot of our heap and a live buffer covering the range.
        unsafe {
            host.device
                .CreateShaderResourceView(buffer, Some(&desc), self.cpu(index))
        };
        self.keep(index, Some(buffer.clone()));
    }

    /// Makes slot `index` (a dynamic one) a raw buffer view that reads
    /// zeros.
    pub(crate) fn null_raw_at(&self, index: u32) {
        null_buffer_view(self.cpu(index));
        self.keep(index, None);
    }

    /// Makes slot `index` (a dynamic one) a raw unordered access view of
    /// `len` bytes of `buffer` from byte `offset` (a multiple of 16): a
    /// root UAV with a size, whose accesses past it do nothing instead of
    /// reaching whatever the host placed after the buffer.
    pub(crate) fn raw_uav_at(&self, index: u32, buffer: &ID3D12Resource, offset: u64, len: u64) {
        let Some(host) = host() else { return };
        let elements = (len / 4).clamp(1, u64::from(u32::MAX)) as u32;
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: offset / 4,
                    NumElements: elements,
                    StructureByteStride: 0,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                },
            },
        };
        // SAFETY: a slot of our heap and a live buffer covering the range.
        unsafe {
            host.device.CreateUnorderedAccessView(
                buffer,
                None::<&ID3D12Resource>,
                Some(&desc),
                self.cpu(index),
            )
        };
        self.keep(index, Some(buffer.clone()));
    }

    /// Makes slot `index` (a dynamic one) a raw unordered access view of
    /// nothing: reads give zeros, writes are dropped.
    pub(crate) fn null_raw_uav_at(&self, index: u32) {
        self.keep(index, None);
        let Some(host) = host() else { return };
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    NumElements: 1,
                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                    ..Default::default()
                },
            },
        };
        // SAFETY: a slot of our heap; a null resource is allowed with a desc.
        unsafe {
            host.device.CreateUnorderedAccessView(
                None::<&ID3D12Resource>,
                None::<&ID3D12Resource>,
                Some(&desc),
                self.cpu(index),
            )
        };
    }

    /// The shader-visible handle of slot `index`.
    pub(crate) fn gpu_handle(&self, index: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        // SAFETY: a live shader-visible heap.
        let start = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart().ptr };
        D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: start + u64::from(index) * u64::from(self.step),
        }
    }

    /// Makes slot `index` (a dynamic one) a constant buffer view of host
    /// memory (the list's own, which it holds).
    pub(crate) fn constant_at(&self, index: u32, gpu: u64, size: u32) {
        self.keep(index, None);
        let Some(host) = host() else { return };
        let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
            BufferLocation: gpu,
            SizeInBytes: constant_size(size),
        };
        // SAFETY: a slot of our heap; the memory is the caller's.
        unsafe {
            host.device
                .CreateConstantBufferView(Some(&desc), self.cpu(index))
        };
    }

    fn cpu(&self, index: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: self.cpu + index as usize * self.step as usize,
        }
    }

    /// For a slot on a page of the title's heap that changed with no call
    /// (`super::sync_heaps`: the title copies records into its heap as
    /// bytes): [`HostHeap::mirror`], if the `bytes` bytes of the record at
    /// `handle` are not the ones this last saw there. True if it did.
    ///
    /// The page says only that something on it was written, and `mirror`
    /// by itself would take every buffer or placed-texture record on it for
    /// newly written each time (it leaves such a slot unmade until a draw
    /// or a pass uses it): the texture queue's entry for the slot would
    /// start again, and the buffer pass would look at 2,048 slots for one
    /// that changed.
    /// `record`: the record's bytes as the caller read them (through the
    /// kernel, which fails on a page the title unmapped meanwhile where a
    /// read in place would fault: the title unmaps heap pages too).
    pub(crate) fn mirror_copied(&self, index: u32, handle: u64, record: &[u8]) -> bool {
        if index >= self.count {
            return false;
        }
        // A slot draws read through a table lately is left to its draws,
        // as the passes over slots picked by index leave it
        // (`texture_slots_remapped`, `slots_written_under`): the title
        // writes its per-draw tables into the same heap every frame, and
        // each draw makes what its table shows in a slot of its own. Those
        // were most of the 26,000 to 43,000 records a frame this pass
        // mirrored in run 200 and in the play session after it, each then
        // made and brought up to date once more by the pass over buffer
        // views (some 50 ms a frame). `VICTORY_HEAP_SYNC_TABLES=1`: they
        // are mirrored too, as in run 200.
        if !sync_tables() && self.tabled_lately(index, crate::frame::current()) {
            COPIED_TABLED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        let n = record.len();
        // Never 0, which is "never looked at". (A multiply-and-fold over
        // the record's words: BLAKE3 here was most of the 35 to 42 ms a
        // frame this pass took over some 250,000 to 300,000 records, run
        // 200.)
        let print = record_print(record) | 1;
        let copied = match self.copied.get() {
            Some(copied) => copied,
            None if record[..n].iter().all(|&b| b == 0) => return false,
            None => self
                .copied
                .get_or_init(|| (0..self.count).map(|_| AtomicU64::new(0)).collect()),
        };
        let Some(slot) = copied.get(index as usize) else {
            return false;
        };
        let mut prev = slot.load(Ordering::Acquire);
        loop {
            if prev == print || (prev == 0 && record[..n].iter().all(|&b| b == 0)) {
                return false;
            }
            match slot.compare_exchange_weak(prev, print, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(actual) => prev = actual,
            }
        }
        self.mirror(index, handle, false);
        true
    }

    /// [`HostHeap::mirror_copied`] for every record of one changed page of
    /// the title's heap (`super::sync_heaps`): title slots `slots`, the
    /// record of slot `i` at title address `start + i * step`, and `page`
    /// the bytes read from title address `from` on. Returns the records
    /// looked at and those mirrored.
    ///
    /// The same records are mirrored as one by one. What differs is the
    /// work for the others, which are nearly all of them (some 330,000
    /// looked at a frame for 14,000 mirrored, 16 ms, run 225): the
    /// fingerprints are made once for the page, and a record whose fingerprint is the one last seen is done
    /// with before the question whether its slot was a table's lately. So
    /// the count of slots "left to the draws" is now of records that
    /// changed, not of every record on the page. The slots to mirror are
    /// made after the page's walk, in slot order as before.
    ///
    /// `due` is the caller's list of the slots to mirror, emptied here and
    /// left holding this page's: one list serves every page.
    pub(crate) fn mirror_copied_page(
        &self,
        slots: std::ops::Range<u64>,
        (start, step): (u64, u64),
        from: usize,
        page: &[u8],
        due: &mut Vec<(u32, u64)>,
    ) -> (u64, u64) {
        let (frame, tables) = (crate::frame::current(), sync_tables());
        let mut looked = 0u64;
        due.clear();
        {
            let copied = self.copied.get_or_init(|| (0..self.count).map(|_| AtomicU64::new(0)).collect());
            for index in slots {
                let handle = start + index * step;
                // A record that straddles the range's ends is left to the
                // range that holds its first byte.
                let Some(at) = (handle as usize).checked_sub(from) else {
                    continue;
                };
                let Some(record) = page.get(at..at + step as usize) else {
                    continue;
                };
                looked += 1;
                // None past the host heap's slots, as `mirror_copied`.
                let Some(slot) = copied.get(index as usize) else {
                    continue;
                };
                let last = slot.load(Ordering::Acquire);
                // Never 0, which is "never looked at".
                let print = record_print(record) | 1;
                // The record last seen, or a slot never looked at that
                // holds nothing: as it was.
                if last == print || (last == 0 && record.iter().all(|&b| b == 0)) {
                    continue;
                }
                // A table's slot lately: left to its draws, and looked at
                // again next time.
                if !tables && self.tabled_lately(index as u32, frame) {
                    COPIED_TABLED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                }
                // Another pass took the slot meanwhile: its mirror reads
                // the record as it is now.
                if slot.compare_exchange(last, print, Ordering::AcqRel, Ordering::Acquire).is_err() {
                    continue;
                }
                due.push((index as u32, handle));
            }
        }
        // The buffer slots whose membership changed go to `changed` once for
        // the page, not with one lock for each.
        let mut changed = Vec::new();
        for &(index, handle) in due.iter() {
            self.mirror_inner(index, handle, false, true, Some(&mut changed));
        }
        if !changed.is_empty() {
            self.changed.lock().unwrap_or_else(|e| e.into_inner()).extend(changed);
        }
        (looked, due.len() as u64)
    }

    /// Makes host slot `index` from the record at `handle` if it is still
    /// the one with `key`, whose texture's upload is in the queue now
    /// (`crate::heap_worker`). The slot's place in the queue of textures
    /// picked by index was settled when that upload was recorded, and stays
    /// as it is. False if the title wrote the slot again.
    pub(crate) fn publish(&self, index: u32, handle: u64, key: u64) -> bool {
        // SAFETY: the handle is a descriptor slot in title memory.
        let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
        if tag != VIEW_TAG || u64::from(generation) << 32 | u64::from(i) != key {
            return false;
        }
        PUBLISHING.set(true);
        let shown = self.mirror(index, handle, true).is_some();
        PUBLISHING.set(false);
        shown
    }

    /// Makes host slot `index` from the record the title has at `handle`
    /// if it changed since, and returns the view recorded there. Buffer
    /// views are made only for draws (`for_draw`), which also copy their
    /// data: a host copy of a buffer is made on first use.
    pub(crate) fn mirror(&self, index: u32, handle: u64, for_draw: bool) -> Option<View> {
        self.mirror_inner(index, handle, for_draw, true, None)
            .map(|(view, _)| view)
    }

    /// [`HostHeap::mirror`] for the pass over slots whose buffer view was
    /// written (`heap_buffers` in the draw code): a slot it mirrors is not
    /// handed back to the pass as changed, which made it look at every
    /// buffer slot it mirrored again on the next frame. The copy the slot
    /// reads ([`buffer_copy`]) comes with it when making the view resolved
    /// it, so the pass need not look it up again.
    pub(crate) fn mirror_heap_buffer(&self, index: u32, handle: u64) -> Option<(View, Option<BufferCopy>)> {
        self.mirror_inner(index, handle, true, false, None)
    }

    /// [`HostHeap::mirror`] with the two choices the callers make:
    /// `record_change` as [`HostHeap::note_buffer_checked`]; `batch`, when
    /// given, takes the buffer slots whose membership changed instead of
    /// each locking [`HostHeap::changed`]. Returns the view, and the copy a
    /// buffer view was made on when making it resolved one.
    fn mirror_inner(
        &self,
        index: u32,
        handle: u64,
        for_draw: bool,
        record_change: bool,
        mut batch: Option<&mut Vec<u32>>,
    ) -> Option<(View, Option<BufferCopy>)> {
        if index >= self.count {
            // Diagnostic (NVIDIA heap cap): the highest title slot past the
            // host heap seen so far.
            static HIGHEST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if HIGHEST.fetch_max(index, std::sync::atomic::Ordering::Relaxed) < index {
                tracing::warn!(
                    index,
                    count = self.count,
                    for_draw,
                    "title slot past the host heap"
                );
            }
            return None;
        }
        // SAFETY: the handle is a descriptor slot in title memory.
        let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
        // Generations start at 1, so a made slot's key is never 0.
        let key = u64::from(generation) << 32 | u64::from(i);
        // Lock-free cache-hit fast path: `made[index]` is stored with
        // `Ordering::Release` only after `make()` and `keep()` complete.
        if tag == VIEW_TAG
            && key != 0
            && self.made[index as usize].load(Ordering::Acquire) == key
            && let Some(view) = view_at(i, generation)
        {
            return Some((view, None));
        }

        let mut needed_stripes = vec![Self::stripe(index)];
        let (guards, view, key, old_key, stale) = loop {
            let guards: Vec<_> = needed_stripes
                .iter()
                .map(|&stripe| {
                    self.slot_locks[stripe]
                        .0
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                })
                .collect();
            // Re-read the title slot under the stripe locks in case another writer
            // updated both the slot and `made[index]` while we waited.
            // SAFETY: the handle is a descriptor slot in title memory.
            let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
            let key = u64::from(generation) << 32 | u64::from(i);
            let old_key = self.made[index as usize].load(Ordering::Acquire);
            let view = (tag == VIEW_TAG).then(|| view_at(i, generation)).flatten();
            let Some(view) = view else {
                // Not a view of ours (never written, or of a resource the
                // title released): nothing to read.
                if old_key != 0 {
                    self.made[index as usize].store(0, Ordering::Release);
                    let displaced = self.make_null_keeping(index);
                    let _ = self.note_buffer_checked(index, None, false, Some(&needed_stripes), record_change, batch.as_deref_mut());
                    drop(guards);
                    if let Some(old) = displaced {
                        crate::host::retire(old);
                    }
                } else if for_draw || self.addresses.get().is_some() {
                    // Left for later and stale since: out of the sets and the stride table.
                    let _ = self.note_buffer_checked(index, None, false, Some(&needed_stripes), record_change, batch.as_deref_mut());
                }
                return None;
            };
            if old_key == key {
                return Some((view, None));
            }
            match self.note_buffer_checked(index, Some(&view), for_draw, Some(&needed_stripes), record_change, batch.as_deref_mut()) {
                Ok(stale) => break (guards, view, key, old_key, stale),
                Err(missing) => {
                    drop(guards);
                    needed_stripes.extend(missing);
                    needed_stripes.sort_unstable();
                    needed_stripes.dedup();
                }
            }
        };

        // Views of other textures in the memory this one describes are stale:
        // their host slots read nothing from here on, and let go of the host
        // copies they held. Every stale slot's stripe lock is already held in
        // `guards` in ascending stripe order, eliminating AB-BA deadlocks and
        // concurrent overwrite races between `note_buffer` and `unmake`.
        let mut displaced = Vec::new();
        for st in stale {
            if st != index
                && let Some(old) = self.unmake_locked(st)
            {
                displaced.push(old);
            }
        }
        // A view of a texture object that has no host copy yet waits
        // for it, as buffer views wait for a draw: the slot is made
        // when the texture is first used (`super::make_waiting`), or
        // here when a draw reads the slot through a table.
        let waiting = if for_draw { None } else { view.waiting_texture() };
        if let Some(texture) = waiting {
            super::wait_for(texture, handle);
        }
        let later = !for_draw && (view.is_buffer() || waiting.is_some());
        // Another view of the placed texture the slot shows already (a
        // title that streams a texture in writes its view again with
        // more mips each time some arrive): the slot goes on showing
        // the old view until a draw or the texture pass makes the new
        // one and uploads what it shows. Made null here, the texture
        // read as nothing for as long as it waited in the queue, every
        // time it got sharper.
        if later
            && keep_views()
            && old_key != 0
            && view_at(old_key as u32, (old_key >> 32) as u32)
                .is_some_and(|old| same_placed_texture(&old, &view))
        {
            KEPT_VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            drop(guards);
            Self::retire_all(displaced);
            return Some((view, None));
        }
        let mut resolved = None;
        if old_key != 0 {
            self.made[index as usize].store(0, Ordering::Release);
        }
        if later {
            // A slot never made is null already and holds no resource: it is
            // not written or let go again (`replace_kept` takes a stripe lock).
            if old_key != 0 {
                if !self.sampler {
                    null_for(&view, self.cpu(index));
                }
                if let Some(old) = self.replace_kept(index, None) {
                    displaced.push(old);
                }
            }
        } else {
            let (kept, copy) = make(&view, self.cpu(index)).unwrap_or((None, None));
            resolved = copy;
            if kept.is_none() && !self.sampler {
                null_for(&view, self.cpu(index));
            }
            if let Some(old) = self.replace_kept(index, kept) {
                displaced.push(old);
            }
            self.made[index as usize].store(key, Ordering::Release);
        }
        drop(guards);
        Self::retire_all(displaced);
        Some((view, resolved))
    }

    /// Whether slot `index` is backed by a made host descriptor (tests).
    #[cfg(test)]
    pub(crate) fn made(&self, index: u32) -> bool {
        self.made
            .get(index as usize)
            .is_some_and(|m| m.load(Ordering::Acquire) != 0)
    }

    /// Whether host slot `index` shows `buffer`; `None` if it shows no
    /// resource (diagnostics: a buffer view's slot must show the host copy
    /// that is brought up to date for it).
    #[cfg(feature = "diagnostics")]
    pub(crate) fn shows(&self, index: u32, buffer: &ID3D12Resource) -> Option<bool> {
        let (stripe, local) = self.kept_slot(index)?;
        let kept = self.kept[stripe].0.lock().unwrap_or_else(|e| e.into_inner());
        let shown = kept.get(local)?.as_ref()?;
        Some(windows::core::Interface::as_raw(shown) == windows::core::Interface::as_raw(buffer))
    }

    /// What the queue of textures picked by index knows of title slot
    /// `index` at `frame`, in a few words (diagnostics).
    #[cfg(feature = "diagnostics")]
    pub(crate) fn texture_slot_state(&self, index: u32, frame: u64) -> String {
        let textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        match textures.slots.get(&index) {
            None => "not queued".into(),
            Some(w) => format!(
                "queued due={} written@{} looks={} part_mapped={} recheck={}",
                if w.due <= frame { "now".to_string() } else { format!("@{}", w.due) },
                w.written,
                w.looks,
                w.mapped,
                w.checked.2 != 0
            ),
        }
    }

    /// The view host slot `index` was made from, in a few words, with the
    /// title and host addresses of a buffer view (diagnostics). "null" for
    /// a slot never made or left null.
    #[cfg(feature = "diagnostics")]
    pub(crate) fn describe_slot(&self, index: u32) -> Option<String> {
        let key = self.made.get(index as usize)?.load(Ordering::Acquire);
        if key == 0 {
            return Some("null".into());
        }
        let view = view_at(key as u32, (key >> 32) as u32)?;
        let mut text: String = format!("{view:?}").chars().take(120).collect();
        if let Some((address, len)) = buffer_range(&view) {
            let host = crate::shadow::find(address, len.max(1)).map(|s| s.gpu(address));
            text.push_str(&format!(" title {address:#x}+{len:#x} host {host:#x?}"));
        }
        Some(text)
    }
}

/// Logs `what` the first time it happens.
macro_rules! once {
    ($($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!($($arg)*));
    }};
}

/// A view description from its words, with the format as the host has it.
fn srv_desc(words: &[u32; 10]) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    let mut words = *words;
    words[0] = crate::resource::pc_format(words[0]);
    // SAFETY: 40 bytes in the host's layout.
    unsafe {
        std::ptr::from_ref(&words)
            .cast::<D3D12_SHADER_RESOURCE_VIEW_DESC>()
            .read_unaligned()
    }
}

fn uav_desc(words: &[u32; 10]) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    let mut words = *words;
    words[0] = crate::resource::pc_format(words[0]);
    // SAFETY: 40 bytes in the host's layout.
    unsafe {
        std::ptr::from_ref(&words)
            .cast::<D3D12_UNORDERED_ACCESS_VIEW_DESC>()
            .read_unaligned()
    }
}

/// A buffer view of title memory at `address` on its host copy: the first
/// element moves by the copy's offset.
/// Bytes per element of a buffer view.
fn element_size(desc: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> u64 {
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    if buffer.StructureByteStride > 0 {
        u64::from(buffer.StructureByteStride)
    } else if buffer.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0 {
        4
    } else {
        crate::resource::element(desc.Format.0 as u32).map_or(0, |(bytes, _)| bytes)
    }
}

/// D3D12 gives an acceleration structure view a null resource and the
/// structure's GPU address in the view's union: here the guest address the
/// title supplied (from dexnom/VIctory 5f4c3df4).
fn acceleration_location(view: &View) -> Option<u64> {
    let View::Shader(0, Some(words)) = view else { return None };
    if words[1] != D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE.0 as u32 {
        return None;
    }
    let desc = srv_desc(words);
    // SAFETY: the acceleration structure member of the union.
    let address = unsafe { desc.Anonymous.RaytracingAccelerationStructure.Location };
    (address != 0).then_some(address)
}

/// The title address a view names: an acceleration structure's, or a
/// buffer view's first byte.
pub(crate) fn guest_srv_address(view: &View) -> Option<u64> {
    acceleration_location(view)
        .or_else(|| buffer_range(view).and_then(|(address, bytes)| (address != 0 && bytes != 0).then_some(address)))
}

/// The title memory a buffer shader or unordered access view reads: its
/// address and length.
pub(crate) fn buffer_range(view: &View) -> Option<(u64, u64)> {
    let (base, words): (u64, &[u32; 10]) = match view {
        View::Shader(r, Some(desc)) => (
            crate::resource::buffer_bytes(*r as crate::object::This)?.0 as u64,
            desc,
        ),
        View::Placed(false, resource, desc) if resource[0] == 1 => (
            u64::from(desc[10]) | u64::from(desc[11]) << 32,
            desc[..10].try_into().ok()?,
        ),
        View::Unordered(r, _, Some(desc)) => {
            let base = crate::resource::buffer_bytes(*r as crate::object::This)?.0 as u64;
            return unordered_range(base, desc);
        }
        View::Placed(true, resource, desc) if resource[0] == 1 => {
            let base = u64::from(desc[10]) | u64::from(desc[11]) << 32;
            return unordered_range(base, desc[..10].try_into().ok()?);
        }
        _ => return None,
    };
    let desc = srv_desc(words);
    let element = element_size(&desc);
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    Some((
        base + buffer.FirstElement * element,
        u64::from(buffer.NumElements) * element,
    ))
}

/// The title memory the host slot of buffer view `view` is made on
/// (address, bytes): the whole buffer the view is of, which is what
/// [`make`] finds the host copy by. [`buffer_range`] is the part of it the
/// view reads.
pub(crate) fn buffer_extent(view: &View) -> Option<(u64, u64)> {
    match view {
        View::Shader(r, Some(_)) | View::Unordered(r, _, Some(_)) => {
            let (data, len) = crate::resource::buffer_bytes(*r as crate::object::This)?;
            Some((data as u64, len))
        }
        View::Placed(_, resource, desc) if resource[0] == 1 => Some((
            u64::from(desc[10]) | u64::from(desc[11]) << 32,
            u64::from(resource[4]) | u64::from(resource[5]) << 32,
        )),
        _ => None,
    }
}

/// Whether a buffer view is brought up to date in the host copy its slot
/// shows ([`buffer_copy`]). On by default; `VICTORY_VIEW_EXTENT=0` looks the
/// copy up by the view's range alone, as before.
fn view_extent() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_VIEW_EXTENT").is_none_or(|v| v != "0"))
}

/// The host copy buffer view `view` reads, and the title range (address,
/// bytes) it reads there; `None` for other views and empty ranges.
///
/// The copy is the one the view's slot shows: [`make`] finds it by the
/// whole buffer the view is of ([`buffer_extent`]). Looked up by the range
/// the view reads, as it was, the copy is another one wherever the two
/// lookups differ: for memory outside buffer objects, a view of a part of a
/// buffer that crosses a window line (the buffer's window is two cells, the
/// part's one), and for buffer objects, a part that also lies in a smaller
/// buffer object. The slot then showed a copy nothing brought up to date,
/// and what shaders wrote through it was looked for in the other copy.
pub(crate) fn buffer_copy(view: &View) -> Option<BufferCopy> {
    let (address, len) = buffer_range(view)?;
    if len == 0 {
        return None;
    }
    // A view off its buffer's elements is made on a copy of its own bytes
    // (`own_bytes`): the same question `buffer_srv` and `buffer_uav` ask,
    // of the copy they find, so that the copy brought up to date is the
    // one the slot shows.
    // The whole buffer the view is of, and the copy made on it: asked once
    // for both questions below.
    let extent = buffer_extent(view);
    let whole = extent.and_then(|(base, width)| crate::shadow::find(base, width));
    if let (Some((base, _)), Some(whole_ref)) = (extent, whole.as_ref())
        && let Some((first, count, element, stride, unknown)) = buffer_fields(view)
        && let Some((start, bytes)) = own_bytes(whole_ref.offset(base), base, first, count, element, stride, unknown)
        && let Some(own) = crate::shadow::exact(start, bytes)
    {
        return Some((own, start, bytes));
    }
    if view_extent()
        && let Some((base, width)) = extent
        // (The console is lax: a view may begin past its buffer's end.)
        && address >= base
        && address - base < width.max(1)
        && let Some(whole) = whole
    {
        return Some((whole, address, len));
    }
    crate::shadow::find(address, len).map(|shadow| (shadow, address, len))
}

/// [`buffer_range`] of an unordered access view of memory at `base`.
fn unordered_range(base: u64, words: &[u32; 10]) -> Option<(u64, u64)> {
    let desc = uav_desc(words);
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    let element = if buffer.StructureByteStride > 0 {
        u64::from(buffer.StructureByteStride)
    } else if buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
        4
    } else {
        crate::resource::element(desc.Format.0 as u32)?.0
    };
    Some((
        base + buffer.FirstElement * element,
        u64::from(buffer.NumElements) * element,
    ))
}

/// The host resource a slot made by [`make`] views, if any.
type Kept = Option<ID3D12Resource>;

/// The host copy a buffer view reads, and the title range (address, bytes)
/// it reads there: what [`buffer_copy`] reports for it.
pub(crate) type BufferCopy = (std::sync::Arc<crate::shadow::Shadow>, u64, u64);

/// Whether the pass over records copied in as bytes also mirrors slots
/// that draws read through tables lately (`VICTORY_HEAP_SYNC_TABLES=1`;
/// off by default: [`HostHeap::mirror_copied`]).
fn sync_tables() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_SYNC_TABLES").is_some_and(|v| v == "1"))
}

/// Since the last frame line: slots on changed heap pages that
/// [`HostHeap::mirror_copied`] left to their draws (tables' slots).
pub(crate) static COPIED_TABLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A 64-bit fingerprint of a descriptor record's bytes (16 or 32 of them):
/// equal records give equal prints, and two different records the same
/// print about once in 2^64. Not a hash anyone can attack: the records
/// are our own.
fn record_print(record: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut h = K ^ record.len() as u64;
    let mut words = record.chunks_exact(8);
    for word in &mut words {
        let w = u64::from_le_bytes(word.try_into().unwrap_or([0; 8]));
        h = (h ^ w).wrapping_mul(K);
        h ^= h >> 29;
    }
    for &b in words.remainder() {
        h = (h ^ u64::from(b)).wrapping_mul(K);
    }
    h ^ (h >> 32)
}

/// Whether a buffer view off its buffer's elements gets a copy of its own
/// bytes (`VICTORY_VIEW_OWN_COPY=0`: it is refused and reads zeros, as
/// before).
fn own_copy_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_VIEW_OWN_COPY").is_none_or(|v| v != "0"))
}

/// The title memory (start, bytes) of a buffer view that cannot be made on
/// the host copy of its buffer: `count` elements of `element` bytes from
/// element `first` of the buffer at `base`, which lies `offset` bytes into
/// that copy, where `offset` is not a whole number of elements and the view
/// cannot be a raw view of the same bytes either ([`raw_window`]:
/// `unknown`, the view has no format, and `stride` is its structure's).
/// The host counts a view's first element from its buffer's start, so such
/// a view has no host view there; it is made on a copy of its own bytes
/// (`crate::shadow::exact`), where it starts at element 0. `None`: the
/// view is made on its buffer's copy, as every other.
fn own_bytes(offset: u64, base: u64, first: u64, count: u32, element: u64, stride: u32, unknown: bool) -> Option<(u64, u64)> {
    own_bytes_if(own_copy_on(), offset, base, first, count, element, stride, unknown)
}

#[allow(clippy::too_many_arguments)]
fn own_bytes_if(
    on: bool,
    offset: u64,
    base: u64,
    first: u64,
    count: u32,
    element: u64,
    stride: u32,
    unknown: bool,
) -> Option<(u64, u64)> {
    if !on || element == 0 || count == 0 || offset.is_multiple_of(element) {
        return None;
    }
    if unknown && raw_window(offset, first, count, stride).is_some() {
        return None;
    }
    let start = base.checked_add(first.checked_mul(element)?)?;
    Some((start, u64::from(count) * element))
}

/// Of a buffer view: its first element, its elements, the bytes of one (a
/// raw view's: 4), its structure's stride, and whether it has no format.
/// As [`buffer_srv`] and [`buffer_uav`] read them from the same words.
fn buffer_fields(view: &View) -> Option<(u64, u32, u64, u32, bool)> {
    const BUFFER: u32 = 1;
    let (words, unordered): (&[u32; 10], bool) = match view {
        View::Shader(_, Some(d)) if d[1] == BUFFER => (d, false),
        View::Unordered(_, _, Some(d)) if d[1] == BUFFER => (d, true),
        View::Placed(unordered, resource, d) if resource[0] == BUFFER => (d[..10].try_into().ok()?, *unordered),
        _ => return None,
    };
    if unordered {
        let d = uav_desc(words);
        // SAFETY: a buffer view's union member.
        let b = unsafe { d.Anonymous.Buffer };
        let element = if b.StructureByteStride > 0 {
            u64::from(b.StructureByteStride)
        } else if b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
            4
        } else {
            crate::resource::element(d.Format.0 as u32).map_or(0, |(bytes, _)| bytes)
        };
        Some((b.FirstElement, b.NumElements, element, b.StructureByteStride, d.Format == DXGI_FORMAT_UNKNOWN))
    } else {
        let d = srv_desc(words);
        // SAFETY: a buffer view's union member.
        let b = unsafe { d.Anonymous.Buffer };
        Some((b.FirstElement, b.NumElements, element_size(&d), b.StructureByteStride, d.Format == DXGI_FORMAT_UNKNOWN))
    }
}

/// A structured buffer view (`first`, `count` elements of `stride` bytes,
/// `offset` bytes into its host copy) as a raw view of the same bytes:
/// its first dword and its dwords. Translated shaders read untyped buffers
/// as raw bytes at `element * stride + offset` (the stride from the
/// used-resource entry, or from the table by heap slot for a buffer picked
/// by index), and a raw read through a structured view gives zeros on
/// NVIDIA (reported upstream with PR #193; structured reads through raw
/// views were right there). So the host's view is raw whenever it can be:
/// the bytes must start on 16 (the host's raw view alignment) and be whole
/// dwords. A view that cannot stays structured.
fn raw_window(offset: u64, first: u64, count: u32, stride: u32) -> Option<(u64, u32)> {
    if stride == 0 {
        return None;
    }
    let start = first.checked_mul(u64::from(stride))?.checked_add(offset)?;
    let bytes = u64::from(count) * u64::from(stride);
    (start % 16 == 0 && bytes % 4 == 0)
        .then(|| u32::try_from(bytes / 4).ok().map(|dwords| (start / 4, dwords)))
        .flatten()
}

/// Whether the host accepts a buffer view with these fields of a buffer of
/// `width` bytes. It removes the device for one it does not (view creation
/// returns nothing), and the console is laxer: the title's heap holds
/// views past their memory's end, of no elements, of typeless formats
/// without the raw flag, with strides the host has no structured buffers
/// for. Since every buffer view of the heap is made for shaders that pick
/// them by index, one such view ended the run (2026-11-24, run 10).
fn buffer_view_ok(format: DXGI_FORMAT, first: u64, count: u32, stride: u32, raw: bool, width: u64) -> bool {
    use crate::resource::family;
    let f = format.0 as u32;
    let element = if raw {
        // raw: R32 typeless, counted in dwords, starting on 16 bytes
        if format != DXGI_FORMAT_R32_TYPELESS || stride != 0 || !first.is_multiple_of(4) {
            return false;
        }
        4
    } else if stride > 0 {
        // structured: no format, a stride the host takes
        if format != DXGI_FORMAT_UNKNOWN || stride > 2048 || !stride.is_multiple_of(4) {
            return false;
        }
        u64::from(stride)
    } else {
        // typed: a format with a size that is not typeless
        let typeless = family(f) == f && family(f + 1) == f;
        match crate::resource::element(f) {
            Some((bytes, 1)) if f != 0 && !typeless && bytes > 0 => bytes,
            _ => return false,
        }
    };
    count > 0
        && first
            .checked_add(u64::from(count))
            .and_then(|end| end.checked_mul(element))
            .is_some_and(|end| end <= width)
}

/// The bytes of a constant buffer view of `size` bytes: whole 256s, and at
/// most the 64 KiB a shader can index. The host removes the device for a
/// larger one, and the title's heap holds old views of any size in slots it
/// does not use.
fn constant_size(size: u32) -> u32 {
    size.min(64 << 10).next_multiple_of(256)
}

/// How many of `count` elements of `element` bytes from `first` on lie in
/// `width` bytes: the console lets a view reach past its buffer's end, the
/// host refuses one. A view with no element inside keeps its count (and is
/// refused).
fn elements_inside(first: u64, count: u32, element: u64, width: u64) -> u32 {
    let room = (width / element.max(1)).saturating_sub(first);
    match u32::try_from(room) {
        Ok(0) | Err(_) => count,
        Ok(room) => count.min(room),
    }
}

/// Diagnostics: a buffer view left null for shaders that pick views by
/// index, and why. Told at each doubling of the distinct views of a kind.
fn left_null(why: &'static str, unordered: bool, address: u64, len: u64, element: u64, first: u64) {
    if !cfg!(feature = "diagnostics") {
        return;
    }
    type Seen = std::collections::BTreeMap<(&'static str, bool, u64), std::collections::HashSet<(u64, u64)>>;
    static SEEN: Mutex<Seen> = Mutex::new(Seen::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let views = seen.entry((why, unordered, element)).or_default();
    if views.insert((address, len)) && views.len().is_power_of_two() {
        tracing::warn!(
            views = views.len(),
            unordered,
            element,
            first,
            len,
            address = format_args!("{address:#x}"),
            "buffer views left null: {why}"
        );
    }
}

fn buffer_srv(
    address: u64,
    len: u64,
    mut desc: D3D12_SHADER_RESOURCE_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
    buffer_view: bool,
) -> Option<(Kept, Option<BufferCopy>)> {
    let whole = crate::shadow::find(address, len)?;
    let offset = whole.offset(address);
    // The title range the view reads ([`buffer_range`]): what `buffer_copy`
    // reports the copy of a made slot with.
    // SAFETY: a buffer view's union member.
    let range = {
        let b = unsafe { desc.Anonymous.Buffer };
        let element = element_size(&desc);
        (address + b.FirstElement * element, u64::from(b.NumElements) * element)
    };
    // Off the elements of that copy: on a copy of its own bytes, from its
    // element 0 (`own_bytes`).
    let own = buffer_view.then_some(()).and_then(|()| {
        // SAFETY: a buffer view's union member.
        let b = unsafe { desc.Anonymous.Buffer };
        let unknown = desc.Format == DXGI_FORMAT_UNKNOWN;
        own_bytes(offset, address, b.FirstElement, b.NumElements, element_size(&desc), b.StructureByteStride, unknown)
            .and_then(|(start, bytes)| crate::shadow::exact(start, bytes).map(|own| (own, start, bytes)))
    });
    let (shadow, offset, own_copy) = match own {
        Some((own, start, bytes)) => {
            desc.Anonymous.Buffer.FirstElement = 0;
            (own, 0, Some((start, bytes)))
        }
        None => (whole, offset, None),
    };
    // A structured view as a raw view of the same bytes (see `raw_window`).
    // SAFETY: a buffer view's union member.
    let structured = unsafe { desc.Anonymous.Buffer };
    let window = (desc.Format == DXGI_FORMAT_UNKNOWN)
        .then(|| raw_window(offset, structured.FirstElement, structured.NumElements, structured.StructureByteStride))
        .flatten();
    if let Some((first, count)) = window {
        desc.Format = DXGI_FORMAT_R32_TYPELESS;
        desc.Anonymous.Buffer = D3D12_BUFFER_SRV {
            FirstElement: first,
            NumElements: count,
            StructureByteStride: 0,
            Flags: D3D12_BUFFER_SRV_FLAG_RAW,
        };
    }
    let element = element_size(&desc);
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { &mut desc.Anonymous.Buffer };
    if window.is_none() {
        if element == 0 || offset % element != 0 {
            left_null("not on an element of their host copy", false, address, len, element, buffer.FirstElement);
            return None;
        }
        buffer.FirstElement += offset / element;
    }
    // SAFETY: a live buffer.
    let width = unsafe { shadow.buffer.GetDesc() }.Width;
    let unit = if window.is_some() { 4 } else { element };
    buffer.NumElements = elements_inside(buffer.FirstElement, buffer.NumElements, unit, width);
    let (first, count, stride) = (buffer.FirstElement, buffer.NumElements, buffer.StructureByteStride);
    let raw = buffer.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0;
    if !buffer_view_ok(desc.Format, first, count, stride, raw, width) {
        REFUSED_VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let why = if raw { "raw, refused by the host" } else { "refused by the host" };
        left_null(why, false, address, len, element, first);
        once!(
            format = desc.Format.0,
            first,
            count,
            stride,
            raw,
            width,
            "buffer view the host would refuse left null"
        );
        return None;
    }
    // The console has no component mapping; one the host does not know is the default's.
    if desc.Shader4ComponentMapping != D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING {
        desc.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    }
    // The copy this slot now shows, as `buffer_copy` reports it: a copy of
    // the view's own bytes, else the whole buffer's, which the view lies in
    // (`VICTORY_VIEW_EXTENT=0` looks copies up by the view's range alone).
    let copy = match own_copy {
        Some((start, bytes)) => Some((shadow.clone(), start, bytes)),
        None if view_extent() && range.1 > 0 && range.0 >= address && range.0 - address < len.max(1) => {
            Some((shadow.clone(), range.0, range.1))
        }
        None => None,
    };
    let host = host()?;
    // SAFETY: a live buffer and a slot of our heap.
    unsafe {
        host.device
            .CreateShaderResourceView(&shadow.buffer, Some(&desc), at)
    };
    Some((Some(shadow.buffer.clone()), copy))
}

/// An unordered access view of title buffer memory at `address` (`len`
/// bytes) on its host copy, as [`buffer_srv`]; false if the copy cannot be
/// written by shaders (upload memory).
/// shortcut: what shaders write stays in the host copy; title memory (and
/// so the CPU, and copies run on the CPU) does not see it.
fn buffer_uav(
    address: u64,
    len: u64,
    mut desc: D3D12_UNORDERED_ACCESS_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
    buffer_view: bool,
) -> Option<(Kept, Option<BufferCopy>)> {
    let whole = crate::shadow::find(address, len)?;
    // The title range the view reads ([`buffer_range`]: `None` where it finds
    // none): what `buffer_copy` reports the copy of a made slot with.
    let range = {
        // SAFETY: a buffer view's union member.
        let b = unsafe { desc.Anonymous.Buffer };
        let element = if b.StructureByteStride > 0 {
            Some(u64::from(b.StructureByteStride))
        } else if b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
            Some(4)
        } else {
            crate::resource::element(desc.Format.0 as u32).map(|(bytes, _)| bytes)
        };
        element.map(|element| (address + b.FirstElement * element, u64::from(b.NumElements) * element))
    };
    // Off the elements of that copy: on a copy of its own bytes, from its
    // element 0 (`own_bytes`).
    let own = buffer_view.then_some(()).and_then(|()| {
        // SAFETY: a buffer view's union member.
        let b = unsafe { desc.Anonymous.Buffer };
        let element = if b.StructureByteStride > 0 {
            u64::from(b.StructureByteStride)
        } else if b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
            4
        } else {
            crate::resource::element(desc.Format.0 as u32).map_or(0, |(bytes, _)| bytes)
        };
        let unknown = desc.Format == DXGI_FORMAT_UNKNOWN;
        own_bytes(whole.offset(address), address, b.FirstElement, b.NumElements, element, b.StructureByteStride, unknown)
            .and_then(|(start, bytes)| crate::shadow::exact(start, bytes).map(|own| (own, start, bytes)))
    });
    let (shadow, own_copy) = match own {
        Some((own, start, bytes)) => {
            desc.Anonymous.Buffer.FirstElement = 0;
            (own, Some((start, bytes)))
        }
        None => (whole, None),
    };
    let own = own_copy.is_some();
    // SAFETY: a live buffer.
    let flags = unsafe { shadow.buffer.GetDesc() }.Flags;
    if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
        once!("unordered access to a buffer whose host copy shaders cannot write");
        return None;
    }
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { &mut desc.Anonymous.Buffer };
    let element = if buffer.StructureByteStride > 0 {
        u64::from(buffer.StructureByteStride)
    } else if buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
        4
    } else {
        crate::resource::element(desc.Format.0 as u32).map_or(0, |(bytes, _)| bytes)
    };
    let offset = if own { 0 } else { shadow.offset(address) };
    // A structured view as a raw view of the same bytes (see `raw_window`).
    let window = (desc.Format == DXGI_FORMAT_UNKNOWN)
        .then(|| raw_window(offset, buffer.FirstElement, buffer.NumElements, buffer.StructureByteStride))
        .flatten();
    if let Some((first, count)) = window {
        buffer.FirstElement = first;
        buffer.NumElements = count;
        buffer.StructureByteStride = 0;
        buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
    } else {
        if element == 0 || offset % element != 0 {
            left_null("not on an element of their host copy", true, address, len, element, buffer.FirstElement);
            return None;
        }
        buffer.FirstElement += offset / element;
    }
    buffer.CounterOffsetInBytes = 0;
    // SAFETY: a live buffer.
    let width = unsafe { shadow.buffer.GetDesc() }.Width;
    let unit = if window.is_some() { 4 } else { element };
    buffer.NumElements = elements_inside(buffer.FirstElement, buffer.NumElements, unit, width);
    let (first, count, stride) = (buffer.FirstElement, buffer.NumElements, buffer.StructureByteStride);
    let raw = buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0;
    if window.is_some() {
        // (Left out at first: every structured unordered access view was then
        // refused by `buffer_view_ok` and left null, runs 15 and 16.)
        desc.Format = DXGI_FORMAT_R32_TYPELESS;
    }
    if !buffer_view_ok(desc.Format, first, count, stride, raw, width) {
        REFUSED_VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let why = if raw { "raw, refused by the host" } else { "refused by the host" };
        left_null(why, true, address, len, element, first);
        once!(
            format = desc.Format.0,
            first,
            count,
            stride,
            raw,
            width,
            "unordered buffer view the host would refuse left null"
        );
        return None;
    }
    let host = host()?;
    // SAFETY: a live buffer and a slot of our heap.
    unsafe {
        host.device
            .CreateUnorderedAccessView(&shadow.buffer, None::<&ID3D12Resource>, Some(&desc), at)
    };
    // The copy this slot now shows, as `buffer_copy` reports it: a copy of
    // the view's own bytes, else the whole buffer's, which the view lies in
    // (`VICTORY_VIEW_EXTENT=0` looks copies up by the view's range alone).
    let copy = match (own_copy, range) {
        (Some((start, bytes)), _) => Some((shadow.clone(), start, bytes)),
        (None, Some((from, bytes)))
            if view_extent() && bytes > 0 && from >= address && from - address < len.max(1) =>
        {
            Some((shadow.clone(), from, bytes))
        }
        _ => None,
    };
    Some((Some(shadow.buffer.clone()), copy))
}

/// Whether the host can make a shader (or unordered access) view of
/// `texture` in `format` (none: the texture's own). The console allows any
/// view; the host refuses (and removes the device for) views without the
/// resource's permission, of typed depth, and in another format than a
/// typed resource's. A view of a typed texture in another format of its
/// family takes the texture's format.
/// shortcut: such a view (sRGB of UNORM, say) reads without its
/// conversion; cast with castable formats if colors come out wrong.
fn fits(texture: &ID3D12Resource, mut format: Option<&mut DXGI_FORMAT>, unordered: bool) -> bool {
    use crate::resource::{DEPTH_FORMATS, family};
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    let allowed = if unordered {
        d.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0
    } else {
        d.Flags.0 & D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE.0 == 0
    };
    let own = d.Format.0 as u32;
    if !allowed || DEPTH_FORMATS.iter().any(|&(typed, _)| typed == own) {
        if unordered {
            crate::resource::DEPTH_UNORDERED[3].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        once!(
            unordered,
            format = own,
            flags = d.Flags.0,
            "view the host does not allow"
        );
        return false;
    }
    // Every family has more than one format, its typeless one first.
    let typeless = family(own) == own && family(own + 1) == own;
    if let Some(format) = format.as_deref_mut()
        && unordered
        && let Some(&(_, linear)) = crate::resource::SRGB
            .iter()
            .find(|&&(s, _)| s == format.0 as u32)
    {
        format.0 = linear as i32;
    }
    let view = format.as_ref().map_or(0, |f| f.0 as u32);
    if view == 0 {
        return !typeless;
    }
    // A view of a target whose format the host refuses, in the title's
    // format: in the stand-in the host copy has.
    if view != own && crate::resource::target_format(view) == own {
        if let Some(format) = format {
            *format = d.Format;
        }
        return true;
    }
    if family(view) != family(own) {
        once!(view, own, "view format of another family");
        return false;
    }
    if let Some(format) = format
        && !typeless
    {
        *format = d.Format;
    }
    true
}

/// The title's unordered access view of a depth texture, as a view of the
/// texture's colour twin `twin` ([`crate::resource::Twin`]): the same mip
/// and slices, in the colour format of the twin's family for a depth or
/// typeless format (R16_UNORM for 16-bit depth, R32_FLOAT for 32-bit; the
/// bits are the same). Without a description: mip 0 of every slice.
/// The twin of a stencil plane (R8) stands for the views of that plane
/// only, which become R8_UINT views of its one plane; `None` for a view of
/// the depth plane, which the twin does not stand for. The depth plane has
/// a twin of its own, viewed as a depth texture's without stencil: a view
/// in the format of the depth plane of a depth-stencil format (21, 46)
/// takes the twin's colour format too.
fn twin_uav(
    twin: &ID3D12Resource,
    desc: Option<&D3D12_UNORDERED_ACCESS_VIEW_DESC>,
) -> Option<D3D12_UNORDERED_ACCESS_VIEW_DESC> {
    // SAFETY: a live resource.
    let d = unsafe { twin.GetDesc() };
    let mut desc = desc.cloned().unwrap_or_else(|| {
        let mut all = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        if d.DepthOrArraySize > 1 {
            all.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
            all.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV {
                ArraySize: u32::from(d.DepthOrArraySize),
                ..Default::default()
            };
        }
        all
    });
    // A stencil plane's twin (R8 typeless, 60): the title's view is of the
    // stencil plane if it names plane 1 or a stencil view format (22, 47).
    if d.Format.0 == 60 {
        let mut plane = 0;
        match desc.ViewDimension {
            D3D12_UAV_DIMENSION_TEXTURE2D => {
                // SAFETY: the member the dimension names.
                let mut view = unsafe { desc.Anonymous.Texture2D };
                plane = std::mem::take(&mut view.PlaneSlice);
                desc.Anonymous.Texture2D = view;
            }
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => {
                // SAFETY: the member the dimension names.
                let mut view = unsafe { desc.Anonymous.Texture2DArray };
                plane = std::mem::take(&mut view.PlaneSlice);
                desc.Anonymous.Texture2DArray = view;
            }
            _ => {}
        }
        if plane != 1 && !matches!(desc.Format.0, 22 | 47) {
            return None;
        }
        desc.Format.0 = 62;
        return Some(desc);
    }
    // None, typeless and depth formats (R16: 0, 53, 55; R32: 0, 39, 40),
    // and those of a depth-stencil format's depth plane (19 to 21, 44 to
    // 46), for the twin of such a plane.
    desc.Format.0 = match (desc.Format.0, d.Format.0) {
        (0 | 53 | 55 | 19..=21 | 44..=46, 53) => 56,
        (0 | 39 | 40 | 19..=21 | 44..=46, 39) => 41,
        (other, _) => other,
    };
    Some(desc)
}

/// Makes the host descriptor for `view` at `at`: the host resource it
/// views, or `None` if it cannot.
/// A buffer view made now also reports the host copy it reads
/// ([`BufferCopy`]), so that the pass that mirrors it
/// ([`HostHeap::mirror_heap_buffer`]) need not look the copy up again.
fn make(view: &View, at: D3D12_CPU_DESCRIPTOR_HANDLE) -> Option<(Kept, Option<BufferCopy>)> {
    let host = host()?;
    let device = &host.device;
    match view {
        &View::Constant(address, size) => {
            let shadow = crate::shadow::find(address, u64::from(size))?;
            let gpu = shadow.gpu(address);
            if gpu % 256 != 0 {
                once!(
                    address = format_args!("{address:#x}"),
                    "constant buffer not 256-byte aligned on the host"
                );
                return None;
            }
            let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
                BufferLocation: gpu,
                SizeInBytes: constant_size(size),
            };
            // SAFETY: a slot of our heap; the range lies in the host copy.
            unsafe { device.CreateConstantBufferView(Some(&desc), at) };
            Some((Some(shadow.buffer.clone()), None))
        }
        View::Shader(resource, desc) => {
            let resource = *resource as crate::object::This;
            if let Some(texture) = crate::resource::host_resource(resource) {
                // The title's views of textures with a stencil plane, as it
                // describes them (format, dimension, component mapping, a
                // pad, then first level, levels, plane): each distinct one
                // once. They say how its shaders read the stencil plane
                // (the lighting takes each pixel's material class and its
                // "inside a room" bit from it).
                #[cfg(feature = "diagnostics")]
                {
                    // SAFETY: a live resource.
                    let own = unsafe { texture.GetDesc() }.Format.0;
                    if matches!(own, 19..=22 | 44..=47) {
                        static SEEN: Mutex<Vec<(i32, Option<[u32; 10]>)>> = Mutex::new(Vec::new());
                        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
                        if seen.len() < 64 && !seen.contains(&(own, *desc)) {
                            seen.push((own, *desc));
                            tracing::info!(
                                ?resource,
                                texture_format = own,
                                view = ?desc,
                                "shader view of a depth-stencil texture"
                            );
                        }
                    }
                }
                let mut desc = desc.as_ref().map(srv_desc);
                if !fits(texture, desc.as_mut().map(|d| &mut d.Format), false) {
                    return None;
                }
                // Levels on demand (`crate::tiles`): a texture made level by
                // level is shown from the first level that is the title's
                // and has memory, if the title's view starts before it (a
                // level without memory reads zeros: a black texture). The
                // title's own view mostly starts there already.
                if let Some(first) = crate::resource::first_shown(resource)
                    && first > 0
                {
                    // SAFETY: a live resource.
                    let own = unsafe { texture.GetDesc() };
                    // The texture's own view (the title gave none): all of it.
                    let whole = D3D12_SHADER_RESOURCE_VIEW_DESC {
                        Format: own.Format,
                        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                        Shader4ComponentMapping: 5768,
                        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                            Texture2D: D3D12_TEX2D_SRV {
                                MostDetailedMip: 0,
                                MipLevels: u32::MAX,
                                PlaneSlice: 0,
                                ResourceMinLODClamp: 0.0,
                            },
                        },
                    };
                    crate::tiles::show_from(desc.get_or_insert(whole), first, u32::from(own.MipLevels));
                }
                // SAFETY: a live texture and a slot of our heap.
                unsafe {
                    device.CreateShaderResourceView(
                        texture,
                        desc.as_ref().map(std::ptr::from_ref),
                        at,
                    )
                };
                Some((Some(texture.clone()), None))
            } else if let (Some((data, len)), Some(desc)) =
                (crate::resource::buffer_bytes(resource), desc)
            {
                // Buffer views: dimension 1 (`buffer_fields`).
                const BUFFER: u32 = 1;
                buffer_srv(data as u64, len, srv_desc(desc), at, desc[1] == BUFFER)
            } else {
                None
            }
        }
        View::Unordered(resource, _counter, desc) => {
            let resource = *resource as crate::object::This;
            let Some(texture) = crate::resource::host_resource(resource) else {
                return match (crate::resource::buffer_bytes(resource), desc) {
                    (Some((data, len)), Some(desc)) => {
                        // Buffer views: dimension 1 (`buffer_fields`).
                        const BUFFER: u32 = 1;
                        buffer_uav(data as u64, len, uav_desc(desc), at, desc[1] == BUFFER)
                    }
                    _ => None,
                };
            };
            // The first such views of depth textures are logged as the
            // title describes them (format, dimension, then mip, slices and
            // plane): they say which planes shaders write, and as what.
            if crate::resource::depth_and_unordered(resource) {
                static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 512 {
                    tracing::info!(
                        ?resource,
                        view = ?desc,
                        "unordered access view of a depth texture"
                    );
                }
            }
            let mut desc = desc.as_ref().map(uav_desc);
            // A depth texture's unordered access is its colour twin's: the
            // host gives a depth texture none.
            let twin = crate::resource::twin_resource(resource)
                .and_then(|twin| Some((twin, twin_uav(twin, desc.as_ref())?)))
                // A view of the depth plane of a texture with a stencil
                // plane: of that plane's own twin.
                .or_else(|| {
                    let twin = crate::resource::twin_plane_resource(resource)?;
                    Some((twin, twin_uav(twin, desc.as_ref())?))
                });
            let texture = match twin {
                Some((twin, view)) => {
                    desc = Some(view);
                    twin
                }
                None => texture,
            };
            if !fits(texture, desc.as_mut().map(|d| &mut d.Format), true) {
                return None;
            }
            // SAFETY: a live texture and a slot of our heap.
            unsafe {
                device.CreateUnorderedAccessView(
                    texture,
                    None::<&ID3D12Resource>,
                    desc.as_ref().map(std::ptr::from_ref),
                    at,
                )
            };
            Some((Some(texture.clone()), None))
        }
        View::Sampler(words) => {
            // SAFETY: 13 words in the host's layout.
            let desc = unsafe {
                std::ptr::from_ref(words)
                    .cast::<D3D12_SAMPLER_DESC>()
                    .read_unaligned()
            };
            // SAFETY: a slot of our sampler heap.
            unsafe { device.CreateSampler(&desc, at) };
            Some((None, None))
        }
        View::ConsoleSampler(words) => {
            let desc = crate::sampler::console(words);
            // SAFETY: a slot of our sampler heap.
            unsafe { device.CreateSampler(&desc, at) };
            Some((None, None))
        }
        View::Placed(unordered, resource, desc) => {
            const BUFFER: u32 = 1;
            if *unordered && resource[0] != BUFFER {
                note_placed_uav(resource, desc);
            }
            if let Some(view) = placed_texture(view) {
                return make(&view, at);
            }
            if resource[0] != BUFFER {
                // Unordered access to a texture the title placed itself,
                // without `VICTORY_PLACED_UAV=1`: left null, what shaders
                // store there is lost.
                return None;
            }
            // The resource description's width, after the alignment.
            let len = u64::from(resource[4]) | u64::from(resource[5]) << 32;
            let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
            let words: &[u32; 10] = desc[..10].try_into().unwrap_or(&[0; 10]);
            // An empty title buffer: nothing to view, and no host copy to
            // look for (#70: such views were in flight at device removals).
            if len == 0 {
                return None;
            }
            if *unordered {
                buffer_uav(address, len, uav_desc(words), at, true)
            } else {
                buffer_srv(address, len, srv_desc(words), at, true)
            }
        }
    }
}

/// Writes of a slot with another view of the placed texture it showed,
/// since the start: the slot kept showing the old view.
pub(crate) static KEPT_VIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether a slot written with another view of the placed texture it
/// shows keeps showing the old one until the new is made;
/// `VICTORY_KEEP_VIEWS=0` makes it null at once, as before.
fn keep_views() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_KEEP_VIEWS").is_none_or(|v| v != "0"))
}

/// Whether both are shader views of one texture the title placed in
/// memory: the same address and the same resource description, whatever
/// part of it each shows.
fn same_placed_texture(a: &View, b: &View) -> bool {
    match (a, b) {
        (View::Placed(false, resource, desc), View::Placed(false, other, other_desc)) => {
            resource[0] != 1 && resource == other && desc[10..12] == other_desc[10..12]
        }
        _ => false,
    }
}

/// A shader view of a texture the title placed in memory itself, as a view
/// of the resource made for that memory (on first use).
/// With `VICTORY_PLACED_UAV=1`, unordered views of the same: as a view of
/// that resource too, so what shaders store lands where shader views of
/// that memory read it (the resource description must match: a view
/// placed in another format gets a resource of its own).
pub(crate) fn placed_texture(view: &View) -> Option<View> {
    const BUFFER: u32 = 1;
    let View::Placed(unordered, resource, desc) = view else {
        return None;
    };
    if resource[0] == BUFFER || (*unordered && !placed_uav()) {
        return None;
    }
    let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
    let texture = crate::resource::texture_at(address, resource)?;
    let words = desc[..10].try_into().ok();
    Some(if *unordered {
        View::Unordered(texture as usize, 0, words)
    } else {
        View::Shader(texture as usize, words)
    })
}

/// `VICTORY_PLACED_UAV=1`: unordered views of textures the title placed
/// itself are made (they were left null: every store through them lost).
fn placed_uav() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PLACED_UAV").is_some_and(|v| v == "1"))
}

/// Unordered views of textures the title placed itself, since the start.
pub(crate) static PLACED_UAVS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Counts such a view and logs the first distinct textures: which passes
/// store into them (depth pyramids, reflections, luminance), and whether
/// other placed textures share the memory (another description: shader
/// views of it would not see the stores even when mirrored).
fn note_placed_uav(resource: &[u32; 14], desc: &[u32; 16]) {
    PLACED_UAVS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    static SEEN: std::sync::Mutex<Vec<(u64, u32, u32)>> = std::sync::Mutex::new(Vec::new());
    let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
    let key = (address, resource[8], desc[0]);
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() >= 128 || seen.contains(&key) {
            return;
        }
        seen.push(key);
    }
    tracing::warn!(
        mirrored = placed_uav(),
        dimension = resource[0],
        width = resource[4],
        height = resource[6],
        depth_levels = format_args!("{:#x}", resource[7]),
        format = resource[8],
        flags = format_args!("{:#x}", resource[12]),
        view_format = desc[0],
        view_dimension = desc[1],
        view_mip = desc[2],
        address = format_args!("{address:#x}"),
        sharing = crate::resource::placed_textures_at(address),
        "unordered view of a texture the title placed itself (VICTORY_PLACED_UAV=1 makes it; else its stores are lost)"
    );
}

/// With `VICTORY_NULL_MAGENTA=1`, a 2D colour texture view that shows
/// nothing yet shows magenta: on the screen, a texture that is not there is
/// told from one whose contents are black. False if it is to be null.
#[cfg(feature = "diagnostics")]
fn magenta_for(desc: &[u32; 16], at: D3D12_CPU_DESCRIPTOR_HANDLE) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("VICTORY_NULL_MAGENTA").is_some_and(|v| v == "1")) {
        return false;
    }
    // Block-compressed colour (BC1 to BC3, BC7) and 8-bit RGBA and BGRA.
    let colour = matches!(desc[0], 27..=29 | 70..=78 | 87..=91 | 97..=99);
    if desc[1] != D3D12_SRV_DIMENSION_TEXTURE2D.0 as u32 || !colour {
        return false;
    }
    let (Some(host), Some(texture)) = (host(), magenta()) else {
        return false;
    };
    // SAFETY: a slot of our heap and a live texture.
    unsafe { host.device.CreateShaderResourceView(texture, None, at) };
    true
}

/// One magenta texel, written by the processor.
#[cfg(feature = "diagnostics")]
fn magenta() -> Option<&'static ID3D12Resource> {
    static TEXTURE: std::sync::OnceLock<Option<ID3D12Resource>> = std::sync::OnceLock::new();
    TEXTURE
        .get_or_init(|| {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_CUSTOM,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
                MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
                ..Default::default()
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: 1,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: valid descriptions and an out pointer; the texel is
            // written while nothing reads the texture.
            unsafe {
                host()?
                    .device
                    .CreateCommittedResource(
                        &heap,
                        D3D12_HEAP_FLAG_NONE,
                        &desc,
                        D3D12_RESOURCE_STATE_COMMON,
                        None,
                        &mut texture,
                    )
                    .ok()?;
                let texture = texture?;
                texture.Map(0, None, None).ok()?;
                let texel = [255u8, 0, 255, 255];
                texture.WriteToSubresource(0, None, texel.as_ptr().cast(), 4, 4).ok()?;
                texture.Unmap(0, None);
                Some(texture)
            }
        })
        .as_ref()
}

/// A null view of the kind `view` is, for a slot whose view the host
/// cannot make: unordered access (to a buffer or a 2D texture) or a shader
/// view. Writes to it are dropped, reads give zeros.
fn null_for(view: &View, at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let is_buffer = |r: &usize| crate::resource::buffer_bytes(*r as crate::object::This).is_some();
    // Unordered views: whether a buffer, and the title's view dimension.
    let (buffer, dimension) = match view {
        View::Unordered(r, _, desc) => (
            is_buffer(r) || desc.is_some_and(|d| d[1] == 1),
            desc.map(|d| d[1]),
        ),
        View::Placed(true, resource, desc) => (resource[0] == 1, Some(desc[1])),
        // Shader views of buffers read nothing through a null buffer view:
        // a null 2D view where the shader expects a buffer is a mismatch
        // the hardware does not survive in general (GPU-based validation
        // reports it).
        View::Shader(r, desc) if is_buffer(r) || desc.is_some_and(|d| d[1] == 1) => {
            return null_buffer_view(at);
        }
        View::Placed(false, resource, _) if resource[0] == 1 => return null_buffer_view(at),
        // Textures: a null view of the title's dimension (1D, arrays, 3D,
        // cubes), not always 2D.
        View::Shader(_, Some(desc)) => return null_like(desc, at),
        View::Placed(false, _, desc) => {
            #[cfg(feature = "diagnostics")]
            if magenta_for(desc, at) {
                return;
            }
            return null_like(desc[..10].try_into().unwrap_or(&[0; 10]), at);
        }
        View::Constant(..) => {
            let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC::default();
            // SAFETY: a slot of our heap; a zero location makes a null view.
            return unsafe { host.device.CreateConstantBufferView(Some(&desc), at) };
        }
        _ => return null_view(at),
    };
    let desc = if buffer {
        D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            ..Default::default()
        }
    } else {
        null_uav_desc(dimension)
    };
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device.CreateUnorderedAccessView(
            None::<&ID3D12Resource>,
            None::<&ID3D12Resource>,
            Some(&desc),
            at,
        )
    };
}

/// A null texture UAV of the title's view dimension (1D, 1D or 2D arrays,
/// 3D), 2D if it names none: a 2D null view where the shader writes a 3D
/// texture is a mismatch GPU-based validation reports (the world's wind
/// field, CS_CopyWindFieldBeforeUpdateWithSmooth, 2026-11-24).
fn null_uav_desc(dimension: Option<u32>) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    let mut desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: DXGI_FORMAT_R32_UINT,
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    match dimension.map(|d| D3D12_UAV_DIMENSION(d as i32)) {
        Some(D3D12_UAV_DIMENSION_TEXTURE1D) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE1D;
        }
        Some(D3D12_UAV_DIMENSION_TEXTURE1DARRAY) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE1DARRAY;
            desc.Anonymous.Texture1DArray = D3D12_TEX1D_ARRAY_UAV {
                ArraySize: 1,
                ..Default::default()
            };
        }
        Some(D3D12_UAV_DIMENSION_TEXTURE2DARRAY) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
            desc.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV {
                ArraySize: 1,
                ..Default::default()
            };
        }
        Some(D3D12_UAV_DIMENSION_TEXTURE3D) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE3D;
            desc.Anonymous.Texture3D = D3D12_TEX3D_UAV {
                WSize: u32::MAX,
                ..Default::default()
            };
        }
        _ => {}
    }
    desc
}

/// A buffer view that reads zeros (raw, as translated shaders mostly read
/// buffers).
fn null_buffer_view(at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R32_TYPELESS,
        ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Buffer: D3D12_BUFFER_SRV {
                NumElements: 1,
                Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                ..Default::default()
            },
        },
    };
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device
            .CreateShaderResourceView(None::<&ID3D12Resource>, Some(&desc), at)
    };
}

/// A texture view that reads zeros with the dimension (and mip and array
/// counts) of the title's description `words`; 2D if it names none.
fn null_like(words: &[u32; 10], at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    const TEXTURES: std::ops::RangeInclusive<u32> = 2..=10;
    if !TEXTURES.contains(&words[1]) {
        return null_view(at);
    }
    let Some(host) = host() else { return };
    // One mip, one slice or cube, plane 0: fields a null view still checks.
    let mut w = [0u32; 10];
    w[1] = words[1];
    match words[1] {
        6 => {}
        7 => w[5] = 1,
        3 | 5 | 10 => (w[5], w[7]) = (1, 1),
        _ => w[5] = 1,
    }
    let mut desc = srv_desc(&w);
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device
            .CreateShaderResourceView(None::<&ID3D12Resource>, Some(&desc), at)
    };
}

/// A view that reads zeros, for slots whose view the host cannot make.
/// shortcut: slots with no view at all (kind unknown) get this null 2D
/// view; null buffer views there did not stop the world's low-address
/// page faults (2026-11-24, run w10), so they are not the cause.
fn null_view(at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MipLevels: 1,
                ..Default::default()
            },
        },
    };
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device
            .CreateShaderResourceView(None::<&ID3D12Resource>, Some(&desc), at)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A null texture UAV keeps the title's dimension (3D for the world's
    /// wind field), and is 2D when the title names none.
    #[test]
    fn null_uavs_keep_the_titles_dimension() {
        let d = null_uav_desc(Some(8));
        assert_eq!(d.ViewDimension, D3D12_UAV_DIMENSION_TEXTURE3D);
        // SAFETY: the member the dimension names.
        assert_eq!(unsafe { d.Anonymous.Texture3D.WSize }, u32::MAX);
        let d = null_uav_desc(Some(5));
        assert_eq!(d.ViewDimension, D3D12_UAV_DIMENSION_TEXTURE2DARRAY);
        // SAFETY: the member the dimension names.
        assert_eq!(unsafe { d.Anonymous.Texture2DArray.ArraySize }, 1);
        let dimension = |d| null_uav_desc(d).ViewDimension;
        assert_eq!(dimension(Some(2)), D3D12_UAV_DIMENSION_TEXTURE1D);
        assert_eq!(dimension(Some(4)), D3D12_UAV_DIMENSION_TEXTURE2D);
        assert_eq!(dimension(None), D3D12_UAV_DIMENSION_TEXTURE2D);
        // Multisampled UAVs need a cap; they stay 2D.
        assert_eq!(dimension(Some(6)), D3D12_UAV_DIMENSION_TEXTURE2D);
    }

    /// What the texture code asks of the spans (levels on demand): where
    /// the next texture starts inside a span, the lowest level a view of a
    /// texture shows, and a texture's slots.
    #[test]
    fn spans_say_the_next_start_and_the_lowest_level_viewed() {
        let mut spans = Spans::default();
        // A 512x512 texture of 0x56000 bytes viewed from levels 3 and 1,
        // and the next one 0x6000 after it (run 188).
        spans.note(1, 0x1_0000, 0x1_0000 + 0x5_6000, true);
        spans.shows(1, 3);
        spans.note(2, 0x1_0000, 0x1_0000 + 0x5_6000, true);
        spans.shows(2, 1);
        spans.note(3, 0x1_6000, 0x1_6000 + 0x5_6000, true);
        assert_eq!(spans.next_start(0x1_0000, 0x1_0000 + 0x5_6000), Some(0x1_6000));
        assert_eq!(spans.next_start(0x1_6000, 0x1_6000 + 0x5_6000), None);
        assert_eq!(spans.first_viewed(0x1_0000), Some(1));
        assert_eq!(spans.first_viewed(0x1_6000), None);
        assert_eq!(spans.slots_at(0x1_0000), vec![1, 2]);
        // The view from level 1 goes: the texture is viewed from level 3.
        spans.forget(2);
        assert_eq!(spans.first_viewed(0x1_0000), Some(3));
    }

    /// Views of one placed texture are told from views of others by the
    /// address and the resource description, not by what they show.
    #[test]
    fn views_of_one_placed_texture_are_told_by_address_and_description() {
        let texture = [3, 0, 0, 0, 512, 0, 512, 0x8_0001, 98, 1, 0, 0x105, 0, 0];
        let view = |resource: [u32; 14], first_mip: u32, address: u32| {
            let mut desc = [0u32; 16];
            (desc[0], desc[1], desc[4], desc[5], desc[10], desc[11]) = (98, 4, first_mip, 8 - first_mip, address, 0xb);
            View::Placed(false, resource, desc)
        };
        let low = view(texture, 3, 0x1000_0000);
        // more mips of the same texture
        assert!(same_placed_texture(&low, &view(texture, 0, 0x1000_0000)));
        // another address, another description, a buffer, an unordered view
        assert!(!same_placed_texture(&low, &view(texture, 3, 0x2000_0000)));
        let mut wider = texture;
        wider[4] = 1024;
        assert!(!same_placed_texture(&low, &view(wider, 3, 0x1000_0000)));
        let mut buffer = texture;
        buffer[0] = 1;
        assert!(!same_placed_texture(&view(buffer, 3, 0x1000_0000), &view(buffer, 3, 0x1000_0000)));
        let View::Placed(_, resource, desc) = low.clone() else { unreachable!() };
        assert!(!same_placed_texture(&low, &View::Placed(true, resource, desc)));
        assert!(!same_placed_texture(&low, &View::Shader(0, None)));
    }

    /// The texture that stands for missing ones can be made and viewed.
    #[cfg(feature = "diagnostics")]
    #[test]
    fn a_magenta_texture_is_made() {
        let Some(host) = host() else {
            return; // no host GPU
        };
        let Some(heap) = HostHeap::new(false, 4) else {
            return;
        };
        let texture = magenta().expect("the texture");
        // SAFETY: a slot of our heap and a live texture.
        unsafe { host.device.CreateShaderResourceView(texture, None, heap.cpu(0)) };
        // SAFETY: a plain device query.
        assert!(unsafe { host.device.GetDeviceRemovedReason() }.is_ok());
    }

    /// A null 3D UAV made on the real device leaves it alive.
    #[test]
    fn null_3d_uav_is_accepted_by_the_device() {
        let Some(host) = host() else {
            return; // no host GPU
        };
        let Some(heap) = HostHeap::new(false, 4) else {
            return;
        };
        let view = View::Unordered(0, 0, Some([28, 8, 0, 0, 0, 0, 0, 0, 0, 0]));
        null_for(&view, heap.cpu(0));
        // SAFETY: a plain device query.
        assert!(unsafe { host.device.GetDeviceRemovedReason() }.is_ok());
    }

    /// Dynamic slots stay taken until given back, and runs do not wrap.
    #[test]
    fn a_structured_view_made_raw_passes_the_host_check() {
        // 100 elements of 20 bytes from element 8, in a copy that starts 32
        // bytes before the view's memory: bytes 192..2192.
        let (first, count) = raw_window(32, 8, 100, 20).unwrap();
        assert_eq!((first, count), (48, 500));
        assert!(buffer_view_ok(DXGI_FORMAT_R32_TYPELESS, first, count, 0, true, 4096));
        // the format a raw view must name; the structured one's is refused
        assert!(!buffer_view_ok(DXGI_FORMAT_UNKNOWN, first, count, 0, true, 4096));
        // past the buffer, not on 16 bytes, no elements
        assert!(!buffer_view_ok(DXGI_FORMAT_R32_TYPELESS, first, count, 0, true, 2000));
        assert!(raw_window(4, 0, 100, 20).is_none());
        assert!(!buffer_view_ok(DXGI_FORMAT_R32_TYPELESS, 0, 0, 0, true, 4096));
        // a structured view the host takes as it is
        assert!(buffer_view_ok(DXGI_FORMAT_UNKNOWN, 1, 10, 12, false, 4096));
    }

    /// The queue of textures picked by index serves the slots looked at
    /// before first, then the newest write and the oldest in turns; a
    /// slot whose memory is not there is looked at again after
    /// 1, 2, 4 frames up to the limit; and what the pass reports about a
    /// slot written again meanwhile is dropped.
    #[test]
    fn constant_views_are_whole_and_at_most_64_kib() {
        assert_eq!(constant_size(1), 256);
        assert_eq!(constant_size(256), 256);
        assert_eq!(constant_size(65_536), 65_536);
        assert_eq!(constant_size(969_472), 65_536);
        assert_eq!(constant_size(u32::MAX), 65_536);
    }

    #[test]
    fn views_past_the_end_are_cut_to_the_elements_inside() {
        assert_eq!(elements_inside(0, 10, 16, 160), 10);
        assert_eq!(elements_inside(4, 10, 16, 160), 6);
        assert_eq!(elements_inside(4, 10, 16, 167), 6);
        // Nothing inside: as it was, for the host check to refuse.
        assert_eq!(elements_inside(10, 10, 16, 160), 10);
        assert_eq!(elements_inside(0, 7, 4, 1 << 40), 7);
    }

    /// The fingerprint of a texture's memory sees a change at its start, in
    /// its middle and at its end, and is the same for the same bytes.
    #[test]
    fn a_fingerprint_sees_changes() {
        let mut memory = vec![3u8; 1 << 20];
        let print = |m: &[u8]| crate::texture::fingerprint(m.as_ptr() as u64, m.len() as u64);
        let before = print(&memory);
        assert_eq!(print(&memory), before);
        let last = memory.len() - 1;
        for at in [0, (memory.len() - 4096) / 11 * 5 & !0xfff, last] {
            memory[at] ^= 1;
            assert_ne!(print(&memory), before, "byte {at}");
            memory[at] ^= 1;
        }
        assert_ne!(print(&memory[..100]), print(&memory[..200]));
    }

    #[test]
    fn texture_queue_keeps_first_looks_in_a_retry_backlog() {
        let mut q = TextureQueue::default();
        for index in 0..10 {
            q.written(index, 0, 0, false);
        }
        for (index, write) in q.due(0, 10) {
            q.later(index, write, 0, 4);
        }
        for index in 10..20 {
            q.written(index, 1, 1, false);
        }
        let slots = |frame, max| q.due(frame, max).into_iter().map(|d| d.0).collect::<Vec<_>>();
        assert_eq!(slots(1, 4), [19, 9, 10, 8]);
        assert_eq!(slots(2, 4), [9, 10, 8, 19]);
        // If time or bytes run out after one item, both classes get a
        // first turn; old and newly written first-look slots do as well.
        assert_eq!(slots(3, 1), [10]);
        assert_eq!(slots(4, 1), [9]);
        assert_eq!(slots(5, 1), [19]);
        assert!(slots(5, 0).is_empty());
    }

    #[test]
    fn the_texture_queue_serves_the_newest_writes_first() {
        let mut q = TextureQueue::default();
        // Slots 10..18 written in frames 1..9, slot 5 last.
        for i in 0..8u32 {
            q.written(10 + i, u64::from(i) + 1, 0, false);
        }
        q.written(5, 9, 0, false);
        let order: Vec<u32> = q.due(9, 100).into_iter().map(|d| d.0).collect();
        // newest, oldest, newest, oldest, and so on
        assert_eq!(order, [5, 10, 17, 11, 16, 12, 15, 13, 14]);
        assert_eq!(q.due(9, 2).len(), 2);
        assert_eq!(q.census(9), [9, 9, 0, 0, 8]);
        // Slot 17's memory is not mapped: again next frame, then in 2, 4, 4.
        let write = |q: &TextureQueue, index: u32| q.due(u64::MAX, 100).iter().find(|d| d.0 == index).unwrap().1;
        let w17 = write(&q, 17);
        let mut frame = 9;
        for wait in [1, 2, 4, 4] {
            q.later(17, w17, frame, 4);
            assert!(!q.due(frame + wait - 1, 100).iter().any(|d| d.0 == 17));
            frame += wait;
            // Due again; these even frames start with a retry.
            assert_eq!(q.due(frame, 100)[0].0, 17);
        }
        assert_eq!(q.census(9)[2], 1);
        q.done(17, w17);
        assert!(!q.due(u64::MAX, 100).iter().any(|d| d.0 == 17));
        // Uploaded with part of its memory there: kept, with what was there.
        let w16 = write(&q, 16);
        q.partly(16, w16, 12, 4096);
        assert_eq!(q.mapped(16), 4096);
        assert_eq!(q.due(12, 100)[0].0, 16);
        // The title writes slot 16 again while the pass works on it: what
        // the pass then says about the old write changes nothing.
        q.written(16, 13, 0, false);
        q.done(16, w16);
        q.later(16, w16, 13, 4);
        assert_eq!(q.mapped(16), 0);
        assert_eq!(q.due(13, 100)[0].0, 16);
        // A draw that makes the host slot (`seen`) keeps what the queue knows,
        let w15 = write(&q, 15);
        q.partly(15, w15, 20, 8192);
        q.written(15, 14, 0, true);
        assert_eq!(q.mapped(15), 8192);
        // and queues a slot the queue did not hold.
        q.written(99, 14, 0, true);
        assert!(q.due(14, 100).iter().any(|d| d.0 == 99));
        // A write due later (a table's slot) is not handed out before.
        q.written(98, 14, 16, false);
        assert!(!q.due(15, 100).iter().any(|d| d.0 == 98));
        assert!(q.due(16, 100).iter().any(|d| d.0 == 98));
    }

    /// Slots held back for lack of room on the card are not handed out
    /// until their frame, keep their place, are not counted as waiting
    /// for their memory, and all come back at once when the card has room;
    /// a slot written again meanwhile is the title's new write.
    #[test]
    fn slots_held_for_lack_of_room_come_back_when_there_is_room() {
        let mut q = TextureQueue::default();
        for i in 0..4u32 {
            q.written(20 + i, 1, 0, false);
        }
        let due = q.due(1, 100);
        assert_eq!(due.len(), 4);
        // The card is full: slots 20 and 21 are held back until frame 101.
        for &(index, write) in due.iter().filter(|d| d.0 < 22) {
            q.hold(index, write, 101);
        }
        let now: Vec<u32> = q.due(2, 100).into_iter().map(|d| d.0).collect();
        assert!(!now.contains(&20) && !now.contains(&21));
        assert!(now.contains(&22) && now.contains(&23));
        assert_eq!(q.census(2)[2], 0, "held slots do not wait for their memory");
        // Due again at their frame, in the order they had.
        assert_eq!(q.due(101, 100), due);
        // Slot 21 is written again while held: due as written.
        q.written(21, 3, 0, false);
        assert!(q.due(3, 100).iter().any(|d| d.0 == 21));
        // Room again at frame 5: slot 20 is due at once, and only it was
        // still held as it was.
        assert_eq!(q.room(5), 1);
        assert!(q.due(5, 100).iter().any(|d| d.0 == 20));
        assert_eq!(q.room(6), 0);
    }

    /// Textures of which only the smallest levels are held, packed into
    /// one another's address range (the three of run 188: 0x2c000 bytes
    /// each at full size, their starts 0x2000 and 0x4000 apart): writing
    /// any of them, in any order and again, leaves the others.
    #[test]
    fn packed_neighbours_stay() {
        let mut spans = Spans::default();
        assert_eq!(spans.rule, Rule::Start);
        let at = 0xb_86bf_8000u64;
        let (a, b, c) = (at, at + 0x2000, at + 0x6000);
        assert!(spans.note(105_998, a, a + 0x2c000, true).is_empty());
        assert!(spans.note(106_004, b, b + 0x2c000, true).is_empty());
        assert!(spans.note(108_521, c, c + 0x2c000, true).is_empty());
        // The first written again (as a title does when levels arrive),
        // from another slot too: its span reaches over the other two.
        assert!(spans.note(7, a, a + 0x2c000, true).is_empty());
        assert!(spans.note(106_004, b, b + 0x2c000, true).is_empty());
        let mut all = spans.slots_in(0, u64::MAX);
        all.sort_unstable();
        assert_eq!(all, [7, 105_998, 106_004, 108_521]);
        // A draw makes a slot of the second texture: it is one of them.
        assert!(spans.note(9, b, b + 0x2c000, false).is_empty());
        assert_eq!(spans.by_slot.get(&9), Some(&b));
    }

    /// A texture written where another starts replaces it: at the same
    /// address with another length, or inside the block of its smallest
    /// levels. The packed neighbours of the one replaced stay.
    #[test]
    fn a_texture_written_at_the_start_of_another_replaces_it() {
        let mut spans = Spans::default();
        let at = 0x7_0000_0000u64;
        assert!(spans.note(1, at, at + 0x2c000, true).is_empty());
        assert!(spans.note(2, at + 0x2000, at + 0x2e000, true).is_empty());
        // Another length from the same start: slot 1 is stale, slot 2 not.
        assert_eq!(spans.note(3, at, at + 0xb000, true), [1]);
        // A start inside the first 4 KiB of slot 3's texture: stale too.
        assert_eq!(spans.note(4, at + 0x800, at + 0x1800, true), [3]);
        // And the other way: a texture that starts just before one.
        assert_eq!(spans.note(5, at + 0x1800, at + 0x4800, true), [2]);
        let mut left = spans.slots_in(0, u64::MAX);
        left.sort_unstable();
        assert_eq!(left, [4, 5]);
        // Exactly 4 KiB apart: both there.
        assert!(spans.note(6, at + 0x2800, at + 0x3800, true).is_empty());
        // A draw's view that clashes is left out, and nothing is stale.
        assert!(spans.note(8, at + 0x900, at + 0x2900, false).is_empty());
        assert_eq!(spans.by_slot.get(&8), None);
        assert_eq!(spans.by_slot.len(), 3);
    }

    /// A slot written with another texture leaves its old span; the same
    /// view written again changes nothing.
    #[test]
    fn a_slot_rewritten_moves_to_its_new_span() {
        let mut spans = Spans::default();
        assert!(spans.note(1, 0x10000, 0x3c000, true).is_empty());
        assert!(spans.note(2, 0x12000, 0x3e000, true).is_empty());
        assert!(spans.note(1, 0x10000, 0x3c000, true).is_empty());
        assert_eq!(spans.slots_in(0x10000, 0x10001), [1]);
        // Slot 1 now views a texture far away.
        assert!(spans.note(1, 0x90000, 0x91000, true).is_empty());
        assert_eq!(spans.slots_in(0x10000, 0x10001), [] as [u32; 0]);
        assert_eq!(spans.slots_in(0x12000, 0x12001), [2]);
        assert_eq!(spans.slots_in(0x90000, 0x90001), [1]);
        spans.forget(1);
        spans.forget(2);
        assert!(spans.by_start.is_empty() && spans.by_slot.is_empty());
    }

    /// Memory the runtime maps, unmaps or writes names the textures that
    /// are there: among packed neighbours the one whose held levels the
    /// range lies in (it ends where the next one starts), and the ones
    /// that start inside the range.
    #[test]
    fn memory_changed_under_packed_neighbours_names_the_ones_there() {
        let mut spans = Spans::default();
        let at = 0x7_0000_0000u64;
        assert!(spans.note(1, at, at + 0x2c000, true).is_empty());
        assert!(spans.note(2, at + 0x2000, at + 0x2e000, true).is_empty());
        assert!(spans.note(3, at + 0x6000, at + 0x32000, true).is_empty());
        // Inside the first one's held levels.
        assert_eq!(spans.slots_in(at + 0x1000, at + 0x2000), [1]);
        // Inside the second's: not the first, which ends where it starts.
        assert_eq!(spans.slots_in(at + 0x3000, at + 0x4000), [2]);
        // Past the last start: the last one.
        assert_eq!(spans.slots_in(at + 0x20000, at + 0x21000), [3]);
        // A range over all of them.
        assert_eq!(spans.slots_in(at, at + 0x8000), [1, 2, 3]);
        // Past the end of all: none.
        assert_eq!(spans.slots_in(at + 0x32000, at + 0x33000), [] as [u32; 0]);
    }

    /// A write names the textures that hold its bytes: the one that
    /// starts where the write starts, not the neighbour before it, whose
    /// held levels end there.
    #[test]
    fn a_write_at_the_start_of_a_packed_texture_names_it_alone() {
        let mut spans = Spans::default();
        let at = 0x7_0000_0000u64;
        assert!(spans.note(1, at, at + 0x2c000, true).is_empty());
        assert!(spans.note(2, at + 0x2000, at + 0x2e000, true).is_empty());
        assert!(spans.note(3, at + 0x6000, at + 0x32000, true).is_empty());
        // A request that fills the second texture's held levels.
        assert_eq!(spans.slots_in(at + 0x2000, at + 0x6000), [1, 2]);
        assert_eq!(spans.slots_holding(at + 0x2000, at + 0x6000), [2]);
        // From inside the first one's held levels on: both.
        assert_eq!(spans.slots_holding(at + 0x1000, at + 0x3000), [1, 2]);
        // A whole 64 KiB page: every texture that starts in it.
        assert_eq!(spans.slots_holding(at, at + 0x10000), [1, 2, 3]);
        // Past the last start, the last one; past all, or no bytes, none.
        assert_eq!(spans.slots_holding(at + 0x20000, at + 0x21000), [3]);
        assert_eq!(spans.slots_holding(at + 0x32000, at + 0x33000), [] as [u32; 0]);
        assert_eq!(spans.slots_holding(at + 0x2000, at + 0x2000), [] as [u32; 0]);
    }

    /// A write behind the guards' back under a placed texture: a slot
    /// waiting for its first upload is left alone, one uploaded before is
    /// counted, and made due two frames on when that is asked for. (The
    /// ranges are handed over here, not through the list all heaps share.)
    #[test]
    fn a_page_write_makes_an_uploaded_texture_slot_due_when_asked() {
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        let slots = |at: u64| -> Vec<u32> { heap.texture_slots_due(at, 8).0.into_iter().map(|d| d.0).collect() };
        // 64x64 BC7, one mip, at an address no test and no title uses.
        let at = 0x7fed_0000_0000u64;
        let mut desc = [0u32; 16];
        (desc[10], desc[11]) = (at as u32, (at >> 32) as u32);
        let view = View::Placed(false, [3, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0], desc);
        assert!(heap.note_buffer(1, Some(&view), false).is_empty());
        let written = [(at, at + 0x1000)];
        // Waiting for its first upload: held, not uploaded, nothing done.
        assert_eq!(heap.slots_written_under(&written, 50, true), [1, 0, 0]);
        let (queued, write) = heap.texture_slots_due(u64::MAX, 8).0[0];
        assert_eq!(queued, 1);
        heap.texture_slot_done(1, write, None);
        assert_eq!(slots(u64::MAX), []);
        // Uploaded and out of the queue. Memory elsewhere: nothing.
        assert_eq!(heap.slots_written_under(&[(at + 0x10_0000, at + 0x10_1000)], 60, true), [0, 0, 0]);
        // Under it, counted only: it stays out of the queue.
        assert_eq!(heap.slots_written_under(&written, 60, false), [1, 1, 0]);
        assert_eq!(slots(u64::MAX), []);
        // Under it, asked for: due two frames on, not before.
        assert_eq!(heap.slots_written_under(&written, 60, true), [1, 1, 1]);
        assert_eq!(slots(61), []);
        assert_eq!(slots(62), [1]);
    }

    /// A slot whose texture was uploaded ahead shows it when it is
    /// published, and does not go back into the queue of textures picked by
    /// index for that; one the title wrote again is left alone.
    #[test]
    fn a_published_slot_stays_out_of_the_queue() {
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        // 64x64 BC7, one mip, at an address no test and no title uses.
        let view = |at: u64| {
            let mut desc = [0u32; 16];
            (desc[0], desc[1], desc[2], desc[5]) = (98, 4, 5768, 1);
            (desc[10], desc[11]) = (at as u32, (at >> 32) as u32);
            View::Placed(false, [3, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0], desc)
        };
        let title = [0u32; 8];
        let handle = title.as_ptr() as u64;
        crate::descriptor::record(handle, view(0x7fec_0000_0000));
        let (first, key) = crate::descriptor::view_and_key(handle).unwrap();
        assert!(heap.note_buffer(1, Some(&first), false).is_empty());
        let (index, write) = heap.texture_slots_due(u64::MAX, 8).0[0];
        assert_eq!(index, 1);
        // Its upload was recorded: out of the queue, the slot not made yet.
        heap.texture_slot_done(1, write, None);
        assert!(!heap.made(1));
        assert!(heap.publish(1, handle, key));
        assert!(heap.made(1));
        assert!(heap.texture_slots_due(u64::MAX, 8).0.is_empty());
        // Written again since the upload was recorded.
        crate::descriptor::record(handle, view(0x7feb_0000_0000));
        assert!(!heap.publish(1, handle, key));
        assert_ne!(crate::descriptor::view_and_key(handle).unwrap().1, key);
    }

    /// What the buffer pass mirrors is not handed back to it as changed: a
    /// buffer slot made with `record_change` false leaves `buffer_changes`
    /// empty, so the pass does not look at it again on the next frame; a
    /// slot written by the title (`record_change` true) is listed.
    #[test]
    fn buffer_slots_the_buffer_pass_made_are_not_changed_again() {
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        // A structured buffer view: 10 elements of 16 bytes.
        let mut desc = [0u32; 16];
        (desc[1], desc[6], desc[7]) = (1, 10, 16);
        let view = View::Placed(false, [1, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0], desc);
        assert!(heap.note_buffer_checked(3, Some(&view), true, None, false, None).is_ok());
        assert!(heap.buffer_changes().is_empty());
        assert!(heap.note_buffer_checked(4, Some(&view), false, None, true, None).is_ok());
        assert_eq!(heap.buffer_changes(), [4]);
    }

    /// `VICTORY_STALE_TEXTURES=0`: nothing is ever stale, and a view that
    /// overlaps others is left out of the spans, as before.
    #[test]
    fn never_stale_leaves_overlapping_views_out() {
        let mut spans = Spans { rule: Rule::Never, ..Default::default() };
        assert!(spans.note(1, 0x1000, 0x3000, true).is_empty());
        assert!(spans.note(2, 0x2000, 0x4000, true).is_empty());
        assert!(spans.note(3, 0x1000, 0x2000, true).is_empty());
        assert_eq!(spans.slots_in(0, u64::MAX), [1]);
    }

    /// The rule until run 188 (`VICTORY_STALE_TEXTURES=overlap`): a view
    /// written over memory that older views describe otherwise makes
    /// those stale; views of the same span are one texture's.
    #[test]
    fn a_newer_view_of_other_memory_makes_the_older_stale() {
        let mut spans = Spans { rule: Rule::Overlap, ..Default::default() };
        // Three textures side by side, the first in two slots.
        assert!(spans.note(1, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert!(spans.note(2, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert!(spans.note(3, 0x3000, 0x4000, &shape(0x3000, 0x4000), true).is_empty());
        assert!(spans.note(4, 0x8000, 0x9000, &shape(0x8000, 0x9000), true).is_empty());
        // The same slot written with the same span again: nothing changes.
        assert!(spans.note(2, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert_eq!(spans.slots_in(0x2000, 0x3800), [1, 2, 3]);
        assert_eq!(spans.slots_in(0x4000, 0x8000), [] as [u32; 0]);
        // A draw makes a slot whose view overlaps them: left out, nothing stale.
        assert!(spans.note(9, 0x2000, 0x3800, &shape(0x2000, 0x3800), false).is_empty());
        assert_eq!(spans.slots_in(0x2000, 0x3800), [1, 2, 3]);
        // The title writes a texture over the end of the first and all of
        // the second: their three slots are stale.
        let mut stale = spans.note(5, 0x2000, 0x4000, &shape(0x2000, 0x4000), true);
        stale.sort_unstable();
        assert_eq!(stale, [1, 2, 3]);
        assert_eq!(spans.slots_in(0, u64::MAX), [5, 4]);
        // Another start in the same memory: stale in turn. Another length
        // from the same start too.
        assert_eq!(spans.note(6, 0x2800, 0x3000, &shape(0x2800, 0x3000), true), [5]);
        assert_eq!(spans.note(7, 0x2800, 0x2c00, &shape(0x2800, 0x2c00), true), [6]);
        // A slot that moves to another texture leaves its span behind.
        assert!(spans.note(7, 0x8000, 0x9000, &shape(0x8000, 0x9000), true).is_empty());
        assert_eq!(spans.slots_in(0x2800, 0x2c00), [] as [u32; 0]);
        assert_eq!(spans.slots_in(0x8000, 0x8001), [4, 7]);
        spans.forget(4);
        spans.forget(7);
        assert!(spans.spans.is_empty() && spans.by_slot.is_empty());
    }

    /// Views of one texture that show more and fewer of its mips are all
    /// its own, and the next texture may begin where the mips that are
    /// there end.
    #[test]
    fn a_texture_of_which_the_smaller_mips_are_there_ends_behind_them() {
        // As the title has them: 512x512 BC7 of which 64x64 is
        // there, and a 1024x512 BC1 texture 8 KiB on.
        let texture = [3, 0, 0, 0, 512, 0, 512, 0x8_0001, 99, 1, 0, 0x105, 0, 0];
        let next = [3, 0, 0, 0, 1024, 0, 512, 0x8_0001, 71, 1, 0, 0x105, 0, 0];
        let mut spans = Spans::default();
        let at = 0xb_5c04_0000u64;
        let end = |spans: &mut Spans, desc: &[u32; 14], at: u64, first| at + spans.bytes(desc, first).unwrap();
        let a = end(&mut spans, &texture, at, 3);
        assert_eq!(a, at + 0x2000);
        assert!(spans.note(1, at, a, &texture, true).is_empty());
        let b = end(&mut spans, &next, a, 3);
        assert!(b <= a + 0x4000);
        assert!(spans.note(2, a, b, &next, true).is_empty());
        assert_eq!(spans.slots_in(at, b), [1, 2]);
        // The smaller mips alone in another slot: the same texture's.
        let small = end(&mut spans, &texture, at, 5);
        assert!(small < a);
        assert!(spans.note(3, at, small, &texture, true).is_empty());
        assert_eq!(spans.slots_in(at, at + 1), [3, 1]);
        // More of it streamed in, over the next texture: that one is stale.
        let more = end(&mut spans, &texture, at, 1);
        assert!(more > b);
        assert_eq!(spans.note(1, at, more, &texture, true), [2]);
        assert_eq!(spans.claim(3, at, small, &texture), (Claim::Own, None));
        assert_eq!(spans.claim(2, a, b, &next), (Claim::Other, Some((at, more))));
    }

    /// A resource description of its own for every span.
    fn shape(start: u64, end: u64) -> [u32; 14] {
        let mut desc = [0; 14];
        (desc[0], desc[4], desc[6]) = (3, start as u32, end as u32);
        desc
    }

    /// Whose the memory of a view is: the slot's own span, no other's, or
    /// one that says otherwise.
    #[test]
    fn spans_say_whose_memory_a_view_is() {
        let mut spans = Spans::default();
        assert!(spans.note(1, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert_eq!(spans.claim(1, 0x1000, 0x3000, &shape(0x1000, 0x3000)), (Claim::Own, None));
        // Another slot's view of the same texture, or of memory nothing describes.
        assert_eq!(spans.claim(2, 0x1000, 0x3000, &shape(0x1000, 0x3000)), (Claim::Free, None));
        assert_eq!(spans.claim(2, 0x3000, 0x4000, &shape(0x3000, 0x4000)), (Claim::Free, None));
        // The same start with another length; another start.
        assert_eq!(spans.claim(2, 0x1000, 0x2000, &shape(0x1000, 0x2000)), (Claim::SameStart, Some((0x1000, 0x3000))));
        assert_eq!(spans.claim(2, 0x800, 0x2000, &shape(0x800, 0x2000)), (Claim::Other, Some((0x1000, 0x3000))));
    }

    /// A view that fell out of the queue with its memory there and nothing
    /// to show for it goes back; one a newer view speaks against does not.
    #[test]
    fn a_texture_sweep_finds_views_nothing_brings_up_to_date() {
        let Some(host) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        // Below 2^40, as a title's memory is.
        // SAFETY: new memory, left to the process.
        let at = unsafe {
            use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
            VirtualAlloc(0x7d_4000_0000usize as *const _, 1 << 20, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) as u64
        };
        if at == 0 {
            return; // taken
        }
        let mut slots = vec![[0u32; 8]; 8];
        let base = slots.as_mut_ptr() as u64;
        let handle = |index: u32| Some(base + u64::from(index) * 32);
        let resource = [3, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0];
        let view = |width: u32, address: u64| {
            let mut desc = [0u32; 16];
            (desc[10], desc[11]) = (address as u32, (address >> 32) as u32);
            let mut resource = resource;
            resource[4] = width;
            View::Placed(false, resource, desc)
        };
        super::super::record(handle(1).unwrap(), view(64, at));
        super::super::record(handle(2).unwrap(), view(64, at + 0x8_0000));
        // A newer, wider texture in the second one's memory.
        host.note_buffer(5, Some(&view(128, at + 0x8_0000)), false);
        let queued = || -> Vec<u32> { host.texture_slots_due(u64::MAX, 8).0.into_iter().map(|d| d.0).collect() };
        for slot in queued() {
            host.textures.lock().unwrap().remove(slot);
        }
        let mut counts = [0; 10];
        host.sweep_part(0..8, handle, 50, false, &mut counts);
        assert_eq!(counts, [2, 0, 0, 0, 0, 0, 1, 1, 0, 0]);
        assert_eq!(queued(), []);
        host.sweep_part(0..8, handle, 50, true, &mut counts);
        assert_eq!(queued(), [1]);
    }

    /// Memory the runtime maps under a slot's texture makes the slot due
    /// again from the next frame; a stale slot leaves the queue.
    #[test]
    fn remapped_memory_makes_its_texture_slots_due() {
        let _one = REMAPPED_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        let slots = |at: u64| -> Vec<u32> { heap.texture_slots_due(at, 8).0.into_iter().map(|d| d.0).collect() };
        // 64x64 BC7, one mip, at an address no test and no title uses.
        let at = 0x7fee_0000_0000u64;
        let view = |address: u64, width: u32| {
            let mut desc = [0u32; 16];
            (desc[10], desc[11]) = (address as u32, (address >> 32) as u32);
            View::Placed(false, [3, 0, 0, 0, width, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0], desc)
        };
        let len = crate::resource::placed_bytes(&[3, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0]).unwrap();
        assert!(heap.note_buffer(1, Some(&view(at, 64)), false).is_empty());
        let (queued, write) = heap.texture_slots_due(u64::MAX, 8).0[0];
        assert_eq!(queued, 1);
        heap.texture_slot_done(1, write, None);
        assert_eq!(slots(u64::MAX), []);
        // Memory elsewhere: nothing. Under the texture: due from the next frame.
        remapped((at + 2 * len) as usize, 0x1000);
        remapped(at as usize, 0x1000);
        assert_eq!(heap.texture_slots_remapped(50), 1);
        assert_eq!(slots(50), []);
        assert_eq!(slots(51), [1]);
        // A wider texture written at the same address: slot 1 is stale.
        assert_eq!(heap.note_buffer(2, Some(&view(at, 128)), false), [1]);
        assert_eq!(slots(u64::MAX), [2]);
    }

    /// The tests that take [`REMAPPED`] and [`OVERFLOWED`], one at a time.
    static REMAPPED_TESTS: Mutex<()> = Mutex::new(());

    /// Ranges dropped on overflow make every placed slot due at the next
    /// pass, once; the pass after has nothing to do.
    #[test]
    fn overflowed_ranges_make_every_placed_slot_due_once() {
        let _one = REMAPPED_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        let slots = |at: u64| -> Vec<u32> { heap.texture_slots_due(at, 8).0.into_iter().map(|d| d.0).collect() };
        let at = 0x7fed_0000_0000u64;
        let mut desc = [0u32; 16];
        (desc[10], desc[11]) = (at as u32, (at >> 32) as u32);
        let view = View::Placed(false, [3, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0], desc);
        assert!(heap.note_buffer(3, Some(&view), false).is_empty());
        let (queued, write) = heap.texture_slots_due(u64::MAX, 8).0[0];
        assert_eq!(queued, 3);
        heap.texture_slot_done(3, write, None);
        REMAPPED.lock().unwrap().clear();
        OVERFLOWED.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(heap.texture_slots_remapped(50), 1);
        assert_eq!(slots(51), [3]);
        assert_eq!(heap.texture_slots_remapped(52), 0);
    }

    /// A slot a draw read through a table leaves the queue; the title's
    /// next write of it waits for its draw, and is due after the grace
    /// frames if none reads it.
    #[test]
    fn slots_read_through_tables_leave_the_texture_queue() {
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        let frame = 100;
        let slots = |at: u64| -> Vec<u32> { heap.texture_slots_due(at, 8).0.into_iter().map(|d| d.0).collect() };
        let texture = View::Placed(false, [3, 0, 0, 0, 4, 0, 4, 0x1_0001, 28, 1, 0, 0x105, 0, 0], [0; 16]);
        assert!(is_texture_view(&texture));
        heap.queue_texture(2, true, false, frame);
        heap.queue_texture(3, true, false, frame);
        assert_eq!(slots(frame), [3, 2]);
        heap.tabled_in(3, true, frame);
        assert_eq!(slots(frame), [2]);
        // Made in place by the draw that read it: still out.
        heap.queue_texture(3, true, true, frame);
        assert_eq!(slots(frame), [2]);
        // Written again in the next frame: due once the grace frames have
        // passed without a draw that reads it.
        heap.queue_texture(3, true, false, frame + 1);
        assert_eq!(slots(frame + TABLED_GRACE), [2]);
        assert_eq!(slots(frame + 1 + TABLED_GRACE), [3, 2]);
        // The draw comes after all: out again.
        heap.tabled_in(3, true, frame + 1);
        assert_eq!(slots(u64::MAX), [2]);
        // A slot of a table no draw read stays queued; its next write waits.
        heap.tabled_in(2, false, frame + 1);
        assert_eq!(slots(frame + 1), [2]);
        heap.queue_texture(2, true, false, frame + 2);
        assert_eq!(slots(frame + 2), []);
        // Long after the last table, a write is due at once.
        heap.queue_texture(2, true, false, frame + 3 + TABLED_FRAMES);
        assert_eq!(slots(frame + 3 + TABLED_FRAMES), [2]);
    }

    #[test]
    fn dynamic_slots_are_held_until_released() {
        let Some(heap) = HostHeap::new(false, 4) else {
            return; // no host GPU
        };
        let a = heap.dynamic(heap.dynamic - 101).unwrap(); // the last slot is never given
        let b = heap.dynamic(60).unwrap();
        assert_eq!((a, b), (4, 4 + heap.dynamic - 101));
        // 40 left at the end: 50 do not fit until something is given back.
        assert_eq!(heap.dynamic(50), None);
        heap.release(a, 50);
        assert_eq!(heap.dynamic(50), Some(4));
        assert_eq!(heap.dynamic(40), Some(4 + heap.dynamic - 41));
    }

    /// Backing buffer and descriptor memory helper for concurrency stress tests
    /// and benchmarks on the host D3D12 device.
    struct StressFixture {
        _backing: Vec<u8>,
        base: u64,
        resource_id: usize,
        slots: Vec<[u32; 8]>,
    }

    impl StressFixture {
        fn new(slot_count: usize) -> Self {
            let backing = vec![0u8; 64 * 1024 + 256];
            let raw = backing.as_ptr() as u64;
            let base = (raw + 255) & !255;
            let resource_id = (&raw const backing[0]) as usize;
            crate::shadow::register(resource_id, base, 64 * 1024);
            Self {
                _backing: backing,
                base,
                resource_id,
                slots: vec![[0u32; 8]; slot_count],
            }
        }

        fn handle(&self, index: u32) -> u64 {
            (&raw const self.slots[index as usize]) as u64
        }

        fn write_cbv(&self, index: u32, offset: u64, size: u32) {
            let addr = self.base + offset;
            let desc = [addr as u32, (addr >> 32) as u32, size, 0];
            crate::descriptor::create_cbv(std::ptr::null(), &desc, self.handle(index));
        }

        fn write_raw_srv(&self, index: u32, offset: u64, dwords: u32) {
            let addr = self.base + offset;
            let resource = [1u32, 0, 0, 0, 64 * 1024, 0, 1, 0x1_0001, 0, 1, 0, 0, 0, 0];
            let mut desc = [0u32; 16];
            desc[0] = 39; // DXGI_FORMAT_R32_TYPELESS
            desc[1] = 1; // D3D12_SRV_DIMENSION_BUFFER
            desc[2] = 5768; // D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING
            desc[6] = dwords;
            desc[8] = 1; // D3D12_BUFFER_SRV_FLAG_RAW
            (desc[10], desc[11]) = (addr as u32, (addr >> 32) as u32);
            crate::descriptor::create_placed_srv(
                std::ptr::null(),
                &resource,
                &desc,
                self.handle(index),
            );
        }

        fn write_placed_tex(&self, index: u32, addr: u64, width: u32) {
            let resource = [
                3u32, 0, 0, 0, width, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0,
            ];
            let mut desc = [0u32; 16];
            desc[0] = 98;
            desc[1] = 4;
            desc[2] = 5768;
            desc[5] = 1;
            (desc[10], desc[11]) = (addr as u32, (addr >> 32) as u32);
            crate::descriptor::create_placed_srv(
                std::ptr::null(),
                &resource,
                &desc,
                self.handle(index),
            );
        }
    }

    impl Drop for StressFixture {
        fn drop(&mut self) {
            crate::shadow::unregister(self.resource_id, self.base);
        }
    }

    #[test]
    fn concurrent_cache_hit_and_distinct_create_view_storm() {
        let Some(host_gpu) = host() else {
            return; // no host GPU
        };
        const SLOTS: usize = 8_192;
        const THREADS: usize = 8;
        let Some(heap) = HostHeap::new(false, SLOTS as u32) else {
            return;
        };
        #[cfg(feature = "diagnostics")]
        {
            // Drain the one-time `probe_view_limit()` error on capped GPUs so
            // the assertion below checks only this test's D3D12 operations.
            let _ = host_gpu.log_messages();
        }
        let fixture = StressFixture::new(SLOTS);
        for i in 0..SLOTS as u32 {
            let off = u64::from(i % 128) * 256;
            if i.is_multiple_of(2) {
                fixture.write_cbv(i, off, 256);
            } else {
                fixture.write_raw_srv(i, off, 64);
            }
        }
        // Phase 1: 8-thread distinct-slot Create*View storm.
        let barrier = std::sync::Barrier::new(THREADS);
        std::thread::scope(|s| {
            for tid in 0..THREADS {
                let (heap, fixture, barrier) = (&heap, &fixture, &barrier);
                s.spawn(move || {
                    barrier.wait();
                    for i in (tid..SLOTS).step_by(THREADS) {
                        let idx = i as u32;
                        let view = heap.mirror(idx, fixture.handle(idx), true);
                        assert!(view.is_some(), "slot {idx} failed to mirror");
                    }
                });
            }
        });
        for i in 0..SLOTS as u32 {
            assert!(heap.made(i), "slot {i} not marked made");
        }
        // Phase 2: 8-thread cache-hit storm across all made slots.
        let barrier = std::sync::Barrier::new(THREADS);
        std::thread::scope(|s| {
            for tid in 0..THREADS {
                let (heap, fixture, barrier) = (&heap, &fixture, &barrier);
                s.spawn(move || {
                    barrier.wait();
                    for iter in 0..5_000usize {
                        let idx = ((iter * 131 + tid * 97) % SLOTS) as u32;
                        let view = heap.mirror(idx, fixture.handle(idx), true);
                        assert!(view.is_some(), "cache hit lost on slot {idx}");
                    }
                });
            }
        });
        // SAFETY: plain device query.
        assert!(unsafe { host_gpu.device.GetDeviceRemovedReason() }.is_ok());
        #[cfg(feature = "diagnostics")]
        assert_eq!(host_gpu.log_messages(), None);
    }

    #[test]
    fn concurrent_same_slot_and_stale_invalidations_do_not_deadlock() {
        let Some(host_gpu) = host() else {
            return; // no host GPU
        };
        const SLOTS: usize = 512;
        const THREADS: usize = 8;
        let Some(heap) = HostHeap::new(false, SLOTS as u32) else {
            return;
        };
        #[cfg(feature = "diagnostics")]
        {
            // Drain the one-time `probe_view_limit()` error on capped GPUs so
            // the assertion below checks only this test's D3D12 operations.
            let _ = host_gpu.log_messages();
        }
        let fixture = StressFixture::new(SLOTS);
        let tex_addr = fixture.base + 0x10_0000_0000;
        // Pre-populate per-thread placed texture descriptors at separate handle
        // slots so reading handles while other threads mutate different slots
        // is data-race-free, while hammering overlapping heap indices across
        // stripes (including reverse-stripe stale != index invalidations).
        let base_handles = StressFixture::new(32);
        base_handles.write_cbv(0, 0, 256);
        base_handles.write_raw_srv(1, 256, 64);
        base_handles.write_placed_tex(2, tex_addr, 64);
        base_handles.write_placed_tex(3, tex_addr, 128);
        let zero_slot = [0u32; 8];
        let zero_handle = (&raw const zero_slot) as u64;

        let barrier = std::sync::Barrier::new(THREADS);
        std::thread::scope(|s| {
            for tid in 0..THREADS {
                let (heap, base_handles, barrier) = (&heap, &base_handles, &barrier);
                s.spawn(move || {
                    barrier.wait();
                    // Target slots across both low and high stripes (e.g. 2, 200,
                    // 257, 259) to force cross-stripe stale != index invalidations
                    // in both ascending and descending stripe orders.
                    let contested = [2u32, 200, 257, 259, 3, 201];
                    for round in 0..800usize {
                        let slot = contested[(round + tid) % contested.len()];
                        match (round + tid) % 7 {
                            0 => {
                                let _ = heap.mirror(slot, base_handles.handle(0), true);
                            }
                            1 => {
                                let _ = heap.mirror(slot, base_handles.handle(1), true);
                            }
                            2 => {
                                let _ = heap.mirror(slot, base_handles.handle(2), false);
                            }
                            3 => {
                                let _ = heap.mirror(slot, base_handles.handle(3), false);
                            }
                            4 => {
                                let _ = heap.mirror(slot, zero_handle, true);
                            }
                            5 => {
                                let _ = heap.texture_slot_gone(slot);
                            }
                            _ => {
                                let h = base_handles.handle(2);
                                let _ = heap.texture_slot_made_from(slot, h);
                                // SAFETY: `h` points to 32 bytes in `base_handles`.
                                let rec = unsafe {
                                    std::slice::from_raw_parts(h as *const u8, 32)
                                };
                                let _ = heap.mirror_copied(slot + 16, h, rec);
                            }
                        }
                        if round % 64 == 0 {
                            let _ = heap.buffer_changes();
                            let _ = heap.texture_slots_due(round as u64, 16);
                            let _ = next_start(tex_addr, tex_addr + 0x10_0000);
                        }
                    }
                });
            }
        });
        // SAFETY: plain device query.
        assert!(unsafe { host_gpu.device.GetDeviceRemovedReason() }.is_ok());
        #[cfg(feature = "diagnostics")]
        assert_eq!(host_gpu.log_messages(), None);
    }

    #[test]
    fn concurrent_stale_invalidation_and_cbv_overwrite_never_clobbers_live_slot() {
        let Some(host_gpu) = host() else {
            return;
        };
        let Some(heap) = HostHeap::new(false, 512) else {
            return;
        };
        #[cfg(feature = "diagnostics")]
        {
            let _ = host_gpu.log_messages();
        }
        let fixture = StressFixture::new(16);
        let tex_addr = fixture.base + 0x40_0000_0000;
        fixture.write_cbv(0, 0, 256);
        fixture.write_placed_tex(1, tex_addr, 64);
        fixture.write_placed_tex(2, tex_addr, 128);
        let zero_slot = [0u32; 8];
        let zero_handle = (&raw const zero_slot) as u64;

        for round in 0..500usize {
            // Alternate low-vs-high stripe ordering (`10 > 2` and `3 < 300`).
            let (tex_slot, clash_slot) = if round.is_multiple_of(2) {
                (10u32, 2u32)
            } else {
                (3u32, 300u32)
            };
            assert!(heap.mirror(tex_slot, fixture.handle(1), false).is_some());
            assert!(heap.mirror(tex_slot, fixture.handle(1), true).is_some());
            assert!(heap.made(tex_slot));

            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|s| {
                let (heap, fixture, barrier) = (&heap, &fixture, &barrier);
                s.spawn(move || {
                    barrier.wait();
                    assert!(heap.mirror(tex_slot, fixture.handle(0), true).is_some());
                });
                s.spawn(move || {
                    barrier.wait();
                    assert!(heap.mirror(clash_slot, fixture.handle(2), false).is_some());
                });
            });
            // Regardless of thread order, `tex_slot` was overwritten with a CBV
            // either after or before `clash_slot`'s stale check, so the live CBV
            // must NEVER be wiped out by a delayed cross-stripe unmake.
            assert!(
                heap.made(tex_slot) && heap.has_kept(tex_slot),
                "round {round}: live CBV on slot {tex_slot} was clobbered"
            );
            // Reset both slots (including `placed` state) for the next round.
            assert!(heap.mirror(tex_slot, zero_handle, true).is_none());
            assert!(heap.mirror(clash_slot, zero_handle, true).is_none());
        }
        // SAFETY: plain device query.
        assert!(unsafe { host_gpu.device.GetDeviceRemovedReason() }.is_ok());
        #[cfg(feature = "diagnostics")]
        assert_eq!(host_gpu.log_messages(), None);
    }

    #[test]
    fn concurrent_texture_views_remade_and_fast_path_stay_consistent() {
        let Some(host_gpu) = host() else {
            return;
        };
        let Some(heap) = HostHeap::new(false, 64) else {
            return;
        };
        #[cfg(feature = "diagnostics")]
        {
            let _ = host_gpu.log_messages();
        }
        let fixture = StressFixture::new(4);
        let tex_addr = fixture.base + 0x50_0000_0000;
        fixture.write_placed_tex(0, tex_addr, 64);
        assert!(heap.mirror(5, fixture.handle(0), false).is_some());
        assert!(heap.mirror(5, fixture.handle(0), true).is_some());
        // SAFETY: `fixture.handle(0)` holds `[VIEW_TAG, i, generation]`.
        let [_, i, generation] =
            unsafe { (fixture.handle(0) as *const [u32; 3]).read_unaligned() };
        let expected_key = u64::from(generation) << 32 | u64::from(i);

        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|s| {
            let (heap, fixture, barrier) = (&heap, &fixture, &barrier);
            s.spawn(move || {
                barrier.wait();
                for _ in 0..300 {
                    levels_changed(tex_addr);
                    let _ = heap.texture_views_remade();
                }
            });
            for _ in 0..7 {
                s.spawn(move || {
                    barrier.wait();
                    for _ in 0..600 {
                        let loaded = heap.made[5].load(Ordering::Acquire);
                        if loaded == expected_key {
                            assert!(heap.has_kept(5), "kept missing while made[5] == key");
                        }
                        assert!(heap.mirror(5, fixture.handle(0), true).is_some());
                    }
                });
            }
        });
        assert!(heap.made(5) && heap.has_kept(5));
        // SAFETY: plain device query.
        assert!(unsafe { host_gpu.device.GetDeviceRemovedReason() }.is_ok());
        #[cfg(feature = "diagnostics")]
        assert_eq!(host_gpu.log_messages(), None);
    }

    #[test]
    fn mirror_release_acquire_later_and_forget_semantics() {
        let Some(host_gpu) = host() else {
            return;
        };
        let Some(heap) = HostHeap::new(false, 512) else {
            return;
        };
        #[cfg(feature = "diagnostics")]
        {
            let _ = host_gpu.log_messages();
        }
        let fixture = StressFixture::new(16);
        fixture.write_cbv(0, 0, 256);
        fixture.write_raw_srv(1, 256, 64);

        // `for_draw = false` defers buffer views (`later == true`): `made` stays 0.
        assert!(heap.mirror(0, fixture.handle(0), false).is_some());
        assert!(!heap.made(0));
        assert!(!heap.has_kept(0));

        // `for_draw = true` creates the descriptor and keeps the resource.
        assert!(heap.mirror(0, fixture.handle(0), true).is_some());
        assert!(heap.made(0));
        assert!(heap.has_kept(0));
        // Second call hits the lock-free fast path.
        assert!(heap.mirror(0, fixture.handle(0), true).is_some());

        // Table-backed resource entry: forgetting the resource invalidates the slot.
        let real_res = crate::resource::texture_at(
            fixture.base + 0x30_0000_0000,
            &[3, 0, 0, 0, 64, 0, 64, 0x1_0001, 28, 1, 0, 0x105, 0, 0],
        )
        .expect("real placed texture resource");
        let tex_srv_desc = [28u32, 4, 5768, 0, 0, 1, 0, 0, 0, 0];
        crate::descriptor::create_srv(
            std::ptr::null(),
            real_res,
            &tex_srv_desc,
            fixture.handle(2),
        );
        assert!(heap.mirror(2, fixture.handle(2), true).is_some());
        assert!(heap.made(2));
        assert!(heap.has_kept(2));
        crate::descriptor::forget(real_res as usize);
        assert!(heap.mirror(2, fixture.handle(2), true).is_none());
        assert!(!heap.made(2));
        assert!(!heap.has_kept(2));

        // Cross-stripe `stale != index` invalidates both ascending and descending.
        // Title write (`for_draw = false`) records the span in `placed` (`later = true`),
        // then draw (`for_draw = true`) creates the host descriptor (`made = true`);
        // a subsequent clashing title write (`for_draw = false`) invalidates it.
        let tex_addr = fixture.base + 0x20_0000_0000;
        fixture.write_placed_tex(3, tex_addr, 64);
        fixture.write_placed_tex(4, tex_addr, 128);
        assert!(heap.mirror(3, fixture.handle(3), false).is_some());
        assert!(!heap.made(3));
        assert!(heap.mirror(3, fixture.handle(3), true).is_some());
        assert!(heap.made(3));
        assert!(heap.has_kept(3));
        // Higher stripe (`300 > 3`) invalidates lower stripe `3`.
        assert!(heap.mirror(300, fixture.handle(4), false).is_some());
        assert!(!heap.made(3));
        assert!(!heap.has_kept(3));
        assert!(heap.mirror(300, fixture.handle(4), true).is_some());
        assert!(heap.made(300));
        assert!(heap.has_kept(300));
        // Lower stripe (`2 < 300`) invalidates higher stripe `300`.
        assert!(heap.mirror(2, fixture.handle(3), false).is_some());
        assert!(!heap.made(300));
        assert!(!heap.has_kept(300));
        assert!(heap.mirror(2, fixture.handle(3), true).is_some());
        assert!(heap.made(2));

        // `mirror_copied` skips zero records without allocating `copied`, mirrors
        // changed non-zero records, skips identical repeats, and nulls on clear.
        let zero_rec = [0u8; 32];
        assert!(!heap.mirror_copied(5, (&raw const zero_rec) as u64, &zero_rec));
        assert!(heap.copied.get().is_none());
        // SAFETY: `fixture.handle(1)` points to 32 bytes.
        let srv_rec = unsafe { std::slice::from_raw_parts(fixture.handle(1) as *const u8, 32) };
        assert!(heap.mirror_copied(5, fixture.handle(1), srv_rec));
        assert!(heap.copied.get().is_some());
        assert!(!heap.made(5)); // `for_draw = false` on buffer view
        assert!(!heap.mirror_copied(5, fixture.handle(1), srv_rec));
        // Sampler record (`later == false`): `mirror_copied` makes it immediately.
        let mut sampler_rec = [0u32; 8];
        crate::descriptor::create_sampler(
            std::ptr::null(),
            &[21, 3, 3, 3, 0, 1, 0, 0, 0, 0, 0, 0x7f7f_ffff, 0],
            (&raw mut sampler_rec) as u64,
        );
        let Some(sampler_heap) = HostHeap::new(true, 16) else {
            return;
        };
        // SAFETY: `sampler_rec` is 16 sampler descriptor bytes.
        let sampler_bytes =
            unsafe { std::slice::from_raw_parts((&raw const sampler_rec) as *const u8, 16) };
        assert!(sampler_heap.mirror_copied(6, (&raw const sampler_rec) as u64, sampler_bytes));
        assert!(sampler_heap.made(6));
        assert!(!sampler_heap.mirror_copied(6, (&raw const sampler_rec) as u64, sampler_bytes));
        assert!(sampler_heap.mirror_copied(6, (&raw const zero_rec) as u64, &zero_rec[..16]));
        assert!(!sampler_heap.made(6));

        // SAFETY: plain device query.
        assert!(unsafe { host_gpu.device.GetDeviceRemovedReason() }.is_ok());
        #[cfg(feature = "diagnostics")]
        assert_eq!(host_gpu.log_messages(), None);
    }

    #[test]
    #[ignore = "8-thread RTX 4070 SUPER benchmark: run with --release -- --ignored --nocapture"]
    fn bench_mirror_fast_path_8_threads() {
        let Some(_host_gpu) = host() else {
            return;
        };
        const SLOTS: usize = 1_024;
        const THREADS: usize = 8;
        const HITS_PER_THREAD: usize = 50_000;
        let run_bench = |cbv_only: bool, label: &str| {
            let Some(heap) = HostHeap::new(false, SLOTS as u32) else {
                return;
            };
            let fixture = StressFixture::new(SLOTS);
            for i in 0..SLOTS as u32 {
                let off = u64::from(i % 128) * 256;
                if cbv_only || i.is_multiple_of(2) {
                    fixture.write_cbv(i, off, 256);
                } else {
                    fixture.write_raw_srv(i, off, 64);
                }
                assert!(heap.mirror(i, fixture.handle(i), true).is_some());
            }
            let mut samples_ns = Vec::new();
            for _trial in 0..5 {
                let start_barrier = std::sync::Barrier::new(THREADS + 1);
                let done_barrier = std::sync::Barrier::new(THREADS + 1);
                let elapsed = std::thread::scope(|s| {
                    for tid in 0..THREADS {
                        let (heap, fixture, start_barrier, done_barrier) =
                            (&heap, &fixture, &start_barrier, &done_barrier);
                        s.spawn(move || {
                            start_barrier.wait();
                            for iter in 0..HITS_PER_THREAD {
                                let idx = ((iter * 131 + tid * 17) % SLOTS) as u32;
                                std::hint::black_box(heap.mirror(idx, fixture.handle(idx), true));
                            }
                            done_barrier.wait();
                        });
                    }
                    start_barrier.wait();
                    let t0 = std::time::Instant::now();
                    done_barrier.wait();
                    t0.elapsed().as_nanos() as u64
                });
                samples_ns.push(elapsed);
            }
            samples_ns.sort_unstable();
            let median_ms = samples_ns[samples_ns.len() / 2] as f64 / 1e6;
            let best_ms = samples_ns[0] as f64 / 1e6;
            let total_ops = THREADS * HITS_PER_THREAD;
            let mops = total_ops as f64 / (median_ms / 1e3) / 1e6;
            eprintln!(
                "BENCH fast_path_8t[{label}]: total_ops={total_ops} median_ms={median_ms:.3} best_ms={best_ms:.3} throughput={mops:.2} Mops/s"
            );
        };
        run_bench(true, "cbv_inline");
        run_bench(false, "mixed_cbv_srv");
    }

    #[test]
    #[ignore = "8-thread RTX 4070 SUPER benchmark: run with --release -- --ignored --nocapture"]
    fn bench_mirror_slow_path_create_views_8_threads() {
        let Some(host_gpu) = host() else {
            return;
        };
        const SLOTS: usize = 16_384;
        const THREADS: usize = 8;
        let fixture = StressFixture::new(SLOTS);
        for i in 0..SLOTS as u32 {
            let off = u64::from(i % 128) * 256;
            if i.is_multiple_of(2) {
                fixture.write_cbv(i, off, 256);
            } else {
                fixture.write_raw_srv(i, off, 64);
            }
        }
        let mut samples_ns = Vec::new();
        for _trial in 0..3 {
            let Some(heap) = HostHeap::new(false, SLOTS as u32) else {
                return;
            };
            let start_barrier = std::sync::Barrier::new(THREADS + 1);
            let done_barrier = std::sync::Barrier::new(THREADS + 1);
            let elapsed = std::thread::scope(|s| {
                for tid in 0..THREADS {
                    let (heap, fixture, start_barrier, done_barrier) =
                        (&heap, &fixture, &start_barrier, &done_barrier);
                    s.spawn(move || {
                        start_barrier.wait();
                        for i in (tid..SLOTS).step_by(THREADS) {
                            let idx = i as u32;
                            std::hint::black_box(heap.mirror(idx, fixture.handle(idx), true));
                        }
                        done_barrier.wait();
                    });
                }
                start_barrier.wait();
                let t0 = std::time::Instant::now();
                done_barrier.wait();
                t0.elapsed().as_nanos() as u64
            });
            assert!(heap.made(0) && heap.made((SLOTS - 1) as u32));
            samples_ns.push(elapsed);
        }
        // SAFETY: plain device query.
        assert!(unsafe { host_gpu.device.GetDeviceRemovedReason() }.is_ok());
        samples_ns.sort_unstable();
        let median_ms = samples_ns[samples_ns.len() / 2] as f64 / 1e6;
        let best_ms = samples_ns[0] as f64 / 1e6;
        let kops = SLOTS as f64 / (median_ms / 1e3) / 1e3;
        eprintln!(
            "BENCH slow_path_create_views_8t: slots={SLOTS} median_ms={median_ms:.3} best_ms={best_ms:.3} throughput={kops:.2} Kviews/s"
        );
    }
}

#[cfg(test)]
mod record_print_tests {
    use super::record_print;

    /// Equal records print alike; a record that differs in any one byte,
    /// or in its length, prints otherwise.
    #[test]
    fn a_record_that_changed_prints_otherwise() {
        let record: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(37).wrapping_add(5)).collect();
        let print = record_print(&record);
        assert_eq!(print, record_print(&record.clone()));
        for at in 0..32 {
            for bit in 0..8 {
                let mut other = record.clone();
                other[at] ^= 1 << bit;
                assert_ne!(record_print(&other), print, "byte {at} bit {bit}");
            }
        }
        assert_ne!(record_print(&record[..16]), print);
        assert_ne!(record_print(&[0u8; 32]), record_print(&[0u8; 16]));
        // A record of zeros is told from "never looked at" by the caller's `| 1`.
        assert_ne!(record_print(&[0u8; 32]) | 1, 0);
    }
}

#[cfg(test)]
mod own_copy_tests {
    use super::own_bytes_if;

    /// A buffer view gets a copy of its own bytes only when it has no host
    /// view on its buffer's copy: off that copy's elements, and not a
    /// structured view that can be a raw view of the same bytes.
    #[test]
    fn a_view_off_its_copys_elements_gets_its_own_bytes() {
        let base = 0xd_9000_0000u64;
        // On an element: the buffer's copy (the two cases of every run's
        // log are the ones below).
        assert_eq!(own_bytes_if(true, 24, base, 3, 10, 12, 0, false), None);
        assert_eq!(own_bytes_if(true, 0, base, 0, 10, 12, 0, false), None);
        // A typed view of 12-byte elements, 5,222,240 bytes into its copy
        // (8 past an element): its own bytes, from its first element.
        assert_eq!(own_bytes_if(true, 5_222_240, base, 3, 10, 12, 0, false), Some((base + 36, 120)));
        // A structured view of 32-byte elements 8 bytes in: not on 16, so
        // no raw view of the copy either.
        assert_eq!(own_bytes_if(true, 8, base, 2, 4, 32, 32, true), Some((base + 64, 128)));
        // The same 16 bytes in: off its elements, but a raw view of the
        // copy's bytes does (as before).
        assert_eq!(own_bytes_if(true, 16, base, 2, 4, 32, 32, true), None);
        // No elements, no size, or the switch off: as before.
        assert_eq!(own_bytes_if(true, 8, base, 2, 0, 32, 32, true), None);
        assert_eq!(own_bytes_if(true, 8, base, 2, 4, 0, 0, false), None);
        assert_eq!(own_bytes_if(false, 8, base, 2, 4, 32, 32, true), None);
    }
}
