//! The DirectX 12 Agility SDK: the D3D12 runtime shipped next to the exe.
//!
//! Windows 10's own D3D12 runtime stops at shader model 6.5 and refuses root
//! signatures that index the descriptor heaps directly (flags 0x400 and
//! 0x800, `E_INVALIDARG`), which every translated pipeline uses: without a
//! newer runtime the title gets no pipelines and crashes seconds in.
//!
//! D3D12 reads these two exports from the process's executable (`build.rs`
//! exports them). When `D3D12SDKVersion` is newer than the system's runtime,
//! it loads `D3D12Core.dll` from `D3D12SDKPath`, relative to the executable,
//! instead. That works on Windows 10 1909 and later. Where the system's
//! runtime is as new or newer (a current Windows 11), the system's is used
//! and the folder is ignored.
//!
//! `scripts/get-agility-sdk.ps1` puts the matching `D3D12Core.dll` (NuGet
//! `Microsoft.Direct3D.D3D12` 1.620.2) in `D3D12\`. Keep the version here,
//! there and in `victory_gpu::requirements` in step: with a mismatched DLL
//! on a PC whose runtime is older, device creation fails with
//! `D3D12_ERROR_INVALID_REDIST` (0x887E0003).
#![allow(non_upper_case_globals)]

/// The minor version of the `Microsoft.Direct3D.D3D12` package (1.620.2).
#[unsafe(no_mangle)]
#[used]
pub static D3D12SDKVersion: u32 = victory_hle::host_gpu::AGILITY_SDK_VERSION;

/// A C string pointer D3D12 reads when it first loads.
#[repr(transparent)]
pub struct SdkPath(*const std::ffi::c_char);

// SAFETY: points at a static, immutable C string.
unsafe impl Sync for SdkPath {}

/// Where `D3D12Core.dll` is, relative to the executable.
#[unsafe(no_mangle)]
#[used]
pub static D3D12SDKPath: SdkPath = SdkPath(c".\\D3D12\\".as_ptr());
