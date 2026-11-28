//! Small D3D12 helpers the recorder and the replayer share.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{Interface, Result};

use super::wire;

/// Lists of our own on a queue, with a fence.
pub(super) struct Worker {
    pub(super) device: ID3D12Device,
    pub(super) queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    event: HANDLE,
    value: u64,
    /// Allocator, list, and the fence value its last run ends with.
    lists: Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList, u64)>,
}

// SAFETY: D3D12 objects are free-threaded; a worker is used by one thread
// at a time (`&mut self`).
unsafe impl Send for Worker {}

impl Worker {
    pub(super) fn new(device: &ID3D12Device, queue: &ID3D12CommandQueue) -> Result<Worker> {
        // SAFETY: plain object creation.
        unsafe {
            Ok(Worker {
                device: device.clone(),
                queue: queue.clone(),
                fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                event: CreateEventW(None, false, false, None)?,
                value: 0,
                lists: Vec::new(),
            })
        }
    }

    /// Records a list and sends it; the value [`Worker::wait`] takes.
    pub(super) fn send(&mut self, record: impl FnOnce(&ID3D12GraphicsCommandList)) -> Result<u64> {
        // SAFETY: lists and allocators of ours whose last run has finished.
        unsafe {
            let done = self.fence.GetCompletedValue();
            let i = match self.lists.iter().position(|l| l.2 <= done) {
                Some(i) => i,
                None => {
                    let allocator: ID3D12CommandAllocator = self
                        .device
                        .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
                    let list: ID3D12GraphicsCommandList = self.device.CreateCommandList(
                        0,
                        D3D12_COMMAND_LIST_TYPE_DIRECT,
                        &allocator,
                        None,
                    )?;
                    list.Close()?;
                    self.lists.push((allocator, list, 0));
                    self.lists.len() - 1
                }
            };
            let (allocator, list, last) = &mut self.lists[i];
            allocator.Reset()?;
            list.Reset(&*allocator, None)?;
            record(list);
            list.Close()?;
            self.queue.ExecuteCommandLists(&[Some(list.cast()?)]);
            self.value += 1;
            *last = self.value;
            self.queue.Signal(&self.fence, self.value)?;
        }
        Ok(self.value)
    }

    /// Everything sent to the queue so far has run.
    pub(super) fn idle(&mut self) -> Result<()> {
        self.value += 1;
        // SAFETY: our queue and fence.
        unsafe { self.queue.Signal(&self.fence, self.value)? };
        self.wait(self.value)
    }

    pub(super) fn wait(&self, value: u64) -> Result<()> {
        // SAFETY: our fence and event.
        unsafe {
            if self.fence.GetCompletedValue() < value {
                self.fence.SetEventOnCompletion(value, self.event)?;
                WaitForSingleObject(self.event, INFINITE);
            }
            self.device.GetDeviceRemovedReason()
        }
    }

    /// Sends and waits.
    pub(super) fn run(&mut self, record: impl FnOnce(&ID3D12GraphicsCommandList)) -> Result<()> {
        let value = self.send(record)?;
        self.wait(value)
    }
}

pub(super) fn buffer_desc(size: u64, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size.max(1),
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

/// A buffer of ours; heap types that are mapped come with their mapping.
pub(super) fn buffer(
    device: &ID3D12Device,
    kind: D3D12_HEAP_TYPE,
    size: u64,
    flags: D3D12_RESOURCE_FLAGS,
) -> Result<(ID3D12Resource, *mut u8)> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: kind,
        ..Default::default()
    };
    let state = match kind {
        D3D12_HEAP_TYPE_UPLOAD => D3D12_RESOURCE_STATE_GENERIC_READ,
        D3D12_HEAP_TYPE_READBACK => D3D12_RESOURCE_STATE_COPY_DEST,
        _ => D3D12_RESOURCE_STATE_COMMON,
    };
    let mut made: Option<ID3D12Resource> = None;
    let mut data = std::ptr::null_mut();
    // SAFETY: plain creation and mapping.
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &buffer_desc(size, flags),
            state,
            None,
            &mut made,
        )?;
        let made = made.ok_or_else(windows::core::Error::empty)?;
        if kind == D3D12_HEAP_TYPE_UPLOAD || kind == D3D12_HEAP_TYPE_READBACK {
            made.Map(0, None, Some(&mut data))?;
        }
        Ok((made, data.cast()))
    }
}

/// Planes of a format (depth and stencil, or a video format's).
pub(super) fn planes(format: i32) -> u32 {
    match format {
        19..=22 | 44..=47 | 103..=106 => 2,
        _ => 1,
    }
}

/// Subresources of a texture (1 for a buffer).
pub(super) fn subresources(d: &wire::Desc) -> u32 {
    if d.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
        return 1;
    }
    let layers = if d.dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D.0 {
        1
    } else {
        u32::from(d.depth)
    };
    u32::from(d.mips.max(1)) * layers.max(1) * planes(d.format)
}

/// The layout of `count` subresources from `first` in a buffer, and its
/// size.
pub(super) fn footprints(
    device: &ID3D12Device,
    desc: &D3D12_RESOURCE_DESC,
    first: u32,
    count: u32,
) -> (Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>, u64) {
    let mut all = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); count as usize];
    let mut total = 0;
    // SAFETY: outputs for `count` subresources.
    unsafe {
        device.GetCopyableFootprints(
            desc,
            first,
            count,
            0,
            Some(all.as_mut_ptr()),
            None,
            None,
            Some(&mut total),
        )
    };
    (all, total)
}

/// A location naming a subresource; no reference is taken.
pub(super) fn sub_location(res: &ID3D12Resource, sub: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer the location never releases.
        pResource: unsafe { std::mem::transmute_copy(res) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: sub,
        },
    }
}

/// A location naming bytes of a buffer; no reference is taken.
pub(super) fn placed_location(
    res: &ID3D12Resource,
    f: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above.
        pResource: unsafe { std::mem::transmute_copy(res) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: f },
    }
}

/// A transition; no reference is taken.
pub(super) fn transition(
    res: &ID3D12Resource,
    sub: u32,
    before: u32,
    after: u32,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: as above.
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: sub,
                StateBefore: D3D12_RESOURCE_STATES(before as i32),
                StateAfter: D3D12_RESOURCE_STATES(after as i32),
            }),
        },
    }
}

/// The barriers that move a texture from `from` (one state, or one for each
/// subresource) to `to`.
pub(super) fn moves(
    res: &ID3D12Resource,
    from: &[u32],
    to: &[u32],
    out: &mut Vec<D3D12_RESOURCE_BARRIER>,
) {
    let n = from.len().max(to.len());
    if n == 1 {
        if from[0] != to[0] {
            out.push(transition(res, wire::ALL, from[0], to[0]));
        }
        return;
    }
    for i in 0..n {
        let (a, b) = (from[i.min(from.len() - 1)], to[i.min(to.len() - 1)]);
        if a != b {
            out.push(transition(res, i as u32, a, b));
        }
    }
}

pub(super) fn to_desc(d: &wire::Desc) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION(d.dimension),
        Alignment: d.alignment,
        Width: d.width,
        Height: d.height,
        DepthOrArraySize: d.depth,
        MipLevels: d.mips,
        Format: DXGI_FORMAT(d.format),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: d.samples[0],
            Quality: d.samples[1],
        },
        Layout: D3D12_TEXTURE_LAYOUT(d.layout),
        Flags: D3D12_RESOURCE_FLAGS(d.flags),
    }
}

pub(super) fn from_desc(d: &D3D12_RESOURCE_DESC) -> wire::Desc {
    wire::Desc {
        dimension: d.Dimension.0,
        alignment: d.Alignment,
        width: d.Width,
        height: d.Height,
        depth: d.DepthOrArraySize,
        mips: d.MipLevels,
        format: d.Format.0,
        samples: [d.SampleDesc.Count, d.SampleDesc.Quality],
        layout: d.Layout.0,
        flags: d.Flags.0,
    }
}

pub(super) fn to_props(p: &wire::HeapProps) -> D3D12_HEAP_PROPERTIES {
    D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE(p.kind),
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY(p.cpu),
        MemoryPoolPreference: D3D12_MEMORY_POOL(p.pool),
        ..Default::default()
    }
}

pub(super) fn from_props(p: &D3D12_HEAP_PROPERTIES) -> wire::HeapProps {
    wire::HeapProps {
        kind: p.Type.0,
        cpu: p.CPUPageProperty.0,
        pool: p.MemoryPoolPreference.0,
    }
}

/// How the CPU sees memory of a heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Seen {
    /// Not at all: the card's own.
    No,
    /// Cached, read and written.
    Cached,
    /// Write-combined: slow to read.
    Combined,
    /// A read-back heap: the CPU only reads.
    ReadBack,
}

pub(super) fn seen(p: &wire::HeapProps) -> Seen {
    match D3D12_HEAP_TYPE(p.kind) {
        D3D12_HEAP_TYPE_DEFAULT => Seen::No,
        D3D12_HEAP_TYPE_UPLOAD => Seen::Combined,
        D3D12_HEAP_TYPE_READBACK => Seen::ReadBack,
        D3D12_HEAP_TYPE_CUSTOM => match D3D12_CPU_PAGE_PROPERTY(p.cpu) {
            D3D12_CPU_PAGE_PROPERTY_WRITE_BACK => Seen::Cached,
            D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE => Seen::Combined,
            _ => Seen::No,
        },
        // GPU upload heaps (type 5).
        _ => Seen::Combined,
    }
}

/// 8-bit RGB of a presented texture's rows (and the greatest channel value
/// of the format, for the signal-to-noise ratio). `None` for formats not
/// shown.
pub(super) fn rgb(
    width: u32,
    height: u32,
    pitch: u32,
    format: i32,
    rows: &[u8],
) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((width * height * 3) as usize);
    let half = |b: &[u8]| {
        let h = u16::from_le_bytes([b[0], b[1]]);
        let (s, e, m) = (h >> 15, (h >> 10) & 31, f32::from(h & 1023));
        let v = match e {
            0 => m * 2f32.powi(-24),
            31 => 65536.0,
            e => (1.0 + m / 1024.0) * 2f32.powi(i32::from(e) - 15),
        };
        if s == 1 {
            0
        } else {
            (v.min(1.0) * 255.0) as u8
        }
    };
    for y in 0..height as usize {
        let row = rows.get(y * pitch as usize..)?;
        for x in 0..width as usize {
            match format {
                27..=32 => out.extend_from_slice(row.get(x * 4..x * 4 + 3)?),
                87 | 88 | 90..=93 => {
                    let p = row.get(x * 4..x * 4 + 3)?;
                    out.extend([p[2], p[1], p[0]]);
                }
                23..=25 => {
                    let p = row.get(x * 4..x * 4 + 4)?;
                    let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                    out.extend([(v >> 2) as u8, (v >> 12) as u8, (v >> 22) as u8]);
                }
                9..=14 => {
                    let p = row.get(x * 8..x * 8 + 6)?;
                    out.extend([half(p), half(&p[2..]), half(&p[4..])]);
                }
                _ => return None,
            }
        }
    }
    Some(out)
}

/// Peak signal-to-noise ratio of two pictures of 8-bit channels, in dB
/// (infinite if equal).
pub(super) fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| u64::from(x.abs_diff(y)).pow(2))
        .sum();
    if sum == 0 && a.len() == b.len() {
        return f64::INFINITY;
    }
    let mean = sum as f64 / a.len().min(b.len()).max(1) as f64;
    10.0 * (255.0 * 255.0 / mean).log10()
}

/// The adapter a device is on: name, vendor and device number.
pub(super) fn adapter(device: &ID3D12Device) -> (String, u32, u32) {
    use windows::Win32::Graphics::Dxgi::*;
    // SAFETY: plain queries.
    let desc = unsafe {
        CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0))
            .and_then(|f| f.EnumAdapterByLuid::<IDXGIAdapter1>(device.GetAdapterLuid()))
            .and_then(|a| a.GetDesc1())
    };
    match desc {
        Ok(d) => {
            let name = String::from_utf16_lossy(&d.Description);
            (
                name.trim_end_matches('\0').to_owned(),
                d.VendorId,
                d.DeviceId,
            )
        }
        Err(_) => ("unknown".into(), 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures() {
        assert_eq!(rgb(1, 1, 4, 28, &[1, 2, 3, 4]).unwrap(), [1, 2, 3]);
        assert_eq!(rgb(1, 1, 4, 87, &[1, 2, 3, 4]).unwrap(), [3, 2, 1]);
        let v = (1023u32 | 512 << 10).to_le_bytes();
        assert_eq!(rgb(1, 1, 4, 24, &v).unwrap(), [255, 128, 0]);
        // 1.0, 0.5 and -1.0 as halves.
        assert_eq!(
            rgb(1, 1, 8, 10, &[0, 0x3c, 0, 0x38, 0, 0xbc, 0, 0]).unwrap(),
            [255, 127, 0]
        );
        assert!(rgb(1, 1, 4, 41, &[0; 4]).is_none());
        assert!(rgb(2, 2, 8, 28, &[0; 12]).is_none(), "too few rows");
        assert_eq!(psnr(&[1, 2], &[1, 2]), f64::INFINITY);
        assert!((psnr(&[0; 4], &[255; 4])).abs() < 1e-9);
    }

    #[test]
    fn states_of_subresources() {
        assert_eq!(planes(44), 2);
        let mut d = from_desc(&buffer_desc(10, D3D12_RESOURCE_FLAG_NONE));
        assert_eq!(subresources(&d), 1);
        assert_eq!(from_desc(&to_desc(&d)), d);
        (d.dimension, d.depth, d.mips, d.format) = (3, 6, 4, 19);
        assert_eq!(subresources(&d), 48);
        d.dimension = 4;
        assert_eq!(subresources(&d), 8);
    }
}
