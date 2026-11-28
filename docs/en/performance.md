[Русский](../ru/performance.md) | [简体中文](../zh/performance.md) | [Español](../es/performance.md) | [Português (BR)](../pt-BR/performance.md)

# Performance

[Back to README](../../README.md)

Wave 11 on `main`.

## ~31 native / 62-63 FG

**Radeon RX 9060 XT + Ryzen 7 7800X3D**

| | |
|---|---|
| Native | ~31 |
| Frame generation | 62-63 FG |

`62-63 FG` is frame generation. Native is ~31.

## 8 GB Safe Mode

Preset `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` on `main` (PR #1, `nathanbzrr`). Measured on **Ryzen 5 5500X3D, Radeon RX 7600 8 GB, 32 GB DDR4, Windows 11**, 1080p, Safe Mode, FSR frame generation, 180 s:

| Metric | Before the 8 GB fit | Safe Mode |
|---|---:|---:|
| Native FPS | 3.67 | **12.7** |
| Frame generation FPS | 7.32 | **24.86** |
| Frame time | p99 1047.30 ms | mean 79.01 ms, p50 68.90 ms, p99 **242.26 ms** |
| VRAM peak | 6548 MB | **6169 MB** (live budget 5934–5981 MB, 0 errors, 0 OOM) |

This is the 8 GB card, not the 7800X3D result above.

## See also

- [How to run](running.md)
- [Hardware](hardware.md)
- [What Wave 11 ships](technical-changes.md)
- [Known limits](known-limits.md)
- [Security, checksums, and building](security-and-build.md)
- [Branches and license](project.md)
