# Diario

Qué se probó, qué salió y qué se decidió. Lo más nuevo va arriba.

## 2026-11-25

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
