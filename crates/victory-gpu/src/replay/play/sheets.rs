//! The frame recorder's watch for stretched geometry in a replay
//! (`VICTORY_RECORD_SHEETS=1`, see `crate::sheets`): for shaders
//! retranslated with the watch (`--retranslate`, then `--shaders`), whose
//! pre-skinned mesh programs note what they see through the root UAV
//! `u<SLOT_SHEETS>`. The recorded root signatures do not have it: each one
//! gets it added as its last parameter, which every command that sets a root
//! signature then binds to a buffer of ours. Each run says what was noted.

use std::collections::HashMap;

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use victory_shader::emit::{ROOT_CBV_SPACE, SHEET_BUCKETS, SHEET_SLOTS, SLOT_SHEETS};

/// The watch in a replay: our buffer, and per root signature the parameter
/// it is bound to.
pub(super) struct Watch {
    buffer: ID3D12Resource,
    data: *mut u32,
    va: u64,
    params: HashMap<u32, u32>,
    /// Root signatures that could not take the parameter.
    refused: u32,
}

impl Watch {
    /// The watch if `VICTORY_RECORD_SHEETS=1`.
    pub(super) fn new(device: &ID3D12Device) -> anyhow::Result<Option<Watch>> {
        if !std::env::var_os("VICTORY_RECORD_SHEETS").is_some_and(|v| v == "1") {
            return Ok(None);
        }
        let bytes = u64::from(SHEET_BUCKETS * SHEET_SLOTS) * 4;
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_CUSTOM,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
            MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut made: Option<ID3D12Resource> = None;
        let mut data = std::ptr::null_mut();
        // SAFETY: plain creation and mapping of a buffer the CPU sees.
        let buffer = unsafe {
            device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut made,
            )?;
            let buffer = made.ok_or_else(|| anyhow::anyhow!("no buffer for the sheet notes"))?;
            buffer.Map(0, None, Some(&mut data))?;
            std::ptr::write_bytes(data.cast::<u8>(), 0, bytes as usize);
            buffer
        };
        // SAFETY: a live buffer.
        let va = unsafe { buffer.GetGPUVirtualAddress() };
        Ok(Some(Watch {
            buffer,
            data: data.cast(),
            va,
            params: HashMap::new(),
            refused: 0,
        }))
    }

    /// Root signature `id` from its recorded `blob`, with the parameter
    /// added where it can be.
    pub(super) fn root(&mut self, device: &ID3D12Device, id: u32, blob: &[u8]) -> windows::core::Result<ID3D12RootSignature> {
        if let Some((more, param)) = with_sheets(blob) {
            // SAFETY: a blob just serialized.
            if let Ok(root) = unsafe { device.CreateRootSignature::<ID3D12RootSignature>(0, &more) } {
                self.params.insert(id, param);
                return Ok(root);
            }
        }
        self.refused += 1;
        // SAFETY: a blob the host took.
        unsafe { device.CreateRootSignature(0, blob) }
    }

    /// After root signature `id` was set on `l`: our parameter bound.
    pub(super) fn bind(&self, l: &ID3D12GraphicsCommandList, graphics: bool, id: u32) {
        if let Some(&param) = self.params.get(&id) {
            // SAFETY: an open list; the buffer lives as long as the player.
            unsafe {
                if graphics {
                    l.SetGraphicsRootUnorderedAccessView(param, self.va);
                } else {
                    l.SetComputeRootUnorderedAccessView(param, self.va);
                }
            }
        }
    }

    /// What a run noted (the card has finished it), cleared for the next.
    pub(super) fn take(&self) -> String {
        let n = (SHEET_BUCKETS * SHEET_SLOTS) as usize;
        // SAFETY: the mapping, of that many dwords; the card is idle.
        let all = unsafe { std::slice::from_raw_parts_mut(self.data, n) };
        let rows = crate::sheets::counts(all);
        all.fill(0);
        let _ = &self.buffer;
        format!(
            "sheets (VICTORY_RECORD_SHEETS): waves noted per span over {:?} of the screen: {rows:?}; {} root signatures with the watch, {} without",
            victory_shader::emit::SHEET_SPANS.map(|s| s / 2.0),
            self.params.len(),
            self.refused
        )
    }
}

/// A root signature blob with a root UAV `u<SLOT_SHEETS>` in
/// `ROOT_CBV_SPACE` added last, and that parameter's index.
fn with_sheets(blob: &[u8]) -> Option<(Vec<u8>, u32)> {
    // SAFETY: plain calls on a blob the host serialized; the descriptions
    // read live as long as the deserializer.
    unsafe {
        let mut out: *mut std::ffi::c_void = std::ptr::null_mut();
        D3D12CreateVersionedRootSignatureDeserializer(
            blob.as_ptr().cast(),
            blob.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID,
            &mut out,
        )
        .ok()?;
        if out.is_null() {
            return None;
        }
        let d = ID3D12VersionedRootSignatureDeserializer::from_raw(out);
        let desc = &*d.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1).ok()?;
        let one = &desc.Anonymous.Desc_1_1;
        let mut params: Vec<D3D12_ROOT_PARAMETER1> =
            std::slice::from_raw_parts(one.pParameters, one.NumParameters as usize).to_vec();
        let param = params.len() as u32;
        params.push(D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                    ShaderRegister: u32::from(SLOT_SHEETS),
                    RegisterSpace: ROOT_CBV_SPACE,
                    Flags: D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        });
        let more = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
            Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
                Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: params.len() as u32,
                    pParameters: params.as_ptr(),
                    ..*one
                },
            },
        };
        let mut out = None;
        D3D12SerializeVersionedRootSignature(&more, &mut out, None).ok()?;
        let out = out?;
        let bytes = std::slice::from_raw_parts(out.GetBufferPointer().cast::<u8>(), out.GetBufferSize()).to_vec();
        let _ = out.as_raw();
        Some((bytes, param))
    }
}
