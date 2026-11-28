<p align="center">
  <span style="font-size:64px">🦋</span>
</p>

<h1 align="center">RAZDA4A XEN</h1>

<p align="center">
  Direct3D 12 рантайм для AMD RDNA.<br>
  <b>Wave 11</b> · <b>~31 native / 62-63 FG</b>
</p>

<p align="center">
  <img alt="Релиз: Wave 11" src="https://img.shields.io/badge/release-Wave%2011-2ea44f">
  <a href="https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip"><img alt="Скачать ZIP ветки main" src="https://img.shields.io/badge/download-main%20ZIP-0969da"></a>
  <a href="LICENSE"><img alt="Лицензия GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue"></a>
  <img alt="Windows 10/11 и Linux через Proton" src="https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20Linux%20Proton-informational">
</p>

<p align="center">
  <a href="README.md">English</a> ·
  Русский ·
  <a href="README.zh.md">简体中文</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (BR)</a>
</p>

## Результаты

| | |
|---|---|
| **Релиз** | Wave 11 (`main`) |
| **Машина** | Radeon RX 9060 XT + Ryzen 7 7800X3D |
| **FPS** | **~31 native / 62-63 FG** |

`62-63 FG` — это frame generation. Нативно — ~31.

Карты на 8 ГБ идут в Safe Mode. На RX 7600 замерено **12.7 native / 24.86 FG**. Таблицы: [Замеры](docs/ru/performance.md).

## Быстрый старт

В репозитории нет файлов игры. Используйте свою копию.

1. Скачайте [main.zip](https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip) или `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git` и распакуйте.
2. Запустите **`RAZDA4A_XEN.exe`** или **`RAZDA4A XEN.bat`**.
3. В лаунчере выберите `.exe` игры, профиль и frame generation, затем **Launch**.

Не запускайте исполняемый файл игры напрямую. Рантайм подключается только через лаунчер.

`F2` или `~` — обзор мышью (`Esc` отпускает курсор). `F3` — показ FPS.

Linux: `launch_linux_proton.sh`. Пресеты и Linux: [Как запускать](docs/ru/running.md).

## Требования

| | Wave 11 |
|---|---|
| **GPU** | AMD RDNA2 / RDNA3 / RDNA4 (RX 6000 / 7000 / 9000), DX12 Ultimate и mesh shaders |
| **CPU** | x86-64-v3 (AMD Zen 2–5, Intel от Haswell до Arrow Lake) |
| **Память** | 16 ГБ ОЗУ на SSD. Малый commit: файл подкачки 24–32 ГБ (`pagefile_16gb_ram.ps1`). 8–12 ГБ VRAM: Safe Mode |
| **ОС** | Windows 10 / 11 или Linux через Proton / VKD3D-Proton |
| **Не эта сборка** | NVIDIA, Intel Arc, RDNA1, GTX 10/16, карты класса 4 ГБ, HDD |

Версия для NVIDIA в пути. Сначала вышла AMD. До неё: [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA).

## Пресеты

| Файл | Назначение |
|---|---|
| `RAZDA4A XEN.bat` | Окно лаунчера: `.exe` игры, frame generation, Safe Mode на 8 ГБ |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 ГБ ОЗУ, страницы на NVMe |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced плюс in-place константные буферы |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Только натив, frame generation выключен |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 ГБ VRAM |

## FAQ

<details>
<summary>Игра внутри?</summary>

Нет. Нужна своя копия. Утёкший GDK не нужен. Ссылок на скачивание игры здесь нет.
</details>

<details>
<summary>NVIDIA или Intel Arc?</summary>

Wave 11 — это AMD RDNA. Версия для NVIDIA в пути. Сначала вышла AMD. Форк сообщества — выше. Intel Arc не поддерживается. Не-AMD адаптер может дать чёрный экран или вылет.
</details>

<details>
<summary>8 ГБ VRAM или меньше?</summary>

8–12 ГБ: Safe Mode (пресет 4, на 8 ГБ лаунчер выбирает его сам). RX 7600: 12.7 native / 24.86 FG. 6 ГБ и меньше — не поддерживаемая конфигурация. У RDNA1 и GTX 10/16 нет mesh shaders, которые нужны этой сборке.
</details>

<details>
<summary>Трассировка лучей?</summary>

В Wave 11 выключена. Построитель структур трассировки не запускается.
</details>

<details>
<summary>Жёсткий диск или 16 ГБ ОЗУ?</summary>

HDD не поддерживается. Нужен NVMe или SATA SSD. На 16 ГБ ОЗУ тесный pagefile закрывают `pagefile_16gb_ram.ps1` или пресет 1.
</details>

## Если что-то не так

- Первый запуск компилирует пайплайны (около 1–3 минут подёргиваний). Закройте, один раз запустите `warm_shaders.bat` и снова откройте лаунчер.
- Windows Defender может пометить неподписанный ZIP как `Trojan:Script/Wacatac.H!ml`. Сверьте `RAZDA4A_XEN.exe` с [SHA256SUMS.txt](SHA256SUMS.txt): `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`.
- `collect_logs.bat` или **Export Logs** в лаунчере пишет `logs\bug_report_*.txt` на этом ПК.
- Linux около 1 FPS или пустая картинка: используйте `launch_linux_proton.sh`, он не даёт `D3D12Core.dll` подменить vkd3d-proton.

[Как запускать](docs/ru/running.md) · [Безопасность и сборка](docs/ru/security-and-build.md)

## О проекте

RAZDA4A XEN — open-source слой Direct3D 12 для 16 ГБ ОЗУ, 6-ядерных CPU и 8–16 ГБ VRAM. Wave 11 на `main` — это лаунчер, пресеты, DLL и эти страницы. Исходники на Rust — в [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).

## Благодарности

- [VIctory](https://github.com/Marvisak/VIctory) от Marvisak, апстрим, GPL-3.0-or-later ([LICENSE](LICENSE)).
- `nathanbzrr` — Safe Mode для 8 ГБ, в составе `main`.
- [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) — форк сообщества для NVIDIA.

## Документация

- [Замеры](docs/ru/performance.md)
- [Как запускать](docs/ru/running.md)
- [Железо](docs/ru/hardware.md)
- [Что входит в Wave 11](docs/ru/technical-changes.md)
- [Известные ограничения](docs/ru/known-limits.md)
- [Безопасность, контрольные суммы и сборка](docs/ru/security-and-build.md)
- [Ветки и лицензия](docs/ru/project.md)
