//! `VICTORY_CONSTANTS_WATCH=<shader hash or *>,<offset>[,<offset>=<f32>]...[,size=<bytes>]`
//! (diagnostics builds, off unless set): one constant buffer, picked by its
//! contents where it is copied (each `<offset>=<f32>` must hold there, and
//! `size=` its size), and the f32 at `<offset>` of it followed.
//!
//! Where constants are copied: every fill of a list's upload memory from
//! title memory when the list is submitted (`Step::Fill`, those the card
//! does as well), whichever draw or root view asked for it; and constant
//! buffers a title copy earlier in the same list wrote (copied on the card,
//! `HostList::place_constant`), looked at when their submission was sent,
//! when title memory holds the copy's bytes. (A table made once in a list
//! is filled for the first draw that names it, which may be of another
//! pipeline: so the buffer is picked by its fields, not by the draw. The
//! hash only counts the draws recorded with a pipeline of that name.)
//!
//! - A time series: the value copied is logged with the frame each time it
//!   differs from the one copied before.
//! - Within the frame only: the same title address is read again when that
//!   submission's lists have been sent, and again at the first present
//!   after; a value other than the one copied is logged then, as written
//!   after our copy. After that present the address is let go: the title's
//!   per-frame memory is used again by later frames, which says nothing.
//! - Every [`EVERY`]: how many buffers were looked at and matched, and how
//!   many draws of the hash were recorded, so that a watch that sees
//!   nothing says so.
//!
//! Rendering is not changed: the watch only reads title memory. Not said:
//! when the title last wrote the bytes (the write watch tells pages, not
//! values).

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// The most lines logged (the counts lines aside).
const LINES: usize = 400;
/// The most title addresses waiting for their submission and present.
const PENDING: usize = 64;
/// The most placed constant buffers waiting for their submission.
const PLACED: usize = 4096;
/// How often the counts are logged.
const EVERY: Duration = Duration::from_secs(10);

/// What is watched.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Watch {
    /// As text in the pipeline's name (for the counts); none for `*`.
    pub hash: Option<String>,
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

/// From `<hash or *>,<offset>[,<offset>=<f32>]...[,size=<bytes>]` (offsets
/// `0x..` or decimal, multiples of 4).
pub(crate) fn parse(s: &str) -> Option<Watch> {
    let mut parts = s.split(',');
    let hash = parts.next()?.trim().to_ascii_lowercase();
    let offset = number(parts.next()?)?;
    if hash.is_empty() || offset % 4 != 0 {
        return None;
    }
    let hash = (hash != "*").then_some(hash);
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
                "VICTORY_CONSTANTS_WATCH wants <shader hash or *>,<offset>[,<offset>=<f32>]...[,size=<bytes>]: off"
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
    /// The list (its address) and the frame of the copy.
    list: usize,
    frame: u64,
    copied: u32,
    sent: bool,
}

/// Counts since the last counts line.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct Counts {
    pub fills: u64,
    pub fills_matched: u64,
    pub placed: u64,
    pub placed_matched: u64,
}

/// The watched buffer's copies waiting for their submission and present,
/// and the series of values copied.
#[derive(Default)]
pub(crate) struct Tracker {
    pending: Vec<Pending>,
    /// Placed constant buffers by list: title address and size.
    placed: Vec<(usize, usize, u32)>,
    last: Option<u32>,
    lines: usize,
    pub(crate) counts: Counts,
}

impl Tracker {
    fn line(&mut self, out: &mut Vec<String>, s: String) {
        if self.lines < LINES {
            self.lines += 1;
            out.push(s);
        }
    }

    /// A copy of the buffer: the series and the address to read again.
    fn copied(&mut self, out: &mut Vec<String>, how: &str, addr: usize, list: usize, frame: u64, bits: u32, sent: bool) {
        if self.last != Some(bits) {
            let was = self.last.map_or("none".into(), |b| f32::from_bits(b).to_string());
            self.last = Some(bits);
            self.line(
                out,
                format!(
                    "constants watch: series: frame {frame}: {} copied (was {was}) at {addr:#x}, list {list:#x}, {how}",
                    f32::from_bits(bits)
                ),
            );
        }
        if self.pending.len() < PENDING {
            self.pending.push(Pending { addr, list, frame, copied: bits, sent });
        }
    }

    /// `len` bytes of title memory at `src` (dwords by byte offset: `read`)
    /// are copied to `list`'s upload memory now.
    pub(crate) fn fill(&mut self, w: &Watch, list: usize, src: usize, len: usize, frame: u64, read: impl Fn(usize) -> u32) -> Vec<String> {
        let mut out = Vec::new();
        self.counts.fills += 1;
        let size = u32::try_from(len).unwrap_or(u32::MAX);
        if w.matches(size, |at| read(src + at as usize)) {
            self.counts.fills_matched += 1;
            let addr = src + w.offset as usize;
            self.copied(&mut out, "filled at submission", addr, list, frame, read(addr), false);
        }
        out
    }

    /// A constant buffer of `size` bytes at title `addr` that a title copy
    /// earlier in `list` wrote is copied on the card.
    pub(crate) fn placed(&mut self, list: usize, addr: usize, size: u32) {
        if self.placed.len() < PLACED {
            self.placed.push((list, addr, size));
        }
    }

    /// `list`'s submission was sent: its copies' bytes are read again, and
    /// its placed constant buffers looked at.
    pub(crate) fn sent(&mut self, w: &Watch, list: usize, frame: u64, read: impl Fn(usize) -> u32) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.pending.len() {
            let p = &self.pending[i];
            if p.list != list || p.sent {
                continue;
            }
            let (addr, copied, frame) = (p.addr, p.copied, p.frame);
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
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.placed).into_iter().partition(|p| p.0 == list);
        self.placed = rest;
        for (_, at, size) in mine {
            self.counts.placed += 1;
            if w.matches(size, |o| read(at + o as usize)) {
                self.counts.placed_matched += 1;
                let addr = at + w.offset as usize;
                self.copied(&mut out, "placed on the card", addr, list, frame, read(addr), true);
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
            let now = read(p.addr);
            if now != p.copied {
                self.line(
                    &mut out,
                    format!(
                        "constants watch: {:#x} holds {} at present {frame}, copied with {} (frame {}, list {:#x}): written after our copy",
                        p.addr,
                        f32::from_bits(now),
                        f32::from_bits(p.copied),
                        p.frame,
                        p.list
                    ),
                );
            }
        }
        out
    }

    /// The counts line, and the counts started again.
    pub(crate) fn counts_line(&mut self, w: &Watch, draws: u64) -> String {
        let c = std::mem::take(&mut self.counts);
        let of = w.hash.as_ref().map_or(String::new(), |h| format!(", {draws} draws recorded with a pipeline named {h}"));
        format!(
            "constants watch: in {} s: {} fills looked at, {} matched; {} placed constant buffers looked at, {} matched{of}",
            EVERY.as_secs(),
            c.fills,
            c.fills_matched,
            c.placed,
            c.placed_matched
        )
    }
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);
static DRAWS: AtomicU64 = AtomicU64::new(0);

fn with<T>(f: impl FnOnce(&mut Tracker) -> T) -> T {
    f(TRACKER.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Tracker::default))
}

fn log(lines: Vec<String>) {
    for line in lines {
        tracing::info!("{line}");
    }
}

fn read(addr: usize) -> u32 {
    // SAFETY: title memory a constants copy reads (mapped while the title
    // holds its constant buffer there); an unaligned read of 4 bytes.
    unsafe { std::ptr::read_unaligned(addr as *const u32) }
}

/// A draw's pipeline, by name (the runtime's, with its shader hashes).
pub(crate) fn pipeline(name: &str) {
    if let Some(w) = watch()
        && let Some(h) = &w.hash
        && name.to_ascii_lowercase().contains(h.as_str())
    {
        DRAWS.fetch_add(1, Relaxed);
    }
}

/// `len` bytes of title memory at `src` are copied to `list`'s upload
/// memory now (a `Step::Fill` at submission).
pub(crate) fn fill(list: usize, src: usize, len: usize) {
    if let Some(w) = watch() {
        log(with(|t| t.fill(w, list, src, len, crate::frame::current(), read)));
    }
}

/// A constant buffer that a title copy earlier in `list` wrote is copied on
/// the card.
pub(crate) fn placed(list: usize, addr: usize, size: usize) {
    if watch().is_some() {
        with(|t| t.placed(list, addr, u32::try_from(size).unwrap_or(u32::MAX)));
    }
}

/// `list`'s submission was sent.
pub(crate) fn sent(list: usize) {
    if let Some(w) = watch() {
        log(with(|t| t.sent(w, list, crate::frame::current(), read)));
    }
}

/// A frame was presented.
pub(crate) fn presented() {
    let Some(w) = watch() else { return };
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let due = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let at = *last.get_or_insert_with(Instant::now);
        let due = at.elapsed() >= EVERY;
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    let mut lines = with(|t| t.presented(crate::frame::current(), read));
    if due {
        lines.push(with(|t| t.counts_line(w, DRAWS.swap(0, Relaxed))));
    }
    log(lines);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The environment given for the phone screen's material.
    const PHONE: &str = "909D7B7AF414D9E0,0x18,0x68=-0.3,0x6c=1,0x80=3,0x84=-1";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// The first 0x90 bytes of material constants as the replay dumped them:
    /// the cafe phone's (two captures, two starts), and two lights drawn with
    /// the same pixel shader (96 and 472 at 0x18).
    const PHONE2: &str = "000000000000000000000000cdcc4c3e00000000000000000000803f000080bf0000803f0000803f0000803f00000000000000000000803f000000000000000000000000000000000000803f0000000000000000000000000000803f0000803f00000000000000009a9999be0000803f0000803f0000803f0000803f0000000000004040000080bf0000000000000000";
    const PHONE1: &str = "000000000000000000000000cdcc4c3e00000000000000000000803f000080bf0000803f0000803f0000803f00000000000000000000803f000000000000000000000000000000000000803f0000000000000000000000000000803f0000803f00000000000000009a9999be0000803f0000803f0000803f0000803f0000000000004040000080bf0000000000000000";
    const LIGHT96: &str = "000000000000000000000000cdcccc3d00000000000000000000c042000080bf0000803f0000803f0000803f00000000000000000000803f000000000000000000000000000000000000803f0000000000000000000000000000803f0000803f00000000000000009a9999be000000000000803f0000803f0000803f00000000000080bf000000000000000000000000";
    const LIGHT472: &str = "000000000000000000000000cdcc4c3e00000000000000000000ec43000080bf0000803f0000803f0000803f00000000000000000000803f000000000000000000000000000000000000803f0000000000000000000000000000803f0000803f00000000000000009a9999be0000c03f0000803f0000803f0000803f00000000000080bf0000803f0000000000000000";

    fn dwords(bytes: &[u8]) -> impl Fn(u32) -> u32 + '_ {
        move |at| {
            let a = at as usize;
            bytes.get(a..a + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
        }
    }

    #[test]
    fn the_phone_environment_matches_the_dumped_phones_and_no_light() {
        let w = parse(PHONE).unwrap();
        for (name, bytes, phone) in [("phone2", PHONE2, true), ("phone1", PHONE1, true), ("light 96", LIGHT96, false), ("light 472", LIGHT472, false)] {
            let b = hex(bytes);
            // As copied: 0x160 bytes (the size the runs log for this layout).
            assert_eq!(w.matches(0x160, dwords(&b)), phone, "{name}");
        }
        assert_eq!(f32::from_bits(dwords(&hex(PHONE2))(0x18)), 1.0);
    }

    #[test]
    fn watches_are_read() {
        let w = parse(PHONE).unwrap();
        assert_eq!(w.hash.as_deref(), Some("909d7b7af414d9e0"));
        assert_eq!(w.offset, 0x18);
        assert_eq!(
            w.fields,
            vec![(0x68, (-0.3f32).to_bits()), (0x6c, 1f32.to_bits()), (0x80, 3f32.to_bits()), (0x84, (-1f32).to_bits())]
        );
        assert_eq!(parse("*,0x18").unwrap().hash, None);
        assert_eq!(parse("909d,24,size=0x160").unwrap().size, Some(0x160));
        for bad in ["", "909d", ",0x18", "909d,0x1a", "909d,x", "909d,0x18,0x6c", "909d,0x18,0x6e=1", "909d,0x18,0x6c=one"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    /// Title memory as a map of addresses to dwords.
    fn memory(at: usize, bytes: &[u8]) -> impl Fn(usize) -> u32 + '_ {
        move |a| {
            let o = a.wrapping_sub(at);
            bytes.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
        }
    }

    #[test]
    fn fills_are_matched_by_contents_and_counted() {
        let w = parse(PHONE).unwrap();
        let (phone, light) = (hex(PHONE2), hex(LIGHT472));
        let mut t = Tracker::default();
        assert!(t.fill(&w, 7, 0x1000, 0x160, 100, memory(0x1000, &light)).is_empty());
        let lines = t.fill(&w, 7, 0x2000, 0x160, 100, memory(0x2000, &phone));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("frame 100: 1 copied (was none) at 0x2018"), "{}", lines[0]);
        assert_eq!(t.counts, Counts { fills: 2, fills_matched: 1, placed: 0, placed_matched: 0 });
        let line = t.counts_line(&w, 3);
        assert!(line.contains("2 fills looked at, 1 matched") && line.contains("3 draws"), "{line}");
        assert_eq!(t.counts, Counts::default());
    }

    #[test]
    fn placed_buffers_are_looked_at_when_sent() {
        let w = parse(PHONE).unwrap();
        let phone = hex(PHONE2);
        let mut t = Tracker::default();
        t.placed(9, 0x3000, 0x160);
        t.placed(8, 0x3000, 0x160);
        let lines = t.sent(&w, 9, 50, memory(0x3000, &phone));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("placed on the card"), "{}", lines[0]);
        assert_eq!(t.counts.placed, 1);
        // The other list's stays until its own submission.
        assert_eq!(t.placed.len(), 1);
    }

    #[test]
    fn changes_count_only_within_the_frame() {
        let w = parse("*,0x18").unwrap();
        let mut t = Tracker::default();
        let value = std::cell::Cell::new(1f32.to_bits());
        let read = |_: usize| value.get();
        assert_eq!(t.fill(&w, 7, 0x10, 0x20, 100, read).len(), 1);
        // Unchanged when sent and at the present: nothing; then let go.
        assert!(t.sent(&w, 7, 100, read).is_empty());
        assert!(t.presented(101, read).is_empty());
        // The title's memory used again by a later frame: not reported.
        value.set(7.5f32.to_bits());
        assert!(t.presented(102, read).is_empty());
        // The next frame copies 7.5: a series line; then written after the
        // copy, before the submission was sent: reported.
        assert_eq!(t.fill(&w, 8, 0x10, 0x20, 103, read).len(), 1);
        value.set(472f32.to_bits());
        let lines = t.sent(&w, 8, 103, read);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("holds 472 when its submission was sent, copied with 7.5"), "{}", lines[0]);
    }

    #[test]
    fn a_write_before_the_first_present_is_reported_once() {
        let w = parse("*,0x18").unwrap();
        let mut t = Tracker::default();
        let value = std::cell::Cell::new(1f32.to_bits());
        let read = |_: usize| value.get();
        t.fill(&w, 7, 0x10, 0x20, 100, read);
        t.sent(&w, 7, 100, read);
        value.set(472f32.to_bits());
        let lines = t.presented(101, read);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("holds 472 at present 101, copied with 1"), "{}", lines[0]);
        assert!(t.presented(102, read).is_empty());
        // A list not yet sent waits through a present.
        t.fill(&w, 9, 0x10, 0x20, 102, read);
        assert!(t.presented(103, read).is_empty());
        assert_eq!(t.pending.len(), 1);
    }

    #[test]
    fn the_same_value_again_is_no_series_line() {
        let w = parse("*,0x18").unwrap();
        let mut t = Tracker::default();
        let read = |_: usize| 1f32.to_bits();
        for list in 0..5usize {
            let n = t.fill(&w, list, 0x10 + list * 0x100, 0x20, 100 + list as u64, read).len();
            assert_eq!(n, usize::from(list == 0));
            t.sent(&w, list, 100, read);
            t.presented(101 + list as u64, read);
        }
    }
}
