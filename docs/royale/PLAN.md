# victory-royale on kroeeee's tree: plan

Copied from the README of this repository's `main` (section 0). The documents it links are under `docs/royale/` here: `forks/`, `design/`, the journal, baseline and memory notes, and the M.2 patches (`patches/`).

## New base: moving to kroeeee's tree (decided 26 Nov 2026)

**Decision.** Work moves from our base (joshuu/victory-amd plus this roadmap) to [kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX](https://github.com/kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX): avatarmanz/VIctory-7900XTX (the cool_fox family) with the fixes it takes under Proton on RADV. Branch **`k7900`** in this repository; the old line stays as tag **`archive/victory-amd-2026-11-26`** and as `main` until `k7900` replaces it.

### Why

Measured on this machine (RX 7900 XT, Ryzen 9 9900X, 32 GB), Jason's house, 26 Nov ([docs/forks/kroeeee-linux-7900.md](forks/kroeeee-linux-7900.md)):

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
| K.3 | **P.4, device method slot 0x218**: pipelines from the console's precompiled shader binaries; no fork implements it (~290-380 draws a frame dropped in every tree) | `device.rs` dumps, [docs/design/p4-pipeline-methods.md](design/p4-pipeline-methods.md) | a new method on their device | missing objects back in every fork |
| K.4 | **Phase 2 / M.2**: GPU memory at the title's addresses (`VK_EXT_map_memory_placed`, dma-buf aliases), the vkd3d-proton and winevulkan patches, the rule of what the GPU may share | `patches/m2/`, `inplace.rs`, [docs/design/m2-vram-hybrid.md](design/m2-vram-hybrid.md) | on top of their renderer port 1 (the 32-bit heap opened on private pages) | the copies (~40 ms) and copy-back (~20 ms) are their biggest pieces |
| K.5 | **Findings**: the title's CPU reads GPU memory ~600k times a frame (mostly windows); page-granular tracking forces the copy-back; slots must not change under running work (C.1, `SHOW_AFTER`); RADV per-submit tracing needs a trigger and a big buffer | [docs/journal.md](journal.md) | as notes and design rules | avoids repeating dead ends |

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
