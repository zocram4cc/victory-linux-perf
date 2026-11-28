[English](../en/security-and-build.md) | [简体中文](../zh/security-and-build.md) | [Español](../es/security-and-build.md) | [Português (BR)](../pt-BR/security-and-build.md)

# Безопасность, ложные срабатывания антивируса и сборка из исходников

[К README](../../README.ru.md)

Браузеры (Brave/Edge) и Windows Defender ML могут помечать неподписанные ZIP-архивы с `.exe` + `.bat` + `.ps1` как **`Trojan:Script/Wacatac.H!ml`**. Не доверяйте неподписанным бинарям на слово — сверьте хеш или соберите бинарь сами из открытой ветки `dev`:

```powershell
# Проверка хеша по SHA256SUMS.txt
Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256
```

- Wave 11 `RAZDA4A_XEN.exe` (`x86-64-v3`) SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (хеши всех DLL см. в [`SHA256SUMS.txt`](../../SHA256SUMS.txt)).

Сборка из исходников (ветка `dev`, Rust MSVC):

```powershell
git clone --branch dev https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
cargo xtask agility
$env:RUSTFLAGS = "-C target-cpu=x86-64-v3"
cargo build --release --locked -p victory
```

## См. также

- [Замеры](performance.md)
- [Как запускать](running.md)
- [Железо](hardware.md)
- [Что входит в Wave 11](technical-changes.md)
- [Известные ограничения](known-limits.md)
- [Ветки и лицензия](project.md)
