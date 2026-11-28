use super::host::GraphicsState;
use std::sync::OnceLock;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

pub(super) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| super::prepared::enabled() && crate::switch::on("VICTORY_PREPARE_PASSES"))
}

fn dual_source(t: &mut D3D12_RENDER_TARGET_BLEND_DESC) {
    t.BlendEnable = true.into();
    t.SrcBlend = D3D12_BLEND_ONE;
    t.SrcBlendAlpha = D3D12_BLEND_ONE;
    t.DestBlend = D3D12_BLEND_SRC1_COLOR;
    t.DestBlendAlpha = D3D12_BLEND_SRC1_ALPHA;
}

pub(super) fn output(format: DXGI_FORMAT) -> GraphicsState {
    let mut g = GraphicsState::default();
    g.rasterizer.FrontCounterClockwise = true.into();
    g.depth.StencilWriteMask = 0;
    g.topology = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    g.targets.NumRenderTargets = 1;
    g.targets.RTFormats[0] = format;
    g
}

pub(super) fn water() -> GraphicsState {
    let mut g = GraphicsState::default();
    dual_source(&mut g.blend.RenderTarget[0]);
    g.rasterizer.CullMode = D3D12_CULL_MODE_NONE;
    g.rasterizer.FrontCounterClockwise = true.into();
    g.depth.DepthEnable = false.into();
    g.depth.DepthWriteMask = D3D12_DEPTH_WRITE_MASK_ZERO;
    g.depth.DepthFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    g.depth.StencilWriteMask = 0;
    g.topology = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    g.targets.NumRenderTargets = 1;
    g.targets.RTFormats[0] = DXGI_FORMAT_R16G16B16A16_FLOAT;
    g
}

fn mesh(g: &GraphicsState) -> Option<GraphicsState> {
    if !g.inputs.is_empty()
        || !g.names.is_empty()
        || g.samples.Count != 1
        || g.topology != D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE
        || !g.depth.DepthEnable.as_bool()
        || g.depth.DepthFunc != D3D12_COMPARISON_FUNC_GREATER_EQUAL
        || g.depth_format != DXGI_FORMAT_D32_FLOAT_S8X24_UINT
    {
        return None;
    }
    let t = &g.blend.RenderTarget[0];
    let capture = g.targets.NumRenderTargets == 6
        && g.targets.RTFormats[..6]
            == [
                DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_FORMAT_R16G16_FLOAT,
                DXGI_FORMAT_R8G8B8A8_UNORM,
            ]
        && g.depth.DepthWriteMask == D3D12_DEPTH_WRITE_MASK_ALL
        && g.depth.StencilEnable.as_bool()
        && !t.BlendEnable.as_bool();
    let combine = g.targets.NumRenderTargets == 1
        && g.targets.RTFormats[0] == DXGI_FORMAT_R16G16B16A16_FLOAT
        && g.depth.DepthWriteMask == D3D12_DEPTH_WRITE_MASK_ZERO
        && !g.depth.StencilEnable.as_bool()
        && !g.blend.IndependentBlendEnable.as_bool()
        && t.BlendEnable.as_bool()
        && t.SrcBlend == D3D12_BLEND_SRC_ALPHA
        && t.DestBlend == D3D12_BLEND_INV_SRC_ALPHA
        && t.SrcBlendAlpha == D3D12_BLEND_SRC_ALPHA
        && t.DestBlendAlpha == D3D12_BLEND_INV_SRC_ALPHA;
    if !capture && !combine {
        return None;
    }
    let mut v = without_inputs(g);
    if capture {
        v.targets.NumRenderTargets = 1;
        v.targets.RTFormats[1..].fill(DXGI_FORMAT_UNKNOWN);
        v.depth_format = DXGI_FORMAT_D16_UNORM;
    } else {
        v.blend.IndependentBlendEnable = true.into();
        dual_source(&mut v.blend.RenderTarget[0]);
        for t in &mut v.blend.RenderTarget[1..] {
            t.RenderTargetWriteMask = 0;
        }
    }
    Some(v)
}

// Only called on passes without input elements (their names are owned by g).
fn without_inputs(g: &GraphicsState) -> GraphicsState {
    GraphicsState {
        blend: g.blend,
        sample_mask: g.sample_mask,
        rasterizer: g.rasterizer,
        depth: g.depth,
        depth_bounds: g.depth_bounds,
        inputs: Vec::new(),
        names: Vec::new(),
        strip_cut: g.strip_cut,
        topology: g.topology,
        targets: g.targets,
        depth_format: g.depth_format,
        samples: g.samples,
    }
}

/// Two shadow-map profiles for the same four cascade states. The other
/// profile doubles both *floating* biases, not the integer DepthBias. Match
/// exact title-emitted coefficients in either direction: warming twice
/// cannot generate more profiles. This only produces cache entries; drawing
/// always uses the descriptor the title actually supplied.
fn shadow(g: &GraphicsState) -> Option<GraphicsState> {
    if g.depth_format != DXGI_FORMAT_D16_UNORM
        || g.topology != D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE
        || !g.depth.DepthEnable.as_bool()
        || g.depth.DepthFunc != D3D12_COMPARISON_FUNC_LESS
        || !matches!(g.targets.NumRenderTargets, 0 | 2)
        || g.samples.Count != 1
        || !g.inputs.is_empty()
        || !g.names.is_empty()
    {
        return None;
    }
    const CLAMP: f32 = 0.0015;
    const SLOPE: f32 = 0.90625;
    let actual = (
        g.rasterizer.DepthBiasClamp,
        g.rasterizer.SlopeScaledDepthBias,
    );
    for (c, s) in [
        (CLAMP, SLOPE),
        (CLAMP * 0.75, SLOPE * 1.15),
        (CLAMP * 1.25, SLOPE * 1.25),
        (CLAMP, SLOPE * 1.25),
    ] {
        let other = if actual == (c, s) {
            (c * 2.0, s * 2.0)
        } else if actual == (c * 2.0, s * 2.0) {
            (c, s)
        } else {
            continue;
        };
        let mut v = without_inputs(g);
        (
            v.rasterizer.DepthBiasClamp,
            v.rasterizer.SlopeScaledDepthBias,
        ) = other;
        return Some(v);
    }
    None
}

pub(super) fn alternate(g: &GraphicsState, mesh_pixel: bool) -> Option<GraphicsState> {
    shadow(g).or_else(|| if mesh_pixel { mesh(g) } else { None })
}
