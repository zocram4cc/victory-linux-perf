//! `VICTORY_PAR_REFRESH=1` (off by default) shares pre-write refreshes of
//! unwatched spans of at least 1 MiB between wide workers in 256 KiB pieces. The host
//! has already merged spans by shadow. Distinct shadows have separate backing
//! buffers, even when they read the same title bytes. Only an unwatched span
//! with no overlapping shader write can be divided without changing the
//! write-watch journal, its partial-page history or the pending-write gaps.
//! Publish each divided span's snapshot invalidation and card dirtiness once,
//! after the workers join and before the submission proceeds.

use super::{Arc, AtomicU64, Ordering, Shadow};
use std::sync::OnceLock;
use std::time::Instant;

pub(crate) type Span = (Arc<Shadow>, usize, usize);
const MIN_BYTES: usize = 1 << 20;
const PIECE: usize = 256 << 10;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PAR_REFRESH").is_some_and(|v| v == "1"))
}

struct Whole {
    shadow: Arc<Shadow>,
    from: usize,
    to: usize,
    started: OnceLock<Instant>,
    elapsed_us: AtomicU64,
}

impl Whole {
    fn eligible(shadow: &Shadow, from: usize, to: usize) -> bool {
        to.saturating_sub(from) >= MIN_BYTES
            && shadow.unwatched()
            && !shadow.in_place()
            && from >= shadow.start as usize
            && to <= (shadow.start + shadow.len) as usize
            && !(shadow.card.is_some() && crate::host::upcheck::on())
            && !shadow.writes_pending_in(from, to)
    }

    fn copy_piece(&self, from: usize) {
        let len = (self.to - from).min(PIECE);
        // SAFETY: the span was checked against this shadow's mapping. Each
        // job writes its own slice, and the Arc keeps the mapping alive.
        let dst = unsafe { self.shadow.data.add(from - self.shadow.start as usize) };
        let start = self.started.get_or_init(Instant::now);
        let _piece = crate::timeline::span("a piece of a pre-write refresh");
        crate::host::copy_mapped(dst, from as *const u8, len, false);
        self.elapsed_us
            .fetch_max(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    }
}

/// `leading` independent texture jobs keep their priority. Ordinary spans
/// still go through the caller's clean check and Shadow::update. Pieces from
/// every divided span share their worker pass, including the submitting
/// thread (wide_each dispatches through wide_run).
pub(crate) fn run(
    parallel: bool,
    mut spans: Vec<Span>,
    leading: usize,
    texture: impl Fn(usize) + Send + Sync + 'static,
    update: impl Fn(&Span) + Send + Sync + 'static,
) -> (u64, u64) {
    let mut whole = Vec::new();
    let mut pieces = Vec::new();
    if parallel {
        spans.retain(|(shadow, from, to)| {
            if !Whole::eligible(shadow, *from, *to) {
                return true;
            }
            pieces.extend((*from..*to).step_by(PIECE).map(|at| (whole.len(), at)));
            whole.push(Whole {
                shadow: shadow.clone(),
                from: *from,
                to: *to,
                started: OnceLock::new(),
                elapsed_us: AtomicU64::new(0),
            });
            false
        });
    }
    if pieces.is_empty() {
        super::wide_each(leading + spans.len(), move |i| {
            match i.checked_sub(leading) {
                None => texture(i),
                Some(i) => update(&spans[i]),
            }
        });
        return (0, 0);
    }
    let whole = Arc::new(whole);
    let work = whole.clone();
    super::wide_each(leading + pieces.len() + spans.len(), move |i| {
        let Some(i) = i.checked_sub(leading) else {
            return texture(i);
        };
        if let Some(span) = spans.get(i) {
            update(span);
        } else {
            let (item, from) = pieces[i - spans.len()];
            work[item].copy_piece(from);
        }
    });
    let (mut us, mut bytes) = (0, 0);
    for item in whole.iter() {
        item.shadow.copied_range(item.from, item.to);
        let len = item.to - item.from;
        let took = item.elapsed_us.load(Ordering::Relaxed);
        #[cfg(feature = "diagnostics")]
        {
            use crate::copystat::{self, C};
            copystat::add(C::WholeBytes, len as u64);
            copystat::whole(item.from, len, took);
            copystat::range(item.from, len, took, 0);
        }
        us += took;
        bytes += len as u64;
    }
    (us, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::{NOT_WATCHED, land_tests::copy_of};

    #[test]
    fn split_only_whole_unwatched_spans_without_pending_writes() {
        if crate::host::host().is_none() {
            return;
        }
        let title = vec![0u8; MIN_BYTES + 2];
        let at = title.as_ptr() as usize;
        let mut shadow = copy_of(at, title.len()).expect("mapped test buffer");
        assert!(!Whole::eligible(&shadow, at, at + MIN_BYTES));
        *shadow.watched.get_mut() = NOT_WATCHED;
        assert!(!Whole::eligible(&shadow, at, at + MIN_BYTES - 1));
        assert!(!Whole::eligible(&shadow, at - 1, at + MIN_BYTES));
        assert!(!Whole::eligible(&shadow, at, at + title.len() + 1));
        let pending = shadow.pending.get_mut().unwrap();
        pending.push((at + MIN_BYTES, at + MIN_BYTES + 1));
        assert!(Whole::eligible(&shadow, at, at + MIN_BYTES));
        assert!(!Whole::eligible(&shadow, at + 1, at + MIN_BYTES + 1));
    }

    #[test]
    fn mixed_batch_matches_unsplit_bytes_and_dirty_ranges() {
        if crate::host::host().is_none() {
            return;
        }
        let len = 2 * MIN_BYTES + 79;
        let title: Vec<u8> = (0..len)
            .map(|i| (1 + (i ^ (i >> 11)) % 251) as u8)
            .collect();
        let at = title.as_ptr() as usize;
        let exercise = |parallel| {
            let mut spans = Vec::new();
            let mut shadows = Vec::new();
            // Aliased title bytes, separate destinations. The second shadow
            // has one pending GPU write; the third is too small to divide.
            for (i, (lo, hi)) in [(13, len - 9), (3, len), (0, 113), (8, MIN_BYTES + 8)]
                .into_iter()
                .enumerate()
            {
                let mut shadow = copy_of(at, len).expect("mapped test buffer");
                *shadow.watched.get_mut() = NOT_WATCHED;
                if i == 1 {
                    shadow
                        .pending
                        .get_mut()
                        .unwrap()
                        .push((at + PIECE - 7, at + PIECE + 9));
                }
                let shadow = Arc::new(shadow);
                spans.push((shadow.clone(), at + lo, at + hi));
                shadows.push(shadow);
            }
            let ordinary = Arc::new(AtomicU64::new(0));
            let count = ordinary.clone();
            let textures = Arc::new(AtomicU64::new(0));
            let leading = textures.clone();
            let (_, divided) = run(
                parallel,
                spans,
                2,
                move |i| {
                    leading.fetch_or(1 << i, Ordering::Relaxed);
                },
                move |(s, a, b)| {
                    count.fetch_add(s.update(*a, *b - *a) as u64, Ordering::Relaxed);
                },
            );
            assert_eq!(textures.load(Ordering::Relaxed), 3);
            let eligible = len - 22 + MIN_BYTES;
            assert_eq!(divided, if parallel { eligible as u64 } else { 0 });
            assert_eq!(
                divided + ordinary.load(Ordering::Relaxed),
                (eligible + len - 3 - 16 + 113) as u64
            );
            shadows
                .into_iter()
                .map(|s| {
                    // SAFETY: the batch has joined and this Arc owns the mapping.
                    (
                        unsafe { std::slice::from_raw_parts(s.data, len) }.to_vec(),
                        s.take_dirty(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let reference = exercise(false);
        let divided = exercise(true);
        assert_eq!(&reference[0].0[13..len - 9], &title[13..len - 9]);
        assert!(reference[1].0[PIECE - 7..PIECE + 9].iter().all(|b| *b == 0));
        assert_eq!(reference[0].1, [(13, len - 9)]);
        for ((bytes, dirty), (want_bytes, want_dirty)) in divided.iter().zip(&reference) {
            assert!(bytes == want_bytes);
            assert_eq!(dirty, want_dirty);
        }
    }
}
