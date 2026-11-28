//! Diagnostics: textures that pop. `VICTORY_POP_WATCH=<from>-<to>[,<from>-<to>..]`
//! (seconds since the start): every presented frame in those times is read
//! back, made small on a thread of its own and compared with its neighbours,
//! block by block. Counted and logged:
//! - blips: blocks that one frame shows otherwise than the frames before and
//!   after it, which agree (a texture black, grey or another one for a frame);
//! - steps: blocks that change from one frame to the next and stay so, while
//!   most of the picture does not move;
//! - lone blocks: blocks found in neither neighbour, wherever the camera may
//!   have moved them, while the neighbours' blocks there are found in each
//!   other (a blip while the camera moves).
//!
//! `VICTORY_POP_FOLDER=<folder>`: the three pictures around a frame with
//! blips (at most 200 times) are saved there, 640 wide; with
//! `VICTORY_POP_ALL=1` every frame's is.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock, mpsc};

use windows::Win32::Graphics::Direct3D12::*;

/// A small picture: so many pixels wide and high.
const WIDE: usize = 640;
const HIGH: usize = 360;
/// Pixels per side of a block.
const BLOCK: usize = 16;
/// A block changed: so much in the mean of its pixels' largest channel change.
const CHANGED: f32 = 10.0;
/// A block stayed.
const STAYED: f32 = 4.0;

/// `10-20,30-40` as seconds.
fn parse(text: &str) -> Vec<(f64, f64)> {
    text.split(',')
        .filter_map(|part| {
            let (from, to) = part.trim().split_once('-')?;
            Some((from.trim().parse().ok()?, to.trim().parse().ok()?))
        })
        .collect()
}

fn asked() -> &'static [(f64, f64)] {
    static ASKED: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    ASKED.get_or_init(|| std::env::var("VICTORY_POP_WATCH").map(|v| parse(&v)).unwrap_or_default())
}

/// Whether frames are watched now.
pub(crate) fn on() -> bool {
    let asked = asked();
    if asked.is_empty() {
        return false;
    }
    let now = crate::host::now_ms() as f64 / 1e3;
    asked.iter().any(|&(from, to)| now >= from && now < to)
}

/// A frame on its way back.
pub(crate) struct Taken {
    buffer: ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
}

struct Job {
    frame: u64,
    taken: Taken,
    /// The submission that copies it.
    flown: u64,
    ms: u64,
    causes: [u64; 4],
    phase: usize,
    /// `crate::list::totals` when the frame was presented.
    counts: [u64; 3],
}

/// Read-back buffers not in use.
static FREE: Mutex<Vec<Taken>> = Mutex::new(Vec::new());
/// How far a block is looked for in a neighbour's picture, in pixels of half
/// the small picture's size, and the side of a block there.
const REACH: i32 = 14;
const HALF_BLOCK: usize = BLOCK / 2;

/// A small picture at half its size, as brightness and two colour differences.
fn half(rgb: &[u8]) -> Vec<[i16; 3]> {
    let (w, h) = (WIDE / 2, HIGH / 2);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut s = [0i16; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let at = ((y * 2 + dy) * WIDE + x * 2 + dx) * 3;
                for (s, v) in s.iter_mut().zip(&rgb[at..at + 3]) {
                    *s += i16::from(*v);
                }
            }
            out.push(s.map(|s| s / 4));
        }
    }
    out
}

/// Per block of `a`: how much it differs from the place in `b` that is most
/// like it, as [`differ`] says it.
fn moved(a: &[[i16; 3]], b: &[[i16; 3]]) -> Vec<f32> {
    let (w, h) = (WIDE / 2, HIGH / 2);
    let (across, down) = (WIDE / BLOCK, HIGH / BLOCK);
    let mut out = vec![f32::MAX; across * down];
    for (n, best) in out.iter_mut().enumerate() {
        let (bx, by) = (n % across * HALF_BLOCK, n / across * HALF_BLOCK);
        let mut least = u32::MAX;
        for dy in -REACH..=REACH {
            let y0 = by as i32 + dy;
            if y0 < 0 || y0 as usize + HALF_BLOCK > h {
                continue;
            }
            for dx in -REACH..=REACH {
                let x0 = bx as i32 + dx;
                if x0 < 0 || x0 as usize + HALF_BLOCK > w {
                    continue;
                }
                let mut sum = 0u32;
                for row in 0..HALF_BLOCK {
                    let ra = &a[(by + row) * w + bx..][..HALF_BLOCK];
                    let rb = &b[(y0 as usize + row) * w + x0 as usize..][..HALF_BLOCK];
                    sum += ra
                        .iter()
                        .zip(rb)
                        .map(|(p, q)| (0..3).map(|i| p[i].abs_diff(q[i])).max().unwrap_or(0) as u32)
                        .sum::<u32>();
                    if sum >= least {
                        break;
                    }
                }
                least = least.min(sum);
            }
        }
        *best = least as f32 / (HALF_BLOCK * HALF_BLOCK) as f32;
    }
    out
}

/// Blocks of `now` found in neither neighbour, where `before` is found in
/// `after`.
fn lone(before: &[[i16; 3]], now: &[[i16; 3]], after: &[[i16; 3]]) -> Vec<usize> {
    let (one, two) = (moved(now, before), moved(now, after));
    let both = moved(before, after);
    // A cut on either side is none.
    if one.iter().filter(|&&d| d > CHANGED).count() * 4 > one.len()
        || two.iter().filter(|&&d| d > CHANGED).count() * 4 > two.len()
    {
        return Vec::new();
    }
    (0..one.len()).filter(|&n| one[n] > CHANGED && two[n] > CHANGED && both[n] < CHANGED / 2.0).collect()
}

/// Frames on their way.
static WAITING: AtomicU64 = AtomicU64::new(0);

/// Records the read-back of `texture` (in the copy-source state).
pub(crate) fn take(device: &ID3D12Device, list: &ID3D12GraphicsCommandList, texture: &ID3D12Resource) -> Option<Taken> {
    if !on() || WAITING.load(Relaxed) > 8 {
        return None;
    }
    let free = FREE.lock().unwrap_or_else(|e| e.into_inner()).pop();
    let Some(taken) = free else {
        let (buffer, footprint) = crate::capture::read_back(device, list, texture)?;
        return Some(Taken { buffer, footprint });
    };
    let to = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer the location never releases.
        pResource: unsafe { std::mem::transmute_copy(&taken.buffer) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: taken.footprint },
    };
    let from = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above.
        pResource: unsafe { std::mem::transmute_copy(texture) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    // SAFETY: an open list, live resources.
    unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
    Some(taken)
}

/// The list with `taken` went to the card as submission `flown` (0: it has
/// run). `frame`: the frame it shows.
pub(crate) fn sent(taken: Taken, flown: u64, frame: u64) {
    static THREAD: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    let Some(to) = THREAD.get_or_init(|| {
        let (to, from) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("pop watch".into()).spawn(move || watch(&from)).ok().map(|_| to)
    }) else {
        return;
    };
    WAITING.fetch_add(1, Relaxed);
    let job = Job {
        frame,
        taken,
        flown,
        ms: crate::host::now_ms(),
        causes: crate::descriptor::host::pop_causes(),
        phase: usize::from(crate::ab::pick("VICTORY_KEEP_SHOWN").unwrap_or(false))
            + 2 * usize::from(crate::ab::pick("VICTORY_UPLOAD_AHEAD").unwrap_or(false)),
        counts: crate::list::totals(),
    };
    let _ = to.send(job);
}

/// What was seen of the frames so far.
#[derive(Default)]
struct Seen {
    frames: u64,
    blips: u64,
    blip_frames: u64,
    steps: u64,
    still: u64,
    lone: u64,
    lone_frames: u64,
}

struct Picture {
    frame: u64,
    ms: u64,
    causes: [u64; 4],
    phase: usize,
    counts: [u64; 3],
    rgb: Vec<u8>,
    half: Vec<[i16; 3]>,
    /// Per block: how much it differs from the frame before.
    changed: Vec<f32>,
    saved: bool,
}

/// Per block, the mean of the pixels' largest channel change.
fn differ(a: &[u8], b: &[u8]) -> Vec<f32> {
    let (across, down) = (WIDE / BLOCK, HIGH / BLOCK);
    let mut out = vec![0.0; across * down];
    for (n, sum) in out.iter_mut().enumerate() {
        let (bx, by) = (n % across * BLOCK, n / across * BLOCK);
        let mut total = 0u32;
        for y in by..by + BLOCK {
            let at = (y * WIDE + bx) * 3;
            let (ra, rb) = (&a[at..at + BLOCK * 3], &b[at..at + BLOCK * 3]);
            total += ra
                .chunks_exact(3)
                .zip(rb.chunks_exact(3))
                .map(|(p, q)| p.iter().zip(q).map(|(x, y)| u32::from(x.abs_diff(*y))).max().unwrap_or(0))
                .sum::<u32>();
        }
        *sum = total as f32 / (BLOCK * BLOCK) as f32;
    }
    out
}

/// The blips of `now` between `before` and `after`, and its steps (the frame
/// before `before` had `quiet` changes): numbers of blocks. `None` for the
/// steps if the picture moves.
fn judge(before: &[u8], now: &[u8], after: &[u8], quiet: &[f32]) -> (usize, Option<usize>) {
    let (one, two, across) = (differ(before, now), differ(now, after), differ(before, after));
    let blips = (0..one.len())
        .filter(|&n| one[n] > CHANGED && two[n] > CHANGED && across[n] < STAYED && across[n] * 3.0 < one[n])
        .count();
    let still = one.iter().filter(|&&d| d < 3.0).count() * 10 >= one.len() * 7;
    let steps = still.then(|| {
        (0..one.len()).filter(|&n| one[n] > CHANGED && two[n] < STAYED && quiet.get(n).is_some_and(|&d| d < STAYED)).count()
    });
    (blips, steps)
}

/// `width * height` 8-bit RGBA (or BGRA) pixels made small.
fn small(rows: &[u8], width: usize, height: usize, pitch: usize, bgra: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(WIDE * HIGH * 3);
    for y in 0..HIGH {
        for x in 0..WIDE {
            let mut sum = [0u32; 3];
            for sy in 0..3 {
                for sx in 0..3 {
                    let px = ((x * 3 + sx) * 2 + 1) * width / (WIDE * 6);
                    let py = ((y * 3 + sy) * 2 + 1) * height / (HIGH * 6);
                    let p = &rows[py.min(height - 1) * pitch + px.min(width - 1) * 4..][..3];
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += u32::from(*v);
                    }
                }
            }
            let c = sum.map(|s| (s / 9) as u8);
            out.extend(if bgra { [c[2], c[1], c[0]] } else { c });
        }
    }
    out
}

fn folder() -> Option<&'static PathBuf> {
    static FOLDER: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOLDER
        .get_or_init(|| {
            let folder = PathBuf::from(std::env::var_os("VICTORY_POP_FOLDER")?);
            std::fs::create_dir_all(&folder).ok()?;
            Some(folder)
        })
        .as_ref()
}

fn save(p: &mut Picture) {
    if p.saved {
        return;
    }
    p.saved = true;
    if let Some(folder) = folder() {
        let png = crate::capture::png(WIDE as u32, HIGH as u32, &p.rgb);
        let _ = std::fs::write(folder.join(format!("{:07}-f{}.png", p.ms, p.frame)), png);
    }
}

fn watch(from: &mpsc::Receiver<Job>) {
    let all = std::env::var_os("VICTORY_POP_ALL").is_some_and(|v| v == "1");
    let mut last: VecDeque<Picture> = VecDeque::new();
    let mut seen: [Seen; 4] = Default::default();
    let mut told = crate::host::now_ms();
    let mut sets = 0;
    while let Ok(job) = from.recv() {
        while crate::host::submissions().1 < job.flown {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let f = job.taken.footprint.Footprint;
        let (rgba, bgra) = (matches!(f.Format.0, 27..=29), matches!(f.Format.0, 87 | 90 | 91));
        let mut data = std::ptr::null_mut();
        // SAFETY: the copy has run; the mapping spans the footprint's rows.
        let rgb = unsafe {
            if !(rgba || bgra) || job.taken.buffer.Map(0, None, Some(&mut data)).is_err() {
                WAITING.fetch_sub(1, Relaxed);
                continue;
            }
            let rows = std::slice::from_raw_parts(data.cast::<u8>(), f.RowPitch as usize * f.Height as usize);
            let rgb = small(rows, f.Width as usize, f.Height as usize, f.RowPitch as usize, bgra);
            job.taken.buffer.Unmap(0, None);
            rgb
        };
        FREE.lock().unwrap_or_else(|e| e.into_inner()).push(job.taken);
        WAITING.fetch_sub(1, Relaxed);
        // Frames that follow each other only.
        if last.back().is_some_and(|p| p.frame + 1 != job.frame) {
            last.clear();
        }
        let changed = last.back().map(|p| differ(&p.rgb, &rgb)).unwrap_or_default();
        let (frame, ms, causes, phase, counts) = (job.frame, job.ms, job.causes, job.phase, job.counts);
        let half = half(&rgb);
        // With every frame saved, what the title recorded between this
        // present and the one before, to set beside the pictures (a frame
        // that draws less or more than its neighbours: the title's own
        // choice of what to draw; the same counts and another picture: what
        // the draws read).
        if all && let Some(p) = last.back() {
            let d: Vec<u64> = counts.iter().zip(&p.counts).map(|(a, b)| a.saturating_sub(*b)).collect();
            let changed = changed.iter().filter(|&&c| c > CHANGED).count();
            tracing::info!(
                "pop watch frame {frame} at {:.3} s: {} draws, {} dispatches, {} indirect executions since the frame before; {changed} blocks of 16x16 changed",
                ms as f64 / 1e3,
                d[0],
                d[1],
                d[2]
            );
        }
        last.push_back(Picture { frame, ms, causes, phase, counts, rgb, half, changed, saved: false });
        if all && let Some(p) = last.back_mut() {
            save(p);
        }
        if last.len() > 4 {
            last.pop_front();
        }
        if let [first, before, now, after] = last.make_contiguous() {
            let (blips, steps) = judge(&before.rgb, &now.rgb, &after.rgb, &before.changed);
            let side = &mut seen[now.phase];
            side.frames += 1;
            side.blips += blips as u64;
            side.blip_frames += u64::from(blips > 0);
            side.still += u64::from(steps.is_some());
            side.steps += steps.unwrap_or(0) as u64;
            let lone = lone(&before.half, &now.half, &after.half);
            side.lone += lone.len() as u64;
            side.lone_frames += u64::from(!lone.is_empty());
            if blips > 0 || lone.len() > 1 || steps.is_some_and(|s| s > 0) {
                let since: Vec<u64> = now.causes.iter().zip(&first.causes).map(|(a, b)| a.saturating_sub(*b)).collect();
                tracing::info!(
                    "pop watch: frame {} at {:.2} s: {blips} blips, {steps:?} steps, lone blocks {lone:?}; in the two frames before it [made null for another texture, kept, neutral, stale] {since:?}",
                    now.frame,
                    now.ms as f64 / 1e3
                );
            }
            if (blips > 0 || lone.len() > 1 || steps.is_some_and(|s| s > 2)) && sets < 200 {
                sets += 1;
                save(before);
                save(now);
                save(after);
            }
        }
        if crate::host::now_ms().saturating_sub(told) > 5000 {
            told = crate::host::now_ms();
            for (n, s) in seen.iter().enumerate().filter(|(_, s)| s.frames > 0) {
                tracing::info!(
                    "pop watch, since the start, A/B VICTORY_KEEP_SHOWN {} VICTORY_UPLOAD_AHEAD {}: {} frames, blips in {} of them ({} blocks of 16x16), {} frames whose picture stood still with {} blocks stepped, lone blocks in {} frames ({} blocks)",
                    n & 1,
                    n >> 1,
                    s.frames,
                    s.blip_frames,
                    s.blips,
                    s.still,
                    s.steps,
                    s.lone_frames,
                    s.lone
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(color: u8) -> Vec<u8> {
        vec![color; WIDE * HIGH * 3]
    }

    fn patch(p: &mut [u8], bx: usize, by: usize, color: u8) {
        for y in by * BLOCK..(by + 1) * BLOCK {
            let at = (y * WIDE + bx * BLOCK) * 3;
            p[at..at + BLOCK * 3].fill(color);
        }
    }

    #[test]
    fn one_frame_otherwise_is_a_blip_and_a_lasting_change_a_step() {
        let (a, mut b) = (picture(100), picture(100));
        patch(&mut b, 3, 4, 0);
        let quiet = vec![0.0; (WIDE / BLOCK) * (HIGH / BLOCK)];
        assert_eq!(judge(&a, &b, &a, &quiet), (1, Some(0)));
        assert_eq!(judge(&a, &b, &b, &quiet), (0, Some(1)));
        assert_eq!(judge(&a, &a, &a, &quiet), (0, Some(0)));
        // All of it changes: a cut, no step.
        assert_eq!(judge(&a, &picture(0), &picture(0), &quiet), (0, None));
        // What moved just before is no step either.
        assert_eq!(judge(&a, &b, &b, &vec![9.0; quiet.len()]), (0, Some(0)));
    }

    #[test]
    fn a_block_one_frame_alone_shows_is_found_while_the_camera_moves() {
        // A picture of noise, moved by 6 pixels a frame.
        let at = |shift: usize| -> Vec<u8> {
            (0..HIGH)
                .flat_map(|y| (0..WIDE).map(move |x| ((x + shift) / 8 * 37 + y / 8 * 91) as u8))
                .flat_map(|v| [v, v.wrapping_mul(3), 255 - v])
                .collect()
        };
        let (a, mut b, c) = (at(0), at(8), at(16));
        assert!(lone(&half(&a), &half(&b), &half(&c)).is_empty());
        patch(&mut b, 20, 10, 128);
        assert_eq!(lone(&half(&a), &half(&b), &half(&c)), vec![10 * (WIDE / BLOCK) + 20]);
    }

    #[test]
    fn times_are_read_and_pictures_made_small() {
        assert_eq!(parse("10-20, 30.5-40"), vec![(10.0, 20.0), (30.5, 40.0)]);
        assert!(parse("x").is_empty());
        let rows: Vec<u8> = [1u8, 2, 3, 255].repeat(1280 * 720);
        let out = small(&rows, 1280, 720, 1280 * 4, true);
        assert_eq!((out.len(), &out[..3]), (WIDE * HIGH * 3, &[3u8, 2, 1][..]));
    }
}
