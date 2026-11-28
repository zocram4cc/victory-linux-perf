use victory_shader::emit::{
    MEMORY_FAULTS,
    ray_probe::{NAME as RAY_PROBE_HASH, WORDS as RAY_PROBE_DWORDS, enabled as ray_probe_enabled},
};
use windows::Win32::Graphics::Direct3D12::*;

use crate::host::{HostList, host, transition};
use crate::pipeline::host::HostPipeline;

pub(crate) fn after_dispatch(list: &HostList, pipeline: &HostPipeline) {
    if !ray_probe_enabled() || pipeline.stages[0].shader.hash != RAY_PROBE_HASH {
        return;
    }
    let frame = crate::queue::presented() + 1;
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
    if LAST
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |last| (last == u64::MAX || frame.saturating_sub(last) >= 32).then_some(frame),
        )
        .is_err()
    {
        return;
    }
    if !pipeline.stages[0]
        .translated()
        .host
        .root_uavs
        .contains(&victory_shader::emit::SLOT_FAULTS)
    {
        tracing::warn!(
            shader = RAY_PROBE_HASH,
            frame,
            "ray intersection fault UAV unavailable"
        );
        return;
    }
    let guard = super::MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    let faults = guard.as_ref().map(|memory| memory.faults.0.clone());
    drop(guard);
    let Some(readback) = faults.and_then(|faults| copy_counters(list, &faults)) else {
        tracing::warn!(
            shader = RAY_PROBE_HASH,
            frame,
            "ray intersection readback unavailable"
        );
        return;
    };
    list.after_gpu(move || {
        let Some(words) = read_counters(&readback) else {
            tracing::warn!(shader = RAY_PROBE_HASH, frame, "ray intersection readback unavailable");
            return;
        };
        let [visits, candidates, node_lo, node_hi, distance, extent, bary_u, bary_v] = words;
        tracing::info!(
            target: "victory_gpu::trace",
            shader = RAY_PROBE_HASH, frame,
            active_node_visits = visits, finite_triangle_candidates = candidates,
            sample_available = candidates != 0,
            first_node_lo = format_args!("{node_lo:#010x}"), first_node_hi = format_args!("{node_hi:#010x}"),
            first_distance_bits = format_args!("{distance:#010x}"), first_extent_bits = format_args!("{extent:#010x}"),
            first_bary_u_numerator_bits = format_args!("{bary_u:#010x}"),
            first_bary_v_numerator_bits = format_args!("{bary_v:#010x}"),
            "cumulative ray intersection counters after GPU fence"
        );
    });
}

fn copy_counters(list: &HostList, faults: &ID3D12Resource) -> Option<ID3D12Resource> {
    let bytes = u64::from(RAY_PROBE_DWORDS) * 4;
    let readback = host()?.read_back_buffer(bytes)?;
    let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    let copy = D3D12_RESOURCE_STATE_COPY_SOURCE;
    unsafe {
        list.list.ResourceBarrier(&[transition(faults, uav, copy)]);
        list.list
            .CopyBufferRegion(&readback, 0, faults, u64::from(MEMORY_FAULTS) * 4, bytes);
        list.list
            .ResourceBarrier(&[transition(faults, copy, D3D12_RESOURCE_STATE_COMMON)]);
    }
    list.keep(faults);
    list.keep(&readback);
    Some(readback)
}

fn read_counters(readback: &ID3D12Resource) -> Option<[u32; RAY_PROBE_DWORDS as usize]> {
    let mut data = std::ptr::null_mut();
    unsafe {
        let range = D3D12_RANGE {
            Begin: 0,
            End: RAY_PROBE_DWORDS as usize * 4,
        };
        readback.Map(0, Some(&range), Some(&mut data)).ok()?;
        let words = std::ptr::NonNull::new(data)
            .map(|p| p.cast::<[u32; RAY_PROBE_DWORDS as usize]>().read());
        readback.Unmap(0, Some(&D3D12_RANGE::default()));
        words
    }
}
