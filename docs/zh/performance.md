[English](../en/performance.md) | [Русский](../ru/performance.md) | [Español](../es/performance.md) | [Português (BR)](../pt-BR/performance.md)

# 实测性能

[返回 README](../../README.zh.md)

`main` 上的 Wave 11。

## ~31 native / 62-63 FG

**Radeon RX 9060 XT + Ryzen 7 7800X3D**

| | |
|---|---|
| 原生 | ~31 |
| 帧生成 | 62-63 FG |

`62-63 FG` 是帧生成。原生是 ~31。

## 8 GB Safe Mode

`main` 上的预设 `presets\4_Safe_Mode_8GB_12GB_VRAM.bat`（PR #1，`nathanbzrr`）。测试平台：**Ryzen 5 5500X3D、Radeon RX 7600 8 GB、32 GB DDR4、Windows 11**，1080p，Safe Mode，FSR 帧生成，180 秒：

| 指标 | 8 GB 适配前 | Safe Mode |
|---|---:|---:|
| 原生 FPS | 3.67 | **12.7** |
| 帧生成 FPS | 7.32 | **24.86** |
| 帧时间 | p99 1047.30 ms | 均值 79.01 ms，p50 68.90 ms，p99 **242.26 ms** |
| 显存峰值 | 6548 MB | **6169 MB**（实时预算 5934–5981 MB，0 错误，0 OOM） |

这是 8 GB 显卡，不是上面的 7800X3D 结果。

## 另见

- [如何运行](running.md)
- [硬件](hardware.md)
- [Wave 11 包含什么](technical-changes.md)
- [已知限制](known-limits.md)
- [安全性、校验与从源码构建](security-and-build.md)
- [分支与许可证](project.md)
