//! A record of every presented frame, for benchmarks (`VICTORY_FRAME_CSV=<file>`,
//! diagnostics builds; unset: nothing is recorded).
//!
//! One line per present: the time since the start, the frame's time, and of
//! the frame, what the counters kept since the start grew by: the submitting
//! thread's preparing and its waits for the card, title-memory copies and
//! texture refreshes (time, bytes), copying back what shaders wrote (time,
//! bytes), waits for the write trackers' locks (all threads), and draws.
//! Counters land when their work ends, so a frame's line holds what ended
//! between its present and the one before.

// The record is made in diagnostics builds only; its line format is tested in all.
#![cfg_attr(not(feature = "diagnostics"), allow(dead_code))]

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;

/// The columns, in order.
pub(crate) const HEADER: &str =
    "t_ms,frame_ms,prepare_ms,gpu_wait_ms,copy_ms,copy_mb,texture_ms,texture_mb,back_ms,back_mb,lock_wait_ms,draws";

/// The counters a line is made of, since the start.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct Totals {
    pub prepare_us: u64,
    pub gpu_us: u64,
    pub copy_us: u64,
    pub copy_bytes: u64,
    pub texture_us: u64,
    pub texture_bytes: u64,
    pub back_us: u64,
    pub back_bytes: u64,
    pub lock_wait_ns: u64,
    pub draws: u64,
}

/// One line: `now` and `last` the totals at this present and the one
/// before, `t_us` the time since the start and `frame_us` since that present.
pub(crate) fn line(t_us: u64, frame_us: u64, now: &Totals, last: &Totals) -> String {
    let d = |a: u64, b: u64| a.saturating_sub(b) as f64;
    format!(
        "{:.3},{:.3},{:.3},{:.3},{:.3},{:.4},{:.3},{:.4},{:.3},{:.4},{:.3},{}",
        t_us as f64 / 1e3,
        frame_us as f64 / 1e3,
        d(now.prepare_us, last.prepare_us) / 1e3,
        d(now.gpu_us, last.gpu_us) / 1e3,
        d(now.copy_us, last.copy_us) / 1e3,
        d(now.copy_bytes, last.copy_bytes) / 1e6,
        d(now.texture_us, last.texture_us) / 1e3,
        d(now.texture_bytes, last.texture_bytes) / 1e6,
        d(now.back_us, last.back_us) / 1e3,
        d(now.back_bytes, last.back_bytes) / 1e6,
        d(now.lock_wait_ns, last.lock_wait_ns) / 1e6,
        now.draws.saturating_sub(last.draws),
    )
}

fn totals() -> Totals {
    let s = &super::TOTAL_SUBMISSION_US;
    let p = &super::TOTAL_PREPARING;
    let b = &super::TOTAL_COPY_BACK;
    #[cfg(feature = "diagnostics")]
    let lock_wait_ns = crate::watch::WAIT_NS_TOTAL.iter().map(|w| w.load(Relaxed)).sum();
    #[cfg(not(feature = "diagnostics"))]
    let lock_wait_ns = 0;
    Totals {
        prepare_us: s[0].load(Relaxed),
        gpu_us: s[1].load(Relaxed),
        copy_us: p[0].load(Relaxed),
        copy_bytes: p[1].load(Relaxed),
        texture_us: p[2].load(Relaxed),
        texture_bytes: p[3].load(Relaxed),
        back_us: b[0].load(Relaxed),
        back_bytes: b[1].load(Relaxed),
        lock_wait_ns,
        draws: crate::draw::drawn() as u64,
    }
}

struct Record {
    out: std::io::BufWriter<std::fs::File>,
    start: std::time::Instant,
    last_at: std::time::Instant,
    last: Totals,
    lines: u64,
}

static RECORD: Mutex<Option<Option<Record>>> = Mutex::new(None);

/// Counts a present (the presenting thread, once the swap chain took it).
pub(crate) fn presented() {
    let mut record = RECORD.lock().unwrap_or_else(|e| e.into_inner());
    let record = record.get_or_insert_with(|| {
        let path = std::env::var_os("VICTORY_FRAME_CSV").filter(|p| !p.is_empty())?;
        let file = std::fs::File::create(&path)
            .inspect_err(|e| tracing::warn!("VICTORY_FRAME_CSV: {e}"))
            .ok()?;
        let mut out = std::io::BufWriter::new(file);
        let _ = writeln!(out, "{HEADER}");
        let now = std::time::Instant::now();
        Some(Record { out, start: now, last_at: now, last: totals(), lines: 0 })
    });
    let Some(r) = record else { return };
    let now = std::time::Instant::now();
    let t = totals();
    let text = line(
        now.duration_since(r.start).as_micros() as u64,
        now.duration_since(r.last_at).as_micros() as u64,
        &t,
        &r.last,
    );
    let _ = writeln!(r.out, "{text}");
    (r.last, r.last_at) = (t, now);
    r.lines += 1;
    if r.lines % 30 == 0 {
        let _ = r.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{HEADER, Totals, line};

    /// A line holds what each counter grew by since the last present, in
    /// the header's order and units.
    #[test]
    fn a_line_holds_what_each_counter_grew_by() {
        let last = Totals { prepare_us: 1_000, draws: 10, lock_wait_ns: 5_000_000, ..Totals::default() };
        let now = Totals {
            prepare_us: 31_500,
            gpu_us: 4_000,
            copy_us: 2_000,
            copy_bytes: 3_000_000,
            texture_us: 500,
            texture_bytes: 250_000,
            back_us: 1_250,
            back_bytes: 1_500_000,
            lock_wait_ns: 7_500_000,
            draws: 1_210,
        };
        let text = line(2_000_000, 45_000, &now, &last);
        assert_eq!(text, "2000.000,45.000,30.500,4.000,2.000,3.0000,0.500,0.2500,1.250,1.5000,2.500,1200");
        assert_eq!(text.split(',').count(), HEADER.split(',').count());
    }
}
