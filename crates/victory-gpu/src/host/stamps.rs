//! Diagnostics: when each submission began and ended on the card, for the
//! timeline (`crate::timeline`). Lists made ahead, each a timestamp and its
//! way into memory the PC reads, go ahead of and behind every submission
//! while the timeline records.

use std::sync::atomic::{AtomicUsize, Ordering};

use windows::core::Interface;

use super::*;

/// Submissions stamped (three frames have some hundred and twenty).
const MOST: usize = 1024;

struct Stamps {
    /// The first half the lists that go ahead, then those that go behind.
    lists: Vec<ID3D12CommandList>,
    /// The mapping of what they write, a tick count each.
    back: usize,
    /// Keeps it all.
    _kept: (ID3D12CommandAllocator, ID3D12QueryHeap, ID3D12Resource),
    /// The card's ticks and the PC's counter at one moment.
    calibrated: (u64, u64),
}

// SAFETY: free-threaded objects, and a mapping only read.
unsafe impl Send for Stamps {}
// SAFETY: as above.
unsafe impl Sync for Stamps {}

static STAMPS: OnceLock<Option<Stamps>> = OnceLock::new();
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Makes the lists, before the recording begins.
pub(crate) fn timeline_ready() {
    let Some(host) = host() else {
        return;
    };
    STAMPS.get_or_init(|| match host.make_stamps() {
        Ok(stamps) => Some(stamps),
        Err(e) => {
            tracing::warn!(error = %e, "the timeline is without the card's times");
            None
        }
    });
}

/// The place of the next submission's stamps, if the timeline records.
pub(super) fn take() -> Option<usize> {
    if !crate::timeline::on() || STAMPS.get()?.is_none() {
        return None;
    }
    let at = NEXT.fetch_add(1, Ordering::Relaxed);
    (at < MOST).then_some(at)
}

/// The list that goes ahead of the submission at `at`, and the one behind.
pub(super) fn lists(at: usize) -> Option<(ID3D12CommandList, ID3D12CommandList)> {
    let stamps = STAMPS.get()?.as_ref()?;
    Some((stamps.lists[at].clone(), stamps.lists[MOST + at].clone()))
}

/// By place, from when to when a submission ran on the card, in ticks of
/// the PC's counter. Once the card has run them.
pub(crate) fn timeline_stamps() -> Vec<(u64, u64)> {
    times(NEXT.load(Ordering::Relaxed).min(MOST))
}

/// The stamps are used from the first again, for the next timeline.
pub(crate) fn timeline_restart() {
    if let Some(Some(stamps)) = STAMPS.get() {
        // SAFETY: the mapping holds twice `MOST` tick counts; the card wrote
        // them some frames ago.
        unsafe { std::ptr::write_bytes(stamps.back as *mut u8, 0, 16 * MOST) };
    }
    NEXT.store(0, Ordering::Relaxed);
}

/// The first `taken` of them.
fn times(taken: usize) -> Vec<(u64, u64)> {
    let (Some(Some(stamps)), Some(host)) = (STAMPS.get(), host()) else {
        return Vec::new();
    };
    let (mut card, mut pc) = (0, 0);
    // SAFETY: plain queries; the mapping holds twice `MOST` tick counts.
    let (frequency, back) = unsafe {
        if host.queue.GetClockCalibration(&mut card, &mut pc).is_err() {
            return Vec::new();
        }
        let frequency = host.queue.GetTimestampFrequency().unwrap_or(0);
        (frequency, std::slice::from_raw_parts(stamps.back as *const u64, 2 * MOST))
    };
    if frequency == 0 {
        return Vec::new();
    }
    // By the two calibrations, the card's clock as it ran between them.
    let (card0, pc0) = stamps.calibrated;
    let rate = match card.saturating_sub(card0) {
        0 => 1e7 / frequency as f64,
        passed => pc.saturating_sub(pc0) as f64 / passed as f64,
    };
    let place = |ticks: u64| (pc0 as f64 + (ticks as f64 - card0 as f64) * rate) as u64;
    (0..taken)
        .map(|at| match (back[at], back[MOST + at]) {
            (0, _) | (_, 0) => (0, 0),
            (from, to) => (place(from), place(to)),
        })
        .collect()
}

impl Host {
    fn make_stamps(&self) -> windows::core::Result<Stamps> {
        let Some((buffer, back)) = self.shared_buffer(16 * MOST as u64) else {
            return Err(windows::core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY));
        };
        // SAFETY: plain object creation; a fresh mapping of that size.
        unsafe {
            std::ptr::write_bytes(back, 0, 16 * MOST);
            let mut heap: Option<ID3D12QueryHeap> = None;
            self.device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC {
                    Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                    Count: 2 * MOST as u32,
                    NodeMask: 0,
                },
                &mut heap,
            )?;
            let heap = heap.ok_or_else(windows::core::Error::empty)?;
            let allocator: ID3D12CommandAllocator = self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let mut lists = Vec::with_capacity(2 * MOST);
            for at in 0..2 * MOST as u32 {
                let list: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
                list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, at);
                list.ResolveQueryData(&heap, D3D12_QUERY_TYPE_TIMESTAMP, at, 1, &buffer, u64::from(at) * 8);
                list.Close()?;
                lists.push(list.cast()?);
            }
            let (mut card, mut pc) = (0, 0);
            self.queue.GetClockCalibration(&mut card, &mut pc)?;
            Ok(Stamps {
                lists,
                back: back as usize,
                _kept: (allocator, heap, buffer),
                calibrated: (card, pc),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What goes ahead of and behind a submission says when it ran, by the
    /// PC's own counter.
    #[test]
    fn a_submissions_times_are_the_pcs() {
        let Some(host) = host() else {
            return;
        };
        timeline_ready();
        let Some((ahead, behind)) = lists(5) else {
            return;
        };
        let sent = crate::timeline::ticks();
        {
            let mut submitted = host.submitted.lock().unwrap_or_else(|e| e.into_inner());
            *submitted += 1;
            // SAFETY: closed lists; the fence and event are the host's, under
            // its lock.
            unsafe {
                host.queue.ExecuteCommandLists(&[Some(ahead), Some(behind)]);
                host.queue.Signal(&host.fence, *submitted).unwrap();
                host.fence.SetEventOnCompletion(*submitted, host.event).unwrap();
                WaitForSingleObject(host.event, INFINITE);
            }
        }
        let done = crate::timeline::ticks();
        let times = times(6);
        assert_eq!(times[4], (0, 0), "not run");
        let (from, to) = times[5];
        // (The two clocks agree to some tens of microseconds.)
        let near = 2_000;
        assert!(from <= to && from + near >= sent && to <= done + near, "{sent} {from} {to} {done}");
    }
}
