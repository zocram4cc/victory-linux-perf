# breakwater-victory-7900xtx

A fork of [VIctory](https://github.com/Marvisak/VIctory), a compatibility layer
that runs an Xbox Series title on a PC from files the user supplies. This fork
is for **Linux under Proton on an AMD Radeon (RADV)**, and its aim is a picture
that holds still before a frame rate that looks good.

**This repository holds source code only: no game files, no assets, no keys, no
screenshots of the title, no binaries built from it.** You need your own copy of
the title. The project is not affiliated with or endorsed by the title's
developer or publisher, nor by Microsoft or AMD.

> Everything below is measured on one machine, in one house and
> the street outside it. Read every number with that in mind.

## At a glance

| | |
|---|---|
| Runs | menus, intro, the open world, on foot |
| Frame rate | about **24 fps** in the house, 18-23 outdoors (Series X profile, 3840x2160 out) |
| Picture | steady light, wooden floors, a readable phone; see [Known faults](#known-faults) |
| Ray tracing | off by default; on it gives 11-14 fps and light that is not yet right |
| Tested on | RX 7900 XTX, Mesa RADV 26.2, vkd3d-proton 3.2, Proton with a patched wineserver, Fedora |
| Based on | [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX) at `c9c932c` |
| License | GPL-3.0-or-later (`LICENSE`) |

## Contents

1. [What you need](#what-you-need)
2. [Build](#build)
3. [Run](#run)
4. [Modes](#modes)
5. [Known faults](#known-faults)
6. [What this fork changed](#what-this-fork-changed)
7. [Measuring a change](#measuring-a-change)
8. [For agents](#for-agents)
9. [Where things are](#where-things-are)
10. [Credits](#credits)

## What you need

- Your own copy of the title (the folder with its executable and `shaders.rpf`).
- Linux with an AMD card of RDNA 3 or 4 and a recent Mesa. 24 GB of video
  memory and 32 GB of RAM are what this was tested with.
- Proton with vkd3d-proton. A wineserver that finds memory views by address
  makes streaming many times faster (the patch of fxceless/victory-fork #184;
  it is not in this tree).
- Microsoft's `d3dcompiler_47.dll` in the prefix. Wine's own compiler refuses
  some of the layer's own shaders.
- Rust (the toolchain is pinned in `rust-toolchain.toml`) and `cargo-xwin`.

## Build

The layer is a Windows program, built on Linux:

```sh
cargo xwin build --release --locked -p victory --target x86_64-pc-windows-msvc
```

With the diagnostics the tools under [Measuring a change](#measuring-a-change)
read (slower: do not measure frame rates with it):

```sh
cargo xwin build --profile diag -p victory --features diagnostics --target x86_64-pc-windows-msvc
```

The result is `target/x86_64-pc-windows-msvc/release/victory.exe`. Put
vkd3d-proton's `d3d12.dll` and `d3d12core.dll` beside it.

## Run

```sh
wine victory.exe run "<title folder>/<title executable>" --console series-x --execute
```

Before the first play session, translate the title's shaders and make the
host pipelines once. Without this the first minutes stutter and draws are
dropped while pipelines are made:

```sh
wine victory.exe shaders "<title folder>/shaders.rpf" --jobs 16
wine victory.exe warm --jobs 8          # after one run has written the pipeline book
```

Both go on where they were stopped. They are keyed on the switches that change
the translation, so run them with the switches you play with.

Every setting is an environment variable read at start. The ones that matter
are under [Modes](#modes); `scripts/play.ps1` lists the play settings the base
fork ships, and the doc comment where each switch is read says what it does.
`linux/launch-ports.sh` is the launcher the measurements were made with, every
switch with its reason beside it; its paths are tokens to replace first
(`linux/README.md`).

## Modes

Set these on top of the play settings in `scripts/play.ps1`.

| Mode | Switches | Frame rate in the house | Picture |
|---|---|---|---|
| **Stable** (recommended) | `VICTORY_PORT_CONSTANTS=0` `VICTORY_REFLECT_SCREEN=1` | about 24 fps | Steady. Faults below. |
| Fast | `VICTORY_PORT_CONSTANTS=1` `VICTORY_REFLECT_SCREEN=1` | 29-30 fps | A white square in the top-left corner, light that flickers in some starts, wrong skin and hair more often. |
| Ray tracing | Stable plus `VICTORY_BVH_BUILDER=1` | 11-14 fps | Rooms lit by the title's own rays, but dark shapes move over walls. Not for play. |

`VICTORY_OUTPUT=2560x1440` makes the title render for a smaller output: about
15 % faster, a larger HUD.

## Known faults

| Fault | How often | What is known |
|---|---|---|
| Long hair missing, or a head with no face | about one start in four | Decided while the world loads. Not fixed. |
| Square patches of wrong skin | some starts | Likely stale blocks of a texture. Not fixed. |
| Dark rooms with a blinding window, grain and blocks on dark walls | always, where little direct light reaches | The stand-in for ray-traced light works from what is on screen. |
| The figure's shadow is a loose blob | always, ray tracing off | The title traces its characters' shadows. |
| A bright blob in the top-left corner, light changing by the frame | the start facing the living room | Milder than in Fast mode; cause open. |
| Ghost images on thin things (palm fronds) | outdoors | The title builds its picture over several frames. |
| The first eleven seconds in the world are slow | every start | 15-18 fps while the place streams in. |

## What this fork changed

On top of the base, each behind a switch that says in its doc comment where it
came from:

| Change | Switch | Effect measured here |
|---|---|---|
| Reflections held right after `CS_ResolveScreenSpace` | `VICTORY_REFLECT_SCREEN=1` | The floor's white glaze gone; the phone's screen readable for the first time. After Trexies' Whiplash patch. |
| Title memory read by address through 2 GiB windows | with `VICTORY_BVH_BUILDER=1` | Ray tracing from 8.8 to 15-19 fps (with port 1 on). |
| Small title buffers kept in the card's memory | `VICTORY_CARD_BUFFERS_MIN_KB` (64 on cards of 20 GiB and more) | Part of 26.7 to 29.4 fps. |
| Write-combined memory left on the card, only changed words sent up | `VICTORY_SKIP_WC_BACK`, `VICTORY_WC_JOIN_BYTES` | 60-77 MB fetched a frame down to 2 MB with ray tracing on. |
| A bake draw's own textures uploaded before it | `VICTORY_BAKE_SOURCES` | No effect seen in the house (nothing was waiting). After fxceless #1264. |
| Compute passes wait for their pipeline at first use | `VICTORY_COMPUTE_WAIT_MS` | No effect with pipelines made ahead. After avatarmanz #440. |
| Instance nodes answered as the hardware does | none | No effect seen yet. After fxceless `9a9091d1`. |

Findings that cost time and are worth knowing before repeating them:

- **Renderer port 1 (`VICTORY_PORT_CONSTANTS=1`) breaks the picture on this
  stack.** Not the order of its arguments (`VICTORY_PORT_CONSTANTS_ORDER`),
  not the delay before the card reads, not huge pages: the faults stay in all
  three. The sharing of that heap between the PC and the card is what fails.
- The title's own render scale stays at 2/3 for a whole run: it is not what
  makes the picture change.
- Precompiling everything changes the start-up, not the picture.
- Legs differ by several milliseconds, and the scripted start lands in one of
  two views (about 6,500 or 9,000 draws a frame). Compare legs of one view, and
  judge hair and flicker over at least four starts.

## Measuring a change

A change without a measurement is not done. What has worked:

- **One scripted start, labelled** (`linux/leg.sh`). Same keys, same seed,
  captures at fixed seconds in the world, frame times from the layer's own
  five-second lines, and a page of all legs (`linux/leg-index.py`).
- **A blink measure** (`linux/flicker.py`). Twelve captures in quick succession while nothing
  moves; the mean difference between neighbours is about 1 for a still picture
  and has been as high as 32. Flicker does not show in single screenshots.
- **A short recording** of every leg, for the same reason.
- **Where a frame's time goes on the card**: a diagnostics build with
  `VICTORY_GPU_TIMING=1 VICTORY_D3D12_DRED=1 VICTORY_SHADER_NAMES=1`.
- **Which pass wrote what**: `VICTORY_CAPTURE=<folder> VICTORY_DUMP_TARGETS=1
  VICTORY_CAPTURE_FRAMES=<n>,<n+1>,...` saves every target of those frames.
- **A word of the title's memory over time**: `VICTORY_IMAGE_WATCH=<name>=<offset>:<bytes>`.
- **The player's own route**: `VICTORY_KEYS_RECORD=<file>` writes the keys held
  in the form `VICTORY_KEYS` plays back.

## For agents

Read this section before changing anything.

**Rules**

1. No game files, assets, keys or captures of the title in the repository, in
   a commit or in an export. `cargo xtask policy` checks it.
2. One game instance at a time. Never edit a script while a run is using it.
3. Measure before and after, in the same view, over several starts. Say which
   build, which switches and which scene a number comes from.
4. One change per commit. The message says what it fixes, the measurement that
   shows it, and the run it came from. Cherry-picks keep their author and source.
5. Everything written here is in English.
6. Do not push, open pull requests or comment upstream without being told to.
7. Never delete a shader or pipeline cache to make room.

**Conventions**

- A new behaviour goes behind a `VICTORY_*` switch with a doc comment that says
  what it does, its default, and the run that measured it. `cargo xtask`
  checks that every switch a Markdown file names is read by the code.
- A switch that changes shader translation must be part of the shader cache's
  key (`crates/victory-gpu/src/shader_cache.rs`); translated shaders are cached
  and a stale cache hides a change.
- State what was verified and what is inferred. "No effect seen" and "not
  tested" are different findings; write the one that is true.

**Where to start**

| Question | Look at |
|---|---|
| Why is a frame slow? | the layer's frame line, then `VICTORY_GPU_TIMING` |
| Why does a surface look wrong? | dump the frame's targets, find the first wrong one, then the pass that wrote it |
| Why does something differ between starts? | count over four starts or more before believing a fix |
| What does the title itself do? | read its code (Ghidra); `crates/victory-loader/src/resolution.rs` shows the form such notes take |

## Where things are

| Path | What |
|---|---|
| `apps/victory/` | The command line: `run`, `shaders`, `warm`, `replay` |
| `crates/victory-gpu/` | The Direct3D 12 side: lists, copies of title memory, textures, pipelines |
| `crates/victory-gpu/src/ambient.rs` | The stand-in for ray-traced light, and the reflection holds |
| `crates/victory-gpu/src/memory.rs` | Title memory read by address (ray tracing) |
| `crates/victory-shader/` | Translation of the console's shader code |
| `crates/victory-hle/` | The console's system libraries, input, memory |
| `crates/victory-loader/` | Loading the title, patches to its code |
| `docs/findings.md` | What the base fork learned: faults found, what was measured and dropped |
| `scripts/play.ps1` | The base fork's play settings (Windows launcher) |
| `docs/fork-ray-tracing.md` | How ray tracing and its stand-in work |
| `docs/notes/NOTES.md` | The running log: every measurement, also the ones that led nowhere |
| `linux/` | The launcher and the measuring scripts as used (paths are tokens) |
| `tools/userptr-thp/` | The preloaded helper that collapses renderer port 1's heap into huge pages |
| `docs/README-avatarmanz.md` | The base fork's README, as it was |

## Credits

VIctory by Marvisak. The base and the renderer ports: avatarmanz. Changes and
ideas taken from fxceless, Belobug, kroeeee, Trexies, cool_fox (codec-d), gleaf,
klbf, triippywhale, DexNom and others, each named where it is used. Part of
this work was done with AI coding agents; every claim above comes from a run
that was looked at.
