//! When the recorder takes a capture: the trigger file, and the gate that
//! keeps captures apart (one at a time, a rest after each).

use std::path::Path;
use std::time::{Duration, Instant};

/// The file that asks for a capture, in the recorder's folder.
pub(super) const GO: &str = "go";

/// Whether the trigger file is in `folder`; if it is, it is taken away (so
/// the next one asks again). One that cannot be removed is renamed; one
/// that can be neither does not count (else it would ask forever).
pub(super) fn take(folder: &Path) -> bool {
    let go = folder.join(GO);
    if !go.exists() {
        return false;
    }
    match std::fs::remove_file(&go) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            let to = folder.join(format!("{GO}.taken-{stamp}"));
            match std::fs::rename(&go, &to) {
                Ok(()) => true,
                Err(_) => {
                    tracing::warn!(error = %e, "frame recorder: the trigger file cannot be taken away; it is not followed");
                    false
                }
            }
        }
    }
}

/// The automatic trigger for stretched geometry (`VICTORY_RECORD_SHEETS=1`,
/// see `crate::sheets`): a frame is captured when at least `min` draws of
/// the pre-skinned programs noted a vertex that jumps more than `jump` (clip
/// units) from both neighbours, at most `most` times a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sheets {
    pub jump: f32,
    pub min: u32,
    pub most: u32,
}

impl Sheets {
    /// Of the largest jumps the draws noted since the last present: how
    /// many are over the line, if they are enough to ask for a capture.
    pub(super) fn seen(&self, jumps: &[f32]) -> Option<u32> {
        let draws = jumps.iter().filter(|&&j| j > self.jump).count() as u32;
        (draws >= self.min.max(1)).then_some(draws)
    }
}

/// What the gate says to a request for a capture.
#[derive(Debug, PartialEq)]
pub(super) enum Verdict {
    /// Take it: the capture's number (1 for the first).
    Take(u32),
    /// A capture is under way.
    Busy,
    /// The last one ended too recently: this long to go.
    Resting(Duration),
    /// Automatic captures: as many as a run may take were taken.
    Spent,
}

/// Keeps captures apart: one at a time, and `gap` after each one ends (the
/// title catches up on the time it was frozen meanwhile, and a frame of that
/// is not the one asked for).
#[derive(Debug)]
pub(super) struct Gate {
    gap: Duration,
    busy: bool,
    ended: Option<Instant>,
    taken: u32,
    /// Of them, asked for by the automatic trigger.
    auto: u32,
    /// An ask (the trigger file) that came while a capture was under way or
    /// in the rest after one: followed as soon as it may be. One is kept.
    queued: bool,
}

impl Gate {
    pub(super) fn new(gap: Duration) -> Gate {
        Gate {
            gap,
            busy: false,
            ended: None,
            taken: 0,
            auto: 0,
            queued: false,
        }
    }

    /// Captures begun so far.
    pub(super) fn taken(&self) -> u32 {
        self.taken
    }

    /// A capture is asked for at `now` (the trigger file, the time set): one
    /// not taken now is kept for later ([`Gate::queued`]).
    pub(super) fn ask(&mut self, now: Instant) -> Verdict {
        let v = self.try_take(now);
        self.queued = !matches!(v, Verdict::Take(_));
        v
    }

    /// Keeps an ask for later (one at most).
    pub(super) fn queue(&mut self) {
        self.queued = true;
    }

    /// Whether an ask is kept for later.
    pub(super) fn queued(&self) -> bool {
        self.queued
    }

    fn try_take(&mut self, now: Instant) -> Verdict {
        if self.busy {
            return Verdict::Busy;
        }
        if let Some(ended) = self.ended {
            let since = now.saturating_duration_since(ended);
            if since < self.gap {
                return Verdict::Resting(self.gap - since);
            }
        }
        self.busy = true;
        self.taken += 1;
        Verdict::Take(self.taken)
    }

    /// The automatic trigger asks for a capture at `now`; it may take `most`
    /// a run.
    pub(super) fn ask_auto(&mut self, now: Instant, most: u32) -> Verdict {
        if self.auto >= most {
            return Verdict::Spent;
        }
        let v = self.try_take(now);
        if matches!(v, Verdict::Take(_)) {
            self.auto += 1;
        }
        v
    }

    /// The capture under way ended (written or failed) at `now`.
    pub(super) fn end(&mut self, now: Instant) {
        if self.busy {
            self.busy = false;
            self.ended = Some(now);
        }
    }
}

/// How long a capture may hold the title: the title has a watchdog of its
/// own (about 30 s) and a capture past it ended a run. Checked between the
/// capture's steps; past it the capture stops and says where.
#[derive(Clone, Copy, Debug)]
pub(super) struct Deadline {
    began: Instant,
    limit: Option<Duration>,
}

impl Deadline {
    /// From `began` on, `limit` (none: no limit).
    pub(super) fn new(began: Instant, limit: Option<Duration>) -> Deadline {
        Deadline { began, limit }
    }

    /// Whether it has passed at `now`.
    pub(super) fn passed(&self, now: Instant) -> bool {
        self.limit.is_some_and(|l| now.saturating_duration_since(self.began) > l)
    }

    /// An error saying so, if it has passed at `now` during `what`.
    pub(super) fn check(&self, now: Instant, what: &str) -> Result<(), String> {
        match self.limit {
            Some(l) if self.passed(now) => Err(format!(
                "the capture passed its limit of {:.0} s (VICTORY_RECORD_LIMIT) during {what}, after {:.1} s: stopped, so that the title's own watchdog does not end the run",
                l.as_secs_f64(),
                now.saturating_duration_since(self.began).as_secs_f64()
            )),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_are_one_at_a_time_and_rest_between() {
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        let mut gate = Gate::new(5 * s);
        assert_eq!(gate.ask(t0), Verdict::Take(1));
        // Asked again while the first is under way: not taken, not queued.
        assert_eq!(gate.ask(t0 + s), Verdict::Busy);
        assert_eq!(gate.ask(t0 + 40 * s), Verdict::Busy);
        gate.end(t0 + 40 * s);
        // Within the gap after it ended.
        assert_eq!(gate.ask(t0 + 42 * s), Verdict::Resting(3 * s));
        assert_eq!(gate.taken(), 1);
        // After it: the next one.
        assert_eq!(gate.ask(t0 + 45 * s), Verdict::Take(2));
        gate.end(t0 + 60 * s);
        assert_eq!(gate.ask(t0 + 70 * s), Verdict::Take(3));
        assert_eq!(gate.taken(), 3);
    }

    #[test]
    fn an_end_without_a_capture_changes_nothing() {
        let t0 = Instant::now();
        let mut gate = Gate::new(Duration::from_secs(5));
        gate.end(t0);
        assert_eq!(gate.ask(t0), Verdict::Take(1));
    }

    #[test]
    fn no_gap_takes_the_next_at_once() {
        let t0 = Instant::now();
        let mut gate = Gate::new(Duration::ZERO);
        assert_eq!(gate.ask(t0), Verdict::Take(1));
        gate.end(t0);
        assert_eq!(gate.ask(t0), Verdict::Take(2));
    }

    #[test]
    fn automatic_captures_are_capped_and_wait_their_turn() {
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        let mut gate = Gate::new(5 * s);
        assert_eq!(gate.ask_auto(t0, 2), Verdict::Take(1));
        assert_eq!(gate.ask_auto(t0, 2), Verdict::Busy);
        gate.end(t0 + 10 * s);
        assert_eq!(gate.ask_auto(t0 + 11 * s, 2), Verdict::Resting(4 * s));
        // A manual one in between does not count against the cap.
        assert_eq!(gate.ask(t0 + 20 * s), Verdict::Take(2));
        gate.end(t0 + 30 * s);
        assert_eq!(gate.ask_auto(t0 + 40 * s, 2), Verdict::Take(3));
        gate.end(t0 + 50 * s);
        assert_eq!(gate.ask_auto(t0 + 60 * s, 2), Verdict::Spent);
        // The cap is of automatic ones only.
        assert_eq!(gate.ask(t0 + 60 * s), Verdict::Take(4));
    }

    #[test]
    fn sheets_are_draws_over_the_line() {
        let s = Sheets { jump: 1.0, min: 2, most: 2 };
        assert_eq!(s.seen(&[0.5, 0.9, 1.0]), None, "small jumps are not counted");
        assert_eq!(s.seen(&[3.0]), None, "under the minimum");
        assert_eq!(s.seen(&[3.0, 0.2, 1.5]), Some(2));
        let any = Sheets { jump: 0.0, min: 0, most: 2 };
        assert_eq!(any.seen(&[]), None, "nothing seen is never enough");
        assert_eq!(any.seen(&[0.3]), Some(1));
    }

    #[test]
    fn an_ask_in_the_rest_or_during_a_capture_is_kept_once() {
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        let mut gate = Gate::new(5 * s);
        assert_eq!(gate.ask(t0), Verdict::Take(1));
        assert!(!gate.queued());
        // During the capture.
        assert_eq!(gate.ask(t0 + s), Verdict::Busy);
        assert!(gate.queued());
        gate.end(t0 + 10 * s);
        // In the rest: still kept, not taken.
        assert_eq!(gate.ask(t0 + 12 * s), Verdict::Resting(3 * s));
        assert!(gate.queued());
        // After it: taken, and no longer kept.
        assert_eq!(gate.ask(t0 + 15 * s), Verdict::Take(2));
        assert!(!gate.queued());
        // The automatic trigger does not keep its asks.
        gate.end(t0 + 20 * s);
        assert_eq!(gate.ask_auto(t0 + 21 * s, 2), Verdict::Resting(4 * s));
        assert!(!gate.queued());
        gate.queue();
        assert!(gate.queued());
    }

    #[test]
    fn a_capture_past_its_limit_is_stopped() {
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        let d = Deadline::new(t0, Some(20 * s));
        assert!(d.check(t0 + 19 * s, "reading").is_ok());
        assert!(d.check(t0 + 20 * s, "reading").is_ok(), "the limit itself is allowed");
        let e = d.check(t0 + 21 * s, "reading the card's memory").unwrap_err();
        assert!(e.contains("20 s") && e.contains("reading the card's memory") && e.contains("21.0 s"), "{e}");
        assert!(d.passed(t0 + 21 * s));
        let none = Deadline::new(t0, None);
        assert!(none.check(t0 + 3600 * s, "anything").is_ok());
    }

    #[test]
    fn the_trigger_file_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!take(dir.path()));
        std::fs::write(dir.path().join(GO), "").unwrap();
        assert!(take(dir.path()));
        assert!(!dir.path().join(GO).exists());
        // Until it is made again.
        assert!(!take(dir.path()));
        std::fs::write(dir.path().join(GO), "").unwrap();
        assert!(take(dir.path()));
        assert!(!take(dir.path()));
    }

    #[test]
    fn a_trigger_folder_that_is_not_there_asks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!take(&dir.path().join("none")));
    }
}
