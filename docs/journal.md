# Journal

What was tried, what came out of it, and what was decided. Newest first. The Spanish journal before 25 Nov 2026 is in [history/es/diario.md](history/es/diario.md).

## 2026-11-25

**Repository**
- The documentation moved to English. The Spanish originals are in `docs/history/es/`.
- A collaborator joins, working on a separate branch.

**Heads investigation: parked**
- 18 runs in total, heads in 3. It is all or nothing per run, and the game does not emit the head draws.
- Ruled out: camera distance (H1), skipping the cutscene (H3), the `fc5e66f8` skip (H4a). In-place reads gave faces in only 1 of 3 runs.
- Full record in [memory.md](memory.md) §7. To be re-checked after each phase.

**Phase 0 measurements**
- **Ceilings:** no copies gives 5.9 fps and 170 ms per frame (×2). Plain copies crash on a page with no backing.
- **The probe:**
  - private memory in place works on vkd3d-proton with RADV;
  - the page pool through sections does not;
  - reading in place over PCIe doubles GPU time.
- **Census:** 72 % of the bytes shaders read are in the page pool.
- **Baseline A:** 2.8 fps, 356 ms per frame, less than 1 % variation between three runs. Baseline B: 2.6 fps, 386 ms.
- All in [memory.md](memory.md) and [baseline.md](baseline.md).

**Phase 0 tooling**
- **`VICTORY_MEDIR=1`** turns on, in the release build:
  - the xmem remap counters;
  - `waitstat`, the title's waits by thread and caller;
  - the CSV columns `remap_ms`, `maps` and `unmaps`.
- **`VICTORY_COPY_MODE`** now works in any build (`medir::copy_mode`).
- **`scripts/bench.sh`** and the `royale` launcher profile, which builds `~/src/victory-royale` itself.
- **`warm --no-pipelines`:** 25,700 shaders translated, 58 refused, 101 s. The variant with pipelines hangs on RADV (seen with joshuu).

**Base**
- `main` is JoshuuDev/victory-amd `003beef`.
- victory-royale (ligmaSec) is kept as `royale-base`: merging it gave 494 conflicts in 63 files.
