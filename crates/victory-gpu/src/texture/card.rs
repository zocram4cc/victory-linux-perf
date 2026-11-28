use super::{Last, Staging};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

const SOURCE: &str = r#"
#define RS "SRV(t0), SRV(t1), UAV(u0), RootConstants(num32BitConstants=8,b0)"
ByteAddressBuffer pixels: register(t0);
ByteAddressBuffer axes: register(t1);
RWByteAddressBuffer outputBuffer: register(u0);
cbuffer Args: register(b0) {uint xs; uint ys; uint w; uint h; uint shift; uint pitch; uint dst; uint unused;};
uint addr(uint x, uint2 row) { return row.x + (axes.Load(xs + 4*x) ^ row.y); }
[RootSignature(RS)]
[numthreads(32,4,1)]
void main(uint3 id: SV_DispatchThreadID) {
    uint byteX = 4*id.x;
    if (id.y >= h || byteX >= (w << shift)) return;
    uint2 row = axes.Load2(ys + 8*id.y);
    uint value = 0;
    if (shift >= 2) {
        value = pixels.Load(addr(byteX >> shift,row) + (byteX & ((1u << shift)-1u)));
    } else {
        uint step = 1u << shift;
        uint mask = shift == 0 ? 255u : 65535u;
        for (uint j=0; j<4; j+=step) {
            uint x = (byteX+j) >> shift;
            if (x < w) {
                uint a = addr(x,row);
                value |= ((pixels.Load(a & ~3u) >> (8*(a & 3u))) & mask) << (8*j);
            }
        }
    }
    outputBuffer.Store(dst+id.y*pitch+byteX,value);
}
"#;

struct Kernel(ID3D12RootSignature, ID3D12PipelineState);
unsafe impl Send for Kernel {}
unsafe impl Sync for Kernel {}

fn kernel() -> Option<&'static Kernel> {
    static K: OnceLock<Option<Kernel>> = OnceLock::new();
    K.get_or_init(|| {
        let dev = &crate::host::host()?.device;
        let bytes = crate::host::compile(SOURCE, s!("main"), s!("cs_5_1")).ok()?;
        unsafe {
            let root = dev.CreateRootSignature(0, &bytes).ok()?;
            let pso = dev
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: bytes.as_ptr().cast(),
                        BytecodeLength: bytes.len(),
                    },
                    ..Default::default()
                })
                .ok()?;
            Some(Kernel(root, pso))
        }
    })
    .as_ref()
}

pub(super) fn on() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var_os("VICTORY_CARD_TEXTURES").is_some_and(|v| v == "1"))
}

static BYTES: AtomicU64 = AtomicU64::new(0);
struct Room(u64);
impl Drop for Room {
    fn drop(&mut self) {
        BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}

pub(super) enum Plan {
    Linear(Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>),
    Tiled(Work),
}

pub(super) struct Work {
    out: ID3D12Resource,
    table: ID3D12Resource,
    groups: Vec<([u32; 8], u32, u32)>,
    kernel: &'static Kernel,
    _room: Room,
}

impl Plan {
    fn new(s: &Staging) -> Option<Self> {
        if s.layout.desc.swizzle == victory_formats::texture::Swizzle::LINEAR
            && s.memory as usize & 511 == 0
        {
            let layouts = s
                .footprints
                .iter()
                .enumerate()
                .map(|(i, fp)| {
                    let (level, layer) = s.part(i);
                    let m = s.layout.mips.get(level as usize)?;
                    let mut f = *fp;
                    f.Offset = s
                        .layout
                        .slice_size
                        .checked_mul(u64::from(layer))?
                        .checked_add(m.offset)?;
                    f.Footprint.RowPitch = m.pitch.checked_mul(s.layout.desc.element_bytes)?;
                    let end = f
                        .Offset
                        .checked_add(u64::from(m.height - 1) * u64::from(f.Footprint.RowPitch))?
                        .checked_add(u64::from(m.width) * u64::from(s.layout.desc.element_bytes))?;
                    (f.Offset % 512 == 0
                        && f.Footprint.RowPitch % 256 == 0
                        && m.height == s.rows[i]
                        && f.Footprint.Depth == 1
                        && end <= s.read as u64)
                        .then_some(f)
                })
                .collect::<Option<Vec<_>>>();
            if let Some(layouts) = layouts {
                return Some(Self::Linear(layouts));
            }
        }
        let kernel = kernel()?;
        let host = crate::host::host()?;
        let mut words = Vec::<u32>::new();
        let mut groups = Vec::new();
        let el = s.layout.desc.element_bytes;
        for (i, fp) in s.footprints.iter().enumerate() {
            let (mip, slice) = s.part(i);
            let (x, y) = s.layout.gpu_axes(mip, slice)?;
            let (w, h) = (u32::try_from(x.len()).ok()?, u32::try_from(y.len()).ok()?);
            let xs = u32::try_from(words.len() * 4).ok()?;
            words.extend(x);
            let ys = u32::try_from(words.len() * 4).ok()?;
            words.extend(y.into_iter().flatten());
            if h != s.rows[i].checked_mul(fp.Footprint.Depth)?
                || w.checked_mul(el)? > fp.Footprint.RowPitch
            {
                return None;
            }
            let args = [
                xs,
                ys,
                w,
                h,
                el.trailing_zeros(),
                fp.Footprint.RowPitch,
                u32::try_from(fp.Offset).ok()?,
                0,
            ];
            groups.push((args, (w * el).div_ceil(128), h.div_ceil(4)));
        }
        let table_bytes = (words.len() * 4) as u64;
        let len = s.bytes.next_multiple_of(4);
        let n = len.next_multiple_of(65536) + table_bytes.next_multiple_of(65536);
        BYTES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |b| {
                b.checked_add(n).filter(|&t| t <= 192 << 20)
            })
            .ok()?;
        let room = Room(n);
        let out = host.uav_buffer(len)?;
        let (table, data) = host.upload_buffer(table_bytes)?;
        unsafe {
            std::ptr::copy_nonoverlapping(words.as_ptr(), data.cast::<u32>(), words.len());
        }
        Some(Self::Tiled(Work {
            out,
            table,
            groups,
            kernel,
            _room: room,
        }))
    }
}

// Descriptor rechecks can create/destroy staging for the same shapes hundreds
// of times. Only the layout and GPU scratch are shared, not the source, target,
// version or dirty mask. Every dispatch -> copy -> COMMON is on Host::queue,
// never on the independent copy worker. Compare exact footprints and swizzle.
struct Cached {
    layout: victory_formats::texture::Layout,
    levels: [u32; 3],
    size: (usize, u64),
    aligned: bool,
    rows: Vec<u32>,
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    plan: Arc<Plan>,
}

impl Cached {
    fn matches(&self, s: &Staging) -> bool {
        self.levels == [s.first, s.level0, s.mips]
            && self.size == (s.read, s.bytes)
            && self.aligned == (s.memory as usize & 511 == 0)
            && self.layout == s.layout
            && self.rows == s.rows
            && self.footprints == s.footprints
    }
    fn bytes(&self) -> u64 {
        match self.plan.as_ref() {
            Plan::Tiled(w) => w._room.0,
            Plan::Linear(_) => 512,
        }
    }
}

fn make_plan(s: &Staging) -> Option<Arc<Plan>> {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("VICTORY_TEXTURE_PLANS").is_some_and(|v| v == "1")) {
        return Plan::new(s).map(Arc::new);
    }
    static CACHE: std::sync::Mutex<Vec<Cached>> = std::sync::Mutex::new(Vec::new());
    let lock = || CACHE.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut c = lock();
        if let Some(i) = c.iter().rposition(|e| e.matches(s)) {
            let found = c.remove(i);
            let plan = found.plan.clone();
            c.push(found);
            return Some(plan);
        }
    }
    let plan = Arc::new(Plan::new(s)?);
    let e = Cached {
        layout: s.layout.clone(),
        levels: [s.first, s.level0, s.mips],
        size: (s.read, s.bytes),
        aligned: s.memory as usize & 511 == 0,
        rows: s.rows.clone(),
        footprints: s.footprints.clone(),
        plan: plan.clone(),
    };
    let n = e.bytes();
    if n <= 32 << 20 {
        let mut c = lock();
        // Another refresh may have populated this shape while we prepared it.
        if let Some(e) = c.iter().find(|e| e.matches(s)) {
            return Some(e.plan.clone());
        }
        let mut total = c.iter().map(Cached::bytes).sum::<u64>() + n;
        while c.len() >= 48 || total > 32 << 20 {
            total -= c.remove(0).bytes();
        }
        c.push(e);
    }
    Some(plan)
}

#[derive(Clone)]
pub(super) struct Ready(Arc<Plan>, ID3D12Resource, u64);

pub(super) fn prepare(s: &Staging, last: &mut Last) -> bool {
    if !on()
        || s.read < 4
        || s.read > 64 << 20
        || s.memory as usize & 15 != 0
        || s.layout.desc.samples != 1
        || (s.mips > 1 && crate::texstore::on())
    {
        return false;
    }
    let start = s.memory as usize;
    if !crate::um::touches_card(start, s.read) {
        return false;
    }
    let len = s.read.next_multiple_of(4);
    if !crate::um::holds(start, len) {
        return false;
    }
    let Some((buf, offset)) = crate::um::mirror_at(start as u64, len as u64) else {
        if s.card_hot.load(Ordering::Relaxed) && !s.card_missed.swap(true, Ordering::Relaxed) {
            tracing::info!(addr = ?s.memory, len, "hot texture not in a GPU mirror");
        }
        return false;
    };
    let Some(plan) = s.card_plan.get_or_init(|| {
        let plan = make_plan(s)?;
        if s.read >= 1 << 20 || s.card_hot.load(Ordering::Relaxed) {
            tracing::info!(addr = ?s.memory, len, linear = matches!(plan.as_ref(), Plan::Linear(_)), "texture uploaded from shared GPU memory");
        }
        Some(plan)
    }) else { return false; };
    if s.on_demand {
        let judged = s.judgement();
        let held = s.held_levels(judged);
        if (s.first..s.mips).any(|m| m >= 32 || held & (1u32 << m) == 0) {
            return false;
        }
        s.judged[0].store(judged[0], Ordering::Relaxed);
        s.judged[1].store(judged[1], Ordering::Relaxed);
        s.detiled.store(held, Ordering::Relaxed);
    }
    let now = crate::frame::current();
    let changed = crate::watch::writes(start, start + len, true);
    let stamp = crate::inplace::texture_stamp(start, start + len);
    let unchanged = last.card.is_some()
        && stamp.is_some_and(|(ver, sealed)| sealed && last.card_version == Some(ver))
        && changed
            .as_ref()
            .is_some_and(|(_, w)| last.asked != 0 && w.iter().all(|&(_, t)| t <= last.asked))
        && now.saturating_sub(last.card_checked) < 120;
    if let Some((at, _)) = changed {
        last.asked = at;
    }
    last.card_version = stamp.map(|s| s.0);
    if unchanged {
        return true;
    }
    last.card_checked = now;
    last.hash = None;
    last.levels.clear();
    last.card = Some(Ready(plan.clone(), buf, offset));
    s.dirty_levels.store(u32::MAX, Ordering::Release);
    s.dirty.store(true, Ordering::Release);
    true
}

impl Ready {
    pub(super) fn record<'a>(&'a self, l: &ID3D12GraphicsCommandList) -> &'a ID3D12Resource {
        let Plan::Tiled(p) = self.0.as_ref() else {
            unsafe {
                l.ResourceBarrier(&[crate::indirect::uav_barrier()]);
            }
            return &self.1;
        };
        unsafe {
            l.ResourceBarrier(&[
                crate::indirect::uav_barrier(),
                crate::host::transition(
                    &p.out,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ]);
            l.SetComputeRootSignature(&p.kernel.0);
            l.SetPipelineState(&p.kernel.1);
            l.SetComputeRootShaderResourceView(0, self.1.GetGPUVirtualAddress() + self.2);
            l.SetComputeRootShaderResourceView(1, p.table.GetGPUVirtualAddress());
            l.SetComputeRootUnorderedAccessView(2, p.out.GetGPUVirtualAddress());
            for (c, x, y) in &p.groups {
                l.SetComputeRoot32BitConstants(3, 8, c.as_ptr().cast(), 0);
                l.Dispatch(*x, *y, 1);
            }
            l.ResourceBarrier(&[crate::host::transition(
                &p.out,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
        }
        &p.out
    }
    pub(super) fn footprint(
        &self,
        i: usize,
        f: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    ) -> D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
        match self.0.as_ref() {
            Plan::Tiled(_) => f,
            Plan::Linear(footprints) => D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: self.2 + footprints[i].Offset,
                ..footprints[i]
            },
        }
    }
    pub(super) fn done(&self, l: &ID3D12GraphicsCommandList) {
        if let Plan::Tiled(p) = self.0.as_ref() {
            unsafe {
                l.ResourceBarrier(&[crate::host::transition(
                    &p.out,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_COMMON,
                )]);
            }
        }
    }
}
