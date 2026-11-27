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

## Ladder on `main` (2026-11-27, cumulative: each rung adds to the one above)

| Rung | Adds | Frame (median) | p90 | Prep | GPU busy | Runs |
|---|---|---|---|---|---|---|
| r0 | every new switch off (play-fast settings) | 80.9 ms | 97.5 | 70.7 | 60 % | 1 |
| r1 | `VICTORY_CARD_BUFFERS_MIN_KB=64` | 70.0 ms | 88.0 | 57.7 | 64 % | 1 |
| r2 | + `VICTORY_WINE_MAP=1 VICTORY_PORT_CONSTANTS=1 VICTORY_PORT_HOME=1` (renderer port 1 under Wine) | 53.9 ms | 103.4 | 43.5 | 77 % | 1 |
| r3 | + `VICTORY_DYNAMIC_BITS=1 VICTORY_TABLE_MEMO=1` | **50.1 ms** | 67.2 | 40.4 | 79 % | 1 |
| r4 | + `VICTORY_GRAPHICS_WATCH=1 VICTORY_SMALL_ALIGN=1 VICTORY_PIPELINE_WATCH=1` | running | | | | 0 |

Second repetition of every rung is running. r2: port 1 opened part of the title's constants in
place and fell back to copies for stretches that are not one section ("title memory not in place,
a copy as before"); the world was drawn (6,594 draws a frame), picture not yet checked by capture.

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
| `VICTORY_CARD_BUFFERS_MIN_KB` / `_MIN_MB` / `_MB` | title buffers from this size up keep their copy in card memory | 4 MiB / 2 GiB on a 16 GB card | `MIN_KB=64`: 80.9 → 70.0 ms (1 run) |
| `VICTORY_WINE_MAP` | Wine: title physical pages shown with the host's `mmap` instead of views | off (`=1`) | with port 1: see r2 |
| `VICTORY_PORT_CONSTANTS` | renderer port 1: the title's graphics heap read by the card in place | off; under Wine also needs `_WINE=1` or the home path | alone under Wine: black world; with `PORT_HOME`: r2 |
| `VICTORY_PORT_HOME` | port 1's heap opened on the title's own pages (needs `WINE_MAP=1`) | off (`=1`) | r2: 70.0 → 53.9 ms (1 run) |
| `VICTORY_DYNAMIC_BITS` | dynamic descriptor slots found a word at a time | on (`=0` off) | with `TABLE_MEMO`: 53.9 → 50.1 ms (1 run) |
| `VICTORY_TABLE_MEMO` (`=verify`, `_SLOTS`) | a list remembers table entries it already made readable | off (`=1`) | see r3 |
| `VICTORY_GRAPHICS_WATCH` | D3D blocks ≥64 KiB get write-watched memory of their own | on (`=0` off) | r4 running |
| `VICTORY_SMALL_ALIGN` | title told 4 KiB alignment for small textures (console value) | on (`=0`: 64 KiB) | r4 running |
| `VICTORY_PIPELINE_WATCH` | Wine: a pipeline worker that dies is replaced, its pipeline given up | on under Wine | r4 running |
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

## Known picture issues (all builds tested since 2026-11-26)

Purple vertical blinds on the first balcony (same colour in v7900-final, fx-upscaler and `main`;
not from the merge); sparkle on glass and speckle on floors; `VICTORY_PORT_CONSTANTS` without
`PORT_HOME` under Wine: black world.

## Next

1. Second run of every rung; r4.
2. Picture check of r2/r3 (captures) — port 1 must not change the image.
3. Combine the winners into the play settings, then A/B the untested switches one at a time on top.
