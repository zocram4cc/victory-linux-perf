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
| Dedicated VRAM | 9.92 GB (`5.06 GB` texture pool) | **8.18 GB** (`3.65 GB` texture pool) |
| Shared GPU (`NON_LOCAL`) | 6.68 GB | **5.45 GB** |
| Copy-back traffic | 27.0 MB / frame | **0.5 MB / frame** |
| CPU `preparing` time | 45.8 ms | **32.2 – 35.4 ms** |
| Real frame time (min / med) | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| Native FPS (FG off) | ~13.7 FPS | **14.5 – 15.5 FPS** |
| Displayed FPS (FSR 3.1.6 FG) | 25–27 FPS (VSync drops to 15–20) | **29.2 – 31.0 FPS** (no VSync drops) |
| Frame Gen HUD / subtitles | Ghosting and warping | Clean (`HUDLessColor` 3-slot RGBA8 ring) |
| Windows 10 D3D12 | Manual Agility SDK setup | Bundled `D3D12Core.dll` (Agility SDK 620) |

## Technical changes

- **Memory & OOM (`0x8007000E`):** Global bounded prelude command-list pool `(64, 16)`; immediate recycling of landed host lists and command allocators (`recycle_landed`) with deferred resource retirement (`bury_lag`); `E_OUTOFMEMORY` reclaim-and-retry in `make_list()` and `record_prelude()`; `NON_LOCAL` high-water throttle (`92%`, `-10%` hysteresis); optional NVMe file-backed physical pages (`FILE_FLAG_DELETE_ON_CLOSE` in `%LOCALAPPDATA%\victory\physical`, >11 GB free commit during loading).
- **Presentation & Frame Generation:** Real 3-slot RGBA8 `HUDLessColor` capture before UI/subtitle/HUD composition; triple-buffered swapchain with `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING` and `DXGI_PRESENT_ALLOW_TEARING`; native AMD FSR 4.1.2 ML upscaling (FSR 3.1 fallback on older GPUs).
- **CPU submission & copy-back:** Write-combined shader writes kept in VRAM (`27 -> 0.5 MB/frame` copy-back); 128-byte AVX2 `vptest` dirty-word skip and vectorized `merge_piece`; lock-free work-stealing landing pool; 64-byte cache-aligned write-watch shards; fused single-pass `ask_once`/`stale_under_flights` scan; lock-free descriptor `mirror`; `OnceLock` canonical command signatures; stack-allocated `Few<Barrier, 16>`; 64-sharded `PLACED_TEXTURES`; unlocked queue `unfold` and lock-free `wake()`; unlocked `upload::ARENAS` zeroing; elevated helper thread priority for 6-core CPUs; pre-planned closed lists on thin workers; hash-before-detile; upload buffer reuse.
- **Fixes & input:** Compute pipeline wait budget (`5000 ms`) fixing missing character faces; RDNA4 crash-shader skip (`240cb516c1af0bc3`); violet ambient probe filter; safe async descriptor flips; failed texture detile retry; PC keyboard/mouse mapping (`F2`/`~` mouse-look, `F3` FPS overlay).

## Current known limits (active work on `dev`)

- Native 30 FPS (`<= 33.3 ms`) is currently CPU-bound on single-thread `victory-submit` (`49–60 ms/frame`; GPU is 52–58% busy).
- Series S profile uses ~8.2 GB VRAM on 16 GB cards; dynamic adapter VRAM budgeting is in progress on `dev`.
- Prologue-to-open-world transition can spike Commit past default Windows pagefiles on 16 GB RAM; a code-level fix (no pagefile changes required) is in progress on `dev`.

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

Prebuilt `RAZDA4A_XEN.exe` SHA-256: `e90b23f05f6d3caa488beaf3afc4cd0e94151b75a33606bfd75594a373e4f18b`

## Branches & license

- `main`: prebuilt binary, runtime DLLs, presets, documentation.
- `dev`: full Rust source code, unit tests, CI, `AGENTS.md`.

Derived from VIctory under **GPL-3.0-or-later** ([LICENSE](LICENSE)). Full corresponding source code is in [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
