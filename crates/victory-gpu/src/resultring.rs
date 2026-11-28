//! The compare's result area as a ring (`VICTORY_RESULT_RING`, on by default): the dwords a
//! plan takes in `CardWork::differs` are given back when its submission has landed, and the
//! next plans go on after the newest ones, wrapping to the start, instead of the whole area
//! being reused only when no plan is in the air.
//!
//! With the relay a plan is nearly always in the air, so the area only filled up, and the
//! submitting thread drained every flight (a wait for the card and the copy-back of all of
//! them) each time it was half taken (`Host::card_results_low`). Plans land in the order they
//! were made, so the space they free comes back in that order too.

use std::collections::VecDeque;

/// Dwords of the result area taken by plans in the air, in the order the plans were made.
#[derive(Debug)]
pub(crate) struct ResultRing {
    /// The area's size, in dwords.
    size: u32,
    /// Where the next plan starts if there is room after it.
    head: u32,
    /// The taken stretches (start, end), oldest first.
    taken: VecDeque<(u32, u32)>,
    /// Their dwords in all.
    used: u32,
    /// Whether stretches are reused before the area is empty ([`ResultRing::new`]).
    wraps: bool,
}

impl ResultRing {
    /// An empty area of `size` dwords. `wraps: false` keeps the former rule: nothing is
    /// reused until every plan is done with.
    pub(crate) fn new(size: u32, wraps: bool) -> ResultRing {
        ResultRing { size, head: 0, taken: VecDeque::new(), used: 0, wraps }
    }

    /// Dwords taken by plans in the air.
    pub(crate) fn used(&self) -> u32 {
        self.used
    }

    /// The area's size, in dwords.
    pub(crate) fn size(&self) -> u32 {
        self.size
    }

    /// Takes up to `need` dwords in one stretch for a plan: `(start, granted)`. Less than
    /// `need` (down to none) when the area has no stretch that long free; the plan then
    /// compares only the ranges that fit, as when the area ran out before.
    pub(crate) fn take(&mut self, need: u32) -> (u32, u32) {
        if need == 0 {
            return (self.head, 0);
        }
        let (start, room) = match self.taken.front() {
            None => {
                self.head = 0;
                (0, self.size)
            }
            Some(_) if !self.wraps => (self.head, self.size - self.head),
            Some(&(oldest, _)) if oldest < self.head => {
                // Taken: [oldest, head). Free: [head, size) and [0, oldest).
                let after = self.size - self.head;
                if after >= need || after >= oldest {
                    (self.head, after)
                } else {
                    (0, oldest)
                }
            }
            // Wrapped (or full): taken [oldest, size) and [0, head); free [head, oldest).
            Some(&(oldest, _)) => (self.head, oldest - self.head),
        };
        let granted = need.min(room);
        if granted > 0 {
            self.taken.push_back((start, start + granted));
            self.head = start + granted;
            self.used += granted;
        }
        (start, granted)
    }

    /// Gives back the stretch a plan took (its `take`'s start and grant); the area is reused
    /// from that point once every older stretch is back too.
    pub(crate) fn give(&mut self, start: u32, granted: u32) {
        if granted == 0 {
            return;
        }
        if let Some(i) = self.taken.iter().position(|&s| s == (start, start + granted)) {
            self.taken.remove(i);
            self.used -= granted;
            // The newest stretch ends at `head`: with it back, the next plan starts after the
            // one before it.
            if self.wraps && start + granted == self.head {
                self.head = self.taken.back().map_or(0, |&(_, end)| end);
            }
        }
        if self.taken.is_empty() {
            self.head = 0;
        }
    }

    /// Every plan is done with: the area starts again from its beginning.
    pub(crate) fn clear(&mut self) {
        self.taken.clear();
        self.head = 0;
        self.used = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::ResultRing;

    /// No two stretches in the air overlap, and each lies inside the area.
    fn disjoint(ring: &ResultRing) {
        let mut stretches: Vec<_> = ring.taken.iter().copied().collect();
        stretches.sort_unstable();
        for w in stretches.windows(2) {
            assert!(w[0].1 <= w[1].0, "{stretches:?}");
        }
        assert!(stretches.iter().all(|&(s, e)| s < e && e <= ring.size()), "{stretches:?}");
        assert_eq!(ring.used(), stretches.iter().map(|&(s, e)| e - s).sum::<u32>());
    }

    #[test]
    fn plans_that_land_in_order_reuse_the_area_without_it_ever_emptying() {
        let mut ring = ResultRing::new(100, true);
        let mut air = std::collections::VecDeque::new();
        // Three plans in the air at all times, 30 dwords each: 300 plans go through an area
        // of 100 dwords, every one compared in full, and never with the area empty.
        for plan in 0..300 {
            if air.len() == 3 {
                let (start, granted) = air.pop_front().unwrap();
                ring.give(start, granted);
            }
            let (start, granted) = ring.take(30);
            assert_eq!(granted, 30, "plan {plan}");
            air.push_back((start, granted));
            disjoint(&ring);
        }
        assert_eq!(ring.used(), 90);
    }

    #[test]
    fn a_full_area_grants_what_is_free_and_nothing_more() {
        let mut ring = ResultRing::new(100, true);
        assert_eq!(ring.take(60), (0, 60));
        assert_eq!(ring.take(60), (60, 40));
        assert_eq!(ring.take(1), (100, 0));
        disjoint(&ring);
        // The oldest lands: its stretch at the start is free again, the newest is not.
        ring.give(0, 60);
        assert_eq!(ring.take(50), (0, 50));
        disjoint(&ring);
        assert_eq!(ring.take(20), (50, 10));
        disjoint(&ring);
    }

    #[test]
    fn a_plan_that_lands_early_frees_its_stretch_once_the_older_ones_are_back() {
        let mut ring = ResultRing::new(100, true);
        let a = ring.take(40);
        let b = ring.take(40);
        // The newer plan is done with first: the older still holds the start.
        ring.give(b.0, b.1);
        assert_eq!(ring.used(), 40);
        assert_eq!(ring.take(70), (40, 60));
        disjoint(&ring);
        ring.give(a.0, a.1);
        disjoint(&ring);
    }

    #[test]
    fn without_wrapping_nothing_is_reused_until_every_plan_is_done_with() {
        let mut ring = ResultRing::new(100, false);
        let a = ring.take(60);
        let b = ring.take(30);
        ring.give(a.0, a.1);
        // The former rule: the start stays unused while a plan is in the air.
        assert_eq!(ring.take(30), (90, 10));
        ring.give(b.0, b.1);
        ring.give(90, 10);
        assert_eq!(ring.take(30), (0, 30));
    }
}
