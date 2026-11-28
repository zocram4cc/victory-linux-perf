[English](../en/technical-changes.md) | [Русский](../ru/technical-changes.md) | [简体中文](../zh/technical-changes.md) | [Español](../es/technical-changes.md)

# O que a Wave 11 traz

[Voltar ao README](../../README.pt-BR.md)

Wave 11 é o binário da `main` (`RAZDA4A_XEN.exe`, x86-64-v3).

- **Árvores, folhagem, cercas, postes e placas aparecem.** As views raw desalinhadas em `LIST_VIEWS` estão corrigidas, inclusive `SW_64KB_D`. A publicação da Wave 11 registra zero quedas por pressão de VRAM na prova de estabilidade.
- **Frame generation e upscale.** FSR 3.1.6 usa uma captura RGBA8 sem HUD de 3 slots e uma swapchain com tearing. O upscale é FSR 4.1.2 ML, com FSR 3.1 de reserva. Os rostos usam a espera de compute de 5000 ms do launcher.
- **8 GB de VRAM.** Safe Mode (65% de texturas, 2048 MB de margem para targets, 768 MB de buffers) é o preset 4 e o caminho automático do launcher em placas de 8 GB.
- **Memória.** Recuperação limitada, `recycle_landed` e degraus DXGI (8/12/16/24 GB). Páginas em NVMe com `VICTORY_FILE_PHYSICAL`.
- **Controles.** `F2` ou `~` para o mouse, `F3` para o FPS. O escudo do shader RDNA4 `240cb516c1af0bc3` está ativo.
- **Windows 10.** `D3D12Core.dll` (Agility SDK 620) vem na árvore.

O binário é x86-64-v3: AVX2, BMI2 e FMA em AMD Zen 2–5 e Intel Haswell até Arrow Lake.

## Veja também

- [Desempenho](performance.md)
- [Como executar](running.md)
- [Hardware](hardware.md)
- [Limites conhecidos](known-limits.md)
- [Segurança, checksums e compilação](security-and-build.md)
- [Branches e licença](project.md)
