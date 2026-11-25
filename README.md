# victory-royale (private)

VIctory with a Linux focus. The goal is an architecture that runs an Xbox Series title **smoothly and stably** under GE-Proton, instead of the 2-4 fps of today's forks.

> This repository contains no game files. Everyone uses their own copy, dumped from a console they own. The code is GPL-3.0-or-later, like VIctory.

**Contents**

1. [Where it starts from](#1-where-it-starts-from)
2. [Why it is slow today](#2-why-it-is-slow-today)
3. [How GPU submission works today](#3-how-gpu-submission-works-today)
4. [The roadmap](#4-the-roadmap)
5. [Working on this repository](#5-working-on-this-repository)
6. [Task board](#6-task-board)
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
| 1.3 | **Copy back only what the CPU reads** | `host.rs`: `card_back`, `hand_back` | Using the census, skip GPU-written ranges the CPU never reads. Guard them against reading (`PAGE_NOACCESS` or a guard page) and fetch only when the title touches them. Target: 79 MB down to under 10 MB per frame. |
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

### Phase 2: shared CPU/GPU memory, no copies

**Goal:** the GPU reads and writes **title memory directly**, as on the console. This is the main architectural change.

**Estimate:** 5-10 days.

#### 2.1 What the platform allows

**Already measured** ([memory.md](docs/memory.md) §3-4):
- **Private memory in place works** on vkd3d-proton with RADV: reads, writes, coherence and write watch all behave.
- **The page pool through sections does not:** `SEC_RESERVE` is refused, and with `SEC_COMMIT` the GPU does not see the views.
- **Reading in place over PCIe** (~27 GB/s) makes GPU time 2-2.5× longer.

**Still to measure:**
- A microbenchmark of GPU bandwidth reading 200 MB from VRAM, from ReBAR and from system memory.
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
| 3.1 | **Frames in flight** | `host.rs` (`submit`, `settle`), `queue.rs`, `fence.rs` | Drop *"every submission is waited for"*. Each submission carries its fence value, and the title's signals complete when the GPU reaches them. Allow 2-3 frames in flight. What `late` and `relay` do today gets rewritten on top of zero-copy memory, which does not have the problem of reading half-remapped pages. |
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
4. **Linux write tracking:**
   - `userfaultfd` write-protect mode, or soft-dirty bits from `/proc/self/pagemap`, instead of page faults handled by Wine.
   - This needs a Unix side: a Wine unixlib, or a helper process sharing memory through `memfd`.
   - Prototype it in isolation first to see whether it pays.
5. **Wine stays.** The title is a Windows PE importing the Windows API; dropping Wine would mean rewriting all of that.

**Exit criteria:** same picture as D3D12, a measurably shorter frame, and no loss of stability.

### Phase 5: stability and visual completeness

**Runs in parallel from phase 1.** Every phase uses it as its exit gate.

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
| **Texture sweep lets go of copies the GPU still reads** (suspected) | under test | 2,196 copies let go 1.4 ms before a device loss; exposed by any speed-up (fast writes, late wait). Test with `VICTORY_TEXTURE_SWEEP=0`; fix the order in `Host::submit` (`sweep` before `make_waiting`) or count pending slots as holders |
| Exit gate of every phase | | 30 minutes in the world without a device loss or crash |

## 5. Working on this repository

1. **One branch per task**, named after it (`p1.2-write-watch`, `p5-warm-hang`…). Merge into `main` once it passes its phase's gate. Pull `main` often; rebase your branch rather than merging `main` into it.
2. **Measure first.** Every performance commit carries a `bench.sh compare` table (before, after, change in scene A) in its message. Every number in the docs names its source.
3. **Every architectural change sits behind a switch** until it becomes the default. That way runs compare within one build, and anything can be switched off without rebuilding.
4. **Write it down.** Add a dated entry to [docs/journal.md](docs/journal.md) for what you tried, what came out of it and what you decided, failures included. Memory figures go to [docs/memory.md](docs/memory.md).
5. **Everything in the repository is in English:** code, comments, docs, commit messages.
6. **No game data in the repository:** no files, screenshots, saves or names. Comparison screenshots stay in `bench/`, which is not committed.
7. **Pull in joshuu and fxceless** at the start of each phase: `git fetch joshuu && git merge joshuu/victory-amd`. If one of their changes touches the same code as a phase in progress, integrate it first.
8. **Test on Linux with the launcher.** It sets up wine, vkd3d-proton, the patched wineserver and the read-only sandbox. Do not launch anything while someone is playing on the same machine: starting our wineserver kills the running game.
9. **Claim a task** on the [task board](#6-task-board) before starting it, so two people do not work on the same code.

## 6. Task board

| Task | Owner | Branch | Status |
|---|---|---|---|
| 1.1 Memory census | maintainer | `p1.1-census` | in progress |
| 1.2/1.3 Stop re-copying our own write-back (`VICTORY_F1_OWN`, `VICTORY_F1_CARD_MERGE`) | maintainer | `p1.1-census` | **done**: uploads −49 %, card sends −80 %, hash passes −82 %, frame −2 % |
| 1.8 Fast reads of title memory (`rep movsb` + fault recovery, `VICTORY_F1_FAST_READ`) | maintainer | `p1.8-fast-reads` | **done**: frame 354 → 317 ms, prepare −17 % |
| 1.9 Fast writes of title memory (copy-back, `VICTORY_F1_FAST_WRITE=1`) | maintainer | `p1.8-fast-reads` | **off**: lost the device once as loading ended; reproducing |
| 1.2 Cheaper write tracking | maintainer | | after 1.1 |
| 1.3 Copy back only what the CPU reads | maintainer | | after 1.1 |
| 0.B Baseline on a second machine (7800X3D, RX 7900 XTX, 32 GB) | collaborator (suggested) | | first task: learn the flow, second data point |
| 0.6 Port royale's DXC clamp and copy-back read | *open* | | good first task |
| 3.4 Fix the `victory warm` hang with pipelines on RADV | collaborator (suggested) | | independent of phases 1-2; needs Linux |
| 2.1 GPU bandwidth microbenchmark (VRAM, ReBAR, system memory) | *open* | | independent research |
| 5 `fc5e66f8` AMD fault: find the access and fix instead of skipping | collaborator (suggested) | | shader translator; needs an AMD card |
| 5 Refused shaders (58), by family | *open* | | shader translator |
| 5 `VICTORY_INPLACE=2` device loss | *open* | | needed before 2.4 |

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
| `main` | our line: joshuu plus the roadmap |
| `royale-base` | ligmaSec/victory-royale as it was, for reference |
| `p<phase>.<task>-<name>` | work in progress per task |

```sh
git remote add joshuu   https://github.com/JoshuuDev/victory-amd
git remote add fxceless https://github.com/fxceless/victory-fork
git remote add upstream https://github.com/Marvisak/VIctory
git remote add royale   https://github.com/ligmaSec/victory-royale
```
