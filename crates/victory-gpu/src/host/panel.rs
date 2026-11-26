//! Diagnostics: the run's numbers on the picture (`VICTORY_PANEL=1`; off by
//! default; F3 shows and hides it while our window has the focus): a sleek
//! dark HUD in the upper left corner with large coloured FPS, live frame-time
//! graph, GPU/VRAM/RAM stats and per-core CPU bars, so that a screenshot
//! carries them. Made again twice a second from the counters of the frame-rate
//! line, memory and textures; a frame itself only copies the box out of a
//! buffer in the card's memory onto the swap chain's buffer.

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F3};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::{Host, HostList, transition};

/// How often the overlay redraws its buffer.
const EVERY: Duration = Duration::from_millis(250);

/// Base 5x7 glyph bitmap size.
const GLYPH: (u32, u32) = (5, 7);

/// Logical canvas dimensions (1x at <=1600p, 2x at 4K).
const BASE_W: u32 = 540;
const BASE_H: u32 = 334;

/// How many recent frames the live frame-time graph keeps.
const HISTORY: usize = 172;

/// The texts are put into one of these in turn: the card reads the others,
/// at most a second old.
const UPLOADS: usize = 4;

// Palette (sRGB 0..255).
const BG: [u8; 3] = [18, 20, 25];
const BORDER: [u8; 3] = [56, 62, 74];
const GRAPH_BG: [u8; 3] = [11, 13, 17];
const GRAPH_GRID: [u8; 3] = [44, 48, 58];
const GRAPH_FLOOR: [u8; 3] = [112, 62, 34];
const BAR_BG: [u8; 3] = [34, 38, 46];
const BAR_FILL_LO: [u8; 3] = [88, 212, 232];
const BAR_FILL_HI: [u8; 3] = [116, 178, 252];
const LABEL: [u8; 3] = [108, 178, 252];
const WHITE: [u8; 3] = [240, 243, 248];
const MUTED: [u8; 3] = [170, 178, 192];
const SEP: [u8; 3] = [96, 104, 118];
const GREEN: [u8; 3] = [138, 236, 72];
const YELLOW: [u8; 3] = [245, 208, 66];
const ORANGE: [u8; 3] = [246, 134, 54];
const RED: [u8; 3] = [242, 76, 66];

fn asked() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_PANEL"))
}

/// Whether it is shown now.
fn shown() -> &'static AtomicBool {
    static SHOWN: OnceLock<AtomicBool> = OnceLock::new();
    SHOWN.get_or_init(|| AtomicBool::new(asked()))
}

/// F3 was pressed since the last call, with one of our windows in front.
fn pressed() -> bool {
    static DOWN: AtomicBool = AtomicBool::new(false);
    // SAFETY: a plain query.
    let down = unsafe { GetAsyncKeyState(i32::from(VK_F3)) } < 0;
    if DOWN.swap(down, Ordering::Relaxed) || !down {
        return false;
    }
    let mut process = 0;
    // SAFETY: plain queries; room for the answer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    process == std::process::id()
}

/// Seven rows of five dots, the leftmost the highest bit.
#[rustfmt::skip]
const fn glyph(c: u8) -> [u8; 7] {
    match c {
        b'0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        b'1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        b'3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        b'4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        b'5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        b'6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        b'7' => [0b11111, 0b00001, 0b00010, 0b00010, 0b00100, 0b00100, 0b00100],
        b'8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        b'9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        b'A' => [0b00100, 0b01010, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001],
        b'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        b'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        b'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        b'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        b'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        b'G' => [0b01111, 0b10000, 0b10000, 0b10011, 0b10001, 0b10001, 0b01111],
        b'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        b'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        b'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        b'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        b'N' => [0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001],
        b'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        b'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        b'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        b'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        b'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        b'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        b'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        b'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        b'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        b'.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        b'/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        b'-' => [0, 0, 0, 0b11111, 0, 0, 0],
        b'|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        b':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        b' ' => [0; 7],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

/// The box's size in pixels.
const fn size(scale: u32) -> (u32, u32) {
    (BASE_W * scale, BASE_H * scale)
}

/// Integer scale for the swap-chain height: 1x up to 1440p, 2x at 4K.
fn scale(height: u32) -> u32 {
    if height >= 1800 { 2 } else { 1 }
}

/// Bytes per pixel for a supported swap-chain format.
fn bpp(format: DXGI_FORMAT) -> Option<usize> {
    match format {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R10G10B10A2_UNORM => Some(4),
        DXGI_FORMAT_R16G16B16A16_FLOAT => Some(8),
        _ => None,
    }
}

/// IEEE 754 binary16 encoding for a non-negative float in `[0, 1]`.
fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127;
    let mant = bits & 0x7f_ffff;
    if exp < -14 {
        return 0;
    }
    let h_exp = ((exp + 15).clamp(0, 31) as u16) << 10;
    let h_mant = (mant >> 13) as u16;
    h_exp | h_mant
}

/// Encodes an RGB colour into `dst` (length == `bpp(format)`).
fn encode(format: DXGI_FORMAT, [r, g, b]: [u8; 3], dst: &mut [u8]) {
    match format {
        DXGI_FORMAT_R8G8B8A8_UNORM => dst[..4].copy_from_slice(&[r, g, b, 0xFF]),
        DXGI_FORMAT_B8G8R8A8_UNORM => dst[..4].copy_from_slice(&[b, g, r, 0xFF]),
        DXGI_FORMAT_R10G10B10A2_UNORM => {
            let r10 = (u32::from(r) * 1023 / 255) & 0x3FF;
            let g10 = (u32::from(g) * 1023 / 255) & 0x3FF;
            let b10 = (u32::from(b) * 1023 / 255) & 0x3FF;
            let word = r10 | (g10 << 10) | (b10 << 20) | (0x3 << 30);
            dst[..4].copy_from_slice(&word.to_le_bytes());
        }
        DXGI_FORMAT_R16G16B16A16_FLOAT => {
            let to_h = |c: u8| {
                let s = f32::from(c) / 255.0;
                // Linearize roughly for HDR/scRGB swap chains.
                f32_to_f16(s * s)
            };
            dst[0..2].copy_from_slice(&to_h(r).to_le_bytes());
            dst[2..4].copy_from_slice(&to_h(g).to_le_bytes());
            dst[4..6].copy_from_slice(&to_h(b).to_le_bytes());
            dst[6..8].copy_from_slice(&0x3C00u16.to_le_bytes());
        }
        _ => {}
    }
}

/// Simple RGB software canvas in logical coordinates (`BASE_W x BASE_H`).
struct Canvas {
    w: usize,
    h: usize,
    rgb: Vec<[u8; 3]>,
}

impl Canvas {
    fn new(w: u32, h: u32, fill: [u8; 3]) -> Canvas {
        Canvas { w: w as usize, h: h as usize, rgb: vec![fill; (w * h) as usize] }
    }

    fn rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
        let x1 = (x + w).min(self.w as u32) as usize;
        let y1 = (y + h).min(self.h as u32) as usize;
        for yy in (y as usize).min(y1)..y1 {
            self.rgb[yy * self.w + (x as usize).min(x1)..yy * self.w + x1].fill(color);
        }
    }

    fn border(&mut self, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
        if w == 0 || h == 0 {
            return;
        }
        self.rect(x, y, w, 1, color);
        self.rect(x, y + h - 1, w, 1, color);
        self.rect(x, y, 1, h, color);
        self.rect(x + w - 1, y, 1, h, color);
    }

    /// Draws `text` at `(x, y)` with integer glyph pixel size `px` (`2` => 10x14).
    /// Returns the ending `x` coordinate.
    fn text(&mut self, mut x: u32, y: u32, px: u32, color: [u8; 3], text: &str) -> u32 {
        let step = (GLYPH.0 + 1) * px;
        for ch in text.bytes() {
            let bits = glyph(ch.to_ascii_uppercase());
            for (row, &mask) in bits.iter().enumerate() {
                if mask == 0 {
                    continue;
                }
                for col in 0..GLYPH.0 {
                    if (mask >> (GLYPH.0 - 1 - col)) & 1 != 0 {
                        self.rect(x + col * px, y + row as u32 * px, px, px, color);
                    }
                }
            }
            x += step;
        }
        x
    }

    /// Draws `text` with `|` separators rendered in `SEP`.
    fn text_sep(&mut self, mut x: u32, y: u32, px: u32, color: [u8; 3], text: &str) -> u32 {
        for (i, part) in text.split('|').enumerate() {
            if i > 0 {
                x = self.text(x, y, px, SEP, "|");
            }
            x = self.text(x, y, px, color, part);
        }
        x
    }

    /// Packs the logical canvas scaled by `scale` into swap-chain bytes.
    fn pack(&self, scale: u32, format: DXGI_FORMAT, pitch: usize) -> Vec<u8> {
        let bytes = bpp(format).unwrap_or(4);
        let s = scale as usize;
        let out_h = self.h * s;
        let mut out = vec![0u8; pitch * out_h];
        let mut enc = [0u8; 8];
        for y in 0..self.h {
            let top = (y * s) * pitch;
            for x in 0..self.w {
                encode(format, self.rgb[y * self.w + x], &mut enc[..bytes]);
                for i in 0..s {
                    let at = top + (x * s + i) * bytes;
                    out[at..at + bytes].copy_from_slice(&enc[..bytes]);
                }
            }
            for i in 1..s {
                out.copy_within(top..top + pitch, top + i * pitch);
            }
        }
        out
    }
}

/// Private committed bytes of the current process.
fn process_private_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    c.PagefileUsage as u64
}

/// CPU brand name from CPUID (`0x80000002..=0x80000004`), trimmed.
fn cpu_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            let mut raw = [0u8; 48];
            for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
                let r = std::arch::x86_64::__cpuid(leaf);
                raw[i * 16..i * 16 + 4].copy_from_slice(&r.eax.to_le_bytes());
                raw[i * 16 + 4..i * 16 + 8].copy_from_slice(&r.ebx.to_le_bytes());
                raw[i * 16 + 8..i * 16 + 12].copy_from_slice(&r.ecx.to_le_bytes());
                raw[i * 16 + 12..i * 16 + 16].copy_from_slice(&r.edx.to_le_bytes());
            }
            let s = String::from_utf8_lossy(&raw);
            let clean = s
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .replace("8-Core Processor", "")
                .replace("16-Core Processor", "")
                .replace("12-Core Processor", "")
                .replace("6-Core Processor", "")
                .replace("Processor", "");
            clean.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            "CPU".to_owned()
        }
    })
}

/// Adapter name and dedicated VRAM size from DXGI, queried once.
fn gpu_info(host: &Host) -> &'static (String, u64) {
    static INFO: OnceLock<(String, u64)> = OnceLock::new();
    INFO.get_or_init(|| {
        let Some(adapter) = host.adapter.as_ref() else {
            return ("GPU".to_owned(), 0);
        };
        // SAFETY: plain DXGI descriptor query.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            return ("GPU".to_owned(), 0);
        };
        let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..end]).trim().to_owned();
        (if name.is_empty() { "GPU".to_owned() } else { name }, desc.DedicatedVideoMemory as u64)
    })
}

/// System RAM: `(used_phys, total_phys, used_commit, total_commit)` in bytes.
fn system_memory() -> (u64, u64, u64, u64) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain sized query.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return (0, 0, 0, 0);
    }
    (
        m.ullTotalPhys.saturating_sub(m.ullAvailPhys),
        m.ullTotalPhys,
        m.ullTotalPageFile.saturating_sub(m.ullAvailPageFile),
        m.ullTotalPageFile,
    )
}

/// Per-logical-CPU utilization tracker via `NtQuerySystemInformation(8)`.
#[derive(Default)]
struct CpuMeter {
    prev: Vec<(i64, i64)>,
    total_pct: f32,
    cores_pct: Vec<f32>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SysProcPerfInfo {
    idle: i64,
    kernel: i64,
    user: i64,
    dpc: i64,
    interrupt: i64,
    interrupt_count: u32,
    _pad: u32,
}

impl CpuMeter {
    fn sample(&mut self) {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        type NtQuery = unsafe extern "system" fn(u32, *mut u8, u32, *mut u32) -> i32;
        static FN: OnceLock<Option<NtQuery>> = OnceLock::new();
        let query = *FN.get_or_init(|| unsafe {
            let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            if ntdll.is_null() {
                return None;
            }
            let p = GetProcAddress(ntdll, c"NtQuerySystemInformation".as_ptr().cast())?;
            Some(std::mem::transmute::<_, NtQuery>(p))
        });
        let Some(query) = query else { return };
        let n = std::thread::available_parallelism().map_or(8, |n| n.get()).min(64);
        let mut buf = vec![SysProcPerfInfo::default(); n];
        let mut ret = 0u32;
        // SystemProcessorPerformanceInformation = 8.
        let status = unsafe {
            query(8, buf.as_mut_ptr().cast(), (buf.len() * size_of::<SysProcPerfInfo>()) as u32, &mut ret)
        };
        if status < 0 {
            return;
        }
        let count = (ret as usize / size_of::<SysProcPerfInfo>()).min(buf.len());
        if count == 0 {
            return;
        }
        let now: Vec<(i64, i64)> = buf[..count].iter().map(|p| (p.idle, p.kernel + p.user)).collect();
        if self.prev.len() == count {
            self.cores_pct.clear();
            let mut sum = 0.0f32;
            for (&(pi, pt), &(ni, nt)) in self.prev.iter().zip(&now) {
                let dt = (nt - pt).max(1) as f32;
                let di = (ni - pi).clamp(0, nt - pt) as f32;
                let busy = ((1.0 - di / dt) * 100.0).clamp(0.0, 100.0);
                self.cores_pct.push(busy);
                sum += busy;
            }
            self.total_pct = sum / count as f32;
        }
        self.prev = now;
    }
}

/// The counters read for it. They grow, and go back to 0 with the frame-rate
/// line (right behind a look of ours).
#[derive(Clone, Copy, Default)]
struct Counters([u64; COUNTERS]);

const COUNTERS: usize = 11;

impl Counters {
    fn now() -> Counters {
        use super::{CARD_CLOCK_US, COPY_BACK, PREPARING};
        let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
        #[cfg(feature = "diagnostics")]
        let record_ns = crate::copystat::peek(crate::copystat::C::RecordNs);
        #[cfg(not(feature = "diagnostics"))]
        let record_ns = 0;
        Counters([
            super::worked(),
            get(&CARD_CLOCK_US[0]),
            get(&CARD_CLOCK_US[1]),
            get(&CARD_CLOCK_US[2]),
            get(&PREPARING[3]),
            get(&COPY_BACK[0]),
            get(&COPY_BACK[1]),
            get(&crate::shadow::CARD_STATS[0]),
            crate::draw::dropped_now(),
            record_ns,
            crate::draw::drawn(),
        ])
    }

    /// What came since `before`, per frame.
    fn since(self, before: Counters, frames: f64) -> [f64; COUNTERS] {
        let mut out = [0.0; COUNTERS];
        for (i, out) in out.iter_mut().enumerate() {
            let (was, is) = (before.0[i], self.0[i]);
            *out = if is >= was { is - was } else { is } as f64 / frames;
        }
        out
    }
}

/// Snapshot of data rendered onto the HUD canvas.
struct Snapshot {
    fps: f64,
    frame_ms: f64,
    gpu_ms: f64,
    gpu_pct: u32,
    submit_ms: f64,
    low1_fps: f64,
    max_ms: f64,
    draws: u64,
    dropped: u64,
    width: u32,
    height: u32,
    gpu_name: String,
    vram_used_gb: f64,
    vram_budget_gb: f64,
    vram_shared_gb: f64,
    vram_rt_gb: f64,
    vram_tex_gb: f64,
    vram_buf_gb: f64,
    vram_bvh_gb: f64,
    vram_oth_gb: f64,
    cpu_name: String,
    cpu_pct: f32,
    cores_pct: Vec<f32>,
    proc_ram_gb: f64,
    sys_ram_used_gb: f64,
    sys_ram_total_gb: f64,
    sys_commit_gb: f64,
    history_ms: Vec<f32>,
}

fn fps_color(fps: f64) -> [u8; 3] {
    if fps >= 25.0 {
        GREEN
    } else if fps >= 14.0 {
        YELLOW
    } else {
        ORANGE
    }
}

fn frametime_color(ms: f32) -> [u8; 3] {
    if ms <= 38.0 {
        GREEN
    } else if ms <= 72.0 {
        YELLOW
    } else if ms <= 115.0 {
        ORANGE
    } else {
        RED
    }
}

fn trunc(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_owned() } else { s[..max].to_owned() }
}

/// Paints the full colour HUD canvas.
fn paint_hud(s: &Snapshot) -> Canvas {
    let mut c = Canvas::new(BASE_W, BASE_H, BG);
    c.border(0, 0, BASE_W, BASE_H, BORDER);

    // 1. Large coloured FPS header.
    let fps_str = format!("{:.0} FPS", s.fps.round());
    c.text(14, 12, 4, fps_color(s.fps), &fps_str);
    // Subtle FPS decimal & ms badge on the right of the header.
    let badge = format!("{:.1} FPS", s.fps);
    c.text(BASE_W - 14 - badge.len() as u32 * 12, 24, 2, MUTED, &badge);

    // 2. Resolution | draws | GPU ms.
    let row2 = format!("{}X{} | {} DRAWS | GPU {:.1}MS", s.width, s.height, s.draws, s.gpu_ms);
    c.text_sep(14, 48, 2, WHITE, &trunc(&row2, 43));

    // 3. Frame | submit | 1% low | max.
    let row3 = format!(
        "FRAME {:.1} | SUB {:.1} | 1% {:.0} | MAX {:.0}",
        s.frame_ms, s.submit_ms, s.low1_fps, s.max_ms
    );
    c.text_sep(14, 68, 2, MUTED, &trunc(&row3, 43));

    // 4. Live frame-time sparkline graph.
    let (gx, gy, gw, gh) = (12u32, 90u32, BASE_W - 24, 76u32);
    c.rect(gx, gy, gw, gh, GRAPH_BG);
    c.border(gx, gy, gw, gh, BORDER);

    // Scale graph ceiling dynamically (min 100 ms, up to 200 ms if spikes exist).
    let peak = s.history_ms.iter().copied().fold(100.0f32, f32::max).min(220.0);
    let max_scale = if peak > 140.0 { 200.0 } else if peak > 90.0 { 133.0 } else { 100.0 };

    // Reference lines at 33 ms (30 FPS) and 66 ms (15 FPS).
    for &ref_ms in &[33.3f32, 66.7f32] {
        if ref_ms < max_scale {
            let ly = gy + gh - 2 - ((ref_ms / max_scale) * (gh - 4) as f32) as u32;
            c.rect(gx + 1, ly, gw - 2, 1, GRAPH_GRID);
            c.text(gx + 4, ly.saturating_sub(8), 1, MUTED, &format!("{:.0}", ref_ms));
        }
    }

    // Bars (3 px wide each, right-aligned so newest frame is on the right).
    let bar_w = 3u32;
    let slots = (gw - 4) / bar_w;
    let n = s.history_ms.len().min(slots as usize);
    let start_x = gx + gw - 2 - (n as u32) * bar_w;
    for (i, &ms) in s.history_ms[s.history_ms.len() - n..].iter().enumerate() {
        let frac = (ms / max_scale).clamp(0.02, 1.0);
        let h = ((frac * (gh - 4) as f32) as u32).max(2);
        let bx = start_x + i as u32 * bar_w;
        let by = gy + gh - 2 - h;
        let top_col = frametime_color(ms);
        // Warm base fill + crisp coloured cap/spike (matches reference style).
        let cap = h.min(if ms > 72.0 { h } else { 5 });
        if h > cap {
            c.rect(bx, by + cap, bar_w - 1, h - cap, GRAPH_FLOOR);
        }
        c.rect(bx, by, bar_w - 1, cap, top_col);
    }

    // 5. Hardware & memory rows.
    let val_x = 104u32;
    let max_chars = ((BASE_W - val_x - 10) / 12) as usize;

    // GPU
    c.text(14, 176, 2, LABEL, "GPU");
    let gpu_line = format!("{} {}%", trunc(&s.gpu_name, max_chars.saturating_sub(5)), s.gpu_pct);
    c.text(val_x, 176, 2, WHITE, &trunc(&gpu_line, max_chars));

    // VRAM
    c.text(14, 196, 2, LABEL, "VRAM");
    let vram_col = if s.vram_budget_gb > 0.0 && s.vram_used_gb / s.vram_budget_gb >= 0.92 {
        ORANGE
    } else {
        WHITE
    };
    let vram_line = format!(
        "{:.1}/{:.1} GB | SHR {:.1} GB",
        s.vram_used_gb, s.vram_budget_gb, s.vram_shared_gb
    );
    c.text_sep(val_x, 196, 2, vram_col, &trunc(&vram_line, max_chars));

    // VRAM breakdown (Render Targets | Textures/Pool | Card Buffers | BVH Arenas | Other/Driver)
    let vram_detail = format!(
        "RT {:.1} | TX {:.1} | BF {:.1} | BV {:.1} | O {:.1}",
        s.vram_rt_gb, s.vram_tex_gb, s.vram_buf_gb, s.vram_bvh_gb, s.vram_oth_gb
    );
    c.text_sep(14, 216, 2, MUTED, &trunc(&vram_detail, 43));

    // CPU + per-core bars
    c.text(14, 236, 2, LABEL, "CPU");
    let cpu_line = format!("{} {:.0}%", trunc(&s.cpu_name, max_chars.saturating_sub(5)), s.cpu_pct);
    c.text(val_x, 236, 2, WHITE, &trunc(&cpu_line, max_chars));

    let cores = s.cores_pct.len().clamp(1, 32);
    let area_w = BASE_W - val_x - 14;
    let gap = 2u32;
    let cw = ((area_w + gap) / cores as u32).saturating_sub(gap).max(4);
    let bar_h = 22u32;
    let bar_y = 256u32;
    for (i, &pct) in s.cores_pct.iter().take(cores).enumerate() {
        let bx = val_x + i as u32 * (cw + gap);
        c.rect(bx, bar_y, cw, bar_h, BAR_BG);
        let fh = ((pct / 100.0) * bar_h as f32).round() as u32;
        if fh > 0 {
            let fh = fh.min(bar_h);
            let col = if pct >= 85.0 { BAR_FILL_HI } else { BAR_FILL_LO };
            c.rect(bx, bar_y + bar_h - fh, cw, fh, col);
        }
    }

    // VICTORY
    c.text(14, 288, 2, LABEL, "VICTORY");
    let vic_line = format!("RAM {:.1} GB | DROP {}", s.proc_ram_gb, s.dropped);
    c.text_sep(val_x, 288, 2, WHITE, &trunc(&vic_line, max_chars));

    // RAM
    c.text(14, 308, 2, LABEL, "RAM");
    let ram_line = format!(
        "{:.1}/{:.1} GB | CMT {:.1}",
        s.sys_ram_used_gb, s.sys_ram_total_gb, s.sys_commit_gb
    );
    c.text_sep(val_x, 308, 2, WHITE, &trunc(&ram_line, max_chars));

    c
}

/// The buffers of a box that size and format.
struct Buffers {
    format: DXGI_FORMAT,
    scale: u32,
    pitch: u32,
    uploads: Vec<(ID3D12Resource, *mut u8)>,
    card: ID3D12Resource,
    /// The upload written last.
    turn: usize,
    /// It is not in the card's buffer yet.
    fresh: bool,
    /// `card` has already been copied into once and now sits in `COPY_SOURCE`.
    uploaded: bool,
}

// SAFETY: free-threaded resources, and memory only written under the presenter's lock.
unsafe impl Send for Buffers {}

impl Buffers {
    fn new(host: &Host, format: DXGI_FORMAT, scale: u32) -> Option<Buffers> {
        let (width, height) = size(scale);
        let pitch = (width * bpp(format)? as u32).next_multiple_of(256);
        let bytes = u64::from(pitch) * u64::from(height);
        let uploads = (0..UPLOADS).map(|_| host.upload_buffer(bytes)).collect::<Option<Vec<_>>>()?;
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
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
        let mut card: Option<ID3D12Resource> = None;
        // SAFETY: plain creation of a buffer.
        unsafe {
            host.device
                .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut card)
                .ok()?;
        }
        Some(Buffers { format, scale, pitch, uploads, card: card?, turn: 0, fresh: false, uploaded: false })
    }

    fn write(&mut self, snap: &Snapshot) {
        let picture = paint_hud(snap).pack(self.scale, self.format, self.pitch as usize);
        self.turn = (self.turn + 1) % UPLOADS;
        // SAFETY: a mapped upload buffer of the picture's size, which the card read a second or more ago.
        unsafe { std::ptr::copy_nonoverlapping(picture.as_ptr(), self.uploads[self.turn].1, picture.len()) };
        self.fresh = true;
    }
}

/// The panel of a presenter.
pub(super) struct Panel {
    since: Instant,
    frames: u32,
    last: Instant,
    worst: f64,
    /// What this took since the last text.
    cost: Duration,
    before: Counters,
    history: VecDeque<f32>,
    cpu: CpuMeter,
    last_size: (u32, u32),
    snap: Option<Snapshot>,
    changed: bool,
    buffers: Option<Buffers>,
    failed: bool,
}

impl Panel {
    pub(super) fn new() -> Panel {
        let now = Instant::now();
        Panel {
            since: now,
            frames: 0,
            last: now,
            worst: 0.0,
            cost: Duration::ZERO,
            before: Counters::default(),
            history: VecDeque::with_capacity(HISTORY),
            cpu: CpuMeter::default(),
            last_size: (0, 0),
            snap: None,
            changed: false,
            buffers: None,
            failed: false,
        }
    }

    /// A frame was presented; `line`: the counters go back to 0 right behind
    /// this call. (Not shown it asks for the key's state and no more.)
    pub(super) fn frame(&mut self, line: bool) {
        if pressed() {
            shown().fetch_xor(true, Ordering::Relaxed);
        }
        if !shown().load(Ordering::Relaxed) {
            self.frames = 0;
            return;
        }
        let now = Instant::now();
        let dt_ms = (now - self.last).as_secs_f64() * 1e3;
        if self.frames == 0 && now - self.last > 2 * EVERY {
            (self.since, self.last, self.worst, self.before) = (now, now, 0.0, Counters::now());
            self.frames = 0;
            self.cpu.sample();
        } else {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(dt_ms as f32);
            self.worst = self.worst.max(dt_ms);
            self.last = now;
        }
        self.frames += 1;
        let elapsed = now - self.since;
        if elapsed < EVERY && !(line && elapsed * 2 >= EVERY) {
            return;
        }
        let Some(host) = super::host() else { return };
        self.cpu.sample();
        let frames = f64::from(self.frames);
        let counters = Counters::now();
        let c = counters.since(self.before, frames);
        self.before = counters;

        let ms = elapsed.as_secs_f64() * 1e3 / frames;
        let fps = 1e3 / ms.max(0.001);
        let idle = (c[1] / 1e3).min(ms);
        let gpu_ms = (ms - idle).max(0.0);
        let gpu_pct = ((gpu_ms / ms.max(0.001)) * 100.0).clamp(0.0, 100.0).round() as u32;
        let submit_ms = c[0] / 1e3;

        // 1% low FPS from recent history.
        let low1_fps = if !self.history.is_empty() {
            let mut sorted: Vec<f32> = self.history.iter().copied().collect();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let idx = (sorted.len() * 99 / 100).min(sorted.len() - 1);
            1e3 / f64::from(sorted[idx]).max(0.001)
        } else {
            fps
        };

        let gb = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
        let private = process_private_bytes();
        let [budget, used, _, shared] = host.video_memory().unwrap_or_default();
        let all_tex = crate::resource::HOST_TEXTURES[1].load(Ordering::Relaxed);
        let filled_tex = crate::resource::KINDS[1].load(Ordering::Relaxed);
        let rt_bytes = all_tex.saturating_sub(filled_tex);
        let pool_bytes = crate::pool::memory()[0] << 20;
        let tex_bytes = filled_tex.max(pool_bytes) + crate::tiles::given_total();
        let [buf_bytes, bvh_bytes] = crate::shadow::vram_breakdown();
        let known = rt_bytes + tex_bytes + buf_bytes + bvh_bytes;
        let oth_bytes = used.saturating_sub(known);
        let (sys_used, sys_total, sys_commit, _) = system_memory();
        let (gpu_name, _) = gpu_info(host);

        self.snap = Some(Snapshot {
            fps,
            frame_ms: ms,
            gpu_ms,
            gpu_pct,
            submit_ms,
            low1_fps,
            max_ms: self.worst,
            draws: c[10].round() as u64,
            dropped: c[8].round() as u64,
            width: self.last_size.0,
            height: self.last_size.1,
            gpu_name: gpu_name.clone(),
            vram_used_gb: gb(used),
            vram_budget_gb: gb(budget),
            vram_shared_gb: gb(shared),
            vram_rt_gb: gb(rt_bytes),
            vram_tex_gb: gb(tex_bytes),
            vram_buf_gb: gb(buf_bytes),
            vram_bvh_gb: gb(bvh_bytes),
            vram_oth_gb: gb(oth_bytes),
            cpu_name: cpu_name().to_owned(),
            cpu_pct: self.cpu.total_pct,
            cores_pct: self.cpu.cores_pct.clone(),
            proc_ram_gb: gb(private),
            sys_ram_used_gb: gb(sys_used),
            sys_ram_total_gb: gb(sys_total),
            sys_commit_gb: gb(sys_commit),
            history_ms: self.history.iter().copied().collect(),
        });
        self.changed = true;
        (self.since, self.frames, self.worst) = (now, 0, 0.0);
        self.cost = now.elapsed();
    }

    /// Records the copy of the box onto `back`, a buffer of the swap chain of
    /// that size and format in the state to be copied into.
    pub(super) fn draw(
        &mut self,
        host: &Host,
        list: &HostList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
    ) {
        self.last_size = (width, height);
        if !shown().load(Ordering::Relaxed) || self.failed {
            return;
        }
        let Some(snap) = self.snap.as_ref() else {
            return;
        };
        let began = Instant::now();
        // Not the title's: the frame recorder leaves it out.
        #[cfg(feature = "diagnostics")]
        let _ours = crate::replay::record::unrecorded();
        let scale = scale(height);
        if self.buffers.as_ref().is_none_or(|b| (b.format, b.scale) != (format, scale)) {
            // (What the card may still read stays: a picture's size changes once or twice in a run.)
            std::mem::forget(self.buffers.take());
            self.buffers = Buffers::new(host, format, scale);
            self.failed = self.buffers.is_none();
            self.changed = true;
        }
        let Some(buffers) = &mut self.buffers else { return };
        if std::mem::take(&mut self.changed) {
            buffers.write(snap);
        }
        let (w, h) = size(scale);
        let list = &list.list;
        // SAFETY: an open list and live resources; the box lies within both.
        unsafe {
            if std::mem::take(&mut buffers.fresh) {
                if buffers.uploaded {
                    list.ResourceBarrier(&[transition(
                        &buffers.card,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    )]);
                }
                list.CopyResource(&buffers.card, &buffers.uploads[buffers.turn].0);
                list.ResourceBarrier(&[transition(
                    &buffers.card,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                )]);
                buffers.uploaded = true;
            }
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&buffers.card),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: format,
                            Width: w,
                            Height: h,
                            Depth: 1,
                            RowPitch: buffers.pitch,
                        },
                    },
                },
            };
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(back),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            let part = D3D12_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: w.min(width),
                bottom: h.min(height),
                back: 1,
            };
            list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&part));
        }
        self.cost += began.elapsed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the HUD uses has its own picture, and no two look alike.
    #[test]
    fn characters_differ() {
        let used: Vec<u8> = (b'0'..=b'9').chain(b'A'..=b'Z').chain(*b"./-|%: ").collect();
        for (i, a) in used.iter().enumerate() {
            assert_ne!(glyph(*a), glyph(b'#'), "{} has a picture", *a as char);
            assert!(glyph(*a).iter().all(|row| row >> GLYPH.0 == 0));
            for b in &used[i + 1..] {
                assert_ne!(glyph(*a), glyph(*b), "{} {}", *a as char, *b as char);
            }
        }
    }

    #[test]
    fn a_box_is_painted() {
        let snap = Snapshot {
            fps: 32.0,
            frame_ms: 31.2,
            gpu_ms: 28.7,
            gpu_pct: 92,
            submit_ms: 19.8,
            low1_fps: 14.0,
            max_ms: 95.0,
            draws: 8903,
            dropped: 0,
            width: 2560,
            height: 1440,
            gpu_name: "AMD RADEON RX 9070 XT".into(),
            vram_used_gb: 11.4,
            vram_budget_gb: 13.2,
            vram_shared_gb: 4.2,
            vram_rt_gb: 2.8,
            vram_tex_gb: 3.5,
            vram_buf_gb: 1.2,
            vram_bvh_gb: 0.8,
            vram_oth_gb: 3.1,
            cpu_name: "AMD RYZEN 7 9800X3D".into(),
            cpu_pct: 34.0,
            cores_pct: vec![60.0, 40.0, 90.0, 20.0],
            proc_ram_gb: 32.4,
            sys_ram_used_gb: 41.2,
            sys_ram_total_gb: 61.4,
            sys_commit_gb: 48.0,
            history_ms: vec![28.0, 31.0, 85.0, 32.0],
        };
        let canvas = paint_hud(&snap);
        for scale in [1u32, 2] {
            let (width, height) = size(scale);
            let pitch = (width * 4).next_multiple_of(256) as usize;
            let packed = canvas.pack(scale, DXGI_FORMAT_R10G10B10A2_UNORM, pitch);
            assert_eq!(packed.len(), pitch * height as usize);
            let mut border = [0u8; 4];
            encode(DXGI_FORMAT_R10G10B10A2_UNORM, BORDER, &mut border);
            assert_eq!(&packed[0..4], &border);
        }
    }

    #[test]
    fn counters_that_went_back_to_nothing_count_from_there() {
        let (mut a, mut b) = (Counters::default(), Counters::default());
        (a.0[0], b.0[0]) = (10, 30);
        (a.0[1], b.0[1]) = (50, 20);
        let c = b.since(a, 10.0);
        assert_eq!((c[0], c[1]), (2.0, 2.0));
    }
}
