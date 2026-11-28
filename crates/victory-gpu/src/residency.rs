//! Host-memory pressure for reusable upload storage and speculative texture work.
//!
//! From codec-d/cool_fox (641835a4, fabb872a, c64fc9b4, 08e4574d, e94d277a; the
//! values of the last). The submission sweep samples free RAM and commit once
//! a second. At or below 3 GiB of free RAM or 4 GiB of commit it trims idle
//! scratch and staging blocks, until both reach 5 GiB and 6 GiB again.
//!
//! With `VICTORY_RESIDENCY=1` ([`on`]), pressure also limits texture prefetch
//! to 64 MiB per frame. Below 1.5 GiB of commit the limit is 16 MiB. The
//! texture pass stops below 512 MiB of commit and new staging blocks for
//! textures picked by index stop below 1 GiB. Hosts with up to 32 GiB of RAM
//! (or `VICTORY_RESIDENCY_LIMITED=1`) use the smaller budgets.
//! An E_OUTOFMEMORY always pauses the texture pass for 500 ms ([`upload_refused`]).
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

const MIB: u64 = 1 << 20;

/// `VICTORY_RESIDENCY=1` enables the optional texture limits.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RESIDENCY").is_some_and(|v| v == "1"))
}
const GIB: u64 = 1 << 30;
static PRESSURED: AtomicBool = AtomicBool::new(false);
static HEADROOM: AtomicU64 = AtomicU64::new(u64::MAX);

fn memory_status() -> Option<MEMORYSTATUSEX> {
    // SAFETY: the API initializes the fields of a correctly sized structure.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status)
}

// The texture sweep may run for seconds inside a single submitted frame.
static NEXT_COMMIT_SAMPLE: AtomicU64 = AtomicU64::new(0);
static AVAILABLE_COMMIT: AtomicU64 = AtomicU64::new(u64::MAX);
static RETRY_UPLOAD_AT: AtomicU64 = AtomicU64::new(0);

pub(crate) fn allocation_error(error: &windows::core::Error) {
    if error.code() == windows::Win32::Foundation::E_OUTOFMEMORY {
        RETRY_UPLOAD_AT.fetch_max(crate::host::coarse_ms().saturating_add(500), Ordering::Relaxed);
    }
}

thread_local! {
    static STAGING_DEFERRED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn note_staging_deferred() {
    STAGING_DEFERRED.set(true);
}

pub(crate) fn take_staging_deferred() -> bool {
    STAGING_DEFERRED.replace(false)
}

pub(crate) fn upload_refused() -> bool {
    crate::host::coarse_ms() < RETRY_UPLOAD_AT.load(Ordering::Relaxed)
}

fn sample_commit(now: u64) -> u64 {
    let next = NEXT_COMMIT_SAMPLE.load(Ordering::Relaxed);
    if now >= next
        && NEXT_COMMIT_SAMPLE
            .compare_exchange(next, now.saturating_add(32), Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        && let Some(status) = memory_status()
    {
        AVAILABLE_COMMIT.store(status.ullAvailPageFile, Ordering::Relaxed);
    }
    AVAILABLE_COMMIT.load(Ordering::Relaxed)
}

fn has_upload_reserve(commit: u64, bake: bool) -> bool {
    // Keep a protective floor against Rust/game OOM aborts without starving
    // bindless world textures or one-shot ped decoration bakes when steady-state
    // commit headroom sits in the 1-4 GiB range on 32 GiB hosts.
    commit >= if bake { 256 * MIB } else { 512 * MIB }
}

fn has_growth_reserve(commit: u64, bake: bool) -> bool {
    // Growing a fresh 64 MiB upload block for speculative bindless prefetch
    // requires more headroom than reusing slices already committed inside the
    // staging pool.
    commit >= if bake { 256 * MIB } else { 1024 * MIB }
}

/// Leave commit for game allocations and submissions already in flight while
/// still letting bindless textures and one-shot material bakes upload.
pub(crate) fn admit_texture_pass(bake: bool) -> bool {
    let now = crate::host::coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) {
        return false;
    }
    if !on() {
        return true;
    }
    has_upload_reserve(sample_commit(now), bake)
}

/// Whether the staging allocator may commit a brand-new OS upload buffer
/// (reusing existing free ranges in already-committed pool blocks stays open
/// whenever `admit_texture_pass` is true).
pub(crate) fn allow_staging_growth(bake: bool) -> bool {
    if !on() {
        return true;
    }
    let now = crate::host::coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) {
        return false;
    }
    has_growth_reserve(sample_commit(now), bake)
}

pub(crate) fn limited_host_ram() -> bool {
    static LIMITED: OnceLock<bool> = OnceLock::new();
    *LIMITED.get_or_init(|| {
        on() && (std::env::var_os("VICTORY_RESIDENCY_LIMITED").is_some_and(|v| v == "1")
            || memory_status().is_some_and(|s| s.ullTotalPhys <= 32 * GIB))
    })
}

/// `VICTORY_PRESSURE_PHYSICAL=1` (with `VICTORY_RESIDENCY=1`): pressure is free
/// physical memory alone, on at 1.5 GiB, off again at 2.5 GiB. With a
/// system-managed page file the commit headroom sits near the commit lines
/// whatever is let go (the page file grows on demand), and most of this
/// process's commit is the card's own memory, not resident (#333): the
/// commit lines turned pressure on once in every run and never off.
fn physical_only() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PRESSURE_PHYSICAL").is_some_and(|v| v == "1"))
}

fn next_pressure_physical(was: bool, physical: u64) -> bool {
    if was { physical < 2560 * MIB } else { physical <= 1536 * MIB }
}

fn next_pressure_for_host(was: bool, physical: u64, commit: u64, total_phys: u64) -> bool {
    if physical_only() {
        return next_pressure_physical(was, physical);
    }
    // On <= 20 GiB RAM hosts (e.g. 16 GB DDR4), Windows normally keeps
    // `ullAvailPhys` around 200-1100 MiB during Open World gameplay due to
    // standby file cache even when 4.5-8.5 GiB of commit headroom is free.
    // Latching pressure at <= 384 MiB phys regardless of commit caused 3x
    // false pool wipes and 750-1500 ms hitch clusters in Wave 6 while
    // 4.2-8.5 GiB of commit was free.
    // Enter pressure when physical is critically low (<= 128 MiB), commit is
    // low (<= 1792 MiB), or both physical <= 256 MiB and commit < 3072 MiB.
    // All hard commit safety guards (`admit_texture_pass`, `critical_commit`,
    // `emergency_pressure`) remain intact.
    if total_phys > 0 && total_phys <= 20 * GIB {
        if was {
            physical < 256 * MIB || commit < 2560 * MIB || (physical < 384 * MIB && commit < 3584 * MIB)
        } else {
            physical <= 128 * MIB || commit <= 1792 * MIB || (physical <= 256 * MIB && commit < 3072 * MIB)
        }
    } else if was {
        physical < 5 * GIB || commit < 6 * GIB
    } else {
        physical <= 3 * GIB || commit <= 4 * GIB
    }
}

#[cfg(test)]
fn next_pressure(was: bool, physical: u64, commit: u64) -> bool {
    next_pressure_for_host(was, physical, commit, 32 * GIB)
}

/// Returns pressure for the idle upload sweep. A failed sample keeps the last state.
pub(crate) fn refresh() -> bool {
    let was = PRESSURED.load(Ordering::Relaxed);
    let Some(status) = memory_status() else { return was };
    let (physical, commit, total_phys) = (status.ullAvailPhys, status.ullAvailPageFile, status.ullTotalPhys);
    let limit_prefetch = on();
    if limit_prefetch {
        HEADROOM.store(physical.min(commit), Ordering::Relaxed);
    }
    let pressure = next_pressure_for_host(was, physical, commit, total_phys);
    if pressure != was {
        PRESSURED.store(pressure, Ordering::Relaxed);
        if pressure {
            tracing::warn!(
                pid = std::process::id(),
                frame = crate::frame::current(),
                physical_free_mib = physical >> 20,
                available_commit_mib = commit >> 20,
                buffer_mirror_span_mib = crate::shadow::live_span_bytes() >> 20,
                texture_staging_mib = crate::texture::live_staging_bytes() >> 20,
                spare_scratch_mib = crate::host::ALIVE[4].load(Ordering::Relaxed) >> 20,
                game_filled_gpu_texture_mib = crate::resource::KINDS[1].load(Ordering::Relaxed) >> 20,
                limit_prefetch,
                "host memory pressure: reclaiming idle upload storage"
            );
        } else {
            tracing::info!(physical_free_mib = physical >> 20, available_commit_mib = commit >> 20, "host memory pressure cleared");
        }
    }
    pressure
}

pub(crate) fn headroom() -> Option<u64> {
    let bytes = HEADROOM.load(Ordering::Relaxed);
    (bytes != u64::MAX).then_some(bytes)
}

/// Pressure as seen by speculative texture work.
pub(crate) fn pressured() -> bool {
    on() && PRESSURED.load(Ordering::Relaxed)
}

/// True when OS commit headroom is critically low (< 1.5 GiB) or an allocation
/// just failed with `E_OUTOFMEMORY`, requiring emergency throttling.
pub(crate) fn critical_commit() -> bool {
    if !on() {
        return false;
    }
    let now = crate::host::coarse_ms();
    let floor = if physical_only() { 256 * MIB } else { 1536 * MIB };
    now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) || sample_commit(now) < floor
}

/// A soft per-frame admission limit. A single texture can cross it. The
/// queue retains unserved slots so they can be retried on later frames.
pub(crate) fn prefetch_limit(bytes: u64, pressure: bool, critical: bool, limited: bool, staging: u64) -> u64 {
    if critical {
        bytes.min(16 << 20)
    } else if pressure || (limited && staging >= GIB) {
        bytes.min(64 << 20)
    } else {
        bytes
    }
}

// ---------------------------------------------------------------------------
// Dynamic DXGI Auto-VRAM Policy (8 / 12 / 16 / 24+ GB adapters)
// ---------------------------------------------------------------------------

/// Hardware VRAM class detected from DXGI (`DedicatedVideoMemory` and live
/// `DXGI_QUERY_VIDEO_MEMORY_INFO.Budget`), or forced via `VICTORY_VRAM_MODE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VramTier {
    Gb8,
    Gb12,
    Gb16,
    Gb24,
}

impl VramTier {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            VramTier::Gb8 => "8GB",
            VramTier::Gb12 => "12GB",
            VramTier::Gb16 => "16GB",
            VramTier::Gb24 => "24GB+",
        }
    }
}

/// Auto-scaled VRAM & shared-memory budgets derived from the host GPU's real
/// DXGI video memory budget, host RAM size, and active console profile.
/// Every field can still be overridden individually via its `VICTORY_*` env var.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VramPolicy {
    pub(crate) tier: VramTier,
    pub(crate) dedicated_bytes: u64,
    pub(crate) local_budget_bytes: u64,
    pub(crate) shared_budget_bytes: u64,
    /// Texture-cache keep watermark (% of local VRAM budget).
    pub(crate) texture_keep_percent: u64,
    /// Texture-queue admission ceiling (% of local VRAM budget).
    pub(crate) texture_room_percent: u64,
    /// Render-target headroom reserved during world load (MiB).
    pub(crate) targets_room_mb: u64,
    /// Smallest title buffer promoted to local VRAM (KiB).
    pub(crate) card_buffers_min_kb: u64,
    /// Max aggregate title-buffer VRAM copies (MiB).
    pub(crate) card_buffers_max_mb: u64,
    /// Max aggregate loose-window VRAM copies (MiB; direct VRAM on Win10/11).
    pub(crate) card_windows_mb: u64,
    /// Max VRAM occupancy (%) for placing loose windows in VRAM.
    pub(crate) card_windows_percent: u64,
    /// Max Resizable BAR (`GPU_UPLOAD`) mapped window budget (MiB, Win11 WDDM 3.0+).
    pub(crate) windows_bar_mb: u64,
    /// Max VRAM occupancy (%) for BAR-mapped windows.
    pub(crate) windows_bar_percent: u64,
    /// Texture pool reclaim watermark (% of local VRAM budget).
    pub(crate) pool_reclaim_percent: u64,
    /// Empty 64 MiB texture blocks kept when `keep_freed` is enabled.
    pub(crate) pool_spare_kept: usize,
    /// Max `NON_LOCAL` texture staging ring cap (MiB; `u64::MAX` = uncapped).
    pub(crate) staging_nonlocal_cap_mb: u64,
    /// Max `NON_LOCAL` loose-window budget under RAM pressure (MiB).
    pub(crate) window_budget_mb: u64,
}

/// Parses an optional env setting where unset, empty, or `"auto"` means
/// "use the dynamic DXGI auto-scaled value", and a valid unsigned integer
/// acts as an explicit manual override.
pub(crate) fn parse_env_auto_u64(setting: Option<&str>) -> Option<u64> {
    let trimmed = setting?.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

/// Reads a `VICTORY_*` environment variable as a manual `u64` override,
/// returning `None` when unset or set to `"auto"`.
pub(crate) fn env_override_u64(name: &str) -> Option<u64> {
    let val = std::env::var(name).ok();
    parse_env_auto_u64(val.as_deref())
}

fn parse_vram_mode(setting: Option<&str>) -> Option<VramTier> {
    let s = setting?.trim().to_ascii_lowercase();
    match s.as_str() {
        "" | "auto" => None,
        "8" | "8g" | "8gb" => Some(VramTier::Gb8),
        "12" | "12g" | "12gb" => Some(VramTier::Gb12),
        "16" | "16g" | "16gb" => Some(VramTier::Gb16),
        "24" | "24g" | "24gb" | "32" | "32gb" => Some(VramTier::Gb24),
        _ => None,
    }
}

/// Classifies the GPU into 8 / 12 / 16 / 24+ GB tiers from DXGI's
/// `DedicatedVideoMemory` and live local `Budget`.
///
/// Uses live `budget_bytes` when available (since WDDM reserves ~5–10% for the
/// OS/DWM, a 16 GB card reports ~14.5–15.5 GB budget, 12 GB reports ~10.8–11.6 GB,
/// 8 GB reports ~7.0–7.6 GB, 24 GB reports ~22.5–23.5 GB) so shared workloads
/// automatically step down safely instead of overcommitting.
pub(crate) fn classify_vram_tier(dedicated_bytes: u64, budget_bytes: u64) -> VramTier {
    let dedicated_mb = dedicated_bytes >> 20;
    let budget_mb = budget_bytes >> 20;
    if budget_mb > 0 {
        if budget_mb <= 8_704 {
            VramTier::Gb8
        } else if budget_mb <= 12_800 {
            VramTier::Gb12
        } else if budget_mb <= 18_432 {
            VramTier::Gb16
        } else {
            VramTier::Gb24
        }
    } else if dedicated_mb <= 9_728 {
        VramTier::Gb8
    } else if dedicated_mb <= 13_824 {
        VramTier::Gb12
    } else if dedicated_mb <= 19_456 {
        VramTier::Gb16
    } else {
        VramTier::Gb24
    }
}

/// Computes the dynamic VRAM policy for a given hardware/profile snapshot.
pub(crate) fn compute_vram_policy(
    mode_override: Option<VramTier>,
    dedicated_bytes: u64,
    local_budget_bytes: u64,
    shared_budget_bytes: u64,
    series_s: bool,
    total_ram_bytes: u64,
) -> VramPolicy {
    let tier = mode_override.unwrap_or_else(|| classify_vram_tier(dedicated_bytes, local_budget_bytes));
    let budget_mb = if local_budget_bytes > 0 {
        local_budget_bytes >> 20
    } else {
        match tier {
            VramTier::Gb8 => 7_400,
            VramTier::Gb12 => 11_400,
            VramTier::Gb16 => 15_400,
            VramTier::Gb24 => 23_200,
        }
    };
    let tight_host_ram = (total_ram_bytes > 0 && total_ram_bytes <= 20 * GIB)
        || (shared_budget_bytes > 0 && shared_budget_bytes <= (8 << 30) + (512 << 20));

    match tier {
        VramTier::Gb8 => VramPolicy {
            tier,
            dedicated_bytes,
            local_budget_bytes,
            shared_budget_bytes,
            texture_keep_percent: 68,
            // An 8 GB card's boot budget (~7.4 GB) is not its gameplay
            // budget. DWM and the title's first world frame commonly lower
            // the live budget to ~5.9 GB, so admitting textures at 78% of
            // the boot budget leaves the process 0.5-1.3 GB over budget.
            // Reserve a larger slice for invisible targets and stop the
            // bindless queue earlier; safe mode's 40% keep watermark then
            // drains idle copies without starving the visible working set.
            texture_room_percent: 65,
            targets_room_mb: 2048,
            card_buffers_min_kb: 512,
            card_buffers_max_mb: (budget_mb / 8).clamp(512, 768),
            card_windows_mb: 0,
            card_windows_percent: 72,
            windows_bar_mb: 0,
            windows_bar_percent: 72,
            pool_reclaim_percent: 76,
            pool_spare_kept: 2,
            staging_nonlocal_cap_mb: if tight_host_ram { 128 } else { 512 },
            window_budget_mb: if tight_host_ram { 96 } else { 256 },
        },
        VramTier::Gb12 => VramPolicy {
            tier,
            dedicated_bytes,
            local_budget_bytes,
            shared_budget_bytes,
            texture_keep_percent: if series_s { 84 } else { 80 },
            texture_room_percent: 91,
            targets_room_mb: 1280,
            card_buffers_min_kb: 128,
            card_buffers_max_mb: (budget_mb / 5).clamp(1536, 2560),
            card_windows_mb: if series_s { 1536 } else { 1024 },
            card_windows_percent: 84,
            windows_bar_mb: if series_s { 2048 } else { 1536 },
            windows_bar_percent: 85,
            pool_reclaim_percent: 88,
            pool_spare_kept: 10,
            staging_nonlocal_cap_mb: if tight_host_ram { 192 } else { 512 },
            window_budget_mb: if tight_host_ram { 128 } else { 256 },
        },
        VramTier::Gb16 => VramPolicy {
            tier,
            dedicated_bytes,
            local_budget_bytes,
            shared_budget_bytes,
            // Restore proven 90% / 95% texture_room_percent: lowering room_pct
            // to 84% caused `resource::room()` (which adds targets_room_kept +
            // card_room_kept on top of raw VRAM) to latch `false` at 2.3 GB
            // textures (432k `card has no room` refusals -> black world).
            texture_keep_percent: if tight_host_ram {
                if series_s { 86 } else { 82 }
            } else if series_s {
                91
            } else {
                88
            },
            texture_room_percent: if tight_host_ram { 94 } else { 95 },
            targets_room_mb: if tight_host_ram { 1024 } else { 1280 },
            card_buffers_min_kb: 64,
            card_buffers_max_mb: (budget_mb / 4).clamp(3072, 4096),
            card_windows_mb: if tight_host_ram {
                if series_s { 2816 } else { 2048 }
            } else if series_s {
                3584
            } else {
                2560
            },
            card_windows_percent: if tight_host_ram { 82 } else { 89 },
            windows_bar_mb: if series_s { 4096 } else { 3072 },
            windows_bar_percent: if tight_host_ram { 85 } else { 90 },
            pool_reclaim_percent: if tight_host_ram { 86 } else { 92 },
            pool_spare_kept: if tight_host_ram { 12 } else { 16 },
            staging_nonlocal_cap_mb: if tight_host_ram { 192 } else { u64::MAX },
            window_budget_mb: if tight_host_ram { 128 } else { 512 },
        },
        VramTier::Gb24 => VramPolicy {
            tier,
            dedicated_bytes,
            local_budget_bytes,
            shared_budget_bytes,
            texture_keep_percent: 92,
            texture_room_percent: 96,
            targets_room_mb: 1536,
            card_buffers_min_kb: 64,
            card_buffers_max_mb: (budget_mb / 3).clamp(4096, 8192),
            card_windows_mb: 5120,
            card_windows_percent: 91,
            windows_bar_mb: 5120,
            windows_bar_percent: 91,
            pool_reclaim_percent: 93,
            pool_spare_kept: 24,
            staging_nonlocal_cap_mb: if tight_host_ram { 256 } else { u64::MAX },
            window_budget_mb: if tight_host_ram { 256 } else { 1024 },
        },
    }
}

/// Active dynamic VRAM policy for this session, resolved once from DXGI
/// (`DedicatedVideoMemory` + `DXGI_QUERY_VIDEO_MEMORY_INFO.Budget`), host RAM,
/// and console profile (`--console series-s` / `series-x`).
pub(crate) fn vram_policy() -> VramPolicy {
    static POLICY: OnceLock<VramPolicy> = OnceLock::new();
    *POLICY.get_or_init(|| {
        let mode_raw = std::env::var("VICTORY_VRAM_MODE").ok();
        let mode_override = parse_vram_mode(mode_raw.as_deref());
        let adapter = crate::dxgi::host_adapter_vram();
        let (dedicated_bytes, local_budget_bytes, shared_budget_bytes, is_uma) = match adapter {
            Some(v) => (v.dedicated_bytes, v.local_budget_bytes, v.shared_budget_bytes, v.is_uma),
            None => (0, 0, 0, false),
        };
        let total_ram_bytes = memory_status().map_or(0, |s| s.ullTotalPhys);
        let series_s = crate::console::current().is_series_s();
        let mut policy = compute_vram_policy(
            mode_override,
            dedicated_bytes,
            local_budget_bytes,
            shared_budget_bytes,
            series_s,
            total_ram_bytes,
        );
        // On UMA / integrated GPUs (where local & shared are the same physical
        // RAM), copying loose windows into a separate DEFAULT heap adds copy
        // traffic without escaping PCIe; disable VRAM window staging there.
        if is_uma {
            policy.card_windows_mb = 0;
            policy.windows_bar_mb = 0;
        }
        tracing::info!(
            tier = policy.tier.label(),
            mode = mode_raw.as_deref().unwrap_or("auto"),
            console = crate::console::current().name(),
            uma = is_uma,
            dedicated_mb = policy.dedicated_bytes >> 20,
            vram_budget_mb = policy.local_budget_bytes >> 20,
            shared_budget_mb = policy.shared_budget_bytes >> 20,
            host_ram_mb = total_ram_bytes >> 20,
            keep_pct = policy.texture_keep_percent,
            room_pct = policy.texture_room_percent,
            targets_room_mb = policy.targets_room_mb,
            card_min_kb = policy.card_buffers_min_kb,
            card_max_mb = policy.card_buffers_max_mb,
            card_windows_mb = policy.card_windows_mb,
            card_windows_pct = policy.card_windows_percent,
            staging_cap_mb = if policy.staging_nonlocal_cap_mb == u64::MAX { 0 } else { policy.staging_nonlocal_cap_mb },
            "auto VRAM policy active"
        );
        policy
    })
}

/// Effective texture-cache keep watermark (% of local VRAM budget):
/// explicit `VICTORY_TEXTURE_KEEP_PERCENT=<40..95>` overrides, while unset or
/// `"auto"` uses [`vram_policy`].
fn base_texture_keep_percent() -> u64 {
    static PCT: OnceLock<u64> = OnceLock::new();
    *PCT.get_or_init(|| {
        env_override_u64("VICTORY_TEXTURE_KEEP_PERCENT")
            .map_or_else(|| vram_policy().texture_keep_percent, |p| p.clamp(40, 95))
    })
}

/// Pure texture-eviction helper matching `host::vram_keep`, with dynamic
/// commit-headroom protection so 16 GB RAM hosts stay OOM-free even when
/// external apps consume system commit.
pub(crate) fn vram_keep_for(budget: u64, used: u64, base_keep_pct: u64, avail_commit: Option<u64>) -> (u64, u64) {
    const KEPT_MS: u64 = 600_000;
    const IDLE_MS: u64 = 5_000;
    const SHORT_MS: u64 = 3_000;
    if budget == 0 {
        return (IDLE_MS, u64::MAX);
    }
    // Step down the texture keep watermark gently if OS commit headroom gets
    // low (every VRAM MiB carries a WDDM backing commit charge).
    let k = match avail_commit {
        Some(commit) if commit < 1536 * MIB => base_keep_pct.saturating_sub(20).max(45),
        Some(commit) if commit < 3 * GIB => base_keep_pct.saturating_sub(10).max(50),
        _ => base_keep_pct,
    };
    let share = |percent: u64| budget / 100 * percent;
    if used < share(k) {
        (KEPT_MS, u64::MAX)
    } else if used < share((k + 5).min(97)) {
        (IDLE_MS, used - share(k.saturating_sub(4)))
    } else {
        (SHORT_MS, used - share(k.saturating_sub(7)))
    }
}

/// Dynamic replacement for `host::vram_keep` called by `resource::let_go_idle`:
/// scales automatically by DXGI VRAM tier (`auto`) while still respecting manual
/// `VICTORY_TEXTURE_KEEP_PERCENT` overrides and live host commit headroom.
/// Setting `VICTORY_VRAM_MODE=legacy` delegates directly to `host::vram_keep`.
pub(crate) fn vram_keep(budget: u64, used: u64) -> (u64, u64) {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    if *LEGACY.get_or_init(|| {
        std::env::var("VICTORY_VRAM_MODE")
            .ok()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("legacy"))
    }) {
        return crate::host::vram_keep(budget, used);
    }
    let base = base_texture_keep_percent();
    let commit = on().then(|| sample_commit(crate::host::coarse_ms()));
    vram_keep_for(budget, used, base, commit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_pressure_turns_on_at_one_and_a_half_and_off_at_two_and_a_half_gib() {
        assert!(!next_pressure_physical(false, 2000 * MIB));
        assert!(next_pressure_physical(false, 1536 * MIB));
        assert!(next_pressure_physical(true, 2000 * MIB));
        assert!(!next_pressure_physical(true, 2560 * MIB));
    }

    #[test]
    fn bake_and_lookahead_leave_room_for_live_game_allocations() {
        for (commit, bake, lookahead, growth) in [
            (128 * MIB, false, false, false),
            (256 * MIB, true, false, false),
            (512 * MIB, true, true, false),
            (768 * MIB, true, true, false),
            (1024 * MIB, true, true, true),
        ] {
            assert_eq!(has_upload_reserve(commit, true), bake);
            assert_eq!(has_upload_reserve(commit, false), lookahead);
            assert_eq!(has_growth_reserve(commit, false), growth);
        }
    }

    #[test]
    fn either_constraint_enters_pressure_and_both_must_recover() {
        assert!(next_pressure(false, 12 * GIB, 4 * GIB));
        assert!(next_pressure(false, 3 * GIB, 16 * GIB));
        assert!(next_pressure(true, 4 * GIB, 7 * GIB));
        assert!(next_pressure(true, 6 * GIB, 5 * GIB));
        assert!(!next_pressure(true, 5 * GIB, 6 * GIB));
        assert!(!next_pressure(false, 4 * GIB, 5 * GIB));
    }

    #[test]
    fn prefetch_respects_existing_budgets_and_resumes_after_recovery() {
        assert_eq!(prefetch_limit(512 << 20, false, false, true, GIB), 64 << 20);
        assert_eq!(prefetch_limit(512 << 20, false, false, true, GIB - 1), 512 << 20);
        assert_eq!(prefetch_limit(512 << 20, false, false, false, 2 * GIB), 512 << 20);
        assert_eq!(prefetch_limit(0, true, true, false, 0), 0);
        assert_eq!(prefetch_limit(8 << 20, true, true, false, 0), 8 << 20);
        assert_eq!(prefetch_limit(512 << 20, true, false, false, 0), 64 << 20);
        assert_eq!(prefetch_limit(512 << 20, true, true, false, 0), 16 << 20);
        assert_eq!(prefetch_limit(192 << 20, false, false, false, 0), 192 << 20);
    }

    #[test]
    fn dxgi_auto_vram_tier_classification_and_policies() {
        // 8 GB GPU (~7.4 GB budget)
        assert_eq!(classify_vram_tier(8 * GIB, 7_400 * MIB), VramTier::Gb8);
        // 12 GB GPU (~11.2 GB budget)
        assert_eq!(classify_vram_tier(12 * GIB, 11_200 * MIB), VramTier::Gb12);
        // 16 GB GPU (RX 9060 XT: 16253 MB dedicated, 15450 MB budget)
        assert_eq!(classify_vram_tier(16_253 * MIB, 15_450 * MIB), VramTier::Gb16);
        // 24 GB GPU (~23.2 GB budget)
        assert_eq!(classify_vram_tier(24 * GIB, 23_200 * MIB), VramTier::Gb24);

        let p8 = compute_vram_policy(None, 8 * GIB, 7_400 * MIB, 7_473 * MIB, true, 16 * GIB);
        let p12 = compute_vram_policy(None, 12 * GIB, 11_200 * MIB, 7_473 * MIB, true, 16 * GIB);
        let p16_16ram = compute_vram_policy(None, 16_253 * MIB, 15_450 * MIB, 7_473 * MIB, true, 16 * GIB);
        let p16_32ram = compute_vram_policy(None, 16_253 * MIB, 15_450 * MIB, 16 * GIB, true, 32 * GIB);
        let p24 = compute_vram_policy(None, 24 * GIB, 23_200 * MIB, 16 * GIB, true, 32 * GIB);

        assert_eq!(p8.tier, VramTier::Gb8);
        assert_eq!(p8.card_windows_mb, 0);
        assert!(p8.texture_keep_percent < p12.texture_keep_percent);
        assert!(p12.texture_keep_percent <= p16_16ram.texture_keep_percent);
        assert!(p16_16ram.texture_keep_percent < p16_32ram.texture_keep_percent);
        assert!(p16_32ram.texture_keep_percent < p24.texture_keep_percent);
        assert_eq!(p8.texture_keep_percent, 68);
        assert_eq!(p8.texture_room_percent, 65);
        assert_eq!(p8.targets_room_mb, 2048);
        assert_eq!(p8.card_buffers_max_mb, 768);
        // 16 GB GPU on 16 GB RAM evicts idle textures at 86% while keeping 94% room_pct
        // so `resource::room()` never false-latches `!room` at 12.8/14.9 GB VRAM.
        assert_eq!(p16_16ram.texture_keep_percent, 86);
        assert_eq!(p16_16ram.texture_room_percent, 94);
        assert_eq!(p16_16ram.card_windows_mb, 2816);
        assert_eq!(p16_16ram.staging_nonlocal_cap_mb, 192);
        // 16 GB GPU on >=32 GB RAM unlocks ~90-94% of live DXGI VRAM (~13.2-14.2 GB)
        assert_eq!(p16_32ram.texture_keep_percent, 91);
        assert_eq!(p16_32ram.texture_room_percent, 95);
        assert!(p16_32ram.card_buffers_max_mb > 2560);
        assert!(p16_32ram.card_windows_mb >= 3584);
    }

    #[test]
    fn env_auto_parsing_and_commit_guard() {
        assert_eq!(parse_env_auto_u64(None), None);
        assert_eq!(parse_env_auto_u64(Some("")), None);
        assert_eq!(parse_env_auto_u64(Some("auto")), None);
        assert_eq!(parse_env_auto_u64(Some(" AUTO ")), None);
        assert_eq!(parse_env_auto_u64(Some("65")), Some(65));
        assert_eq!(parse_vram_mode(Some("auto")), None);
        assert_eq!(parse_vram_mode(Some("16gb")), Some(VramTier::Gb16));
        assert_eq!(parse_vram_mode(Some("8")), Some(VramTier::Gb8));

        let budget = 15_450 * MIB;
        // Healthy commit headroom keeps textures up to 83% (~12.8 GB)
        assert_eq!(vram_keep_for(budget, 12_000 * MIB, 83, Some(6 * GIB)).0, 600_000);
        // Low commit (<3 GiB) steps watermark down to 73% (~11.3 GB) to avoid OOM
        assert_eq!(vram_keep_for(budget, 12_000 * MIB, 83, Some(2 * GIB)).0, 5_000);
        // Critical commit (<1.5 GiB) steps watermark down to 63% (~9.7 GB)
        assert_eq!(vram_keep_for(budget, 12_000 * MIB, 83, Some(GIB)).0, 3_000);
        // 16 GB RAM hosts do not false-latch pool wipes on Windows standby dips when commit >= 3.5 GiB
        assert!(!next_pressure_for_host(false, 768 * MIB, 6 * GIB, 16 * GIB));
        assert!(!next_pressure_for_host(false, 362 * MIB, 5594 * MIB, 16 * GIB));
        assert!(!next_pressure_for_host(false, 192 * MIB, 4174 * MIB, 16 * GIB));
        assert!(next_pressure_for_host(false, 128 * MIB, 6 * GIB, 16 * GIB));
        assert!(next_pressure_for_host(false, 256 * MIB, 2500 * MIB, 16 * GIB));
        assert!(next_pressure_for_host(false, 2500 * MIB, 1792 * MIB, 16 * GIB));
        assert!(next_pressure_for_host(true, 200 * MIB, 4 * GIB, 16 * GIB));
        assert!(next_pressure_for_host(true, 300 * MIB, 3000 * MIB, 16 * GIB));
        assert!(!next_pressure_for_host(true, 300 * MIB, 4000 * MIB, 16 * GIB));
        assert!(!next_pressure_for_host(true, 384 * MIB, 2560 * MIB, 16 * GIB));
    }
}
