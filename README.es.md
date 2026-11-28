<p align="center">
  <span style="font-size:64px">🦋</span>
</p>

<h1 align="center">RAZDA4A XEN</h1>

<p align="center">
  Runtime Direct3D 12 para AMD RDNA.<br>
  <b>Wave 11</b> · <b>~31 native / 62-63 FG</b>
</p>

<p align="center">
  <img alt="Versión: Wave 11" src="https://img.shields.io/badge/release-Wave%2011-2ea44f">
  <a href="https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip"><img alt="Descargar el ZIP de main" src="https://img.shields.io/badge/download-main%20ZIP-0969da"></a>
  <a href="LICENSE"><img alt="Licencia GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue"></a>
  <img alt="Windows 10/11 y Linux con Proton" src="https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20Linux%20Proton-informational">
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.zh.md">简体中文</a> ·
  Español ·
  <a href="README.pt-BR.md">Português (BR)</a>
</p>

## Resultados

| | |
|---|---|
| **Versión** | Wave 11 (`main`) |
| **Equipo** | Radeon RX 9060 XT + Ryzen 7 7800X3D |
| **FPS** | **~31 native / 62-63 FG** |

`62-63 FG` es frame generation. El nativo es ~31.

Las tarjetas de 8 GB usan Safe Mode. Una RX 7600 midió **12,7 nativos / 24,86 FG**. Tablas: [Rendimiento](docs/es/performance.md).

## Inicio rápido

Este repositorio no trae archivos del juego. Usa tu propia copia.

1. Descarga [main.zip](https://github.com/crossducking/RAZDA4A-XEN/archive/refs/heads/main.zip) o `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git` y descomprime.
2. Arranca **`RAZDA4A_XEN.exe`** o **`RAZDA4A XEN.bat`**.
3. En el lanzador, elige el `.exe` del juego, un perfil y frame generation, y pulsa **Launch**.

No ejecutes el `.exe` del juego directamente. El runtime solo entra si lo arranca el lanzador.

`F2` o `~` alterna la cámara con el ratón (`Esc` suelta el cursor). `F3` alterna el FPS.

En Linux, `launch_linux_proton.sh`. Presets y Linux: [Cómo ejecutarlo](docs/es/running.md).

## Requisitos

| | Wave 11 |
|---|---|
| **GPU** | AMD RDNA2 / RDNA3 / RDNA4 (RX 6000 / 7000 / 9000), DX12 Ultimate y mesh shaders |
| **CPU** | x86-64-v3 (AMD Zen 2–5, Intel Haswell hasta Arrow Lake) |
| **Memoria** | 16 GB de RAM en SSD. Commit justo: pagefile de 24–32 GB (`pagefile_16gb_ram.ps1`). 8–12 GB de VRAM: Safe Mode |
| **SO** | Windows 10 / 11, o Linux con Proton / VKD3D-Proton |
| **No es esta build** | NVIDIA, Intel Arc, RDNA1, GTX 10/16, tarjetas de 4 GB y HDD |

La versión NVIDIA está en camino. AMD salió primero. Mientras tanto: [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA).

## Presets

| Archivo | Uso |
|---|---|
| `RAZDA4A XEN.bat` | Ventana del lanzador: `.exe` del juego, frame generation, Safe Mode en 8 GB |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB de RAM, páginas en NVMe |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado, más búferes de constantes in-place |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Solo nativo, frame generation apagado |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB de VRAM |

## FAQ

<details>
<summary>¿Incluye el juego?</summary>

No. Aporta tu copia. No hace falta un GDK filtrado. Aquí no hay enlaces de descarga del juego.
</details>

<details>
<summary>¿NVIDIA o Intel Arc?</summary>

Wave 11 es AMD RDNA. La versión NVIDIA está en camino. AMD salió primero. El fork de la comunidad está arriba. Intel Arc no está soportado. Un adaptador que no sea AMD puede quedar en negro o cerrarse.
</details>

<details>
<summary>¿8 GB de VRAM, o menos?</summary>

8–12 GB: Safe Mode (preset 4, o el lanzador lo elige en 8 GB). RX 7600: 12,7 nativos / 24,86 FG. 6 GB o menos no es una configuración soportada. RDNA1 y GTX 10/16 no tienen los mesh shaders que esta build necesita.
</details>

<details>
<summary>¿Ray tracing?</summary>

Apagado en Wave 11. El constructor de estructuras de ray tracing no se ejecuta.
</details>

<details>
<summary>¿Disco duro, o 16 GB de RAM?</summary>

Un HDD no está soportado. Usa un SSD NVMe o SATA. Con 16 GB de RAM, `pagefile_16gb_ram.ps1` o el preset 1 cubren un pagefile justo.
</details>

## Si algo falla

- El primer arranque compila pipelines (cerca de 1–3 minutos de tirones). Cierra, ejecuta `warm_shaders.bat` una vez y vuelve a abrir el lanzador.
- Windows Defender puede marcar el ZIP sin firmar como `Trojan:Script/Wacatac.H!ml`. Compara `RAZDA4A_XEN.exe` con [SHA256SUMS.txt](SHA256SUMS.txt): `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`.
- `collect_logs.bat`, o **Export Logs** en el lanzador, escribe `logs\bug_report_*.txt` en este PC.
- Linux a ~1 FPS o imagen vacía: usa `launch_linux_proton.sh`, que evita que `D3D12Core.dll` sustituya a vkd3d-proton.

[Cómo ejecutarlo](docs/es/running.md) · [Seguridad y compilación](docs/es/security-and-build.md)

## Acerca del proyecto

RAZDA4A XEN es una capa Direct3D 12 de código abierto para 16 GB de RAM, CPUs de 6 núcleos y 8–16 GB de VRAM. Wave 11 en `main` es el lanzador, los presets, las DLL y estas páginas. El código Rust está en [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).

## Créditos

- [VIctory](https://github.com/Marvisak/VIctory), de Marvisak, upstream, bajo GPL-3.0-or-later ([LICENSE](LICENSE)).
- `nathanbzrr` — Safe Mode de 8 GB, publicado en `main`.
- [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) — fork NVIDIA de la comunidad.

## Documentación

- [Rendimiento](docs/es/performance.md)
- [Cómo ejecutarlo](docs/es/running.md)
- [Hardware](docs/es/hardware.md)
- [Qué trae Wave 11](docs/es/technical-changes.md)
- [Límites conocidos](docs/es/known-limits.md)
- [Seguridad, checksums y compilación](docs/es/security-and-build.md)
- [Ramas y licencia](docs/es/project.md)
