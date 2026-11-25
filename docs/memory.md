# Memory: the data for designing an efficient system

This document collects everything known about how the title uses memory and what the platform allows. The platform is Linux, GE-Proton, vkd3d-proton, RADV and an RX 7900 XT. This is the basis for phases 1 and 2 of the [README](../README.md).

Every figure names its source. When something new is measured, it goes here.

## 1. How much memory moves per frame

Scene A, baseline at commit `176cc10`. Scene B is in [baseline.md](baseline.md).

| What | Per frame | Source |
|---|---|---|
| Copies to the GPU | **215 MB, 62 ms** (~3.5 GB/s effective) | [baseline.md](baseline.md) |
| Copy-back into title memory | **79 MB, 68 ms** | same |
| Textures (detile + upload) | 6-16 MB, 2-4 ms | same |
| Hash passes over copies in VRAM | 558 | same |
| Title remaps (static scene) | 1.9 maps, 7.5 unmaps, 0.5 ms under the remap lock | `VICTORY_MEDIR=1` |
| Draws / indirect dispatches | ~4,970 / ~635; ~340 dropped (no host pipeline) | same |

**What this means:**
- Copies run at ~3.5 GB/s, far below PCIe (~27 GB/s) or `memcpy`. The cost is in **deciding what to copy** (write watch, hashes, guards), not in moving bytes.
- With the patched wineserver, remaps no longer matter in a static scene (0.5 ms per frame).

## 2. Where the memory shaders read lives

Census from `VICTORY_INPLACE_PROBE=1`, in the world.

**Title buffers** used by draws: **3,705 buffers, 1,037 MB**.

| Kind of memory | Buffers | MB | Share of MB | Can it be opened in place today? |
|---|---|---|---|---|
| Private, committed whole (`XMemVirtualAlloc`) | 3,574 | 294 | 28 % | **yes** |
| Page-pool view, one run of a section | 114 | 422 | 41 % | no (section) |
| Page-pool view, scattered pages | 17 | 320 | 31 % | no |

**Windows outside buffers:** 7, 28 MB (4 MB private).

**What this means:**
- **72 % of the bytes shaders read live in the page pool**, in only 131 large buffers. Private memory is many small buffers (82 KB on average).
- Without rebuilding the pool (phase 2.3), in-place memory covers 28 % at most.
- **`INPLACE=2` attempt:** 3,555-3,611 buffers (188 MB) were in place in the world, and 740-1,507 were turned down as view memory. The device was lost after 60 s (0x887A0005). That has to be understood before the path is reused.

## 3. What the platform allows

Probe of 25 Nov 2026: vkd3d-proton v3.1-83-GE, RADV, Mesa 26.3.1.

| Test | Result |
|---|---|
| `ID3D12Device3` / `ID3D12Device13` | yes / yes |
| `OpenExistingHeapFromAddress` (whole allocation) | **ok** |
| `OpenExistingHeapFromAddress1` (part of an allocation) | **ok** |
| Resulting heap | custom type, CPU write-back, in system memory (L0 pool), 64 KiB alignment; flags shared, cross-adapter, write watch, shader atomics |
| GPU reads what the CPU wrote afterwards, with no call in between | **yes** (coherent) |
| CPU reads what the GPU wrote (by copy and by UAV) | **yes** |
| Write watch on opened memory | keeps working |
| Title protects or decommits pages while the heap is open | allowed, inside and outside the opened part |
| `OpenExistingHeapFromFileMapping` on a `SEC_RESERVE` section (today's page pool) | **refused** (0x80070057) |
| `OpenExistingHeapFromFileMapping` on a `SEC_COMMIT` section | opens, but **the GPU does not see the views' bytes**, neither reading nor writing |
| Cost of opening 64 MB never touched | 8.2 ms (private), 4.3 ms (section); the pages get locked in RAM |
| GPU copies 64 MB out of system memory | 2.4 ms, **~27 GB/s** (same as an ordinary shared buffer) |
| Cross-adapter row-major textures | no |
| Heap tier | 2 |

**What this means:**
- Private memory can be shared with the GPU reliably.
- The page pool cannot be shared through sections: vkd3d opens the section as separate memory, not as the views.

## 4. The cost of reading in place (`VICTORY_INPLACE=1`, 3 runs)

| | baseline | `INPLACE=1` |
|---|---|---|
| In place | — | 73-78 windows, 340-360 MB |
| GPU | 159 ms | **285-396 ms** |
| Copies | 63 ms, 216 MB | 84 ms, 401 MB (first run) |
| fps | 2.8 | 1.6-2.0 |

**What this means:**
- Shaders that read hundreds of MB over PCIe instead of VRAM spend **2-2.5× longer on the GPU**.
- In-place system memory is only good for small data, data read once, or data the CPU reads. Large data the GPU reads every frame has to be in VRAM.
- Why the copy volume went up under `INPLACE=1` is still open. The windows may also be uploaded through another path.

## 5. Ceiling without copies (`VICTORY_COPY_MODE=none`)

Same scene, with 5,037 draws per frame against 4,971. The picture is black, as expected.

| | baseline | no copies | change |
|---|---|---|---|
| ms per frame | 354 | **170** | −52 % |
| fps | 2.8 | **5.9** | ×2.1 |
| prepare | 124 | 72 | |
| · copies left | 63 ms | 22 ms, 68 MB | paths that do not go through `copy_mapped` |
| GPU | 159 | 72 | with zeroed data the GPU does less real work |
| copy-back | 67 ms, 78 MB | 23 ms, 0.15 MB | what is left is waiting, not bytes |
| hash passes | 558 | 125 | |

**What this means:**
- **Removing copies gives ×2 at most (≈6 fps).** Even with no copies, the frame is still a ~170 ms serial chain: prepare, wait for the GPU, copy back.
- Getting past ~6 fps needs **both** zero-copy memory (phases 1-2) and CPU/GPU overlap (phase 3).
- This justifies the order of the roadmap. It also means phase 3 must not be left for the end if phases 1-2 stall.

## 6. Title memory has holes (`VICTORY_COPY_MODE=plain`)

- With plain copies (`memcpy` without the guard), the game crashes before the first cutscene. The cause is `0xc0000005` in `ucrtbase` reading `0xd6cde0000: reserved, never committed`.
- **Title buffers span pages with no backing** (tiled or partly resident memory). That is why `copy_mapped` reads through the kernel (`ReadProcessMemory`) and treats an unmapped page as zeros.
- **Design requirement:** any new system must handle sparse ranges with holes, as the console does with tiled memory. On the GPU side that points to reserved resources and `UpdateTileMappings` (sparse binding in Vulkan).

## 7. Missing heads (investigation parked)

Characters show only eyes and mouth. In 3 of 18 runs, the faces of **all** characters rendered correctly, with skin and lighting.

| Run | Heads |
|---|---|
| baseline A (×3), `MEDIR`, `PROBE`, `PROBE+MEDIR` (×3), waiting 150 s then turning, full intro cutscene (×2), no `fc5e66f8` skip (×2), `INPLACE=1` (×2) | no |
| `INPLACE=1` (1st run), `PROBE+MEDIR` (1st run), **baseline B, no switches** | **yes** |

**What was established:**
- **It is all or nothing per run, decided at load.** Waiting or moving the camera changes nothing.
- **The game does not emit the head draws.** The runs with faces use **182 shaders** (66 mesh, 100 pixel, 8 vertex, 5 compute; list in [data/face-shaders.txt](data/face-shaders.txt)) that appear in none of the runs without.
  - None of them fails to translate.
  - VIctory drops the same ~340 draws per frame in every run.
- DirectStorage reports no failed request in any run.

**Hypotheses:**

| | Hypothesis | Status |
|---|---|---|
| H1 | level of detail by camera distance | ruled out (moving the camera does nothing) |
| H3 | skipping the cutscene | ruled out (2 full intros, no faces) |
| H4a | the AMD skip of the culling program `fc5e66f8` | ruled out (2 runs without it, no faces) |
| H4b | GPU-driven culling or LOD read back by the CPU through a stale copy-back | open, but `INPLACE=1` did not reproduce the faces (1 of 3) |
| H5 | timing during world load (streaming, at our 2-3 fps) | open; plausible, since faster loading would change it |

**Decision:** parked as a phase 5 item. Phases 1-3 change load timing and the copy-back, so re-check the heads after each phase. To take it up again, compare the GPU-written buffers the CPU reads at world load in a run with heads and one without.

## 8. The title's threads (`VICTORY_MEDIR=1`)

- 42 threads. **Even the least-waiting one waits 82 % of the time**, almost all of it in `WaitForSingleObject` at `title+0x2215eca` (job workers, 2-280 times a second).
- No waits on GPU fences (`[fence]`).
- Game CPU: 1.4 of 24 cores.
- **What this means:** the title's CPU is not the limit. There is plenty of room for our own work in parallel (phase 3) and for write tracking.

## 9. Memory layout

Taken from the belobug fork's memory log at boot, as a reference.

| What | Size |
|---|---|
| private | 6,747 MiB (6 allocations of 64 MiB or more add up to 4,995 MiB; the largest is 2,199 MiB at `0xf00200000`) |
| mapped (page-pool views) | 10,264 MiB |
| physical pages | 7,131 MiB in sections |
| host textures | 4,428 MiB (1,211) |
| VRAM in use | 4,761 of 18,943 MiB |

This still has to be measured with royale in the world; that log is not printed in the release build.

## 9b. Our own write-back was half the uploads (phase 1, 25 Nov 2026)

- With `VICTORY_F1_OWN`, ~100 MB a frame of the 216 MB uploaded turned out to be pages we had written back ourselves. 95 % of them were identical to the copy.
- Skipping them halves the bytes, but **not the time**: 63-66 ms either way. The copy time is the kernel reads of title memory (`copy_mapped`, `read_into`), not the bytes moved.

## 10. Design rules drawn from the data

1. **The cost is in deciding what to copy and in reading title memory through the kernel, not in moving bytes** (confirmed in §9b). Every phase 1 improvement is measured in hash passes and write-watch queries, not only in MB.
2. **VRAM for what the GPU reads heavily. In-place system memory only for small data or data the CPU reads.** Reading in place over PCIe doubles GPU time.
3. **The page pool (72 % of the bytes) cannot be opened through sections.** The GPU needs its own tile mapping over opened memory or VRAM; the CPU can keep its views.
4. **Tolerate holes:** title ranges have pages with no backing.
5. **Zero-copy alone is not enough (×2 at most).** It has to be combined with frames in flight (phase 3).
6. **Correctness matters:** any new cache must guarantee the GPU sees the title's latest write.
