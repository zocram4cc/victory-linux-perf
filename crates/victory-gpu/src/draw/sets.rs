//! What the host list was last given for the bindings `draw.rs` sets on it
//! for every draw and dispatch: the vertex and index buffers, and per bind
//! point the root constants, root views and descriptor tables, by root
//! parameter. A call that would give the list what it already holds is
//! skipped (`VICTORY_SET_CACHE=0`: every call is made, as before).
//!
//! Exact because a binding keeps the value it was given until something gives
//! it another or voids it, and giving the same value again changes nothing: a
//! view, table or constant is read by the GPU at the draw, not at the call.
//! What changes a binding behind this cache is forgotten where it happens
//! (the `forget_*` calls in `draw.rs` and `list.rs`).

use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Graphics::Direct3D12::{D3D12_INDEX_BUFFER_VIEW, D3D12_VERTEX_BUFFER_VIEW};

/// Vertex buffer slots a list has.
const SLOTS: usize = 32;

/// A root argument as the host list has it.
#[derive(Clone, PartialEq)]
pub(crate) enum Bound {
    Constants(Vec<u32>),
    Cbv(u64),
    Srv(u64),
    Uav(u64),
    Table(u64),
}

/// What one host list has bound, as far as this state knows it.
pub(crate) struct Sets {
    enabled: bool,
    vertex: [Option<D3D12_VERTEX_BUFFER_VIEW>; SLOTS],
    index: Option<D3D12_INDEX_BUFFER_VIEW>,
    /// Graphics, compute; by root parameter.
    roots: [Vec<Option<Bound>>; 2],
    /// Calls given to the host list and skipped, by this state. They go to
    /// [`SENT`] and [`SKIPPED`] when the state is dropped: a shared counter
    /// touched per call is a cache line every translating thread writes.
    sent: u64,
    skipped: u64,
}

/// Calls given to host lists and skipped, by the states dropped so far.
static SENT: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);

/// Calls given (sent) and skipped by the states dropped since the last call.
/// A state's counts come in when it is reset, detached or dropped.
pub(crate) fn take_counts() -> (u64, u64) {
    (
        SENT.swap(0, Ordering::Relaxed),
        SKIPPED.swap(0, Ordering::Relaxed),
    )
}

impl Drop for Sets {
    fn drop(&mut self) {
        SENT.fetch_add(self.sent, Ordering::Relaxed);
        SKIPPED.fetch_add(self.skipped, Ordering::Relaxed);
    }
}

/// The slot of root parameter `param` in `roots`, made if it is not there yet.
fn root_slot(roots: &mut Vec<Option<Bound>>, param: u32) -> &mut Option<Bound> {
    let at = param as usize;
    if roots.len() <= at {
        roots.resize(at + 1, None);
    }
    &mut roots[at]
}

impl Default for Sets {
    fn default() -> Self {
        Sets::new(crate::toggled!("VICTORY_SET_CACHE"))
    }
}

impl Sets {
    pub(crate) fn new(enabled: bool) -> Self {
        Sets {
            enabled,
            vertex: [None; SLOTS],
            index: None,
            roots: [Vec::new(), Vec::new()],
            sent: 0,
            skipped: 0,
        }
    }

    /// Counts one decision and returns it: whether the call must be made.
    fn tally(&mut self, sent: bool) -> bool {
        if sent {
            self.sent += 1;
        } else {
            self.skipped += 1;
        }
        sent
    }

    /// Whether the vertex buffers in slots `0..views.len()` must be given.
    pub(crate) fn vertex_buffers(&mut self, views: &[D3D12_VERTEX_BUFFER_VIEW]) -> bool {
        debug_assert!(views.len() <= SLOTS);
        let known = self.enabled
            && views
                .iter()
                .zip(&self.vertex)
                .all(|(v, bound)| bound.as_ref() == Some(v));
        if !known {
            for (bound, v) in self.vertex.iter_mut().zip(views) {
                *bound = Some(*v);
            }
        }
        self.tally(!known)
    }

    /// Whether the index buffer must be given.
    pub(crate) fn index_buffer(&mut self, view: &D3D12_INDEX_BUFFER_VIEW) -> bool {
        let known = self.enabled && self.index.as_ref() == Some(view);
        if !known {
            self.index = Some(*view);
        }
        self.tally(!known)
    }

    /// Whether the root constants of parameter `param` must be given.
    pub(crate) fn constants(&mut self, compute: bool, param: u32, values: &[u32]) -> bool {
        let enabled = self.enabled;
        let slot = root_slot(&mut self.roots[usize::from(compute)], param);
        let known = if let Some(Bound::Constants(old)) = slot {
            let known = enabled && old.as_slice() == values;
            if !known {
                old.clear();
                old.extend_from_slice(values);
            }
            known
        } else {
            *slot = Some(Bound::Constants(values.to_vec()));
            false
        };
        self.tally(!known)
    }

    /// Whether root view or table `value` of parameter `param` must be given.
    pub(crate) fn root(&mut self, compute: bool, param: u32, value: Bound) -> bool {
        let enabled = self.enabled;
        let slot = root_slot(&mut self.roots[usize::from(compute)], param);
        let known = enabled && slot.as_ref() == Some(&value);
        if !known {
            *slot = Some(value);
        }
        self.tally(!known)
    }

    /// A root signature was set on a bind point, or its root arguments may
    /// have changed behind this: none of its root arguments is known.
    pub(crate) fn forget_roots(&mut self, compute: bool) {
        self.roots[usize::from(compute)].clear();
    }

    /// One root parameter was written behind this.
    pub(crate) fn forget_root(&mut self, compute: bool, param: u32) {
        if let Some(slot) = self.roots[usize::from(compute)].get_mut(param as usize) {
            *slot = None;
        }
    }

    /// The vertex buffers were set behind this.
    pub(crate) fn forget_vertex(&mut self) {
        self.vertex = [None; SLOTS];
    }

    /// A list was replayed, reset or closed: nothing it had set is known.
    pub(crate) fn forget_all(&mut self) {
        self.forget_vertex();
        self.index = None;
        self.forget_roots(false);
        self.forget_roots(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

    fn vertex(address: u64) -> D3D12_VERTEX_BUFFER_VIEW {
        D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: address,
            SizeInBytes: 64,
            StrideInBytes: 16,
        }
    }

    fn index(address: u64) -> D3D12_INDEX_BUFFER_VIEW {
        D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: address,
            SizeInBytes: 64,
            Format: DXGI_FORMAT(57),
        }
    }

    #[test]
    fn a_vertex_buffer_given_again_is_skipped() {
        let mut s = Sets::new(true);
        assert!(s.vertex_buffers(&[vertex(1)]), "the first call is sent");
        assert!(
            !s.vertex_buffers(&[vertex(1)]),
            "the same view again is skipped"
        );
        assert!(s.vertex_buffers(&[vertex(2)]), "another view is sent");
    }

    #[test]
    fn a_shorter_list_leaves_later_slots_known() {
        let mut s = Sets::new(true);
        assert!(s.vertex_buffers(&[vertex(1), vertex(2)]));
        assert!(
            !s.vertex_buffers(&[vertex(1)]),
            "slot 0 is what it was; slot 1 stays bound"
        );
        assert!(
            !s.vertex_buffers(&[vertex(1), vertex(2)]),
            "nothing changed on the list"
        );
    }

    #[test]
    fn an_index_buffer_is_skipped_only_when_the_same() {
        let mut s = Sets::new(true);
        assert!(s.index_buffer(&index(1)));
        assert!(!s.index_buffer(&index(1)), "the same view again is skipped");
        assert!(s.index_buffer(&index(2)));
    }

    #[test]
    fn root_constants_are_skipped_only_when_every_value_and_count_is_the_same() {
        let mut s = Sets::new(true);
        assert!(s.constants(false, 0, &[1, 2, 3]));
        assert!(
            !s.constants(false, 0, &[1, 2, 3]),
            "the same values again are skipped"
        );
        assert!(s.constants(false, 0, &[1, 2, 4]));
        assert!(
            s.constants(false, 0, &[1, 2, 4, 0]),
            "another count is sent"
        );
    }

    #[test]
    fn root_views_and_tables_are_skipped_only_when_kind_and_address_are_the_same() {
        let mut s = Sets::new(true);
        assert!(s.root(true, 3, Bound::Uav(0x100)));
        assert!(
            !s.root(true, 3, Bound::Uav(0x100)),
            "the same view again is skipped"
        );
        assert!(
            s.root(true, 3, Bound::Srv(0x100)),
            "the same address as another kind is sent"
        );
        assert!(s.root(true, 3, Bound::Srv(0x200)));
        assert!(
            s.root(true, 4, Bound::Table(0x200)),
            "another parameter is sent"
        );
        assert!(
            !s.root(true, 4, Bound::Table(0x200)),
            "the same table again is skipped"
        );
    }

    #[test]
    fn a_root_signature_change_resends_only_its_bind_point() {
        let mut s = Sets::new(true);
        assert!(s.constants(false, 0, &[7]));
        assert!(s.constants(true, 0, &[7]));
        assert!(s.root(false, 1, Bound::Cbv(0x40)));
        assert!(s.root(true, 1, Bound::Cbv(0x40)));
        s.forget_roots(false);
        assert!(
            s.constants(false, 0, &[7]),
            "graphics constants are void after its root signature"
        );
        assert!(
            s.root(false, 1, Bound::Cbv(0x40)),
            "graphics views are void after its root signature"
        );
        assert!(
            !s.constants(true, 0, &[7]),
            "compute constants are untouched"
        );
        assert!(
            !s.root(true, 1, Bound::Cbv(0x40)),
            "compute views are untouched"
        );
    }

    #[test]
    fn a_replay_resends_everything() {
        let mut s = Sets::new(true);
        assert!(s.vertex_buffers(&[vertex(1)]));
        assert!(s.index_buffer(&index(1)));
        assert!(s.constants(true, 0, &[9]));
        assert!(s.root(false, 2, Bound::Table(0x80)));
        assert!(!s.vertex_buffers(&[vertex(1)]), "known before the replay");
        assert!(!s.index_buffer(&index(1)), "known before the replay");
        assert!(!s.constants(true, 0, &[9]), "known before the replay");
        assert!(
            !s.root(false, 2, Bound::Table(0x80)),
            "known before the replay"
        );
        s.forget_all();
        assert!(
            s.vertex_buffers(&[vertex(1)]),
            "vertex buffers are sent to a new list"
        );
        assert!(
            s.index_buffer(&index(1)),
            "the index buffer is sent to a new list"
        );
        assert!(
            s.constants(true, 0, &[9]),
            "constants are sent to a new list"
        );
        assert!(
            s.root(false, 2, Bound::Table(0x80)),
            "tables are sent to a new list"
        );
    }

    #[test]
    fn a_single_constant_write_resends_only_its_parameter() {
        let mut s = Sets::new(true);
        assert!(s.constants(true, 0, &[1, 2]));
        assert!(s.constants(true, 1, &[5]));
        s.forget_root(true, 0);
        assert!(
            s.constants(true, 0, &[1, 2]),
            "parameter 0 had a word written behind it"
        );
        assert!(!s.constants(true, 1, &[5]), "parameter 1 is untouched");
    }

    #[test]
    fn rectangles_resend_the_vertex_buffers() {
        let mut s = Sets::new(true);
        assert!(s.vertex_buffers(&[vertex(1)]));
        assert!(
            !s.vertex_buffers(&[vertex(1)]),
            "known before the expansion"
        );
        s.forget_vertex();
        assert!(
            s.vertex_buffers(&[vertex(1)]),
            "the expansion may have set slot 0"
        );
    }

    #[test]
    fn switched_off_every_call_is_sent() {
        let mut on = Sets::new(true);
        assert!(on.index_buffer(&index(1)));
        assert!(
            !on.index_buffer(&index(1)),
            "switched on, the repeat is skipped"
        );
        let mut off = Sets::new(false);
        for _ in 0..2 {
            assert!(off.vertex_buffers(&[vertex(1)]));
            assert!(off.index_buffer(&index(1)));
            assert!(off.constants(false, 0, &[1]));
            assert!(off.root(false, 1, Bound::Srv(0x10)));
        }
    }

    /// The counts reach the totals when the state is dropped: run with
    /// `--test-threads 1`, as the totals are process-wide.
    #[test]
    fn calls_are_counted_as_sent_or_skipped() {
        take_counts();
        {
            let mut s = Sets::new(true);
            s.index_buffer(&index(1));
            s.index_buffer(&index(1));
            s.index_buffer(&index(1));
        }
        assert_eq!(take_counts(), (1, 2));
    }
}
