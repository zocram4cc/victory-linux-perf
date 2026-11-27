//! Scripted input (`VICTORY_KEYS`) and the recording of a player's input
//! (`VICTORY_KEYS_RECORD`), on one clock: from the first time input is read.
//!
//! A recording holds, one a line:
//!
//! - `<from>-<to>:<key>`: a keyboard key held (`vkXX` for a key
//!   `VICTORY_KEYS` has no name for);
//! - `<from>-<to>:pad<slot>:<key>`: a pad button held, named by the key the
//!   keyboard's stand-in pad has for that button (`Enter` for A, `Esc` for
//!   the menu button, and so on: [`super::FALLBACK_BUTTONS`]);
//! - `<t> pad<slot> axes <lx> <ly> <rx> <ry> <lt> <rt>`: the sticks (-1 to
//!   1, up positive) and triggers (0 to 1) of a pad, whenever one moved by
//!   [`AXIS_STEP`] or more.
//!
//! `scripts/victory-route.py` turns it into a route (`scripts/bench/*.keys`).
//! A route holds keys only: a stick replays as full deflection on W/A/S/D
//! (I/J/K/L for the right one), a trigger as Z or C fully pressed.

use std::fmt::Write as _;
use std::time::Duration;

use super::{FALLBACK_BUTTONS, PADS, PadState};

/// The names of keys `VICTORY_KEYS` takes besides letters and digits.
const NAMED: [(&str, u8); 11] = [
    ("Enter", 0x0D),
    ("Esc", 0x1B),
    ("Tab", 0x09),
    ("Back", 0x08),
    ("Up", 0x26),
    ("Down", 0x28),
    ("Left", 0x25),
    ("Right", 0x27),
    ("Space", 0x20),
    ("Shift", 0xA0),
    ("Ctrl", 0xA2),
];

/// The virtual key `VICTORY_KEYS` means by `name`.
pub(super) fn key_of(name: &str) -> Option<u8> {
    let name = name.trim();
    if let Some(&(_, vk)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(vk);
    }
    match name.as_bytes() {
        [c] if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase()),
        _ => None,
    }
}

/// The name `VICTORY_KEYS` has for a virtual key, if it has one.
pub(super) fn name_of(vk: u8) -> Option<String> {
    if let Some(&(name, _)) = NAMED.iter().find(|(_, k)| *k == vk) {
        return Some(name.to_owned());
    }
    (vk.is_ascii_digit() || vk.is_ascii_uppercase()).then(|| char::from(vk).to_string())
}

/// `VICTORY_KEYS`: `<seconds>:<key>` (held 0.2 s) or `<from>-<to>:<key>`,
/// separated by commas or new lines; what does not parse is left out.
pub(super) fn parse(list: &str) -> Vec<(Duration, Duration, u8)> {
    list.split([',', '\n'])
        .filter_map(|entry| {
            let (at, key) = entry.split_once(':')?;
            let vk = key_of(key)?;
            let time = |t: &str| Duration::try_from_secs_f64(t.trim().parse().ok()?).ok();
            let (from, to) = match at.split_once('-') {
                Some((from, to)) => (time(from)?, time(to)?),
                None => (time(at)?, time(at)? + Duration::from_millis(200)),
            };
            Some((from, to, vk))
        })
        .collect()
}

/// A stick or trigger that moved this far since the last line is written
/// again.
pub(super) const AXIS_STEP: f32 = 0.02;

/// What the recording has seen and not written yet: keys held since when,
/// per pad the buttons held since when and the axes last written.
#[derive(Default)]
pub(super) struct Recorder {
    keys: Vec<(u8, Duration)>,
    buttons: [Vec<(u32, Duration)>; PADS],
    axes: [Option<[f32; 6]>; PADS],
}

/// The end written for a hold: a key let go within a poll is still held a
/// moment.
fn until(from: Duration, now: Duration) -> f64 {
    now.as_secs_f64().max(from.as_secs_f64() + 0.05)
}

impl Recorder {
    /// The first line of a recording.
    pub(super) const HEADER: &str = "# VICTORY_KEYS_RECORD: <from>-<to>:<key> keyboard, <from>-<to>:pad<slot>:<key> pad \
         buttons by the stand-in pad's keys, <t> pad<slot> axes <lx> <ly> <rx> <ry> <lt> <rt>; seconds on the \
         VICTORY_KEYS clock. scripts/victory-route.py makes a route of it.\n";

    /// The keyboard's keys held at `now` (none without focus): the holds
    /// that ended are written to `out`.
    pub(super) fn keys(&mut self, now: Duration, held: &[u8], out: &mut String) {
        self.keys.retain(|&(vk, from)| {
            if held.contains(&vk) {
                return true;
            }
            let name = name_of(vk).unwrap_or_else(|| format!("vk{vk:02X}"));
            let _ = writeln!(out, "{:.3}-{:.3}:{name}", from.as_secs_f64(), until(from, now));
            false
        });
        for &vk in held {
            if !self.keys.iter().any(|&(k, _)| k == vk) {
                self.keys.push((vk, now));
            }
        }
    }

    /// Host pad `slot` at `now` (`None`: no pad there): button holds that
    /// ended, and its axes where they moved, are written to `out`.
    pub(super) fn pad(&mut self, now: Duration, slot: usize, pad: Option<&PadState>, out: &mut String) {
        let buttons = pad.map_or(0, |p| p.buttons);
        self.buttons[slot].retain(|&(bit, from)| {
            if buttons & bit != 0 {
                return true;
            }
            if let Some(name) = FALLBACK_BUTTONS.iter().find(|(_, b)| *b == bit).and_then(|&(vk, _)| name_of(vk)) {
                let _ = writeln!(out, "{:.3}-{:.3}:pad{slot}:{name}", from.as_secs_f64(), until(from, now));
            }
            false
        });
        for &(_, bit) in &FALLBACK_BUTTONS {
            if buttons & bit != 0 && !self.buttons[slot].iter().any(|&(b, _)| b == bit) {
                self.buttons[slot].push((bit, now));
            }
        }
        let axes = pad.map(|p| [p.left_x, p.left_y, p.right_x, p.right_y, p.left_trigger, p.right_trigger]);
        let last = self.axes[slot];
        let moved = match (last, axes) {
            (None, Some(a)) => a.iter().any(|v| v.abs() >= AXIS_STEP),
            (Some(l), Some(a)) => l.iter().zip(&a).any(|(l, a)| (l - a).abs() >= AXIS_STEP),
            // A pad gone with its sticks pushed: at rest from now.
            (Some(l), None) => l.iter().any(|v| v.abs() >= AXIS_STEP),
            (None, None) => false,
        };
        if moved {
            let a = axes.unwrap_or_default();
            let _ = writeln!(
                out,
                "{:.3} pad{slot} axes {:.3} {:.3} {:.3} {:.3} {:.3} {:.3}",
                now.as_secs_f64(),
                a[0],
                a[1],
                a[2],
                a[3],
                a[4],
                a[5]
            );
            self.axes[slot] = axes;
        } else if axes.is_none() {
            self.axes[slot] = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: f64) -> Duration {
        Duration::from_secs_f64(secs)
    }

    /// Every key the recording names is a key the script takes, and back.
    #[test]
    fn recorded_names_are_script_keys() {
        for vk in 0..=255u8 {
            if let Some(name) = name_of(vk) {
                assert_eq!(key_of(&name), Some(vk), "{name}");
            }
        }
        for &(vk, _) in &FALLBACK_BUTTONS {
            assert!(name_of(vk).is_some(), "stand-in pad key {vk:#x} has a name");
        }
        assert_eq!(key_of("w"), Some(b'W'));
        assert_eq!(key_of("F1"), None);
    }

    #[test]
    fn scripts_take_commas_and_lines() {
        let keys = parse("12:Enter,1.5-3:W\n4-5:Space\nbad,6:Nope");
        assert_eq!(
            keys,
            vec![(s(12.0), s(12.2), 0x0D), (s(1.5), s(3.0), b'W'), (s(4.0), s(5.0), 0x20)]
        );
    }

    /// A key held over polls is one hold, written when it is let go; a key
    /// without a name is written by its code.
    #[test]
    fn keys_are_written_as_holds() {
        let mut r = Recorder::default();
        let mut out = String::new();
        r.keys(s(1.0), &[b'W'], &mut out);
        r.keys(s(1.5), &[b'W', 0x5B], &mut out);
        assert!(out.is_empty());
        r.keys(s(2.0), &[0x5B], &mut out);
        r.keys(s(2.01), &[], &mut out);
        assert_eq!(out, "1.000-2.000:W\n1.500-2.010:vk5B\n");
        let mut out = String::new();
        r.keys(s(3.0), &[0x0D], &mut out);
        r.keys(s(3.0), &[], &mut out);
        assert_eq!(out, "3.000-3.050:Enter\n");
    }

    /// A pad's buttons by the stand-in pad's keys, its axes when they move.
    #[test]
    fn pads_are_written_as_buttons_and_axes() {
        let mut r = Recorder::default();
        let mut out = String::new();
        let mut pad = PadState { buttons: super::super::A, ..Default::default() };
        r.pad(s(1.0), 0, Some(&pad), &mut out);
        assert!(out.is_empty(), "at rest: no axes line");
        pad.left_y = 0.8;
        r.pad(s(1.2), 0, Some(&pad), &mut out);
        pad.left_y = 0.81;
        r.pad(s(1.3), 0, Some(&pad), &mut out);
        pad.buttons = 0;
        r.pad(s(1.4), 0, Some(&pad), &mut out);
        r.pad(s(2.0), 0, None, &mut out);
        r.pad(s(2.1), 0, None, &mut out);
        assert_eq!(
            out,
            "1.200 pad0 axes 0.000 0.800 0.000 0.000 0.000 0.000\n\
             1.000-1.400:pad0:Enter\n\
             2.000 pad0 axes 0.000 0.000 0.000 0.000 0.000 0.000\n"
        );
    }
}
