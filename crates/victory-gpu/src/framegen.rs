//! AMD's frame generation on the host, in place of a console's (`VICTORY_
//! FRAMEGEN=1`; needs `VICTORY_FIDELITYFX_DIR`, the folder with the SDK's
//! `signedbin`): the window's swap chain is AMD's frame generation one
//! ([`swap_chain`]), a frame generation context is fed by the title's own
//! upscale (`prepare`: its depth, motion vectors, jitter and camera, at the
//! render size the upscaler was given), and each present is interpolated from
//! the image before it and the one it shows.
//!
//! Ported from the fork `MrSkelee/VIctory-MrSkele-AMD` (`crates/victory-gpu/
//! src/ffx.rs`, GPL-3.0 like this fork), whose `VICTORY_FRAMEGEN=1` does the
//! same over its two-plane composition. Here the interface is already baked
//! into the image by `crate::compose`, so no UI resource is registered and the
//! generated frames carry it as the real ones do.
//!
//! What it is worth: the title renders 4-5 frames a second and the card shows
//! two per real one (what the frame-rate line calls "presented"), the
//! interpolation being work of its own on the same card. It does not make the
//! title faster: a 4 fps run shows ~8 fps.

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::OnceLock;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue;
use windows::Win32::Graphics::Dxgi::{
    DXGI_SWAP_CHAIN_DESC1, IDXGIFactory2, IDXGISwapChain3, IDXGISwapChain4,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{Interface, PCSTR, PCWSTR};

/// The API's types and constants (`ffx_api.h`, `ffx_framegeneration.h`, SDK
/// 2.4). The console's own names are the same: the title's DLLs are the same
/// API since FSR 3.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Header {
    kind: u64,
    next: *mut Header,
}

/// `FfxApiResource`: the resource, its description, its state.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Resource {
    pub resource: *mut std::ffi::c_void,
    pub description: [u32; 8],
    pub state: u32,
}
const _: () = assert!(std::mem::size_of::<Resource>() == 0x30);

impl Resource {
    /// A resource that is not there (a slot the title left empty).
    pub const NONE: Resource = Resource {
        resource: std::ptr::null_mut(),
        description: [0; 8],
        state: 0,
    };
}

const OK: u32 = 0;
const CREATE_FRAMEGEN: u64 = 0x0002_0001;
const CONFIGURE_FRAMEGEN: u64 = 0x0002_0002;
const DISPATCH_FRAMEGEN_PREPARE_V2: u64 = 0x0002_000c;
const CREATE_SWAP_CHAIN_FOR_HWND: u64 = 0x0003_0006;
const DISPATCH_SWAP_CHAIN_WAIT_FOR_PRESENTS: u64 = 0x0003_0007;
const CREATE_SWAP_CHAIN_VERSION: u64 = 0x0003_000b;
const BACKEND_DX12: u64 = 2;
/// `FFX_FRAMEGENERATION_SWAPCHAIN_DX12_VERSION` of SDK 2.4 (4.0.0).
const SWAP_CHAIN_VERSION: u32 = 4 << 22;

/// `ffxCreateContextDescFrameGenerationSwapChainVersionDX12`.
#[repr(C)]
struct Version {
    header: Header,
    version: u32,
}

/// `ffxCreateContextDescFrameGenerationSwapChainForHwndDX12`.
#[repr(C)]
struct CreateSwapChainForHwnd {
    header: Header,
    swap_chain: *mut *mut std::ffi::c_void,
    hwnd: *mut std::ffi::c_void,
    desc: *const std::ffi::c_void,
    fullscreen: *const std::ffi::c_void,
    factory: *mut std::ffi::c_void,
    queue: *mut std::ffi::c_void,
}

/// `ffxCreateBackendDX12Desc`.
#[repr(C)]
struct BackendDx12 {
    header: Header,
    device: *mut std::ffi::c_void,
}

/// `ffxCreateContextDescFrameGeneration`.
#[repr(C)]
struct CreateFramegen {
    header: Header,
    flags: u32,
    display: [u32; 2],
    max_render: [u32; 2],
    format: u32,
}

type GenerateCallback = unsafe extern "C" fn(*mut Header, *mut std::ffi::c_void) -> u32;

/// `ffxConfigureDescFrameGeneration`.
#[repr(C)]
struct ConfigureFramegen {
    header: Header,
    swap_chain: *mut std::ffi::c_void,
    present_callback: *const std::ffi::c_void,
    present_context: *mut std::ffi::c_void,
    generate_callback: Option<GenerateCallback>,
    generate_context: *mut std::ffi::c_void,
    enabled: bool,
    async_workloads: bool,
    hudless: Resource,
    flags: u32,
    only_generated: bool,
    rect: [i32; 4],
    frame: u64,
}
const _: () = assert!(std::mem::size_of::<ConfigureFramegen>() == 0x90);

/// `ffxDispatchDescFrameGenerationPrepareV2`.
#[repr(C)]
struct PrepareV2 {
    header: Header,
    frame: u64,
    flags: u32,
    command_list: *mut std::ffi::c_void,
    render_size: [u32; 2],
    jitter: [f32; 2],
    motion_vector_scale: [f32; 2],
    frame_time_delta: f32,
    reset: bool,
    camera: [f32; 4],
    depth: Resource,
    motion_vectors: Resource,
    camera_vectors: [f32; 12],
}
const _: () = assert!(std::mem::size_of::<PrepareV2>() == 0xe8);

const NO_RESOURCE: Resource = Resource {
    resource: std::ptr::null_mut(),
    description: [0; 8],
    state: 0,
};

type Create = unsafe extern "C" fn(
    *mut *mut std::ffi::c_void,
    *mut Header,
    *const std::ffi::c_void,
) -> u32;
type Call = unsafe extern "C" fn(*mut *mut std::ffi::c_void, *const Header) -> u32;

struct Api {
    create: Create,
    configure: Call,
    dispatch: Call,
    destroy: Call,
}

/// What the frame generation context is made with, learnt from the upscale
/// context and the swap chain.
struct Framegen {
    upscale_flags: u32,
    max_render: [u32; 2],
    display: [u32; 2],
    /// An `FfxApiSurfaceFormat`.
    format: u32,
}

static FRAMEGEN: std::sync::Mutex<Framegen> = std::sync::Mutex::new(Framegen {
    upscale_flags: 0,
    max_render: [0; 2],
    display: [0; 2],
    format: 0,
});
static SWAP_CHAIN_CONTEXT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
static SWAP_CHAIN: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
static CONTEXT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
/// Set on the first failure: frame generation stays off for the run.
static FAILED: AtomicBool = AtomicBool::new(false);
/// The frame identifier of the next present.
static FRAME: AtomicU64 = AtomicU64::new(0);
/// The last frame prepared, plus one.
static PREPARED: AtomicU64 = AtomicU64::new(0);

/// Whether `VICTORY_FRAMEGEN=1` asks for frame generation, with the PC
/// FidelityFX build (`VICTORY_FIDELITYFX_DIR`) to feed it.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let asked = std::env::var_os("VICTORY_FRAMEGEN").is_some_and(|v| v == "1");
        let dir = std::env::var_os("VICTORY_FIDELITYFX_DIR");
        let on = asked && dir.is_some();
        if asked && !on {
            tracing::warn!("frame generation off: VICTORY_FRAMEGEN=1 needs VICTORY_FIDELITYFX_DIR (the SDK's signedbin)");
        } else if on {
            tracing::info!("frame generation asked (VICTORY_FRAMEGEN=1)");
        }
        on
    })
}

/// AMD's loader, with the upscaler and the frame generation build beside it
/// loaded by full path first (so the loader finds them without a search
/// path change). `None` (logged once) without them.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("VICTORY_FIDELITYFX_DIR")?);
        let load = |name: &str| -> Option<windows::Win32::Foundation::HMODULE> {
            let path = std::path::absolute(dir.join(name)).ok()?;
            let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
            wide.push(0);
            // SAFETY: a NUL-terminated full path; the SDK's own folder and
            // System32 only, never the current folder.
            unsafe {
                LoadLibraryExW(
                    PCWSTR(wide.as_ptr()),
                    None,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            }
            .ok()
        };
        let _ = load("amd_fidelityfx_upscaler_dx12.dll");
        let _ = load("amd_fidelityfx_framegeneration_dx12.dll");
        let loader = load("amd_fidelityfx_loader_dx12.dll")?;
        let get = |name: &[u8]| {
            // SAFETY: a loaded module and a NUL-terminated name.
            unsafe { GetProcAddress(loader, PCSTR(name.as_ptr())) }
        };
        // SAFETY: the SDK's exports, of these signatures (`ffx_api.h`).
        unsafe {
            Some(Api {
                create: std::mem::transmute::<_, Create>(get(b"ffxCreateContext\0")?),
                configure: std::mem::transmute::<_, Call>(get(b"ffxConfigure\0")?),
                dispatch: std::mem::transmute::<_, Call>(get(b"ffxDispatch\0")?),
                destroy: std::mem::transmute::<_, Call>(get(b"ffxDestroyContext\0")?),
            })
        }
    })
    .as_ref()
}

/// The upscale context's flags and largest render size, for the frame
/// generation context (the motion vector and depth bits are at the same
/// positions in both; HDR is the back buffer's, not the upscale input's).
pub fn upscale_context(flags: u32, max_render: [u32; 2]) {
    if !on() {
        return;
    }
    let mut fg = FRAMEGEN.lock().unwrap_or_else(|e| e.into_inner());
    fg.upscale_flags = flags;
    fg.max_render = max_render;
}

/// The FfxApiSurfaceFormat of a swap chain format.
fn surface_format(dxgi: i32) -> u32 {
    match dxgi {
        28 => 10,
        87 => 14,
        24 => 17,
        10 => 4,
        _ => 0,
    }
}

fn header(kind: u64) -> Header {
    Header {
        kind,
        next: std::ptr::null_mut(),
    }
}

/// The frame generation swap chain for `hwnd`, made as `desc` says on
/// `queue`; `None` (one line logged, frame generation off for the run) if
/// the SDK cannot make it, and then the window's own is made as before.
pub fn swap_chain(
    hwnd: HWND,
    desc: &DXGI_SWAP_CHAIN_DESC1,
    factory: &IDXGIFactory2,
    queue: &ID3D12CommandQueue,
) -> Option<IDXGISwapChain3> {
    let api = api()?;
    if FAILED.load(Ordering::Relaxed) {
        return None;
    }
    let mut out = std::ptr::null_mut();
    let mut version = Version {
        header: header(CREATE_SWAP_CHAIN_VERSION),
        version: SWAP_CHAIN_VERSION,
    };
    let mut create = CreateSwapChainForHwnd {
        header: Header {
            kind: CREATE_SWAP_CHAIN_FOR_HWND,
            next: (&raw mut version).cast(),
        },
        swap_chain: &raw mut out,
        hwnd: hwnd.0,
        desc: std::ptr::from_ref(desc).cast(),
        fullscreen: std::ptr::null(),
        factory: factory.as_raw(),
        queue: queue.as_raw(),
    };
    // SAFETY: the SDK's call with a chain that outlives it.
    let r = unsafe {
        (api.create)(
            SWAP_CHAIN_CONTEXT.as_ptr(),
            (&raw mut create).cast(),
            std::ptr::null(),
        )
    };
    if r != OK || out.is_null() {
        tracing::warn!(
            r,
            "frame generation off: the SDK made no frame generation swap chain; presenting as before"
        );
        FAILED.store(true, Ordering::Relaxed);
        return None;
    }
    // SAFETY: the swap chain the SDK made, with a reference for us.
    let swap_chain = unsafe { IDXGISwapChain4::from_raw(out) };
    SWAP_CHAIN.store(out, Ordering::Release);
    tracing::info!("presenting through AMD's frame generation swap chain");
    swap_chain.cast().ok()
}

/// The swap chain's size and format, at creation and after each resize; a
/// frame generation context made for another is let go (made again at the
/// next prepare).
pub fn display(width: u32, height: u32, format: i32) {
    let mut fg = FRAMEGEN.lock().unwrap_or_else(|e| e.into_inner());
    let format = surface_format(format);
    if (fg.display, fg.format) == ([width, height], format) {
        return;
    }
    (fg.display, fg.format) = ([width, height], format);
    let Some(api) = api() else { return };
    if CONTEXT.load(Ordering::Acquire).is_null() {
        return;
    }
    configure(api, &fg, FRAME.load(Ordering::Acquire), false);
    let wait = header(DISPATCH_SWAP_CHAIN_WAIT_FOR_PRESENTS);
    // SAFETY: the SDK's calls on the contexts it made.
    unsafe {
        (api.dispatch)(
            SWAP_CHAIN_CONTEXT.as_ptr(),
            (&raw const wait).cast(),
        );
        (api.destroy)(CONTEXT.as_ptr(), std::ptr::null());
    }
    CONTEXT.store(std::ptr::null_mut(), Ordering::Release);
}

/// Records the frame generation prepare for the next present on the
/// upscale's host list, with the upscale's inputs (in the state it left
/// them in); the first upscale after a present only.
pub fn prepare(
    list: *mut std::ffi::c_void,
    depth: Resource,
    motion_vectors: Resource,
    jitter: [f32; 2],
    motion_vector_scale: [f32; 2],
    render_size: [u32; 2],
    frame_time_delta: f32,
    reset: bool,
    camera: [f32; 4],
) {
    if FAILED.load(Ordering::Relaxed)
        || SWAP_CHAIN_CONTEXT.load(Ordering::Acquire).is_null()
        || depth.resource.is_null()
        || motion_vectors.resource.is_null()
    {
        return;
    }
    let frame = FRAME.load(Ordering::Acquire);
    if PREPARED.swap(frame + 1, Ordering::AcqRel) == frame + 1 {
        return;
    }
    let Some(api) = api() else { return };
    let fg = FRAMEGEN.lock().unwrap_or_else(|e| e.into_inner());
    if CONTEXT.load(Ordering::Acquire).is_null() && !create(api, &fg) {
        PREPARED.store(0, Ordering::Release);
        return;
    }
    let prepare = PrepareV2 {
        header: header(DISPATCH_FRAMEGEN_PREPARE_V2),
        frame,
        flags: 0,
        command_list: list,
        render_size,
        jitter,
        motion_vector_scale,
        frame_time_delta,
        reset,
        camera,
        depth,
        motion_vectors,
        // Camera position, up, right and forward: unknown here, left zero.
        camera_vectors: [0.0; 12],
    };
    // SAFETY: the SDK's call on an open host list with live textures.
    let r = unsafe { (api.dispatch)(CONTEXT.as_ptr(), (&raw const prepare).cast()) };
    if r != OK {
        PREPARED.store(0, Ordering::Release);
    }
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        tracing::info!(
            frame,
            render = ?render_size,
            result = r,
            "the first frame generation prepare was recorded"
        );
    });
}

/// Makes the frame generation context; `false` if it cannot be made yet (no
/// swap chain size or upscale context known) or at all (logged once, frame
/// generation off for the run: the swap chain presents as is).
fn create(api: &Api, fg: &Framegen) -> bool {
    let Some(device) = crate::external::host_device() else {
        return false;
    };
    if fg.display.contains(&0) || fg.max_render.contains(&0) {
        return false;
    }
    let mut dx12 = BackendDx12 {
        header: header(BACKEND_DX12),
        device: device.as_raw(),
    };
    let mut create = CreateFramegen {
        header: Header {
            kind: CREATE_FRAMEGEN,
            next: (&raw mut dx12).cast(),
        },
        flags: fg.upscale_flags & 0b1_1110,
        display: fg.display,
        max_render: fg.max_render,
        format: fg.format,
    };
    // SAFETY: the SDK's call with a device that outlives it.
    let r = unsafe {
        (api.create)(
            CONTEXT.as_ptr(),
            (&raw mut create).cast(),
            std::ptr::null(),
        )
    };
    if r != OK {
        tracing::warn!(
            r,
            display = ?fg.display,
            render = ?fg.max_render,
            "frame generation off: no frame generation context; presenting as before"
        );
        FAILED.store(true, Ordering::Relaxed);
        CONTEXT.store(std::ptr::null_mut(), Ordering::Release);
        return false;
    }
    tracing::info!(
        display = ?fg.display,
        render = ?fg.max_render,
        flags = create.flags,
        format = fg.format,
        "frame generation context on the host GPU"
    );
    true
}

fn configure(api: &Api, fg: &Framegen, frame: u64, enabled: bool) {
    let c = ConfigureFramegen {
        header: header(CONFIGURE_FRAMEGEN),
        swap_chain: SWAP_CHAIN.load(Ordering::Acquire),
        present_callback: std::ptr::null(),
        present_context: std::ptr::null_mut(),
        generate_callback: Some(generate),
        generate_context: CONTEXT.as_ptr().cast(),
        enabled,
        async_workloads: false,
        hudless: NO_RESOURCE,
        flags: 0,
        only_generated: false,
        rect: [0, 0, fg.display[0] as i32, fg.display[1] as i32],
        frame,
    };
    // SAFETY: the SDK's call on the context it made.
    let r = unsafe { (api.configure)(CONTEXT.as_ptr(), (&raw const c).cast()) };
    if r != OK {
        static REPORTED: std::sync::Once = std::sync::Once::new();
        REPORTED.call_once(|| {
            tracing::warn!(r, "frame generation configuration refused")
        });
    }
}

/// The SDK's call back to generate a frame: its dispatch on our context.
unsafe extern "C" fn generate(params: *mut Header, context: *mut std::ffi::c_void) -> u32 {
    let Some(api) = api() else { return 1 };
    // SAFETY: the SDK's description, for the context it was configured with.
    unsafe { (api.dispatch)(context.cast(), params) }
}

/// Before a present through the frame generation swap chain: frame
/// generation is on for this frame if an upscale prepared it.
pub fn before_present() {
    let Some(api) = api() else { return };
    if CONTEXT.load(Ordering::Acquire).is_null() {
        return;
    }
    let fg = FRAMEGEN.lock().unwrap_or_else(|e| e.into_inner());
    let frame = FRAME.load(Ordering::Acquire);
    configure(api, &fg, frame, PREPARED.load(Ordering::Acquire) == frame + 1);
}

/// After a present through the frame generation swap chain, given the swap
/// chain's count of presents (real and generated): once a second, the real
/// frame rate and the one shown.
pub fn after_present(presented: Option<u32>) {
    let real = FRAME.fetch_add(1, Ordering::AcqRel) + 1;
    static SINCE: std::sync::Mutex<Option<(std::time::Instant, u64, u32)>> =
        std::sync::Mutex::new(None);
    let mut since = SINCE.lock().unwrap_or_else(|e| e.into_inner());
    let presented = presented.unwrap_or(0);
    let Some((at, real_then, presented_then)) = *since else {
        *since = Some((std::time::Instant::now(), real, presented));
        return;
    };
    let elapsed = at.elapsed();
    if elapsed < std::time::Duration::from_secs(1) {
        return;
    }
    let seconds = elapsed.as_secs_f64();
    tracing::info!(
        real = (real - real_then) as f64 / seconds,
        presented = (presented.saturating_sub(presented_then)) as f64 / seconds,
        "frames rendered and frames shown (VICTORY_FRAMEGEN=1: the extra ones are generated)"
    );
    *since = Some((std::time::Instant::now(), real, presented));
}
