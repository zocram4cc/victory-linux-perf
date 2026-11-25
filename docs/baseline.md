# Línea base

## Escena A (estática), commit `176cc10`, 25-nov-2026

- **Equipo:** RX 7900 XT con RADV (Mesa 26.3.1), Ryzen 9 9900X, GE-Proton11-14, wineserver con árbol de vistas.
- **Variables:** las del perfil `royale` (`VICTORY_SKIP_PIPELINES=fc5e66f8f873d3bb VICTORY_NO_WATCHDOG=1`, `VKD3D_CONFIG=descriptor_heap`).
- **Caché:** traducciones calientes (`warm --no-pipelines`).
- **Medida:** tres ejecuciones de 60 s (56 filas cada una). La variación de `ms_per_frame` es menor del 1 %.

| Medida | A1 | A1b | A3 | **Media** |
|---|---|---|---|---|
| fps | 2,8 | 2,8 | 2,8 | **2,8** |
| ms por frame | 354,3 | 355,3 | 358,3 | **356** |
| peor frame | 363,0 | 363,5 | 365,8 | **364** |
| preparar (CPU) | 123,9 | 123,0 | 124,8 | **124** |
| · copias | 63,4 ms / 215,7 MB | 60,8 / 214,3 | 62,1 / 215,0 | **62 ms / 215 MB** |
| · texturas | 4,0 | 3,9 | 3,8 | **4** |
| GPU | 158,6 | 159,0 | 159,4 | **159** |
| GPU ocupada | 45,1 % | 45,2 % | 44,9 % | **45 %** |
| copia de vuelta | 67,2 ms / 78,3 MB | 68,8 / 77,9 | 69,5 / 80,6 | **68 ms / 79 MB** |
| pasadas de hash | 558 | 559 | 558 | **558** |
| CPU del juego | 1,4 núcleos | 1,4 | 1,3 | **1,4** |
| CPU de wineserver | (sin medir) | 0,06 | 0,06 | **0,06** |
| draws sin pipeline | ~340 | | | **~340** (constante: no es la caché) |

**Lectura:**
- Las copias de subida y de vuelta suman unos **130 ms** de los 356 de cada frame (37 %).
- En la "GPU" (159 ms) la tarjeta solo está ocupada el 45 %, así que lo demás es espera.
- La CPU del juego no está saturada: 1,4 núcleos.
- El wineserver parcheado ya no pesa: 0,06 núcleos.

Los datos completos están en `bench/` (local).

## Escena B (caminando 30 s), commit `b6660c1`, una ejecución

| fps | ms por frame | preparar | GPU | copias | copia de vuelta |
|---|---|---|---|---|---|
| 2,6 | 386 | 144 | 167 | 69 ms / 229 MB | 70 ms / 86 MB |

Moverse cuesta un 9 % más que la escena estática, por más copias. **Se ven las caras** (ver `memoria.md` §7). Faltan dos ejecuciones más para que cuente como línea base.

## Experimentos de techo (fase 0.5), 25-nov-2026

| Ejecución | fps | ms por frame | GPU | copias | Imagen (según el usuario) |
|---|---|---|---|---|---|
| base | 2,8 | 354 | 159 ms | 63 ms / 216 MB | cabezas invisibles, texturas que parpadean |
| `COPY_MODE=none` | 2,8 | 359 | 160 | 64 / 220 | **no válida**: el switch solo funcionaba en la build de diagnóstico (arreglado en `medir::copy_mode`, hay que repetirla) |
| `COPY_MODE=plain` | 2,8 | 360 | 160 | 64 / 223 | **no válida**, por lo mismo |
| `INPLACE=1` | **1,6** | **638** | **396 ms** | 84 / 401 | **se ven las caras** (de espaldas bien; de frente negras); sin parpadeos. El usuario se movió un poco |
| `INPLACE=2` | — | — | — | — | **pierde el dispositivo** al llegar al mundo (0x887A0005) |
| `INPLACE_PROBE=1 MEDIR=1` | 2,7 | 369 | 159 | 67 / 230 | **caras de Lucía y de los demás bien**, texturas bien, un objeto algo negro |
| `COPY_MODE=none` (repetida, ya válida) | **5,9** | **170** | 72 | 22 / 68 | pantalla negra (lo esperado); los mismos draws que la base (5.037) |
| `COPY_MODE=plain` (repetida) | — | — | — | — | se cierra antes de la cinemática: lee una página *reserved, never committed* |
| `MEDIR=1` solo | 2,8 | 358 | 160 | 62 / 216 | caras invisibles |
| `INPLACE_PROBE=1` solo | 2,7 | 366 | 161 | 63 / 216 | caras invisibles |

### Lo que dice la sonda (vkd3d-proton + RADV en este PC)

- **`OpenExistingHeapFromAddress` y `…1` sobre memoria privada: funcionan.**
  - La GPU lee lo que escribe la CPU sin ninguna llamada de por medio.
  - La CPU lee lo que escribe la GPU, por copia y por UAV.
  - El *write watch* sigue funcionando.
  - El título puede seguir protegiendo y liberando sus páginas.
  - Abrir 64 MB cuesta 8 ms.
- **`OpenExistingHeapFromFileMapping`:**
  - Rechaza las secciones `SEC_RESERVE`, que es como está hoy el *pool* de páginas.
  - Acepta las `SEC_COMMIT`, pero **la GPU no ve los bytes de las vistas**, ni al leer ni al escribir.
  - Conclusión: el *pool* actual no se puede abrir en sitio por secciones. Hay que rehacerlo.
- **Ancho de banda:** la tarjeta copia 64 MB de memoria del sistema en 2,4 ms (~27 GB/s, PCIe).
- **Coste real de `INPLACE=1`:** pone 340-360 MB de "ventanas" en sitio, y el tiempo de GPU pasa de 159 a 396 ms, porque los shaders leen por PCIe en lugar de VRAM.

### Esperas del título (`VICTORY_MEDIR=1`)

- 42 hilos. **Incluso el que menos espera pasa un 82 % del tiempo esperando**, casi todo en `title+0x2215eca` (los *workers* de trabajos).
- Ninguna espera es sobre un fence de la GPU.
- Encaja con lo medido el 24-nov: la CPU del título está ociosa y el frame lo marca la cadena de VIctory.

### Conclusiones

El desarrollo de todo esto está en [memoria.md](memoria.md).


1. **Las caras no faltan por los shaders.** Con los datos leídos en sitio se ven, así que lo más probable es que las copias del host lleguen **viejas o incompletas**. Es un problema de coherencia de las copias, y también de corrección, no solo de velocidad.
2. **La memoria del sistema en sitio no sirve para lo que la GPU lee mucho**: duplica el tiempo de GPU. La fase 2 tiene que llevar esos datos a VRAM (ReBAR o copia solo de lo que cambia), y dejar en sitio solo lo que se lee poco o lo que lee la CPU.
3. **Que las caras se vean con `PROBE+MEDIR`, que no colocan nada en sitio, apunta a una carrera de tiempos.** `waitstat` cambia un poco el ritmo de las esperas. Hay que separar `MEDIR=1` y `INPLACE_PROBE=1` y repetir.
