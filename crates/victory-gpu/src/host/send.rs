//! What goes up to copies in the card's memory goes up as it was when its
//! submission was prepared (`VICTORY_SEND_OWN`, on by default; `0`: as
//! before).
//!
//! The sends were copies out of the copies' CPU sides, which the card makes
//! when it comes to the submission. By then later submissions are prepared
//! (two frames of them with `VICTORY_RELAY`), and their refreshes have
//! written the same CPU sides: the earlier submission's shaders then read a
//! later one's bytes. For what the title's CPU writes that is as on the
//! console (it does not write what the card still reads). But the title's
//! own buffer copies are commands in its lists, and those are done by us
//! when a submission is prepared: "a value to a buffer, a draw that reads
//! it" in one submission and the same with another value in the next is
//! ordinary, and both draws read the second value.
//!
//! So each submission's bytes are taken out of the CPU sides into memory of
//! the submission's own when they are taken from the dirty lists, and the
//! card copies them from there. Runs of a few words, which a refresh that
//! sends only what changed makes many of, go as pairs of offset and value,
//! stored by one dispatch per copy; the others as a copy each.

use super::*;
use crate::shadow::Shadow;

pub(super) fn own() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SEND_OWN").is_none_or(|v| v != "0"))
}

/// Runs up to this many bytes go as pairs.
const SHORT: usize = 256;

/// Whether a run of `len` bytes at offset `at` of a copy goes as pairs.
fn short(at: usize, len: usize) -> bool {
    len <= SHORT && at.is_multiple_of(4) && len.is_multiple_of(4)
}

/// Whether a run of `len` bytes at offset `at` of a copy in the card's memory
/// is no copy command of its own, but pairs of one dispatch.
pub(crate) fn goes_as_words(at: usize, len: usize) -> bool {
    own() && short(at, len)
}

/// Whether the title's bytes that go in under flights' writes
/// (`Shadow::merge_under_writes`) go in by whole words
/// (`VICTORY_UNDER_WORDS=0`: by the bytes that changed, as before).
///
/// By bytes nearly every run (a float's three low bytes) was a copy command
/// of its own, and a submission with more than 4,096 of them stood until the
/// flights had landed: 4 to 14 ms of a steady frame (session play-1125-0203:
/// "more changed runs than a submission sends up"), for some 20 KB. Whole
/// words are pairs of one dispatch, and are not counted.
///
/// A word is taken to be the title's or the shaders', not shared: where the
/// title changed a byte of a word of which a flight's shader writes another
/// byte, that byte is as it was before the flight. The landing rests on the
/// same ([`crate::shadow::exact_back`]).
pub(crate) fn words_under() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| own() && std::env::var_os("VICTORY_UNDER_WORDS").is_none_or(|v| v != "0"))
}

const SCATTER: &str = r#"
#define RS "RootConstants(num32BitConstants=2, b0), SRV(t0), UAV(u0)"
cbuffer Part : register(b0) { uint at; uint count; };
ByteAddressBuffer pairs : register(t0);
RWByteAddressBuffer card : register(u0);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= count) return;
    uint2 p = pairs.Load2(at + id.x * 8u);
    card.Store(p.x, p.y);
}
"#;

struct Pass {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
}

fn pass(host: &Host) -> Option<&'static Pass> {
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    PASS.get_or_init(|| {
        let code = compile(SCATTER, windows::core::s!("main"), windows::core::s!("cs_5_1")).ok()?;
        // SAFETY: the code lives through the calls. Not the blob for the
        // root signature: under Wine the host refuses the signature inside
        // it (E_FAIL), while a description read from the source's `RS` and
        // serialized here works, as the title's own signatures do.
        unsafe {
            let root: ID3D12RootSignature = crate::rootstring::make(&host.device, SCATTER).ok()?;
            let pso: ID3D12PipelineState = host
                .device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: code.as_ptr().cast(),
                        BytecodeLength: code.len(),
                    },
                    ..Default::default()
                })
                .ok()?;
            name(&pso, || "victory: changed words sent to the card".into());
            Some(Pass { root, pso })
        }
    })
    .as_ref()
}

/// Memory a submission's sends come from, mapped.
pub(super) struct Memory {
    buffer: ID3D12Resource,
    data: usize,
    size: u64,
}

// SAFETY: a D3D12 object (free-threaded) and its mapping, with one owner.
unsafe impl Send for Memory {}

/// What submissions that have run gave back.
static SPARE: Mutex<Vec<Memory>> = Mutex::new(Vec::new());

/// At most this much is kept (of the card's memory, with [`on_card`]).
fn kept() -> u64 {
    if on_card() { 256 << 20 } else { 768 << 20 }
}

/// Whether the memory lies in the card's memory, written by the CPU through
/// the bus (`VICTORY_SEND_BAR=1`): the copies on the card's drawing queue, which
/// nothing overlaps, then cross no bus. In a captured frame they took 5.4 ms of
/// the card's 30 (342 copies), and take 0.4 this way; but filling it takes the
/// submitting thread 8-9 ms a frame instead of 3 (73-93 MB, run card-3 against
/// card-1), so it is a loss while the card waits for that thread: off by
/// default.
fn on_card() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SEND_BAR").is_some_and(|v| v == "1"))
}

fn memory(host: &Host, size: u64) -> Option<Memory> {
    {
        let mut spare = SPARE.lock().unwrap_or_else(|e| e.into_inner());
        let fit = spare.iter().enumerate().filter(|(_, m)| m.size >= size).min_by_key(|(_, m)| m.size).map(|(i, _)| i);
        if let Some(i) = fit {
            return Some(spare.swap_remove(i));
        }
    }
    // By doubles, so that the next of about this size finds it: what
    // submissions send differs from frame to frame.
    let size = size.max(8 << 20).next_power_of_two();
    let mapped = if on_card() { host.mapped_card_buffer(size, false) } else { None };
    let (buffer, data) = match mapped {
        Some(made) => made,
        None => host.upload_buffer(size)?,
    };
    name(&buffer, || "bytes a submission sends up".into());
    Some(Memory {
        buffer,
        data: data as usize,
        size,
    })
}

/// The submission has run.
pub(super) fn give_back(memory: Memory) {
    let mut spare = SPARE.lock().unwrap_or_else(|e| e.into_inner());
    spare.push(memory);
    spare.sort_by_key(|m| std::cmp::Reverse(m.size));
    while spare.iter().map(|m| m.size).sum::<u64>() > kept() {
        spare.remove(0);
    }
}

/// Since the last frame-rate line: bytes copied, runs copied, words stored,
/// microseconds of taking them out of the CPU sides.
pub(super) static STATS: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// One copy's sends: ranges copied (offset in the copy, in the memory,
/// bytes), and its pairs (where in the memory, how many).
struct Copy {
    runs: Vec<(usize, u64, usize)>,
    pairs: (u64, u32),
}

/// Where a submission's bytes are taken to.
pub(super) struct Own {
    memory: Memory,
    copies: Vec<Copy>,
    /// Bytes of memory used.
    used: u64,
}

/// Runs whose pairs are one piece of work.
const RUNS_AT_A_TIME: usize = 4096;

/// Takes what `dirty` sends up out of the CPU sides.
pub(super) fn take(host: &Host, dirty: &[(std::sync::Arc<Shadow>, Vec<(usize, usize)>)]) -> Option<Own> {
    let began = std::time::Instant::now();
    pass(host)?;
    // Runs of whole words of the copy: only those are pairs. (A byte beside
    // them in a word may be the card's.)
    let short = |&(a, b): &(usize, usize)| short(a, b - a);
    let mut at = 0u64;
    let mut copies = Vec::with_capacity(dirty.len());
    let mut total_move_bytes = 0u64;
    let mut total_move_runs = 0u64;
    let mut total_words = 0u64;
    // Collect relative offsets in one pass over `dirty`, then apply `memory.data`.
    let mut rel_parts: Vec<(usize, u64, usize)> = Vec::new();
    let mut rel_work: Vec<(usize, &[(usize, usize)], u64)> = Vec::new();
    const CHUNK: usize = 64 << 10;
    for (shadow, ranges) in dirty {
        let from = shadow.card_data() as usize;
        let mut runs = Vec::new();
        let mut copy_words = 0usize;
        let pairs_start = {
            for &(a, b) in ranges {
                let len = b - a;
                if short(&(a, b)) {
                    copy_words += len / 4;
                } else {
                    runs.push((a, at, len));
                    total_move_bytes += len as u64;
                    total_move_runs += 1;
                    let mut off = 0;
                    while off < len {
                        let n = CHUNK.min(len - off);
                        rel_parts.push((from + a + off, at + off as u64, n));
                        off += n;
                    }
                    at += (len as u64).next_multiple_of(16);
                }
            }
            at
        };
        if copy_words > 0 {
            let mut to_off = pairs_start;
            for chunk in ranges.chunks(RUNS_AT_A_TIME) {
                let w: usize = chunk.iter().filter(|r| short(r)).map(|r| (r.1 - r.0) / 4).sum();
                if w > 0 {
                    rel_work.push((from, chunk, to_off));
                    to_off += (w * 8) as u64;
                }
            }
            total_words += copy_words as u64;
            at += (copy_words as u64 * 8).next_multiple_of(16);
        }
        copies.push(Copy { runs, pairs: (pairs_start, copy_words as u32) });
    }
    if at == 0 {
        return None;
    }
    let memory = memory(host, at)?;
    STATS[0].fetch_add(total_move_bytes, Ordering::Relaxed);
    STATS[1].fetch_add(total_move_runs, Ordering::Relaxed);
    STATS[2].fetch_add(total_words, Ordering::Relaxed);
    let base = memory.data;
    let n_work = rel_work.len();
    let total = n_work + rel_parts.len();
    if total > 0 {
        crate::shadow::wide_each(total, |i| {
            if i < n_work {
                let (from, ranges, to_off) = rel_work[i];
                let mut to = (base + to_off as usize) as *mut [u32; 2];
                for &(a, b) in ranges.iter().filter(|r| short(r)) {
                    for offset in (a..b).step_by(4) {
                        // SAFETY: inside the copy's mapping and inside `memory`.
                        unsafe {
                            let word = ((from + offset) as *const u32).read_unaligned();
                            to.write([offset as u32, word]);
                            to = to.add(1);
                        }
                    }
                }
            } else {
                let (src, dst_off, len) = rel_parts[i - n_work];
                // SAFETY: non-overlapping ranges alive until `wide_each` returns.
                unsafe { copy_wc(src as *const u8, (base + dst_off as usize) as *mut u8, len) };
            }
        });
    }
    STATS[3].fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
    Some(Own { memory, copies, used: at })
}

/// Copies `len` bytes from cached CPU memory `src` into write-combined upload
/// memory `dst`. Uses 512-bit non-temporal streaming stores (`vmovntdq`) on
/// AVX-512 CPUs to avoid RFO traffic and maximize WC write-combining bandwidth.
#[inline]
unsafe fn copy_wc(mut src: *const u8, mut dst: *mut u8, mut len: usize) {
    #[cfg(target_feature = "avx512f")]
    {
        use std::arch::x86_64::{__m512i, _mm_sfence, _mm512_loadu_si512, _mm512_stream_si512};
        if len >= 256 {
            let mis = (dst as usize) & 63;
            if mis != 0 {
                let head = 64 - mis;
                unsafe { std::ptr::copy_nonoverlapping(src, dst, head) };
                src = unsafe { src.add(head) };
                dst = unsafe { dst.add(head) };
                len -= head;
            }
            while len >= 256 {
                unsafe {
                    let v0 = _mm512_loadu_si512(src as *const __m512i);
                    let v1 = _mm512_loadu_si512(src.add(64) as *const __m512i);
                    let v2 = _mm512_loadu_si512(src.add(128) as *const __m512i);
                    let v3 = _mm512_loadu_si512(src.add(192) as *const __m512i);
                    _mm512_stream_si512(dst as *mut __m512i, v0);
                    _mm512_stream_si512(dst.add(64) as *mut __m512i, v1);
                    _mm512_stream_si512(dst.add(128) as *mut __m512i, v2);
                    _mm512_stream_si512(dst.add(192) as *mut __m512i, v3);
                    src = src.add(256);
                    dst = dst.add(256);
                }
                len -= 256;
            }
            if len > 0 {
                unsafe { std::ptr::copy_nonoverlapping(src, dst, len) };
            }
            unsafe { _mm_sfence() };
            return;
        }
    }
    unsafe { std::ptr::copy_nonoverlapping(src, dst, len) };
}

impl Own {
    /// Records the sends into `list`; the copies that `looked` says are in
    /// `COPY_DEST`, the others in the common state. Those with a "before" are
    /// left readable by compute shaders.
    ///
    /// # Safety
    /// An open list of ours, whose compute state is set anew behind this.
    pub(super) unsafe fn record(
        &self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        dirty: &[(std::sync::Arc<Shadow>, Vec<(usize, usize)>)],
        looked: impl Fn(&Shadow) -> bool,
        from: Option<&ID3D12Resource>,
    ) {
        // (With `crate::host::upq`: a copy of the memory in the card's.)
        let source = from.unwrap_or(&self.memory.buffer);
        let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
        let write = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        let dest = D3D12_RESOURCE_STATE_COPY_DEST;
        let (mut first, mut last) = (Vec::new(), Vec::new());
        for ((shadow, _), copy) in dirty.iter().zip(&self.copies) {
            for &(a, from, n) in &copy.runs {
                // SAFETY: the caller's; live buffers, ranges inside them.
                unsafe { list.CopyBufferRegion(&shadow.buffer, a as u64, source, from, n as u64) };
            }
            let mut state = (looked(shadow) || !copy.runs.is_empty()).then_some(dest);
            if copy.pairs.1 > 0 {
                if let Some(was) = state {
                    first.push(transition(&shadow.buffer, was, write));
                }
                state = Some(write);
            }
            let hashed = shadow.card_sides().is_some_and(|(_, before, _)| before.is_some());
            if let Some(was) = state
                && hashed
                && (!copy.runs.is_empty() || copy.pairs.1 > 0)
            {
                last.push(transition(&shadow.buffer, was, read));
            }
        }
        // SAFETY: the caller's; live objects.
        unsafe {
            if !first.is_empty() {
                list.ResourceBarrier(&first);
            }
            if let Some(pass) = pass(host)
                && self.copies.iter().any(|c| c.pairs.1 > 0)
            {
                list.SetComputeRootSignature(&pass.root);
                list.SetPipelineState(&pass.pso);
                list.SetComputeRootShaderResourceView(1, source.GetGPUVirtualAddress());
                for ((shadow, _), copy) in dirty.iter().zip(&self.copies).filter(|(_, c)| c.pairs.1 > 0) {
                    list.SetComputeRootUnorderedAccessView(2, shadow.buffer.GetGPUVirtualAddress());
                    const MOST: u32 = 64 * D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION;
                    let (at, words) = copy.pairs;
                    let mut done = 0;
                    while done < words {
                        let n = (words - done).min(MOST);
                        let constants = [at as u32 + done * 8, n];
                        list.SetComputeRoot32BitConstants(0, 2, constants.as_ptr().cast(), 0);
                        list.Dispatch(n.div_ceil(64), 1, 1);
                        done += n;
                    }
                }
            }
            if !last.is_empty() {
                list.ResourceBarrier(&last);
            }
        }
    }

    /// The memory and the bytes of it used.
    pub(super) fn source(&self) -> (&ID3D12Resource, u64) {
        (&self.memory.buffer, self.used)
    }

    /// The memory, to be kept until the submission has run.
    pub(super) fn memory(self) -> Memory {
        self.memory
    }
}
