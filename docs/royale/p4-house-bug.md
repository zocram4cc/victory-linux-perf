# P.4: the half-invisible house (27 Nov 2026), found, explained and fixed

## What the user saw

With the then 12 "whole" 0x218 families on (77 native compute programs: `VICTORY_NATIVE_COMPUTE=1 VICTORY_NATIVE_COMPUTE_ONLY=bench/p4-full-families.txt`), **half of Jason's house was invisible**: the walls and the deck floor were gone, while the windows, the blinds, the plants and Jason himself were drawn (`bench/p4-12families-house-missing.png`). The intro before the loading screen also flickered more than usual. Without P.4 the house is whole.

## How it was found (about 40 minutes, unattended)

1. **One list per family.** `bench/p4-fam/<root signature>.txt` holds the programs of each whole family (12 then).
2. **Halving by family**, with `scripts/p4-family-bisect.sh`:
   - Each run starts from the original save (`K_SAVE`) with P.4's own pipeline cache (`K_CACHE=…/cache-p4`).
   - It takes the world-entry screenshot (`K_EARLY_SHOT=1`) and measures the mean luminance of the window region on the left (the house's dark interior).
   - Calibration: no P.4 gave 62, all 12 families 142 (the house gone, sky behind).
   - Steps: the first 6 held (65); of the other 6, the first 3 broke it (140); of those 3, the first held (61); of the last 2, the first gave 22.

   The script then picked `0x33cb34f0`.
3. **The script's answer was wrong, and the user caught it.** 22 is not "holds": it is darker than normal. While the confirmation run of the other suspect was on screen, the user said "this one has the house problem".
4. **Each suspect alone**, screenshots side by side (`bench/k-…-fam-only-*`):
   - `0x33cb3490` alone: house gone.
   - `0x33cb34f0` alone: house there. The window region was darker because the camera stood elsewhere: plants behind glass.
   - no P.4: house there.

   **The culprit is `0x33cb3490`.**
5. **When it became whole.** It only became whole with translator step 17 (`git log -- bench/p4-full-families.txt`: 0 of its 4 programs in the list before, 4 after). That points at step 17's code.

**Lesson for the script:** a run is broken when its window value is *far from* the plain run's, not only when it is above a limit. Next time, judge by the distance to the calibration run, both ways.

## What family 0x33cb3490 is

4 programs (`218_0000000f00a2d400`, `…a2e600`, `…a2f800`, `…a33700`).

**The game's GPU occlusion culling.** It decides which objects to draw:
- 64 threads, wave64;
- it reads instances and meshes from structured buffers (288- and 456-byte elements);
- it tests each bounding box against the depth pyramid (`image_gather4_l` and three `image_gather4_l_o` with offsets (2,0) (0,2) (2,2): a 4×4 block). The translator already turns these into four `textureLoad`s at the level;
- it writes the list of visible objects and its counter.

If it wrongly concludes "hidden", the object is not drawn. That is why whole pieces of the house were missing rather than black.

## The cause

The program fetches its descriptors like this (the start of `218_0000000f00a2d400`):

```
s_mov_b32  s16, s7          ; s[16:17] = (s7, 0): a 32-bit base
s_mov_b32  s17, 0
s_lshl_b32 s9, s6, 5        ; s6 * 32: an index into a heap of 32-byte descriptors
s_load_dwordx4 s[20:23], s[16:17], s9    ; a buffer descriptor (V#)
s_buffer_load_dwordx2 s[8:9], s[20:23], s18   ; used as a buffer right away
```

So **user SGPR 7 is the view heap's base and SGPR 6 an index**. The packet's RSRC2 gives 8 user SGPRs (s0-s7).

Our layout (`native::fspx_program`) was:

| param | user SGPRs | why |
|---|---|---|
| Table (slot 0) | s4 | record `41 … 00 03 …`, user data 4 |
| Constants (slot 1) | **s5-s8** | record `41 … 02 00 …`, `e[20]=4`, user data 5 |

- **`e[20]` is the descriptor's size in dwords** (4 for a buffer V#), not the number of SGPRs of the root constants. s5-s8 even runs past the 8 user SGPRs.
- Because that range "covered" s6 and s7, `fspx_program` placed no implicit heap (its rule puts the view heap at n-2 = s6 and the sampler heap at n-1 = s7 where nothing listed covers them). Here the view heap is at **n-1**, so the rule would have been wrong too.
- The tracer therefore saw s7 as "an address in an SGPR that is no root parameter" and gave it `param: MEMORY`.

Before step 17 that stopped translation ("a descriptor outside the heaps"). **Step 17 accepted any `MEMORY` address** as "an address read from memory", computing `(address − heap start) / 32` with the heap start as a host word. In game, though, the layer does not fill s7 at all (no layout entry: it stays 0). The load read `0 + s6*32 − start`, which is not a descriptor. The log has the trace: `title memory by address: shaders touched pages that had no copy … some=["0x10000"]`, reads near address 0.

## The fix (commit "descriptors through a memory address only when that address was loaded from memory")

- `resource::by_address` now records, in `Address::offset` (unused for `MEMORY`), **which of the `RAW` reasons** made the address raw.
- `emit/lower.rs` (`heap_index`, `fetched_index`) takes the step-17 path only for `MEMORY_LOADED` (3, "an address loaded from memory").
- `VICTORY_WHY_FROM_MEMORY=1` prints each load that takes it, with the address record and the layout (user SGPR ranges).

**Result:** none of the 46 programs step 17 had gained has a really loaded address; all were this misplaced-heap case. The count goes **1,248 → 1,202**. Whole families go back to 9 (40 programs: `bench/p4-full-families.txt`). The game was relaunched with those for the user to check the house.

## What remains (and would bring the 46 back properly)

1. **Root constants' SGPR count.** It cannot be read from `e[20]`. It should come from the root signature (`Num32BitValues`). The dump's `root.bin` is a title object (it holds `[compute b80cd30e37e3fa05]`, the program name), not a serialized signature: decode it, or take the count from the code (the highest user SGPR read below the heaps).
2. **Where the heaps are.** Here the view heap is at n-1, against our n-2 rule. Census which user SGPRs are used as load bases with an index × 32 (view) or × 16 (sampler) across the 2,680 programs, and derive the rule.
3. With both, the layer fills that SGPR with the heap's console start (as for the existing implicit heaps), and these programs translate as plain heap loads, without step 17's path.
4. **Every family that becomes whole: check it in game** (`scripts/p4-family-bisect.sh`, or one run per family) before trusting it.

## Follow-up (27 Nov, 17:20-17:40): the heap's place fixed, the house still breaks

- **The view heap's SGPR now comes from the code** (`native::view_heap_sgpr`):
  - it finds the user SGPR used as a 32-bit base `(sX, 0)` with an offset shifted left by 5, anywhere in the program;
  - only s[n-1] or s[n-2] are accepted (the census: 187 programs use s[n-1], 59 use s[n-2], always times 32);
  - root constants are clamped below it.

  This took family 0x33cb3490 through the heap instead of step 17's path, and gave **1,202 → 1,228** overall, none lost.
- **A test that proved nothing** (`bench/k-20261127-171822-fam10-heapfix`): its list was built with `cat bench/p4-fam/*.txt`, which still held two families that were no longer whole (`0x306fbc40`, 31 programs; `0x33cb34f0`). That made partial chains, which break the picture by themselves. **The family lists are now regenerated from the current whole families only; stale files are removed.**
- **The clean test** (`bench/k-…-cull-alone-heapfix`), 0x33cb3490 alone with the heap fix: **half the house is still missing**. The user saw it too. So the culprit is certainly this family, and the misplaced heap was *a* fault of it, not the one that culls the house.
- **0x33cb3490 stays out** (`bench/p4-bad-families.txt`). The play list `bench/p4-full-families.txt` holds the other 9 whole families (40 programs), which held in game (user, 16:55).
- **Next suspects in the culling programs, offline:**
  1. **s5.** The record at user data 5 is kind 2 (CBV) with 4 descriptor dwords, which our layout calls root constants. The code reads it as `s_and_b32 s9, s5, 0x3ffffff`, a 32-bit address of a buffer. If it is a root CBV's address, the layer must fill it with the address, not with constants.
  2. **The depth-pyramid reads** (`image_gather4_l_o`: level rounding, edge clamping, the gather order).
  3. **The visible-list counter** (raw buffer atomics).

  A way to tell them apart: run one program of the family against a captured dispatch (inputs from a frame), and compare its output with the console's.

## Meticulous tests on the isolated family (27 Nov, from 17:30)

- **The end of the program**, the visible-list write:
  - the record word `s8` (dword 4 of a 24-byte record per group, loaded with `s_buffer_load_dwordx2 s[8:9]`) carries a counter slot in bits 20-29 and an output base in bits 0-19;
  - `buffer_atomic_add` (returning) on the counter buffer (table + 0x80, made raw in code) at `slot * (s5 & 0x3ffffff) & -4`, where s5 is the first root constant;
  - `buffer_store_format_xyzw … idxen` of the visible entries into a typed buffer (table + 0x60) at `base + old count + lane`.
- **A hypothesis dropped:** at first I read `s8` as the group id. With TGID_X enabled in RSRC2 (0x90: 8 user SGPRs, then the group id), the group id *is* s8 at the start, and its bits 20+ would have collided with the layer's 2^20-group clamp on indirect dispatches. But the program overwrites s8 with the record word before that use: the group id only picks the record (`s8 * 24`). Not the cause.
- **The trace** (`VICTORY_NATIVE_TRACE=60`) printed nothing for these 4 programs. Their dispatches do not go through `draw::dispatch`'s native trace, so they are most likely indirect (ExecuteIndirect). Next: trace there.
- **One program at a time** (`bench/p4-cull/<program>.txt`, runs `bench/k-…-cull-one-*`): results below.
