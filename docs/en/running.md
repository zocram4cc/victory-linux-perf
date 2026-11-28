[Русский](../ru/running.md) | [简体中文](../zh/running.md) | [Español](../es/running.md) | [Português (BR)](../pt-BR/running.md)

# How to run

[Back to README](../../README.md)

Launch with the provided launcher: **`RAZDA4A_XEN.exe`** or **`RAZDA4A XEN.bat`**. Do not run the game executable directly. The runtime is applied only when the launcher starts the game.

No game files, assets, or executables are included. You must supply your own copy of *The Great Car-Borrowing Simulator from the Starry Studio*.

## Quick Start (Windows)

1. Download `main` (**Code -> Download ZIP** or `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) and extract (e.g. `D:\RAZDA4A-XEN\`). Required DLLs (`D3D12\D3D12Core.dll`, `fidelityfx\*.dll`) are included.
2. Launch with **`RAZDA4A_XEN.exe`** or **`RAZDA4A XEN.bat`**. Do not start your game `.exe` by double-clicking it.
   - **`RAZDA4A XEN.bat`** opens the launcher window: click **Browse...** to select your **`your_executable.exe`** (automatically saved to `config.bat` in UTF-8), choose your profile and Frame Generation toggle, and click **Launch**.
   - External launchers / CLI (`VILauncher`, etc.): pass **`RAZDA4A XEN.bat --no-gui`** after setting `config.bat`.

| File | Purpose |
|---|---|
| `RAZDA4A XEN.bat` | Main launcher GUI (Browse `.exe`, FG ON/OFF, auto-Safe-Mode for 8 GB GPUs, Warm Shaders & Export Logs buttons) |
| `RAZDA4A_XEN.exe` | Runtime the launcher starts. Do not skip it and run the game executable directly. |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Tight pagefile / 16 GB RAM: NVMe file-backed physical pages (`VICTORY_FILE_PHYSICAL=1`) |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced + experimental in-place constant buffers (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Pure native rendering, Frame Gen OFF, lowest input latency |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB VRAM Safe Mode (`65%` room / `2048 MB` targets / `768 MB` card buffers, VSync ON) |
| `warm_shaders.bat` | Run once after closing your first session to pre-compile encountered PSOs |
| `collect_logs.bat` | Write a sanitized hardware + `logs\last_run.log` report to `logs\bug_report_*.txt` on this PC. |
| `pagefile_16gb_ram.ps1` / `backup_saves.bat` | Optional 16–32 GB SSD pagefile helper / backup `data\` |

Controls: **`F2` / `~`** toggles mouse-look (`Esc` unlocks cursor), **`F3`** toggles the top-right FPS overlay.

## Quick Start (Linux — Proton / Wine / VKD3D-Proton)

1. Install **Proton Experimental / GE-Proton** (via `umu-run`, Steam, or Lutris) with **Mesa RADV** on an AMD RDNA2+ GPU.
2. Run `launch_linux_proton.sh` (automatically sets `WINEDLLOVERRIDES="d3d12,d3d12core=n"` so the bundled Windows 10 `D3D12Core.dll` does not shadow `vkd3d-proton` and cause a 1 FPS / blank 3D screen). The script starts `RAZDA4A_XEN.exe` for you. Do not run the game executable directly.
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

## See also

- [Performance](performance.md)
- [Hardware](hardware.md)
- [What Wave 11 ships](technical-changes.md)
- [Known limits](known-limits.md)
- [Security, checksums, and building](security-and-build.md)
- [Branches and license](project.md)
