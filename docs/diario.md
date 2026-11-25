# Diario

Qué se probó, qué salió y qué se decidió. Lo más nuevo va arriba.

## 2026-11-25

- **`warm --no-pipelines` con royale:** 25.700 traducidos y 58 rechazados (joshuu rechazaba 106) en 101 s. Termina sin colgarse.
- **Imagen de royale (base joshuu), según el usuario.**
  - **Bien:** se ve muy completo. Aparecen las paredes de la habitación y cosas que en otros forks no salían, y muchas texturas se ven bien.
  - **Fallos:**
    - las cabezas no se ven, solo la boca y los ojos;
    - algunas texturas parpadean.
  - Va a la fase 5. No es prioritario ahora.
- **Compilar los shaders antes de jugar.**
  - La caché de traducciones va por el hash de las fuentes del traductor, no por el del exe. Los cambios fuera de `victory-shader` no la invalidan.
  - `warm --no-pipelines` (solo traducción) se lanza antes de medir.
  - Los pipelines gráficos dependen del estado que crea el juego, así que no se pueden hacer todos antes. Joshuu ya guarda la lista de los usados en ejecuciones anteriores y los empieza al cargar.
- **Primera vez de royale (`f0b709b`) en el mundo.**
  - Lanzado a mano, sin `VICTORY_FRAMETIME_DUMP`, con la caché de shaders fría. Llega al prólogo.
  - Cifras del log en el mundo:

    | Medida | Valor |
    |---|---|
    | fps | 2,1-2,6 (380-470 ms por frame) |
    | preparar | 150-200 ms |
    | copias | 75-100 ms, **240-320 MB** por frame |
    | texturas | 17-34 ms |
    | GPU | 155-180 ms |
    | copia de vuelta | 70-80 ms, 83-98 MB |
    | GPU ocupada | 42-43 %; parada ~225 ms por frame, casi todo *"while the submitting thread was working"* (la GPU espera a VIctory) |
    | draws | 300-700 sin pipeline todavía (caché fría) |

  - Confirma el diagnóstico del README: el frame es la cadena en serie de VIctory, y las copias son mayores que las medidas del 24-nov.
  - No vale como línea base: caché fría y sin la escena fijada.
- **Fase 0.4 (sin probar en vivo todavía).**
  - `VICTORY_MEDIR=1` activa en la build normal los contadores de remapeo de xmem (antes solo en la build de diagnóstico) y el módulo `waitstat`: esperas del título por hilo y por quién llama, con `[fence]` si es un fence de la GPU.
  - El CSV de frametime gana `remap_ms`, `maps` y `unmaps` por frame.
- **Base.**
  - `main` = JoshuuDev/victory-amd `003beef`.
  - victory-royale (ligmaSec) queda en `royale-base`: fusionarlo daba 494 conflictos en 63 archivos.
  - Los commits de royale `652aed4` (DXC para el *clamp*) y `0d34233` (copias de vuelta bajo Wine) chocan con joshuu; se portarán a mano (fase 0.6).
- **Lanzador.**
  - Perfil `royale` en victory-proton, que compila directamente `~/src/victory-royale`.
  - Compila con cargo-xwin sin cambios.
- **`victory warm` (perfil joshuu, 24-25 nov).**
  - Tradujo unos 25.700 shaders y rechazó 106.
  - Creó unos 1.700 pipelines de cómputo y se quedó colgado: 0 % de CPU y todos los hilos en `futex_wait`. No llegó a imprimir el resumen.
  - No pude ver la pila: `ptrace_scope=1` impide engancharse.
  - Pendiente para la fase 3.4.
- **`scripts/bench.sh`** y `docs/bench.md`.
  - `summary` y `compare` probados con datos sintéticos.
  - Falta la primera medida real, con la partida en el patio.
