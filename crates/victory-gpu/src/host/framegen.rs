//! Frame generation's interface plane (`VICTORY_FRAMEGEN=1`, [`crate::ffx`]):
//! the title's second plane is copied beside the scene and the SDK composes
//! it over the real and the generated frames, so the interface is not
//! interpolated.

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Whether a plane of `plane` (format, width, height) can be copied whole
/// onto a back buffer of `back`'s: the same format and size.
pub(super) fn copyable(plane: (DXGI_FORMAT, u32, u32), back: (DXGI_FORMAT, u32, u32)) -> bool {
    plane == back
}

/// The interface plane's texture: the back buffer's size and format, in the
/// common state, made once per size in `slot`; `None` if it cannot be made.
pub(super) fn ui_texture(
    device: &ID3D12Device,
    slot: &mut Option<ID3D12Resource>,
    (width, height, format): (u32, u32, DXGI_FORMAT),
) -> Option<ID3D12Resource> {
    if slot.is_none() {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: width.into(),
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ..Default::default()
        };
        // SAFETY: plain creation of a render target.
        let made = unsafe {
            device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, slot)
        };
        if let Err(e) = made {
            tracing::warn!(error = %e, "no interface texture for frame generation: the frame is composed and not interpolated");
            return None;
        }
    }
    slot.clone()
}

/// Copies the interface plane `over` (in `over_state`) into `ui`, which is
/// in the common state, and puts `over` back in its state.
pub(super) fn record_ui(
    list: &ID3D12GraphicsCommandList,
    over: &ID3D12Resource,
    over_state: D3D12_RESOURCE_STATES,
    ui: &ID3D12Resource,
) {
    let mut before = Vec::with_capacity(2);
    if over_state != D3D12_RESOURCE_STATE_COPY_SOURCE {
        before.push(super::transition(over, over_state, D3D12_RESOURCE_STATE_COPY_SOURCE));
    }
    before.push(super::transition(ui, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST));
    let mut after = Vec::with_capacity(2);
    after.push(super::transition(ui, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON));
    if over_state != D3D12_RESOURCE_STATE_COPY_SOURCE {
        after.push(super::transition(over, D3D12_RESOURCE_STATE_COPY_SOURCE, over_state));
    }
    // SAFETY: a recording list of ours and live textures of one format and size.
    unsafe {
        list.ResourceBarrier(&before);
        list.CopyResource(ui, over);
        list.ResourceBarrier(&after);
    }
}
