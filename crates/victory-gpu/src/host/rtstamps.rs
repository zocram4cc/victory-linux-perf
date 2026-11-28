//! Diagnostics: how long the card works on each compute dispatch, by its own
//! timestamps (`VICTORY_RT_STAMPS=1`, diagnostics builds; for the ray-tracing port
//! (`crate::port_rt`)). A timestamp goes in front of and behind every
//! dispatch the title records (direct and indirect), and the pair is read
//! back some frames later. The sums are kept by pipeline and by kind: a stage
//! of the ray-tracing structure builder, a program that walks the structures
//! (it reads title memory by address), and the others.
//!
//! What a pair measures: from the moment the work ahead of the first stamp
//! was done to the moment the dispatch's was. Work of other queues that runs
//! at the same time is in it; a dispatch that overlaps the one ahead of it is
//! counted from the end of that one. Sums of many dispatches are the card's
//! time on them to within that.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use windows::Win32::Graphics::Direct3D12::*;

use super::Host;

/// Pairs of stamps in the ring: they are read long before they come round.
const PAIRS: usize = 1 << 17;

/// What kind of dispatch a pair is of.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Kind {
    /// A stage of the structure builder.
    Builder,
    /// A program that reads title memory by address (walks the structures).
    Walks,
    /// Any other compute dispatch.
    Other,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Builder => "the structure builder",
            Kind::Walks => "programs that walk the structures",
            Kind::Other => "other dispatches",
        }
    }
}

struct Ring {
    heap: ID3D12QueryHeap,
    back: ID3D12Resource,
    /// The mapping of `back`: two tick counts a pair.
    data: usize,
    ticks_per_ms: f64,
}

// SAFETY: free-threaded objects, and a mapping read and cleared behind a lock.
unsafe impl Send for Ring {}
// SAFETY: as above.
unsafe impl Sync for Ring {}

static RING: std::sync::OnceLock<Option<Ring>> = std::sync::OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A pair of stamps asked for and not read yet.
struct Pending {
    pair: usize,
    kind: Kind,
    /// The pipeline (its state object's address) and its name.
    pso: usize,
    frame: u64,
}

#[derive(Default)]
struct Sums {
    pending: Vec<Pending>,
    names: HashMap<usize, String>,
    /// By kind: dispatches, ticks.
    kinds: HashMap<Kind, (u64, u64)>,
    /// By pipeline: its kind, dispatches, ticks.
    pipelines: HashMap<usize, (Kind, u64, u64)>,
    /// Pairs never written (their list did not run).
    lost: u64,
}

static SUMS: Mutex<Option<Sums>> = Mutex::new(None);

pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RT_STAMPS").is_some_and(|v| v == "1"))
}

fn ring() -> Option<&'static Ring> {
    RING.get_or_init(|| {
        let host = super::host()?;
        host.make_rt_ring()
            .inspect_err(|e| tracing::warn!(error = %e, "no stamps around dispatches"))
            .ok()
    })
    .as_ref()
}

impl Host {
    fn make_rt_ring(&self) -> windows::core::Result<Ring> {
        // SAFETY: plain object creation; the mapping holds `PAIRS` pairs.
        unsafe {
            let mut heap: Option<ID3D12QueryHeap> = None;
            self.device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC {
                    Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                    Count: 2 * PAIRS as u32,
                    NodeMask: 0,
                },
                &mut heap,
            )?;
            let heap = heap.ok_or_else(windows::core::Error::empty)?;
            let back = self.read_back_buffer(16 * PAIRS as u64).ok_or_else(windows::core::Error::empty)?;
            let mut data = std::ptr::null_mut();
            back.Map(0, None, Some(&mut data))?;
            std::ptr::write_bytes(data.cast::<u8>(), 0, 16 * PAIRS);
            Ok(Ring {
                heap,
                back,
                data: data as usize,
                ticks_per_ms: self.queue.GetTimestampFrequency()? as f64 / 1000.0,
            })
        }
    }
}

/// The first stamp of a dispatch about to be recorded into `list`; the pair's
/// place, for [`after`].
pub(crate) fn before(list: &ID3D12GraphicsCommandList) -> Option<usize> {
    if !on() {
        return None;
    }
    let ring = ring()?;
    let pair = (NEXT.fetch_add(1, Relaxed) as usize) % PAIRS;
    // SAFETY: an open list; the query is inside the heap.
    unsafe { list.EndQuery(&ring.heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * pair as u32) };
    Some(pair)
}

/// The second stamp of the dispatch just recorded, and its pair read back.
pub(crate) fn after(list: &ID3D12GraphicsCommandList, pair: Option<usize>, kind: Kind, pso: usize, name: &str) {
    let (Some(pair), Some(ring)) = (pair, RING.get().and_then(Option::as_ref)) else {
        return;
    };
    // SAFETY: an open list; queries and destination inside heap and buffer
    // (a read-back buffer is always a copy's destination).
    unsafe {
        list.EndQuery(&ring.heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * pair as u32 + 1);
        list.ResolveQueryData(&ring.heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * pair as u32, 2, &ring.back, 16 * pair as u64);
    }
    let mut sums = SUMS.lock().unwrap_or_else(|e| e.into_inner());
    let sums = sums.get_or_insert_with(Default::default);
    if !sums.names.contains_key(&pso) {
        sums.names.insert(pso, name.to_owned());
    }
    sums.pending.push(Pending {
        pair,
        kind,
        pso,
        frame: crate::frame::current(),
    });
    if sums.pending.len() % 4096 == 0 {
        read(ring, sums, crate::frame::current());
    }
}

/// Reads the pairs of frames that have run.
fn read(ring: &Ring, sums: &mut Sums, now: u64) {
    let pending = std::mem::take(&mut sums.pending);
    for p in pending {
        if p.frame + 3 > now {
            sums.pending.push(p);
            continue;
        }
        // SAFETY: inside the mapping; the card writes whole words.
        let (start, end) = unsafe {
            let at = (ring.data as *mut u64).add(2 * p.pair);
            (at.read_volatile(), at.add(1).read_volatile())
        };
        if start == 0 || end == 0 {
            if p.frame + 30 > now {
                sums.pending.push(p);
            } else {
                sums.lost += 1;
            }
            continue;
        }
        // SAFETY: as above.
        unsafe {
            let at = (ring.data as *mut u64).add(2 * p.pair);
            at.write_volatile(0);
            at.add(1).write_volatile(0);
        }
        let took = end.saturating_sub(start);
        let k = sums.kinds.entry(p.kind).or_default();
        *k = (k.0 + 1, k.1 + took);
        let e = sums.pipelines.entry(p.pso).or_insert((p.kind, 0, 0));
        *e = (e.0, e.1 + 1, e.2 + took);
    }
}

/// For the frame-rate line: the sums since the last one, per frame.
pub(crate) fn line(frames: f64) {
    let Some(ring) = RING.get().and_then(Option::as_ref) else {
        return;
    };
    let mut guard = SUMS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sums) = guard.as_mut() else {
        return;
    };
    read(ring, sums, crate::frame::current());
    let ms = |ticks: u64| ticks as f64 / ring.ticks_per_ms / frames;
    let mut kinds: Vec<String> = [Kind::Builder, Kind::Walks, Kind::Other]
        .iter()
        .map(|k| {
            let (n, t) = sums.kinds.get(k).copied().unwrap_or_default();
            format!("{} {:.1} ms in {:.0}", k.name(), ms(t), n as f64 / frames)
        })
        .collect();
    kinds.push(format!("{:.1} pairs not written", sums.lost as f64 / frames));
    tracing::info!("renderer port RT: the card's time on dispatches per frame, by their own stamps (VICTORY_RT_STAMPS=1): {}", kinds.join(", "));
    let mut top: Vec<(usize, (Kind, u64, u64))> = sums.pipelines.drain().collect();
    top.sort_unstable_by_key(|(_, (_, _, t))| std::cmp::Reverse(*t));
    for (pso, (kind, n, t)) in top.into_iter().take(12) {
        let name = sums.names.get(&pso).map_or("?", String::as_str);
        tracing::info!(
            "renderer port RT: dispatches by their own stamps per frame: {:.2} ms in {:.1} ({}) {name}",
            ms(t),
            n as f64 / frames,
            kind.name()
        );
    }
    sums.kinds.clear();
    sums.lost = 0;
}
