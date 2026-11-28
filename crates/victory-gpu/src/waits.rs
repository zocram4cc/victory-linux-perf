#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

pub(crate) const COMPUTE: u8 = 1;
pub(crate) const DRAWS: u8 = 2;
pub(crate) const SHADERS: u8 = COMPUTE | DRAWS;
const UNNAMED: u8 = 4;

const GLOBAL: u32 = 0;
const TEXTURE: u32 = 1;
const BUFFER: u32 = 2;
const MOST: u32 = 4096;

#[repr(C)]
struct Group {
    kind: u32,
    count: u32,
    barriers: *const u8,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Scope {
    sync_before: u32,
    sync_after: u32,
    access_before: u32,
    access_after: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Asked(u8);

impl Asked {
    pub(crate) const ALL: Asked = Asked(SHADERS | UNNAMED);

    pub(crate) unsafe fn of(count: u32, groups: *const u8) -> Asked {
        if groups.is_null() || count > MOST {
            return Asked::ALL;
        }
        let mut asked = 0;
        for g in 0..count as usize {
            let group = unsafe { groups.cast::<Group>().add(g).read_unaligned() };
            let stride = match group.kind {
                GLOBAL => 16,
                TEXTURE => 64,
                BUFFER => 40,
                _ => return Asked::ALL,
            };
            if group.count > MOST || (group.count > 0 && group.barriers.is_null()) {
                return Asked::ALL;
            }
            for b in 0..group.count as usize {
                let scope = unsafe { group.barriers.add(b * stride).cast::<Scope>().read_unaligned() };
                #[cfg(all(windows, feature = "diagnostics"))]
                seen(group.kind, scope);
                asked |= waited(group.kind, scope);
            }
        }
        Asked(asked)
    }

    pub(crate) fn word(self) -> u64 {
        u64::from(self.0)
    }

    pub(crate) fn from_word(word: u64) -> Asked {
        Asked(word as u8 & (SHADERS | UNNAMED))
    }

    pub(crate) fn stages(self) -> u8 {
        self.0 & SHADERS
    }

    pub(crate) fn named(self) -> bool {
        self.0 & UNNAMED == 0
    }
}

fn waited(kind: u32, scope: Scope) -> u8 {
    const NONE: u32 = 0;
    const NO_ACCESS: u32 = 0x8000_0000;
    const UNORDERED: u32 = 0x10;
    const DRAWING: u32 = 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x40;
    const COMPUTING: u32 = 0x80 | 0x800;
    const SHADING: u32 = 0x1000 | 0x2000;
    const READS: u32 = 0x1 | 0x2 | 0x4 | 0x40 | 0x80 | 0x200 | 0x800 | 0x2000 | 0x4000 | 0x1_0000;
    if scope.sync_before == NONE || scope.access_before == NO_ACCESS {
        return 0;
    }
    let reads = |access: u32| access != 0 && access & !READS == 0;
    if reads(scope.access_before) && (reads(scope.access_after) || scope.access_after == NO_ACCESS) {
        return 0;
    }
    if kind == TEXTURE {
        let unordered = |access: u32| access == 0 || (access != NO_ACCESS && access & UNORDERED != 0);
        if !unordered(scope.access_before) && !unordered(scope.access_after) {
            return 0;
        }
    }
    if scope.sync_before & !(DRAWING | COMPUTING | SHADING) != 0 {
        return SHADERS | UNNAMED;
    }
    let mut stages = 0;
    if scope.sync_before & (COMPUTING | SHADING) != 0 {
        stages |= COMPUTE;
    }
    if scope.sync_before & (DRAWING | SHADING) != 0 {
        stages |= DRAWS;
    }
    stages
}

#[cfg(windows)]
pub(crate) fn precise() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        if std::env::var_os("VICTORY_PRECISE_WAITS").is_some_and(|v| v == "0") {
            return false;
        }
        let Some(host) = crate::host::host() else { return false };
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
        let known = unsafe {
            host.device.CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS12,
                (&raw mut options).cast(),
                size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS12>() as u32,
            )
        };
        known.is_ok() && options.EnhancedBarriersSupported.as_bool()
    })
}

#[cfg(windows)]
pub(crate) fn after(stages: u8) -> D3D12_GLOBAL_BARRIER {
    let mut before = 0;
    if stages & COMPUTE != 0 {
        before |= D3D12_BARRIER_SYNC_COMPUTE_SHADING.0;
    }
    if stages & DRAWS != 0 {
        before |= D3D12_BARRIER_SYNC_VERTEX_SHADING.0 | D3D12_BARRIER_SYNC_PIXEL_SHADING.0;
    }
    D3D12_GLOBAL_BARRIER {
        SyncBefore: D3D12_BARRIER_SYNC(before),
        SyncAfter: D3D12_BARRIER_SYNC_ALL_SHADING,
        AccessBefore: D3D12_BARRIER_ACCESS_UNORDERED_ACCESS,
        AccessAfter: D3D12_BARRIER_ACCESS_UNORDERED_ACCESS,
    }
}

#[cfg(windows)]
pub(crate) const CLAMPED: D3D12_GLOBAL_BARRIER = D3D12_GLOBAL_BARRIER {
    SyncBefore: D3D12_BARRIER_SYNC_COMPUTE_SHADING,
    SyncAfter: D3D12_BARRIER_SYNC_EXECUTE_INDIRECT,
    AccessBefore: D3D12_BARRIER_ACCESS_UNORDERED_ACCESS,
    AccessAfter: D3D12_BARRIER_ACCESS_INDIRECT_ARGUMENT,
};

#[cfg(windows)]
pub(crate) unsafe fn record(list: &ID3D12GraphicsCommandList7, barrier: &D3D12_GLOBAL_BARRIER) {
    unsafe {
        list.Barrier(&[D3D12_BARRIER_GROUP {
            Type: D3D12_BARRIER_TYPE_GLOBAL,
            NumBarriers: 1,
            Anonymous: D3D12_BARRIER_GROUP_0 { pGlobalBarriers: barrier },
        }]);
    }
}

#[cfg(all(windows, feature = "diagnostics"))]
#[derive(Clone, Copy)]
pub(crate) enum Count {
    Groups,
    GroupsNothing,
    GroupsCompute,
    GroupsDraws,
    GroupsUnnamed,
    Syncs,
    ClampsAfterWait,
    ClampsAsRecorded,
}

#[cfg(all(windows, feature = "diagnostics"))]
static COUNTS: [std::sync::atomic::AtomicU64; 8] = [const { std::sync::atomic::AtomicU64::new(0) }; 8];

#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn count(what: Count) {
    COUNTS[what as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn count_groups(asked: Asked, precise: bool) {
    count(Count::Groups);
    count(match (precise && asked.named(), asked.stages()) {
        (false, _) => Count::GroupsUnnamed,
        (true, 0) => Count::GroupsNothing,
        (true, COMPUTE) => Count::GroupsCompute,
        (true, _) => Count::GroupsDraws,
    });
}

#[cfg(all(windows, feature = "diagnostics"))]
#[cfg(all(windows, feature = "diagnostics"))]
static SEEN: std::sync::Mutex<Option<std::collections::HashMap<(u32, Scope), u64>>> = std::sync::Mutex::new(None);

#[cfg(all(windows, feature = "diagnostics"))]
fn seen(kind: u32, scope: Scope) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_default();
    if seen.len() < 256 || seen.contains_key(&(kind, scope)) {
        *seen.entry((kind, scope)).or_default() += 1;
    }
}

#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn line(frames: f64) {
    let c = COUNTS.each_ref().map(|c| c.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / frames);
    if c.iter().all(|&v| v == 0.0) {
        return;
    }
    tracing::info!(
        "waits per frame (precise {}): {:.0} barrier group calls of the title ({:.0} wait for nothing, {:.0} for compute, {:.0} for draws too, {:.0} for every shader), {:.0} pipeline syncs; clamp passes: {:.0} after a wait of their own, {:.0} as recorded",
        precise(),
        c[0],
        c[1],
        c[2],
        c[3],
        c[4],
        c[5],
        c[6],
        c[7]
    );
    let mut seen: Vec<_> = SEEN.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default().into_iter().collect();
    seen.sort_by_key(|&(shape, count)| (std::cmp::Reverse(count), shape.0, shape.1.sync_before, shape.1.access_before));
    let top: Vec<String> = seen
        .iter()
        .take(24)
        .map(|((kind, s), count)| {
            format!(
                "{:.1}x {}{:#x}/{:#x}>{:#x}/{:#x}={}",
                *count as f64 / frames,
                ["global ", "texture ", "buffer "].get(*kind as usize).copied().unwrap_or("? "),
                s.sync_before,
                s.access_before,
                s.sync_after,
                s.access_after,
                waited(*kind, *s)
            )
        })
        .collect();
    tracing::info!("the title's barriers per frame, sync/access before>after=stages waited for: {}", top.join("; "));
}
