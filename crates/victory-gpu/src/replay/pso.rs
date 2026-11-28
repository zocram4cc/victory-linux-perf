//! Pipeline descriptions as parts: every form D3D12 takes them in (graphics,
//! compute, stream) is kept as the subobjects of a stream, and made again
//! from them as a stream.

use std::ffi::{CString, c_void};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, PCSTR};

use super::wire::{Blob, Part};

/// What a subobject holds, by its type: bytes, and whether it starts with a
/// pointer (aligned to 8).
enum Holds {
    Root,
    Code,
    Layout,
    /// Pointers the recorder does not follow.
    Pointers(usize),
    Plain(usize),
}

fn holds(kind: i32) -> Option<Holds> {
    Some(match kind {
        0 => Holds::Root,
        1..=6 | 24 | 25 => Holds::Code,
        7 => Holds::Pointers(size_of::<D3D12_STREAM_OUTPUT_DESC>()),
        8 => Holds::Plain(size_of::<D3D12_BLEND_DESC>()),
        9 | 13 | 14 | 16 | 18 | 20 => Holds::Plain(4),
        10 => Holds::Plain(size_of::<D3D12_RASTERIZER_DESC>()),
        11 => Holds::Plain(size_of::<D3D12_DEPTH_STENCIL_DESC>()),
        12 => Holds::Layout,
        15 => Holds::Plain(size_of::<D3D12_RT_FORMAT_ARRAY>()),
        17 => Holds::Plain(size_of::<DXGI_SAMPLE_DESC>()),
        19 => Holds::Pointers(size_of::<D3D12_CACHED_PIPELINE_STATE>()),
        21 => Holds::Plain(size_of::<D3D12_DEPTH_STENCIL_DESC1>()),
        22 => Holds::Pointers(size_of::<D3D12_VIEW_INSTANCING_DESC>()),
        26 => Holds::Plain(size_of::<D3D12_DEPTH_STENCIL_DESC2>()),
        27 => Holds::Plain(size_of::<D3D12_RASTERIZER_DESC1>()),
        28 => Holds::Plain(size_of::<D3D12_RASTERIZER_DESC2>()),
        _ => return None,
    })
}

/// What the parts need from the recorder.
pub(super) trait Keep {
    fn root(&mut self, root: *mut c_void) -> u32;
    fn code(&mut self, code: &[u8]) -> u32;
    /// Something in a description that is not kept.
    fn lost(&mut self, what: &str);
}

/// # Safety
/// `at` points at a value of `size` bytes.
unsafe fn plain(kind: i32, at: *const c_void, size: usize) -> Part {
    // SAFETY: the caller's.
    Part::Plain {
        kind,
        data: Blob(unsafe { std::slice::from_raw_parts(at.cast::<u8>(), size) }.to_vec()),
    }
}

unsafe fn code(kind: i32, c: &D3D12_SHADER_BYTECODE, keep: &mut dyn Keep) -> Option<Part> {
    if c.pShaderBytecode.is_null() || c.BytecodeLength == 0 {
        return None;
    }
    // SAFETY: the description's own code.
    let bytes =
        unsafe { std::slice::from_raw_parts(c.pShaderBytecode.cast::<u8>(), c.BytecodeLength) };
    Some(Part::Code {
        kind,
        blob: keep.code(bytes),
    })
}

unsafe fn layout(l: &D3D12_INPUT_LAYOUT_DESC) -> Option<Part> {
    if l.pInputElementDescs.is_null() || l.NumElements == 0 {
        return None;
    }
    // SAFETY: the description's own elements, with their names.
    let elements =
        unsafe { std::slice::from_raw_parts(l.pInputElementDescs, l.NumElements as usize) }
            .iter()
            .map(|e| {
                // SAFETY: as above.
                let name = unsafe { e.SemanticName.to_string() }.unwrap_or_default();
                let rest = [
                    e.SemanticIndex,
                    e.Format.0 as u32,
                    e.InputSlot,
                    e.AlignedByteOffset,
                    e.InputSlotClass.0 as u32,
                    e.InstanceDataStepRate,
                ];
                (name, rest)
            })
            .collect();
    Some(Part::Layout { elements })
}

/// # Safety
/// A valid description.
pub(super) unsafe fn compute(
    d: &D3D12_COMPUTE_PIPELINE_STATE_DESC,
    keep: &mut dyn Keep,
) -> Vec<Part> {
    // SAFETY: the caller's description.
    unsafe {
        let mut parts = vec![Part::Root {
            root: keep.root(std::mem::transmute_copy(&d.pRootSignature)),
        }];
        parts.extend(code(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CS.0, &d.CS, keep));
        if d.Flags.0 != 0 {
            parts.push(plain(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_FLAGS.0,
                (&raw const d.Flags).cast(),
                4,
            ));
        }
        parts
    }
}

/// # Safety
/// A valid description.
pub(super) unsafe fn graphics(
    d: &D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    keep: &mut dyn Keep,
) -> Vec<Part> {
    // SAFETY: the caller's description.
    unsafe {
        let mut parts = vec![Part::Root {
            root: keep.root(std::mem::transmute_copy(&d.pRootSignature)),
        }];
        for (kind, c) in [(1, &d.VS), (2, &d.PS), (3, &d.DS), (4, &d.HS), (5, &d.GS)] {
            parts.extend(code(kind, c, keep));
        }
        if d.StreamOutput.NumEntries != 0 {
            keep.lost("a pipeline's stream output");
        }
        parts.push(plain(
            8,
            (&raw const d.BlendState).cast(),
            size_of::<D3D12_BLEND_DESC>(),
        ));
        parts.push(plain(9, (&raw const d.SampleMask).cast(), 4));
        parts.push(plain(
            10,
            (&raw const d.RasterizerState).cast(),
            size_of::<D3D12_RASTERIZER_DESC>(),
        ));
        parts.push(plain(
            11,
            (&raw const d.DepthStencilState).cast(),
            size_of::<D3D12_DEPTH_STENCIL_DESC>(),
        ));
        parts.extend(layout(&d.InputLayout));
        parts.push(plain(13, (&raw const d.IBStripCutValue).cast(), 4));
        parts.push(plain(14, (&raw const d.PrimitiveTopologyType).cast(), 4));
        let targets = D3D12_RT_FORMAT_ARRAY {
            RTFormats: d.RTVFormats,
            NumRenderTargets: d.NumRenderTargets,
        };
        parts.push(plain(
            15,
            (&raw const targets).cast(),
            size_of::<D3D12_RT_FORMAT_ARRAY>(),
        ));
        parts.push(plain(16, (&raw const d.DSVFormat).cast(), 4));
        parts.push(plain(
            17,
            (&raw const d.SampleDesc).cast(),
            size_of::<DXGI_SAMPLE_DESC>(),
        ));
        if d.Flags.0 != 0 {
            parts.push(plain(20, (&raw const d.Flags).cast(), 4));
        }
        parts
    }
}

/// # Safety
/// A valid description.
pub(super) unsafe fn stream(
    d: &D3D12_PIPELINE_STATE_STREAM_DESC,
    keep: &mut dyn Keep,
) -> Vec<Part> {
    let base = d.pPipelineStateSubobjectStream.cast::<u8>().cast_const();
    let mut parts = Vec::new();
    let mut at = 0usize;
    while at + 4 <= d.SizeInBytes {
        // SAFETY: the caller's stream holds whole subobjects, each at a
        // multiple of the size of a pointer.
        unsafe {
            let kind = base.add(at).cast::<i32>().read();
            let Some(h) = holds(kind) else {
                keep.lost(&format!(
                    "a pipeline stream's subobject of type {kind}, and those after it"
                ));
                break;
            };
            let (from, size) = match h {
                Holds::Root => (8, 8),
                Holds::Code | Holds::Layout => (8, 16),
                Holds::Pointers(n) => (8, n),
                Holds::Plain(n) => (4, n),
            };
            let value = base.add(at + from);
            match h {
                Holds::Root => parts.push(Part::Root {
                    root: keep.root(value.cast::<*mut c_void>().read()),
                }),
                Holds::Code => parts.extend(code(kind, &*value.cast(), keep)),
                Holds::Layout => parts.extend(layout(&*value.cast())),
                Holds::Pointers(_) => {
                    // A cached blob is only a shortcut.
                    let empty = std::slice::from_raw_parts(value, size)
                        .iter()
                        .all(|&b| b == 0);
                    if kind != D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CACHED_PSO.0 && !empty {
                        keep.lost(&format!("a pipeline stream's subobject of type {kind}"));
                    }
                }
                Holds::Plain(n) => parts.push(plain(kind, value.cast(), n)),
            }
            at += (from + size).next_multiple_of(8);
        }
    }
    parts
}

/// A pipeline made again.
pub(super) fn make(
    device: &ID3D12Device2,
    parts: &[Part],
    root: impl Fn(u32) -> Option<ID3D12RootSignature>,
    code: impl Fn(u32) -> Option<Vec<u8>>,
) -> Result<ID3D12PipelineState, String> {
    // Everything the stream points at, kept until the pipeline is made.
    let mut codes: Vec<Vec<u8>> = Vec::new();
    let mut names: Vec<CString> = Vec::new();
    let mut elements: Vec<Vec<D3D12_INPUT_ELEMENT_DESC>> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    let mut roots = Vec::new();
    for part in parts {
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        match part {
            Part::Root { root: 0 } => {}
            Part::Root { root: id } => {
                let r = root(*id)
                    .ok_or_else(|| format!("root signature {id} is not in the capture"))?;
                bytes.extend([0u8; 8]);
                bytes.extend((r.as_raw() as usize).to_le_bytes());
                roots.push(r);
            }
            Part::Code { kind, blob } => {
                let c =
                    code(*blob).ok_or_else(|| format!("shader {blob} is not in the capture"))?;
                bytes.extend(kind.to_le_bytes());
                bytes.extend([0u8; 4]);
                bytes.extend((c.as_ptr() as usize).to_le_bytes());
                bytes.extend(c.len().to_le_bytes());
                codes.push(c);
            }
            Part::Plain { kind, data } => {
                bytes.extend(kind.to_le_bytes());
                bytes.extend_from_slice(&data.0);
            }
            Part::Layout { elements: all } => {
                let made: Vec<D3D12_INPUT_ELEMENT_DESC> = all
                    .iter()
                    .map(|(name, r)| {
                        names.push(CString::new(name.as_str()).unwrap_or_default());
                        D3D12_INPUT_ELEMENT_DESC {
                            SemanticName: PCSTR(
                                names.last().map_or(std::ptr::null(), |n| n.as_ptr().cast()),
                            ),
                            SemanticIndex: r[0],
                            Format: DXGI_FORMAT(r[1] as i32),
                            InputSlot: r[2],
                            AlignedByteOffset: r[3],
                            InputSlotClass: D3D12_INPUT_CLASSIFICATION(r[4] as i32),
                            InstanceDataStepRate: r[5],
                        }
                    })
                    .collect();
                bytes.extend(
                    D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_INPUT_LAYOUT
                        .0
                        .to_le_bytes(),
                );
                bytes.extend([0u8; 4]);
                bytes.extend((made.as_ptr() as usize).to_le_bytes());
                bytes.extend((made.len() as u64).to_le_bytes());
                elements.push(made);
            }
        }
    }
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    // Aligned for the pointers in it.
    let mut words: Vec<u64> = bytes
        .chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap_or_default()))
        .collect();
    let desc = D3D12_PIPELINE_STATE_STREAM_DESC {
        SizeInBytes: words.len() * 8,
        pPipelineStateSubobjectStream: words.as_mut_ptr().cast(),
    };
    // SAFETY: a stream of whole subobjects; all it points at is alive.
    let made: windows::core::Result<ID3D12PipelineState> =
        unsafe { device.CreatePipelineState(&desc) };
    made.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Kept {
        codes: Vec<Vec<u8>>,
        lost: Vec<String>,
    }

    impl Keep for Kept {
        fn root(&mut self, root: *mut c_void) -> u32 {
            root as usize as u32
        }
        fn code(&mut self, code: &[u8]) -> u32 {
            self.codes.push(code.to_vec());
            self.codes.len() as u32 - 1
        }
        fn lost(&mut self, what: &str) {
            self.lost.push(what.into());
        }
    }

    #[test]
    fn a_stream_is_taken_apart() {
        #[repr(C, align(8))]
        struct Sub<T>(i32, T);
        #[repr(C)]
        struct Stream {
            root: Sub<usize>,
            ms: Sub<D3D12_SHADER_BYTECODE>,
            blend: Sub<D3D12_BLEND_DESC>,
            mask: Sub<u32>,
            raster: Sub<D3D12_RASTERIZER_DESC>,
            depth: Sub<D3D12_DEPTH_STENCIL_DESC>,
            targets: Sub<D3D12_RT_FORMAT_ARRAY>,
            dsv: Sub<DXGI_FORMAT>,
            samples: Sub<DXGI_SAMPLE_DESC>,
            unknown: Sub<u32>,
        }
        let bytes = [1u8, 2, 3];
        let mut s = Stream {
            root: Sub(0, 77),
            ms: Sub(
                25,
                D3D12_SHADER_BYTECODE {
                    pShaderBytecode: bytes.as_ptr().cast(),
                    BytecodeLength: 3,
                },
            ),
            blend: Sub(8, D3D12_BLEND_DESC::default()),
            mask: Sub(9, 5),
            raster: Sub(
                10,
                D3D12_RASTERIZER_DESC {
                    DepthBias: 6,
                    ..Default::default()
                },
            ),
            depth: Sub(11, D3D12_DEPTH_STENCIL_DESC::default()),
            targets: Sub(
                15,
                D3D12_RT_FORMAT_ARRAY {
                    NumRenderTargets: 3,
                    ..Default::default()
                },
            ),
            dsv: Sub(16, DXGI_FORMAT(40)),
            samples: Sub(
                17,
                DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
            ),
            unknown: Sub(99, 0),
        };
        let d = D3D12_PIPELINE_STATE_STREAM_DESC {
            SizeInBytes: size_of::<Stream>(),
            pPipelineStateSubobjectStream: (&raw mut s).cast(),
        };
        let mut kept = Kept::default();
        // SAFETY: the stream above.
        let parts = unsafe { stream(&d, &mut kept) };
        assert_eq!(kept.codes, [vec![1, 2, 3]]);
        assert_eq!(kept.lost.len(), 1, "the unknown subobject is reported");
        assert_eq!(parts.len(), 9);
        assert_eq!(parts[0], Part::Root { root: 77 });
        assert_eq!(parts[1], Part::Code { kind: 25, blob: 0 });
        assert_eq!(
            parts[3],
            Part::Plain {
                kind: 9,
                data: Blob(5u32.to_le_bytes().to_vec())
            }
        );
        let Part::Plain { kind: 10, data } = &parts[4] else {
            panic!("{:?}", parts[4])
        };
        assert_eq!(data.0.len(), size_of::<D3D12_RASTERIZER_DESC>());
        assert_eq!(
            parts[7],
            Part::Plain {
                kind: 16,
                data: Blob(40u32.to_le_bytes().to_vec())
            }
        );
    }
}
