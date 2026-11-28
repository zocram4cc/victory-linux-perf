//! AMD FidelityFX frame generation for frames the title upscales
//! (diagnostics, `VICTORY_FIDELITYFX_FRAMEGEN=1` beside
//! `VICTORY_FIDELITYFX_DIR`). The title asks for no frame generation; the
//! inputs are its upscale's: at each host upscale (`external::record`
//! through the HLE FidelityFX module) the depth and motion vectors and the
//! frame's constants are kept, and those of the last submitted list feed
//! the next present. The presenter's swap chain is then AMD's frame
//! generation proxy (`amd_fidelityfx_framegeneration_dx12.dll`), which
//! inserts the generated frames.
//!
//! Mode 1 uses the analytical provider. Experimental mode 4 supplies the
//! title's world-space camera from its velocity pass when it matches the
//! current upscale.
//!
//! shortcut: an RGBA8 swap chain only; no HUD-less colour, so the UI is
//! interpolated with the scene.

use std::ffi::{CStr, c_void};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, D3D12_RESOURCE_BARRIER};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{DXGI_SWAP_CHAIN_DESC1, IDXGIFactory2, IDXGISwapChain3, IDXGISwapChain4};
use windows::core::Interface;

use crate::host::HostList;
use crate::resource::HostTexture;

pub(crate) mod camera;

const DLL: &str = "amd_fidelityfx_framegeneration_dx12.dll";
const OK: u32 = 0;

// FidelityFX SDK v2.4.0: ffx_api.h, ffx_framegeneration.h,
// ffx_api_dx12.h, and dx12/ffx_api_framegeneration_dx12.h.
const BACKEND_DX12: u64 = 2;
const GET_VERSIONS: u64 = 4;
const OVERRIDE_VERSION: u64 = 5;
const PROVIDER_VERSION: u64 = 6;
const CREATE: u64 = 0x20001;
const CONFIGURE: u64 = 0x20002;
const PREPARE: u64 = 0x2000c;
const VERSION: u64 = 0x2000e;
const SWAPCHAIN_CREATE: u64 = 0x30006;
const SWAPCHAIN_WAIT: u64 = 0x30007;
const SWAPCHAIN_VERSION: u64 = 0x3000b;
const RGBA8_UNORM: u32 = 10;
/// Display-resolution motion vectors, motion-vector jitter cancellation,
/// inverted depth, and infinite depth. The upscale HDR flag is separate.
const MOTION_DEPTH_FLAGS: u32 = 0b11110;
/// The FidelityFX resource state of compute reads, and the host's.
const FFX_COMPUTE_READ: u32 = 1 << 2;
const HOST_NON_PIXEL_SHADER_RESOURCE: u32 = 0x40;

#[repr(C)]
struct Header {
    kind: u64,
    next: *mut Header,
}

fn header(kind: u64) -> Header {
    Header { kind, next: null_mut() }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Resource {
    resource: *mut c_void,
    description: [u32; 8],
    state: u32,
}

#[repr(C)]
struct BackendDx12 {
    header: Header,
    device: *mut c_void,
}

#[repr(C)]
struct Version {
    header: Header,
    version: u32,
}

#[repr(C)]
struct Versions {
    header: Header,
    effect: u64,
    device: *mut c_void,
    count: *mut u64,
    ids: *mut u64,
    names: *mut *const std::ffi::c_char,
}

#[repr(C)]
struct ProviderOverride {
    header: Header,
    id: u64,
}

#[repr(C)]
struct SwapchainDesc {
    header: Header,
    output: *mut *mut c_void,
    hwnd: HWND,
    desc: *const DXGI_SWAP_CHAIN_DESC1,
    fullscreen_desc: *const c_void,
    factory: *mut c_void,
    queue: *mut c_void,
}

#[repr(C)]
struct Create {
    header: Header,
    flags: u32,
    display: [u32; 2],
    max_render: [u32; 2],
    format: u32,
}

type Callback = unsafe extern "C" fn(*mut Generate, *mut c_void) -> u32;

#[repr(C)]
struct Configure {
    header: Header,
    swapchain: *mut c_void,
    present_callback: *const c_void,
    present_user: *mut c_void,
    generate_callback: Option<Callback>,
    generate_user: *mut c_void,
    enabled: bool,
    allow_async: bool,
    hudless: Resource,
    flags: u32,
    only_generated: bool,
    rect: [i32; 4],
    id: u64,
}

#[repr(C)]
struct Prepare {
    header: Header,
    id: u64,
    flags: u32,
    list: *mut c_void,
    render: [u32; 2],
    jitter: [f32; 2],
    scale: [f32; 2],
    time_ms: f32,
    reset: bool,
    camera: [f32; 4],
    depth: Resource,
    motion: Resource,
    camera_position: [f32; 3],
    camera_up: [f32; 3],
    camera_right: [f32; 3],
    camera_forward: [f32; 3],
}

#[repr(C)]
struct Generate {
    header: Header,
    list: *mut c_void,
    color: Resource,
    output: [Resource; 4],
    count: u32,
}

#[repr(C)]
struct ProviderVersion {
    header: Header,
    id: u64,
    name: *const std::ffi::c_char,
}

const _: () = {
    use std::mem::offset_of;

    // These x64 offsets catch reordered fields even when the size is unchanged.
    assert!(size_of::<Resource>() == 48);
    assert!(offset_of!(Resource, description) == 8);
    assert!(offset_of!(Resource, state) == 40);
    assert!(size_of::<Versions>() == 56);
    assert!(offset_of!(Versions, count) == 32);
    assert!(offset_of!(Versions, ids) == 40);
    assert!(offset_of!(Versions, names) == 48);
    assert!(size_of::<ProviderOverride>() == 24);
    assert!(offset_of!(ProviderOverride, id) == 16);
    assert!(size_of::<SwapchainDesc>() == 64);
    assert!(offset_of!(SwapchainDesc, output) == 16);
    assert!(offset_of!(SwapchainDesc, desc) == 32);
    assert!(offset_of!(SwapchainDesc, queue) == 56);
    assert!(size_of::<Create>() == 40);
    assert!(offset_of!(Create, display) == 20);
    assert!(offset_of!(Create, max_render) == 28);
    assert!(size_of::<Configure>() == 144);
    assert!(offset_of!(Configure, generate_callback) == 40);
    assert!(offset_of!(Configure, generate_user) == 48);
    assert!(offset_of!(Configure, enabled) == 56);
    assert!(offset_of!(Configure, id) == 136);
    assert!(size_of::<Prepare>() == 232);
    assert!(offset_of!(Prepare, render) == 40);
    assert!(offset_of!(Prepare, jitter) == 48);
    assert!(offset_of!(Prepare, scale) == 56);
    assert!(offset_of!(Prepare, depth) == 88);
    assert!(offset_of!(Prepare, motion) == 136);
    assert!(offset_of!(Generate, color) == 24);
    assert!(offset_of!(Generate, output) == 72);
    assert!(offset_of!(Generate, count) == 264);
};

type CreateFn = unsafe extern "C" fn(*mut *mut c_void, *mut Header, *const c_void) -> u32;
type DestroyFn = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> u32;
type CallFn = unsafe extern "C" fn(*mut *mut c_void, *mut Header) -> u32;

struct Api {
    create: CreateFn,
    destroy: DestroyFn,
    configure: CallFn,
    query: CallFn,
    dispatch: CallFn,
    legacy_providers: Option<Providers>,
    /// In mode 4, the built-in Frame Generation 3 providers, for a GPU
    /// that does not offer the 4.0.2 generator (before RX 9000).
    fallback_providers: Option<Providers>,
}

/// Mode 4 asked for, and the GPU offered no 4.0.2 generator: Frame
/// Generation 3 runs instead (set before the first present).
static ML_FELL_BACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Providers {
    generator: u64,
    swapchain: u64,
}

static API: OnceLock<Api> = OnceLock::new();

/// A provider for an effect. A null device enumerates the SDK's built-in
/// providers. The actual DX12 device also exposes its driver providers.
fn find_provider(query: CallFn, effect: u64, device: *mut c_void, version: &[u8]) -> Option<u64> {
    let mut count = 0;
    let mut versions = Versions {
        header: header(GET_VERSIONS),
        effect,
        device,
        count: &mut count,
        ids: null_mut(),
        names: null_mut(),
    };
    // SAFETY: a null context requests version enumeration. The descriptor
    // and its count remain live for both calls.
    if unsafe { query(null_mut(), &mut versions.header) } != OK || !(1..=64).contains(&count) {
        return None;
    }
    let mut ids = vec![0; count as usize];
    let mut names = vec![null(); count as usize];
    versions.ids = ids.as_mut_ptr();
    versions.names = names.as_mut_ptr();
    // SAFETY: both arrays have the capacity in the count slot.
    if unsafe { query(null_mut(), &mut versions.header) } != OK {
        return None;
    }
    ids.into_iter().zip(names).take(count as usize).find_map(|(id, name)| {
        if id == 0 || name.is_null() {
            return None;
        }
        // SAFETY: the API returns static NUL-terminated version names.
        let name = unsafe { CStr::from_ptr(name) }.to_bytes();
        let matches = if version.ends_with(b".") { name.starts_with(version) } else { name == version };
        matches.then_some(id)
    })
}

/// Loads AMD's frame generation from `dir` (the FidelityFX SDK's
/// `signedbin`, as for the upscaler); its presence turns frame generation
/// on. The DLL's version, or why it did not load.
pub(crate) fn load(dir: &Path, ml: bool) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };
    let path = std::path::absolute(dir.join(DLL)).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("no {DLL} there"));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: a NUL-terminated path; its own folder and System32 only.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if module.is_null() {
        return Err(format!("{DLL} did not load ({})", std::io::Error::last_os_error()));
    }
    let find = |name: &CStr| {
        // SAFETY: a loaded module and a NUL-terminated name.
        unsafe { GetProcAddress(module, name.as_ptr().cast()) }
            .map(|f| f as *const c_void)
            .ok_or_else(|| format!("{DLL} has no {}", name.to_string_lossy()))
    };
    // SAFETY: the API's query export, with its signature.
    let query = unsafe { std::mem::transmute::<*const c_void, CallFn>(find(c"ffxQuery")?) };
    let legacy_providers = if ml {
        None
    } else {
        Some(Providers {
            generator: find_provider(query, CREATE, null_mut(), b"3.")
                .ok_or("no built-in Frame Generation 3 provider")?,
            swapchain: find_provider(query, SWAPCHAIN_CREATE, null_mut(), b"3.")
                .ok_or("no built-in Frame Generation 3 swap chain provider")?,
        })
    };
    // SAFETY: the API's exports, with their signatures.
    let api = unsafe {
        Api {
            create: std::mem::transmute::<*const c_void, CreateFn>(find(c"ffxCreateContext")?),
            destroy: std::mem::transmute::<*const c_void, DestroyFn>(find(c"ffxDestroyContext")?),
            configure: std::mem::transmute::<*const c_void, CallFn>(find(c"ffxConfigure")?),
            query,
            dispatch: std::mem::transmute::<*const c_void, CallFn>(find(c"ffxDispatch")?),
            legacy_providers,
            fallback_providers: if ml {
                find_provider(query, CREATE, null_mut(), b"3.")
                    .zip(find_provider(query, SWAPCHAIN_CREATE, null_mut(), b"3."))
                    .map(|(generator, swapchain)| Providers { generator, swapchain })
            } else {
                None
            },
        }
    };
    let _ = API.set(api);
    Ok(path.display().to_string())
}

fn api() -> Option<&'static Api> {
    API.get()
}

/// Whether frame generation is on (its DLL loaded).
pub(crate) fn enabled() -> bool {
    API.get().is_some()
}

pub(crate) fn ml_enabled() -> bool {
    api().is_some_and(|api| api.legacy_providers.is_none()) && !ML_FELL_BACK.load(Ordering::Relaxed)
}

/// With frame generation the present's list is sent ahead like any other
/// (`Host::present_planes`), and the card finishes the frame while the next
/// one is prepared (the default; the launchers set it since 7b02c83: 13.7-14.3
/// real fps against 9.4). `VICTORY_FRAMEGEN_AHEAD=0`: it runs at once and
/// every present waits for all the card's work first: 24 ms a frame in run
/// t1128-2117-1-B and 17.8 in t1128-2117-2-A, with the card idle behind it.
pub(crate) fn ahead() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = !std::env::var("VICTORY_FRAMEGEN_AHEAD").is_ok_and(|v| v.trim() == "0");
        tracing::info!(on, "FidelityFX frame generation: presents sent ahead (VICTORY_FRAMEGEN_AHEAD)");
        on
    })
}

/// What frame generation needs of an upscaled frame (see
/// `external::FrameInputs`), with the host copies of its depth and motion
/// vectors, which it holds.
#[derive(Clone)]
pub(crate) struct Inputs {
    pub(crate) depth: Arc<HostTexture>,
    pub(crate) motion: Arc<HostTexture>,
    pub(crate) frame: crate::external::FrameInputs,
    pub(crate) pose: Option<camera::Pose>,
}

// SAFETY: D3D12 resources are free-threaded; the rest is plain data.
unsafe impl Send for Inputs {}
// SAFETY: as above.
unsafe impl Sync for Inputs {}

static SUBMITTED: Mutex<Option<Inputs>> = Mutex::new(None);

/// At an upscale recorded into `list`: its inputs feed the next present
/// once the list is submitted (the present runs after it on the queue).
pub(crate) fn record(list: &HostList, inputs: Inputs) {
    list.defer_host(move || {
        let mut submitted = inputs.clone();
        if ml_enabled() {
            submitted.pose = camera::take_matching(submitted.frame.render, submitted.frame.jitter);
        }
        *SUBMITTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(submitted);
    });
}

fn take_inputs() -> Option<Inputs> {
    SUBMITTED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

struct Context {
    api: &'static Api,
    handle: *mut c_void,
}

// SAFETY: the presenter's mutex serializes every use, including the
// callbacks the proxy makes during Present.
unsafe impl Send for Context {}

impl Context {
    fn create(api: &'static Api, desc: &mut Header) -> Result<Self, u32> {
        let mut ctx = Self { api, handle: null_mut() };
        // SAFETY: the typed chain and the handle slot live through the call.
        let code = unsafe { (api.create)(&mut ctx.handle, desc, null()) };
        if code == OK && !ctx.handle.is_null() { Ok(ctx) } else { Err(code) }
    }

    fn configure(&self, desc: &mut Configure) -> u32 {
        // SAFETY: our context and the matching description.
        unsafe { (self.api.configure)(&mut { self.handle }, &mut desc.header) }
    }

    fn dispatch(&self, desc: &mut Header) -> u32 {
        // SAFETY: our context and the matching description.
        unsafe { (self.api.dispatch)(&mut { self.handle }, desc) }
    }

    fn provider(&self) -> String {
        let mut q = ProviderVersion { header: header(PROVIDER_VERSION), id: 0, name: null() };
        // SAFETY: a provider query on our context.
        let code = unsafe { (self.api.query)(&mut { self.handle }, &mut q.header) };
        if code != OK || q.name.is_null() {
            return String::new();
        }
        // SAFETY: a NUL-terminated name the DLL keeps.
        unsafe { CStr::from_ptr(q.name) }.to_string_lossy().into_owned()
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: owned, and no present is in flight when it is released.
        unsafe { (self.api.destroy)(&mut self.handle, null()) };
    }
}

struct Generator {
    context: Context,
    generated: AtomicU64,
    error: AtomicU32,
}

/// Total interpolated frames generated across presenters (for the HUD).
static TOTAL_GENERATED: AtomicU64 = AtomicU64::new(0);
/// Currently armed frame-generation mode: `0` off/disarmed, `3` FG 3.x, `4` FG 4.0.2 ML.
static ACTIVE_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// F7 pauses frame generation and starts it again. The proxy swap chain
/// stays, so a paused present is an ordinary one (as with no upscaled frame).
static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether F7 has paused frame generation. It starts paused with
/// `VICTORY_FRAMEGEN_PAUSED=1` (F7 turns it on).
pub(crate) fn paused() -> bool {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        if std::env::var("VICTORY_FRAMEGEN_PAUSED").is_ok_and(|v| v.trim() == "1") {
            PAUSED.store(true, Ordering::Relaxed);
            tracing::info!("FidelityFX frame generation starts paused (VICTORY_FRAMEGEN_PAUSED=1): F7 turns it on");
        }
    });
    PAUSED.load(Ordering::Relaxed)
}

/// Whether a present with frame generation runs at once, after all the
/// card's work ([`ahead`] off and not paused). Paused, a present is an
/// ordinary one, sent ahead: F7 gives the frame rate back, not only the
/// generated frames. (F7 is looked at first, so a present that turns it on
/// again is already one that runs at once.)
pub(crate) fn present_at_once() -> bool {
    poll_key();
    !ahead() && !paused()
}

/// F7 was pressed since the last call, with one of our windows in front:
/// frame generation paused or started again.
fn poll_key() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F7};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    static DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // SAFETY: a plain key state query.
    let down = unsafe { GetAsyncKeyState(i32::from(VK_F7)) } < 0;
    if DOWN.swap(down, Ordering::Relaxed) || !down {
        return;
    }
    let mut process = 0;
    // SAFETY: plain queries; room for the answer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    if process == std::process::id() {
        let paused = !PAUSED.fetch_xor(true, Ordering::Relaxed);
        tracing::info!(paused, "F7: FidelityFX frame generation {}", if paused { "paused" } else { "on again" });
    }
}

/// Live frame-generation status for the HUD: `(armed_mode, total_generated)`.
pub(crate) fn hud_status() -> (u8, u64) {
    (ACTIVE_MODE.load(Ordering::Relaxed), TOTAL_GENERATED.load(Ordering::Relaxed))
}

/// The proxy's call during Present: generate the frames.
unsafe extern "C" fn generate(desc: *mut Generate, user: *mut c_void) -> u32 {
    // SAFETY: the configured callback gets our boxed generator and a live
    // dispatch description the SDK owns.
    let g = unsafe { &*user.cast::<Generator>() };
    // SAFETY: as above.
    let code = unsafe { (g.context.api.dispatch)(&mut { g.context.handle }, desc.cast()) };
    if code == OK {
        // SAFETY: the description is live for this call; the SDK may update
        // the generated-frame count in it.
        let count = unsafe { std::ptr::addr_of!((*desc).count).read() };
        g.generated.fetch_add(u64::from(count), Ordering::Relaxed);
        TOTAL_GENERATED.fetch_add(u64::from(count), Ordering::Relaxed);
    } else {
        g.error.store(code, Ordering::Relaxed);
    }
    code
}

/// Frame generation for one presenter: the proxy swap chain's context and
/// the generator for the current sizes.
pub(crate) struct Presentation {
    api: &'static Api,
    generator_provider: u64,
    proxy: Context,
    generator: Option<Box<Generator>>,
    key: Option<([u32; 2], [u32; 2], u32)>,
    armed: bool,
    next_id: u64,
    continuous: u64,
    last_frame: u64,
    base_presents: u32,
    logged: bool,
    missing_camera_warning: camera::MissingCameraWarning,
}

impl Presentation {
    /// The proxy swap chain for `hwnd` and its frame generation, in place
    /// of the plain one; `None` when off, for another format, or if the
    /// proxy is refused (the presenter makes a plain one).
    pub(crate) fn create(
        factory: &IDXGIFactory2,
        device: &ID3D12Device,
        queue: &ID3D12CommandQueue,
        hwnd: HWND,
        desc: &DXGI_SWAP_CHAIN_DESC1,
    ) -> Option<(IDXGISwapChain3, Self)> {
        let api = api()?;
        if desc.Format != DXGI_FORMAT_R8G8B8A8_UNORM {
            tracing::warn!(format = desc.Format.0, "FidelityFX frame generation: only an RGBA8 swap chain; presenting without it");
            return None;
        }
        let providers = if let Some(legacy) = api.legacy_providers {
            legacy
        } else if let (Some(generator), Some(swapchain)) = (
            find_provider(api.query, CREATE, device.as_raw(), b"4.0.2"),
            find_provider(api.query, SWAPCHAIN_CREATE, device.as_raw(), b"4.0.0"),
        ) {
            ML_FELL_BACK.store(false, Ordering::Relaxed);
            Providers { generator, swapchain }
        } else if let Some(fallback) = api.fallback_providers {
            // FSR 4's generator is RX 9000 only: an older card gets FSR 3's,
            // as `VICTORY_FIDELITYFX_FRAMEGEN=1` would give.
            tracing::warn!("FidelityFX frame generation: the GPU did not offer the 4.0.2 generator (RX 9000 only); Frame Generation 3 instead");
            ML_FELL_BACK.store(true, Ordering::Relaxed);
            fallback
        } else {
            tracing::warn!("FidelityFX frame generation: the GPU did not offer the 4.0.2 generator and 4.0.0 proxy, nor Frame Generation 3; off");
            return None;
        };
        let mut version = Version { header: header(SWAPCHAIN_VERSION), version: 4 << 22 };
        let mut provider = ProviderOverride {
            header: Header { kind: OVERRIDE_VERSION, next: &mut version.header },
            id: providers.swapchain,
        };
        let mut output = null_mut();
        let mut args = SwapchainDesc {
            header: Header { kind: SWAPCHAIN_CREATE, next: &mut provider.header },
            output: &mut output,
            hwnd,
            desc,
            fullscreen_desc: null(),
            factory: factory.as_raw(),
            queue: queue.as_raw(),
        };
        let proxy = Context::create(api, &mut args.header)
            .inspect_err(|code| tracing::warn!(code, "FidelityFX frame generation: no proxy swap chain; presenting without it"))
            .ok()?;
        if output.is_null() {
            return None;
        }
        // SAFETY: the SDK returned an owned IDXGISwapChain4 reference.
        let swap_chain = unsafe { IDXGISwapChain4::from_raw(output) }.cast().ok()?;
        let proxy_provider = proxy.provider();
        if ml_enabled() && proxy_provider != "4.0.0" {
            tracing::warn!(provider = proxy_provider, "FidelityFX frame generation: unexpected ML proxy provider; off");
            return None;
        }
        tracing::info!(provider = proxy_provider, "FidelityFX frame generation: proxy swap chain");
        Some((
            swap_chain,
            Self { api, generator_provider: providers.generator, proxy, generator: None, key: None, armed: false, next_id: 0, continuous: 0, last_frame: 0, base_presents: 0, logged: false, missing_camera_warning: camera::MissingCameraWarning::default() },
        ))
    }

    /// Frame generation off for the coming presents (another size, or no
    /// upscaled frame): waits for the proxy's queued frames.
    pub(crate) fn disable(&mut self, chain: &IDXGISwapChain3) {
        if !self.armed {
            return;
        }
        if let Some(g) = &self.generator {
            let code = g.context.configure(&mut self.config(chain, false, [0; 2]));
            if code != OK {
                tracing::warn!(code, "FidelityFX frame generation: not switched off");
            }
        }
        let _ = self.proxy.dispatch(&mut header(SWAPCHAIN_WAIT));
        self.armed = false;
        ACTIVE_MODE.store(0, Ordering::Relaxed);
    }

    fn config(&self, chain: &IDXGISwapChain3, armed: bool, display: [u32; 2]) -> Configure {
        let generator = self.generator.as_deref().filter(|_| armed);
        Configure {
            header: header(CONFIGURE),
            swapchain: chain.as_raw(),
            present_callback: null(),
            present_user: null_mut(),
            generate_callback: generator.map(|_| generate as Callback),
            generate_user: generator.map_or(null_mut(), |g| std::ptr::from_ref(g).cast_mut().cast()),
            enabled: armed,
            allow_async: false,
            hudless: Resource { resource: null_mut(), description: [0; 8], state: 0 },
            flags: 0,
            only_generated: false,
            rect: [0, 0, display[0] as i32, display[1] as i32],
            id: self.next_id,
        }
    }

    /// Before presenting frame `frame` at `display` size: the generator
    /// configured and given the last submitted upscale's depth and motion
    /// (recorded into `list`, which runs before the present). True if
    /// frames will be generated for this present.
    pub(crate) fn prepare(
        &mut self,
        chain: &IDXGISwapChain3,
        device: &ID3D12Device,
        list: &HostList,
        display: [u32; 2],
        frame: u64,
    ) -> bool {
        if ml_enabled() {
            camera::clear();
        }
        poll_key();
        if paused() {
            take_inputs();
            self.disable(chain);
            return false;
        }
        let Some(input) = take_inputs().filter(|i| i.frame.render.into_iter().zip(display).all(|(r, d)| r > 0 && r <= d)) else {
            self.disable(chain);
            return false;
        };
        let pose = if ml_enabled() {
            if self.missing_camera_warning.observe(input.pose.is_some()) {
                tracing::warn!(
                    upscales = camera::WARNING_AFTER_UPSCALES,
                    "FidelityFX Frame Generation 4.0.2: no matching velocity-pass camera; presenting ordinary frames. The shader pair was verified on title 1.0.2.1. Set VICTORY_LOG=info,victory_gpu::framegen=debug for matching details"
                );
            }
            let Some(pose) = input.pose else {
                tracing::debug!(frame, "FidelityFX frame generation: no camera matching this upscale");
                self.disable(chain);
                return false;
            };
            pose
        } else {
            camera::Pose::LEGACY
        };
        let flags = input.frame.flags & MOTION_DEPTH_FLAGS;
        let key = (display, input.frame.render, flags);
        if self.key != Some(key) {
            self.renew(chain, device, key);
        }
        let Some(g) = &self.generator else { return false };
        let code = g.context.configure(&mut self.config(chain, true, display));
        if code != OK {
            tracing::warn!(code, "FidelityFX frame generation: configure failed");
            self.disable(chain);
            return false;
        }
        let reset = input.frame.reset || !self.armed || self.last_frame + 1 != frame;
        let mut barriers = Vec::new();
        let depth = read(list, &input.depth, input.frame.depth_desc, &mut barriers);
        let motion = read(list, &input.motion, input.frame.motion_desc, &mut barriers);
        if !barriers.is_empty() {
            // SAFETY: an open list and textures the inputs hold.
            unsafe { list.list.ResourceBarrier(&barriers) };
        }
        list.keep(&input.depth.texture);
        list.keep(&input.motion.texture);
        let mut prepare = Prepare {
            header: header(PREPARE),
            id: self.next_id,
            flags: 0,
            list: list.list.as_raw(),
            render: input.frame.render,
            jitter: input.frame.jitter,
            scale: input.frame.motion_scale,
            time_ms: input.frame.frame_time_ms,
            reset,
            camera: input.frame.camera,
            depth,
            motion,
            camera_position: pose.position,
            camera_up: pose.up,
            camera_right: pose.right,
            camera_forward: pose.forward,
        };
        self.armed = true;
        let code = g.context.dispatch(&mut prepare.header);
        if code != OK {
            tracing::warn!(code, "FidelityFX frame generation: prepare failed");
            self.disable(chain);
            return false;
        }
        self.last_frame = frame;
        self.next_id += 1;
        self.continuous += u64::from(!reset);
        ACTIVE_MODE.store(if ml_enabled() { 4 } else { 3 }, Ordering::Relaxed);
        if self.next_id == 1 {
            let screen = display;
            tracing::info!(frame, render = ?input.frame.render, ?screen, camera_pose = ml_enabled(), "FidelityFX frame generation: first upscaled frame's depth and motion given to it");
        }
        true
    }

    /// A new generator for (display, render, flags).
    fn renew(&mut self, chain: &IDXGISwapChain3, device: &ID3D12Device, key: ([u32; 2], [u32; 2], u32)) {
        self.disable(chain);
        // The old generator goes only once nothing can use it: the proxy's
        // queued frames call it back (`generate_user` points at it), and the
        // lists sent ahead hold its prepares. It went before the wait, while
        // the proxy could still call it.
        let _ = self.proxy.dispatch(&mut header(SWAPCHAIN_WAIT));
        if self.generator.is_some() {
            crate::host::settle_for_present();
        }
        self.generator = None;
        self.key = Some(key);
        self.next_id = 0;
        self.continuous = 0;
        let _ = self.proxy.dispatch(&mut header(SWAPCHAIN_WAIT));
        // SAFETY: the proxy swap chain is live; the presenter serializes us.
        self.base_presents = unsafe { chain.GetLastPresentCount() }.unwrap_or(0);
        let (screen, render, flags) = key;
        let mut backend = BackendDx12 { header: header(BACKEND_DX12), device: device.as_raw() };
        let mut version = Version { header: Header { kind: VERSION, next: &mut backend.header }, version: (4 << 22) | 2 };
        let mut provider = ProviderOverride {
            header: Header { kind: OVERRIDE_VERSION, next: &mut version.header },
            id: self.generator_provider,
        };
        let mut args = Create { header: Header { kind: CREATE, next: &mut provider.header }, flags, display: screen, max_render: render, format: RGBA8_UNORM };
        match Context::create(self.api, &mut args.header) {
            Ok(context) => {
                let provider = context.provider();
                let expected = if ml_enabled() { provider == "4.0.2" } else { provider.starts_with("3.") };
                if expected {
                    tracing::info!(provider, ?screen, ?render, "FidelityFX frame generation: generator for the upscaled frames");
                    self.generator = Some(Box::new(Generator { context, generated: AtomicU64::new(0), error: AtomicU32::new(0) }));
                } else {
                    tracing::warn!(provider, "FidelityFX frame generation: unexpected generator provider; off");
                }
            }
            Err(code) => tracing::warn!(code, ?screen, ?render, "FidelityFX frame generation: no generator"),
        }
    }

    /// After presenting frame `frame`: a failed generation switches it off;
    /// the first steady stretch is logged with the presents it made.
    pub(crate) fn presented(&mut self, chain: &IDXGISwapChain3, frame: u64) {
        let Some(g) = &self.generator else { return };
        let error = g.error.load(Ordering::Relaxed);
        if error != OK {
            tracing::warn!(error, "FidelityFX frame generation: generating failed; off");
            self.disable(chain);
        } else if !self.logged && self.continuous >= 2 && g.generated.load(Ordering::Relaxed) >= 3 {
            let waited = self.proxy.dispatch(&mut header(SWAPCHAIN_WAIT));
            // SAFETY: a live proxy swap chain with completed presents.
            let presents = unsafe { chain.GetLastPresentCount() }.unwrap_or(0).saturating_sub(self.base_presents);
            tracing::info!(
                frame,
                rendered = self.next_id,
                continuous = self.continuous,
                generated = g.generated.load(Ordering::Relaxed),
                presents,
                waited,
                "FidelityFX frame generation: generated frames"
            );
            self.logged = true;
        }
    }
}

/// `copy` as frame generation reads it (compute read), moved there in
/// `list` (`barriers` gets the transition, if any).
fn read(list: &HostList, copy: &Arc<HostTexture>, description: [u32; 8], barriers: &mut Vec<D3D12_RESOURCE_BARRIER>) -> Resource {
    barriers.extend(list.move_texture(copy, HOST_NON_PIXEL_SHADER_RESOURCE));
    Resource { resource: copy.texture.as_raw(), description, state: FFX_COMPUTE_READ }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_dll_frame_generation_stays_off() {
        let dir = std::env::temp_dir().join("victory-no-framegen");
        let _ = std::fs::create_dir_all(&dir);
        assert!(load(&dir, false).unwrap_err().contains(DLL));
    }

    #[test]
    fn selects_the_builtin_three_provider_for_each_effect() {
        unsafe extern "C" fn query(ctx: *mut *mut c_void, desc: *mut Header) -> u32 {
            // SAFETY: find_provider passes its live Versions descriptor.
            let versions = unsafe { &mut *desc.cast::<Versions>() };
            if !ctx.is_null() || versions.header.kind != GET_VERSIONS || !versions.device.is_null() {
                return 1;
            }
            let provider_id = match versions.effect {
                CREATE => 31,
                SWAPCHAIN_CREATE => 32,
                _ => return 1,
            };
            // SAFETY: count is live. On the second call both arrays have two entries.
            unsafe {
                if versions.ids.is_null() && versions.names.is_null() {
                    *versions.count = 2;
                } else if *versions.count == 2 && !versions.ids.is_null() && !versions.names.is_null() {
                    *versions.ids = 40;
                    *versions.ids.add(1) = provider_id;
                    *versions.names = c"4.0.2".as_ptr();
                    *versions.names.add(1) = c"3.1.6".as_ptr();
                } else {
                    return 1;
                }
            }
            OK
        }

        assert_eq!(find_provider(query, CREATE, null_mut(), b"3."), Some(31));
        assert_eq!(find_provider(query, SWAPCHAIN_CREATE, null_mut(), b"3."), Some(32));
    }
}
