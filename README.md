English | [Русский](README.ru.md) | [简体中文](README.zh.md) | [Español](README.es.md) | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

https://x.com/crossducking

Direct3D 12 Windows runtime layer targeting **16 GB RAM**, **6-core CPUs**, **8–16 GB GPUs**, and **Windows 10**.

No game files, assets, or executables are included. You must supply your own copy of *The Great Car-Borrowing Simulator from the Starry Studio*.

## Differences from upstream VIctory

Upstream VIctory is developed and profiled primarily on 64 GB RAM, 24 GB GPUs, and Windows 11 (WDDM 3.0+ with `D3D12_HEAP_TYPE_GPU_UPLOAD`). Running it on a 16 GB RAM / Windows 10 system hits `0x8007000E` (`E_OUTOFMEMORY`), VSync frame-rate halving, HUD ghosting under Frame Generation, and CPU submit bottlenecks.

Measured on **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 GB, 16 GB DDR4, Windows 10 (19045)** — 1080p Series S profile, RT off, 180-second continuous runs, 0 errors:

| Metric | Baseline (upstream config) | RAZDA4A XEN |
|---|---|---|
| Private Commit (steady) | 21.8 – 30+ GB (`0x8007000E` OOM) | **19.3 GB** (6.2 GB RAM WS, 5.5 GB free commit) |
| Dedicated VRAM | 9.92 GB (`5.06 GB` texture pool) | **12.11 GB** (`5.12 GB` texture pool; up to `13.18 GB` / `6.40 GB` pool; auto 8–24 GB DXGI) |
| Shared GPU (`NON_LOCAL`) | 6.68 GB (5.58 GB Series S) | **2.18 GB** (`-61%` vs Series S baseline; `2.01 GB` at 91% keep) |
| Copy-back traffic | 27.0 MB / frame | **0.5 MB / frame** |
| CPU `preparing` time | 45.8 ms | **32.2 – 35.4 ms** |
| Real frame time (min / med) | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| Native FPS & pacing (FG off, steady Open World) | 14.08 FPS (mean 71.03 ms, p99 152.1 ms / 1% low 6.6 FPS) | **15.2 – 23.4 FPS** (p50 **60.4 ms** / 16.6 FPS, `list_al` **0.03 ms**, `alloc` **4.8 ms**, `0` 5s/10s freezes, `0` OOM / `0` emergency reclaim) |
| Heap buffers pass (gameplay max / >50 ms spikes) | 821.1 ms (8 spikes >50 ms) | **33.8 ms (0 spikes >50 ms; steady 8.6–10.0 ms)** |
| Settle waits & descriptor `take()` (steady, FG off) | 0.2–2.6 ms settles / 0.75–1.07 ms `take()` | **0.0 ms settles / 0.31–0.47 ms `take()`** |
| Displayed FPS (FSR 3.1.6 FG) | 25–27 FPS (VSync drops to 15–20) | **29.2 – 31.0 FPS** (no VSync drops) |
| Frame Gen HUD / subtitles | Ghosting and warping | Clean (`HUDLessColor` 3-slot RGBA8 ring) |
| Windows 10 D3D12 | Manual Agility SDK setup | Bundled `D3D12Core.dll` (Agility SDK 620) |

## Technical changes

- **Memory & OOM (`0x8007000E`):** Global bounded prelude command-list pool `(64, 16)`; immediate recycling of landed host lists and command allocators (`recycle_landed`) with deferred resource retirement (`bury_lag`); proactive `emergency_reclaim()` below 2.5 GiB free commit or >=88% Shared GPU (`NON_LOCAL`) with aggressive LRU eviction of idle textures/staging/pools and dynamic `thin` recording concurrency cap (1–2 workers under pressure); `E_OUTOFMEMORY` reclaim, retry, and safe allocator reset/retirement in `HostList::close()`, `make_list()`, and `record_prelude()`; `MEM_RESET` on unmapped freed physical pages; upstream fixes `#2143` (constant table after instructions), `#2234` (view heap releases title-memory table), `#2462` (`typed_store` UAV format check), `#2844` (RAII `CpuViews` descriptor slots), and `#3540` (16-byte aligned raw buffer views); optional NVMe file-backed physical pages (`FILE_FLAG_DELETE_ON_CLOSE` in `%LOCALAPPDATA%\victory\physical`, >11 GB free commit during loading). Verified on 180 s native No-FG run with standard pagefile (`VICTORY_FILE_PHYSICAL=0`): 0 errors, 0 OOM, `min_free_commit 3.47 GB`, steady `5.10–5.57 GB` free commit.
- **Presentation & Frame Generation:** Real 3-slot RGBA8 `HUDLessColor` capture before UI/subtitle/HUD composition; triple-buffered swapchain with `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING` and `DXGI_PRESENT_ALLOW_TEARING`; native AMD FSR 4.1.2 ML upscaling (FSR 3.1 fallback on older GPUs).
- **CPU submission & copy-back:** Write-combined shader writes kept in VRAM (`27 -> 0.5 MB/frame` copy-back); 128-byte AVX2 `vptest` dirty-word skip and vectorized `merge_piece`; lock-free work-stealing landing pool; 64-byte cache-aligned write-watch shards; fused single-pass `ask_once`/`stale_under_flights` scan; lock-free descriptor `mirror`; `OnceLock` canonical command signatures; stack-allocated `Few<Barrier, 16>`; 64-sharded `PLACED_TEXTURES`; unlocked queue `unfold` and lock-free `wake()`; unlocked `upload::ARENAS` zeroing; elevated helper thread priority for 6-core CPUs; pre-planned closed lists on thin workers; hash-before-detile; upload buffer reuse.
- **Dynamic DXGI Auto-VRAM & Shared-GPU reduction:** Automatic 8 / 12 / 16 / 24+ GB tier policy from `DedicatedVideoMemory` and live `DXGI_QUERY_VIDEO_MEMORY_INFO` (`VICTORY_VRAM_MODE=auto|8gb|12gb|16gb|24gb|legacy`, with `"auto"` defaults and manual env overrides for all VRAM knobs, UMA detection, and commit-headroom step-down). On 16 GB GPUs, RAM-aware budgets keep `<=20 GB` RAM hosts at `84%` keep / `90%` room (`12.11 GB` VRAM, `2.18 GB` Shared GPU, zero OS working-set paging, `+55%` 1% low FPS) and `>=32 GB` RAM hosts at `91%` keep / `95%` room (`~13.2–14.2 GB` VRAM). Zero-staging direct VRAM loose windows (`CardCpu::Direct`), compact CPU shadows (`CardCpu::Cpu`), `DIRTY_CARDED` O(dirty) submission drain, incremental VRAM window eviction with hysteresis (max 8/s), shared depth-stencil twin scratch buffers (upstream `#2162`), 16 GB RAM `host_pressure` threshold fix (`send_allocs` `117/15s -> 0/15s`), NVMe auto-detection (`VICTORY_FILE_PHYSICAL=auto`), and title-only `XMemGetWorkingSetStatistics`.
- **Frame pacing & spike elimination (native No-FG):** Asynchronous incremental `sweep()` (`victory-sweep`, `VICTORY_SWEEP_ASYNC=1`) eliminating the 1.5 s synchronous `settle(WHY_SYNC)` pipeline drain; throttled background `bury()` reaper (`victory-bury`, `VICTORY_BURY_ASYNC=1`); unlocked `Line.seq` fence/value waits under relay (`WHY_FENCE` / `WHY_VALUE`); skipping redundant `sync_heaps` on `execute_own` presents (`0.0` unmarked rounds); 64-bit `AtomicU64` word-scan in `port_descriptors::take()` (`2–3×` faster); precision `frame::wait` sleep without `Sleep(0)` yields; built-in per-frame present pacing telemetry (`avg / p50 / p95 / p99 / 1% low / max / >50 ms / >100 ms`, `VICTORY_FRAME_TIMES_FILE`) and flattened `heap buffers` (`VICTORY_TEXTURE_STAGE_MS=4`, `VICTORY_TEXTURE_CATCHUP=0`, `0` spikes >50 ms).
- **Phase 2 CPU submit & descriptor optimizations (`feat/phase2-30fps`):** Range-granular cross-submission `VirtualQuery` `MappedCache` in `card_steps` (`12.56 ms / 36 calls -> 0.00 ms / 0 calls`, `own_cs` `15.1 -> 2.5 ms`); lock-free per-slot atomic generation table (`ENTRY_GENS`) + 8192-slot TLS `descriptor::VIEWS` / `words_for` cache (`100.0%` TLS hit rate); inline CBV fast-path + per-`State` 128-slot dynamic descriptor slab bump allocator & batched `release_dynamic_batch` in `draw_bind::tables` + per-thread `RETIRE_TLS` batching; zero-churn list recycling (`BACKS_MOST=512`, `IDLE_BYTES=512 MiB`, `list::boxed()` → `fresh_list()`, `list_al` `-99.7%`, `alloc` `-83%`, `lists` churn `+0`); per-copy incremental `BufferSweep` + skip-unchanged live slots in `heap_buffer_pass` (`HBP_SPANS` `10.94 -> 0.51 ms`); expanded `PRELUDES_MOST` `(512, 128)`, `SPARE_BATCH` `(128/256)`, `send::SPARE` `(256/128 MiB)`, and `VICTORY_LIST_IDLE_MOST=1024`; dedicated compute PSO priority queue + 4 compute/screen workers with late gameplay wait cap (`VICTORY_COMPUTE_LATE_WAIT_MS=400`, zero 5s/10s gameplay hitches, `p99.9` `-80%`, `1% low` `3.2×`); 16 GB RAM `residency` threshold fix (`unmap_null` `-97%`, `VICTORY_KEEP_SHOWN=2`, `VICTORY_TEXTURE_AWAY_MS=10000`); 64 KiB page-granular `gpu_written_pages` bitmap for `PORT_CBV`; `VICTORY_PANEL_POS=top-right` HUD; opt-in deep profiler (`VICTORY_PHASE2_PROFILE=1`).
- **Fixes & input:** Compute pipeline wait budget (`5000 ms`) fixing missing character faces; RDNA4 crash-shader skip (`240cb516c1af0bc3`); violet ambient probe filter; safe async descriptor flips; failed texture detile retry; PC keyboard/mouse mapping (`F2`/`~` mouse-look, `F3` FPS overlay in top-right); built-in launcher checkbox to toggle Frame Generation ON/OFF.

## Current known limits (active work on `dev`)

- Native 30 FPS (`<= 33.3 ms`) is actively being pushed further on `dev`: `victory-submit` is down to `34.4 ms/frame` while 4-thread command list recording (`victory-list-0..3`) and CBV copies remain the next optimization target.
- Prologue-to-open-world transition can spike Commit on tightly constrained pagefiles; proactive `emergency_reclaim()` and NVMe `VICTORY_FILE_PHYSICAL=auto` mitigate it on `dev`.

## Verified Hardware & Community Results (Wave 4)

### Verified Hardware Matrix

| Hardware / OS | Reporter / Bench | Status & Measured Results |
|---|---|---|
| **AMD Ryzen 7 7800X3D + Radeon RX 9060 XT (RDNA 4)** — Windows | Main dev bench | Stable. Wave 4: **~14–24 native FPS** (`14.0 FPS` Open World mean, `15–19 FPS` typical scenes) + 2× smoothness with native **AMD FSR 3.1.6 FG (`HUDLessColor`)**; local VRAM `~11.9 GB`. After latest update: **35 native FPS (FG off)** / **60+ FPS with FG**. |
| **AMD Ryzen 5 5600 + 16 GB RAM** — Windows | 16 GB RAM verification bench | Stable Open World gameplay without multi-second stutters or freezes after Wave 4/5 fixes. |
| **AMD Ryzen 7 7700X + Radeon RX 7800 XT (16 GB) + 64 GB RAM** — Windows | `@LynxarAD` | Working: **31–37 FPS with FSR FG** (`15.7×2`–`18.6×2`, `~16–19` native FPS), local VRAM `~12.3 GiB`, GPU `46%` / CPU `75%`. Second launch is much smoother after PSO cache warm-up; cutscenes and new-zone streaming can dip to `~7 FPS`. |
| **AMD Radeon RX 6600 (8 GB) + 16 GB RAM** — Windows | `@TriguerosZoe99` | Working at **14–20 native FPS** (FG off) **only with `Preset 1 (Low-Commit NVMe)` or `Preset 4 (Safe Mode 8–12GB VRAM)`** + NVMe pagefile. Without those presets on 8 GB VRAM, hangs on a black screen after the main menu. |
| **Intel Core i7-8700 + NVIDIA RTX 2070 (8 GB) + 16 GB DDR4** — Linux | `@supimcinnamon` | Boots at **~10 native FPS** only on a modified fork over `RAZDA4A-XEN` with a patched `wineserver`. |
| **AMD Ryzen 5 5500 + NVIDIA RTX 5060 (8 GB) + 16 GB RAM** — Windows | `@Golden_L2` | Does not work (`0 FPS` in 3D, black screen, RAM exhaustion `15.3/15.9 GiB` before first scene). This branch supports AMD RDNA GPUs only; RTX users should use community NVIDIA forks (e.g. `trustno1-coder/VIctory-NVIDIA`). |

### Known Behavior & Tips

- **No build/compilation needed:** Download the ready-to-run files from `main` (or Releases) and copy them directly into the root folder alongside your unpacked files (or keep them in their own folder and point `config.bat` to it):
  ```text
  YourUnpackedFolder\
  ├── RAZDA4A XEN.bat          <-- run this
  ├── RAZDA4A_XEN.exe
  ├── config.bat               <-- set GAME_DIR and GAME_EXE
  ├── launcher_ui.ps1
  ├── D3D12\D3D12Core.dll
  ├── fidelityfx\*.dll
  └── presets\*.bat
  ```
- **First launch vs. second launch:** The first launch may freeze or stutter while compiling initial compute PSOs. From the second launch onward, the shader/PSO cache is warm and frame pacing becomes noticeably smoother.
- **Known Wave 4 limitations (addressed across Wave 5+):** Flickering/rough edges on thin geometry, meshes, and foliage/trees (both native and with FG), sawtooth frame times from descriptor lock contention (`~76.6 ms`), and high CPU thread utilization.

## Quick start

1. Download `main` (**Code -> Download ZIP** or `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) and extract (e.g. `D:\RAZDA4A-XEN\`). Required DLLs (`D3D12\D3D12Core.dll`, `fidelityfx\*.dll`) are included.
2. Edit **`config.bat`** to point to your folder and **`your_executable.exe`**:
   ```bat
   set "GAME_DIR=D:\Path\To\Your\GameFolder"
   set "GAME_EXE=%GAME_DIR%\your_executable.exe"
   ```
3. Run **`RAZDA4A XEN.bat`** (or choose a preset from `presets\`):

| File | Profile |
|---|---|
| `RAZDA4A XEN.bat` | Balanced launcher with **Browse... `.exe` picker**, **Frame Generation ON/OFF checkbox** (saved in `launcher_prefs.bat`), auto-Safe-Mode for 8 GB GPUs, preflight warnings, HUDLess FG + Tearing |
| `launch_linux_proton.sh` | Linux (Proton / Wine / VKD3D) direct launcher without PowerShell/WinForms GUI |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Tight pagefile / background apps: NVMe file-backed physical pages, ~8.0 GB VRAM, >11 GB load commit headroom |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced + in-place constant buffers (experimental) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Native rendering, Frame Gen off, lowest input latency |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB VRAM / troubleshooting: strict VRAM/commit caps, VSync on, full descriptor scans |

- **`F2` / `~`** — toggle mouse-look (`Esc` unlocks cursor)
- **`F3`** — toggle FPS overlay
- **`warm_shaders.bat`** — run once after closing your first session to pre-compile encountered pipelines
- **`backup_saves.bat`** — backup/restore `data\`
- **`pagefile_16gb_ram.ps1`** — optional admin helper (16–32 GB SSD pagefile)

## Build from source & binary verification

Unsigned prebuilt binaries should never be trusted blindly. You can build the exact binary yourself from the `dev` branch:

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

Prebuilt `RAZDA4A_XEN.exe` SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`

## Branches & license

- `main`: prebuilt binary, runtime DLLs, presets, documentation.
- `dev`: full Rust source code, unit tests, CI, `AGENTS.md`.

Derived from VIctory under **GPL-3.0-or-later** ([LICENSE](LICENSE)). Full corresponding source code is in [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
