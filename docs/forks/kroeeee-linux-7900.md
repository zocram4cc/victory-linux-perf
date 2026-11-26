# kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX: notes

Read and run on 26 Nov 2026 (`6c970fe`, 790 commits; clone `~/src/victory-linux-7900`, launcher `~/Games/victory-data/run-kroeeee.sh`, profile `perfiles/kroeeee` with a copy of royale-save's data, its own Proton Experimental 11.0-100 prefix). Base: avatarmanz/VIctory-7900XTX (as cool_fox) with Linux changes: single-latch loops in its DXIL (dxil-spirv stack overflows), renderer port 1 on private pages under Wine, **write guards that remember uncommitted regions instead of asking Wine's `VirtualQuery` 75,000-216,000 times a second**, `VICTORY_COPY_COMPARE`, and renderer ports 1, 2, 5, 6, 7 (`VICTORY_PORT_*`), with `VICTORY_THIN=1`, `ASYNC_SUBMIT`, `SHOW_AFTER`, `INDIRECT_BATCH=2`. The author reports ~16-17 fps in the open world on a 7900 XT + 5800X3D.

## On this machine (Jason's house)

| run | fps | ms/frame | preparing | copies | GPU | copy-back | "asking the write watch and the guards" |
|---|---|---|---|---|---|---|---|
| 1st (16 GB in swap, pipelines being made) | 7-9 | 108-144 | 73-102 | 28-51 | 2-28 | 20-25 | 60.6 ms |
| 2nd (5 GB in swap) | 12.5-13.7 | 73-80 | 58-74 | 22-29 | 2-4 | 25 | 11-27 ms |

The author gets 0.5 ms on the write-watch line, ~15 fps at ~7,500 draws.

## What it tells us

- **Their GPU takes 2-28 ms a frame for the same scene; ours ~165 ms.** Our GPU time is ours (our translation, clamps, barriers or pipelines), not the card's or RADV's. The biggest single lead we have.
- Their frame is CPU preparation (copies) and copy-back; ~5,000-8,000 draws.
- Write watch under Wine is very sensitive to swap (zram): 60 ms with 16 GB swapped, 11-27 with 5. Check ours the same way.
