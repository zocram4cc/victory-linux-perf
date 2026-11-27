#!/usr/bin/env bash
# Runs the windows-gnu build of victory.exe under Proton on Linux.
#
#   cargo build --profile diag -p victory --features diagnostics --target x86_64-pc-windows-gnu
#   ./run-victory-proton.sh /path/to/GameFolder [--seconds N] [--release] [--capture]
#                           [--console series-x|series-s]
#
#   --seconds N  stop the run after N seconds (default: run until the title exits)
#   --release    run target/.../release/victory.exe instead of the diag build
#   --capture    save presented frames (diag build) to <runtime>/frames
#
# Environment overrides:
#   PROTON_DIR       Proton install to use (default: Proton - Experimental)
#   VICTORY_RUNTIME  folder for the Wine prefix, cache, saves, logs and frames; it must be
#                    outside the repository (default: ~/Documents/victory-runtime)
#   VICTORY_VKD3D_DIR  folder holding another d3d12.dll + d3d12core.dll (for example a tracing
#                    build for VKD3D_CONFIG=breadcrumbs); unset, the prefix gets Proton's own again
#   VICTORY_EXE      run this victory.exe instead of target/x86_64-pc-windows-gnu/<profile>/victory.exe
#                    (for example a build made on aiserver: ~/Documents/victory-runtime/builds/<name>/victory.exe)
#   MANGOHUD         MangoHud overlay in the game window, on by default; MANGOHUD=0 turns it off
# VICTORY_* debugging variables (VICTORY_LOG, VICTORY_KEYS, ...) pass through unchanged.
# VICTORY_CONTROL_PORT=<port> starts the control server on 127.0.0.1 for the victory-control window
# (README, "Control panel").
# The title's videos are cut to one frame by default (diag build; VICTORY_SKIP_MOVIES=0 plays
# them), and VICTORY_KEYS="20:Enter,26:Enter,...,110:Enter" (Enter every 6 s) gets from the start
# screen into the world unattended, in about 75 s on a Ryzen 7800X3D.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
usage="usage: $0 GAME_FOLDER [--seconds N] [--release] [--capture] [--console series-x|series-s]"
game="${1:?$usage}"
shift
game="$(realpath "$game")"

seconds=0 profile=diag capture=0 console=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --seconds) seconds="${2:?$usage}"; shift 2 ;;
        --release) profile=release; shift ;;
        --capture) capture=1; shift ;;
        --console) console="${2:?$usage}"; shift 2 ;;
        *) echo "unknown option $1" >&2; echo "$usage" >&2; exit 2 ;;
    esac
done
[[ "$seconds" =~ ^[0-9]+$ ]] || { echo "--seconds takes a whole number" >&2; exit 2; }

steam_root="${STEAM_ROOT:-$HOME/.local/share/Steam}"
proton_dir="${PROTON_DIR:-$steam_root/steamapps/common/Proton - Experimental}"
runtime="$(realpath -m "${VICTORY_RUNTIME:-$HOME/Documents/victory-runtime}")"
victory="${VICTORY_EXE:-$repo/target/x86_64-pc-windows-gnu/$profile/victory.exe}"
game_exe="$game/game_scarlett_master.exe"
wine="$proton_dir/files/bin/wine"
wineserver="$proton_dir/files/bin/wineserver"

case "$runtime/" in
    "$repo"/*) echo "VICTORY_RUNTIME must be outside the repository" >&2; exit 2 ;;
esac
for f in "$victory" "$game_exe" "$proton_dir/proton" "$wine" "$wineserver"; do
    [[ -f "$f" ]] || { echo "Missing required file $f" >&2; exit 1; }
done

# Linux path -> Wine Z: path
winpath() { printf 'Z:%s' "${1//\//\\}"; }

prefix="$runtime/prefix"
mkdir -p "$prefix" "$runtime/cache" "$runtime/data" "$runtime/logs" "$runtime/vkd3d-cache"
export STEAM_COMPAT_DATA_PATH="$prefix"
export STEAM_COMPAT_CLIENT_INSTALL_PATH="$steam_root"

# First run: let Proton build the prefix (installs vkd3d-proton and DXVK into it).
if [[ ! -f "$prefix/pfx/system.reg" ]]; then
    "$proton_dir/proton" run cmd /c exit || true
fi

# vkd3d-proton: Proton's own unless another build is given (undoing an earlier run's copy)
vkd3d_dir="$(realpath -m "${VICTORY_VKD3D_DIR:-$proton_dir/files/lib/wine/vkd3d-proton/x86_64-windows}")"
for f in d3d12.dll d3d12core.dll; do
    [[ -f "$vkd3d_dir/$f" ]] || { echo "Missing required file $vkd3d_dir/$f" >&2; exit 1; }
    cp --remove-destination "$vkd3d_dir/$f" "$prefix/pfx/drive_c/windows/system32/$f"
done
echo "vkd3d-proton from $vkd3d_dir"

VICTORY_GAME_DIR="$(winpath "$game")"
VICTORY_CACHE_DIR="$(winpath "$runtime/cache")"
VICTORY_DATA_DIR="$(winpath "$runtime/data")"
export VICTORY_GAME_DIR VICTORY_CACHE_DIR VICTORY_DATA_DIR
if [[ "$capture" -eq 1 ]]; then
    mkdir -p "$runtime/frames"
    VICTORY_CAPTURE="$(winpath "$runtime/frames")"
    export VICTORY_CAPTURE
fi
export VKD3D_SHADER_CACHE_PATH="$runtime/vkd3d-cache"

# Run through Proton's wine directly: `proton run` detaches the console and hides the output.
export WINEPREFIX="$prefix/pfx"
export WINEDLLOVERRIDES="d3d12,d3d12core,dxgi,d3d11,d3d10core,d3d9=n,b${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
export WINEDEBUG="${WINEDEBUG:--all}"
export MANGOHUD="${MANGOHUD:-1}"
export VICTORY_SKIP_MOVIES="${VICTORY_SKIP_MOVIES:-1}"

args=(run "$(winpath "$game_exe")" --execute)
[[ -n "$console" ]] && args+=(--console "$console")

log="$runtime/logs/victory.log"
[[ -f "$log" ]] && mv -f "$log" "$runtime/logs/victory.prev.log"
# The control panel's log pane tails this file when the run ends or crashes (a Linux path: the
# token file that names it is read by a Linux program).
export VICTORY_LOG_FILE="$log"
echo "Starting VIctory under $(basename "$proton_dir"); log in $log"

cd "$runtime"
"$wine" "$victory" "${args[@]}" >"$log" 2>&1 &
wine_pid=$!
if [[ "$seconds" -gt 0 ]]; then
    ( sleep "$seconds"; "$wineserver" -k ) >/dev/null 2>&1 &
    timer_pid=$!
    trap 'kill "$timer_pid" 2>/dev/null || true' EXIT
fi
code=0
wait "$wine_pid" || code=$?
tail -n 5 "$log"
[[ $code -eq 0 ]] || echo "victory.exe exited with $code. See $log and $runtime/data" >&2
exit $code
