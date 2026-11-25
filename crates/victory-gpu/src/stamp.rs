//! "Already checked in this submission" (`VICTORY_REFRESH_STAMP=1`; off by
//! default). Ported from clankenjoyer/victory-fps commit 5e30085
//! (`crates/victory-gpu/src/stamp.rs`), without its fault-tracked variant:
//! `watch::fault` is not in this tree.
//!
//! A world submission asks for some 20,000 refreshes of host copies, most
//! of them of ranges an earlier refresh of the same submission already
//! brought up to date. With the switch each copy remembers, by 64 KiB page,
//! what it checked under the current sequence number ([`now`]), and a
//! refresh wholly inside checked pages is skipped (`Shadow::update`).
//!
//! The sequence advances ([`advance`], or a [`Changed`] held across a
//! change) wherever title memory may change under a copy while a submission
//! is prepared, so that what was checked before is checked again: each
//! submission's start (`host`), the end of the submission's one write-watch
//! asking (`watch::ask_once_end`), our own writes into title memory
//! (`host::write_mapped`, `host::copy_mapped`, `host::read_backs`,
//! `watch::wrote`, `watch::wrote_own`), what the runtime tells of title
//! writes and remaps (`watch::title_memory_written`,
//! `watch::title_memory_remapped`, `watch::Remapping`), guard opens
//! (`watch::open_here`), and the descriptor records written into title
//! memory (`descriptor`).
//!
//! What changes: as with the submission's one asking (`watch::once`, on by
//! default), what the title's other threads write while a submission is
//! prepared is seen by the next submission. A range shaders may be writing
//! (a copy's `pending`) is never marked checked.
//!
//! Model: Ryujinx `MultiRegionHandle.QueryModified` (`_sequenceNumberBitmap`)
//! and `Buffer.SynchronizeMemory` (MIT).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::watch::PAGE;

/// Whether the stamp is on (see the module).
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_REFRESH_STAMP").is_some_and(|v| v == "1");
        if on {
            tracing::info!("refreshes of ranges a copy already checked in this submission return at once (VICTORY_REFRESH_STAMP=1)");
        }
        on
    })
}

/// The sequence number; 0 is never current.
static SEQ: AtomicU64 = AtomicU64::new(1);

/// Title memory may have changed under every copy: what was checked must be
/// checked again.
pub(crate) fn advance() {
    SEQ.fetch_add(1, Ordering::AcqRel);
}

/// The current sequence number.
pub(crate) fn now() -> u64 {
    SEQ.load(Ordering::Acquire)
}

/// Advances the sequence when dropped. A function that changes title memory
/// (or what the copies check against) holds one from its top, so the
/// sequence moves after the change whichever way the function returns.
pub(crate) struct Changed;

impl Drop for Changed {
    fn drop(&mut self) {
        advance();
    }
}

/// Per copy: which of its pages were checked under which sequence.
#[derive(Default)]
pub(crate) struct Stamp {
    /// The sequence `bits` belong to (0: none), read without the lock.
    seq: AtomicU64,
    bits: Mutex<Bits>,
}

#[derive(Default)]
struct Bits {
    seq: u64,
    /// One bit per page of the copy, from the copy's first page.
    words: Vec<u64>,
    /// Pages of the copy not checked under `seq`.
    unchecked: usize,
}

/// The copy's pages: `(first page number, count)` for buffer `[start, end)`.
fn pages(start: usize, end: usize) -> (usize, usize) {
    (start / PAGE, end.div_ceil(PAGE) - start / PAGE)
}

/// The pages `[p0, p1)` that `[from, to)` holds whole, as far as they are in
/// the buffer `[start, end)` (a page only partly in it is copied in part).
fn whole_pages(from: usize, to: usize, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut p0 = from / PAGE;
    if (p0 * PAGE).max(start) < from {
        p0 += 1;
    }
    let mut p1 = to.div_ceil(PAGE);
    if p1 > p0 && ((p1 * PAGE).min(end)) > to {
        p1 -= 1;
    }
    (p0 < p1).then_some((p0, p1))
}

impl Stamp {
    /// Whether `range` (inside the buffer `span`) was checked whole under
    /// `seq`, which is current.
    pub(crate) fn current(
        &self,
        seq: u64,
        (from, to): (usize, usize),
        (start, _): (usize, usize),
    ) -> bool {
        if from >= to || self.seq.load(Ordering::Acquire) != seq {
            return false;
        }
        let b = self.bits.lock().unwrap_or_else(|e| e.into_inner());
        if b.seq != seq {
            return false;
        }
        if b.unchecked == 0 {
            return true;
        }
        let base = start / PAGE;
        all_set(&b.words, from / PAGE - base, (to - 1) / PAGE - base)
    }

    /// `range` (inside `span`) was checked under `seq`, read before the check
    /// began: its whole pages are noted, unless the sequence advanced meanwhile.
    pub(crate) fn mark(&self, seq: u64, range: (usize, usize), span: (usize, usize)) {
        self.note(seq, now(), range, span);
    }

    /// [`Stamp::mark`] with the sequence now `latest`.
    fn note(
        &self,
        seq: u64,
        latest: u64,
        (from, to): (usize, usize),
        (start, end): (usize, usize),
    ) {
        if from >= to || latest != seq {
            return;
        }
        let Some((p0, p1)) = whole_pages(from, to, start, end) else {
            return;
        };
        let (base, count) = pages(start, end);
        let mut b = self.bits.lock().unwrap_or_else(|e| e.into_inner());
        if b.seq > seq {
            return;
        }
        if b.seq != seq {
            b.seq = seq;
            b.words.clear();
            b.words.resize(count.div_ceil(64), 0);
            b.unchecked = count;
        }
        let new = set_range(&mut b.words, p0 - base, p1 - 1 - base);
        b.unchecked -= new.min(b.unchecked);
        self.seq.store(seq, Ordering::Release);
    }

    /// Refreshes `range` (inside `span`) through `refresh`, unless it was
    /// checked under `seq` already: then nothing is done and this returns
    /// `None`. Otherwise it returns the bytes `refresh` copied, and notes
    /// the range checked under `seq` (read before the call).
    pub(crate) fn refresh(
        &self,
        seq: u64,
        range: (usize, usize),
        span: (usize, usize),
        refresh: impl FnOnce() -> usize,
    ) -> Option<usize> {
        if self.current(seq, range, span) {
            return None;
        }
        let bytes = refresh();
        self.mark(seq, range, span);
        Some(bytes)
    }
}

/// Whether bits `[first, last]` are all set.
fn all_set(words: &[u64], first: usize, last: usize) -> bool {
    let mut at = first;
    while at <= last {
        let (w, b) = (at / 64, at % 64);
        let n = (last - at + 1).min(64 - b);
        let mask = if n == 64 {
            u64::MAX
        } else {
            ((1u64 << n) - 1) << b
        };
        match words.get(w) {
            Some(&v) if v & mask == mask => {}
            _ => return false,
        }
        at += n;
    }
    true
}

/// Sets bits `[first, last]`; how many were not set.
fn set_range(words: &mut [u64], first: usize, last: usize) -> usize {
    let mut at = first;
    let mut new = 0;
    while at <= last {
        let (w, b) = (at / 64, at % 64);
        let n = (last - at + 1).min(64 - b);
        let mask = if n == 64 {
            u64::MAX
        } else {
            ((1u64 << n) - 1) << b
        };
        let Some(v) = words.get_mut(w) else {
            break;
        };
        new += (mask & !*v).count_ones() as usize;
        *v |= mask;
        at += n;
    }
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: usize = 0x1000_0000 + 100;
    const END: usize = START + 10 * PAGE;

    #[test]
    fn a_checked_range_is_current_until_the_sequence_changes() {
        let s = Stamp::default();
        assert!(!s.current(5, (START, START + PAGE), (START, END)));
        s.note(5, 5, (START, END), (START, END));
        assert!(s.current(5, (START, END), (START, END)));
        assert!(s.current(5, (START + 3 * PAGE, START + 4 * PAGE), (START, END)));
        assert!(!s.current(6, (START, START + 1), (START, END)));
        // A new sequence starts from nothing checked.
        s.note(6, 6, (START, START + 2 * PAGE), (START, END));
        assert!(!s.current(6, (START + 5 * PAGE, START + 6 * PAGE), (START, END)));
        assert!(!s.current(5, (START, END), (START, END)));
    }

    #[test]
    fn only_pages_held_whole_are_marked() {
        let s = Stamp::default();
        let page = START.div_ceil(PAGE) * PAGE; // the copy's second page
        // Half of a page, then the next page whole.
        s.note(7, 7, (page + PAGE / 2, page + 2 * PAGE), (START, END));
        assert!(!s.current(7, (page, page + PAGE), (START, END)));
        assert!(s.current(7, (page + PAGE, page + 2 * PAGE), (START, END)));
        assert!(!s.current(7, (page + PAGE, page + 2 * PAGE + 1), (START, END)));
        // The copy's first page is in the buffer from START on: whole if
        // the range starts there.
        s.note(7, 7, (START, page), (START, END));
        assert!(s.current(7, (START, page), (START, END)));
        // And its last page up to END.
        s.note(7, 7, (END / PAGE * PAGE, END), (START, END));
        assert!(s.current(7, (END - 1, END), (START, END)));
        s.note(7, 7, (END - 10, END), (START, END));
        assert!(!s.current(7, (END - PAGE, END), (START, END)));
    }

    #[test]
    fn a_mark_made_while_the_sequence_advanced_is_ignored() {
        let s = Stamp::default();
        s.note(3, 4, (START, END), (START, END));
        assert!(!s.current(3, (START, END), (START, END)));
        assert!(!s.current(4, (START, END), (START, END)));
        // Nor does an older mark undo a newer one.
        s.note(4, 4, (START, END), (START, END));
        s.note(3, 3, (START, END), (START, END));
        assert!(s.current(4, (START, END), (START, END)));
    }

    #[test]
    fn the_sequence_advances() {
        let a = now();
        advance();
        assert!(now() > a);
    }

    #[test]
    fn bit_ranges_cross_words() {
        let mut w = vec![0u64; 3];
        assert_eq!(set_range(&mut w, 60, 130), 71);
        assert!(all_set(&w, 60, 130));
        assert!(!all_set(&w, 59, 130));
        assert!(!all_set(&w, 60, 131));
        assert_eq!(set_range(&mut w, 0, 191), 192 - 71);
        assert!(all_set(&w, 0, 191));
        assert!(!all_set(&w, 0, 192));
    }

    /// A second refresh of a checked range is skipped; a refresh after an
    /// advance of the sequence is not.
    #[test]
    fn a_second_refresh_of_a_checked_range_is_skipped_and_one_after_an_advance_is_not() {
        let s = Stamp::default();
        let mut copied = 0;
        let seq = now();
        assert_eq!(
            s.refresh(seq, (START, END), (START, END), || {
                copied += 1;
                END - START
            }),
            Some(END - START)
        );
        assert_eq!(copied, 1);
        // Same sequence: checked already, so nothing is refreshed.
        assert_eq!(
            s.refresh(seq, (START, END), (START, END), || {
                copied += 1;
                END - START
            }),
            None
        );
        assert_eq!(copied, 1);
        // The sequence advanced: the range is refreshed again.
        advance();
        assert_eq!(
            s.refresh(now(), (START, END), (START, END), || {
                copied += 1;
                END - START
            }),
            Some(END - START)
        );
        assert_eq!(copied, 2);
    }
}
