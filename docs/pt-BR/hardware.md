[English](../en/hardware.md) | [Русский](../ru/hardware.md) | [简体中文](../zh/hardware.md) | [Español](../es/hardware.md)

# Hardware

[Voltar ao README](../../README.pt-BR.md)

| Pergunta | Wave 11 |
|---|---|
| **Quais AMD?** | **RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000). Exige DX12 Ultimate e mesh shaders. |
| **NVIDIA?** | Não nesta build. A versão NVIDIA está a caminho. A AMD saiu primeiro. Fork da comunidade: [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). Um adaptador que não seja AMD pode ficar preto ou fechar. |
| **Intel Arc?** | Não suportado. |
| **CPU Intel + GPU AMD?** | Sim. O binário da Wave 11 é `-C target-cpu=x86-64-v3`. |
| **8 GB / 6 GB / 4 GB de VRAM?** | **8–12 GB:** Safe Mode (RX 7600: 12,7 nativos / 24,86 FG; também verificado numa RX 6600). **6 GB ou menos:** não é uma configuração suportada. **RDNA1 (RX 5500/5700) e GTX 10/16:** não suportados (sem mesh shaders, ou pouca VRAM). |
| **16 GB de RAM / HDD?** | **16 GB de RAM:** sim, em **SSD NVMe ou SATA**, com pagefile de 24–32 GB (`pagefile_16gb_ram.ps1`) ou o preset `1_Low_Commit_NVMe_16GB.bat`. **HDD:** não suportado. |
| **GDK vazado?** | Não. Runtime autônomo, GPL-3.0-or-later. |
| **Logs?** | `collect_logs.bat` ou **Export Logs** grava `logs\bug_report_*.txt` neste PC (sem caminhos pessoais). |

## Veja também

- [Desempenho](performance.md)
- [Como executar](running.md)
- [O que a Wave 11 traz](technical-changes.md)
- [Limites conhecidos](known-limits.md)
- [Segurança, checksums e compilação](security-and-build.md)
- [Branches e licença](project.md)
