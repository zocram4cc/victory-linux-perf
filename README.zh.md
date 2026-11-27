[English](README.md) | [Русский](README.ru.md) | 简体中文 | [Español](README.es.md) | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

面向 **16 GB 内存**、**6 核 CPU**、**8–16 GB 显存显卡**与 **Windows 10** 的原生 Direct3D 12 运行层。

本仓库不包含任何游戏文件、资源或可执行程序，需自行提供*星辰大厂「伟大借车模拟器」*的本地副本。

## 与上游 VIctory 的差异

上游 VIctory 主要在 64 GB 内存、24 GB 显卡和 Windows 11（支持 `D3D12_HEAP_TYPE_GPU_UPLOAD`）上测试。在 16 GB 内存 + Windows 10 上运行会出现 `0x8007000E` (`E_OUTOFMEMORY`)、VSync 帧率腰斩、帧生成 UI 重影和 CPU 单核提交瓶颈。

实测平台：**Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 GB, 16 GB DDR4, Windows 10 (19045)** — 1080p Series S 配置，关闭光追，连续运行 180 秒，0 报错：

| 指标 | 基线（上游配置） | RAZDA4A XEN |
|---|---|---|
| Private Commit（稳定值） | 21.8 – 30+ GB (`0x8007000E` OOM) | **19.3 GB**（6.2 GB RAM WS，5.5 GB 空闲） |
| 独立显存 (VRAM) | 9.92 GB（`5.06 GB` 纹理池） | **8.18 GB**（`3.65 GB` 纹理池） |
| 共享 GPU 内存 (`NON_LOCAL`) | 6.68 GB | **5.45 GB** |
| Copy-Back 流量 | 27.0 MB / 帧 | **0.5 MB / 帧** |
| CPU `preparing` 耗时 | 45.8 ms | **32.2 – 35.4 ms** |
| 真实帧时间（最低 / 中位） | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| 原生 FPS（关闭 FG） | ~13.7 FPS | **14.5 – 15.5 FPS** |
| 显示 FPS（FSR 3.1.6 FG） | 25–27 FPS（VSync 掉至 15–20） | **29.2 – 31.0 FPS**（无 VSync 阶梯） |
| 帧生成 HUD / 字幕 | 重影与扭曲 | 无鬼影（`HUDLessColor` 3 槽 RGBA8） |
| Windows 10 D3D12 | 需手动配置 Agility SDK | 内置 `D3D12Core.dll`（Agility SDK 620） |

## 技术改动

- **内存与 OOM 防护 (`0x8007000E`)：** 全局有界 prelude 池 `(64, 16)`；命令列表与分配器即时回收 (`recycle_landed`) 与延迟资源释放 (`bury_lag`)；`make_list()` / `record_prelude()` 遇 `E_OUTOFMEMORY` 自动回收重试；`NON_LOCAL` 高水位节流（`92%`，`-10%` 迟滞）；可选 NVMe 文件映射物理页（`FILE_FLAG_DELETE_ON_CLOSE` 位于 `%LOCALAPPDATA%\victory\physical`，加载期保留 >11 GB 提交余量）。
- **呈现与帧生成：** UI 合成前 3 槽 RGBA8 `HUDLessColor` 捕获；三缓冲撕裂交换链（`DXGI_PRESENT_ALLOW_TEARING`）；原生 AMD FSR 4.1.2 ML 超分（旧卡自动回落 FSR 3.1）。
- **CPU 提交与回读优化：** write-combined 着色器写入保留在显存（`27 -> 0.5 MB/帧`）；128 字节 AVX2 `vptest` 快速跳过与向量化 `merge_piece`；无锁 work-stealing `land` 线程池；64 字节对齐 write-watch 分片；单遍融合 `ask_once`/`stale_under_flights`；无锁描述符 `mirror`；`OnceLock` 命令签名；栈分配 `Few<Barrier, 16>`；64 分片 `PLACED_TEXTURES`；`unfold` 解锁与无锁 `wake()`；锁外 `upload::ARENAS` 清零；6 核辅助线程优先级提升；thin 预规划；detile 前哈希校验；上传缓冲复用。
- **修复与输入：** 5000 ms compute 管线等待预算修复人物面部缺失；RDNA4 崩溃着色器屏蔽（`240cb516c1af0bc3`）；紫色异常探针过滤；安全异步描述符翻转；纹理 detile 失败重试；PC 键鼠控制（`F2`/`~` 鼠标视角，`F3` FPS 面板）。

## 当前已知限制（`dev` 分支正在处理）

- 原生 30 FPS（`<= 33.3 ms`）目前受限于单线程 `victory-submit`（`49–60 ms/帧`，GPU 占用仅 52–58%）。
- Series S 配置在 16 GB 显卡上仅用约 8.2 GB 显存；动态显存自适应预算正在 `dev` 开发中。
- 序章进入开放世界时默认虚拟内存可能触发 `0x8007000E`；无需修改虚拟内存的代码级修复正在 `dev` 开发中。

## 快速开始

1. 下载 `main`（**Code -> Download ZIP** 或 `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`）并解压（如 `D:\RAZDA4A-XEN\`）。依赖 DLL 已内置。
2. 编辑 **`config.bat`** 指向你的目录和 **`你的可执行文件.exe`**：
   ```bat
   set "GAME_DIR=D:\Path\To\Your\GameFolder"
   set "GAME_EXE=%GAME_DIR%\你的可执行文件.exe"
   ```
3. 运行 **`RAZDA4A XEN.bat`** 或选择 `presets\` 中的预设：

| 文件 | 说明 |
|---|---|
| `RAZDA4A XEN.bat` | 默认均衡：29–31 FG FPS，~8.2 GB 显存，6.2 GB 内存，HUDLess FG + Tearing |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 小虚拟内存 / 后台开浏览器：NVMe 物理页，~8.0 GB 显存，加载期 >11 GB 提交余量 |
| `presets\2_Max_FPS_Experimental_CBV.bat` | 均衡 + 原地常量缓冲（实验性） |
| `presets\3_Native_No_FG_Zero_Latency.bat` | 原生帧无帧生成，最低输入延迟 |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB 显存 / 安全模式：严格显存限制，开启 VSync，全量描述符扫描 |

- **`F2` / `~`** — 切换鼠标视角（`Esc` 解锁）
- **`F3`** — 开关 FPS 面板
- **`warm_shaders.bat`** — 首次试玩关闭后运行一次预编译着色器
- **`backup_saves.bat`** — 备份/恢复 `data\`
- **`pagefile_16gb_ram.ps1`** — 可选管理员脚本（设置 16–32 GB SSD 虚拟内存）

## 从源码编译与校验

如不信任未签名二进制，可直接从 `dev` 分支自行编译：

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --locked -p victory
```

预编译 `RAZDA4A_XEN.exe` SHA-256: `e90b23f05f6d3caa488beaf3afc4cd0e94151b75a33606bfd75594a373e4f18b`

## 分支与协议

- `main`：预编译二进制、运行 DLL、预设与文档。
- `dev`：完整 Rust 源码、单元测试、CI 与 `AGENTS.md`。

基于 VIctory，采用 **GPL-3.0-or-later** 协议（[LICENSE](LICENSE)）。完整源码位于 [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev) 分支。
