# victory-royale (privado)

VIctory centrado en Linux. El objetivo es una arquitectura que permita jugar **de forma fluida y estable** a un título de Xbox Series con GE-Proton, en lugar de los 2-4 fps de los forks actuales.

> Este repositorio no contiene archivos del juego. Cada uno usa su propia copia, volcada de una consola suya. El código es GPL-3.0-or-later, como VIctory.

**Índice**

1. [De dónde parte](#1-de-dónde-parte)
2. [Por qué va lento hoy](#2-por-qué-va-lento-hoy)
3. [Cómo funciona hoy el envío a la GPU](#3-cómo-funciona-hoy-el-envío-a-la-gpu)
4. [La ruta](#4-la-ruta)
   - [Fase 0: base, escena de referencia y medición](#fase-0-base-escena-de-referencia-y-medición)
   - [Fase 1: abaratar las copias que hay hoy](#fase-1-abaratar-las-copias-que-hay-hoy)
   - [Fase 2: memoria compartida CPU/GPU, sin copias](#fase-2-memoria-compartida-cpugpu-sin-copias)
   - [Fase 3: trabajo en paralelo y ritmo de frames](#fase-3-trabajo-en-paralelo-y-ritmo-de-frames)
   - [Fase 4: backend Vulkan nativo](#fase-4-backend-vulkan-nativo)
   - [Fase 5: estabilidad y completitud visual](#fase-5-estabilidad-y-completitud-visual)
5. [Cómo trabajamos](#5-cómo-trabajamos)
6. [Riesgos y decisiones abiertas](#6-riesgos-y-decisiones-abiertas)
7. [Compilar y ejecutar](#7-compilar-y-ejecutar)
8. [Ramas y remotos](#8-ramas-y-remotos)
9. [Estado](#9-estado)

---

## 1. De dónde parte

La base es [JoshuuDev/victory-amd](https://github.com/JoshuuDev/victory-amd) (`003beef`, 25-nov-2026). Es el fork que mejor renderiza el mundo, y viene en esta cadena:

| Capa | Repositorio | Qué aporta |
|---|---|---|
| 1 | [Marvisak/VIctory](https://github.com/Marvisak/VIctory) `ffe310e` | el loader, la emulación del sistema de Xbox (HLE), el backend D3D12 y el traductor de shaders |
| 2 | [fxceless/victory-fork](https://github.com/fxceless/victory-fork) | que el mundo se dibuje: cinemáticas y prólogo con texturas |
| 3 | [myokex/victory-amd](https://github.com/myokex/victory-amd) | arreglos para AMD (texturas por memoria real, dispatches partidos, vistas recortadas) |
| 4 | JoshuuDev/victory-amd | arreglo de DirectStorage, más shaders traducidos, bloqueo de remapeo por cada 16 MB, volcado de frametime |

**Por qué no se parte de [ligmaSec/victory-royale](https://github.com/ligmaSec/victory-royale):**

- Es el VIctory oficial con unos 700 commits de PRs abiertos, más un lanzador y una CI para Linux.
- No lleva el trabajo de renderizado de fxceless; según su README, el mundo sale casi negro.
- Fusionarlo con joshuu da **494 conflictos en 63 archivos**, casi todos en el núcleo de GPU, porque son dos reescrituras independientes del mismo código.

Su historia completa se conserva en la rama `royale-base`. Lo que interesa de ahí se porta a mano en la fase 0.

## 2. Por qué va lento hoy

Estas medidas son de la sesión del 24-nov-2026, con el fork de fxceless `4e17478`. Las de la base actual se toman en la fase 0.

- **Equipo:** RX 7900 XT con RADV (Mesa 26.3.1), Ryzen 9 9900X, GE-Proton11-14 y el [wineserver parcheado](https://github.com/datingesp-hash/victory-proton).
- **Escena:** el patio del prólogo, con los switches por defecto.

| Medida | Valor |
|---|---|
| frame completo | ~600 ms (1,6-2,1 fps) |
| VIctory preparando el frame | 100-130 ms, de ellos ~50-60 ms copiando **150-190 MB** hacia la GPU |
| refresco de texturas | 15-35 ms por frame |
| GPU | 75-105 ms; ocupada **25-39 %** del frame |
| copia de vuelta a la memoria del título | ~50 ms, **70-100 MB** |
| pasadas de hash sobre copias en la GPU | 1.100-1.300 por frame, ~1.370 trozos traídos (~89 MB) |
| CPU del proceso | ~1,1 núcleos de 24 (67 % código PE: título + VIctory; Wine ~15 %; RADV 2,5 %) |
| esperas del título en fences de la GPU | ninguna |
| map/unmap de vistas (con el wineserver parcheado) | 500-1.200 por segundo, 9-15 µs cada una |

En la Xbox, la CPU y la GPU comparten **una sola memoria**. En el PC están separadas, así que en cada frame VIctory tiene que:

1. Averiguar qué ha escrito el título, protegiendo páginas y calculando hashes.
2. Subirlo a la GPU.
3. Dejar que la GPU dibuje.
4. Bajar lo que la GPU escribió a la memoria del título.

Todo esto ocurre **en serie**: el título, después la preparación, la GPU y la copia de vuelta. No hay nada saturado: la GPU espera a la CPU, y la CPU espera a las copias. En Windows pasa lo mismo, por eso fxceless también va a ~2 fps.

**Conclusión:** subir los fps exige **dejar de copiar la memoria**, no optimizar los shaders ni el streaming. Los shaders ya se guardan en disco y apenas afectan a los fps estables. La GPU no es el límite.

## 3. Cómo funciona hoy el envío a la GPU

Este es el mapa del código que la ruta va a cambiar. Las rutas son relativas a `crates/`.

| Pieza | Dónde | Qué hace hoy |
|---|---|---|
| Memoria del título | `victory-hle/src/modules/xmem.rs` | La memoria de GPU del título es memoria del PC con *write watch*. Las páginas físicas son un *pool* de secciones `SEC_RESERVE`, y `map_alias` y `map` las muestran como vistas de 64 KiB. Hay un bloqueo de remapeo, que en joshuu es uno por cada 16 MB. |
| Recursos | `victory-gpu/src/resource.rs` | Los heaps y recursos son memoria del PC, y la dirección de GPU es la de CPU, como en la consola. Las texturas tienen copia en el host. |
| Copias de buffers | `victory-gpu/src/shadow.rs` | Cada buffer del título que usa un draw tiene una copia persistente en el host. Antes de cada envío se actualizan los rangos que se van a leer. |
| Qué escribió el título | `victory-gpu/src/watch.rs` | Pregunta al *write watch* (páginas de 4 KiB escritas) y lleva un diario para copias que se solapan. Con la protección de vistas, una escritura por un alias también marca los demás. |
| Texturas | `victory-gpu/src/texture.rs` | *Detile* de la memoria de consola a un *staging*, y después copia a la textura del host, cuando cambió. |
| Envío | `victory-gpu/src/host.rs`, `Host::submit` | **Un envío cada vez, de la preparación a la copia de vuelta**: `run_deferred` y `batch.flush`, después `card_before` (copias en VRAM y hashes), `ExecuteCommandLists` + `Signal`, la espera, `card_back` + `hand_back` + `read_backs`. El propio código lo marca como atajo provisional: *"every submission is waited for, so CPU and GPU never overlap"*. |
| Envío adelantado | `host.rs`, `late` y `relay` | `VICTORY_LATE_WAIT` y `VICTORY_RELAY` intentan solapar, pero en AMD fallan al terminar la carga (GPUVM fault, [#184](https://github.com/fxceless/victory-fork/issues/184)). |
| Memoria en sitio | `victory-gpu/src/inplace.rs` | `VICTORY_INPLACE=1/2` abre memoria privada del título como heap de la GPU y coloca un buffer encima, sin copia. **No acepta el *pool* de páginas**, porque su sección es `SEC_RESERVE` y sus vistas cambian. `VICTORY_INPLACE_PROBE` comprueba qué permite el driver. |
| Colas y fences | `victory-gpu/src/queue.rs`, `fence.rs` | Las listas se ejecutan y se espera por ellas, así que las señales del título se completan al instante. |
| Ritmo de frames | `victory-gpu/src/frame.rs` | Un reloj de intervalo que imita el vblank de la consola. |
| Medición | `frametime.rs`, `copystat.rs` | `VICTORY_FRAMETIME_DUMP=1` escribe un CSV por intervalo, con frame, preparación, GPU, copias, MB, pasadas y CPU. `VICTORY_COPY_MODE=plain/none` cambia o se salta las copias, solo para medir tiempos. |

## 4. La ruta

El orden es fijo, y cada fase depende de lo que mide la anterior.

- Una fase se cierra cuando se cumplen sus **criterios de salida** en la escena de referencia.
- Los objetivos de fps son **hipótesis**: se confirman o se corrigen con medidas, y se anotan en `docs/diario.md`.
- Las estimaciones de tiempo son orientativas.

### Fase 0: base, escena de referencia y medición

**Objetivo:** medidas repetibles en las que confiar, y saber desde el principio cuánto se puede ganar.

**Estimación:** 1-2 días.

#### 0.1 Base y lanzador

1. `main` es joshuu `003beef` con este README. Ya está hecho.
2. Un perfil `royale` en el lanzador `victory-proton`, que apunta a este repositorio (`main`). Es privado, así que git se autentica con `gh auth setup-git`.
3. Comprobar que compila con `cargo xwin` y que llega al mundo **igual que joshuu**: en el mismo sitio, sin perder el dispositivo.

#### 0.2 Escena de referencia

1. **Escena A, estática:** una partida guardada en el patio del prólogo. Se carga, se deja la cámara quieta y se miden 60 s a partir de 20 s después del primer frame del mundo.
2. **Escena B, en movimiento:** desde la misma partida, caminar en línea recta 30 s.
3. La partida es del usuario y **no entra en el repositorio**. Se copia a la carpeta de datos del perfil (`.../perfiles/royale/data/<título>/saves`).
4. Cómo reproducirla, en `docs/bench.md`: qué partida, qué hacer, cuánto esperar y qué switches usar.

#### 0.3 `scripts/bench.sh`

Mide desde Linux, sin tocar el código de VIctory.

- **Lanzamiento:** lanza el perfil con `VICTORY_FRAMETIME_DUMP=1` y con los switches que se le pasen. El CSV va a `<cache>/frametime.csv`.
- **Ventana de medida:** en la terminal pide *Enter* cuando estás en la escena y mide una ventana de N segundos (60 por defecto).
- **Durante la ventana**, muestrea la CPU del proceso y la de `wineserver` desde `/proc`.
- **Resultados:** los guarda en `bench/<fecha>-<etiqueta>/`, que no se sube al repositorio:
  - `frametime.csv` recortado a la ventana;
  - `summary.json` con la media, p50, p99 y el peor de cada columna: fps, `ms_per_frame`, `cpu_prepare_ms`, `gpu_ms`, `gpu_pct`, `copies_ms`/`mb`, `textures_ms`, `copyback_ms`/`mb`, `card_*`, `cpu_pct`, y la CPU de wineserver;
  - el trozo del log de esa ventana;
  - los switches y el commit.
- **Comparación:** `bench.sh compare A B` saca una tabla con las diferencias.

#### 0.4 Instrumentación que falta

Hay que portar los contadores de diagnóstico de la sesión del 24-nov, que están en el *worktree* local `VIctory-gta6-medir`. Van detrás de `VICTORY_MEDIR=1`, sin coste cuando está apagado:

1. **`xmem`:** llamadas y tiempo de `map` y `unmap`, y tiempo bajo el bloqueo de remapeo por segundo. joshuu ya tiene una parte (`e5e06c4`): se reutiliza y se completa.
2. **Esperas del título:** una sustitución HLE de `WaitForSingleObject(Ex)`, `WaitForMultipleObjects(Ex)`, `Sleep(Ex)`, `WaitOnAddress` y `SleepConditionVariableSRW/CS`. Agrupa por hilo y por quién llama (`RtlCaptureStackBackTrace`), y marca si la espera es sobre un fence de `SetEventOnCompletion`.
3. **Columnas nuevas en el CSV:** `remap_lock_ms`, `maps`, `unmaps`, `title_wait_ms`, para cruzarlas con el resto.

#### 0.5 Experimentos de techo (antes de escribir ninguna optimización)

| Experimento | Cómo | Qué responde |
|---|---|---|
| Sin copias | `VICTORY_COPY_MODE=none` (la imagen sale mal) | cuántos ms son copias: el techo de las fases 1-2 |
| Copias sin protección | `VICTORY_COPY_MODE=plain` | cuánto cuesta la protección de páginas mapeadas |
| Memoria en sitio | `VICTORY_INPLACE=1` y luego `=2` | cuánto ganan ya las ventanas en sitio, y si el driver lo acepta |
| Prueba del driver | `VICTORY_INPLACE_PROBE=1` | qué acepta vkd3d-proton con RADV de `OpenExistingHeapFromAddress`, `…1` y `…FromFileMapping`. Es el primer dato de la fase 2 |
| GPU real | `VICTORY_GPU_TIMING=1` | el tiempo de GPU por pipeline: cuánto tardaría el frame solo con la GPU |
| Perfil de CPU | `perf record -e cycles:u` con `VP_WRAP` | en qué funciones de VIctory se va la preparación |

#### 0.6 Portar de royale

Ninguno es imprescindible: el lanzador ya pone `d3dcompiler_47`.

1. `652aed4`: compilar el *clamp* de argumentos indirectos con DXC cuando D3DCompile lo rechaza. Así no hace falta winetricks.
2. `0d34233`: lectura comprobada en el mismo proceso para las copias de vuelta bajo Wine.

#### Entregables y salida

**Entregables:**

| Archivo | Contenido |
|---|---|
| `scripts/bench.sh` | el script de medida |
| `docs/bench.md` | cómo reproducir la escena |
| `docs/baseline.md` | escenas A y B, tres ejecuciones cada una, más los experimentos de techo |
| `docs/diario.md` | lo que se va aprendiendo |

**Salida:**
- La variación entre ejecuciones es menor del 10 % en `ms_per_frame`.
- 10 minutos en el mundo sin perder el dispositivo.
- La tabla de techos está escrita, y con ella se confirma o se corrige el orden de las fases 1 y 2.

### Fase 1: abaratar las copias que hay hoy

**Objetivo:** recortar el coste de copiar dentro de la arquitectura actual. Son mejoras pequeñas, medibles y reversibles. También sirven de banco de pruebas para entender qué memoria se usa y cómo.

**Estimación:** 3-5 días.

Cada tarea:
- va en su propio commit;
- tiene su switch (`VICTORY_F1_*`), activado por defecto solo cuando gana;
- mide con `bench.sh` antes y después.

| # | Tarea | Dónde | Cómo |
|---|---|---|---|
| 1.1 | **Censo de memoria por uso** | `copystat.rs`, `watch.rs` | Clasificar cada rango copiado: CPU-escribe/GPU-lee, GPU-escribe/CPU-lee, solo GPU, textura. Con tamaño y frecuencia por frame. Es la base de 1.3 y de toda la fase 2. |
| 1.2 | **Detección de escrituras más barata** | `watch.rs`, `host.rs`, `card_before` | Sustituir las 1.100-1.300 pasadas de hash por frame por el *write watch*: solo se compara lo que tiene páginas escritas. Las consultas se agrupan en una por envío (ya existe `VICTORY_ASK_ONCE`, hay que medirla). |
| 1.3 | **Copiar de vuelta solo lo que la CPU lee** | `host.rs`, `card_back`, `hand_back` | Con el censo de 1.1: los rangos que escribe la GPU y la CPU nunca lee no se bajan. Se protegen contra lectura (`PAGE_NOACCESS` o *guard*) y se bajan solo si el título los toca. Objetivo: de 70-100 MB a menos de 10 MB por frame. |
| 1.4 | **Staging persistente** | `host.rs`: `upload`, `scratch`, `take_chunk` | Ring buffers de subida reutilizados, alineados a 64 KiB, sin crear y destruir recursos por frame. Medir primero si las asignaciones aparecen en `perf`. |
| 1.5 | **Menos vistas en los alias** | `xmem.rs`: `map_alias` y `map` | Agrupar páginas contiguas en una sola vista cuando origen y destino son contiguos. En el censo del 24-nov la media era 1,0 páginas por llamada, así que primero hay que ver si joshuu lo cambió (`948e9d3`, `476a020`). Si no hay contigüidad, se descarta. |
| 1.6 | **Texturas solo cuando cambian** | `texture.rs` | Comprobar con el *write watch* antes del *detile*, y no volver a detilar mips que no cambiaron. |

**Salida:**
- En la escena A: preparación de menos de 40 ms y copia de vuelta de menos de 20 ms.
- Sin regresiones visuales: capturas de la escena A comparadas con las de la línea base, con el mismo encuadre.
- El censo de memoria (1.1) documentado en `docs/memoria.md`.
- Hipótesis: 4-6 fps.

### Fase 2: memoria compartida CPU/GPU, sin copias

**Objetivo:** que la GPU lea y escriba **directamente la memoria del título**, como en la consola. Es el cambio de arquitectura principal.

**Estimación:** 5-10 días.

#### 2.1 Qué permite la plataforma

Hay que medirlo primero, con la prueba de la fase 0.5:

- **D3D12 en vkd3d-proton:** `OpenExistingHeapFromAddress` se implementa con `VK_EXT_external_memory_host`. Comprobar si existen `OpenExistingHeapFromAddress1` y `OpenExistingHeapFromFileMapping`, y con qué límites: memoria bloqueada, alineación y protección uniforme.
- **Ancho de banda:** que la GPU lea memoria del sistema por PCIe 4.0 da unos 25 GB/s; la VRAM, más de 800 GB/s. Hay que medir con un *shader* de prueba cuánto tarda en leer 200 MB de cada tipo.
- **ReBAR:** VRAM que la CPU ve directamente (`D3D12_HEAP_TYPE_GPU_UPLOAD`, o `DEVICE_LOCAL|HOST_VISIBLE` en Vulkan). vkd3d ya lo detecta: el log dice *"HVV access is viable"*. La CPU escribe rápido en ella, pero **leer es muy lento** (memoria no cacheada).

#### 2.2 Diseño por tipo de memoria

Con el censo de la fase 1.1:

| Tipo | Dónde vive | Copias |
|---|---|---|
| CPU escribe, GPU lee poco (constantes, argumentos) | memoria del título importada como heap de la GPU | ninguna |
| CPU escribe, GPU lee mucho (vértices, índices) | VRAM visible por la CPU (ReBAR), y el título escribe ahí directamente | ninguna |
| GPU escribe, CPU lee (consultas, resultados) | memoria del sistema importada, coherente | ninguna |
| Solo GPU (render targets, intermedios) | VRAM normal | nunca, salvo que la CPU lo lea (fase 1.3) |
| Texturas con *tiling* de consola | VRAM, con *detile* | solo cuando cambian |

#### 2.3 Rediseñar el *pool* de páginas de `xmem`

Es el trabajo más grande de esta fase.

- **El problema:** hoy las páginas físicas son secciones `SEC_RESERVE`, que el host no acepta abrir como heap, y sus vistas se remapean sin parar (`map_alias`).
- **La opción a explorar:**
  - Reservar el *pool* de GPU como **una sola asignación grande**, ya importada como heap de la GPU.
  - Que el mapeo de páginas de la consola se traduzca en **mapeos de tiles en la GPU** (`UpdateTileMappings` y *reserved resources*, que vkd3d convierte en *sparse binding*) en lugar de vistas de CPU de 64 KiB.
  - La CPU vería la misma memoria por su dirección fija.
- **Qué hay que resolver:** cómo ve la CPU el alias. Si la consola muestra la misma página física en dos direcciones de CPU, hace falta seguir teniendo vistas de CPU, pero ya no para la GPU. Hay que medir cuántos alias son solo para la GPU (en principio, los de tiled resources) y cuántos para la CPU.

#### 2.4 Coherencia y sincronización

- En las regiones sin copias desaparecen las protecciones de escritura y los hashes. Las barreras y fences del título bastan, como en la consola.
- **Condición:** la CPU del host no debe leer una región mientras la GPU la escribe. Eso ya está garantizado mientras se espere por cada envío (lo que hay hoy). En la fase 3 lo garantizarán los fences.
- `VICTORY_INPLACE=2` ya documenta esa restricción (*"Do not combine 2 with late wait"*).

#### 2.5 Implementación por pasos, cada uno medible

1. Extender `VICTORY_INPLACE` a toda la memoria privada del título. Cuánta es, lo dice el censo.
2. Constantes y argumentos indirectos sin copia.
3. Vértices e índices en ReBAR, si el ancho de banda de 2.1 lo justifica.
4. El *pool* de páginas importado (2.3) detrás de `VICTORY_ZEROCOPY=1`.
5. Quitar las copias de vuelta de lo que ya está en sitio.

**Salida:**
- Los MB subidos y bajados por frame caen más de un 80 % frente a la línea base.
- El frame lo limitan la GPU o la CPU del propio título, no VIctory (`cpu_prepare_ms` menor que `gpu_ms`).
- La misma imagen que la línea base y 30 minutos sin perder el dispositivo.
- Hipótesis: 10-20 fps.

### Fase 3: trabajo en paralelo y ritmo de frames

**Objetivo:** que la CPU y la GPU trabajen a la vez. Sin copias (fase 2), lo que queda es la espera de cada envío.

**Estimación:** 4-7 días.

| # | Tarea | Dónde | Cómo |
|---|---|---|---|
| 3.1 | **Frames en vuelo** | `host.rs` (`submit`, `settle`), `queue.rs`, `fence.rs` | Quitar el *"every submission is waited for"*. Cada envío lleva su valor de fence, y las señales del título se completan cuando la GPU llega a ellas, no al instante. Se permiten 2 o 3 frames en vuelo. Lo que hoy hacen `late` y `relay` se reescribe sobre memoria sin copias, que no tiene el problema de leer páginas remapeadas a medias. |
| 3.2 | **Seguridad del remapeo** | `xmem.rs` + `host.rs` | Una página que la GPU está usando no se remapea hasta que su fence se cumpla: una cola de liberaciones diferidas. Esto es lo que rompe hoy `VICTORY_LATE_WAIT` en AMD. |
| 3.3 | **Grabación en varios hilos** | `list.rs`, `draw.rs` | El título graba desde 5 hilos. La traducción de cada lista al host se hace en su hilo, no en el del envío. |
| 3.4 | **Pipelines sin bloquear** | `pipeline/make.rs`, `library.rs`, `apps/victory/src/translate.rs` | La compilación en hilos ya existe; hay que medir cuánto se espera todavía. Además: **arreglar el cuelgue de `victory warm`** (con RADV se queda en espera para siempre al acabar los pipelines de cómputo; reproducido el 25-nov), y lanzar `warm` desde el lanzador tras cada actualización. |
| 3.5 | **Frame pacing** | `frame.rs`, `host.rs::present` | La cola de presentación acotada y una latencia estable. |

**Salida:**
- La GPU está ocupada más del 70 % en la escena A.
- El p99 del frametime es menor que el doble de la mediana.
- No hay pausas al repetir la escena B.
- 30 minutos sin perder el dispositivo en AMD.

### Fase 4: backend Vulkan nativo

**Esta fase solo se hace si las medidas lo justifican.**

**Estimación:** semanas.

Hoy la cadena es esta: shader de consola → DXIL → dxil-spirv → SPIR-V → RADV, y D3D12 → vkd3d-proton → Vulkan.

1. **Decisión con datos.** Tras las fases 2 y 3, medir con `perf` qué parte del frame se va en vkd3d-proton, dxil-spirv y winevulkan. Si es menos del 15 % del frame, esta fase se aplaza.
2. **Generar SPIR-V directamente** desde el IR de `victory-shader`, como segundo backend junto a `emit` (DXIL). Así se evitan dxil-spirv y sus casos raros.
3. **Backend Vulkan en `victory-gpu`**, detrás de un *trait* común con el de D3D12:
   - *sparse binding* nativo para los heaps con *tiles*;
   - `VK_EXT_external_memory_host` directo;
   - `VK_EXT_descriptor_buffer` para los heaps de descriptores del título.
4. **Detección de escrituras propia de Linux:**
   - `userfaultfd` en modo protección de escritura, o los bits *soft-dirty* de `/proc/self/pagemap`, en lugar de fallos de página gestionados por Wine.
   - Requiere una parte Unix: una *unixlib* de Wine, o un proceso auxiliar que comparte memoria por `memfd`.
   - Primero un prototipo aislado para medir si compensa.
5. **Wine se queda.** El título es un PE de Windows que importa la API de Windows; quitar Wine significaría reescribirlo.

**Salida:** la misma imagen que con D3D12, un frame más corto (medido) y sin pérdida de estabilidad.

### Fase 5: estabilidad y completitud visual

**Esta fase va en paralelo desde la fase 1.** Cada fase la tiene como puerta de salida.

| Tema | Estado | Qué hacer |
|---|---|---|
| Fallo de AMD en `fc5e66f8f873d3bb` ([fxceless#3](https://github.com/fxceless/victory-fork/issues/3)) | hoy se salta | entender el acceso y arreglar la traducción |
| GPUVM fault con `late-wait` ([#184](https://github.com/fxceless/victory-fork/issues/184)) | evitado | lo resuelve la fase 3.2 |
| 106 shaders sin traducir (`warm`, 25-nov) | abiertos | por familias: *"descriptor read only as data"* (caras y piel), `v_cvt_f32_f64`, SGPR de hardware, descriptores elegidos por flujo de control, primera mitad de interpolación |
| Trazado de rayos (sombras negras) | no se ejecuta | DXR en vkd3d-proton; después de la fase 3 |
| Parpadeo del vídeo de la intro | abierto | medir con capturas |
| Puerta de cada fase | | 30 minutos en el mundo sin perder el dispositivo y sin cierres |

## 5. Cómo trabajamos

1. **Una rama por tarea** (`f1-hash-watch`, `f2-pool`…), que se fusiona en `main` cuando pasa la puerta de su fase.
2. **Primero medir.** Cada commit de rendimiento lleva en su mensaje la tabla de `bench.sh compare`: antes, después y diferencia en la escena A.
3. **Todo cambio de arquitectura va detrás de un switch** hasta que sea la ruta por defecto. Así se compara en la misma compilación, y se puede volver atrás sin recompilar.
4. **`docs/diario.md`:** qué se probó, qué salió y qué se decidió, con fecha. Así nada se repite y el trabajo de varios días tiene hilo.
5. **Traer joshuu y fxceless** al empezar cada fase (`git fetch joshuu && git merge joshuu/victory-amd`). Si un cambio suyo toca lo mismo que una fase en curso, se integra primero.
6. **Sin datos del juego en el repositorio:** ni archivos, ni capturas, ni partidas, ni nombres. Las capturas para comparar se quedan en `bench/`, que no se sube.
7. **Probar siempre con el [lanzador](https://github.com/datingesp-hash/victory-proton):** wine, vkd3d-proton, el wineserver parcheado y el sandbox de solo lectura.
8. **No lanzar nada mientras se juega:** arrancar el wineserver propio cierra el juego abierto.

## 6. Riesgos y decisiones abiertas

| Riesgo | Cómo se mitiga |
|---|---|
| vkd3d-proton no implementa `OpenExistingHeapFromFileMapping` o `…Address1` | Se sabe en la fase 0.5. Si falta, la fase 2.3 va por una asignación grande y *tiles* en la GPU, o se añade a vkd3d-proton (es código abierto). |
| La GPU leyendo memoria del sistema por PCIe es más lenta que copiar | Se mide en la fase 2.1. Lo que se lee mucho va a ReBAR o se sigue copiando, pero solo cuando cambia. |
| Los alias de CPU de la consola no encajan en un *pool* importado | Censo de alias en la fase 2.3. Las vistas de CPU se mantienen para la CPU y la GPU usa *tiles*. |
| joshuu y fxceless cambian el mismo código a la vez | Traerlos al empezar cada fase. Cambios pequeños, detrás de switches. |
| Una fase no da lo que prometía | Los experimentos de techo de la fase 0.5 lo dicen antes. Si una fase no mueve las cifras, se anota en el diario y se pasa a la siguiente. |
| Más velocidad expone más condiciones de carrera (fallos de GPU) | La puerta de 30 minutos en cada fase. Diagnóstico con `VICTORY_D3D12_DEBUG=1` (build de diagnóstico). |

**Decisiones abiertas:** se deciden con datos en el momento indicado.

- **Fase 2.2:** ¿ReBAR o memoria del sistema importada para vértices e índices?
- **Fase 3.1:** ¿2 o 3 frames en vuelo?
- **Fase 4.1:** ¿se hace la fase 4?

## 7. Compilar y ejecutar

Con el lanzador, que tiene el perfil `royale`:

```sh
victory-proton -p royale                 # actualiza, compila y lanza
victory-proton -p royale victory warm    # traduce los shaders por adelantado
```

Es privado: git tiene que poder autenticarse en GitHub (`gh auth setup-git`).

Para compilar a mano desde Linux, con [cargo-xwin](https://github.com/rust-cross/cargo-xwin):

```sh
cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
```

Las notas del fork base, con sus switches y diagnósticos, están en [docs/fork-joshuu-README.md](docs/fork-joshuu-README.md).

## 8. Ramas y remotos

| Rama | Qué es |
|---|---|
| `main` | nuestra línea: joshuu más la ruta |
| `royale-base` | ligmaSec/victory-royale tal cual, como referencia |

```sh
git remote add joshuu   https://github.com/JoshuuDev/victory-amd
git remote add fxceless https://github.com/fxceless/victory-fork
git remote add upstream https://github.com/Marvisak/VIctory
git remote add royale   https://github.com/ligmaSec/victory-royale
```

## 9. Estado

| Fase | Estado |
|---|---|
| 0 | en curso: base y README listos; siguiente, el perfil del lanzador y `bench.sh` |
| 1-5 | pendientes |
