[English](../en/technical-changes.md) | [Русский](../ru/technical-changes.md) | [简体中文](../zh/technical-changes.md) | [Português (BR)](../pt-BR/technical-changes.md)

# Qué trae Wave 11

[Volver al README](../../README.es.md)

Wave 11 es el binario de `main` (`RAZDA4A_XEN.exe`, x86-64-v3).

- **Árboles, follaje, vallas, postes y señales se dibujan.** Las vistas raw no alineadas de `LIST_VIEWS` están corregidas, incluido `SW_64KB_D`. La publicación de Wave 11 registra cero caídas por presión de VRAM en su prueba de estabilidad.
- **Frame generation y escalado.** FSR 3.1.6 usa una captura RGBA8 sin HUD de 3 ranuras y una swapchain con tearing. El escalado es FSR 4.1.2 ML, con FSR 3.1 de reserva. Los rostros usan la espera de compute de 5000 ms del lanzador.
- **8 GB de VRAM.** Safe Mode (65% de texturas, 2048 MB de margen para targets, 768 MB de búferes) es el preset 4 y la vía automática del lanzador en tarjetas de 8 GB.
- **Memoria.** Reclamo acotado, `recycle_landed` y escalones DXGI (8/12/16/24 GB). Páginas en NVMe con `VICTORY_FILE_PHYSICAL`.
- **Controles.** `F2` o `~` para el ratón, `F3` para el FPS. El escudo del shader de RDNA4 `240cb516c1af0bc3` está activo.
- **Windows 10.** `D3D12Core.dll` (Agility SDK 620) viene en el árbol.

El binario es x86-64-v3: AVX2, BMI2 y FMA en AMD Zen 2–5 e Intel Haswell hasta Arrow Lake.

## Ver también

- [Rendimiento](performance.md)
- [Cómo ejecutarlo](running.md)
- [Hardware](hardware.md)
- [Límites conocidos](known-limits.md)
- [Seguridad, checksums y compilación](security-and-build.md)
- [Ramas y licencia](project.md)
