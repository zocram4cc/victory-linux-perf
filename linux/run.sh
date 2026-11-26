#!/usr/bin/env bash
# Runs the cross-built victory.exe under Proton (through umu-run), which
# supplies Direct3D 12 by way of vkd3d-proton.
#
# Usage:
#   linux/run.sh                 start the title (run <exe> --execute)
#   linux/run.sh <victory args>  any other command, e.g. `config`, or
#                                `run 'Z:\path\to\title.exe'` for a dry run
#
# Settings come from the environment or from linux/local.env (git-ignored):
#   GAME_DIR   Linux path of your dump of the title (required to start it)
#   GAME_EXE   the title's executable inside it (default: the only *.exe)
#   PROFILE    release (default) or diag
#   PROTONPATH the Proton build to use. Its vkd3d-proton must be 3.2.0 or
#              newer: 3.1.0 turns one of the title's pixel shaders into
#              invalid SPIR-V, and RADV then crashes creating the pipeline
#   D3DCOMPILER a Windows 10 era d3dcompiler_47.dll (see below)
#   STATE_DIR prefix, cache, saves and logs (default ~/.local/share/victory-linux)
# VICTORY_* switches (README) are passed through unchanged.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$ROOT/linux/local.env" ]] && source "$ROOT/linux/local.env"

PROFILE="${PROFILE:-release}"
EXE="$ROOT/target/x86_64-pc-windows-msvc/$PROFILE/victory.exe"
[[ -f "$EXE" ]] || { echo "$EXE not built yet: run linux/build.sh" >&2; exit 1; }

STATE_DIR="${STATE_DIR:-$HOME/.local/share/victory-linux}"
mkdir -p "$STATE_DIR"/{cache,data}

# Wine shows the Linux file system as drive Z:.
win() { printf 'Z:%s' "${1//\//\\}"; }

export WINEPREFIX="$STATE_DIR/prefix"
export GAMEID="${GAMEID:-umu-default}"
export PROTONPATH="${PROTONPATH:-/usr/share/steam/compatibilitytools.d/proton-cachyos-slr}"
export VICTORY_CACHE_DIR="${VICTORY_CACHE_DIR:-$(win "$STATE_DIR/cache")}"
export VICTORY_DATA_DIR="${VICTORY_DATA_DIR:-$(win "$STATE_DIR/data")}"
# Quieter Wine output; the layer's own log goes to the terminal.
export WINEDEBUG="${WINEDEBUG:--all}"
if [[ -n "${GAME_DIR:-}" ]]; then
    export VICTORY_GAME_DIR="$(win "$GAME_DIR")"
fi

# A first, short run creates the prefix or brings it up to the Proton build
# in use, so that the files put into it below are not replaced afterwards.
# (The verb stays Proton's default: `runinprefix` skips its Direct3D set-up,
# and the title then gets Wine's own d3d12 instead of vkd3d-proton.)
cd "$STATE_DIR"
SYS32="$WINEPREFIX/drive_c/windows/system32"
umu-run "$(win "$EXE")" config >/dev/null 2>&1 || true
[[ -d "$SYS32" ]] || { echo "Proton did not create $WINEPREFIX" >&2; exit 1; }

# The layer compiles a few HLSL helpers of its own (cs_5_1, with root
# signatures) through d3dcompiler_47.dll: the indirect draws the world is
# made of, and copies in card memory. Wine's built-in one cannot compile
# them, and the one `winetricks d3dcompiler_47` installs (6.3.9600, 2013)
# predates cs_5_1. The prefix needs a Windows 10 era copy (10.0.x, as many
# games ship beside their executable): name one as D3DCOMPILER in
# linux/local.env and it is put into the prefix here.
D3DC="$SYS32/d3dcompiler_47.dll"
if [[ -n "${D3DCOMPILER:-}" ]] && ! cmp -s "$D3DCOMPILER" "$D3DC"; then
    rm -f "$D3DC"
    cp "$D3DCOMPILER" "$D3DC"
fi
export WINEDLLOVERRIDES="d3dcompiler_47=n,b${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
if ! grep -aq 'cs_5_1' "$D3DC" 2>/dev/null; then
    echo "warning: $D3DC cannot compile cs_5_1 shaders: the world will not draw (set D3DCOMPILER, see linux/run.sh)" >&2
fi

if (($# == 0)); then
    [[ -n "${GAME_DIR:-}" ]] || { echo "set GAME_DIR (see linux/local.env)" >&2; exit 1; }
    if [[ -z "${GAME_EXE:-}" ]]; then
        shopt -s nullglob
        exes=("$GAME_DIR"/*.exe)
        ((${#exes[@]} == 1)) || { echo "set GAME_EXE: ${#exes[@]} executables in $GAME_DIR" >&2; exit 1; }
        GAME_EXE="$(basename "${exes[0]}")"
    fi
    set -- run "$(win "$GAME_DIR/$GAME_EXE")" --execute
fi

# vkd3d-proton keeps its own pipeline cache in the working directory by
# default: derived data, which must stay out of the source tree.
export VKD3D_SHADER_CACHE_PATH="${VKD3D_SHADER_CACHE_PATH:-$STATE_DIR/cache}"

# Under vkd3d-proton a pipeline's DXIL is turned into SPIR-V on the stack of
# the thread that asks for the pipeline (on Windows the driver does that work
# on threads of its own), and the largest compute programs overflow Rust's
# default 2 MB there. Under Wine the thread then dies silently: `victory warm`
# never ends and a run keeps pipelines "being made" for good. This is the
# stack of every thread the layer starts itself.
export RUST_MIN_STACK="${RUST_MIN_STACK:-67108864}"

# Some Proton builds start the program through steam.exe, which gives it a
# console window of its own instead of our terminal. Its output is therefore
# sent to a file by a batch file, and followed from here.
LOG="${VICTORY_LOG_FILE:-$STATE_DIR/victory.log}"
BAT="$STATE_DIR/launch.bat"
{
    printf '@echo off\r\n"%s"' "$(win "$EXE")"
    for a in "$@"; do printf ' "%s"' "${a//%/%%}"; done
    printf ' > "%s" 2>&1\r\nexit /b %%ERRORLEVEL%%\r\n' "$(win "$LOG")"
} > "$BAT"
: > "$LOG"
tail -n +1 -F "$LOG" 2>/dev/null &
TAIL=$!
trap 'kill $TAIL 2>/dev/null' EXIT
rc=0
umu-run cmd.exe /c "$(win "$BAT")" >"${PROTON_LOG_FILE:-$STATE_DIR/proton.log}" 2>&1 || rc=$?
/usr/bin/sleep 0.5
exit $rc
