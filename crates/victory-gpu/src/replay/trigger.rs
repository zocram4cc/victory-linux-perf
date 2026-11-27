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

/// What the gate says to a request for a capture.
#[derive(Debug, PartialEq)]
pub(super) enum Verdict {
    /// Take it: the capture's number (1 for the first).
    Take(u32),
    /// A capture is under way.
    Busy,
    /// The last one ended too recently: this long to go.
    Resting(Duration),
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
}

impl Gate {
    pub(super) fn new(gap: Duration) -> Gate {
        Gate {
            gap,
            busy: false,
            ended: None,
            taken: 0,
        }
    }

    /// Captures begun so far.
    pub(super) fn taken(&self) -> u32 {
        self.taken
    }

    /// A capture is asked for at `now`.
    pub(super) fn ask(&mut self, now: Instant) -> Verdict {
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

    /// The capture under way ended (written or failed) at `now`.
    pub(super) fn end(&mut self, now: Instant) {
        if self.busy {
            self.busy = false;
            self.ended = Some(now);
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
