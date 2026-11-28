[English](../en/known-limits.md) | [Русский](../ru/known-limits.md) | [简体中文](../zh/known-limits.md) | [Español](../es/known-limits.md)

# Limites conhecidos

[Voltar ao README](../../README.pt-BR.md)

Limites da **Wave 11** na `main`.

- **Ray tracing está desligado.** O construtor de estruturas não roda nesta versão. O launcher não o liga.
- **A primeira execução compila pipelines** que ainda não estão em cache, cerca de 1–3 minutos de engasgos. Feche, rode `warm_shaders.bat` uma vez e abra de novo pelo launcher.

O hardware que esta build não executa está em [Hardware](hardware.md): NVIDIA (uma versão está a caminho; a AMD saiu primeiro), Intel Arc, RDNA1, GTX 10/16, placas de 4 GB e HDD.

## Veja também

- [Desempenho](performance.md)
- [Como executar](running.md)
- [Hardware](hardware.md)
- [O que a Wave 11 traz](technical-changes.md)
- [Segurança, checksums e compilação](security-and-build.md)
- [Branches e licença](project.md)
