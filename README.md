# VIctory UM

[VIctory](https://github.com/Marvisak/VIctory) on Linux with an AMD card.
A patched RADV maps the title's memory straight into the GPU, the way the
console shares it, so nothing is copied back and forth every frame.

No game files in here. Bring your own dump.

## Status (2026-11-27)

About 44 fps at 1080p (Series X profile, ray tracing off) on the one machine
tested: Ryzen 9 5950X, 32 GB, RX 9070 XT, Arch, Wine 11.22.

Broken: the main character's face skin (`VICTORY_RECORDS=0` fixes it, slower).

## Run

```sh
./setup.sh
echo 'GAME=/path/to/your/dump' > run.conf
./run.sh shaders
./run.sh play
```

`setup.sh` fetches and builds Mesa, vkd3d-proton, a wineserver for your Wine,
the Windows SDK and VIctory itself. It names any tool it misses.
Saves, caches and logs stay in `local/`.

Settings go after the mode, or in `run.conf`:

| Setting | Effect |
|---------|--------|
| `CONSOLE=series-s` | Series S profile |
| `VICTORY_OUTPUT=2560x1440` | Render size |
| `VICTORY_BVH_BUILDER=1` | Ray tracing |
| `VICTORY_FSR_4K=performance` | FSR 4 to 4K, after `./setup.sh fsr4` |
| `SAFE=1` | Without the newer, faster paths |
| `MUTE=1` | No sound |

## Patches

`patches/` holds what changes in Mesa, vkd3d-proton and Wine. Edit under
`deps/<name>`, rebuild with `./setup.sh <name>`, save with
`tools/patch.sh <name>`.

## Credits

[VIctory](https://github.com/Marvisak/VIctory) by Marvisak and contributors,
through [avatarmanz](https://github.com/avatarmanz/VIctory-7900XTX),
[fxceless](https://github.com/fxceless/victory-fork),
[DexNom](https://github.com/DexNom/VIctory),
[Belobug](https://github.com/Belobug/VIctory-Belobug-AMD),
[FQY55kai](https://github.com/FQY55kai/ZJOU-VICTORY),
[myokex](https://github.com/myokex/victory-amd),
[triippywhale](https://github.com/triippywhale/VIctory-7900XTX),
[trustno1-coder](https://github.com/trustno1-coder/VIctory-NVIDIA),
[PastiiJay](https://github.com/PastiiJay/klbf-lab), Geckomacabre and
cxy0072019-sketch.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
