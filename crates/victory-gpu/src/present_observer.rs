//! Bounded host Present timing for the matched Series X replay.

use std::fmt::Write;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering::Relaxed},
};
use std::time::Instant;

const ROOM_CAPTURE: u64 = 224;
const LIMIT: u64 = 366;

static ORIGIN: AtomicU64 = AtomicU64::new(0);
static PREVIOUS_DRAWS: AtomicU64 = AtomicU64::new(0);
static PRESENTS: Mutex<Option<Recording>> = Mutex::new(None);

struct Recording {
    start: Instant,
    rows: Vec<Present>,
    saved: bool,
}

struct Present {
    frame: u64,
    micros: u128,
    width: u32,
    height: u32,
    result: i32,
    draws: u64,
    dispatches: u64,
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| crate::switch::on("VICTORY_PRESENT_OBSERVER"))
}

pub(super) fn submitted(frame: u64) {
    if !enabled() || ORIGIN.load(Relaxed) != 0 {
        return;
    }
    let draws = crate::draw::drawn();
    let previous = PREVIOUS_DRAWS.swap(draws, Relaxed);
    if draws.saturating_sub(previous) >= 500
        && ORIGIN.compare_exchange(0, frame, Relaxed, Relaxed).is_ok()
    {
        tracing::info!(frame, "the first frame of 500 draws or more: common present observer origin");
    }
}

pub(super) fn shown(frame: u64, width: u32, height: u32, result: i32) {
    let origin = ORIGIN.load(Relaxed);
    if origin == 0 || frame < origin {
        return;
    }
    let now = Instant::now();
    let mut holder = PRESENTS.lock().unwrap_or_else(|error| error.into_inner());
    let data = holder.get_or_insert_with(|| Recording {
        start: now,
        rows: Vec::with_capacity(LIMIT as usize + 1),
        saved: false,
    });
    if data.saved {
        return;
    }
    if data.rows.len() <= LIMIT as usize {
        data.rows.push(Present {
            frame,
            micros: now.saturating_duration_since(data.start).as_micros(),
            width,
            height,
            result,
            draws: crate::draw::drawn(),
            dispatches: crate::draw::dispatched(),
        });
    }
    if frame - origin == ROOM_CAPTURE {
        tracing::info!(frame, "common present observer room view");
    }
    if frame - origin >= LIMIT {
        let mut text = format!("# world={origin}\nframe\tus\twidth\theight\thr\tdraws\tdispatches\n");
        for row in &data.rows {
            let _ = writeln!(text, "{}\t{}\t{}\t{}\t{}\t{}\t{}", row.frame, row.micros,
                row.width, row.height, row.result, row.draws, row.dispatches);
        }
        match std::fs::write("room-presents.tsv", text) {
            Ok(()) => tracing::info!(frame, "common present observer completed"),
            Err(error) => tracing::warn!(%error, "cannot save present sample"),
        }
        data.saved = true;
    }
}
