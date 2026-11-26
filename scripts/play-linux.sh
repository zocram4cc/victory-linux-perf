#!/usr/bin/env bash
# Linux launcher: runs the Windows build under Proton-GE through umu-launcher,
# with the same settings as Play-VIctory.cmd (pareto-residency, Series S).
#
#   VICTORY_GAME_DIR=~/Games/MyDump scripts/play-linux.sh [pareto-residency] [--safe] [--ray-tracing]
#   scripts/play-linux.sh shaders <the shader archives>   # translate every shader ahead of time
#   scripts/play-linux.sh warm --used                     # make pipelines earlier runs drew with
#   scripts/play-linux.sh exe <args...>                   # any other victory.exe command
#
# Every VICTORY_* default below can be overridden from the environment.
# Runner settings (overridable):
#   PROTONPATH      Proton build to use (default: Proton-GE Latest from Steam's compatibilitytools.d)
#   WINEPREFIX      prefix for VIctory alone (default: ~/.local/share/victory/prefix)
#   VICTORY_EXE     victory.exe to run (default: the cross-built release, then a release exe beside this repo)
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Linux path -> path Wine sees (its Z: drive is the Linux root).
winpath() { local p; p="$(realpath -m -- "$1")"; printf 'Z:%s' "${p//\//\\}"; }

command -v umu-run >/dev/null || { echo "umu-run not found: sudo dnf install umu-launcher" >&2; exit 2; }

export PROTONPATH="${PROTONPATH:-$HOME/.local/share/Steam/compatibilitytools.d/Proton-GE Latest}"
export WINEPREFIX="${WINEPREFIX:-$HOME/.local/share/victory/prefix}"
export GAMEID="${GAMEID:-umu-0}"
[[ -d "$PROTONPATH" ]] || { echo "No Proton at $PROTONPATH; set PROTONPATH" >&2; exit 2; }
mkdir -p "$WINEPREFIX"

exe="${VICTORY_EXE:-}"
if [[ -z "$exe" ]]; then
  for c in "$repo/victory-3d-rtx-pareto-residency.exe" \
           "$repo/target/x86_64-pc-windows-msvc/release/victory.exe" \
           "$repo/target/release/victory.exe"; do
    [[ -f "$c" ]] && { exe="$c"; break; }
  done
fi
[[ -n "$exe" && -f "$exe" ]] || {
  echo "No victory.exe. Build it: cargo xwin build --release --locked -p victory --target x86_64-pc-windows-msvc" >&2
  exit 2
}

# Caches and data (saves, settings) live on the Linux side, not inside the
# prefix, so a fresh prefix keeps them.
cache="${VICTORY_CACHE_DIR:-$HOME/.cache/victory/cache-pareto}"
data="${VICTORY_DATA_DIR:-$HOME/.local/share/victory/data}"
mkdir -p "$cache" "$data"
export VICTORY_CACHE_DIR="$(winpath "$cache")"
export VICTORY_DATA_DIR="$(winpath "$data")"
if [[ -n "${VICTORY_CONFIG:-}" ]]; then export VICTORY_CONFIG="$(winpath "$VICTORY_CONFIG")"; fi

# Microsoft's DXC (dxcompiler.dll, dxil.dll) beside the exe: under Wine the
# indirect-draw clamp and card compare only compile with it (no 3D without).
# Copied in from VICTORY_DXC_DIR when missing, so a rebuild keeps working.
dxc_dir="${VICTORY_DXC_DIR:-$HOME/.local/share/victory/dxc}"
for dll in dxcompiler.dll dxil.dll; do
  [[ -f "$(dirname "$exe")/$dll" ]] || { [[ -f "$dxc_dir/$dll" ]] && cp -f "$dxc_dir/$dll" "$(dirname "$exe")/"; } || true
done
[[ -f "$(dirname "$exe")/dxcompiler.dll" ]] || echo "warning: no dxcompiler.dll beside $exe or in $dxc_dir;" \
  "get dxcompiler.dll and dxil.dll from Microsoft's DirectXShaderCompiler release (bin/x64)." >&2

# vkd3d-proton settings from meeyao/victory-8gb-linux (tested on NVIDIA under Proton).
export VKD3D_CONFIG="${VKD3D_CONFIG-descriptor_heap}"
export VKD3D_DISABLE_EXTENSIONS="${VKD3D_DISABLE_EXTENSIONS-VK_NV_raw_access_chains}"
export VKD3D_SHADER_CACHE_PATH="${VKD3D_SHADER_CACHE_PATH:-$HOME/.cache/victory/vkd3d}"
mkdir -p "$VKD3D_SHADER_CACHE_PATH"

cd "$repo"   # victory.toml is looked for in the working directory

# The exe goes in as a Wine path: given a Linux path, Proton starts it through
# its umu.exe wrapper and its output never reaches this terminal. (umu warns
# it cannot find the file; Proton finds it.) Linux paths in the arguments
# work as they are, since the working directory is on Wine's Z: drive.
win_exe="$(winpath "$exe")"

case "${1:-}" in
  shaders|warm) exec umu-run "$win_exe" "$@" ;;
  exe) shift; exec umu-run "$win_exe" "$@" ;;
esac

safe=0
for a in "$@"; do
  case "$a" in
    pareto-residency) ;;
    --safe) safe=1 ;;
    --ray-tracing) export VICTORY_BVH_BUILDER="${VICTORY_BVH_BUILDER:-1}" ;;
    *) echo "Usage: $0 [pareto-residency] [--safe] [--ray-tracing] | shaders ... | warm ... | exe ..." >&2; exit 2 ;;
  esac
done

game="${VICTORY_GAME_DIR:-}"
[[ -n "$game" && -f "$game/game_scarlett_master.exe" ]] || {
  echo "Set VICTORY_GAME_DIR to your own dump folder containing game_scarlett_master.exe." >&2
  exit 2
}

# The same defaults as Play-VIctory.cmd.
d() { [[ -n "${!1:-}" ]] || export "$1=$2"; }
d VICTORY_AMBIENT_AIR 1.5
d VICTORY_TEXTURE_NO_WAIT 1
d VICTORY_RELAY 2
d VICTORY_WIDE_BACK 1
d VICTORY_CARD_REHASH_RUNS 1
d VICTORY_JOIN_COPIES 1
d VICTORY_INDIRECT_WINDOW 1
d VICTORY_RELAY_COPY 1
d VICTORY_CARD_STEPS 2
d VICTORY_INDIRECT_BATCH 2
d VICTORY_MAP_THREADS 1
d VICTORY_CARD_AWAKE 1
d VICTORY_SHOW_AFTER 1
d VICTORY_AMBIENT 0.8
if (( safe )); then
  # scripts/play.ps1 -Safe: without the newer, faster ways.
  d VICTORY_UNDER_WIDE 0
  d VICTORY_LAND_ECHO 0
else
  d VICTORY_THIN 1
  d VICTORY_JOIN_PARTS 8
  d VICTORY_UPLOAD_AHEAD 1
fi
d VICTORY_LEVELS_ON_DEMAND 1
d VICTORY_CARD_BUFFERS_MIN_KB 64
# Both faces: under Proton character heads and hair are culled away otherwise.
d VICTORY_CULL_NONE 1
[[ -n "${VICTORY_LOG:-}${RUST_LOG:-}" ]] || export VICTORY_LOG=warn

exec umu-run "$win_exe" run "$(winpath "$game/game_scarlett_master.exe")" --console series-s --execute
