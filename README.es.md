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
| VRAM dedicada | 9.92 GB (`5.06 GB` pool de texturas) | **12.11 GB** (`5.12 GB` pool de texturas; hasta `13.18 GB` / `6.40 GB` pool; auto 8–24 GB DXGI) |
| GPU compartida (`NON_LOCAL`) | 6.68 GB (5.58 GB Series S) | **2.18 GB** (`-61%` frente a base Series S; `2.01 GB` al 91% keep) |
| Tráfico copy-back | 27.0 MB / cuadro | **0.5 MB / cuadro** |
| CPU `preparing` | 45.8 ms | **32.2 – 35.4 ms** |
| Cuadro real (mín / med) | 68.6 / 73.1 ms | **64.3 / 66.2 – 69.6 ms** |
| FPS nativos y estabilidad (sin FG, mundo abierto estable) | 14.08 FPS (media 71.03 ms, p99 152.1 ms / 1% low 6.6 FPS) | **15.2 – 23.4 FPS** (p50 **60.4 ms** / 16.6 FPS, `list_al` **0.03 ms**, `alloc` **4.8 ms**, `0` bloqueos de 5–10 s, `0` OOM / `0` emergency reclaim) |
| Pase `heap buffers` (máx. / picos >50 ms) | 821.1 ms (8 picos >50 ms) | **33.8 ms (0 picos >50 ms; estable 8.6–10.0 ms)** |
| Esperas `settle` y `take()` (estable sin FG) | 0.2–2.6 ms settle / 0.75–1.07 ms `take()` | **0.0 ms settle / 0.31–0.47 ms `take()`** |
| FPS mostrados (FSR 3.1.6 FG) | 25–27 FPS (caídas VSync a 15–20) | **29.2 – 31.0 FPS** (sin caídas VSync) |
| HUD y subtítulos en FG | Ghosting y deformación | Limpio (`HUDLessColor` RGBA8 de 3 ranuras) |
| Windows 10 D3D12 | Instalación manual de Agility SDK | `D3D12Core.dll` incluido (Agility SDK 620) |

## Cambios técnicos

- **Memoria y OOM (`0x8007000E`):** pool global acotado de preludios `(64, 16)`; reciclaje inmediato de listas y asignadores (`recycle_landed`) con retiro diferido (`bury_lag`); `emergency_reclaim()` proactivo con <2.5 GiB de commit libre o >=88% de Shared GPU (`NON_LOCAL`), con expulsión LRU agresiva de texturas/staging/pools y límite dinámico de concurrencia en `thin` (1–2 hilos bajo presión); recuperación, reintento y reinicio/retiro seguro del asignador ante `E_OUTOFMEMORY` en `HostList::close()`, `make_list()` y `record_prelude()`; `MEM_RESET` en páginas físicas liberadas sin mapeo; correcciones upstream `#2143`, `#2234`, `#2462`, `#2844` (`CpuViews` RAII) y `#3540` (alineación de 16 bytes en vistas raw); páginas físicas opcionales en NVMe (`FILE_FLAG_DELETE_ON_CLOSE` en `%LOCALAPPDATA%\victory\physical`, >11 GB libres al cargar). Verificado en ejecución nativa de 180 s sin FG con archivo de paginación estándar (`VICTORY_FILE_PHYSICAL=0`): 0 errores, 0 OOM, `min_free_commit 3.47 GB`, `5.10–5.57 GB` libres en estado estable.
- **Presentación y Frame Generation:** captura `HUDLessColor` RGBA8 de 3 ranuras antes de componer UI/subtítulos/HUD; swapchain triple-buffer con `DXGI_PRESENT_ALLOW_TEARING`; AMD FSR 4.1.2 ML nativo (respaldo FSR 3.1 en GPUs anteriores).
- **Envío CPU y copy-back:** escrituras write-combined retenidas en VRAM (`27 -> 0.5 MB/cuadro`); salto rápido AVX2 `vptest` de 128 B y `merge_piece` vectorizado; pool lock-free work-stealing en `land`; shards write-watch alineados a 64 B; escaneo fusionado `ask_once`/`stale_under_flights`; `mirror` lock-free; `OnceLock` en firmas de comando; `Few<Barrier, 16>` en pila; `PLACED_TEXTURES` en 64 shards; `unfold` sin bloqueo y `wake()` lock-free; puesta a cero de `upload::ARENAS` fuera del mutex; prioridad elevada para hilos auxiliares en 6 núcleos; listas preplanificadas; hash antes de detile; reutilización de búferes de subida.
- **Auto-VRAM dinámico DXGI y reducción de Shared GPU:** escalado automático 8 / 12 / 16 / 24+ GB mediante `DedicatedVideoMemory` y `DXGI_QUERY_VIDEO_MEMORY_INFO` en vivo (`VICTORY_VRAM_MODE=auto|8gb|12gb|16gb|24gb|legacy`, soporte `"auto"` y sobrescrituras manuales, detección UMA y descenso escalonado por Commit). En GPUs de 16 GB adapta el presupuesto según la RAM: `<=20 GB` RAM usa `84%` keep / `90%` room (`12.11 GB` VRAM, `2.18 GB` Shared GPU, sin paginación de working set, `+55%` en 1% low FPS) y `>=32 GB` RAM usa `91%` keep / `95%` room (`~13.2–14.2 GB` VRAM). Ventanas VRAM directas sin staging (`CardCpu::Direct`), copias CPU compactas (`CardCpu::Cpu`), cola `DIRTY_CARDED` O(dirty), expulsión incremental de ventanas con histéresis (máx. 8/s), scratch compartido para depth-stencil (upstream `#2162`), corrección del umbral `host_pressure` en 16 GB RAM (`send_allocs` `117/15s -> 0/15s`), autodetección NVMe (`VICTORY_FILE_PHYSICAL=auto`) y estadísticas de memoria propias del título en `XMemGetWorkingSetStatistics`.
- **Frame pacing y eliminación de picos (nativo sin FG):** `sweep()` asíncrono incremental (`victory-sweep`, `VICTORY_SWEEP_ASYNC=1`) eliminando el drenaje síncrono `settle(WHY_SYNC)` cada 1.5 s; recolección en segundo plano `bury()` con limitación (`victory-bury`, `VICTORY_BURY_ASYNC=1`); esperas `WHY_FENCE`/`WHY_VALUE` por `Line.seq` sin bloquear `host.submitted`; omisión de `sync_heaps` redundante en `execute_own` (`0.0` rondas sin marca); escaneo de 64 bits `AtomicU64` en `port_descriptors::take()` (`2–3×` más rápido); `frame::wait` de precisión sin `Sleep(0)`; telemetría por cuadro `present pacing` (`avg / p50 / p95 / p99 / 1% low / max / >50 ms / >100 ms`, `VICTORY_FRAME_TIMES_FILE`) y `heap buffers` plano (`VICTORY_TEXTURE_STAGE_MS=4`, `VICTORY_TEXTURE_CATCHUP=0`, `0` picos >50 ms).
- **Fase 2 — optimización de envío CPU y descriptores (`feat/phase2-30fps`):** caché `VirtualQuery` `MappedCache` con invalidación por rango (`12.56 ms -> 0.00 ms`, `own_cs` `15.1 -> 2.5 ms`); tabla atómica `ENTRY_GENS` + caché TLS de 8192 entradas (`100.0%` de aciertos); ruta rápida inline-CBV, asignador slab dinámico de 128 ranuras + `release_dynamic_batch` en `draw_bind::tables` y lotes `RETIRE_TLS`; reciclaje de listas sin pérdidas (`BACKS_MOST=512`, `IDLE_BYTES=512 MiB`, `list::boxed()` → `fresh_list()`, `list_al` `-99.7%`, `alloc` `-83%`, `lists` churn `+0`); `BufferSweep` incremental (`HBP_SPANS` `10.94 -> 0.51 ms`); pools ampliados `PRELUDES_MOST` `(512, 128)`, `SPARE_BATCH` `(128/256)`, `VICTORY_LIST_IDLE_MOST=1024`; cola prioritaria compute PSO + 4 workers compute/screen con límite de espera tardía (`VICTORY_COMPUTE_LATE_WAIT_MS=400`, cero bloqueos de 5–10 s, `p99.9` `-80%`, `1% low` `3.2×`); corrección de umbrales `residency` en 16 GB RAM (`unmap_null` `-97%`, `VICTORY_KEEP_SHOWN=2`); mapa `gpu_written_pages` de 64 KiB; HUD arriba a la derecha (`VICTORY_PANEL_POS=top-right`) y casilla para activar/desactivar Frame Generation en el lanzador.
- **Correcciones y controles:** presupuesto de espera compute (`5000 ms`) corrigiendo rostros faltantes; omisión de shader con fallo en RDNA4 (`240cb516c1af0bc3`); filtro de sondas violetas; flips asíncronos seguros; reintento de detile; teclado y ratón PC (`F2`/`~` cámara, `F3` panel FPS arriba a la derecha).

## Límites conocidos (en desarrollo en `dev`)

- Los 30 FPS nativos (`<= 33.3 ms`) siguen optimizándose en `dev`: `victory-submit` ya bajó a `34.4 ms/cuadro`; el siguiente foco es la grabación paralela (`victory-list-0..3`) y copias CBV.
- Los picos de Commit en la transición prólogo -> mundo abierto en archivos de paginación muy ajustados se mitigan en `dev` con `emergency_reclaim()` y autodetección NVMe (`VICTORY_FILE_PHYSICAL=auto`).

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
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

SHA-256 de `RAZDA4A_XEN.exe`: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`

## Ramas y licencia

- `main`: binario compilado, DLLs de ejecución, presets y documentación.
- `dev`: código fuente completo en Rust, pruebas, CI y `AGENTS.md`.

Derivado de VIctory bajo **GPL-3.0-or-later** ([LICENSE](LICENSE)). Código fuente completo en [`dev`](https://github.com/crossducking/RAZDA4A-XEN/tree/dev).
