[Русский](../ru/known-limits.md) | [简体中文](../zh/known-limits.md) | [Español](../es/known-limits.md) | [Português (BR)](../pt-BR/known-limits.md)

# Known limits

[Back to README](../../README.md)

These are limits of **Wave 11** on `main`.

- **Ray tracing is off.** The structure builder does not run in this release. The launcher does not turn it on.
- **The first launch compiles pipelines** it has not cached yet, about 1–3 minutes of stutter. Quit, run `warm_shaders.bat` once, and start again from the launcher.

Hardware that this build does not run is listed in [Hardware](hardware.md): NVIDIA (a version is coming; AMD shipped first), Intel Arc, RDNA1, GTX 10/16, 4 GB-class cards, and HDDs.

## See also

- [Performance](performance.md)
- [How to run](running.md)
- [Hardware](hardware.md)
- [What Wave 11 ships](technical-changes.md)
- [Security, checksums, and building](security-and-build.md)
- [Branches and license](project.md)
