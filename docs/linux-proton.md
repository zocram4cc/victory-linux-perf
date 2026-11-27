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
| GPU: the ray-tracing builder is its 52 programs, not any program that spills its user data | with ray tracing off, every compute program whose user data spills was dropped as a builder stage; now the builder is the 52 programs of `XboxBvhBuilder.dll` (by hash, build 1.0.2.1), and other spilled programs run (logged once each) |
| Linux: Proton fork at 5fc510e2 | its Wine imports section memory as dma-bufs (udmabuf): `VICTORY_PORT_CONSTANTS` needs it (see below) |
| FidelityFX: under Wine FSR 4.1.2 only when its code loads | Proton's builtin `amdxc64` makes AMD's SDK offer FSR 4.1.2 although its code (`amdxcffx64.dll`) is missing; the context made on it upscaled wrongly (doubled, smeared outlines). Without the DLL the newest FSR 3.1 is used |
| GPU: raw twins of buffer views off 16 bytes begin at the view's own dword (lloyd's change from avatarmanz's fork, #437; views beginning inside a dword stay as before; `VICTORY_TWIN_EXACT=0` turns it off) | under D3D12's 16-byte rule such twins were null views and shaders read zeros (about 370 a frame in the jail): trees and palms dropped out for single frames (the intro's title shot) and the print on the back of Lucia's jumpsuit was missing. Known issue: in about one run in five the diner waitress's face shows coloured speckles; the cause is not found yet |

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

Measured in the jail scene with our Proton build, one run each (2026-11-26/27):

| Settings | Frame | Notes |
|---|---|---|
| the fork's old `linux` branch | about 384 ms | |
| Belobug's defaults | 116-127 ms | renders correctly |
| `scripts\play.ps1`'s settings + the five renderer patches (`VICTORY_PORT_CONSTANTS`, `_DESCRIPTORS`, `_CBV`, `_GUARD`, `_JOURNAL`) | 78-98 ms | needs our Proton build at 5fc510e2 or later |
| the same + PC FSR 3.1 (`VICTORY_FSR_4K=performance`, below) | 72-102 ms | 0.4 GB less video memory |

- **`VICTORY_PORT_CONSTANTS` with any other Proton build turns every frame
  black.** It opens the title's 185 MB graphics region as a GPU heap over a
  second view of its section (`OpenExistingHeapFromAddress1`). Stock Wine
  cannot import that memory, and stock vkd3d-proton then silently gives the
  heap other memory: the card reads constants the title never wrote. Our
  Proton build imports section memory as dma-bufs (and fails a heap whose
  import fails, so the tree keeps copies instead of drawing black). The other
  four patches and all of `play.ps1`'s settings render with any Proton build.
- `VICTORY_THIN` (eight threads of ours translating the title's lists) is
  most of the gain: without it 133-152 ms.
- Known issues: at normal priority the game's ~150 busy threads made the
  desktop's audio crackle (`scripts/play-linux.sh` runs it at nice 10, at the
  same frame times); the game's own sound is still choppy at times (under
  investigation); frame pacing is uneven; in about one run in five the diner
  waitress's face shows coloured speckles (since the raw-twin change above;
  `VICTORY_TWIN_EXACT=0` turns it off); video memory peaks near 15 GB
  (Belobug's texture budget fills up to nine tenths of what the system grants,
  plus up to 4 GiB of card copies, `VICTORY_MEMORY_CARD_MB`).
- The pipeline-library fix above is needed for pipelines to be kept between
  runs under Proton at all.
- Lighting: ray tracing is off by Belobug's default (`VICTORY_BVH_BUILDER`
  turns the title's builder on, as `play.ps1 -RayTracing` does; here it costs
  50-80 ms of GPU time a frame). With the builder off the title still traces
  into an empty structure: eyes glow blue-white (their reflections find the
  sky) and rooms go grey. We play the title's own path for no ray tracing
  instead (`VICTORY_RT_FALLBACK_NO_BVH=1`: natural eyes, the console's warmth)
  with Belobug's estimate of the missing bounce light at 0.35 and its crease
  shading off (`VICTORY_AMBIENT=0.35 VICTORY_AMBIENT_SHADE=0`). Cell scene:
  brightness 67 and red/blue 1.74 against the console's 68-71 and 1.86-2.12
  (`play.ps1`'s `VICTORY_AMBIENT=0.8` without the fallback: 77 and 1.55).

### PC FSR

Belobug's bridge (from DexNom) serves the title's FidelityFX calls with AMD's
PC upscaler: put `amd_fidelityfx_upscaler_dx12.dll` and
`amd_fidelityfx_loader_dx12.dll` from the
[FidelityFX SDK 2.4.0 `signedbin`](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/tree/v2.4.0/Kits/FidelityFX/signedbin)
in a folder, then `VICTORY_FIDELITYFX_DIR='Z:\that\folder'` and
`VICTORY_FSR_4K=quality|balanced|performance` (2560x1440, 2258x1270 or
1920x1080 drawn, 3840x2160 out). On RDNA2 and older AMD cards, and on other
vendors, that is FSR 3.1. FSR 4 needs an RDNA3 or RDNA4 card and AMD's FSR 4
code in the prefix: `PROTON_FSR4_UPGRADE=1` makes our Proton build download
it (GE-Proton's option); without it FSR 3.1 is used.

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
(`VKD3D_HOST_IMPORT_STRICT=0` restores that). Its Wine imports section memory
(file mappings) as dma-bufs through `/dev/udmabuf`, which the renderer port's
in-place heap needs, and refuses host-pointer imports of other memory (amdgpu
re-checks those at every submission and rejected them; VIctory keeps copies
there): a deliberate, VIctory-specific choice, so other programs that import
private memory fall back or fail under this build. `/dev/udmabuf` must be
readable and writable for the user (on most distributions the `kvm` group).
Its vkd3d-proton also takes `VKD3D_VIEW_HEAP_DESCRIPTORS`, the most views a
heap may hold when `VK_EXT_descriptor_heap` is not used (stock: 1,000,000,
below the 4,194,304 VIctory's raw twins need). `scripts/play-linux.sh` turns
the extension off (`VKD3D_DISABLE_EXTENSIONS=VK_EXT_descriptor_heap`) and asks
for 4,194,304 when the build has the switch: through the extension, RADV
(Mesa 26.3) compiles typed UAV loads and stores without their coherence bits,
and shaders whose waves share data through them read stale values (the face
network that animates Lucia went wrong this way; her mouth hung open).
More fixes for VIctory under Wine go there as they come. It is not supported
by GloriousEggroll: report problems with it here, not to GE-Proton.

Building it takes Docker (or Podman), about 40 GB of disk and, the first
time, about 40 minutes on 16 threads:

```sh
git submodule update --init --recursive proton-ge   # about 8 GB of sources
cd proton-ge
./patches/protonprep-valve-staging.sh &> patchlog.txt   # grep -i fail patchlog.txt
cd .. && mkdir proton-build && cd proton-build
../proton-ge/configure.sh --build-name=GE-Proton11-14-victory5
make ENABLE_CCACHE=1 redist   # ccache (in ~/.ccache) speeds up later full builds
tar -xf GE-Proton11-14-victory5.tar.gz -C ~/.steam/steam/compatibilitytools.d/
```

After pulling a newer `proton-ge`, `make ENABLE_CCACHE=1 redist` again in the
same folder rebuilds what changed (about two minutes with nothing changed).

A prefix holds the DLLs of the Proton that last set it up, and
`runinprefix` does not refresh them: after switching Proton, run something
once with `run` (`"$PROTON" run cmd /c exit`) before using `runinprefix`.

## Running

Use a Proton prefix of its own and Proton's vkd3d-proton (`d3d12` native).
`scripts/play-linux.sh <game folder>` does all of the below (the fast set,
the lighting, the face fix with our Proton build, the shader store once per
build, the game at nice 10; `--safe` for Belobug's defaults, `--fsr` for the
PC upscaler). By hand:

```sh
PROTON=~/.steam/steam/compatibilitytools.d/GE-Proton11-14-victory5/proton
export STEAM_COMPAT_DATA_PATH=~/victory-prefix STEAM_COMPAT_CLIENT_INSTALL_PATH=~/.steam/steam
export WINEDLLOVERRIDES="d3d12=n;d3d12core=n;dxgi=n;d3d11=n;d3d10core=n;d3d9=n"
export VICTORY_GAME_DIR='Z:\path\to\GTA6.Xbox.v1.0.2.1'
export VICTORY_NO_WATCHDOG=1
# Lighting (see "Lighting" above).
export VICTORY_RT_FALLBACK_NO_BVH=1 VICTORY_AMBIENT=0.35 VICTORY_AMBIENT_SHADE=0
# Faces (needs our Proton build, see above): vkd3d-proton's descriptor heap
# off, and a view heap large enough for VIctory's raw twins.
export VKD3D_DISABLE_EXTENSIONS=VK_EXT_descriptor_heap VKD3D_VIEW_HEAP_DESCRIPTORS=4194304
# Optional: go on past the instance-list pool stop in the intro (things go
# missing from then on).
# export VICTORY_NO_POOL_TRAP=1

# The fast set (scripts\play.ps1's settings, the five renderer patches,
# VICTORY_WC_NO_BACK: shaders' writes into write-combined title memory stay on
# the card, 66 -> 57 ms a frame on an RX 6800 XT, and
# VICTORY_CARD_BUFFERS_MIN_MB=1: shader-written copies of title buffers from
# 1 MiB up live on the card, not only from 4 MiB, 57.7 -> 48.6 ms;
# VICTORY_PORT_CONSTANTS needs our Proton build, see above). Leave it out for
# Belobug's defaults.
export VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1 \
  VICTORY_JOIN_COPIES=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2 \
  VICTORY_INDIRECT_BATCH=2 VICTORY_MAP_THREADS=1 VICTORY_CARD_AWAKE=1 VICTORY_SHOW_AFTER=1 \
  VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1 \
  VICTORY_PORT_CONSTANTS=1 VICTORY_PORT_DESCRIPTORS=1 VICTORY_PORT_CBV=1 VICTORY_PORT_GUARD=1 \
  VICTORY_PORT_JOURNAL=1 VICTORY_WC_NO_BACK=1 VICTORY_CARD_BUFFERS_MIN_MB=1
# Optional, PC FSR (see "PC FSR" above):
# export VICTORY_FIDELITYFX_DIR='Z:\path\to\fidelityfx' VICTORY_FSR_4K=performance

# Once, and again after updating this branch: translate every shader into the
# shader store (minutes; it goes on where it was stopped).
"$PROTON" runinprefix victory.exe shaders "$VICTORY_GAME_DIR"

prlimit --nice=10:10 -- nice -n 10 "$PROTON" runinprefix victory.exe run "$VICTORY_GAME_DIR\game_scarlett_master.exe" --execute
```

Paths given to `victory.exe` are Windows paths (`Z:` is `/`).
`VICTORY_CACHE_DIR` picks another cache folder (or `victory.toml`, see
`victory.example.toml`). The first run makes the pipelines it needs as it
goes and keeps them in the pipeline library; `victory.exe warm --used` makes
those of earlier runs ahead, after a driver or build change.
