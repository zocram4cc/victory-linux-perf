[English](../en/hardware.md) | [Русский](../ru/hardware.md) | [Español](../es/hardware.md) | [Português (BR)](../pt-BR/hardware.md)

# 硬件

[返回 README](../../README.zh.md)

| 问题 | Wave 11 |
|---|---|
| **哪些 AMD？** | **RDNA2 / RDNA3 / RDNA4**（RX 6000 / 7000 / 9000）。需要 DX12 Ultimate 和 mesh shaders。 |
| **NVIDIA？** | 这个版本没有。NVIDIA 版本正在到来。AMD 先完成。社区分支：[`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA)。非 AMD 适配器可能黑屏或崩溃。 |
| **Intel Arc？** | 不支持。 |
| **Intel CPU + AMD 显卡？** | 可以。Wave 11 二进制使用 `-C target-cpu=x86-64-v3`。 |
| **8 GB / 6 GB / 4 GB 显存？** | **8–12 GB：** Safe Mode（RX 7600：12.7 native / 24.86 FG；RX 6600 也验证过）。**6 GB 及以下：** 不是受支持的配置。**RDNA1（RX 5500/5700）和 GTX 10/16：** 不支持（没有 mesh shaders，或显存不够）。 |
| **16 GB 内存 / HDD？** | **16 GB 内存：** 可以，需 **NVMe 或 SATA 固态硬盘**，以及 24–32 GB 页面文件（`pagefile_16gb_ram.ps1`）或预设 `1_Low_Commit_NVMe_16GB.bat`。**HDD：** 不支持。 |
| **泄露的 GDK？** | 不需要。独立运行层，GPL-3.0-or-later。 |
| **日志？** | `collect_logs.bat` 或 **Export Logs** 在本机写出 `logs\bug_report_*.txt`（已去掉私人路径）。 |

## 另见

- [实测性能](performance.md)
- [如何运行](running.md)
- [Wave 11 包含什么](technical-changes.md)
- [已知限制](known-limits.md)
- [安全性、校验与从源码构建](security-and-build.md)
- [分支与许可证](project.md)
