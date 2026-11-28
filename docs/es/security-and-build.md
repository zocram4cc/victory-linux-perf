[English](../en/security-and-build.md) | [Русский](../ru/security-and-build.md) | [简体中文](../zh/security-and-build.md) | [Português (BR)](../pt-BR/security-and-build.md)

# Seguridad, falsos positivos y compilación desde código fuente

[Volver al README](../../README.es.md)

Brave/Edge y Windows Defender ML pueden marcar ZIPs sin firmar con `.exe` + `.bat` + `.ps1` como **`Trojan:Script/Wacatac.H!ml`**. Verifica el SHA-256 o compila tú mismo desde `dev`:

```powershell
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (todos los DLL en [`SHA256SUMS.txt`](../../SHA256SUMS.txt)).

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## Ver también

- [Rendimiento](performance.md)
- [Cómo ejecutarlo](running.md)
- [Hardware](hardware.md)
- [Qué trae Wave 11](technical-changes.md)
- [Límites conocidos](known-limits.md)
- [Ramas y licencia](project.md)
