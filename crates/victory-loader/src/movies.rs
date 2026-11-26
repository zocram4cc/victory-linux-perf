//! Opt-in skipping of the title's Bink movies, to shorten test runs.
//!
//! `VICTORY_SKIP_MOVIES=<strategy>` binds a few of the title's imports from
//! Bink 2 (`bink2scarlett.dll`) to the stubs below instead of the DLL; every
//! other import stays with the DLL and no title code is patched. Unset, empty
//! or `0`: nothing changes. An unknown value is reported and ignored.
//!
//! - `open`: `BinkOpenWithOptions` fails as Bink does when it cannot open a
//!   file (a null `HBINK`); the DLL is not called.
//! - `end`: movies open normally; the first `BinkNextFrame` after each open
//!   becomes `BinkGoto(bink, Frames, 0)` (the DLL's own seek, as the title
//!   uses it: decode follows), so the frame the title decodes next is the
//!   last one and it ends the movie. Later calls go to the DLL, so a looping
//!   movie loops from there.
//! - `fast`: movies open normally; until a movie's last frame is decoded,
//!   `BinkWait` answers 0 ("time for the next frame") and `BinkShouldSkip`
//!   1 ("behind"), so the title's own catch-up loop decodes frame after
//!   frame without waiting. Without the seek of `end`, but only as fast as
//!   decoding, and only for movies whose player asks `BinkShouldSkip`.
//!
//! Audio, threads, frame buffers, pause and close always stay with the DLL.

#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use victory_pe::ImportSymbol;

/// The switch; see the module documentation.
pub(crate) const SKIP_MOVIES_ENV: &str = "VICTORY_SKIP_MOVIES";

/// The title's Bink 2 DLL.
const BINK_DLL: &str = "bink2scarlett.dll";

/// Bink 2's `BINK` starts with `u32` Width, Height, Frames, FrameNum,
/// LastFrameNum (1-based). The title copies Frames after opening a movie
/// and ends it once LastFrameNum reaches that count; Bink's own
/// `BinkShouldSkip` compares FrameNum with Frames the same way.
const FRAMES: usize = 2;
const LAST_FRAME_NUM: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strategy {
    Open,
    End,
    Fast,
}

impl Strategy {
    /// `Ok(None)` is off; `Err` a value this build does not know.
    fn parse(value: &str) -> Result<Option<Self>, ()> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "0" => Ok(None),
            "open" => Ok(Some(Self::Open)),
            "end" => Ok(Some(Self::End)),
            "fast" => Ok(Some(Self::Fast)),
            _ => Err(()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::End => "end",
            Self::Fast => "fast",
        }
    }

    /// The Bink imports this strategy binds to stubs.
    fn replaces(self) -> &'static [&'static str] {
        match self {
            Self::Open => &["BinkOpenWithOptions"],
            Self::End => &["BinkOpenWithOptions", "BinkNextFrame"],
            Self::Fast => &["BinkOpenWithOptions", "BinkWait", "BinkShouldSkip"],
        }
    }

    /// The DLL's own entry points its stubs call.
    fn needs(self) -> &'static [&'static str] {
        match self {
            Self::Open => &[],
            Self::End => &["BinkOpenWithOptions", "BinkNextFrame", "BinkGoto"],
            Self::Fast => &["BinkOpenWithOptions", "BinkWait", "BinkShouldSkip"],
        }
    }
}

/// What the stubs act on: the strategy and the DLL's entry points (0 where
/// the strategy needs none).
#[derive(Debug, Clone, Copy)]
struct Skip {
    strategy: Strategy,
    open: usize,
    next_frame: usize,
    goto: usize,
    wait: usize,
    should_skip: usize,
}

impl Skip {
    /// `None` if the DLL lacks an entry point the strategy needs.
    fn find(strategy: Strategy, export: &dyn Fn(&str) -> Option<usize>) -> Option<Self> {
        let get = |name: &str| {
            if strategy.needs().contains(&name) {
                export(name)
            } else {
                Some(0)
            }
        };
        Some(Self {
            strategy,
            open: get("BinkOpenWithOptions")?,
            next_frame: get("BinkNextFrame")?,
            goto: get("BinkGoto")?,
            wait: get("BinkWait")?,
            should_skip: get("BinkShouldSkip")?,
        })
    }
}

type Hbink = *mut c_void;
type OpenFn = unsafe extern "system" fn(*const c_void, *const c_void, u32) -> Hbink;
type NextFrameFn = unsafe extern "system" fn(Hbink);
type GotoFn = unsafe extern "system" fn(Hbink, u32, i32);
type QueryFn = unsafe extern "system" fn(Hbink) -> i32;

/// Set once, while binding (before any title code runs).
static ACTIVE: OnceLock<Option<Skip>> = OnceLock::new();

/// Movies opened and not yet skipped, with how often `fast` answered for
/// each. A handle closed before it plays stays listed; Bink only hands it
/// out again from an open, which lists it anyway.
static ARMED: Mutex<BTreeMap<usize, u32>> = Mutex::new(BTreeMap::new());

static ACTS: AtomicU64 = AtomicU64::new(0);

fn strategy() -> Option<Strategy> {
    static STRATEGY: OnceLock<Option<Strategy>> = OnceLock::new();
    *STRATEGY.get_or_init(|| {
        let value = std::env::var(SKIP_MOVIES_ENV).unwrap_or_default();
        Strategy::parse(&value).unwrap_or_else(|()| {
            tracing::warn!(
                "{SKIP_MOVIES_ENV}={value:?} is not open, end or fast: movies play as usual"
            );
            None
        })
    })
}

/// The stub to bind for the title DLL import `dll!symbol` instead of the
/// DLL's own export, if `VICTORY_SKIP_MOVIES` asks for one. `export` finds
/// that DLL's exports by name.
pub(crate) fn redirect(
    dll: &str,
    symbol: &ImportSymbol,
    export: &dyn Fn(&str) -> Option<usize>,
) -> Option<usize> {
    let strategy = strategy()?;
    let ImportSymbol::Name { name, .. } = symbol else {
        return None;
    };
    if !dll.eq_ignore_ascii_case(BINK_DLL) || !strategy.replaces().contains(&name.as_str()) {
        return None;
    }
    let skip = (*ACTIVE.get_or_init(|| {
        let found = Skip::find(strategy, export);
        if found.is_none() {
            tracing::warn!(
                "{SKIP_MOVIES_ENV}={}: {dll} lacks one of {:?}; movies play as usual",
                strategy.name(),
                strategy.needs()
            );
        }
        found
    }))?;
    let stub = stub(skip.strategy, name)?;
    tracing::info!(
        "{SKIP_MOVIES_ENV}={}: {dll}!{name} bound to a stub",
        skip.strategy.name()
    );
    Some(stub)
}

fn stub(strategy: Strategy, name: &str) -> Option<usize> {
    if !strategy.replaces().contains(&name) {
        return None;
    }
    Some(match name {
        "BinkOpenWithOptions" => open_with_options as OpenFn as usize,
        "BinkNextFrame" => next_frame as NextFrameFn as usize,
        "BinkWait" => wait as QueryFn as usize,
        "BinkShouldSkip" => should_skip as QueryFn as usize,
        _ => return None,
    })
}

fn active() -> Option<Skip> {
    ACTIVE.get().copied().flatten()
}

fn armed() -> std::sync::MutexGuard<'static, BTreeMap<usize, u32>> {
    ARMED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Logs the first skip at INFO, later ones at powers of two (DEBUG between).
fn acted(strategy: Strategy, call: &str, frames: Option<u32>) {
    let n = ACTS.fetch_add(1, Ordering::Relaxed) + 1;
    let strategy = strategy.name();
    if n == 1 {
        tracing::info!(frames, "movie skipped (strategy={strategy}, call={call})");
    } else if n.is_power_of_two() {
        tracing::info!(
            frames,
            skips = n,
            "movie skipped again (strategy={strategy}, call={call})"
        );
    } else {
        tracing::debug!(
            frames,
            skips = n,
            "movie skipped (strategy={strategy}, call={call})"
        );
    }
}

/// # Safety
/// `bink` is a live handle from Bink's open.
unsafe fn field(bink: Hbink, index: usize) -> u32 {
    // SAFETY: the first fields of `BINK` (see `FRAMES`); the DLL may write
    // them from its own threads, so read once and as they are.
    unsafe { bink.cast::<u32>().add(index).read_volatile() }
}

/// `HBINK BinkOpenWithOptions(name, options, flags)`; null when it fails.
unsafe extern "system" fn open_with_options(
    name: *const c_void,
    options: *const c_void,
    flags: u32,
) -> Hbink {
    match active() {
        // SAFETY: the title's arguments, passed on unchanged.
        Some(skip) => unsafe { open_with(&skip, name, options, flags) },
        None => std::ptr::null_mut(),
    }
}

/// # Safety
/// The arguments of `BinkOpenWithOptions`; `skip` from `Skip::find`.
unsafe fn open_with(skip: &Skip, name: *const c_void, options: *const c_void, flags: u32) -> Hbink {
    if skip.strategy == Strategy::Open {
        acted(skip.strategy, "BinkOpenWithOptions", None);
        return std::ptr::null_mut();
    }
    // SAFETY: the DLL's export, with its own signature.
    let open: OpenFn = unsafe { std::mem::transmute::<usize, OpenFn>(skip.open) };
    // SAFETY: the title's arguments, passed on unchanged.
    let bink = unsafe { open(name, options, flags) };
    if !bink.is_null() {
        armed().insert(bink as usize, 0);
    }
    bink
}

/// `void BinkNextFrame(HBINK)`.
unsafe extern "system" fn next_frame(bink: Hbink) {
    if let Some(skip) = active() {
        // SAFETY: the title's handle, passed on.
        unsafe { next_frame_with(&skip, bink) }
    }
}

/// # Safety
/// `bink` as the title passes it to `BinkNextFrame`; `skip` from `Skip::find`.
unsafe fn next_frame_with(skip: &Skip, bink: Hbink) {
    if !bink.is_null() && armed().remove(&(bink as usize)).is_some() {
        // SAFETY: a handle the DLL opened and the title still uses.
        let frames = unsafe { field(bink, FRAMES) };
        if frames > 1 {
            acted(skip.strategy, "BinkNextFrame", Some(frames));
            // SAFETY: the DLL's export, with its own signature; called where
            // the title calls its own `BinkGoto` (no decode in flight), and
            // like there the title decodes the frame next.
            unsafe { std::mem::transmute::<usize, GotoFn>(skip.goto)(bink, frames, 0) };
            return;
        }
    }
    // SAFETY: the DLL's export, with its own signature and the title's handle.
    unsafe { std::mem::transmute::<usize, NextFrameFn>(skip.next_frame)(bink) }
}

/// `S32 BinkWait(HBINK)`: non-zero while it is not yet time for the next frame.
unsafe extern "system" fn wait(bink: Hbink) -> i32 {
    match active() {
        // SAFETY: the title's handle, passed on.
        Some(skip) => unsafe { query_with(&skip, bink, "BinkWait", 0, skip.wait) },
        None => 0,
    }
}

/// `S32 BinkShouldSkip(HBINK)`: non-zero when playback is behind.
unsafe extern "system" fn should_skip(bink: Hbink) -> i32 {
    match active() {
        // SAFETY: the title's handle, passed on.
        Some(skip) => unsafe { query_with(&skip, bink, "BinkShouldSkip", 1, skip.should_skip) },
        None => 0,
    }
}

/// `fast`: answers `forced` for a movie that has not shown its last frame
/// yet, the DLL's `real` answer from then on.
///
/// # Safety
/// `bink` as the title passes it to Bink; `real` that call's export.
unsafe fn query_with(skip: &Skip, bink: Hbink, call: &str, forced: i32, real: usize) -> i32 {
    if !bink.is_null() {
        let mut armed = armed();
        if let Some(count) = armed.get_mut(&(bink as usize)) {
            // SAFETY: a handle the DLL opened and the title still uses.
            let (frames, last) = unsafe { (field(bink, FRAMES), field(bink, LAST_FRAME_NUM)) };
            // Bounded, should the frame count never be reached (a loop).
            if last < frames && *count <= frames.saturating_mul(2).saturating_add(16) {
                *count += 1;
                drop(armed);
                acted(skip.strategy, call, Some(frames));
                return forced;
            }
            armed.remove(&(bink as usize));
        }
    }
    // SAFETY: the DLL's export, with its own signature and the title's handle.
    unsafe { std::mem::transmute::<usize, QueryFn>(real)(bink) }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, AtomicUsize};

    use super::*;

    #[test]
    fn values() {
        for off in ["", "0", " 0 "] {
            assert_eq!(Strategy::parse(off), Ok(None));
        }
        assert_eq!(Strategy::parse("open"), Ok(Some(Strategy::Open)));
        assert_eq!(Strategy::parse("End"), Ok(Some(Strategy::End)));
        assert_eq!(Strategy::parse("fast\n"), Ok(Some(Strategy::Fast)));
        for unknown in ["1", "yes", "skip", "end,fast"] {
            assert_eq!(Strategy::parse(unknown), Err(()));
        }
    }

    fn exports(missing: &'static str) -> impl Fn(&str) -> Option<usize> {
        move |name: &str| (name != missing).then_some(0x1000 + name.len())
    }

    #[test]
    fn each_strategy_replaces_only_its_imports() {
        for strategy in [Strategy::Open, Strategy::End, Strategy::Fast] {
            assert!(Skip::find(strategy, &exports("")).is_some());
            for name in [
                "BinkClose",
                "BinkDoFrame",
                "BinkGoto",
                "BinkPause",
                "BinkSetSoundSystem",
            ] {
                assert_eq!(stub(strategy, name), None, "{name}");
            }
            for name in strategy.replaces() {
                assert!(stub(strategy, name).is_some(), "{name}");
            }
        }
        assert_eq!(stub(Strategy::Open, "BinkNextFrame"), None);
        assert_eq!(stub(Strategy::End, "BinkWait"), None);
        assert_eq!(stub(Strategy::Fast, "BinkNextFrame"), None);
        // A DLL without the seek cannot do `end`; `open` needs nothing.
        assert!(Skip::find(Strategy::End, &exports("BinkGoto")).is_none());
        assert!(Skip::find(Strategy::Open, &exports("BinkOpenWithOptions")).is_some());
    }

    /// A stand-in `BINK`: Width, Height, Frames, FrameNum, LastFrameNum.
    fn movie(frames: u32) -> Box<[u32; 8]> {
        Box::new([1920, 1080, frames, 1, 1, 30, 1, 0])
    }

    static OPENED: AtomicUsize = AtomicUsize::new(0);
    static GOTO: AtomicU32 = AtomicU32::new(0);
    static GOTO_FLAGS: AtomicU32 = AtomicU32::new(u32::MAX);
    static NEXT: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn fake_open(name: *const c_void, _: *const c_void, _: u32) -> Hbink {
        OPENED.fetch_add(1, Ordering::SeqCst);
        name as Hbink
    }
    unsafe extern "system" fn fake_next(_: Hbink) {
        NEXT.fetch_add(1, Ordering::SeqCst);
    }
    unsafe extern "system" fn fake_goto(_: Hbink, frame: u32, flags: i32) {
        GOTO.store(frame, Ordering::SeqCst);
        GOTO_FLAGS.store(flags as u32, Ordering::SeqCst);
    }
    unsafe extern "system" fn fake_query(_: Hbink) -> i32 {
        7
    }

    fn fakes(strategy: Strategy) -> Skip {
        Skip {
            strategy,
            open: fake_open as OpenFn as usize,
            next_frame: fake_next as NextFrameFn as usize,
            goto: fake_goto as GotoFn as usize,
            wait: fake_query as QueryFn as usize,
            should_skip: fake_query as QueryFn as usize,
        }
    }

    /// One test drives the shared fakes, so the counters are its own.
    #[test]
    fn strategies_call_by_call() {
        let null = std::ptr::null();
        // `open`: fails without asking the DLL.
        let open = fakes(Strategy::Open);
        let mut one = movie(300);
        let handle = one.as_mut_ptr().cast::<c_void>();
        assert!(unsafe { open_with(&open, handle, null, 0) }.is_null());
        assert_eq!(OPENED.load(Ordering::SeqCst), 0);

        // `end`: the first next frame seeks to the last one, later ones pass.
        let end = fakes(Strategy::End);
        assert_eq!(unsafe { open_with(&end, handle, null, 0) }, handle);
        assert_eq!(OPENED.load(Ordering::SeqCst), 1);
        unsafe { next_frame_with(&end, handle) };
        assert_eq!(
            (GOTO.load(Ordering::SeqCst), NEXT.load(Ordering::SeqCst)),
            (300, 0)
        );
        assert_eq!(GOTO_FLAGS.load(Ordering::SeqCst), 0);
        unsafe { next_frame_with(&end, handle) };
        assert_eq!(NEXT.load(Ordering::SeqCst), 1);
        // A movie nobody opened through the stub is left alone.
        let mut other = movie(50);
        unsafe { next_frame_with(&end, other.as_mut_ptr().cast()) };
        assert_eq!(
            (GOTO.load(Ordering::SeqCst), NEXT.load(Ordering::SeqCst)),
            (300, 2)
        );

        // `fast`: ready and behind until the last frame is decoded.
        let fast = fakes(Strategy::Fast);
        let mut two = movie(3);
        let handle = two.as_mut_ptr().cast::<c_void>();
        assert_eq!(unsafe { open_with(&fast, handle, null, 0) }, handle);
        assert_eq!(
            unsafe { query_with(&fast, handle, "BinkWait", 0, fast.wait) },
            0
        );
        assert_eq!(
            unsafe { query_with(&fast, handle, "BinkShouldSkip", 1, fast.should_skip) },
            1
        );
        two[LAST_FRAME_NUM] = 3;
        assert_eq!(
            unsafe { query_with(&fast, handle, "BinkShouldSkip", 1, fast.should_skip) },
            7
        );
        two[LAST_FRAME_NUM] = 1;
        assert_eq!(
            unsafe { query_with(&fast, handle, "BinkWait", 0, fast.wait) },
            7
        );

        // `fast` on a movie that never reaches its count stops answering.
        let mut three = movie(4);
        let handle = three.as_mut_ptr().cast::<c_void>();
        unsafe { open_with(&fast, handle, null, 0) };
        let forced = (0..100)
            .filter(|_| unsafe { query_with(&fast, handle, "BinkShouldSkip", 1, fast.should_skip) } == 1)
            .count();
        assert_eq!(forced, 25);
    }
}
