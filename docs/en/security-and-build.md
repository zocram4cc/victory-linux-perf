[Русский](../ru/security-and-build.md) | [简体中文](../zh/security-and-build.md) | [Español](../es/security-and-build.md) | [Português (BR)](../pt-BR/security-and-build.md)

# Security, antivirus false positives, and build from source

[Back to README](../../README.md)

Browsers (Brave/Edge) and Windows Defender ML may flag unsigned GitHub ZIPs containing `.exe` + `.bat` + `.ps1` scripts as **`Trojan:Script/Wacatac.H!ml`**. Never trust unsigned binaries blindly — verify checksums or compile from the 100% open-source `dev` branch yourself:

```powershell
# Verify downloaded binary against SHA256SUMS.txt
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (all DLL hashes are listed in [`SHA256SUMS.txt`](../../SHA256SUMS.txt)).

Build the exact binary from source (`dev` branch, MSVC Rust toolchain):

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## See also

- [Performance](performance.md)
- [How to run](running.md)
- [Hardware](hardware.md)
- [What Wave 11 ships](technical-changes.md)
- [Known limits](known-limits.md)
- [Branches and license](project.md)
