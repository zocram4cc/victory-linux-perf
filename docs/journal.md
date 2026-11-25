# Journal

What was tried, what came out of it, and what was decided. Newest first. The Spanish journal before 25 Nov 2026 is in [history/es/diario.md](history/es/diario.md).

## 2026-11-25

**Our base is behind: fxceless moved a long way today**
- `RADV_DEBUG=allbos`: still lost 2 of 2, so not memory missing from a submission.
- Release with the skip fixed and `VICTORY_DRED_DEVICE=both`: 1 of 2 lost; the other loaded with the 3D picture blown out to white (HUD right; screenshot). That looks like auto-exposure fed a wrong value. Forced held submissions do not correlate with the losses.
- **fxceless/main has 15 commits past our base (joshuu `003beef`, which is fxceless + myokex + joshuu).** It now carries the myokex view guard and joshuu's DirectStorage fix itself. Among them:
  - `a26859f` *"The first room renders right: byte-copied descriptor records are mirrored"* (descriptor records the title copies as bytes were not mirrored: wrong data in slots);
  - `f85101b` a submit thread of the layer's own (`VICTORY_ASYNC_SUBMIT`) and a copy queue for fetches (`VICTORY_QUEUE2`), i.e. our phase 3;
  - `465b94f` / `3973179` **9.6-10.3 fps in the first rooms** on an RTX 4070.
- Upstream VIctory has 402 commits past our base: barriers, memory-value waits, texture work.
- **Consequence:** phases 1-3 duplicate work the forks are doing faster, on a base a day old. The descriptor fix may be exactly our wrong reads. Proposed: rebase onto fxceless/main, carry over the Linux-specific pieces (fast reads under Wine, the skip fix, `VICTORY_KEYS` in release, bench tooling), then test both scenes on Linux.

**Roadmap reordered: phase S (stability foundation) before phases 2-3**
- The user's point: the prologue is a closed space, and the game has to work anywhere. Every speed-up ran into the same correctness bug, and phase 2 rewrites the paths where it lives.
- Phase 1 stays (small, measured, harmless), except fast writes, which stay off.
- New phase S: wrong reads / device loss (Jason as reproducer), exact lifetimes, texture coherence, and an open-world reference scene. Exit: release loads Jason 5 of 5 with the right picture.

**Found: `VICTORY_SKIP_PIPELINES` only worked in diagnostics builds (fixed)**
- `pipeline::is_skipped` and the three checks in `draw.rs` were `cfg(feature = "diagnostics")`. **Every release run of ours, and of the launcher's fork profiles, never skipped `fc5e66f8f873d3bb`**, although the profile set it. Fixed: the skip works in every build. The 24 Nov finding that the GPU faulted ~9 minutes in without the skip came from release builds, where the skip did nothing.
- It does **not** fix the saved game's loss alone: 2 runs after the fix, one lost the device, one did not (it was still loading at 1,503 draws).
- Found by listing every diagnostics-only statement that does more than measure. The other diag-only paths are notes, traces and counters.
- Also: texture views from `make()` never reach past their host texture (0 in a run). `VKD3D_CONFIG=fault` gave no report and no loss in that run, but a white-and-black picture, like release with DRED.

**Device loss: the fault is not in a buffer copy; likely a texture read out of range**
- `VICTORY_PAD_KB=256` (256 KiB more at the end of every host buffer made in `host.rs`): still lost 2 of 2. The faults were at `0x8003bb7f0000` and `0x800410e00000`.
- **`VICTORY_ALLOC_DUMP=<file>`** writes, on device loss, every live host copy of a title buffer and every window: GPU address, bytes, title address, kind.
  - vkd3d's addresses are sign-extended (`0xffff8001…`); RADV prints the low 48 bits. Compare `addr & (2^48 − 1)`.
- One run: fault at `0x80035fac4000`. The 4,070 live copies (3,402 buffers, 658 windows, 10 card copies) span `0x800112200000`-`0x800332200000`. **The fault is ~740 MB above the highest, in no copy of ours.**
- The faulting client is TCP (texture unit), so it is most likely a **texture read**: images live elsewhere in the GPU's address space. Retired textures were kept alive in the leak runs, so not a freed texture.
- **Leading hypothesis:** a shader resource view that describes more mips or array slices than the host texture has (textures streamed in part, mips picked from the first one present). On the console those reads land in title memory; here they run past the image's allocation. Next: check every texture view made against its host texture and count the mismatches.

**Device loss: two more hypotheses ruled out (saved game, release build)**

| Switch | Lost | Furthest draws a frame |
|---|---|---|
| `VICTORY_LEAK_RETIRED=1`: nothing passed to `retire()` is dropped, nor texture staging (×3) | **3 of 3** | 12,246-14,399 |
| `VICTORY_SERIAL_OPS=all`: a global UAV barrier before every draw, dispatch and indirect command (×2) | **2 of 2** | 7,375-13,498 |

- Not a host object freed through `retire()` (shadows, texture copies, slot views, staging).
- Not a missing barrier between commands on the GPU.
- **Not caused by phase 1:** with `VICTORY_F1_OWN=0 VICTORY_F1_CARD_MERGE=0 VICTORY_F1_FAST_READ=0` the save still lost 2 of 2. The base has the bug.
- Not the copies in the card's memory: `VICTORY_CARD_BUFFERS=0` still lost 2 of 2.
- Not DirectStorage's parallel loading: `VICTORY_DSTORAGE_WORKERS=1` (one worker instead of 24) still lost 2 of 2.
- What did avoid it: **the CPU waiting for the GPU to go idle after each submission** (`RADV_DEBUG=hang`, `breadcrumbs_sync`), and the diag build with the runtime's DRED. So something the CPU side touches while the GPU still works. Candidates:
  - DirectStorage's worker threads (`victory:disk`) writing destinations during loading;
  - work vkd3d keeps in flight past our wait;
  - a GPU read out of range on data not complete yet.
- **Fault addresses so far:** `0x80038cc00000`, `0x80036a000000`, `0x8003b080e000`, `0x8002f0c73000`, `0x800335e04000`, `0x8003378e1000`, `0x80038cc00000`. Several sit exactly on 1-2 MB boundaries: a shader reading past the end of an allocation into unmapped address space.
  - Reads through descriptors are bounds-checked (D3D12 robustness), but raw addresses are not: root descriptors, and title memory read by address (`memory.rs`).
  - Whether a read past the end faults depends on what lies after the allocation, which changes with timing and allocation order. That would explain why DRED or the diag build hide it, and why the one release run that lived drew an almost white picture (garbage instead of a fault).
  - **Leading hypothesis now: an out-of-range raw read.**

**The saved game's device loss: what keeps it alive (all runs load the save, `royale-save`)**

| Build and switches | Device lost | Notes |
|---|---|---|
| release (×4, also with every sweep off) | **4 of 4** | ~45-60 s in, while loading the world |
| release, `VKD3D_CONFIG=…,breadcrumbs` | yes | vkd3d printed no breadcrumb report |
| release, `RADV_DEBUG=hang` | no in 108 s | killed from outside |
| release, `VKD3D_CONFIG=…,breadcrumbs,breadcrumbs_sync` | no | 0.2 fps, reached 11,878 draws a frame |
| release, `VICTORY_DRED_DEVICE=faults` or `=breadcrumbs` (DRED setting alone) | yes, both | |
| release, `VICTORY_DRED_DEVICE=both` | **1 of 2** | the run that lived: 2.9 fps, 13,405 draws, **picture almost white, Jason a shadow** (user) |
| diag, no DRED | yes | |
| diag, `VICTORY_D3D12_DRED=1` (×4, with or without our timestamps, markers or object names) | **0 of 4** | 2.3-2.7 fps, **picture right**: face, skin, water, clouds, minimap (screenshot and user) |
| diag, DRED without the runtime settings (`VICTORY_DRED_SETTINGS=0`) | yes | |

- Every release-only code path (`cfg(not(feature = "diagnostics"))`) was read: all equivalent (counters, captures, log text). The builds differ in timing, not in what they draw.
- **Conclusion:** the loss depends on timing, as with fast writes and the forks' late wait. What helps is slowing things down or serialising the GPU (`breadcrumbs_sync`, `RADV_DEBUG=hang`), plus the extra work of the diag build with the runtime's DRED on. It is the main blocker for any speed-up. Whether it is a missing GPU barrier or a use after free is not settled.
- The white picture in the release run that lived is a second symptom to look at once it stops crashing.
- **New switches:**
  - `VICTORY_DRED_DEVICE=breadcrumbs|faults|both` (any build);
  - `VICTORY_DRED_SETTINGS=0`, `VICTORY_DRED_NAMES=0`, `VICTORY_NOTE_STAMPS=0`, `VICTORY_NOTE_MARKERS=0` (diag).
- **Tooling:** `bench.sh` sees diag builds. In diag runs the frametime CSV is not where `--auto` looks; to fix.

**Unattended runs, and a reproducible device loss**
- **`bench.sh --auto` works:** it presses A at the start screen (`VICTORY_KEYS`, now in release builds), the intro plays through, the world is reached at ~105 s, and it measures with nobody at the keyboard. Current defaults: **327 ms per frame** (auto-keys run, ~5,080 draws, stable). Holding A does not skip the intro.
- **The user's saved game (Jason, in the world)**, installed in a separate launcher profile `royale-save` (own data folder, same build):
  - **loses the device while loading, 3 of 3** (GPUVM fault ~45 s in, then the title's own crash handler), with no fast writes and no texture freeing in the trail;
  - vkd3d's `breadcrumbs` gave no report in this GE build;
  - with `RADV_DEBUG=hang` (a wait after every submission) it **ran 108 s without a fault**, until something outside sent SIGTERM (another session was using the launcher at the same time).
  - The waits remove the loss, so this points to a race in timing again, not a broken command. It is a reliable test case for the lifetime work, in a heavier scene than the prologue.
- **Caution:** two sessions launching the game at once kill each other's wineserver. Coordinate before measuring.

**Freeing vs fast writes: three more runs**

| Run | Lost device | Picture (user) | ms |
|---|---|---|---|
| fast writes, `VICTORY_TEXTURE_FREE=0` (a) | no | lighting missing, **Lucia's orange shirt black** | 289 |
| fast writes, `VICTORY_TEXTURE_FREE=0` (b, moving) | no | textures more broken and flat | 327 |
| no fast writes, freeing aggressive (`IDLE_MS=500`, `IDLE_FRAMES=1`), breadcrumbs | **no** | — | 314 |

- **Freeing alone does not lose the device, even when aggressive.** The losses need fast writes **and** freeing together.
- **With fast writes and no freeing, the picture breaks.** With freeing on, it looked as usual: textures recreated from title memory come back right.
- Open question: does the picture break because freeing is off (textures go stale, and freeing was hiding it), or because fast writes write something wrong? To settle it: `VICTORY_TEXTURE_FREE=0` **without** fast writes.
- Note: the sweep's trail lines reach the log only when the device is lost, so a run without a loss does not show what was freed.

**Correction: `VICTORY_TEXTURE_SWEEP=0` turns off two things**
- It stops `resource::let_go_idle` (freeing host texture copies), **and** `HostHeap::texture_sweep`, a repair pass. That pass finds descriptor slots left showing nothing (a texture a shader would read as black) and puts them back in the upload queue.
- So the missing lighting, window and models in the sweep-off runs were most likely the repair pass being off, **not** stale textures. "Bug 2" as written above is unproven.
- What texture.rs does say about staleness: *"Only textures read through descriptor tables are brought up to date (directly indexed views are not known per draw)"*. Textures picked by index get rechecks at 4-400 frames after upload (joshuu, `VICTORY_HEAP_RECHECK`), plus remap notices.
- **New switches:**
  - `VICTORY_TEXTURE_FREE=0` keeps every host copy alive and leaves the repair pass on;
  - `VICTORY_TEXTURE_IDLE_MS` and `VICTORY_TEXTURE_IDLE_FRAMES` (5000 and 4) make the freeing aggressive, to reproduce the lifetime bug on purpose.
- The `crumbs-a` run (fast writes plus vkd3d breadcrumbs) got through without a loss, so there is nothing to read yet.

**Sweep test: two texture bugs, not one**

| Run (fast writes on) | ms per frame | Device | Picture |
|---|---|---|---|
| sweep on (3 runs) | 308 (when it got through) | **lost 2 of 3** | — |
| `VICTORY_TEXTURE_SWEEP=0` a | 307 | ok | — |
| `VICTORY_TEXTURE_SWEEP=0` b | 303 | ok | **lighting missing** |
| `VICTORY_TEXTURE_SWEEP=0` c | **294** (3.4 fps) | ok | **lighting, a window and many models missing** |

- **Same draws in every run** (~4,980 per frame, ~340 dropped), so the game emits the same work. What is missing is wrong **data** (stale textures or buffers), not missing draws.
- **Bug 1 (lifetime):** with the sweep on, a faster frame lets it free host texture copies that the GPU still reads. With the sweep off, no device loss in 3 runs. Every slot path I read (`HostHeap::keep`, `mirror`, `make_waiting`) holds its texture, so the unheld reference is somewhere else. Next: let the GPU name it (`VKD3D_CONFIG=descriptor_heap,breadcrumbs`, and the trail).
- **Bug 2 (staleness):** with the sweep off, textures go stale and stay so. The sweep was hiding it: copies it frees are made again later from title memory, fresh. So change detection for textures misses some updates. This is a phase 1-2 correctness problem (`texture.rs`, `VICTORY_HEAP_RECHECK`), and probably related to the flicker and missing pieces seen before.
- Fast reads, own write-back and card merge stay on. Fast writes stay off until bug 1 is fixed.

**Fast writes: 2 device losses in 3 runs; a lead in the texture sweep**
- `VICTORY_F1_FAST_WRITE=1`, three runs:
  - **lost the device twice**, as loading ended and a few seconds into the first cutscene (GPUVM faults on host GPU addresses, TCP client: a texture read);
  - **when it gets through, copy-back falls from 69 to 53 ms** (−23 %) and the frame from 317 to 308 ms.
  - Without fast writes: no device loss in ~25 runs.
- **The crash log's trail:** 1.4 ms before the loss, the sweep (`resource::let_go_idle`) let go of **2,196 host copies of textures the title fills (1.58 GB)**, *"each held by its title texture alone (no list, no slot)"*. The fork's late-wait crash of 24 Nov also had a sweep letting go a few ms before.
- **Why speed would expose it:**
  - a texture copy is let go after 5 s **and** 4 frames without use;
  - during loading, frames take seconds, so a faster copy-back reaches the 4 frames sooner;
  - textures loaded long before become eligible right as the cutscene starts using them.
  - In `Host::submit`, `sweep()` runs before `descriptor::make_waiting()` makes the slots the title wrote since the last submission. A texture a pending slot is about to show may still count as held by no slot.
- **Decisive test:** `VICTORY_TEXTURE_SWEEP=0` with fast writes on. If the losses stop, fix the sweep (ordering, or count pending slots as holders) instead of hiding it. This lifetime bug would also hit phase 3.

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
