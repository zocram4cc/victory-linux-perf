//! Diagnostics: switches that change while the title runs, so that one run
//! measures both sides in the same scene. `VICTORY_AB=<seconds>,<switch>[,<switch>..]`:
//! every so many seconds, at a present, the next phase begins; switch number
//! n is on in the phases whose number has bit n, so that all combinations
//! come one after another, again and again. Each phase's frame times are
//! logged at its end (the frames of its first second left out). A switch
//! named here does not go by its own variable any more.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

struct Asked {
    seconds: f64,
    switches: Vec<String>,
}

fn parse(text: &str) -> Option<Asked> {
    let mut parts = text.split(',').map(str::trim);
    let seconds = parts.next()?.parse::<f64>().ok().filter(|s| *s > 1.0)?;
    let switches: Vec<String> = parts.filter(|s| !s.is_empty()).map(str::to_string).collect();
    (!switches.is_empty() && switches.len() <= 8).then_some(Asked { seconds, switches })
}

fn asked() -> Option<&'static Asked> {
    static ASKED: OnceLock<Option<Asked>> = OnceLock::new();
    ASKED.get_or_init(|| parse(&std::env::var("VICTORY_AB").ok()?)).as_ref()
}

/// The switches that are on now, a bit each.
static BITS: AtomicU32 = AtomicU32::new(0);

/// Whether `switch` is on now, if it is one of those that change.
pub(crate) fn pick(switch: &str) -> Option<bool> {
    bit(switch).map(on)
}

/// The number of `switch` among those that change.
pub(crate) fn bit(switch: &str) -> Option<u32> {
    Some(asked()?.switches.iter().position(|s| s == switch)? as u32)
}

/// Whether the switch of that number is on now.
pub(crate) fn on(bit: u32) -> bool {
    BITS.load(Relaxed) >> bit & 1 != 0
}

/// What the run has seen of the phase under way.
struct Phase {
    started: crate::clock::Instant,
    number: u64,
    began: crate::clock::Instant,
    last: crate::clock::Instant,
    drawn: u64,
    frames: Vec<f32>,
    draws: u64,
}

/// The frames' mean, median and nine tenths.
fn summary(frames: &mut [f32]) -> (f32, f32, f32) {
    if frames.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mean = frames.iter().sum::<f32>() / frames.len() as f32;
    frames.sort_by(f32::total_cmp);
    (mean, frames[frames.len() / 2], frames[frames.len() * 9 / 10])
}

/// The title's present ends on this thread. `change`: called when another
/// phase begins, before any switch changes, with what `switch` is and what it
/// will be.
pub(crate) fn presented(change: impl Fn(&str, bool, bool)) {
    let Some(asked) = asked() else {
        return;
    };
    static PHASE: Mutex<Option<Phase>> = Mutex::new(None);
    let now = crate::clock::Instant::now();
    let drawn = crate::draw::drawn();
    let mut phase = PHASE.lock().unwrap_or_else(|e| e.into_inner());
    let p = phase.get_or_insert_with(|| Phase {
        started: now,
        number: 0,
        began: now,
        last: now,
        drawn,
        frames: Vec::new(),
        draws: 0,
    });
    if (now - p.began).as_secs_f64() > 1.0 {
        p.frames.push((now - p.last).as_secs_f32() * 1000.0);
        p.draws += drawn.saturating_sub(p.drawn);
    }
    (p.last, p.drawn) = (now, drawn);
    let number = ((now - p.started).as_secs_f64() / asked.seconds) as u64;
    if number == p.number {
        return;
    }
    let bits = |number: u64| (number % (1 << asked.switches.len())) as u32;
    let (was, will) = (bits(p.number), bits(number));
    let (mean, median, most) = summary(&mut p.frames);
    let on: Vec<&str> = asked.switches.iter().enumerate().filter(|(n, _)| was >> n & 1 != 0).map(|(_, s)| s.as_str()).collect();
    tracing::info!(
        "A/B phase {} bits {was} on [{}]: {} frames, mean {mean:.1} ms, median {median:.1}, p90 {most:.1}, {:.0} draws a frame",
        p.number,
        on.join(" "),
        p.frames.len(),
        p.draws as f64 / p.frames.len().max(1) as f64
    );
    for (n, switch) in asked.switches.iter().enumerate() {
        change(switch, was >> n & 1 != 0, will >> n & 1 != 0);
    }
    BITS.store(will, Relaxed);
    (p.number, p.began, p.draws) = (number, now, 0);
    p.frames.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_and_seconds_are_read_and_frames_summed_up() {
        let a = parse("10, VICTORY_A,VICTORY_B").unwrap();
        assert_eq!((a.seconds, a.switches), (10.0, vec!["VICTORY_A".to_string(), "VICTORY_B".to_string()]));
        assert!(parse("10").is_none());
        assert!(parse("x,VICTORY_A").is_none());
        assert!(parse("0,VICTORY_A").is_none());
        assert_eq!(summary(&mut [30.0, 10.0, 20.0]), (20.0, 20.0, 30.0));
        assert_eq!(summary(&mut []), (0.0, 0.0, 0.0));
        assert_eq!(pick("VICTORY_NONE_SUCH"), None);
    }
}
