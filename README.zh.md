<p align="center">
  <span style="font-size:64px">🦋</span>
</p>

<h1 align="center">RAZDA4A XEN</h1>

<p align="center">
  面向 AMD RDNA 的 Direct3D 12 运行层。<br>
  <b>Wave 11</b> · <b>~31 native / 62-63 FG</b>
</p>

<p align="center">
  <img alt="版本：Wave 11" src="https://img.shields.io/badge/release-Wave%2011-2ea44f">
  <a href="https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip"><img alt="下载 main 分支 ZIP" src="https://img.shields.io/badge/download-main%20ZIP-0969da"></a>
  <a href="LICENSE"><img alt="许可证 GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue"></a>
  <img alt="Windows 10/11 与 Linux Proton" src="https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20Linux%20Proton-informational">
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.ru.md">Русский</a> ·
  简体中文 ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (BR)</a>
</p>

## 结果

| | |
|---|---|
| **版本** | Wave 11（`main`） |
| **机器** | Radeon RX 9060 XT + Ryzen 7 7800X3D |
| **FPS** | **~31 native / 62-63 FG** |

`62-63 FG` 是帧生成。原生是 ~31。

8 GB 显卡走 Safe Mode。一块 RX 7600 的实测是 **12.7 native / 24.86 FG**。表格见 [实测性能](docs/zh/performance.md)。

## 快速开始

本仓库没有游戏文件。请使用你自己的副本。

1. 下载 [main.zip](https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip)，或 `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`，然后解压。
2. 启动 **`RAZDA4A_XEN.exe`** 或 **`RAZDA4A XEN.bat`**。
3. 在启动器里选择游戏 `.exe`、配置和帧生成，然后点击 **Launch**。

不要直接运行游戏可执行文件。运行层只在启动器拉起游戏时生效。

`F2` 或 `~` 切换鼠标视角（`Esc` 释放光标）。`F3` 开关 FPS 显示。

Linux 使用 `launch_linux_proton.sh`。预设和 Linux 步骤见 [如何运行](docs/zh/running.md)。

## 要求

| | Wave 11 |
|---|---|
| **显卡** | AMD RDNA2 / RDNA3 / RDNA4（RX 6000 / 7000 / 9000），需要 DX12 Ultimate 和 mesh shaders |
| **CPU** | x86-64-v3（AMD Zen 2–5，Intel Haswell 到 Arrow Lake） |
| **内存** | 16 GB 内存，搭配 SSD。提交空间紧时用 24–32 GB 页面文件（`pagefile_16gb_ram.ps1`）。8–12 GB 显存：Safe Mode |
| **系统** | Windows 10 / 11，或通过 Proton / VKD3D-Proton 的 Linux |
| **不是这个版本** | NVIDIA、Intel Arc、RDNA1、GTX 10/16、4 GB 级显卡、机械硬盘 |

NVIDIA 版本正在到来。AMD 先完成。在那之前：[trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA)。

## 预设

| 文件 | 用途 |
|---|---|
| `RAZDA4A XEN.bat` | 启动器窗口：游戏 `.exe`、帧生成、8 GB 自动 Safe Mode |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB 内存，NVMe 文件页 |
| `presets\2_Max_FPS_Experimental_CBV.bat` | 均衡，并加上原地常量缓冲 |
| `presets\3_Native_No_FG_Zero_Latency.bat` | 仅原生，关闭帧生成 |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB 显存 |

## 常见问题

<details>
<summary>包含游戏吗？</summary>

不包含。请自备副本。不需要泄露的 GDK。这里没有游戏下载链接。
</details>

<details>
<summary>NVIDIA 或 Intel Arc？</summary>

Wave 11 是 AMD RDNA。NVIDIA 版本正在到来。AMD 先完成。社区分支见上方。不支持 Intel Arc。非 AMD 适配器可能黑屏或崩溃。
</details>

<details>
<summary>8 GB 显存，或更少？</summary>

8–12 GB：Safe Mode（预设 4，8 GB 时启动器会自动选用）。RX 7600：12.7 native / 24.86 FG。6 GB 及以下不是受支持的配置。RDNA1 和 GTX 10/16 没有这个版本需要的 mesh shaders。
</details>

<details>
<summary>光线追踪？</summary>

Wave 11 中关闭。光线追踪结构构建器不运行。
</details>

<details>
<summary>机械硬盘，或 16 GB 内存？</summary>

不支持 HDD。请使用 NVMe 或 SATA 固态硬盘。16 GB 内存下，`pagefile_16gb_ram.ps1` 或预设 1 用来应对偏小的页面文件。
</details>

## 遇到问题时

- 首次启动会编译管线（大约 1–3 分钟卡顿）。退出后运行一次 `warm_shaders.bat`，再从启动器打开。
- Windows Defender 可能把未签名 ZIP 标成 `Trojan:Script/Wacatac.H!ml`。把 `RAZDA4A_XEN.exe` 和 [SHA256SUMS.txt](SHA256SUMS.txt) 对照：`4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`。
- `collect_logs.bat` 或启动器里的 **Export Logs** 会在本机写出 `logs\bug_report_*.txt`。
- Linux 接近 1 FPS 或画面空白：使用 `launch_linux_proton.sh`，它会避免内置 `D3D12Core.dll` 盖过 vkd3d-proton。

[如何运行](docs/zh/running.md) · [安全性与从源码构建](docs/zh/security-and-build.md)

## 关于

RAZDA4A XEN 是面向 16 GB 内存、6 核 CPU 和 8–16 GB 显存的开源 Direct3D 12 运行层。`main` 上的 Wave 11 是启动器、预设、DLL 和这些页面。Rust 源码在 [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev)。

## 致谢

- [VIctory](https://github.com/Marvisak/VIctory)，作者 Marvisak，上游，GPL-3.0-or-later（[LICENSE](LICENSE)）。
- `nathanbzrr` — 8 GB Safe Mode，已随 `main` 发布。
- [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) — 社区 NVIDIA 分支。

## 文档

- [实测性能](docs/zh/performance.md)
- [如何运行](docs/zh/running.md)
- [硬件](docs/zh/hardware.md)
- [Wave 11 包含什么](docs/zh/technical-changes.md)
- [已知限制](docs/zh/known-limits.md)
- [安全性、校验与从源码构建](docs/zh/security-and-build.md)
- [分支与许可证](docs/zh/project.md)
