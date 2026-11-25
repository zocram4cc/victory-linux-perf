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

## Experimentos de techo (fase 0.5)

Pendientes.
