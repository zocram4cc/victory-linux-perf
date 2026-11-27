//! Diagnostics: the run's numbers on the picture (`VICTORY_PANEL=1`; off by
//! default; F3 shows and hides it while our window has the focus): white
//! text on a black box in the upper left corner of what is shown, so that a
//! screenshot carries them, with what it costs itself on the submitting
//! thread. Made again twice a second from the counters of
//! the frame-rate line, memory and textures; a frame itself only copies the
//! box out of a buffer in the card's memory. Copied onto the swap chain's
//! buffer, behind the title's picture: saved frames and the frame recorder
//! see the title's own.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F3};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::{Host, HostList, transition};

/// How often the numbers change.
const EVERY: Duration = Duration::from_millis(500);

/// A character's dots, and with the space beside and below it.
const GLYPH: (u32, u32) = (5, 7);
const CELL: (u32, u32) = (6, 9);
/// Characters in a line, lines, and the box's rim in dots.
const COLUMNS: u32 = 40;
const LINES: u32 = 9;
const RIM: u32 = 3;

/// The texts are put into one of these in turn: the card reads the others,
/// at most half a second old.
const UPLOADS: usize = 4;

fn asked() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_PANEL"))
}

/// Whether it is shown now.
fn shown() -> &'static AtomicBool {
    static SHOWN: OnceLock<AtomicBool> = OnceLock::new();
    SHOWN.get_or_init(|| AtomicBool::new(asked()))
}

/// F3 was pressed since the last call, with one of our windows in front.
fn pressed() -> bool {
    static DOWN: AtomicBool = AtomicBool::new(false);
    // SAFETY: a plain query.
    let down = unsafe { GetAsyncKeyState(i32::from(VK_F3)) } < 0;
    if DOWN.swap(down, Ordering::Relaxed) || !down {
        return false;
    }
    let mut process = 0;
    // SAFETY: plain queries; room for the answer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    process == std::process::id()
}

/// Seven rows of five dots, the leftmost the highest bit.
#[rustfmt::skip]
const fn glyph(c: u8) -> [u8; 7] {
    match c {
        b'0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        b'1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        b'3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        b'4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        b'5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        b'6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        b'7' => [0b11111, 0b00001, 0b00010, 0b00010, 0b00100, 0b00100, 0b00100],
        b'8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        b'9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        b'A' => [0b00100, 0b01010, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001],
        b'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        b'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        b'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        b'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        b'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        b'G' => [0b01111, 0b10000, 0b10000, 0b10011, 0b10001, 0b10001, 0b01111],
        b'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        b'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        b'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        b'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        b'N' => [0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001],
        b'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        b'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        b'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        b'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        b'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        b'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        b'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        b'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        b'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        b'.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        b'/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        b'-' => [0, 0, 0, 0b11111, 0, 0, 0],
        b' ' => [0; 7],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

/// The box's size in pixels, a dot being `scale` of them each way.
const fn size(scale: u32) -> (u32, u32) {
    ((COLUMNS * CELL.0 + 2 * RIM) * scale, (LINES * CELL.1 + 2 * RIM) * scale)
}

/// Pixels to a dot for a picture that high: a 720-line screenshot of it
/// shows one and a third.
fn scale(height: u32) -> u32 {
    (height / 540).max(1)
}

/// Whether a dot of the picture of `lines` is white.
fn lit(lines: &[String], x: u32, y: u32) -> bool {
    let (Some(x), Some(y)) = (x.checked_sub(RIM), y.checked_sub(RIM)) else {
        return false;
    };
    if x % CELL.0 >= GLYPH.0 || y % CELL.1 >= GLYPH.1 || x / CELL.0 >= COLUMNS {
        return false;
    }
    let Some(&c) = lines.get((y / CELL.1) as usize).and_then(|l| l.as_bytes().get((x / CELL.0) as usize)) else {
        return false;
    };
    (glyph(c.to_ascii_uppercase())[(y % CELL.1) as usize] >> (GLYPH.0 - 1 - x % CELL.0)) & 1 != 0
}

/// The bytes of a black and a white pixel of a swap chain's format.
fn colours(format: DXGI_FORMAT) -> Option<[&'static [u8]; 2]> {
    Some(match format {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM => [&[0, 0, 0, 0xFF], &[0xFF; 4]],
        DXGI_FORMAT_R10G10B10A2_UNORM => [&[0, 0, 0, 0xC0], &[0xFF; 4]],
        // Halves of 0 and 1.
        DXGI_FORMAT_R16G16B16A16_FLOAT => [
            &[0, 0, 0, 0, 0, 0, 0, 0x3C],
            &[0, 0x3C, 0, 0x3C, 0, 0x3C, 0, 0x3C],
        ],
        _ => return None,
    })
}

/// The box as rows of `pitch` bytes.
fn painted(lines: &[String], scale: u32, colours: [&[u8]; 2], pitch: usize) -> Vec<u8> {
    let (width, height) = size(scale);
    let bytes = colours[0].len();
    let mut out = vec![0; pitch * height as usize];
    for y in 0..height / scale {
        let top = pitch * (y * scale) as usize;
        for x in 0..width / scale {
            let colour = colours[usize::from(lit(lines, x, y))];
            for i in 0..scale {
                let at = top + (x * scale + i) as usize * bytes;
                out[at..at + bytes].copy_from_slice(colour);
            }
        }
        for i in 1..scale as usize {
            out.copy_within(top..top + pitch, top + i * pitch);
        }
    }
    out
}

/// The counters read for it. They grow, and go back to 0 with the frame-rate
/// line (right behind a look of ours).
#[derive(Clone, Copy, Default)]
struct Counters([u64; COUNTERS]);

const COUNTERS: usize = 11;

impl Counters {
    fn now() -> Counters {
        use super::{CARD_CLOCK_US, COPY_BACK, PREPARING};
        use crate::copystat::{C, peek};
        let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
        Counters([
            super::worked(),
            get(&CARD_CLOCK_US[0]),
            get(&CARD_CLOCK_US[1]),
            get(&CARD_CLOCK_US[2]),
            get(&PREPARING[3]),
            get(&COPY_BACK[0]),
            get(&COPY_BACK[1]),
            get(&crate::shadow::CARD_STATS[0]),
            crate::draw::dropped_now(),
            peek(C::RecordNs),
            crate::draw::drawn(),
        ])
    }

    /// What came since `before`, per frame.
    fn since(self, before: Counters, frames: f64) -> [f64; COUNTERS] {
        let mut out = [0.0; COUNTERS];
        for (i, out) in out.iter_mut().enumerate() {
            let (was, is) = (before.0[i], self.0[i]);
            *out = if is >= was { is - was } else { is } as f64 / frames;
        }
        out
    }
}

/// The text. `c`: [`Counters`] per frame, `ms` a frame's time, `worst` the
/// longest frame's, `cost` what this takes of a frame, in microseconds.
fn text(c: &[f64; COUNTERS], ms: f64, worst: f64, cost: f64, host: &Host) -> Vec<String> {
    let gb = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
    let (working, private) = crate::copystat::process_memory();
    let [budget, used, _, shared] = host.video_memory().unwrap_or_default();
    let idle = (c[1] / 1e3).min(ms);
    vec![
        format!("FPS {:.1}  FRAME {ms:.1} MS  LONGEST {worst:.0}", 1e3 / ms.max(0.001)),
        format!("DRAWS {:.0}  DROPPED {:.0}", c[10], c[8]),
        format!("SUBMIT BUSY {:.1}  WAITS FOR GPU {:.1}", c[0] / 1e3, c[3] / 1e3),
        format!("GPU BUSY {:.1}  IDLE BEHIND US {:.1}", ms - idle, c[2] / 1e3),
        format!("RECORDING {:.0} MS  COPY BACK {:.1}", c[9] / 1e6, c[5] / 1e3),
        format!("MB UP {:.0}  TEXTURES {:.0}  BACK {:.0}", c[7] / 1e6, c[4] / 1e6, c[6] / 1e6),
        format!("RAM {:.1} GB  IN MEMORY {:.1}", gb(private), gb(working)),
        format!("VRAM {:.1}/{:.1} GB  SHARED {:.1}", gb(used), gb(budget), gb(shared)),
        format!("THIS PANEL {:.0} US A FRAME", cost),
    ]
}

/// The buffers of a box that size and format.
struct Buffers {
    format: DXGI_FORMAT,
    scale: u32,
    pitch: u32,
    uploads: Vec<(ID3D12Resource, *mut u8)>,
    card: ID3D12Resource,
    /// The upload written last.
    turn: usize,
    /// It is not in the card's buffer yet.
    fresh: bool,
    /// `card` has already been copied into once and now sits in `COPY_SOURCE`.
    uploaded: bool,
}

// SAFETY: free-threaded resources, and memory only written under the presenter's lock.
unsafe impl Send for Buffers {}

impl Buffers {
    fn new(host: &Host, format: DXGI_FORMAT, scale: u32) -> Option<Buffers> {
        let (width, height) = size(scale);
        let pitch = (width * colours(format)?[0].len() as u32).next_multiple_of(256);
        let bytes = u64::from(pitch) * u64::from(height);
        let uploads = (0..UPLOADS).map(|_| host.upload_buffer(bytes)).collect::<Option<Vec<_>>>()?;
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut card: Option<ID3D12Resource> = None;
        // SAFETY: plain creation of a buffer.
        unsafe {
            host.device
                .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut card)
                .ok()?;
        }
        Some(Buffers { format, scale, pitch, uploads, card: card?, turn: 0, fresh: false, uploaded: false })
    }

    fn write(&mut self, lines: &[String]) {
        let Some(colours) = colours(self.format) else { return };
        let picture = painted(lines, self.scale, colours, self.pitch as usize);
        self.turn = (self.turn + 1) % UPLOADS;
        // SAFETY: a mapped upload buffer of the picture's size, which the card read a second or more ago.
        unsafe { std::ptr::copy_nonoverlapping(picture.as_ptr(), self.uploads[self.turn].1, picture.len()) };
        self.fresh = true;
    }
}

/// The panel of a presenter.
pub(super) struct Panel {
    since: Instant,
    frames: u32,
    last: Instant,
    worst: f64,
    /// What this took since the last text.
    cost: Duration,
    before: Counters,
    lines: Vec<String>,
    changed: bool,
    buffers: Option<Buffers>,
    failed: bool,
}

impl Panel {
    pub(super) fn new() -> Panel {
        let now = Instant::now();
        Panel {
            since: now,
            frames: 0,
            last: now,
            worst: 0.0,
            cost: Duration::ZERO,
            before: Counters::default(),
            lines: Vec::new(),
            changed: false,
            buffers: None,
            failed: false,
        }
    }

    /// A frame was presented; `line`: the counters go back to 0 right behind
    /// this call. (Not shown it asks for the key's state and no more.)
    pub(super) fn frame(&mut self, line: bool) {
        if pressed() {
            shown().fetch_xor(true, Ordering::Relaxed);
        }
        if !shown().load(Ordering::Relaxed) {
            self.frames = 0;
            return;
        }
        let now = Instant::now();
        if self.frames == 0 && now - self.last > 2 * EVERY {
            // (Shown from here on.)
            (self.since, self.last, self.worst, self.before) = (now, now, 0.0, Counters::now());
            self.frames = 0;
        }
        self.frames += 1;
        self.worst = self.worst.max((now - self.last).as_secs_f64() * 1e3);
        self.last = now;
        let elapsed = now - self.since;
        if elapsed < EVERY && !(line && elapsed * 2 >= EVERY) {
            return;
        }
        let Some(host) = super::host() else { return };
        let frames = f64::from(self.frames);
        let counters = Counters::now();
        let c = counters.since(self.before, frames);
        self.before = counters;
        let cost = self.cost.as_secs_f64() * 1e6 / frames;
        self.lines = text(&c, elapsed.as_secs_f64() * 1e3 / frames, self.worst, cost, host);
        self.changed = true;
        (self.since, self.frames, self.worst) = (now, 0, 0.0);
        self.cost = now.elapsed();
    }

    /// Records the copy of the box onto `back`, a buffer of the swap chain of
    /// that size and format in the state to be copied into.
    pub(super) fn draw(
        &mut self,
        host: &Host,
        list: &HostList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
    ) {
        if !shown().load(Ordering::Relaxed) || self.lines.is_empty() || self.failed {
            return;
        }
        let began = Instant::now();
        // Not the title's: the frame recorder leaves it out.
        let _ours = crate::replay::record::unrecorded();
        let scale = scale(height);
        if self.buffers.as_ref().is_none_or(|b| (b.format, b.scale) != (format, scale)) {
            // (What the card may still read stays: a picture's size changes once or twice in a run.)
            std::mem::forget(self.buffers.take());
            self.buffers = Buffers::new(host, format, scale);
            self.failed = self.buffers.is_none();
            self.changed = true;
        }
        let Some(buffers) = &mut self.buffers else { return };
        if std::mem::take(&mut self.changed) {
            buffers.write(&self.lines);
        }
        let (w, h) = size(scale);
        let list = &list.list;
        // SAFETY: an open list and live resources; the box lies within both.
        unsafe {
            if std::mem::take(&mut buffers.fresh) {
                if buffers.uploaded {
                    list.ResourceBarrier(&[transition(
                        &buffers.card,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    )]);
                }
                list.CopyResource(&buffers.card, &buffers.uploads[buffers.turn].0);
                list.ResourceBarrier(&[transition(
                    &buffers.card,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                )]);
                buffers.uploaded = true;
            }
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&buffers.card),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: format,
                            Width: w,
                            Height: h,
                            Depth: 1,
                            RowPitch: buffers.pitch,
                        },
                    },
                },
            };
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(back),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            let part = D3D12_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: w.min(width),
                bottom: h.min(height),
                back: 1,
            };
            list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&part));
        }
        self.cost += began.elapsed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the lines use has its own picture, and no two look
    /// alike.
    #[test]
    fn characters_differ() {
        let used: Vec<u8> = (b'0'..=b'9').chain(b'A'..=b'Z').chain(*b"./- ").collect();
        for (i, a) in used.iter().enumerate() {
            assert_ne!(glyph(*a), glyph(b'#'), "{} has a picture", *a as char);
            assert!(glyph(*a).iter().all(|row| row >> GLYPH.0 == 0));
            for b in &used[i + 1..] {
                assert_ne!(glyph(*a), glyph(*b), "{} {}", *a as char, *b as char);
            }
        }
    }

    #[test]
    fn a_box_is_painted() {
        let lines = vec!["I".to_owned(), " i".to_owned()];
        for scale in [1, 2, 4] {
            let (width, height) = size(scale);
            let pitch = (width * 4).next_multiple_of(256) as usize;
            let colours = colours(DXGI_FORMAT_R10G10B10A2_UNORM).unwrap();
            let picture = painted(&lines, scale, colours, pitch);
            assert_eq!(picture.len(), pitch * height as usize);
            let at = |x: u32, y: u32| &picture[y as usize * pitch + x as usize * 4..][..4];
            // The rim is black, the first letter's top row lit in its middle.
            assert_eq!(at(0, 0), colours[0]);
            assert_eq!(at((RIM + 2) * scale, RIM * scale), colours[1]);
            assert_eq!(at((RIM + 2) * scale + scale - 1, RIM * scale + scale - 1), colours[1]);
            assert_eq!(at(RIM * scale, RIM * scale), colours[0]);
            // Small letters are shown as capitals, a line and a cell on.
            assert_eq!(at((RIM + CELL.0 + 2) * scale, (RIM + CELL.1) * scale), colours[1]);
            // Nothing past a line's end.
            assert_eq!(at((RIM + CELL.0 * 2 + 2) * scale, (RIM + CELL.1) * scale), colours[0]);
        }
    }

    #[test]
    fn counters_that_went_back_to_nothing_count_from_there() {
        let (mut a, mut b) = (Counters::default(), Counters::default());
        (a.0[0], b.0[0]) = (10, 30);
        (a.0[1], b.0[1]) = (50, 20);
        let c = b.since(a, 10.0);
        assert_eq!((c[0], c[1]), (2.0, 2.0));
    }
}
