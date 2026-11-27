//! What the host's D3D12 must offer before a title starts.
//!
//! Translated pipelines need shader model 6.6, mesh shaders and root
//! signatures that index the descriptor heaps directly. Without them every
//! pipeline is refused and the title, given none, crashes seconds in on a
//! null read in its own code: a crash that says nothing of the cause. This
//! checks first and names what is missing.
//!
//! Windows 10's own runtime has none of the three (shader model 6.5 at
//! most); the DirectX 12 Agility SDK in `D3D12\` beside `victory.exe` brings
//! them. See `apps/victory/src/agility.rs`.

use std::path::PathBuf;

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::*;

/// The Agility SDK version `victory.exe` asks for (`D3D12SDKVersion`): the
/// minor version of NuGet `Microsoft.Direct3D.D3D12` 1.620.2.
pub const AGILITY_SDK_VERSION: u32 = 620;

/// `D3D12_ERROR_INVALID_REDIST`: the app-local `D3D12Core.dll` is missing,
/// or of another version than `D3D12SDKVersion`, on a PC whose own runtime
/// is older.
const INVALID_REDIST: u32 = 0x887E_0003;

/// The host's D3D12 as found.
pub struct Report {
    /// The GPU the device is on.
    pub adapter: String,
    /// The `D3D12Core.dll` this process runs.
    pub core: Option<PathBuf>,
    /// Whether that is the Agility SDK beside the executable.
    pub agility: bool,
}

impl Report {
    /// One line: the runtime and the adapter.
    pub fn summary(&self) -> String {
        let runtime = match (&self.core, self.agility) {
            (Some(core), true) => {
                format!("Agility SDK 1.{AGILITY_SDK_VERSION} ({})", core.display())
            }
            (Some(core), false) => format!("Windows' own ({})", core.display()),
            (None, _) => "unknown".to_owned(),
        };
        format!("{runtime}, on {}", self.adapter)
    }
}

/// The path of the `D3D12Core.dll` loaded in this process, if any.
pub fn loaded_core() -> Option<PathBuf> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    let name: Vec<u16> = "D3D12Core.dll".encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated name; the module is not unloaded while D3D12
    // is in use, and the buffer's length is passed.
    unsafe {
        let module = GetModuleHandleW(name.as_ptr());
        if module.is_null() {
            return None;
        }
        let mut path = [0u16; 1024];
        let len = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize;
        (len > 0 && len < path.len()).then(|| String::from_utf16_lossy(&path[..len]).into())
    }
}

/// Whether `core` is the Agility SDK's, in `D3D12\` beside the executable.
fn is_app_local(core: &std::path::Path) -> bool {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("D3D12")))
    else {
        return false;
    };
    core.parent().is_some_and(|p| {
        p.to_string_lossy()
            .eq_ignore_ascii_case(&dir.to_string_lossy())
    })
}

/// Logs which D3D12 runtime the process runs (after a device exists).
pub fn log_runtime() {
    let core = loaded_core();
    let agility = core.as_deref().is_some_and(is_app_local);
    tracing::info!(
        core = %core.as_deref().map_or("none".into(), |c| c.display().to_string()),
        agility,
        sdk = AGILITY_SDK_VERSION,
        "host D3D12 runtime"
    );
}

/// Where the Agility SDK should be, for messages.
fn agility_dir() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("D3D12").display().to_string()))
        .unwrap_or_else(|| "D3D12 beside victory.exe".to_owned())
}

/// Checks the host GPU and runtime: `Err` names what is missing and what to
/// do about it.
pub fn check() -> Result<Report, String> {
    let device = victory_shader::emit::host::create_preferred_device().map_err(|e| {
        if e.code().0 as u32 == INVALID_REDIST {
            format!(
                "Direct3D 12 refused the runtime in {} (0x887E0003): D3D12Core.dll there is \
                 missing or is not the Agility SDK 1.{AGILITY_SDK_VERSION} one. Extract the \
                 whole zip again, D3D12 folder included.",
                agility_dir()
            )
        } else {
            format!("no Direct3D 12 device: {e}. Update the graphics driver.")
        }
    })?;
    let core = loaded_core();
    let agility = core.as_deref().is_some_and(is_app_local);
    let report = Report {
        adapter: adapter_name(&device).unwrap_or_else(|| "an unknown GPU".to_owned()),
        core,
        agility,
    };

    let mut missing = Vec::new();
    let mut model = D3D12_FEATURE_DATA_SHADER_MODEL {
        HighestShaderModel: D3D_SHADER_MODEL_6_6,
    };
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
    let mut options7 = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
    // SAFETY: each query writes its own structure of the given size.
    let (sm_ok, options_ok, options7_ok) = unsafe {
        (
            device
                .CheckFeatureSupport(
                    D3D12_FEATURE_SHADER_MODEL,
                    (&raw mut model).cast(),
                    size_of_val(&model) as u32,
                )
                .is_ok(),
            device
                .CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS,
                    (&raw mut options).cast(),
                    size_of_val(&options) as u32,
                )
                .is_ok(),
            device
                .CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS7,
                    (&raw mut options7).cast(),
                    size_of_val(&options7) as u32,
                )
                .is_ok(),
        )
    };
    if !sm_ok || model.HighestShaderModel.0 < D3D_SHADER_MODEL_6_6.0 {
        missing.push(if sm_ok {
            format!(
                "shader model 6.6 (this runtime has {}.{})",
                model.HighestShaderModel.0 >> 4,
                model.HighestShaderModel.0 & 0xf
            )
        } else {
            "shader model 6.6".to_owned()
        });
    }
    if !options7_ok || options7.MeshShaderTier == D3D12_MESH_SHADER_TIER_NOT_SUPPORTED {
        missing.push("mesh shaders".to_owned());
    }
    if !options_ok || options.ResourceBindingTier.0 < D3D12_RESOURCE_BINDING_TIER_3.0 {
        missing.push("resource binding tier 3".to_owned());
    }
    // What the 0x80070057 on every pipeline was: the runtime itself refusing
    // the flags, whatever the GPU can do.
    if let Err(e) = serialize_heap_indexed() {
        missing.push(format!(
            "root signatures with directly indexed heaps (flags 0xc00 refused: {:#010x})",
            e.code().0 as u32
        ));
    }
    if missing.is_empty() {
        return Ok(report);
    }

    let advice = if report.agility {
        "The DirectX 12 Agility SDK is loaded, so the GPU or its driver lacks these: \
         update the graphics driver. VIctory needs a DirectX 12 Ultimate GPU \
         (Radeon RX 6000 or newer, GeForce RTX 20 or newer, Intel Arc)."
            .to_owned()
    } else {
        format!(
            "Windows' own Direct3D 12 runtime is in use, and on Windows 10 it lacks these. \
             VIctory brings a newer one (the DirectX 12 Agility SDK): {}\\D3D12Core.dll must \
             be there. Extract the whole zip, D3D12 folder included, then update Windows \
             (Windows 10 1909 or later) and the graphics driver.",
            agility_dir()
        )
    };
    Err(format!(
        "this PC's Direct3D 12 cannot run the game's shaders: no {}. Runtime: {}. {advice}",
        missing.join(", no "),
        report.summary()
    ))
}

/// Serializes the smallest root signature with the heaps directly indexed.
fn serialize_heap_indexed() -> windows::core::Result<()> {
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                Flags: D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED
                    | D3D12_ROOT_SIGNATURE_FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED,
                ..Default::default()
            },
        },
    };
    let mut blob = None;
    // SAFETY: a valid description and out pointer.
    unsafe { D3D12SerializeVersionedRootSignature(&desc, &mut blob, None) }
}

fn adapter_name(device: &ID3D12Device) -> Option<String> {
    // SAFETY: plain DXGI queries.
    unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapterByLuid(device.GetAdapterLuid()).ok()?;
        let desc = adapter.GetDesc1().ok()?;
        Some(
            String::from_utf16_lossy(&desc.Description)
                .trim_end_matches('\0')
                .to_owned(),
        )
    }
}
