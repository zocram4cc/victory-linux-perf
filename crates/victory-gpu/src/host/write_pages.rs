use super::{ID3D12Resource, host};
use crate::inplace::WriteMapping;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const PAGE: usize = 65536;
const BYTES: usize = 8192;
const BATCH: usize = 32;

struct Slot {
    buffer: ID3D12Resource,
    data: usize,
    offset: usize,
}

static FREE: Mutex<Vec<Slot>> = Mutex::new(Vec::new());
static COUNTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

struct Mask(Option<Slot>);

impl Mask {
    fn take() -> Option<Arc<Self>> {
        let mut free = FREE.lock().unwrap_or_else(|e| e.into_inner());
        if free.is_empty() {
            let (buffer, start) = host()?.shared_buffer((BYTES * BATCH) as u64)?;
            free.extend((0..BATCH).map(|i| Slot {
                buffer: buffer.clone(),
                data: start as usize + i * BYTES,
                offset: i * BYTES,
            }));
        }
        let slot = free.pop()?;
        unsafe {
            std::ptr::write_bytes(slot.data as *mut u8, 0, BYTES);
        }
        Some(Arc::new(Self(Some(slot))))
    }

    fn gpu(&self) -> u64 {
        let slot = self.0.as_ref().unwrap();
        unsafe { slot.buffer.GetGPUVirtualAddress() + slot.offset as u64 }
    }

    fn ranges(&self, start: usize, end: usize) -> Vec<(usize, usize)> {
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let data = self.0.as_ref().unwrap().data as *const AtomicU32;
        for i in 0..(end - start).div_ceil(PAGE * 32) {
            let mut bits = unsafe { &*data.add(i) }.load(Ordering::Acquire);
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let at = start + (32 * i + bit) * PAGE;
                if at >= end {
                    break;
                }
                let to = end.min(at + PAGE);
                if let Some(last) = ranges.last_mut()
                    && last.1 == at
                {
                    last.1 = to;
                } else {
                    ranges.push((at, to));
                }
            }
        }
        ranges
    }
}

impl Drop for Mask {
    fn drop(&mut self) {
        if let Some(slot) = self.0.take() {
            FREE.lock().unwrap_or_else(|e| e.into_inner()).push(slot);
        }
    }
}

#[derive(Default)]
pub(super) struct Recording {
    targets: Vec<((usize, usize), Arc<Mask>)>,
    pending: Vec<(usize, usize)>,
}

impl Recording {
    pub(super) fn target(&mut self, broad: &mut Vec<(usize, usize)>) -> u64 {
        if let &[range] = self.pending.as_slice() {
            if let Some((_, mask)) = self.targets.iter().find(|(r, _)| *r == range) {
                self.pending.clear();
                return mask.gpu();
            }
            if let Some(mask) = Mask::take() {
                let gpu = mask.gpu();
                self.targets.push((range, mask));
                self.pending.clear();
                return gpu;
            }
        }
        broad.append(&mut self.pending);
        static SINK: OnceLock<ID3D12Resource> = OnceLock::new();
        let sink = SINK.get_or_init(|| {
            host()
                .and_then(|h| h.uav_buffer(BYTES as u64))
                .unwrap_or_else(|| {
                    super::fail(format_args!("no culling write bitmap or fallback UAV"))
                })
        });
        unsafe { sink.GetGPUVirtualAddress() }
    }

    pub(super) fn add(&mut self, at: usize, len: usize) {
        let range = (at, at + len);
        if !self.pending.contains(&range) {
            self.pending.push(range);
        }
    }

    pub(super) fn capture(&self, flights: &mut Vec<Flight>) {
        flights.extend(self.targets.iter().map(|&((start, end), ref mask)| Flight {
            start,
            end,
            mask: mask.clone(),
            mapping: WriteMapping::capture(start, end),
        }));
    }
}

pub(super) struct Flight {
    start: usize,
    end: usize,
    mask: Arc<Mask>,
    mapping: WriteMapping,
}

impl Flight {
    pub(super) fn range(&self) -> (usize, usize) {
        (self.start, self.end)
    }

    pub(super) fn land(&self, dirty: &mut Vec<(usize, usize)>) {
        let ranges = self.mask.ranges(self.start, self.end);
        self.mapping.select(&ranges).landed();
        COUNTS[0].fetch_add(1, Ordering::Relaxed);
        COUNTS[1].fetch_add(
            (self.end - self.start).div_ceil(PAGE) as u64,
            Ordering::Relaxed,
        );
        COUNTS[2].fetch_add(
            ranges
                .iter()
                .map(|&(a, b)| (b - a).div_ceil(PAGE) as u64)
                .sum(),
            Ordering::Relaxed,
        );
        dirty.extend(ranges);
    }
}

pub(super) fn line(frames: f64) {
    let [m, possible, touched] =
        [0, 1, 2].map(|i| COUNTS[i].swap(0, Ordering::Relaxed) as f64 / frames);
    if m > 0.0 {
        tracing::info!(
            "culling writes per frame: {m:.1} fenced views, {touched:.1} pages reported by the GPU out of {possible:.1} possible"
        );
    }
}
