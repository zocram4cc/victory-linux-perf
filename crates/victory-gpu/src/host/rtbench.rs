//! What the walking programs' reads of title memory by address cost this
//! card, as the translator makes them (renderer port RT, part 2): a chase of
//! 64-byte nodes, each read in four loads of four dwords after a look in a
//! directory, through a raw view picked per lane (`NonUniformResourceIndex`)
//! or one the wave shares. Ignored by default; run it with
//! `cargo test -p victory-gpu --release --lib host::rtbench -- --ignored --nocapture`.
//! Needs `dxc.exe` of a Windows SDK (`VICTORY_DXC`, else the newest SDK's).

use super::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

/// Nodes per copy (32 MiB of 64-byte nodes), copies, steps of each chase.
const NODES: u32 = 1 << 19;
const COPIES: u32 = 8;
const STEPS: u32 = 48;
/// Threads: a 1440p frame's pixels, a ray each.
const THREADS: u32 = 2560 * 1440;

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=9)), UAV(u0)"
cbuffer C : register(b0) { uint mode; uint steps; uint mask; uint spread; };
ByteAddressBuffer dir : register(t0);
ByteAddressBuffer nodes[8] : register(t1);
RWByteAddressBuffer outb : register(u0);

uint4 node(uint copy, uint at, out uint4 b, out uint4 c, out uint4 d) {
    uint4 a;
    if (mode == 0) {
        a = nodes[0].Load4(at); b = nodes[0].Load4(at + 16); c = nodes[0].Load4(at + 32); d = nodes[0].Load4(at + 48);
    } else if (mode == 1) {
        a = nodes[NonUniformResourceIndex(copy)].Load4(at);
        b = nodes[NonUniformResourceIndex(copy)].Load4(at + 16);
        c = nodes[NonUniformResourceIndex(copy)].Load4(at + 32);
        d = nodes[NonUniformResourceIndex(copy)].Load4(at + 48);
    } else if (mode == 2) {
        // One loop for the node's four loads, a pass per copy the wave reads.
        a = 0; b = 0; c = 0; d = 0;
        [loop] for (;;) {
            uint first = WaveReadLaneFirst(copy);
            [branch] if (copy == first) {
                a = nodes[first].Load4(at); b = nodes[first].Load4(at + 16);
                c = nodes[first].Load4(at + 32); d = nodes[first].Load4(at + 48);
                break;
            }
        }
    } else {
        // The wave's own copy where all its lanes share it, else as mode 1.
        uint first = WaveReadLaneFirst(copy);
        [branch] if (WaveActiveAllTrue(copy == first)) {
            a = nodes[first].Load4(at); b = nodes[first].Load4(at + 16);
            c = nodes[first].Load4(at + 32); d = nodes[first].Load4(at + 48);
        } else {
            a = nodes[NonUniformResourceIndex(copy)].Load4(at);
            b = nodes[NonUniformResourceIndex(copy)].Load4(at + 16);
            c = nodes[NonUniformResourceIndex(copy)].Load4(at + 32);
            d = nodes[NonUniformResourceIndex(copy)].Load4(at + 48);
        }
    }
    return a;
}

[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint id : SV_DispatchThreadID) {
    // Neighbouring rays start near each other, as a frame's do.
    uint p = ((id >> 6) * 2654435761u + (id & 63) * 4099u) & mask;
    uint acc = 0;
    for (uint i = 0; i < steps; i++) {
        // The directory: the copy and the byte offset of the node's page.
        uint2 e = spread == 99 ? uint2(0, (p >> 10) * 1024 * 64) : dir.Load2((p >> 10) * 8);
        uint copy = (e.x + ((id >> spread) & 7)) & 7;
        if (spread == 0 || spread == 99) { copy = e.x; }
        uint at = e.y + (p & 1023) * 64;
        uint4 b, c, d;
        uint4 a = node(copy, at, b, c, d);
        acc += a.x ^ b.y ^ c.z ^ d.w;
        // The next node: near this one mostly, as a walk down a tree.
        p = (p * 1103515245u + 12345u + ((a.y ^ c.x) & 255)) & mask;
    }
    outb.Store(id * 4, acc);
}
"#;

fn dxc() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("VICTORY_DXC") {
        return Some(p.into());
    }
    let bin = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<_> = std::fs::read_dir(bin).ok()?.filter_map(Result::ok).map(|e| e.path()).collect();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(r"x64\dxc.exe")).find(|p| p.exists())
}

#[test]
#[ignore]
fn bench_title_memory_reads() {
    let Some(host) = super::host() else {
        eprintln!("no host GPU");
        return;
    };
    let Some(dxc) = dxc() else {
        eprintln!("no dxc.exe");
        return;
    };
    let dir = std::env::temp_dir().join(format!("victory-rtbench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("b.hlsl"), SOURCE).unwrap();
    let out = std::process::Command::new(&dxc)
        .args(["-T", "cs_6_0", "-E", "main", "-O3", "-Fo"])
        .arg(dir.join("b.cso"))
        .arg(dir.join("b.hlsl"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let code = std::fs::read(dir.join("b.cso")).unwrap();
    let device = &host.device;
    // SAFETY: plain object creation with valid descriptions; everything
    // outlives the lists run below.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code).unwrap();
        let pso: ID3D12PipelineState = device
            .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: code.as_ptr().cast(),
                    BytecodeLength: code.len(),
                },
                ..Default::default()
            })
            .unwrap();
        let queue: ID3D12CommandQueue = device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
            .unwrap();
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList =
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).unwrap();
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        let event = CreateEventW(None, false, false, None).unwrap();
        let frequency = queue.GetTimestampFrequency().unwrap() as f64;
        let mut done = 0u64;
        let run = |list: &ID3D12GraphicsCommandList, done: &mut u64| {
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
            *done += 1;
            queue.Signal(&fence, *done).unwrap();
            fence.SetEventOnCompletion(*done, event).unwrap();
            WaitForSingleObject(event, INFINITE);
            allocator.Reset().unwrap();
            list.Reset(&allocator, None).unwrap();
        };
        // The directory: every page in copy 0 at its own place (so a copy
        // number the wave shares, as nodes of one structure are).
        let pages = NODES / 1024;
        let dir_bytes = u64::from(pages) * 8;
        let node_bytes = u64::from(NODES) * 64;
        let (up, up_ptr) = host.upload_buffer(dir_bytes + node_bytes).unwrap();
        let words = up_ptr.cast::<u32>();
        for page in 0..pages as usize {
            *words.add(2 * page) = 0;
            *words.add(2 * page + 1) = page as u32 * 1024 * 64;
        }
        let mut x = 0x1234_5678u32;
        for i in 0..(node_bytes / 4) as usize {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *words.add((dir_bytes / 4) as usize + i) = x;
        }
        let directory = host.uav_buffer(dir_bytes).unwrap();
        let copies: Vec<ID3D12Resource> = (0..COPIES).map(|_| host.uav_buffer(node_bytes).unwrap()).collect();
        let outb = host.uav_buffer(u64::from(THREADS) * 4).unwrap();
        list.CopyBufferRegion(&directory, 0, &up, 0, dir_bytes);
        for c in &copies {
            list.CopyBufferRegion(c, 0, &up, dir_bytes, node_bytes);
        }
        let mut barriers = vec![transition(&directory, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE)];
        barriers.extend(copies.iter().map(|c| transition(c, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE)));
        list.ResourceBarrier(&barriers);
        run(&list, &mut done);
        let heap: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 1 + COPIES,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
            .unwrap();
        let step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as usize;
        let start = heap.GetCPUDescriptorHandleForHeapStart();
        for (i, (buffer, bytes)) in std::iter::once((&directory, dir_bytes)).chain(copies.iter().map(|c| (c, node_bytes))).enumerate() {
            device.CreateShaderResourceView(
                buffer,
                Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_TYPELESS,
                    ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer: D3D12_BUFFER_SRV {
                            FirstElement: 0,
                            NumElements: (bytes / 4) as u32,
                            StructureByteStride: 0,
                            Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                        },
                    },
                }),
                D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + i * step },
            );
        }
        let mut stamps: Option<ID3D12QueryHeap> = None;
        device
            .CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: 2, NodeMask: 0 }, &mut stamps)
            .unwrap();
        let stamps = stamps.unwrap();
        let back = host.read_back_buffer(16).unwrap();
        let mut time = |mode: u32, spread: u32| -> f64 {
            list.SetComputeRootSignature(&root);
            list.SetPipelineState(&pso);
            list.SetDescriptorHeaps(&[Some(heap.clone())]);
            list.SetComputeRoot32BitConstants(0, 4, [mode, STEPS, NODES - 1, spread].as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(1, heap.GetGPUDescriptorHandleForHeapStart());
            list.SetComputeRootUnorderedAccessView(2, outb.GetGPUVirtualAddress());
            list.EndQuery(&stamps, D3D12_QUERY_TYPE_TIMESTAMP, 0);
            list.Dispatch(THREADS / 64, 1, 1);
            list.EndQuery(&stamps, D3D12_QUERY_TYPE_TIMESTAMP, 1);
            list.ResolveQueryData(&stamps, D3D12_QUERY_TYPE_TIMESTAMP, 0, 2, &back, 0);
            run(&list, &mut done);
            let mut data = std::ptr::null_mut();
            back.Map(0, None, Some(&mut data)).unwrap();
            let t = std::slice::from_raw_parts(data.cast::<u64>(), 2);
            let ms = t[1].saturating_sub(t[0]) as f64 / frequency * 1e3;
            back.Unmap(0, None);
            ms
        };
        let cases: [(&str, u32, u32); 8] = [
            ("one view (uniform)", 0, 0),
            ("one view, no directory", 0, 99),
            ("view per lane, all lanes one copy", 1, 0),
            ("one loop per node, all lanes one copy", 2, 0),
            ("the wave's view if shared, all lanes one copy", 3, 0),
            ("view per lane, 2 copies a wave", 1, 5),
            ("one loop per node, 2 copies a wave", 2, 5),
            ("view per lane, 8 copies a wave", 1, 0x1f & 1),
        ];
        for &(_, mode, spread) in &cases {
            time(mode, spread);
        }
        for (name, mode, spread) in cases {
            let mut t: Vec<f64> = (0..5).map(|_| time(mode, spread)).collect();
            t.sort_by(f64::total_cmp);
            println!("{name}: {:.2} ms (median of 5; {STEPS} nodes a thread, {THREADS} threads)", t[2]);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
