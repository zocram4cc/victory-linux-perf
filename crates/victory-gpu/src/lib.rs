//! D3D12.X on host D3D12.
//!
//! D3D12.X records commands from code inlined into the title, which writes
//! packets into the command list while its put pointer is below a limit and
//! calls the list's method otherwise. Our lists keep the limits at zero, so
//! every command arrives here as a call, to be run on host D3D12. The
//! title's GPU addresses are CPU addresses (console memory is unified).
//!
//! [`device`] is the device object titles create, [`dxgi`] its adapter side,
//! [`resource`] heaps and resources, [`fence`] fences, [`queue`] command
//! queues, [`list`] command allocators and lists, [`descriptor`]
//! descriptor heaps, [`root`] root signatures, [`query`] query heaps,
//! [`signature`] command signatures, [`pipeline`] pipeline states,
//! [`frame`] frame pacing, [`object`] what all console graphics objects
//! share, [`console`] which console model the title sees. Planned:
//! `present` (swap chain).

#[cfg(windows)]
mod ambient;
#[cfg(windows)]
mod lightprobe;
#[cfg(windows)]
mod texlate;
#[cfg(windows)]
mod texstore;
#[cfg(all(windows, feature = "diagnostics"))]
mod capture;
#[cfg(windows)]
mod compose;
pub mod console;
#[cfg(all(windows, feature = "diagnostics"))]
mod copystat;
#[cfg(all(windows, feature = "diagnostics"))]
mod faultbook;
#[cfg(windows)]
mod cores;
#[cfg(windows)]
mod debug_hud;
pub mod descriptor;
pub mod device;
#[cfg(windows)]
pub mod ffx;
#[cfg(windows)]
mod draw;
pub mod dxgi;
pub mod fence;
pub mod frame;
pub mod frametime;
#[cfg(all(windows, feature = "diagnostics"))]
mod book;
#[cfg(windows)]
mod heap_worker;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod indirect;
#[cfg(windows)]
mod inplace;
#[cfg(windows)]
mod port;
/// Whether renderer port #1 is on here (`VICTORY_PORT_CONSTANTS`; off under
/// Wine unless `VICTORY_PORT_CONSTANTS_WINE=1`), for xmem's half of it.
#[cfg(windows)]
pub use port::constants_switch_here as port_constants_switch;
#[cfg(windows)]
mod port_cbv;
#[cfg(windows)]
pub mod port_descriptors;
#[cfg(windows)]
mod port_journal;
#[cfg(windows)]
mod land;
pub mod list;
pub mod object;
pub mod pipeline;
#[cfg(windows)]
mod picked;
#[cfg(windows)]
mod pool;
#[cfg(all(windows, feature = "diagnostics"))]
mod popwatch;
#[cfg(windows)]
mod primitive;
pub mod query;
pub mod queue;
pub mod remap;
#[cfg(all(windows, feature = "diagnostics"))]
pub mod replay;
#[cfg(windows)]
mod residency;
pub mod resource;
pub mod root;
#[cfg(windows)]
mod safecopy;
#[cfg(windows)]
mod sampler;
#[cfg(windows)]
pub mod shader_cache;
#[cfg(windows)]
mod memory;
#[cfg(windows)]
mod shadow;
#[cfg(windows)]
mod tiles;
pub mod signature;
mod switch;
#[cfg(all(windows, feature = "diagnostics"))]
mod ab;
#[cfg(windows)]
mod tdr;
#[cfg(windows)]
mod texture;
pub mod timeline;
#[cfg(windows)]
pub mod titlewrite;
#[cfg(windows)]
mod upload;
#[cfg(windows)]
mod watch;
#[cfg(windows)]
mod texback;
#[cfg(windows)]
mod wine;
mod wcskip; // VICTORY_SKIP_WC_BACK=1 (off), VICTORY_SKIP_WC_MIN_KB=1024

#[cfg(windows)]
pub use host::on_failure;
#[cfg(windows)]
pub use host::{early as host_early, prepare as prepare_host};

/// Whether `switch` is on now, if it is one of those `VICTORY_AB` turns on and
/// off while the title runs (`ab`).
#[cfg(all(windows, feature = "diagnostics"))]
pub fn switched(switch: &str) -> Option<bool> {
    ab::pick(switch)
}

/// What the frame is at (`copystat::phase`), for the sampling profiler.
#[cfg(all(windows, feature = "diagnostics"))]
pub fn frame_phase() -> u32 {
    copystat::phase()
}


/// Frames the title has begun (its frame waits that ended), for logs.
pub fn frames_begun() -> u64 {
    frame::current()
}

/// The title reserved `[at, at + size)` asking for protection `protect`,
/// the console's bits included: for `VICTORY_SKIP_WC_BACK` (`wcskip`).
#[cfg(windows)]
pub fn title_memory_reserved(at: usize, size: usize, protect: u32) {
    wcskip::reserved(at, size, protect);
}
#[cfg(windows)]
pub use inplace::{title_memory_freeing, title_memory_section, title_memory_section_gone, title_memory_view};
#[cfg(windows)]
pub use watch::{
    kernel_writing, title_memory_allocated, title_memory_freed, title_memory_reading, title_memory_remapped,
    title_memory_remapping, title_memory_remapping_in, title_memory_remapping_with, title_memory_splitting,
    title_memory_unmapping, title_memory_written,
    writing as kernel_will_write,
};
#[cfg(windows)]
pub use watch::title_view_space;
#[cfg(windows)]
pub use inplace::title_memory_home;
