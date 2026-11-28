[English](../en/known-limits.md) | [Русский](../ru/known-limits.md) | [简体中文](../zh/known-limits.md) | [Português (BR)](../pt-BR/known-limits.md)

# Límites conocidos

[Volver al README](../../README.es.md)

Límites de **Wave 11** en `main`.

- **El ray tracing está apagado.** El constructor de estructuras no se ejecuta en esta versión. El lanzador no lo activa.
- **El primer arranque compila pipelines** que aún no están en caché, cerca de 1–3 minutos de tirones. Cierra, ejecuta `warm_shaders.bat` una vez y vuelve a abrir el lanzador.

El hardware que esta build no ejecuta está en [Hardware](hardware.md): NVIDIA (hay una versión en camino; AMD salió primero), Intel Arc, RDNA1, GTX 10/16, tarjetas de 4 GB y HDD.

## Ver también

- [Rendimiento](performance.md)
- [Cómo ejecutarlo](running.md)
- [Hardware](hardware.md)
- [Qué trae Wave 11](technical-changes.md)
- [Seguridad, checksums y compilación](security-and-build.md)
- [Ramas y licencia](project.md)
