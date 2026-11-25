# Journal

What was tried, what came out of it, and what was decided. Newest first. The Spanish journal before 25 Nov 2026 is in [history/es/diario.md](history/es/diario.md).

## 2026-11-25

**Phase 1.8 measured: fast reads work, −10 % frame time**

| Scene A | baseline | 1.7 (own write-back) | 1.8 (fast reads) |
|---|---|---|---|
| ms per frame | 354 | 348 | **317** (3.2 fps) |
| prepare | 124 | 129 | **102** |
| copies | 63 ms | 66 | **49** |
| GPU | 159 | 143 | 142 |
| copy-back | 67 ms | 71 | 69 |

- **17,190 reads of title memory per frame** (288 MB), each one a system call before. 20 per frame meet a missing or guard page and take the kernel path.
- No crash after the guard-page fix.
- **Copy-back did not move.** It writes with `NtWriteVirtualMemory` (`host.rs::write_mapped`), the same per-call cost in the other direction. **Next: fast writes**, the same technique.
- **Fast writes (1.9):** `write_mapped` tries the same routine first (`VICTORY_F1_FAST_WRITE=0` turns it off). Unit tests under wine:
  - a write to a missing page and to a read-only page fails to the kernel path;
  - **a fast write into write-watched memory is reported by `GetWriteWatch`**, so Wine tracks our writes like the title's and the journal stays correct.
- **First run with fast writes: device lost as loading ended.** A GPUVM fault at `0x80038cc00000` (a host GPU address, not title memory), TCP client, ring timeout. It happened where the forks' late-wait switches also faulted.
  - **Ruled out:** a write-watch difference. A test shows Wine's kernel write (`WriteProcessMemory`) also marks the page written.
  - The bytes written are the same either way. Leading hypothesis: the faster copy-back changes timing and exposes an existing GPU-side race (resource lifetime around loading).
- **Decision:** fast writes off by default (`VICTORY_F1_FAST_WRITE=1` turns them on); fast reads stay on. To settle it: two more runs with fast writes on. If they fault again at the same point, take a diagnostics build with `VICTORY_D3D12_DEBUG=1` to name the command.

**Phase 1.8: profile of the submitting thread, and fast reads (branch `p1.8-fast-reads`)**
- **Profile** (`bench.sh --perf`, scene A, 30 s). The submitting thread is busy about half the time (~170 ms per frame):

  | What | Share of the thread |
  |---|---|
  | `memcpy` / `memcmp` in ucrtbase (copies, and the `F1_OWN` compare) | ~29 % |
  | **Wine's `ReadProcessMemory` path:** `virtual_check_buffer_for_write` ~10 %, its libc copy ~10 %, syscall dispatch ~3 % | **~23 %** |
  | `RtlQueryPerformanceCounter` (clock reads) | **~8 %**, caller unknown (no call graphs) |
  | VIctory's own code: `watch::writes`, B-tree walks of the journal, blake3, merges | ~22 % |

  - Wineserver: 0.08 cores, no memory-read requests. Reads stay in-process.
  - Symbolising PE code: victory.exe was at `0x6ffffd670000` in that run; `llvm-symbolizer --obj=victory.exe` takes `0x140000000 + (address − base)`. Find the base by scoring candidate bases on plausible names.
- **`crate::fastcopy`:** title memory is read with `rep movsb` in a small assembly routine. A read of a missing page faults, and our vectored handler (first in the chain, ahead of the crash reporter) moves the thread to the routine's failure exit; `read()` then falls back to `ReadProcessMemory` as before. This is the kernels' technique for copying from user memory. `VICTORY_F1_FAST_READ=0` turns it off. A unit test (mapped page, reserved page, reuse after a fault) passes under wine.
- New CSV columns: `fast_reads`, `fast_mb`, `fast_faults`.
- **First run in the world: crash** `0x80000001` (`STATUS_GUARD_PAGE_VIOLATION`) at `victory_fast_copy_at`, reading `0x11000`. Some title range holds a guard page. That raises its own one-shot exception, not an access violation, and the fault clears the guard. **Fix:** the handler also takes guard-page faults at our instruction, puts `PAGE_GUARD` back on the page, and fails the copy to the kernel path. A second unit test covers it, and both pass under wine.
- **Next:** measure scene A. Then find who reads the clock so often.

**Phase 1.8 started: fast reads of title memory**
- **Why:** half the bytes, same copy time, so the cost is per read, not per byte. All reads of title memory go through `ReadProcessMemory` (`host.rs::read`), one call per piece, with fallbacks to 64 KiB and 4 KiB pieces and `VirtualQuery` when a page is missing.
- **Plan** (README 1.8): profile first, then plain `memcpy` for ranges known to be readable, then a fault safety net, then batching.
- First `--perf` run: **wineserver used 0.08 cores, with no memory-read request among its hot symbols**: title memory reads do not go through the server, so their cost is inside the game process. The game's own profile came out empty; perf errors now go to `perf.err`.
- `bench.sh run --perf` records the game's and wineserver's user-space profiles during the window.

**Phase 1: own write-back measured, both switches on by default**

| Scene A | baseline | `F1_OWN` | `F1_CARD_MERGE` | both (×2) |
|---|---|---|---|---|
| ms per frame | 354 | 344 | 365 | 348 |
| MB uploaded per frame | 216 | 108 | 230 | 110-113 |
| MB sent up to the card | 93 | 54 | 59 | 18 |
| hash passes | 558 | 536 | 144 | 102-104 |
| GPU ms | 159 | 149 | 155 | 143-144 |
| copy time | 63 ms | 64 | 67 | 66 |

- **About 100 MB a frame of uploads were our own write-back.** 95 % of those pages compare equal and are now skipped (`own_skip_mb` 100-104, `own_copy_mb` 5).
- The picture is unchanged, by the user's eye.
- **Copy time did not drop with half the bytes.** The cost is reading title memory, which the compare has to do too. Every read goes through the kernel (`ReadProcessMemory` / `NtReadVirtualMemory`) so that pages with no backing do not crash. This confirms design rule 1 of [memory.md](memory.md) by measurement.
- **Decision:** both switches are on by default (`=0` turns them off). Next: a fast path for reading title memory.

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
