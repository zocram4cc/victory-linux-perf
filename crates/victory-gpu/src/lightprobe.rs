//! Diagnostics of the picture's light, frame by frame
//! (`VICTORY_LIGHT_PROBE=<file.csv>`): what the title's exposure programs
//! and the stand-in for the indirect light (`crate::ambient`) hold, written
//! by the card into a ring of records the processor reads a few frames
//! later, one line a frame.
//!
//! What the title does (programs by their names in the containers):
//! - `PS_CalculateExposure` (one 1x1 target a frame) moves the exposure
//!   toward a target by the frame's time (constant 0x3a4, 1/15 s at most) at
//!   0.65 of the difference a second, within a least and a greatest the
//!   title sets (both -5.5 in the two recorded frames: a fixed exposure).
//!   Read here: its inputs, the average luminance (1x1) and the exposure of
//!   the frame before (1x1: exposure, its log, the target).
//! - `ComputeLuxCompressionInfo` (one group, the end of the frame) turns the
//!   scene's luminance range (5x1, first texel) into the factor every
//!   lighting program of the next frame scales light by (16 bytes of title
//!   memory: factor, 1/factor, ...). Read here: that output and its input.
//! - The stand-in's slow mean of the light surfaces send out, and the mean
//!   luminance over sampled pixels of what the title's resolve left, what
//!   the stand-in found, and what was written.
//!
//! Columns: frame, ms since start, average luminance (4), exposure before
//! (4), lux factor output (4), lux input (4), sampled pixels, title's resolve
//! mean, stand-in mean, written mean, stand-in slow mean (rgb).

use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

use crate::host::{HostList, host};

/// The title's programs looked at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// `PS_CalculateExposure`.
    Exposure,
    /// `ComputeLuxCompressionInfo` (and its smooth form).
    Lux,
}

/// The lux program is looked at too (`VICTORY_LIGHT_PROBE_LUX=1`; off: run
/// light-21 lost the device in the pass after it, with a view of the frame
/// before bound as a raw buffer where the plain form has a texture).
fn lux_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_LIGHT_PROBE_LUX").is_some_and(|v| v == "1"))
}

pub(crate) fn role(container: &[u8]) -> Option<Role> {
    let has = |name: &[u8]| container.windows(name.len()).any(|w| w == name);
    if has(b"PS_CalculateExposure") {
        Some(Role::Exposure)
    } else if lux_on() && has(b"ComputeLuxCompressionInfo") {
        Some(Role::Lux)
    } else {
        None
    }
}

/// The exposure program's inputs (root parameter, dwords into its table):
/// the average luminance and the exposure of the frame before.
pub(crate) const AVERAGE: (u8, u32) = (1, 8);
pub(crate) const BEFORE: (u8, u32) = (1, 16);
/// The lux program's input (5x1) and output (16 bytes).
pub(crate) const LUX_IN: (u8, u32) = (1, 8);
pub(crate) const LUX_OUT: (u8, u32) = (1, 24);

fn file() -> Option<&'static Mutex<std::io::BufWriter<std::fs::File>>> {
    static FILE: OnceLock<Option<Mutex<std::io::BufWriter<std::fs::File>>>> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = std::env::var_os("VICTORY_LIGHT_PROBE").filter(|v| !v.is_empty() && v != "0")?;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).ok()?);
        let _ = writeln!(
            f,
            "frame,ms,avg0,avg1,avg2,avg3,exp,logexp,target,exp3,lux,luxinv,lux2,lux3,in0,in1,in2,in3,samples,title,standin,written,mean_r,mean_g,mean_b"
        );
        tracing::info!(path = %std::path::Path::new(&path).display(), "light probe: a line a frame (VICTORY_LIGHT_PROBE)");
        Some(Mutex::new(f))
    })
    .as_ref()
}

/// Whether the probe is on.
pub(crate) fn on() -> bool {
    file().is_some()
}

const RS_EXPOSURE: &str = "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0)), DescriptorTable(SRV(t1)), UAV(u0), UAV(u1), UAV(u2)";
const RS_LUX: &str = "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0)), DescriptorTable(UAV(u3)), UAV(u0)";

/// Dwords a record; records in the ring.
const RECORD: u64 = 64;
const RING: u64 = 64;
/// Records read this many after they were asked for.
const LAG: u64 = 16;

const SOURCE: &str = r#"
cbuffer Set : register(b0) { uint record; uint cells; uint has_stats; uint unused; };
RWByteAddressBuffer ring : register(u0);
#ifdef EXPOSURE
Texture2D<float4> average : register(t0);
Texture2D<float4> before : register(t1);
RWByteAddressBuffer stats : register(u1);
RWStructuredBuffer<float4> mean : register(u2);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void exposure() {
    uint at = record * 256;
    ring.Store4(at, asuint(average.Load(int3(0, 0, 0))));
    ring.Store4(at + 16, asuint(before.Load(int3(0, 0, 0))));
    if (has_stats != 0) {
        ring.Store4(at + 64, stats.Load4(0));
        stats.Store4(0, uint4(0, 0, 0, 0));
        ring.Store4(at + 80, asuint(mean[cells]));
    } else {
        ring.Store4(at + 64, uint4(0, 0, 0, 0));
        ring.Store4(at + 80, uint4(0, 0, 0, 0));
    }
    ring.Store(at + 252, record + 1);
}
#endif
#ifdef LUX
Texture2D<float4> input : register(t0);
RWByteAddressBuffer output : register(u3);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void lux() {
    uint at = record * 256;
    ring.Store4(at + 32, output.Load4(0));
    ring.Store4(at + 48, asuint(input.Load(int3(0, 0, 0))));
}
#endif
"#;

struct Pass {
    exposure: (ID3D12RootSignature, ID3D12PipelineState),
    lux: (ID3D12RootSignature, ID3D12PipelineState),
    ring: ID3D12Resource,
    data: usize,
}
// SAFETY: D3D12 objects are free-threaded; the mapping lives as the buffer.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

fn pipeline(
    device: &ID3D12Device,
    part: &str,
    rs: &str,
    entry: windows::core::PCSTR,
) -> windows::core::Result<(ID3D12RootSignature, ID3D12PipelineState)> {
    let code = crate::host::compile(&format!("#define {part}\n#define RS \"{rs}\"\n{SOURCE}"), entry, s!("cs_5_1"))?;
    // SAFETY: the root signature is inside the shader; plain creations.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code)?;
        let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
            ..Default::default()
        };
        let pso = device.CreateComputePipelineState(&desc)?;
        Ok((root, pso))
    }
}

fn pass() -> Option<&'static Pass> {
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    PASS.get_or_init(|| {
        let host = host()?;
        let made = || -> windows::core::Result<Pass> {
            let exposure = pipeline(&host.device, "EXPOSURE", RS_EXPOSURE, s!("exposure"))?;
            let lux = pipeline(&host.device, "LUX", RS_LUX, s!("lux"))?;
            let (ring, data) = host.shared_buffer(RECORD * RING * 4).ok_or_else(windows::core::Error::empty)?;
            // SAFETY: the mapping holds the whole buffer.
            unsafe { std::ptr::write_bytes(data, 0, (RECORD * RING * 4) as usize) };
            Ok(Pass { exposure, lux, ring, data: data as usize })
        };
        made().inspect_err(|e| tracing::warn!(error = %e, "no light probe")).ok()
    })
    .as_ref()
}

/// Records asked for so far (the exposure program runs once a frame), and
/// when each was.
static ASKED: AtomicU64 = AtomicU64::new(0);
static WHEN: [AtomicU64; RING as usize] = [const { AtomicU64::new(0) }; RING as usize];

fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

/// Writes the record asked for `LAG` records ago (the card has run it).
fn write_old(pass: &Pass, now: u64) {
    let Some(old) = now.checked_sub(LAG) else {
        return;
    };
    let slot = old % RING;
    // SAFETY: the ring's mapping; the card wrote this record frames ago.
    let words: [u32; RECORD as usize] = unsafe { ((pass.data + (slot * RECORD * 4) as usize) as *const [u32; RECORD as usize]).read_volatile() };
    if u64::from(words[63]) != slot + 1 {
        return;
    }
    let f = |i: usize| f32::from_bits(words[i]);
    let ms = WHEN[slot as usize].load(Relaxed);
    let samples = words[16];
    let mean = |i: usize| if samples == 0 { 0.0 } else { f64::from(words[i]) / 256.0 / f64::from(samples) };
    if let Some(file) = file() {
        let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(
            file,
            "{old},{ms},{:.5},{:.5},{:.5},{:.5},{:.5},{:.4},{:.4},{:.4},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5},{samples},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5}",
            f(0), f(1), f(2), f(3), f(4), f(5), f(6), f(7), f(8), f(9), f(10), f(11), f(12), f(13), f(14), f(15),
            mean(17), mean(18), mean(19), f(20), f(21), f(22)
        );
        if old % 64 == 0 {
            let _ = file.flush();
        }
    }
    // Mark it read.
    // SAFETY: as above.
    unsafe { ((pass.data + (slot * RECORD * 4 + 252) as usize) as *mut u32).write_volatile(0) };
}

/// After the exposure program's draw, in `list` with its heap set: its two
/// inputs. Replaces the list's compute root signature and pipeline.
pub(crate) fn after_exposure(list: &HostList, average: D3D12_GPU_DESCRIPTOR_HANDLE, before: D3D12_GPU_DESCRIPTOR_HANDLE) -> bool {
    let Some(pass) = pass() else {
        return false;
    };
    let n = ASKED.fetch_add(1, Relaxed);
    WHEN[(n % RING) as usize].store(crate::host::now_ms(), Relaxed);
    write_old(pass, n);
    let stats = crate::ambient::probe_buffers();
    let words = [(n % RING) as u32, crate::ambient::CELLS_AT, u32::from(stats.is_some()), 0];
    list.keep(&pass.ring);
    let l = &list.list;
    // SAFETY: an open list with the views' heap set; buffers kept.
    unsafe {
        uav_barrier(l);
        l.SetComputeRootSignature(&pass.exposure.0);
        l.SetPipelineState(&pass.exposure.1);
        l.SetComputeRoot32BitConstants(0, 4, words.as_ptr().cast(), 0);
        l.SetComputeRootDescriptorTable(1, average);
        l.SetComputeRootDescriptorTable(2, before);
        l.SetComputeRootUnorderedAccessView(3, pass.ring.GetGPUVirtualAddress());
        let (a, b) = match &stats {
            Some((stats, cells)) => {
                list.keep(stats);
                list.keep(cells);
                (stats.GetGPUVirtualAddress(), cells.GetGPUVirtualAddress())
            }
            None => (pass.ring.GetGPUVirtualAddress(), pass.ring.GetGPUVirtualAddress()),
        };
        l.SetComputeRootUnorderedAccessView(4, a);
        l.SetComputeRootUnorderedAccessView(5, b);
        l.Dispatch(1, 1, 1);
        uav_barrier(l);
    }
    true
}

/// After the lux program's dispatch: its input and output.
pub(crate) fn after_lux(list: &HostList, input: D3D12_GPU_DESCRIPTOR_HANDLE, output: D3D12_GPU_DESCRIPTOR_HANDLE) -> bool {
    let Some(pass) = pass() else {
        return false;
    };
    let n = ASKED.load(Relaxed).saturating_sub(1);
    let words = [(n % RING) as u32, 0, 0, 0];
    list.keep(&pass.ring);
    let l = &list.list;
    // SAFETY: as in `after_exposure`.
    unsafe {
        uav_barrier(l);
        l.SetComputeRootSignature(&pass.lux.0);
        l.SetPipelineState(&pass.lux.1);
        l.SetComputeRoot32BitConstants(0, 4, words.as_ptr().cast(), 0);
        l.SetComputeRootDescriptorTable(1, input);
        l.SetComputeRootDescriptorTable(2, output);
        l.SetComputeRootUnorderedAccessView(3, pass.ring.GetGPUVirtualAddress());
        l.Dispatch(1, 1, 1);
        uav_barrier(l);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_by_name() {
        assert_eq!(role(b"..PS_CalculateExposure_396dfccc_Wrapped.."), Some(Role::Exposure));
        assert_eq!(role(b"avgLuminanceCompute:ComputeLuxCompressionInfoSmooth_5b8554dd"), lux_on().then_some(Role::Lux));
        assert_eq!(role(b"PS_SetExposurePrevious"), None);
    }

    /// The passes compile with the system's compiler.
    #[test]
    fn the_passes_build() {
        let Some(host) = host() else {
            return;
        };
        pipeline(&host.device, "EXPOSURE", RS_EXPOSURE, s!("exposure")).expect("exposure pass");
        pipeline(&host.device, "LUX", RS_LUX, s!("lux")).expect("lux pass");
    }
}
