[English](../en/hardware.md) | [简体中文](../zh/hardware.md) | [Español](../es/hardware.md) | [Português (BR)](../pt-BR/hardware.md)

# Железо

[К README](../../README.ru.md)

| Вопрос | Wave 11 |
|---|---|
| **Какие AMD?** | **RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000). Нужны DX12 Ultimate и mesh shaders. |
| **NVIDIA?** | В этой сборке нет. Версия для NVIDIA в пути. Сначала вышла AMD. Форк сообщества: [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). Не-AMD адаптер может дать чёрный экран или вылет. |
| **Intel Arc?** | Не поддерживается. |
| **Intel CPU + AMD GPU?** | Да. Бинарь Wave 11 собран с `-C target-cpu=x86-64-v3`. |
| **8 ГБ / 6 ГБ / 4 ГБ VRAM?** | **8–12 ГБ:** Safe Mode (RX 7600: 12.7 native / 24.86 FG; также проверено на RX 6600). **6 ГБ и меньше:** не поддерживаемая конфигурация. **RDNA1 (RX 5500/5700) и GTX 10/16:** не поддерживаются (нет mesh shaders или мало VRAM). |
| **16 ГБ ОЗУ / HDD?** | **16 ГБ ОЗУ:** да, на **NVMe или SATA SSD**, с файлом подкачки 24–32 ГБ (`pagefile_16gb_ram.ps1`) или пресетом `1_Low_Commit_NVMe_16GB.bat`. **HDD:** не поддерживается. |
| **Утёкший GDK?** | Нет. Автономный рантайм, GPL-3.0-or-later. |
| **Логи?** | `collect_logs.bat` или **Export Logs** пишет `logs\bug_report_*.txt` на этом ПК (без личных путей). |

## См. также

- [Замеры](performance.md)
- [Как запускать](running.md)
- [Что входит в Wave 11](technical-changes.md)
- [Известные ограничения](known-limits.md)
- [Безопасность, контрольные суммы и сборка](security-and-build.md)
- [Ветки и лицензия](project.md)
