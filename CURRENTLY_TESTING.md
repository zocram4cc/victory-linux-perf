# CURRENTLY TESTING

Status of the switches on `main`, and every frame time recorded so far on the test machine.
**Work in progress: numbers move as runs land. Read "1 run" figures as ±5 ms** (Series S runs are
bimodal; differences under ~5 ms need ≥4 runs an arm).

## Test machine and method

- Ryzen 7 5800X3D, Radeon RX 6900 XT 16 GB, Linux 7.3, Mesa RADV 26.3, Proton Experimental
  (11.0-20261112, vkd3d-proton 3.2.0) with the view-tree wineserver, Series S mode (`CONSOLE=series-s`,
  720p + FSR). Board MSI B450M MORTAR MAX: the card links at **PCIe 3.0 x16 (8 GT/s)**, not 4.0 (AMD's
  400-series has no Gen4), so anything the card reads from system memory crosses ~16 GB/s a direction.
- Scripted runs: keys to the world, then 150 frame-time rows (one a second) of the world (`draws > 100`).
  Figure = median `ms_per_frame` over world rows 15–89 (the steady phase); "prep" = median `cpu_prepare_ms`;
  "submit thread" = median of the `queue::behind` line's "that thread worked N ms". Arms are interleaved
  pairs in one batch, base first; identical runs agree within ~1–2 ms; cross-batch numbers are not comparable.
- **Since 2026-11-28 13:00 every scripted run restores the save from `~/7952987A - 70 percent.zip`**
  (`scripted-i19.sh`; `SAVE_ZIP=` empty turns it off). Before that, runs carried the save on and the scene
  drifted between arms (draws 4.1k–5.9k). Restored runs start alike: **~8.5–8.7k draws a frame, 47–51 ms**.
  Only compare numbers from restored runs with each other. A scripted run wipes play progress on that save.
- **`gpu_pct` / "card % busy" overstates the card**: it counts the time any submission of ours is in flight,
  hand-off included. The card's own timestamps (`VICTORY_CARD_SPANS=1`, diag) give the real figure (below).
- Release build (`--features scripted`), caches warm (prebuild + full `warm`, 93,821 pipelines).

## Latest (2026-11-28 day): `main` = e352af4a8

**Play set now: `play-fast9.sh`** = `play-fast8.sh`'s settings on a **thin-LTO + x86-64-v3 build**
(`bin/main-f6`, `main` e352af4a8) and the Proton copy **`Proton-Experimental-viewtree-homebo-ww`**, whose
hand-built `ntdll.so` runs `NtGetWriteWatch`'s kernel scan (`PAGEMAP_SCAN`) after Wine's `virtual_mutex`
is left instead of under it (patch and build script in the lab notes `wine-writewatch/`; the same idea was
found independently in Some1sm/clankerVI). `VICTORY_SKIP_MOVIES=end` is no longer set (movies play). Its
prefix is a symlink to the play prefix (saves shared). `DIAG=1` uses `bin/main-f6/victory-diag.exe`.
Rollback: `play-fast8.sh`.

### Where the frame goes now (restored save, ~8.6k draws, Series S)

The frame is set by one thread, the submitting thread (`victory-submit`): it works **36–39 ms of a
47–51 ms frame** (ab42). The card is **not** on the critical path:

| Measure (card timestamps, `VICTORY_CARD_SPANS=1`, diag) | Per frame |
|---|---|
| Card actually working | **27.5–28.8 ms** over ~16 submissions (48 %; the old counter said 58–80 %) |
| Card idle waiting on the submitting thread | ~32 ms |
| Submitting thread waiting on the card | 0.7 ms |
| Our own GPU work (compare lists; send/hash lists) | 1.98 ms; 0.05 ms (≈7 % of card time, + ≤1.3 ms alias copies inside title lists) |
| The title's own GPU work | ~25 ms; flat: the largest pipeline 0.54 ms, the top ten 3.8 ms |

| Submitting thread, by stage (medians, diag) | ms a frame |
|---|---|
| **Refreshes** (copy and texture refresh) | **17.3** |
| Write-watch asks | 5.5 |
| Copies and fills | 4.8 |
| Card preparation before sending (`card_before`) | 2.2 |
| Lock waits | 2.1 |
| Card-step planning | 0.9 |

Copy-refresh time by memory (diag, `zc-t5`): views of the title's physical pages 81 %, the second
graphics pool 15 %, the rest 4 %. Only 0.4 MB of the ~30 MB refreshed a frame actually changes.

### Measured today

| Change | Frame (ms) | Prep / submit thread | Notes |
|---|---|---|---|
| **thin LTO + x86-64-v3 build + Wine `NtGetWriteWatch` outside the lock** (= `play-fast9`) | **40.7 / 40.0 vs 43.2 / 41.9** | prep 29.6 / 28.9 vs 33.0 / 32.2 | ab40, lighter drifted scene (~5.6k draws): −2.2 ms; AVX2 instructions in the exe 3,263 → 40,355 |
| same, heavier scene (70 % save, ~5.9k draws, 1 valid pair) | 41.8 vs 41.7 / 42.4 | prep 26.3 vs 29.1 / 29.5; submit thread 34.1 vs 36.4 / 36.4 | ab41; frame flat, CPU work down; M2 drifted to 4.5k draws (excluded) |
| fat LTO + one codegen unit (dfmbrollins' profile) vs thin | 49.6 / 47.6 vs 50.1 / 46.8 | prep 34.0 / 32.7 vs 35.1 / 32.8 | ab42, restored save (8.5–8.7k draws): no gain; tails vary more by run than by build (worst-frame p95 183 / 233 vs 258 / 116); thin kept (build 35 s vs 182 s) |
| `VICTORY_ZERO_COPY=1270-1290 VICTORY_ZERO_COPY_TRACK=1` (pool 2: copies kept on the card, writes found by our own userfaultfd; `perf/zero-copy`) on `play-fast8` | 40.5 vs 41.5; 38.9 vs 40.2 | prep −2.3 / −2.5 | zc-ab2/ab3; −1.0…−1.3 ms; Wine asks 270 → 34 a frame; picture checked |
| same on `play-fast9` | 39.4 / 37.6 / 39.7 vs 38.7 / 38.4 / 38.8; 39.0 / 39.0 / 38.7 vs 40.0 / 38.4 / 38.2 | prep −0.7 | zc-ab5/ab6 (drifted saves): flat; the Wine patch already removed what it saved; off |
| `VICTORY_ZERO_COPY` on pool 2 read **in place** by the card | +12.5 | prep −10, GPU wait 1.6 → 25 ms | zc-ab1; PCIe 3.0 ceiling; pool 1 in place: noise |
| `VICTORY_TRACK_VIEWS=1` (view memory tracked by userfaultfd instead of guards) | 220.9 vs 54.5 (diag) | copies 236 vs 20 | one kernel scan per check (~3,600 a frame, mostly clean) vs the guards' bits; off |
| crashbashash's sync guards (`VICTORY_SYNC_GUARDS=1` + Proton `-homebo-ww-uffd` with his Wine patch 1860c50) | 50.2 / 49.4 / 51.4 vs 50.8 / 49.7 / 48.4 | prep +3.6…+6; copies +3…+5 | zc-ab7, restored save: title threads lose their fault cost, the submitting thread gains bookkeeping + a syscall per arm/disarm; off |
| refused-ask cache (dfmbrollins, `VICTORY_REFUSED_CACHE=1`) | — | ~0.1 ms | refusals are 3–10 a frame now (were ~240 before 11-26); off |

- Wine microbenchmark (`GetWriteWatch` latency while 4 threads storm the lock): with others asking too,
  p99 124–175 µs → 4–6 µs and 5–6× the throughput; with others taking guard faults the asking thread
  itself got slower (median 4 → 12–51 µs). In-game the combination above was a net gain.
- Avenues checked and not applicable here: `VICTORY_KEEP_SNAPSHOTS_MB=1024` (never refused),
  Marvisak 19533398f (clean copies already skipped), `VICTORY_POOL_THREADS` (4 helpers), `MERGE_GROUPS`
  (already one call a submission), Marvisak 2a4938e66 (0.4 ms of sparse reads).
- Quiesce before free (from clankerVI 949b4625b): every flight settles before a zero-copy region is freed
  (`perf/zero-copy` 7b624d091).

### Frame generation and FSR 4 (`perf/framegen`, not merged)

- **White screen with AMD frame generation: vkd3d-proton 3.2.0's `VK_EXT_present_timing`.** Presenting
  through AMD's frame-generation swap chain with it on shows white; our back buffer is correct in every
  white run. With `VKD3D_DISABLE_EXTENSIONS=VK_EXT_present_timing` the scene shows, generation off and on
  (e.g. 15.4 real + 9.6 generated = 26.9 fps shown, diag). 6697719ed sets it in-process when
  `VICTORY_FRAMEGEN=1` (before the device is made; `_putenv_s`). In-game check of the in-process path pending.
- The HUD's first line and the window title name the upscaler and real/shown rates
  (`24.6 FPS real / 49.2 shown  FSR 3.1.6+FG`); the frame-time CSV has an `upscaler` column.
- **FSR 4 cannot run on this stack:** the SDK 2.4 loader offers only FSR 3.1.6 and 2.3.4 upscalers and FG
  3.1.6 on this card under Proton (device reports Shader Model 6.8). FSR FG 4.x needs RX 9000 by AMD's docs.
- Known gap: the camera data given to frame-generation prepare is wrong (near 10000 / far 0.15 swapped,
  zero camera vectors).

### Fixes waiting for a re-warm (`fix/rdna2-picture`, not merged; translator version 7 → 8)

Caullen Omdahl's legacy multiply-add `-0` fix (Vice-Tay #308), Yaekai's BDD memo cap, Marvis' probe
readback reuse (diag), ruvenesh's `VICTORY_NO_WATCHDOG` hook (off; targets this dump's watchdog stop at
`image+0x2215c35`; hung instead of stopping in 3 of 6 runs on a 6900 XT in its PR thread). Five more
fixes from that list were already on `main`.

### In progress

- clankerVI wave 24a (`send_copied` tests a copy's range before upgrading it; our `send_copied` walked all
  ~3,500 carded copies a call) + 24b + 29c on `perf/clanker-waves`: A/B `cw-ab1` running.
- Submitting-thread breakdown and the other upstream leads (`perf/submit-chain`), card-side merges and
  Series X GPU levers (`perf/gpu-load`), batching the sync guards' arm/disarm (`perf/zero-copy`).

## Latest (2026-11-28 night): `main` = 123ea30a0

**Play set now: `play-fast8.sh`** (exe `bin/main-f5` = `main` 6092804c5 code, on the `-homebo` Proton; saves
shared with the play prefix; rollback `play-fast7.sh`): the play-fast settings below + combined +
`VICTORY_SKIP_WC_BACK=1 VICTORY_AMBIENT_MAGENTA=1` + the landings and fast track 1 switches +
`VICTORY_HOME_BO=1 VICTORY_ASK_SPREAD=1 VICTORY_WATCH_SPANS=1 VICTORY_EARLY_TEXTURES_MS=40
VICTORY_BAKE_TEXTURES_MS=10000` + new tonight: `VICTORY_SHADER_OPT=63 VICTORY_WINDOWS_BAR_MB=4096
VICTORY_MAPPED_CACHE=1 VICTORY_STORE_BESIDE=1 VICTORY_COPY_BESIDE=1`. `VICTORY_LATE_WAIT=default` in the set
is unset by `run-i19.sh` (stage 3 with the relay).

**Start of the night against now, one batch, 3 interleaved pairs (ab34, rows 15–89):** 48.2 / 48.1 / 47.7 ms
(play set of 00:53: `bin/main-homebo`, `SHADER_OPT=0`, none of tonight's switches) against **40.5 / 41.6 /
40.1 ms** (`play-fast8`): **−7.3 ms a frame (−15 %), 20.8 → 24.6 fps**; prep 40.0 → 33.1 ms, heap pass
16.5 → 9.6 ms, 8.40 → 7.23 ms per 1,000 draws. No exceptions, warm caches (0 misses) in all six runs.

Same method; old save; play set + `VICTORY_SHADER_OPT=63 VICTORY_WINDOWS_BAR_MB=4096` (both new in
play). Single 150-row runs moved by up to 3.9 ms between identical setups (ab25: 47.4 vs 43.5). Two
causes: the title's idle camera swings in after ~90 world seconds (draws ~2x, +~10 ms a frame) at a
slightly different time each run, and the time of day follows the wall clock (night at 04:08, day at
04:50 from the same save). From ab26 on: medians over world rows 15–89 (the steady phase), pairs
interleaved in one batch, plus ms per 1,000 draws; identical runs then agree within 1–2 ms.

| Change | Frame | Prep | Heap | Notes |
|---|---|---|---|---|
| **`VICTORY_STORE_BESIDE=1 VICTORY_COPY_BESIDE=1`** (on `main`, off by default; kroeeee sets STORE) | **39.8 / 39.1 vs 42.5 / 42.9** | 33.0 / 31.9 vs 36.2 | 10.2 / 9.6 | ab29, rows 15–89, 2 pairs: −3.2 ms (−7.5 %); STORE alone 41.1 / 40.5; picture checked; now in play (`play-fast8.sh`) |
| `VICTORY_WIDE_SPIN=50` | 42.4 / 42.3 vs 41.5 / 42.5 | | | ab26; no effect; off |
| nice −10 on the submitting, relay and wide threads | 42.3 / 42.8 vs 42.6 / 43.0 | | | ab27; no effect |
| `VICTORY_REFRESH_PREFETCH=1` (branch `perf/refresh-prefetch`, not merged) | 64.5 / 64.2 vs 45.0 / 45.9 | | | ab28; +19 ms: the prefetch ran on the list threads before a list was handed over |
| `VICTORY_REFRESH_PREFETCH=2` (a helper thread, the submitting thread never waits; not merged) | 42.8 / 44.3 vs 40.3 / 40.9 | 36.2 / 33.9 vs 32.4 / 32.0 | | ab31; +3 ms: its own write-watch asks and memory traffic cost more than the earlier refresh saves |
| `VICTORY_ASK_NORESET=1`, `VICTORY_PLACED_CONSTANTS=1`, `VICTORY_PICKED_WRITES=1`, `VICTORY_UNMAP_RUNS=1` | +1.2 / +2.1 / +1.0 / 40.9 and a stop | | | ab30 vs 39.5 / 40.4; all off |
| kroeeee's measured Linux defaults we lacked: `VICTORY_COPY_COMPARE=1`, `VICTORY_FRAME_INTERVAL_MS=0`, `VICTORY_PORT_DESCRIPTORS=1` | 42.9 / 41.8–42.0 / 41.1–42.2 vs 40.3 / 40.9 | | | ab31; no consistent effect; off |
| upstream fixes: `..` stops at a mounted drive's root (cwWwWwW), TLS callback lists over the limit rejected (pwu2775-dev) | | | | correctness; ported tests red without the fixes |
| PR #2 (GiovanniFadanelli): exact raw twins only under Wine | | | | native D3D12 removed the device at the first twin off 16 bytes; Wine/Proton unchanged (`on_wine()`); merged 4aaf2ff01 |
| PR #3 (GiovanniFadanelli): `VICTORY_LATE_WAIT=default` means the default | | | | was off when passed to the exe directly (Windows: 70.0 → 43.4 ms); our `run-i19.sh` already unset it; merged 123ea30a0 |
| `VICTORY_ASK_TEXTURES=1` | 44.4 / 44.2 / 42.4 vs 44.4 / 43.8 / 45.8 | | | ab33; texture asks 47.8 → 0.9 a frame, frame no consistent effect; off |
| `VICTORY_UP_QUEUE=1`, `VICTORY_JOIN_BARRIERS=1`, `VICTORY_AHEAD_GAP_MB=64` (kroeeee, branch `perf/ahead-gap`) | 43.6 / 40.7, 41.8 / 41.5, 42.7 / 45.7 vs 42.6 / 42.1 | | | ab32; no consistent effect; off (`VICTORY_LANE_CORE=1` did not act on this CPU) |

| Change | Frame | Prep | Heap | Notes |
|---|---|---|---|---|
| `VICTORY_SHADER_OPT=63` (was forced 0 by `run-i19.sh`) | 48.3 vs 49.0 | 39.9 vs 39.8 | | ab20; card as busy (RADV's compiler already cleans up) |
| + `VICTORY_WINDOWS_BAR_MB=4096` | **45.9** vs 48.3 | 38.9 | | ab20; card 77 → 68 % busy, worst p95 199 → 133 |
| submit-thread Vecs kept/pre-sized (d08f825a3, no switch) | 44.9 vs 43.7 | 37.0 vs 35.8 | | ab21; result-identical |
| list-thread hashing and resource borrows (18749873f, af023c26f, no switch) | 43.5 vs 43.7 | 36.7 | | ab21; result-identical |
| `VICTORY_MAPPED_CACHE=1` (506a3bc75, off by default) | 43.8 vs 44.4 | 37.2 | **11.0 vs 15.5** | ab22; heap pass −29 % |
| fast-4 (all four) + `MAPPED_CACHE=1` | 43.9 vs 43.5 / 47.4 | 34.7 | 10.9 | ab25 arm F; p90 53.8 |
| `VICTORY_ASK_EARLY=2`, `VICTORY_GUARD_HOT=200`, both | 47.8 / 46.8 / 45.8 | | | ab25; no effect seen; off |
| spin-before-sleep shim on Wine's `virtual_mutex` (LD_PRELOAD, not merged) | 45.8 vs 44.9 (2 pairs) | | 11.8 vs 15.6 | ab24; heap faster, frame not |

- Where the frame goes (diag timeline, 15 frames): the card runs 18 ms a frame and idles 19 ms between
  its ~13 submissions, waiting on the submitting thread's serial preparation; half of that idle is
  `refreshes ahead of writes` (notes: `submit-overlap.md`). Wine's `virtual_mutex` is held 35 % of the
  time but waited on 18 s per 10 s (55k acquisitions/s: the heap pass's working-set queries, guard
  faults' protection changes, write-watch asks); relieving it speeds the heap pass, not the frame.
- Kernel write watches (userfaultfd + PAGEMAP_SCAN) and ntsync are active in this Proton.

## Latest (2026-11-27 night): `main` = 505d89688

Each step merged into `main` after an A/B against the step before (single runs, 150–200 world rows,
same play-fast set; caches prebuilt and warm, no builds during runs).

| Step | Adds | Frame | Prep | Copies | Copy-back | Texture refresh | Notes |
|---|---|---|---|---|---|---|---|
| combined (below) | r3 + `COMPUTE_WAIT_MS=500` | 48.7 ms | | 20.7 | 21.7 | 16.3 | ab7 arm A |
| integration 1 | rounds r9–r19 (coolfox, LibsMQG, breakwater, Valhalla, translator v7, guards, remap counters, in-place S1) | 54.3 ms | | | | 16.9 | plain, new switches off |
| + `VICTORY_SKIP_WC_BACK=1` | LibsMQG: copy-back skips card pieces | **47.8 ms** | | | 18.4 | | 47.4 in a second run |
| + landings (r20) | `VICTORY_LAND_RING=1 VICTORY_KEEP_SNAPSHOTS=1 VICTORY_BURY_ASIDE=1 VICTORY_WOKEN_WAITS=1` (coolfox, rx6600) | **46.4 ms** | 36.7 | 19.2 | **5.6** | 14.8 | copy-back −2/3 |
| fast track 1 | `VICTORY_REFRESH_RANGES=1 VICTORY_SEND_CHANGED=1 VICTORY_READBACK_POOL=1 VICTORY_COMPARE_SHARE=1 VICTORY_DIRTY_QUEUE=1` (AntoninMichelq, Belobug) | 50.5 vs 50.2 | 37.0 vs 40.2 | 20.0 vs 22.1 | 5.8 | 14.0 vs 15.9 | ab10, 200 rows; p90 64.3 vs 66.3, worst p95 203 vs 223 |
| + Wine `memcmp` | DoubleGhosty d78b9eb5b: 16 bytes a step under Wine (no switch, same results) | 50.1 vs 50.3 | 37.7 vs 38.3 | 21.0 vs 22.0 | 5.7 | | prof2; p90 63.6 vs 67.9 |
| fast track 2 (switches off) | heap round, journal round, guard round: `PLAN_REUSE`, `UPLOAD_BARRIERS`, `JOURNAL_SHARED=2`, `HEAP_RECHECK_PERSISTENT`, `EXACT_NO_WAIT` | 48.3 vs 47.1 (all on) | 38.0 vs 35.8 | 20.5 vs 19.2 | 5.4 | 12.7 vs 13.1 | ab11: no gain, worse tail (p95 210 vs 152); merged with the switches off |
| + alias hold fix | `alias_of` released a reference it never took when the texture was being released (`then_some` builds its argument eagerly) | | | | | | crash fix, no switch |
| + `VICTORY_HOME_BO=1` | port 1's home heap is a memfd the card opens by a dma-buf handle (needs patched vkd3d-proton + win32u: the `-homebo` Proton and prefix) | 51.6 vs 54.0 | 38.8 vs 40.2 | 20.5 vs 21.2 | | | ab14; userptr walks per 10 s: 0 vs ~17,500; worst p95 171 vs 205 |
| + `VICTORY_ASK_SPREAD=1` | existing switch, was off | 52.3 vs 54.0 | 41.5 vs 40.2 | 18.9 vs 21.2 | | | ab14; p90 65.4 vs 71.5 |
| + Vice-Tay `VICTORY_WATCH_SPANS=1` (merged, off) | write-watch shards walked without range lists | 46.7 vs 48.0 | 38.9 vs 39.2 | 19.3 vs 19.8 | 5.0 | | ab15; tail worse (p90 65.3 vs 61.3, worst p95 202 vs 153): not in the play set yet |

- Run to run, one setup moves by about 4 ms (same exe and switches: 46.4 ms at 19:55, 50.2 at 20:24).
- Fast track 1: equal frame, 3 ms less CPU prep, steadier tail. Prep (37 ms) is now below the frame
  (50 ms) and the card is 80 % busy: what sets the frame now is not our copy work alone (profile next).
- Where prep goes now: copies 20 ms (38 MB a frame, so the cost is finding and reading what changed,
  not moving bytes) + heap pass 15 ms (textures picked by index, on the submitting thread; the
  buffer half already runs beside it, `VICTORY_HEAP_PASS_ASIDE` on by default) + textures 1.4 ms.
- Picture: violet blinds and trees fixed by `VICTORY_AMBIENT_MAGENTA=1` (Belobug's probe-light filter);
  integration 1 with it: same scenes as before, hair complete, no new artifacts (captures 1400–3000).
- Kept off: `VICTORY_HOME_NOHUGE=1` (+19 ms: 4 KiB pages on port 1's heap), `VICTORY_INPLACE=1` (+200 ms
  texture refresh, until in-place track A), THP collapse (median −1.8 ms, worse tail).
- d3dcompiler_47 10.0.26100.7705 against our 10.0.22621.608: 49.3 vs 47.4 ms (1 run each), no
  compile messages either way; kept 22621.
- Whole-process profile (prof2-P, 10 s, all threads): ~11 of 16 hardware threads busy. The 8
  list-recording threads take ~4.7 cores (a fifth of it in d3d12core and RADV, much of the rest our
  locks and descriptor writes); the submitting thread ~0.46 core (write-watch asks: kernel page-map
  scans and TLB-flush interrupts); vkd3d's queue thread 41 % in the kernel's `hmm_vma_walk_pmd`; the
  title's main thread ~0.65 core spinning in `Sleep(0)`. Wine's `memcpy` is not a cost (1.4 %).
- vkd3d's queue thread (bpftrace, hmm1/hh1/ab11): amdgpu walks the page tables of the one userptr
  buffer, port 1's 185 MB home heap, at EVERY submission: ~17,500 walks in 10 s (~90 a frame),
  64–512 µs each, 1.7–4.2 s of every 10 s; never caused by invalidation. `VICTORY_HOME_HUGE=1`
  (huge pages on that heap, branch `perf/home-huge`): the advice takes (180 of 185 MB huge) but no
  gain (49.7 vs 48.4 ms): the walk fills a 4 KiB-granular page array whatever the page size. The
  lever is fewer submissions a frame.
- `VICTORY_HEAP_WORKER=1` (fxceless #1007, branch `perf/heap-worker`): heap time on the submitting
  thread 16.1 → 0.0 ms, but frame 46.4 vs 46.3 and a worse tail (p95 214 vs 134): each submission
  waits for the worker's pass, so it is still in series. Its first run crashed at start-up (the
  `then_some` bug above, hit at once by the worker). Off until the pass can begin at present.

Play set then (superseded by `play-fast8.sh`, see the top section): the play-fast settings below + combined + `VICTORY_SKIP_WC_BACK=1
VICTORY_AMBIENT_MAGENTA=1` + the landings and fast track 1 switches + `VICTORY_HOME_BO=1
VICTORY_ASK_SPREAD=1` (`play-fast6.sh`, on the `-homebo` Proton; saves shared with the play prefix).
List-recording threads: 8 (default) stays; 12 and 14 raised the heap pass to 19-20 ms (ab14).

## Ladder on `main` (2026-11-27, cumulative: each rung adds to the one above)

| Rung | Adds | Run 1 | Run 2 | Mean | Prep | GPU busy |
|---|---|---|---|---|---|---|
| r0 | every new switch off (play-fast settings) | 80.9 | 74.5 | 77.7 ms | 70.7 / 63.7 | 60 / 58 % |
| r1 | `VICTORY_CARD_BUFFERS_MIN_KB=64` | 70.0 | 64.0 | 67.0 ms | 57.7 / 52.4 | 64 / 61 % |
| r2 | + `VICTORY_WINE_MAP=1 VICTORY_PORT_CONSTANTS=1 VICTORY_PORT_HOME=1` (renderer port 1 under Wine) | 53.9 | 52.2 | 53.1 ms | 43.5 / 43.7 | 77 / 76 % |
| r3 | + `VICTORY_DYNAMIC_BITS=1 VICTORY_TABLE_MEMO=1` | 50.1 | 49.2 | **49.7 ms** | 40.4 / 41.3 | 79 / 78 % |
| r4 | + `VICTORY_GRAPHICS_WATCH=1 VICTORY_SMALL_ALIGN=1 VICTORY_PIPELINE_WATCH=1` | 51.0 | 52.6 | 51.8 ms | 42.1 / 42.4 | 76 / 76 % |

p90 of the frame: r0 97.5/92.5, r1 88.0/88.9, r2 103.4/69.9, r3 67.2/63.9, r4 66.7/68.2 ms.
**Best so far: r3, 49.7 ms (−36 % against r0).** r4's three switches gave nothing here (+2 ms, within
noise): left off for now.

r2: port 1 opened part of the title's constants in place and fell back to copies for stretches
that are not one section ("title memory not in place, a copy as before"). Picture check (captures
of world frames 1400/1500, r0 against r3): the same image.

## On top of r3 (one switch at a time, 2 runs each)

| Added to r3 | Run 1 | Run 2 | Mean |
|---|---|---|---|
| `VICTORY_CLAMP_SYNC=1` | 49.0 | 49.0 | 49.0 ms |
| `VICTORY_COMPUTE_WAIT_MS=500` | 47.9 | 48.5 | **48.2 ms** |
| `VICTORY_WAKE_ONE=1` | 51.9 | 48.2 | 50.0 ms |
| `VICTORY_JOIN_PARTS_DRAWS=500` | 50.4 | 49.1 (rerun: the first title stopped itself at start) | 49.8 ms |
| `VICTORY_CARD_BUFFERS_MB=4096` | 49.5 | 50.2 | 49.8 ms |
| **all on** (r4 + the five above) | 51.2 | 49.2 | 50.2 ms |

Only `COMPUTE_WAIT_MS=500` gains (−1.5 ms, both runs; upstream also has it put missing heads
right). The rest are within noise, and all of them at once gain nothing over r3.

## Combined (play-fast2's defaults now)

`VICTORY_CARD_BUFFERS_MIN_KB=64 VICTORY_WINE_MAP=1 VICTORY_PORT_CONSTANTS=1 VICTORY_PORT_HOME=1
VICTORY_DYNAMIC_BITS=1 VICTORY_TABLE_MEMO=1 VICTORY_COMPUTE_WAIT_MS=500` on top of the play-fast
settings: **48.2 ms** against 77.7 with every new switch off (−38 %).

### Series X reference (same combined set, `CONSOLE` unset: Series X mode, the title's own upscaler)

| Run | Frame (median) | p90 | Prep | GPU busy |
|---|---|---|---|---|
| sx-comb-1 | 61.7 ms | 86.9 | 44.8 | 84 % |
| sx-comb-2 | 63.2 ms | 83.4 | 46.1 | 83 % |
| **mean** | **62.5 ms** | | | |

Earlier Series X runs here, before these switches: 85–88 ms. Series X is close to card-bound
(83–84 % busy) where Series S is not (78 %).

## Earlier measurements on this machine (300 world seconds, 1 run each, `perf/v7900-final`)

All with Series S, `VICTORY_PORT_CBV=1`, `VICTORY_MAP_LOCK_FINE=1`, `VICTORY_SKIP_MOVIES=end`:

| Knobs | Median | p10 | p90 | Prep |
|---|---|---|---|---|
| none | 73.3 | 61.6 | 101.3 | 64.5 |
| `PORT_GUARD` | 74.8 | 62.7 | 96.8 | 66.1 |
| `PORT_JOURNAL` | 70.9 | 58.9 | 102.6 | 62.6 |
| `PORT_GUARD` + `PORT_JOURNAL` | 74.4 | 63.0 | 101.4 | 63.8 |
| `PARTIAL_DELTA` | 77.2 | 63.6 | 97.4 | 67.9 |
| `REFRESH_ONCE` | 74.5 | 62.0 | 100.0 | 66.5 |
| all four | **69.8** | 57.6 | 95.6 | 60.1 |

`VICTORY_MAP_LOCK_FINE=1`: 77.7 vs 83.5 ms, slowest frame 97 vs 106 ms (steadier). `WINE_CPU_TOPOLOGY`
in Windows sibling order: −3 %. Measured flat or worse (not retried without a new reason):
`TITLE_PRIORITIES=1`, `ASK_TEXTURES=1`, `GUARD_RUNS=1`, `COPY_BESIDE=1`, `SUBMIT_FRAMES=2`, guard shards.

## Switches new on `main` (rounds 1–7 merge)

| Switch | What | Code default | Tested here |
|---|---|---|---|
| `VICTORY_CARD_BUFFERS_MIN_KB` / `_MIN_MB` / `_MB` | title buffers from this size up keep their copy in card memory | 4 MiB / 2 GiB on a 16 GB card | `MIN_KB=64`: 77.7 → 67.0 ms (2 runs) |
| `VICTORY_WINE_MAP` | Wine: title physical pages shown with the host's `mmap` instead of views | off (`=1`) | with port 1: see r2 |
| `VICTORY_PORT_CONSTANTS` | renderer port 1: the title's graphics heap read by the card in place | off; under Wine also needs `_WINE=1` or the home path | alone under Wine: black world; with `PORT_HOME`: r2 |
| `VICTORY_PORT_HOME` | port 1's heap opened on the title's own pages (needs `WINE_MAP=1`) | off (`=1`) | r2: 67.0 → 53.1 ms (2 runs) |
| `VICTORY_DYNAMIC_BITS` | dynamic descriptor slots found a word at a time | on (`=0` off) | with `TABLE_MEMO`: 53.1 → 49.7 ms (2 runs) |
| `VICTORY_TABLE_MEMO` (`=verify`, `_SLOTS`) | a list remembers table entries it already made readable | off (`=1`) | see r3 |
| `VICTORY_GRAPHICS_WATCH` | D3D blocks ≥64 KiB get write-watched memory of their own | on (`=0` off) | r4: no gain |
| `VICTORY_SMALL_ALIGN` | title told 4 KiB alignment for small textures (console value) | on (`=0`: 64 KiB) | r4: no gain |
| `VICTORY_PIPELINE_WATCH` | Wine: a pipeline worker that dies is replaced, its pipeline given up | on under Wine | r4: no gain (recovery only) |
| `VICTORY_CLAMP_SYNC` | Wine: clamp waits as narrow (compute-only) barriers | off here (`=1`) | not yet; upstream saw no gain with batched clamps |
| `VICTORY_SLOT_COUNTS` | shaders read buffer record counts from a host table (RDNA 4 compiler crash workaround) | on (`=0` off) | off in all runs here |
| `VICTORY_COMPUTE_WAIT_MS` | first compute use waits for its pipeline up to this budget | off | not yet (upstream: fixes missing heads) |
| `VICTORY_JOIN_PARTS_DRAWS` | join a frame's submissions only after frames of ≥ n draws | unset | not yet |
| `VICTORY_WAKE_ONE` | spread work wakes only the helpers it needs | off | not yet |
| `VICTORY_HOST_EARLY` | host GPU made before the title starts (start watchdog) | off | not yet |
| `VICTORY_RT_MISS` / `VICTORY_RT_SKIN` | skip ray-tracing skinning passes | off | not yet |
| `VICTORY_FRAME_INTERVAL_MS` | our frame clock instead of the title's 30 fps | unset | not yet (matters below 33 ms) |
| `VICTORY_UNWIND_CHECKS` | profiler stack walk checks memory first | on | diagnostics only |

## The play-fast settings every run above uses

`VICTORY_PORT_CBV=1 VICTORY_MAP_LOCK_FINE=1 VICTORY_THIN=1 VICTORY_SKIP_MOVIES=end
VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1 VICTORY_PARTIAL_DELTA=1 VICTORY_REFRESH_ONCE=1
VICTORY_STEP_OVER=5149aec,63966f VICTORY_LATE_WAIT=default VICTORY_PIPELINE_WAIT_MS=2500
VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1
VICTORY_JOIN_COPIES=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2
VICTORY_INDIRECT_BATCH=2 VICTORY_MAP_THREADS=1 VICTORY_TITLE_PRIORITIES=0 VICTORY_CARD_AWAKE=1
VICTORY_SHOW_AFTER=1 VICTORY_AMBIENT=0.8 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1
VICTORY_PIPELINE_BOOK=1 VICTORY_LAND_SKIP=1 GLIBC_TUNABLES=glibc.pthread.rseq=0
WINE_CPU_TOPOLOGY=16:0,8,1,9,2,10,3,11,4,12,5,13,6,14,7,15`, FSR through `VICTORY_FFX_DIR`.
`play-fast9.sh` no longer sets `VICTORY_SKIP_MOVIES=end` (movies play); scripted runs keep it.

## Known picture issues

Sparkle on glass and speckle on floors; `VICTORY_PORT_CONSTANTS` without `PORT_HOME` under Wine:
black world. Purple blinds and violet trees on the first balcony: fixed with
`VICTORY_AMBIENT_MAGENTA=1` (in the play set).

## Next

1. The submitting thread is the frame: refreshes (17.3 ms) first — clankerVI's waves (`send_copied`,
   lock-free `refresh_front`, single-pass `send::take`), xavifortes' shared list-front scan, Caullen's
   `ASYNC_WAIT` / `DISPATCH_RUNS` / `MULTI_FLIGHT`.
2. Write-watch asks (5.5 ms): batch the sync guards' arm/disarm so the submitting thread doesn't pay them.
3. Merge `fix/rdna2-picture` with a full re-warm and a picture check; test `VICTORY_NO_WATCHDOG=1`.
4. GPU levers (wave32 on RDNA, card-side merges) on Series X 1440p, where the card is the limit
   (ab37: card ~89 %).
5. Frame generation: in-game check of 6697719ed, the camera data, then frame pacing at this base rate
   (AMD advises ≥60 fps base; ours is ~20).
6. Vice-Tay round (`perf/vicetay-round`): five correctness fixes to build, picture-check and merge.
