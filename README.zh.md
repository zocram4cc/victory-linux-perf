[English](README.md) | [Русский](README.ru.md) | 简体中文 | [Español](README.es.md) | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

面向 **AMD RDNA2 / RDNA3 / RDNA4 显卡** 的 Direct3D 12 Windows 运行层，针对 **16 GB 内存**、**6 核 CPU**、**8–16 GB 显存** 和 **Windows 10 / 11**（以及 Linux Proton / VKD3D-Proton）优化。

本仓库不包含任何游戏文件、资源或可执行文件，请自行提供 *星际工作室的伟大借车模拟器* 的文件。

## 实测性能（原生 FPS 与帧生成分开标示）

以下数据均严格区分 **原生 FPS（关闭帧生成）** 与 **显示 FPS（FSR 3.1.6 帧生成 2x）**，并注明场景与硬件配置。

### 16 GB 显存 / 16 GB 内存基准（开放世界，1080p Series S，关闭光追）

测试平台：**Ryzen 5 5600 (6C/12T)、Radeon RX 9060 XT 16 GB、16 GB DDR4、Windows 10 (19045)** — 180 秒连续开放世界测试 + Wave 11 22 分钟稳定性测试（18,393 帧），0 错误，0 OOM：

| 指标 | 基线（上游配置） | RAZDA4A XEN（Wave 7 / Wave 11） |
|---|---|---|
| **原生 FPS（关闭 FG，开放世界）** | 平均 14.08（71.03 ms，p99 152.1 ms / 1% low 6.6 FPS） | **p50 18.19**（54.97 ms），平均 **16.17**，稳态 15.2–23.4，p99.9 **478.6 ms** |
| **显示 FPS（FSR 3.1.6 FG 2x）** | 25–27 FPS（VSync 掉至 15–20） | **29.2 – 31.0 FPS** 稳定（HUDLess 3 槽 RGBA8 环，无 VSync 减半） |
| 私有提交内存（稳态） | 21.8 – 30+ GB（`0x8007000E` OOM） | **19.3 GB**（6.2 GB RAM WS，5.5 GB 可用提交） |
| 专用显存 VRAM | 9.92 GB（纹理池 5.06 GB） | **12.11 GB**（8–24 GB DXGI 自动档位；Wave 11 22 分钟 0 次显存拒绝） |
| 共享显存（`NON_LOCAL`） | 6.68 GB（Series S 5.58 GB） | **2.18 GB**（较 Series S 基线下降 61%） |
| 回拷流量 | 27.0 MB / 帧 | **0.5 MB / 帧** |
| CPU `preparing` / `heap buffers` | 45.8 ms / 峰值 821.1 ms | **32.2 – 35.4 ms** / **峰值 33.8 ms**（0 次 >50 ms 尖峰） |
| 原始缓冲视图（`LIST_VIEWS`，Wave 11） | 每帧 372.1–575.8 次 null（树木/道具消失） | **每帧 0.0 次 null**（每帧 28,212 次；树木、树叶、围栏、木桩与路牌已修复） |
| Windows 10 D3D12 | 需手动配置 Agility SDK | 内置 `D3D12Core.dll`（Agility SDK 620） |

### 8 GB 显存 Safe Mode 实测（PR #1，作者 `nathanbzrr`）

测试平台：**Ryzen 5 5500X3D、Radeon RX 7600 8 GB、32 GB DDR4、Windows 11 (26340)** — 1080p Series S，Safe Mode（`65%` 纹理空间 / `2048 MB` 渲染目标 / `768 MB` 显存缓冲），FSR FG 2x，180 秒连续测试：

| 指标 | 优化前 | Safe Mode 实时预算适配后 |
|---|---:|---:|
| **原生 FPS（不含 FG）** | 3.67 | **12.7**（均值 79.01 ms，p50 68.90 ms，p99 **242.26 ms** 对比 1047.30 ms） |
| **显示 FPS（FSR FG 2x）** | 7.32 | **24.86**（渲染 1803 帧 / 生成 1803 帧） |
| 峰值 VRAM / 实时预算 | 6548 MB | **6169 MB** / 5934–5981 MB（0 错误、0 OOM、0 设备丢失） |

## 核心技术改进

- **Wave 11（植被/道具与跨平台 CPU 构建）：** 修复 `LIST_VIEWS` 中每帧 `372–575` 个未对齐原始缓冲视图被强制置 `NULL` 的问题（解决树木、树叶、围栏、木桩和路牌消失）；支持 `SW_64KB_D` 布局；优化 16 GB 显存驻留（22 分钟 0 次显存压力拒绝）；改为通用 **`x86-64-v3`** 发布构建（同时兼容 AMD Zen 2–5 与 Intel Haswell–Arrow Lake）。
- **Wave 6–7（纹理、描述符与稳定性）：** 描述符 HashMap + 8192 槽 TLS 缓存（100% 命中）、最优适配显存池、交错纹理队列（过场积压 `3936 -> 0` 帧）、16 GB 内存滞回、正常 CPU 优先级（不拖死后台应用）以及 `VICTORY_LOG_FILE` 日志落盘。
- **内存与 OOM（`0x8007000E`）：** 有界前导命令池、`recycle_landed` 即时回收、主动 `emergency_reclaim()`、`E_OUTOFMEMORY` 重试恢复、`MEM_RESET`、动态 DXGI Auto-VRAM（8/12/16/24 GB）以及可选 NVMe 文件物理页（`VICTORY_FILE_PHYSICAL=auto`）。
- **画面与键鼠：** 3 槽 RGBA8 `HUDLessColor` 帧生成捕获、允许撕裂交换链、原生 AMD FSR 4.1.2 ML 超分（旧卡回退 FSR 3.1）、`5000 ms` compute 等待（修复角色面部）、RDNA4 崩溃着色器屏蔽（`240cb516c1af0bc3`）、PC 键鼠控制（`F2`/`~` 鼠标视角，`F3` 右上角 FPS 面板）。

## 当前已知限制（`dev` 分支持续推进）

- 密集城区在长时间运行下原生 FPS 仍可能降至 `5–10` 并出现描述符表长卡顿（正在 `dev` 的 **Wave 12** 中修复）。
- 光线追踪（RT）暂未开启，当前优先稳定基础渲染、显存预算与原生 30 FPS。
- 首次启动需要编译 PSO 着色器（约 1–3 分钟卡顿）；关闭后运行 `warm_shaders.bat`，第二次启动会顺畅得多。

## 硬件兼容性与常见问题 (FAQ)

| 问题 | 说明 |
|---|---|
| **支持哪些 AMD 显卡？** | **AMD RDNA2 / RDNA3 / RDNA4**（RX 6000 / 7000 / 9000 系列，需硬件 DX12 Ultimate + Mesh Shaders）。 |
| **支持 NVIDIA / Intel Arc 吗？** | **本分支不支持。** RTX 显卡会出现黑屏或崩溃；请使用社区 NVIDIA 分支，如 [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA)。 |
| **Intel CPU + AMD 显卡可以吗？** | **可以。** Wave 11 起使用通用 `-C target-cpu=x86-64-v3` 编译。 |
| **8 GB / 6 GB / <=4 GB 显存？** | **8 GB：** 使用 **Safe Mode** 可运行（RX 7600 实测 12.7 原生 / 24.86 FG，RX 6600 已验证）。**6 GB：** 实验性/较卡顿（Safe Mode + 关闭 FG）。**<=4 GB、RDNA1（RX 5500/5700）、GTX 10/16：** 不支持（缺少 Mesh Shaders 或显存不足）。 |
| **16 GB 内存 / 机械硬盘 HDD？** | **16 GB 内存：** 需搭配 **NVMe/SATA 固态硬盘** 与 24–32 GB 页面文件（`pagefile_16gb_ram.ps1`）或 `Low-Commit NVMe` 预设。**HDD：** 不支持（流式加载卡死）。 |
| **需要泄露的 GDK 吗？** | **不需要。** 完全独立的开源运行层（`GPL-3.0-or-later`）。 |
| **如何反馈 Bug 与日志？** | 运行 **`collect_logs.bat`**（或点击启动器 **Export Logs**）生成已脱敏的 `logs\bug_report_*.txt`，附在 [GitHub Issues](https://github.com/crossducking/RAZDA4A-XEN/issues) 中即可。 |

## 快速开始（Windows）

1. 下载 `main` 分支（**Code -> Download ZIP** 或 `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`）并解压。内置 `D3D12\D3D12Core.dll` 与 `fidelityfx\*.dll`。
2. 运行 **`RAZDA4A XEN.bat`**，点击 **Browse...** 选择 **`your_executable.exe`**（以 UTF-8 自动保存到 `config.bat`，支持中文路径），选择配置与 Frame Generation 开关后点击 **Launch**。
   - 第三方启动器 / 命令行：配置好 `config.bat` 后可运行 **`RAZDA4A XEN.bat --no-gui`**。

| 文件 | 说明 |
|---|---|
| `RAZDA4A XEN.bat` | 图形启动器（选择 `.exe`、FG 开关、8 GB 自动 Safe Mode、Warm Shaders 与 Export Logs） |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB 内存 / 小页面文件：NVMe 文件物理页（`VICTORY_FILE_PHYSICAL=1`） |
| `presets\2_Max_FPS_Experimental_CBV.bat` | 均衡 + 实验性原地 CBV（`VICTORY_PORT_CBV=1`） |
| `presets\3_Native_No_FG_Zero_Latency.bat` | 纯原生渲染，关闭帧生成，最低延迟 |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB 显存 Safe Mode（`65%` / `2048 MB` 目标 / `768 MB` 缓冲，开启 VSync） |
| `warm_shaders.bat` | 首次游玩结束后运行一次，预编译已遇到的 PSO |
| `collect_logs.bat` | 一键生成脱敏硬件与 `logs\last_run.log` 报错报告 |

快捷键：**`F2` / `~`** 切换鼠标视角（`Esc` 释放光标），**`F3`** 开关右上角 FPS 面板。

## 快速开始（Linux — Proton / Wine / VKD3D-Proton）

1. 在 AMD RDNA2+ 显卡与 **Mesa RADV** 环境下安装 **Proton Experimental / GE-Proton**（`umu-run`、Steam 或 Lutris）。
2. 运行 `launch_linux_proton.sh`（自动设置 `WINEDLLOVERRIDES="d3d12,d3d12core=n"`，防止内置 Win10 `D3D12Core.dll` 覆盖 `vkd3d-proton` 导致 1 FPS 或无 3D 画面）：
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```

## 安全性、杀软误报与从源码构建

浏览器（Brave/Edge）与 Windows Defender 机器学习模型可能会将含 `.exe` + `.bat` + `.ps1` 的未签名压缩包误报为 **`Trojan:Script/Wacatac.H!ml`**。请勿盲目信任未签名文件——请校验 SHA-256 或直接从 100% 开源的 `dev` 分支自行编译：

```powershell
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe`（`x86-64-v3`）SHA-256：`4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`（所有 DLL 校验见 [`SHA256SUMS.txt`](SHA256SUMS.txt)）。

从源码构建（`dev` 分支，MSVC Rust 工具链）：

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## 分支与许可证

- `main`：预编译二进制、运行库 DLL、预设、启动器、校验和与文档。
- `dev`：完整 Rust 源码、单元测试、`xtask policy` 审计、CI 与 `AGENTS.md`。

基于 VIctory 在 **GPL-3.0-or-later**（[LICENSE](LICENSE)）下派生，完整源码见 [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev)。
