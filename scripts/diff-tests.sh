#!/usr/bin/env bash
# The translator's differential tests (crates/victory-shader/src/emit/
# diff_tests.rs) on this Linux machine: the Windows test binary under the
# game's Proton (vkd3d-proton on the real GPU), the console compiler under
# that same Wine, and the public PC compiler. Each test compiles one HLSL
# source for the console (then translates the console's native code) and for
# PC, runs both on the GPU with the same inputs and compares the results.
#
#   scripts/diff-tests.sh [TEST NAME FILTER]   (default: all diff tests)
#
# Needs (local, not in the repository):
#   K_CONSOLE_DXC  the console SDK's native-code compiler (xdxc.exe of the
#                  current console generation); the user's own SDK copy
#   K_PC_DXC       DirectXShaderCompiler's public release (dxc.exe, dxil.dll)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROTON=${PROTON:-$HOME/Games/victory-data/proton-vt/GE-Proton11-14-vt/proton}
PREFIX=${K_DIFF_PREFIX:-$HOME/Games/victory-data/perfiles/difftest}
CONSOLE=${K_CONSOLE_DXC:-$HOME/Games/xbox/2026.10-GDKX-ExtractedforBuildSystems/Microsoft GDK/261000/xbox/bin/gen9/xdxc.exe}
PC=${K_PC_DXC:-$HOME/Games/victory-data/tools/dxc-pc/unpacked/x64}
win() { echo "Z:${1//\//\\}"; }
[[ -f $CONSOLE ]] || { echo "console compiler not found: $CONSOLE" >&2; exit 1; }
[[ -f $PC/dxc.exe ]] || { echo "PC compiler not found in $PC" >&2; exit 1; }
cd "$ROOT"
CARGO_TARGET_DIR=$ROOT/target-diff PATH=$HOME/.cargo/bin:$PATH \
    cargo xwin test --no-run --release -p victory-shader --target x86_64-pc-windows-msvc >/dev/null 2>&1
exe=$(ls -t "$ROOT"/target-diff/x86_64-pc-windows-msvc/release/deps/victory_shader-*.exe | head -n1)
mkdir -p "$PREFIX"
export STEAM_COMPAT_DATA_PATH=$PREFIX
export STEAM_COMPAT_CLIENT_INSTALL_PATH=${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}
export VICTORY_CONSOLE_DXC=$(win "$CONSOLE") VICTORY_PC_DXC=$(win "$PC/dxc.exe") VICTORY_DXIL=$(win "$PC/dxil.dll")
export WINEDEBUG=${WINEDEBUG:--all}
# Proton does not pass a console program's output through: to a file.
log=$PREFIX/diff-tests.log
rm -f "$log"
"$PROTON" run cmd /c "$(win "$exe") ${1:-diff_tests} --test-threads=1 --nocapture > $(win "$log") 2>&1" >/dev/null 2>&1 || true
# `proton run` can return before the program ends: wait for this prefix's
# wineserver to go.
WINEPREFIX=$PREFIX/pfx "$(dirname "$PROTON")/files/bin/wineserver" -w 2>/dev/null || true
tr -d '\r' < "$log" | grep -v openvr
