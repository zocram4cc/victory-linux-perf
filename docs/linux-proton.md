# VIctory on Linux with Proton

Branch `linux-v2` of [crashbashash/VIctory-proton](https://github.com/crashbashash/VIctory-proton)
is [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)
(branch `a-publier`, the fxceless → avatarmanz → Belobug family; this README's
tree) plus what it takes to run it under Proton. Every commit on top of
Belobug's is listed below; on Windows the tree behaves as Belobug's (what is
added is either a Proton fix that Windows takes too, or off unless switched on).

Tested on: AMD RX 6800 XT (RDNA2), Mesa RADV, CachyOS, our Proton build (see
"Our Proton build" below), Grand Theft Auto VI 1.0.2.1 (Xbox Series).

## What the branch adds

| Commit | Why |
|---|---|
| GPU: ask for an empty pipeline library with a null blob | the `windows` crate passes an empty slice's pointer, which is never null, and vkd3d-proton refuses a non-null pointer with no bytes (`E_INVALIDARG`): under Proton there was no pipeline library, so every run made every pipeline again in game (the first run stayed black for a long time while they compiled) |
| Loader: opt-in title hooks (`VICTORY_NO_WATCHDOG`, `VICTORY_NO_POOL_TRAP`, `VICTORY_RT_FALLBACK`, `VICTORY_RT_FALLBACK_NO_BVH`) | byte-checked, build 1.0.2.1 only, each off unless set. The title's watchdog ends the process when its main thread has not advanced for about 30 s, which a long load on a slow host exceeds; the instance-list pool stop of upstream issue #698 (returns NULL instead: things go missing from then on); the title's own non-ray-traced fallback lighting (its "is the top-level structure there?" check says no), optionally with the acceleration-structure builds skipped too. The RT fallback hooks are not yet measured in game |
| Input: live keys from a file (`VICTORY_KEYS_FILE`, diagnostics builds) | lines appended to a file while the game runs are pressed (`<key>[+<key>...][:<ms>]`), on top of `VICTORY_KEYS` and a connected controller: for hands-free and scripted test runs |
| Linux: our Proton build as a submodule, `proton-ge`; this file | see below |

What earlier Wine fixes of the fork's `linux` branch became here:

| `linux` commit | Here |
|---|---|
| indirect argument clamp built with Wine's compiler; `WSAConnectByList`/`GetAddrInfoExA`; free memory is not watched | Belobug's tree has its own equivalents (it renders under Proton without them) |
| castable formats within what vkd3d-proton has, at most nine a texture | not needed: this tree makes no castable format lists (a view in another format of the texture's family takes the texture's format) |
| DirectStorage reads through the fault path | already handled: the destination is held open for the kernel's write (`kernel_writing`), and a refused read is retried |
| a 64-bit image atomic gets a handle typed U64 | already in the translator (handles are keyed by both property words; 64-bit atomics take a U64 handle) |
| a root constant's dword 4 is a value (#2699) | not needed: this tree reads root constants by dword and has no root-view-offset dword |
| skipped pipelines in every build, the Wine skip list, the pre-build's crash record | not needed with our Proton build (its vkd3d-proton makes all of the pack's pipelines). With a stock GE-Proton, a diagnostics build's `VICTORY_SKIP_PIPELINES` can leave crashing shaders out |
| `VICTORY_OUTPUT_1080P`, `VICTORY_RENDER_SCALE_FULL` | not ported: this tree's `VICTORY_OUTPUT` and `VICTORY_RENDER_SCALE` do the same |
| performance work on our own tree (merged views, guard runs, loose windows, indirect clamp barriers, heap read lock) | not ported: this tree's own architecture replaces it; each is to be measured again here before being carried over |

## Belobug's switches under Proton

Measured in the jail scene with our Proton build, one run each (2026-11-26):

- **Belobug's defaults** (no launcher settings, no renderer patches) render
  correctly: 116-127 ms a frame, where the fork's `linux` branch took about
  384 ms in the same scene. Frame pacing in the intro, the loading cards and
  the world is uneven, and characters' mouths do not move in dialogue.
- **`scripts\play.ps1`'s settings together with the five renderer patches
  (`VICTORY_PORT_CONSTANTS`, `_DESCRIPTORS`, `_CBV`, `_GUARD`, `_JOURNAL`)
  turn every frame black** under Proton (the card does next to no work; port 1
  falls back to copies, "title memory not in place"). Under investigation:
  which setting or patch does it is not known yet. Until then run with the
  defaults.
- The pipeline-library fix above is needed for pipelines to be kept between
  runs under Proton at all.
- Ray tracing is off by Belobug's default (`VICTORY_BVH_BUILDER` turns the
  title's builder on, as `play.ps1 -RayTracing` does); not yet tried here.

## Building

On Linux, cross-compile with [cargo-xwin](https://github.com/rust-cross/cargo-xwin)
(the toolchain is pinned in `rust-toolchain.toml`):

```sh
cargo xwin build --locked --profile diag -p victory --features diagnostics --target x86_64-pc-windows-msvc
# target/x86_64-pc-windows-msvc/diag/victory.exe
```

`VICTORY_KEYS` and `VICTORY_KEYS_FILE` exist in diagnostics builds only.

## Our Proton build

`proton-ge` is a submodule: our fork of GE-Proton, branch `victory`
([crashbashash/proton-ge-victory](https://github.com/crashbashash/proton-ge-victory)).
It is GE-Proton11-14 with a newer vkd3d-proton, whose dxil-spirv converts
the 22 compute shaders GE-Proton11-14's crashes on, and it sets
`VICTORY_PROTON`. Its wineserver finds memory views in a tree instead of a
list (CachyOS's patch, Wine bug 60868): VIctory maps tens of thousands, and
every map or unmap walked them all. Its vkd3d-proton fails a heap opened
over existing memory (`OpenExistingHeapFromAddress`, which this tree's
in-place paths use) when the host-memory import fails, where stock
vkd3d-proton gives the heap fresh memory that the title never sees
(`VKD3D_HOST_IMPORT_STRICT=0` restores that). More fixes for VIctory under
Wine go there as they come. It is not supported by
GloriousEggroll: report problems with it here, not to GE-Proton.

Building it takes Docker (or Podman), about 40 GB of disk and, the first
time, about 40 minutes on 16 threads:

```sh
git submodule update --init --recursive proton-ge   # about 8 GB of sources
cd proton-ge
./patches/protonprep-valve-staging.sh &> patchlog.txt   # grep -i fail patchlog.txt
cd .. && mkdir proton-build && cd proton-build
../proton-ge/configure.sh --build-name=GE-Proton11-14-victory3
make ENABLE_CCACHE=1 redist   # ccache (in ~/.ccache) speeds up later full builds
tar -xf GE-Proton11-14-victory3.tar.gz -C ~/.steam/steam/compatibilitytools.d/
```

After pulling a newer `proton-ge`, `make ENABLE_CCACHE=1 redist` again in the
same folder rebuilds what changed (about two minutes with nothing changed).

A prefix holds the DLLs of the Proton that last set it up, and
`runinprefix` does not refresh them: after switching Proton, run something
once with `run` (`"$PROTON" run cmd /c exit`) before using `runinprefix`.

## Running

Use a Proton prefix of its own and Proton's vkd3d-proton (`d3d12` native).
`scripts\play.ps1` is PowerShell for Windows; under Proton set the variables
yourself:

```sh
PROTON=~/.steam/steam/compatibilitytools.d/GE-Proton11-14-victory3/proton
export STEAM_COMPAT_DATA_PATH=~/victory-prefix STEAM_COMPAT_CLIENT_INSTALL_PATH=~/.steam/steam
export WINEDLLOVERRIDES="d3d12=n;d3d12core=n;dxgi=n;d3d11=n;d3d10core=n;d3d9=n"
export VICTORY_GAME_DIR='Z:\path\to\GTA6.Xbox.v1.0.2.1'
export VICTORY_NO_WATCHDOG=1
# Optional: go on past the instance-list pool stop in the intro (things go
# missing from then on).
# export VICTORY_NO_POOL_TRAP=1

# Once, and again after updating this branch: translate every shader into the
# shader store (minutes; it goes on where it was stopped).
"$PROTON" runinprefix victory.exe shaders "$VICTORY_GAME_DIR"

"$PROTON" runinprefix victory.exe run "$VICTORY_GAME_DIR\game_scarlett_master.exe" --execute
```

Paths given to `victory.exe` are Windows paths (`Z:` is `/`).
`VICTORY_CACHE_DIR` picks another cache folder (or `victory.toml`, see
`victory.example.toml`). The first run makes the pipelines it needs as it
goes and keeps them in the pipeline library; `victory.exe warm --used` makes
those of earlier runs ahead, after a driver or build change.
