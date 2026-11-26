# Baseline

## Scene A (static), commit `176cc10`, 25 Nov 2026

- **Machine:** RX 7900 XT with RADV (Mesa 26.3.1), Ryzen 9 9900X, GE-Proton11-14, wineserver with the view tree.
- **Switches:** those of the `royale` profile: `VICTORY_SKIP_PIPELINES=fc5e66f8f873d3bb`, `VICTORY_NO_WATCHDOG=1`, `VKD3D_CONFIG=descriptor_heap`.
- **Cache:** translations warm (`warm --no-pipelines`).
- **Runs:** three of 60 s, 56 rows each. `ms_per_frame` varies by less than 1 %.

| Measure | A1 | A1b | A3 | **Mean** |
|---|---|---|---|---|
| fps | 2.8 | 2.8 | 2.8 | **2.8** |
| ms per frame | 354.3 | 355.3 | 358.3 | **356** |
| worst frame | 363.0 | 363.5 | 365.8 | **364** |
| prepare (CPU) | 123.9 | 123.0 | 124.8 | **124** |
| · copies | 63.4 ms / 215.7 MB | 60.8 / 214.3 | 62.1 / 215.0 | **62 ms / 215 MB** |
| · textures | 4.0 | 3.9 | 3.8 | **4** |
| GPU | 158.6 | 159.0 | 159.4 | **159** |
| GPU busy | 45.1 % | 45.2 % | 44.9 % | **45 %** |
| copy-back | 67.2 ms / 78.3 MB | 68.8 / 77.9 | 69.5 / 80.6 | **68 ms / 79 MB** |
| hash passes | 558 | 559 | 558 | **558** |
| game CPU | 1.4 cores | 1.4 | 1.3 | **1.4** |
| wineserver CPU | (not measured) | 0.06 | 0.06 | **0.06** |
| draws without a host pipeline | ~340 | | | **~340** (constant: not the cache) |

**What this means:**
- Copies up and back take about **130 ms** of the 356 ms frame (37 %).
- During the 159 ms "GPU" phase the card is busy only 45 % of the time; the rest is waiting.
- The game's CPU is not saturated (1.4 cores).
- The patched wineserver no longer matters (0.06 cores).

The raw results are in `bench/`, which stays local.

## Scene B (walking 30 s), commit `b6660c1`, one run

| fps | ms per frame | prepare | GPU | copies | copy-back |
|---|---|---|---|---|---|
| 2.6 | 386 | 144 | 167 | 69 ms / 229 MB | 70 ms / 86 MB |

Moving costs 9 % more than the static scene, because more is copied. Two more runs are needed before this counts as a baseline.

## Ceiling experiments (phase 0.5), 25 Nov 2026

| Run | fps | ms per frame | GPU | copies | Picture |
|---|---|---|---|---|---|
| baseline | 2.8 | 354 | 159 ms | 63 ms / 216 MB | no heads |
| `COPY_MODE=none` | **5.9** | **170** | 72 | 22 / 68 | black (expected); same draws as the baseline (5,037) |
| `COPY_MODE=plain` | — | — | — | — | crashes before the cutscene: reads a *reserved, never committed* page |
| `INPLACE=1` (3 runs) | 1.6-2.0 | 503-638 | 285-396 | 84 / 401 | heads in 1 of 3 |
| `INPLACE=2` | — | — | — | — | **device lost** on reaching the world (0x887A0005) |
| `INPLACE_PROBE=1` + `MEDIR=1` (3 runs) | 2.7-2.8 | 361-369 | 159-160 | 63-67 / 215-230 | heads in 1 of 3 |
| `MEDIR=1` | 2.8 | 358 | 160 | 62 / 216 | no heads |
| `INPLACE_PROBE=1` | 2.7 | 366 | 161 | 63 / 216 | no heads |
| no `fc5e66f8` skip (2 runs) | 2.8 | 353-356 | 159 | — | no heads, no GPU fault |

The first `COPY_MODE=none` and `plain` runs were invalid: the switch only worked in the diagnostics build. It is now `medir::copy_mode`, available in any build, and the runs above are the repeated ones.

The analysis is in [memory.md](memory.md).

## Open world, outside the house (Jason save), 26 Nov 2026

- **Machine and stack:** as above; build `c697e7c` (texture pool and slot wait on).
- **Scene:** the user's save (`royale-save`), the house seen from outside for the ~45 s after the world comes up; then the save cuts indoors, which is not measured. `BENCH_SETTLE=5 scripts/trials.sh -n 3 -l world-out -s 30`.
- **Runs:** three of 30 s, all valid, none lost the device.

| Measure | run 1 | run 2 | run 3 | **Mean** |
|---|---|---|---|---|
| ms per frame | 495.6 | 504.4 | 491.5 | **497** (2.0 fps) |
| prepare | 124.3 | 116.6 | 115.8 | **119** |
| copies | 49.4 ms / 134.5 MB | 53.9 / 132.4 | 48.8 / 136.7 | **51 ms / 135 MB** |
| GPU | 261.8 | 268.7 | 265.7 | **265** (card busy 53 %) |
| copy-back | 74.2 ms / 46.1 MB | 71.1 / 45.8 | 71.5 / 45.0 | **72 ms / 46 MB** |
| draws | 12,909 | 13,343 | 13,188 | **13,147** |
| heap buffer pass | 133 ms, 1 a frame | | | |

With the pool off: 485-502 ms, the same within noise.

## Where the open-world frame goes, and what each piece is worth (26 Nov 2026, morning)

Jason save, outside the house, ~480 ms a frame (2.0-2.1 fps), `main` at `7b58468`. Sources: [journal](journal.md) entries of 26 Nov; `bench/trials-20261126-*`.

| Part of the frame | Cost | Piece | What the measurements say |
|---|---|---|---|
| The card working | ~265 ms (223-233 ms with the copy-back off) | Q | near its limit in this way of working |
| The card idle while our thread prepares and copies back | 173-228 ms | B, M, Q | the serial chain; 3.1 overlaps it (~270 ms a frame in theory) |
| Copy-back | 72 ms, ~40 MB | B | the CPU reads ~1 page of ~10,600 a frame (census) |
| Snapshots before each submission | 408-437 MB copied | B | needed only because the copy-back is |
| Ceiling without copy-back and snapshots | 362-374 ms (−22 %), black picture | B, M | GPU→GPU coherence goes through title memory: ~245 written ranges a frame overlap another copy, always a buffer copy over a 4 MiB window |
| Heap pass (`heap_buffers`) | 30 ms average, 231 ms spike every 16 passes | D | not on the critical path: the frame does not move with it |
| Streaming textures by index (`heap_textures`) | 6-115 ms | A | detiling is 0.0-0.4 ms of it; the rest is texture creation, view mirrors, mapped-page queries |
| Draws dropped for want of a pipeline | ~300 + ~900 indirect a frame | P | two Xbox pipeline methods (`0x1c8`, `0x218`) make pipelines from console RDNA2 code |
| World entry | 9.5 s frame → 0.63 s | P | the pipeline library was never made; fixed (`c436241`) |

**The order that follows:** M.1 (one copy per byte: windows under buffers) → 1.3 step 2 (copy-back on demand: the −22 %) → 3.1 (frames in flight). Alongside: P.4 (the missing pipelines, for the picture), A.2 (texture creation off the recording thread).
