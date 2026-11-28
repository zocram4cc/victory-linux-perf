[Русский](../ru/hardware.md) | [简体中文](../zh/hardware.md) | [Español](../es/hardware.md) | [Português (BR)](../pt-BR/hardware.md)

# Hardware

[Back to README](../../README.md)

| Question | Wave 11 |
|---|---|
| **Which AMD GPUs?** | **RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000). DX12 Ultimate and mesh shaders required. |
| **NVIDIA?** | Not in this build. An NVIDIA version is coming. AMD shipped first. Community fork: [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). A non-AMD adapter can black-screen or crash. |
| **Intel Arc?** | Not supported. |
| **Intel CPU + AMD GPU?** | Yes. The Wave 11 binary is `-C target-cpu=x86-64-v3`. |
| **8 GB / 6 GB / 4 GB VRAM?** | **8–12 GB:** Safe Mode (RX 7600: 12.7 native / 24.86 FG; also checked on an RX 6600). **6 GB and below:** not a supported configuration. **RDNA1 (RX 5500/5700) and GTX 10/16:** not supported (no mesh shaders, or not enough VRAM). |
| **16 GB RAM / HDD?** | **16 GB RAM:** yes, on an **NVMe or SATA SSD**, with a 24–32 GB pagefile (`pagefile_16gb_ram.ps1`) or preset `1_Low_Commit_NVMe_16GB.bat`. **HDD:** not supported. |
| **Leaked GDK?** | No. Standalone runtime, GPL-3.0-or-later. |
| **Logs?** | `collect_logs.bat` or **Export Logs** writes `logs\bug_report_*.txt` on this PC (paths stripped). |

## See also

- [Performance](performance.md)
- [How to run](running.md)
- [What Wave 11 ships](technical-changes.md)
- [Known limits](known-limits.md)
- [Security, checksums, and building](security-and-build.md)
- [Branches and license](project.md)
