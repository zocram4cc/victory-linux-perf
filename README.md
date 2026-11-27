# RYUKI: VIctory on an RX 7900 XT

A fork of [VIctory](https://github.com/Marvisak/VIctory), a compatibility layer
that runs an Xbox Series title natively on Windows from files the user supplies,
tuned and measured on an **RX 7900 XT with 32 GB of RAM** by **ryuki**.

It is [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)
(`a-publier`, itself [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)'s
latest with Belobug's AMD changes) with RYUKI's work on top: frame generation,
an audio fix that lets the title load on any output, a faster copy-back, and the
settings measured on this PC. The older RYUKI line is kept as the branch
`xtx-2026-11-27`.

**This repository contains no game files, code or assets.** You need your own
copy of the game. It is not affiliated with or endorsed by the game's developer
or publisher.

## The PC it was tested on

| | |
|---|---|
| GPU | AMD Radeon RX 7900 XT, 20 GB (driver 32.0.32015.7029), Resizable BAR on |
| CPU | AMD Ryzen 7 7800X3D (8 cores, 16 threads) |
| RAM | 32 GB DDR5 |
| OS | Windows 11 Pro, build 26300 |
| Display | 2560x1440 |
| Page file | 32 GB initial, 88 GB most |

## Where it stands (2026-11-27)

Release build, free roam from a fixed save (standing, then walking), pipelines
built ahead, `scripts\play.ps1 -FrameGen`:

| | Rendered | Shown (frame generation) |
|---|---|---|
| Free roam, 6 minutes | median 26.7 fps | **median 51.6 fps, slowest 1 % 37.6, every second at 30 or more** |
| Free roam, 10 minutes (four list threads) | median 21.3 fps | median 42.6 fps, 99 % of seconds at 30 or more |
| The older RYUKI line, no frame generation | 16-17 standing, 14.5-15.6 walking | the same |

- **Frame generation** is AMD's, through its swap chain, fed by the title's own
  FidelityFX upscale. It needs the **Series S profile**: in the Series X profile
  the title has no FidelityFX upscale and nothing is generated. Fast motion
  ghosts: the frames between are guessed from about 25 real ones.
- About 20 ms of every frame is the title's own CPU work; rendering itself is not
  at 30 fps yet.
- The first run after a new build or driver stutters while pipelines are made:
  build them ahead (below).

## How to run it

Windows 10/11 x64, a CPU with AVX2, a Direct3D 12 GPU with shader model 6.6 and
mesh shaders, Rust (the toolchain is pinned), the MSVC build tools, your own dump
of the title, and AMD's FidelityFX SDK DLLs (`Kits\FidelityFX\signedbin` of
[GPUOpen-LibrariesAndSDKs/FidelityFX-SDK](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK);
tested with the build that reports 4.1.2).

**With 32 GB of RAM, raise the page file first**: the process commits 35-40 GB.

```powershell
git clone https://github.com/YukiRyuu/RYUKI-7900XT
cd RYUKI-7900XT
copy victory.example.toml victory.toml      # set game_dir, cache_dir, data_dir: all outside this folder
cargo build --release --locked -p victory
```

Translate the shaders ahead **with the renderer-port switches set**, since port 5
changes them (translating with other settings makes the game translate again
while it plays):

```powershell
$env:VICTORY_PORT_CONSTANTS='1'; $env:VICTORY_PORT_DESCRIPTORS='1'; $env:VICTORY_PORT_CBV='1'
$env:VICTORY_PORT_GUARD='1'; $env:VICTORY_PORT_JOURNAL='1'
targeteleaseictory.exe shaders --jobs 14 <your game folder>
```

Then play:

```powershell
scripts\play.ps1 -Build release -FrameGen -Exe <your game folder>\<the game's .exe> -FidelityFxDir <FidelityFX SDK>\Kits\FidelityFX\signedbin
```

After a first session, `targeteleaseictory.exe warm --jobs 10` (with the same
port switches set) builds every pipeline the game used, so later sessions start
smooth.

If PowerShell refuses scripts: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`,
then `Unblock-File scripts\play.ps1`.

## What RYUKI adds

| | |
|---|---|
| `-FrameGen` | Series S, renderer ports 1, 2, 5, 6 and 7, the 60 fps unlock, AMD frame generation, Resizable BAR windows, six list threads and the RX 7900 XT settings below, in one switch |
| Audio as a console has it | The title talks to Windows audio itself and expects a console's output: always there, 48 kHz. With a monitor's HDMI audio (it will not open), a 44.1 kHz Bluetooth speaker or no output at all, its loading screen never ended. `modules::endpoint` gives it an output that opens, read as 48 kHz with Windows converting, else a silent one. `VICTORY_AUDIO_DEVICE=<part of a name>` or `silent` picks one; `VICTORY_AUDIO_CONSOLE=0` turns it off. |
| Frame generation | `VICTORY_FRAMEGEN=1` (from gleaf-6, after MrSkelee): AMD's swap chain, fed by the host upscaler that answers the title's FidelityFX calls. `VICTORY_FFX_HOST=1`: the host upscaler alone. |
| Copy-back | `VICTORY_SKIP_WC_BACK` (on; from PastiiJay/klbf-lab, after fxceless/victory-fork #868 and #1268): shader writes to memory the title reserved write-combined are not fetched back, for buffers of 1 MiB and more. 9.6 to 4.4 ms a frame. |
| Freeze fix | `VICTORY_SOFT_VIEWS=copies` (off by default; from Op-lite/victory-scarlett): views of the title's page-mapping copies kept in our books and mapped only if touched. In the game (6 min, `=copies`): no faults, every second at 30+ shown, about 6,000 map and 6,500 unmap calls spared per 5 s, no clear frame-rate gain. `=verify` found 8 pages in 6 min that differed from what the books would have read (or changed between the two reads): **left off** until that is understood. |
| Caption | The window reads `RYUKI`, then fps, ms and draws (`VICTORY_CAPTION_GAME=1` adds the game's name) |
| Tests | `VICTORY_QUIT_AFTER_S=<seconds>`: an unattended run ends itself |

The RX 7900 XT settings `-FrameGen` sets are listed in `scripts\play.ps1`: memory
windows of 1 MiB with 12 MB at roots, shader-write compares on the card for buffers
of 1 MiB and more, title memory in place, texture steps of 80 ms at scene cuts,
the list threads joined, among others. All measurements and what was tried and not
kept: CHANGELOG.md on the `xtx-2026-11-27` branch.

## Changing it, or forking it

Read [AGENTS.md](AGENTS.md) first (people and coding agents alike), and add an entry to
[CHANGELOG.md](CHANGELOG.md) for every change: what, why, measured how, with what result, and
where it came from. Forks: please keep both files and keep adding to them; that is how fixes
travel between forks.

## Credits

This is a tree of other people's work with more on top. The history is kept, and
commits taken from elsewhere name the commit and its author.

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors: the loader, the runtime services, the Direct3D 12 back end and
  the shader recompiler.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**: the fork
  this one started from, and the people whose pull requests it carries.
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)**: ray tracing fixes,
  cube gradients, sparse textures.
- **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**:
  the ambient light idea, pipeline workers, and work by romainhedouin.
- **[FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY)**.
- **[myokex/victory-amd](https://github.com/myokex/victory-amd)**: the guard over
  every view of a page (with Spexivus, joroyt and tightfall), and Joshua Odom's
  reads into title memory.
- **Geckomacabre** and **cxy0072019-sketch**: the texture read-back.
- **[XCL](https://github.com/ivall/xcl)** (GPL-2.0-only) was read for how the same
  title behaves under another layer; no code of it is here.
- **[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)**: the
  renderer ports and the 60 fps unlock this build runs with.
- **[gleaf-6/VIctory-7900XTX-](https://github.com/gleaf-6/VIctory-7900XTX-)** and
  **MrSkelee/VIctory-MrSkele-AMD**: frame generation.
- **[Op-lite/victory-scarlett](https://github.com/Op-lite/victory-scarlett)**: the
  freeze fix (soft views).
- **[PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab)**, Chipsraine,
  iGoldenNinja and Proudnoob4: the write-combining copy-back skip.
- RYUKI's changes (audio, frame generation port, measurements) were made by ryuki
  with [Claude Code](https://claude.com/claude-code).

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file that is not
covered is named in [docs/asset-policy.md](docs/asset-policy.md).
