[English](../en/technical-changes.md) | [简体中文](../zh/technical-changes.md) | [Español](../es/technical-changes.md) | [Português (BR)](../pt-BR/technical-changes.md)

# Что входит в Wave 11

[К README](../../README.ru.md)

Wave 11 — бинарь на `main` (`RAZDA4A_XEN.exe`, x86-64-v3).

- **Деревья, листва, заборы, столбы и знаки рисуются.** Невыровненные raw-view в `LIST_VIEWS` исправлены, включая `SW_64KB_D`. В заметке к релизу Wave 11 на прогоне стабильности нет просадок из-за давления VRAM.
- **Frame generation и апскейл.** FSR 3.1.6 использует 3-слотовый RGBA8-захват без HUD и swapchain с tearing. Апскейл — FSR 4.1.2 ML, запасной путь — FSR 3.1. Лица используют ожидание compute 5000 мс из лаунчера.
- **8 ГБ VRAM.** Safe Mode (65% текстур, 2048 МБ запаса под таргеты, 768 МБ буферов) — пресет 4 и автоматический путь лаунчера на картах 8 ГБ.
- **Память.** Ограниченный возврат, `recycle_landed` и ступени DXGI (8/12/16/24 ГБ). Страницы на NVMe через `VICTORY_FILE_PHYSICAL`.
- **Управление.** `F2` или `~` — мышь, `F3` — FPS. Щит краш-шейдера RDNA4 `240cb516c1af0bc3` включён.
- **Windows 10.** `D3D12Core.dll` (Agility SDK 620) лежит в дереве.

Бинарь — x86-64-v3: AVX2, BMI2 и FMA на AMD Zen 2–5 и Intel от Haswell до Arrow Lake.

## См. также

- [Замеры](performance.md)
- [Как запускать](running.md)
- [Железо](hardware.md)
- [Известные ограничения](known-limits.md)
- [Безопасность, контрольные суммы и сборка](security-and-build.md)
- [Ветки и лицензия](project.md)
