# victory-royale (private)

VIctory with a Linux focus. The goal is an architecture that runs an Xbox Series title **smoothly and stably** under GE-Proton, instead of the 2-4 fps of today's forks.

> This repository contains no game files. Everyone uses their own copy, dumped from a console they own. The code is GPL-3.0-or-later, like VIctory.

**Contents**

0. [New base: moving to kroeeee's tree](#0-new-base-moving-to-kroeeees-tree-decided-26-nov-2026)
1. [Where it starts from](#1-where-it-starts-from)
2. [Why it is slow today](#2-why-it-is-slow-today)
3. [How GPU submission works today](#3-how-gpu-submission-works-today)
4. [The roadmap](#4-the-roadmap)
5. [Working on this repository](#5-working-on-this-repository)
6. [Task board](#6-task-board)

Also: [Known blocker: timing-dependent device loss](#known-blocker-timing-dependent-device-loss).
7. [Risks and open decisions](#7-risks-and-open-decisions)
8. [Building and running](#8-building-and-running)
9. [Branches and remotes](#9-branches-and-remotes)

Documents:

| Document | Contents |
|---|---|
| [docs/memory.md](docs/memory.md) | every figure about memory, and the design rules drawn from them |
| [docs/baseline.md](docs/baseline.md) | baseline and ceiling measurements |
| [docs/bench.md](docs/bench.md) | how to reproduce a measurement |
| [docs/journal.md](docs/journal.md) | what was tried and decided, by date |
| [docs/history/es/](docs/history/es/) | the original Spanish documents |

---

## 0. New base: moving to kroeeee's tree (decided 26 Nov 2026)

> **Now (26 Nov evening): read [`k7900`'s docs/royale/STATUS.md](../../tree/k7900/docs/royale/STATUS.md)**: 17-19 fps at Jason's house with the view-tree wineserver (PR #1 to the author), P.4 in progress.

**Decision.** Work moves from our base (joshuu/victory-amd plus this roadmap) to [kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX](https://github.com/kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX): avatarmanz/VIctory-7900XTX (the cool_fox family) with the fixes it takes under Proton on RADV. Branch **`k7900`** in this repository; the old line stays as tag **`archive/victory-amd-2026-11-26`** and as `main` until `k7900` replaces it.

### Why

Measured on this machine (RX 7900 XT, Ryzen 9 9900X, 32 GB), Jason's house, 26 Nov ([docs/forks/kroeeee-linux-7900.md](docs/forks/kroeeee-linux-7900.md)):

| | our tree | kroeeee's tree |
|---|---|---|
| fps (median over a run) | ~2.7 | ~6 (peaks 11-13.7; the author gets ~16 on the same card) |
| GPU a frame | ~165 ms | ~10-16 ms |
| what the frame is | prepare ~120 → GPU ~170 → copy-back ~60, in series, ~2 cores busy | prepare ~80 (copies ~40), copy-back ~20, GPU small; many threads |

Today's own work closed C.1 at −2 to −7 %. The gap is architectural (their translation off the title's threads, async submission, renderer ports, guards that do not ask Wine), not something polishing our base closes. The author offered to work on it together.

### What carries over, and how

| # | Piece | From | Into kroeeee's tree | Value |
|---|---|---|---|---|
| K.1 | **Pipeline library fix**: an empty `ID3D12PipelineLibrary` made with a null blob through the raw vtable (vkd3d's `d3d12_pipeline_library_init` rejects a non-null pointer with length 0; an empty Rust slice is non-null) | `pipeline/library.rs` (P.1) | their log says "no host pipeline library: pipelines are not kept" | world entry 9.5 → 0.63 s for us; cutscenes on time |
| K.2 | **Measurement tooling**: `scripts/bench.sh`, `trials.sh` (window-only screenshots, per-run tables), `perf-symbols.py`, `cpu-reads.py`, `rgp-size.py`; the G.1 GPU census by pipeline | `scripts/`, `host.rs` | adapted to their log lines (`… fps, … ms/frame (… preparing …)`) and their launcher | every later step is measured the same way |
| K.3 | **P.4, device method slot 0x218**: pipelines from the console's precompiled shader binaries; no fork implements it (~290-380 draws a frame dropped in every tree) | `device.rs` dumps, [docs/design/p4-pipeline-methods.md](docs/design/p4-pipeline-methods.md) | a new method on their device | missing objects back in every fork |
| K.4 | **Phase 2 / M.2**: GPU memory at the title's addresses (`VK_EXT_map_memory_placed`, dma-buf aliases), the vkd3d-proton and winevulkan patches, the rule of what the GPU may share | `patches/m2/`, `inplace.rs`, [docs/design/m2-vram-hybrid.md](docs/design/m2-vram-hybrid.md) | on top of their renderer port 1 (the 32-bit heap opened on private pages) | the copies (~40 ms) and copy-back (~20 ms) are their biggest pieces |
| K.5 | **Findings**: the title's CPU reads GPU memory ~600k times a frame (mostly windows); page-granular tracking forces the copy-back; slots must not change under running work (C.1, `SHOW_AFTER`); RADV per-submit tracing needs a trigger and a big buffer | [docs/journal.md](docs/journal.md) | as notes and design rules | avoids repeating dead ends |

Not carried over: our copy model's fixes (S.x, I.1/I.2, C.1 switches). Their tree has its own answers (`INDIRECT_BATCH=2`, `INDIRECT_WINDOW`, `THIN`, `ASYNC_SUBMIT`, `SHOW_AFTER`).

### Steps, in order

1. **K.0 Base and baseline.** Build `k7900` (`cargo xwin build --release -p victory --target x86_64-pc-windows-msvc`), run it with `~/Games/victory-data/run-kroeeee.sh` (their `linux/victory-proton.sh` settings, Proton Experimental, prefix and saves of its own: profile `perfiles/kroeeee`). Baseline: median fps, ms a frame, preparing, copies, GPU, copy-back, and "asking the write watch and the guards" over a whole run, standing and moving. Done once on 26 Nov (6.0 fps median, write watch ~20 ms median).
2. **K.1 Pipeline library.** Port the null-blob fix; check the log no longer says "pipelines are not kept"; measure world entry and cutscene loading against K.0. Offer it to the author.
3. **K.2 Tooling.** Make `bench.sh`/`trials.sh` launch `k7900` and parse its frame line; keep the run tables in `bench/`.
4. **K.6 Write watch on this machine.** Ours is 10-46 ms a frame (median ~20) against the author's 2-4. Tested and ruled out with the author: zram (disk swap only: same), two CCDs (pinned to CPUs 0-5,12-17: 20.4 ms), THP for shmem (already `advise`). In test: ntsync (`PROTON_NO_NTSYNC=1`). Then: kernel 7.3 against his 7.2, `perf` on the guard paths (mprotect, faults, `NtGetWriteWatch`).
5. **K.3 P.4 (slot 0x218).** From our dumps and fxceless's `VICTORY_NATIVE_PSO_DUMP`: decode the precompiled binaries' container, map them to the shaders we already translate, make the pipeline.
6. **K.4 Phase 2 on port 1.** Rebuild the M.2 mechanism on their private-page heap: which buffers can live in VRAM (static ones first; per-frame dynamic data was what broke the picture for us), measured by the copies and copy-back they remove.
7. **K.7 Streaming dips.** Turning the camera: 2-5 fps, the frame up to ~250 ms longer than its listed parts (the title waiting on I/O and page mappings). Measure where the title's threads wait (perf, Wine's file and mapping calls).

### Working rules

- `k7900` follows kroeeee's `main` by merge (`git fetch kroeeee && git merge kroeeee/main`); our changes stay small commits that could go upstream as they are.
- Each step: a switch where it changes behaviour, measured against the baseline, written in the journal, then on by default if it holds.
- Nothing is published to the author's repository (issues, PRs, comments) unless asked; public messages are written in the first person singular.
- The game folder is never touched; the game is never started while the user plays.

---

## 1. Where it starts from

The base is [JoshuuDev/victory-amd](https://github.com/JoshuuDev/victory-amd) at `003beef` (25 Nov 2026). It is the fork that renders the world best. It comes from this chain:

| Layer | Repository | What it brings |
|---|---|---|
| 1 | [Marvisak/VIctory](https://github.com/Marvisak/VIctory) `ffe310e` | loader, Xbox system emulation (HLE), D3D12 backend, shader translator |
| 2 | [fxceless/victory-fork](https://github.com/fxceless/victory-fork) | the world draws: cutscenes and the prologue with textures |
| 3 | [myokex/victory-amd](https://github.com/myokex/victory-amd) | AMD fixes: textures by true memory, split dispatches, clamped views |
| 4 | JoshuuDev/victory-amd | DirectStorage fix, more shaders translated, remap lock per 16 MB, frametime dump |

**Why not [ligmaSec/victory-royale](https://github.com/ligmaSec/victory-royale):**
- It is upstream VIctory plus ~700 commits of open PRs, plus a Linux launcher and CI.
- It lacks fxceless's rendering work; by its own README the world is mostly black.
- Merging it into joshuu gives **494 conflicts in 63 files**, almost all in the GPU core: they are two independent rewrites of the same code.

Its full history is kept on the `royale-base` branch. The parts worth having are ported by hand (task 0.6).

## 2. Why it is slow today

**Measurement setup:**
- RX 7900 XT with RADV (Mesa 26.3.1), Ryzen 9 9900X, GE-Proton11-14.
- The [patched wineserver](https://github.com/datingesp-hash/victory-proton).
- Reference scene A ([docs/bench.md](docs/bench.md)), commit `176cc10`.

| Measure | Value |
|---|---|
| frame | **356 ms (2.8 fps)**, variation <1 % between runs |
| VIctory preparing the frame | 124 ms, of which 62 ms copy **215 MB** to the GPU |
| GPU | 159 ms; the card is busy **45 %** of it |
| copy-back into title memory | 68 ms, **79 MB** |
| hash passes over copies in VRAM | 558 per frame |
| game CPU | 1.4 of 24 cores; the least-waiting title thread waits 82 % of the time |
| title waits on GPU fences | none |

On the console, CPU and GPU share **one memory**. On a PC they are separate, so every frame VIctory has to:

1. Work out what the title wrote, with page guards, write watch and hashes.
2. Upload it.
3. Let the GPU draw.
4. Copy what the GPU wrote back into title memory.

All of it runs **in series**: the title, then preparing, the GPU, the copy-back. Nothing is saturated: the GPU waits for the CPU and the CPU waits for the copies. Windows shows the same; fxceless runs at ~2 fps there too.

**Measured ceiling:** with copies switched off, the frame drops from 354 to 170 ms (×2, ~6 fps). The rest is the serial chain itself. So the two levers are:
- **stop copying memory** (phases 1-2);
- **let CPU and GPU overlap** (phase 3).

Shader caching and streaming are not the limit, and the GPU is not either. Details in [docs/memory.md](docs/memory.md).

## 3. How GPU submission works today

This is the code the roadmap changes. Paths are relative to `crates/`.

| Piece | Where | What it does today |
|---|---|---|
| Title memory | `victory-hle/src/modules/xmem.rs` | Title GPU memory is PC memory with write watch. Physical pages are a pool of `SEC_RESERVE` sections, shown as 64 KiB views by `map_alias` and `map`. There is a remap lock per 16 MB. |
| Resources | `victory-gpu/src/resource.rs` | Heaps and resources are PC memory, and the GPU address equals the CPU address, as on the console. Textures get a host copy. |
| Buffer copies | `victory-gpu/src/shadow.rs` | Every title buffer a draw uses gets a persistent host copy. Before each submission, the ranges it reads are brought up to date. |
| What the title wrote | `victory-gpu/src/watch.rs` | Write watch (dirty 4 KiB pages) plus a journal for overlapping copies. With the view guard, a write through one alias marks the others. |
| Textures | `victory-gpu/src/texture.rs` | Detile from console memory into staging, then copy into the host texture when it changed. |
| Submission | `victory-gpu/src/host.rs`, `Host::submit` | **One submission at a time, from preparation to copy-back:** `run_deferred` + `batch.flush`, then `card_before` (VRAM copies, hashes), `ExecuteCommandLists` + `Signal`, wait, then `card_back` + `hand_back` + `read_backs`. The code itself calls this a shortcut: *"every submission is waited for, so CPU and GPU never overlap"*. |
| Submitting ahead | `host.rs`, `late` / `relay` | `VICTORY_LATE_WAIT` and `VICTORY_RELAY` try to overlap; on AMD they fault the GPU as loading ends ([fxceless/victory-fork#184](https://github.com/fxceless/victory-fork/issues/184)). |
| In-place memory | `victory-gpu/src/inplace.rs` | `VICTORY_INPLACE=1/2` opens private title memory as a GPU heap with a buffer placed over it. **It cannot take the page pool**. `VICTORY_INPLACE_PROBE` reports what the driver allows. |
| Queues and fences | `victory-gpu/src/queue.rs`, `fence.rs` | Lists run and are waited for, so the title's fence signals complete at once. |
| Frame pacing | `victory-gpu/src/frame.rs` | An interval clock standing in for the console's vblank. |
| Measuring | `frametime.rs`, `medir.rs` | `VICTORY_FRAMETIME_DUMP=1` writes a CSV row per interval. `VICTORY_MEDIR=1` adds remap counters and the title's waits. `VICTORY_COPY_MODE=plain/none` changes or skips copies, for timing only. |

## 4. The roadmap

The order is fixed, and each phase depends on what the previous one measured.
- A phase closes when its **exit criteria** hold in the reference scene.
- Frame-rate targets are **hypotheses**: they get confirmed or corrected by measurement, in [docs/journal.md](docs/journal.md).
- Time estimates are rough.

### The target: pieces that fit this machine

The roadmap builds one architecture for **one platform**: Linux, GE-Proton, vkd3d-proton, RADV, an RX 7900 XT (RDNA3, 20 GB) and a Ryzen 9 9900X (24 threads). Other forks tune for Windows and NVIDIA. We tune for what this platform gives us:
- **ReBAR is on:** BAR0 is 32 GiB, so the CPU can address all of the VRAM (`/sys/bus/pci/devices/0000:03:00.0/resource`, 25 Nov).
- **Spare CPU:** the title uses 1.4 of 24 threads ([memory.md](docs/memory.md) §8).
- **An open stack:** we already patch the wineserver, and vkd3d-proton, wine's ntdll and RADV's settings are all ours to change.
- **Linux kernel tools:** soft-dirty bits and `userfaultfd` track writes without page faults through Wine.

Each piece owns one job and states what it guarantees to the others. A phase builds or replaces pieces; it does not add a shortcut that a later phase must undo.

| Piece | Job | Guarantee to the others | Today | Target on this machine | Phases |
|---|---|---|---|---|---|
| **M** Memory placement | where each kind of title memory lives for the GPU | one GPU address per title range, valid while any list can reach it | every buffer copied into a host copy | by kind (§2.2): VRAM through ReBAR for what the CPU writes and the GPU reads heavily; opened title memory for small or CPU-read data; plain VRAM for GPU-only data | 1.1, 2 |
| **W** Write tracking | what the title changed since the GPU last saw it | no missed write; cost proportional to what changed | write watch, guards, 558 hash passes, kernel reads | write watch asked once per submission; reads without system calls (done, 1.8); then soft-dirty or `userfaultfd` (track L) | 1.2, 1.8, L |
| **B** Copy-back | what the GPU wrote that the CPU will read | the CPU never reads a stale byte | 79 MB a frame, on the critical path | only ranges the CPU reads, fetched when touched (1.3); none for in-place memory (2) | 1.3, 2.5 |
| **L** Lifetimes | when a host object can be freed or a page remapped | nothing is freed or remapped while a list can reach it | time and frame heuristics; frees the GPU still reads | exact references plus fence-based deferred release | S.2, 3.2 |
| **T** Textures | console-tiled images to host images | the host image matches title memory at every draw | detile on change detection that misses some updates | change detection without relying on the sweep; detiling on spare cores | S.3, 1.6, 3.3 |
| **Q** Submission and fences | ordering CPU and GPU work | the title's fence signals mean what they mean on the console | every submission waited for | frames in flight, signals at GPU completion, **copy-back off the critical path first** | 3.1, 3.5 |
| **D** Descriptors and views | title descriptor tables to host descriptors | a view never reaches past its resource | a fault outside every copy of ours, likely a view out of range | views checked against their resource; descriptor records mirrored when the title copies them as bytes (fxceless `a26859f`) | S.1, S.5 |
| **P** Pipelines | title shaders and states to host pipelines, before they are needed | no draw waits on a compile in the frame; no draw is dropped for good | translation cached on disk, but each pipeline created on first bind on the submitting thread: ~3,000 in the first 2-3 minutes of the open world, one frame of 10 s (26 Nov play log) | a journal of the pipelines each run creates (inputs, not outputs); replayed on spare cores during the loading screen; the driver's cache saved at calm moments; a pipeline still missing in play built on a worker while its draw waits a few frames. Model: KytyPS5's shader journal, pipeline journal and driver-cache saver (`~/kyty/KytyPS5/src/graphics/host_gpu/renderer/pipeline/`) | P.1-P.3 |
| **A** Asynchronous uploads | getting title data (textures, buffers) to the card off the submitting thread | data reaches the card before the first list that reads it, in order | every new texture made, detiled and copied on the submitting thread inside the frame; the world comes in black for tens of seconds | a copy queue of its own (the 7900 XT's SDMA), fed by workers on spare cores, signalling a fence the graphics queue waits on; with the pool (S.6), no allocation on that path | A.1-A.2 |

**Lessons that fixed this order (25 Nov, branch `fx`, [journal](docs/journal.md)):**
1. **Overlap before the copy-back is small makes Linux slower.** fxceless's relay thread overlaps submissions but lands the copy-back on the title's critical path. On RADV the title then waits ~430 ms a frame: 610 ms per frame against our 317 ms. So **B (1.3) comes before Q (3.1)**.
2. **Another fork's speed-up is a hypothesis here, not a result.** fxceless measures on Windows and NVIDIA. Its changes enter our base one at a time, behind a switch, and only when `bench.sh` on this machine shows a gain (track U).
3. **Every switch must work in release builds.** Three of ours and fxceless's were diagnostics-only, which made weeks of runs mean something else.
4. **A measurement window must hold one scene.** A pause screen once read as a 4× speed-up; `bench.sh` now marks such windows `invalid`.

### Standing tracks (alongside the phases)

| Track | What | Rule |
|---|---|---|
| **U** Upstream intake | review fxceless, joshuu and upstream commits ([AGENTS.md](AGENTS.md): `vr-upstream`) | port one change at a time, behind a switch, measured here; record the verdict in the journal |
| **L** Linux write tracking | prototype soft-dirty (`/proc/self/pagemap`) or `userfaultfd` write-protect, alone, outside the game | it replaces W only if the prototype beats today's write watch per 4 KiB page; needs a Unix side (a Wine unixlib or a helper process) |
| **G** Gates | the 30-minute run, the open-world scene and the screenshots | every phase passes them before merging into `main` |

### Phase 0: base, reference scene, measurement — **done** (25 Nov 2026)

| # | Task | Status |
|---|---|---|
| 0.1 | Base (joshuu `003beef`); `royale` launcher profile building `~/src/victory-royale` | done |
| 0.2 | Reference scene: new game, skip the first cutscene, first playable moment. A = static 60 s, B = walking 30 s | done |
| 0.3 | `scripts/bench.sh` (`run`, `mark`, `summary`, `compare`) | done |
| 0.4 | `VICTORY_MEDIR=1`: remap counters, title waits by thread and caller, CSV columns `remap_ms`, `maps`, `unmaps` | done |
| 0.5 | Ceiling experiments: no copies, plain copies, in-place, probe | done ([docs/baseline.md](docs/baseline.md)) |
| 0.6 | Port from royale: `652aed4` (indirect-argument clamp through DXC when D3DCompile refuses it) and `0d34233` (checked same-process read for copy-backs under Wine). Both conflict with joshuu, so they are redone by hand. Optional: the launcher installs `d3dcompiler_47` anyway | **open** (good first task) |

**Exit criteria:**
- Variation under 10 %: met (<1 %).
- Ceilings written down: met.
- Not done yet: three runs of scene B, and a 10-minute stability run (every run so far lasted 2-3 minutes without a device loss).

### Phase 1: make today's copies cheaper

**Goal:** cut the cost of copying inside the current architecture. The changes are small, measurable and reversible. They also tell us which memory is used how.

**Estimate:** 3-5 days.

Each task follows the same rules:
- its own branch and commit;
- its own switch (`VICTORY_F1_*`), on by default only when it wins;
- `bench.sh compare` before and after, in the commit message.

| # | Task | Where | How |
|---|---|---|---|
| 1.1 | **Memory census by use** | `shadow.rs`, `watch.rs`, `host.rs` | Classify every copied range (CPU-writes/GPU-reads, GPU-writes/CPU-reads, GPU-only, texture), with size and frequency per frame. Logged with `VICTORY_MEDIR=1`. It is the input for 1.3 and for all of phase 2. |
| 1.2 | **Cheaper write tracking** | `watch.rs`, `host.rs::card_before` | Replace the 558 hash passes per frame with the write-watch answer: only compare what has dirty pages, and ask once per submission. Measure `VICTORY_ASK_ONCE` first. |
| 1.3 | **Copy back only what the CPU reads** — **first after phase S; it gates 3.1** | `host.rs`: `card_back`, `hand_back` | Using the census, skip GPU-written ranges the CPU never reads. Guard them against reading (`PAGE_NOACCESS` or a guard page) and fetch only when the title touches them. Target: 79 MB down to under 10 MB per frame. |
| 1.4 | **Persistent staging** | `host.rs`: `upload`, `scratch`, `take_chunk` | Reused, 64 KiB-aligned upload rings instead of creating resources per frame. Check `perf` first to see whether allocation shows up. |
| 1.5 | **Fewer alias views** | `xmem.rs`: `map_alias`, `map` | Merge contiguous pages into one view when source and target are contiguous. On 24 Nov the average was 1.0 page per call; check whether joshuu's `948e9d3` and `476a020` changed that. Drop the task if there is no contiguity. |
| 1.6 | **Textures only when they change** | `texture.rs` | Check write watch before detiling; do not detile mips that did not change. |
| 1.7 | **Do not re-copy our own write-back** — **done** | `shadow.rs`: `note_own`, `copy_comparing`, `card_merge`; `host.rs`: compare mode 2 | `VICTORY_F1_OWN` and `VICTORY_F1_CARD_MERGE`, on by default. Scene A: uploads 216 → 110 MB, card sends 93 → 18 MB, hash passes 558 → 103, GPU 159 → 143 ms, frame 354 → 348 ms. Copy time did not move (63-66 ms), which pointed to 1.8. |
| 1.8 | **Fast reads of title memory** — **next** | `host.rs`: `copy_mapped`, `read`, `read_into`, `read_sparse`, `read_pieces`; `watch.rs` (handler); `xmem.rs` (remap notices) | Every read of title memory today is a `ReadProcessMemory` call per piece, so that pages with no backing do not crash. Under Wine that per-call cost, not the bytes, is the copy time. Steps: **(a)** profile scene A (`bench.sh run --perf`) to see where the time goes: `NtReadVirtualMemory` in ntdll, a wineserver round trip, or the kernel; **(b)** a "known readable" mark per range, set by a clean read and cleared by any remap, decommit or free (xmem already reports remaps), with plain `memcpy` for marked ranges; **(c)** a safety net: an access violation inside such a `memcpy` is caught by our vectored handler, the copy is abandoned and redone through the kernel; **(d)** batch what still needs the kernel into fewer, larger calls; **(e)** measure. Target: copy time and compare time well under half. |

**Exit criteria:**
- Scene A: prepare under 40 ms and copy-back under 20 ms.
- No visual regressions: screenshots of scene A at the same framing as the baseline.
- The census (1.1) is written up in [docs/memory.md](docs/memory.md).
- Hypothesis: 4-6 fps.

> **Measured ceiling ([memory.md](docs/memory.md) §5):** phases 1-2 alone give ×2 at most. If phase 1 stalls, tasks 3.1-3.2 (frames in flight) move ahead of phase 2.

### Phase S: stability foundation — **now, before phases 2-3** (reordered 25 Nov 2026)

**Why it moved ahead.** Phase 5 was meant to run "in parallel". Instead, every speed-up so far (fast writes, the forks' late wait) ran into the same bug, and the one open-world scene we have (the user's saved game, Jason) loses the device or draws wrong data in release builds. Phase 2 rewrites exactly the memory paths where this lives, and phase 3 makes timing tighter. Building speed on top would put pieces in before the base holds.

| # | Task | Exit |
|---|---|---|
| S.1 | **Wrong reads / device loss** ([Known blocker](#known-blocker-timing-dependent-device-loss)), with the saved game as the reproducer | release build loads Jason 5 of 5 with the right picture |
| S.2 | **Exact lifetimes** for host copies (design rule 7 in [memory.md](docs/memory.md)) | fast writes on: no device loss in 5 prologue runs |
| S.3 | **Texture coherence:** no reliance on the sweep recreating copies to pick up changes | `VICTORY_TEXTURE_FREE=0`: same picture as with freeing |
| S.4 | **Two reference scenes:** prologue (A, B) and open world (Jason, `royale-save`), both measured with `bench.sh --auto` | baselines for both in [baseline.md](docs/baseline.md) |
| S.5 | **Descriptor records copied as bytes** (piece D): evaluate fxceless `a26859f` (*"byte-copied descriptor records are mirrored, views off their copy's elements get their own bytes"*, 1,188 lines in 13 files) as a cause of the wrong reads; port it behind a switch if the census shows such copies in the Jason scene | the Jason loss rate with and without it, 5 runs each |

The phase closes when S.1-S.4 hold. Then phases 2 and 3, measured in both scenes.

**Note:** fxceless sees the same washed-out picture (~6 of 21 runs) and an open-world device loss on its own base, so moving to another base would not have removed this phase.

### Phase 2: shared CPU/GPU memory, no copies

**Goal:** the GPU reads and writes **title memory directly**, as on the console. This is the main architectural change.

**Estimate:** 5-10 days.

#### 2.1 What the platform allows

**Already measured** ([memory.md](docs/memory.md) §3-4):
- **Private memory in place works** on vkd3d-proton with RADV: reads, writes, coherence and write watch all behave.
- **The page pool through sections does not:** `SEC_RESERVE` is refused, and with `SEC_COMMIT` the GPU does not see the views.
- **Reading in place over PCIe** (~27 GB/s) makes GPU time 2-2.5× longer.

**Known about this machine:** ReBAR is on (BAR0 32 GiB covers all 20 GB of VRAM), so CPU-visible VRAM is not limited to 256 MB.

**Still to measure:**
- A microbenchmark of GPU bandwidth reading 200 MB from VRAM, from ReBAR and from system memory, and of **CPU write bandwidth into ReBAR** (the title would write vertices and constants straight into it).
- Whether `D3D12_HEAP_TYPE_GPU_UPLOAD` (ReBAR) exists in this vkd3d-proton. Its log already says *"HVV access is viable"*.

#### 2.2 Placement by kind of memory

Based on the census from 1.1:

| Kind | Where it lives | Copies |
|---|---|---|
| CPU writes, GPU reads lightly (constants, arguments) | title memory opened as a GPU heap | none |
| CPU writes, GPU reads heavily (vertices, indices) | CPU-visible VRAM (ReBAR); the title writes straight into it | none |
| GPU writes, CPU reads (queries, results) | system memory opened in place, coherent | none |
| GPU only (render targets, intermediates) | plain VRAM | never, unless the CPU reads it (1.3) |
| Console-tiled textures | VRAM, with detiling | only when they change |

#### 2.3 Rebuild the `xmem` page pool

This is the largest job of the phase.

**Constraint confirmed by the probe:** sections do not work. The GPU's pool must come from private memory or VRAM, and the console's page mapping must become **tile mappings on the GPU side**: reserved resources plus `UpdateTileMappings`, which vkd3d turns into sparse binding. The CPU keeps its own views.

**To work out first:** how many aliases exist only for the GPU (tiled resources) and how many for the CPU. Title ranges also have pages with no backing ([memory.md](docs/memory.md) §6), so the design must tolerate holes.

#### 2.4 Coherence and synchronisation

- Zero-copy regions lose their write guards and hashes. The title's own barriers and fences are enough, as on the console.
- **Condition:** the host CPU must not read a region while the GPU writes it. That holds today because every submission is waited for; in phase 3 fences will guarantee it.
- `VICTORY_INPLACE=2` already documents this (*"Do not combine 2 with late wait"*). It also lost the device after 60 s in the world, which has to be understood first.

#### 2.5 Steps, each measured

1. Extend `VICTORY_INPLACE` to all private title memory.
2. Constants and indirect arguments without copies.
3. Vertices and indices in ReBAR, if the 2.1 bandwidth figures justify it.
4. The page pool opened in place (2.3), behind `VICTORY_ZEROCOPY=1`.
5. Drop the copy-back of what is now in place.

**Exit criteria:**
- MB uploaded and copied back per frame fall by more than 80 % against the baseline.
- The frame is limited by the GPU or by the title's own CPU, not by VIctory (`cpu_prepare_ms` below `gpu_ms`).
- Same picture as the baseline, and 30 minutes without a device loss.
- Hypothesis: 10-20 fps together with phase 3.

### Phase 3: overlap and frame pacing

**Goal:** CPU and GPU working at the same time. Once copies are gone (phase 2), what remains is the wait on every submission.

**Estimate:** 4-7 days.

| # | Task | Where | How |
|---|---|---|---|
| 3.1 | **Frames in flight** (needs 1.3: lesson 1) | `host.rs` (`submit`, `settle`), `queue.rs`, `fence.rs` | Drop *"every submission is waited for"*. Each submission carries its fence value, and the title's signals complete when the GPU reaches them. Allow 2-3 frames in flight. What `late` and `relay` do today gets rewritten on top of zero-copy memory, which does not have the problem of reading half-remapped pages. |
| 3.2 | **Safe remapping** | `xmem.rs` + `host.rs` | A page the GPU is using is not remapped until its fence passes: a deferred-release queue. This is what breaks `VICTORY_LATE_WAIT` on AMD today. |
| 3.3 | **Multithreaded recording** | `list.rs`, `draw.rs` | The title records from 5 threads. Translate each list on its own thread, not on the submitting one. |
| 3.4 | **Non-blocking pipelines** | `pipeline/make.rs`, `library.rs`, `apps/victory/src/translate.rs` | Measure how long draws still wait for pipelines. **Fix the `victory warm` hang**: with pipelines on RADV, it waits forever once the compute pipelines are done. Run `warm` from the launcher after each update. |
| 3.5 | **Frame pacing** | `frame.rs`, `host.rs::present` | A bounded present queue and steady latency. |

**Exit criteria:**
- The GPU is busy more than 70 % of the time in scene A.
- The p99 frame time is under twice the median.
- No hitches when scene B is repeated.
- 30 minutes on AMD without a device loss.

### Phase 4: native Vulkan backend

**Only if the measurements justify it.**

**Estimate:** weeks.

Today the chain is: console shader → DXIL → dxil-spirv → SPIR-V → RADV, and D3D12 → vkd3d-proton → Vulkan.

1. **Decide with data.** After phases 2-3, measure with `perf` the share of the frame spent in vkd3d-proton, dxil-spirv and winevulkan. If it is under 15 %, the phase is postponed.
2. **SPIR-V straight from `victory-shader`'s IR**, as a second backend next to `emit` (DXIL).
3. **A Vulkan backend in `victory-gpu`** behind a trait shared with D3D12:
   - native sparse binding for tiled heaps;
   - direct `VK_EXT_external_memory_host`;
   - `VK_EXT_descriptor_buffer` for the title's descriptor heaps.
4. **Linux write tracking** moved to track L, so its prototype does not wait for this phase.
5. **Wine stays.** The title is a Windows PE importing the Windows API; dropping Wine would mean rewriting all of that.

**Exit criteria:** same picture as D3D12, a measurably shorter frame, and no loss of stability.

### Phase 5: stability and visual completeness

The correctness items that block speed work moved to **phase S** (above). What is left here: missing shaders, ray tracing, and the 30-minute gate of every phase.

| Topic | Status | What to do |
|---|---|---|
| AMD fault in `fc5e66f8f873d3bb` ([fxceless#3](https://github.com/fxceless/victory-fork/issues/3)) | skipped today. Two runs without the skip did not fault in 2-3 minutes | find the faulting access and fix the translation, then test 10+ minutes without the skip |
| GPUVM fault with late wait ([#184](https://github.com/fxceless/victory-fork/issues/184)) | avoided | fixed by 3.2 |
| 58 shaders `warm` refuses | open | by family: *"descriptor read only as data"*, `v_cvt_f32_f64`, hardware SGPR input, descriptor chosen by control flow, *"a lane mask holding lanes elsewhere read as data"* |
| **Heads missing (eyes and mouth only)** | **parked**: the game does not emit the head draws in 15 of 18 runs; all or nothing per run ([memory.md](docs/memory.md) §7) | re-check after each phase; then compare the GPU-written buffers the CPU reads at world load, between a run with heads and one without |
| Flickering textures | open (seen 25 Nov) | check for partially uploaded textures (`texture.rs`, `VICTORY_HEAP_RECHECK`) |
| Intro video flicker | open | screenshots |
| Ray tracing (black shadows) | not run | DXR on vkd3d-proton, after phase 3 |
| `VICTORY_INPLACE=2` device loss | open | needed for phase 2.4 |
| **Texture sweep frees copies the GPU still reads** | confirmed (0 losses of 3 with the sweep off, 2 of 3 with it on) | find the unheld reference (vkd3d breadcrumbs); blocks fast writes and phase 3 |
| **Texture copies go stale?** | unproven: `VICTORY_TEXTURE_SWEEP=0` also turned off the slot repair pass, the likelier cause of the missing pieces | retest with `VICTORY_TEXTURE_FREE=0` (freeing off, repair on) |
| Exit gate of every phase | | 30 minutes in the world without a device loss or crash |

## Known blocker: timing-dependent device loss

> **Cause found (25 Nov, late).** Host descriptor slots were written into the shader-visible heap while a submission ran on the GPU. `VICTORY_SLOT_WAIT` (now on by default) makes those writes wait: the open-world save holds **5 of 5** with the right picture, and fast writes hold too (3 of 3). Details below, under *Resolution*; the history is kept because it shows what was ruled out.

**Symptom**
- The GPU faults: `radv: GPUVM fault detected at address 0x8003…`. The kernel shows `[gfxhub] page fault`, client TCP (a shader memory read).
- The device is lost (`0x887A0005`), usually while a world loads or as the first cutscene starts.

**When it happens**

| Case | Rate |
|---|---|
| Loading the user's saved game (Jason; launcher profile `royale-save`), release build | **4 of 4** — the reproducer |
| Prologue with fast writes (`VICTORY_F1_FAST_WRITE=1`) | 2-3 of 4 |
| Prologue with the forks' late-wait switches (`VICTORY_LATE_WAIT`, `VICTORY_RELAY`) | 3 of 4 (24 Nov) |
| Prologue with today's defaults | 0 of ~30 |

**What keeps it away**
- Diagnostics build with `VICTORY_D3D12_DRED=1`: 0 of 4, and the picture is right.
- `VKD3D_CONFIG=…,breadcrumbs_sync`, or `RADV_DEBUG=hang`, which serialise the GPU (0.2 fps).
- In the release build, `VICTORY_DRED_DEVICE=both` lives only 1 time in 2, and that run's picture was almost white.

**What it is not**
- Not the texture sweep alone: with every sweep off (`VICTORY_SWEEP=0`), the save still loses the device.
- Not a release-only code path: every `cfg(not(feature = "diagnostics"))` block was read, and all are equivalent.
- Not a broken command: serialising the same commands avoids it.

**Open hypotheses**

Ruled out:
- the texture sweep;
- objects freed through `retire()` (`VICTORY_LEAK_RETIRED=1`: still 3 of 3);
- barriers between commands (`VICTORY_SERIAL_OPS=all`: still 2 of 2);
- DirectStorage's parallel workers (`VICTORY_DSTORAGE_WORKERS=1`: still 2 of 2);
- the card copies (`VICTORY_CARD_BUFFERS=0`: still 2 of 2);
- phase 1's changes (all three off: still 2 of 2; the base has the bug).

Left, since only the CPU waiting for an idle GPU helps:
- GPU work our wait does not cover: the **present** (vkd3d's swapchain blit on its own queue), vkd3d's internal queues;
- work vkd3d keeps in flight past our wait;
- **an out-of-range raw read (leading):** several fault addresses sit exactly on 1-2 MB boundaries, which is what a shader reading past the end of an allocation into unmapped space looks like. Raw addresses (root descriptors, title memory by address) have no bounds checks.

**Status before the resolution (25 Nov):**
- `VICTORY_ALLOC_DUMP` placed a fault about 740 MB away from every live buffer copy and window. With the TCP (texture unit) client, it points to a **texture view reading beyond its image** (more mips or slices than the host copy has).
- **Next step:** check every texture view against its host texture and count the mismatches. Details and every run are in [docs/journal.md](docs/journal.md) (entries of 25 Nov).

**Why it mattered:** it blocked fast writes, frames in flight (phase 3) and the open-world scene for measurements.

**Resolution (25 Nov, late; [journal](docs/journal.md))**
1. A reliable reproducer: `scripts/trials.sh` with the Jason save loses the device within about a minute of the world drawing (10 of 12 runs).
2. Ruled out: mirroring descriptors the title copies as bytes (`VICTORY_HEAP_SYNC=1`, 5 of 5 lost); every object we make leaked (`VICTORY_LEAK_ALL=1`, 4 of 4 lost); VRAM pressure (at most 8.2 of 20 GB).
3. **`VKD3D_CONFIG=fault`** names the resource: the fault is inside a **texture made ~2 s before** the loss is reported, at an address that is *"currently mapped"*.
4. **Mechanism:** `descriptor/host.rs` `make()` writes host slots lazily from the title's threads, **15,600-58,300 a frame while a submission runs**. Work in flight that reads such a slot sees a resource whose page-table update only the next submission waits for.
5. **`VICTORY_SLOT_WAIT=1`:** a slot write waits until no submission runs. Open world: 5 of 5 held, 13,000 draws, 2.1 fps, right picture; ~90 writes a frame wait.

**Still open:** a different, rarer fault seen once in the prologue with the wait on (SQC reading address 0, right after the sweep let ~5,000 texture copies go); being measured. The wait is the minimal form; phase 3 needs slot writes staged at submission boundaries (task S.6).

## 5. Working on this repository

1. **One branch per task**, named after it (`p1.2-write-watch`, `p5-warm-hang`…). Merge into `main` once it passes its phase's gate. Pull `main` often; rebase your branch rather than merging `main` into it.
2. **Measure first.** Every performance commit carries a `bench.sh compare` table (before, after, change in scene A) in its message. Every number in the docs names its source.
3. **Every architectural change sits behind a switch** until it becomes the default. That way runs compare within one build, and anything can be switched off without rebuilding.
4. **Write it down.** Add a dated entry to [docs/journal.md](docs/journal.md) for what you tried, what came out of it and what you decided, failures included. Memory figures go to [docs/memory.md](docs/memory.md).
5. **Everything in the repository is in English:** code, comments, docs, commit messages.
6. **No game data in the repository:** no files, screenshots, saves or names. Comparison screenshots stay in `bench/`, which is not committed.
7. **Other forks come in through track U:** one change at a time, behind a switch, kept only if `bench.sh` on this machine shows a gain. Do not merge a fork's branch wholesale.
8. **Test on Linux with the launcher.** It sets up wine, vkd3d-proton, the patched wineserver and the read-only sandbox. Do not launch anything while someone is playing on the same machine: starting our wineserver kills the running game.
9. **Claim a task** on the [task board](#6-task-board) before starting it, so two people do not work on the same code.
10. **Agents:** the pi subagents and their rules are described in [AGENTS.md](AGENTS.md). They write code and read results; game runs are done by the maintainer.

## 6. Task board

In roadmap order. Piece letters refer to [the target](#the-target-pieces-that-fit-this-machine).

| Task | Piece | Owner | Branch | Status |
|---|---|---|---|---|
| S.1 Wrong reads / device loss, Jason as reproducer | D | maintainer | `s5-heap-sync` | **found**: slot writes during a submission; `VICTORY_SLOT_WAIT=1` holds 5 of 5 |
| S.6 Host textures in pre-mapped heaps ([design](docs/design/s6-premapped-textures.md)); the proper form of the wait, needed for 3.1 | M, D, Q | maintainer | `s5-heap-sync` | **done**: `pool.rs`, on by default; no wait needed (5 of 5 against 8 of 8 lost); not faster (the earlier "~6 %" came from invalid windows) |
| S.8 Bound texture memory | T, L | maintainer | `s5-heap-sync` | the sweep works with the pool again (25,800 live, 9.2 GB); left: release empty heaps |
| D.1 Heap buffer pass: 133 ms a frame in the open world (`draw.rs` `heap_buffers`, 110,000 views, 4,085 copies) | D, W, A | maintainer | | split: a 231 ms full rescan every 16 passes (`VICTORY_HEAP_ROLL=1` spreads it, measuring) and 6-115 ms of streaming textures on the recording thread (→ A.2) |
| M.2 Title memory shared with the card (VRAM at the title's addresses, [design and status](docs/design/m2-vram-hybrid.md)) | M | maintainer | | **parked 26 Nov**: the mechanism works under Wine (patched winevulkan + vkd3d, `patches/m2/`); small/mid buffers in place break the picture (hypothesis: per-frame data rewritten while the card reads); all switches off |
| M.1 One host copy per title byte ([design](docs/design/m1-one-copy-per-range.md)): the overlap is a buffer copy and a 4 MiB window over the same memory (two names) | M | maintainer | | `VICTORY_ONE_COPY=1` (window answers buffer asks) breaks the picture: off; redesign |
| G.1 GPU census by pipeline in release (`VICTORY_GPU_CENSUS=1`): where the GPU's ~265 ms a frame go | G, P | maintainer | | **next** (26 Nov reorder: the GPU is the biggest piece; the copy-back needs phase 2) |
| I.1 Indirect records clamped in the list's prelude in one run (`VICTORY_INDIRECT_BATCH`) | Q, D | maintainer | | **done, on by default**: GPU 272 → 200 ms |
| I.2 Records the list's own shaders write: windows (`VICTORY_INDIRECT_WINDOW`, on by default) | Q, D | maintainer | | **done**: inline clamps 9,700 → ~830, GPU 187 ms, frame ~400 ms (2.5 fps); a clamp after the writer (Neptune) for the last ~830 if they matter |
| S.9 Interior lighting: white speckles and blown-out areas indoors (Jason at the TV); seen again in play on 26 Nov (`bench/user-play-20261126/shot-now.png`): dark interior, characters as black silhouettes, frost-like white speckles on floors and tables, windows blown out | T | *open* | | seen twice |
| P.1 Pipeline census and the library: compile time, cache hits, the load into the world | P | maintainer | | **library fixed** (`c436241`, it was never made): world-entry frame 9.5 s → 0.63 s, 993 pipelines from the library and 1,012 started early on the second run; timing counters in |
| P.2 Pipeline journal and replay at start-up on spare cores (KytyPS5's model) | P | *open* | | after P.1 |
| P.3 Pipelines missing in play built on workers; the draw waits a few frames instead of the frame waiting | P | *open* | | after P.2 |
| P.4 Two pipeline creation methods not implemented ([notes](docs/design/p4-pipeline-methods.md); device `0x1c8`, `0x218`, `device.rs` `unknown_pipeline`): the title goes without those pipelines (16 + 13 logged calls, up to thousands; no fork implements them, upstream included). In play: ~320 draws and ~980 indirect draws a frame dropped for want of a host pipeline, minutes in (26 Nov play log); a candidate for S.9 and missing objects | P | *open* | | to decode: the descriptions are (pointer, size) pairs into the title image |
| A.1 Upload census: time and bytes of texture and buffer uploads on the submitting thread, during the load and in play | A | maintainer | | detiling is 0.0-0.4 ms a frame: `heap_textures`' 6-115 ms is texture creation, mirrors and mapped-page queries; that is A.2's target |
| A.2 Uploads on a copy queue of their own, fed by workers | A | *open* | | after A.1 |
| S.7 Prologue null fault (SQC, address 0): 1 of 6 prologue runs with the wait | L | maintainer | | a likely cause (root view left at 0, `draw.rs`) guarded with zeros; 6 of 6 held since, the guard never fired: 1 in 12; watched |
| S.5 Evaluate fxceless `a26859f` (descriptor mirroring) | D | maintainer | `s5-heap-sync` | **done**: ported (`VICTORY_HEAP_SYNC=1`, off); not the cause; the title copies ~225,000 records a frame as bytes |
| S.4 Open-world baseline (Jason, outside the house) | G | maintainer | `s5-heap-sync` | **done**: 497 ms (2.0 fps), GPU-bound, a serial chain ([baseline.md](docs/baseline.md)) |
| S.2 Exact lifetimes for host copies | L | *open* | | after S.1 |
| S.3 Texture coherence without the sweep | T | *open* | | after S.1 |
| 1.3 Copy back only what the CPU reads ([design](docs/design/p1.3-copyback-on-demand.md)): read census first | B | maintainer | | **in progress**: **closed for now**: the copy-back is the byte merge that page-granular write tracking needs (~125 MB a frame of GPU bytes would come back stale without it); its ceiling is −22 %; it goes with phase 2 (in-place memory) |
| 1.2 Cheaper write tracking (ask write watch once per submission) | W | *open* | | |
| 1.1 Memory census by use | M | maintainer | `p1.1-census` | in progress (input for 1.3 and phase 2) |
| 1.7 Stop re-copying our own write-back (`VICTORY_F1_OWN`, `VICTORY_F1_CARD_MERGE`) | B | maintainer | `p1.1-census` | **done**: uploads −49 %, hash passes −82 %, frame −2 % |
| 1.8 Fast reads of title memory (`crate::fastcopy`) | W | maintainer | `p1.8-fast-reads` | **done**: frame 354 → 317 ms |
| 1.9 Fast writes of title memory (`VICTORY_F1_FAST_WRITE=1`) | B | maintainer | `p1.8-fast-reads` | safe with the wait (3 of 3), but no gain in the open world (2.03-2.15 fps); off |
| 2.1 GPU and CPU bandwidth microbenchmark: VRAM, ReBAR, system memory | M | *open* | | independent research |
| L.1 Soft-dirty / `userfaultfd` prototype, outside the game | W | *open* | | independent research |
| U Review fxceless, joshuu and upstream since our base | — | agents (`vr-upstream`) | | standing |
| 0.B Baseline on a second machine (7800X3D, RX 7900 XTX, 32 GB) | G | collaborator (suggested) | | first task: learn the flow, second data point |
| 0.6 Port royale's DXC clamp and copy-back read | — | *open* | | good first task |
| 3.4 Fix the `victory warm` hang with pipelines on RADV | — | collaborator (suggested) | | independent; needs Linux |
| 5 `fc5e66f8` AMD fault: find the access and fix instead of skipping | — | collaborator (suggested) | | shader translator; needs an AMD card |
| 5 Refused shaders (58), by family | — | *open* | | shader translator |
| 5 `VICTORY_INPLACE=2` device loss | M | *open* | | needed before 2.4 |

**Branch `fx`** (fxceless/main plus our Linux pieces) is kept for reference and for track U. It is not the base: on this machine it runs at 610 ms per frame.

## 7. Risks and open decisions

| Risk | Mitigation |
|---|---|
| vkd3d-proton lacks what phase 2.3 needs (tile mappings over opened private memory) | Test reserved resources plus `UpdateTileMappings` over an `OpenExistingHeapFromAddress` heap early in phase 2. vkd3d-proton is open source and can be extended. |
| GPU reads over PCIe are slower than copying | Already measured (2-2.5× GPU time). Heavily-read data goes to ReBAR or stays copied, only when it changes. |
| The console's CPU aliases do not fit an opened pool | Alias census in 2.3. Keep CPU views for the CPU and use tiles for the GPU. |
| joshuu and fxceless change the same code at the same time | Pull them in at the start of each phase. Small changes behind switches. |
| A phase does not deliver | The ceilings say so in advance. If a phase does not move the figures, record it in the journal and move on. |
| More speed exposes more races (GPU faults) | The 30-minute gate at each phase. `VICTORY_D3D12_DEBUG=1` in the diagnostics build. |

**Open decisions** (settled with data when their time comes):
- 2.2: ReBAR or opened system memory for vertices and indices?
- 3.1: 2 or 3 frames in flight?
- 4.1: does phase 4 happen?
- If phase 1 stalls: does phase 3.1-3.2 move ahead of phase 2?

## 8. Building and running

With the [victory-proton](https://github.com/datingesp-hash/victory-proton) launcher:

```sh
victory-proton profile add https://github.com/datingesp-hash/victory-royale-private royale main
victory-proton -p royale                                # update, build, run
victory-proton -p royale victory warm --no-pipelines    # translate shaders ahead of time
scripts/bench.sh run -l my-test VICTORY_SOMETHING=1     # measure (docs/bench.md)
```

The repository is private: git must be able to authenticate to GitHub (`gh auth setup-git`).

Building by hand from Linux, with [cargo-xwin](https://github.com/rust-cross/cargo-xwin):

```sh
cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
```

The base fork's own notes, with its switches and diagnostics, are in [docs/fork-joshuu-README.md](docs/fork-joshuu-README.md).

## 9. Branches and remotes

| Branch | What it is |
|---|---|
| `k7900` | **the new base**: kroeeee's tree plus our pieces (section 0) |
| `main` | the old line: joshuu plus the roadmap (until `k7900` replaces it; tag `archive/victory-amd-2026-11-26`) |
| `fx` | fxceless/main plus our Linux pieces; reference only (track U) |
| `royale-base` | ligmaSec/victory-royale as it was, for reference |
| `p<phase>.<task>-<name>` | work in progress per task |

```sh
git remote add joshuu   https://github.com/JoshuuDev/victory-amd
git remote add fxceless https://github.com/fxceless/victory-fork
git remote add upstream https://github.com/Marvisak/VIctory
git remote add royale   https://github.com/ligmaSec/victory-royale
git remote add kroeeee  https://github.com/kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX
```
