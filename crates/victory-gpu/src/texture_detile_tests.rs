//! (Neptune) The fallback of the card's detile (`crate::detile`) to the CPU's, on the
//! software adapter: a child module of `crate::texture`.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use victory_formats::texture::{Desc, Dimension, Layout, Swizzle};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

fn warp() -> Option<ID3D12Device> {
    // SAFETY: plain DXGI queries and device creation on the software adapter.
    unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
        let adapter: IDXGIAdapter = factory.EnumWarpAdapter().ok()?;
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).ok()?;
        device
    }
}

/// A mapped upload buffer for `bytes` on `device`, as `stage_buffer` gives (zeros, its size
/// class: the next power of two from 64 KiB).
fn upload(device: &ID3D12Device, bytes: u64) -> Option<(ID3D12Resource, *mut u8)> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    let mut data = std::ptr::null_mut();
    // SAFETY: plain creation and mapping.
    unsafe {
        device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buffer)
            .ok()?;
        buffer.as_ref()?.Map(0, None, Some(&mut data)).ok()?;
    }
    Some((buffer?, data.cast()))
}

/// A 2D layout of `w` x `h` texels, `mips` levels and `slices` array slices: BC7 (`bc`) or
/// four bytes a texel.
fn layout(w: u32, h: u32, mips: u32, slices: u32, bc: bool, swizzle: Swizzle) -> Layout {
    Layout::new(&Desc {
        dimension: Dimension::Tex2d,
        width: w,
        height: h,
        depth: slices,
        mips,
        element_bytes: if bc { 16 } else { 4 },
        block: if bc { 4 } else { 1 },
        samples: 1,
        swizzle,
    })
    .unwrap()
}

/// A staging state for the texture `layout` describes at `title`, laid out by the device as
/// the host texture is (every level of every slice); with a staging buffer from `device`, or
/// none.
fn staging(device: &ID3D12Device, title: &[u8], layout: &Layout, cpu: bool) -> Staging {
    let desc = &layout.desc;
    let d = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(desc.width),
        Height: desc.height,
        DepthOrArraySize: desc.depth as u16,
        MipLevels: desc.mips as u16,
        Format: if desc.block == 4 { DXGI_FORMAT_BC7_UNORM } else { DXGI_FORMAT_R8G8B8A8_UNORM },
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    let count = desc.mips * desc.depth;
    let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); count as usize];
    let mut rows = vec![0u32; count as usize];
    let mut total = 0;
    // SAFETY: outputs for `count` subresources.
    unsafe {
        device.GetCopyableFootprints(&d, 0, count, 0, Some(footprints.as_mut_ptr()), Some(rows.as_mut_ptr()), None, Some(&mut total))
    };
    let side = std::sync::OnceLock::new();
    if cpu {
        let (buffer, data) = upload(device, total).expect("an upload buffer");
        STAGED[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        STAGED[1].fetch_add(total, std::sync::atomic::Ordering::Relaxed);
        let _ = side.set(CpuStaging { buffer, data: data as usize, base: 0, _piece: None });
    }
    Staging {
        cpu: side,
        #[cfg(feature = "diagnostics")]
        size: total,
        footprints,
        rows,
        layout: layout.clone(),
        mips: desc.mips,
        first: 0,
        read: layout.size as usize,
        level0: 0,
        serial: 0,
        holds: Arc::default(),
        once: AtomicBool::new(false),
        expected: AtomicU64::new(0),
        exact: AtomicBool::new(false),
        shape: [0; 4],
        bytes: total,
        memory: title.as_ptr(),
        last: Mutex::default(),
        dirty: AtomicBool::new(false),
        uploaded: AtomicBool::new(false),
        used: AtomicU64::new(0),
        on_demand: false,
        detiled: AtomicU32::new(0),
        judged: [const { AtomicU64::new(0) }; 2],
        waiting: AtomicU64::new(0),
    }
}

/// The staging buffer as the CPU's detile of `title` lays it out.
fn expected(s: &Staging, title: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; s.bytes as usize];
    for (i, (f, &rows)) in s.footprints.iter().zip(&s.rows).enumerate() {
        let (mip, slice) = s.part(i);
        let pitch = f.Footprint.RowPitch as usize;
        let at = f.Offset as usize;
        // The last subresource's last row ends at its bytes, not at a whole pitch.
        let len = (pitch * rows as usize).min(out.len() - at);
        s.layout.detile(title, mip, slice, &mut out[at..at + len], pitch, pitch * rows as usize).unwrap();
    }
    out
}

fn held(s: &Staging) -> Vec<u8> {
    let data = s.cpu.get().expect("a staging buffer").data;
    // SAFETY: the mapping holds `bytes` bytes, written by the refresh on this thread.
    unsafe { std::slice::from_raw_parts(data as *const u8, s.bytes as usize).to_vec() }
}

/// A texture whose staging buffer was let go (as `unstage` and the idle sweep let it go)
/// comes back detiled on the card, and finds the rings full: the upload goes the CPU's way
/// at once, with a staging buffer made for it, and the refresh detiles into that buffer
/// although the memory did not change since the texture was last looked at (the state with
/// no buffer had already taken its hash).
#[test]
fn warp_a_full_ring_after_the_staging_buffer_went_detiles_on_the_cpu() {
    use crate::detile::{Arena, GRAIN, Why, take_slices};
    use std::sync::atomic::Ordering::Relaxed;
    let Some(device) = warp() else {
        eprintln!("no software adapter: skipped");
        return;
    };
    let layout = layout(256, 256, 9, 1, true, Swizzle::SW_64KB_S);
    let title: Vec<u8> = (0..layout.size).map(|i| (i.wrapping_mul(0x9e37_79b9) >> 24) as u8).collect();

    // 1. Detiled on the CPU into a staging buffer of its own, which then goes.
    let first = Arc::new(staging(&device, &title, &layout, true));
    first.refresh();
    let want = expected(&first, &title);
    assert!(want.iter().any(|&b| b != 0));
    assert!(held(&first) == want, "the CPU's way, before");
    drop(first);

    // 2. The same texture, staged anew with the card's detile on: no staging buffer.
    let again = Arc::new(staging(&device, &title, &layout, false));
    assert!(again.cpu.get().is_none());
    // A check of it (a list that samples it and uploads nothing): noted, nothing written.
    again.refresh();
    assert!(again.last.lock().unwrap().hash.is_some(), "the check took the memory's hash");
    again.dirty.store(false, Relaxed);

    // 3. The raw ring full: the card's way is refused at once, nothing stays reserved.
    let (mut raw, mut out) = (Arena::new(GRAIN), Arena::new(1 << 20));
    assert!(raw.alloc(1).is_some(), "fill the raw ring");
    let (raw_used, out_used) = (raw.used(), out.used());
    let routed = route(
        &again,
        || take_slices(&mut raw, &mut out, again.read as u64, again.bytes).map(|_| unreachable!("the ring is full")),
        |bytes| upload(&device, bytes).map(|(buffer, data)| CpuStaging { buffer, data: data as usize, base: 0, _piece: None }),
    );
    assert!(matches!(routed, Route::Cpu), "the CPU's way");
    assert_eq!((raw.used(), out.used()), (raw_used, out_used), "no slice half taken");
    assert!(again.cpu.get().is_some(), "a staging buffer made for it");
    assert!(again.dirty.load(Relaxed), "to be uploaded");
    assert!(again.last.lock().unwrap().hash.is_none(), "what it was detiled from is forgotten");

    // 4. The submission's refresh: the memory unchanged, and detiled all the same.
    again.refresh();
    assert!(held(&again) == want, "the CPU's way, after the fallback");

    // 5. From now on the CPU's way, without asking the card or making another buffer.
    let routed = route(&again, || unreachable!("not asked again"), |_| unreachable!("made once"));
    assert!(matches!(routed, Route::Cpu));

    // 6. No memory for a staging buffer either: nothing recorded, and asked again later.
    let bare = staging(&device, &title, &layout, false);
    let routed = route(&bare, || Err(Why::OutFull), |_| None);
    assert!(matches!(routed, Route::Nothing));
    assert!(bare.cpu.get().is_none());

    // 7. The card's ring full with the raw one free: refused the same way, the raw slice
    // given back.
    let (mut raw, mut out) = (Arena::new(1 << 20), Arena::new(GRAIN));
    assert!(out.alloc(1).is_some());
    assert_eq!(take_slices(&mut raw, &mut out, 1000, 1000).map(|_| ()), Err(Why::OutFull));
    assert_eq!(raw.used(), 0, "the raw slice given back");
}

/// What `crate::detile::reserve` checks of each part, with the device's own footprints: every
/// level of every slice passes (the last one's last row ends at its bytes, not at a whole
/// pitch), and a slice's table is slice 0's moved by whole slices.
#[test]
fn warp_every_part_of_a_staging_layout_fits_its_slices() {
    use crate::detile::{check_part, pack, table_slice};
    let Some(device) = warp() else {
        eprintln!("no software adapter: skipped");
        return;
    };
    for l in [
        layout(256, 256, 9, 1, true, Swizzle::SW_64KB_S),
        layout(1024, 512, 11, 1, true, Swizzle::SW_64KB_D),
        layout(512, 512, 10, 6, true, Swizzle::SW_4KB_S),
        layout(300, 200, 9, 3, false, Swizzle::SW_64KB_S),
    ] {
        let title = vec![0u8; l.size as usize];
        let s = staging(&device, &title, &l, false);
        let subs = s.subs();
        assert_eq!(subs.len(), (l.desc.mips * l.desc.depth) as usize);
        for (i, sub) in subs.iter().enumerate() {
            let (keyed, shift) = table_slice(&l, sub.slice);
            assert_eq!(keyed, 0, "one table for every slice");
            let depth_pitch = sub.row_pitch as usize * sub.rows as usize;
            let packed = pack(&l.copy_table(sub.mip, keyed, sub.row_pitch as usize, depth_pitch).unwrap()).unwrap();
            let bytes = check_part(sub, &packed, shift, s.read as u64, s.bytes).unwrap_or_else(|why| panic!("{:?} part {i}: {why:?}", l.desc));
            assert!(sub.offset + bytes <= s.bytes);
            if i + 1 == subs.len() {
                assert!(bytes < sub.row_pitch * u64::from(sub.rows), "the last part ends inside its last row's pitch");
            }
            // Moved by its shift, slice 0's table reads what this slice's own table reads.
            let own = pack(&l.copy_table(sub.mip, sub.slice, sub.row_pitch as usize, depth_pitch).unwrap()).unwrap();
            assert_eq!(own.tiled_end, packed.tiled_end + shift);
        }
    }
}
