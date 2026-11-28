//! Host-memory admission for speculative texture work and reusable upload storage.
//!
//! From codec-d/cool_fox (641835a4, fabb872a, c64fc9b4, 08e4574d, e94d277a; the
//! values of the last). Only with `VICTORY_RESIDENCY=1` ([`on`]): the memory
//! of the system is then asked once a second; below 3 GiB of free RAM or 4 GiB
//! of commit (out again above 5 / 6) texture prefetch is limited to 64 MiB a
//! frame and idle scratch and staging blocks are let go; below 1.5 GiB of
//! commit to 16 MiB; the texture pass stops below 512 MiB of commit, new
//! staging blocks for textures picked by index below 1 GiB. With 32 GiB of RAM
//! or less (or `VICTORY_RESIDENCY_LIMITED=1`) the budgets of a smaller PC.
//! Off, nothing here changes anything, except that for 500 ms after the card
//! refused memory (E_OUTOFMEMORY) the texture pass stops ([`upload_refused`]).
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

const MIB: u64 = 1 << 20;

/// `VICTORY_RESIDENCY=1`: the memory budgets of this module (off by default).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RESIDENCY").is_some_and(|v| v == "1"))
}
const GIB: u64 = 1 << 30;
static PRESSURED: AtomicBool = AtomicBool::new(false);
static TRIM_NEEDED: AtomicBool = AtomicBool::new(false);
static TIGHT_RAM: AtomicBool = AtomicBool::new(false);
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
        OUT_OF_MEMORY.set(true);
    }
}

thread_local! {
    static STAGING_DEFERRED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static OUT_OF_MEMORY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread was refused memory ([`allocation_error`]) since the
/// last call.
pub(crate) fn take_out_of_memory() -> bool {
    OUT_OF_MEMORY.replace(false)
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

/// Whether the PC's memory is small enough that it runs out before the
/// card's (`VICTORY_SMALL_HOST=1`/`0`; unset: 24 GiB of RAM or less): spare
/// upload memory is then kept small (`crate::host::send`). From Neptune
/// (`residency::small_host_ram`), measured there on 16 GB PCs.
pub(crate) fn small_host_ram() -> bool {
    static SMALL: OnceLock<bool> = OnceLock::new();
    *SMALL.get_or_init(|| {
        let small = match std::env::var("VICTORY_SMALL_HOST").ok().as_deref().map(str::trim) {
            Some("1") => true,
            Some("0") => false,
            _ => memory_status().is_some_and(|s| s.ullTotalPhys <= 24 * GIB),
        };
        tracing::info!(small, "small host memory (VICTORY_SMALL_HOST): spare upload memory kept small");
        small
    })
}

/// Whether this host has `<= 32 GiB` physical RAM (or `VICTORY_32GB_TUNE=1`),
/// independent of `VICTORY_RESIDENCY=1`. Used to enable zero-thrash pool
/// reclamation and shadow/slab caps without throttling texture streaming.
pub(crate) fn is_32gb_host() -> bool {
    static IS_32GB: OnceLock<bool> = OnceLock::new();
    *IS_32GB.get_or_init(|| match std::env::var("VICTORY_32GB_TUNE").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => memory_status().is_some_and(|s| s.ullTotalPhys <= 32 * GIB),
    })
}

pub(crate) fn limited_host_ram() -> bool {
    static LIMITED: OnceLock<bool> = OnceLock::new();
    *LIMITED.get_or_init(|| {
        on() && (std::env::var_os("VICTORY_RESIDENCY_LIMITED").is_some_and(|v| v == "1")
            || memory_status().is_some_and(|s| s.ullTotalPhys <= 32 * GIB))
    })
}

fn next_trim(was: bool, physical: u64, commit: u64) -> bool {
    if was {
        physical < 7 * GIB || commit < 10 * GIB
    } else {
        physical < 5 * GIB || commit < 8 * GIB
    }
}

fn next_tight(was: bool, physical: u64, commit: u64) -> bool {
    if was {
        physical < 4 * GIB || commit < 6 * GIB
    } else {
        physical < 2560 * MIB || commit < 4 * GIB
    }
}

fn next_pressure(was: bool, physical: u64, commit: u64) -> bool {
    if was {
        physical < 5 * GIB || commit < 6 * GIB
    } else {
        physical <= 3 * GIB || commit <= 4 * GIB
    }
}

/// `VICTORY_COMMIT_WATCH=0`: PCs of more than 32 GiB watch nothing (as
/// before). On by default: there too the trim and tight watermarks follow the
/// commit left (RAM plus page file), not the free RAM. A 48 GB PC ended with
/// "memory allocation of 3076541 bytes failed" (28 Nov 2026): nothing of ours
/// was let go early there, since the watermarks ran on 32 GiB PCs only.
pub(crate) fn commit_watch() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COMMIT_WATCH").is_none_or(|v| v != "0"))
}

/// Allocations of ours the system refused, each retried
/// ([`crate::alloc_retry`]); and how many the sweep has seen.
pub(crate) static ALLOCATIONS_REFUSED: AtomicU64 = AtomicU64::new(0);
static REFUSALS_SEEN: AtomicU64 = AtomicU64::new(0);

/// Called by the once-per-second submission sweep. A failed sample keeps the last good state.
pub(crate) fn refresh() {
    let tune_32 = is_32gb_host();
    let watch = !tune_32 && commit_watch();
    if !on() && !tune_32 && !watch {
        return;
    }
    let Some(status) = memory_status() else { return };
    let (physical, commit) = (status.ullAvailPhys, status.ullAvailPageFile);
    let refused = ALLOCATIONS_REFUSED.load(Ordering::Relaxed);
    let newly_refused = refused != REFUSALS_SEEN.swap(refused, Ordering::Relaxed);
    if newly_refused {
        tracing::warn!(
            refused,
            available_commit_mib = commit >> 20,
            commit_limit_mib = status.ullTotalPageFile >> 20,
            physical_free_mib = physical >> 20,
            "the PC refused memory to us (its RAM and page file are full): waited for it and let go of what we keep spare"
        );
    }
    if watch {
        // The free RAM is left out: with plenty of it the commit can still run out.
        let trim = next_trim(trim_needed(), u64::MAX, commit) || newly_refused;
        let tight = next_tight(tight_ram(), u64::MAX, commit) || newly_refused;
        if trim != trim_needed() || tight != tight_ram() {
            tracing::info!(
                trim,
                tight,
                available_commit_mib = commit >> 20,
                commit_limit_mib = status.ullTotalPageFile >> 20,
                "commit watch (VICTORY_COMMIT_WATCH): the commit left crossed a watermark; spare memory of ours kept short while it is low"
            );
        }
        TRIM_NEEDED.store(trim, Ordering::Relaxed);
        TIGHT_RAM.store(tight, Ordering::Relaxed);
    }
    if tune_32 && newly_refused {
        TRIM_NEEDED.store(true, Ordering::Relaxed);
        TIGHT_RAM.store(true, Ordering::Relaxed);
    }
    if tune_32 && !newly_refused {
        let was_trim = trim_needed();
        let trim = next_trim(was_trim, physical, commit);
        if trim != was_trim {
            TRIM_NEEDED.store(trim, Ordering::Relaxed);
            if trim {
                tracing::info!(
                    physical_free_mib = physical >> 20,
                    available_commit_mib = commit >> 20,
                    buffer_mirror_span_mib = crate::shadow::live_span_bytes() >> 20,
                    texture_staging_mib = crate::texture::live_staging_bytes() >> 20,
                    "32 GiB host trim watermark reached: reclaiming idle pools and cold shadows"
                );
            }
        }
        let was_tight = tight_ram();
        let tight = next_tight(was_tight, physical, commit);
        if tight != was_tight {
            TIGHT_RAM.store(tight, Ordering::Relaxed);
            if tight {
                tracing::warn!(
                    physical_free_mib = physical >> 20,
                    available_commit_mib = commit >> 20,
                    buffer_mirror_span_mib = crate::shadow::live_span_bytes() >> 20,
                    texture_staging_mib = crate::texture::live_staging_bytes() >> 20,
                    "32 GiB tight-RAM watermark reached: dropping spare pools and trimming shadow cache"
                );
            } else {
                tracing::info!(
                    physical_free_mib = physical >> 20,
                    available_commit_mib = commit >> 20,
                    "32 GiB tight-RAM watermark cleared"
                );
            }
        }
    }
    if !on() {
        return;
    }
    HEADROOM.store(physical.min(commit), Ordering::Relaxed);
    let was = pressured();
    let pressure = next_pressure(was, physical, commit);
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
                "host memory pressure: limiting texture prefetch and idle upload storage"
            );
        } else {
            tracing::info!(physical_free_mib = physical >> 20, available_commit_mib = commit >> 20, "host memory pressure cleared");
        }
    }
}

pub(crate) fn trim_needed() -> bool {
    TRIM_NEEDED.load(Ordering::Relaxed)
}

pub(crate) fn tight_ram() -> bool {
    TIGHT_RAM.load(Ordering::Relaxed)
}

pub(crate) fn headroom() -> Option<u64> {
    let bytes = HEADROOM.load(Ordering::Relaxed);
    (bytes != u64::MAX).then_some(bytes)
}

pub(crate) fn pressured() -> bool {
    PRESSURED.load(Ordering::Relaxed)
}

/// True when OS commit headroom is critically low (< 1.5 GiB) or an allocation
/// just failed with `E_OUTOFMEMORY`, requiring emergency throttling.
pub(crate) fn critical_commit() -> bool {
    if !on() {
        return false;
    }
    let now = crate::host::coarse_ms();
    now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) || sample_commit(now) < 1536 * MIB
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn trim_and_tight_thresholds_use_hysteresis() {
        assert!(next_trim(false, 4 * GIB, 12 * GIB));
        assert!(next_trim(false, 12 * GIB, 7 * GIB));
        assert!(!next_trim(false, 5 * GIB, 8 * GIB));
        assert!(next_trim(true, 6 * GIB, 12 * GIB));
        assert!(!next_trim(true, 7 * GIB, 10 * GIB));

        assert!(next_tight(false, 2 * GIB, 10 * GIB));
        assert!(next_tight(false, 8 * GIB, 3 * GIB));
        assert!(!next_tight(false, 3 * GIB, 5 * GIB));
        assert!(next_tight(true, 3 * GIB, 8 * GIB));
        assert!(!next_tight(true, 4 * GIB, 6 * GIB));
    }
}
