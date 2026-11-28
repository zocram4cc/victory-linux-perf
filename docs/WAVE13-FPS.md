# Wave 13 FPS notes

Verified on branch `integration/upstream-port-2` against the Rust sources (grep), not against a running game. The notes below are kept. Sentences that were wrong for this tree are corrected in place and marked **[branch]**.

Names that no Rust file reads are written **without** the `VICTORY_` prefix, as `NAME (not a switch in this tree)`. `cargo xtask` rejects a markdown file that names a switch the code does not read. Do not put the prefix back on those names.

| Written here | What this branch actually has |
|---|---|
| `HEAP_RECHECK` | Switch exists. Default **on** unless the value is `0` (`draw.rs`). |
| `PIPELINE_WAIT_MS` | Switch exists. Default **500 ms** per frame (`pipeline/make.rs`). |
| `SCREEN_WAIT_MS` | Switch exists. Default **10 000 ms** if unset. |
| `COMPUTE_WAIT_MS` | Switch exists. Default **10 000 ms** if unset. The launcher and preset 4 set **5000**. Do not lower it (character faces). |
| `LANE_CORE` | Switch exists. Default **off**. Exact value `1` turns it on (`cores.rs`). |
| `LEVELS_ON_DEMAND` | Switch exists in `crates/victory-gpu/src/tiles.rs` (not a `texture/` path). Default **off**. Exact `1` turns it on. |
| `PORT_CONSTANTS` | Switch exists (`port.rs`, `xmem.rs`). Off unless the value is exactly `1`. Not enabled by the experimental preset. |
| `INPLACE` | Switch exists (`inplace.rs`). `0` / `1` / `2`, default `0`. Not enabled by the experimental preset. |
| `TITLE_AFFINITY`, `CCD_SPLIT` | Not in this tree. Upstream pull request 3635 was not ported. The preset does not set them. |
| `PORT_BUFFERS` | Not in this tree. Do not invent it. The in-place switch above is the one that exists. |
| `GPU_DETILE` | Not in this tree. Source commit `3461f52f` was not ported. |
| `PORT_CONSTANTS_ORDER` | Not in this tree. Ordering commits `631400f5`, `15c3d0ea`, `3e04ca0a` were not ported. |
| `warm --used` | Supported. `warm_shaders.bat` already runs `RAZDA4A_XEN.exe warm --used --jobs 6`. |

The experimental preset sets only `HEAP_RECHECK=0`, `PIPELINE_WAIT_MS=16`, and `LANE_CORE=1`, on top of the balanced 16 GB launcher block. It does not change the default profile.

---

# FPS research for RAZDA4A XEN: what the VIctory community has found, and what we can take

Written 2026-11-28 (Europe/Moscow, UTC+3). Read-only research: nothing was cloned for editing and nothing was pushed or posted.
Our context: dev `host.rs` ≈13k lines, heavily rewritten; main = Wave 11, dev = source, `integration/upstream-port-2` = PR #6.
Target case: CPU-bound city (GPU ≈15% busy, submit ≈42 ms, long freeze), ≈31 FPS native / 62–63 with FG on RX 9060 XT + 7800X3D, 16 GB RAM, memory creeps up over a session.

## 0. How to read this file

* **Every number is quoted from its source and nothing more.** Most are single-machine, often self-reported by AI-assisted READMEs. Treat them as hypotheses to A/B on our box, not as promises.
* "Applicable" = likely / maybe / no, judged against the files of ours I could read (`host.rs`, `shadow.rs`, `inplace.rs`, `cores.rs`, `port.rs`, `land.rs`, `thin.rs`, `list.rs`, `queue.rs`, `behind.rs`, `ab.rs`, `send.rs`, `watch.rs`, `crates/victory-gpu/src/tiles.rs` **[branch]**, `pool.rs`, `library.rs`, `pipeline.rs`, `pipeline/make.rs`, `draw.rs`, `descriptor/host.rs`, `card_steps.rs`, `sampler_order.rs`) and the launcher `RAZDA4A XEN.bat`.
  GitHub code search on our repo returned 0 hits (index is default-branch only and stale), so "we don't have switch X" means **not found in the files I fetched**. Grep dev for it before porting.
* The repos are mostly AI-assisted and very noisy; hashes can be rewritten by force-push. Always re-fetch the commit before cherry-picking.
* Hardware tags: RDNA = AMD; "NV" = NVIDIA. Our machine is RDNA 4 (9060 XT).
* Effort is a rough size: **config** (launcher line only), **small** (<150 lines), **medium**, **large**.

### Headline findings

1. **The community consensus is the same as ours: the frame is the single submitting thread's burst, not pixel work.** Frame-time tables from other cards say the same (RTX 5090/Wine: 136 ms CPU vs 155 ms GPU commands, upstream #3425; RTX 5080: "card stays idle behind the submitting thread", trustno1-coder `docs/findings.md`).
2. **We already have most of the community's big wins**: THIN translation workers, async submit lane, JOIN_PARTS, NtWriteVirtualMemory copy-back, thread-local quick-find cache in `shadow.rs`, core placement (`cores.rs`), A/B tooling (`ab.rs`), late-wait via relay, PORT_GUARD/JOURNAL/DESCRIPTORS, packed DynamicBits in `descriptor/host.rs`, own send thread (`VICTORY_SEND_OWN`), library store-later, sampler order. Do **not** re-port those.
3. **The levers still unused by us** are mostly *switches that exist in our tree but are not set in the launcher* (`VICTORY_HEAP_RECHECK=0`, `VICTORY_PIPELINE_WAIT_MS`, `VICTORY_LANE_CORE=1`, `VICTORY_LEVELS_ON_DEMAND=1`, `VICTORY_PORT_CONSTANTS`/`INPLACE`) and a few things we do not have (GPU detile, `PORT_BUFFERS`, TITLE_AFFINITY ignore, user-mode title copy, memory-stack trims).
4. **The honest gain of the in-place family is small.** The best independent A/B (HAL9000RELOADED, 7900 XTX, Win10) shows −3.2…−4.9 ms CPU preparation and only −0.9…−1.7 ms frame. The README "+5 FPS" claims mix many changes.
5. **Warning from two sources**: THIN=1/JOIN_PARTS can hurt (RTX 5080 + 6-core Ryzen 5 9600X: 14 FPS with, 21–28 without; trustno1-coder `985b3f70`). THIN_THREADS scaling can help (Copperframe: THIN_THREADS=14 gave +2.8 FPS warm on a 7900 XT-class machine). Our THIN_THREADS=4 deserves a sweep.

---

## 1. Idea catalogue

Format: **ID – name** · Source · What it does · Claimed gain (hardware) · Applicable? · Risk · How to port / gate.

### Group A: config-only (switch already exists in our tree, launcher change)

**A1 – `VICTORY_HEAP_RECHECK=0`**
* Source: upstream issue #3505 by YUYING234 (edsuuu/VI-BR-PORT notes) https://github.com/Marvisak/VIctory/issues/3505 ; `docs/benchmarks-4060-laptop-8gb.md` in https://github.com/edsuuu/VI-BR-PORT
* What: stops re-looking at textures picked by heap index after their first upload (our `draw.rs::heap_recheck`, default on).
* Claimed: RTX 4060 Laptop 8 GB, i9-14900HX: prepare 44 → 29 ms/frame, copies 12 ms/70 MB → 10 ms/55 MB; "no visual difference" (source). Frame rate did not change there because GPU was the limit.
* Applicable: **likely.** Directly cuts host-side prepare in a CPU-bound frame. Switch already exists in `draw.rs` (`VICTORY_HEAP_RECHECK`).
* Risk: our code comment says the recheck exists to catch textures the title paints through another view (otherwise "shown four frames late, over and over") → possible stale/late textures in animated surfaces (water, screens, UI render targets). Check visually.
* Port: none. Add `set VICTORY_HEAP_RECHECK=0` to launcher profile, A/B with `VICTORY_AB` if it supports it, or two runs on the same save/scene. Compare "prepare" and "HEAP_TEXTURES_RECHECKED" counters in the frame line.

**A2 – Pipeline wait budget: `VICTORY_PIPELINE_WAIT_MS` (and `VICTORY_COMPUTE_WAIT_MS`, `VICTORY_SCREEN_WAIT_MS`)**
* Source: upstream PR #2960 (ynnmuraii) https://github.com/Marvisak/VIctory/pull/2960 ; DoubleGhosty/cool_fox README (mirror of codec-d/cool_fox, which is now not visible to us) https://github.com/DoubleGhosty/cool_fox : "`VICTORY_PIPELINE_WAIT_MS=0` & `VICTORY_LIBRARY_STORE_LATER=1` in launchers to eliminate 500 ms frame stalls and 200 ms+ pipeline-library save hitches".
* What: per-frame budget the title thread may block waiting for host pipelines being made by workers. In our `pipeline/make.rs` the defaults are **500 ms per frame** (`VICTORY_PIPELINE_WAIT_MS`), **10 000 ms** for screen passes (`VICTORY_SCREEN_WAIT_MS`) and for compute (`VICTORY_COMPUTE_WAIT_MS`), with `VICTORY_COMPUTE_LATE_WAIT_MS` = 400. Our launcher sets `VICTORY_PIPELINE_WORKERS=4`, `VICTORY_LIBRARY_STORE_LATER=1`, and `VICTORY_COMPUTE_WAIT_MS=5000` (character faces; do not lower this). It does not set `VICTORY_PIPELINE_WAIT_MS` (code default 500 ms) or `VICTORY_SCREEN_WAIT_MS` (code default 10 000 ms). **[branch]** The original notes said none of the wait budgets were set. That is wrong for compute.
* Claimed: #2960: RTX 4060 8 GB, Ryzen 7 5700X: world frames 17 s → 0.3–2 s with ASYNC_SUBMIT + QUEUE2 + PIPELINE_WORKERS=4 and PIPELINE_WAIT_MS=24 (only the combination worked; each alone left 17 s frames). cool_fox: "eliminate 500 ms frame stalls" (no hardware given).
* Applicable: **likely for the "long freeze" in the city.** If the freeze is a cold pipeline being compiled when new streets load, the title thread is parked for up to the budget. Zero code.
* Risk: with a small budget draws using a not-yet-made pipeline are dropped (pop-in / missing meshes / missing compute passes such as the head blend-shape pass our comment mentions) until the worker finishes; `warm --used` before play mostly hides this. Keep SCREEN/COMPUTE budgets high (they exist to protect once-only passes).
* Port: launcher: `set VICTORY_PIPELINE_WAIT_MS=24` (A/B 0, 24, 100). Look in the log for our own messages "a pass over the screen was waited for" and "a compute dispatch waited for its host pipeline" (`pipeline/make.rs`) and the "host pipeline wait budget per frame" line. If the freeze lines match, this is the fix. Ship `warm --used` in the README (see section 2).

**A3 – `VICTORY_LANE_CORE=1`** (dedicated core for the submitting thread)
* Source: our own `cores.rs` (`VICTORY_LANE_CORE`, default off) + upstream PR #3635 (TG0000) https://github.com/Marvisak/VIctory/pull/3635 on thread placement.
* What: nothing else of the process runs on the lane's core.
* Claimed: none for this switch alone. #3635 (affinity ignore, see C1) reports 15.7 → 17.8–18.2 median FPS on Ryzen 9 5900X + RX 6800 (ReBAR) but "from a local fork that differs a lot from main", 1–2 runs.
* Applicable: **maybe.** 7800X3D is a single-CCD part (8 cores, one L3) so CCD_SPLIT is irrelevant, but a private core for the lane and avoiding SMT siblings may help the burst.
* Risk: low; can starve THIN workers on a small CPU.
* Port: launcher `set VICTORY_LANE_CORE=1`; use our `ab.rs` (`ab::pick("VICTORY_LANE_CORE")`) to flip live.

**A4 – `VICTORY_LEVELS_ON_DEMAND=1`**
* Source: cool_fox README (default on in its launchers: "card memory allocated only for texture mip levels the title holds, saving up to 3 GB of VRAM") https://github.com/DoubleGhosty/cool_fox ; our `crates/victory-gpu/src/tiles.rs` **[branch]** already has it (default **off**).
* What: host texture copies take card memory level by level.
* Claimed: "up to 3 GB VRAM" (cool_fox; our own run 189 note in `tiles.rs`: 2.2–3.0 GB of 4.3–5.5 GB of copies sat in unmapped levels).
* Applicable: **likely for the memory problem** (VRAM and residency pressure), not for CPU FPS.
* Risk: more small uploads when the title maps a new level; `LEVELS_MIN_KB` exists.
* Port: launcher `set VICTORY_LEVELS_ON_DEMAND=1`; watch the memory lines and texture-latency lines over a 20-minute session.

**A5 – `VICTORY_MEMORY_CARD_MB`**
* Source: upstream #3505 https://github.com/Marvisak/VIctory/issues/3505 : 1024 → 2048 was "worth ~6x" (1–3 fps → 13–17 fps, GPU 3–25% → 85%) on an **8 GB laptop RTX 4060**.
* Applicable: **no for us** (16 GB card: `shadow.rs` lets it grow while there is room). Only relevant to 8 GB users. Mention in FAQ for low-VRAM users.

**A6 – `DRED (not a switch in this tree)` / diagnostics must be off**
* Source: upstream PR #1215 (LooperSalty) https://github.com/Marvisak/VIctory/pull/1215 : "leaving `VICTORY_D3D12_DRED` unset (it costs about 2.5x per frame on its own)". VIctory-Fast README: `GPU_TIMING` + `DRED` "incur heavy instrumentation overhead (~3–6 fps)".
* Applicable: **already satisfied** (launcher clears inherited `VICTORY_*` and leaves DRED unset). Keep it that way, tell testers not to enable it for FPS runs.

### Group B: in-place memory / renderer ports (the "AMD tree" work: Belobug, avatarmanz, Copperframe)

**B1 – PORT_CONSTANTS + PORT_CBV (port 1) with `PORT_CONSTANTS_ORDER (not a switch in this tree)`**
* Source: Belobug/VIctory-Belobug-AMD and avatarmanz/VIctory-7900XTX (merged in XaviFortes/VIctory-RDNA, commits `631400f5`, `15c3d0ea`, `3e04ca0a`, `7ab13ed4`, `1e728610`, `8ba4f34d`) https://github.com/XaviFortes/VIctory-RDNA ; cool_fox "AMD users should use Play-VIctory-AMD.cmd which enables all ports".
* What: constant buffers are read in place by the card instead of being copied/refreshed per draw; `ORDER` fixes the flicker (trees, lamp-lit rooms) the early version caused; indirect executions whose arguments live in in-place memory are clamped alone just before they run.
* Claimed: Copperframe README gives several FPS numbers for the whole preset; the isolated A/B is in B2. cool_fox: PORT_GUARD+JOURNAL "save 2–5 ms per frame on all tested GPUs" (we already have those).
* Applicable: **likely.** We have `port.rs`, `port_cbv.rs`, `port_descriptors.rs`, `port_journal.rs`; PORT_CONSTANTS is not set in our launcher and PORT_CBV only in `cbv_exp`. The `ORDER`/`ORDER_ALONE` fix commits are the missing pieces if our `port.rs` predates them (check for "ORDER" in `port.rs`).
* Risk: flicker / pop-in (was the reason it was opt-in), interaction with late wait: our code warns **not to combine INPLACE=2 with late wait**.
* Port: cherry-pick `631400f5`, `15c3d0ea`, `3e04ca0a` (ordering) only if `port.rs` lacks them; gate = `VICTORY_PORT_CONSTANTS=1` (+ `VICTORY_PORT_CBV=1`, `PORT_CONSTANTS_ORDER (not a switch in this tree)=0` is what Copperframe's preset ships, see `8ba4f34d`). Test sequence: off / CONSTANTS / CONSTANTS+CBV, each 10 min on the same save, compare "prep" ms and screenshot the trees/lamp scene.

**B2 – `PORT_BUFFERS (not a switch in this tree)=1` + `VICTORY_INPLACE=2` (+ GPU-side copies from in-place buffers)**
* Source: XaviFortes/VIctory-RDNA `b2917b48` (PORT_BUFFERS for >4 GiB contiguous-section buffers with relay hazard tracking), `4487275b` (keep >4 GiB big buffers in local VRAM), `5e6fec2d` (allow GPU-side copies/fills from in-place buffers to avoid 7–9 ms settles), `4c210deb` (GPU-side place_copy so JOIN_PARTS stays active), `540dc6cc` (preset enables both) https://github.com/XaviFortes/VIctory-RDNA . Independent A/B: HAL9000RELOADED, `730f8e82` https://github.com/XaviFortes/VIctory-RDNA/commit/730f8e82 (docs/benchmarks/rx7900xtx-win10.md).
* Claimed (HAL9000RELOADED, RX 7900 XTX, Windows 10, same build `7c3616d`, 25-min runs): 1440p CPU preparation 27.2 → 24.0 ms, frame 35.2 → 33.5 ms; 4K CPU preparation 26.1 → 21.2 ms (−4.9), frame mean 37.1 → 36.2 ms, median 36.3 → 33.8, p90 unchanged. The author says the frame-time gain is small and the preparation gain is the solid part; the "on" run also went partly cold.
* Applicable: **maybe.** Our `inplace.rs` exists (`VICTORY_INPLACE`), `PORT_BUFFERS` not found in the files I fetched. Expected gain ≈3–5 ms of CPU prep — real but not a 2× result.
* Risk: memory (VRAM + commit) pressure on a 16 GB machine; the warning on INPLACE=2 + late wait; large patch surface in `shadow.rs`/`host.rs`/`card_steps.rs` which we rewrote.
* Port: description-level port, not cherry-pick: gate behind `PORT_BUFFERS (not a switch in this tree)=1`; start by A/B-ing the existing `VICTORY_INPLACE=1/2` alone, and only then port `4487275b` + `5e6fec2d`.

**B3 – `PORT_RT (not a switch in this tree)=1`** (in-place render targets, "Renderer port RT part 1")
* Source: avatarmanz/VIctory-7900XTX → Copperframe `84917c34`, `d13e9e0c`, `014d52de`; VIctory-Fast README ("reduce copyback & memory traffic").
* Claimed: nothing independently measured. Off by default upstream.
* Applicable: **maybe, later.** Skip for Wave 13; revisit after B1/B2.

**B4 – Skip in-place buffers in refresh/deferred/stale + parallelise `take_all` for ≥256 KiB**
* Source: Copperframe `0a075385` (and `e1f8b656`, `56638976`, `83ba8e20`: finer 4 MiB journal shards, 256 KiB WIDE_JOB fan-out, `ask_ahead` short-circuit, `VICTORY_ASK_SPREAD=1`).
* Claimed: none isolated. Our `watch.rs` already has `ASK_SPREAD` and `ASK_NORESET`.
* Applicable: **maybe.** Only if B2 is adopted.

### Group C: thread placement and scheduling

**C1 – `TITLE_AFFINITY (not a switch in this tree)=ignore` (+ `CCD_SPLIT (not a switch in this tree)=1`)**
* Source: upstream PR #3635 (TG0000) https://github.com/Marvisak/VIctory/pull/3635 ; also in maren-halv/VIctory-Fast commit `eb37550c` ("affinity/shadow TLS") https://github.com/maren-halv/VIctory-Fast .
* What: `SetThreadAffinityMask`, group affinity and ideal-processor calls from the title are remembered and answered back but not given to Windows, so console core numbers do not pin busy threads onto two SMT siblings of one core while others idle. `CCD_SPLIT` = prefer the upper L3's cores for disk workers (multi-CCD CPUs only).
* Claimed: Ryzen 9 5900X + RX 6800 16 GB (ReBAR), Adrenalin 26.11.2, Win11: median 15.7 FPS (all off, 1 run) → 18.2 / 17.8 FPS (ignore + three other switches, 2 runs); a "spread over physical cores" mode 15.2. The author states the numbers come from a local fork that differs a lot from main.
* Applicable: **likely worth a test.** 7800X3D has 8 cores / 16 threads; if the title pins its 6–7 worker threads to console numbers 0–7, they may land on 4 physical cores. Not in the files I fetched.
* Risk: low, off by default; behaviour change only for titles that depend on affinity for correctness (none reported).
* Port: small. Implement five import hooks in HLE (kernel32) returning remembered values; gate `TITLE_AFFINITY (not a switch in this tree)=ignore`. Cherry-pick source: the PR #3635 diff (read with `get_pull_request_diff`). Our `cores.rs` already manages process-level placement; make sure the two do not fight.

**C2 – `VICTORY_THIN_THREADS` sweep and THIN/JOIN_PARTS A/B**
* Source: Copperframe `47bd1166` "preset: set VICTORY_THIN_THREADS=14 (+2.8 FPS warm, −22.5 ms GPU idle)" https://github.com/XaviFortes/VIctory-RDNA/commit/47bd1166 ; upstream PR #3449 (thin workers + async lane, legayclightning22) https://github.com/Marvisak/VIctory/pull/3449 ; trustno1-coder/VIctory-NVIDIA `985b3f70` (F1 menu: switches for THIN, JOIN PARTS and ASYNC SUBMIT) https://github.com/trustno1-coder/VIctory-NVIDIA .
* Claimed: Copperframe: +2.8 FPS warm, −22.5 ms GPU idle going to 14 THIN threads (machine in that README: RX 7900 XT-class; Copperframe's `fast` preset also sets JOIN_PARTS=8). trustno1-coder: with THIN=1 + JOIN_PARTS=8, RTX 5080 + Ryzen 5 9600X (6 cores, 32 GB) missing character heads and 14 FPS; with both unset heads draw and 21–28 FPS; "Frame generation needs the line of submissions: with THIN off, ASYNC_SUBMIT=1 gives it".
* Applicable: **likely.** Ours: THIN_THREADS=4, LAND_THREADS=3, JOIN_PARTS=16 (safe 12). 7800X3D can afford 6–8 THIN threads; the 14 is for a 16-thread CPU.
* Risk: more threads = more RAM for per-thread buffers and more contention on `shadow` locks; JOIN_PARTS=16 vs 8 differences have been discussed as heads-missing/ordering bugs on NV.
* Port: config sweep first: THIN_THREADS ∈ {4, 6, 8} × JOIN_PARTS ∈ {8, 12, 16}, 3-min runs in the same city spot, measure frame ms and "GPU idle". Add a README note: 6-core CPUs should test THIN=0 + ASYNC_SUBMIT=1.

### Group D: GPU-side moves that relieve the CPU

**D1 – `GPU_DETILE (not a switch in this tree)` (compute-shader detile of title textures)**
* Source: fxceless/victory-fork `3461f52f` "GPU_DETILE (not a switch in this tree) (0/1/alt, off; detile50): staging buffers hold the title's tiled bytes and a compute pass in the list's prelude detiles them on the card (byte-exact against the CPU detile on WARP; CPU falls back for what it cannot take)" https://github.com/fxceless/victory-fork ; Hydra and VIctory-Fast ("batched compute-queue gpu_detile (oversized = 0)", "GPU Compute Detiler & BCPack Decoder", maren-halv/VIctory-Fast) and skornuta/victory-memory-stack `MEMORY_STACK.md`.
* What: moves texture detiling from CPU (during streaming) to a compute pass.
* Claimed: no isolated number in the commit. VIctory-Fast lists it among the contributors to its 37–43 FPS RX 9070 XT figure (self-reported).
* Applicable: **maybe.** City streaming spikes load texture detile on the CPU. Not found in our fetched files; commit adds `gpudetile.rs` (714 lines) + 168 lines in `victory-formats/src/texture.rs` + hooks in `host.rs`/`texture.rs`: our texture path (`texstore.rs`, `texlate.rs`, `texture/`) is different, so port by description.
* Risk: wrong swizzle modes → garbled textures (hence "byte-exact on WARP" test), extra VRAM for staging.
* Port: medium-large. Cherry-pick `gpudetile.rs` and `formats/texture.rs` (self-contained), then wire in our texture upload. Gate `GPU_DETILE (not a switch in this tree)=1`, CPU fallback on.

**D2 – `GPU_OVERLAP (not a switch in this tree)=1` (upstream #3120) / timeline of submissions (#595, #1923)**
* Source: https://github.com/Marvisak/VIctory/pull/3120 (0xkn0wh3r3), https://github.com/Marvisak/VIctory/pull/595 (wbkepler), https://github.com/Marvisak/VIctory/pull/1923 (slatefield rebase).
* Claimed: #595: RTX 5090 median 283 → 267 ms/frame, GPU wait/frame 79 → 1 ms; #3425 (RTX 5090, Linux/Wine): #3120 "~16% less frame time (before #3312)". Copperframe/Belobug side: "GPU_OVERLAP (no gain on RX 9070 XT)" in my earlier reading of the AMD forks.
* Applicable: **no.** Our `behind.rs` async lane + relay already overlaps, and our GPU is idle, so the problem is not GPU wait.

**D3 – VRAM-backed shadow buffers / page tables / dirty bitmaps (#3312, #3102, #2958)**
* Source: https://github.com/Marvisak/VIctory/pull/3312 (danielibabet), https://github.com/Marvisak/VIctory/pull/3102 (0xkn0wh3r3), https://github.com/Marvisak/VIctory/pull/2958 (moe-miau: one lane sets a mark its wave shares, RTX 4070 SUPER frames 411 → 283 ms).
* Claimed: #3425: #3312 553–563 → 386 ms on RTX 5090/Wine (biggest single gain there); #2958: 871 → 673 ms there.
* Applicable: **no / low.** All three fight PCIe traffic of discrete NV cards without `GPUUploadHeapSupported`. Our RDNA 4 with SAM/ReBAR has an upload heap; `PORT_*` already keeps data local. #2958 (shader change reducing atomic marks per wave) may still help GPU time but our GPU is idle. Skip.

### Group E: reduce per-draw/per-frame CPU work

**E1 – Per-thread shadow lookup cache (`LOOKUP_CACHE (not a switch in this tree)`) and heap worker (`HEAP_WORKER (not a switch in this tree)`)**
* Source: upstream #3535 (danielibabet) https://github.com/Marvisak/VIctory/pull/3535 : `shadow::find` is called 245 000–540 000 times per frame; "forks targeting the same hardware report ~89% hit rate and −14 ms/frame on the submit thread"; HEAP_WORKER moves `texture::refresh_all` (62–115 ms/frame on RTX per the PR) to its own thread.
* Applicable: LOOKUP_CACHE **no** (our `shadow.rs` already has the thread-local `FOUND` quick-find cache). HEAP_WORKER **maybe:** our texture queue (`texture.rs`/`draw.rs` HEAP_TEXTURES_QUEUE) is similar; check how long our `refresh_all`-equivalent runs on the recording thread in the frame line before porting.
* Risk: ordering of upload lists (PR prepends them in `Host::execute`).
* Port: HEAP_WORKER ≈ small-medium; follow the PR's `bindless::drain_worker_lists` idea.

**E2 – User-mode copy of title memory (no `ReadProcessMemory`)** (upstream #1010)
* Source: https://github.com/Marvisak/VIctory/pull/1010 (TG0000).
* Claimed: microbenchmark only, Ryzen 9 5900X: 64 B copy 801 ns → 11 ns, 256 B 796 → 81 ns, 4 KiB 3029 → 1003 ns (heap), 4096×256 B fills 2.94 ms → 0.046 ms. Author: "the in-game frame time of this branch alone was not measured".
* Applicable: **maybe.** `host.rs` still has `ReadProcessMemory` in 3 places (it uses `NtWriteVirtualMemory` for write-back already). A few ms/frame at best in a frame with 10k draws of constant buffer fills.
* Risk: vectored exception handler for faulting reads; AV in title pools if the fallback is wrong.
* Port: small (leaf routine + VEH). Gate `USER_COPY (not a switch in this tree)=1`.

**E3 – Descriptor sync / DynamicBits**
* Source: Copperframe `13e1f5ed` (packed `DynamicBits` allocator, batched `release_dynamic_many`) https://github.com/XaviFortes/VIctory-RDNA ; ToLIManl/VIctory-Hydra "descriptor-sync fingerprint"; VIctory-Fast "bulk descriptor range hooks (fallback scan 8.5 → 0.15 ms/f)" (self-reported).
* Applicable: **likely partly done** (our `descriptor/host.rs` has `DynamicBits`). Remaining idea: skip sync of unchanged descriptor ranges via a fingerprint. #3425 found "descriptor heap mirroring (384k slots) 18.1 ms" on its stack with no PR aimed at it, so this is where the community still has a gap.
* Risk: stale descriptors → GPU faults.
* Port: medium; ours is rewritten, so port the idea (fingerprint per 64-slot block, skip if equal) behind `DESC_FINGERPRINT (not a switch in this tree)`.

**E4 – Lock contention on write-guards (`watch::GUARDS`)**
* Source: upstream #3425 https://github.com/Marvisak/VIctory/issues/3425 : the lock is held ~49 ms/frame by the layer's own paths while the title spends 52 ms in its write-fault handler queuing behind it. Fix direction in cool_fox: PORT_GUARD ("write-guard lock sharing") and PORT_JOURNAL ("journal splitting"), "2–5 ms per frame on all tested GPUs".
* Applicable: **already have** PORT_GUARD/PORT_JOURNAL on. Remaining: Copperframe `watch`/`shadow` shard work (`e1f8b656`: SHARD_SHIFT=6 / 4 MiB shards), VIctory-Fast's "lock-free `FOUND`/seqlock" and "O(1) clean-shard watch check" (`7c90c19e`, `79a92c7c`).
* Port: medium, only if a lock profile (our `VICTORY_PROFILE`-style stack sampling, LooperSalty #1215 diag feature) shows `GUARDS` hot on our run. **First measure, then port.**

**E5 – Copy-back reductions (Mode-2 tripwired copyback, masked patch)**
* Source: VIctory-Fast README: "Mode-2 tripwired copyback (0.6–1.6 MB/f, −80%)", "`VICTORY_WINDOW_KB=512`", "`list::thin 2.9–3.2 µs/call (−2×)`", "+2.5–3× 1% lows in heavy scenes (p99 46–60 ms vs 132–167 ms)" (maren-halv, RX 9070 XT, self-reported); upstream `cd94d353` "shaders mark the pages they write through views; copy-backs compare only those" (in DexNom's tree).
* Applicable: **maybe.** We have `WINDOW_KB` and WC_NO_BACK. The "mark pages written via views" copy-back is upstream core work that may already be in our 17 + 6 ported commits (check `docs/upstream-sync-analysis.md`).

### Group F: memory growth over a session (the 16 GB problem)

**F1 – "Memory stack": `SLOT_EVICT (not a switch in this tree)`, `ALLOCATOR_TRIM (not a switch in this tree)`, `BUDGET_PACK (not a switch in this tree)` (+ COMPARE_PIECES, PRELUDE_POOL)**
* Source: skornuta/victory-memory-stack `MEMORY_STACK.md` and `AGENTS.md` https://github.com/skornuta/victory-memory-stack ; trustno1-coder/VIctory-NVIDIA `5986a64d`: "Memory stack on by default again: SLOT_EVICT (not a switch in this tree), ALLOCATOR_TRIM (not a switch in this tree) and BUDGET_PACK (not a switch in this tree)" https://github.com/trustno1-coder/VIctory-NVIDIA .
* What: evicts idle descriptor slots, trims allocator pools, packs the VRAM budget.
* Claimed: reduce RAM/VRAM growth in long sessions (no single headline number I can quote; see the repo doc for per-switch MB).
* Applicable: **maybe.** None of these names are in the files I fetched (our `pool.rs`, `list.rs` have `LIST_IDLE_TRIM_S`, `residency.rs` exists). Memory growth is a stated problem of ours.
* Risk: hitches when a trimmed block is needed again; correctness of evicted slots (the in-place guard).
* Port: medium; take the doc first, then cherry-pick per switch behind its own variable. Our `LIST_IDLE_TRIM_S=10` is the same idea for list allocators; try it now (config).

**F2 – Texture level residency (`LEVELS_ON_DEMAND`)** see A4. **Ring-allocated command allocators** ("frame-ring command allocators 498 → ~49 live", VIctory-Fast `dd836294`/`2472db10`, self-reported) → **maybe**, medium, small RAM win.

### Group G: shaders / GPU time (low value for us; GPU is idle)

* `RDNA_WAVE32 (not a switch in this tree)=2`, `VICTORY_SHADER_OPT=127`, `LEGACY_MUL_PLAIN (not a switch in this tree)=1` ("+2.6 FPS" claimed) in maren-halv/VIctory-Fast README (RX 9070 XT). **no for the CPU-bound case**; `LEGACY_MUL_PLAIN` changes the arithmetic of `v_mul_legacy_f32` (we ported "denormal as zero" from upstream) and may give visual differences. Only revisit if GPU time becomes the limit after the CPU fixes.
* `GPU_OVERLAP (not a switch in this tree)` (see D2), `#2472` batched argument clamp (35 ms of GPU time in #3425; irrelevant while GPU idle).

### Group H: not worth it / negative evidence

* THIN off on small CPUs (see C2) is the negative side of our current default; keep a launcher profile for 6-core CPUs.
* `RELAY=0` on RX 6800: X user szshoz's workaround (relay off on RDNA 2); cool_fox also made relay off by default on AMD (`963f7195`, issue #62). We keep `RELAY=2` and our own late-wait tests; **no change** unless a user on RDNA 2 reports hangs.
* Frame cap: `VICTORY_FRAME_INTERVAL_MS=16.667` is the same as cool_fox/Copperframe; the title otherwise caps itself at 30 fps (#2469 mentioned in #3425).
* `VICTORY_D3D12_WARP`, `GBV`, `DEBUG` = diagnostics only.

---

## 2. User-side tweaks (settings, driver, OS): candidates for README / launcher

Sources are cited; items marked "(general advice, no source)" are my suggestions only, test before claiming a gain.

1. **Pagefile / commit limit.** Upstream README: a world run takes ≈33 GB Windows commit (RAM + paging file) and ≈10 GB video memory; the tester has 32 GB RAM and a ≈80 GB commit limit; check **Committed** in Task Manager → Performance → Memory. Copperframe's troubleshooting adds error 1455 advice (enlarge the paging file). Our FAQ already says "keep a pagefile": make the launcher warn if the commit limit < 48 GB. https://github.com/Marvisak/VIctory (README)
2. **Pre-warm shaders and pipelines.** `victory shaders <archive>` then `victory warm --used` (README of cool_fox, VIctory-Fast; upstream flow). In HAL9000RELOADED's 4K A/B the run that was not warmed compiled 82 pipelines and had 414 shader misses; the warmed run compiled 6. Cold compile is the usual cause of long hitches. Put a "warm" step into `RAZDA4A XEN.bat` (we already have `warm_shaders.bat`) and tell users to re-run it after every update (translator version changes the cache key).
3. **Do not run diagnostics for gameplay.** `VICTORY_D3D12_DRED`, `VICTORY_GPU_TIMING`, `VICTORY_D3D12_DEBUG`: DRED ≈2.5× per frame (#1215), 3–6 fps (VIctory-Fast). Keep logging at `info` or lower, and test `VICTORY_LOG=warn` in one A/B run to see whether log volume costs anything (not measured by any source).
4. **Resizable BAR / Smart Access Memory on, "Above 4G decoding" on.** Upstream #3312 shows the cost of a discrete GPU **without** `GPUUploadHeapSupported` (NV 4070 without ReBAR); #3635 tester ran RX 6800 with ReBAR; X user 1700xkg's XKG Launcher advertises "Smart Access Memory uploads" for VIctory-RX9070 (https://x.com/1700xkg/status/2126150801890361699). Add a check "SAM/ReBAR enabled?" to the launcher (DXGI/`GPUUploadHeapSupported`).
5. **AFMF 2 / FSR frame generation alternative**: Copperframe README suggests AFMF 2 in Adrenalin as an alternative to the layer's FG. We already ship FG; AFMF would double-generate, so leave it off while the layer's FG is on.
6. **Close Electron apps (Discord, Chrome), keep RAM free** (already in our FAQ) because of the 16 GB limit; the memory-stack docs say the same.
7. **Windows 10 users**: use the build with the Agility SDK (`--features agility-sdk` + `D3D12Core.dll` 1.619.6 next to the exe) per cool_fox/Kemerd (`victory-win10.exe`); HAL9000RELOADED's 7900 XTX report is on Win10 and works.
8. **Power plan / Game Mode / HAGS** (general advice, no source): High performance plan, Game Mode on; HAGS on/off is a coin flip: put it in the A/B list, don't promise.
9. **Process priority**: our launcher sets HELPER_PRIORITY=1 already. (general advice) Avoid "High" for the whole process (can starve the audio/IO threads).
10. **Disable overlays** (Steam/Discord/Radeon overlay/Xbox Game Bar): general advice; the layer has its own HUD (`F3/F4` in VIctory-Fast).
11. **Render scale vs CPU**: upstream #3505 notes frame rate is nearly independent of draw count and resolution on an 8 GB laptop ("the cost is per-frame fixed overhead"). Lowering `VICTORY_OUTPUT` / FSR scale will **not** fix a CPU-bound city; it only helps the GPU-bound case. Say so in the FAQ so users stop dropping resolution.
12. **`HOME (not a switch in this tree)`/cache on the fastest NVMe** (cool_fox/trustno1-coder docs): shader cache and pipeline library on SSD, not HDD.

---

## 3. Top 10 by expected impact for our CPU-bound city case

(1 = try first. "Cost" is the effort to test it.)

| # | Idea | Why it ranks here | Cost | ID |
|---|---|---|---|---|
| 1 | `VICTORY_HEAP_RECHECK=0` | The only config switch with a published CPU-side number (prepare 44 → 29 ms, 4060 laptop); exists in our `draw.rs`; direct hit on the submit thread. Check for stale textures. | config | A1 |
| 2 | Pipeline wait budget (`VICTORY_PIPELINE_WAIT_MS` 0–24; keep SCREEN/COMPUTE) + `warm --used` in the launcher | Targets the **long freeze**. Default is 500 ms per frame in our code and the launcher does not override `VICTORY_PIPELINE_WAIT_MS` **[branch]** (it does set `VICTORY_COMPUTE_WAIT_MS=5000`); #2960 and cool_fox both report freeze removal. | config | A2 |
| 3 | THIN_THREADS / JOIN_PARTS sweep (4/6/8 × 8/12/16) | Copperframe +2.8 FPS and −22.5 ms GPU idle from THIN_THREADS=14; our value is 4. Same sweep tells us if THIN hurts (RTX 5080 case). | config | C2 |
| 4 | PORT_CONSTANTS (+PORT_CBV) with the ordering fixes (`631400f5`, `15c3d0ea`, `3e04ca0a`) | "All ports" is the AMD recommendation of cool_fox/Belobug/Copperframe; real but modest CPU-prep gain (−3…−5 ms in the best A/B). Needs flicker check and late-wait interaction test. | small–medium | B1 |
| 5 | `TITLE_AFFINITY=ignore` **[branch: not a switch here]** | +16% median on a 5900X/RX 6800 (15.7 → 17.8–18.2, weak evidence); cheap hooks; zero risk when off. | small | C1 |
| 6 | `VICTORY_LANE_CORE=1` | Already in our tree; one A/B run; plausible for a single-CCD X3D. | config | A3 |
| 7 | `PORT_BUFFERS=1` + `INPLACE=2` (Copperframe `b2917b48`, `4487275b`, `5e6fec2d`, `4c210deb`) | HAL9000RELOADED A/B: −4.9 ms CPU prep at 4K, small frame gain. Larger port, memory risk on 16 GB. After #4. | medium–large | B2 |
| 8 | Memory-stack trims: `SLOT_EVICT`, `ALLOCATOR_TRIM`, `BUDGET_PACK` (+ `LEVELS_ON_DEMAND=1`, `LIST_IDLE_TRIM_S=10`) | Not an FPS gain: fixes the session-long memory creep and the frame hitches caused by commit pressure on 16 GB. | config + medium | F1, A4 |
| 9 | `GPU_DETILE (not a switch in this tree)` (fxceless `3461f52f`) | Offloads texture detile from the CPU during city streaming; no isolated number; biggest port among the candidates. | medium–large | D1 |
| 10 | Micro-wins bundle: user-mode title copy (#1010), `HEAP_WORKER` (#3535), descriptor fingerprint (Hydra / #3425 row "18.1 ms"), `GUARDS` shard work (Copperframe `e1f8b656`) | Each is "a few ms at best", but #3425's frame budget shows these are the remaining named items on the CPU side (copy-back 50 ms, refresh checks 24 ms, descriptor mirroring 18 ms, bindless upload 10 ms). Measure our own breakdown first. | small each | E1–E4 |

Not on the list on purpose: GPU_OVERLAP (D2), VRAM shadow buffers (D3), shader flags (G): all target a GPU-bound case we don't have.

---

## 4. Suggested test plan for Wave 13 (cheap first)

Use the same save, same city spot, 3 minutes warm, `warm --used` run before. Record frame ms, "prep", "GPU idle", "copy back", pipeline-wait lines, RAM/VRAM at start and end (`ab.rs` flips switches live for the ones that read `ab::pick`).

1. Baseline (current launcher).
2. `+HEAP_RECHECK=0`.
3. `+PIPELINE_WAIT_MS=24` (then 0, 100) and look for freeze lines.
4. `+LANE_CORE=1`.
5. THIN_THREADS 6 and 8; JOIN_PARTS 8 / 12 / 16.
6. `PORT_CONSTANTS=1` (then +CBV). Screenshot the tree/lamp scene for flicker. Do **not** combine with `INPLACE=2` and late wait in the first pass.
7. `LEVELS_ON_DEMAND=1`, `LIST_IDLE_TRIM_S=10`: 20-minute memory curve.
8. Only then start the code ports: C1 (affinity hooks), B2, D1, F1.

Report format per run: switch set, FPS median / 1% low, prep ms, GPU idle %, committed GB at end, and "visual diffs seen: yes/no".

---

## 5. X (Twitter) findings (public posts, 2026-11-24 … 2026-11-28)

I stayed well inside the ≈60 X-read budget. Almost everything is benchmark roundups and clickbait; no post gave a new env var that is not already in the repos above. Useful ones:
* XKG Launcher v1.0 (1700xkg): launcher with toggles/presets, "shaders skiplist crashing AMD's driver compiler", "Smart Access Memory uploads", frame-time result after each run, VIctory-RX9070 fork of Belobug: https://x.com/1700xkg/status/2126110615014080531 , https://x.com/1700xkg/status/2126150801890361699 (idea for our launcher: per-run frame-time summary).
* Community roundups with hardware/FPS: h4nkick https://x.com/h4nkick/status/2126281905817743796 (VIctory-RDNA ≈21 fps at 4K, RX 7900 XT 20 GB, 24-min capture), marcoscamara01 https://x.com/marcoscamara01/status/2126331212725653694 (RX 9070 XT 14 fps over 37 min in open world, 7900 XTX 16–23 fps), malexc_ai https://x.com/malexc_ai/status/2126005373207527647 (RX 6600 + Ryzen 5 4500 16 GB: 6–7 / 11–12 with FG), https://x.com/malexc_ai/status/2126595596655964487 (a new low/mid-range VIctory fork by "Frazer", RX 7600 + 5700X3D ≈12 fps).
* Earlier in this task (before this summary): szshoz (RELAY=0 workaround on RX 6800), lexCodesX, orangepony1337 (VIctory-7900XTX tree/lamp flicker fix, https://x.com/orangepony1337/status/2126053792877613551), nowXono, Fooudub; none added an unlisted FPS switch.
* Ignored: the mass-reply spam posts ("2 FPS without shaders…", JackSarfatti replies) are noise.
* Our own fork is being quoted in the wild (e.g. https://x.com/soflysojoe/status/2126538858862903619, "35 native FPS … 60+ with FG"): make sure README numbers stay conservative.

---

## 6. Source list

Upstream https://github.com/Marvisak/VIctory: PRs/issues #595, #1010, #1215, #1923, #2281, #2958, #2960, #3102, #3120, #3312, #3409, #3425, #3449, #3505, #3535, #3570, #3635. (#3305 closed, #400 RTX 5070 Ti "where the ~0.8 s per frame go" read earlier.)
* #3409 (pes7): tested merge candidate of 53 open PRs; its warning is useful: "mergeable and green" PRs broke the title in combination (device removal, black picture, AVs). Use it as a **vetted list** to pick from rather than taking PRs blindly.
* #3425 (neonbyte1): frame budget of a fixed scene with per-PR results (RTX 5090 Linux/Wine); the CPU-side table is the best public breakdown of where the submit time goes.
* #3570 (KwameKai4): RX 7900 GRE + Ryzen 7 5700X3D: 4K 81.8 ms / 12.2 fps, 1080p 54.4 ms / 18.4 fps with an AMD fork patch set (write-combined retention, upload-fill forwarding, placed-copy ordering, disjoint refresh spans, single texture-queue traversal); author asks for a `VICTORY_WC_NO_BACK` A/B; we already set `WC_NO_BACK=1`.

Forks / repos read: XaviFortes/VIctory-RDNA (Copperframe; `scripts/rdna-preset.json`, docs/benchmarks, commits listed above), Belobug/VIctory-Belobug-AMD, avatarmanz/VIctory-7900XTX, HAL9000RELOADED (benchmark docs in Copperframe), fxceless/victory-fork (`3461f52f`), ToLIManl/VIctory-Hydra, skornuta/victory-memory-stack, trustno1-coder/VIctory-NVIDIA (`985b3f70`, `5986a64d`, `docs/findings.md` and two findings commits), maren-halv/VIctory-Fast (tracks "cool_fox"), DoubleGhosty/cool_fox (mirror; codec-d/cool_fox returns "not accessible"), Kemerd/VIctory-NVIDIA-RT (`SEND_THREAD (not a switch in this tree)`, `VICTORY_THIN`; README claim RTX 5090 2560×1440 DLSS Quality median 11.8 → 21.9 fps official vs fork "fast", which mixes many changes), DexNom/VIctory (RayTime, mostly shaders/RT), singlag/VIctory-Belobug-AMD, Thevynex/VIctory-AMD-Optimized, myokex/victory-amd, edsuuu/VI-BR-PORT, LooperSalty/AnyXboxX.

Caveats on trust: VIctory-Fast figures (37–43 fps on RX 9070 XT) and many README tables are self-reported by AI-assisted workflows and not reproduced elsewhere; the independent A/B in HAL9000RELOADED's report and the PR-attached numbers (#2960, #3505, #3425) are the most reliable. Several upstream PRs are still open and unreviewed.
