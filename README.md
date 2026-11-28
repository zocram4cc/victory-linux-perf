English | [Русский](README.ru.md) | [简体中文](README.zh.md) | [Español](README.es.md) | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

Direct3D 12 Windows runtime layer for **AMD RDNA2 / RDNA3 / RDNA4 GPUs**, targeting **16 GB RAM**, **6-core CPUs**, **8–16 GB VRAM**, and **Windows 10 / 11** (plus Linux via Proton / VKD3D-Proton).

No game files, assets, or executables are included. You must supply your own copy of *The Great Car-Borrowing Simulator from the Starry Studio*.

## Measured Performance (Native FPS vs. Frame Generation)

All FPS figures below separate **Native FPS (Frame Gen OFF)** from **Displayed FPS (FSR 3.1.6 Frame Gen 2x)** and specify the exact scene and hardware.

### 16 GB VRAM / 16 GB RAM Baseline (Open World, 1080p Series S, RT OFF)

Measured on **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 GB, 16 GB DDR4, Windows 10 (19045)** — 180 s continuous Open World runs + 22-minute Wave 11 stability run (18,393 frames), 0 errors, 0 OOM:

| Metric | Baseline (upstream config) | RAZDA4A XEN (Wave 7 / Wave 11) |
|---|---|---|
| **Native FPS (FG OFF, Open World)** | 14.08 avg (71.03 ms, p99 152.1 ms / 1% low 6.6 FPS) | **18.19 p50** (54.97 ms), **16.17** mean, 15.2–23.4 steady range, p99.9 **478.6 ms** |
| **Displayed FPS (FSR 3.1.6 FG 2x)** | 25–27 FPS (VSync drops to 15–20) | **29.2 – 31.0 FPS** steady (HUDLess 3-slot RGBA8 ring, no VSync halving) |
| Private Commit (steady) | 21.8 – 30+ GB (`0x8007000E` OOM) | **19.3 GB** (6.2 GB RAM WS, 5.5 GB free commit) |
| Dedicated VRAM | 9.92 GB (`5.06 GB` texture pool) | **12.11 GB** (auto 8–24 GB DXGI; `0` VRAM refusals over 22 min in Wave 11) |
| Shared GPU (`NON_LOCAL`) | 6.68 GB (5.58 GB Series S) | **2.18 GB** (`-61%` vs Series S baseline) |
| Copy-back traffic | 27.0 MB / frame | **0.5 MB / frame** |
| CPU `preparing` / `heap buffers` | 45.8 ms / 821.1 ms max | **32.2 – 35.4 ms** / **33.8 ms max** (0 spikes >50 ms) |
| Raw buffer views (`LIST_VIEWS`, Wave 11) | 372.1–575.8 null/frame (missing trees/props) | **0.0 null/frame** (28,212/frame, 932.5 unaligned -> 0 null; trees/foliage/fences fixed) |
| Windows 10 D3D12 | Manual Agility SDK setup | Bundled `D3D12Core.dll` (Agility SDK 620) |

### 8 GB VRAM Safe Mode Verification (PR #1 by `nathanbzrr`)

Measured on **Ryzen 5 5500X3D, Radeon RX 7600 8 GB, 32 GB DDR4, Windows 11 (26340)** — 1080p Series S, Safe Mode (`65%` texture room / `2048 MB` targets / `768 MB` card buffers), FSR FG 2x, 180 s continuous run:

| Metric | Before 8 GB fit | After Safe Mode live-budget fit |
|---|---:|---:|
| **Native FPS (FG OFF equivalent)** | 3.67 | **12.7** (mean 79.01 ms, p50 68.90 ms, p99 **242.26 ms** vs 1047.30 ms) |
| **Displayed FPS (FSR FG 2x)** | 7.32 | **24.86** (1803 generated / 1803 rendered) |
| VRAM peak / live budget | 6548 MB | **6169 MB** / 5934–5981 MB (`0` errors, `0` OOM, `0` device removal) |

## Technical Changes

- **Wave 11 (foliage/props & portable CPU build):** Fixed `372–575` forced-`NULL` unaligned raw buffer views per frame in `LIST_VIEWS` (eliminates disappearing trees, foliage, fences, wooden posts, and road signs); `SW_64KB_D` layout handling; tuned 16 GB VRAM residency (`0` `card has no room` / `host memory pressure` events across 22 minutes); portable **`x86-64-v3`** release binary (full AVX2/BMI2/FMA on both AMD Zen 2–5 and Intel Haswell–Arrow Lake CPUs).
- **Wave 6–7 (textures, descriptors & stability):** Descriptor HashMap + 8192-slot TLS cache (`100%` hit rate), best-fit pool, bounded `let_go_idle`, interleaved texture queue (cutscene backlog `3936 -> 0` frames), 16 GB RAM hysteresis, Normal CPU priority (prevents 100% CPU lockups of background apps), and `VICTORY_LOG_FILE` file logging.
- **Memory & OOM (`0x8007000E`):** Bounded prelude pools, immediate `recycle_landed`, proactive `emergency_reclaim()` (<2.5 GiB free commit or >=88% Shared GPU), `E_OUTOFMEMORY` retry/recovery, `MEM_RESET`, dynamic DXGI Auto-VRAM (`8/12/16/24 GB`), and optional NVMe file-backed physical pages (`VICTORY_FILE_PHYSICAL=auto`).
- **Presentation & Controls:** 3-slot RGBA8 `HUDLessColor` capture for FSR 3.1.6 FG, tearing swapchain, native AMD FSR 4.1.2 ML upscaling (FSR 3.1 fallback), `5000 ms` compute wait (character faces fix), RDNA4 crash-shader shield (`240cb516c1af0bc3`), PC keyboard/mouse controls (`F2`/`~` mouse-look, `F3` top-right FPS HUD).

## Known Limits (Active Work on `dev`)

- Dense city zones can still drop to `5–10` native FPS and trigger long descriptor-table spikes during extended sessions (active **Wave 12** work on `dev`).
- Ray Tracing is off while core rendering, VRAM budgets, and native 30 FPS CPU bottlenecks are being stabilized.
- First launch compiles PSO shaders (1–3 minutes of stutters); second launch onward is significantly smoother after warming the cache (`warm_shaders.bat`).

## Hardware Compatibility & FAQ

| Question | Answer |
|---|---|
| **AMD GPUs supported?** | **AMD RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000 series, DX12 Ultimate + Mesh Shaders required). |
| **NVIDIA / Intel Arc GPUs?** | **Not supported in this fork.** RTX users experience black screens / crashes; use community NVIDIA forks such as [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). |
| **Intel CPU + AMD GPU?** | **Supported.** Wave 11+ binaries are built with portable `-C target-cpu=x86-64-v3`. |
| **8 GB / 6 GB / <=4 GB VRAM?** | **8 GB:** Works with **Safe Mode** (verified 12.7 native / 24.86 FG on RX 7600; also verified on RX 6600). **6 GB:** Experimental / rough (Safe Mode + FG OFF). **<=4 GB, RDNA1 (RX 5500/5700), GTX 10/16:** Unsupported (missing hardware Mesh Shaders or insufficient VRAM). |
| **16 GB RAM / HDD?** | **16 GB RAM:** Supported on **NVMe/SATA SSD** with a 24–32 GB pagefile (`pagefile_16gb_ram.ps1`) or `Low-Commit NVMe` preset. **HDD:** Not supported (streaming / DirectStorage stalls). |
| **Do I need a leaked GDK?** | **No.** Completely standalone open-source runtime (`GPL-3.0-or-later`). |
| **Where do I send bug reports & logs?** | Run **`collect_logs.bat`** (or click **Export Logs** in the launcher) to generate a sanitized `logs\bug_report_*.txt` (no private paths) and attach it to [GitHub Issues](https://github.com/crossducking/RAZDA4A-XEN/issues). |

## Quick Start (Windows)

1. Download `main` (**Code -> Download ZIP** or `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) and extract (e.g. `D:\RAZDA4A-XEN\`). Required DLLs (`D3D12\D3D12Core.dll`, `fidelityfx\*.dll`) are included.
2. Run **`RAZDA4A XEN.bat`**, click **Browse...** to select your **`your_executable.exe`** (automatically saved to `config.bat` in UTF-8), choose your profile and Frame Generation toggle, and click **Launch**.
   - Do not double-click the game executable. Windows then looks for console DLLs (`d3d12_xs.dll` and others) that only the launcher supplies, and the window stops with "was not found".
   - Keep the whole unpacked folder together, including `D3D12\`, `fidelityfx\`, and `presets\`. The launcher is not a single file you copy into the game directory.
   - External launchers / CLI (`VILauncher`, etc.): pass **`RAZDA4A XEN.bat --no-gui`** after setting `config.bat`.

| File | Purpose |
|---|---|
| `RAZDA4A XEN.bat` | Main launcher GUI (Browse `.exe`, FG ON/OFF, auto-Safe-Mode for 8 GB GPUs, Warm Shaders & Export Logs buttons) |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Tight pagefile / 16 GB RAM: NVMe file-backed physical pages (`VICTORY_FILE_PHYSICAL=1`) |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced + experimental in-place constant buffers (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Pure native rendering, Frame Gen OFF, lowest input latency |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB VRAM Safe Mode (`65%` room / `2048 MB` targets / `768 MB` card buffers, VSync ON) |
| `warm_shaders.bat` | Run once after closing your first session to pre-compile encountered PSOs |
| `collect_logs.bat` | Export a sanitized hardware + `logs\last_run.log` bug report |
| `pagefile_16gb_ram.ps1` / `backup_saves.bat` | Optional 16–32 GB SSD pagefile helper / backup `data\` |

Controls: **`F2` / `~`** toggles mouse-look (`Esc` unlocks cursor), **`F3`** toggles the top-right FPS overlay.

## Quick Start (Linux — Proton / Wine / VKD3D-Proton)

1. Install **Proton Experimental / GE-Proton** (via `umu-run`, Steam, or Lutris) with **Mesa RADV** on an AMD RDNA2+ GPU.
2. Run `launch_linux_proton.sh` (automatically sets `WINEDLLOVERRIDES="d3d12,d3d12core=n"` so the bundled Windows 10 `D3D12Core.dll` does not shadow `vkd3d-proton` and cause a 1 FPS / blank 3D screen):
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```
   Logs are saved to `logs/last_run.log`. Use `XEN_PROFILE=balanced` on 12–16+ GB VRAM GPUs.

## Security, Antivirus False Positives & Build from Source

Browsers (Brave/Edge) and Windows Defender ML may flag unsigned GitHub ZIPs containing `.exe` + `.bat` + `.ps1` scripts as **`Trojan:Script/Wacatac.H!ml`**. Never trust unsigned binaries blindly — verify checksums or compile from the 100% open-source `dev` branch yourself:

```powershell
# Verify downloaded binary against SHA256SUMS.txt
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (all DLL hashes are listed in [`SHA256SUMS.txt`](SHA256SUMS.txt)).

Build the exact binary from source (`dev` branch, MSVC Rust toolchain):

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## Branches & License

- `main`: prebuilt binary, runtime DLLs, presets, launcher, checksums, documentation.
- `dev`: full Rust source code, unit tests, `xtask policy` asset-policy guard, CI, `AGENTS.md`.

Derived from VIctory under **GPL-3.0-or-later** ([LICENSE](LICENSE)). Full corresponding source code is in [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
