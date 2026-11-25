# Journal

What was tried, what came out of it, and what was decided. Newest first. The Spanish journal before 25 Nov 2026 is in [history/es/diario.md](history/es/diario.md).

## 2026-11-25

**Phase 1: first real change (branch `p1.1-census`, both switches off by default)**
- **Finding from reading the code:** the copy-back writes what shaders wrote into title memory. Write watch, or the view guards, then report those pages as written by the title, so the next refresh copies them again. For copies in the card's memory, `card_merge` also marks the whole merged range to go back up to the card and be rehashed, although the card already holds those bytes. The code says so itself: *"Our write shows as a title write at the copy's next refresh, which copies those pages once more (some dozens of MB a frame)"*.
- **`VICTORY_F1_OWN=1`:** a copy remembers the pages it wrote back. At its next refresh it compares each such page with title memory (one read) instead of copying it again. If they are equal, nothing is copied or sent up. Correct by construction: a page the title wrote meanwhile differs and is copied as before. New CSV columns `own_skip_mb` and `own_copy_mb`.
- **`VICTORY_F1_CARD_MERGE=1`:**
  - after a merge, only the bytes where the merge kept the title's bytes go back up to the card;
  - the compare after the lists stores its hashes (`compare = 2`, as the relay already does), so pieces that were not sent up are not found differing again.
- Builds. The `shadow::` unit tests pass under wine. `watch::tests::writes_in_partly_committed_view_blocks_are_reported` fails only when run together with other tests (shared state); alone it passes, and the file was not touched.
- **To measure:** scene A with each switch and with both, against the baseline, checking the picture for corruption.

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
