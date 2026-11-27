use std::sync::Mutex;

use windows::Win32::Graphics::Direct3D12::{
    D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT, ID3D12Resource,
};

use crate::host::Host;

const MAX_IDLE_BYTES: u64 = 16 << 20;
const MAX_IDLE_BUFFERS: usize = 64;

static IDLE: Mutex<Pool> = Mutex::new(Pool {
    buffers: Vec::new(),
    bytes: 0,
});

#[cfg(test)]
thread_local! {
    pub(super) static TEST_POOLING: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

/// Whether a read-back buffer goes back to a pool when its list has reset,
/// for the next list to take (`VICTORY_READBACK_POOL=1`; off by default: each
/// copy back creates a buffer of its own, and frees it after its list).
pub(super) fn pooling() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_POOLING.get() {
        return on;
    }
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("VICTORY_READBACK_POOL").is_some_and(|v| v == "1"));
    *ON
}

pub(super) struct Readback {
    pub(super) resource: ID3D12Resource,
    bytes: u64,
    pooled: bool,
}

impl Readback {
    pub(super) fn acquire(host: &Host, size: u64, pooled: bool) -> Option<Self> {
        if !pooled {
            let resource = host.read_back_buffer(size)?;
            return Some(Self { resource, bytes: size, pooled });
        }
        let alignment = u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT);
        let bytes = size.max(1).checked_next_multiple_of(alignment)?;
        if bytes <= MAX_IDLE_BYTES {
            let mut idle = IDLE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(buffer) = idle.take(bytes) {
                return Some(buffer);
            }
        }
        let resource = host.read_back_buffer(bytes)?;
        Some(Self { resource, bytes, pooled })
    }

    /// Return the buffer only after its command list has finished and reset.
    pub(super) fn recycle(self) {
        if !self.pooled {
            return;
        }
        let mut idle = IDLE.lock().unwrap_or_else(|e| e.into_inner());
        if idle.buffers.len() < MAX_IDLE_BUFFERS && self.bytes <= MAX_IDLE_BYTES - idle.bytes {
            idle.bytes += self.bytes;
            idle.buffers.push(self);
        }
    }
}

struct Pool {
    buffers: Vec<Readback>,
    bytes: u64,
}

impl Pool {
    fn take(&mut self, bytes: u64) -> Option<Readback> {
        let (index, _) = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| b.bytes >= bytes)
            .min_by_key(|(_, b)| b.bytes)?;
        let buffer = self.buffers.swap_remove(index);
        self.bytes -= buffer.bytes;
        Some(buffer)
    }
}
