//! A private COPY queue for completed pages. The submission lock proves that
//! live sources are idle at admission and protects their capture on reuse.
//! Immutable GPU snapshots have no dependency on subsequent direct submissions.
use super::*;
use crate::shadow::Shadow;
use crate::watch::{PAGE, lazy_back};
use std::sync::Arc;
use windows::Win32::Foundation::E_FAIL;

#[derive(Clone)]
pub(crate) enum Page {
    Live(Arc<Shadow>, usize),
    Snapshot(Arc<Snapshot>, usize),
}

impl Page {
    fn source(&self) -> (&ID3D12Resource, usize) {
        match self {
            Self::Live(shadow, offset) => (&shadow.buffer, *offset),
            Self::Snapshot(saved, offset) => (&saved.buffer, *offset),
        }
    }

    // Device-local card copies also have a CPU-side reference, which the
    // next submit may read. A mapped shadow has no such second buffer.
    pub(crate) fn needs_cpu_reference(&self) -> bool {
        matches!(self, Self::Live(shadow, _) if shadow.carded())
    }
}

struct Reader {
    queue: ID3D12CommandQueue,
    list: ID3D12GraphicsCommandList,
    allocator: ID3D12CommandAllocator,
    fence: ID3D12Fence,
    sent: u64,
    readback: ID3D12Resource,
    mapped: usize,
    // Set before execution; if completion cannot be proved, retain these
    // references and refuse reuse of the allocator and destination.
    outstanding: Vec<ID3D12Resource>,
}

pub(crate) const BATCH: usize = 64;

pub(crate) struct Snapshot {
    buffer: ID3D12Resource,
    bytes: usize,
}
static SAVED_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
impl Drop for Snapshot {
    fn drop(&mut self) {
        SAVED_BYTES.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

impl Reader {
    fn new(host: &Host) -> Option<Self> {
        let (readback, mapped) = host.shared_buffer((PAGE * BATCH) as u64)?;
        // SAFETY: fresh COM objects, private to this reader.
        unsafe {
            let queue = host
                .device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_COPY,
                    ..Default::default()
                })
                .ok()?;
            let allocator = host
                .device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)
                .ok()?;
            let list: ID3D12GraphicsCommandList = host
                .device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)
                .ok()?;
            list.Close().ok()?;
            let fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
            Some(Self {
                queue,
                list,
                allocator,
                fence,
                sent: 0,
                readback,
                mapped: mapped as usize,
                outstanding: Vec::new(),
            })
        }
    }

    fn copy(&mut self, pages: &[Page], dest: &ID3D12Resource) -> windows::core::Result<()> {
        if !self.outstanding.is_empty() {
            return Err(windows::core::Error::from_hresult(E_FAIL));
        }
        // SAFETY: all sources are complete, the destination holds all pages.
        // Fence completion precedes any CPU access or allocator reuse.
        unsafe {
            self.allocator.Reset()?;
            self.list.Reset(&self.allocator, None)?;
            for (i, page) in pages.iter().enumerate() {
                let (resource, offset) = page.source();
                self.list.CopyBufferRegion(
                    dest,
                    (i * PAGE) as u64,
                    resource,
                    offset as u64,
                    PAGE as u64,
                );
            }
            self.list.Close()?;
            let list = self.list.cast()?;
            self.outstanding.reserve(pages.len() + 1);
            self.outstanding
                .extend(pages.iter().map(|p| p.source().0.clone()));
            self.outstanding.push(dest.clone());
            self.queue.ExecuteCommandLists(&[Some(list)]);
            self.sent += 1;
            self.queue.Signal(&self.fence, self.sent)?;
            self.fence
                .SetEventOnCompletion(self.sent, HANDLE::default())?;
            if self.fence.GetCompletedValue() != self.sent {
                return Err(windows::core::Error::from_hresult(E_FAIL));
            }
            self.outstanding.clear();
        }
        Ok(())
    }
}

fn reader() -> Option<&'static Mutex<Reader>> {
    static READER: OnceLock<Option<Mutex<Reader>>> = OnceLock::new();
    READER
        .get_or_init(|| Some(Mutex::new(Reader::new(host()?)?)))
        .as_ref()
}

pub(crate) fn ready() -> bool {
    reader().is_some()
}

pub(crate) fn fetch(pages: &[Page], dest: &mut [u8]) -> Result<(), ()> {
    assert_eq!(pages.len() * PAGE, dest.len());
    let mut reader = reader()
        .ok_or(())?
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let buffer = reader.readback.clone();
    for (pages, dest) in pages.chunks(BATCH).zip(dest.chunks_mut(BATCH * PAGE)) {
        reader
            .copy(pages, &buffer)
            .map_err(|error| tracing::error!(%error, "deferred GPU fetch failed"))?;
        // SAFETY: copy waited for the entire batch. Each mirror belongs to
        // a live shadow and its full offset passed page admission.
        unsafe {
            std::ptr::copy_nonoverlapping(
                reader.mapped as *const u8,
                dest.as_mut_ptr(),
                dest.len(),
            );
            for (page, bytes) in pages.iter().zip(dest.chunks_exact(PAGE)) {
                if let Page::Live(shadow, at) = page
                    && shadow.carded()
                {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        shadow.card_data().add(*at).cast_mut(),
                        PAGE,
                    );
                }
            }
        }
    }
    Ok(())
}

/// Preserve mapped GPU pages on the card before another submit can reuse the
/// original buffers. No title or CPU-side bytes are fetched by this operation.
pub(crate) fn snapshot(pages: &[Page]) -> Result<Option<Arc<Snapshot>>, ()> {
    let host = host().ok_or(())?;
    let bytes = pages.len() * PAGE;
    // The pager serializes allocation; a released snapshot only reduces the total.
    if SAVED_BYTES.load(Ordering::Relaxed) + bytes > 64 << 20 {
        return Ok(None);
    }
    let Some(buffer) = host.uav_buffer(bytes as u64) else {
        return Ok(None);
    };
    let mut reader = reader()
        .ok_or(())?
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    reader
        .copy(pages, &buffer)
        .map_err(|error| tracing::error!(%error, "deferred GPU snapshot failed"))?;
    SAVED_BYTES.fetch_add(bytes, Ordering::Relaxed);
    Ok(Some(Arc::new(Snapshot { buffer, bytes })))
}

impl Host {
    pub(crate) fn defer_card_page(
        &self,
        shadow: &Arc<Shadow>,
        offset: usize,
        send_back: bool,
    ) -> bool {
        let at = shadow.span().0 as usize + offset;
        if !lazy_back::eligible(shadow.id(), at) || !ready() {
            return false;
        }
        // Compare under the pager lock: a retiring version of this page
        // can still be updating the staging side in a racing fault.
        let matches = |title: &[u8]| {
            // SAFETY: the completed plan owns this full staging page.
            title == unsafe { std::slice::from_raw_parts(shadow.card_data().add(offset), PAGE) }
        };
        if !lazy_back::arm_gpu(at, matches, Page::Live(shadow.clone(), offset)) {
            return false;
        }
        if send_back {
            shadow.card_changed(offset, offset + PAGE);
        }
        crate::watch::wrote(at, PAGE);
        true
    }
}
