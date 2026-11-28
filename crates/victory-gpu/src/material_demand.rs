use crate::{
    descriptor::{self, Heap, View},
    host::{HostList, host},
};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed},
};
use windows::Win32::Graphics::Direct3D12::{D3D12_RANGE, ID3D12Resource};

const LIMIT: usize = 8;
const STRIDE: u32 = 64;
const TABLE_OFFSET: u32 = 688 * 4;
const MAX_AGE: u64 = 4;

pub(crate) fn fields(hash: &str) -> Option<&'static [u32]> {
    match hash {
        "d9fcb8a4425fc59b" | "43fc2af1f70920f1" => Some(&[0, 4, 8]),
        "271a87c54ac648a5" => Some(&[0, 4, 12, 16]),
        "d2e683031b764939" => Some(&[0, 4, 8, 16]),
        "43e24028d4a1faef" | "6590393ed93ff071" | "ee78da6862364ddd" | "c10e7d95d8266d2a"
        | "8a5351822b1a4929" | "09eb0a73df63b87d" | "899ef7a2932ae6c1" | "16053957ff2dff5c"
        | "7c18c77e40e164e4" | "47ec43235084505f" | "247a288b2ba23986" | "6d609ecfb830511f" => {
            Some(&[0, 4, 12, 16, 20])
        }
        "573d1857e903fab5" | "8e9b3db8a3eeba51" | "b8df138593588098" | "b5640b1075483a54"
        | "89afedb5b7720e76" | "085d374c5a795549" | "efc089fe381c43bf" | "0fbc5c91bdab02d2"
        | "072f26dae8824688" | "794198ca2f7c9b67" | "cedd7b38d3be97a1" | "8b901a7fb38c169b" => {
            Some(&[0, 4, 12, 16, 20, 24])
        }
        "3ec62ecabb36f1d2" | "3711fdad456538a1" | "de41d84ca748cc29" | "659fd97066bea4fc"
        | "3f93c9225b740fd9" | "26319e68357cef57" => {
            Some(&[0, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44])
        }
        _ => None,
    }
}

type RecordKey = (u32, &'static [u32]);

#[derive(Default)]
struct Sampler {
    frame: Option<u64>,
    cursor: Option<RecordKey>,
    active: Vec<RecordKey>,
    next: Vec<RecordKey>,
    wrap: Vec<RecordKey>,
}

impl Sampler {
    fn keep_smallest(batch: &mut Vec<RecordKey>, key: RecordKey) {
        if let Err(at) = batch.binary_search(&key)
            && at < LIMIT
        {
            if batch.len() == LIMIT {
                batch.pop();
            }
            batch.insert(at, key);
        }
    }

    fn select(&mut self, frame: u64, key: RecordKey) -> bool {
        if self.frame.is_some_and(|old| frame < old) {
            return false;
        }
        if self.frame != Some(frame) {
            self.active.clear();
            self.active.append(&mut self.next);
            let left = LIMIT - self.active.len();
            self.active.extend(self.wrap.drain(..).take(left));
            self.cursor = self.active.last().copied();
            self.frame = Some(frame);
        }
        if self.cursor.is_none_or(|last| key > last) {
            Self::keep_smallest(&mut self.next, key);
        } else {
            Self::keep_smallest(&mut self.wrap, key);
        }
        if let Some(at) = self.active.iter().position(|&selected| selected == key) {
            self.active.swap_remove(at);
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
pub(crate) struct Demand {
    sampler: Mutex<Sampler>,
    ready: Arc<Mutex<Vec<Observation>>>,
}

struct Observation {
    source: u64,
    view: View,
    frame: u64,
    stamp: u64,
    slots: Vec<u32>,
}

struct Pool {
    back: ID3D12Resource,
    free: AtomicU32,
}

struct Sample(&'static Pool, u32);

impl Sample {
    fn reserve() -> Option<Self> {
        static POOL: OnceLock<Option<Pool>> = OnceLock::new();
        let pool = POOL
            .get_or_init(|| {
                Some(Pool {
                    back: host()?.read_back_buffer(u64::from(32 * STRIDE))?,
                    free: AtomicU32::new(u32::MAX),
                })
            })
            .as_ref()?;
        let bits = pool
            .free
            .fetch_update(Relaxed, Relaxed, |bits| {
                (bits != 0).then(|| bits & (bits - 1))
            })
            .ok()?;
        Some(Self(pool, bits.trailing_zeros()))
    }

    fn indices(&self, bytes: usize, fields: &[u32], count: u32) -> Option<Vec<u32>> {
        let begin = (self.1 * STRIDE) as usize;
        let mut at = std::ptr::null_mut();
        let back = &self.0.back;
        let range = D3D12_RANGE {
            Begin: begin,
            End: begin + bytes,
        };
        unsafe {
            back.Map(0, Some(&range), Some(&mut at)).ok()?;
            let mut slots = Vec::new();
            if !at.is_null() {
                let words =
                    std::slice::from_raw_parts(at.cast::<u8>().add(begin).cast::<u32>(), bytes / 4);
                for field in fields {
                    let index = words[(field / 4) as usize] & 0x07ff_ffff;
                    if index < count && !slots.contains(&index) {
                        slots.push(index);
                    }
                }
            }
            back.Unmap(0, Some(&D3D12_RANGE::default()));
            Some(slots)
        }
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        self.0.free.fetch_or(1 << self.1, Relaxed);
    }
}

impl Demand {
    pub(crate) fn record(
        &self,
        list: &HostList,
        heap: &Heap,
        host: &crate::descriptor::host::HostHeap,
        table: u64,
        bound: u32,
        offset: u32,
        fields: &'static [u32],
    ) {
        let frame = crate::frame::current();
        if !self
            .sampler
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .select(frame, (offset, fields))
        {
            return;
        }
        let Some(handle) = table.checked_add(u64::from(TABLE_OFFSET)) else {
            return;
        };
        if heap.index(handle).is_none_or(|slot| heap.handle(slot) != Some(handle)) {
            return;
        }
        let records = crate::records::on();
        let Some(actual) = bound
            .checked_add(TABLE_OFFSET)
            .filter(|at| records || at % 32 == 0)
            .map(|at| at / 32)
        else {
            return;
        };
        let stamp = host.material_stamp();
        let Some(view @ View::Placed(false, ..)) = descriptor::checked_view(handle) else {
            return;
        };
        let Some((source, start, len)) = descriptor::host::buffer_copy(&view) else {
            return;
        };
        let bytes = fields.last().copied().unwrap_or(STRIDE) + 4;
        if bytes > STRIDE
            || offset % 4 != 0
            || u64::from(offset) + u64::from(bytes) > len
            || !records && host.shows(actual, &source.buffer) != Some(true)
        {
            return;
        }
        let Some(at) = start.checked_add(u64::from(offset)) else {
            return;
        };
        if source.offset_in(at).is_none() || source.left(at) < u64::from(bytes) {
            return;
        }
        let from = source.offset(at);
        let Some(sample) = Sample::reserve() else {
            return;
        };
        let count = if records { heap.count() } else { host.count() };
        let start = heap.start;
        let ready = Arc::downgrade(&self.ready);
        let done = AtomicBool::new(false);
        unsafe {
            list.list.CopyBufferRegion(
                &sample.0.back,
                u64::from(sample.1 * STRIDE),
                &source.buffer,
                from,
                u64::from(bytes),
            );
        }
        list.keep(&source.buffer);
        list.after_gpu(move || {
            if done.swap(true, Relaxed) || crate::frame::current().abs_diff(frame) > MAX_AGE {
                return;
            }
            let Some(ready) = ready.upgrade() else { return };
            let Some(mut slots) = sample.indices(bytes as usize, fields, count) else {
                return;
            };
            if records {
                slots = slots.into_iter().filter_map(|slot| crate::records::slot_at(start + u64::from(slot) * 32)).collect();
            }
            let mut ready = ready.lock().unwrap_or_else(|e| e.into_inner());
            if !slots.is_empty() && ready.len() < 32 {
                static READ: AtomicU64 = AtomicU64::new(0);
                let reads = READ.fetch_add(1, Relaxed) + 1;
                if reads.is_power_of_two() {
                    tracing::info!(
                        frame,
                        reads,
                        "GPU material records read for texture scheduling"
                    );
                }
                ready.push(Observation {
                    source: handle,
                    view: view.clone(),
                    frame,
                    stamp,
                    slots,
                });
            }
        });
    }

    pub(crate) fn take(&self, frame: u64) -> Vec<(u32, u64)> {
        let observations =
            std::mem::take(&mut *self.ready.lock().unwrap_or_else(|e| e.into_inner()));
        let mut wanted = Vec::new();
        for obs in observations {
            if frame.abs_diff(obs.frame) > MAX_AGE || descriptor::checked_view(obs.source).as_ref() != Some(&obs.view) {
                continue;
            }
            for slot in obs.slots {
                if let Some((_, old)) = wanted.iter_mut().find(|(i, _)| *i == slot) {
                    *old = u64::max(*old, obs.stamp);
                } else {
                    wanted.push((slot, obs.stamp));
                }
            }
        }
        wanted
    }
}
