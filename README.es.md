[English](README.md) | [Русский](README.ru.md) | [简体中文](README.zh.md) | Español | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

Capa de ejecución Direct3D 12 para Windows orientada a **GPUs AMD RDNA2 / RDNA3 / RDNA4**, **16 GB de RAM**, **CPUs de 6 núcleos**, **8–16 GB de VRAM** y **Windows 10 / 11** (y Linux mediante Proton / VKD3D-Proton).

No se incluyen archivos, assets ni ejecutables del juego. Debes aportar tu propia copia de *El Gran Simulador de Préstamo de Coches del Estudio Estrellado*.

## Rendimiento Medido (FPS Nativos vs. Frame Generation)

Todas las métricas separan **FPS nativos (Frame Gen OFF)** de **FPS mostrados (FSR 3.1.6 Frame Gen 2x)** indicando escena y hardware exactos.

### Base 16 GB VRAM / 16 GB RAM (Mundo Abierto, 1080p Series S, RT OFF)

Medido en **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 GB, 16 GB DDR4, Windows 10 (19045)** — pruebas de 180 s en mundo abierto + prueba de estabilidad Wave 11 de 22 minutos (18.393 cuadros), 0 errores, 0 OOM:

| Métrica | Base (config upstream) | RAZDA4A XEN (Wave 7 / Wave 11) |
|---|---|---|
| **FPS nativos (FG OFF, Mundo Abierto)** | 14,08 media (71,03 ms, p99 152,1 ms / 1% low 6,6 FPS) | **18,19 p50** (54,97 ms), **16,17** media, 15,2–23,4 estable, p99.9 **478,6 ms** |
| **FPS mostrados (FSR 3.1.6 FG 2x)** | 25–27 FPS (caídas a 15–20 por VSync) | **29,2 – 31,0 FPS** estables (anillo RGBA8 `HUDLessColor`, sin caídas VSync) |
| Private Commit (estable) | 21,8 – 30+ GB (`0x8007000E` OOM) | **19,3 GB** (6,2 GB RAM WS, 5,5 GB commit libre) |
| VRAM dedicada | 9,92 GB (`5,06 GB` pool de texturas) | **12,11 GB** (auto 8–24 GB DXGI; `0` rechazos VRAM en 22 min en Wave 11) |
| GPU compartida (`NON_LOCAL`) | 6,68 GB (5,58 GB Series S) | **2,18 GB** (`-61%` vs Series S) |
| Tráfico copy-back | 27,0 MB / cuadro | **0,5 MB / cuadro** |
| CPU `preparing` / `heap buffers` | 45,8 ms / 821,1 ms máx. | **32,2 – 35,4 ms** / **33,8 ms máx.** (0 picos >50 ms) |
| Vistas raw (`LIST_VIEWS`, Wave 11) | 372,1–575,8 null/cuadro (árboles/objetos desaparecían) | **0,0 null/cuadro** (28.212/cuadro; árboles, follaje, vallas y postes corregidos) |
| Windows 10 D3D12 | Configuración manual Agility SDK | `D3D12Core.dll` incluido (Agility SDK 620) |

### Verificación 8 GB VRAM — Safe Mode (PR #1 de `nathanbzrr`)

Probado en **Ryzen 5 5500X3D, Radeon RX 7600 8 GB, 32 GB DDR4, Windows 11 (26340)** — 1080p Series S, Safe Mode (`65%` texturas / `2048 MB` targets / `768 MB` búferes), FSR FG 2x, 180 s:

| Métrica | Antes del ajuste 8 GB | Después del ajuste a presupuesto vivo |
|---|---:|---:|
| **FPS nativos (sin FG)** | 3,67 | **12,7** (media 79,01 ms, p50 68,90 ms, p99 **242,26 ms** vs 1047,30 ms) |
| **FPS mostrados (FSR FG 2x)** | 7,32 | **24,86** (1803 generados / 1803 renderizados) |
| VRAM pico / presupuesto vivo | 6548 MB | **6169 MB** / 5934–5981 MB (`0` errores, `0` OOM, `0` device removal) |

## Cambios Técnicos

- **Wave 11 (follaje/objetos y binario portable):** corrige `372–575` vistas raw `NULL` por cuadro en `LIST_VIEWS` (elimina la desaparición de árboles, follaje, vallas, postes y señales); soporte `SW_64KB_D`; ajuste de residencia 16 GB (`0` rechazos VRAM en 22 min); binario portable **`x86-64-v3`** (AVX2/BMI2/FMA en AMD Zen 2–5 e Intel Haswell–Arrow Lake).
- **Wave 6–7 (texturas, descriptores y estabilidad):** caché TLS de 8192 ranuras (`100%` aciertos), cola de texturas intercalada (retraso en cinemáticas `3936 -> 0`), prioridad normal de CPU y registro en archivo `VICTORY_LOG_FILE`.
- **Memoria y OOM (`0x8007000E`):** `recycle_landed`, `emergency_reclaim()` proactivo, recuperación `E_OUTOFMEMORY`, Auto-VRAM DXGI (`8/12/16/24 GB`) y páginas físicas NVMe (`VICTORY_FILE_PHYSICAL=auto`).
- **Presentación y controles:** `HUDLessColor` RGBA8 para FSR 3.1.6 FG, tearing swapchain, FSR 4.1.2 ML nativo, espera compute `5000 ms` (rostros), escudo RDNA4 (`240cb516c1af0bc3`), ratón/teclado PC (`F2`/`~` cámara, `F3` HUD FPS).

## Límites Conocidos (Trabajo Activo en `dev`)

- Las zonas urbanas densas aún pueden bajar a `5–10` FPS nativos y generar pausas largas en sesiones extensas (**Wave 12** en curso en `dev`).
- Ray Tracing permanece desactivado mientras se estabilizan el render base y los 30 FPS nativos.
- El primer inicio compila shaders PSO (1–3 min con tirones); ejecuta `warm_shaders.bat` tras cerrar para que el segundo inicio sea mucho más fluido.

## Compatibilidad de Hardware y FAQ

| Pregunta | Respuesta |
|---|---|
| **¿GPUs AMD soportadas?** | **AMD RDNA2 / RDNA3 / RDNA4** (RX 6000 / 7000 / 9000; requiere DX12 Ultimate + Mesh Shaders). |
| **¿NVIDIA / Intel Arc?** | **No soportado en este fork.** En RTX hay pantalla negra/crashes; usa forks NVIDIA de la comunidad como [`trustno1-coder/VIctory-NVIDIA`](https://github.com/trustno1-coder/VIctory-NVIDIA). |
| **¿CPU Intel + GPU AMD?** | **Sí.** Desde Wave 11 el binario usa `-C target-cpu=x86-64-v3`. |
| **¿8 GB / 6 GB / <=4 GB VRAM?** | **8 GB:** funciona con **Safe Mode** (12,7 nativos / 24,86 FG en RX 7600; probado en RX 6600). **6 GB:** experimental/lento (Safe Mode + FG OFF). **<=4 GB, RDNA1 (RX 5500/5700), GTX 10/16:** no soportado (sin Mesh Shaders o poca VRAM). |
| **¿16 GB RAM / HDD?** | **16 GB RAM:** soportado en **SSD NVMe/SATA** con pagefile de 24–32 GB (`pagefile_16gb_ram.ps1`) o preset `Low-Commit NVMe`. **HDD:** no soportado. |
| **¿Necesito el GDK filtrado?** | **No.** Runtime open-source independiente (`GPL-3.0-or-later`). |
| **¿Dónde enviar bugs y logs?** | Ejecuta **`collect_logs.bat`** (o **Export Logs** en el lanzador) para generar `logs\bug_report_*.txt` sin rutas privadas y adjúntalo en [GitHub Issues](https://github.com/crossducking/RAZDA4A-XEN/issues). |

## Inicio Rápido (Windows)

1. Descarga `main` (**Code -> Download ZIP** o `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) y descomprime. Incluye `D3D12\D3D12Core.dll` y `fidelityfx\*.dll`.
2. Ejecuta **`RAZDA4A XEN.bat`**, pulsa **Browse...** para elegir **`your_executable.exe`** (guardado en UTF-8 en `config.bat`), selecciona perfil y Frame Generation y pulsa **Launch**.
   - No abras el ejecutable del juego con doble clic. Windows pedirá DLL de consola que solo aporta el lanzador.
   - Deja junta toda la carpeta descomprimida, incluidas `D3D12\`, `fidelityfx\` y `presets\`.
   - Lanzadores externos (`VILauncher`, etc.): usa **`RAZDA4A XEN.bat --no-gui`**.

| Archivo | Uso |
|---|---|
| `RAZDA4A XEN.bat` | Lanzador principal (selector `.exe`, FG ON/OFF, auto-Safe-Mode 8 GB, Warm Shaders y Export Logs) |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB RAM / pagefile pequeño: páginas físicas en NVMe |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado + CBV in-place experimental (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Nativo sin Frame Gen, latencia mínima |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | Safe Mode 8–12 GB VRAM (`65%` / `2048 MB` targets / `768 MB` búferes, VSync ON) |
| `warm_shaders.bat` / `collect_logs.bat` | Precompilar PSOs / exportar reporte sanitizado con `logs\last_run.log` |

Controles: **`F2` / `~`** alterna cámara con ratón (`Esc` libera cursor), **`F3`** alterna HUD FPS.

## Inicio Rápido (Linux — Proton / Wine / VKD3D-Proton)

1. Instala **Proton Experimental / GE-Proton** (`umu-run`, Steam o Lutris) con **Mesa RADV** en GPU AMD RDNA2+.
2. Ejecuta `launch_linux_proton.sh` (configura automáticamente `WINEDLLOVERRIDES="d3d12,d3d12core=n"` para evitar que `D3D12Core.dll` de Windows 10 bloquee `vkd3d-proton` y cause 1 FPS / pantalla 3D vacía):
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```

## Seguridad, Falsos Positivos y Compilación desde Código Fuente

Brave/Edge y Windows Defender ML pueden marcar ZIPs sin firmar con `.exe` + `.bat` + `.ps1` como **`Trojan:Script/Wacatac.H!ml`**. Verifica el SHA-256 o compila tú mismo desde `dev`:

```powershell
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (todos los DLL en [`SHA256SUMS.txt`](SHA256SUMS.txt)).

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## Ramas y Licencia

- `main`: binario precompilado, DLLs, presets, lanzador, checksums y documentación.
- `dev`: código fuente Rust completo, tests, `xtask policy`, CI y `AGENTS.md`.

Derivado de VIctory bajo **GPL-3.0-or-later** ([LICENSE](LICENSE)). Código completo en [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
