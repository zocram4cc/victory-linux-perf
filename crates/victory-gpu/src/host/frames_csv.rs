//! `VICTORY_FRAMES_CSV=<file>`: one row a presented frame, for reading a run
//! afterwards (the playtest launcher puts it in the run's folder). Off, and
//! free, when the switch is unset; in every build, not only diagnostics ones.
//!
//! A row is what the counters of the frame line and the panel moved by since
//! the frame before (they go back to 0 at each frame line, so the frame
//! after one reads from 0). The presenting thread only formats the row; a
//! thread of its own writes the file and flushes it once a second, so a
//! crash loses at most the last second.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::Host;

/// The columns. Times in milliseconds, sizes in megabytes (10^6 bytes).
const HEADER: &str = "frame,t_ms,frame_ms,submit_busy_ms,gpu_idle_ms,gpu_idle_behind_us_ms,\
waits_for_gpu_ms,title_copy_ms,title_copy_mb,texture_refresh_ms,texture_mb,copy_back_ms,\
copy_back_mb,card_up_mb,draws,draws_dropped,pipelines_queued,pipelines_made,vram_used_mb";

/// VRAM is asked of DXGI only every this many frames (the column is empty
/// between).
const VRAM_EVERY: u64 = 30;

const COUNTERS: usize = 14;

/// The counters as of now, in [`HEADER`]'s order from `submit_busy_ms`.
fn counters() -> [u64; COUNTERS] {
    let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
    [
        super::worked(),
        get(&super::CARD_CLOCK_US[0]),
        get(&super::CARD_CLOCK_US[1]),
        get(&super::CARD_CLOCK_US[2]),
        get(&super::PREPARING[0]),
        get(&super::PREPARING[1]),
        get(&super::PREPARING[2]),
        get(&super::PREPARING[3]),
        get(&super::COPY_BACK[0]),
        get(&super::COPY_BACK[1]),
        get(&crate::shadow::CARD_STATS[0]),
        crate::draw::drawn(),
        crate::draw::dropped_now(),
        crate::pipeline::made(),
    ]
}

struct State {
    rows: mpsc::Sender<String>,
    start: Instant,
    last: Instant,
    frame: u64,
    before: [u64; COUNTERS],
}

/// `None` inside: asked, and off (unset, or the file could not be made).
static STATE: Mutex<Option<Option<State>>> = Mutex::new(None);

fn open() -> Option<State> {
    let path = std::env::var_os("VICTORY_FRAMES_CSV").filter(|p| !p.is_empty())?;
    let file = match std::fs::File::create(&path) {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!(error = %e, path = %std::path::Path::new(&path).display(), "no frames CSV");
            return None;
        }
    };
    let (rows, received) = mpsc::channel::<String>();
    let spawned = std::thread::Builder::new().name("frames csv".into()).spawn(move || {
        let mut out = std::io::BufWriter::with_capacity(1 << 16, file);
        let _ = writeln!(out, "{HEADER}");
        loop {
            match received.recv_timeout(Duration::from_secs(1)) {
                Ok(row) => {
                    let _ = writeln!(out, "{row}");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = out.flush();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = out.flush();
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "no frames CSV: its thread could not start");
        return None;
    }
    tracing::info!(path = %std::path::Path::new(&path).display(), "frames CSV");
    let now = Instant::now();
    Some(State { rows, start: now, last: now, frame: 0, before: counters() })
}

/// A frame was presented on `host`.
pub(super) fn frame(host: &Host) {
    let Ok(mut state) = STATE.lock() else { return };
    let Some(state) = state.get_or_insert_with(open).as_mut() else { return };
    let now = Instant::now();
    let is = counters();
    let mut moved = [0u64; COUNTERS];
    for (i, m) in moved.iter_mut().enumerate() {
        // Back to 0 at a frame line: what came since then.
        *m = if is[i] >= state.before[i] { is[i] - state.before[i] } else { is[i] };
    }
    state.before = is;
    let frame_ms = if state.frame == 0 { 0.0 } else { (now - state.last).as_secs_f64() * 1e3 };
    state.last = now;
    let ms = |us: u64| us as f64 / 1e3;
    let mb = |bytes: u64| bytes as f64 / 1e6;
    let vram = if state.frame % VRAM_EVERY == 0 {
        host.video_memory().map_or(String::new(), |[_, used, _, _]| format!("{:.0}", mb(used)))
    } else {
        String::new()
    };
    let row = format!(
        "{},{:.1},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{},{},{},{}",
        state.frame,
        (now - state.start).as_secs_f64() * 1e3,
        frame_ms,
        ms(moved[0]),
        ms(moved[1]),
        ms(moved[2]),
        ms(moved[3]),
        ms(moved[4]),
        mb(moved[5]),
        ms(moved[6]),
        mb(moved[7]),
        ms(moved[8]),
        mb(moved[9]),
        mb(moved[10]),
        moved[11],
        moved[12],
        crate::pipeline::making(),
        moved[13],
        vram,
    );
    state.frame += 1;
    let _ = state.rows.send(row);
}
