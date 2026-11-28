[Русский](../ru/technical-changes.md) | [简体中文](../zh/technical-changes.md) | [Español](../es/technical-changes.md) | [Português (BR)](../pt-BR/technical-changes.md)

# What Wave 11 ships

[Back to README](../../README.md)

Wave 11 is the binary on `main` (`RAZDA4A_XEN.exe`, x86-64-v3).

- **Trees, foliage, fences, posts, and signs render.** Unaligned raw buffer views in `LIST_VIEWS` are fixed, including `SW_64KB_D`. The Wave 11 release records zero VRAM pressure drops on its stability run.
- **Frame generation and upscaling.** FSR 3.1.6 frame generation uses a 3-slot RGBA8 HUD-less capture and a tearing swap chain. Upscaling is FSR 4.1.2 ML, with FSR 3.1 as the fallback. Character faces use the 5000 ms compute wait shipped in the launcher.
- **8 GB VRAM.** Safe Mode (65% texture room, 2048 MB of target headroom, 768 MB of card buffers) is preset 4 and the launcher's automatic path for 8 GB cards.
- **Memory.** Bounded reclaim, `recycle_landed`, and dynamic DXGI VRAM steps (8/12/16/24 GB). Optional NVMe-backed pages via `VICTORY_FILE_PHYSICAL`.
- **Controls.** `F2` or `~` for mouse-look, `F3` for the FPS readout. RDNA4 crash-shader shield `240cb516c1af0bc3` is on.
- **Windows 10.** `D3D12Core.dll` (Agility SDK 620) is in the tree, so the in-box Windows 10 runtime is not a manual install.

The portable binary targets x86-64-v3: AVX2, BMI2, and FMA on AMD Zen 2–5 and Intel Haswell through Arrow Lake.

## See also

- [Performance](performance.md)
- [How to run](running.md)
- [Hardware](hardware.md)
- [Known limits](known-limits.md)
- [Security, checksums, and building](security-and-build.md)
- [Branches and license](project.md)
