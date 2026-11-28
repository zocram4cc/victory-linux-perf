[English](../en/technical-changes.md) | [Русский](../ru/technical-changes.md) | [Español](../es/technical-changes.md) | [Português (BR)](../pt-BR/technical-changes.md)

# Wave 11 包含什么

[返回 README](../../README.zh.md)

Wave 11 是 `main` 上的二进制（`RAZDA4A_XEN.exe`，x86-64-v3）。

- **树木、树叶、围栏、木桩和路牌会画出来。** `LIST_VIEWS` 里未对齐的原始缓冲视图已修复，包括 `SW_64KB_D`。Wave 11 的发布说明记录稳定性测试中没有显存压力掉帧。
- **帧生成与超分。** FSR 3.1.6 使用 3 槽 RGBA8 无 HUD 捕获和允许撕裂的交换链。超分是 FSR 4.1.2 ML，回退到 FSR 3.1。角色面部使用启动器里的 5000 ms compute 等待。
- **8 GB 显存。** Safe Mode（65% 纹理空间、2048 MB 渲染目标余量、768 MB 显存缓冲）是预设 4，也是启动器在 8 GB 显卡上的自动路径。
- **内存。** 有界回收、`recycle_landed`，以及 DXGI 档位（8/12/16/24 GB）。可用 `VICTORY_FILE_PHYSICAL` 把页放到 NVMe。
- **操作。** `F2` 或 `~` 是鼠标视角，`F3` 是 FPS。RDNA4 崩溃着色器屏蔽 `240cb516c1af0bc3` 已开启。
- **Windows 10。** 树里带有 `D3D12Core.dll`（Agility SDK 620）。

二进制目标是 x86-64-v3：AMD Zen 2–5 与 Intel Haswell 到 Arrow Lake 上的 AVX2、BMI2 和 FMA。

## 另见

- [实测性能](performance.md)
- [如何运行](running.md)
- [硬件](hardware.md)
- [已知限制](known-limits.md)
- [安全性、校验与从源码构建](security-and-build.md)
- [分支与许可证](project.md)
