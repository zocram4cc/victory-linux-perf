//! How the frame recorder and replayer do at the size of a title's frame
//! (not run by default: `cargo test --release ... -- --ignored --nocapture`):
//! thousands of textures and buffers, a view heap of 1.5 million slots, 30
//! submissions of 40,000 calls.
//!
//! `BIG_TEXTURES` (17000), `BIG_CARD` (600 buffers of 1 MB in the card's
//! memory) and `BIG_FOLDER` (a temporary one) change it.
#![cfg(all(windows, feature = "diagnostics"))]

use std::mem::ManuallyDrop;
use std::time::Instant;

use victory_gpu::replay::play::{self, Options};
use victory_gpu::replay::record::{self, At, Settings};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;

const SUBMITS: usize = 30;
const CALLS: usize = 40_000;
const VIEWS: u32 = 1_500_000;
const PORTABLE_VIEWS: u32 = 1_000_000;

fn buffer(device: &ID3D12Device, kind: u32, size: u64) -> (ID3D12Resource, *mut u8) {
    let (heap, state, flags) = match kind {
        0 => (
            D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ),
        1 => (
            D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_UPLOAD,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_GENERIC_READ,
            D3D12_RESOURCE_FLAG_NONE,
        ),
        _ => (
            D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_CUSTOM,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
                MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ),
    };
    let desc = D3D12_RESOURCE_DESC {
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
    };
    let mut made: Option<ID3D12Resource> = None;
    let mut data = std::ptr::null_mut();
    // SAFETY: plain creation and mapping.
    unsafe {
        device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut made)
            .unwrap();
        let made = made.unwrap();
        if kind > 0 {
            made.Map(0, None, Some(&mut data)).unwrap();
            // Not all zero, as real data is.
            for at in (0..size as usize).step_by(4096) {
                data.cast::<u8>().add(at).write(at as u8 | 1);
            }
        }
        (made, data.cast())
    }
}

fn barrier(
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

/// A number from the environment.
fn number(name: &str, otherwise: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(otherwise)
}

struct Big {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    value: u64,
    lists: Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)>,
    _textures: Vec<ID3D12Resource>,
    card: Vec<ID3D12Resource>,
    shared: Vec<(ID3D12Resource, *mut u8)>,
    upload: Vec<(ID3D12Resource, *mut u8)>,
    views: ID3D12DescriptorHeap,
    target: ID3D12Resource,
}

impl Big {
    fn new(device: &ID3D12Device, textures: usize) -> Big {
        let texture = |size: u32, flags, state| {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: u64::from(size),
                Height: size,
                DepthOrArraySize: 1,
                MipLevels: if size == 256 { 9 } else { 1 },
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Flags: flags,
                ..Default::default()
            };
            let mut made: Option<ID3D12Resource> = None;
            // SAFETY: plain creation.
            unsafe {
                device
                    .CreateCommittedResource(
                        &heap,
                        D3D12_HEAP_FLAG_NONE,
                        &desc,
                        state,
                        None,
                        &mut made,
                    )
                    .unwrap()
            };
            made.unwrap()
        };
        // SAFETY: plain creation; slots of the heap just made.
        unsafe {
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .unwrap();
            let lists = (0..SUBMITS)
                .map(|_| {
                    let a: ID3D12CommandAllocator = device
                        .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                        .unwrap();
                    let l: ID3D12GraphicsCommandList = device
                        .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &a, None)
                        .unwrap();
                    l.Close().unwrap();
                    (a, l)
                })
                .collect();
            let make_views = |count: u32| -> windows::core::Result<ID3D12DescriptorHeap> {
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: count,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
            };
            let (views, view_count) = match make_views(VIEWS) {
                Ok(heap) => (heap, VIEWS),
                Err(_) => (make_views(PORTABLE_VIEWS).unwrap(), PORTABLE_VIEWS),
            };
            let step = device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                as usize;
            let start = views.GetCPUDescriptorHandleForHeapStart().ptr;
            let textures: Vec<ID3D12Resource> = (0..textures)
                .map(|_| {
                    texture(
                        256,
                        D3D12_RESOURCE_FLAG_NONE,
                        D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE,
                    )
                })
                .collect();
            for i in 0..view_count as usize {
                let at = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: start + i * step,
                };
                device.CreateShaderResourceView(&textures[i % textures.len()], None, at);
            }
            let shared: Vec<(ID3D12Resource, *mut u8)> =
                (0..1000).map(|_| buffer(device, 2, 1 << 20)).collect();
            for (i, (b, _)) in shared.iter().enumerate() {
                let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_TYPELESS,
                    ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer: D3D12_BUFFER_SRV {
                            NumElements: 1 << 18,
                            Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                            ..Default::default()
                        },
                    },
                };
                device.CreateShaderResourceView(
                    b,
                    Some(&desc),
                    D3D12_CPU_DESCRIPTOR_HANDLE {
                        ptr: start + i * step,
                    },
                );
            }
            Big {
                device: device.clone(),
                queue,
                fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap(),
                value: 0,
                lists,
                _textures: textures,
                card: (0..number("BIG_CARD", 600))
                    .map(|_| buffer(device, 0, 1 << 20).0)
                    .collect(),
                shared,
                upload: (0..1000).map(|_| buffer(device, 1, 1 << 20)).collect(),
                views,
                target: texture(
                    1024,
                    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
            }
        }
    }

    fn wait(&mut self) {
        self.value += 1;
        // SAFETY: our queue and fence.
        unsafe {
            let event = CreateEventW(None, false, false, None).unwrap();
            self.queue.Signal(&self.fence, self.value).unwrap();
            self.fence.SetEventOnCompletion(self.value, event).unwrap();
            WaitForSingleObject(event, INFINITE);
            let _ = windows::Win32::Foundation::CloseHandle(event);
            self.device.GetDeviceRemovedReason().unwrap();
        }
    }

    fn frame(&mut self, n: usize) {
        if n == 0 {
            // Nothing in the card's buffers is all zero, as in a title's.
            // SAFETY: a list whose last run has finished; live buffers.
            unsafe {
                let (a, l) = &self.lists[0];
                a.Reset().unwrap();
                l.Reset(a, None).unwrap();
                for (i, card) in self.card.iter().enumerate() {
                    l.CopyBufferRegion(card, 0, &self.upload[i % self.upload.len()].0, 0, 1 << 20);
                }
                l.Close().unwrap();
                self.queue.ExecuteCommandLists(&[Some(l.cast().unwrap())]);
            }
            self.wait();
        }
        for s in 0..SUBMITS {
            // SAFETY: lists whose last run has finished; mappings written
            // inside their bounds.
            unsafe {
                let (a, l) = &self.lists[s];
                a.Reset().unwrap();
                l.Reset(a, None).unwrap();
                l.SetDescriptorHeaps(&[Some(self.views.clone())]);
                for i in 0..CALLS / SUBMITS / 4 {
                    let card = &self.card[(i * 7 + s) % self.card.len()];
                    let from = &self.upload[(i + s * 31) % self.upload.len()].0;
                    l.CopyBufferRegion(card, (i % 1000 * 1000) as u64 & !3, from, 0, 256);
                    l.ResourceBarrier(&[barrier(
                        card,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    )]);
                    l.ResourceBarrier(&[barrier(
                        card,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    )]);
                    l.SetMarker(1, None, 0);
                }
                l.Close().unwrap();
                for i in 0..60 {
                    let at = (n * 97 + s * 13 + i * 7) % 1000;
                    self.shared[at].1.add(i * 4096).write((n + s) as u8);
                    self.upload[at].1.add(i * 8192).write((n + s) as u8);
                }
                self.queue.ExecuteCommandLists(&[Some(l.cast().unwrap())]);
            }
            self.wait();
        }
        record::present(&self.target, D3D12_RESOURCE_STATE_COPY_SOURCE);
    }
}

#[test]
#[ignore = "takes gigabytes of the card's memory and the disk"]
fn a_frame_of_a_titles_size() {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: plain creation.
    unsafe { D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap() };
    let device = device.unwrap();
    let textures = number("BIG_TEXTURES", 17_000);
    let timed = |what: &str, began: Instant| {
        println!("{what}: {:.1} ms", began.elapsed().as_secs_f64() * 1e3)
    };
    let began = Instant::now();
    {
        let mut big = Big::new(&device, textures);
        timed("made, without the recorder", began);
        for n in 0..4 {
            let began = Instant::now();
            big.frame(n);
            timed("frame, without the recorder", began);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let folder = std::env::var_os("BIG_FOLDER").map_or(dir.path().to_owned(), Into::into);
    let settings = Settings {
        folder,
        at: At::Frame(4),
        frames: 1,
        always: false,
    };
    let folder = record::start(settings, &device).unwrap();
    let began = Instant::now();
    let mut big = Big::new(&device, textures);
    timed("made, with the recorder", began);
    for n in 0..8 {
        let began = Instant::now();
        big.frame(n);
        timed(
            [
                "frame",
                "frame, lists recorded",
                "frame, captured",
                "frame, after",
            ][[0, 0, 0, 0, 1, 1, 2, 3][n]],
            began,
        );
    }
    assert_eq!(record::finished(), Some(folder.clone()));
    drop(big);
    record::unhook(&device);
    println!(
        "{}",
        std::fs::read_to_string(folder.join("notes.txt")).unwrap()
    );
    for f in std::fs::read_dir(&folder).unwrap().flatten() {
        println!(
            "{:>8} MB {}",
            f.metadata().unwrap().len() >> 20,
            f.file_name().to_string_lossy()
        );
    }
    let began = Instant::now();
    let reports = play::replay(
        &folder,
        &Options {
            loops: 3,
            ..Default::default()
        },
    )
    .unwrap();
    timed("loaded and replayed", began);
    for (what, r) in &reports {
        println!("{}", r.text(what));
    }
}
