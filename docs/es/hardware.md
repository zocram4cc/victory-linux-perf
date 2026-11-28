[English](../en/hardware.md) | [Русский](../ru/hardware.md) | [简体中文](../zh/hardware.md) | [Português (BR)](../pt-BR/hardware.md)

# Hardware

[Volver al README](../../README.es.md)

| Pregunta | Wave 11 |
|---|---|
| **¿Qué AMD?** | **RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000). Hacen falta DX12 Ultimate y mesh shaders. |
| **¿NVIDIA?** | No en esta build. La versión NVIDIA está en camino. AMD salió primero. Fork de la comunidad: [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). Un adaptador que no sea AMD puede quedar en negro o cerrarse. |
| **¿Intel Arc?** | No soportado. |
| **¿CPU Intel + GPU AMD?** | Sí. El binario de Wave 11 es `-C target-cpu=x86-64-v3`. |
| **¿8 GB / 6 GB / 4 GB de VRAM?** | **8–12 GB:** Safe Mode (RX 7600: 12,7 nativos / 24,86 FG; también comprobado en una RX 6600). **6 GB o menos:** no es una configuración soportada. **RDNA1 (RX 5500/5700) y GTX 10/16:** no soportados (sin mesh shaders, o poca VRAM). |
| **¿16 GB de RAM / HDD?** | **16 GB de RAM:** sí, en **SSD NVMe o SATA**, con pagefile de 24–32 GB (`pagefile_16gb_ram.ps1`) o el preset `1_Low_Commit_NVMe_16GB.bat`. **HDD:** no soportado. |
| **¿GDK filtrado?** | No. Runtime autónomo, GPL-3.0-or-later. |
| **¿Registros?** | `collect_logs.bat` o **Export Logs** escribe `logs\bug_report_*.txt` en este PC (sin rutas privadas). |

## Ver también

- [Rendimiento](performance.md)
- [Cómo ejecutarlo](running.md)
- [Qué trae Wave 11](technical-changes.md)
- [Límites conocidos](known-limits.md)
- [Seguridad, checksums y compilación](security-and-build.md)
- [Ramas y licencia](project.md)
