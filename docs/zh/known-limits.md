[English](../en/known-limits.md) | [Русский](../ru/known-limits.md) | [Español](../es/known-limits.md) | [Português (BR)](../pt-BR/known-limits.md)

# 已知限制

[返回 README](../../README.zh.md)

这些是 `main` 上 **Wave 11** 的限制。

- **光线追踪关闭。** 这个版本不运行结构构建器。启动器不会打开它。
- **首次启动会编译尚未缓存的管线**，大约 1–3 分钟卡顿。退出后运行一次 `warm_shaders.bat`，再从启动器打开。

这个版本跑不了的硬件见 [硬件](hardware.md)：NVIDIA（版本正在到来；AMD 先完成）、Intel Arc、RDNA1、GTX 10/16、4 GB 级显卡和机械硬盘。

## 另见

- [实测性能](performance.md)
- [如何运行](running.md)
- [硬件](hardware.md)
- [Wave 11 包含什么](technical-changes.md)
- [安全性、校验与从源码构建](security-and-build.md)
- [分支与许可证](project.md)
