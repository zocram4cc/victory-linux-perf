//! Skipping the title's Bink movies (`VICTORY_SKIP_BINK`). The title's own
//! Bink DLL stays loaded and opens every movie as before, but a movie that is
//! skipped is put on its last frame before its first one is shown and no
//! frame of it is decoded, so the title's own end-of-movie check ends it at
//! once. Off unless asked for: `1` skips the movies that do not loop (logos,
//! intros), `all` those that loop too (menu backgrounds, which then stay
//! dark).
//!
//! Only the exports a player loop calls on an open movie are wrapped; they
//! all take the movie first and at most three integer or pointer arguments,
//! so a wrapper passes all four argument registers on unchanged. Opening,
//! frame buffers, sound and the decoding threads go straight to the DLL.
//! Each movie is looked at once, on the first of these calls for it, when
//! its looping flag is final (`BinkSetWillLoop` may change it after opening).
//!
//! Imports bound at load are wrapped ([`SkipMovies`]); a Bink export looked up
//! with `GetProcAddress` is not.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use victory_loader::{ImportResolver, Provenance, Resolution};
use victory_pe::ImportSymbol;

/// Which movies are skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Mode {
    Off,
    /// Movies that play once.
    NotLooping,
    All,
}

fn parse_mode(value: Option<&OsStr>) -> Mode {
    let Some(value) = value.and_then(OsStr::to_str) else {
        return Mode::Off;
    };
    match value.trim() {
        "" | "0" => Mode::Off,
        "1" => Mode::NotLooping,
        v if v.eq_ignore_ascii_case("all") => Mode::All,
        v => {
            tracing::warn!(
                value = v,
                "VICTORY_SKIP_BINK: expected 1 or all; not skipping"
            );
            Mode::Off
        }
    }
}

/// The start of the player's movie structure (`HBINK` points at it).
#[repr(C)]
#[allow(dead_code)] // the layout; not every field is read
struct Movie {
    width: u32,
    height: u32,
    frames: u32,
    /// The frame shown now, from 1.
    frame_num: u32,
    last_frame_num: u32,
    frame_rate: u32,
    frame_rate_div: u32,
    read_error: u32,
    open_flags: u32,
}
const _: () = assert!(std::mem::offset_of!(Movie, open_flags) == 0x20);

/// `open_flags`: the movie starts over after its last frame.
const WILL_LOOP: u32 = 0x80;

/// The wrapped exports, in the order of [`WRAPPERS`].
const NAMES: [&str; 9] = [
    "BinkClose",
    "BinkWait",
    "BinkShouldSkip",
    "BinkDoFrame",
    "BinkDoFrameAsync",
    "BinkDoFrameAsyncMulti",
    "BinkDoFrameAsyncWait",
    "BinkNextFrame",
    "BinkGoto",
];
const CLOSE: usize = 0;

type Export = extern "system" fn(*mut Movie, usize, usize, usize) -> usize;

const WRAPPERS: [Export; 9] = [
    close,
    wait,
    should_skip,
    do_frame,
    do_frame_async,
    do_frame_async_multi,
    do_frame_async_wait,
    next_frame,
    goto,
];

/// The DLL's own address of each wrapped export (0 until bound).
static ORIGINALS: [AtomicUsize; 9] = [const { AtomicUsize::new(0) }; 9];
/// The [`Mode`] the wrappers were bound with.
static MODE: AtomicU8 = AtomicU8::new(Mode::Off as u8);
/// Open movies seen so far, and whether each is skipped.
static MOVIES: Mutex<Option<HashMap<usize, bool>>> = Mutex::new(None);

/// The address an import of `dll!name` found at `address` is bound to:
/// a wrapper when movies are skipped and it is a wrapped Bink export.
pub fn wrap(dll: &str, name: &str, address: usize) -> usize {
    static FROM_ENV: OnceLock<Mode> = OnceLock::new();
    let mode = *FROM_ENV.get_or_init(|| {
        let mode = parse_mode(std::env::var_os("VICTORY_SKIP_BINK").as_deref());
        if mode != Mode::Off {
            tracing::info!(?mode, "Bink movies are skipped");
        }
        mode
    });
    wrap_with(mode, dll, name, address)
}

fn wrap_with(mode: Mode, dll: &str, name: &str, address: usize) -> usize {
    let is_bink = dll
        .get(..4)
        .is_some_and(|start| start.eq_ignore_ascii_case("bink"));
    if mode == Mode::Off || !is_bink || address == 0 {
        return address;
    }
    let Some(i) = NAMES.iter().position(|&n| n == name) else {
        return address;
    };
    // One Bink DLL: the wrappers call the first one's exports.
    let first = ORIGINALS[i].compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire);
    if first.is_err_and(|bound| bound != address) {
        tracing::warn!(dll, name, "a second Bink DLL: its movies are not skipped");
        return address;
    }
    MODE.store(mode as u8, Ordering::Relaxed);
    WRAPPERS[i] as usize
}

/// Calls the DLL's own export `i`.
fn forward(i: usize, movie: *mut Movie, a: usize, b: usize, c: usize) -> usize {
    let address = ORIGINALS[i].load(Ordering::Acquire);
    // SAFETY: only a wrapper calls this, and a wrapper is only bound once
    // its original is stored; every wrapped export has this shape (see the
    // module comment).
    let original: Export = unsafe { std::mem::transmute::<usize, Export>(address) };
    original(movie, a, b, c)
}

fn movies() -> std::sync::MutexGuard<'static, Option<HashMap<usize, bool>>> {
    MOVIES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `movie` is skipped, deciding it the first time; a skipped movie
/// is (kept) on its last frame.
fn skipped(movie: *mut Movie) -> bool {
    if movie.is_null() {
        return false;
    }
    let mut movies = movies();
    let movies = movies.get_or_insert_with(HashMap::new);
    let skip = *movies.entry(movie as usize).or_insert_with(|| {
        // SAFETY: the title passes the handle the DLL opened; it points at
        // the movie structure, which starts with `Movie`.
        let m = unsafe { &*movie };
        let looping = m.open_flags & WILL_LOOP != 0;
        let skip = m.frames > 0 && (!looping || MODE.load(Ordering::Relaxed) == Mode::All as u8);
        tracing::info!(
            width = m.width,
            height = m.height,
            frames = m.frames,
            looping,
            skip,
            "Bink movie"
        );
        skip
    });
    if skip {
        // SAFETY: as above; the title reads these fields, the DLL does not
        // run for this movie any more (except to close it).
        unsafe {
            let frames = (*movie).frames;
            (*movie).frame_num = frames;
            (*movie).last_frame_num = frames;
        }
    }
    skip
}

extern "system" fn close(movie: *mut Movie, a: usize, b: usize, c: usize) -> usize {
    if let Some(movies) = movies().as_mut() {
        movies.remove(&(movie as usize));
    }
    forward(CLOSE, movie, a, b, c)
}

/// Generates the wrapper of export `$i` (in [`NAMES`]): it answers
/// `$skipped` for a skipped movie and calls the DLL for any other.
macro_rules! wrapper {
    ($name:ident, $i:literal, $skipped:expr) => {
        extern "system" fn $name(movie: *mut Movie, a: usize, b: usize, c: usize) -> usize {
            if skipped(movie) {
                return $skipped;
            }
            forward($i, movie, a, b, c)
        }
    };
}

// No waiting for the frame's time; nothing to skip; decoding done at once
// (a started asynchronous decode is reported started and finished); no
// advancing or seeking off the last frame.
wrapper!(wait, 1, 0);
wrapper!(should_skip, 2, 0);
wrapper!(do_frame, 3, 0);
wrapper!(do_frame_async, 4, 1);
wrapper!(do_frame_async_multi, 5, 1);
wrapper!(do_frame_async_wait, 6, 1);
wrapper!(next_frame, 7, 0);
wrapper!(goto, 8, 0);

/// Resolves through `inner`, binding the title's Bink exports to the
/// wrappers above when `VICTORY_SKIP_BINK` asks for it.
pub struct SkipMovies<'a> {
    pub inner: &'a dyn ImportResolver,
}

impl ImportResolver for SkipMovies<'_> {
    fn resolve(&self, dll: &str, symbol: &ImportSymbol) -> Resolution {
        match (self.inner.resolve(dll, symbol), symbol) {
            (
                Resolution::Found {
                    address,
                    provenance: Provenance::Title,
                },
                ImportSymbol::Name { name, .. },
            ) => Resolution::Found {
                address: wrap(dll, name, address),
                provenance: Provenance::Title,
            },
            (found, _) => found,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_values() {
        let mode = |v: &str| parse_mode(Some(OsStr::new(v)));
        assert_eq!(parse_mode(None), Mode::Off);
        assert_eq!(mode(""), Mode::Off);
        assert_eq!(mode("0"), Mode::Off);
        assert_eq!(mode("1"), Mode::NotLooping);
        assert_eq!(mode("all"), Mode::All);
        assert_eq!(mode(" ALL "), Mode::All);
        assert_eq!(mode("yes"), Mode::Off);
    }

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static CLOSED: AtomicUsize = AtomicUsize::new(0);

    extern "system" fn dll_wait(_: *mut Movie, _: usize, _: usize, _: usize) -> usize {
        CALLS.fetch_add(1, Ordering::Relaxed);
        7
    }
    extern "system" fn dll_next(movie: *mut Movie, _: usize, _: usize, _: usize) -> usize {
        CALLS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the test's own movie.
        unsafe { (*movie).frame_num += 1 };
        0
    }
    extern "system" fn dll_close(_: *mut Movie, _: usize, _: usize, _: usize) -> usize {
        CLOSED.fetch_add(1, Ordering::Relaxed);
        0
    }

    fn movie(frames: u32, open_flags: u32) -> Box<Movie> {
        Box::new(Movie {
            width: 1920,
            height: 1080,
            frames,
            frame_num: 1,
            last_frame_num: 0,
            frame_rate: 30,
            frame_rate_div: 1,
            read_error: 0,
            open_flags,
        })
    }

    // The wrappers' state is global, so all of it in one test.
    #[test]
    fn skips_movies_and_passes_the_rest_through() {
        let bind = |mode, dll: &str, name: &str, f: Export| -> Export {
            let address = wrap_with(mode, dll, name, f as usize);
            // SAFETY: either `f` or a wrapper of the same type.
            unsafe { std::mem::transmute::<usize, Export>(address) }
        };
        let dll = "Bink2Scarlett.dll";
        // Off, other DLLs and exports that are not wrapped stay as they are.
        assert_eq!(wrap_with(Mode::Off, dll, "BinkWait", 0x1000), 0x1000);
        assert_eq!(wrap_with(Mode::All, "game.dll", "BinkWait", 0x1000), 0x1000);
        assert_eq!(wrap_with(Mode::All, dll, "BinkGetRects", 0x1000), 0x1000);

        let wait = bind(Mode::NotLooping, dll, "BinkWait", dll_wait);
        let next = bind(Mode::NotLooping, dll, "BinkNextFrame", dll_next);
        let close = bind(Mode::NotLooping, dll, "BinkClose", dll_close);
        let do_async = bind(Mode::NotLooping, dll, "BinkDoFrameAsyncMulti", dll_wait);
        assert_ne!(wait as usize, dll_wait as *const () as usize);
        // A second Bink DLL keeps its own exports.
        assert_eq!(
            wrap_with(Mode::NotLooping, "bink2other.dll", "BinkWait", 0x2000),
            0x2000
        );

        // A movie that plays once is on its last frame at its first wait,
        // and stays there; the DLL is not called until it is closed.
        let mut intro = movie(300, 0);
        let intro_ptr: *mut Movie = &mut *intro;
        assert_eq!(wait(intro_ptr, 0, 0, 0), 0);
        assert_eq!((intro.frame_num, intro.last_frame_num), (300, 300));
        assert_eq!(do_async(intro_ptr, 0, 0, 0), 1);
        assert_eq!(next(intro_ptr, 0, 0, 0), 0);
        assert_eq!(intro.frame_num, 300);
        assert_eq!(CALLS.load(Ordering::Relaxed), 0);

        // A looping one plays.
        let mut menu = movie(120, WILL_LOOP);
        let menu_ptr: *mut Movie = &mut *menu;
        assert_eq!(wait(menu_ptr, 0, 0, 0), 7);
        next(menu_ptr, 0, 0, 0);
        assert_eq!(menu.frame_num, 2);
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);

        close(intro_ptr, 0, 0, 0);
        close(menu_ptr, 0, 0, 0);
        assert_eq!(CLOSED.load(Ordering::Relaxed), 2);
        assert!(movies().as_ref().unwrap().is_empty());

        // With `all`, looping movies are skipped too (decided per movie,
        // when it is first seen).
        bind(Mode::All, dll, "BinkWait", dll_wait);
        let mut menu = movie(120, WILL_LOOP);
        let menu_ptr: *mut Movie = &mut *menu;
        assert_eq!(wait(menu_ptr, 0, 0, 0), 0);
        assert_eq!(menu.frame_num, 120);
        close(menu_ptr, 0, 0, 0);
        // A null handle (a failed open) goes to the DLL.
        assert_eq!(wait(std::ptr::null_mut(), 0, 0, 0), 7);
    }
}
