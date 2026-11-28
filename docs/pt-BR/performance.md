[English](../en/performance.md) | [Русский](../ru/performance.md) | [简体中文](../zh/performance.md) | [Español](../es/performance.md)

# Desempenho

[Voltar ao README](../../README.pt-BR.md)

Wave 11 na `main`.

## ~31 native / 62-63 FG

**Radeon RX 9060 XT + Ryzen 7 7800X3D**

| | |
|---|---|
| Nativo | ~31 |
| Frame generation | 62-63 FG |

`62-63 FG` é frame generation. O nativo é ~31.

## Safe Mode de 8 GB

Preset `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` na `main` (PR #1, `nathanbzrr`). Medido em **Ryzen 5 5500X3D, Radeon RX 7600 8 GB, 32 GB DDR4, Windows 11**, 1080p, Safe Mode, frame generation FSR, 180 s:

| Métrica | Antes do ajuste de 8 GB | Safe Mode |
|---|---:|---:|
| FPS nativos | 3,67 | **12,7** |
| FPS com frame generation | 7,32 | **24,86** |
| Tempo de quadro | p99 1047,30 ms | média 79,01 ms, p50 68,90 ms, p99 **242,26 ms** |
| Pico de VRAM | 6548 MB | **6169 MB** (orçamento vivo 5934–5981 MB, 0 erros, 0 OOM) |

É a placa de 8 GB, não o resultado do 7800X3D acima.

## Veja também

- [Como executar](running.md)
- [Hardware](hardware.md)
- [O que a Wave 11 traz](technical-changes.md)
- [Limites conhecidos](known-limits.md)
- [Segurança, checksums e compilação](security-and-build.md)
- [Branches e licença](project.md)
