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
| Dedicated VRAM | 9.92 ГБ (`5.06 ГБ` пул текстур) | **12.11 ГБ** (`5.12 ГБ` пул текстур; до `13.18 ГБ` / `6.40 ГБ` пул; авто 8–24 ГБ DXGI) |
| Shared GPU (`NON_LOCAL`) | 6.68 ГБ (5.58 ГБ Series S) | **2.18 ГБ** (`-61%` к базе Series S; `2.01 ГБ` при keep 91%) |
| Трафик copy-back | 27.0 МБ / кадр | **0.5 МБ / кадр** |
| CPU `preparing` | 45.8 мс | **32.2 – 35.4 мс** |
| Реальный кадр (мин / медиана) | 68.6 / 73.1 мс | **64.3 / 66.2 – 69.6 мс** |
| Нативный FPS и фреймтайм (без FG, открытый мир) | 14.08 FPS (ср. 71.03 мс, p99 152.1 мс / 1% low 6.6 FPS) | **16.6 – 19.5 FPS** (p50 **60.0 мс**, `victory-submit` **34.4 мс** вместо 59.7 мс, `VirtualQuery` **0.00 мс**, `BufferSweep` **0.82 мс**, `0` OOM / `0` emergency reclaim) |
| Проход `heap buffers` (макс. в игре / спайки >50 мс) | 821.1 мс (8 спайков >50 мс) | **33.8 мс (0 спайков >50 мс; в стационаре 8.6–10.0 мс)** |
| Ожидания `settle` и `port_descriptors::take()` (без FG) | 0.2–2.6 мс settle / 0.75–1.07 мс `take()` | **0.0 мс settle / 0.31–0.47 мс `take()`** |
| Отображаемый FPS (FSR 3.1.6 FG) | 25–27 FPS (провалы VSync до 15–20) | **29.2 – 31.0 FPS** (без провалов VSync) |
| HUD и субтитры в Frame Gen | Шлейфы и двоение | Без артефактов (`HUDLessColor`, 3-слотовый RGBA8) |
| Windows 10 D3D12 | Ручная установка Agility SDK | Встроенный `D3D12Core.dll` (Agility SDK 620) |

## Технические изменения

- **Память и устранение OOM (`0x8007000E`):** глобальный ограниченный пул прелюдий `(64, 16)`; немедленное переиспользование приземлившихся списков и командных аллокаторов (`recycle_landed`) с отложенным освобождением ресурсов (`bury_lag`); проактивный `emergency_reclaim()` при свободном Commit <2.5 ГБ или Shared GPU (`NON_LOCAL`) >=88% с агрессивной LRU-выгрузкой неактивных текстур/staging/пулов и динамическим троттлингом потоков записи `thin` (1–2 воркера под давлением); защита `HostList::close()`, `make_list()` и `record_prelude()` при `E_OUTOFMEMORY` (reclaim, retry и безопасный сброс/вывод аллокатора без краша); `MEM_RESET` для освобождённых несмапленных физических страниц; upstream-фиксы `#2143` (таблица констант после инструкций), `#2234` (освобождение таблицы памяти при удалении view heap), `#2462` (проверка `typed_store` для UAV), `#2844` (RAII-слоты `CpuViews`) и `#3540` (16-байтное выравнивание raw buffer views); опциональные автоудаляемые файлы физических страниц на NVMe (`FILE_FLAG_DELETE_ON_CLOSE` в `%LOCALAPPDATA%\victory\physical`, >11 ГБ свободного Commit при загрузке). Подтверждено на 180-секундном нативном прогоне без FG на стандартном файле подкачки (`VICTORY_FILE_PHYSICAL=0`): 0 ошибок, 0 OOM, `min_free_commit 3.47 ГБ`, стабильно `5.10–5.57 ГБ` свободного Commit.
- **Вывод кадра и Frame Generation:** 3-слотовый RGBA8 захват `HUDLessColor` до наложения UI/субтитров/HUD; тройная буферизация с `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING` и `DXGI_PRESENT_ALLOW_TEARING`; нативный апскейл AMD FSR 4.1.2 ML (автофоллбэк на FSR 3.1 на старых GPU).
- **CPU-сабмит и copy-back:** write-combined записи шейдеров остаются в VRAM (`27 -> 0.5 МБ/кадр`); 128-байтный AVX2 `vptest` fast-skip и векторизованный `merge_piece`; lock-free work-stealing пул `land`; 64-байтные cache-aligned шарды write-watch; однопроходный объединённый скан `ask_once`/`stale_under_flights`; lock-free `mirror` дескрипторов; `OnceLock` сигнатуры команд; стековые `Few<Barrier, 16>`; 64-шардовый `PLACED_TEXTURES`; разблокировка очереди при `unfold` и lock-free `wake()`; зануление `upload::ARENAS` вне мьютекса; повышенный приоритет helper-потоков на 6 ядрах; предпланирование списков на thin-воркерах; хэш до detile; переиспользование upload-буферов.
- **Динамический DXGI авто-VRAM и снижение Shared GPU:** автоматическое масштабирование профилей 8 / 12 / 16 / 24+ ГБ по `DedicatedVideoMemory` и живому `DXGI_QUERY_VIDEO_MEMORY_INFO` (`VICTORY_VRAM_MODE=auto|8gb|12gb|16gb|24gb|legacy`, поддержка `"auto"` и ручных env-переопределений, детекция UMA и ступенчатая защита по Commit). На 16 ГБ GPU политика учитывает объём RAM: на системах `<=20 ГБ` RAM держит `84%` keep / `90%` room (`12.11 ГБ` VRAM, `2.18 ГБ` Shared GPU, без свопа working set в Windows, `+55%` к 1% low FPS), на `>=32 ГБ` RAM — `91%` keep / `95%` room (`~13.2–14.2 ГБ` VRAM). Прямые VRAM-окна без дублирующего staging (`CardCpu::Direct`), компактные CPU-тени (`CardCpu::Cpu`), O(dirty) очередь `DIRTY_CARDED`, пошаговая эвикция VRAM-окон с гистерезисом (не более 8/с), общий scratch для depth-stencil двойников (upstream `#2162`), исправление защёлки `host_pressure` на 16 ГБ RAM (`send_allocs` `117/15с -> 0/15с`), автодетекция NVMe (`VICTORY_FILE_PHYSICAL=auto`) и точный учёт памяти игры в `XMemGetWorkingSetStatistics`.
- **Фреймпейсинг и устранение спайков (нативный режим без FG):** асинхронный инкрементальный `sweep()` (`victory-sweep`, `VICTORY_SWEEP_ASYNC=1`), убирающий синхронную остановку конвейера `settle(WHY_SYNC)` каждые 1.5 с; фоновый сборщик `bury()` с троттлингом (`victory-bury`, `VICTORY_BURY_ASYNC=1`); ожидание `WHY_FENCE`/`WHY_VALUE` по `Line.seq` без блокировки `host.submitted`; пропуск лишнего `sync_heaps` на `execute_own` при презенте (`0.0` немаркированных раундов); 64-битный скан `AtomicU64` в `port_descriptors::take()` (ускорение в 2–3×); точный сон `frame::wait` без лишних `Sleep(0)`; покадровая телеметрия `present pacing` (`avg / p50 / p95 / p99 / 1% low / max / >50 мс / >100 мс`, `VICTORY_FRAME_TIMES_FILE`) и плоский `heap buffers` (`VICTORY_TEXTURE_STAGE_MS=4`, `VICTORY_TEXTURE_CATCHUP=0`, `0` спайков >50 мс).
- **Фаза 2 — оптимизация CPU-сабмита и дескрипторов (`feat/phase2-30fps`):** межсабмитный `MappedCache` с диапазонной инвалидацией для `VirtualQuery` в `card_steps` (`12.56 мс / 36 вызовов -> 0.00 мс / 0 вызовов`, `own_cs` `15.1 -> 2.5 мс`); lock-free послотовая таблица поколений `ENTRY_GENS` + 8192-слотовый TLS-кэш `descriptor::VIEWS` и `words_for` (`100.0%` попаданий); быстрый inline-CBV путь в `draw_bind::tables` + потоковый батчинг `RETIRE_TLS`; покопийный инкрементальный `BufferSweep` и пропуск неизменившихся слотов в `heap_buffer_pass` (`HBP_SPANS` `10.94 -> 0.51 мс`); расширенные пулы `PRELUDES_MOST` `(512, 128)`, `SPARE_BATCH` `(128/256)`, `send::SPARE` `(256/128 MiB)` и `VICTORY_LIST_IDLE_MOST=1024`; приоритетная очередь compute PSO + 4 compute/screen воркера с общим бюджетом (`p99.9` `-71%`, `1% low` в `2×` лучше); исправление порогов `residency` на 16 ГБ RAM (`0` ложных pressure, `unmap_null` `-97%`, `VICTORY_KEEP_SHOWN=2`, `VICTORY_TEXTURE_AWAY_MS=10000`); постраничная (64 KiB) битовая карта `gpu_written_pages` для `PORT_CBV`; HUD в правом верхнем углу (`VICTORY_PANEL_POS=top-right`); встроенный профилировщик `VICTORY_PHASE2_PROFILE=1`.
- **Исправления и управление:** бюджет ожидания compute-пайплайнов (`5000 мс`) — исправлены отсутствующие лица персонажей; пропуск проблемного шейдера RDNA4 (`240cb516c1af0bc3`); фильтр битых фиолетовых ambient-проб; безопасный async-флип дескрипторов; повтор сбойного detile текстур; ПК-управление (`F2`/`~` обзор мышью, `F3` оверлей FPS); встроенная галочка выбора Frame Generation (вкл/выкл) в лаунчере.

## Текущие известные ограничения (в работе на `dev`)

- Нативные 30 FPS (`<= 33.3 мс`) продолжаем дожимать на `dev`: `victory-submit` уже снижен до `34.4 мс/кадр`, следующая цель — потоки записи списков (`victory-list-0..3`) и копии CBV.
- Переход из пролога в открытый мир на сильно урезанных файлах подкачки смягчён проактивным `emergency_reclaim()` и автодетекцией NVMe (`VICTORY_FILE_PHYSICAL=auto`) на `dev`.

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
| `RAZDA4A XEN.bat` | Основной лаунчер с **галочкой Frame Generation (вкл/выкл)** (запоминает выбор в `config.bat`), авто-VRAM, HUDLess FG + Tearing |
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

SHA-256 готового `RAZDA4A_XEN.exe`: `7b2d7b7a5059ec41c423c60c34d0fd0025dc141239fe81a645fbfe4f2307376b`

## Ветки и лицензия

- `main`: готовый бинарник, системные DLL, пресеты, документация.
- `dev`: полный исходный код Rust, юнит-тесты, CI, `AGENTS.md`.

Основано на VIctory, лицензия **GPL-3.0-or-later** ([LICENSE](LICENSE)). Полный исходный код доступен в ветке [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
