//! `VICTORY_CONSTANTS_WATCH=<shader hash>[,<offset>]` (diagnostics builds,
//! off unless set): the constant buffers that draws of a pipeline whose name
//! has the hash in it take from their descriptor tables. Each time one is
//! copied to the list's upload memory (when the list is submitted), the
//! title address of its source and the f32 at the offset (default 0x18) are
//! logged; at each present the same title addresses are read again, and a
//! value that differs from the one last logged is logged with both. For
//! telling a value the title writes after our copy from one it holds.
//! Rendering is not changed: the watch only reads title memory.
//!
//! Rate: the first [`ADDRESSES`] addresses are followed; a fill of one of
//! them is logged when its value differs from the one last logged; at most
//! [`LINES`] lines in all.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The most title addresses followed.
const ADDRESSES: usize = 16;
/// The most lines logged.
const LINES: usize = 200;

/// What is watched: the hash (as text in the pipeline's name) and the byte
/// offset of the f32 in the constant buffer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Watch {
    pub hash: String,
    pub offset: u32,
}

/// From `<hash>[,<offset>]`, the offset `0x..` or decimal.
pub(crate) fn parse(s: &str) -> Option<Watch> {
    let (hash, offset) = match s.split_once(',') {
        Some((h, o)) => {
            let o = o.trim();
            let o = match o.strip_prefix("0x") {
                Some(x) => u32::from_str_radix(x, 16).ok()?,
                None => o.parse().ok()?,
            };
            (h, o)
        }
        None => (s, 0x18),
    };
    let hash = hash.trim().to_ascii_lowercase();
    (!hash.is_empty() && offset % 4 == 0).then_some(Watch { hash, offset })
}

fn watch() -> Option<&'static Watch> {
    static W: OnceLock<Option<Watch>> = OnceLock::new();
    W.get_or_init(|| {
        let w = parse(&std::env::var("VICTORY_CONSTANTS_WATCH").ok()?);
        match &w {
            Some(w) => tracing::info!(
                "constants watch: pipelines with {} in their name, the f32 at {:#x} of their table constant buffers (VICTORY_CONSTANTS_WATCH)",
                w.hash,
                w.offset
            ),
            None => tracing::warn!("VICTORY_CONSTANTS_WATCH wants <shader hash>[,<offset>, a multiple of 4]: off"),
        }
        w
    })
    .as_ref()
}

/// The addresses followed and what was last logged of them.
#[derive(Default)]
pub(crate) struct Tracker {
    last: HashMap<usize, u32>,
    order: Vec<usize>,
    lines: usize,
}

impl Tracker {
    /// A fill of `addr` with `bits` (in a draw at `place`): the line to log.
    pub(crate) fn filled(&mut self, addr: usize, bits: u32, place: &str) -> Option<String> {
        if self.lines >= LINES {
            return None;
        }
        let line = match self.last.get(&addr) {
            Some(&b) if b == bits => return None,
            Some(&b) => format!(
                "constants watch: {addr:#x} copied with {} (was {} at the last line) {place}",
                f32::from_bits(bits),
                f32::from_bits(b)
            ),
            None if self.order.len() >= ADDRESSES => return None,
            None => {
                self.order.push(addr);
                format!("constants watch: {addr:#x} copied with {} {place}", f32::from_bits(bits))
            }
        };
        self.last.insert(addr, bits);
        self.lines += 1;
        Some(line)
    }

    /// At a present: the addresses followed, and for each that `read` gives
    /// another value than the one last logged, the line to log.
    pub(crate) fn presented(&mut self, frame: u64, read: impl Fn(usize) -> u32) -> Vec<String> {
        let mut out = Vec::new();
        for &addr in &self.order {
            if self.lines >= LINES {
                break;
            }
            let now = read(addr);
            let was = self.last[&addr];
            if now != was {
                out.push(format!(
                    "constants watch: {addr:#x} holds {} at present {frame}, copied with {} (the title wrote it after our copy)",
                    f32::from_bits(now),
                    f32::from_bits(was)
                ));
                self.last.insert(addr, now);
                self.lines += 1;
            }
        }
        out
    }
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

thread_local! {
    /// Whether the draw being recorded on this thread is watched.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// A draw's pipeline, by name (the runtime's, with its shader hashes).
pub(crate) fn pipeline(name: &str) {
    if let Some(w) = watch() {
        ACTIVE.set(name.to_ascii_lowercase().contains(&w.hash));
    }
}

fn read(addr: usize) -> u32 {
    // SAFETY: title memory the fill copies from (mapped while the title
    // holds its constant buffer there); an unaligned read of four bytes.
    unsafe { std::ptr::read_unaligned(addr as *const u32) }
}

/// A table constant buffer of `size` bytes at title `address` is to be
/// copied for the draw being recorded, into list `list`.
pub(crate) fn fill(address: u64, size: u32, list: usize) {
    let Some(w) = watch() else { return };
    if !ACTIVE.get() || w.offset + 4 > size {
        return;
    }
    let addr = address as usize + w.offset as usize;
    let place = format!("(list {list:#x}, frame {}, buffer {address:#x}+{size:#x})", crate::frame::current());
    let line = TRACKER.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Tracker::default).filled(
        addr,
        read(addr),
        &place,
    );
    if let Some(line) = line {
        tracing::info!("{line}");
    }
}

/// A frame was presented.
pub(crate) fn presented() {
    if watch().is_none() {
        return;
    }
    let lines = match TRACKER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        Some(t) => t.presented(crate::frame::current(), read),
        None => return,
    };
    for line in lines {
        tracing::info!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watches_are_read() {
        assert_eq!(parse("909D7B7AF414D9E0"), Some(Watch { hash: "909d7b7af414d9e0".into(), offset: 0x18 }));
        assert_eq!(parse("909d,0x1c"), Some(Watch { hash: "909d".into(), offset: 0x1c }));
        assert_eq!(parse("909d,32"), Some(Watch { hash: "909d".into(), offset: 32 }));
        for bad in ["", ",0x18", "909d,0x1a", "909d,x"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn fills_are_logged_when_new_or_changed() {
        let mut t = Tracker::default();
        let one = 1f32.to_bits();
        assert!(t.filled(0x100, one, "a").is_some());
        assert!(t.filled(0x100, one, "b").is_none());
        let line = t.filled(0x100, 472f32.to_bits(), "c").unwrap();
        assert!(line.contains("472") && line.contains("was 1"), "{line}");
        // No more than ADDRESSES addresses are followed.
        for k in 1..ADDRESSES {
            assert!(t.filled(0x100 + k * 16, one, "d").is_some());
        }
        assert!(t.filled(0x1_0000, one, "e").is_none());
    }

    #[test]
    fn presents_say_what_changed_after_the_copy() {
        let mut t = Tracker::default();
        t.filled(0x10, 1f32.to_bits(), "a");
        t.filled(0x20, 2f32.to_bits(), "a");
        let read = |a: usize| if a == 0x10 { 472f32.to_bits() } else { 2f32.to_bits() };
        let lines = t.presented(7, read);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("0x10") && lines[0].contains("472") && lines[0].contains("copied with 1"));
        // Logged once: the new value is what is held now.
        assert!(t.presented(8, read).is_empty());
    }

    #[test]
    fn lines_stop_at_the_limit() {
        let mut t = Tracker::default();
        let mut n = 0;
        for k in 0..LINES * 2 {
            if t.filled(0x10, (k as f32).to_bits(), "a").is_some() {
                n += 1;
            }
        }
        assert_eq!(n, LINES);
        assert!(t.presented(1, |_| 0).is_empty());
    }
}
