//! What the CPU wrote, put in without what the card wrote.
//!
//! The host layer brings results of shaders back to title memory, and sends
//! pieces of title memory up again (64 KiB at a time) when the title changed
//! some of them. What goes up therefore holds results of the recorded frame.
//! Shaders that give out places with atomics give other places in every run:
//! the recorded results put over a replay's own leave lists and what points
//! into them of two different frames. So where a copy brings memory of the
//! CPU to a buffer the card writes, words the card has written since the
//! start of the frame (which are not what was last put there) stay.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;

use super::*;

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=2, b0), SRV(t0), UAV(u0), UAV(u1), UAV(u2)"
cbuffer C : register(b0) { uint words; uint call; };
ByteAddressBuffer from : register(t0);
RWByteAddressBuffer to : register(u0);
RWByteAddressBuffer base : register(u1);
// For each call: words put, words of the card's left that were to get
// another value, and of those the ones that were to get back what stood
// there before the card wrote.
RWByteAddressBuffer counts : register(u2);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= words) return;
    uint at = id.x * 4;
    uint v = from.Load(at), now = to.Load(at), was = base.Load(at), n;
    if (now != was) {
        if (v == now) return;
        counts.InterlockedAdd(call * 12 + 4, 1, n);
        if (v == was) counts.InterlockedAdd(call * 12 + 8, 1, n);
        return;
    }
    if (v == now) return;
    counts.InterlockedAdd(call * 12, 1, n);
    to.Store(at, v);
    base.Store(at, v);
}
"#;

pub(super) struct Merger {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    /// For each buffer: what was last put there (at first, what it held at
    /// the frame's start).
    pub(super) bases: HashMap<u32, ID3D12Resource>,
    pub(super) counts: ID3D12Resource,
    /// The merges recorded: buffer, offset, length (in `CALLS` at the most).
    pub(super) calls: std::cell::RefCell<Vec<(u32, u64, u64)>>,
}

const CALLS: u64 = 1 << 16;

impl Player {
    /// The copy of a piece of a card buffer's CPU side to it: its source's
    /// address, then the buffer, offset, 0 and length.
    pub(super) fn merged_copy(&self, c: &Cmd) -> Option<(u64, (u32, u64, u64, u64))> {
        let Cmd::CopyBuffer {
            dst,
            dst_at,
            src,
            src_at,
            len,
        } = c
        else {
            return None;
        };
        if !self.merging
            || !self.pairs(*src, *dst)
            || dst_at != src_at
            || dst_at % 4 != 0
            || len % 4 != 0
        {
            return None;
        }
        self.merger.as_ref()?.bases.get(dst)?;
        let from = self.objects.get(src)?.va + src_at;
        Some((from, (*dst, *dst_at, 0, *len)))
    }

    /// Whether `src` is the CPU's side of card buffer `dst`.
    fn pairs(&self, src: u32, dst: u32) -> bool {
        let (Some(a), Some(b)) = (self.objects.get(&src), self.objects.get(&dst)) else {
            return false;
        };
        let (a, b) = (
            &self.frame.resources[a.index],
            &self.frame.resources[b.index],
        );
        let cpu =
            matches!(&a.place, Place::Committed { props, .. } if gpu::seen(props) == Seen::Cached);
        cpu && b.name.ends_with("on the card")
            && a.desc.width == b.desc.width
            && self.card_writes.contains(&dst)
    }

    /// Records merges: (buffer, offset in it, offset from `from`, length).
    ///
    /// # Safety
    /// An open list; `from` is the address of memory that holds the bytes.
    pub(super) unsafe fn merge_all(
        &self,
        list: &ID3D12GraphicsCommandList,
        from: u64,
        all: &[(u32, u64, u64, u64)],
    ) {
        let Some(m) = &self.merger else { return };
        // SAFETY: the caller's; live buffers.
        unsafe {
            list.SetComputeRootSignature(&m.root);
            list.SetPipelineState(&m.pso);
            for (id, at, source, len) in all {
                let (Some(o), Some(base)) = (self.objects.get(id), m.bases.get(id)) else {
                    continue;
                };
                let base = base.GetGPUVirtualAddress();
                // At most 65,535 groups in a dispatch.
                const MOST: u64 = 8 << 20;
                let mut done = 0;
                while done < *len {
                    let n = MOST.min(len - done);
                    let words = (n / 4) as u32;
                    let mut calls = m.calls.borrow_mut();
                    let call = (calls.len() as u64).min(CALLS - 1) as u32;
                    if calls.len() < CALLS as usize {
                        calls.push((*id, at + done, n));
                    }
                    drop(calls);
                    list.SetComputeRoot32BitConstants(0, 2, [words, call].as_ptr().cast(), 0);
                    list.SetComputeRootUnorderedAccessView(4, m.counts.GetGPUVirtualAddress());
                    list.SetComputeRootShaderResourceView(1, from + source + done);
                    list.SetComputeRootUnorderedAccessView(2, o.va + at + done);
                    list.SetComputeRootUnorderedAccessView(3, base + at + done);
                    list.Dispatch(words.div_ceil(64), 1, 1);
                    done += n;
                }
            }
        }
    }

    /// Switches merging on or off; makes what it needs at first.
    pub(super) fn set_merging(&mut self, on: bool) -> Result<()> {
        self.merging = on;
        if let Some(m) = &self.merger {
            m.calls.borrow_mut().clear();
        }
        if !on || self.merger.is_some() {
            return Ok(());
        }
        let mut code: Option<ID3DBlob> = None;
        // SAFETY: the source and names live through the call; the blob's own
        // bytes.
        let (root, pso) = unsafe {
            D3DCompile(
                SOURCE.as_ptr().cast(),
                SOURCE.len(),
                None,
                None,
                None,
                windows::core::s!("main"),
                windows::core::s!("cs_5_1"),
                0,
                0,
                &mut code,
                None,
            )?;
            let code = code.context("no merge shader")?;
            let bytes = std::slice::from_raw_parts(
                code.GetBufferPointer().cast::<u8>(),
                code.GetBufferSize(),
            );
            let root: ID3D12RootSignature = self.device.CreateRootSignature(0, bytes)?;
            let pso =
                self.device
                    .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                        pRootSignature: std::mem::transmute_copy(&root),
                        CS: D3D12_SHADER_BYTECODE {
                            pShaderBytecode: bytes.as_ptr().cast(),
                            BytecodeLength: bytes.len(),
                        },
                        ..Default::default()
                    })?;
            (root, pso)
        };
        let mut ids: HashSet<u32> = HashSet::new();
        for c in self.commands.iter().flatten() {
            if let Cmd::CopyBuffer { dst, src, .. } = c
                && self.pairs(*src, *dst)
            {
                ids.insert(*dst);
            }
        }
        ids.extend(
            self.delta_parts
                .iter()
                .flatten()
                .map(|(d, _)| d.res)
                .filter(|id| self.card_writes.contains(id)),
        );
        let mut bases = HashMap::new();
        let mut total = 0;
        for id in ids {
            if !self.masters.iter().any(|m| m.0 == id) {
                continue;
            }
            let size = self.frame.resources[self.objects[&id].index].desc.width;
            let flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
            bases.insert(
                id,
                gpu::buffer(&self.device, D3D12_HEAP_TYPE_DEFAULT, size, flags)?.0,
            );
            total += size;
        }
        let masters: HashMap<u32, ID3D12Resource> = self.masters.iter().cloned().collect();
        self.worker.run(|list| {
            for (id, base) in &bases {
                // SAFETY: an open list; live buffers of one size.
                unsafe { list.CopyResource(base, &masters[id]) };
            }
        })?;
        tracing::info!(
            buffers = bases.len(),
            mb = total >> 20,
            "bases of merged copies"
        );
        let flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        let counts = gpu::buffer(&self.device, D3D12_HEAP_TYPE_DEFAULT, CALLS * 12, flags)?.0;
        self.merger = Some(Merger {
            root,
            pso,
            bases,
            counts,
            calls: Default::default(),
        });
        Ok(())
    }

    /// What the merges of the last run (of `loops`, which add up) found.
    pub(super) fn merge_notes(&mut self, loops: u32) -> Result<Vec<String>> {
        let Some(m) = &self.merger else {
            return Ok(Vec::new());
        };
        let (counts, calls) = (m.counts.clone(), m.calls.borrow().clone());
        let (back, data) = gpu::buffer(
            &self.device,
            D3D12_HEAP_TYPE_READBACK,
            CALLS * 12,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        self.worker.run(|list| {
            // SAFETY: an open list; live buffers.
            unsafe { list.CopyBufferRegion(&back, 0, &counts, 0, CALLS * 12) };
        })?;
        // SAFETY: the mapping; the copy into it has run.
        let all = unsafe { std::slice::from_raw_parts(data.cast::<u32>(), CALLS as usize * 3) };
        let each = |k: usize| {
            all.iter()
                .skip(k)
                .step_by(3)
                .map(|&n| u64::from(n))
                .sum::<u64>()
                / u64::from(loops.max(1))
        };
        let mut out = vec![format!(
            "{} copies of what the CPU wrote to memory the card writes: {} words put; {} left as the card wrote them in this run, where other values were to go ({} of them the values from before the card wrote)",
            calls.len(),
            each(0),
            each(1),
            each(2)
        )];
        let mut worst: Vec<(u32, usize)> = (0..calls.len())
            .map(|i| (all[i * 3 + 2], i))
            .filter(|w| w.0 > 0)
            .collect();
        worst.sort_by_key(|a| std::cmp::Reverse(a.0));
        for (n, i) in worst.into_iter().take(8) {
            let (id, at, len) = calls[i];
            out.push(format!(
                "    values from before the card wrote: {} words, {} +{at:#x} {len:#x} bytes",
                n / loops.max(1),
                self.called(id)
            ));
        }
        Ok(out)
    }
}
