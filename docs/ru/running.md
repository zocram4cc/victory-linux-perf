[English](../en/running.md) | [简体中文](../zh/running.md) | [Español](../es/running.md) | [Português (BR)](../pt-BR/running.md)

# Как запускать

[К README](../../README.ru.md)

Запускайте через прилагаемый лаунчер: **`RAZDA4A_XEN.exe`** или **`RAZDA4A XEN.bat`**. Не запускайте исполняемый файл игры напрямую. Рантайм подключается только если игру стартует лаунчер.

Файлы игры, ассеты и исполняемые файлы игры в репозиторий не входят — используйте собственную копию *Великого симулятора автоугона от Звёздной студии*.

## Быстрый старт (Windows)

1. Скачайте ветку `main` (**Code -> Download ZIP** или `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) и распакуйте (например, в `D:\RAZDA4A-XEN\`). Все DLL (`D3D12\D3D12Core.dll`, `fidelityfx\*.dll`) уже внутри.
2. Запускайте **`RAZDA4A_XEN.exe`** или **`RAZDA4A XEN.bat`**. Не запускайте `.exe` игры двойным щелчком.
   - **`RAZDA4A XEN.bat`** открывает окно лаунчера: через **Browse...** выберите **`ваш_запускной_файл.exe`** (сохранится в `config.bat` в UTF-8, включая русские пути), выберите профиль и галочку Frame Generation и нажмите **Launch**.
   - Для сторонних лаунчеров (`VILauncher` и т.д.): используйте **`RAZDA4A XEN.bat --no-gui`** после настройки `config.bat`.

| Файл | Назначение |
|---|---|
| `RAZDA4A XEN.bat` | Основной лаунчер (выбор `.exe`, FG вкл/выкл, авто-Safe-Mode для 8 ГБ, кнопки Warm Shaders и Export Logs) |
| `RAZDA4A_XEN.exe` | Рантайм, который стартует лаунчер. Не обходите его и не запускайте exe игры напрямую. |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Для 16 ГБ RAM и малого pagefile: NVMe-файл физических страниц (`VICTORY_FILE_PHYSICAL=1`) |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced + экспериментальные in-place CBV (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Чистый нативный рендер без Frame Gen, минимальная задержка |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | Safe Mode для 8–12 ГБ VRAM (`65%` / `2048 МБ` таргеты / `768 МБ` буферы, VSync вкл.) |
| `warm_shaders.bat` | Предкомпиляция встреченных PSO после первой сессии |
| `collect_logs.bat` | Очищенный отчёт (железо + `logs\last_run.log`) в `logs\bug_report_*.txt` на этом ПК. |
| `pagefile_16gb_ram.ps1` / `backup_saves.bat` | Помощник файла подкачки 16–32 ГБ / бэкап `data\` |

Управление: **`F2` / `~`** — обзор мышью (`Esc` — отпустить курсор), **`F3`** — HUD FPS в правом верхнем углу.

## Быстрый старт (Linux — Proton / Wine / VKD3D-Proton)

1. Установите **Proton Experimental / GE-Proton** (через `umu-run`, Steam или Lutris) и **Mesa RADV** на AMD RDNA2+ GPU.
2. Запустите `launch_linux_proton.sh` (скрипт автоматически выставляет `WINEDLLOVERRIDES="d3d12,d3d12core=n"`, чтобы встроенный Windows 10 `D3D12Core.dll` не перекрывал `vkd3d-proton` и не вызывал 1 FPS / пустой 3D-экран). Скрипт сам запускает `RAZDA4A_XEN.exe`. Не запускайте исполняемый файл игры напрямую.
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```
   Лог пишется в `logs/last_run.log`. На картах 12–16+ ГБ VRAM можно использовать `XEN_PROFILE=balanced`.

## См. также

- [Замеры](performance.md)
- [Железо](hardware.md)
- [Что входит в Wave 11](technical-changes.md)
- [Известные ограничения](known-limits.md)
- [Безопасность, контрольные суммы и сборка](security-and-build.md)
- [Ветки и лицензия](project.md)
