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
