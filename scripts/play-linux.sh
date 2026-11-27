#!/usr/bin/env bash
# Starts the title under our Proton build with the settings this branch is played with on Linux: the counterpart of
# scripts/play.ps1 (docs/linux-proton.md, "Running").
#   scripts/play-linux.sh <game folder> [--safe] [--fsr quality|balanced|performance <fidelityfx folder>]
#   --safe   Belobug's defaults: none of play.ps1's settings or the renderer patches (the lighting below stays)
#   --fsr    AMD's PC upscaler from <fidelityfx folder> (FSR 3.1, or FSR 4 on RDNA3/4 with PROTON_FSR4_UPGRADE=1)
# Environment: PROTON (default ~/.steam/steam/compatibilitytools.d/GE-Proton11-14-victory5/proton), STEAM_COMPAT_DATA_PATH
# (default ~/victory-prefix), VICTORY_EXE (default target/x86_64-pc-windows-msvc/diag/victory.exe in this checkout).
#
# The game runs at nice 10 and cannot raise a thread above it (prlimit): at normal or raised priority its ~150 busy
# threads starved the desktop's audio (music and the game's own sound crackled); at nice 10 frame times were the same.
set -euo pipefail
GAME=${1:?usage: play-linux.sh <game folder> [--safe] [--fsr <preset> <fidelityfx folder>]}; shift
SAFE=0; FSR=""; FFX=""
while [ $# -gt 0 ]; do
  case "$1" in
    --safe) SAFE=1 ;;
    --fsr) FSR=${2:?preset}; FFX=${3:?fidelityfx folder}; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
HERE=$(cd "$(dirname "$0")/.." && pwd)
PROTON=${PROTON:-$HOME/.steam/steam/compatibilitytools.d/GE-Proton11-14-victory5/proton}
EXE=${VICTORY_EXE:-$HERE/target/x86_64-pc-windows-msvc/diag/victory.exe}
win() { printf 'Z:%s' "$(realpath "$1")" | sed 's,/,\\,g'; }
[ -x "$PROTON" ] || { echo "no Proton at $PROTON (set PROTON)" >&2; exit 1; }
[ -f "$EXE" ] || { echo "no $EXE: build it first (docs/linux-proton.md)" >&2; exit 1; }
[ -f "$GAME/game_scarlett_master.exe" ] || { echo "no game_scarlett_master.exe in $GAME" >&2; exit 1; }
export STEAM_COMPAT_DATA_PATH=${STEAM_COMPAT_DATA_PATH:-$HOME/victory-prefix}
export STEAM_COMPAT_CLIENT_INSTALL_PATH=${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.steam/steam}
mkdir -p "$STEAM_COMPAT_DATA_PATH"
export WINEDLLOVERRIDES="d3d12=n;d3d12core=n;dxgi=n;d3d11=n;d3d10core=n;d3d9=n"
VICTORY_GAME_DIR=$(win "$GAME"); export VICTORY_GAME_DIR
export VICTORY_NO_WATCHDOG=1
# Lighting: the title's own path for no ray tracing (without it the title traces into an empty structure: eyes glow
# blue-white, rooms go grey), with Belobug's stand-in for the missing bounce light at 0.35, no crease shading.
export VICTORY_RT_FALLBACK_NO_BVH=1 VICTORY_AMBIENT=0.35 VICTORY_AMBIENT_SHADE=0
# Faces (and any shader whose waves share data through typed views): through vkd3d-proton's VK_EXT_descriptor_heap,
# RADV (Mesa 26.3) compiles typed UAV loads and stores without their coherence bits and waves read stale values
# (Lucia's mouth hung open and misshapen). Without the extension stock vkd3d-proton caps view heaps at 1,000,000,
# below the 4,194,304 VIctory's raw twins need; our Proton's vkd3d-proton takes VKD3D_VIEW_HEAP_DESCRIPTORS. Only
# with that build is the extension turned off.
if grep -qa VKD3D_VIEW_HEAP_DESCRIPTORS "$(dirname "$PROTON")/files/lib/wine/vkd3d-proton/x86_64-windows/d3d12core.dll" 2> /dev/null; then
  export VKD3D_DISABLE_EXTENSIONS=VK_EXT_descriptor_heap VKD3D_VIEW_HEAP_DESCRIPTORS=4194304
fi
if [ "$SAFE" = 0 ]; then
  export VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1 \
    VICTORY_JOIN_COPIES=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2 \
    VICTORY_INDIRECT_BATCH=2 VICTORY_MAP_THREADS=1 VICTORY_CARD_AWAKE=1 VICTORY_SHOW_AFTER=1 \
    VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1 \
    VICTORY_PORT_CONSTANTS=1 VICTORY_PORT_DESCRIPTORS=1 VICTORY_PORT_CBV=1 VICTORY_PORT_GUARD=1 \
    VICTORY_PORT_JOURNAL=1
fi
if [ -n "$FSR" ]; then
  VICTORY_FIDELITYFX_DIR=$(win "$FFX"); export VICTORY_FIDELITYFX_DIR VICTORY_FSR_4K=$FSR
fi
# The prefix holds the DLLs of the Proton that last ran `proton run`; refresh them when that was another build.
grep -qs "$(basename "$(dirname "$PROTON")")" "$STEAM_COMPAT_DATA_PATH/config_info" || "$PROTON" run cmd /c exit
# Translate the shader store once per build of victory.exe (minutes; it goes on where it was stopped).
STAMP="$STEAM_COMPAT_DATA_PATH/victory-shaders.stamp"; SUM=$(sha256sum "$EXE" | cut -d' ' -f1)
if [ "$(cat "$STAMP" 2>/dev/null)" != "$SUM" ]; then
  prlimit --nice=10:10 -- nice -n $((10 - $(nice))) "$PROTON" runinprefix "$(win "$EXE")" shaders "$VICTORY_GAME_DIR"
  echo "$SUM" > "$STAMP"
fi
exec prlimit --nice=10:10 -- nice -n $((10 - $(nice))) "$PROTON" runinprefix "$(win "$EXE")" \
  run "$VICTORY_GAME_DIR\\game_scarlett_master.exe" --execute
