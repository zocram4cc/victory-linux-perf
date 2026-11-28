//! Keeps the card's clocks up between submissions (`VICTORY_CARD_AWAKE=1`; off
//! by default). The card stands idle half of every frame, some thirty times for
//! a millisecond or two each, while the CPU prepares the next submission. The
//! driver takes that for a light load and lowers the clocks, and what comes
//! next starts slow: a captured frame whose lists take 30 ms sent in one go
//! takes 38-40 ms with waits of 1.2-2.4 ms between its submissions
//! (`victory replay --pace`).
//!
//! One group of 64 threads counts on a compute queue, over and
//! over: it takes one or two of the card's few thousand wave slots, and the
//! card is never idle. Nothing is sent once no submission has gone out for
//! a quarter of a second (a title that stands still; not a menu, which goes on
//! drawing: the system's counter shows the compute queue at 100 % there too).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;

/// About 2 ms at full clocks: 500 sends a second.
const COUNT: &str = r#"
#define RS "UAV(u0)"
RWByteAddressBuffer sum : register(u0);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint x = id.x;
    [loop] for (uint i = 0; i < 400000u; i++) x = x * 1664525u + 1013904223u;
    sum.Store(id.x * 4u, x);
}
"#;

/// Quiet this long after the last submission.
const QUIET_MS: u64 = 250;

fn now_ms() -> u64 {
    static START: OnceLock<crate::clock::Instant> = OnceLock::new();
    START.get_or_init(crate::clock::Instant::now).elapsed().as_millis() as u64
}

/// When a submission was last sent ([`now_ms`] plus one; 0: never).
static LAST: AtomicU64 = AtomicU64::new(0);

fn wanted() -> bool {
    let (last, now) = (LAST.load(Ordering::Relaxed), now_ms());
    last != 0 && now + 1 < last + QUIET_MS && (mode() != Mode::ByTurns || now / TURN_MS % 2 == 1)
}

/// With `VICTORY_CARD_AWAKE=turns`: this long on, this long off.
const TURN_MS: u64 = 20_000;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Off,
    On,
    /// On and off by turns, to compare the two in one run (the log says when).
    ByTurns,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_CARD_AWAKE").as_deref() {
        Ok("1") => Mode::On,
        Ok("turns") => Mode::ByTurns,
        _ => Mode::Off,
    })
}

/// A submission of the title's goes to `device`'s drawing queue.
pub(super) fn sent(device: &ID3D12Device) {
    if mode() == Mode::Off {
        return;
    }
    let now = now_ms();
    let before = LAST.swap(now + 1, Ordering::Relaxed);
    if mode() == Mode::ByTurns && before.saturating_sub(1) / TURN_MS != now / TURN_MS {
        tracing::info!(on = now / TURN_MS % 2 == 1, "the card is kept awake, by turns");
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        // Runs as long as the process does.
        std::mem::forget(start(device, wanted));
    });
}

/// The thread that keeps the card busy; it ends when this is dropped.
pub(crate) struct Awake {
    stop: std::sync::Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Awake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Keeps the card of `device` busy while `wanted`.
pub(crate) fn start(device: &ID3D12Device, wanted: impl Fn() -> bool + Send + 'static) -> Option<Awake> {
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (device, ended) = (device.clone(), stop.clone());
    let thread = std::thread::Builder::new()
        .name("victory-card-awake".into())
        .spawn(move || {
            if let Err(e) = run(&device, &wanted, &ended) {
                tracing::warn!(error = %e, "the card is not kept awake");
            }
        })
        .ok()?;
    Some(Awake {
        stop,
        thread: Some(thread),
    })
}

fn run(device: &ID3D12Device, wanted: &dyn Fn() -> bool, stop: &AtomicBool) -> windows::core::Result<()> {
    let code = super::compile(COUNT, windows::core::s!("main"), windows::core::s!("cs_5_1"))?;
    // SAFETY: plain object creation; the code lives through the calls, the
    // root signature is inside it. Two closed lists, sent in turn, whose
    // objects live until what was sent last has run.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code)?;
        let pso: ID3D12PipelineState = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: code.as_ptr().cast(),
                BytecodeLength: code.len(),
            },
            ..Default::default()
        })?;
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_COMPUTE,
            Priority: D3D12_COMMAND_QUEUE_PRIORITY_NORMAL.0,
            ..Default::default()
        })?;
        let mut sum: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: 256,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut sum,
        )?;
        let sum = sum.ok_or_else(windows::core::Error::empty)?;
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let mut lists = Vec::new();
        for _ in 0..2 {
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COMPUTE, &allocator, &pso)?;
            list.SetComputeRootSignature(&root);
            list.SetComputeRootUnorderedAccessView(0, sum.GetGPUVirtualAddress());
            list.Dispatch(1, 1, 1);
            list.Close()?;
            lists.push(Some(list.cast::<ID3D12CommandList>()?));
        }
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
        let event = CreateEventW(None, false, false, None)?;
        let mut value = 0u64;
        let mut result = Ok(());
        while !stop.load(Ordering::Relaxed) && result.is_ok() {
            if !wanted() {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            // Never more than two on the way.
            if value >= 2 && fence.GetCompletedValue() + 2 <= value {
                result = fence.SetEventOnCompletion(value - 1, event);
                if result.is_ok() {
                    WaitForSingleObject(event, 1000);
                    result = device.GetDeviceRemovedReason();
                }
                continue;
            }
            queue.ExecuteCommandLists(&lists[(value % 2) as usize..][..1]);
            value += 1;
            result = queue.Signal(&fence, value);
        }
        if result.is_ok() && fence.GetCompletedValue() < value && fence.SetEventOnCompletion(value, event).is_ok() {
            WaitForSingleObject(event, INFINITE);
        }
        let _ = CloseHandle(event);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Card memory the CPU maps holds what the CPU wrote, with the card kept
    /// awake meanwhile; the thread ends when it is let go.
    #[test]
    fn mapped_card_memory_is_written_by_the_cpu_and_read_by_the_card() {
        let Some(host) = crate::host::host() else {
            return;
        };
        let awake = start(&host.device, || true);
        assert!(awake.is_some());
        let Some((buffer, data)) = host.mapped_card_buffer(4096, true) else {
            return;
        };
        let (seen, back) = host.shared_buffer(4096).unwrap();
        // SAFETY: mappings of 4096 bytes; a list, a queue and a fence of the
        // test's own, and nothing is read before the copy has run.
        let read = unsafe {
            for i in 0..4096 {
                data.add(i).write((i * 7 + i / 251) as u8);
            }
            let device = &host.device;
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .unwrap();
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).unwrap();
            list.CopyBufferRegion(&seen, 0, &buffer, 0, 4096);
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
            queue.Signal(&fence, 1).unwrap();
            // No event: returns when the fence is there.
            fence.SetEventOnCompletion(1, windows::Win32::Foundation::HANDLE::default()).unwrap();
            std::slice::from_raw_parts(back, 4096).to_vec()
        };
        assert!(read.iter().enumerate().all(|(i, &b)| b == (i * 7 + i / 251) as u8));
        drop(awake);
    }
}
