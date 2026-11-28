[English](../en/running.md) | [Русский](../ru/running.md) | [Español](../es/running.md) | [Português (BR)](../pt-BR/running.md)

# 如何运行

[返回 README](../../README.zh.md)

请通过提供的启动器启动：**`RAZDA4A_XEN.exe`** 或 **`RAZDA4A XEN.bat`**。不要直接运行游戏可执行文件。只有启动器拉起游戏时，运行层才会生效。

本仓库不包含任何游戏文件、资源或可执行文件，请自行提供 *星际工作室的伟大借车模拟器* 的文件。

## 快速开始（Windows）

1. 下载 `main` 分支（**Code -> Download ZIP** 或 `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`）并解压。内置 `D3D12\D3D12Core.dll` 与 `fidelityfx\*.dll`。
2. 使用 **`RAZDA4A_XEN.exe`** 或 **`RAZDA4A XEN.bat`** 启动。不要双击游戏 `.exe`。
   - **`RAZDA4A XEN.bat`** 会打开启动器窗口：点击 **Browse...** 选择 **`your_executable.exe`**（以 UTF-8 自动保存到 `config.bat`，支持中文路径），选择配置与 Frame Generation 开关后点击 **Launch**。
   - 第三方启动器 / 命令行：配置好 `config.bat` 后可运行 **`RAZDA4A XEN.bat --no-gui`**。

| 文件 | 说明 |
|---|---|
| `RAZDA4A XEN.bat` | 图形启动器（选择 `.exe`、FG 开关、8 GB 自动 Safe Mode、Warm Shaders 与 Export Logs） |
| `RAZDA4A_XEN.exe` | 启动器拉起的运行时。不要跳过它去直接运行游戏可执行文件。 |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB 内存 / 小页面文件：NVMe 文件物理页（`VICTORY_FILE_PHYSICAL=1`） |
| `presets\2_Max_FPS_Experimental_CBV.bat` | 均衡 + 实验性原地 CBV（`VICTORY_PORT_CBV=1`） |
| `presets\3_Native_No_FG_Zero_Latency.bat` | 纯原生渲染，关闭帧生成，最低延迟 |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB 显存 Safe Mode（`65%` / `2048 MB` 目标 / `768 MB` 缓冲，开启 VSync） |
| `warm_shaders.bat` | 首次游玩结束后运行一次，预编译已遇到的 PSO |
| `collect_logs.bat` | 在本机写出脱敏硬件与 `logs\last_run.log` 报告（`logs\bug_report_*.txt`）。 |

快捷键：**`F2` / `~`** 切换鼠标视角（`Esc` 释放光标），**`F3`** 开关右上角 FPS 面板。

## 快速开始（Linux — Proton / Wine / VKD3D-Proton）

1. 在 AMD RDNA2+ 显卡与 **Mesa RADV** 环境下安装 **Proton Experimental / GE-Proton**（`umu-run`、Steam 或 Lutris）。
2. 运行 `launch_linux_proton.sh`（自动设置 `WINEDLLOVERRIDES="d3d12,d3d12core=n"`，防止内置 Win10 `D3D12Core.dll` 覆盖 `vkd3d-proton` 导致 1 FPS 或无 3D 画面）。脚本会启动 `RAZDA4A_XEN.exe`。不要直接运行游戏可执行文件。
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```

## 另见

- [实测性能](performance.md)
- [硬件](hardware.md)
- [Wave 11 包含什么](technical-changes.md)
- [已知限制](known-limits.md)
- [安全性、校验与从源码构建](security-and-build.md)
- [分支与许可证](project.md)
