[English](../en/running.md) | [Русский](../ru/running.md) | [简体中文](../zh/running.md) | [Português (BR)](../pt-BR/running.md)

# Cómo ejecutarlo

[Volver al README](../../README.es.md)

Arranca con el lanzador incluido: **`RAZDA4A_XEN.exe`** o **`RAZDA4A XEN.bat`**. No ejecutes el ejecutable del juego directamente. El runtime solo se aplica si el lanzador arranca el juego.

No se incluyen archivos, assets ni ejecutables del juego. Debes aportar tu propia copia de *El Gran Simulador de Préstamo de Coches del Estudio Estrellado*.

## Inicio Rápido (Windows)

1. Descarga `main` (**Code -> Download ZIP** o `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) y descomprime. Incluye `D3D12\D3D12Core.dll` y `fidelityfx\*.dll`.
2. Arranca con **`RAZDA4A_XEN.exe`** o **`RAZDA4A XEN.bat`**. No inicies el `.exe` del juego con doble clic.
   - **`RAZDA4A XEN.bat`** abre la ventana del lanzador: pulsa **Browse...** para elegir **`your_executable.exe`** (guardado en UTF-8 en `config.bat`), selecciona perfil y Frame Generation y pulsa **Launch**.
   - Lanzadores externos (`VILauncher`, etc.): usa **`RAZDA4A XEN.bat --no-gui`**.

| Archivo | Uso |
|---|---|
| `RAZDA4A XEN.bat` | Lanzador principal (selector `.exe`, FG ON/OFF, auto-Safe-Mode 8 GB, Warm Shaders y Export Logs) |
| `RAZDA4A_XEN.exe` | Runtime que arranca el lanzador. No lo saltes para ejecutar el `.exe` del juego directamente. |
| `presets\1_Low_Commit_NVMe_16GB.bat` | 16 GB RAM / pagefile pequeño: páginas físicas en NVMe |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado + CBV in-place experimental (`VICTORY_PORT_CBV=1`) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Nativo sin Frame Gen, latencia mínima |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | Safe Mode 8–12 GB VRAM (`65%` / `2048 MB` targets / `768 MB` búferes, VSync ON) |
| `warm_shaders.bat` / `collect_logs.bat` | Precompilar PSOs / escribir un reporte sanitizado con `logs\last_run.log` en este PC. |

Controles: **`F2` / `~`** alterna cámara con ratón (`Esc` libera cursor), **`F3`** alterna HUD FPS.

## Inicio Rápido (Linux — Proton / Wine / VKD3D-Proton)

1. Instala **Proton Experimental / GE-Proton** (`umu-run`, Steam o Lutris) con **Mesa RADV** en GPU AMD RDNA2+.
2. Ejecuta `launch_linux_proton.sh` (configura automáticamente `WINEDLLOVERRIDES="d3d12,d3d12core=n"` para evitar que `D3D12Core.dll` de Windows 10 bloquee `vkd3d-proton` y cause 1 FPS / pantalla 3D vacía). El script arranca `RAZDA4A_XEN.exe`. No ejecutes el ejecutable del juego directamente.
   ```bash
   chmod +x launch_linux_proton.sh
   GAME_DIR="/path/to/unpacked" \
   GAME_EXE="/path/to/unpacked/your_executable.exe" \
   RUNNER="umu-run" \
   XEN_PROFILE="safe" \
   XEN_FRAMEGEN="0" \
   ./launch_linux_proton.sh
   ```

## Ver también

- [Rendimiento](performance.md)
- [Hardware](hardware.md)
- [Qué trae Wave 11](technical-changes.md)
- [Límites conocidos](known-limits.md)
- [Seguridad, checksums y compilación](security-and-build.md)
- [Ramas y licencia](project.md)
