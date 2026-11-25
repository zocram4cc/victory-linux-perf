# La memoria: datos para diseñar un sistema eficiente

Este documento reúne todo lo que se sabe de cómo usa la memoria el título y de lo que permite la plataforma (Linux, GE-Proton, vkd3d-proton, RADV y la RX 7900 XT). Es la base de las fases 1 y 2 del [README](../README.md).

Cada dato lleva su fuente. Cuando se mida algo nuevo, se añade aquí.

## 1. Cuánta memoria se mueve (escena A, línea base `176cc10`; escena B: 2,6 fps, 386 ms, 229 MB de subida y 86 MB de vuelta)

| Qué | Por frame | Fuente |
|---|---|---|
| Copias hacia la GPU | **215 MB, 62 ms** (~3,5 GB/s efectivos) | `docs/baseline.md` |
| Copia de vuelta a la memoria del título | **79 MB, 68 ms** | idem |
| Texturas (*detile* + subida) | 6-16 MB, 2-4 ms | idem |
| Pasadas de hash sobre copias en la VRAM | 558 | idem |
| Remapeos del título en la escena estática | 1,9 *maps* y 7,5 *unmaps*, 0,5 ms bajo el bloqueo | `bench/*caras-medir`, `VICTORY_MEDIR=1` |
| Draws / dispatches indirectos | ~4.970 / ~635; ~340 descartados (sin pipeline) | idem |

**Lectura:**
- Las copias se mueven a unos 3,5 GB/s, muy lejos de los ~27 GB/s de PCIe y de lo que da `memcpy`. Lo que cuesta es **decidir qué copiar** (*write watch*, hashes, guardas), no mover los bytes.
- En la escena estática los remapeos ya no pesan, con el wineserver parcheado (0,5 ms por frame).

## 2. Dónde vive la memoria que leen los shaders (censo de `VICTORY_INPLACE_PROBE=1`)

**Buffers del título** que usan los draws: **3.705 buffers, 1.037 MB**.

| Tipo de memoria | Buffers | MB | % de los MB | ¿Se puede abrir en sitio hoy? |
|---|---|---|---|---|
| Privada, entera *committed* (`XMemVirtualAlloc`) | 3.574 | 294 | 28 % | **sí** |
| Vista del *pool*, un tramo de una sección | 114 | 422 | 41 % | no (sección) |
| Vista del *pool*, páginas dispersas | 17 | 320 | 31 % | no |

**Ventanas fuera de buffers:** 7 ventanas, 28 MB, de las que 4 MB son privados.

**Lectura:**
- **El 72 % de los bytes que leen los shaders vive en el *pool* de páginas**, en solo 131 buffers grandes. La memoria privada son muchos buffers pequeños (82 KB de media).
- Sin rehacer el *pool* (fase 2.3), la memoria en sitio solo cubre un 28 %.
- Con `INPLACE=2`, en el mundo había 3.555-3.611 buffers en sitio (188 MB) y entre 740 y 1.507 rechazados por ser memoria de vista. El dispositivo se perdió a los 60 s (0x887A0005). Hay que averiguar por qué antes de reutilizar esa ruta.

## 3. Lo que permite la plataforma (sonda del 25-nov, vkd3d-proton v3.1-83-GE con RADV y Mesa 26.3.1)

| Prueba | Resultado |
|---|---|
| `ID3D12Device3` / `ID3D12Device13` | sí / sí |
| `OpenExistingHeapFromAddress` (asignación entera) | **ok** |
| `OpenExistingHeapFromAddress1` (parte de una asignación) | **ok** |
| Heap resultante | tipo custom, CPU *write-back*, en la memoria del PC (pool L0), alineación 64 KiB, flags *shared*, *cross-adapter*, *write watch* y *shader atomics* |
| La GPU lee lo que la CPU escribió después, sin llamadas de por medio | **sí** (coherente) |
| La CPU lee lo que la GPU escribió (por copia y por UAV) | **sí** |
| *Write watch* sobre memoria abierta | sigue funcionando |
| El título protege o libera páginas con el heap abierto | permitido, dentro y fuera de la parte abierta |
| `OpenExistingHeapFromFileMapping` con `SEC_RESERVE` (el *pool* actual) | **rechazado** (0x80070057) |
| `OpenExistingHeapFromFileMapping` con `SEC_COMMIT` | se abre, pero **la GPU no ve los bytes de las vistas**, ni al leer ni al escribir |
| Coste de abrir 64 MB nunca tocados | 8,2 ms (memoria privada), 4,3 ms (sección); se fijan en la RAM |
| La GPU copia 64 MB de memoria del sistema | 2,4 ms, **~27 GB/s** (igual que un buffer compartido normal) |
| Filas de texturas *cross-adapter* | no |
| Heap tier | 2 |

**Lectura:**
- La memoria privada se puede compartir con la GPU de forma fiable.
- El *pool* por secciones no se puede compartir: vkd3d abre la sección como otra memoria, no como las vistas.

## 4. Coste de leer en sitio (`VICTORY_INPLACE=1`, una ejecución)

| | base | `INPLACE=1` |
|---|---|---|
| En sitio | — | 73-78 ventanas, 340-360 MB |
| GPU | 159 ms | **396 ms** |
| Copias | 63 ms, 216 MB | 84 ms, 401 MB |
| fps | 2,8 | 1,6 |
| Imagen | sin caras | **con caras** (negras de frente) y sin parpadeos |

**Lectura:**
- Los shaders que leen cientos de MB por PCIe en lugar de VRAM **multiplican por 2,5 el tiempo de GPU**.
- La memoria del sistema en sitio solo sirve para datos pequeños, o leídos una vez, o leídos por la CPU. Los datos grandes que la GPU lee cada frame tienen que estar en VRAM.
- Por qué subió el volumen de copias con `INPLACE=1` está por explicar. Puede que las ventanas se suban a la vez por otra ruta.

## 5. Techo sin copias (`VICTORY_COPY_MODE=none`; la imagen sale negra)

La misma escena, con 5.037 draws por frame frente a 4.971:

| | base | sin copias | diferencia |
|---|---|---|---|
| ms por frame | 354 | **170** | −52 % |
| fps | 2,8 | **5,9** | ×2,1 |
| preparar | 124 | 72 | |
| · copias que quedan | 63 ms | 22 ms, 68 MB | rutas que no pasan por `copy_mapped` |
| GPU | 159 | 72 | con datos a cero, la GPU hace menos trabajo real |
| copia de vuelta | 67 ms, 78 MB | 23 ms, 0,15 MB | el tiempo que queda es la espera, no los bytes |
| pasadas de hash | 558 | 125 | |

**Lectura:**
- **Quitar las copias da como máximo ×2 (≈6 fps).** Aun sin copias, el frame sigue siendo una cadena en serie de unos 170 ms: preparar, esperar a la GPU y la copia de vuelta.
- Para pasar de ~6 fps hacen falta **las dos cosas**: memoria sin copias (fases 1-2) y CPU y GPU trabajando a la vez (fase 3).
- Este dato justifica el orden de la ruta, y obliga a no dejar la fase 3 para el final si las fases 1 y 2 se estancan.

## 6. La memoria tiene huecos (`VICTORY_COPY_MODE=plain`)

- Con copias planas (`memcpy` sin protección), el juego se cierra antes de la primera cinemática. La causa es `0xc0000005` en `ucrtbase` al leer `0xd6cde0000: reserved, never committed`.
- **Los buffers del título abarcan páginas sin respaldo** (memoria con *tiles* o residente a medias). Por eso `copy_mapped` lee a través del kernel (`ReadProcessMemory`) y trata la página sin mapear como ceros.
- **Requisito de diseño:** cualquier sistema nuevo tiene que tolerar rangos dispersos con huecos, igual que la consola con su memoria con *tiles*. Para la GPU eso apunta a *reserved resources* y `UpdateTileMappings` (*sparse binding* en Vulkan).

## 7. Correción: las caras

| Ejecución | Caras |
|---|---|
| base (varias) | invisibles: solo boca y ojos |
| `INPLACE=1` | se ven (de espaldas bien; de frente negras) |
| `INPLACE_PROBE=1` + `MEDIR=1` (una vez) | **bien**, las de todos los personajes |
| `MEDIR=1` solo | invisibles |
| `INPLACE_PROBE=1` solo | invisibles |
| `INPLACE_PROBE=1` + `MEDIR=1` (×2 más, cámara quieta) | invisibles |
| **base B, sin switches, caminando** | **bien: cara completa con piel e iluminación** (captura del usuario) |

**Lectura:**
- Con los datos leídos en sitio se ven. Esto apunta a que **las copias del host de esos datos llegan viejas o incompletas**, no a shaders sin traducir.
- Que una sola ejecución de `PROBE+MEDIR` las mostrara y por separado no, apunta a algo **no determinista**: una carrera entre las escrituras del título y nuestras copias, o el orden de carga.
- **Actualizado tras 9 ejecuciones:** las caras **no dependen de los switches**. Se ven cuando **la cámara se mueve o se acerca**: en la base B sin switches, caminando, se ven bien; con la cámara quieta, en ninguna.
- **Dos hipótesis:**
  - **H1, nivel de detalle:** la cabeza cercana usa otro modelo o material que sí se dibuja bien. La de lejos, o la de la cámara fija del principio, usa uno que no se dibuja: sería un shader sin traducir o un draw descartado entre los ~340 por frame.
  - **H2, copias viejas:** los datos de la cara llegan después de la primera copia, y el host no los refresca hasta que algo lo fuerza (`INPLACE=1`, que lee en sitio, también las mostraba).
- **Pruebas para separarlas (fase 1.7):**
  - escena A esperando 2 minutos sin mover la cámara, y después girarla: si aparece con el tiempo, H2; si solo al acercarse, H1;
  - cruzar el momento en que aparece con los draws descartados y los hashes de pipeline del log.

## 8. El título y sus hilos (`VICTORY_MEDIR=1`)

- 42 hilos. **El que menos espera pasa un 82 % del tiempo esperando.** Casi todo es `WaitForSingleObject` en `title+0x2215eca` (*workers* de trabajos, de 2 a 280 veces por segundo).
- No hay esperas sobre fences de la GPU (`[fence]`).
- CPU del juego: 1,4 núcleos de 24.
- **Lectura:** la CPU del título no es el límite. Hay margen de sobra para trabajo nuestro en paralelo (fase 3) y para la detección de escrituras.

## 9. Disposición de la memoria (log de memoria del fork belobug al arrancar, como referencia)

| Concepto | Tamaño |
|---|---|
| privada | 6.747 MiB (6 asignaciones de 64 MiB o más suman 4.995 MiB; la mayor, 2.199 MiB en `0xf00200000`) |
| mapeada (vistas del *pool*) | 10.264 MiB |
| páginas físicas | 7.131 MiB en secciones |
| texturas en el host | 4.428 MiB (1.211) |
| VRAM usada | 4.761 de 18.943 MiB |

Hay que medirlo con royale en el mundo; ese log no aparece en la build normal.

## 10. Reglas de diseño que salen de los datos

1. **El coste está en decidir qué copiar, no en copiar.** Toda mejora de la fase 1 se mide en pasadas de hash y en consultas al *write watch*, no solo en MB.
2. **VRAM para lo que la GPU lee mucho; memoria del sistema en sitio solo para lo pequeño o lo que lee la CPU.** Leer en sitio por PCIe duplica el tiempo de GPU.
3. **El *pool* de páginas (72 % de los bytes) no se puede abrir con secciones.** La GPU necesita su propio mapeo con *tiles* sobre memoria abierta o VRAM, y la CPU puede seguir con sus vistas.
4. **Tolerar huecos:** los rangos del título tienen páginas sin respaldo.
5. **Sin copias no basta (×2 como máximo).** Hay que combinarlo con frames en vuelo (fase 3).
6. **La corrección importa:** las copias viejas explican las caras. Cualquier caché nueva tiene que garantizar que la GPU ve la última escritura del título.
