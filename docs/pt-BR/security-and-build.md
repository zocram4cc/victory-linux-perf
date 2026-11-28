[English](../en/security-and-build.md) | [Русский](../ru/security-and-build.md) | [简体中文](../zh/security-and-build.md) | [Español](../es/security-and-build.md)

# Segurança, falsos positivos e compilação pelo código-fonte

[Voltar ao README](../../README.pt-BR.md)

Brave/Edge e Windows Defender ML podem sinalizar ZIPs sem assinatura contendo `.exe` + `.bat` + `.ps1` como **`Trojan:Script/Wacatac.H!ml`**. Verifique o SHA-256 ou compile você mesmo a partir da `dev`:

```powershell
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (todas as DLLs em [`SHA256SUMS.txt`](../../SHA256SUMS.txt)).

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## Veja também

- [Desempenho](performance.md)
- [Como executar](running.md)
- [Hardware](hardware.md)
- [O que a Wave 11 traz](technical-changes.md)
- [Limites conhecidos](known-limits.md)
- [Branches e licença](project.md)
