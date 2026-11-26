//! What changed on the CPU's side of copies in the card's memory goes up on
//! a copy queue of its own (`VICTORY_UP_QUEUE=1`; off by default; A/B-able
//! with `VICTORY_AB=<s>,VICTORY_UP_QUEUE`).
//!
//! Without it the drawing queue copies the changed ranges from the PC's
//! memory to the card's side before a submission's lists: in a replay of
//! capture replay5, 269 copies and 76.6 MB, about 5 ms in which the drawing
//! queue does nothing else (PCIe-bound). With it the copy queue brings the
//! same bytes into a buffer of ours in the card's memory as soon as the
//! submission is prepared, while the drawing queue still runs earlier
//! submissions; the drawing queue waits for that copy (a fence) and then
//! copies from our buffer to the card's side, in the card's memory (about
//! 0.2 ms for those 76 MB), at the same place in its order as before. So
//! every list of the submission and of later ones finds the new bytes, and
//! no earlier one does: the card's side is still written only by the
//! drawing queue.
//!
//! With the sends of `send` (`VICTORY_SEND_OWN`, on by default) the bytes are
//! in the submission's own memory in the PC's: that comes up whole, in one
//! copy, and the sends (copies and pairs of offset and word) read it from our
//! buffer instead.
//!
//! The copy queue reads the CPU's side when the drawing queue would have,
//! or earlier, and never before the bytes were put there (it is sent after
//! that). Our buffer and lists belong to the submission's plan
//! (`CardPlan::up`) and come back with it when its submission has run
//! (`Host::card_done`), so they are not written again while in use.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use windows::Win32::Graphics::Direct3D12::*;

/// Whether it is on now (`VICTORY_UP_QUEUE=1`, or its phase of `VICTORY_AB`).
pub(super) fn on() -> bool {
    #[cfg(feature = "diagnostics")]
    if let Some(on) = crate::ab::pick("VICTORY_UP_QUEUE") {
        return on;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_UP_QUEUE").is_some_and(|v| v == "1"))
}

/// The most bytes one submission sends this way; more go as before.
const MOST: u64 = 512 << 20;

/// Our buffers are made in sizes of powers of two from this on.
const LEAST: u64 = 4 << 20;

/// One submission's: a copy list and our buffer in the card's memory.
pub(super) struct Slot {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    bounce: Option<(ID3D12Resource, u64)>,
}

// SAFETY: free-threaded D3D12 objects, used by one submission at a time.
unsafe impl Send for Slot {}

impl Slot {
    /// Our buffer, which holds what went up.
    pub(super) fn bounce(&self) -> Option<&ID3D12Resource> {
        self.bounce.as_ref().map(|b| &b.0)
    }
}

struct Queue {
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    sent: u64,
    free: Vec<Slot>,
}

// SAFETY: as `Slot`.
unsafe impl Send for Queue {}

/// `None` inside: none could be made (then nothing goes this way).
static QUEUE: Mutex<Option<Option<Queue>>> = Mutex::new(None);

/// Since the last frame-rate line: submissions, bytes, copies sent this
/// way; submissions that went as before because they were too big or
/// something could not be made.
pub(super) static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn make_queue(device: &ID3D12Device) -> Option<Queue> {
    // SAFETY: plain object creation.
    let made = unsafe {
        (|| -> windows::core::Result<Queue> {
            let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_COPY,
                Priority: D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0,
                ..Default::default()
            })?;
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            Ok(Queue { queue, fence, sent: 0, free: Vec::new() })
        })()
    };
    match made {
        Ok(q) => {
            super::name(&q.queue, || "copies in the card's memory: up queue".into());
            Some(q)
        }
        Err(e) => {
            tracing::warn!(error = %e, "no copy queue for what goes up to copies in the card's memory: it goes on the drawing queue");
            None
        }
    }
}

fn make_slot(device: &ID3D12Device) -> Option<Slot> {
    // SAFETY: plain object creation.
    unsafe {
        (|| -> windows::core::Result<Slot> {
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)?;
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)?;
            list.Close()?;
            Ok(Slot { allocator, list, bounce: None })
        })()
        .ok()
    }
}

/// One range: the card side, its offset, the CPU side, the bytes.
pub(super) type Range<'a> = (&'a ID3D12Resource, u64, &'a ID3D12Resource, u64);

/// Sends `ranges` (card side, offset, CPU side, bytes; the same offset on
/// both sides) up on the copy queue into our buffer and makes the drawing
/// queue wait for that; then `record` gets, for each range, our buffer and
/// the range's offset in it, to copy from there on the drawing queue.
/// `None`: nothing was sent (too many bytes, or nothing could be made); the
/// caller copies as before.
///
/// # Safety
/// Live buffers, the ranges lie in them; the CPU's sides hold the bytes to
/// send; `drawing` is the queue the lists that read the card sides go to.
pub(super) unsafe fn send(
    host: &super::Host,
    drawing: &ID3D12CommandQueue,
    ranges: &[Range<'_>],
    mut record: impl FnMut(usize, &ID3D12Resource, u64),
) -> Option<Slot> {
    let total: u64 = ranges.iter().map(|r| r.3.next_multiple_of(256)).sum();
    if total == 0 || total > MOST {
        STATS[3].fetch_add(1, Relaxed);
        return None;
    }
    let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(q) = guard.get_or_insert_with(|| make_queue(&host.device)).as_mut() else {
        STATS[3].fetch_add(1, Relaxed);
        return None;
    };
    let Some(mut slot) = q.free.pop().or_else(|| make_slot(&host.device)) else {
        STATS[3].fetch_add(1, Relaxed);
        return None;
    };
    if slot.bounce.as_ref().is_none_or(|b| b.1 < total) {
        let size = total.next_power_of_two().max(LEAST);
        slot.bounce = None;
        let Some(made) = host.uav_buffer(size) else {
            q.free.push(slot);
            STATS[3].fetch_add(1, Relaxed);
            return None;
        };
        super::name(&made, || format!("copies in the card's memory: what goes up, {} MB", size >> 20));
        slot.bounce = Some((made, size));
    }
    let Some((bounce, _)) = slot.bounce.clone() else {
        return None;
    };
    // SAFETY: the slot's last submission has run (it came back with its
    // plan); the caller's for the buffers.
    let recorded = unsafe {
        (|| -> windows::core::Result<()> {
            slot.allocator.Reset()?;
            slot.list.Reset(&slot.allocator, None)?;
            let mut at = 0u64;
            for &(_, offset, cpu, bytes) in ranges {
                slot.list.CopyBufferRegion(&bounce, at, cpu, offset, bytes);
                at += bytes.next_multiple_of(256);
            }
            slot.list.Close()?;
            q.queue.ExecuteCommandLists(&[Some(windows::core::Interface::cast(&slot.list)?)]);
            q.sent += 1;
            q.queue.Signal(&q.fence, q.sent)?;
            Ok(())
        })()
    };
    if let Err(e) = recorded {
        // (Nothing waits for a copy that was not sent.)
        tracing::warn!(error = %e, "what goes up to copies in the card's memory could not go on the copy queue: as before");
        q.free.push(slot);
        STATS[3].fetch_add(1, Relaxed);
        return None;
    }
    // SAFETY: a fence of ours that the copy queue signals.
    if let Err(e) = unsafe { drawing.Wait(&q.fence, q.sent) } {
        // The drawing queue cannot be made to wait: wait here, so that
        // what follows still finds the bytes.
        tracing::warn!(error = %e, "the drawing queue could not wait for the up queue: waited on the CPU");
        // SAFETY: as above; a null event waits until the value is reached.
        let _ = unsafe { q.fence.SetEventOnCompletion(q.sent, windows::Win32::Foundation::HANDLE::default()) };
    }
    let mut at = 0u64;
    for (i, r) in ranges.iter().enumerate() {
        record(i, &bounce, at);
        at += r.3.next_multiple_of(256);
    }
    STATS[0].fetch_add(1, Relaxed);
    STATS[1].fetch_add(ranges.iter().map(|r| r.3).sum::<u64>(), Relaxed);
    STATS[2].fetch_add(ranges.len() as u64, Relaxed);
    Some(slot)
}

/// A plan's slot, back once its submission has run.
pub(super) fn give_back(slot: Slot) {
    let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(Some(q)) = guard.as_mut() {
        q.free.push(slot);
    }
}

/// For the frame-rate line.
pub(super) fn line(frames: f64) {
    let [subs, bytes, copies, before] = [0, 1, 2, 3].map(|i| STATS[i].swap(0, Relaxed) as f64 / frames);
    if subs + before > 0.0 {
        tracing::info!(
            "copies in the card's memory brought up on a copy queue per frame (VICTORY_UP_QUEUE): {subs:.1} submissions, \
             {copies:.0} ranges, {:.1} MB; {before:.1} submissions sent up on the drawing queue (too big, or nothing to send with)",
            bytes / 1048576.0
        );
    }
}
