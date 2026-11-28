[English](../en/performance.md) | [Русский](../ru/performance.md) | [简体中文](../zh/performance.md) | [Português (BR)](../pt-BR/performance.md)

# Rendimiento

[Volver al README](../../README.es.md)

Wave 11 en `main`.

## ~31 native / 62-63 FG

**Radeon RX 9060 XT + Ryzen 7 7800X3D**

| | |
|---|---|
| Nativo | ~31 |
| Frame generation | 62-63 FG |

`62-63 FG` es frame generation. El nativo es ~31.

## Safe Mode de 8 GB

Preset `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` en `main` (PR #1, `nathanbzrr`). Medido en **Ryzen 5 5500X3D, Radeon RX 7600 8 GB, 32 GB DDR4, Windows 11**, 1080p, Safe Mode, frame generation FSR, 180 s:

| Métrica | Antes del ajuste 8 GB | Safe Mode |
|---|---:|---:|
| FPS nativos | 3,67 | **12,7** |
| FPS con frame generation | 7,32 | **24,86** |
| Tiempo de cuadro | p99 1047,30 ms | media 79,01 ms, p50 68,90 ms, p99 **242,26 ms** |
| Pico de VRAM | 6548 MB | **6169 MB** (presupuesto vivo 5934–5981 MB, 0 errores, 0 OOM) |

Es la tarjeta de 8 GB, no el resultado del 7800X3D de arriba.

## Ver también

- [Cómo ejecutarlo](running.md)
- [Hardware](hardware.md)
- [Qué trae Wave 11](technical-changes.md)
- [Límites conocidos](known-limits.md)
- [Seguridad, checksums y compilación](security-and-build.md)
- [Ramas y licencia](project.md)
