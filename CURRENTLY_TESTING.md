# CURRENTLY TESTING

Status of the switches on `main`, and every frame time recorded so far on the test machine.
**Work in progress: numbers move as runs land. Read "1 run" figures as ±5 ms** (Series S runs are
bimodal; differences under ~5 ms need ≥4 runs an arm).

## Test machine and method

- Ryzen 7 5800X3D, Radeon RX 6900 XT 16 GB, Linux 7.3, Mesa RADV 26.3, Proton Experimental
  (11.0-20261112) with the view-tree wineserver, Series S mode (`CONSOLE=series-s`, 720p + FSR).
- Scripted runs from a fresh save: keys to the world, then 150 frame-time rows (one a second) of the
  world (`draws > 100`). Figure = median `ms_per_frame` of those rows; "prep" = median `cpu_prepare_ms`.
- Release build (`--features scripted`), caches warm (prebuild + full `warm`, 93,821 pipelines).

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

Play set now: the play-fast settings below + combined + `VICTORY_SKIP_WC_BACK=1
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

## Known picture issues

Sparkle on glass and speckle on floors; `VICTORY_PORT_CONSTANTS` without `PORT_HOME` under Wine:
black world. Purple blinds and violet trees on the first balcony: fixed with
`VICTORY_AMBIENT_MAGENTA=1` (in the play set).

## Next

1. The heap pass (~15-20 ms, on the submitting thread) is the holdup now; the async heap worker
   (`VICTORY_HEAP_WORKER`) stays off (tail +84 % in ab13).
2. `VICTORY_EARLY_TEXTURES_MS` (12 here; 40 in another fork's launcher) and
   `VICTORY_BAKE_TEXTURES_MS` (1500; 10000 there): single pass ab16.
3. Vice-Tay round (branch `perf/vicetay-round`, not merged): five correctness fixes built and
   tested before the round's last reverts; build, picture check and merge next session; then
   `VICTORY_COPY_BACK`, `VICTORY_LIMITED_STAGING_MB=32`, `VICTORY_IDLE_UNDER_PRESSURE` to re-apply.
4. Picture A/B for the correctness switches of fast track 2 (`HEAP_RECHECK_PERSISTENT`,
   `EXACT_NO_WAIT`, `JOURNAL_SHARED=2`).
5. `VICTORY_FRAME_INTERVAL_MS=16.667` once frames get near the title's 33 ms.
