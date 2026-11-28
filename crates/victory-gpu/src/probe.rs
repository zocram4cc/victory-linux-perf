use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

use crate::host::HostList;

struct Probe {
    dir: PathBuf,
    shaders: Vec<String>,
    at: u64,
    frames: u64,
    each: u64,
    cap: u64,
    text: Mutex<Option<std::fs::File>>,
    counts: Mutex<HashMap<String, u64>>,
}

static FIRST: AtomicU64 = AtomicU64::new(u64::MAX);
static SEQ: AtomicU64 = AtomicU64::new(0);

fn probe() -> Option<&'static Probe> {
    static PROBE: OnceLock<Option<Probe>> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            let dir = PathBuf::from(std::env::var_os("VICTORY_PROBE").filter(|v| !v.is_empty())?);
            let number = |name: &str, or: u64| std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(or);
            let shaders = std::env::var("VICTORY_PROBE_SHADERS").unwrap_or_default().split(',').filter(|w| !w.is_empty()).map(str::to_owned).collect();
            let _ = std::fs::create_dir_all(&dir);
            let text = std::fs::File::create(dir.join("probe.txt")).ok();
            Some(Probe {
                dir,
                shaders,
                at: number("VICTORY_PROBE_AT", 90),
                frames: number("VICTORY_PROBE_FRAMES", 1),
                each: number("VICTORY_PROBE_EACH", 2),
                cap: number("VICTORY_PROBE_MB", 32) << 20,
                text: Mutex::new(text),
                counts: Mutex::new(HashMap::new()),
            })
        })
        .as_ref()
}

pub(crate) fn wanted(hash: &str) -> Option<(u64, bool)> {
    let p = probe()?;
    if !p.shaders.iter().any(|w| hash.starts_with(w.as_str())) || crate::host::now_ms() < p.at * 1000 {
        return None;
    }
    let frame = crate::frame::current();
    let first = match FIRST.compare_exchange(u64::MAX, frame + 1, Relaxed, Relaxed) {
        Ok(_) => frame + 1,
        Err(first) => first,
    };
    if frame < first || frame >= first + p.frames {
        return None;
    }
    let seq = SEQ.fetch_add(1, Relaxed);
    let mut counts = p.counts.lock().unwrap_or_else(|e| e.into_inner());
    let n = counts.entry(hash.to_owned()).or_default();
    *n += 1;
    Some((seq, *n <= p.each))
}

pub(crate) fn note(text: &str) {
    let Some(p) = probe() else {
        return;
    };
    if let Some(file) = p.text.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(file, "{text}");
        let _ = file.flush();
    }
}

pub(crate) fn snap(list: &HostList, seq: u64, tag: &'static str, ranges: &[(u64, u64)]) {
    let (Some(p), Some(host)) = (probe(), crate::host::host()) else {
        return;
    };
    let mut done: Vec<(u64, u64)> = Vec::new();
    for &(start, len) in ranges {
        let len = len.min(p.cap);
        if start == 0 || len == 0 || done.contains(&(start, len)) {
            continue;
        }
        done.push((start, len));
        let source = if crate::records::on() {
            crate::um::mirror_at(start, len)
        } else {
            crate::shadow::find(start, len).map(|s| (s.buffer.clone(), s.offset(start)))
        };
        let Some((source, offset)) = source else {
            note(&format!("{seq} {tag} {start:#x} {len:#x} no host buffer"));
            continue;
        };
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: len,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut back: Option<ID3D12Resource> = None;
        let made = unsafe { host.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut back) };
        let Some(back) = back.filter(|_| made.is_ok()) else {
            note(&format!("{seq} {tag} {start:#x} {len:#x} no read-back buffer"));
            continue;
        };
        let barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER { pResource: std::mem::ManuallyDrop::new(None) }),
            },
        };
        unsafe {
            list.list.ResourceBarrier(&[barrier]);
            list.list.CopyBufferRegion(&back, 0, &source, offset, len);
        }
        let path = p.dir.join(format!("{seq:05}-{tag}-{start:x}-{len:x}.bin"));
        let cpu = p.dir.join(format!("{seq:05}-{tag}cpu-{start:x}-{len:x}.bin"));
        let small = len <= 1 << 16;
        list.after_gpu(move || {
            let mut data = std::ptr::null_mut();
            unsafe {
                if back.Map(0, None, Some(&mut data)).is_ok() {
                    let _ = std::fs::write(&path, std::slice::from_raw_parts(data.cast::<u8>(), len as usize));
                    back.Unmap(0, None);
                }
            }
            if small {
                let mut bytes = vec![0u8; len as usize];
                crate::host::read_there(start as *const u8, &mut bytes);
                let _ = std::fs::write(&cpu, &bytes);
            }
        });
    }
}
