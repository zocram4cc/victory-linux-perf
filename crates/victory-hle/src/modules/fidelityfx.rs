//! `amd_fidelityfx_loader_xbox.dll`: AMD FidelityFX's API, served by the
//! host's PC build of it on the host GPU when the user names a folder that
//! holds one (`VICTORY_FIDELITYFX_DIR`). Without it, or if it offers no
//! upscaler for the host GPU, this module stays out of the registry and the
//! title's own console DLLs run as before.
//!
//! Both builds have the same API (the FidelityFX SDK's, MIT): five functions
//! over chains of descriptions, each starting with a type id and the next
//! one's address; contexts are opaque pointers. The console-specific parts
//! are translated: the backend description names the console device (type
//! 0x0100_0002; on PC type 2 with the host device), and a dispatch names the
//! title's command list and resources (their host list and host copies, in
//! the states the title gives, see `victory_gpu::external`). The rest passes
//! through unchanged: sizes, flags, jitter, motion-vector scale, camera
//! values, the resource descriptions, the context. Only the upscaler: other
//! effects the title asks for (frame generation) are refused. A diagnostics
//! build generates frames itself from the upscale's depth and motion
//! vectors with `VICTORY_FIDELITYFX_FRAMEGEN=1` (`victory_gpu::external::
//! frame_generation_inputs`).
#![allow(non_snake_case)]
// Title code calls these through its import table.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{CStr, c_char, c_void};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use victory_gpu::object::Object;
use windows::Win32::Graphics::Direct3D12::ID3D12Device;
use windows::core::Interface;

use crate::{HleModule, hle_export};

/// The PC build's upscaler, which has the five functions itself.
const DLL: &str = "amd_fidelityfx_upscaler_dx12.dll";

// Return codes.
const OK: u32 = 0;
const UNKNOWN_TYPE: u32 = 2;
const NO_PROVIDER: u32 = 4;
const PARAMETER: u32 = 6;

// Description types.
const CREATE_UPSCALE: u64 = 0x1_0000;
const DISPATCH_UPSCALE: u64 = 0x1_0001;
const CONSOLE_BACKEND: u64 = 0x100_0002;
const HOST_BACKEND: u64 = 2;
const GET_VERSIONS: u64 = 4;
/// `ffxOverrideVersion` (ffx_api.h): a version id from `GET_VERSIONS`.
const OVERRIDE_VERSION: u64 = 5;
const GET_PROVIDER_VERSION: u64 = 6;

/// `VICTORY_FFX_VERSION=<prefix>` (e.g. `3.1`): the host upscaler of the
/// first version whose name starts with it, instead of the DLL's default.
/// Under vkd3d-proton on an RX 7900 XTX the default (FSR 4.1.1) drew a grey
/// or black picture (2026-11-27); 3.1 is the non-ML upscaler. Set by
/// [`versions`] when the DLL offers such a version.
static OVERRIDE: OnceLock<u64> = OnceLock::new();

#[repr(C)]
struct OverrideVersion {
    header: Header,
    id: u64,
}
const MEMORY_USAGE_WITH_DEVICE: u64 = 0x1_0009;

#[repr(C)]
#[derive(Clone, Copy)]
struct Header {
    kind: u64,
    next: *mut Header,
}

/// An upscaler context: flags, largest render and output sizes, a message
/// callback.
#[repr(C)]
#[derive(Clone, Copy)]
struct CreateUpscale {
    header: Header,
    flags: u32,
    max_render: [u32; 2],
    max_output: [u32; 2],
    message: usize,
}

/// The backend description: a device.
#[repr(C)]
struct Backend {
    header: Header,
    device: *mut c_void,
}

/// A resource, its description (type, format, sizes, mips, flags, usage)
/// and the state it is in.
#[repr(C)]
#[derive(Clone, Copy)]
struct Resource {
    resource: *mut c_void,
    description: [u32; 8],
    state: u32,
}

/// One upscaling: the command list, then colour, depth, motion vectors,
/// exposure, reactive mask, transparency mask and output; then jitter,
/// motion-vector scale, sizes, sharpening, frame time, pre-exposure, reset,
/// camera and flags, which pass through.
#[repr(C)]
#[derive(Clone, Copy)]
struct DispatchUpscale {
    header: Header,
    list: *mut c_void,
    resources: [Resource; 7],
    rest: [u32; 18],
}

// FidelityFX SDK v2.4.0, upscalers/include/ffx_upscale.h.
const _: () = {
    use std::mem::offset_of;

    assert!(size_of::<CreateUpscale>() == 0x30);
    assert!(size_of::<Resource>() == 0x30);
    assert!(size_of::<DispatchUpscale>() == 0x1b0);
    assert!(offset_of!(DispatchUpscale, list) == 0x10);
    assert!(offset_of!(DispatchUpscale, resources) == 0x18); // color
    assert!(offset_of!(DispatchUpscale, rest) == 0x168); // jitterOffset
};

/// The versions a backend offers: count in and out, ids, names.
#[repr(C)]
struct GetVersions {
    header: Header,
    create_kind: u64,
    device: *mut c_void,
    count: *mut u64,
    ids: *mut u64,
    names: *mut *const c_char,
}

#[repr(C)]
struct ProviderVersion {
    header: Header,
    id: u64,
    name: *const c_char,
}

type Create = unsafe extern "C" fn(*mut *mut c_void, *mut Header, *const c_void) -> u32;
type Destroy = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> u32;
/// Configure, query and dispatch.
type Call = unsafe extern "C" fn(*mut *mut c_void, *mut Header) -> u32;

struct Host {
    create: Create,
    destroy: Destroy,
    configure: Call,
    query: Call,
    dispatch: Call,
    device: ID3D12Device,
}

// SAFETY: D3D12 devices are free-threaded; the rest is code.
unsafe impl Send for Host {}
// SAFETY: as above.
unsafe impl Sync for Host {}

static HOST: OnceLock<Host> = OnceLock::new();

macro_rules! once {
    ($level:ident, $($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::$level!($($arg)*));
    }};
}

/// Serves the title's FidelityFX calls with the PC upscaler in `dir`
/// (`VICTORY_FIDELITYFX_DIR`) if it offers one for the host GPU; else the
/// title's own DLLs run. Call once, before the import registry is first
/// used: it holds this module only when serving.
pub fn use_host_dir(dir: Option<&Path>) {
    let Some(dir) = dir else {
        tracing::info!(
            "no FidelityFX folder (VICTORY_FIDELITYFX_DIR): a title's own FidelityFX DLLs run"
        );
        return;
    };
    match load(dir) {
        Ok((host, versions)) => {
            tracing::info!(folder = %dir.display(), versions, "FidelityFX: the host's upscaler serves the title's");
            let _ = HOST.set(host);
            victory_gpu::external::enable_host_fidelityfx();
            #[cfg(feature = "diagnostics")]
            if let Ok(mode) = std::env::var("VICTORY_FIDELITYFX_FRAMEGEN")
                && matches!(mode.as_str(), "1" | "4")
            {
                match victory_gpu::external::use_frame_generation(dir, mode == "4") {
                    Ok(dll) => tracing::info!(dll, mode, "FidelityFX: frame generation on the title's upscaled frames"),
                    Err(why) => tracing::warn!("FidelityFX frame generation: {why}; off"),
                }
            }
        }
        Err(why) => tracing::warn!(
            folder = %dir.display(),
            "FidelityFX: {why}; the title's own DLLs run"
        ),
    }
}

/// Whether the host's FidelityFX serves the title's.
pub fn serving() -> bool {
    HOST.get().is_some()
}

fn load(dir: &Path) -> Result<(Host, String), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LoadLibraryExW,
    };
    // The search flags below want a full path.
    let path = std::path::absolute(dir.join(DLL)).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("no {DLL} there"));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // Its own folder and System32 only, never the current folder.
    let search = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
    // SAFETY: a NUL-terminated path to the user's FidelityFX build.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), null_mut(), search) };
    if module.is_null() {
        return Err(format!(
            "{DLL} did not load ({})",
            std::io::Error::last_os_error()
        ));
    }
    let find = |name: &CStr| {
        // SAFETY: a loaded module and a NUL-terminated name.
        unsafe { GetProcAddress(module, name.as_ptr().cast()) }
            .map(|f| f as *const c_void)
            .ok_or_else(|| format!("{DLL} has no {}", name.to_string_lossy()))
    };
    let device = victory_gpu::external::host_device().ok_or("there is no host GPU")?;
    // SAFETY: the API's exports, with its signatures.
    let host = unsafe {
        Host {
            create: std::mem::transmute::<*const c_void, Create>(find(c"ffxCreateContext")?),
            destroy: std::mem::transmute::<*const c_void, Destroy>(find(c"ffxDestroyContext")?),
            configure: std::mem::transmute::<*const c_void, Call>(find(c"ffxConfigure")?),
            query: std::mem::transmute::<*const c_void, Call>(find(c"ffxQuery")?),
            dispatch: std::mem::transmute::<*const c_void, Call>(find(c"ffxDispatch")?),
            device,
        }
    };
    let versions = versions(&host)?;
    Ok((host, versions))
}

/// The upscaler versions the host DLL offers for the host GPU, by name; an
/// error if none.
fn versions(host: &Host) -> Result<String, String> {
    let mut count = 0u64;
    let mut query = GetVersions {
        header: Header {
            kind: GET_VERSIONS,
            next: null_mut(),
        },
        create_kind: CREATE_UPSCALE,
        device: host.device.as_raw(),
        count: &mut count,
        ids: null_mut(),
        names: null_mut(),
    };
    // SAFETY: a version query naming the host device.
    let code = unsafe { (host.query)(null_mut(), &mut query.header) };
    if code != OK || count == 0 {
        return Err(format!(
            "it offers no upscaler for the host GPU (code {code})"
        ));
    }
    let mut names = vec![null::<c_char>(); count as usize];
    let mut ids = vec![0u64; count as usize];
    query.names = names.as_mut_ptr();
    query.ids = ids.as_mut_ptr();
    // SAFETY: as above, with room for `count` names and ids.
    unsafe { (host.query)(null_mut(), &mut query.header) };
    let named: Vec<(u64, String)> = names
        .iter()
        .zip(&ids)
        .filter(|(n, _)| !n.is_null())
        // SAFETY: NUL-terminated names the DLL keeps.
        .map(|(&n, &id)| (id, unsafe { CStr::from_ptr(n) }.to_string_lossy().into_owned()))
        .collect();
    if let Ok(want) = std::env::var("VICTORY_FFX_VERSION") {
        let want = want.trim();
        match named.iter().find(|(_, name)| name.starts_with(want)) {
            Some(&(id, ref name)) => {
                let _ = OVERRIDE.set(id);
                tracing::info!(version = %name, id = format_args!("{id:#x}"), "FidelityFX: host upscaler version chosen (VICTORY_FFX_VERSION)");
            }
            None => tracing::warn!(want, "FidelityFX: no host upscaler version starts with VICTORY_FFX_VERSION: the default runs"),
        }
    }
    Ok(named.into_iter().map(|(_, name)| name).collect::<Vec<_>>().join(", "))
}

/// Puts the host backend after the title's upscaler description, in place
/// of the console's backend that follows it (where the title puts it);
/// anywhere else the console's is left for the host DLL to ignore, ours
/// coming first.
fn link(upscale: &mut CreateUpscale, backend: &mut Backend) {
    let after = upscale.header.next;
    // SAFETY: the title's chain.
    backend.header.next = match unsafe { after.as_ref() } {
        Some(h) if h.kind == CONSOLE_BACKEND => h.next,
        _ => after,
    };
    backend.header.kind = HOST_BACKEND;
    upscale.header.next = &raw mut backend.header;
}

/// A FidelityFX resource state as the host resource state it means (as the
/// PC backend reads it); `None` for generic read and indirect arguments,
/// which a host texture cannot be in.
fn host_state(state: u32) -> Option<u32> {
    Some(match state {
        0x1 | 0x80 => 0,  // common, present
        0x2 => 0x8,       // unordered access
        0x4 => 0x40,      // compute read
        0x8 => 0x80,      // pixel read
        0xc => 0xc0,      // both
        0x10 => 0x800,    // copy source
        0x20 => 0x400,    // copy destination
        0x100 => 0x4,     // render target
        0x200 => 0x10,    // depth write
        _ => return None, // generic read, indirect arguments
    })
}

extern "C" fn ffxCreateContext(
    context: *mut *mut c_void,
    desc: *mut Header,
    memory: *const c_void,
) -> u32 {
    let Some(host) = HOST.get() else {
        return NO_PROVIDER;
    };
    if context.is_null() || desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: the title passes a chain of descriptions.
    let kind = unsafe { (*desc).kind };
    if kind != CREATE_UPSCALE {
        once!(
            warn,
            kind = format_args!("{kind:#x}"),
            "FidelityFX: only the upscaler runs on the host; another effect was refused"
        );
        return NO_PROVIDER;
    }
    // SAFETY: an upscaler description, by its type.
    let mut upscale = unsafe { desc.cast::<CreateUpscale>().read() };
    let mut backend = Backend {
        header: Header {
            kind: HOST_BACKEND,
            next: null_mut(),
        },
        device: host.device.as_raw(),
    };
    link(&mut upscale, &mut backend);
    // `VICTORY_FFX_VERSION`: the chosen version, right after our backend.
    let mut chosen = OverrideVersion {
        header: Header {
            kind: OVERRIDE_VERSION,
            next: null_mut(),
        },
        id: 0,
    };
    if let Some(&id) = OVERRIDE.get() {
        chosen.id = id;
        chosen.header.next = backend.header.next;
        backend.header.next = &raw mut chosen.header;
    }
    // SAFETY: a PC chain; the title's later descriptions live for the call.
    let code = unsafe { (host.create)(context, &mut upscale.header, memory) };
    if code != OK {
        tracing::error!(code, render = ?upscale.max_render, output = ?upscale.max_output, flags = format_args!("{:#x}", upscale.flags), "FidelityFX: the host refused the title's upscaler");
        return code;
    }
    let mut version = ProviderVersion {
        header: Header {
            kind: GET_PROVIDER_VERSION,
            next: null_mut(),
        },
        id: 0,
        name: null(),
    };
    // SAFETY: a provider query on the context just made.
    unsafe { (host.query)(context, &mut version.header) };
    // SAFETY: a NUL-terminated name the DLL keeps, if any.
    let name = (!version.name.is_null())
        .then(|| unsafe { CStr::from_ptr(version.name) }.to_string_lossy());
    tracing::info!(
        provider = name.as_deref().unwrap_or("?"),
        id = format_args!("{:#x}", version.id),
        render = ?upscale.max_render,
        output = ?upscale.max_output,
        flags = format_args!("{:#x}", upscale.flags),
        "FidelityFX: upscaler context on the host"
    );
    // SAFETY: a successful create wrote the context.
    #[cfg(feature = "diagnostics")]
    context_flags(|f| f.insert(unsafe { *context } as usize, upscale.flags));
    OK
}

/// The creation flags of each upscaler context, for frame generation's
/// (diagnostics).
#[cfg(feature = "diagnostics")]
fn context_flags<R>(f: impl FnOnce(&mut std::collections::HashMap<usize, u32>) -> R) -> R {
    static FLAGS: std::sync::Mutex<Option<std::collections::HashMap<usize, u32>>> = std::sync::Mutex::new(None);
    f(FLAGS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default())
}

extern "C" fn ffxDestroyContext(context: *mut *mut c_void, memory: *const c_void) -> u32 {
    #[cfg(feature = "diagnostics")]
    if !context.is_null() {
        // SAFETY: the title passes its context slot.
        context_flags(|f| f.remove(&(unsafe { *context } as usize)));
    }
    // SAFETY: the title passes its context.
    HOST.get()
        .map_or(NO_PROVIDER, |h| unsafe { (h.destroy)(context, memory) })
}

/// Settings (key-value pairs, debug messages) name nothing console-specific.
extern "C" fn ffxConfigure(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    // SAFETY: the title passes its context and descriptions.
    HOST.get()
        .map_or(NO_PROVIDER, |h| unsafe { (h.configure)(context, desc) })
}

extern "C" fn ffxQuery(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    let Some(host) = HOST.get() else {
        return NO_PROVIDER;
    };
    if desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: the title passes a description.
    let kind = unsafe { (*desc).kind };
    // These carry the console device.
    if kind == GET_VERSIONS || kind == MEMORY_USAGE_WITH_DEVICE {
        once!(
            warn,
            kind = format_args!("{kind:#x}"),
            "FidelityFX: a query naming a device was refused"
        );
        return PARAMETER;
    }
    // SAFETY: as above.
    unsafe { (host.query)(context, desc) }
}

extern "C" fn ffxDispatch(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    let Some(host) = HOST.get() else {
        return NO_PROVIDER;
    };
    if desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: the title passes a description.
    let kind = unsafe { (*desc).kind };
    if kind != DISPATCH_UPSCALE {
        once!(
            warn,
            kind = format_args!("{kind:#x}"),
            "FidelityFX: only upscaling runs on the host; a dispatch was refused"
        );
        return UNKNOWN_TYPE;
    }
    // SAFETY: an upscaling, by its type.
    let mut dispatch = unsafe { desc.cast::<DispatchUpscale>().read() };
    let title_list = dispatch.list.cast::<Object>().cast_const();
    // The host list as the title's calls so far left it: with `VICTORY_THIN`
    // they are translated now, on this thread (`victory_gpu::external::
    // on_host_list`); without that the color had no host copy at the call and
    // the upscaling never ran (a black screen).
    let run = move |list: *const Object| -> u32 {
        // Depth and motion as the title names them, for frame generation.
        #[cfg(feature = "diagnostics")]
        let title = dispatch;
        // Alive for the call (the list holds them too).
        let mut textures = Vec::new();
        for (index, r) in dispatch
            .resources
            .iter_mut()
            .enumerate()
            .filter(|(_, r)| !r.resource.is_null())
        {
            let wanted = host_state(r.state);
            let found = wanted.and_then(|w| victory_gpu::external::host_texture(list, r.resource.cast::<Object>(), w));
            let texture = found.clone().filter(|&(_, state)| Some(state) == wanted);
            let Some((texture, _)) = texture else {
                // Which of the dispatch's resources (0 color, 1 depth, 2 motion
                // vectors, then exposure, reactive, transparency, output), and
                // whether it has no host copy or one that cannot take the state.
                once!(
                    warn,
                    state = r.state,
                    index,
                    wanted = ?wanted,
                    host = ?found.as_ref().map(|(t, state)| (state, unsafe { t.GetDesc() }.Flags.0, unsafe { t.GetDesc() }.Format.0)),
                    "FidelityFX: a resource has no host texture in its state; the upscaling did not run"
                );
                return PARAMETER;
            };
            r.resource = texture.as_raw();
            textures.push(texture);
        }
        once!(info, formats = ?textures.iter().map(|t| {
            // SAFETY: a live texture.
            unsafe { t.GetDesc() }.Format.0
        }).collect::<Vec<_>>(), "FidelityFX: first upscaling, host formats of its textures");
        let code = victory_gpu::external::record(list, |host_list| {
            dispatch.list = host_list.as_raw();
            // SAFETY: the PC dispatch on host objects; the context is the host's.
            unsafe { (host.dispatch)(context, &raw mut dispatch.header) }
        })
        .unwrap_or(PARAMETER);
        #[cfg(feature = "diagnostics")]
        if code == OK && !context.is_null() {
            // SAFETY: the title passes its context slot.
            let flags = context_flags(|f| f.get(&(unsafe { *context } as usize)).copied()).unwrap_or(0);
            let (depth, motion) = (title.resources[1], title.resources[2]);
            if !depth.resource.is_null() && !motion.resource.is_null() {
                let frame = frame_inputs(&title, flags);
                once!(info, result = code, render = ?frame.render, "FidelityFX: first successful host upscale");
                victory_gpu::external::frame_generation_inputs(
                    list,
                    depth.resource.cast::<Object>().cast_const(),
                    motion.resource.cast::<Object>().cast_const(),
                    frame,
                );
            }
        }
        code
    };
    victory_gpu::external::on_host_list(title_list, run).unwrap_or_else(|| {
        once!(warn, "FidelityFX: the title's list has no host list; the upscaling did not run");
        PARAMETER
    })
}

/// What frame generation needs of an upscaling (`rest` in the order of
/// `ffxDispatchDescUpscale`: jitter, motion-vector scale, render size,
/// output size, sharpening, sharpness, frame time, pre-exposure, reset,
/// camera, flags) and its context's creation `flags`.
#[cfg(feature = "diagnostics")]
fn frame_inputs(d: &DispatchUpscale, flags: u32) -> victory_gpu::external::FrameInputs {
    let f = |i: usize| f32::from_bits(d.rest[i]);
    victory_gpu::external::FrameInputs {
        render: [d.rest[4], d.rest[5]],
        jitter: [f(0), f(1)],
        motion_scale: [f(2), f(3)],
        frame_time_ms: f(10),
        reset: d.rest[12] & 0xff != 0,
        camera: [f(13), f(14), f(15), f(16)],
        flags,
        depth_desc: d.resources[1].description,
        motion_desc: d.resources[2].description,
    }
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "fidelityfx",
        dlls: &["amd_fidelityfx_loader_xbox.dll"],
        exports: vec![
            hle_export!(ffxConfigure),
            hle_export!(ffxCreateContext),
            hle_export!(ffxDestroyContext),
            hle_export!(ffxDispatch),
            hle_export!(ffxQuery),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: u64, next: *mut Header) -> Header {
        Header { kind, next }
    }

    /// The title's chain: upscaler, console backend, version.
    #[test]
    fn the_console_backend_becomes_the_hosts() {
        let mut version = header(0x1_000b, null_mut());
        let mut console = Backend {
            header: header(CONSOLE_BACKEND, &raw mut version),
            device: 0x1234 as *mut c_void,
        };
        let mut upscale = CreateUpscale {
            header: header(CREATE_UPSCALE, &raw mut console.header),
            flags: 0x29,
            max_render: [1280, 720],
            max_output: [1920, 1080],
            message: 0,
        };
        let mut backend = Backend {
            header: header(0, null_mut()),
            device: 0x5678 as *mut c_void,
        };
        link(&mut upscale, &mut backend);
        assert_eq!(upscale.header.next, &raw mut backend.header);
        assert_eq!(backend.header.kind, HOST_BACKEND);
        assert_eq!(backend.header.next, &raw mut version);
        assert_eq!(backend.device, 0x5678 as *mut c_void);
        // Without a console backend right after it, ours goes in front.
        let mut upscale = CreateUpscale {
            header: header(CREATE_UPSCALE, &raw mut version),
            ..upscale
        };
        link(&mut upscale, &mut backend);
        assert_eq!(backend.header.next, &raw mut version);
    }

    /// Without the PC build the title's own DLL serves its imports.
    #[test]
    fn without_the_dll_the_titles_own_runs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).err().unwrap().contains(DLL));
        assert!(!serving());
        let symbol = victory_pe::ImportSymbol::Name {
            hint: 0,
            name: "ffxCreateContext".into(),
        };
        let resolver = crate::HleResolver::new(crate::HostPolicy::none());
        assert_eq!(
            resolver.classify("amd_fidelityfx_loader_xbox.dll", &symbol),
            None
        );
    }

    /// Frame generation reads the upscale's constants from their places in
    /// the description.
    #[cfg(feature = "diagnostics")]
    #[test]
    fn frame_generation_reads_the_upscales_constants() {
        let mut rest = [0u32; 18];
        let floats = [(0, 0.25f32), (1, -0.5), (2, 1920.0), (3, 1080.0), (10, 33.3), (13, 0.1), (14, 1000.0), (15, 1.2), (16, 1.0)];
        for (i, v) in floats {
            rest[i] = v.to_bits();
        }
        (rest[4], rest[5], rest[12]) = (1920, 1080, 1);
        let mut resources = [Resource { resource: null_mut(), description: [0; 8], state: 0 }; 7];
        resources[1].description[1] = 7;
        resources[2].description[1] = 9;
        let d = DispatchUpscale { header: header(DISPATCH_UPSCALE, null_mut()), list: null_mut(), resources, rest };
        let f = frame_inputs(&d, 0x29);
        assert_eq!(f.render, [1920, 1080]);
        assert_eq!((f.jitter, f.motion_scale), ([0.25, -0.5], [1920.0, 1080.0]));
        assert_eq!((f.frame_time_ms, f.reset, f.flags), (33.3, true, 0x29));
        assert_eq!(f.camera, [0.1, 1000.0, 1.2, 1.0]);
        assert_eq!((f.depth_desc[1], f.motion_desc[1]), (7, 9));
    }

    #[test]
    fn resource_states_read_as_the_pc_backend_reads_them() {
        assert_eq!(host_state(0xc), Some(0xc0));
        assert_eq!(host_state(0x2), Some(0x8));
        assert_eq!(host_state(0x200), Some(0x10));
        assert_eq!(host_state(0x14), None);
    }
}
