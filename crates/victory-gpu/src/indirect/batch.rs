//! Clamps of many executions in one pass: the arguments of every indirect
//! execution between two points where they may change are bounded together,
//! at the first of them.
//!
//! An execution that asks for a record already in the table (the title's
//! tile classes) reads the same clamped one.
//!
//! What the records charge the list's budget is as for records clamped
//! alone, times the executions that read them.

use windows::Win32::Graphics::Direct3D12::*;

use super::{Around, Clamp, uav_barrier};
use crate::host::transition;

/// The jobs of one pass.
pub(crate) const JOBS: u32 = 256;
/// The bytes of its table in upload memory: the count, then the jobs.
pub(crate) const TABLE: u64 = 16 + 16 * JOBS as u64;
/// The bytes of a clamped record's place in the scratch buffer.
pub(crate) const SLOT: u32 = 32;
/// The source buffers a pass can read.
pub(crate) const SOURCES: usize = 4;
/// The most executions charged by one job: the most a record costs (2^26)
/// as often still fits 32 bits.
pub(crate) const TIMES: u32 = 31;

pub(crate) const SOURCE: &str = r#"
#define RS "SRV(t0), SRV(t1), SRV(t2), SRV(t3), SRV(t4), UAV(u0), UAV(u1)"
ByteAddressBuffer src0 : register(t0);
ByteAddressBuffer src1 : register(t1);
ByteAddressBuffer src2 : register(t2);
ByteAddressBuffer src3 : register(t3);
ByteAddressBuffer jobs : register(t4);
RWByteAddressBuffer dst_args : register(u0);
RWByteAddressBuffer budget : register(u1);
static const uint BUDGET = 1u << 27;
static const uint GROUPS = 1u << 20;
groupshared uint costs[64];
groupshared uint fits[64];
groupshared uint largest_asked;
groupshared uint all_asked;
uint load(uint source, uint at) {
    if (source == 0) return src0.Load(at);
    if (source == 1) return src1.Load(at);
    if (source == 2) return src2.Load(at);
    return src3.Load(at);
}
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID, uint gi : SV_GroupIndex) {
    uint count = jobs.Load(0);
    bool active = id.x < count;
    uint4 job = uint4(0, 0, 0, 0);
    uint record[5] = {0, 0, 0, 0, 0};
    uint cost = 0;
    uint asked = 0;
    uint kind = 0;
    uint words = 0;
    if (gi == 0) {
        largest_asked = 0;
        all_asked = 0;
    }
    // As the clamp of one execution (`super::SOURCE`) bounds them.
    if (active) {
        job = jobs.Load4(16 + id.x * 16);
        kind = (job.x >> 4) & 15;
        words = min((job.x >> 8) & 15, 5);
        for (uint w = 0; w < words; w++) {
            record[w] = load(job.x & 3, job.y + w * 4);
        }
        if (kind <= 1) {
            if (record[0] >= 1u << 23) {
                record[0] = 0;
                record[1] = 0;
            }
            record[1] = min(record[1], (1u << 26) / max(record[0], 1u));
            cost = record[0] * record[1];
        } else if (kind == 2) {
            uint3 g = uint3(record[0], record[1], record[2]);
            g = min(g, uint3(g.x <= WIDEST ? WIDEST : 65535u, 65535u, 65535u));
            if ((g.y != 0 && g.x > GROUPS / g.y) || (g.z != 0 && g.x * g.y > GROUPS / g.z)) {
                g = uint3(0, 0, 0);
            }
            cost = g.x * g.y * g.z * 64u;
            record[0] = g.x;
            record[1] = g.y;
            record[2] = g.z;
        } else {
            if (record[0] > 65535u) {
                record[0] = 0;
                record[1] = 0;
                record[2] = 0;
            }
            asked = record[0];
            record[1] = min(record[1], 1u);
            record[2] = min(record[2], 1u);
            cost = record[0] * record[1] * record[2] * 128u;
        }
        cost *= job.x >> 16;
    }
    costs[gi] = cost;
    GroupMemoryBarrierWithGroupSync();
    if (active && kind > 2) {
        uint was;
        InterlockedMax(largest_asked, asked, was);
        InterlockedAdd(all_asked, min(asked, 1u << 24), was);
    }
    uint first = id.x - gi;
    uint n = count > first ? min(64u, count - first) : 0u;
    if (gi == 0 && n > 0) {
        uint before;
        budget.InterlockedOr(0, 0, before);
        [allow_uav_condition]
        while (before < BUDGET) {
            uint next = before;
            for (uint j = 0; j < n; j++) {
                uint c = costs[j];
                if (c > 0 && next < BUDGET) {
                    next = (BUDGET - next < c) ? (REST ? BUDGET : next) : (next + c);
                }
            }
            if (next == before) break;
            uint prev;
            budget.InterlockedCompareExchange(0, before, next, prev);
            if (prev == before) break;
            before = prev;
        }
        uint at = before;
        uint refused = 0;
        uint biggest = 0;
        for (uint k = 0; k < n; k++) {
            uint c = costs[k];
            uint fit = 1;
            if (c > 0) {
                fit = (at < BUDGET && BUDGET - at >= c) ? 1u : 0u;
                at = (at >= BUDGET || BUDGET - at < c) ? (REST ? BUDGET : at) : (at + c);
                refused += 1 - fit;
                biggest = max(biggest, c);
            }
            fits[k] = fit;
        }
        // Diagnostics: the records that run empty for want of budget, and
        // the largest cost there was (small ones are not told).
        uint told;
        if (refused > 0) {
            budget.InterlockedAdd(12, refused, told);
        }
        if (biggest >= (1u << 22)) {
            budget.InterlockedMax(16, biggest, told);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    if (gi == 0 && n > 0 && all_asked > 0) {
        uint was;
        budget.InterlockedMax(4, largest_asked, was);
        budget.InterlockedAdd(8, all_asked, was);
    }
    if (!active) {
        return;
    }
    if (fits[gi] == 0) {
        record[0] = 0;
        record[1] = 0;
        if (kind > 1) {
            record[2] = 0;
        }
    }
    for (uint s = 0; s < words; s++) {
        dst_args.Store(job.z + s * 4, record[s]);
    }
}
"#;

/// The words of a record of `kind` (PC argument numbering).
pub(crate) fn words(kind: u32) -> Option<u32> {
    super::min_stride(kind).map(|bytes| bytes / 4)
}

/// A job's first word.
pub(crate) fn job(source: u32, kind: u32, times: u32) -> u32 {
    source | (kind << 4) | (words(kind).unwrap_or(0) << 8) | (times << 16)
}

/// The jobs of a pass as the CPU fills them ([`SOURCE`]), and where it
/// keeps which record is where.
#[derive(Default)]
pub(crate) struct Table {
    /// The table in upload memory ([`TABLE`] bytes).
    at: usize,
    #[cfg(feature = "diagnostics")]
    sized: bool,
    #[cfg(feature = "diagnostics")]
    meshes: u32,
    /// The place of the first clamped record in the scratch buffer.
    base: u32,
    /// The host copies it can read, by number ([`crate::shadow::Shadow::id`];
    /// 0: none).
    sources: [u64; SOURCES],
    /// How many executions each job is charged for so far.
    times: Vec<u32>,
    /// The job of a record (source, offset, kind).
    known: std::collections::HashMap<(u32, u32, u32), u32>,
}

impl Table {
    /// A table at `at`, with no job yet.
    ///
    /// # Safety
    /// `at` is [`TABLE`] bytes of mapped upload memory, 4-byte aligned, that
    /// live until the list that reads them is recorded and is not run
    /// before.
    pub(crate) unsafe fn new(at: *mut u8, base: u32, sources: [u64; SOURCES]) -> Table {
        // SAFETY: the caller's.
        unsafe {
            let header = at.cast::<u32>();
            for (i, v) in [0, 0, 1, 1].into_iter().enumerate() {
                header.add(i).write_volatile(v);
            }
        }
        let mut table = Table::default();
        table.at = at as usize;
        table.base = base;
        table.sources = sources;
        table
    }

    /// The place in the scratch buffer of the record of `kind` at `offset`
    /// of the host copy `id`, once the pass has run: a job of it is added if
    /// there is none. `None` if the pass cannot read that copy or has no
    /// room for another job.
    pub(crate) fn place(&mut self, id: u64, offset: u64, kind: u32) -> Option<u64> {
        let source = self.sources.iter().position(|&s| s == id)? as u32;
        let offset = u32::try_from(offset).ok().filter(|o| o.is_multiple_of(4))?;
        offset.checked_add(words(kind)? * 4)?;
        let key = (source, offset, kind);
        let table = self.at as *mut u32;
        let place = |i: u32| u64::from(self.base + i * SLOT);
        if let Some(&i) = self.known.get(&key)
            && self.times[i as usize] < TIMES
        {
            self.times[i as usize] += 1;
            // SAFETY: the job's own word of the table ([`Table::new`]).
            unsafe { table.add(4 + 4 * i as usize).write_volatile(job(source, kind, self.times[i as usize])) };
            #[cfg(feature = "diagnostics")]
            { self.meshes += u32::from(kind == 10); }
            return Some(place(i));
        }
        let i = self.times.len() as u32;
        if i == JOBS {
            return None;
        }
        self.times.push(1);
        self.known.insert(key, i);
        // SAFETY: inside the table: fewer than [`JOBS`] jobs before it.
        unsafe {
            let to = table.add(4 + 4 * i as usize);
            to.write_volatile(job(source, kind, 1));
            to.add(1).write_volatile(offset);
            to.add(2).write_volatile(self.base + i * SLOT);
            to.add(3).write_volatile(0);
            if i.is_multiple_of(64) {
                table.add(1).write_volatile(i / 64 + 1);
            }
            table.write_volatile(i + 1);
        }
        #[cfg(feature = "diagnostics")]
        { self.meshes += u32::from(kind == 10); }
        Some(place(i))
    }
}

/// Records the pass over the table at `jobs` (a host address) into `dst`,
/// which is in state `before` and left in `INDIRECT_ARGUMENT`: after every
/// earlier shader write (the title's argument passes), before what reads
/// the clamped records.
#[cfg(test)]
pub(super) fn record(
    c: &Clamp,
    l: &ID3D12GraphicsCommandList,
    dst: (&ID3D12Resource, D3D12_RESOURCE_STATES),
    budget: u64,
    sources: [u64; SOURCES],
    jobs: u64,
) {
    record_sized(c, l, dst.0, Around::States(dst.1), budget, sources, jobs, None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_sized(
    c: &Clamp,
    l: &ID3D12GraphicsCommandList,
    dst: &ID3D12Resource,
    around: Around,
    budget: u64,
    sources: [u64; SOURCES],
    jobs: u64,
    dispatch: Option<(&ID3D12CommandSignature, &ID3D12Resource, u64)>,
) {
    // SAFETY: an open list; our root signature and pipeline; live buffers:
    // the sources and the table are read in the common state (promoted).
    unsafe {
        match around {
            Around::States(before) => l.ResourceBarrier(&[
                uav_barrier(),
                transition(dst, before, D3D12_RESOURCE_STATE_UNORDERED_ACCESS),
            ]),
            Around::Named(precise, stages) => {
                if stages != 0 {
                    crate::waits::record(precise, &crate::waits::after(stages));
                }
            }
        }
        l.SetComputeRootSignature(&c.root);
        l.SetPipelineState(&c.pso);
        for (i, source) in sources.into_iter().enumerate() {
            l.SetComputeRootShaderResourceView(i as u32, source);
        }
        l.SetComputeRootShaderResourceView(SOURCES as u32, jobs);
        l.SetComputeRootUnorderedAccessView(SOURCES as u32 + 1, dst.GetGPUVirtualAddress());
        l.SetComputeRootUnorderedAccessView(SOURCES as u32 + 2, budget);
        if let Some((signature, table, offset)) = dispatch {
            l.ExecuteIndirect(signature, 1, table, offset + 4, None::<&ID3D12Resource>, 0);
        } else {
            l.Dispatch(JOBS.div_ceil(64), 1, 1);
        }
        match around {
            Around::States(_) => l.ResourceBarrier(&[transition(
                dst,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            )]),
            Around::Named(precise, _) => crate::waits::record(precise, &crate::waits::CLAMPED),
        }
    }
    #[cfg(feature = "diagnostics")]
    crate::waits::count(match around {
        Around::Named(_, 0) => crate::waits::Count::ClampsAsRecorded,
        _ => crate::waits::Count::ClampsAfterWait,
    });
}

#[cfg(feature = "diagnostics")]
static WORK: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

#[cfg(feature = "diagnostics")]
impl Table {
    pub(super) fn count_sized(mut self, sized: bool) -> Self {
        self.sized = sized;
        self
    }
}

#[cfg(feature = "diagnostics")]
impl Drop for Table {
    fn drop(&mut self) {
        if self.sized {
            use std::sync::atomic::Ordering::Relaxed;
            WORK[0].fetch_add(1, Relaxed);
            WORK[1].fetch_add(self.times.len().div_ceil(64) as u64, Relaxed);
            WORK[2].fetch_add(u64::from(self.meshes), Relaxed);
        }
    }
}

#[cfg(feature = "diagnostics")]
pub(crate) fn line(frames: f64) {
    let [batches, groups, meshes] = WORK.each_ref().map(|c| c.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / frames);
    if batches > 0.0 {
        let reserve = batches * f64::from(JOBS.div_ceil(64));
        tracing::info!("sized indirect batches per frame: {batches:.1} passes, {groups:.1} of {reserve:.1} GPU groups, {meshes:.1} mesh draws");
    }
}
