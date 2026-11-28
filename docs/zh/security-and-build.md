[English](../en/security-and-build.md) | [Русский](../ru/security-and-build.md) | [Español](../es/security-and-build.md) | [Português (BR)](../pt-BR/security-and-build.md)

# 安全性、杀软误报与从源码构建

[返回 README](../../README.zh.md)

浏览器（Brave/Edge）与 Windows Defender 机器学习模型可能会将含 `.exe` + `.bat` + `.ps1` 的未签名压缩包误报为 **`Trojan:Script/Wacatac.H!ml`**。请勿盲目信任未签名文件——请校验 SHA-256 或直接从 100% 开源的 `dev` 分支自行编译：

```powershell
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe`（`x86-64-v3`）SHA-256：`4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622`（所有 DLL 校验见 [`SHA256SUMS.txt`](../../SHA256SUMS.txt)）。

从源码构建（`dev` 分支，MSVC Rust 工具链）：

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## 另见

- [实测性能](performance.md)
- [如何运行](running.md)
- [硬件](hardware.md)
- [Wave 11 包含什么](technical-changes.md)
- [已知限制](known-limits.md)
- [分支与许可证](project.md)
