[English](README.md) | Русский | [简体中文](README.zh.md) | [Español](README.es.md) | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

Direct3D 12 Windows-рантайм для **видеокарт AMD RDNA2 / RDNA3 / RDNA4**, нацеленный на **16 ГБ ОЗУ**, **6-ядерные CPU**, **8–16 ГБ VRAM** и **Windows 10 / 11** (а также Linux через Proton / VKD3D-Proton).

Файлы игры, ассеты и исполняемые файлы игры в репозиторий не входят — используйте собственную копию *Великого симулятора автоугона от Звёздной студии*.

## Подтверждённые замеры (Нативный FPS отдельно от Frame Generation)

Во всех таблицах ниже **нативный FPS (без Frame Gen)** всегда указан отдельно от **отображаемого FPS (FSR 3.1.6 Frame Gen 2x)** с указанием точной сцены и конфигурации.

### 16 ГБ VRAM / 16 ГБ RAM (Открытый мир, 1080p Series S, RT выкл.)

Замерено на **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 ГБ, 16 ГБ DDR4, Windows 10 (19045)** — 180-секундные прогоны в открытом мире + 22-минутный прогон Wave 11 (18 393 кадра), 0 ошибок, 0 OOM:

| Метрика | База (апстрим-конфиг) | RAZDA4A XEN (Wave 7 / Wave 11) |
|---|---|---|
| **Нативный FPS (FG выкл., открытый мир)** | 14.08 средн. (71.03 мс, p99 152.1 мс / 1% low 6.6 FPS) | **18.19 p50** (54.97 мс), **16.17** средн., 15.2–23.4 стабильно, p99.9 **478.6 мс** |
| **Отображаемый FPS (FSR 3.1.6 FG 2x)** | 25–27 FPS (падения до 15–20 из-за VSync) | **29.2 – 31.0 FPS** стабильно (HUDLess RGBA8-кольцо, без просадок VSync) |
| Private Commit (устоявшийся) | 21.8 – 30+ ГБ (`0x8007000E` OOM) | **19.3 ГБ** (6.2 ГБ RAM WS, 5.5 ГБ свободного Commit) |
| Выделенная VRAM | 9.92 ГБ (`5.06 ГБ` пул текстур) | **12.11 ГБ** (авто 8–24 ГБ DXGI; `0` отказов по VRAM за 22 мин в Wave 11) |
| Shared GPU (`NON_LOCAL`) | 6.68 ГБ (5.58 ГБ Series S) | **2.18 ГБ** (`-61%` к базе Series S) |
| Трафик copy-back | 27.0 МБ / кадр | **0.5 МБ / кадр** |
| CPU `preparing` / `heap buffers` | 45.8 мс / 821.1 мс макс. | **32.2 – 35.4 мс** / **33.8 мс макс.** (0 пиков >50 мс) |
| Raw buffer views (`LIST_VIEWS`, Wave 11) | 372.1–575.8 null/кадр (пропадали деревья/объекты) | **0.0 null/кадр** (28 212/кадр, 932.5 невыровн. -> 0 null; деревья/листва/заборы починены) |
| Windows 10 D3D12 | Ручная настройка Agility SDK | Встроенный `D3D12Core.dll` (Agility SDK 620) |

### Проверка 8 ГБ VRAM — Safe Mode (PR #1 от `nathanbzrr`)

Замерено на **Ryzen 5 5500X3D, Radeon RX 7600 8 ГБ, 32 ГБ DDR4, Windows 11 (26340)** — 1080p Series S, Safe Mode (`65%` лимит текстур / `2048 МБ` рендер-таргеты / `768 МБ` буферы карты), FSR FG 2x, 180 с:

| Метрика | До подгонки 8 ГБ | После подгонки под живой бюджет |
|---|---:|---:|
| **Нативный FPS (без FG)** | 3.67 | **12.7** (средн. 79.01 мс, p50 68.90 мс, p99 **242.26 мс** против 1047.30 мс) |
| **Отображаемый FPS (FSR FG 2x)** | 7.32 | **24.86** (1803 сгенерировано / 1803 отрендерено) |
| Пик VRAM / живой бюджет | 6548 МБ | **6169 МБ** / 5934–5981 МБ (`0` ошибок, `0` OOM, `0` device removal) |

## Технические изменения

- **Wave 11 (деревья/листва и портативный бинарь):** исправлено `372–575` принудительных `NULL` raw-буферных view за кадр в `LIST_VIEWS` (перестали пропадать деревья, листва, заборы, столбы и знаки); поддержка `SW_64KB_D`; донастройка 16 ГБ VRAM residency (`0` отказов `card has no room` и `host memory pressure` за 22 минуты); переносимый бинарь **`x86-64-v3`** (полный AVX2/BMI2/FMA на AMD Zen 2–5 и Intel Haswell–Arrow Lake без AMD-специфичных расширений `znver3`).
- **Wave 6–7 (текстуры, дескрипторы и стабильность):** HashMap + 8192-слотовый TLS-кэш дескрипторов (`100%` попаданий), best-fit пул, ограниченная `let_go_idle`, перемежающаяся очередь текстур (очередь в катсценах `3936 -> 0` кадров), гистерезис для 16 ГБ RAM, обычный приоритет CPU (не вешает фоновые приложения) и запись лога в файл `VICTORY_LOG_FILE`.
- **Память и OOM (`0x8007000E`):** ограниченные пулы прелюдий, мгновенная переработка `recycle_landed`, упреждающая `emergency_reclaim()` (<2.5 ГБ свободного Commit или >=88% Shared GPU), восстановление при `E_OUTOFMEMORY`, `MEM_RESET`, динамический DXGI Auto-VRAM (`8/12/16/24 ГБ`), опциональный NVMe-файл физических страниц (`VICTORY_FILE_PHYSICAL=auto`).
- **Отрисовка и управление:** 3-слотовый RGBA8-захват `HUDLessColor` для FSR 3.1.6 FG, swapchain с tearing, нативный апскейл AMD FSR 4.1.2 ML (откат на FSR 3.1), бюджет ожидания compute `5000 мс` (исправление лиц), защита от краш-шейдера RDNA4 (`240cb516c1af0bc3`), управление ПК (`F2`/`~` — обзор мышью, `F3` — HUD FPS в правом верхнем углу).

## Известные ограничения (в работе на `dev`)

- В плотных городских районах нативный FPS может падать до `5–10` с длинными статтерами на таблице дескрипторов в долгих сессиях (активно решается в **Wave 12** на `dev`).
- Трассировка лучей (RT) отключена на время стабилизации базового рендера, VRAM и достижения нативных 30 FPS.
- Первый запуск компилирует PSO-шейдеры (1–3 минуты возможных фризов); со второго запуска и после `warm_shaders.bat` работает заметно плавнее.

## Совместимость железа и FAQ

| Вопрос | Ответ |
|---|---|
| **Какие AMD GPU поддерживаются?** | **AMD RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000 — требуются DX12 Ultimate и Mesh Shaders). |
| **Работает ли на NVIDIA / Intel Arc?** | **Нет.** Этот форк только для AMD RDNA; на RTX будет чёрный экран/вылет. Для NVIDIA используйте форки сообщества, например [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). |
| **Intel CPU + AMD GPU подойдёт?** | **Да.** Сборки начиная с Wave 11 используют универсальный `-C target-cpu=x86-64-v3`. |
| **8 ГБ / 6 ГБ / <=4 ГБ VRAM?** | **8 ГБ:** работает через **Safe Mode** (проверено: 12.7 native / 24.86 FG на RX 7600, также проверено на RX 6600). **6 ГБ:** экспериментально и тяжело (Safe Mode + FG выкл.). **<=4 ГБ, RDNA1 (RX 5500/5700), GTX 10/16:** не поддерживаются (нет аппаратных Mesh Shaders или слишком мало VRAM). |
| **16 ГБ ОЗУ / HDD?** | **16 ГБ ОЗУ:** поддерживается на **NVMe/SATA SSD** с файлом подкачки 24–32 ГБ (`pagefile_16gb_ram.ps1`) или пресетом `Low-Commit NVMe`. **HDD:** не поддерживается (подгрузка / DirectStorage). |
| **Нужен ли утёкший GDK?** | **Нет.** Полностью автономный open-source рантайм (`GPL-3.0-or-later`). |
| **Куда отправлять баги и логи?** | Запустите **`collect_logs.bat`** (или кнопку **Export Logs** в лаунчере) — создастся очищенный от личных путей `logs\bug_report_*.txt`, приложите его в [GitHub Issues](https://github.com/crossducking/RAZDA4A-XEN/issues). |

## Быстрый старт (Windows)

1. Скачайте ветку `main` (**Code -> Download ZIP** или `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) и распакуйте (например, в `D:\RAZDA4A-XEN\`). Все DLL (`D3D12\D3D12Core.dll`, `fidelityfx\*.dll`) уже внутри.
2. Запустите **`RAZDA4A XEN.bat`**, через **Browse...** выберите **`ваш_запускной_файл.exe`** (сохранится в `config.bat` в UTF-8, включая русские пути), выберите профиль и галочку Frame Generation и нажмите **Launch**.
   - Для сторонних лаунчеров (`VILauncher` и т.д.): используйте **`RAZDA4A XEN.bat --no-gui`** после настройки `config.bat`.

| Файл | Назначение |
|---|---|
| `RAZDA4A XEN.bat` | Основной лаунчер (выбор `.exe`, FG вкл/выкл, авто-Safe-Mode для 8 ГБ, кнопки Warm Shaders и Export Logs) |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Для 16 ГБ RAM и малого pagefile: NVMe-файл физических страниц (`VICTORY_FILE_PHYSICAL=1`) |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced + экспериментальные in-place CBV (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Чистый нативный рендер без Frame Gen, минимальная задержка |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | Safe Mode для 8–12 ГБ VRAM (`65%` / `2048 МБ` таргеты / `768 МБ` буферы, VSync вкл.) |
| `warm_shaders.bat` | Предкомпиляция встреченных PSO после первой сессии |
| `collect_logs.bat` | Сбор очищенного багрепорта (железо + `logs\last_run.log`) |
| `pagefile_16gb_ram.ps1` / `backup_saves.bat` | Помощник файла подкачки 16–32 ГБ / бэкап `data\` |

Управление: **`F2` / `~`** — обзор мышью (`Esc` — отпустить курсор), **`F3`** — HUD FPS в правом верхнем углу.

## Быстрый старт (Linux — Proton / Wine / VKD3D-Proton)

1. Установите **Proton Experimental / GE-Proton** (через `umu-run`, Steam или Lutris) и **Mesa RADV** на AMD RDNA2+ GPU.
2. Запустите `launch_linux_proton.sh` (скрипт автоматически выставляет `WINEDLLOVERRIDES="d3d12,d3d12core=n"`, чтобы встроенный Windows 10 `D3D12Core.dll` не перекрывал `vkd3d-proton` и не вызывал 1 FPS / пустой 3D-экран):
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

## Безопасность, ложные срабатывания антивируса и сборка из исходников

Браузеры (Brave/Edge) и Windows Defender ML могут помечать неподписанные ZIP-архивы с `.exe` + `.bat` + `.ps1` как **`Trojan:Script/Wacatac.H!ml`**. Не доверяйте неподписанным бинарям на слово — сверьте хеш или соберите бинарь сами из открытой ветки `dev`:

```powershell
# Проверка хеша по SHA256SUMS.txt
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (хеши всех DLL см. в [`SHA256SUMS.txt`](SHA256SUMS.txt)).

Сборка из исходников (ветка `dev`, Rust MSVC):

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## Ветки и лицензия

- `main`: готовый бинарь, DLL, пресеты, лаунчер, контрольные суммы, документация.
- `dev`: полный исходный код на Rust, юнит-тесты, `xtask policy`, CI, `AGENTS.md`.

Производный проект от VIctory под лицензией **GPL-3.0-or-later** ([LICENSE](LICENSE)). Полный исходный код доступен в [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
