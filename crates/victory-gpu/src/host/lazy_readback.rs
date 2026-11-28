use super::*;

impl Host {
    /// Fetch a completed synchronous submission's page on the direct queue.
    /// These private objects never acquire the submit lock in the fault handler.
    pub(crate) fn defer_card_page(
        &self,
        shadow: &std::sync::Arc<crate::shadow::Shadow>,
        offset: usize,
        send_back: bool,
    ) -> bool {
        use crate::watch::{PAGE, lazy_back};
        use std::sync::atomic::AtomicU64;
        type Reader = (
            Prelude,
            ID3D12Resource,
            usize,
            ID3D12Fence,
            ID3D12CommandQueue,
        );
        static READER: OnceLock<Option<Reader>> = OnceLock::new();
        let Some(((allocator, list), dst, mapped, fence, queue)) = READER.get_or_init(|| {
            let prelude = self.make_prelude()?;
            let (dst, mapped) = self.shared_buffer(PAGE as u64)?;
            let fence = unsafe { self.device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
            Some((prelude, dst, mapped as usize, fence, self.queue.clone()))
        }) else {
            return false;
        };
        let at = shadow.span().0 as usize + offset;
        if !lazy_back::eligible(shadow.id(), at) {
            return false;
        }
        let owner = shadow.clone();
        // SAFETY: the completed plan contains the full page in the staging
        // buffer. Its CPU side is stable until the next submit.
        let baseline = unsafe { std::slice::from_raw_parts(shadow.card_data().add(offset), PAGE) };
        if !lazy_back::arm(at, baseline, move |own| {
            // An unconfirmed submission owns the reusable allocator and
            // buffer until process exit. Leave its source pinned and its page
            // guarded; kernel I/O reports failure without reusing either.
            static FAILED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if FAILED.load(Ordering::Acquire) {
                return Err(());
            }
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let value = NEXT.fetch_add(1, Ordering::Relaxed);
            // SAFETY: the previous call waited for completion before leaving
            // the pager lock. A null event synchronously waits on this fence.
            let result = unsafe {
                (|| -> windows::core::Result<()> {
                    allocator.Reset()?;
                    list.Reset(allocator, None)?;
                    list.CopyBufferRegion(dst, 0, &owner.buffer, offset as u64, PAGE as u64);
                    list.Close()?;
                    queue.ExecuteCommandLists(&[Some(list.cast()?)]);
                    queue.Signal(fence, value)?;
                    fence.SetEventOnCompletion(value, HANDLE::default())?;
                    if fence.GetCompletedValue() != value {
                        return Err(windows::core::Error::from_hresult(
                            windows::Win32::Foundation::E_FAIL,
                        ));
                    }
                    std::ptr::copy_nonoverlapping(*mapped as *const u8, own, PAGE);
                    std::ptr::copy_nonoverlapping(
                        *mapped as *const u8,
                        owner.card_data().add(offset).cast_mut(),
                        PAGE,
                    );
                    Ok(())
                })()
            };
            if let Err(error) = result {
                tracing::error!(%error, at, "deferred GPU readback failed");
                FAILED.store(true, Ordering::Release);
                return Err(());
            }
            Ok(())
        }) {
            return false;
        }
        if send_back {
            shadow.card_changed(offset, offset + PAGE);
        }
        crate::watch::wrote(at, PAGE);
        true
    }
}
