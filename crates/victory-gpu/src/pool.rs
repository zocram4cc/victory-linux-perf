//! Host textures placed in heaps mapped ahead of time (roadmap S.6,
//! `docs/design/s6-premapped-textures.md`).
//!
//! The title makes hundreds of textures a frame over its own memory (the
//! open world: 230-700 a frame, all "at a title address"). Made committed,
//! each gets fresh memory whose GPU page tables the kernel updates
//! asynchronously; work already submitted that reads a slot pointed at it
//! faults (the S.1 device loss). Placed in a heap that is already mapped,
//! a texture changes no page table.
//!
//! - Heaps of [`HEAP_BYTES`], made up front ([`initial_mb`]) and grown one
//!   at a time. A heap is used only once a submission sent after it was
//!   made has finished, so its mapping is visible to every submission
//!   that can name a texture in it.
//! - First fit over a free list per heap, at the placement alignment.
//! - Exact lifetimes (design rule 7): the pool holds one reference to each
//!   texture it placed. At each submission boundary ([`collect`]) a texture
//!   only the pool still holds goes through `retire()`, and its block is
//!   free again once the submissions running then have finished.
//!
//! `VICTORY_TEXTURE_POOL=1` turns it on; `VICTORY_TEXTURE_POOL_MB` sets the
//! initial size (default 2,048).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

/// Bytes of one pool heap.
const HEAP_BYTES: u64 = 256 << 20;
/// The default placement alignment.
const ALIGN: u64 = 64 << 10;
/// Textures larger than this stay committed.
const LARGEST: u64 = 64 << 20;

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TEXTURE_POOL").is_some_and(|v| v == "1"))
}

fn initial_mb() -> u64 {
    std::env::var("VICTORY_TEXTURE_POOL_MB").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(2048)
}

struct PoolHeap {
    heap: ID3D12Heap,
    /// Free ranges: offset to bytes.
    free: BTreeMap<u64, u64>,
    /// Usable once a submission numbered above this has finished.
    ready_after: u64,
}

struct Live {
    resource: ID3D12Resource,
    heap: usize,
    offset: u64,
    bytes: u64,
}

#[derive(Default)]
struct Pool {
    heaps: Vec<PoolHeap>,
    live: Vec<Live>,
    /// A heap is being made: no second one until it is ready.
    growing: bool,
}

static POOL: Mutex<Option<Pool>> = Mutex::new(None);

/// Since the last frame line: textures placed, textures that fell back to
/// committed memory (no ready room), blocks given back, heaps made; then,
/// as levels, bytes in use and bytes in all heaps.
pub(crate) static STATS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

fn pool() -> std::sync::MutexGuard<'static, Option<Pool>> {
    POOL.lock().unwrap_or_else(|e| e.into_inner())
}

fn make_heap(device: &ID3D12Device) -> Option<ID3D12Heap> {
    let desc = D3D12_HEAP_DESC {
        SizeInBytes: HEAP_BYTES,
        Properties: D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        },
        Alignment: 0,
        // Resource heap tier 2 (probed): buffers and every kind of texture.
        Flags: D3D12_HEAP_FLAG_ALLOW_ALL_BUFFERS_AND_TEXTURES,
    };
    let mut heap: Option<ID3D12Heap> = None;
    // SAFETY: plain heap creation; the out pointer is ours.
    let made = unsafe { device.CreateHeap(&desc, &mut heap) };
    if let Err(e) = made {
        tracing::warn!(error = %e, "texture pool: no heap");
        return None;
    }
    STATS[3].fetch_add(1, Relaxed);
    STATS[5].fetch_add(HEAP_BYTES, Relaxed);
    heap
}

/// Takes `bytes` at `align` from `free`: the offset.
fn carve(free: &mut BTreeMap<u64, u64>, bytes: u64, align: u64) -> Option<u64> {
    let (start, len, at) = free.iter().find_map(|(&start, &len)| {
        let at = start.div_ceil(align) * align;
        (at + bytes <= start + len).then_some((start, len, at))
    })?;
    free.remove(&start);
    if at > start {
        free.insert(start, at - start);
    }
    if at + bytes < start + len {
        free.insert(at + bytes, start + len - at - bytes);
    }
    Some(at)
}

/// Gives `[offset, offset + bytes)` back to `free`, joined to its
/// neighbours.
fn give_back(free: &mut BTreeMap<u64, u64>, offset: u64, bytes: u64) {
    let (mut start, mut end) = (offset, offset + bytes);
    if let Some((&before, &len)) = free.range(..start).next_back()
        && before + len == start
    {
        free.remove(&before);
        start = before;
    }
    if let Some(&len) = free.get(&end) {
        free.remove(&end);
        end += len;
    }
    free.insert(start, end - start);
}

/// A host texture placed in the pool, or `None` (the caller makes it
/// committed): the pool is off, the texture is too large, or no ready heap
/// has room (a new heap is then made, ready for later textures).
///
/// # Safety
/// `desc` and `clear` as for `CreatePlacedResource`.
pub(crate) unsafe fn place(
    device: &ID3D12Device,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: Option<*const D3D12_CLEAR_VALUE>,
) -> Option<ID3D12Resource> {
    if !on() {
        return None;
    }
    // SAFETY: the caller's description.
    let info = unsafe { device.GetResourceAllocationInfo(0, std::slice::from_ref(&*desc)) };
    if info.SizeInBytes == u64::MAX || info.SizeInBytes == 0 || info.SizeInBytes > LARGEST {
        return None;
    }
    let align = info.Alignment.max(ALIGN);
    let bytes = info.SizeInBytes.div_ceil(ALIGN) * ALIGN;
    let (finished, started) = crate::host::submissions();
    let mut guard = pool();
    let pool = guard.get_or_insert_with(|| {
        let mut pool = Pool::default();
        for _ in 0..initial_mb().div_ceil(HEAP_BYTES >> 20) {
            if let Some(heap) = make_heap(device) {
                pool.heaps.push(PoolHeap {
                    heap,
                    free: BTreeMap::from([(0, HEAP_BYTES)]),
                    ready_after: started,
                });
            }
        }
        tracing::info!(heaps = pool.heaps.len(), mb = pool.heaps.len() as u64 * (HEAP_BYTES >> 20), "texture pool made");
        pool
    });
    let found = pool.heaps.iter_mut().enumerate().find_map(|(i, h)| {
        if finished <= h.ready_after {
            return None;
        }
        carve(&mut h.free, bytes, align).map(|offset| (i, offset))
    });
    if pool.growing && pool.heaps.last().is_some_and(|h| finished > h.ready_after) {
        pool.growing = false;
    }
    let Some((index, offset)) = found else {
        STATS[1].fetch_add(1, Relaxed);
        if !pool.growing
            && let Some(heap) = make_heap(device)
        {
            pool.heaps.push(PoolHeap {
                heap,
                free: BTreeMap::from([(0, HEAP_BYTES)]),
                ready_after: started,
            });
            pool.growing = true;
        }
        return None;
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: a live heap; the block is ours and large enough for the
    // description's allocation; the caller's description and clear value.
    let result = unsafe { device.CreatePlacedResource(&pool.heaps[index].heap, offset, desc, state, clear, &mut made) };
    let Some(resource) = made.filter(|_| result.is_ok()) else {
        give_back(&mut pool.heaps[index].free, offset, bytes);
        STATS[1].fetch_add(1, Relaxed);
        if let Err(e) = result {
            tracing::warn!(error = %e, "texture pool: placement refused");
        }
        return None;
    };
    pool.live.push(Live {
        resource: resource.clone(),
        heap: index,
        offset,
        bytes,
    });
    STATS[0].fetch_add(1, Relaxed);
    STATS[4].fetch_add(bytes, Relaxed);
    Some(resource)
}

/// A pool texture nobody else holds: dropped by `retire()` once the
/// submissions running when it was found have finished, then its block is
/// free again.
struct Freed(Option<ID3D12Resource>, usize, u64, u64);

impl Drop for Freed {
    fn drop(&mut self) {
        drop(self.0.take());
        let Freed(_, heap, offset, bytes) = *self;
        if let Some(pool) = pool().as_mut()
            && let Some(h) = pool.heaps.get_mut(heap)
        {
            give_back(&mut h.free, offset, bytes);
        }
        STATS[2].fetch_add(1, Relaxed);
        STATS[4].fetch_sub(bytes, Relaxed);
    }
}

/// The references to `resource` besides the caller's.
fn others(resource: &ID3D12Resource) -> u32 {
    let raw = resource.as_raw();
    // SAFETY: a live COM object; AddRef then Release leaves the count as it
    // was, and Release returns it.
    unsafe {
        let vtable = *(raw as *const *const windows::core::IUnknown_Vtbl);
        ((*vtable).AddRef)(raw);
        ((*vtable).Release)(raw).saturating_sub(1)
    }
}

/// At a submission boundary: pool textures only the pool still holds are
/// retired.
pub(crate) fn collect() {
    if !on() {
        return;
    }
    let dead: Vec<Freed> = {
        let mut guard = pool();
        let Some(pool) = guard.as_mut() else { return };
        let (dead, live): (Vec<Live>, Vec<Live>) = std::mem::take(&mut pool.live).into_iter().partition(|l| others(&l.resource) == 0);
        pool.live = live;
        dead.into_iter().map(|l| Freed(Some(l.resource), l.heap, l.offset, l.bytes)).collect()
    };
    // Outside the lock: `retire` may drop at once, and `Freed` takes it.
    for freed in dead {
        crate::host::retire(freed);
    }
}

/// With the frame line: what the pool did per frame.
pub(crate) fn pool_line(frames: f64) {
    if !on() {
        return;
    }
    let [placed, fell_back, freed, heaps] = [0, 1, 2, 3].map(|i| STATS[i].swap(0, Relaxed));
    let (used, all) = (STATS[4].load(Relaxed), STATS[5].load(Relaxed));
    let live = pool().as_ref().map_or(0, |p| p.live.len());
    tracing::info!(
        "texture pool per frame: {:.1} placed, {:.1} committed for want of ready room, {:.1} given back; {heaps} heaps made; {live} live, {} of {} MB in use",
        placed as f64 / frames,
        fell_back as f64 / frames,
        freed as f64 / frames,
        used >> 20,
        all >> 20
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carving_and_giving_back_join_the_free_list() {
        let mut free = BTreeMap::from([(0, 1 << 20)]);
        let a = carve(&mut free, 128 << 10, 64 << 10).unwrap();
        let b = carve(&mut free, 64 << 10, 64 << 10).unwrap();
        assert_eq!((a, b), (0, 128 << 10));
        give_back(&mut free, a, 128 << 10);
        assert_eq!(free.get(&0), Some(&(128 << 10)));
        give_back(&mut free, b, 64 << 10);
        assert_eq!(free, BTreeMap::from([(0, 1 << 20)]));
    }

    #[test]
    fn carving_honours_alignment() {
        let mut free = BTreeMap::from([(64 << 10, 16 << 20)]);
        let at = carve(&mut free, 4 << 20, 4 << 20).unwrap();
        assert_eq!(at, 4 << 20);
        assert_eq!(free.get(&(64 << 10)), Some(&((4 << 20) - (64 << 10))));
    }

    #[test]
    fn a_block_too_large_is_refused() {
        let mut free = BTreeMap::from([(0, 1 << 20)]);
        assert_eq!(carve(&mut free, 2 << 20, 64 << 10), None);
        assert_eq!(free, BTreeMap::from([(0, 1 << 20)]));
    }
}
