# VIctory on Linux (Proton)

This repository is [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)
with the changes it takes to run it **on Linux under Proton**, on AMD with Mesa (RADV).
It is not a Linux port of the layer: the Windows build runs under Proton
Experimental, and this repository makes that work and keeps it fast.

**No game files are included.** You need your own dump of the Xbox Series build.

## Where it stands

| | |
|---|---|
| Tested on | RX 7900 XT 20 GB, Ryzen 7 5800X3D, 32 GB RAM, Proton Experimental 11.0, Mesa/RADV |
| Menus, videos | 30 fps (the title's cap) |
| Open world (4,000-5,000 draws) | **~18-19 fps** (53-57 ms a frame) |
| Very heavy views (8,000-10,000 draws) | **~16-17 fps** |
| Processor | all 16 threads of a 5800X3D at 65-80 % |
| Stability | long sessions without GPU hangs; saving and loading work |

The numbers need the huge pages of [`linux/thp/`](linux/thp/) (one `setcap`, see Setup); without
them it is luck, between ~9 and ~18 fps in the same scene.

Not tested: NVIDIA (proprietary or NVK), Intel, AMD cards other than RDNA 3.
**On Windows** use [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)
with its `scripts\play.ps1` (and a page file of 32 GB or more with 32 GB of RAM).

## What this repository changes for Linux

- **Shader translation**: one back edge per loop header (single latch) in the DXIL
  it writes. vkd3d-proton's dxil-spirv recursed without end on several shaders;
  the pipeline workers died ("overflowed its stack") and pipelines were never made
  (missing faces, stalls).
- **Renderer port 1 under Wine** (`VICTORY_PORT_CONSTANTS`): vkd3d-proton opens a
  heap from an address by giving the pages to the AMD driver as a user pointer, and
  the driver takes anonymous memory only. On a view of Wine's sections the card read
  nothing and the picture was black. The title's 32-bit heap's fresh pages are now
  private memory, and the heap is opened on the title's own pages.
- **Write guards**: uncommitted regions are remembered instead of asking Wine
  75,000-216,000 times a second (`VirtualQuery` under Wine's process-wide lock).
- **`VICTORY_COPY_COMPARE`**: only the 4 KiB pieces whose bytes changed are sent up.
- **`VICTORY_WIDE_THREADS`**, per-site counters of memory calls in the log
  ("memory calls by site"), **`VICTORY_LOG_FILE`** (a log file under Proton).
- **Huge pages for renderer port 1** ([`linux/thp/`](linux/thp/)): amdgpu checks the port-1 heap
  (a userptr, ~185 MB) page by page on every submission, and with 4 KiB pages that kept the
  `vkd3d_queue` thread at 100 % of a core. The start script collapses it into 2 MiB pages from
  outside the process (`process_madvise(MADV_COLLAPSE)`): ~9 -> ~18-19 fps in the same scene.
- **[`linux/victory-proton.sh`](linux/victory-proton.sh)**: a start script with the
  settings that worked best.

## Setup

### Prerequisites

- Steam with **Proton - Experimental** (Library -> Tools)
- An AMD GPU on Mesa/RADV
- 32 GB RAM minimum (tight), 64 GB is better
- git, [rustup](https://rustup.rs), clang and lld
  (Arch: `pacman -S clang lld`; Debian/Ubuntu: `apt install clang lld`)
- A native x64 **d3dcompiler_47.dll**: `winetricks d3dcompiler_47` and copy it out of
  that prefix's `system32`, or extract it from the Firefox 62 installer.
  Wine's builtin one cannot compile the layer's helper shaders and the 3D world stays black.

### Build

```sh
git clone <this repository>
cd <this repository>
cargo install cargo-xwin            # once; rust-toolchain.toml picks the toolchain
cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
```

The first build downloads the Windows SDK headers and libraries (accept the license prompt).

### Huge pages (once, recommended)

```
cc -O2 -o linux/thp/victory-thp-collapse linux/thp/victory-thp-collapse.c
sudo setcap cap_sys_nice+ep linux/thp/victory-thp-collapse
```

The helper gets one capability (`CAP_SYS_NICE`), not root. Details in [`linux/thp/README.md`](linux/thp/README.md).

### Run

1. Edit the paths at the top of `linux/victory-proton.sh`:
   `GAME_EXE` (your dump's main executable), `D3DCOMPILER`, `PREFIX` (a new folder for
   its own Proton prefix), optionally `LOGDIR`.
2. `./linux/victory-proton.sh`

The first start translates and compiles shaders and takes minutes; later starts are
faster. `VICTORY_HUD=1 ./linux/victory-proton.sh` shows a MangoHud overlay.

## Settings that matter

- **Huge pages for port 1** (above): the biggest single factor here after the renderer ports.
  `run-*.thp` next to the log says whether they took (`88 collapsed, 0 not`).
- **Optional: a wineserver with a view tree** ([`linux/wineserver/`](linux/wineserver/)). The stock server
  walks all of the process's views on every map and unmap; with the title's tens of thousands of views
  that was 11-24 % of all CPU samples here. Patched, on GE-Proton11-14: 8.3 -> 17.7 fps, write watch
  17-26 -> 4.6 ms. It has to be built against your Proton's own Wine.
- **Do not set `RADV_DEBUG=hang`.** It turns on syncshaders and makes the GPU the
  bottleneck: ~205 ms of GPU time a frame, about 4 fps.
- The script turns on the fork's renderer ports 1, 2, 5, 6 and 7
  (`VICTORY_PORT_CONSTANTS`, `_DESCRIPTORS`, `_CBV`, `_GUARD`, `_JOURNAL`):
  ~11 -> ~16 fps here, and 4-7 -> 14-15 fps in the heaviest views (port 1).
- `VICTORY_FRAME_INTERVAL_MS=0`: the title's own 30 fps cap is off (menus and
  cutscenes go above 30; the game logic is not tied to it). `33.333` puts it back.
- `VICTORY_ONE_PAD=0`: the fork's keyboard stand-in pad is not merged with real pads
  (an analog keyboard's gamepad mode would get every key twice).
- Devices that pretend to be joysticks (drawing tablets, analog keyboards) can steer
  on their own: list them in `SDL_GAMECONTROLLER_IGNORE_DEVICES` (`0xVID/0xPID,...`).
  OpenTabletDriver's virtual tablet (`0x0000/0x0000`) is already in the script.
- The Series S profile (`--console series-s`) works, but looked much worse here for
  little more speed.

## Troubleshooting

| | |
|---|---|
| The 3D world is black | `d3dcompiler_47` is not the native one |
| The picture is black from the first screen | port 1 without this repository's change, or a build from upstream |
| "overflowed its stack" in the log, missing faces | built from a tree without the single-latch change |
| The game steers on its own | a joystick-like device: see above |
| Stutter on first runs | pipelines being made; they are kept for the next runs |

Logs are in `LOGDIR` (default `~/victory-logs/run-*.txt`).

## Known issues

- The picture can wash out (too bright) now and then for a while.
- The intro can flicker.
- RAM: 32 GB is tight; close big applications. The process commits 30-40 GB;
  on Linux that needs swap (16 GB or more is a good idea).
- VRAM: 15-18 GB used on a 20 GB card. 12 GB cards will likely run out; try
  `VICTORY_CARD_BUFFERS_MB=512` and `--console series-s`.
- Measured and not kept here: `VICTORY_INPLACE=1` (~1,100 ms a frame under Proton:
  the GPU reads title memory over PCIe), `VICTORY_SUBMIT_FRAMES=2`,
  `VICTORY_GUARD_REST=100000`, more helper threads.

## License and credits

GPL-3.0, as the projects it builds on: [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX),
[fxceless/victory-fork](https://github.com/fxceless/victory-fork) and the forks credited in
[README.md](README.md). The ocean copy-back idea for floating boats goes back to
Geckomacabre's vx-layer via Neptune_VI, the vkd3d SQC fault analysis was done with
the community. Not affiliated with or endorsed by the game's developer or publisher.
