use crate::shadow::{CardSpan, Shadow};

pub(super) struct Pass<'a> {
    pub(super) shadow: &'a Shadow,
    pub(super) span: CardSpan,
    pub(super) at: u32,
    pub(super) picked: bool,
}

/// The compare shader reads whole pieces. Byte ranges that share a piece
/// can use the same result while still copying back their exact bytes.
pub(super) struct Plan<'a> {
    pub(super) passes: Vec<Pass<'a>>,
    pub(super) taken: u32,
    capacity: u32,
}

impl<'a> Plan<'a> {
    pub(super) fn new(taken: u32, capacity: u32) -> Self {
        Self {
            passes: Vec::new(),
            taken,
            capacity,
        }
    }

    pub(super) fn add(&mut self, shadow: &'a Shadow, span: CardSpan, picked: bool) -> Option<u32> {
        let previous = self.passes.last_mut().filter(|pass| {
            std::ptr::eq(pass.shadow, shadow)
                && pass.picked == picked
                && (pass.span.first..=pass.span.first + pass.span.pieces).contains(&span.first)
        });
        let at = previous
            .as_ref()
            .map_or(self.taken, |pass| pass.at + (span.first - pass.span.first));
        let end = at + span.pieces;
        if end > self.capacity {
            return None;
        }
        if let Some(pass) = previous {
            pass.span.pieces = pass.span.pieces.max(end - pass.at);
            pass.span.to = pass.span.to.max(span.to);
        } else {
            self.passes.push(Pass {
                shadow,
                span,
                at,
                picked,
            });
        }
        self.taken = self.taken.max(end);
        Some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_ranges_use_the_same_result_for_a_shared_piece() {
        let shadow = Shadow::unbacked(0x20010, std::ptr::null_mut(), 4 << 16, false);
        let mut compares = Plan::new(9, 12);
        let span = |at| shadow.card_span(at, at + 2).unwrap();
        for at in (0x20100..0x20200).step_by(4) {
            assert_eq!(compares.add(&shadow, span(at), false), Some(9));
        }
        assert_eq!(compares.passes.len(), 1);
        assert_eq!(compares.passes[0].span.pieces, 1);
        assert_eq!(compares.taken, 10);

        assert_eq!(compares.add(&shadow, span(0x30010), false), Some(10));
        assert_eq!(compares.add(&shadow, span(0x50010), false), Some(11));
        assert_eq!(compares.passes.len(), 2);
        assert_eq!(compares.passes[0].span.pieces, 2);
        assert_eq!(compares.passes[1].span.first, 3);

        // An existing result can still be reused when the buffer is full.
        assert_eq!(compares.add(&shadow, span(0x60000), false), None);
        assert_eq!(compares.add(&shadow, span(0x50030), true), None);
        assert_eq!(compares.add(&shadow, span(0x50030), false), Some(11));
        assert_eq!(compares.taken, 12);
    }
}
