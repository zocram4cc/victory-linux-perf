English | [Русский](README.ru.md) | [简体中文](README.zh.md) | [Español](README.es.md) | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

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
| Native FPS & pacing (FG off, steady Open World) | 14.08 FPS (mean 71.03 ms, p99 152.1 ms / 1% low 6.6 FPS) | **16.6 – 19.5 FPS** (p50 **60.0 ms**, `victory-submit` **34.4 ms** vs 59.7 ms, `VirtualQuery` **0.00 ms**, `BufferSweep` **0.82 ms**, `0` OOM / `0` emergency reclaim) |
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
- **Phase 2 CPU submit & descriptor optimizations (`feat/phase2-30fps`):** Range-granular cross-submission `VirtualQuery` `MappedCache` in `card_steps` (`12.56 ms / 36 calls -> 0.00 ms / 0 calls`, `own_cs` `15.1 -> 2.7 ms`); lock-free per-slot atomic generation table (`ENTRY_GENS`) + 8192-slot TLS `descriptor::VIEWS` & `words_for` cache (`100.0%` TLS hit rate); per-copy incremental `BufferSweep` + skip-unchanged live slots in `heap_buffer_pass` (`HBP_SPANS` `10.94 -> 0.82 ms`); expanded `PRELUDES_MOST` `(256, 64)`, `send::SPARE` `(256 MiB / 128 MiB warm)` and `VICTORY_LIST_IDLE_MOST=320` eliminating per-frame D3D12 allocator/buffer churn; `Batch::last_ahead` 1-slot cache; 64 KiB page-granular `gpu_written_pages` bitmap for `PORT_CBV` shadow-direct mode; `VICTORY_PANEL_POS=top-right` HUD corner placement; opt-in deep profiler (`VICTORY_PHASE2_PROFILE=1`).
- **Fixes & input:** Compute pipeline wait budget (`5000 ms`) fixing missing character faces; RDNA4 crash-shader skip (`240cb516c1af0bc3`); violet ambient probe filter; safe async descriptor flips; failed texture detile retry; PC keyboard/mouse mapping (`F2`/`~` mouse-look, `F3` FPS overlay in top-right).

## Current known limits (active work on `dev`)

- Native 30 FPS (`<= 33.3 ms`) is actively being pushed further on `dev`: `victory-submit` is down to `34.4 ms/frame` while 4-thread command list recording (`victory-list-0..3`) and CBV copies remain the next optimization target.
- Prologue-to-open-world transition can spike Commit on tightly constrained pagefiles; proactive `emergency_reclaim()` and NVMe `VICTORY_FILE_PHYSICAL=auto` mitigate it on `dev`.

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
| `RAZDA4A XEN.bat` | Balanced (default): 29–31 FG FPS, ~8.2 GB VRAM, 6.2 GB RAM, HUDLess FG + Tearing |
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
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --locked -p victory
```

Prebuilt `RAZDA4A_XEN.exe` SHA-256: `044366d1ddc3e72c815273f600d1fce5ce6efad3af42ac3a5d0155080964be77`

## Branches & license

- `main`: prebuilt binary, runtime DLLs, presets, documentation.
- `dev`: full Rust source code, unit tests, CI, `AGENTS.md`.

Derived from VIctory under **GPL-3.0-or-later** ([LICENSE](LICENSE)). Full corresponding source code is in [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
