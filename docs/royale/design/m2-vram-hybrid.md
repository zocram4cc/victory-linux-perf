# M.2: title GPU memory in VRAM behind ReBAR, page by page

Piece **M** (memory placement), phase 2. Written 26 Nov 2026 from that morning's measurements ([journal](../journal.md)). It replaces "copies of title memory" with **one memory both sides use**, as on the console, where the data says it pays.

## Why

- Our thread spends ~200 ms a frame keeping host copies and title memory equal: refreshes (~125 MB), snapshots (~420 MB), copy-back (~45 MB). The card waits for it ([baseline](../baseline.md), 26 Nov).
- The copy-back cannot go while copies exist. Page-granular write tracking makes title memory the byte merge ([journal](../journal.md), "1.3 line closed").
- Removing copies gives ×2 at most on its own ([memory.md](../memory.md) §5), and it is what makes frames in flight (3.1) worth doing.

## What the platform gives (measured)

| Fact | Source |
|---|---|
| RADV on the 7900 XT exposes `VK_EXT_map_memory_placed`, `VK_EXT_external_memory_dma_buf`, `VK_KHR_external_memory_fd`, `VK_EXT_external_memory_host` | `vulkaninfo`, 26 Nov |
| A memory type that is VRAM, host-visible and coherent covers the 20 GB heap (ReBAR) | `vulkaninfo` |
| VRAM through ReBAR: CPU writes 11.7 GB/s (memset), 22.6 GB/s (streaming); **CPU reads 0.08 GB/s**; scattered 64 B read-modify-write 0.94 M/s; GPU copies out at 262 GB/s | `tools/rebar-bench` |
| Cached system RAM: CPU reads 21 GB/s; GPU reads 28.6 GB/s | `tools/rebar-bench` |
| The title's own code loads from GPU memory ~600,000 times a frame (63 of 70 samples in windows, 7 in buffers, spread over many pages) | IBS census, `scripts/cpu-reads.py` |

So the GPU wants VRAM (9× faster), and the CPU can write it, but every read the title makes there costs ~0.5-1 µs. **All-in-VRAM would add 300-500 ms of title stalls a frame.**

## Design

**Placement by page, decided by what the title's CPU does:**

| Page | Where | Why |
|---|---|---|
| Written by the CPU, read by the GPU, not read by the CPU | **VRAM behind ReBAR, at the title's address** | no copies, the GPU at full speed, CPU writes write-combined |
| Read by the CPU (any) | **cached RAM** | CPU reads stay fast. For the GPU: an in-place heap over that RAM (private memory: `OpenExistingHeapFromAddress1`, measured to work, [memory.md](../memory.md) §3), or a copy as today for big GPU-heavy data |
| Written by the GPU, read by the CPU (results, queries) | cached RAM, in place | the title reads them; the GPU writes over PCIe once |

**Mechanism (Linux, under Wine):**

1. **VRAM at the title's address.** The layer asks vkd3d-proton (a patch of ours, a private device extension) for a heap whose memory is a VRAM allocation **mapped at a given virtual address** (`vkMapMemory2` with `VK_EXT_map_memory_placed`). Wine's view of that range must say "mapped, not ours to touch"; `xmem` reserves the title's ranges, so the address is known before the title writes.
2. **Aliases** (the page pool maps one physical page at several addresses): the VRAM object exported as a dma-buf and mapped again at each alias (`mmap` of the fd, `MAP_FIXED`), on the Unix side (a Wine unixlib of ours or a vkd3d hook).
3. **Resources over it:** title buffers and textures whose memory is in such a heap become **placed resources in it** at the matching offset: no host copy, no refresh, no snapshot, no copy-back. The layer's views point at them as today.
4. **Sorting pages:** at first, per kind of memory with what is known (buffers in VRAM, windows in RAM, since 63 of 70 title reads were in windows). Later, per page with a census over a longer run (IBS, or a one-shot fault census on a sample), and a move when a VRAM page turns out to be read: copy it to RAM, remap it, and patch the resources over it.

## Steps

1. **vkd3d-proton patch spike (native first):** a heap from a placed VRAM mapping at a chosen address, a placed buffer in it, the CPU writes and the GPU reads the same bytes. Done in the native `wineless` tree first (no Wine view bookkeeping), then under Wine.
2. **Buffers only** (`VICTORY_VRAM_BUFFERS=1`): title buffers whose memory is private `XMemVirtualAlloc` memory, committed whole (28 % of the bytes shaders read, [memory.md](../memory.md) §2), allocated in VRAM from the start (`xmem`), resources placed over it. Measure: refresh MB, copy-back MB, frame, the IBS census of title reads of that memory.
3. **The page pool:** sections replaced by VRAM objects plus dma-buf aliases.
4. **Per-page moves** for pages the CPU turns out to read.

## Risks

- **Wine's address-space bookkeeping**: a placed map at a range Wine reserved must not be unmapped or protected behind our back (`VirtualProtect` on it, guard pages, write watch: VRAM has no write watch, and nothing needs it there).
- **The title reading a VRAM page in a hot loop**: 0.5-1 µs a load. The census must find those before a kind moves wholesale, and a page that turns hot must be movable.
- **Device loss and reset**: VRAM content is lost if the GPU resets, and the title's memory with it. Today a loss ends the run anyway.
- **Size**: 1.1 GB of buffers + 3.6 GB of windows fit the 20 GB card with textures, but VRAM is also our host textures (9 GB live in the pool).

## Status (26 Nov 2026, parked): what works, what does not, how to resume

**Parked with every switch off.** Nothing of M.2 is on by default; the game runs as before (~400 ms, picture right).

### What works

- **The mechanism, end to end on this machine:** title buffers in ReBAR VRAM mapped at their own addresses, under Wine, the CPU and the GPU on the same bytes (`bench/trials-…-vram-wine`: 286 placements). Linux side proven alone first (`tools/rebar-bench/placed-alias.c`: placed map, dma-buf aliases, CPU and GPU agree).
- **Windows alone in place** (system memory, `VICTORY_INPLACE=1`): the picture is right; the GPU is 2× slower (379 ms) reading them over PCIe.
- **Buffers ≥ 1 MiB alone in place** (`VICTORY_INPLACE=3 VICTORY_INPLACE_BUFFER_KB=1024,10000000`): right; no gain (read-only, cheap to copy).

### What does not, and what is ruled out

- **Small and mid buffers in place together break the picture** (the house dark, the dock and the water cut and bent, debris on the water), in system memory and in VRAM alike, and in-place runs lose the device now and then.
- Ruled out: shader writes under copies (0 declared on buffers in place); the title's buffer copies (0 touch in-place memory; they are now recorded on the card when both sides are in place); host-import failures (none with `VKD3D_DEBUG=warn`); the address directory's offset (a real bug, fixed in `memory.rs`, not the whole cause).
- **Hypothesis left:** per-frame data the title's threads rewrite while the card still draws. Copies give the card a snapshot taken at submission; in place it reads the bytes as they change. To test: time the title's CPU writes to those buffers against the submission that reads them (write watch per submission, or IBS stores by address), and compare with when the title's fences say the card is done.

### The next design step

Share only memory the title does not rewrite while the card reads it (static data, streamed geometry, the big buffers), keep copies for dynamic data, and learn the split per buffer from that census. Then the Wine/VRAM path for what is shared, and per-page placement for windows (the title's CPU reads them: 63 of 70 IBS samples).

### How to resume (all pieces are kept)

| Piece | Where | How |
|---|---|---|
| vkd3d-proton patch (`VKD3D_VRAM_AT_ADDRESS=1`: heaps opened on host memory become ReBAR VRAM placed at the same address; asks for `VK_EXT_map_memory_placed` though Wine does not list it) | `patches/m2/vkd3d-proton/` (on GE-Proton11-14's vkd3d commit `cae8353e` plus GE's four patches); worktree `~/src/vkd3d-proton-vr`, branch `victory-vram` | `meson setup --cross-file build-win64.txt --buildtype release build-vr-win64 && ninja -C build-vr-win64` (needs `mingw-w64-gcc`, `meson`: installed) |
| Wine patch (`winevulkan`'s loader lets `VK_EXT_map_memory_placed` through) | `patches/m2/wine-winevulkan-map-memory-placed.patch` | On GE's prepared source (`victory-data/wineserver/build-GE-Proton11-14/ge/wine`): regenerate `include/wine/vulkan.h` (`dlls/winevulkan/make_vulkan -x vk.xml -X video.xml`; GE's prep left it empty), configure a separate tree `--with-vulkan` (`build-vk`), `make dlls/winevulkan/x86_64-windows/winevulkan.dll` |
| Proton copy with the patched `winevulkan.dll` | `~/Games/victory-data/proton-vr/GE-Proton11-14-vr` (reflinked copy; the original DLL as `winevulkan.dll.ge`) | `VP_PROTON=<that folder>` |
| Exe copy beside the patched `d3d12.dll`/`d3d12core.dll` | `~/src/victory-royale-wt/vram/release/` (the folder must be named `release` or `diag`: `bench.sh` finds the game by that) | `VP_EXE=<that>/victory.exe`, copy a fresh `victory.exe` there after each build |
| Layer switches | `VICTORY_INPLACE` (1 windows, 2 windows and buffers, 3 buffers only for VRAM: no shared 64 KiB pages, heaps kept), `VICTORY_INPLACE_BUFFER_KB=min,max` (bisection by size), `VICTORY_BACK_OVERLAP=1` (censuses on the frame line) | e.g. `VP_PROTON=… VP_EXE=… scripts/trials.sh -n 1 -l x VICTORY_INPLACE=3 VKD3D_VRAM_AT_ADDRESS=1 VKD3D_DEBUG=warn` |

Journal entries of 26 Nov hold every run and its numbers.
