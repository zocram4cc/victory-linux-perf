# victory-coolfox-linux

codec_d's cool_fox build of [VIctory](https://github.com/Marvisak/VIctory), carried on for **Linux under Proton** (tested on an NVIDIA RTX 5080), with the Wine and NVIDIA fixes of the forks credited below and tooling to measure playtests. The Windows instructions and the upstream AMD fork's notes follow unchanged.

**This repository contains no game files, code or assets.** You need your own
copy of the game. It is not affiliated with or endorsed by the game's developer
or publisher.

## Run the codec_d Windows build

codec_d's own instructions, kept as written: this fork has not tested the Windows build, and the codec-d/cool_fox repository and its releases no longer resolve (2026-11-26).

Download and extract the Windows x64 ZIP from the [private releases](https://github.com/codec-d/cool_fox/releases/latest). Open PowerShell in the extracted folder and point the launcher at your own complete Xbox dump.

```powershell
$env:VICTORY_GAME_DIR = 'D:\Games\MyDump'
.\Play-VIctory.cmd pareto-residency
```

The launcher selects Series S and the RTX-tested residency settings. It builds its shader cache locally under `%LOCALAPPDATA%\VIctory\cache-pareto`. Set `VICTORY_CACHE_DIR` to reuse an existing cache outside the checkout. Windows x64, an AVX2 CPU, and a Direct3D 12 GPU with shader model 6.6 and mesh shader support are required.

To build from this checkout, install the pinned Rust toolchain and the MSVC C++ build tools with a Windows SDK, then run `cargo build --release --locked -p victory`. The same launcher finds `target\release\victory.exe`. The upstream AMD fork's notes, further down, describe its own separate launcher.

## Run on Linux (Proton)

The Windows build runs under Proton-GE through [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher), with vkd3d-proton turning Direct3D 12 into Vulkan.

### Where it stands on Linux (2026-11-26)

One machine, one route: Fedora 45, NVIDIA RTX 5080 (16 GB, driver 620.70.06), Ryzen 7 9800X3D, 30 GB RAM, GE-Proton11-14 through umu, the Series S profile, the opening of the story from a new game.

| | |
|---|---|
| Menus, videos, loading screens | 30 fps |
| Opening cutscenes, first 305 s in the world | about 22 fps on average (25 median), with the view-tree wineserver; 7.7 on average without it |
| The drive after the first cutscene | slower: frames drop while the title streams the city |
| Faces | drawn and animated: mouths move, expressions show (the fxceless PR #927 port) |
| Memory | the title holds 19-26 GB in the world; on 30 GB it swaps (the launcher notes when less than 20 GB is free) |
| Longest run so far | about 7 minutes, no crash |

What is known to be wrong:

- Stutter on camera cuts, and pop-in: after a quick cut the title shows its far, plain versions of people and places until the real ones stream in.
- Lighting at night is flat and too dark, fine shadows (hair on a neck) are missing, and eyes reflect the sky indoors. The title lights its scenes with rays through a structure its own builder makes; that builder is off (`--ray-tracing` turns it on, and other NVIDIA forks report it does not run there; untried here). What stands in for the indirect light is worked out from the frame.
- Some z-fighting and shimmer in backgrounds, and a few textures missing (a phone screen in a cutscene).
- NVIDIA's shader-visible view heap holds 1,000,000 views and the title asks for 1,245,184; the views past the cap read an empty slot.
- The title fills about 15 GB of the 16 GB card: another program holding GPU memory (a dictation model with 4.9 GB, in one run) took the opening from 24 to 3 frames a second. The launcher lists such programs before a run.

You need rustup, clang, lld and llvm (`sudo dnf install rustup clang lld llvm` on Fedora), `umu-run` on your PATH (on Fedora 45 use the release's zipapp, the RPMs are built for older Python), and **GE-Proton11-13 or later** in `~/.local/share/Steam/compatibilitytools.d/Proton-GE Latest` (or set `PROTONPATH`). Older vkd3d-proton (before v3.1-83, so GE-Proton11-12 and earlier) converts DXIL with dxil-spirv's legacy structurizer, which turns about 6% of the title's pipelines into invalid SPIR-V that NVIDIA's shader compiler crashes on, and the title dies as it loads; the launcher warns when it can tell.

**For speed, a patched wineserver.** The title maps memory views by the hundred thousand, and Wine's wineserver keeps each process's views in a list it walks on every map. [GE-Proton PR #1016](https://github.com/GloriousEggroll/proton-ge-custom/pull/1016) keeps them in a tree instead. Built into a copy of GE-Proton11-14 at `~/.local/share/victory/proton/GE-Proton11-14-viewtree` (GE's own patch set, then that patch, then `make server/wineserver`, with the copy's `files/bin/wineserver` replaced), it took the opening cutscenes on an RTX 5080 from 7.7 to 22 frames a second on average, and the long stalls went away. The launcher uses that copy when it is there; `--stock-wineserver` runs Proton as shipped.

**The first launch translates the title's shaders** before the game starts (about 70 s; a few seconds after that), and makes the pipelines earlier runs drew. Without it a first run shows a black screen for minutes while they are made on demand. It runs again by itself whenever the build or a `VICTORY_*` switch changes; `VICTORY_PREBUILD=0` skips it.

Cross-compile with [cargo-xwin](https://github.com/rust-cross/cargo-xwin); it downloads Microsoft's CRT and Windows SDK, whose licence you accept with `XWIN_ACCEPT_LICENSE=1`:

```sh
rustup-init -y --default-toolchain none --no-modify-path
export PATH="$HOME/.cargo/bin:$PATH"
rustup toolchain install 1.98.1 --target x86_64-pc-windows-msvc
cargo install --locked cargo-xwin
scripts/build-linux.sh            # or --diag for the diagnostics build
```

Put `dxcompiler.dll` and `dxil.dll` from the x64 folder of Microsoft's [DirectXShaderCompiler release](https://github.com/microsoft/DirectXShaderCompiler/releases) in `~/.local/share/victory/dxc` (or set `VICTORY_DXC_DIR`); the launcher copies them beside `victory.exe`. Under Wine the system shader compiler refuses the indirect-draw clamp and the card compare, and without DXC there is no 3D.

```sh
VICTORY_GAME_DIR=~/Games/MyDump scripts/play-linux.sh --skip-intro --new-game
scripts/play-linux.sh shaders <the shader archives>   # translate every shader ahead of time
scripts/play-linux.sh warm --used                     # pipelines earlier runs drew with
scripts/play-linux.sh --help                          # every option
```

Options for a game run (`scripts/play-linux.sh --help` has them all):

| Option | What |
|---|---|
| `--skip-intro` | presses through the boot screens and logos (Enter/A at 12-50 s), so the run reaches the load sooner |
| `--new-game` | starts at the opening scene: the story save and player record are moved to `~/.local/share/victory/save-backups/<time>` (nothing is deleted; settings stay), then a player record from `~/.local/share/victory/save-templates/new-game` is copied in, so the terms of service are not asked again. Runs started this way compare frame for frame |
| `--diag` | the diagnostics build (`scripts/build-linux.sh --diag`) |
| `--stock-wineserver` | Proton-GE Latest as shipped, even where the view-tree copy is installed |
| `--levels-on-demand` | texture mip levels as draws need them (off by default: it used more memory, stuttered more and left a face black) |
| `--no-texture-waits` | the title's shorter waits for textures in the first frames and the first cutscene |
| `--safe`, `--ray-tracing` | as `scripts/play.ps1 -Safe` and `-RayTracing` |
| `--bench <route>` | a scripted run from `scripts/bench/<route>.keys`, stopped when it ends |
| `--mangohud` | also MangoHud's frame log (off by default: its NVIDIA thread aborted the title under umu) |
| `--proton-log` | Wine's own log into the run folder (large) |

The launcher uses its own prefix (`~/.local/share/victory/prefix`), keeps caches in `~/.cache/victory/`, and sets the same defaults as `Play-VIctory.cmd`, plus meeyao's vkd3d settings (`VKD3D_CONFIG=descriptor_heap`, `VK_NV_raw_access_chains` off), `VICTORY_CULL_NONE=1` (both faces drawn), every texture's mip levels as the title loads it (`VICTORY_LEVELS_ON_DEMAND=0`) and the willy build's longer texture waits (`VICTORY_EARLY_TEXTURES_MS=40`, `VICTORY_BAKE_TEXTURES_MS=10000`). Every variable can be overridden from the environment. Before a run it notes when less than 20 GB of RAM is free (the title wants about that much in the world, and swaps otherwise), and lists other programs holding 1 GB or more of GPU memory.

Game memory goes into VRAM through Resizable BAR by default (`VICTORY_WINDOWS_BAR_MB=2048`, `VICTORY_CARD_WINDOWS_MB=1024`; `0` turns either off). On an RTX 5080 under Windows it was the difference between about 8 and 18 fps (Zarina35's measurement, not this fork's); vkd3d-proton has the GPU upload heap it needs.

### Playtesting

Each game run leaves a folder in `~/.local/share/victory/runs/<date-time>/` (`latest` points at the newest). It holds no game data, only numbers, logs and your screenshots:

| File | What |
|---|---|
| `run.json` | build, git revision, Proton, umu, driver, GPU, CPU, RAM, every `VICTORY_*`/`VKD3D_*` setting, exit code, length, whether Resizable BAR was taken |
| `victory.log` | the engine's output, with times |
| `frames.csv` | one row a frame (`VICTORY_FRAMES_CSV`): frame time, the submitting thread's work, the card's idle and waits, copies, copy-back, uploads, draws and drops, pipelines queued and made, VRAM |
| `mangohud.csv` | with `--mangohud`: MangoHud's own frame log (Shift+F2 pauses it, Shift+F12 shows its overlay) |
| `system.csv` | once a second: the title's memory and swap, free RAM, swap, commit, VRAM, GPU load, clock and heat, CPU load |
| `marks.csv`, `marks/` | moments you marked, with a screenshot each |

```sh
scripts/play-linux.sh --skip-intro [--diag]         # play; Ctrl-C (or closing the window or the terminal) ends the run cleanly
scripts/victory-mark.sh --install                   # once: the desktop entry; bind it to Ctrl+Alt+M in System Settings
scripts/victory-mark.sh --note 3 "bridge stutter"   # a note on mark 3 afterwards
scripts/victory-report.py                           # the newest run, summed up
scripts/victory-report.py <run> --against <run>     # two runs side by side
scripts/play-linux.sh --bench <route>               # a scripted run from scripts/bench/<route>.keys
```

Press the mark shortcut at anything odd: a stutter, a glitch, a crash coming. The run is ended with SIGTERM, never SIGKILL: killing the title while its mappings are live can wedge the NVIDIA driver until a reboot. A benchmark route is a list of `VICTORY_KEYS` holds and a `duration=` line; see `scripts/bench/example.keys`. Leave enough RAM free before a run: the title wants most of 30 GB.

## The upstream AMD fork's notes

What follows is the upstream AMD fork's own description, as codec_d's cool_fox
carried it: its goals, its measurements (made on that author's Windows PC, not
by this fork) and its changes.

A fork of [VIctory](https://github.com/Marvisak/VIctory), a compatibility layer
that runs an Xbox Series title natively on Windows from files the user supplies.
This one is about **speed on AMD RDNA 3**: the world at 30 frames a second on
one PC, without giving up the picture. It is not there yet.

### Where it stood upstream, on Windows and AMD (2026-11-25)

Measured by the upstream AMD fork on its author's machine, not by this fork:
Windows 11, Intel i9-12900K, 64 GB RAM, Radeon RX 7900 XTX, 1440p, the Series X
profile.

| | |
|---|---|
| Menus, videos, loading screens | 30 fps |
| Prologue, indoors | 50-75 ms a frame (13-20 fps) |
| First mission with the boat | 46-53 ms (19-22 fps) |
| Street chase with many cars and people | 100-165 ms (6-10 fps), single frames far longer |
| Outdoors on foot, the camp | 60-90 ms |
| Flying | 42-57 ms |
| Longest session so far | 41 minutes, no crash |
| Memory | 40-46 GB of private memory: **64 GB of RAM recommended** |

It started at 280 ms a frame in the same scenes two days earlier.

What is known to be wrong:

- Trees, plants and far buildings flicker outdoors in some places.
- Rooms with little direct light are too dark without `-RayTracing`.
- Textures of what just came into view appear late while the title streams.
- The first run with a new driver or a new build stutters while the driver makes
  pipelines (`victory warm` makes them ahead).
- Recording a frame in the world makes the title's own watchdog end it.

### How to run it (upstream, Windows)

You need Windows 10/11 x64, a CPU with AVX2, a Direct3D 12 GPU with shader
model 6.6 and mesh shaders, Rust (the toolchain is pinned), the MSVC build
tools, and your own dump of the title.

```powershell
git clone https://github.com/avatarmanz/VIctory-7900XTX
cd VIctory-7900XTX
copy victory.example.toml victory.toml      # then set your paths, outside this folder
cargo build --locked --profile diag -p victory --features diagnostics
scripts\play.ps1 -Exe <path of your copy's executable>
```

`scripts\play.ps1 -Safe` leaves out the newest, faster ways; try it first when
something is wrong. `-RayTracing` turns the title's ray-traced indirect light on.

Before the first run, ahead of time (both can be stopped and go on later):

```powershell
target\diag\victory.exe shaders <the shader archives>   # every shader translated, minutes
target\diag\victory.exe warm --used                     # pipelines earlier runs drew with, for a new driver or build
```

Commands, debugging switches and upstream's own description:
[docs/README-fxceless.md](docs/README-fxceless.md). Every `VICTORY_...` switch is
described in a comment where it is read.

### What is different upstream

In short; the reasons and the measurements are in [docs/findings.md](docs/findings.md).

- **List calls are written down, not carried out, on the title's threads**
  (`VICTORY_THIN`): eight threads of ours translate them.
- **Submissions are prepared by a thread of ours and joined**
  (`VICTORY_ASYNC_SUBMIT`, `VICTORY_JOIN_PARTS`), with what can be spread over
  threads spread: refreshes, compares (AVX2), descriptor records, landings.
- **What shaders wrote comes back beside the write guards**, by the dwords that
  changed, on a thread of its own (`VICTORY_RELAY`, `VICTORY_LAND`).
- **Buffer fills that need bytes still on the card are done on the card**
  (`VICTORY_CARD_STEPS`) instead of waiting for it.
- **The title's writes are caught more cheaply**: one view opened at a fault,
  blocks that rest, places in the title's code that look before they write.
- **Textures go up on a copy queue of their own** and a slot shows one only after
  its upload ran, which also ended a device loss that came every 15 minutes or so.
- **Boats float**: textures that shaders write and the title's CPU reads go back
  to title memory (from fxceless and Geckomacabre, on by default here).
- **Frame record and replay** with per-pass times and shader variants compared
  turn by turn ([docs/replay.md](docs/replay.md)).

## Credits

This is a tree of other people's work with more on top. The history is kept, and
commits taken from elsewhere name the commit and its author.

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors: the loader, the runtime services, the Direct3D 12 back end and
  the shader recompiler.
- **codec_d's cool_fox** (mirrored at
  [junikka/cool_fox](https://github.com/junikka/cool_fox)): the Windows build this
  fork carries on, from its commit f4b48ad.
- **[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)**:
  the upstream AMD fork whose notes are kept above.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**: the fork
  the upstream AMD fork started from, and the people whose pull requests it carries.
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
- For Linux and NVIDIA, this fork took:
  - [fxceless PR #927](https://github.com/fxceless/victory-fork/pull/927): 16-bit
    values through descriptor tables move as they are, so the face network
    computes and faces animate.
  - [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA):
    d1fe0bd and fdc8bbc (D3D12 error 538), c05b44b (error 646) and
    `VICTORY_PIPELINE_WAIT_ONCE`.
  - The willy build (a release zip): the NaN guard on stores
    (`VICTORY_NAN_GUARD`), R11G11B10 for R9G9B9E5 targets (`VICTORY_E5_TARGETS`),
    and its texture settings (mip levels as a texture loads, longer texture waits).
  - [GE-Proton PR #1016](https://github.com/GloriousEggroll/proton-ge-custom/pull/1016):
    the wineserver that keeps memory views in a tree.
  - [meeyao/victory-8gb-linux](https://github.com/meeyao/victory-8gb-linux): the
    Wine fixes (the DXC fallback, the null pipeline-library blob, both faces
    drawn) and the vkd3d settings.
  - [Zarina35/victory-coolfox-nvidia](https://github.com/Zarina35/victory-coolfox-nvidia):
    the NVIDIA wave32 fixes, scripted sticks and the Resizable BAR sizes for an
    RTX 5080.
  - [kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX](https://github.com/kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX):
    one back edge per loop header (single latches), and the idea of the
    view-tree wineserver.
- This fork's changes were made with [Claude Code](https://claude.com/claude-code).

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file that is not
covered is named in [docs/asset-policy.md](docs/asset-policy.md).
