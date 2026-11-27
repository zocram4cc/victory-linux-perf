[English](README.md) | [Русский](README.ru.md) | [简体中文](README.zh.md) | Español | [Português (BR)](README.pt-BR.md)

# RAZDA4A XEN

Capa de ejecución Direct3D 12 para Windows enfocada en **16 GB de RAM**, **CPUs de 6 núcleos**, **GPUs de 8–16 GB VRAM** y **Windows 10**.

No incluye archivos de juego, recursos ni ejecutables. Debes aportar tu propia copia de *El Gran Simulador de Préstamo de Autos del Estudio Estelar*.

## Diferencias frente al VIctory original

El proyecto VIctory original se perfila principalmente en 64 GB de RAM, GPUs de 24 GB y Windows 11 (WDDM 3.0+ con `D3D12_HEAP_TYPE_GPU_UPLOAD`). En equipos con 16 GB de RAM y Windows 10 provoca cierres `0x8007000E` (`E_OUTOFMEMORY`), caídas a la mitad por VSync, ghosting de HUD en Frame Generation y cuello de botella en el hilo de envío CPU.

Medido en **Ryzen 5 5600 (6C/12T), Radeon RX 9060 XT 16 GB, 16 GB DDR4, Windows 10 (19045)** — 1080p perfil Series S, sin RT, ejecuciones continuas de 180 s, 0 errores:

| Métrica | Base (configuración upstream) | RAZDA4A XEN |
|---|---|---|
| Private Commit (estable) | 21.8 – 30+ GB (`0x8007000E` OOM) | **19.3 GB** (6.2 GB RAM WS, 5.5 GB libres) |
| VRAM dedicada | 9.92 GB (`5.06 GB` pool de texturas) | **8.18 GB** (`3.65 GB` pool de texturas) |
| GPU compartida (`NON_LOCAL`) | 6.68 GB | **5.45 GB** |
| Tráfico copy-back | 27.0 MB / cuadro | **0.5 MB / cuadro** |
| CPU `preparing` | 45.8 ms | **32.2 – 35.4 ms** |
| Cuadro real (mín / med) | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| FPS nativos (sin FG) | ~13.7 FPS | **14.5 – 15.5 FPS** |
| FPS mostrados (FSR 3.1.6 FG) | 25–27 FPS (caídas VSync a 15–20) | **29.2 – 31.0 FPS** (sin caídas VSync) |
| HUD y subtítulos en FG | Ghosting y deformación | Limpio (`HUDLessColor` RGBA8 de 3 ranuras) |
| Windows 10 D3D12 | Instalación manual de Agility SDK | `D3D12Core.dll` incluido (Agility SDK 620) |

## Cambios técnicos

- **Memoria y OOM (`0x8007000E`):** pool global acotado de preludios `(64, 16)`; reciclaje inmediato de listas y asignadores (`recycle_landed`) con retiro diferido (`bury_lag`); recuperación y reintento ante `E_OUTOFMEMORY` en `make_list()` y `record_prelude()`; control `NON_LOCAL` (`92%`, histéresis `-10%`); páginas físicas opcionales en NVMe (`FILE_FLAG_DELETE_ON_CLOSE` en `%LOCALAPPDATA%\victory\physical`, >11 GB libres al cargar).
- **Presentación y Frame Generation:** captura `HUDLessColor` RGBA8 de 3 ranuras antes de componer UI/subtítulos/HUD; swapchain triple-buffer con `DXGI_PRESENT_ALLOW_TEARING`; AMD FSR 4.1.2 ML nativo (respaldo FSR 3.1 en GPUs anteriores).
- **Envío CPU y copy-back:** escrituras write-combined retenidas en VRAM (`27 -> 0.5 MB/cuadro`); salto rápido AVX2 `vptest` de 128 B y `merge_piece` vectorizado; pool lock-free work-stealing en `land`; shards write-watch alineados a 64 B; escaneo fusionado `ask_once`/`stale_under_flights`; `mirror` lock-free; `OnceLock` en firmas de comando; `Few<Barrier, 16>` en pila; `PLACED_TEXTURES` en 64 shards; `unfold` sin bloqueo y `wake()` lock-free; puesta a cero de `upload::ARENAS` fuera del mutex; prioridad elevada para hilos auxiliares en 6 núcleos; listas preplanificadas; hash antes de detile; reutilización de búferes de subida.
- **Correcciones y controles:** presupuesto de espera compute (`5000 ms`) corrigiendo rostros faltantes; omisión de shader con fallo en RDNA4 (`240cb516c1af0bc3`); filtro de sondas violetas; flips asíncronos seguros; reintento de detile; teclado y ratón PC (`F2`/`~` cámara, `F3` panel FPS).

## Límites conocidos (en desarrollo en `dev`)

- Los 30 FPS nativos (`<= 33.3 ms`) están limitados por el hilo `victory-submit` (`49–60 ms/cuadro`; GPU al 52–58%).
- El perfil Series S usa ~8.2 GB VRAM en tarjetas de 16 GB; el presupuesto dinámico de VRAM está en progreso en `dev`.
- La transición prólogo -> mundo abierto puede superar el archivo de paginación por defecto en 16 GB RAM (`0x8007000E`); la solución en código sin tocar la paginación está en progreso en `dev`.

## Inicio rápido

1. Descarga `main` (**Code -> Download ZIP** o `git clone --branch main https://github.com/crossducking/RAZDA4A-XEN.git`) y descomprime (ej. `D:\RAZDA4A-XEN\`). Las DLL necesarias ya están incluidas.
2. Edita **`config.bat`** apuntando a tu carpeta y **`tu_archivo_ejecutable.exe`**:
   ```bat
   set "GAME_DIR=D:\Ruta\A\Tu\Carpeta"
   set "GAME_EXE=%GAME_DIR%\tu_archivo_ejecutable.exe"
   ```
3. Ejecuta **`RAZDA4A XEN.bat`** o elige un preset en `presets\`:

| Archivo | Perfil |
|---|---|
| `RAZDA4A XEN.bat` | Equilibrado (predeterminado): 29–31 FG FPS, ~8.2 GB VRAM, 6.2 GB RAM, HUDLess FG + Tearing |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Paginación ajustada / navegador abierto: páginas físicas en NVMe, ~8.0 GB VRAM, >11 GB libres al cargar |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Equilibrado + búferes de constantes en sitio (experimental) |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Renderizado nativo sin Frame Gen, latencia mínima |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB VRAM / modo seguro: límites estrictos, VSync activado, escaneo completo de descriptores |

- **`F2` / `~`** — alternar ratón (`Esc` libera cursor)
- **`F3`** — panel FPS
- **`warm_shaders.bat`** — ejecutar una vez tras cerrar la primera sesión para precompilar shaders
- **`backup_saves.bat`** — respaldar/restaurar `data\`
- **`pagefile_16gb_ram.ps1`** — script opcional de administrador (paginación SSD 16–32 GB)

## Compilar desde código fuente y verificación

Si prefieres no usar binarios sin firmar, compila directamente desde la rama `dev`:

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --locked -p victory
```

SHA-256 de `RAZDA4A_XEN.exe`: `e90b23f05f6d3caa488beaf3afc4cd0e94151b75a33606bfd75594a373e4f18b`

## Ramas y licencia

- `main`: binario compilado, DLLs de ejecución, presets y documentación.
- `dev`: código fuente completo en Rust, pruebas, CI y `AGENTS.md`.

Derivado de VIctory bajo **GPL-3.0-or-later** ([LICENSE](LICENSE)). Código fuente completo en [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
