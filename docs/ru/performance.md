[English](../en/performance.md) | [简体中文](../zh/performance.md) | [Español](../es/performance.md) | [Português (BR)](../pt-BR/performance.md)

# Замеры

[К README](../../README.ru.md)

Wave 11 на `main`.

## ~31 native / 62-63 FG

**Radeon RX 9060 XT + Ryzen 7 7800X3D**

| | |
|---|---|
| Нативно | ~31 |
| Frame generation | 62-63 FG |

`62-63 FG` — это frame generation. Нативно — ~31.

## Safe Mode для 8 ГБ

Пресет `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` на `main` (PR #1, `nathanbzrr`). Замер на **Ryzen 5 5500X3D, Radeon RX 7600 8 ГБ, 32 ГБ DDR4, Windows 11**, 1080p, Safe Mode, FSR frame generation, 180 с:

| Метрика | До подгонки 8 ГБ | Safe Mode |
|---|---:|---:|
| Нативный FPS | 3.67 | **12.7** |
| FPS с frame generation | 7.32 | **24.86** |
| Время кадра | p99 1047.30 мс | среднее 79.01 мс, p50 68.90 мс, p99 **242.26 мс** |
| Пик VRAM | 6548 МБ | **6169 МБ** (живой бюджет 5934–5981 МБ, 0 ошибок, 0 OOM) |

Это карта на 8 ГБ, не результат 7800X3D выше.

## См. также

- [Как запускать](running.md)
- [Железо](hardware.md)
- [Что входит в Wave 11](technical-changes.md)
- [Известные ограничения](known-limits.md)
- [Безопасность, контрольные суммы и сборка](security-and-build.md)
- [Ветки и лицензия](project.md)
