//! The frame recorder and replayer on the PC's own adapter (skipped without
//! one): a small program with clears, indirect draws of textures picked from
//! a heap, compute, buffers the CPU changes between submissions, descriptors
//! replaced between them, and textures that share memory. It runs without
//! the recorder, then with it, then one frame of it is replayed; every
//! picture and buffer must be the same.
//!
//! `VICTORY_TEST_D3D_DEBUG=1` runs it under the debug layer.
#![cfg(all(windows, feature = "diagnostics"))]

use std::mem::ManuallyDrop;

use victory_gpu::replay::play::{self, Buffers, Options, Player};
use victory_gpu::replay::record::{self, At, Settings};
use windows::Win32::Foundation::{HANDLE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    D3D_FEATURE_LEVEL_12_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCSTR, s, w};

const SIZE: u32 = 128;

const DRAW: &str = r#"
#define RS "RootConstants(num32BitConstants=2, b0), CBV(b1), DescriptorTable(SRV(t0, numDescriptors=unbounded)), DescriptorTable(Sampler(s0))"
cbuffer Pick : register(b0) { uint pick; uint pad; };
cbuffer Place : register(b1) { float4 place; };
Texture2D all[] : register(t0);
SamplerState point_sampler : register(s0);
struct V { float4 at : SV_Position; float2 uv : UV; };
[RootSignature(RS)]
V vs(uint i : SV_VertexID) {
    float2 c = float2(i & 1, (i >> 1) & 1);
    if (i == 3) c = float2(1, 0);
    if (i == 4) c = float2(0, 1);
    if (i == 5) c = float2(1, 1);
    V v;
    v.uv = c;
    v.at = float4(place.xy + c * place.zw, 0.5, 1);
    return v;
}
float4 ps(V v) : SV_Target { return all[NonUniformResourceIndex(pick)].SampleLevel(point_sampler, v.uv, 0); }
"#;

/// Triangles of vertices and indices.
const PLAIN: &str = r#"
#define RS "RootFlags(ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT)"
struct V { float4 at : SV_Position; float3 color : COLOR; };
[RootSignature(RS)]
V vs(float2 at : POSITION, float3 color : COLOR1) { V v; v.at = float4(at, 0.4, 1); v.color = color; return v; }
float4 ps(V v) : SV_Target { return float4(v.color, 1); }
"#;

/// A quad from a mesh shader, its texture picked from the heap by index.
const MESH: &str = r#"
#define RS "RootFlags(CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED), RootConstants(num32BitConstants=5, b0)"
cbuffer C : register(b0) { float4 place; uint pick; };
struct V { float4 at : SV_Position; float2 uv : UV; };
[RootSignature(RS)]
[numthreads(1, 1, 1)]
[outputtopology("triangle")]
void ms(out vertices V v[4], out indices uint3 t[2]) {
    SetMeshOutputCounts(4, 2);
    for (uint i = 0; i < 4; i++) {
        float2 c = float2(i & 1, i >> 1);
        v[i].at = float4(place.xy + c * place.zw, 0.3, 1);
        v[i].uv = c;
    }
    t[0] = uint3(0, 1, 2);
    t[1] = uint3(2, 1, 3);
}
[RootSignature(RS)]
float4 ps(V v) : SV_Target {
    Texture2D<float4> t = ResourceDescriptorHeap[pick];
    return t.Load(int3(v.uv * 16, 0)) * 0.5 + float4(v.uv, 0, 0) * 0.5;
}
"#;

/// Compiles with the Windows SDK's dxc, if there is one.
fn dxc(source: &str, entry: &str, target: &str) -> Option<Vec<u8>> {
    let kits = std::fs::read_dir("C:/Program Files (x86)/Windows Kits/10/bin").ok()?;
    let exe = kits
        .flatten()
        .map(|e| e.path().join("x64/dxc.exe"))
        .filter(|p| p.exists())
        .max()?;
    let dir = tempfile::tempdir().ok()?;
    let (from, to) = (dir.path().join("a.hlsl"), dir.path().join("a.dxil"));
    std::fs::write(&from, source).ok()?;
    let out = std::process::Command::new(exe)
        .args(["-T", target, "-E", entry, "-Fo"])
        .arg(&to)
        .arg(&from)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(to).ok()
}

/// The mesh pipeline, from a stream, by way of a pipeline library; and the
/// signature of its indirect draws. `None` without dxc or mesh shaders.
fn mesh(device: &ID3D12Device, blend: D3D12_BLEND_DESC) -> Option<(Made, ID3D12CommandSignature)> {
    #[repr(C, align(8))]
    struct Sub<T>(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE, T);
    #[repr(C)]
    struct Stream {
        root: Sub<*mut std::ffi::c_void>,
        ms: Sub<D3D12_SHADER_BYTECODE>,
        ps: Sub<D3D12_SHADER_BYTECODE>,
        blend: Sub<D3D12_BLEND_DESC>,
        mask: Sub<u32>,
        raster: Sub<D3D12_RASTERIZER_DESC>,
        depth: Sub<D3D12_DEPTH_STENCIL_DESC>,
        targets: Sub<D3D12_RT_FORMAT_ARRAY>,
        dsv: Sub<DXGI_FORMAT>,
        samples: Sub<DXGI_SAMPLE_DESC>,
        topology: Sub<D3D12_PRIMITIVE_TOPOLOGY_TYPE>,
    }
    let (ms, ps) = (dxc(MESH, "ms", "ms_6_6")?, dxc(MESH, "ps", "ps_6_6")?);
    // SAFETY: plain queries and creation; the stream holds whole subobjects.
    unsafe {
        let mut o = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        let size = size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS7>() as u32;
        device
            .CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, (&raw mut o).cast(), size)
            .ok()?;
        if o.MeshShaderTier == D3D12_MESH_SHADER_TIER_NOT_SUPPORTED {
            return None;
        }
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &ms).unwrap();
        let mut formats = D3D12_RT_FORMAT_ARRAY {
            NumRenderTargets: 1,
            ..Default::default()
        };
        formats.RTFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
        let mut stream = Stream {
            root: Sub(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_ROOT_SIGNATURE,
                root.as_raw(),
            ),
            ms: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS, code(&ms)),
            ps: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PS, code(&ps)),
            blend: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_BLEND, blend),
            mask: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_MASK, u32::MAX),
            raster: Sub(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RASTERIZER,
                D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID,
                    CullMode: D3D12_CULL_MODE_NONE,
                    ..Default::default()
                },
            ),
            depth: Sub(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL,
                D3D12_DEPTH_STENCIL_DESC::default(),
            ),
            targets: Sub(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RENDER_TARGET_FORMATS,
                formats,
            ),
            dsv: Sub(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL_FORMAT,
                DXGI_FORMAT_D32_FLOAT,
            ),
            samples: Sub(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_DESC,
                DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
            ),
            topology: Sub(
                D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PRIMITIVE_TOPOLOGY,
                D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            ),
        };
        let desc = D3D12_PIPELINE_STATE_STREAM_DESC {
            SizeInBytes: size_of::<Stream>(),
            pPipelineStateSubobjectStream: (&raw mut stream).cast(),
        };
        let library: ID3D12PipelineLibrary1 = device
            .cast::<ID3D12Device1>()
            .unwrap()
            .CreatePipelineLibrary::<ID3D12PipelineLibrary>(&[])
            .unwrap()
            .cast()
            .unwrap();
        {
            let made: ID3D12PipelineState = device
                .cast::<ID3D12Device2>()
                .unwrap()
                .CreatePipelineState(&desc)
                .unwrap();
            library.StorePipeline(w!("mesh"), &made).unwrap();
        }
        let pso: ID3D12PipelineState = library.LoadPipeline(w!("mesh"), &desc).unwrap();
        record::name(&pso, "a mesh pipeline");
        let argument = D3D12_INDIRECT_ARGUMENT_DESC {
            Type: D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH,
            ..Default::default()
        };
        let mut signature: Option<ID3D12CommandSignature> = None;
        let desc = D3D12_COMMAND_SIGNATURE_DESC {
            ByteStride: 12,
            NumArgumentDescs: 1,
            pArgumentDescs: &argument,
            NodeMask: 0,
        };
        device
            .CreateCommandSignature(&desc, None::<&ID3D12RootSignature>, &mut signature)
            .unwrap();
        Some((Made { root, pso }, signature.unwrap()))
    }
}

/// Writes the arguments of indirect draws (two records: six vertices, and
/// one far too many that the clamp cuts to none).
const ARGS: &str = r#"
#define RS "SRV(t0), UAV(u0)"
ByteAddressBuffer from : register(t0);
RWByteAddressBuffer args : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {
    args.Store4(0, uint4(from.Load(0), 1, 0, 0));
    args.Store4(16, uint4(4000000000u, 1, 0, 0));
}
"#;

/// The argument clamp's parameters, with a simple bound. The same argument
/// buffer is written again while still bound to the clamp.
const CLAMP: &str = r#"
#define RS "RootConstants(num32BitConstants=6, b0), SRV(t0, flags=DATA_VOLATILE), SRV(t1, flags=DATA_VOLATILE), UAV(u0), UAV(u1)"
cbuffer C : register(b0) { uint kind; uint stride; uint records; uint args_offset; uint has_count; uint count_offset; };
ByteAddressBuffer from : register(t0);
ByteAddressBuffer count : register(t1);
RWByteAddressBuffer to : register(u0);
RWByteAddressBuffer budget : register(u1);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= records) return;
    uint4 a = from.Load4(args_offset + id.x * stride);
    if (a.x > 1000) a.x = 0;
    to.Store4(id.x * stride, a);
    uint was;
    budget.InterlockedAdd(0, a.x, was);
}
"#;

/// Adds what the CPU gives to what was there, and writes it on.
const ADD: &str = r#"
#define RS "SRV(t0), DescriptorTable(UAV(u0)), UAV(u1)"
ByteAddressBuffer from : register(t0);
RWTexture2D<float4> image : register(u0);
RWByteAddressBuffer sum : register(u1);
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint at = (id.y * 16 + id.x) * 4;
    uint v = from.Load(at) + sum.Load(at) * 3;
    sum.Store(at, v);
    image[id.xy] = image[id.xy] + float4((v & 15) / 255.0, 0, 1 / 255.0, 1);
}
"#;

struct Made {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
}

fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Vec<u8> {
    let (mut code, mut errors): (Option<ID3DBlob>, Option<ID3DBlob>) = (None, None);
    // SAFETY: the source and names live through the call; the blobs' own
    // bytes.
    unsafe {
        let r = D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            None,
            None,
            None,
            entry,
            target,
            1 << 20,
            0,
            &mut code,
            Some(&mut errors),
        );
        if r.is_err() {
            let e = errors.unwrap();
            let text =
                std::slice::from_raw_parts(e.GetBufferPointer().cast::<u8>(), e.GetBufferSize());
            panic!("{}", String::from_utf8_lossy(text));
        }
        let code = code.unwrap();
        std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
            .to_vec()
    }
}

fn code(bytes: &[u8]) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE {
        pShaderBytecode: bytes.as_ptr().cast(),
        BytecodeLength: bytes.len(),
    }
}

fn compute(device: &ID3D12Device, source: &str) -> Made {
    let cs = compile(source, s!("main"), s!("cs_5_1"));
    // SAFETY: plain creation.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &cs).unwrap();
        let pso = device
            .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: code(&cs),
                ..Default::default()
            })
            .unwrap();
        Made { root, pso }
    }
}

fn buffer_desc(size: u64, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    }
}

fn texture_desc(
    size: u32,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(size),
        Height: size,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: flags,
        ..Default::default()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum In {
    Card,
    Upload,
    Back,
    /// The PC's memory, mapped, that shaders write too.
    Shared,
}

fn committed(
    device: &ID3D12Device,
    place: In,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
) -> (ID3D12Resource, *mut u8) {
    let heap = match place {
        In::Card => D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        },
        In::Upload => D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            ..Default::default()
        },
        In::Back => D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_READBACK,
            ..Default::default()
        },
        In::Shared => D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_CUSTOM,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
            MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
            ..Default::default()
        },
    };
    let mut made: Option<ID3D12Resource> = None;
    let mut data = std::ptr::null_mut();
    // SAFETY: plain creation and mapping.
    unsafe {
        device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, desc, state, None, &mut made)
            .unwrap();
        let made = made.unwrap();
        if place != In::Card {
            made.Map(0, None, Some(&mut data)).unwrap();
        }
        (made, data.cast())
    }
}

fn moved(
    res: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: a borrowed pointer never released.
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
        ..Default::default()
    }
}

fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: ManuallyDrop::new(None),
            }),
        },
        ..Default::default()
    }
}

fn aliased(before: &ID3D12Resource, after: &ID3D12Resource) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Aliasing: ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                // SAFETY: borrowed pointers never released.
                pResourceBefore: unsafe { std::mem::transmute_copy(before) },
                // SAFETY: as above.
                pResourceAfter: unsafe { std::mem::transmute_copy(after) },
            }),
        },
        ..Default::default()
    }
}

const COMMON: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COMMON;
const TARGET: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_RENDER_TARGET;
const READ: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
const UAV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
const TO: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_DEST;
const FROM: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_SOURCE;
const ARGUMENTS: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT;

/// A texture the frames do not touch: what each subresource holds, row by
/// row, with the rows' distance in its copyable layout.
#[derive(PartialEq, Debug, Clone)]
struct Still {
    name: &'static str,
    subs: Vec<(usize, Vec<Vec<u8>>)>,
}

/// What the program ends with.
#[derive(PartialEq, Debug)]
struct Outcome {
    still: Vec<Still>,
    picture: Vec<u8>,
    image: Vec<u8>,
    sum: Vec<u8>,
    shared: Vec<u8>,
}

struct Scene {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    event: HANDLE,
    value: u64,
    lists: Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)>,
    draw: Made,
    plain: Made,
    mesh: Option<(Made, ID3D12CommandSignature)>,
    vertices: (ID3D12Resource, *mut u8),
    samplers: ID3D12DescriptorHeap,
    still: Vec<(&'static str, ID3D12Resource, D3D12_RESOURCE_STATES)>,
    copies: (
        ID3D12CommandQueue,
        ID3D12CommandAllocator,
        ID3D12GraphicsCommandList,
    ),
    args: Made,
    clamp: Made,
    add: Made,
    signature: ID3D12CommandSignature,
    views: ID3D12DescriptorHeap,
    step: usize,
    rtvs: ID3D12DescriptorHeap,
    dsvs: ID3D12DescriptorHeap,
    target: ID3D12Resource,
    _depth: ID3D12Resource,
    fixed: ID3D12Resource,
    one: ID3D12Resource,
    other: ID3D12Resource,
    _heap: ID3D12Heap,
    image: ID3D12Resource,
    sum: ID3D12Resource,
    arguments: ID3D12Resource,
    scratch: ID3D12Resource,
    budget: ID3D12Resource,
    shared: (ID3D12Resource, *mut u8),
    constants: (ID3D12Resource, *mut u8),
    texels: (ID3D12Resource, *mut u8),
    back: (ID3D12Resource, *mut u8),
    swap: Option<(IDXGISwapChain3, HWND)>,
}

impl Scene {
    fn new(device: &ID3D12Device, window: bool) -> Scene {
        // SAFETY: plain creation throughout.
        unsafe {
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .unwrap();
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
            let lists = (0..3)
                .map(|_| {
                    let allocator: ID3D12CommandAllocator = device
                        .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                        .unwrap();
                    let list: ID3D12GraphicsCommandList = device
                        .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                        .unwrap();
                    list.Close().unwrap();
                    (allocator, list)
                })
                .collect();
            let (vs, ps) = (
                compile(DRAW, s!("vs"), s!("vs_5_1")),
                compile(DRAW, s!("ps"), s!("ps_5_1")),
            );
            let root: ID3D12RootSignature = device.CreateRootSignature(0, &vs).unwrap();
            let mut blend = D3D12_BLEND_DESC::default();
            blend.RenderTarget[0].RenderTargetWriteMask = 15;
            let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: code(&vs),
                PS: code(&ps),
                BlendState: blend,
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID,
                    CullMode: D3D12_CULL_MODE_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                    DepthEnable: true.into(),
                    DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                    DepthFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
                    ..Default::default()
                },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                DSVFormat: DXGI_FORMAT_D32_FLOAT,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                ..Default::default()
            };
            desc.RTVFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
            let pso: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc).unwrap();
            record::name(&pso, "a drawing pipeline");
            let draw = Made { root, pso };
            let (vs, ps) = (
                compile(PLAIN, s!("vs"), s!("vs_5_1")),
                compile(PLAIN, s!("ps"), s!("ps_5_1")),
            );
            let root: ID3D12RootSignature = device.CreateRootSignature(0, &vs).unwrap();
            let element = |name, index, format, at| D3D12_INPUT_ELEMENT_DESC {
                SemanticName: name,
                SemanticIndex: index,
                Format: format,
                InputSlot: 0,
                AlignedByteOffset: at,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            };
            let elements = [
                element(s!("POSITION"), 0, DXGI_FORMAT_R32G32_FLOAT, 0),
                element(s!("COLOR"), 1, DXGI_FORMAT_R32G32B32_FLOAT, 8),
            ];
            desc.pRootSignature = std::mem::transmute_copy(&root);
            (desc.VS, desc.PS) = (code(&vs), code(&ps));
            desc.InputLayout = D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: elements.as_ptr(),
                NumElements: 2,
            };
            let pso: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc).unwrap();
            let plain = Made { root, pso };
            let clamp = compute(device, CLAMP);
            record::name(&clamp.pso, play::CLAMP);
            let argument = D3D12_INDIRECT_ARGUMENT_DESC {
                Type: D3D12_INDIRECT_ARGUMENT_TYPE_DRAW,
                ..Default::default()
            };
            let mut signature: Option<ID3D12CommandSignature> = None;
            let desc = D3D12_COMMAND_SIGNATURE_DESC {
                ByteStride: 16,
                NumArgumentDescs: 1,
                pArgumentDescs: &argument,
                NodeMask: 0,
            };
            device
                .CreateCommandSignature(&desc, None::<&ID3D12RootSignature>, &mut signature)
                .unwrap();
            let heap = |kind, count, flags| -> ID3D12DescriptorHeap {
                device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: kind,
                        NumDescriptors: count,
                        Flags: flags,
                        NodeMask: 0,
                    })
                    .unwrap()
            };
            let samplers = heap(
                D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
                4,
                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            );
            let sampler = D3D12_SAMPLER_DESC {
                Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                AddressW: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                MaxLOD: 8.0,
                ..Default::default()
            };
            let mut at = samplers.GetCPUDescriptorHandleForHeapStart();
            at.ptr += 2 * device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER)
                as usize;
            device.CreateSampler(&sampler, at);
            let views = heap(
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                5000,
                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            );
            let rtvs = heap(
                D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                1,
                D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            );
            let dsvs = heap(
                D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                1,
                D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            );
            let rgba = DXGI_FORMAT_R8G8B8A8_UNORM;
            let (target, _) = committed(
                device,
                In::Card,
                &texture_desc(SIZE, rgba, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET),
                FROM,
            );
            let (depth, _) = committed(
                device,
                In::Card,
                &texture_desc(
                    SIZE,
                    DXGI_FORMAT_D32_FLOAT,
                    D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
                ),
                D3D12_RESOURCE_STATE_DEPTH_WRITE,
            );
            let (fixed, _) = committed(
                device,
                In::Card,
                &texture_desc(16, rgba, D3D12_RESOURCE_FLAG_NONE),
                TO,
            );
            let (image, _) = committed(
                device,
                In::Card,
                &texture_desc(16, rgba, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS),
                UAV,
            );
            let mut memory: Option<ID3D12Heap> = None;
            let desc = D3D12_HEAP_DESC {
                SizeInBytes: 4 << 20,
                Properties: D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                Alignment: 0,
                Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
            };
            device.CreateHeap(&desc, &mut memory).unwrap();
            let memory = memory.unwrap();
            let placed = || {
                let mut made: Option<ID3D12Resource> = None;
                let d = texture_desc(16, rgba, D3D12_RESOURCE_FLAG_NONE);
                device
                    .CreatePlacedResource(&memory, 65536, &d, TO, None, &mut made)
                    .unwrap();
                made.unwrap()
            };
            // Two textures in one place.
            let (one, other) = (placed(), placed());
            let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
            let card = |size| committed(device, In::Card, &buffer_desc(size, uav), COMMON).0;
            let (sum, arguments, scratch, budget) = (card(1024), card(4096), card(4096), card(256));
            let shared = committed(device, In::Shared, &buffer_desc(70_000, uav), COMMON);
            let constants = committed(
                device,
                In::Upload,
                &buffer_desc(8192, D3D12_RESOURCE_FLAG_NONE),
                D3D12_RESOURCE_STATE_GENERIC_READ,
            );
            let texels = committed(
                device,
                In::Upload,
                &buffer_desc(65536, D3D12_RESOURCE_FLAG_NONE),
                D3D12_RESOURCE_STATE_GENERIC_READ,
            );
            let back = committed(
                device,
                In::Back,
                &buffer_desc(1 << 20, D3D12_RESOURCE_FLAG_NONE),
                TO,
            );
            let vertices = committed(
                device,
                In::Upload,
                &buffer_desc(4096, D3D12_RESOURCE_FLAG_NONE),
                D3D12_RESOURCE_STATE_GENERIC_READ,
            );
            let still = {
                let blocks = D3D12_RESOURCE_DESC {
                    MipLevels: 5,
                    DepthOrArraySize: 3,
                    ..texture_desc(64, DXGI_FORMAT_BC7_UNORM, D3D12_RESOURCE_FLAG_NONE)
                };
                let volume = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE3D,
                    DepthOrArraySize: 8,
                    MipLevels: 3,
                    ..texture_desc(16, DXGI_FORMAT_R16_FLOAT, D3D12_RESOURCE_FLAG_NONE)
                };
                let stencil = texture_desc(
                    32,
                    DXGI_FORMAT_R32G8X24_TYPELESS,
                    D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
                );
                let (write, read) = (
                    D3D12_RESOURCE_STATE_DEPTH_WRITE,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                );
                [
                    ("blocks", blocks, TO, READ),
                    ("volume", volume, TO, read),
                    ("stencil", stencil, write, write),
                ]
                .map(|(name, d, made_in, ends)| {
                    let (t, _) = committed(device, In::Card, &d, made_in);
                    record::name(&t, name);
                    (name, t, ends)
                })
            };
            let copies: (
                ID3D12CommandQueue,
                ID3D12CommandAllocator,
                ID3D12GraphicsCommandList,
            ) = {
                let queue = device
                    .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                        Type: D3D12_COMMAND_LIST_TYPE_COPY,
                        ..Default::default()
                    })
                    .unwrap();
                let allocator: ID3D12CommandAllocator = device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)
                    .unwrap();
                let list: ID3D12GraphicsCommandList = device
                    .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)
                    .unwrap();
                list.Close().unwrap();
                (queue, allocator, list)
            };
            for (r, n) in [
                (&sum, "sum"),
                (&image, "image"),
                (&target, "target"),
                (&shared.0, "shared"),
                (&one, "one"),
                (&other, "other"),
            ] {
                record::name(r, n);
            }
            device.CreateRenderTargetView(&target, None, rtvs.GetCPUDescriptorHandleForHeapStart());
            device.CreateDepthStencilView(&depth, None, dsvs.GetCPUDescriptorHandleForHeapStart());
            let step = device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                as usize;
            let swap = window.then(|| swap_chain(&queue));
            let scene = Scene {
                device: device.clone(),
                queue,
                fence,
                event: CreateEventW(None, false, false, None).unwrap(),
                value: 0,
                lists,
                draw,
                plain,
                mesh: mesh(device, blend),
                vertices,
                samplers,
                still: still.to_vec(),
                copies,
                args: compute(device, ARGS),
                clamp,
                add: compute(device, ADD),
                signature: signature.unwrap(),
                views,
                step,
                rtvs,
                dsvs,
                target,
                _depth: depth,
                fixed,
                one,
                other,
                _heap: memory,
                image,
                sum,
                arguments,
                scratch,
                budget,
                shared,
                constants,
                texels,
                back,
                swap,
            };
            // Null views everywhere, by copies of a slot made elsewhere.
            let block = heap(
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                16,
                D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            );
            let null = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: rgba,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            let from = block.GetCPUDescriptorHandleForHeapStart();
            for i in 0..16 {
                let at = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: from.ptr + i * step,
                };
                device.CreateShaderResourceView(None::<&ID3D12Resource>, Some(&null), at);
            }
            for i in (0..4992).step_by(16) {
                device.CopyDescriptorsSimple(
                    16,
                    scene.cpu(i),
                    from,
                    D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                );
            }
            // The rest with the other copying method, in unlike pieces.
            let to = [scene.cpu(4992), scene.cpu(4994)];
            let sources = [
                from,
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: from.ptr + 4 * step,
                },
            ];
            device.CopyDescriptors(
                2,
                to.as_ptr(),
                Some([2u32, 6].as_ptr()),
                2,
                sources.as_ptr(),
                Some([4u32, 4].as_ptr()),
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            );
            scene.show(4001, &scene.other);
            scene.show(4002, &scene.one);
            device.CreateUnorderedAccessView(
                &scene.image,
                None::<&ID3D12Resource>,
                None,
                scene.cpu(7),
            );
            scene
        }
    }

    fn cpu(&self, i: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: a slot of the heap.
        let start = unsafe { self.views.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: start.ptr + i * self.step,
        }
    }

    /// Slot `i` shows `texture`.
    fn show(&self, i: usize, texture: &ID3D12Resource) {
        // SAFETY: a slot of the heap and a live texture.
        unsafe {
            self.device
                .CreateShaderResourceView(texture, None, self.cpu(i))
        };
    }

    fn wait(&mut self) {
        self.value += 1;
        // SAFETY: our queue, fence and event.
        unsafe {
            self.queue.Signal(&self.fence, self.value).unwrap();
            self.fence
                .SetEventOnCompletion(self.value, self.event)
                .unwrap();
            WaitForSingleObject(self.event, INFINITE);
            self.device.GetDeviceRemovedReason().unwrap();
        }
    }

    fn open(&self, i: usize) -> ID3D12GraphicsCommandList {
        let (allocator, list) = &self.lists[i];
        // SAFETY: its last run was waited for.
        unsafe {
            allocator.Reset().unwrap();
            list.Reset(allocator, None).unwrap();
        }
        list.clone()
    }

    fn send(&self, lists: &[&ID3D12GraphicsCommandList]) {
        let all: Vec<Option<ID3D12CommandList>> =
            lists.iter().map(|l| Some(l.cast().unwrap())).collect();
        // SAFETY: closed lists.
        unsafe { self.queue.ExecuteCommandLists(&all) };
    }

    /// Fills rows of a 16x16 texture in the upload buffer at `at`.
    fn fill(&self, at: usize, color: [u8; 4]) {
        for y in 0..16 {
            for x in 0..16 {
                let mut c = color;
                c[1] = c[1].wrapping_add((x * 8 + y) as u8);
                // SAFETY: inside the mapping (16 rows of 256 bytes).
                unsafe {
                    self.texels
                        .1
                        .add(at + y * 256 + x * 4)
                        .cast::<[u8; 4]>()
                        .write(c)
                };
            }
        }
    }

    fn texture_copy(&self, l: &ID3D12GraphicsCommandList, texture: &ID3D12Resource, at: u64) {
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: a borrowed pointer never released.
            pResource: unsafe { std::mem::transmute_copy(&self.texels.0) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: footprint(at, 16),
            },
        };
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above.
            pResource: unsafe { std::mem::transmute_copy(texture) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        // SAFETY: an open list; live resources.
        unsafe { l.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
    }

    /// A quad of the texture in slot `pick`, at the constants at `at`.
    fn quad(&self, l: &ID3D12GraphicsCommandList, pick: u32, at: u64, record: u64) {
        // SAFETY: an open list; live objects.
        unsafe {
            l.SetGraphicsRootSignature(&self.draw.root);
            l.SetPipelineState(&self.draw.pso);
            l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            l.SetGraphicsRoot32BitConstant(0, pick, 0);
            l.SetGraphicsRootConstantBufferView(1, self.constants.0.GetGPUVirtualAddress() + at);
            l.SetGraphicsRootDescriptorTable(2, self.views.GetGPUDescriptorHandleForHeapStart());
            let step = self
                .device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER);
            let first = self.samplers.GetGPUDescriptorHandleForHeapStart().ptr;
            l.SetGraphicsRootDescriptorTable(
                3,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: first + 2 * u64::from(step),
                },
            );
            l.ExecuteIndirect(
                &self.signature,
                1,
                &self.scratch,
                record * 16,
                None::<&ID3D12Resource>,
                0,
            );
        }
    }

    /// The arguments made and clamped, as the host layer does it.
    fn arguments(&self, l: &ID3D12GraphicsCommandList, before: D3D12_RESOURCE_STATES) {
        // SAFETY: an open list; live objects.
        unsafe {
            l.SetComputeRootSignature(&self.args.root);
            l.SetPipelineState(&self.args.pso);
            l.SetComputeRootShaderResourceView(0, self.shared.0.GetGPUVirtualAddress() + 65536);
            l.SetComputeRootUnorderedAccessView(1, self.arguments.GetGPUVirtualAddress());
            l.Dispatch(1, 1, 1);
            l.ResourceBarrier(&[
                uav_barrier(),
                moved(
                    &self.arguments,
                    UAV,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                ),
            ]);
            l.ResourceBarrier(&[uav_barrier(), moved(&self.scratch, before, UAV)]);
            l.SetComputeRootSignature(&self.clamp.root);
            l.SetPipelineState(&self.clamp.pso);
            let constants = [0u32, 16, 2, 0, 0, 0];
            l.SetComputeRoot32BitConstants(0, 6, constants.as_ptr().cast(), 0);
            l.SetComputeRootShaderResourceView(1, self.arguments.GetGPUVirtualAddress());
            l.SetComputeRootShaderResourceView(2, self.arguments.GetGPUVirtualAddress());
            l.SetComputeRootUnorderedAccessView(3, self.scratch.GetGPUVirtualAddress());
            l.SetComputeRootUnorderedAccessView(4, self.budget.GetGPUVirtualAddress());
            l.Dispatch(1, 1, 1);
            l.ResourceBarrier(&[uav_barrier(), moved(&self.scratch, UAV, ARGUMENTS)]);
            l.ResourceBarrier(&[moved(
                &self.arguments,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                UAV,
            )]);
        }
    }

    fn place(&self, at: usize, place: [f32; 4]) {
        // SAFETY: inside the mapping.
        unsafe { self.constants.1.add(at).cast::<[f32; 4]>().write(place) };
    }

    fn give(&self, at: usize, value: u32) {
        // SAFETY: inside the mapping.
        unsafe { self.shared.1.add(at).cast::<u32>().write(value) };
    }

    fn targets(&self, l: &ID3D12GraphicsCommandList) {
        // SAFETY: an open list; live objects.
        unsafe {
            let (rtv, dsv) = (
                self.rtvs.GetCPUDescriptorHandleForHeapStart(),
                self.dsvs.GetCPUDescriptorHandleForHeapStart(),
            );
            l.OMSetRenderTargets(1, Some(&rtv), false, Some(&dsv));
            let size = SIZE as f32;
            l.RSSetViewports(&[D3D12_VIEWPORT {
                Width: size,
                Height: size,
                MaxDepth: 1.0,
                ..Default::default()
            }]);
            l.RSSetScissorRects(&[RECT {
                left: 0,
                top: 0,
                right: SIZE as i32,
                bottom: SIZE as i32,
            }]);
            l.SetDescriptorHeaps(&[Some(self.views.clone()), Some(self.samplers.clone())]);
        }
    }

    /// A triangle with vertices and indices.
    fn triangle(&self, l: &ID3D12GraphicsCommandList, n: u32) {
        // SAFETY: an open list; live objects; inside the mapping, which the
        // card does not use now.
        unsafe {
            let x = 0.5 + 0.05 * n as f32;
            let vertices = [
                [x, 0.4, 1.0, 0.0, 0.0],
                [x + 0.3, 0.4, 0.0, 1.0, 0.0],
                [x, 0.9, 0.0, 0.0, 1.0f32],
            ];
            self.vertices.1.cast::<[[f32; 5]; 3]>().write(vertices);
            self.vertices
                .1
                .add(1024)
                .cast::<[u16; 4]>()
                .write([9, 2, 0, 1]);
            let at = self.vertices.0.GetGPUVirtualAddress();
            l.SetGraphicsRootSignature(&self.plain.root);
            l.SetPipelineState(&self.plain.pso);
            l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            l.IASetVertexBuffers(
                0,
                Some(&[D3D12_VERTEX_BUFFER_VIEW {
                    BufferLocation: at,
                    SizeInBytes: 60,
                    StrideInBytes: 20,
                }]),
            );
            let indices = D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: at + 1024,
                SizeInBytes: 8,
                Format: DXGI_FORMAT_R16_UINT,
            };
            l.IASetIndexBuffer(Some(&indices));
            l.DrawIndexedInstanced(3, 1, 1, 0, 0);
        }
    }

    /// Fills the textures no frame touches.
    fn fill_still(&mut self) {
        // SAFETY: an open list; live objects; mappings written inside their
        // bounds.
        unsafe {
            let (up, data) = committed(
                &self.device,
                In::Upload,
                &buffer_desc(1 << 20, D3D12_RESOURCE_FLAG_NONE),
                D3D12_RESOURCE_STATE_GENERIC_READ,
            );
            for i in 0..1 << 20 {
                data.add(i).write((i * 31 + i / 251) as u8 & 0x3b);
            }
            let l = self.open(0);
            let mut at = 0;
            for (name, t, ends) in &self.still {
                if *name == "stencil" {
                    let view = D3D12_DEPTH_STENCIL_VIEW_DESC {
                        Format: DXGI_FORMAT_D32_FLOAT_S8X24_UINT,
                        ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
                        ..Default::default()
                    };
                    let heap: ID3D12DescriptorHeap = self
                        .device
                        .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                            Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                            NumDescriptors: 1,
                            ..Default::default()
                        })
                        .unwrap();
                    let dsv = heap.GetCPUDescriptorHandleForHeapStart();
                    self.device.CreateDepthStencilView(t, Some(&view), dsv);
                    let both = D3D12_CLEAR_FLAG_DEPTH | D3D12_CLEAR_FLAG_STENCIL;
                    l.ClearDepthStencilView(dsv, both, 0.75, 3, None);
                    let part = RECT {
                        left: 5,
                        top: 6,
                        right: 20,
                        bottom: 17,
                    };
                    l.ClearDepthStencilView(dsv, both, 0.25, 200, Some(&[part]));
                    continue;
                }
                let desc = t.GetDesc();
                let layers = if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D {
                    1
                } else {
                    desc.DepthOrArraySize
                };
                for sub in 0..u32::from(layers * desc.MipLevels) {
                    let mut f = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
                    let mut total = 0;
                    self.device.GetCopyableFootprints(
                        &desc,
                        sub,
                        1,
                        at,
                        Some(&mut f),
                        None,
                        None,
                        Some(&mut total),
                    );
                    at = (at + total).next_multiple_of(512);
                    let from = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&up),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: f },
                    };
                    l.CopyTextureRegion(&sub_of(t, sub), 0, 0, 0, &from, None);
                }
                l.ResourceBarrier(&[moved(t, TO, *ends)]);
            }
            l.Close().unwrap();
            self.send(&[&l]);
            self.wait();
        }
    }

    /// What they hold.
    fn read_still(&mut self) -> Vec<Still> {
        let mut all = Vec::new();
        for (name, t, state) in self.still.clone() {
            // SAFETY: as above.
            unsafe {
                let desc = t.GetDesc();
                let layers = if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D {
                    1
                } else {
                    desc.DepthOrArraySize
                };
                let planes = if name == "stencil" { 2 } else { 1 };
                let mut subs = Vec::new();
                for sub in 0..u32::from(layers * desc.MipLevels) * planes {
                    let mut f = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
                    let (mut rows, mut len) = (0, 0);
                    self.device.GetCopyableFootprints(
                        &desc,
                        sub,
                        1,
                        0,
                        Some(&mut f),
                        Some(&mut rows),
                        Some(&mut len),
                        None,
                    );
                    let l = self.open(0);
                    l.ResourceBarrier(&[moved(&t, state, FROM)]);
                    let to = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&self.back.0),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: f },
                    };
                    l.CopyTextureRegion(&to, 0, 0, 0, &sub_of(&t, sub), None);
                    l.ResourceBarrier(&[moved(&t, FROM, state)]);
                    l.Close().unwrap();
                    self.send(&[&l]);
                    self.wait();
                    let pitch = f.Footprint.RowPitch as usize;
                    let rows = (0..rows as usize * f.Footprint.Depth as usize)
                        .map(|r| {
                            std::slice::from_raw_parts(self.back.1.add(r * pitch), len as usize)
                                .to_vec()
                        })
                        .collect();
                    subs.push((pitch, rows));
                }
                all.push(Still { name, subs });
            }
        }
        all
    }

    fn frame(&mut self, n: u32) {
        // SAFETY: open lists of ours, live objects; memory is written while
        // the card does not use it.
        unsafe {
            self.wait();
            if n == 0 {
                self.fill_still();
                self.fill(0, [200, 10, 30, 255]);
                self.fill(8192, [10, 40, 220, 255]);
                let l = self.open(0);
                self.texture_copy(&l, &self.fixed, 0);
                self.texture_copy(&l, &self.other, 8192);
                l.ResourceBarrier(&[moved(&self.fixed, TO, READ), moved(&self.other, TO, READ)]);
                l.Close().unwrap();
                self.send(&[&l]);
                self.wait();
            }
            // The first submission.
            self.show(4000, &self.fixed);
            self.place(0, [-1.0 + 0.1 * n as f32, -1.0, 0.5, 0.5]);
            self.place(256, [0.0, 0.0, 0.5, 0.5]);
            self.give(65536, 6);
            self.give(40, 7 + n);
            let l = self.open(0);
            l.ResourceBarrier(&[moved(&self.target, FROM, TARGET)]);
            let (rtv, dsv) = (
                self.rtvs.GetCPUDescriptorHandleForHeapStart(),
                self.dsvs.GetCPUDescriptorHandleForHeapStart(),
            );
            l.ClearRenderTargetView(rtv, &[0.1 * n as f32, 0.2, 0.3, 1.0], None);
            l.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 1.0, 0, None);
            self.arguments(&l, COMMON);
            self.targets(&l);
            self.quad(&l, 4000, 0, 0);
            self.quad(&l, 4001, 256, 0);
            self.triangle(&l, n);
            // Nothing: the clamp cut it to no vertices.
            self.quad(&l, 4001, 0, 1);
            l.ResourceBarrier(&[moved(&self.scratch, ARGUMENTS, COMMON)]);
            l.Close().unwrap();
            self.send(&[&l]);
            self.wait();
            // The second: other constants and another texture in the same
            // places, and the second texture in the memory of the first.
            self.show(4000, &self.other);
            // Arguments of a mesh draw.
            self.constants
                .1
                .add(1024)
                .cast::<[u32; 3]>()
                .write([1, 1, 1]);
            self.place(0, [-0.9, 0.3, 0.6, 0.6]);
            self.place(256, [0.3, -0.8, 0.6, 0.5]);
            self.give(40, 100 + n);
            self.give(65536, 3);
            self.fill(16384, [30, 200, 10 + 5 * n as u8, 255]);
            let l = self.open(1);
            self.targets(&l);
            self.arguments(&l, COMMON);
            self.quad(&l, 4000, 0, 0);
            l.ResourceBarrier(&[moved(&self.other, READ, TO)]);
            l.ResourceBarrier(&[aliased(&self.other, &self.one)]);
            self.texture_copy(&l, &self.one, 16384);
            l.ResourceBarrier(&[moved(&self.one, TO, READ)]);
            self.arguments(&l, ARGUMENTS);
            self.quad(&l, 4002, 256, 0);
            if let Some((made, signature)) = &self.mesh {
                l.SetGraphicsRootSignature(&made.root);
                l.SetPipelineState(&made.pso);
                let constants = [
                    (-0.3f32).to_bits(),
                    (-0.9f32).to_bits(),
                    0.5f32.to_bits(),
                    0.4f32.to_bits(),
                    4002,
                ];
                l.SetGraphicsRoot32BitConstants(0, 5, constants.as_ptr().cast(), 0);
                l.ExecuteIndirect(
                    signature,
                    1,
                    &self.constants.0,
                    1024,
                    None::<&ID3D12Resource>,
                    0,
                );
            }
            l.ResourceBarrier(&[moved(&self.scratch, ARGUMENTS, COMMON)]);
            // And back, for the next frame.
            l.ResourceBarrier(&[moved(&self.one, READ, TO)]);
            l.ResourceBarrier(&[aliased(&self.one, &self.other)]);
            self.texture_copy(&l, &self.other, 8192);
            l.ResourceBarrier(&[moved(&self.other, TO, READ)]);
            l.Close().unwrap();
            let second = l;
            let l = self.open(2);
            l.SetDescriptorHeaps(&[Some(self.views.clone())]);
            l.SetComputeRootSignature(&self.add.root);
            l.SetPipelineState(&self.add.pso);
            l.SetComputeRootShaderResourceView(0, self.shared.0.GetGPUVirtualAddress());
            let table = self.views.GetGPUDescriptorHandleForHeapStart().ptr + 7 * self.step as u64;
            l.SetComputeRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: table });
            l.SetComputeRootUnorderedAccessView(2, self.sum.GetGPUVirtualAddress());
            for _ in 0..3 {
                l.Dispatch(2, 2, 1);
                l.ResourceBarrier(&[uav_barrier()]);
            }
            // The card writes memory the CPU sees.
            l.ResourceBarrier(&[
                moved(&self.sum, UAV, FROM),
                moved(&self.shared.0, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, TO),
            ]);
            l.CopyBufferRegion(&self.shared.0, 30_000, &self.sum, 0, 1024);
            l.ResourceBarrier(&[moved(&self.target, TARGET, FROM)]);
            l.Close().unwrap();
            self.send(&[&second, &l]);
            self.wait();
            // Another queue fetches a result, as the host layer's relay does.
            let (queue, allocator, list) = &self.copies;
            allocator.Reset().unwrap();
            list.Reset(allocator, None).unwrap();
            list.CopyBufferRegion(&self.back.0, 1 << 18, &self.sum, 0, 1024);
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
            self.value += 1;
            queue.Signal(&self.fence, self.value).unwrap();
            self.fence
                .SetEventOnCompletion(self.value, self.event)
                .unwrap();
            WaitForSingleObject(self.event, INFINITE);
            if let Some((swap, _)) = &self.swap {
                swap.Present(0, DXGI_PRESENT(0)).unwrap();
            }
            record::present(&self.target, FROM);
        }
    }

    /// What everything holds now.
    fn outcome(&mut self) -> Outcome {
        self.wait();
        let still = self.read_still();
        let l = self.open(0);
        // SAFETY: an open list; live objects.
        unsafe {
            let to = |at: u64, size| D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&self.back.0),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: footprint(at, size),
                },
            };
            let of = |t: &ID3D12Resource| D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(t),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            l.ResourceBarrier(&[moved(&self.image, UAV, FROM)]);
            l.CopyTextureRegion(&to(0, SIZE), 0, 0, 0, &of(&self.target), None);
            l.CopyTextureRegion(&to(1 << 18, 16), 0, 0, 0, &of(&self.image), None);
            l.ResourceBarrier(&[moved(&self.image, FROM, UAV)]);
            l.CopyBufferRegion(&self.back.0, 1 << 19, &self.sum, 0, 1024);
            l.Close().unwrap();
            self.send(&[&l]);
            self.wait();
            let at = |from: usize, n: usize| {
                std::slice::from_raw_parts(self.back.1.add(from), n).to_vec()
            };
            Outcome {
                still,
                picture: at(0, (SIZE * 512) as usize),
                image: rows(&at(1 << 18, 4096)),
                sum: at(1 << 19, 1024),
                shared: std::slice::from_raw_parts(self.shared.1, 70_000).to_vec(),
            }
        }
    }
}

fn sub_of(t: &ID3D12Resource, sub: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer never released.
        pResource: unsafe { std::mem::transmute_copy(t) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: sub,
        },
    }
}

/// The 64 bytes of each row of a 16x16 texture.
fn rows(bytes: &[u8]) -> Vec<u8> {
    bytes.chunks(256).flat_map(|r| r[..64].to_vec()).collect()
}

fn footprint(at: u64, size: u32) -> D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
        Offset: at,
        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Width: size,
            Height: size,
            Depth: 1,
            RowPitch: (size * 4).next_multiple_of(256),
        },
    }
}

/// A swap chain of a window that is never shown.
fn swap_chain(queue: &ID3D12CommandQueue) -> (IDXGISwapChain3, HWND) {
    // SAFETY: plain creation.
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("replay test"),
            WS_POPUP,
            0,
            0,
            64,
            64,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 64,
            Height: 64,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        let chain = factory
            .CreateSwapChainForHwnd(queue, hwnd, &desc, None, None)
            .unwrap()
            .cast()
            .unwrap();
        (chain, hwnd)
    }
}

fn table_of(device: &ID3D12Device) -> usize {
    // SAFETY: a live COM object starts with its table's address.
    unsafe { *device.as_raw().cast::<usize>() }
}

fn check(player: &mut Player, options: &Options, expected: &Outcome, what: &str) -> play::Report {
    let report = player.run(options).unwrap();
    println!("{}", report.text(what));
    // Combining original submissions removes implicit resource state decay.
    // Track that separate experiment's existing barrier mismatch separately.
    let errors: Vec<_> = report.notes.iter().filter(|note| {
        note.starts_with("debug layer:")
            && !(options.one_submit && note.contains("errors 527, first:"))
    }).collect();
    assert!(errors.is_empty(), "{what}: D3D12 validation errors: {errors:?}");
    assert_eq!(report.frame_ms.len(), options.loops as usize);
    assert!(report.median_ms() > 0.0);
    let (db, same) = report.picture.unwrap();
    assert!(
        same && db.is_infinite(),
        "{what}: the picture differs ({db} dB)"
    );
    let mut get = |name: &str| {
        let id = player.named(name).unwrap();
        player.contents(id).unwrap().unwrap()
    };
    assert_eq!(get("sum"), expected.sum, "{what}: sum");
    assert_eq!(rows(&get("image")), expected.image, "{what}: image");
    assert_eq!(get("target"), expected.picture, "{what}: target");
    assert!(
        get("shared") == expected.shared,
        "{what}: memory the CPU sees"
    );
    for still in &expected.still {
        let id = player.named(still.name).unwrap();
        for (sub, (pitch, rows)) in still.subs.iter().enumerate() {
            let have = player.contents_of(id, sub as u32).unwrap().unwrap();
            for (r, row) in rows.iter().enumerate() {
                assert_eq!(
                    &have[r * pitch..][..row.len()],
                    &row[..],
                    "{what}: {} subresource {sub} row {r}",
                    still.name
                );
            }
        }
    }
    // Nothing was left out but what another queue ran.
    let known = [
        "submissions,",
        "waits of the drawing queue",
        "lists sent to a queue other than",
    ];
    assert!(
        !report
            .notes
            .iter()
            .any(|n| n.contains("recorder:") && !known.iter().any(|k| n.contains(k))),
        "{what}: {:?}",
        report.notes
    );
    assert!(report.notes.iter().any(|n| n.contains(known[2])));
    report
}

#[test]
fn a_frame_is_recorded_and_replayed() {
    let debug = std::env::var_os("VICTORY_TEST_D3D_DEBUG").is_some();
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: plain creation.
    unsafe {
        if debug {
            let mut layer: Option<ID3D12Debug> = None;
            D3D12GetDebugInterface(&mut layer).unwrap();
            layer.unwrap().EnableDebugLayer();
        }
        let _ = D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device);
    }
    let Some(device) = device else {
        eprintln!("no adapter: skipped");
        return;
    };
    let own = table_of(&device);
    // Without the recorder.
    let without = {
        let mut scene = Scene::new(&device, false);
        (0..5).for_each(|n| scene.frame(n));
        scene.outcome()
    };
    eprintln!("ran without the recorder");
    // Something is in the picture.
    assert!(without.sum.iter().any(|&b| b != 0));
    for s in &without.still {
        assert!(
            s.subs
                .iter()
                .all(|(_, rows)| rows.iter().flatten().any(|&b| b != 0)),
            "{}",
            s.name
        );
    }
    assert_eq!(
        without
            .still
            .iter()
            .map(|s| s.subs.len())
            .collect::<Vec<_>>(),
        [15, 3, 2]
    );
    let colors: std::collections::HashSet<[u8; 4]> = without
        .picture
        .chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect();
    assert!(colors.len() > 30, "{}", colors.len());

    // With it: armed at the first present, the fourth frame is captured (a
    // frame whose lists are all recorded comes first).
    assert!(!record::on());
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings {
        folder: dir.path().to_owned(),
        at: At::Frame(1),
        frames: 1,
        always: false,
    };
    let folder = record::start(settings, &device).unwrap();
    assert!(record::on());
    assert_ne!(table_of(&device), own, "the device is hooked");
    assert!(
        record::start(
            Settings {
                folder: dir.path().to_owned(),
                at: At::Trigger,
                frames: 1,
                always: false
            },
            &device
        )
        .is_err(),
        "one recorder a process"
    );
    eprintln!("recorder on");
    let mut scene = Scene::new(&device, true);
    eprintln!("scene made");
    (0..4).for_each(|n| scene.frame(n));
    assert_eq!(
        record::finished(),
        Some(folder.clone()),
        "see {}",
        folder.display()
    );
    let expected = scene.outcome();
    // It goes on as it did without the recorder.
    scene.frame(4);
    let after = scene.outcome();
    assert!(
        after == without,
        "the recorder changed what the program makes"
    );
    drop(scene);
    record::unhook(&device);
    assert_eq!(table_of(&device), own, "the device is as it was");
    if debug {
        let q = device.cast::<ID3D12InfoQueue>().unwrap();
        let mut errors = Vec::new();
        // SAFETY: plain queries; messages read into buffers of their size.
        unsafe {
            for i in 0..q.GetNumStoredMessages() {
                let mut size = 0;
                let _ = q.GetMessage(i, None, &mut size);
                let mut buffer = vec![0u64; size.div_ceil(8)];
                let m = buffer.as_mut_ptr().cast::<D3D12_MESSAGE>();
                if q.GetMessage(i, Some(m), &mut size).is_ok()
                    && (*m).Severity.0 <= D3D12_MESSAGE_SEVERITY_WARNING.0
                {
                    let text =
                        std::slice::from_raw_parts((*m).pDescription, (*m).DescriptionByteLength);
                    if (*m).Severity.0 <= D3D12_MESSAGE_SEVERITY_ERROR.0 {
                        errors.push((*m).ID.0);
                    }
                    println!(
                        "D3D12 {} {}: {}",
                        (*m).Severity.0,
                        (*m).ID.0,
                        String::from_utf8_lossy(text)
                    );
                }
            }
        }
        assert!(errors.is_empty(), "source frame D3D12 validation errors: {errors:?}");
    }
    for name in [
        "frame.bin",
        "contents.bin",
        "deltas.bin",
        "shaders.bin",
        "present.bin",
        "notes.txt",
        "frame-0.png",
    ] {
        assert!(folder.join(name).exists(), "{name}");
    }
    println!(
        "{}",
        std::fs::read_to_string(folder.join("notes.txt")).unwrap()
    );

    let plain = Options {
        loops: 3,
        ..Default::default()
    };
    let mut player = Player::load(dir.path(), &plain).unwrap();
    check(&mut player, &plain, &expected, "unchanged");
    let detailed = Options {
        detailed: true,
        ..plain.clone()
    };
    let report = check(&mut player, &detailed, &expected, "detailed");
    let count = |kind: &str| {
        report
            .categories
            .iter()
            .find(|c| c.0 == kind)
            .map_or(0, |c| c.2)
    };
    assert_eq!(count("argument clamps (ours)"), 3);
    assert_eq!(count("indirect draws"), 5);
    assert_eq!(count("draws"), 1);
    let meshes = count("indirect mesh draws");
    println!("{meshes} mesh draws");
    if meshes > 0 {
        assert!(
            report
                .shaders
                .iter()
                .any(|s| s.2.starts_with("a mesh pipeline (DXIL ")),
            "{:?}",
            report.shaders
        );
    }
    assert_eq!(count("dispatches"), 6);
    assert_eq!(count("clears"), 2);
    assert!(
        report
            .shaders
            .iter()
            .any(|s| s.2.starts_with("a drawing pipeline (DXIL ")),
        "{:?}",
        report.shaders
    );
    let merged = Options {
        merge_barriers: true,
        ..plain.clone()
    };
    check(&mut player, &merged, &expected, "merged barriers");
    let clamps = Options {
        skip_clamps: true,
        ..plain.clone()
    };
    let report = check(&mut player, &clamps, &expected, "without clamps");
    assert!(
        report.notes.iter().any(|n| n.contains("3 clamp passes")),
        "{:?}",
        report.notes
    );
    let one = Options {
        one_submit: true,
        ..plain.clone()
    };
    check(&mut player, &one, &expected, "one submission");
    let hash = Options {
        skip_hash_passes: true,
        ..plain.clone()
    };
    check(
        &mut player,
        &hash,
        &expected,
        "without hash passes (there are none)",
    );
    // These change what is drawn: they run, and then the frame is as before.
    let no_barriers = Options {
        no_barriers: true,
        ..plain.clone()
    };
    player.run(&no_barriers).unwrap();
    let some = Options {
        only: Some((0, 1)),
        ..plain.clone()
    };
    player.run(&some).unwrap();
    let skip = Options {
        skip_shaders: vec!["a drawing".into()],
        ..plain.clone()
    };
    let report = player.run(&skip).unwrap();
    assert!(
        !report.picture.unwrap().1,
        "without its draws the picture is another"
    );
    assert!(
        report.notes.iter().any(|n| n.contains("1 pipelines match"))
            && report
                .notes
                .iter()
                .any(|n| n.contains("left out 5 commands"))
    );
    let kept = Options {
        restore: false,
        ..plain.clone()
    };
    player.run(&kept).unwrap();
    check(&mut player, &plain, &expected, "unchanged again");
    drop(player);
    for buffers in [Buffers::Card, Buffers::Pc] {
        let o = Options {
            buffers,
            ..plain.clone()
        };
        let mut player = Player::load(dir.path(), &o).unwrap();
        check(&mut player, &o, &expected, &format!("buffers {buffers:?}"));
    }
    let o = Options {
        compare: true,
        merge_barriers: true,
        loops: 2,
        ..Default::default()
    };
    let both = play::replay(dir.path(), &o).unwrap();
    assert_eq!(both.len(), 2);
    assert!(both.iter().all(|(_, r)| r.picture.unwrap().1));
    assert!(folder.join("replay.png").exists() && folder.join("replay-unchanged.png").exists());
}
