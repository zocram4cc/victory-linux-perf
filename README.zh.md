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
| 独立显存 (VRAM) | 9.92 GB（`5.06 GB` 纹理池） | **12.11 GB**（`5.12 GB` 纹理池；91% keep 可达 `13.18 GB` / `6.40 GB` 纹理池；DXGI 8–24 GB 自适应） |
| 共享 GPU 内存 (`NON_LOCAL`) | 6.68 GB（Series S 基线 5.58 GB） | **2.18 GB**（较 Series S 基线 `-61%`；91% keep 下 `2.01 GB`） |
| Copy-Back 流量 | 27.0 MB / 帧 | **0.5 MB / 帧** |
| CPU `preparing` 耗时 | 45.8 ms | **32.2 – 35.4 ms** |
| 真实帧时间（最低 / 中位） | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| 原生 FPS 与帧稳（关闭 FG，开放世界稳态） | 14.08 FPS（均值 71.03 ms，p99 152.1 ms / 1% low 6.6 FPS） | **15.2 – 23.4 FPS**（p50 **60.4 ms** / 16.6 FPS，`list_al` **0.03 ms**，`alloc` **4.8 ms**，0 次 5–10 秒卡死，0 OOM / 0 emergency reclaim） |
| `heap buffers` 通道（峰值 / >50 ms 卡顿） | 821.1 ms（8 次 >50 ms） | **33.8 ms（0 次 >50 ms；稳态 8.6–10.0 ms）** |
| `settle` 等待与 `take()` 耗时（稳态无 FG） | 0.2–2.6 ms settle / 0.75–1.07 ms `take()` | **0.0 ms settle / 0.31–0.47 ms `take()`** |
| 显示 FPS（FSR 3.1.6 FG） | 25–27 FPS（VSync 掉至 15–20） | **29.2 – 31.0 FPS**（无 VSync 阶梯） |
| 帧生成 HUD / 字幕 | 重影与扭曲 | 无鬼影（`HUDLessColor` 3 槽 RGBA8） |
| Windows 10 D3D12 | 需手动配置 Agility SDK | 内置 `D3D12Core.dll`（Agility SDK 620） |

## 技术改动

- **内存与 OOM 防护 (`0x8007000E`)：** 全局有界 prelude 池 `(64, 16)`；命令列表与分配器即时回收 (`recycle_landed`) 与延迟资源释放 (`bury_lag`)；可用提交 <2.5 GiB 或 Shared GPU (`NON_LOCAL`) >=88% 时主动触发 `emergency_reclaim()`（激进 LRU 清理空闲纹理/staging/池并动态将 `thin` 并发录制限制为 1–2 线程）；`HostList::close()`、`make_list()` 与 `record_prelude()` 遇 `E_OUTOFMEMORY` 自动回收重试并安全重置/退役分配器；已释放未映射物理页调用 `MEM_RESET`；合入上游修复 `#2143`（常量表始于指令之后）、`#2234`（释放 view heap 时清理页表）、`#2462`（UAV `typed_store` 格式校验）、`#2844`（RAII `CpuViews` 描述符槽位）与 `#3540`（raw 缓冲视图 16 字节对齐）；可选 NVMe 文件映射物理页（`FILE_FLAG_DELETE_ON_CLOSE` 位于 `%LOCALAPPDATA%\victory\physical`，加载期保留 >11 GB 提交余量）。默认虚拟内存（`VICTORY_FILE_PHYSICAL=0`）180 秒原生无 FG 实测：0 错误、0 OOM、`min_free_commit 3.47 GB`、稳态可用提交 `5.10–5.57 GB`。
- **呈现与帧生成：** UI 合成前 3 槽 RGBA8 `HUDLessColor` 捕获；三缓冲撕裂交换链（`DXGI_PRESENT_ALLOW_TEARING`）；原生 AMD FSR 4.1.2 ML 超分（旧卡自动回落 FSR 3.1）。
- **CPU 提交与回读优化：** write-combined 着色器写入保留在显存（`27 -> 0.5 MB/帧`）；128 字节 AVX2 `vptest` 快速跳过与向量化 `merge_piece`；无锁 work-stealing `land` 线程池；64 字节对齐 write-watch 分片；单遍融合 `ask_once`/`stale_under_flights`；无锁描述符 `mirror`；`OnceLock` 命令签名；栈分配 `Few<Barrier, 16>`；64 分片 `PLACED_TEXTURES`；`unfold` 解锁与无锁 `wake()`；锁外 `upload::ARENAS` 清零；6 核辅助线程优先级提升；thin 预规划；detile 前哈希校验；上传缓冲复用。
- **动态 DXGI 自适应显存与共享显存削减：** 基于 `DedicatedVideoMemory` 与实时 `DXGI_QUERY_VIDEO_MEMORY_INFO` 自动分档 8 / 12 / 16 / 24+ GB 显卡（`VICTORY_VRAM_MODE=auto|8gb|12gb|16gb|24gb|legacy`，支持 `"auto"` 与全部手动环境变量覆写、UMA 识别与 Commit 余量阶梯保护）。16 GB 显卡按主机内存自适应：`<=20 GB` 内存采用 `84%` keep / `90%` room（`12.11 GB` VRAM、`2.18 GB` 共享内存，避免 Windows 工作集换页，1% low 提升 `+55%`），`>=32 GB` 内存解锁 `91%` keep / `95%` room（`~13.2–14.2 GB` VRAM）。零 staging 直连 VRAM 窗口（`CardCpu::Direct`）、紧凑 CPU 影子（`CardCpu::Cpu`）、`DIRTY_CARDED` O(dirty) 脏队列、带迟滞的渐进窗口淘汰（每秒最多 8 个）、深度模板共享 scratch（上游 `#2162`）、16 GB 内存 `host_pressure` 阈值修复（`send_allocs` `117/15s -> 0/15s`）、NVMe 自动检测（`VICTORY_FILE_PHYSICAL=auto`）及仅统计游戏本体的 `XMemGetWorkingSetStatistics`。
- **原生帧步调与卡顿消除（无 FG）：** 异步增量 `sweep()`（`victory-sweep`，`VICTORY_SWEEP_ASYNC=1`）移除每 1.5 秒的同步 `settle(WHY_SYNC)` 停顿；节流后台 `bury()` 回收线程（`victory-bury`，`VICTORY_BURY_ASYNC=1`）；relay 下基于 `Line.seq` 的无锁 `WHY_FENCE`/`WHY_VALUE` 等待；`execute_own` 呈现跳过冗余 `sync_heaps`（`0.0` 无标记轮次）；`port_descriptors::take()` 64 位 `AtomicU64` 扫描（提速 2–3×）；高精度 `frame::wait` 避免 `Sleep(0)` 让出；内置逐帧 `present pacing` 遥测（`avg / p50 / p95 / p99 / 1% low / max / >50 ms / >100 ms`，`VICTORY_FRAME_TIMES_FILE`）与平坦化 `heap buffers`（`VICTORY_TEXTURE_STAGE_MS=4`，`VICTORY_TEXTURE_CATCHUP=0`，0 次 >50 ms 尖峰）。
- **第二阶段 CPU 提交与描述符优化（`feat/phase2-30fps`）：** `card_steps` 跨提交区间粒度失效 `VirtualQuery` `MappedCache`（`12.56 ms / 36 次 -> 0.00 ms / 0 次`，`own_cs` `15.1 -> 2.5 ms`）；无锁逐槽原子代号表 `ENTRY_GENS` 与 8192 槽 TLS `descriptor::VIEWS` / `words_for` 缓存（`100.0%` 命中率）；`draw_bind::tables` 内联 CBV 快速路径、128 槽动态描述符 slab 分配器与批量 `release_dynamic_batch` 及每线程 `RETIRE_TLS`；零重建列表回收（`BACKS_MOST=512`、`IDLE_BYTES=512 MiB`、`list::boxed()` → `fresh_list()`，`list_al` `-99.7%`、`alloc` `-83%`、`lists` churn `+0`）；增量 `BufferSweep` 与 `heap_buffer_pass` 跳过未变更存活槽位（`HBP_SPANS` `10.94 -> 0.51 ms`）；扩容 `PRELUDES_MOST` `(512, 128)`、`SPARE_BATCH` `(128/256)`、`send::SPARE` `(256/128 MiB)` 与 `VICTORY_LIST_IDLE_MOST=1024`；compute PSO 优先队列 + 4 个 compute/screen 专用线程与后期等待上限（`VICTORY_COMPUTE_LATE_WAIT_MS=400`，消除 5–10 秒游戏卡死，`p99.9` `-80%`，`1% low` `3.2×`）；16 GB 内存 `residency` 阈值修复（`unmap_null` `-97%`，`VICTORY_KEEP_SHOWN=2`）；64 KiB 分页粒度 `gpu_written_pages` 位图；右上角 HUD（`VICTORY_PANEL_POS=top-right`）与内置 Frame Generation 开关勾选框。
- **修复与输入：** 5000 ms compute 管线等待预算修复人物面部缺失；RDNA4 崩溃着色器屏蔽（`240cb516c1af0bc3`）；紫色异常探针过滤；安全异步描述符翻转；纹理 detile 失败重试；PC 键鼠控制（`F2`/`~` 鼠标视角，`F3` 右上角 FPS 面板）。

## 当前已知限制（`dev` 分支正在处理）

- 原生 30 FPS（`<= 33.3 ms`）正在 `dev` 持续推进：`victory-submit` 已降至 `34.4 ms/帧`，下一阶段重点优化 4 线程命令录制（`victory-list-0..3`）与 CBV 拷贝。
- 序章进入开放世界在极小虚拟内存下的提交峰值已由 `dev` 中的 `emergency_reclaim()` 与 NVMe 自动检测（`VICTORY_FILE_PHYSICAL=auto`）缓解。

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

预编译 `RAZDA4A_XEN.exe` SHA-256: `7fcb4ba60b6dee7a6b71b6e110cf01cd86c5576c38bfcbe707a618c6e8f37c5d`

## 分支与协议

- `main`：预编译二进制、运行 DLL、预设与文档。
- `dev`：完整 Rust 源码、单元测试、CI 与 `AGENTS.md`。

基于 VIctory，采用 **GPL-3.0-or-later** 协议（[LICENSE](LICENSE)）。完整源码位于 [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev) 分支。
