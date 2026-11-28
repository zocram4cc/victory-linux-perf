use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

const PAGE: usize = 65536;
pub(super) const LIMIT: usize = 64;
const SOURCE: &str = r#"
#define RS "SRV(t0), UAV(u0), RootConstants(num32BitConstants=3,b0)"
ByteAddressBuffer source:register(t0);
RWByteAddressBuffer page:register(u0);
cbuffer Params:register(b0){uint first;uint end;uint fresh;};
[RootSignature(RS)]
[numthreads(64,1,1)]
void main(uint tid:SV_DispatchThreadID){
 uint at=(first&~3u)+4*tid;
 if(at>=end)return;
 uint mask=(0xffffffffu<<(8u*(max(at,first)-at))) & (0xffffffffu>>(8u*(4-min(4u,end-at))));
 uint v=source.Load(at);
 uint p=page.Load(65536+at);
 if(fresh || ((p^v)&mask)){
   page.Store(65536+at,(p&~mask)|(v&mask));
   if(mask==0xffffffffu)page.Store(at,v);else page.Store(at,(page.Load(at)&~mask)|(v&mask));
 }
}
"#;

pub(crate) fn on() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var_os("VICTORY_CARD_ARENAS").is_some_and(|v| v == "1"))
}

struct Kernel(ID3D12RootSignature, ID3D12PipelineState);
unsafe impl Send for Kernel {}
unsafe impl Sync for Kernel {}
fn kernel() -> Option<&'static Kernel> {
    static K: OnceLock<Option<Kernel>> = OnceLock::new();
    K.get_or_init(|| {
        let h = crate::host::host()?;
        let bytes = crate::host::compile(SOURCE, s!("main"), s!("cs_5_1")).ok()?;
        unsafe {
            let root = h.device.CreateRootSignature(0, &bytes).ok()?;
            let ps = h
                .device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: bytes.as_ptr().cast(),
                        BytecodeLength: bytes.len(),
                    },
                    ..Default::default()
                })
                .ok()?;
            Some(Kernel(root, ps))
        }
    })
    .as_ref()
}

pub(super) fn page() -> Option<ID3D12Resource> {
    kernel()?;
    crate::host::host()?.uav_buffer((PAGE * 2) as u64)
}

pub(crate) struct Job {
    dst: ID3D12Resource,
    src: ID3D12Resource,
    offset: u64,
    args: [u32; 3],
}
unsafe impl Send for Job {}
static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
static PENDING: AtomicUsize = AtomicUsize::new(0);

pub(super) fn add(dst: &ID3D12Resource, at: u64, first: usize, end: usize, fresh: bool) {
    if first >= end {
        return;
    }
    let (src, off) = crate::um::mirror_at(at, PAGE as u64).unwrap_or_else(|| {
        let (buf, data) = crate::host::host()
            .and_then(|h| h.upload_buffer(PAGE as u64))
            .unwrap_or_else(|| {
                crate::host::fail(format_args!("no upload buffer for an arena page"))
            });
        let lo = first & !3;
        crate::host::copy_mapped(
            data.wrapping_add(lo),
            (at as usize + lo) as *const u8,
            end.next_multiple_of(4) - lo,
            false,
        );
        (buf, 0)
    });
    let mut work = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    work.push(Job {
        dst: dst.clone(),
        src,
        offset: off,
        args: [first as u32, end as u32, u32::from(fresh)],
    });
    PENDING.store(work.len(), Ordering::Release);
}

pub(crate) fn take() -> Vec<Job> {
    if PENDING.load(Ordering::Acquire) == 0 {
        return Vec::new();
    }
    let mut work = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    PENDING.store(0, Ordering::Relaxed);
    std::mem::take(&mut *work)
}

pub(crate) fn record(host: &crate::host::HostList, list: &ID3D12GraphicsCommandList, jobs: &[Job]) {
    let Some(k) = (!jobs.is_empty()).then(kernel).flatten() else {
        return;
    };
    unsafe {
        list.SetComputeRootSignature(&k.0);
        list.SetPipelineState(&k.1);
        for j in jobs {
            list.ResourceBarrier(&[crate::indirect::uav_barrier()]);
            list.SetComputeRootShaderResourceView(0, j.src.GetGPUVirtualAddress() + j.offset);
            list.SetComputeRootUnorderedAccessView(1, j.dst.GetGPUVirtualAddress());
            list.SetComputeRoot32BitConstants(2, 3, j.args.as_ptr().cast(), 0);
            list.Dispatch((j.args[1] - (j.args[0] & !3)).div_ceil(256), 1, 1);
            host.keep(&j.src);
            host.keep(&j.dst);
        }
        list.ResourceBarrier(&[crate::indirect::uav_barrier()]);
    }
}
