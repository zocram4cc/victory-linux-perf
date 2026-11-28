use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{Interface, s};

const LIMIT: usize = 256;
const SOURCE: &str = r#"
#define RS "SRV(t0), UAV(u0), RootConstants(num32BitConstants=2,b0)"
ByteAddressBuffer inputBuffer:register(t0);
RWByteAddressBuffer outputBuffer:register(u0);
cbuffer Keys:register(b0){ uint count; uint index; };
groupshared uint3 bins[64];
[RootSignature(RS)]
[numthreads(64,1,1)]
void main(uint lane:SV_GroupIndex){
    uint n = min(count,12288u);
    uint3 sum = 0;
    for(uint i=lane;i<n;i+=64){
        uint at = count<=12288u || i<4096u ? i : (i<8192u ? ((count/2u)&~1023u)+i-4096u : count-12288u+i);
        uint w=inputBuffer.Load(4u*at);
        uint h=(w ^ (i*2654435761u))*2246822519u;
        h=(h^(h>>13))*3266489917u;
        sum.x+=h;
        sum.y^=(h<<(i&15u)) | (h>>((32u-i)&31u));
        sum.z|=w;
    }
    bins[lane]=sum;
    GroupMemoryBarrierWithGroupSync();
    for(uint width=32;width!=0;width>>=1){
        if(lane<width){uint3 b=bins[lane+width];bins[lane]=uint3(bins[lane].x+b.x,bins[lane].y^b.y,bins[lane].z|b.z);}
        GroupMemoryBarrierWithGroupSync();
    }
    if(lane==0)outputBuffer.Store3(12*index,bins[0]);
}
"#;

type Key = (u64, u64);

type Answer = (u64, bool);

#[derive(Default)]
struct Entry {
    at: u64,
    value: Option<Answer>,
    pending: bool,
}

#[derive(Default)]
struct Work {
    entries: HashMap<Key, Entry>,
    pending: VecDeque<Key>,
}

static WORK: OnceLock<Mutex<Work>> = OnceLock::new();
static DEVICE: OnceLock<Option<Mutex<Unit>>> = OnceLock::new();
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn on() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("VICTORY_CARD_STAMPS").is_some_and(|v| v == "1"))
}

pub(super) fn query(at: u64, len: u64) -> Option<Answer> {
    if !on()
        || FAILED.load(std::sync::atomic::Ordering::Acquire)
        || len == 0
        || len > u32::MAX as u64
        || len & 3 != 0
        || at & 15 != 0
        || !crate::um::touches_card(at as usize, len as usize)
        || !crate::um::covers(at, len)
        || crate::um::piece_end(at) < at + len
        || DEVICE.get().is_some_and(Option::is_none)
    {
        return None;
    }
    let frame = crate::frame::current();
    let mut s = WORK
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if !s.entries.contains_key(&(at, len)) && s.entries.len() >= 8192 {
        s.entries
            .retain(|_, v| v.pending || v.at.saturating_add(240) > frame);
        if s.entries.len() >= 8192 {
            return None;
        }
    }
    let e = s.entries.entry((at, len)).or_default();
    let result = e.value.filter(|_| e.at.saturating_add(4) >= frame);
    if !e.pending && (e.value.is_none() || frame > e.at.saturating_add(1)) {
        e.pending = true;
        s.pending.push_back((at, len));
    }
    Some(result.unwrap_or((0, false)))
}

struct Unit {
    root: ID3D12RootSignature,
    shader: ID3D12PipelineState,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    output: ID3D12Resource,
    _backing: ID3D12Resource,
    mapped: usize,
    serial: u64,
    active: Vec<(Key, ID3D12Resource, u64)>,
}
unsafe impl Send for Unit {}

impl Unit {
    fn new() -> Option<Self> {
        let h = crate::host::host()?;
        let device = &h.device;
        let bc = crate::host::compile(SOURCE, s!("main"), s!("cs_5_1")).ok()?;
        unsafe {
            let root = device.CreateRootSignature(0, &bc).ok()?;
            let shader = device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: bc.as_ptr().cast(),
                        BytecodeLength: bc.len(),
                    },
                    ..Default::default()
                })
                .ok()?;
            let allocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .ok()?;
            let list: ID3D12GraphicsCommandList = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                .ok()?;
            list.Close().ok()?;
            let fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
            let bytes = (LIMIT * 12) as u64;
            let output = h.uav_buffer(bytes)?;
            let back = h.read_back_buffer(bytes)?;
            let mut mapped = std::ptr::null_mut();
            back.Map(0, None, Some(&mut mapped)).ok()?;
            Some(Self {
                root,
                shader,
                allocator,
                list,
                fence,
                output,
                _backing: back,
                mapped: mapped as usize,
                serial: 0,
                active: Vec::new(),
            })
        }
    }
    fn update(&mut self, work: &Mutex<Work>) -> windows::core::Result<()> {
        let frame = crate::frame::current();
        let mut lock = work.lock().unwrap_or_else(|e| e.into_inner());
        if !self.active.is_empty() {
            if unsafe { self.fence.GetCompletedValue() } < self.serial {
                return Ok(());
            }
            for (i, ((at, len), _, _)) in self.active.drain(..).enumerate() {
                let [a, b, z] =
                    unsafe { ((self.mapped + 12 * i) as *const [u32; 3]).read_volatile() };
                if let Some(e) = lock.entries.get_mut(&(at, len)) {
                    e.pending = false;
                    e.at = frame;
                    e.value = Some(if z == 0 {
                        (super::blank_print(len), true)
                    } else {
                        ((u64::from(a) << 32 | u64::from(b)) | 1, false)
                    });
                }
            }
        }
        if lock.pending.is_empty() {
            return Ok(());
        }
        let n = lock.pending.len().min(LIMIT);
        let keys = lock.pending.drain(..n).collect::<Vec<_>>();
        drop(lock);
        for key @ (at, len) in keys {
            if let Some((src, off)) = crate::um::mirror_at(at, len) {
                self.active.push((key, src, off));
            } else {
                work.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entries
                    .remove(&key);
            }
        }
        if self.active.is_empty() {
            return Ok(());
        }
        let h = crate::host::host().unwrap();
        unsafe {
            self.allocator.Reset()?;
            self.list.Reset(&self.allocator, &self.shader)?;
            self.list.SetComputeRootSignature(&self.root);
            self.list.ResourceBarrier(&[
                crate::indirect::uav_barrier(),
                crate::host::transition(
                    &self.output,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ]);
            self.list
                .SetComputeRootUnorderedAccessView(1, self.output.GetGPUVirtualAddress());
            for (i, ((_, len), src, off)) in self.active.iter().enumerate() {
                self.list
                    .SetComputeRootShaderResourceView(0, src.GetGPUVirtualAddress() + off);
                let args = [(*len / 4) as u32, i as u32];
                self.list
                    .SetComputeRoot32BitConstants(2, 2, args.as_ptr().cast(), 0);
                self.list.Dispatch(1, 1, 1);
            }
            self.list.ResourceBarrier(&[crate::host::transition(
                &self.output,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            self.list.CopyBufferRegion(
                &self._backing,
                0,
                &self.output,
                0,
                (12 * self.active.len()) as u64,
            );
            self.list.ResourceBarrier(&[crate::host::transition(
                &self.output,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            )]);
            self.list.Close()?;
            h.queue().ExecuteCommandLists(&[Some(self.list.cast()?)]);
            self.serial += 1;
            h.queue().Signal(&self.fence, self.serial)
        }
    }
}

pub(crate) fn pump() {
    if !on() || FAILED.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let Some(device) = DEVICE.get_or_init(|| {
        let unit = Unit::new();
        if unit.is_none() {
            tracing::warn!(
                "GPU signature sampler unavailable; texture checks use the ordinary path"
            );
        }
        unit.map(Mutex::new)
    }) else {
        return;
    };
    let Some(work) = WORK.get() else {
        return;
    };
    let mut device = device.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = device.update(work) {
        // No completed fence means no valid digest. Leave any submitted
        // resources alive, but never publish or reuse this unit again.
        FAILED.store(true, std::sync::atomic::Ordering::Release);
        tracing::error!(error=%e,"could not sample texture signatures");
    }
}
