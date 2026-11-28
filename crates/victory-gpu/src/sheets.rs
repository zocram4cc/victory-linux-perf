//! The frame recorder's watch for stretched geometry ("sheets": character
//! meshlets drawn with vertices in the wrong place, as huge flat triangles
//! across the screen for a frame or a few). `VICTORY_RECORD_SHEETS=1`: the
//! title's pre-skinned mesh programs (`PreSkin` in their container's name)
//! are translated with [`victory_shader::emit::with_sheet_watch`] and note,
//! per draw, waves that span much of the screen and vertices far from both
//! neighbours ([`victory_shader::emit::SLOT_SHEETS`]), in a buffer in the
//! PC's memory: each watched draw is bound a region of its own, and what the
//! draw was (pipeline, the title's root views, tables and constants) is kept
//! here by region. The recorder reads and clears it at each present
//! ([`take`]): no copy, no wait. What is drawn does not change.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use victory_shader::emit::{SHEET_BUCKETS, SHEET_EVENT_DWORDS, SHEET_EVENTS, SHEET_EVENTS_AT, SHEET_REGION_DWORDS, SHEET_REGIONS};
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

/// Dwords of the buffer.
const DWORDS: u32 = SHEET_REGIONS * SHEET_REGION_DWORDS;
/// Bytes of the buffer.
pub const BYTES: u64 = DWORDS as u64 * 4;

/// Whether the watch is on (`VICTORY_RECORD_SHEETS=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RECORD_SHEETS").is_some_and(|v| v == "1"))
}

/// The watch's id of a program (in its events): never 0.
pub fn program_id(container: &[u8]) -> u32 {
    let h = blake3::hash(container);
    let b = h.as_bytes();
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) | 1
}

/// The id to translate a program with: 0 (no watch) unless the watch is on,
/// the program is pre-skinned and the buffer is made (a watched program's
/// root view must always be bound).
pub(crate) fn watched(container: &[u8]) -> u32 {
    if !on() || !pre_skinned(container) || notes().is_none() {
        return 0;
    }
    let id = program_id(container);
    let hash = blake3::hash(container).to_hex()[..16].to_owned();
    if let Ok(mut names) = NAMES.lock()
        && !names.iter().any(|n| n.0 == id)
    {
        names.push((id, hash));
    }
    id
}

/// Programs watched: id and the hash the log names them by.
static NAMES: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());

/// Whether a container is of a pre-skinned program (the replay's
/// `--retranslate` asks, with the watch on).
pub fn pre_skinned(container: &[u8]) -> bool {
    is_watched_name(crate::pipeline::named::name_in(container).as_deref())
}

/// The pre-skinned programs: those that draw the characters' meshlets.
fn is_watched_name(name: Option<&str>) -> bool {
    name.is_some_and(|n| n.contains("PreSkin"))
}

struct Notes {
    _buffer: ID3D12Resource,
    data: *mut u32,
    gpu: u64,
    next: AtomicU32,
    /// By region: the present it was handed out at, and what the draw was.
    draws: Mutex<Vec<Option<(u64, String)>>>,
}

// SAFETY: the mapping lives as long as the buffer, which lives as long as
// the process; its dwords are read and written as single volatile words.
unsafe impl Send for Notes {}
// SAFETY: as above.
unsafe impl Sync for Notes {}

/// Presents so far ([`take`] counts them).
static PRESENTS: AtomicU64 = AtomicU64::new(0);

fn notes() -> Option<&'static Notes> {
    static NOTES: OnceLock<Option<Notes>> = OnceLock::new();
    NOTES
        .get_or_init(|| {
            let host = crate::host::host()?;
            let (buffer, data) = host.shared_buffer(BYTES)?;
            crate::host::name(&buffer, || "frame recorder: sheets".into());
            // SAFETY: a fresh mapping of that many bytes.
            unsafe { std::ptr::write_bytes(data, 0, BYTES as usize) };
            // SAFETY: a live buffer.
            let gpu = unsafe { buffer.GetGPUVirtualAddress() };
            tracing::info!("frame recorder: the pre-skinned mesh programs note stretched geometry (VICTORY_RECORD_SHEETS)");
            Some(Notes {
                _buffer: buffer,
                data: data.cast::<u32>(),
                gpu,
                next: AtomicU32::new(0),
                draws: Mutex::new(vec![None; SHEET_REGIONS as usize]),
            })
        })
        .as_ref()
}

/// For a draw of a watched program: the address of a region of its own to
/// bind as its root UAV; `what` says what the draw is (made only here).
pub(crate) fn region_for(what: impl FnOnce() -> String) -> Option<u64> {
    if !on() {
        return None;
    }
    let n = notes()?;
    let r = n.next.fetch_add(1, Ordering::Relaxed) % SHEET_REGIONS;
    if let Ok(mut draws) = n.draws.lock() {
        draws[r as usize] = Some((PRESENTS.load(Ordering::Relaxed), what()));
    }
    Some(n.gpu + u64::from(r) * u64::from(SHEET_REGION_DWORDS) * 4)
}

/// A vertex far from both neighbours, as a draw noted it.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub program: u32,
    pub group: u32,
    pub thread: u32,
    /// The largest of |dx|, |dy|, |dw| to the nearer neighbour (clip units).
    pub jump: f32,
    /// Clip x, y, z, w of the vertex, the one below and the one above.
    pub at: [[f32; 4]; 3],
}

/// What one draw noted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draw {
    /// Its waves' widest span row passed (`SHEET_SPANS`), if any.
    pub rows: [bool; SHEET_BUCKETS as usize],
    pub events: Vec<Event>,
}

impl Draw {
    /// The largest jump of its events.
    pub fn jump(&self) -> f32 {
        self.events.iter().map(|e| e.jump).fold(0.0, f32::max)
    }
}

/// One region's notes, read from `dwords` (a region's), if there are any.
pub fn read_region(dwords: &[u32]) -> Option<Draw> {
    let mut d = Draw::default();
    for (k, row) in d.rows.iter_mut().enumerate() {
        *row = dwords.get(k).is_some_and(|&v| v != 0);
    }
    for e in 0..SHEET_EVENTS as usize {
        let at = SHEET_EVENTS_AT as usize + e * SHEET_EVENT_DWORDS as usize;
        let Some(w) = dwords.get(at..at + SHEET_EVENT_DWORDS as usize) else {
            break;
        };
        if w[0] == 0 {
            continue;
        }
        let f = |i: usize| f32::from_bits(w[i]);
        d.events.push(Event {
            program: w[0],
            group: w[1],
            thread: w[2],
            jump: f(3),
            at: [
                [f(4), f(5), f(6), f(7)],
                [f(8), f(9), f(10), f(11)],
                [f(12), f(13), f(14), f(15)],
            ],
        });
    }
    (d.rows.iter().any(|&r| r) || !d.events.is_empty()).then_some(d)
}

/// What draws noted since the last call (which clears it): with each, the
/// present it was drawn at and what it was. Counts a present. None while
/// the watch is off.
pub fn take() -> Option<Vec<(u64, String, Draw)>> {
    if !on() {
        return None;
    }
    PRESENTS.fetch_add(1, Ordering::Relaxed);
    let n = notes()?;
    let mut out = Vec::new();
    let draws = n.draws.lock().ok()?;
    let mut words = vec![0u32; SHEET_REGION_DWORDS as usize];
    for (r, draw) in draws.iter().enumerate() {
        let base = r * SHEET_REGION_DWORDS as usize;
        let mut any = false;
        for (i, out) in words.iter_mut().enumerate() {
            // SAFETY: inside the mapping; the card may be writing (a note
            // missed or half read now is one of the next present's).
            let at = unsafe { n.data.add(base + i) };
            // SAFETY: as above.
            let v = unsafe { at.read_volatile() };
            *out = v;
            if v != 0 {
                any = true;
                // SAFETY: as above.
                unsafe { at.write_volatile(0) };
            }
        }
        if !any {
            continue;
        }
        if let Some(d) = read_region(&words) {
            let (at, what) = draw.clone().unwrap_or_default();
            out.push((at, what, d));
        }
    }
    Some(out)
}

/// The hash of a watched program, by its id.
pub fn program_name(id: u32) -> String {
    NAMES
        .lock()
        .ok()
        .and_then(|n| n.iter().find(|p| p.0 == id).map(|p| p.1.clone()))
        .unwrap_or_else(|| format!("{id:08x}"))
}

/// One line for a draw's notes (the log and the replay).
pub fn line(at: u64, what: &str, d: &Draw) -> String {
    let rows: String = d.rows.iter().map(|&r| if r { '1' } else { '.' }).collect();
    let mut s = format!("present {at}: rows {rows}, largest jump {:.2}: {what}", d.jump());
    for e in &d.events {
        let p = |v: [f32; 4]| format!("({:.2} {:.2} {:.2} {:.2})", v[0], v[1], v[2], v[3]);
        s.push_str(&format!(
            "; {} group {} thread {} jump {:.2} at {} below {} above {}",
            program_name(e.program),
            e.group,
            e.thread,
            e.jump,
            p(e.at[0]),
            p(e.at[1]),
            p(e.at[2])
        ));
    }
    s
}

/// Whether a flagged draw's bytes are logged too
/// (`VICTORY_RECORD_SHEETS_BYTES=1`, with `VICTORY_RECORD_SHEETS`; off
/// unless set): [`bytes_line`].
pub(crate) fn bytes_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| on() && std::env::var_os("VICTORY_RECORD_SHEETS_BYTES").is_some_and(|v| v == "1"))
}

/// Where a pre-skinned draw finds its positions, by its record's range word
/// (the record's byte 0x54): which of the two buffers (bits 24-25), where in
/// it (the low 24 bits x 16: a 32-byte header {count, 0xdeadbeef}, then the
/// vertices), and how they are laid out (bit 29 float3, bit 28 10:10:10,
/// else 24:24:16 in 8 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub second: bool,
    pub offset: u64,
    pub stride: usize,
}

impl Range {
    pub fn of(word: u32) -> Range {
        let stride = if word & 0x2000_0000 != 0 {
            12
        } else if word & 0x1000_0000 != 0 {
            4
        } else {
            8
        };
        Range { second: (word >> 24) & 3 != 0, offset: u64::from(word << 4 & 0x0fff_fff0), stride }
    }

    pub fn kind(&self) -> &'static str {
        match self.stride {
            12 => "float3",
            4 => "10:10:10",
            _ => "24:24:16",
        }
    }

    /// A vertex's position from its bytes, `None` for one hidden on purpose
    /// (all ones).
    pub fn decode(&self, raw: &[u8]) -> Option<[f32; 3]> {
        let w = |k: usize| u32::from_le_bytes(raw[k * 4..k * 4 + 4].try_into().unwrap_or([0; 4]));
        match self.stride {
            12 => (!raw.iter().all(|&b| b == 0xff)).then(|| [0, 1, 2].map(|k| f32::from_bits(w(k)))),
            4 => {
                let d = w(0);
                (d != u32::MAX).then(|| [0, 10, 20].map(|s| -3.0 + 6.0 * ((d >> s) & 1023) as f32 / 1023.0))
            }
            _ => {
                let (d0, d1) = (w(0), w(1));
                (d0 & d1 != u32::MAX).then(|| {
                    [
                        -3.0 + (d0 >> 8) as f32 * 6.0 / 16_777_216.0,
                        -3.0 + ((d0 & 0xff) | (d1 & 0xffff) << 8) as f32 * 6.0 / 16_777_216.0,
                        -3.0 + (d1 >> 16) as f32 * 6.0 / 65535.0,
                    ]
                })
            }
        }
    }
}

/// The vertices of `positions` (decoded, `None` hidden) that are both far
/// from the median (more than 1.0 in a coordinate) and lone (more than 0.5
/// in a coordinate from both neighbours), as the census tool counts spikes.
pub fn outliers(positions: &[Option<[f32; 3]>]) -> Vec<usize> {
    let shown: Vec<[f32; 3]> = positions.iter().flatten().copied().collect();
    if shown.len() < 3 {
        return Vec::new();
    }
    let median: [f32; 3] = std::array::from_fn(|k| {
        let mut v: Vec<f32> = shown.iter().map(|p| p[k]).filter(|x| x.is_finite()).collect();
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    });
    let apart = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0f32, |m, d| if d.is_nan() { f32::INFINITY } else { m.max(d) });
    let mut out = Vec::new();
    for (i, p) in positions.iter().enumerate() {
        let Some(p) = *p else { continue };
        if apart(p, median) <= 1.0 {
            continue;
        }
        let near = |j: Option<usize>| j.and_then(|j| positions.get(j).copied().flatten()).is_some_and(|q| apart(p, q) <= 0.5);
        if !near(i.checked_sub(1)) && !near(Some(i + 1)) {
            out.push(i);
        }
    }
    out
}

fn table(what: &str, name: &str) -> Option<u64> {
    let at = what.find(name)? + name.len();
    let hex = what[at..].strip_prefix("0x")?;
    let end = hex.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(hex.len());
    u64::from_str_radix(&hex[..end], 16).ok()
}

/// For a draw the watch flagged (`what` as [`region_for`] was told it): the
/// position buffer it read, its range, up to 8 of the range's outlier
/// vertices with their bytes as title memory holds them now and as the
/// CPU's side of the copy holds them (what was last sent to the card; the
/// card's own copy is not read: that would wait for the card), and of the
/// 64 KiB piece the first lies in: frames since a landing put shaders' bytes
/// into it, whether hold-back holds title bytes there, and whether shaders
/// were declared to write the copy this frame. The record and the views are
/// looked up now, a present after the draw: a slot the title wrote since
/// shows its new view. None if it is no pre-skinned draw of these tables.
#[cfg(windows)]
pub fn bytes_line(what: &str) -> Option<String> {
    let (t0, t1) = (table(what, "table0=")?, table(what, "table1=")?);
    let range_of = |handle: u64| crate::descriptor::view(handle).and_then(|v| crate::descriptor::host::buffer_range(&v));
    let (record, _) = range_of(t1)?;
    let mut w = [0u8; 4];
    if !crate::host::read_into((record + 0x54) as *const u8, &mut w) {
        return Some(format!("record at {record:#x}: its range word cannot be read"));
    }
    let word = u32::from_le_bytes(w);
    let r = Range::of(word);
    let view = crate::descriptor::view(t0 + if r.second { 0xa80 } else { 0x420 })?;
    let Some((shadow, base, _)) = crate::descriptor::host::buffer_copy(&view) else {
        return Some(format!("range word {word:#010x}: positions-{} has no host copy", u8::from(r.second)));
    };
    let at = base + r.offset;
    let mut head = [0u8; 8];
    crate::host::read_into(at as *const u8, &mut head);
    let count = u32::from_le_bytes(head[..4].try_into().unwrap_or([0; 4])) as usize;
    let marker = u32::from_le_bytes(head[4..].try_into().unwrap_or([0; 4]));
    let (start, len) = shadow.span();
    let mut s = format!(
        "range word {word:#010x}: positions-{} {base:#x} (copy {start:#x}+{len:#x}) +{:#x}, {}, header count {count} marker {marker:#x}",
        u8::from(r.second),
        r.offset,
        r.kind()
    );
    let n = count.min(1 << 16);
    let bytes = n * r.stride;
    let first = (at + 32) as usize;
    let mut title = vec![0u8; bytes];
    let title_ok = crate::host::read_into(first as *const u8, &mut title);
    let held = shadow.card_held(first - start as usize, bytes);
    if held.len() < bytes {
        s.push_str("; the copy holds less than the range (not in the card's memory, or cut)");
        return Some(s);
    }
    let decode = |b: &[u8]| (0..n).map(|i| r.decode(&b[i * r.stride..(i + 1) * r.stride])).collect::<Vec<_>>();
    let sent = outliers(&decode(&held));
    let titles = if title_ok { outliers(&decode(&title)) } else { Vec::new() };
    let mut all: Vec<usize> = sent.iter().chain(&titles).copied().collect();
    all.sort_unstable();
    all.dedup();
    s.push_str(&format!(
        "; outliers: {} as sent to the card, {} in title memory{}",
        sent.len(),
        titles.len(),
        if title_ok { "" } else { " (title memory not read)" }
    ));
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    for &i in all.iter().take(8) {
        let k = i * r.stride..(i + 1) * r.stride;
        s.push_str(&format!("; v{i} title {} sent {}", hex(&title[k.clone()]), hex(&held[k])));
    }
    if let Some(&i) = all.first() {
        let (landed, holds, declared) = shadow.sheet_piece(first + i * r.stride);
        let now = crate::frame::current();
        s.push_str(&format!(
            "; its piece: {}, hold-back {}, shaders declared to write the copy {}",
            landed.map_or("no landing noted".to_owned(), |f| format!("landed {} frames ago", now.saturating_sub(f))),
            if holds { "holds title bytes there" } else { "holds nothing there" },
            declared.map_or("never".to_owned(), |f| match now.saturating_sub(f) {
                0 => "this frame".to_owned(),
                d => format!("{d} frames ago"),
            }),
        ));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_word_names_buffer_place_and_layout() {
        let r = Range::of(0x6001_0e88);
        assert_eq!((r.second, r.offset, r.kind()), (false, 0x10e880, "float3"));
        let r = Range::of(0x4101_4631);
        assert_eq!((r.second, r.offset, r.kind()), (true, 0x146310, "24:24:16"));
        assert_eq!(Range::of(0x1000_0000).kind(), "10:10:10");
        assert_eq!(Range::of(0x6000_0000).decode(&[0xff; 12]), None, "hidden on purpose");
        let p = Range::of(0x6000_0000).decode(&[0, 0, 0x80, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0xc0]).unwrap();
        assert_eq!(p, [1.0, 0.0, -2.0]);
    }

    #[test]
    fn a_lone_far_vertex_is_an_outlier_and_a_run_of_far_ones_is_not() {
        let near = |x: f32| Some([x, 0.0, 0.0]);
        let mut v: Vec<Option<[f32; 3]>> = (0..20).map(|i| near(i as f32 * 0.01)).collect();
        v[7] = Some([0.1, -1.0e34, 0.0]);
        v[12] = None;
        assert_eq!(outliers(&v), [7]);
        // Two far vertices side by side are each other's neighbour.
        v[13] = Some([3.0, 3.0, 3.0]);
        v[14] = Some([3.0, 3.0, 3.1]);
        assert_eq!(outliers(&v), [7]);
        assert_eq!(table("p table0=0xa680000 table1=0xbf3d920 x", "table1="), Some(0xbf3d920));
    }

    #[test]
    fn the_pre_skinned_programs_are_watched() {
        assert!(is_watched_name(Some("MS_VS_PedTransformPreSkinD_Packed")));
        assert!(!is_watched_name(Some("MS_VS_PedTransform")));
        assert!(!is_watched_name(None));
        assert_ne!(program_id(b"any"), 0);
    }

    #[test]
    fn a_region_reads_as_rows_and_events() {
        let mut r = vec![0u32; SHEET_REGION_DWORDS as usize];
        assert_eq!(read_region(&r), None, "nothing noted");
        r[3] = 1;
        let e = SHEET_EVENTS_AT as usize + 2 * SHEET_EVENT_DWORDS as usize;
        r[e] = 0x1234_5679;
        r[e + 1] = 7;
        r[e + 2] = 40;
        r[e + 3] = 2.5f32.to_bits();
        r[e + 7] = 3.0f32.to_bits();
        let d = read_region(&r).unwrap();
        assert_eq!(d.rows, [false, false, false, true]);
        assert_eq!(d.events.len(), 1);
        assert_eq!((d.events[0].program, d.events[0].group, d.events[0].thread), (0x1234_5679, 7, 40));
        assert_eq!(d.jump(), 2.5);
        assert_eq!(d.events[0].at[0][3], 3.0);
        assert!(line(5, "draw", &d).contains("largest jump 2.50"));
        // A row alone is a note too.
        let mut r = vec![0u32; SHEET_REGION_DWORDS as usize];
        r[0] = 1;
        assert_eq!(read_region(&r).map(|d| d.events.len()), Some(0));
    }
}
