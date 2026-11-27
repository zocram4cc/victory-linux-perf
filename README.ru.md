[English](README.md) | Русский | [简体中文](README.zh.md) | [Español](README.es.md) | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

Слой совместимости Direct3D 12 под Windows для систем с **16 ГБ RAM**, **6-ядерным CPU**, **видеокартами 8–16 ГБ VRAM** и **Windows 10**.

Репозиторий не содержит файлов игры, ассетов или исполняемых файлов. Используется собственный дамп *«Великого Автоугонщика» от Звёздной Компании*.

## Отличия от оригинального VIctory

Оригинальный VIctory разрабатывается и профилируется под стенды с 64 ГБ RAM, 24 ГБ видеокартами и Windows 11 (WDDM 3.0+ с `D3D12_HEAP_TYPE_GPU_UPLOAD`). На ПК с 16 ГБ RAM и Windows 10 это приводит к вылетам `0x8007000E` (`E_OUTOFMEMORY`), падению FPS вдвое из-за VSync, двоению HUD в Frame Generation и упору в однопоточный CPU.

Замеры на **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 ГБ, 16 ГБ DDR4, Windows 10 (19045)** — 1080p профиль Series S, RT выкл., 180-секундные прогоны, 0 ошибок:

| Параметр | База (конфиг upstream) | RAZDA4A XEN |
|---|---|---|
| Private Commit (установившийся) | 21.8 – 30+ ГБ (вылеты `0x8007000E`) | **19.3 ГБ** (6.2 ГБ RAM WS, 5.5 ГБ свободно) |
| Dedicated VRAM | 9.92 ГБ (`5.06 ГБ` пул текстур) | **8.18 ГБ** (`3.65 ГБ` пул текстур) |
| Shared GPU (`NON_LOCAL`) | 6.68 ГБ | **5.45 ГБ** |
| Трафик copy-back | 27.0 МБ / кадр | **0.5 МБ / кадр** |
| CPU `preparing` | 45.8 мс | **32.2 – 35.4 мс** |
| Реальный кадр (мин / медиана) | 68.6 / 73.1 мс | **64.3 / 66.2 – 69.6 мс** |
| Нативный FPS (без FG) | ~13.7 FPS | **14.5 – 15.5 FPS** |
| Отображаемый FPS (FSR 3.1.6 FG) | 25–27 FPS (провалы VSync до 15–20) | **29.2 – 31.0 FPS** (без провалов VSync) |
| HUD и субтитры в Frame Gen | Шлейфы и двоение | Без артефактов (`HUDLessColor`, 3-слотовый RGBA8) |
| Windows 10 D3D12 | Ручная установка Agility SDK | Встроенный `D3D12Core.dll` (Agility SDK 620) |

## Технические изменения

- **Память и устранение OOM (`0x8007000E`):** глобальный ограниченный пул прелюдий `(64, 16)`; немедленное переиспользование приземлившихся списков и командных аллокаторов (`recycle_landed`) с отложенным освобождением ресурсов (`bury_lag`); очистка и повтор при `E_OUTOFMEMORY` в `make_list()` и `record_prelude()`; ограничение `NON_LOCAL` (`92%`, гистерезис `-10%`); опциональные автоудаляемые файлы физических страниц на NVMe (`FILE_FLAG_DELETE_ON_CLOSE` в `%LOCALAPPDATA%\victory\physical`, >11 ГБ свободного Commit при загрузке).
- **Вывод кадра и Frame Generation:** 3-слотовый RGBA8 захват `HUDLessColor` до наложения UI/субтитров/HUD; тройная буферизация с `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING` и `DXGI_PRESENT_ALLOW_TEARING`; нативный апскейл AMD FSR 4.1.2 ML (автофоллбэк на FSR 3.1 на старых GPU).
- **CPU-сабмит и copy-back:** write-combined записи шейдеров остаются в VRAM (`27 -> 0.5 МБ/кадр`); 128-байтный AVX2 `vptest` fast-skip и векторизованный `merge_piece`; lock-free work-stealing пул `land`; 64-байтные cache-aligned шарды write-watch; однопроходный объединённый скан `ask_once`/`stale_under_flights`; lock-free `mirror` дескрипторов; `OnceLock` сигнатуры команд; стековые `Few<Barrier, 16>`; 64-шардовый `PLACED_TEXTURES`; разблокировка очереди при `unfold` и lock-free `wake()`; зануление `upload::ARENAS` вне мьютекса; повышенный приоритет helper-потоков на 6 ядрах; предпланирование списков на thin-воркерах; хэш до detile; переиспользование upload-буферов.
- **Исправления и управление:** бюджет ожидания compute-пайплайнов (`5000 мс`) — исправлены отсутствующие лица персонажей; пропуск проблемного шейдера RDNA4 (`240cb516c1af0bc3`); фильтр битых фиолетовых ambient-проб; безопасный async-флип дескрипторов; повтор сбойного detile текстур; ПК-управление клавиатурой и мышью (`F2`/`~` обзор мышью, `F3` оверлей FPS).

## Текущие известные ограничения (в работе на `dev`)

- Нативные 30 FPS (`<= 33.3 мс`) пока упираются в однопоточный `victory-submit` (`49–60 мс/кадр`; GPU занята на 52–58%).
- Профиль Series S использует ~8.2 ГБ VRAM на 16 ГБ картах; динамический авто-бюджет VRAM в разработке на `dev`.
- Переход из пролога в открытый мир может превышать стандартный файл подкачки на 16 ГБ RAM (`0x8007000E`); исправление в коде без увеличения подкачки в разработке на `dev`.

## Быстрый старт

1. Скачай ветку `main` (**Code -> Download ZIP** или `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) и распакуй (например `D:\RAZDA4A-XEN\`). Все нужные DLL (`D3D12\D3D12Core.dll`, `fidelityfx\*.dll`) уже внутри.
2. Укажи в **`config.bat`** свою папку и **`ваш_запускной_файл.exe`**:
   ```bat
   set "GAME_DIR=D:\Path\To\Your\GameFolder"
   set "GAME_EXE=%GAME_DIR%\ваш_запускной_файл.exe"
   ```
3. Запусти **`RAZDA4A XEN.bat`** или выбери пресет из `presets\`:

| Файл | Профиль |
|---|---|
| `RAZDA4A XEN.bat` | Сбалансированный (по умолчанию): 29–31 FG FPS, ~8.2 ГБ VRAM, 6.2 ГБ RAM, HUDLess FG + Tearing |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Маленький файл подкачки / открыт браузер: физические страницы на NVMe, ~8.0 ГБ VRAM, >11 ГБ свободного Commit при загрузке |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Сбалансированный + чтение констант на месте (экспериментальный) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Нативный рендер без Frame Gen, минимальная задержка ввода |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | Видеокарты 8–12 ГБ VRAM / безопасный режим: жёсткие лимиты, VSync вкл., полное сканирование дескрипторов |

- **`F2` / `~`** — переключить обзор мышкой (`Esc` освобождает курсор)
- **`F3`** — оверлей FPS
- **`warm_shaders.bat`** — запустить один раз после закрытия первой сессии для предкомпиляции шейдеров
- **`backup_saves.bat`** — бэкап/восстановление `data\`
- **`pagefile_16gb_ram.ps1`** — опциональный скрипт администратора (файл подкачки 16–32 ГБ на SSD)

## Сборка из исходников и проверка бинарника

Если не хочешь запускать неподписанный `.exe`, собери бинарник сам из исходников в ветке `dev`:

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --locked -p victory
```

SHA-256 готового `RAZDA4A_XEN.exe`: `e90b23f05f6d3caa488beaf3afc4cd0e94151b75a33606bfd75594a373e4f18b`

## Ветки и лицензия

- `main`: готовый бинарник, системные DLL, пресеты, документация.
- `dev`: полный исходный код Rust, юнит-тесты, CI, `AGENTS.md`.

Основано на VIctory, лицензия **GPL-3.0-or-later** ([LICENSE](LICENSE)). Полный исходный код доступен в ветке [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
