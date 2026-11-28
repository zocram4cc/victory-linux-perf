//! Buffers shaders write through views they pick from the heap by an index
//! they compute (`VICTORY_PICKED_WRITES`; off by default, [`stage`]).
//!
//! Which views a draw or dispatch writes through is known from its tables
//! and root parameters, and their ranges are declared: what is written there
//! comes back to title memory (`Shadow::gpu_writes`). A view picked by an
//! index out of a buffer, per lane, names any of the heap's. What was
//! written through one stayed on the card: title memory, and the title's own
//! copies and textures made from it, never saw it, and a refresh of its 64
//! KiB put title memory's older bytes over it. (Captures replay1 and
//! replay2: `[compute 82a2ad6d07417d36]` writes 4.6 MB a frame of a 24 MB
//! buffer so.) To declare all such a shader could write would be 987 MB of
//! 573 buffers for every list that runs one.
//!
//! Two things make it small. The compare on the card says what changed since
//! the last time it looked, whoever wrote it and whenever: looking now and
//! then misses no buffer, it only finds it later. And a buffer that was
//! written so once is written so again. So with a submission that has a
//! shader that stores through a picked view ([`stores_picked`])
//!
//! - the buffers found written that way ([`found`]) are declared: compared
//!   and landed with the submission, as any declared write;
//! - of the others that the heap shows unordered views of the next [`slice`]
//!   in turn, once a frame, is compared, and nothing else: at 32 MB all of
//!   them in some 30 frames. Where something changed that nobody declared,
//!   the buffer is one of the first kind from then on.
//!
//! What is found late is not landed late: the title may have other things in
//! that memory by then. It goes to title memory with the next declared write
//! to its 64 KiB. So what is written once, into a buffer not found yet, stays
//! on the card as before.
//!
//! The declarations are the submission's, made when its refreshes are done
//! (`Host::submit`): no refresh of it finds a range closed on their account.
//!
//! Only copies in the card's memory: for those in the PC's, a declared range
//! is a snapshot and a compare on the CPU. (They hold 55 MB of the 987.)
//!
//! What it costs: the hashing of the slice and of the buffers found, and
//! what changed of the latter coming back. What else changes: a step on the
//! CPU that reads a buffer found waits for the flight, as it should always
//! have done.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::host::HostList;
use crate::shadow::Shadow;

pub(crate) fn on() -> bool {
    stage() > 0
}

/// `VICTORY_PICKED_WRITES`: 0 (off), 1 (what is found is declared, so that
/// no refresh sends title memory over it while a flight may write it, and
/// not landed), 2 (landed too).
fn stage() -> u8 {
    static STAGE: OnceLock<u8> = OnceLock::new();
    *STAGE.get_or_init(|| match std::env::var("VICTORY_PICKED_WRITES").as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        _ => 0,
    })
}

/// Whether what is found is landed.
pub(crate) fn lands() -> bool {
    stage() > 1
}

/// Bytes looked at in turn per frame (`VICTORY_PICKED_MB`).
fn slice() -> u64 {
    static SLICE: OnceLock<u64> = OnceLock::new();
    *SLICE.get_or_init(|| {
        let mb = std::env::var("VICTORY_PICKED_MB").ok().and_then(|v| v.trim().parse::<u64>().ok());
        mb.unwrap_or(32) << 20
    })
}

/// A list's key for [`HostList::first_use`]: it runs such a shader (no title
/// resource is there).
const KEY: usize = usize::MAX - 0x7a;

/// `list` runs a shader that stores through a picked view.
pub(crate) fn runs_in(list: &HostList) {
    list.first_use(KEY);
}

/// [`crate::pipeline::Translated::picks`]: not asked yet, asked, the answers.
const UNKNOWN: u8 = 0;
const ASKED: u8 = 1;
const NO: u8 = 2;
const YES: u8 = 3;

/// Whether `shader`, which stores to buffers, does so through a view it
/// picks. Tracing its descriptors takes some 50 ms, so a thread of ours does
/// it when the shader is first bound, and it is "no" until then: what it
/// writes is found some frames later in any case. (Of the 25,904 programs of
/// the title 2,923 store and have the view heap among their parameters; 37
/// compute programs store through views they pick.)
pub(crate) fn picks(shader: &Arc<crate::pipeline::Shader>, known: &AtomicU8) -> bool {
    match known.load(Relaxed) {
        YES => true,
        UNKNOWN => {
            if known.compare_exchange(UNKNOWN, ASKED, Relaxed, Relaxed).is_ok() {
                ask(shader.clone());
            }
            false
        }
        _ => false,
    }
}

fn ask(shader: Arc<crate::pipeline::Shader>) {
    use std::sync::mpsc::{Sender, channel};
    static ASKS: OnceLock<Mutex<Sender<Arc<crate::pipeline::Shader>>>> = OnceLock::new();
    let asks = ASKS.get_or_init(|| {
        let (send, asks) = channel::<Arc<crate::pipeline::Shader>>();
        let _ = std::thread::Builder::new().name("victory-picked".into()).spawn(move || {
            use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
            // SAFETY: this thread's own.
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
            for shader in asks {
                let yes = shader.programs().is_none_or(|all| all.iter().any(stores_picked));
                if yes {
                    tracing::info!(hash = %shader.hash, "a shader stores through views it picks from the heap (VICTORY_PICKED_WRITES=1)");
                }
                if let Some(t) = shader.translated() {
                    t.picks.store(if yes { YES } else { NO }, Relaxed);
                }
            }
        });
        Mutex::new(send)
    });
    let _ = asks.lock().unwrap_or_else(|e| e.into_inner()).send(shader);
}

/// Whether the code of `program` stores to a buffer through a descriptor
/// that its tables do not name: loaded from the view heap, or from a table at
/// an offset it computes (programs with the view heap among their parameters). Yes where that cannot be told.
fn stores_picked(program: &victory_shader::NativeProgram) -> bool {
    use victory_shader::HwStage;
    use victory_shader::gfx10::Fields;
    use victory_shader::native::RootType;
    use victory_shader::resource::{Role, Source};
    // (The programs that are translated.)
    if !matches!(program.stage, HwStage::Vs | HwStage::Ps | HwStage::Cs | HwStage::Ms) {
        return false;
    }
    let heap = |param: usize| program.layout.get(param).is_some_and(|p| p.root == RootType::ResourceHeap);
    if !(0..program.layout.len()).any(heap) {
        return false;
    }
    let Ok(p) = victory_shader::spill::build(program) else {
        return true;
    };
    // The structure is what takes time, and only per-lane descriptors need
    // it.
    let plain = victory_shader::resource::track(&p, &program.layout, program.stage, None);
    let res = plain.or_else(|_| {
        let st = victory_shader::structure::structure(&p);
        victory_shader::resource::track(&p, &program.layout, program.stage, st.as_ref().ok())
    });
    let Ok(res) = res else {
        return true;
    };
    res.descriptors.iter().filter(|d| d.role == Role::Resource).any(|d| {
        let inst = &p.blocks[d.site.0].insts[d.site.1];
        let stores = matches!(inst.fields, Fields::Buffer { .. }) && (inst.name().contains("store") || inst.name().contains("atomic"));
        stores
            && d.source.leaves().into_iter().any(|s| match s {
                Source::Loaded { load, .. } | Source::Fetched { load, .. } => res
                    .scalar
                    .iter()
                    .find(|(site, _)| site == load)
                    .is_some_and(|(_, a)| heap(a.param) || a.dynamic.is_some()),
                _ => false,
            })
    })
}

/// A buffer the heap shows unordered views of: title addresses, from the
/// first to the last.
struct Shown {
    copy: Weak<Shadow>,
    from: u64,
    to: u64,
}

#[derive(Default)]
struct State {
    shown: BTreeMap<u64, Shown>,
    /// The copies (by number) found written through picked views.
    found: BTreeSet<u64>,
    /// Where the next slice begins: copy, offset from its first address.
    next: (u64, u64),
    /// The frame of the last slice.
    frame: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

impl State {
    /// The ranges of the next `most` bytes in turn, at most all once.
    fn turn(&mut self, most: u64) -> Vec<(u64, u64, u64)> {
        let mut out: Vec<(u64, u64, u64)> = Vec::new();
        let mut left = most;
        // (The one it begins in may come twice: its end first.)
        for _ in 0..=self.shown.len() {
            let (copy, offset) = self.next;
            let Some((&id, s)) = self.shown.range(copy..).next().or_else(|| self.shown.iter().next()) else {
                break;
            };
            let offset = if id == copy { offset } else { 0 };
            let len = s.to - s.from;
            let n = len.saturating_sub(offset).min(left);
            if n > 0 && !self.found.contains(&id) {
                if out.iter().any(|r| r.0 == id) {
                    break;
                }
                out.push((id, s.from + offset, n));
                left -= n;
                if offset + n < len {
                    self.next = (id, offset + n);
                    break;
                }
            }
            self.next = (id + 1, 0);
            if left == 0 {
                break;
            }
        }
        out
    }
}

/// The pass over the heap's buffer views (`crate::draw`): the unordered ones
/// it looked at (copy, from, to); `all` if those are all there are.
pub(crate) fn shown(views: Vec<(Arc<Shadow>, u64, u64)>, all: bool) {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = state.get_or_insert_with(State::default);
    if all {
        state.shown.clear();
    }
    for (copy, from, to) in views.into_iter().filter(|v| v.0.carded() && v.1 < v.2) {
        let e = state.shown.entry(copy.id()).or_insert_with(|| Shown {
            copy: Arc::downgrade(&copy),
            from,
            to,
        });
        (e.from, e.to) = (e.from.min(from), e.to.max(to));
    }
}

/// A range of a copy: title address and bytes.
pub(crate) type Range = (Arc<Shadow>, usize, usize);

/// What is done with a submission.
#[derive(Default)]
pub(crate) struct Ranges {
    /// To be declared.
    pub(crate) written: Vec<Range>,
    /// To be compared only.
    pub(crate) looked: Vec<Range>,
}

/// Of a submission that is about to go to the card, its refreshes done: what
/// is to be declared and looked at, if one of its lists runs a shader that
/// stores through a picked view.
pub(crate) fn declare(runs: &[&HostList]) -> Ranges {
    if !on() || !runs.iter().any(|list| list.used(KEY)) {
        return Ranges::default();
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = state.as_mut() else {
        return Ranges::default();
    };
    let frame = crate::frame::current();
    let written: Vec<(u64, u64, u64)> =
        state.found.iter().filter_map(|id| state.shown.get(id).map(|s| (*id, s.from, s.to - s.from))).collect();
    let mut looked = Vec::new();
    if state.frame != frame {
        state.frame = frame;
        looked = state.turn(slice());
    }
    let of = |ranges: Vec<(u64, u64, u64)>, count: usize| -> Vec<Range> {
        COUNTS[count].fetch_add(ranges.iter().map(|r| r.2).sum(), Relaxed);
        ranges
            .into_iter()
            .filter_map(|(id, at, len)| Some((state.shown.get(&id)?.copy.upgrade()?, at as usize, len as usize)))
            .collect()
    };
    Ranges { written: of(written, 0), looked: of(looked, 1) }
}

/// The compare found a piece changed (it begins at offset `at` of `copy`)
/// where it only looked: something wrote it through a view nobody declared.
pub(crate) fn found(copy: &Arc<Shadow>, at: usize) {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.get_or_insert_with(State::default).found.insert(copy.id()) {
        let (start, len) = copy.span();
        tracing::info!(
            "a buffer is written through views picked from the heap: {start:#x}+{len:#x} (first seen at +{at:#x}; VICTORY_PICKED_WRITES=1): \
             declared with every submission that may write it from here on"
        );
    }
}

/// Since the last frame-rate line: bytes declared, and bytes looked at.
static COUNTS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// For the frame-rate line.
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let [declared, looked] = [0, 1].map(|i| COUNTS[i].swap(0, Relaxed) as f64 / frames / (1 << 20) as f64);
    let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let (shown, found) = state.as_ref().map_or((0, 0), |s| (s.shown.len(), s.found.len()));
    tracing::info!(
        "views picked from the heap, per frame (VICTORY_PICKED_WRITES=1): {declared:.1} MB declared for them, {looked:.1} MB \
         looked at in turn; unordered views of {shown} buffers on the card, {found} of them found written"
    );
}

/// Declared writes, per copy, and those of `picked`.
pub(crate) fn with(mut back: Vec<(Arc<Shadow>, Vec<(usize, usize)>)>, picked: &[Range]) -> Vec<(Arc<Shadow>, Vec<(usize, usize)>)> {
    for (copy, at, len) in picked {
        let range = (*at, *at + *len);
        match back.iter_mut().find(|b| Arc::ptr_eq(&b.0, copy)) {
            Some((_, ranges)) if ranges.contains(&range) => {}
            Some((_, ranges)) => ranges.push(range),
            None => back.push((copy.clone(), vec![range])),
        }
    }
    back
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(sizes: &[u64]) -> State {
        let mut state = State::default();
        for (i, &n) in sizes.iter().enumerate() {
            let from = (i as u64 + 1) << 32;
            state.shown.insert(i as u64 * 3 + 2, Shown { copy: Weak::new(), from, to: from + n });
        }
        state
    }

    /// Every byte in turn, none left out, whatever the slice.
    #[test]
    fn all_buffers_are_looked_at_in_turn() {
        let sizes = [100u64, 7, 1000, 1, 350];
        let total: u64 = sizes.iter().sum();
        for most in [1u64, 10, 99, 350, 1000, 5000] {
            let mut s = state(&sizes);
            let mut seen = std::collections::BTreeMap::<u64, u64>::new();
            // (A turn ends with the list: it may take one more.)
            let turns = total.div_ceil(most) + sizes.len() as u64 + 1;
            for _ in 0..turns {
                let turn = s.turn(most);
                assert!(turn.iter().map(|r| r.2).sum::<u64>() <= most);
                for (id, at, n) in turn {
                    let from = s.shown[&id].from;
                    for b in at - from..at - from + n.min(2000) {
                        *seen.entry(id << 32 | b).or_default() += 1;
                    }
                }
            }
            assert_eq!(seen.len() as u64, total, "slices of {most}");
        }
    }

    #[test]
    fn what_was_found_has_no_turn() {
        let mut s = state(&[64, 64, 64]);
        s.found.insert(5);
        let turn = s.turn(1000);
        assert_eq!(turn.iter().map(|r| r.0).collect::<Vec<_>>(), [2, 8]);
        assert!(state(&[]).turn(10).is_empty());
        let mut all = state(&[64]);
        all.found.insert(2);
        assert!(all.turn(10).is_empty());
    }
}
