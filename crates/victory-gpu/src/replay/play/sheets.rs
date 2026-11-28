//! The frame recorder's watch for stretched geometry in a replay
//! (`VICTORY_RECORD_SHEETS=1`, see `crate::sheets`): for shaders
//! retranslated with the watch (`--retranslate`, then `--shaders`), whose
//! pre-skinned mesh programs note what they see through the root UAV
//! `u<SLOT_SHEETS>`. The recorded root signatures do not have it: each one
//! gets it added as its last parameter. Each draw is bound a region of our
//! buffer of its own (the game's host does the same), named by its list and
//! command; each run says what the draws noted.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use victory_shader::emit::{ROOT_CBV_SPACE, SHEET_REGION_DWORDS, SHEET_REGIONS, SLOT_SHEETS};

/// The watch in a replay: our buffer, and per root signature the parameter
/// it is bound to.
pub(super) struct Watch {
    buffer: ID3D12Resource,
    data: *mut u32,
    va: u64,
    params: HashMap<u32, u32>,
    /// Root signatures that could not take the parameter.
    refused: u32,
    /// The parameter of the graphics root signature set last.
    current: Cell<Option<u32>>,
    /// The next region, and what each was given to.
    next: AtomicU32,
    labels: Mutex<Vec<String>>,
    /// By pipeline: whether its mesh program is pre-skinned.
    watched: Mutex<HashMap<u32, bool>>,
}

/// The last region is bound when a root signature is set, for draws that
/// are not given one of their own; what lands there is not reported.
const SPARE: u32 = SHEET_REGIONS - 1;

impl Watch {
    /// The watch if `VICTORY_RECORD_SHEETS=1`.
    pub(super) fn new(device: &ID3D12Device) -> anyhow::Result<Option<Watch>> {
        if !std::env::var_os("VICTORY_RECORD_SHEETS").is_some_and(|v| v == "1") {
            return Ok(None);
        }
        let bytes = u64::from(SHEET_REGIONS * SHEET_REGION_DWORDS) * 4;
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
            current: Cell::new(None),
            next: AtomicU32::new(0),
            labels: Mutex::new(vec![String::new(); SHEET_REGIONS as usize]),
            watched: Mutex::new(HashMap::new()),
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

    fn region(&self, r: u32) -> u64 {
        self.va + u64::from(r) * u64::from(SHEET_REGION_DWORDS) * 4
    }

    /// After root signature `id` was set on `l`: our parameter bound (to
    /// the spare region until a draw is given its own).
    pub(super) fn bind(&self, l: &ID3D12GraphicsCommandList, graphics: bool, id: u32) {
        let param = self.params.get(&id).copied();
        if graphics {
            self.current.set(param);
        }
        if let Some(param) = param {
            // SAFETY: an open list; the buffer lives as long as the player.
            unsafe {
                if graphics {
                    l.SetGraphicsRootUnorderedAccessView(param, self.region(SPARE));
                } else {
                    l.SetComputeRootUnorderedAccessView(param, self.region(SPARE));
                }
            }
        }
    }

    /// Whether pipeline `pso` draws with a pre-skinned mesh program: by the
    /// hash in its name (`mesh <hash>`) and that container in `containers`
    /// beside the capture (`--retranslate` collects them).
    pub(super) fn watched(&self, pso: u32, frame: &crate::replay::wire::Frame, folder: &std::path::Path) -> bool {
        let Ok(mut known) = self.watched.lock() else {
            return false;
        };
        *known.entry(pso).or_insert_with(|| {
            let Some(p) = frame.pipelines.iter().find(|p| p.id == pso) else {
                return false;
            };
            let Some(at) = p.name.find("mesh ") else {
                return false;
            };
            let hash: String = p.name[at + 5..].chars().take_while(char::is_ascii_hexdigit).collect();
            let containers = folder.with_file_name("containers");
            std::fs::read(containers.join(format!("{hash}.cdxil"))).is_ok_and(|c| crate::sheets::pre_skinned(&c))
        })
    }

    /// Before a draw: a region of its own, named by `label`.
    pub(super) fn draw(&self, l: &ID3D12GraphicsCommandList, label: impl FnOnce() -> String) {
        let Some(param) = self.current.get() else {
            return;
        };
        let r = self.next.fetch_add(1, Ordering::Relaxed) % SPARE;
        if let Ok(mut labels) = self.labels.lock() {
            labels[r as usize] = label();
        }
        // SAFETY: an open list; the buffer lives as long as the player.
        unsafe { l.SetGraphicsRootUnorderedAccessView(param, self.region(r)) };
    }

    /// What a run's draws noted (the card has finished it), cleared for the
    /// next: a summary and a line per draw.
    pub(super) fn take(&self) -> Vec<String> {
        let n = (SHEET_REGIONS * SHEET_REGION_DWORDS) as usize;
        // SAFETY: the mapping, of that many dwords; the card is idle.
        let all = unsafe { std::slice::from_raw_parts_mut(self.data, n) };
        let labels = self.labels.lock().map(|l| l.clone()).unwrap_or_default();
        let mut lines = Vec::new();
        let (mut rows, mut jumps) = ([0u32; 4], Vec::new());
        for (r, region) in all.chunks_mut(SHEET_REGION_DWORDS as usize).enumerate().take(SPARE as usize) {
            if let Some(d) = crate::sheets::read_region(region) {
                for (k, &on) in d.rows.iter().enumerate() {
                    rows[k] += u32::from(on);
                }
                if !d.events.is_empty() {
                    jumps.push(d.jump());
                }
                lines.push(format!("sheet draw {}", crate::sheets::line(0, &labels[r], &d)));
            }
        }
        // (The lists are built once and run every loop: the regions keep
        // their draws.)
        all.fill(0);
        let _ = &self.buffer;
        jumps.sort_by(|a, b| b.total_cmp(a));
        let mut out = vec![format!(
            "sheets (VICTORY_RECORD_SHEETS): {} draws given a region; draws per widest span over {:?} of the screen: {rows:?}; {} draws with a jump, largest {:?}; {} root signatures with the watch, {} without",
            labels.iter().filter(|l| !l.is_empty()).count(),
            victory_shader::emit::SHEET_SPANS.map(|s| s / 2.0),
            jumps.len(),
            jumps.iter().take(12).map(|j| (j * 100.0).round() / 100.0).collect::<Vec<_>>(),
            self.params.len(),
            self.refused
        )];
        out.extend(lines);
        out
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
