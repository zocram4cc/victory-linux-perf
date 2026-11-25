# Escena de referencia y `bench.sh`

Este documento corresponde a la fase 0.2 y 0.3 del [README](../README.md).

## La escena

No hace falta partida guardada:

1. Empieza una partida nueva.
2. Sáltate la primera cinemática.
3. Al llegar al primer momento en el que controlas a la protagonista, esa es la escena.

Antes de medir tras actualizar el traductor, calienta la caché con `victory-proton -p royale victory warm --no-pipelines` (unos 100 s). La versión con pipelines se cuelga con RADV (fase 3.4).

## Medir

| Escena | Qué hacer | Comando |
|---|---|---|
| A, estática | llega a la escena, deja la cámara quieta, espera unos 20 s tras el primer frame del mundo, y pulsa Enter | `scripts/bench.sh run -l base-A` |
| B, en movimiento | desde el mismo punto, pulsa Enter y camina en línea recta durante 30 s | `scripts/bench.sh run -l base-B -s 30` |

- **Lo que hace `run`:**
  1. Lanza el juego con `victory-proton -p royale run --no-update` y `VICTORY_FRAMETIME_DUMP=1`.
  2. Espera a que pulses Enter y mide la ventana.
  3. Al acabar, cierra el juego. Con `--keep` lo deja abierto, y entonces `scripts/bench.sh mark -l ...` vuelve a medir sin relanzarlo.
- **Para probar un switch**, añádelo al final del comando, por ejemplo: `scripts/bench.sh run -l techo-sin-copias VICTORY_COPY_MODE=none`.
- **Resultados:** van a `bench/<fecha>-<etiqueta>/`, que no se sube al repositorio:
  - `frametime.csv`: solo la ventana medida;
  - `summary.json`: media, p50, p95, mín. y máx. de cada columna, más los núcleos usados por el juego y por wineserver;
  - `meta.json`: commit, switches y perfil;
  - `log.txt`: el log de la ventana.
- **Comparar ejecuciones:**
  ```sh
  scripts/bench.sh compare bench/<A> bench/<B> [...]   # tabla con la diferencia frente a A
  ```
  Esa tabla es la que va en el mensaje de cada commit de rendimiento.

## Reglas para que las cifras valgan

- **Tres ejecuciones por configuración.** Si `ms_per_frame` varía más de un 10 % entre ellas, la medida no vale. Hay que buscar la causa: caché de shaders fría, otra carga en el PC, otra escena…
- **La primera ejecución tras cambiar de commit calienta la caché de shaders y se descarta.** Otra opción es lanzar antes `victory-proton -p royale victory warm`.
- **Con el mismo PC, el mismo GE-Proton y el wineserver parcheado.** El `log.txt` dice cuál se usó.
- **Sin otros juegos ni programas pesados abiertos.**
