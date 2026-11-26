# cool_fox (junikka/cool_fox, from avatarmanz/VIctory-7900XTX): notes for track U

Read on 26 Nov 2026 at `f4b48ad` (627 commits on fxceless/victory-fork). Windows 11, i9-12900K, 64 GB, **RX 7900 XTX**, 1440p, Series X. A checkout is at `~/src/VIctory-coolfox` (the user is trying it under our launcher); read only. Its `docs/findings.md` holds the measurements.

## Their numbers

- Prologue: 280 → 46-65 ms a frame in two days; first boat mission 46-53 ms (19-22 fps); street chase 100-165 ms.
- Of a 55 ms frame: **the preparing thread 48 ms** (their wall), **the card ~41 ms busy**. List calls: 36,000 a frame, 230 ms of work over eight threads.
- Memory: 40-46 GB private (list allocators 9 GB, windows 7, uploads 4): 64 GB of RAM recommended.
- Ours (26 Nov): ~400 ms a frame, the card ~187 ms on a 7900 XT under vkd3d-proton/RADV. **The card's work is ~4.5× theirs**: in our layer, or in vkd3d/RADV against AMD's Windows driver. Their build on this machine would tell which.

## What gave them the most (their table, in order of size)

| What | Frame |
|---|---|
| **Descriptor records of draws' tables left to the draws** (made at the draw, not mirrored whole) | 280 → ~124 ms |
| Our hash passes over card copies, 64 threads a piece | card 79 → 66 ms |
| **Preparing on a thread of its own** (`VICTORY_ASYNC_SUBMIT`), refreshes and records spread over threads | 103 → 75 |
| **`VICTORY_THIN`**: list calls written down and translated by eight threads | 75 → 68 |
| Submissions joined (`VICTORY_JOIN_PARTS`), steps on the card (`VICTORY_CARD_STEPS`), textures sent ahead | 68 → ~63 |
| Title bytes under shader writes compared with AVX2 on six threads | 14 ms → 4-7 |
| Stores into guarded memory looked at first, fewer guard resets | 69 ms of threads → ~6 |

Also: textures on a copy queue of their own, shown only after their upload ran (ended a device loss every ~16 min: `VICTORY_SHOW_AFTER`, our S.1 family); the ocean's maps read back (boats float, `VICTORY_TEXTURE_BACK`); `victory warm --used` (pipelines ahead, as our P.2); indirect dispatches cut at 65,535 groups (blocky patches; ours: `bound_split`); quad wave reads with helper lanes on AMD (holes in shadows); DXIL `Log` is base 2.

## Measured there and not worth it

Series S profile, smaller textures, a second drawing queue, wave sizes, fast math, leaving out barriers ("nothing to 0.1 ms each"), running console ISA natively ("the Windows driver takes no raw code").

## Their build on this machine (26 Nov, the user played it, no warm)

`perfiles/coolfox/logs/20261126-100428-run.log`: **436-603 ms a frame (~2 fps)**, 3,900-6,800 draws. Faces show; some textures broken (others report it too). The card's clock: **about 11 % busy (~60 ms a frame), and most of the frame "the title had not submitted"**: on Linux their wall is before submission (the title's threads and their list-call translation under Wine), not the card. Their frame line's "on the GPU" (20-34 ms) counts the submitting thread's wait, not the card's work; by the card's clock the cost per draw (~12 µs) is close to ours (~15 µs). So their Windows card figure (41 ms) is not evidence of a cheaper layer on this platform.

## For us

1. **Run their build here** (the user is): their fps on Linux says whether our gap is the layer or the platform.
2. Their biggest win, descriptor records left to the draws, is our D.1 territory (the heap pass, `heap_buffers`, 30-230 ms) and the descriptor census (S.5: ~225,000 records a frame copied as bytes).
3. `VICTORY_THIN` and `VICTORY_ASYNC_SUBMIT`: the title's list calls and our preparing off the title's threads, the CPU side of our serial chain.
