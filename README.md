# victory-royale (privado)

VIctory centrado en Linux. El objetivo es una arquitectura que permita jugar **de forma fluida y estable** a un título de Xbox Series con GE-Proton, en lugar de los 2-4 fps de los forks actuales.

> Este repositorio no contiene archivos del juego. Cada uno usa su propia copia, volcada de una consola suya. El código es GPL-3.0-or-later, como VIctory.

- [De dónde parte](#de-dónde-parte)
- [Por qué va lento hoy](#por-qué-va-lento-hoy)
- [La ruta](#la-ruta)
- [Reglas de trabajo](#reglas-de-trabajo)
- [Compilar y ejecutar](#compilar-y-ejecutar)
- [Ramas y remotos](#ramas-y-remotos)

## De dónde parte

La base es [JoshuuDev/victory-amd](https://github.com/JoshuuDev/victory-amd) (`003beef`, 25-nov-2026). Es el fork que mejor renderiza el mundo, y viene en esta cadena:

| Capa | Repositorio | Qué aporta |
|---|---|---|
| 1 | [Marvisak/VIctory](https://github.com/Marvisak/VIctory) `ffe310e` | el loader, la emulación del sistema de Xbox, el backend D3D12 y el traductor de shaders |
| 2 | [fxceless/victory-fork](https://github.com/fxceless/victory-fork) | que el mundo se dibuje: cinemáticas y prólogo con texturas |
| 3 | [myokex/victory-amd](https://github.com/myokex/victory-amd) | arreglos para AMD (texturas por memoria real, dispatches partidos, vistas recortadas) |
| 4 | JoshuuDev/victory-amd | arreglo de DirectStorage, más shaders traducidos y el bloqueo de remapeo por cada 16 MB |

**Por qué no se parte de [ligmaSec/victory-royale](https://github.com/ligmaSec/victory-royale):** es el VIctory oficial con unos 700 commits de PRs abiertos, más un lanzador y una CI para Linux. No lleva el trabajo de renderizado de fxceless, y según su README el mundo sale casi negro. Fusionarlo con joshuu da **494 conflictos en 63 archivos**, casi todos en el núcleo de GPU, porque son dos reescrituras independientes del mismo código. Su historia completa se conserva en la rama `royale-base`. Lo que interesa de ahí se porta a mano en la fase 0.

## Por qué va lento hoy

Estas son las medidas en un RX 7900 XT con RADV, GE-Proton11-14 y el [wineserver parcheado](https://github.com/datingesp-hash/victory-proton). La escena es el patio del prólogo, con los switches por defecto.

| Medida | Valor |
|---|---|
| frame completo | ~600 ms (1,6-2,1 fps) |
| VIctory preparando el frame | 100-130 ms, de ellos ~50-60 ms copiando **150-190 MB** hacia la GPU |
| GPU | 75-105 ms; ocupada **25-39 %** del frame |
| copia de vuelta a la memoria del título | ~50 ms, **70-100 MB** |
| pasadas de hash sobre copias en la GPU | 1.100-1.300 por frame |
| CPU del proceso | ~1,1 núcleos de 24 |
| esperas del título en fences de la GPU | ninguna |

En la Xbox, la CPU y la GPU comparten **una sola memoria**. En el PC están separadas, así que en cada frame VIctory tiene que:

1. Averiguar qué ha escrito el título, protegiendo páginas y calculando hashes.
2. Subirlo a la GPU.
3. Dejar que la GPU dibuje.
4. Bajar lo que la GPU escribió a la memoria del título.

Todo esto ocurre **en serie**: el título, después la preparación, la GPU y la copia de vuelta. No hay nada saturado. La GPU espera a la CPU y la CPU espera a las copias. En Windows ocurre lo mismo, por eso fxceless también va a ~2 fps.

**Conclusión:** subir los fps exige **dejar de copiar la memoria**, no optimizar los shaders ni el streaming. Los shaders ya se guardan en disco y apenas afectan a los fps estables. La GPU no es el límite.

## La ruta

El orden es fijo, y cada fase depende de lo que mide la anterior. Una fase se cierra cuando se cumplen sus **criterios de salida** en la escena de referencia. Los objetivos de fps son hipótesis que hay que confirmar con medidas.

### Fase 0: base, escena de referencia y medición

Sin medidas repetibles no se puede saber si un cambio mejora algo.

1. **Base.** El fork joshuu está en `main`, con el remoto `joshuu` para traer sus avances (ver [Ramas y remotos](#ramas-y-remotos)).
2. **Escena de referencia.**
   - Una partida guardada en el patio del prólogo, con la cámara quieta durante 60 s tras cargar, más una segunda escena con movimiento (caminar 30 s por el patio).
   - Se documenta en `docs/bench.md` cómo reproducirla. La partida es del usuario y no entra en el repositorio.
3. **`victory bench`.** Un subcomando, o un script en `scripts/`, que lanza la escena y vuelca un JSON con:
   - fps medio, p50, p99 y peor frame;
   - el tiempo de cada fase del frame: título, preparación, espera de GPU, GPU y copia de vuelta;
   - los MB subidos y bajados por frame;
   - el % de ocupación de la GPU;
   - el tiempo bajo el bloqueo de remapeo;
   - las llamadas a map/unmap y su coste;
   - la CPU de wineserver.

   Se aprovecha el volcado de frametime de joshuu (`VICTORY_FRAMETIME_DUMP`) y los contadores que ya existen en los logs.
4. **Instrumentación permanente.** Se portan los contadores de diagnóstico de la sesión de medidas:
   - tiempos de map/unmap y del bloqueo en `xmem.rs`;
   - esperas del título por quien las llama, una sustitución HLE de `WaitFor*`, `Sleep*`, `WaitOnAddress` y `SleepConditionVariable*`.

   Van detrás de `VICTORY_MEDIR=1`, sin coste cuando está apagado.
5. **Requisitos de Linux** asumidos en todas las medidas: el wineserver con árbol de vistas, `d3dcompiler_47` nativo, `VKD3D_CONFIG=descriptor_heap` y GE-Proton ≥ 11-13.
6. **Traer de royale, portado a mano:**
   - la compilación del *clamp* de argumentos indirectos con DXC cuando falla D3DCompile (`652aed4`), para no depender de winetricks;
   - la lectura comprobada en el mismo proceso para las copias de vuelta bajo Wine (`0d34233`).

**Salida:**
- `docs/baseline.md` con las cifras de la escena, de tres ejecuciones con una variación de menos del 10 %.
- El juego llega al mundo sin perder el dispositivo en 10 minutos.

### Fase 1: abaratar las copias que hay hoy

Son mejoras dentro de la arquitectura actual. Cada una se mide por separado y todas pueden desactivarse con un switch.

1. **Detección de escrituras más barata.** Sustituir las 1.100-1.300 pasadas de hash por frame por información de páginas escritas, agrupando las consultas de *write watch*. No se vuelve a calcular el hash de lo que no ha cambiado.
2. **Copiar de vuelta solo lo que la CPU lee.** Registrar qué recursos escritos por la GPU lee el título desde la CPU (con protección de lectura en esos rangos) y bajar solo esos, no 70-100 MB por frame.
3. **Staging persistente.** Ring buffers de subida reutilizados y alineados, sin asignar memoria en cada frame.
4. **Menos vistas en los alias.** Fusionar páginas contiguas de `map_alias` en vistas más grandes cuando la granularidad del título lo permita, para reducir tanto el trabajo del wineserver como el bloqueo de remapeo.
5. **Solapar frames.** Preparar el frame N+1 mientras la GPU ejecuta el N, cuando no dependan entre sí.

**Salida:**
- Preparación de menos de 40 ms y copia de vuelta de menos de 20 ms en la escena de referencia.
- Sin regresiones visuales: capturas comparadas con la línea base.
- Hipótesis: 4-6 fps.

### Fase 2: memoria compartida CPU/GPU, sin copias (el núcleo)

Que la GPU lea y escriba **directamente la memoria del título**, como en la consola.

1. **Prueba en este PC.** `crates/victory-gpu/src/inplace.rs` ya tiene una prueba de `OpenExistingHeapFromAddress`, `OpenExistingHeapFromAddress1` y `OpenExistingHeapFromFileMapping` (`VICTORY_INPLACE_PROBE`).
   - Ejecutarla con vkd3d-proton y RADV, que implementa la primera con `VK_EXT_external_memory_host`.
   - Anotar qué acepta, con qué límites (memoria bloqueada, alineación, protección uniforme) y qué ancho de banda tiene la GPU leyendo memoria del sistema por PCIe frente a VRAM.
2. **Clasificar la memoria del título por uso**, con un censo por región en la escena de referencia:
   - **CPU escribe, GPU lee** (constantes, vértices, argumentos): memoria del título importada como heap de la GPU, o en VRAM visible por la CPU (ReBAR). Cero copias.
   - **GPU escribe, CPU lee** (poco frecuente): memoria del sistema importada, coherente.
   - **Solo GPU** (render targets, buffers intermedios): solo VRAM, y nunca se copia de vuelta salvo que la CPU la lea (fase 1.2).
   - **Texturas con *tiling* de consola**: siguen necesitando conversión (*detile*), pero solo cuando cambian.
3. **Pool de páginas compatible.** `xmem` reserva hoy la memoria del título con `SEC_RESERVE`, que `OpenExistingHeapFromFileMapping` no acepta. Hay que rediseñar la reserva para que las regiones de GPU se puedan abrir como heap, ya sean secciones `SEC_COMMIT` o asignaciones completas.
4. **Coherencia.** En las regiones sin copias se quitan las protecciones de escritura y las pasadas de hash. Las barreras y los fences del título bastan, como en la consola.
5. **Ruta conmutable** con `VICTORY_ZEROCOPY=1` hasta que sea la ruta por defecto.

**Salida:**
- Los MB subidos y bajados por frame caen más de un 80 %.
- El frame lo limitan la GPU o la CPU del propio título, no VIctory.
- Hipótesis: 10-20 fps.

### Fase 3: trabajo en paralelo y ritmo de frames

1. **Separar el envío del título del envío al host.** Dos o tres frames en vuelo, fences por frame y sin la cadena en serie.
2. **Grabación de comandos en varios hilos.** El título graba desde 5 hilos, y la traducción de listas a la GPU del host debe repartirse igual.
3. **Pipelines sin bloquear.**
   - Compilar en hilos de trabajo, con la caché en disco que ya existe.
   - Arreglar el cuelgue de `victory warm` al terminar la fase de pipelines de cómputo: con RADV se queda en espera para siempre.
   - Lanzar `warm` automáticamente tras cada actualización.
4. **Frame pacing.** Controlar la cola de presentación y la latencia.

**Salida:**
- La GPU está ocupada más del 70 % en la escena de referencia.
- El p99 del frametime es menor que el doble de la mediana.
- No hay pausas al repetir una escena ya vista.

### Fase 4: backend Vulkan nativo (solo si las medidas lo justifican)

Hoy la cadena es esta: shader de consola → DXIL → dxil-spirv → SPIR-V → RADV, y D3D12 → vkd3d-proton → Vulkan.

1. **Decisión con datos.** Tras las fases 2 y 3, medir con `perf` qué parte del frame se va en vkd3d-proton y dxil-spirv. Si es menos del 15 %, esta fase se aplaza.
2. **Generar SPIR-V directamente** desde el IR de `victory-shader`, como segundo backend junto a DXIL.
3. ***Sparse binding*** para los heaps con *tiles*, en lugar de miles de vistas de 64 KiB (`map_alias`).
4. **Detección de escrituras propia de Linux:**
   - `userfaultfd` en modo protección de escritura, o los bits *soft-dirty*, en lugar de fallos de página gestionados por Wine.
   - Requiere una parte Unix (una *unixlib* de Wine o un proceso auxiliar con memoria compartida por `memfd`), porque el código PE no llama a Linux directamente.
   - Hay que estudiarlo antes de decidir.
5. **Wine se queda.** El título es un PE de Windows que importa la API de Windows, y quitar Wine significaría reescribirlo.

**Salida:** el mismo resultado visual que con D3D12 y un frame más corto, medido.

### Fase 5: estabilidad y completitud visual (en paralelo desde la fase 1)

- **Fallos de GPU en AMD:** el shader `fc5e66f8f873d3bb` ([fxceless/victory-fork#3](https://github.com/fxceless/victory-fork/issues/3)) y los *late-wait* que fallan al acabar la carga ([#184](https://github.com/fxceless/victory-fork/issues/184)).
- **Shaders sin traducir.** `warm` encuentra 106: caras y piel ("descriptor read only as data"), `v_cvt_f32_f64`, SGPR de hardware y descriptores elegidos por flujo de control.
- **Trazado de rayos** (DXR en vkd3d-proton) para las sombras negras.
- **Puerta de cada fase:** 30 minutos de juego sin perder el dispositivo y sin cierres.

## Reglas de trabajo

- **Primero medir.** Cada commit de rendimiento incluye en el mensaje las cifras antes y después en la escena de referencia.
- **Todo cambio de arquitectura va detrás de un switch** hasta que sea la ruta por defecto, para poder comparar en la misma compilación.
- **Sin datos del juego en el repositorio:** ni archivos, ni capturas, ni partidas, ni nombres.
- **Traer los avances de joshuu y fxceless** con regularidad, sobre todo en la fase 0 y la fase 1, antes de divergir mucho.
- **Probar en Linux** con el [lanzador](https://github.com/datingesp-hash/victory-proton), que ya pone wine, vkd3d-proton, el wineserver y el sandbox de solo lectura.

## Compilar y ejecutar

Con el lanzador, añadiendo este repositorio como perfil:

```sh
victory-proton profile add https://github.com/datingesp-hash/victory-royale-private royale main
victory-proton -p royale
```

Es privado: git tiene que poder autenticarse en GitHub (por ejemplo con `gh auth setup-git`).

Para compilar a mano, desde Linux, con [cargo-xwin](https://github.com/rust-cross/cargo-xwin):

```sh
cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
```

Las notas del fork base, con sus switches y diagnósticos, están en [docs/fork-joshuu-README.md](docs/fork-joshuu-README.md).

## Ramas y remotos

| Rama | Qué es |
|---|---|
| `main` | nuestra línea: joshuu más esta ruta |
| `royale-base` | ligmaSec/victory-royale tal cual, como referencia |

```sh
git remote add joshuu   https://github.com/JoshuuDev/victory-amd
git remote add fxceless https://github.com/fxceless/victory-fork
git remote add upstream https://github.com/Marvisak/VIctory
git remote add royale   https://github.com/ligmaSec/victory-royale
git fetch joshuu && git merge joshuu/victory-amd   # traer avances de la base
```
