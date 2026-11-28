//! (Neptune) Texture detiling on the card (`VICTORY_GPU_DETILE=1`; off by default).
//!
//! Without it every title texture a list samples has a staging buffer of its own in upload
//! memory (system memory, the size of the texture, kept for as long as the texture is used:
//! 9-12 GB of them in the open world), and the submitting thread detiles title memory into it
//! on the CPU. With it a texture's upload is: title memory read once (and hashed, as the
//! refresh hashes it), copied as it is into a slice of a ring of upload memory, and detiled
//! by a compute shader into a slice of a ring in the card's memory, laid out as the texture's
//! copyable footprints; the list's `CopyTextureRegion`s read that slice. Nothing is kept per
//! texture but the staging state's bookkeeping.
//!
//! The shader copies by table (`victory_formats::texture::Layout::copy_table`): the runs the
//! CPU's detile copies, the column runs once and the rows once, cached by layout, mip and
//! pitch in a buffer of upload memory. An array slice of a thin layout is slice 0's table
//! moved by a whole slice (`shift`, a root constant), so one table serves every slice.
//!
//! Safety of the card: both rings are fixed in size (`VICTORY_GPU_DETILE_RAW_MB`, 256;
//! `VICTORY_GPU_DETILE_OUT_MB`, 192, made only while the card's budget has room for it), a
//! slice is reserved when the list records the upload and given back only when the list has
//! let it go and the card has run every submission that used it. Whatever does not fit (a
//! ring full, the table buffer full, a table not in whole 32-bit words, a table reaching
//! outside its slices) goes the CPU's way, at once: nothing waits, nothing is overrun. Every
//! table is checked against its slices on the CPU, and the shader checks every word again.
//!
//! `VICTORY_GPU_DETILE_VERIFY=<n>`: up to `n` uploads a second are read back and compared
//! with the CPU's detile of the same bytes; a difference is logged with the texture.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};

use victory_formats::texture::{CopyTable, Dimension, Layout};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

/// Whether textures are detiled on the card (`VICTORY_GPU_DETILE=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_GPU_DETILE").is_some_and(|v| v == "1"))
}

/// Uploads a second read back and compared (`VICTORY_GPU_DETILE_VERIFY`, 0: none).
fn verify_per_second() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| std::env::var("VICTORY_GPU_DETILE_VERIFY").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0))
}

/// Megabytes from `var`, `default` unset, clamped to `[least, most]`, in bytes.
fn mb(var: &str, default: u64, least: u64, most: u64) -> u64 {
    std::env::var(var).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default).clamp(least, most) << 20
}

/// Why an upload went the CPU's way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// The rings or the shader could not be made.
    NotReady,
    /// A table not in whole 32-bit words (narrow levels of 1- and 2-byte formats).
    Unaligned,
    /// The layout has no table (`copy_table` refused it).
    NoTable,
    /// The buffer of tables is full.
    Tables,
    /// A table reaches outside the slices it would be given (never expected).
    Bounds,
    /// The upload is larger than a ring.
    TooBig,
    /// The ring of upload memory has no room now.
    RawFull,
    /// The ring in the card's memory has no room now.
    OutFull,
}

/// Since the start: uploads detiled on the card and their bytes of title memory; uploads that
/// went the CPU's way, by [`Why`] (8); refills left out because the slices were still in the
/// air (a list submitted again); verified uploads and those that differed.
pub(crate) static STATS: [AtomicU64; 14] = [const { AtomicU64::new(0) }; 14];
const DONE: usize = 0;
const DONE_BYTES: usize = 1;
const WHY_AT: usize = 2;
const REUSED: usize = 10;
const CHECKED: usize = 11;
const DIFFERED: usize = 12;
const DISPATCHES: usize = 13;

pub(crate) fn refused(why: Why) {
    STATS[WHY_AT + why as usize].fetch_add(1, Relaxed);
}

// ---------------------------------------------------------------------------------------------
// The allocator of both rings (no D3D12).

/// Granularity of the rings' slices: a texture copy's placement wants 512 bytes, a root view
/// 4; whole 64 KiB keeps the free list short.
pub(crate) const GRAIN: u64 = 64 << 10;

/// First-fit allocation of `size` bytes in `GRAIN`s. A slice given back with a fence value is
/// free once the card has passed it ([`Arena::reclaim`]); slices go back in any order.
#[derive(Debug)]
pub(crate) struct Arena {
    size: u64,
    /// Free ranges: start, length; never touching (joined on release).
    free: BTreeMap<u64, u64>,
    /// Given back, waiting for the card: start, length, fence value.
    waiting: Vec<(u64, u64, u64)>,
    used: u64,
}

impl Arena {
    pub(crate) fn new(size: u64) -> Arena {
        let size = size / GRAIN * GRAIN;
        let mut free = BTreeMap::new();
        if size > 0 {
            free.insert(0, size);
        }
        Arena { size, free, waiting: Vec::new(), used: 0 }
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Bytes reserved or waiting for the card.
    pub(crate) fn used(&self) -> u64 {
        self.used
    }

    /// The start of `len` bytes (rounded up to `GRAIN`), or `None` if no free range holds
    /// them now. Never waits.
    pub(crate) fn alloc(&mut self, len: u64) -> Option<u64> {
        let len = len.max(1).checked_next_multiple_of(GRAIN)?;
        let (&at, &have) = self.free.iter().find(|&(_, &have)| have >= len)?;
        self.free.remove(&at);
        if have > len {
            self.free.insert(at + len, have - len);
        }
        self.used += len;
        Some(at)
    }

    /// Gives `[at, at + len)` back once the card has passed `fence` (0: now).
    pub(crate) fn release(&mut self, at: u64, len: u64, fence: u64) {
        let len = len.max(1).next_multiple_of(GRAIN);
        if fence == 0 {
            self.insert_free(at, len);
        } else {
            self.waiting.push((at, len, fence));
        }
    }

    /// Frees what waits for fence values up to `completed`.
    pub(crate) fn reclaim(&mut self, completed: u64) {
        let mut i = 0;
        while i < self.waiting.len() {
            let (at, len, fence) = self.waiting[i];
            if fence <= completed {
                self.waiting.swap_remove(i);
                self.insert_free(at, len);
            } else {
                i += 1;
            }
        }
    }

    fn insert_free(&mut self, mut at: u64, mut len: u64) {
        debug_assert!(at + len <= self.size);
        self.used -= len;
        if let Some((&before, &blen)) = self.free.range(..at).next_back()
            && before + blen == at
        {
            self.free.remove(&before);
            at = before;
            len += blen;
        }
        if let Some(&alen) = self.free.get(&(at + len)) {
            self.free.remove(&(at + len));
            len += alen;
        }
        self.free.insert(at, len);
    }
}

// ---------------------------------------------------------------------------------------------
// Tables as the shader reads them (no D3D12).

/// The most bytes one run of a packed table copies: one thread's work.
const RUN_MOST: u64 = 64;

/// A table packed for the shader: `words` are the runs (tiled, linear, bytes) and then the
/// rows (tiled, xor, linear), three 32-bit words each; `tiled_end` and `linear_end` bound
/// every byte it reads and writes (slice 0, no shift).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Packed {
    pub(crate) words: Vec<u32>,
    pub(crate) runs: u32,
    pub(crate) rows: u32,
    pub(crate) tiled_end: u64,
    pub(crate) linear_end: u64,
}

/// `table` packed for the shader, long runs cut into pieces of [`RUN_MOST`] bytes (where no
/// row flips bits, as the console's XOR modes do); `Err` if it is not in whole 32-bit words
/// or does not fit 32-bit offsets.
pub(crate) fn pack(table: &CopyTable) -> Result<Packed, Why> {
    if !table.aligned(4) {
        return Err(Why::Unaligned);
    }
    let flips = table.rows.iter().any(|r| r.xor != 0);
    let mut runs: Vec<(u64, u64, u64)> = Vec::with_capacity(table.runs.len());
    for r in &table.runs {
        let (tiled, linear, len) = (r.tiled, u64::from(r.linear), u64::from(r.len));
        if flips || len <= RUN_MOST {
            runs.push((tiled, linear, len));
            continue;
        }
        let mut k = 0;
        while k < len {
            runs.push((tiled + k, linear + k, RUN_MOST.min(len - k)));
            k += RUN_MOST;
        }
    }
    let tiled_end = if flips {
        table
            .rows
            .iter()
            .flat_map(|row| runs.iter().map(move |&(t, _, n)| row.tiled + (t ^ u64::from(row.xor)) + n))
            .max()
            .unwrap_or(0)
    } else {
        table.rows.iter().map(|r| r.tiled).max().unwrap_or(0) + runs.iter().map(|&(t, _, n)| t + n).max().unwrap_or(0)
    };
    let small = |v: u64| u32::try_from(v).map_err(|_| Why::TooBig);
    small(tiled_end)?;
    small(table.linear_bytes)?;
    let mut words = Vec::with_capacity(3 * (runs.len() + table.rows.len()));
    for &(t, l, n) in &runs {
        words.extend([small(t)?, small(l)?, small(n)?]);
    }
    for r in &table.rows {
        words.extend([small(r.tiled)?, r.xor, small(r.linear)?]);
    }
    Ok(Packed {
        words,
        runs: runs.len() as u32,
        rows: table.rows.len() as u32,
        tiled_end,
        linear_end: table.linear_bytes,
    })
}

/// The slice whose table serves array slice `slice` of `layout`, and the bytes the table is
/// moved by for it: slice 0 and a whole slice per slice for a thin layout without XOR bits
/// (its rows then differ by that alone), the slice itself otherwise.
pub(crate) fn table_slice(layout: &Layout, slice: u32) -> (u32, u64) {
    if layout.desc.dimension != Dimension::Tex3d && !layout.thick && !layout.desc.swizzle.xor() {
        (0, u64::from(slice) * layout.slice_size)
    } else {
        (slice, 0)
    }
}

/// What the shader does with a packed table, on the CPU: for tests, and the reference of the
/// shader's bounds (a word outside `raw` or `out` is left out, as on the card).
#[cfg(test)]
pub(crate) fn emulate(words: &[u32], runs: u32, rows: std::ops::Range<u32>, shift: u32, raw: &[u8], out: &mut [u8], out_at: u32) {
    let w = |i: u32| words[i as usize];
    let rows_at = 3 * runs;
    for row in rows {
        for r in 0..runs {
            let (t, l, n) = (w(3 * r), w(3 * r + 1), w(3 * r + 2));
            let (rt, rx, rl) = (w(rows_at + 3 * row), w(rows_at + 3 * row + 1), w(rows_at + 3 * row + 2));
            let src = rt + shift + (t ^ rx);
            let dst = out_at + rl + l;
            let mut k = 0;
            while k < n {
                let (s, d) = ((src + k) as usize, (dst + k) as usize);
                if s + 4 <= raw.len() && d + 4 <= out.len() {
                    out[d..d + 4].copy_from_slice(&raw[s..s + 4]);
                }
                k += 4;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The shader.

/// One job: copy by a table, or (`run_count` 0) zeros.
///
/// Root constants: `runs_at` and `rows_at` are word offsets into the table buffer; a group's
/// threads take runs (x) of one row (y, from `row_first`); `shift` moves every tiled offset
/// (a slice of a thin layout); `out_at` is the subresource's offset in the output slice;
/// `raw_len` and `out_len` bound the slices, which every word is checked against. Zeros:
/// `rows_at` words from `out_at` on.
pub(crate) const DETILE: &str = r#"
#define RS "RootConstants(num32BitConstants=8, b0), SRV(t0), SRV(t1), UAV(u0)"
cbuffer Job : register(b0) {
    uint runs_at;
    uint run_count;
    uint rows_at;
    uint row_first;
    uint shift;
    uint out_at;
    uint raw_len;
    uint out_len;
};
ByteAddressBuffer raw : register(t0);
ByteAddressBuffer table : register(t1);
RWByteAddressBuffer outb : register(u0);

[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (run_count == 0) {
        uint at = out_at + id.x * 4;
        if (id.x < rows_at && at + 4 <= out_len) {
            outb.Store(at, 0);
        }
        return;
    }
    if (id.x >= run_count) {
        return;
    }
    uint3 run = table.Load3((runs_at + id.x * 3) * 4);
    uint3 row = table.Load3((rows_at + (row_first + id.y) * 3) * 4);
    uint src = row.x + shift + (run.x ^ row.y);
    uint dst = out_at + row.z + run.y;
    for (uint k = 0; k < run.z; k += 4) {
        if (src + k + 4 <= raw_len && dst + k + 4 <= out_len) {
            outb.Store(dst + k, raw.Load(src + k));
        }
    }
}
"#;

/// Most groups along one axis of a dispatch.
const GROUPS_MOST: u32 = 65_535;

// ---------------------------------------------------------------------------------------------
// The rings, the tables and the jobs of a submission.

/// A table in the table buffer: where its words start, and its bounds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Table {
    at: u32,
    runs: u32,
    rows: u32,
    tiled_end: u64,
    linear_end: u64,
}

/// What one subresource of an upload is: its mip, where it lies in the output slice and how
/// many bytes, and its table (and the shift that moves the table to its slice).
#[derive(Debug, Clone)]
pub(crate) struct Part {
    pub(crate) mip: u32,
    offset: u64,
    bytes: u64,
    table: Table,
    shift: u32,
}

/// The slices of one recorded upload. Given back to the rings when the last list that holds
/// it lets it go, once the card has run every submission that used it.
pub(crate) struct Upload {
    raw: (u64, u64),
    out: (u64, u64),
    parts: Vec<Part>,
    /// The fence value of the last submission that used it (0: none yet).
    sent: AtomicU64,
}

impl Upload {
    /// Where the output slice starts in [`out_buffer`].
    pub(crate) fn out_offset(&self) -> u64 {
        self.out.0
    }

    /// Bytes of title memory the upload copies (its raw slice).
    pub(crate) fn raw_len(&self) -> u64 {
        self.raw.1
    }

    /// Whether a submission that used it may still be running: its slices may not be
    /// written now.
    pub(crate) fn in_flight(&self) -> bool {
        let sent = self.sent.load(Relaxed);
        sent != 0 && crate::host::host().is_none_or(|h| h.completed() < sent)
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        let sent = *self.sent.get_mut();
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = state.as_mut() {
            state.raw_arena.release(self.raw.0, self.raw.1, sent);
            state.out_arena.release(self.out.0, self.out.1, sent);
        }
    }
}

struct Job {
    constants: [u32; 8],
    raw: u64,
    out: u64,
    groups: (u32, u32),
}

/// An upload to read back and compare ([`verify_per_second`]): the output slice's bytes
/// (`len` from `at`), the CPU's detile of the same title bytes, and the ranges to compare.
struct Check {
    at: u64,
    len: u64,
    expected: Vec<u8>,
    ranges: Vec<(u64, u64)>,
    what: String,
    back: Option<(ID3D12Resource, usize)>,
    fence: u64,
}

struct State {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    raw: (ID3D12Resource, usize, u64),
    raw_arena: Arena,
    out: (ID3D12Resource, u64),
    out_arena: Arena,
    /// The table buffer: resource, mapping, address, bytes, bytes used.
    tables: (ID3D12Resource, usize, u64, u64, u64),
    cache: HashMap<Key, Result<Table, Why>>,
    jobs: Vec<Job>,
    /// Uploads this submission fills or uses again: told its fence value when it is sent.
    filled: Vec<Arc<Upload>>,
    /// Lists for the work before a submission: allocator, list, fence value of its last run
    /// (`u64::MAX`: recorded, not sent yet).
    lists: Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList, u64)>,
    checks: Vec<Check>,
    /// When a check was last taken, and how many in that second.
    checked: (u64, u64),
    /// (Neptune) The rings' full sizes (raw, card), when an upload last wanted more than the
    /// small rings give, and when a resize was last made ([`trim`]).
    full: (u64, u64),
    busy_ms: u64,
    tried_ms: u64,
}

// SAFETY: D3D12 objects (free-threaded) and persistent mappings, used under `STATE`'s lock.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// A table's identity: the layout, mip and slice it is made for, and the pitches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    layout: (u8, u32, u32, u32, u32, u32, u32, u32),
    mip: u32,
    slice: u32,
    row_pitch: u64,
    depth_pitch: u64,
}

fn key(layout: &Layout, mip: u32, slice: u32, row_pitch: u64, depth_pitch: u64) -> Key {
    let d = &layout.desc;
    let dimension = match d.dimension {
        Dimension::Tex1d => 1,
        Dimension::Tex2d => 2,
        Dimension::Tex3d => 3,
    };
    Key {
        layout: (dimension, d.width, d.height, d.depth, d.mips, d.element_bytes, d.block, d.swizzle.0),
        mip,
        slice,
        row_pitch,
        depth_pitch,
    }
}

/// Whether the rings and the shader are there (made on the first ask; once).
pub(crate) fn ready() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| {
        if !on() {
            return false;
        }
        match make() {
            Ok(state) => {
                tracing::info!(
                    raw_mb = state.raw_arena.size() >> 20,
                    out_mb = state.out_arena.size() >> 20,
                    table_mb = state.tables.3 >> 20,
                    verify = verify_per_second(),
                    "VICTORY_GPU_DETILE=1: textures are detiled on the card"
                );
                *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "VICTORY_GPU_DETILE=1: no detile on the card (textures are detiled on the CPU)");
                false
            }
        }
    })
}

fn make() -> windows::core::Result<State> {
    let empty = windows::core::Error::empty;
    let host = crate::host::host().ok_or_else(empty)?;
    let code = crate::host::compile(DETILE, windows::core::s!("main"), windows::core::s!("cs_5_1"))?;
    // SAFETY: the blob's bytes; the root signature is inside the shader.
    let (root, pso) = unsafe {
        let root: ID3D12RootSignature = host.device.CreateRootSignature(0, &code)?;
        let pso: ID3D12PipelineState = host.device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
            ..Default::default()
        })?;
        (root, pso)
    };
    let raw_size = mb("VICTORY_GPU_DETILE_RAW_MB", 256, 32, 2048);
    let (raw, raw_data, out, out_size) = make_rings(raw_size, mb("VICTORY_GPU_DETILE_OUT_MB", 192, 32, 1024))?;
    let table_size = mb("VICTORY_GPU_DETILE_TABLE_MB", 32, 4, 256);
    let (tables, table_data) = host.upload_buffer(table_size).ok_or_else(empty)?;
    crate::host::name(&tables, || "detile: tables".into());
    // SAFETY: live buffers.
    let (raw_gpu, out_gpu, table_gpu) =
        unsafe { (raw.GetGPUVirtualAddress(), out.GetGPUVirtualAddress(), tables.GetGPUVirtualAddress()) };
    Ok(State {
        root,
        pso,
        raw: (raw, raw_data, raw_gpu),
        raw_arena: Arena::new(raw_size),
        out: (out, out_gpu),
        out_arena: Arena::new(out_size),
        tables: (tables, table_data as usize, table_gpu, table_size, 0),
        cache: HashMap::new(),
        jobs: Vec::new(),
        filled: Vec::new(),
        lists: Vec::new(),
        checks: Vec::new(),
        checked: (0, 0),
        full: (raw_size, out_size),
        busy_ms: crate::host::now_ms(),
        tried_ms: 0,
    })
}

/// The rings: the raw one of `raw_size` bytes (and its mapping), and the card's one of up to
/// `out_size` bytes, with its size.
fn make_rings(raw_size: u64, mut out_size: u64) -> windows::core::Result<(ID3D12Resource, usize, ID3D12Resource, u64)> {
    let empty = windows::core::Error::empty;
    let host = crate::host::host().ok_or_else(empty)?;
    let (raw, raw_data) = host.upload_buffer(raw_size).ok_or_else(empty)?;
    crate::host::name(&raw, || "detile: title bytes".into());
    // The card's ring: only with room under 85 % of the budget after it, halved down to
    // 32 MB to find room; none at all, and no detile on the card, without.
    let out = loop {
        let room = host.video_memory().is_none_or(|[budget, used, ..]| used + out_size <= budget / 100 * 85);
        if room && let Some(buffer) = host.uav_buffer(out_size) {
            break buffer;
        }
        out_size /= 2;
        if out_size < 32 << 20 {
            tracing::warn!("VICTORY_GPU_DETILE=1: no room in the card's budget for the detile ring");
            return Err(empty());
        }
    };
    crate::host::name(&out, || "detile: detiled textures".into());
    Ok((raw, raw_data as usize, out, out_size))
}

/// (Neptune) `VICTORY_GPU_DETILE_TRIM=1` (off by default): the rings shrink to
/// `VICTORY_GPU_DETILE_SMALL_MB` each (64) once no upload has wanted more than that for
/// `VICTORY_GPU_DETILE_TRIM_S` seconds (20), and grow back to their full sizes when one does.
/// In the world the rings hold a few MB at most; the full ones (512 + 512 MB in `play.ps1`)
/// are for loading, and both take the process's commit (the card's one its memory too). A
/// resize is made only when both rings are empty, with nothing reserved and nothing the card
/// may still read, and the old rings go behind the submissions still running (`retire`); an
/// upload the rings cannot take meanwhile is detiled on the CPU, as every refused one is.
/// The seconds, and the small size in bytes.
fn trim() -> Option<(u64, u64)> {
    static T: OnceLock<Option<(u64, u64)>> = OnceLock::new();
    *T.get_or_init(|| {
        std::env::var_os("VICTORY_GPU_DETILE_TRIM").is_some_and(|v| v == "1").then(|| {
            let secs = std::env::var("VICTORY_GPU_DETILE_TRIM_S").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(20u64).clamp(2, 3600);
            (secs, mb("VICTORY_GPU_DETILE_SMALL_MB", 64, 32, 1024))
        })
    })
}

/// Since the start: the rings shrunk, and grown back ([`trim`]).
pub(crate) static RESIZES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// With [`trim`], at a submission that detiles nothing: the rings are made the size they
/// should be now, if they hold nothing.
fn fit(state: &mut State, completed: u64) {
    let Some((secs, small)) = trim() else {
        return;
    };
    let now = crate::host::now_ms();
    let small = (small.min(state.full.0), small.min(state.full.1));
    let want = if now.saturating_sub(state.busy_ms) < secs * 1000 { state.full } else { small };
    let have = (state.raw_arena.size(), state.out_arena.size());
    if (have.0 == want.0 / GRAIN * GRAIN && have.1 == want.1 / GRAIN * GRAIN) || now.saturating_sub(state.tried_ms) < 1000 {
        return;
    }
    state.raw_arena.reclaim(completed);
    state.out_arena.reclaim(completed);
    let quiet = state.raw_arena.used() == 0
        && state.out_arena.used() == 0
        && state.jobs.is_empty()
        && state.filled.is_empty()
        && state.checks.is_empty()
        && state.lists.iter().all(|l| l.2 <= completed);
    if !quiet {
        return;
    }
    state.tried_ms = now;
    let grow = want.0 > have.0 || want.1 > have.1;
    match make_rings(want.0, want.1) {
        Ok((raw, raw_data, out, out_size)) => {
            // SAFETY: live buffers.
            let (raw_gpu, out_gpu) = unsafe { (raw.GetGPUVirtualAddress(), out.GetGPUVirtualAddress()) };
            let old_raw = std::mem::replace(&mut state.raw, (raw, raw_data, raw_gpu));
            let old_out = std::mem::replace(&mut state.out, (out, out_gpu));
            state.raw_arena = Arena::new(want.0);
            state.out_arena = Arena::new(out_size);
            if grow && out_size < want.1 {
                // The card had no room for the whole ring: tried again in ten seconds; the full
                // size stays what was asked.
                state.tried_ms = now + 9_000;
            }
            crate::host::retire((old_raw.0, old_out.0));
            RESIZES[usize::from(grow)].fetch_add(1, Relaxed);
            tracing::info!(raw_mb = want.0 >> 20, card_mb = out_size >> 20, "gpu detile: rings {}", if grow { "grown back" } else { "shrunk" });
        }
        Err(e) => tracing::warn!(error = %e, "gpu detile: the rings could not be resized (kept as they are)"),
    }
}

/// The ring the detiled textures are in: the source of the lists' copies.
pub(crate) fn out_buffer() -> Option<ID3D12Resource> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.out.0.clone())
}

/// One subresource of an upload, as the staging state lays it out: its mip and array slice,
/// its footprint and rows.
pub(crate) struct Sub {
    pub(crate) mip: u32,
    pub(crate) slice: u32,
    pub(crate) offset: u64,
    pub(crate) row_pitch: u64,
    pub(crate) rows: u32,
    pub(crate) depth: u32,
}

impl State {
    /// The table for a subresource: from the cache, or made, checked and put in the table
    /// buffer.
    fn table(&mut self, layout: &Layout, sub: &Sub) -> Result<(Table, u64), Why> {
        let (slice, shift) = table_slice(layout, sub.slice);
        let depth_pitch = sub.row_pitch * u64::from(sub.rows);
        let key = key(layout, sub.mip, slice, sub.row_pitch, depth_pitch);
        if let Some(found) = self.cache.get(&key) {
            return found.map(|t| (t, shift));
        }
        let made = layout
            .copy_table(sub.mip, slice, sub.row_pitch as usize, depth_pitch as usize)
            .map_err(|_| Why::NoTable)
            .and_then(|t| pack(&t));
        let packed = match made {
            Ok(packed) => packed,
            Err(why) => {
                // What the layout is does not change: asked once.
                self.cache.insert(key, Err(why));
                return Err(why);
            }
        };
        let bytes = 4 * packed.words.len() as u64;
        let at = self.tables.4.next_multiple_of(16);
        if at + bytes > self.tables.3 || at / 4 > u64::from(u32::MAX) {
            // Not cached: a table refused for room is asked again (and refused again: the
            // buffer only fills).
            return Err(Why::Tables);
        }
        // SAFETY: inside the table buffer's mapping (checked above); the GPU reads only
        // tables written before the lists that use them were sent.
        unsafe {
            std::ptr::copy_nonoverlapping(packed.words.as_ptr().cast::<u8>(), (self.tables.1 as *mut u8).add(at as usize), bytes as usize);
        }
        self.tables.4 = at + bytes;
        let table = Table {
            at: (at / 4) as u32,
            runs: packed.runs,
            rows: packed.rows,
            tiled_end: packed.tiled_end,
            linear_end: packed.linear_end,
        };
        self.cache.insert(key, Ok(table));
        Ok((table, shift))
    }
}

/// The slices and tables for an upload of `raw_len` bytes of title memory into `out_len`
/// bytes laid out as `subs`, or why it goes the CPU's way. Never waits.
pub(crate) fn reserve(layout: &Layout, subs: &[Sub], raw_len: u64, out_len: u64) -> Result<Arc<Upload>, Why> {
    if !ready() {
        return Err(Why::NotReady);
    }
    let completed = crate::host::host().map_or(0, |h| h.completed());
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.as_mut().ok_or(Why::NotReady)?;
    if let Some((_, small)) = trim() {
        // An upload of more than a quarter of the small rings, or rings filled past half of
        // them, is a burst: the full rings are wanted.
        if raw_len.max(out_len) > small / 4 || state.raw_arena.used().max(state.out_arena.used()) > small / 2 {
            state.busy_ms = crate::host::now_ms();
        }
    }
    if raw_len > state.raw_arena.size() || out_len > state.out_arena.size() || raw_len > u64::from(u32::MAX) {
        return Err(Why::TooBig);
    }
    let mut parts = Vec::with_capacity(subs.len());
    for sub in subs {
        let (table, shift) = state.table(layout, sub)?;
        parts.push(part(sub, table, shift, raw_len, out_len)?);
    }
    state.raw_arena.reclaim(completed);
    state.out_arena.reclaim(completed);
    let taken = take_slices(&mut state.raw_arena, &mut state.out_arena, raw_len, out_len);
    if taken.is_err() && trim().is_some() {
        state.busy_ms = crate::host::now_ms();
    }
    let (raw, out) = taken?;
    drop(guard);
    Ok(Arc::new(Upload {
        raw: (raw, raw_len),
        out: (out, out_len),
        parts,
        sent: AtomicU64::new(0),
    }))
}

/// The part of an upload for `sub`, its table moved by `shift`, checked against slices of
/// `raw_len` and `out_len` bytes: every byte the table reads lies in the raw slice, every
/// byte it writes in the subresource's part of the output slice. That part is whole rows of
/// the pitch, but the last subresource's last row ends at its bytes (the footprints' total
/// says no more).
fn part(sub: &Sub, table: Table, shift: u64, raw_len: u64, out_len: u64) -> Result<Part, Why> {
    let whole = sub.row_pitch * u64::from(sub.rows) * u64::from(sub.depth);
    let bytes = whole.min(out_len.saturating_sub(sub.offset));
    if shift + table.tiled_end > raw_len || table.linear_end > bytes {
        return Err(Why::Bounds);
    }
    Ok(Part {
        mip: sub.mip,
        offset: sub.offset,
        bytes,
        table,
        shift: u32::try_from(shift).map_err(|_| Why::TooBig)?,
    })
}

/// [`part`] for a packed table at word 0 of the table buffer (tests).
#[cfg(test)]
pub(crate) fn check_part(sub: &Sub, packed: &Packed, shift: u64, raw_len: u64, out_len: u64) -> Result<u64, Why> {
    let table = Table { at: 0, runs: packed.runs, rows: packed.rows, tiled_end: packed.tiled_end, linear_end: packed.linear_end };
    part(sub, table, shift, raw_len, out_len).map(|p| p.bytes)
}

/// Both slices of an upload, or neither: a ring without room now refuses at once, and what
/// the other ring gave is given back.
pub(crate) fn take_slices(raw_arena: &mut Arena, out_arena: &mut Arena, raw_len: u64, out_len: u64) -> Result<(u64, u64), Why> {
    let raw = raw_arena.alloc(raw_len).ok_or(Why::RawFull)?;
    match out_arena.alloc(out_len) {
        Some(out) => Ok((raw, out)),
        None => {
            raw_arena.release(raw, raw_len, 0);
            Err(Why::OutFull)
        }
    }
}

/// Whether the upload being filled now is to be read back and compared.
pub(crate) fn check_due() -> bool {
    let most = verify_per_second();
    if most == 0 {
        return false;
    }
    let now = crate::host::now_ms() / 1000;
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return false;
    };
    if state.checked.0 != now {
        state.checked = (now, 0);
    }
    if state.checked.1 >= most {
        return false;
    }
    state.checked.1 += 1;
    true
}

/// What a check compares: the CPU's detile of the same bytes into a buffer of the output
/// slice's size, the ranges of it that are the texture's (rows without their padding), and
/// the texture named.
pub(crate) struct Expected {
    pub(crate) bytes: Vec<u8>,
    pub(crate) ranges: Vec<(u64, u64)>,
    pub(crate) what: String,
}

/// At submission: `raw` (title memory as read now, [`Upload::raw_len`] bytes) goes into the
/// upload's raw slice and its jobs into this submission's work; the subresources of mips
/// not in `held` (a bit a mip; `u32::MAX`: all) are zeros. The upload must not be in the
/// air ([`Upload::in_flight`]).
pub(crate) fn fill(upload: &Arc<Upload>, raw: &[u8], held: u32, expected: Option<Expected>) {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return;
    };
    let n = raw.len().min(upload.raw.1 as usize);
    // SAFETY: inside the raw ring's mapping (the slice was reserved for `raw.1` bytes); no
    // submission that reads the slice is in the air (the caller checked).
    unsafe { std::ptr::copy_nonoverlapping(raw.as_ptr(), (state.raw.1 as *mut u8).add(upload.raw.0 as usize), n) };
    let raw_gpu = state.raw.2 + upload.raw.0;
    let out_gpu = state.out.1 + upload.out.0;
    let (raw_len, out_len) = (n as u32, upload.out.1 as u32);
    for part in &upload.parts {
        if part.mip < 32 && held & (1 << part.mip) == 0 {
            let words = (part.bytes / 4) as u32;
            let mut done = 0;
            while done < words {
                let now = (words - done).min(GROUPS_MOST * 64);
                let at = (part.offset + 4 * u64::from(done)) as u32;
                state.jobs.push(Job {
                    constants: [0, 0, now, 0, 0, at, raw_len, out_len],
                    raw: raw_gpu,
                    out: out_gpu,
                    groups: (now.div_ceil(64), 1),
                });
                done += now;
            }
            continue;
        }
        let t = part.table;
        let mut first = 0;
        while first < t.rows {
            let rows = (t.rows - first).min(GROUPS_MOST);
            state.jobs.push(Job {
                constants: [t.at, t.runs, t.at + 3 * t.runs, first, part.shift, part.offset as u32, raw_len, out_len],
                raw: raw_gpu,
                out: out_gpu,
                groups: (t.runs.div_ceil(64), rows),
            });
            first += rows;
        }
    }
    if let Some(e) = expected {
        state.checks.push(Check {
            at: upload.out.0,
            len: upload.out.1,
            expected: e.bytes,
            ranges: e.ranges,
            what: e.what,
            back: None,
            fence: u64::MAX,
        });
    }
    state.filled.push(upload.clone());
    STATS[DONE].fetch_add(1, Relaxed);
    STATS[DONE_BYTES].fetch_add(n as u64, Relaxed);
}

/// At submission: a list that used `upload` is submitted again while its last submission may
/// still read the slices. They are not written (the detile there is the one of that
/// submission); the upload is noted for this submission's fence value.
pub(crate) fn reuse(upload: &Arc<Upload>) {
    STATS[REUSED].fetch_add(1, Relaxed);
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.filled.push(upload.clone());
    }
}

/// At submission, after the lists' steps: the list that detiles what they filled, to go
/// ahead of them in the same call; `None` if they filled nothing.
pub(crate) fn pre_list() -> Option<ID3D12CommandList> {
    let host = crate::host::host()?;
    let completed = host.completed();
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.as_mut()?;
    land_checks(state, completed);
    if state.jobs.is_empty() {
        fit(state, completed);
        return None;
    }
    let jobs = std::mem::take(&mut state.jobs);
    let at = match state.lists.iter().position(|l| l.2 <= completed) {
        Some(at) => at,
        None => {
            // SAFETY: plain object creation.
            let made: windows::core::Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)> = unsafe {
                (|| {
                    let allocator: ID3D12CommandAllocator = host.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
                    let list: ID3D12GraphicsCommandList =
                        host.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
                    list.Close()?;
                    Ok((allocator, list))
                })()
            };
            match made {
                Ok((allocator, list)) => {
                    state.lists.push((allocator, list, 0));
                    state.lists.len() - 1
                }
                Err(e) => crate::host::fail(format_args!("no list for the detile on the card ({e}, VICTORY_GPU_DETILE=1)")),
            }
        }
    };
    // Readbacks for the checks taken in this submission.
    for check in state.checks.iter_mut().filter(|c| c.fence == u64::MAX && c.back.is_none()) {
        check.back = host.shared_buffer(check.len).map(|(b, d)| (b, d as usize));
    }
    let (allocator, list, _) = &state.lists[at];
    let out = &state.out.0;
    let (common, uav, source) = (D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    // SAFETY: the allocator's last list has run (its fence value is passed); live buffers
    // (the rings and the table buffer live for the run); every job's offsets were checked
    // against its slices when the upload was reserved.
    let recorded = unsafe {
        (|| -> windows::core::Result<()> {
            allocator.Reset()?;
            list.Reset(allocator, None)?;
            // The ring rests in the common state between submissions, as `through` does:
            // the lists' copies after this one read it from there.
            list.ResourceBarrier(&[crate::host::transition(out, common, uav)]);
            list.SetComputeRootSignature(&state.root);
            list.SetPipelineState(&state.pso);
            list.SetComputeRootShaderResourceView(2, state.tables.2);
            for job in &jobs {
                list.SetComputeRoot32BitConstants(0, 8, job.constants.as_ptr().cast(), 0);
                list.SetComputeRootShaderResourceView(1, job.raw);
                list.SetComputeRootUnorderedAccessView(3, job.out);
                list.Dispatch(job.groups.0, job.groups.1, 1);
            }
            list.ResourceBarrier(&[crate::host::transition(out, uav, source)]);
            for check in state.checks.iter().filter(|c| c.fence == u64::MAX) {
                if let Some((back, _)) = &check.back {
                    list.CopyBufferRegion(back, 0, out, check.at, check.len);
                }
            }
            list.ResourceBarrier(&[crate::host::transition(out, source, common)]);
            list.Close()
        })()
    };
    if let Err(e) = recorded {
        crate::host::fail(format_args!("the detile on the card could not be recorded ({e}, VICTORY_GPU_DETILE=1)"));
    }
    STATS[DISPATCHES].fetch_add(jobs.len() as u64, Relaxed);
    state.lists[at].2 = u64::MAX;
    let list = state.lists[at].1.cast().unwrap_or_else(|e| crate::host::fail(format_args!("detile command list unavailable ({e})")));
    drop(guard);
    status();
    Some(list)
}

/// After a submission is sent with fence value `fence`: what it filled or used again, its
/// list and its checks are noted with it.
pub(crate) fn sent(fence: u64) {
    let filled = {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = guard.as_mut() else {
            return;
        };
        for l in state.lists.iter_mut().filter(|l| l.2 == u64::MAX) {
            l.2 = fence;
        }
        for c in state.checks.iter_mut().filter(|c| c.fence == u64::MAX) {
            c.fence = fence;
        }
        std::mem::take(&mut state.filled)
    };
    for upload in &filled {
        upload.sent.fetch_max(fence, Relaxed);
    }
    // Dropped here, outside the lock: the last holder gives the slices back under it.
    drop(filled);
}

/// The checks whose submission the card has run: compared, and let go.
fn land_checks(state: &mut State, completed: u64) {
    let mut i = 0;
    while i < state.checks.len() {
        let check = &state.checks[i];
        if check.fence == u64::MAX || check.fence > completed {
            i += 1;
            continue;
        }
        let check = state.checks.swap_remove(i);
        let Some((_, data)) = &check.back else {
            continue;
        };
        // SAFETY: the readback buffer's mapping, `len` bytes, written by a list that has run.
        let got = unsafe { std::slice::from_raw_parts(*data as *const u8, check.len as usize) };
        STATS[CHECKED].fetch_add(1, Relaxed);
        let first = check.ranges.iter().find_map(|&(at, len)| {
            let (a, b) = (at as usize, (at + len) as usize);
            (got[a..b] != check.expected[a..b]).then(|| a + got[a..b].iter().zip(&check.expected[a..b]).position(|(x, y)| x != y).unwrap_or(0))
        });
        if let Some(at) = first {
            let n = STATS[DIFFERED].fetch_add(1, Relaxed);
            if n < 32 {
                tracing::error!(
                    texture = %check.what,
                    first_difference = at,
                    card = got[at],
                    cpu = check.expected[at],
                    "VICTORY_GPU_DETILE_VERIFY: the card's detile differs from the CPU's"
                );
            }
        }
    }
}

/// Every ten seconds, with the switch on: what went to the card and what did not.
fn status() {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::host::now_ms();
    let last = LAST.load(Relaxed);
    if now < last + 10_000 || LAST.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    let s = |i: usize| STATS[i].load(Relaxed);
    let (raw, out) = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or((0, 0), |st| (st.raw_arena.used() >> 20, st.out_arena.used() >> 20));
    tracing::info!(
        "gpu detile: {} uploads ({} MB of title memory), {} dispatches; rings in use {} MB raw, {} MB card; \
         to the CPU: not ready {}, unaligned {}, no table {}, tables full {}, bounds {}, too big {}, raw ring full {}, card ring full {}; \
         reused in the air {}; verify: {} checked, {} differed",
        s(DONE),
        s(DONE_BYTES) >> 20,
        s(DISPATCHES),
        raw,
        out,
        s(WHY_AT),
        s(WHY_AT + 1),
        s(WHY_AT + 2),
        s(WHY_AT + 3),
        s(WHY_AT + 4),
        s(WHY_AT + 5),
        s(WHY_AT + 6),
        s(WHY_AT + 7),
        s(REUSED),
        s(CHECKED),
        s(DIFFERED),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_formats::texture::{Desc, Swizzle};

    fn desc(w: u32, h: u32, mips: u32, bpe: u32, block: u32, depth: u32, swizzle: Swizzle) -> Desc {
        Desc {
            dimension: Dimension::Tex2d,
            width: w,
            height: h,
            depth,
            mips,
            element_bytes: bpe,
            block,
            samples: 1,
            swizzle,
        }
    }

    fn layouts() -> Vec<Layout> {
        let mut out = Vec::new();
        for swizzle in [Swizzle::LINEAR, Swizzle::SW_256B_S, Swizzle::SW_4KB_S, Swizzle::SW_4KB_D, Swizzle::SW_64KB_S, Swizzle::SW_64KB_D] {
            out.push(Layout::new(&desc(512, 256, 10, 16, 4, 1, swizzle)).unwrap());
            out.push(Layout::new(&desc(300, 200, 9, 8, 4, 6, swizzle)).unwrap());
            out.push(Layout::new(&desc(257, 129, 9, 4, 1, 3, swizzle)).unwrap());
            out.push(Layout::new(&desc(96, 64, 7, 8, 1, 1, swizzle)).unwrap());
        }
        out.push(Layout::new(&Desc { dimension: Dimension::Tex3d, ..desc(64, 32, 6, 4, 1, 16, Swizzle::SW_4KB_S) }).unwrap());
        out
    }

    #[test]
    fn the_arena_fills_refuses_and_frees_by_fence() {
        let mut a = Arena::new(4 * GRAIN);
        let x = a.alloc(GRAIN).unwrap();
        let y = a.alloc(2 * GRAIN - 1).unwrap();
        assert_eq!(a.used(), 3 * GRAIN);
        assert_eq!(a.alloc(2 * GRAIN), None, "full: refused, not waited for");
        let z = a.alloc(1).unwrap();
        assert_eq!(a.alloc(1), None);
        // Given back in another order, waiting for the card.
        a.release(y, 2 * GRAIN - 1, 7);
        a.release(x, GRAIN, 5);
        assert_eq!(a.alloc(1), None, "nothing free until the card passes the fences");
        a.reclaim(5);
        assert_eq!(a.alloc(2 * GRAIN), None, "x alone is free");
        a.reclaim(7);
        // x and y join: three grains in one range.
        assert_eq!(a.alloc(3 * GRAIN), Some(0));
        a.release(0, 3 * GRAIN, 0);
        a.release(z, 1, 0);
        assert_eq!(a.used(), 0);
        assert_eq!(a.alloc(4 * GRAIN), Some(0));
        assert_eq!(Arena::new(GRAIN - 1).alloc(1), None);
    }

    /// A packed table, run as the shader runs it, writes what the CPU's detile writes; every
    /// array slice from slice 0's table and its shift.
    #[test]
    fn packed_tables_detile_as_the_cpu_does_and_slices_share_one() {
        let mut checked = 0;
        for l in layouts() {
            let raw: Vec<u8> = (0..l.size).map(|i| (i.wrapping_mul(0x9e37_79b9) >> 24) as u8).collect();
            let n = l.desc.element_bytes as usize;
            let slices = if l.desc.dimension == Dimension::Tex3d { 1 } else { l.slices };
            for mip in 0..l.desc.mips {
                let m = l.mips[mip as usize];
                let pitch = (m.width as usize * n).next_multiple_of(256);
                let plane = pitch * m.height as usize;
                let Ok(packed) = l.copy_table(mip, 0, pitch, plane).map_err(|_| Why::NoTable).and_then(|t| pack(&t)) else {
                    continue;
                };
                assert!(packed.words.chunks(3).take(packed.runs as usize).all(|r| u64::from(r[2]) <= RUN_MOST || l.desc.swizzle.xor()));
                for slice in 0..slices {
                    let (keyed, shift) = table_slice(&l, slice);
                    let table = if keyed == 0 { packed.clone() } else { pack(&l.copy_table(mip, keyed, pitch, plane).unwrap()).unwrap() };
                    assert!(shift + table.tiled_end <= l.size, "{:?} mip {mip} slice {slice}", l.desc);
                    let len = plane * m.depth as usize;
                    let (mut cpu, mut card) = (vec![0xcc; len], vec![0xcc; len]);
                    l.detile(&raw, mip, slice, &mut cpu, pitch, plane).unwrap();
                    emulate(&table.words, table.runs, 0..table.rows, shift as u32, &raw, &mut card, 0);
                    assert!(cpu == card, "{:?} mip {mip} slice {slice}", l.desc);
                    checked += 1;
                }
            }
        }
        assert!(checked > 300, "{checked}");
    }

    #[test]
    fn narrow_levels_of_small_elements_are_refused() {
        let l = Layout::new(&desc(256, 256, 9, 1, 1, 1, Swizzle::SW_64KB_S)).unwrap();
        let t = l.copy_table(8, 0, 256, 256).unwrap();
        assert_eq!(pack(&t), Err(Why::Unaligned));
    }

    /// The shader's checks: a table reaching past the slices writes nothing outside them.
    #[test]
    fn words_outside_the_slices_are_left_out() {
        let l = Layout::new(&desc(64, 64, 1, 16, 4, 1, Swizzle::SW_64KB_S)).unwrap();
        let p = pack(&l.copy_table(0, 0, 256, 256 * 16).unwrap()).unwrap();
        let raw = vec![7u8; p.tiled_end as usize];
        let mut out = vec![0u8; 100];
        emulate(&p.words, p.runs, 0..p.rows, 0, &raw, &mut out, 0);
        assert!(out.iter().all(|&b| b == 0 || b == 7));
        let mut tail = vec![0u8; 4096];
        emulate(&p.words, p.runs, 0..p.rows, 1 << 30, &raw, &mut tail, 0);
        assert!(tail.iter().all(|&b| b == 0), "nothing read past the raw slice");
    }

    /// The shader itself, on the software adapter, against the CPU's detile: BC-sized tiled
    /// elements, an array slice by shift, a split dispatch and zeros.
    #[test]
    fn warp_the_shader_detiles_as_the_cpu_does() {
        let Some(w) = warp::Warp::new() else {
            eprintln!("no software adapter: skipped");
            return;
        };
        let code = crate::host::compile(DETILE, windows::core::s!("main"), windows::core::s!("cs_5_1")).expect("the detile shader compiles");
        for l in [
            Layout::new(&desc(512, 512, 10, 16, 4, 2, Swizzle::SW_64KB_S)).unwrap(),
            Layout::new(&desc(300, 200, 9, 8, 4, 1, Swizzle::SW_4KB_D)).unwrap(),
            Layout::new(&desc(200, 100, 1, 4, 1, 1, Swizzle::LINEAR)).unwrap(),
        ] {
            let raw: Vec<u8> = (0..l.size).map(|i| (i.wrapping_mul(0x9e37_79b9) >> 24) as u8).collect();
            let n = l.desc.element_bytes as usize;
            for mip in [0, l.desc.mips - 1] {
                let m = l.mips[mip as usize];
                let pitch = (m.width as usize * n).next_multiple_of(256);
                let plane = pitch * m.height as usize;
                let slice = l.slices - 1;
                let (keyed, shift) = table_slice(&l, slice);
                let p = pack(&l.copy_table(mip, keyed, pitch, plane).unwrap()).unwrap();
                let mut cpu = vec![0u8; plane + 512];
                l.detile(&raw, mip, slice, &mut cpu[512..], pitch, plane).unwrap();
                // The words of the table first, then 512 bytes of zeros the shader writes too.
                let got = w.run(&code, &raw, &p, shift as u32, 512, plane + 512);
                for y in 0..m.height as usize {
                    let at = 512 + y * pitch;
                    let len = m.width as usize * n;
                    assert_eq!(&got[at..at + len], &cpu[at..at + len], "{:?} mip {mip} row {y}", l.desc);
                }
                assert!(got[..512].iter().all(|&b| b == 0), "zeros");
            }
        }
    }

    mod warp {
        use super::super::*;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
        use windows::Win32::Graphics::Dxgi::*;
        use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

        pub(super) struct Warp {
            device: ID3D12Device,
            queue: ID3D12CommandQueue,
            allocator: ID3D12CommandAllocator,
            list: ID3D12GraphicsCommandList,
            fence: ID3D12Fence,
            event: HANDLE,
            sent: std::cell::Cell<u64>,
        }

        impl Warp {
            pub(super) fn new() -> Option<Warp> {
                // SAFETY: plain DXGI queries and object creation on the software adapter.
                unsafe {
                    let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
                    let adapter: IDXGIAdapter = factory.EnumWarpAdapter().ok()?;
                    let mut device: Option<ID3D12Device> = None;
                    D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).ok()?;
                    let device = device?;
                    let queue: ID3D12CommandQueue = device
                        .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })
                        .ok()?;
                    let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).ok()?;
                    let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).ok()?;
                    list.Close().ok()?;
                    let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
                    let event = CreateEventW(None, false, false, None).ok()?;
                    Some(Warp { device, queue, allocator, list, fence, event, sent: std::cell::Cell::new(0) })
                }
            }

            fn buffer(&self, kind: D3D12_HEAP_TYPE, size: u64, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES) -> ID3D12Resource {
                let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
                let desc = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                    Width: size.max(4),
                    Height: 1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                    Flags: flags,
                    ..Default::default()
                };
                let mut buffer: Option<ID3D12Resource> = None;
                // SAFETY: plain creation.
                unsafe { self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut buffer).unwrap() };
                buffer.unwrap()
            }

            fn upload(&self, bytes: &[u8]) -> ID3D12Resource {
                let b = self.buffer(D3D12_HEAP_TYPE_UPLOAD, bytes.len() as u64, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ);
                // SAFETY: a mapped upload buffer of at least `bytes.len()` bytes.
                unsafe {
                    let mut data = std::ptr::null_mut();
                    b.Map(0, None, Some(&mut data)).unwrap();
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.cast::<u8>(), bytes.len());
                    b.Unmap(0, None);
                }
                b
            }

            /// Runs the detile shader as `pre_list` records it: zeros over the first
            /// `zeros` bytes, then the table at `zeros`, into a ring of `out_len` bytes;
            /// gives the ring's bytes.
            pub(super) fn run(&self, code: &[u8], raw: &[u8], p: &Packed, shift: u32, zeros: u32, out_len: usize) -> Vec<u8> {
                let bytes: Vec<u8> = p.words.iter().flat_map(|w| w.to_le_bytes()).collect();
                let (raw_b, table_b) = (self.upload(raw), self.upload(&bytes));
                let out = self.buffer(D3D12_HEAP_TYPE_DEFAULT, out_len as u64, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON);
                let back = self.buffer(D3D12_HEAP_TYPE_READBACK, out_len as u64, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST);
                // SAFETY: our device, list and buffers; the root signature is in the shader.
                unsafe {
                    let root: ID3D12RootSignature = self.device.CreateRootSignature(0, code).unwrap();
                    let pso: ID3D12PipelineState = self
                        .device
                        .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                            pRootSignature: std::mem::transmute_copy(&root),
                            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
                            ..Default::default()
                        })
                        .unwrap();
                    self.allocator.Reset().unwrap();
                    self.list.Reset(&self.allocator, None).unwrap();
                    let l = &self.list;
                    l.ResourceBarrier(&[crate::host::transition(&out, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
                    l.SetComputeRootSignature(&root);
                    l.SetPipelineState(&pso);
                    l.SetComputeRootShaderResourceView(1, raw_b.GetGPUVirtualAddress());
                    l.SetComputeRootShaderResourceView(2, table_b.GetGPUVirtualAddress());
                    l.SetComputeRootUnorderedAccessView(3, out.GetGPUVirtualAddress());
                    let (raw_len, out_len) = (raw.len() as u32, out_len as u32);
                    let words = zeros / 4;
                    let c = [0u32, 0, words, 0, 0, 0, raw_len, out_len];
                    l.SetComputeRoot32BitConstants(0, 8, c.as_ptr().cast(), 0);
                    l.Dispatch(words.div_ceil(64), 1, 1);
                    // The rows in two dispatches, as a table of more than 65,535 rows is.
                    let half = p.rows / 2;
                    for (first, rows) in [(0, half), (half, p.rows - half)] {
                        if rows == 0 {
                            continue;
                        }
                        let c = [0u32, p.runs, 3 * p.runs, first, shift, zeros, raw_len, out_len];
                        l.SetComputeRoot32BitConstants(0, 8, c.as_ptr().cast(), 0);
                        l.Dispatch(p.runs.div_ceil(64), rows, 1);
                    }
                    l.ResourceBarrier(&[crate::host::transition(&out, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                    l.CopyBufferRegion(&back, 0, &out, 0, u64::from(out_len));
                    l.ResourceBarrier(&[crate::host::transition(&out, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON)]);
                    l.Close().unwrap();
                    self.queue.ExecuteCommandLists(&[Some(l.cast().unwrap())]);
                    self.sent.set(self.sent.get() + 1);
                    self.queue.Signal(&self.fence, self.sent.get()).unwrap();
                    self.fence.SetEventOnCompletion(self.sent.get(), self.event).unwrap();
                    WaitForSingleObject(self.event, INFINITE);
                    let mut data = std::ptr::null_mut();
                    back.Map(0, None, Some(&mut data)).unwrap();
                    let got = std::slice::from_raw_parts(data.cast::<u8>(), out_len as usize).to_vec();
                    back.Unmap(0, None);
                    got
                }
            }
        }
    }
}
