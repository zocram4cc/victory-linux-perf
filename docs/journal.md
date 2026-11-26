# Journal

What was tried, what came out of it, and what was decided. Newest first. The Spanish journal before 25 Nov 2026 is in [history/es/diario.md](history/es/diario.md).

## 2026-11-26

**Buffers in place, what is ruled out and what is left (end of the 26 Nov sessions on M.2)**
- With the directory bias fixed, `VICTORY_INPLACE=2` (windows and buffers) still breaks (the user: "se ve igual de roto"), and so does `=3`.
- **Ruled out:** shader writes into memory in place under copies (**0 shader writes are declared on buffers in place**, `…-inplace3-texcensus`); title buffer copies (0 touch it); host-import failures (`VKD3D_DEBUG=warn`, `…-inplace3-warn`: none, only the usual alignment notes); the directory offset (fixed, not the whole cause).
- **What holds:** windows alone in place (right picture, GPU 2× slower over PCIe); the ≥ 1 MiB buffers alone (right, no gain: read-only, cheap to copy). **What breaks:** the small and mid buffers together in place, and in-place runs lose the device now and then (`…-inplace3-warn` lost it).
- **Hypothesis left:** those buffers hold per-frame data (constants, transforms) that the title's threads rewrite while the card is still drawing. Copies hand the card a snapshot taken at submission; in place it reads what is being written. The console's card runs within the frame; ours runs ~200 ms later and is waited for, while the title's other threads go on. If so, in place needs to know when the title considers each range free (its fences and rings), or must keep copies for dynamic data and share only static data.
- **M.2 status:** the mechanism works end to end on this machine (VRAM at the title's address under Wine, patched `winevulkan` and vkd3d). What is missing is the rule for which memory may be shared. Parked here with all switches off; nothing of it is on by default.

**In-place buffers: a bias bug in the address directory, and GPU-written memory under copies**
- **Bug:** the directory of title memory by address (`memory.rs`, for shaders that read through raw addresses: the ray-tracing builder and the lighting that walks its structures) gave a page's offset as `offset_in` (address − copy start). For memory in place the buffer starts at the 64 KiB line below the title memory, so the right offset is `Shadow::offset` (with the bias). Fixed (`memory.rs`). Descriptor views already used `offset`.
- With the fix, **the ≥ 1 MiB buffers alone in place hold the picture** (`…-inplace-bigcensus`: 35 buffers, 135 MB, **none written by shaders**; the user: "ahora se veía todo bien"), 399-411 ms.
- **All buffers in place still break it** (`…-inplace3-fixed`, the user: "se ve la casa rota de nuevo"; 377 ms, GPU 179): 299 ranges in place, **132 under host copies** (50 MB). The big ones lie under copies too (26 of 35, 52 MB), but nothing writes them on the card; among the small and mid ones some are written by shaders, and **a shader's write into memory in place is not seen by a copy of the same bytes**. One copy per byte matters exactly for memory the GPU writes.
- Also: the title's buffer copies (`CopyBufferRegion`) are made by the CPU at submission; with both sides in place they are now recorded on the card in list order (`list.rs` `in_place_copy`). None happened here (0 a frame; 6-7 tiny mixed ones).
- Next: `VICTORY_INPLACE=2` (windows and buffers together, ~no overlap) again with the directory fixed: its earlier break may have been the bias bug.

**M.2 milestone: title buffers live in VRAM at their own addresses under Wine; the picture breaks as in RAM, so the fault is the layer's, not the memory's**
- **Wine:** `winevulkan`'s Windows-side loader (`dlls/winevulkan/loader.c`, `is_device_extension_supported`) refuses extensions it does not list, and `VK_EXT_map_memory_placed` is unlisted (`make_vulkan`, `UNEXPOSED_EXTENSIONS`); forcing it from vkd3d gave `vkCreateDevice` `VK_ERROR_EXTENSION_NOT_PRESENT` (-7) and a white screen. **Patched to let that one extension through**, and only `winevulkan.dll` built: GE's prepared tree had an empty generated `include/wine/vulkan.h`, regenerated with `make_vulkan -x vk.xml -X video.xml`; built in a separate tree configured `--with-vulkan` (`wineserver/build-GE-Proton11-14/build-vk`). Installed in a **reflinked copy of GE-Proton11-14** (`victory-data/proton-vr/GE-Proton11-14-vr`, `VP_PROTON`); the user's GE is untouched. The original DLL is kept beside it (`winevulkan.dll.ge`). The shared source's `loader.c` and generated files were changed; the GE tree is reset by the launcher's prep when it rebuilds the wineserver.
- **vkd3d-proton** (`~/src/vkd3d-proton-vr`, `victory-vram`): also asks for `VK_EXT_map_memory_placed` and `VK_KHR_map_memory2` though Wine does not list them.
- **Result** (`bench/trials-…-vram-wine`, `VICTORY_INPLACE=3 VKD3D_VRAM_AT_ADDRESS=1`): the device is made, **286 "are VRAM now" placements**: title buffers in ReBAR VRAM at their own addresses, read and written by the CPU and the GPU there. The console model works end to end on this machine.
- **The picture breaks exactly as with buffers in place in system memory** (the user: "la casa se ve rota, le faltan cosas"; screenshot in the session: the house dark, the dock and the water cut). So the userptr-invalidation hypothesis is wrong: what breaks is how the layer treats the big buffers (≥ 1 MiB) once they are in place, whatever the memory. Next: which big buffers, and what the layer does with them that assumes a copy.

**Buffers in place, split by size: what breaks is the ≥ 1 MiB ones, and they are nearly all the bytes**
- `VICTORY_INPLACE_BUFFER_KB=min,max` (new, bisection) with `VICTORY_INPLACE=3`, the user watching each run:

| Buffers in place | Count / MB | Picture | Frame / GPU |
|---|---|---|---|
| < 64 KiB (`…-inplace-small`) | 214 / 1 MB | right | 351 ms / 129 ms; **lost the device** at the end |
| 64 KiB-1 MiB (`…-inplace-mid`) | 70 / 15 MB | right ("lo que ha cargado no se vuelve negro y Jason se ve bien") | 427 ms / 194 ms |
| ≥ 64 KiB (`…-inplace-big`) | 100 / 130 MB | **broken** | 434 ms / 220 ms |

- So the buffers ≥ 1 MiB (~30, ~115 MB) are what breaks the picture, and they are the bytes that matter; what holds in place is too little to gain from.
- **Hypothesis to test:** buffers in place are host-pointer imports (`VK_EXT_external_memory_host`: amdgpu userptr). The title's memory is write-watched; Wine's write watch resets protections at each submission (`mprotect`), and every protection change on a userptr range triggers an MMU notifier that invalidates the import. Big buffers the title rewrites often would be invalidated again and again: stale reads, and now and then a lost device. Windows in place held because what lies in them is written seldom (to check). **VRAM placed at the address is device memory, not an import**: it would not have this problem. That is the next step, and it needs the Wine patch.
- The user asked whether the biggest could be compressed: not for these (the GPU reads them raw, every frame, and the cost is keeping them in step, not their size); textures already travel compressed (BCn) and the card compresses its targets itself.

**Windows alone in place: the picture is right; buffers in place are what breaks it. Windows over PCIe cost the GPU 2×**
- `VICTORY_INPLACE=1` (`bench/trials-20261126-102825-inplace1`): 78 windows, 360 MB in place, no buffers. **The picture is right** (`shot-1.png`: the house in colour, the dock and the water whole, Jason on the deck), better than with copies. But **GPU 379 ms** (187 with copies) and 582 ms a frame: the windows' 360 MB read over PCIe, as on 25 Nov.
- So **buffers in place break the picture, not memory in place as such**, and not copies over them (none left under `=2`). Something about how a title buffer is opened in place (`inplace::place_part`: the 64 KiB-aligned heap around it, the bias, its length, or its retirement when the title releases the buffer object while its memory lives on) is wrong. That is the next thing to find, by halves (by size, by kind of use).
- For M.2 this means: windows can be in place and correct; to be fast they must be in VRAM, and windows are where the title's CPU reads (63 of 70 IBS samples), so windows need the per-page split (CPU-read pages in RAM, the rest in VRAM).

**`VICTORY_INPLACE=2` on today's base: no overlaps left, the picture still breaks; GPU 244 ms**
- `bench/trials-20261126-102344-inplace2` and `…-inplace2-copies`: 78 windows (368 MB) and ~3,300 title buffers (188 MB) in place; **3,372 ranges in place, 4 under any host copy, 0.0 MB of overlap**. Frame 454-457 ms, **GPU 244 ms** (187 with copies: the GPU reads system memory over PCIe, as on 25 Nov). One of three runs closed before the world.
- **The picture breaks the same way** (`shot-2.png`: the house dark, the dock and the water cut and bent, debris over the water). So it is not copies over in-place memory: in-place memory itself loses or mistimes something. Next, by halves: windows only (`=1`), then buffers by kind.

**Summary of 26 Nov (written at the user's request)**

*Gains, on by default, picture right:*
- **Pipeline library fixed** (`c436241`): world entry 9.5 s → 0.63 s; ~1,000 pipelines loaded from disk on later runs. The intro cutscenes play nearly whole and load in time for the first time (the user).
- **Indirect draws no longer serialised** (I.1 `VICTORY_INDIRECT_BATCH`, I.2 `VICTORY_INDIRECT_WINDOW`): clamps inline 9,700 → ~830 a frame; **GPU 272 → 187 ms, frame ~500 → ~400 ms (2.0 → 2.5 fps)**. Gates: Jason 4 of 4, prologue 3 of 3.
- S.1 (device loss) fixed overnight (texture pool, slot wait); S.7 guarded.

*Measured and understood:*
- **Where the frame goes** ([baseline.md](baseline.md)): the card ~190 ms, our thread ~200 ms (refreshes ~125 MB, copy-back ~45 MB, snapshots), one after the other.
- **The copy-back cannot simply go** (1.3 closed): title memory is the byte merge between the title's page-tracked writes and the GPU's writes (~125 MB a frame of GPU bytes would come back stale). Ceiling without it: −22 %, black picture.
- **The GPU is draw-bound, not pixel-bound** (1080p and Series S change ≤10 %); G.1 census by pipeline in release.
- **Two other forks read** ([forks/neptune-vi.md](forks/neptune-vi.md), [forks/cool-fox.md](forks/cool-fox.md)): cool_fox on this machine runs ~2 fps (its wall here is before submission).

*The direct path (M.2, memory shared as on the console):*
- The hardware and driver allow it: ReBAR VRAM mapped at a chosen address and aliased through a dma-buf, CPU and GPU agree (`tools/rebar-bench/placed-alias.c`). CPU writes to VRAM are fast, **CPU reads are 250× slower** (IBS census: the title's code reads its GPU memory ~600,000 times a frame, mostly windows), so it must be by page: what the CPU reads stays in RAM.
- vkd3d-proton is patched for it (`~/src/vkd3d-proton-vr`), but **Wine hides `VK_EXT_map_memory_placed`** from Windows programs: a Wine patch is needed.
- **One copy per byte is the rule:** buffers in place under window copies break the picture (121 of 287, 47 MB of overlap): GPU writes into memory in place are not seen by write tracking, so any other copy goes stale. In place must cover whole window cells with their buffers.

*Next, in order:* (1) windows and buffers in place together in RAM (`VICTORY_INPLACE=2` on today's base): picture, frame, stability; (2) the Wine patch so in-place memory can be VRAM; (3) per-page placement for what the title's CPU reads; then frames in flight (3.1).

**Why in-place buffers break the picture: window copies over them miss the GPU's writes**
- `VICTORY_INPLACE=3` with the new census (`bench/trials-…-inplace3-windows`): **287 title buffers in place, 121 of them under window copies, 47 MB of overlap**. A shader's write into a buffer in place lands in title memory directly, and Wine's write watch does not see writes the GPU makes; the window copy of the same bytes keeps its old bytes, and draws that read through the window see stale data. The user saw the same broken picture as before ("se ve igual"). This run also lost the device (as `INPLACE=2` did on 25 Nov): not yet understood.
- **For M.2 this is the same rule as M.1: one copy per byte.** Memory shared in place (RAM now, VRAM later) works only where nothing else holds a copy of it. Windows (4 MiB cells of raw-address memory) are the other copies. So a buffer goes in place only with its whole window cell, or windows must answer from the in-place memory, or in-place memory is refreshed into any window over it every submission. `VICTORY_ONE_COPY=2` went the other way (small buffers take their window's copy), which is why it held the picture.

**M.2 step 2 under Wine: vkd3d patched and built; Wine hides the placed-map extension; in-place buffers in RAM break the picture**
- **vkd3d-proton patch** (`~/src/vkd3d-proton-vr`, branch `victory-vram`: GE-Proton11-14's commit `cae8353e` plus GE's four patches plus ours): `VKD3D_VRAM_AT_ADDRESS=1` turns a heap opened on host memory (`OpenExistingHeapFromAddress1`) into ReBAR VRAM mapped at the same address (`vkMapMemory2` placed), its bytes copied first. Built for Wine with mingw-w64 (installed by the user). The layer side: `VICTORY_INPLACE=3`, title buffers only, no two heaps over one 64 KiB page, heaps kept for the run. The DLLs and an exe copy sit in `~/src/victory-royale-wt/vram/release/` (`VP_EXE`).
- First runs reported "lost": **not the GPU**. `bench.sh` finds the game by a path holding `release` or `diag`; the copy's folder did not, so it decided the game had not started and killed it while loading. Fixed by the folder name.
- **The VRAM path never ran:** no "are VRAM now" line. **Wine does not expose `VK_EXT_map_memory_placed` to Windows programs** (`dlls/winevulkan/make_vulkan`, `UNEXPOSED_EXTENSIONS`: it uses it only for WoW64 mappings, `win32u/vulkan.c`). For a 64-bit client `win32u_vkMapMemory2KHR` would pass a placed map through, but the extension is never enabled for it. Next: a Wine patch (expose it, or honour a client's placed address), built from GE's source as our wineserver is.
- **So both runs were `VICTORY_INPLACE=3` with buffers in place in system memory** (279-286 title buffers, 123-134 MB): **362-363 ms a frame against ~400, GPU 156-158 ms against 187**. But the picture breaks: the user, live, "media casa no cargaba, se veía muy raro" (`bench/user-play-20261126/` has no copy; screenshot in the session): the house dark and untextured, the dock and the water cut and bent, debris over the water. The first of the two runs looked right to the user while moving; the second did not. In-place buffers (title memory read by the GPU as it is) still miss something, as `INPLACE=2` did on 25 Nov ([memory.md](memory.md) §2). Off; to understand before M.2 goes on, since VRAM at the address has the same semantics.

**M.2 step 1, the Linux side works: VRAM at a chosen address, and aliased through a dma-buf**
- `tools/rebar-bench/placed-alias.c` (native Vulkan, RADV, 7900 XT): `memoryMapPlaced 1`, `memoryUnmapReserve 1`, alignment 4 KiB. A 64 MiB ReBAR allocation (type 3, VRAM, host-visible, coherent; exported as a dma-buf) **mapped by `vkMapMemory2` at the address asked for** (`0x7e0000000000`, a range reserved first, as Wine reserves the title's), **mapped again through its dma-buf at a second address**, and **one 64 KiB page of it at a third address from a 5 MiB offset** (a scattered alias, as the page pool does). CPU writes through any of the three read back through the others, and a GPU copy reads them all. Everything the console model needs from the driver is there.
- Next: the vkd3d-proton side (a heap whose memory is such an allocation, placed resources in it), built for Wine. It needs mingw-w64 (the user is installing it).

**Milestone (the user, watching the prologue gate):** "es la primera vez que veo las cinemáticas sin estar totalmente bugeadas, no es perfecto pero todo carga a tiempo más o menos". The first build where the intro cutscenes play nearly whole and load in time: the pipeline library that now exists (`c436241`: pipelines made before they are needed), with the indirect clamps out of the draws' way (I.1, I.2).

**Gates passed: `VICTORY_INDIRECT_BATCH` and `VICTORY_INDIRECT_WINDOW` on by default**
- Jason (`bench/trials-20261126-075607-gate-batch-jason` plus `…-batch-window`): **world 4 of 4, 395-408 ms**. A fifth run stayed on the start screen for 6 minutes (30 fps, 3 draws a frame, **no indirect draw at all**, 6 DirectStorage "request ran before its page array was filled" warnings): it never reached anything the switches touch; noted as a start-screen hang to watch.
- Prologue (`…-gate-batch-prologue`): **world 3 of 3, 351-371 ms** (windows invalid as always: the intro cutscene).
- Both switches are now on by default (`=0` turns each off).

**I.2 step 1: batch plus windows, inline clamps ~9,700 → ~830, GPU 187 ms, frame 407 ms (2.45 fps), picture held**
- The batch now takes CPU-written records first, and `VICTORY_INDIRECT_WINDOW=1` takes what is left (records the list's shaders write: one clamp per window of up to 64). `bench/trials-20261126-075253-batch-window` (same scene, ~11,900 draws): **frame 407-408 ms, GPU 187 ms, clamps left inline 805-864 (6.5-6.8 ms)**; 4,900-5,400 records a frame clamped in preludes. Picture right (`shot-1.png`: railings, umbrella, Jason on the deck).
- The day's GPU line: 272 ms (start) → 200 (I.1) → 187 (I.1 + windows). Frame 501 → 430 → 407 ms.
- Next: the gates (5 Jason runs, 3 prologue runs) before both become the default.

**2.1 census of the CPU's reads of GPU memory (AMD IBS): all-in-VRAM is not viable; a per-page hybrid is**
- AMD IBS op sampling system-wide (`perf record -a -e ibs_op/cnt_ctl=1/ -c 200000 -d`; needs `kernel.perf_event_paranoid=-1`, set by the user with `pkexec` for this; reverts at reboot) during the open-world window (`bench/20261126-074822-cpu-reads`, 409 ms, 23 frames in 10 s), with the GPU memory map written by the layer (`VICTORY_REGIONS_FILE`: 1,096 MB of buffers, 3,590 MB of windows, 67 MB of textures the title fills). Classified by instruction: in the title image (`victory-image-b99f876b…`, mapped at `0x6ffff1be0000`, 176 MB) or not (our layer, Wine, other DLLs). Script: `scripts/cpu-reads.py` (the column order perf prints is pid, addr, data_src, ip).
- **Title code's loads of GPU memory: 70 samples (0.37 % of its loads), ~600,000 a frame**: 63 in windows, 7 in buffers, on 54 sampled pages (none hot: 2-5 samples each). Through ReBAR each would be an uncached read (~0.5-1 µs, `tools/rebar-bench`): **300-500 ms a frame**. Title stores to GPU memory: ~900,000 a frame (write-combined in VRAM, fine).
- Our side's loads of GPU memory: 1,112 samples, ~9.6 M a frame (refreshes, snapshots, compares): what the direct path removes.
- **Decision:** the pure console model (all GPU memory in VRAM) would trade our 200 ms of copies for 300-500 ms of title stalls. The way is **piece M by page**: pages the title's CPU reads stay in cached RAM, the rest goes to VRAM behind ReBAR (written by the CPU, read by the GPU at full speed, no copies). The census by page (`VICTORY_REGIONS_FILE` + IBS) is the tool that sorts them; the reads are spread thin (54 pages seen in 10 s), so it needs a longer sample, or a per-page fault census, to find the full set.

**Phase 2, the direct path: title GPU memory in VRAM through ReBAR (the console model). The platform allows it; the CPU's reads decide it**
- The user asked why the GPU cannot read all of title memory, and for the most direct path. Two obstacles, apart: (1) technical, the page pool (72 % of the bytes) is sections and views, which vkd3d's `OpenExistingHeapFromAddress` (a host-pointer import on Linux) does not see; (2) physical, system memory over PCIe is ~25 GB/s for the GPU against VRAM's hundreds.
- **The direct path:** the title's GPU memory in **VRAM**, mapped for the CPU at the title's own addresses through ReBAR. The GPU reads at full speed; there are no copies, refreshes or copy-back. **This machine has the pieces** (`vulkaninfo`): RADV exposes `VK_EXT_map_memory_placed` (VRAM mapped at a chosen address, made for Wine), `VK_EXT_external_memory_dma_buf` and `VK_KHR_external_memory_fd` (one VRAM object mapped several times: the page pool's aliases), and a memory type that is VRAM, host-visible and coherent on the 20 GB heap (ReBAR). vkd3d-proton would need a heap made from such memory (a patch of ours).
- **Microbenchmark 2.1** (`tools/rebar-bench/rebar-bench.c`, native Vulkan, 256 MB):

| Memory | CPU write (memset / 32 B streaming) | CPU read | Scattered 64 B read-modify-write | GPU copy out of it |
|---|---|---|---|---|
| VRAM through ReBAR (coherent) | 11.7 / 22.6 GB/s | **0.082 GB/s** | 0.94 M/s | **262 GB/s** |
| system RAM, coherent uncached | 15.1 / 30.5 GB/s | 0.64 GB/s | 5.6 M/s | 28.4 GB/s |
| system RAM, cached | 13.3 / 30.6 GB/s | 21.2 GB/s | 87 M/s | 28.6 GB/s |

- **What it decides:** the GPU reads VRAM 9× faster than RAM, and the CPU writes VRAM fast enough, but **reads it 250× slower than cached RAM** (1 MB of CPU reads a frame = 12 ms). The direct path holds only if the title's CPU seldom reads its GPU memory, including the data it writes itself and reads again (command buffers, structures updated in place), not only what the GPU writes (that census said ~1 page a frame).
- **Next measurement:** the title CPU's reads of its GPU memory by page and per frame (a sampled census; the earlier guard census faulted on a page the title reads, so this needs a safer form).

**I.1 `VICTORY_INDIRECT_BATCH=1`: GPU 272 → 200 ms (−26 %), frame 501 → 422-441 ms (−14 %), picture held**
- Every indirect record was clamped right before its draw into **the list's one scratch buffer** (`indirect_scratch`: every clamp writes offset 0), so two global UAV barriers per draw were needed by construction, and no two draws ever overlapped.
- **I.1:** a single record that no shader of the list wrote before it gets a slot of its own in a per-list buffer (`HostList::batch_slot`). Its clamp is left for the list's **prelude** (`indirect::record_batch`: one wait, all the clamps back to back, one wait and the move to `INDIRECT_ARGUMENT`), and the draw reads its slot with no pass of ours.
- First run (`…-indirect-batch`): only ~270 records a frame batched. Why (`…-batch-why`): 344 split dispatches, **~5,000 in card copies** (excluded out of caution, but they are synced by their own lists before the submission's: `card_before`), **~4,000 written by the list's own shaders**.
- With card copies batched (`bench/trials-20261126-073515-batch-cards`, same scene, ~11,800 draws): **4,800 records a frame clamped in preludes; GPU 199-201 ms against 269-273; frame 422-441 ms against 501-515**; clamps left inline 4,500 (22 ms). The user: "se ve bien", only Jason's head black (pre-existing).
- **Next, I.2:** the ~3,800 records the list's own shaders write (the title's GPU culling): clamp each written range once, after its writer, into slots of their own, and let the draws read them until the next writer (Neptune VI's `CLAMP_AFTER_WRITER`).

**`VICTORY_INDIRECT_WINDOW=1`: −5 % with the picture held; Series S: −10 % GPU, black picture. The GPU is bound by draws, not pixels**
- `VICTORY_INDIRECT_WINDOW=1` with the census (`bench/trials-…-indirect-window`): run 1 (same scene, 11,840 draws): **477 ms against 501-502, GPU 261 against 272-273, clamps 5,715 (21.8 ms) against ~9,750 (37 ms)**. Run 2 opened from another camera, closer to Jason (the user saw it; 5,917 draws, 392 ms): not comparable. Picture held ("se ve bien"). A gain but small: ~5,700 clamps a frame still serialise the draws.
- **Series S** (`--console series-s`, new `BENCH_RUN_ARGS` in `bench.sh`; `bench/trials-…-series-s`): GPU 244 ms against ~270, frame 409 ms, copy-back 18.5 MB against ~45. **The picture is black**: the title's upscaler DLL is refused (`amd_fidelityfx_upscaler_xbox.dll`, host policy). Neptune VI hands those calls to AMD's PC DLL (`VICTORY_FSR_DLL`, `crates/victory-hle/src/modules/fidelityfx.rs`) and plays the Series S path at 9-11 fps. Here a much lower internal resolution saves only ~10 % of GPU, so the GPU is bound by per-draw cost.
- **Next (piece Q/D, "indirect without serialising"):** clamp a list's indirect records in batches, before the draws (a prologue pass for CPU-written records, one pass after each GPU writer for GPU-written ones: Neptune's `CLAMP_BATCH` and `CLAMP_AFTER_WRITER`), so the draws themselves run back to back.

**G.1 with barriers apart: ~9,700 indirect clamps a frame; barriers themselves are cheap; the clamps serialise every draw**
- `bench/trials-…-gpu-census-barriers`: **our indirect clamps 37.1-37.5 ms a frame over 9,736-9,806 clamps**; after the title's barriers 0.0 ms (3-4), buffer barriers made global 0 (0), waits for groups (made a global UAV barrier) 2.3 ms (~900), our state transitions 2.6-2.9 ms (~800-900). 218 ms timed.
- So the barriers we add are not the cost by themselves; the clamp pass around nearly every indirect draw is. Each clamp sits between two global UAV barriers: no two draws ever overlap, and each pays its full ramp-up and drain (mesh draws ~30 µs each). This is the same mechanism Neptune VI fixed (4.6 → 8.4 fps).
- **Our base already has the fix half-built: `VICTORY_INDIRECT_WINDOW=1`** (off): clamps a window of up to 64 records in one pass, the following draws read them with no pass of ours. Its doc comment measured 14-16 µs for a near-empty shadow draw, 4,126 records a frame (run 79). To measure now with the census.

**Not resolution: 1080p changes nothing. Neptune VI's numbers point at per-draw GPU overhead; our indirect clamps are the suspect**
- `VICTORY_DISPLAY_MODE=1920x1080` with the census (`bench/trials-…-display1080`): GPU 267-274 ms, 469-475 ms a frame, the same as at 3840×2160. Not pixel-bound: ~260 ms over 13,000-15,000 draws is ~20 µs per draw.
- **Neptune VI** (zm0kiemedia, the user found it; notes in [forks/neptune-vi.md](forks/neptune-vi.md)): open world ~6,500 draws, **90 ms a frame with the GPU 35 % busy (~30 ms)** on an RTX 5070 Ti. Their biggest win: indirect draws run on the title's own arguments, sanitised once after the GPU writer, 4.6 → 8.4 fps.
- **Ours clamps every indirect draw** (`indirect.rs` `record_clamp_as`): a compute dispatch between **two global UAV barriers**, ~1,000 a frame. That drains the GPU each time. G.1 now counts the clamps apart (`GPU_KEY_INDIRECT`).

**G.1: the GPU's time is spread over ~1,400 pipelines; the costliest are all mesh shaders. The title renders for 4K**
- `VICTORY_GPU_CENSUS=1` (`bench/trials-20261126-070908-gpu-census`, 481-509 ms, GPU 260 ms on the frame line): **209-212 ms timed** across 1,394-1,402 title pipelines a frame. **No single culprit**: the costliest is 12 ms (353 draws), the top 20 sum to ~115 ms.
- **The top 8 are all mesh pipelines** (shaders `adc5c689…` 22.9 KB, `0a2ea81c…` twice, `b7a81f5b…`, `903776ba…` 21.7 KB, `cf43bdbf…`, `8f690b0f…`, `db875f50…`): the console's geometry, translated. The interval between two marks counts the draw's whole work, pixels included.
- **The title renders for a 4K display:** the layer reports 3840×2160 (`dxgi.rs` `display_size`, *"display mode reported to the title width=3840 height=2160"*), the swap chain is 3840×2160, and the window is fitted to 1920×1080. A user suggested native 1080p without FSR; `VICTORY_DISPLAY_MODE=1920x1080` already exists (like a Series X on a 1080p set). Run with the census next.
- Jason's head black again, in a run with only the census (pre-existing, S.9).

**1.3 line closed: the copy-back is the merge, and stays until phase 2. The GPU's 265 ms is the next target (G.1)**
- `bench/trials-…-refresh-over-gpu`: **~860 refreshes a frame take ~125 MB of bytes the GPU wrote in the last submission back from title memory** (pages the title's CPU dirtied hold GPU-written bytes too). With the copy-back those bytes are the GPU's; without it they are stale and undo the GPU's work in our own copy. That is why every ceiling, even with one copy and no overlaps, lost the picture. Write tracking by page (Wine's write watch) cannot tell the title's bytes from the GPU's in a page; only the copy-back's byte merge does.
- So 1.3 step 2 as designed is not possible on this tracking. What removes the copy-back is phase 2 (memory the GPU reads in place, ReBAR) or byte-exact tracking. Its ceiling is −22 % in any case. `VICTORY_ONE_COPY=2` stays as a measured option (overlaps −82 %, picture held) for phase 2.
- **Reordering (the user asked for a recap: "¿no estamos un poco atascados?"):** the GPU takes ~265 ms a frame on a 7900 XT for a title the console runs at 30 fps (~33 ms). That is the biggest piece, and nothing tells us yet what it is spent on. The existing timestamps (`host.rs` `note`, `slowest`) are diagnostics-only and tied to DRED. **G.1:** a GPU census by pipeline in release builds (`VICTORY_GPU_CENSUS=1`): timestamps around draws and dispatches, summed per pipeline, the top 20 on the frame line. Then attack the top of that list.
- Jason's head black was seen again in a run with no M.1 switch (`…-unwatched`): pre-existing, intermittent (S.9).

**Selective ceiling: black too. The hypothesis now: page-granular refreshes undo the GPU's bytes without the copy-back**
- `VICTORY_ONE_COPY=2 VICTORY_BACK_CEILING=2` (only ranges another copy overlaps go back): the user, live: "veo esta prueba en negro". So neither the remaining overlaps nor textures are what needs the copy-back.
- **Hypothesis:** title memory has to stay the merged truth because CPU writes are tracked by page. The title writes its own data into the same pages shaders write (its command memory shares the big GPU heap: `shadow.rs` `copy_back_with`'s comment). Without the copy-back, a CPU write of a few bytes marks a page dirty. The next refresh then copies the whole page from title memory into our copy, with **stale bytes where the GPU wrote**, and undoes the GPU's results in our own copy. The copy-back's byte merge (GPU's byte where the copy changed, else the title's) is what prevents it. That would explain a black picture with one copy and no overlaps.
- **If it holds:** the copy-back cannot simply be skipped. The ways out are byte-exact CPU tracking (none on this platform) or no copies at all (phase 2: memory the GPU reads in place, ReBAR). 1.3 step 2 would then become "skip the copy-back only for pages no CPU write touches", and phase 2 rises in priority.
- **To measure:** at refresh, the pages that are both dirty by the CPU and written by the GPU in the last submission.

**Not textures: no written range lies over a title-filled texture**
- `bench/trials-20261126-065834-texture-overlap` (`VICTORY_ONE_COPY=2`): **0 copy-back ranges over the memory of textures the title fills** (their staging), every frame. Overlaps with other buffer copies: 46-47 ranges, ~100 MB (big buffers across cells).
- Left as suspects: (a) those ~46 overlaps; (b) title reads one frame or more later (the census lifted its guards at the next submission); (c) aliases (the same pages at another address). `VICTORY_BACK_CEILING=2` sends back only ranges another copy overlaps: if the picture holds, (a); if not, (b) or (c).

**Ceiling with one copy: still no picture, so buffers and windows are not the only consumers**
- `VICTORY_BACK_CEILING=1 VICTORY_ONE_COPY=2` (`bench/trials-…-ceiling-onecopy`): the user, live: "no se ve nada, la pantalla parpadea en blanco y negro", in both runs. Overlaps between our buffer copies are down 82 %, yet without the copy-back there is no picture at all.
- So other consumers read what the GPU wrote through title memory. The likeliest: **textures the title fills** (`crate::texture` staging: detiled from title memory by us at the next use). A pass that writes a buffer the title then samples as a texture reaches the texture only through the copy-back. Our own reads were outside the CPU census (lifted at the next submission). Next: count written ranges over title-filled textures' memory.

**M.1: `VICTORY_ONE_COPY=2` (small buffers take their cell's window): overlaps −82 %, the picture holds**
- `bench/trials-20261126-065216-one-copy3*`: **overlapped ranges 44 a frame** (240-249 without, 130 with mode 1), 66-68 copies, 97 MB of overlap: what is left is big buffers that span more than one 4 MiB cell. Frame 464-507 ms (unchanged while the copy-back runs).
- The user watched each run: "se ve muy bien", black only while things stream in; in one run Jason's head was black, in the next all was right. Jason's head black was also seen in two earlier runs **without** the switch (census runs, `…-063055-window-use`, `…-063417-lookup-miss`): an intermittent problem that predates M.1 (S.9 family), not caused by it.
- Next: the ceiling with one copy (`VICTORY_BACK_CEILING=1 VICTORY_ONE_COPY=2`): if the picture mostly holds without the copy-back now, the path to the −22 % is open.

**M.1: `VICTORY_ONE_COPY=1` decided per buffer holds the picture and halves the overlaps**
- Why the per-ask version broke the picture: descriptor slots ask for the whole buffer (`descriptor::host::buffer_copy`, `view_extent`), which no window holds, and got the buffer copy; smaller asks got the window. One buffer was split between two copies in the same frame.
- Per buffer (`bench/trials-20261126-064703-one-copy2*`): a buffer with no copy yet that a window holds whole takes the window as its copy for every ask. **The picture holds** (the user, live: "Jason se ve bien y la mayoría de cosas también", black only while streaming in). **Overlapped ranges 240-249 → 130 a frame**; all that is left is a buffer copy over a window (buffers whose copy came before the window). Frame 476-484 ms: unchanged, as expected while half the overlaps remain and the copy-back still runs.
- Next: `VICTORY_ONE_COPY=2`, a buffer that lies in one window cell takes that cell's window, made if needed (not the card copies).

**M.1 step 2, `VICTORY_ONE_COPY=1`: the picture breaks; off**
- `bench/trials-20261126-063805-one-copy-overlap` and `…-063931-one-copy`: 15,000-17,000 buffer asks a frame took a window's copy. Overlapped ranges 240-249 → 137-138, but **113 are now a written window over a buffer copy** made before the window (still used by other asks). Frame 478-503 ms (no gain).
- **The picture breaks** (`shot-1.png` against `…-lookup-miss/shot-1.png`): the deck railings go dark, the umbrella loses its yellow, Jason's head is black. The user saw it live ("se veía su cabeza en negro y también algunas partes de la casa"). A buffer ask answered by a window gets the window's bytes; something about buffer copies (their own refresh by resource, card copies, slots made from the buffer copy) is not carried by a window. **Off; not a base for M.1.**
- So the two names for the same memory cannot be joined by handing out the window. The next design for M.1 must make the window and the buffer copy one object from the start (a buffer copy that is a view into the window's host buffer, or windows that defer to buffers registered later). Back to design before more code.
- `scripts/trials.sh` now captures the game window only (`hyprctl` geometry), not the whole output: a capture of the output showed another workspace once.

**M.1: the overlaps come from two names for the same memory**
- `bench/trials-20261126-063417-lookup-miss`: **~87,000 asks a frame take a window, and every one of them because no title buffer held the address then** (0 missed by the 64-start search, 0 running past a buffer's end). The title reaches the same memory both through buffer objects and by raw address (descriptors, root views), and each name got its own copy. (The census's scan per ask is O(buffers): that run's second window was 747 ms; it is for counting only.)
- **`VICTORY_ONE_COPY=1` (`M.1 step 2`):** a buffer ask that a window holds whole takes the window's copy, so both names share one copy. A buffer copy made before any window, or an ask bigger than a window, still has its own. To measure: overlaps (`VICTORY_BACK_OVERLAP`), frame, picture.

**M.1: the windows under buffers are in use**
- `bench/trials-20261126-063055-window-use`: of 272-301 windows met under written buffer copies a frame, **250-290 were used in the last two frames, ~100 were asked for in the very bytes the buffer covers**. They are live copies the GPU reads, not leftovers: retiring them alone would only make them again.
- Next measurement: why an ask inside a buffer takes a window (`lookup` searches only the 64 buffers starting below the address; or the ask runs past the buffer's end; or no buffer holds it).

**M.1 step 1: every overlap is a buffer copy over a 4 MiB window**
- `bench/trials-20261126-061009-overlap-kinds` (482-483 ms): **240-249 of ~470 written ranges a frame overlap 244-253 other copies (58-62 MB), and all of them are a written buffer copy over a window** (`LOOSE`, made for asks outside buffers). No buffer under a buffer, no window under a window, no written window over a buffer.
- So M.1 is case 2 of the design alone: windows of title memory that later buffers cover. Step 2 becomes "an ask inside a buffer never meets a window, and a window under a buffer is retired", not the outermost-buffer lookup.
- (The wait loop of that run matched its own command line with `pgrep -f`; noted for scripts: match the process name, not `-f` on a pattern the waiter contains.)

**1.3: more than half of the ranges written back have another host copy over them; A.1: detiling is not the texture cost**
- `VICTORY_BACK_OVERLAP=1` (`bench/trials-20261126-060513-back-overlap`, 487-488 ms): **per frame, ~465 written ranges declared (1.07-1.16 GB of views), 253-263 of them overlapped by ~267 other host copies (buffer copies or windows), 66-85 MB of overlap**. Aliases (other addresses of the same pages) are not counted, so this is a lower bound. It confirms the ceiling's reading: what one copy's shaders write reaches the others through title memory.
- **So 1.3 becomes a piece-M question first:** one host copy per title range (nested buffers and windows resolved to the outermost copy, with views at offsets), or GPU→GPU copies between overlapping copies at the end of a submission. Either removes the hub; then the CPU side is the census's ~1 page a frame, fetched on demand.
- **A.1, detiling:** `textures detiled per frame: 9-139 textures, 0.6-6.8 MB, 0.0-0.4 ms` on all threads. Detiling is not what `heap_textures` spends its 6-115 ms on: that is making host textures, view mirrors and the mapped-page queries per slot. A.2's first target is then texture creation (the pool already pre-maps memory) and the per-slot queries, not a copy queue.

**1.3 ceiling: −22 % frame without copy-back, but the picture goes black; title memory is the hub between host copies**
- `VICTORY_BACK_CEILING=1` (`bench/trials-20261126-055937-back-ceiling`, measurement only): **362-374 ms against 477-486** (windows marked invalid: the draw count swings). Frame line: preparing ~50 ms (119 before), copying back 9 ms / 8.5 MB (72 ms / 46 MB; what is left is the card copies), GPU ~228 ms (265); the card idle behind us **~60 ms a frame (173-228 before)**.
- **The picture is almost all black** (`shot-*.png`): only faint UI outlines. Yet the census saw the title's CPU read about one page a frame of what goes back.
- **What that means:** the copy-back is not only for the CPU. What one submission's shaders write into one host copy reaches later GPU reads through title memory: other host copies of the same memory (views, aliases, the 64 KiB page-pool mappings) are refreshed from it. Title memory is the hub of GPU→GPU coherence between our copies. The census measured CPU reads only, and is blind to this.
- **For 1.3 step 2:** an on-demand copy-back must keep that path: either GPU→GPU copies between our host copies of the same title range (on the card, no CPU), or one host copy per title range (piece M, phase 2). The CPU-read side alone (census) is a small part of the problem. The design doc gets this before any step-2 code.

**D.1: spreading the full rescan is no gain; the heap pass is not on the frame's critical path now**
- `VICTORY_HEAP_ROLL=1` (`bench/trials-20261126-055355-heap-roll`): 477-486 ms against 478-486 without (`…-d1-parts`). Each pass now looks at ~18,800 slots, ~130 ms a frame of the slot loop (against ~30 ms on average plus the 231 ms spike every 16 passes); p95 505-511 against 508-529. **Off.**
- The frame does not move with the pass's cost either way, so the heap pass is not what the frame waits for at present (the serial chain is). D.1 drops in priority; its texture side goes with A.
- Next: the ceiling of 1.3, `VICTORY_BACK_CEILING=1` (no snapshots, no copy-back; measurement only, the picture may break).

**D.1 split: one full rescan every 16 passes, and streaming textures on the recording thread**
- New frame line (`bench/trials-20261126-055025-d1-parts`, 478-486 ms): **heap pass per frame: 4-60 ms over ~3,800 changed slots; 231 ms when it looks at all 119,048 slots (every 16th pass); 2-5 ms queueing span refreshes (2.4 GB of spans, refreshed at submission); 6-115 ms in `heap_textures`**.
- The full rescan is a spike of ~230 ms every 16 frames. `VICTORY_HEAP_ROLL=1` spreads it, a sixteenth of the slots per pass (each slot still looked at every 16 passes). To measure.
- **`heap_textures` is piece A:** it makes, detiles and stages the textures shaders pick by index (streamed in), up to 192 MB or 250 ms a frame (512 MB / 1.5 s while catching up), on the thread that records the frame. This is the "black world for tens of seconds" path. A.2 (a copy queue fed by workers) is where it belongs.

**1.3 census, all of it: 40 MB a frame copied back, the title reads about one page of it**
- `VICTORY_BACK_CENSUS=2` (read-write view memory guarded too), `bench/trials-20261126-054413-census-views`: **10,450-10,800 pages a frame written back and guarded (39.5-41 MB), none skipped; the title touched 0-1 page a frame before the next submission** (always a first read; pages `0x15fd93000`, `0x15fdc3000`). Frame 487-488 ms, as without the census.
- Run 3 ended in an access violation in title code (`+0x4dbf69d`) **reading `0x15fdc3cd0`, in the very page the census had just seen the title read**. So the census caused it: a guard on the one page the title does read is not safe as it is (why the retried read faults is not known yet). Off by default; not to be run again until understood.
- **What it means for 1.3:** within one submission, the copy-back is almost entirely bytes nobody reads. Unknowns before step 2: (a) reads through another view of the same pages (aliases are not guarded: a lower bound); (b) reads later than the next submission (the census lifts its guards there); (c) the one or two pages the title does read, which the step-2 design must keep eager (they look like results the title polls). Next for 1.3: a census that keeps a page guarded until it is read or the GPU writes it again, without guards on the pages already known to be read.

**1.3 census, private memory: the title reads none of what is copied back there**
- First runs (`bench/trials-…-census`): all three ended with an unhandled `0x80000001` (guard page) in title code. `lift()` took the pages out of its set before taking their guards off, so a title thread that met a guard in between found no page and its fault went on to the title. Fixed (`b4cfe43`): pages leave the set only after their guards are off.
- Again (`bench/trials-20261126-053943-census2`): two runs in the world, 468-485 ms. **Per frame, 1,790-1,940 private pages written back and guarded (6.1-6.7 MB); the title touched 0-0.3 of them before the next submission** (one page, read). The third run ended in an access violation in title code during the load (`+0x4fd71ae`, reading a read-write page), with the census on; whether the census caused it is not known. The census stays off by default.
- **8,400-9,000 pages a frame are skipped** (~35 MB): not private read-write memory. Most of the copy-back (40-42 MB a frame here) goes to them. `VICTORY_BACK_CENSUS=2` now guards read-write view memory too (a lower bound: reads through another view of the same pages are not seen), and the skipped count is split by reason.
- **So far, for private memory, the hypothesis of the design holds:** what is copied back there is not read by the CPU before the GPU writes it again.

**P.1: with the library made, the world-entry freeze is gone (9.5 s → 0.63 s)**
- `bench/trials-20261126-053012-pipelib`, Jason save, two runs in a row after `c436241`:
  - run 1 (empty library): the frame the world appears in takes **9,487 ms** (1,378 pipelines made in those 5 s), then ~460-480 ms;
  - run 2: *"host pipeline library loaded bytes=265016"*, *"title pipelines to start early pipelines=1466"*; **993 pipelines made in the first 5 s of the process**, 482 more by 30 s, all during the loading screen. The world's first frame takes **629 ms**, and no frame is over 2 s.
- The loading screen lasted longer in run 2 (world at 69 s against 42 s), with no pipeline work in that gap (1 pipeline from 50 to 60 s): variation in the title's own load or the auto keys, not pipelines. More runs will tell.
- The library is small (265 KB): with vkd3d-proton's own disk cache, the application library keeps only metadata, and vkd3d's cache holds the compiled pipelines.
- Unchanged: **~300 draws and 800-860 indirect draws a frame still dropped for want of a pipeline**, from the very first world frame (P.4).

**S.7 guard: 6 of 6 prologue runs held, the guard never fired**
- `bench/trials-20261126-051802-s7-zeros` (prologue, pool and wait on): **world 6 of 6**, 354-376 ms (windows `invalid`: the intro cutscene). No run logged a root view bound to zeros. So the guard did no work here, and the cause is not confirmed: S.7 stands at 1 crash in 12 prologue runs with the wait. Watched; the guard stays (it costs nothing when unused) and its log line will name the address if it fires.

**P.1 begun: the pipeline library was never made; draws dropped for good**
- `pipeline/library.rs` asked vkd3d-proton for an empty library with an empty slice, whose pointer is not null. vkd3d refuses a pointer with a length of 0 (`if (!blob_length && blob) return E_INVALIDARG`, `libs/vkd3d/cache.c` `d3d12_pipeline_library_init`). So **every run went without a library** (*"no host pipeline library: pipelines are not kept"*), the early list of title pipelines stayed at 0, and each run made all its pipelines again: ~3,000 entering the open world. Fixed in `c436241` (a null blob); with it, the 0.5 s/frame budget and the early start (decision 0007) can work as designed. To measure: two runs in a row, the second should load from the library.
- New counters on the "host pipelines all made" line: ms in the host's make and load calls, the workers' total, title threads' wait, draws dropped after waiting.
- **In play, minutes in:** `dropped 342 without a host pipeline (3 being made) and 981 indirect` per frame (play log 04:13). Not pipelines still being made: pipelines that never come. Likely the two Xbox pipeline creation methods (`0x1c8`, `0x218`) that no fork implements (P.4). Mesa's disk cache is used (13 new entries over the whole session), so RADV compiles are not the load's cost.

**Playing it: the load is slow and black, and two pieces were missing (P, A)**
- The user played the Jason save (our build, pool and S.7 guard not yet built). Seen: the world comes in black (Jason's house included) with many textures missing for a long time, and ~3 fps. Screenshot in play (`bench/user-play-20261126/shot-now.png`): S.9 again, a dark interior, Jason as a black silhouette, white frost-like speckles on the floor and table, windows blown out.
- Play log (`royale-save/logs/20261126-050658-run.log`): ~3,000 pipelines first bound in the first 2-3 minutes of the world (1,795 in the first minute), **one frame of 9,953 ms** at 04:10:19, then 2.1-3.0 fps. The shader translation hits its disk cache; the pipelines are created on first bind, on the submitting thread.
- **Two pieces added to the target (README §4):** **P** pipelines (a journal of what each run creates, replayed on spare cores at the loading screen; a pipeline still missing built on a worker), modelled on KytyPS5's shader journal, pipeline journal and driver-cache saver; **A** asynchronous uploads (a copy queue of its own, fed by workers). Neither is in the 1.3 → 3.1 chain, so they do not wait for it; the steady ~2-3 fps is the serial chain, which only B, D.1 and Q fix.

**S.7 traced: a root view left at address 0; the 1.3 read census built**
- The only SQC fault on record (kernel log, 25 Nov 23:30:38; prologue run `royale/logs/20261125-232915-run.log`): *SQC (data) reading address 0x0*, `PERMISSION_FAULTS 0x3`. The SQC is the scalar cache: constant buffer loads. 100 ms before, the sweep had let go of 252 texture copies and a texture staging buffer.
- `draw.rs`: a root CBV or SRV whose title address does not resolve to a host copy (`root_view` returns `None`, or the title never set the root) was **skipped with `continue`**. vkd3d then leaves the root descriptor at 0, and the shader's constant loads read address 0: that fault exactly.
- **Fix (`9e3e8e5`):** such a root view is bound to 64 KiB of zeros in the list's scratch; the first 8 are logged with their address, and the frame line keeps the count (`root views bound to zeros so far`). If the count shows up in prologue runs, the next question is why the address had no host copy (a window the sweep let go?).
- **1.3 step 1 (`5395625`), `VICTORY_BACK_CENSUS=1`, off:** `census.rs`. `write_mapped` (all of its callers are copy-backs) notes pages; after the bottom-of-pipe writes they get `PAGE_GUARD` (private read-write pages only); at the next submission's start the guards left are lifted, before we read title memory. A guard fault from the title's code counts as touched (read or write first). Faults at fastcopy's copy instruction (it resumes the copy) or in our own image are counted apart, as the layer's own. Kernel reads of a guarded page would fail, so the census needs the fast reads (on by default) and is off with the relay.

**The open world is a serial chain; the order of the roadmap holds**
- The layer's card clock, open world outside: **the card has nothing of ours queued 212-284 ms a frame (busy 48-57 %), 173-228 ms of that while our submitting thread is working**; the thread then waits for the card ~265 ms. CPU and card never overlap. Overlapped, the frame would fall towards ~270 ms (phase 3, ~1.8×).
- Within our thread's work: **408-437 MB a frame snapshotted on the CPU** to find what shaders wrote in title buffers' copies (copy-back, piece B), plus 72 ms / 46 MB of copy-back.
- `VICTORY_CARD_BUFFERS_MIN_MB=1` (the card compares copies of 1 MiB or more): 491-494 ms against 491-504; prepare 119 → 107 ms but GPU 265 → 276 ms, and the CPU still snapshots 408 MB. No gain; stays off (`bench/trials-…-world-out-card1`).
- **Next:** 1.3, measured first: how much of what is copied back does the CPU read before the GPU writes it again ([design](design/p1.3-copyback-on-demand.md)). Lesson 1 (from `fx`) says the copy-back must shrink before frames in flight.

**S.4: the open-world baseline (outside the house), and where its time goes**
- **Baseline** (`BENCH_SETTLE=5 scripts/trials.sh -n 3 -l world-out -s 30`, all windows valid, none lost): **491-504 ms a frame (2.0 fps)**, prepare 116-124 ms, GPU 262-269 ms (card busy 53 % of it), copies 49-54 ms / 133 MB, copy-back 71-74 ms / 46 MB, ~13,100 draws. Recorded in [baseline.md](baseline.md).
- **Correction:** with the pool off (and the wait on), the same: 485-502 ms. The pool's earlier "~6 % faster" came from invalid windows of different scenes. Its value is stability without waits (frames in flight), not speed.
- **Unlike the prologue, the GPU phase dominates** (53 % of the frame). And a once-a-frame pass over the heap's buffer views (`draw.rs` `heap_buffers`, for shaders that pick buffers by index) takes **133 ms a frame**: ~110,000 views over 4,085 host copies spanning 2.4 GB; the full pass every 16th frame takes 389 ms, the others ~23 ms.
- **Profiles** (`bench.sh --perf`, new `scripts/perf-symbols.py`, which finds victory.exe's base by itself and symbolises it from the PDB): victory.exe 45 % of the process's samples, the title's own code 19 %. **Mutex lock/unlock/spin is ~14 % of all samples**, the same with the pool off, so it predates it. Branch records (`BENCH_PERF_G=lbr`, AMD LBR v2) place it near `shadow::find`, the descriptor slot paths and `Shadow::refresh`, but not exactly: under Wine an unlock enters ntdll, which fills the 16 records. Frame-pointer stacks (`BENCH_PERF_G=1` with a `force-frame-pointers` build) stop at Rust's std, built without them.
- `bench.sh --perf` leaves the game running at the end (seen 3 times); closed by hand with `victory-proton kill`. To fix.

**The open-world scene is two scenes: outside, then indoors**
- Draws in the world windows stepped down partway (16,000 → 2,400, 14,000 → 8,100, 14,000 → 6,100) with no input: the A taps stop at 50 s and the world comes at ~78 s.
- A screenshot at the window's end shows why: after ~45 s of the house from outside, the save **cuts indoors to Jason at the TV** (prompts "PHONE", "TV (X)"). The TV's video plays right. The interior shows **white speckles on the floor and blown-out areas**: a lighting fault for S.3 / phase 5, not stability.
- `bench.sh` takes `BENCH_SETTLE` (seconds after the world is detected, default 20). The open-world baseline measures the outside: `BENCH_SETTLE=5 scripts/trials.sh -n 3 -l world-out -s 30` ([bench.md](bench.md)). `trials.sh` now also takes a screenshot near the window's end.

**S.8 first step: the pool had stopped the sweep; fixed**
- The sweep lets a texture copy go only when `crate::host::references` says its title texture alone holds it (1). The pool's own reference made that 2 for every pool texture, so **nothing was ever let go**: that, not the title, is why the gate run reached 37,700 live textures and 18.9 GB of VRAM.
- Fix: the pool keeps a set of what it holds, and `references()` leaves its reference out. A copy the sweep lets go then reaches the pool's `collect` with no other holder, goes through `retire()`, and its block is reused.
- **Jason save, 2 runs of 120 s** (`bench/trials-…-s8-sweep`): both held, with blocks given back and reused (5-28 a frame). 25,800 live textures, 9.2 GB in use (against 37,700 and 13.3 GB), VRAM at most 17.8 GB. The heaps themselves are never released (11.5 GB made); trimming empty heaps is the rest of S.8.
- **Rule 7 holds:** the sweep's "unused for 5 s" now only chooses *which* copy to let go. Whether it is safe comes from exact holders (no list, no slot) and `retire()` (no running submission), and the block reaches the allocator only after that.

**Stability gate: 10 minutes in the open world with the new defaults (pool and slot wait), no loss; the picture more complete**
- Jason save, one run with a 600 s window (`bench/trials-20261126-002900-gate10`): held to the end. Frame 363 ms on average (2.77 fps), window marked `invalid` because the draws varied; not a speed figure yet.
- **The picture is more complete than with committed textures:** the house shows its painted boards, towels on the railing and wood detail, where the screenshots with committed textures and the wait showed it dark and flat. Textures that failed or went stale on fresh memory now hold. To be checked against S.3 (texture coherence).
- **VRAM:** 18.8-18.9 GB of 20.4 GB from the first minute on, GTT 4.5-5.9 GB (sampled every 5 s). The pool holds 37,700 textures, 13.3 GB in 14.6 GB of heaps. It did not grow during the static 10 minutes, but walking will: S.8 (bound texture memory) is the next task.

**The texture pool on by default; what holds 35,000 textures**
- 4 KiB small-resource alignment: 98 % of pool textures take it, yet use barely falls (12.3 GB for 34,700 textures, ~360 KB each on average). The bytes are real.
- **Live textures by references besides the pool's** (`texture pool:` frame line): none with 0, ~12,000-13,000 with 1 (their title texture), ~22,000 with 2 (also a descriptor slot, which keeps its last view alive by design), ~930 with more. Nothing is given back because the title never lets these textures go, not because the count is wrong. The committed path holds the same set.
- **New task (T/L): bound texture memory.** The live set grows as the world streams in (12-14 GB after a few minutes); a texture held only by its title texture and unread for a long time is the candidate, which is what the sweep does today by time. It needs an exact rule (rule 7), measured over a long run.
- `collect` took ~1 ms per call over 35,000 textures, at every submission; now at most every 250 ms.
- **Pool on by default** (`VICTORY_TEXTURE_POOL=0` turns it off; 4 GiB to start). The slot wait stays on as a safety net; it costs nothing measurable today, and goes once frames in flight need it gone.

**S.6 confirmed: textures in pre-mapped heaps remove the device loss at its root, with no wait, and faster**
- The first pool run (2 GiB, grown only when full) still lost 5 of 5: the pool filled, 16-60 textures a frame fell back to committed memory. A fault report placed the culprit in a 16 MB vkd3d block, not a pool heap.
- Fixed: the pool grows ahead of need (a new heap when ready free space falls under 1 GiB) and counts every texture it does not take. With 8 GiB to start: 0 committed, 0 not taken.
- **Jason save, no slot wait (`VICTORY_SLOT_WAIT=0`):** without the pool **8 of 8 lost** (`bench/trials-20261126-000514-control-nowait` and earlier); **with the pool 5 of 5 held** (`bench/trials-…-pool2-nowait`), at **406-453 ms a frame** (2.21-2.50 fps) against 469-489 ms with the wait and committed textures. Two of the five windows are valid (444, 453 ms); the others saw the scene change.
- **The cause is settled:** fresh memory mappings of textures made while work that can read them is in flight. The slot wait hides it by serialising; the pool removes it, which is what frames in flight (3.1) need.
- **Open before it becomes the default:** 32,000 textures live, 12.4 GB "in use", none ever given back. Small textures take a whole 64 KiB block each (95 % are under 1 MiB, many far smaller); D3D12's 4 KiB small-resource alignment would cut this. And why none is given back (held by descriptor slots that keep their last view, or a wrong count) has to be measured.

## 2026-11-25

**S.6 step 1 measured: the open world makes hundreds of host textures a frame, all over title memory; the pool built**
- New frame lines: host textures made per frame, by size and by the title texture's backing (`resource.rs` `TEXTURES_MADE`, `MADE_KINDS`).
- Jason save, settled in the world (`bench/trials-…-texsize`, `…-texkinds`): **60-635 host textures made a frame, 15-200 MB a frame**, 0.3-3.5 ms creating. About 95 % are under 1 MiB, the largest 10 MB. **All of them are textures the title makes "at a title address"** (over its own memory: streaming); none placed in a title heap, none committed. 10-50 % repeat the address and description of one made before. The sweep let none go in that window, so this is the title's own churn, not ours.
- Each of those got fresh committed memory: each was a chance for the S.1 race. A console texture is a view over memory; ours was an allocation.
- **`pool.rs` (`VICTORY_TEXTURE_POOL=1`, off):** host textures up to 64 MiB are placed in 256 MiB heaps made up front (2 GiB by default). A heap is used only after a submission sent after its creation has finished. First fit with alignment from `GetResourceAllocationInfo`. The pool holds one reference to each texture; at each submission boundary, a texture only the pool holds goes through `retire()` and its block comes back afterwards (design rule 7, exact lifetimes). The allocator's tests pass (run natively; the Windows test binary needs vkd3d's DLLs in its prefix).

**`VICTORY_SLOT_WAIT` on by default; the prologue had the bug too; S.6 designed**
- **Open world:** 5 of 5 held with the wait (`bench/trials-20261125-232200-slot-wait5` makes the fifth). Fast writes with the wait: 3 of 3 held (they lost the device before), but no gain in the open world (2.03-2.15 fps against 2.10), so they stay off.
- **Prologue, 3 runs each:** without the wait 1 of 3 lost, with a TCP fault on a texture (the S.1 kind); with the wait 0 of 3. Frame 358-364 ms with the wait against 359-372 without: no measured cost. Both sets of windows are marked `invalid` because the intro cutscene pulls the draws down during the window, so this is a rough comparison.
- **On by default** from here (`VICTORY_SLOT_WAIT=0` turns it off), per the rule that a switch becomes the default when it wins.
- **New, rare (S.7):** one prologue run with the wait lost the device on a different fault: **SQC (scalar data) reading address 0**, right after the sweep let ~5,000 texture copies go. 1 of 6 prologue runs with the wait. Not the S.1 mechanism (a null pointer, not a fresh texture). Watched; next step when it repeats is `VKD3D_CONFIG=fault` on it.
- **S.6 designed** ([design/s6-premapped-textures.md](design/s6-premapped-textures.md)): the wait does not survive frames in flight, and staging writes at submission boundaries is not enough either. The race is between a fresh memory mapping and GPU work sent before it. Host textures placed in heaps mapped ahead of time remove it in any submission model, and are the first piece of memory the layer owns (piece M).

**S.1 found: host descriptor slots written while the GPU runs (`VICTORY_SLOT_WAIT=1` holds the open world, 4 of 4)**
- **`VICTORY_LEAK_ALL=1`** (new: every resource and host descriptor heap the layer makes is held forever, `crate::host::pin` at all 18 creation sites): still **4 of 4 lost**. No title heap with a host heap was released in any run. So the invalid memory is not a use-after-free of anything we make (`bench/trials-20261125-225445-leak-all`).
- **VRAM is not short:** sampled every 0.5 s through a losing run, at most 8,252 of 20,464 MiB of VRAM and 4,344 MiB of GTT.
- **`VKD3D_CONFIG=fault` names the faulting resource** (vkd3d's address-binding tracker with `VK_EXT_device_fault`). In both reports:
  - the address is **inside a texture made about 2 s before** the loss was reported (512×512 BC1 with 8 levels at offset 0; 512×512×32 layers with 10 levels at offset 0x1e2000);
  - vkd3d: *"Fault address is currently mapped. This should not normally page fault."*
  - vkd3d reports a loss about 2 s after it happens, so **the GPU faults right after the texture is made**.
- **Mechanism:** host slots are made lazily (`descriptor/host.rs` `make()`), from the title's threads, into the shader-visible heap, **while a submission may be running**. Counted: **15,600-58,300 slots a frame written while a submission ran** in the open world. If the running work reads such a slot, it sees a resource made after it was submitted, whose page-table update only the *next* submission waits for. That fits every known fact: timing-dependent, hidden by anything that serialises the GPU, the open world (texture streaming) far more than the prologue, and a mapped address that faults.
- **`VICTORY_SLOT_WAIT=1`** (new): `make()` waits until no submission runs (at most 2 s). **Jason save: 4 of 4 reached and held the world** (13,000-13,500 draws a frame, 2.05-2.13 fps, valid windows, the picture right: house, light, water), against 1 of 2 with the counter only and 10 of 12 lost earlier tonight. Only 82-97 writes a frame had to wait; none gave up (`bench/trials-20261125-231520-slot-wait`).
- **Architectural reading (piece D, guarantee "a view never reaches past its resource"):** a shader-visible slot is GPU state. It may only change at a point where no work that can read it is in flight, as on the console, where the title's own fences guard it. The wait is the minimal form. Frames in flight (phase 3) need the proper form: slot writes staged and applied at submission boundaries.

**S.5 measured: mirroring byte-copied descriptors does not stop the open-world loss**
- Ported the descriptor part of fxceless `a26859f` (`VICTORY_HEAP_SYNC=1`, off by default; branch `s5-heap-sync`). New `scripts/trials.sh` runs one configuration N times unattended and classifies each run.
- **The title does copy descriptors as bytes, a lot:** per frame in the open world, 260-320 changed heap pages, 530,000-650,000 slots looked at, **~225,000 records new and mirrored**.
- **Loss rate, Jason save, same binary:** sync on **5 of 5 lost**; sync off 4 of 5 lost. The one survivor drew 2,935 draws a frame with an `invalid` window, so it was likely not in the world yet. In practice **every run loses the device within about a minute once the world draws 12,000-25,000 draws a frame**: a fast, reliable reproducer (`bench/trials-20261125-223829-hs-on`, `…-224417-hs-off`).
- **Kernel fault details** (`journalctl -k`): `GCVM_L2_PROTECTION_FAULT_STATUS 0x00701031`, client TCP, a read, `PERMISSION_FAULTS=0x3`, `MAPPING_ERROR=0`. The page has a page-table entry marked invalid: memory freed or never bound, not a wild address.
- vkd3d breadcrumbs (non-sync) print nothing on loss with GE-Proton's vkd3d-proton: the config is parsed but no report is made. A breadcrumb report needs our own vkd3d-proton build.
- **Reasoning for the next step:** the bug is timing-dependent (anything that slows or serialises the GPU hides it), the page is invalid, and it lies in no live copy of ours. That fits **memory released while the GPU still reads it** better than a view out of range, which would fault whatever the timing. `VICTORY_LEAK_RETIRED` covered only `retire()`.
- **Found in the code:** when the title releases a shader-visible heap, its `HostHeap` (the `ID3D12DescriptorHeap` and every resource its slots hold in `kept`) is dropped at once, with no fence waited for. `HostHeap` has no `Drop` and does not go through `retire()`.
- Next: `VICTORY_LEAK_ALL=1` (every resource and host descriptor heap the layer makes is held forever) and a log line when a title heap with a host heap is released.

**Decision: back to our base; the roadmap rebuilt around pieces that fit this machine**
- **Base:** `p1.8-fast-reads` (joshuu `003beef` plus phases 0, 1.7 and 1.8), not `fx`. On this machine it is twice as fast (317 against 610 ms per frame), and the stability bug is in both bases. `main` is advanced to it. `fx` stays as a reference for track U.
- Carried over from `fx`: the `bench.sh` check that marks a window `invalid` when its draws fall below half their top, and the journal entries.
- **README section 4 now opens with the target architecture:** seven pieces (M memory placement, W write tracking, B copy-back, L lifetimes, T textures, Q submission and fences, D descriptors and views), each with its guarantee, today's state, the target on this machine and its phases. Lessons from `fx` fix the order: **B (1.3) comes before Q (3.1)**, because overlapping while the copy-back is large puts it on the title's critical path.
- New: task S.5 (evaluate fxceless's descriptor mirroring `a26859f` as a cause of the wrong reads), track U (upstream intake, one measured change at a time), track L (soft-dirty / `userfaultfd` prototype, moved out of phase 4), track G (gates).
- **This machine:** ReBAR is on (BAR0 32 GiB over 20 GB of VRAM), so CPU-visible VRAM is not limited to 256 MB. Phase 2.1 also measures CPU write bandwidth into it.
- **Agents:** [AGENTS.md](../AGENTS.md) sets out the rules and roles of the pi subagents (scout, architect, worker, reviewer, analyst, upstream). They write code in worktrees and read results; game runs stay with the maintainer.

**fx on Linux: why it is slower, and a measurement trap**
- **The bisect was wrong in its premise.** The title stalls (~280-460 ms a frame) at every fxceless commit tested, back to `3bff182`. What made the old base fast is not in fxceless's line at all; it came from joshuu/myokex commits fxceless did not take. joshuu's remapping commits `948e9d3` and `476a020`, ported, change nothing.
- **fxceless's new defaults** (`c96b4f2`):
  - relay on (copy-back by its own thread, stage 3 sending ahead);
  - wide copy-back;
  - card copies from 1 MiB to 1,024 MB;
  - 256 lists in the air.
- **With the relay, the title waits for the relay thread's landing.** Per frame it waits 55-72 ms for the card and 100-122 ms for fetches of card copies' pieces, 158-176 ms landing in all. The card is idle while the title waits.
- With the old defaults (`RELAY=0`, `WIDE_BACK=0`, card 16-512 MB, 128 lists): **7.5 s a frame** (copy-back 6.3 s). fxceless rebuilt the copy-back around the relay, so it cannot simply be turned off.
- `VICTORY_QUEUE2=1`: 623 ms. `VICTORY_ASYNC_SUBMIT=1`: 660 ms. No gain.
- **Trap:** a `CARD_BUFFERS_MIN_MB=16` run read "140 ms" because the game went to a 199-draw screen (pause?) during the window. In the world it was 1.5 fps, the same. `bench.sh` now marks a window `invalid` when draws fall below half their top.
- `waitstat` on fx: the same waits as the old base (one thread ~700 ms/s at `title+0x4e7f358`). The title's `mwaitx` wait is native on this Ryzen and not seen.

**Branch `fx`: the base moved to fxceless/main (`d510ee4`)**
- Carried over by hand (the old commits do not apply):
  - `VICTORY_SKIP_PIPELINES` and `VICTORY_KEYS` in release builds; fxceless still has both diagnostics-only;
  - `crate::fastcopy` (reads only; writes off);
  - docs and `bench.sh`, which now reads the log's frame-rate lines when there is no frametime CSV (fxceless has none).
- Not carried yet: `medir` (waitstat, CSV columns), `F1_OWN` / `F1_CARD_MERGE` (fxceless has `VICTORY_BACK_OWN_VIEW` for the same purpose), and the diagnostic switches of the device-loss hunt.
- fxceless has no `victory warm`. The first run after a translator change is the warm-up.
- **Scene A on Linux, fxceless defaults: 610 ms per frame (1.7 fps)**, against 317 ms on our old base. The layer is faster (prepare 62 ms, GPU 52-66 ms), copy-back slower (145 ms). But the card is idle ~420 ms a frame **"while the title had not submitted"**: the title itself stalls, where on the old base it waited ~2 ms.
- fxceless's 10 fps switch set (`JOIN_PARTS`, `BACK_OWN_VIEW`, `INDIRECT_GATHER`, `CARD_REHASH_RUNS`) changes nothing here: 603 ms, title idle ~427 ms.
- fxceless also reports washed-out frames in ~6 of 21 runs with no switch (our white picture) and an open-world device loss "with the card out of room for new textures". Both are base bugs, not Linux ones.
- **Bisect under way** (`2119b0e`, good per the old base, .. `fxceless/main`) on how long the title waits per frame. It runs in a worktree, unattended (`scratchpad/bisect/step.sh`).

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
