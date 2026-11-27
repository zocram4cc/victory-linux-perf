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

#[cfg(test)]
mod tests {
    use super::*;

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
