//! `VICTORY_CONSTANTS_WATCH=<shader hash>,<offset>[,<offset>=<f32>]...[,size=<bytes>]`
//! (diagnostics builds, off unless set): one constant buffer of the draws of
//! a pipeline whose name has the hash in it, picked by its contents (each
//! `<offset>=<f32>` must hold there, and `size=` its size, when the draw is
//! recorded), and the f32 at `<offset>` of it followed.
//!
//! - A time series: the value copied to the list's upload memory (when the
//!   list is submitted) is logged with the frame each time it differs from
//!   the one copied before.
//! - Within the frame only: the same title address is read again when that
//!   submission's lists have been sent, and again at the first present
//!   after; a value other than the one copied is logged then, as written
//!   after our copy. After that present the address is let go: the title's
//!   per-frame memory is used again by later frames, which says nothing.
//!
//! Rendering is not changed: the watch only reads title memory. Not said:
//! when the title last wrote the bytes (the write watch tells pages, not
//! values).

use std::cell::Cell;
use std::sync::{Mutex, OnceLock};

/// The most lines logged.
const LINES: usize = 400;
/// The most title addresses waiting for their submission and present.
const PENDING: usize = 64;

/// What is watched.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Watch {
    /// As text in the pipeline's name.
    pub hash: String,
    /// The byte offset of the f32 followed.
    pub offset: u32,
    /// Byte offsets and the f32 bits that must be there.
    pub fields: Vec<(u32, u32)>,
    /// The constant buffer's size in bytes, if it must be one.
    pub size: Option<u32>,
}

fn number(s: &str) -> Option<u32> {
    let s = s.trim();
    match s.strip_prefix("0x") {
        Some(x) => u32::from_str_radix(x, 16).ok(),
        None => s.parse().ok(),
    }
}

/// From `<hash>,<offset>[,<offset>=<f32>]...[,size=<bytes>]` (offsets `0x..`
/// or decimal, multiples of 4).
pub(crate) fn parse(s: &str) -> Option<Watch> {
    let mut parts = s.split(',');
    let hash = parts.next()?.trim().to_ascii_lowercase();
    let offset = number(parts.next()?)?;
    if hash.is_empty() || offset % 4 != 0 {
        return None;
    }
    let mut w = Watch { hash, offset, fields: Vec::new(), size: None };
    for p in parts {
        let (k, v) = p.split_once('=')?;
        if k.trim() == "size" {
            w.size = Some(number(v)?);
        } else {
            let at = number(k)?;
            if at % 4 != 0 {
                return None;
            }
            w.fields.push((at, v.trim().parse::<f32>().ok()?.to_bits()));
        }
    }
    Some(w)
}

impl Watch {
    /// Whether a constant buffer of `size` bytes whose dwords `read` gives
    /// (by byte offset) is the one watched.
    pub(crate) fn matches(&self, size: u32, read: impl Fn(u32) -> u32) -> bool {
        self.offset + 4 <= size
            && self.size.is_none_or(|s| s == size)
            && self.fields.iter().all(|&(at, bits)| at + 4 <= size && read(at) == bits)
    }
}

fn watch() -> Option<&'static Watch> {
    static W: OnceLock<Option<Watch>> = OnceLock::new();
    W.get_or_init(|| {
        let w = parse(&std::env::var("VICTORY_CONSTANTS_WATCH").ok()?);
        match &w {
            Some(w) => tracing::info!("constants watch: {w:?} (VICTORY_CONSTANTS_WATCH)"),
            None => tracing::warn!(
                "VICTORY_CONSTANTS_WATCH wants <shader hash>,<offset>[,<offset>=<f32>]...[,size=<bytes>]: off"
            ),
        }
        w
    })
    .as_ref()
}

#[derive(Debug)]
struct Pending {
    /// Title address of the f32.
    addr: usize,
    /// The list (its address) and the frame it was recorded in.
    list: usize,
    frame: u64,
    /// The value copied (none before the list's submission).
    copied: Option<u32>,
    sent: bool,
}

/// The watched buffer's copies waiting for their submission and present,
/// and the series of values copied.
#[derive(Default)]
pub(crate) struct Tracker {
    pending: Vec<Pending>,
    last: Option<u32>,
    lines: usize,
}

impl Tracker {
    fn line(&mut self, out: &mut Vec<String>, s: String) {
        if self.lines < LINES {
            self.lines += 1;
            out.push(s);
        }
    }

    /// A draw recorded into `list` in frame `frame` takes the watched buffer,
    /// its f32 at title address `addr`.
    pub(crate) fn recorded(&mut self, addr: usize, list: usize, frame: u64) {
        if self.pending.len() < PENDING && !self.pending.iter().any(|p| p.addr == addr && p.list == list) {
            self.pending.push(Pending { addr, list, frame, copied: None, sent: false });
        }
    }

    /// `list` is submitted: its copies are made now (`read` gives what they
    /// copy). A line for each change of the series.
    pub(crate) fn filling(&mut self, list: usize, read: impl Fn(usize) -> u32) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.pending.len() {
            if self.pending[i].list != list || self.pending[i].copied.is_some() {
                continue;
            }
            let bits = read(self.pending[i].addr);
            self.pending[i].copied = Some(bits);
            if self.last != Some(bits) {
                let (addr, frame) = (self.pending[i].addr, self.pending[i].frame);
                let was = self.last.map_or("none".into(), |b| f32::from_bits(b).to_string());
                self.last = Some(bits);
                self.line(
                    &mut out,
                    format!(
                        "constants watch: series: frame {frame}: {} copied (was {was}) at {addr:#x}, list {list:#x}",
                        f32::from_bits(bits)
                    ),
                );
            }
        }
        out
    }

    /// `list`'s submission was sent: its copies' bytes are read again.
    pub(crate) fn sent(&mut self, list: usize, read: impl Fn(usize) -> u32) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.pending.len() {
            let p = &self.pending[i];
            let (Some(copied), false) = (p.copied, p.sent) else { continue };
            if p.list != list {
                continue;
            }
            let (addr, frame) = (p.addr, p.frame);
            self.pending[i].sent = true;
            let now = read(addr);
            if now != copied {
                self.line(
                    &mut out,
                    format!(
                        "constants watch: {addr:#x} holds {} when its submission was sent, copied with {} (frame {frame}, list {list:#x}): written after our copy",
                        f32::from_bits(now),
                        f32::from_bits(copied)
                    ),
                );
            }
        }
        out
    }

    /// A present: the copies whose submission was sent are read a last time
    /// and let go.
    pub(crate) fn presented(&mut self, frame: u64, read: impl Fn(usize) -> u32) -> Vec<String> {
        let mut out = Vec::new();
        let (done, rest): (Vec<Pending>, Vec<Pending>) =
            std::mem::take(&mut self.pending).into_iter().partition(|p| p.sent);
        self.pending = rest;
        for p in done {
            let copied = p.copied.unwrap_or_default();
            let now = read(p.addr);
            if now != copied {
                self.line(
                    &mut out,
                    format!(
                        "constants watch: {:#x} holds {} at present {frame}, copied with {} (frame {}, list {:#x}): written after our copy",
                        p.addr,
                        f32::from_bits(now),
                        f32::from_bits(copied),
                        p.frame,
                        p.list
                    ),
                );
            }
        }
        out
    }
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

thread_local! {
    /// Whether the draw being recorded on this thread is of the hash.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

fn with<T>(f: impl FnOnce(&mut Tracker) -> T) -> T {
    f(TRACKER.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Tracker::default))
}

fn log(lines: Vec<String>) {
    for line in lines {
        tracing::info!("{line}");
    }
}

/// A draw's pipeline, by name (the runtime's, with its shader hashes).
pub(crate) fn pipeline(name: &str) {
    if let Some(w) = watch() {
        ACTIVE.set(name.to_ascii_lowercase().contains(&w.hash));
    }
}

fn read(addr: usize) -> u32 {
    // SAFETY: title memory a constants fill copies from (mapped while the
    // title holds its constant buffer there); an unaligned read of 4 bytes.
    unsafe { std::ptr::read_unaligned(addr as *const u32) }
}

/// A table constant buffer of `size` bytes at title `address` is to be
/// copied for the draw being recorded, into list `list`.
pub(crate) fn fill(address: u64, size: u32, list: usize) {
    let Some(w) = watch() else { return };
    if !ACTIVE.get() || !w.matches(size, |at| read(address as usize + at as usize)) {
        return;
    }
    let frame = crate::frame::current();
    with(|t| t.recorded(address as usize + w.offset as usize, list, frame));
}

/// `list` is being submitted: its upload memory is filled now.
pub(crate) fn filling(list: usize) {
    if watch().is_some() {
        log(with(|t| t.filling(list, read)));
    }
}

/// `list`'s submission was sent.
pub(crate) fn sent(list: usize) {
    if watch().is_some() {
        log(with(|t| t.sent(list, read)));
    }
}

/// A frame was presented.
pub(crate) fn presented() {
    if watch().is_some() {
        log(with(|t| t.presented(crate::frame::current(), read)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "909D7B7AF414D9E0,0x18,0x6c=1,0x68=-0.3,0x80=3,0x84=-1";

    #[test]
    fn watches_are_read() {
        let w = parse(PHONE).unwrap();
        assert_eq!(w.hash, "909d7b7af414d9e0");
        assert_eq!(w.offset, 0x18);
        assert_eq!(
            w.fields,
            vec![(0x6c, 1f32.to_bits()), (0x68, (-0.3f32).to_bits()), (0x80, 3f32.to_bits()), (0x84, (-1f32).to_bits())]
        );
        assert_eq!(parse("909d,24,size=0x200").unwrap().size, Some(0x200));
        for bad in ["", "909d", ",0x18", "909d,0x1a", "909d,x", "909d,0x18,0x6c", "909d,0x18,0x6e=1", "909d,0x18,0x6c=one"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    /// A constant buffer as dwords by byte offset.
    fn buffer(values: &[(u32, f32)]) -> impl Fn(u32) -> u32 + '_ {
        move |at| values.iter().find(|v| v.0 == at).map_or(0, |v| v.1.to_bits())
    }

    #[test]
    fn the_material_is_picked_by_its_fields() {
        let w = parse(PHONE).unwrap();
        let phone = [(0x18, 1.0), (0x68, -0.3), (0x6c, 1.0), (0x80, 3.0), (0x84, -1.0)];
        assert!(w.matches(0x200, buffer(&phone)));
        // A light of the same shader: 472 and other fields.
        let light = [(0x18, 472.0), (0x68, -0.3), (0x6c, 1.5), (0x80, -1.0), (0x84, 1.0)];
        assert!(!w.matches(0x200, buffer(&light)));
        // Fields past the end do not match; nor another size when one is asked.
        assert!(!w.matches(0x80, buffer(&phone)));
        let sized = parse("909d,0x18,size=0x1e0").unwrap();
        assert!(!sized.matches(0x200, buffer(&phone)));
        assert!(sized.matches(0x1e0, buffer(&phone)));
    }

    #[test]
    fn changes_count_only_within_the_frame() {
        let mut t = Tracker::default();
        let value = std::cell::Cell::new(1f32.to_bits());
        let read = |_: usize| value.get();
        t.recorded(0x10, 7, 100);
        let series = t.filling(7, read);
        assert_eq!(series.len(), 1);
        assert!(series[0].contains("frame 100: 1 copied (was none)"), "{}", series[0]);
        // Unchanged when sent and at the present: nothing; then let go.
        assert!(t.sent(7, read).is_empty());
        assert!(t.presented(101, read).is_empty());
        // The title's memory used again by a later frame: not reported.
        value.set(7.5f32.to_bits());
        assert!(t.presented(102, read).is_empty());
        // The next frame copies 7.5 there: a series line, no change line.
        t.recorded(0x10, 8, 103);
        assert_eq!(t.filling(8, read).len(), 1);
        // Written after the copy, before the submission was sent: reported.
        value.set(472f32.to_bits());
        let lines = t.sent(8, read);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("holds 472 when its submission was sent, copied with 7.5"), "{}", lines[0]);
    }

    #[test]
    fn a_write_before_the_first_present_is_reported_once() {
        let mut t = Tracker::default();
        let value = std::cell::Cell::new(1f32.to_bits());
        let read = |_: usize| value.get();
        t.recorded(0x10, 7, 100);
        t.filling(7, read);
        t.sent(7, read);
        value.set(472f32.to_bits());
        let lines = t.presented(101, read);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("holds 472 at present 101, copied with 1"), "{}", lines[0]);
        assert!(t.presented(102, read).is_empty());
        // A list not yet submitted waits through a present.
        t.recorded(0x20, 9, 102);
        assert!(t.presented(103, read).is_empty());
        assert_eq!(t.pending.len(), 1);
    }

    #[test]
    fn the_same_value_again_is_no_series_line() {
        let mut t = Tracker::default();
        let read = |_: usize| 1f32.to_bits();
        for list in 0..5usize {
            t.recorded(0x10 + list * 0x100, list, 100 + list as u64);
            let n = t.filling(list, read).len();
            assert_eq!(n, usize::from(list == 0));
            t.sent(list, read);
            t.presented(101 + list as u64, read);
        }
    }
}
