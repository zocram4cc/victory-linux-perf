#!/usr/bin/env bash
# Runs a VIctory build (Windows, Rust) under Proton on Linux.
# Part of this repository (avatarmanz/VIctory-7900XTX + the Linux/Proton changes).
# Tested with Proton Experimental (Wine 11, vkd3d-proton), RADV, RX 7900 XT,
# Ryzen 7 5800X3D,
# 32 GB: ~18-19 fps in the open world, ~16-17 fps in 8,000-10,000 draw views
# (with the huge pages of linux/thp/).
#
# Usage:  victory-proton.sh [extra args for `victory run`]
# Every VICTORY_* / VKD3D_* / RADV_* variable you set yourself wins.
#
# Before the first run:
#  - build the fork:  cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
#  - put a native d3dcompiler_47.dll (x64, e.g. from the Firefox 62 installer,
#    as winetricks fetches it) at $D3DCOMPILER below. Wine's builtin cannot
#    compile the fork's helper shaders: indirect draws are dropped, 3D is black.
set -euo pipefail

# --- Your paths ---------------------------------------------------------------
REPO="${REPO:-$(cd "$(dirname "$0")/.." && pwd)}"     # this checkout
GAME_EXE="${GAME_EXE:-$HOME/games/xbox-dump/game.exe}" # your own copy's executable
D3DCOMPILER="${D3DCOMPILER:-$HOME/victory-tools/d3dcompiler_47.dll}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
PREFIX="${PREFIX:-$HOME/.local/share/victory-prefix}"  # a Proton prefix of its own
LOGDIR="${LOGDIR:-$HOME/victory-logs}"
PROFILE="${VICTORY_PROFILE:-release}"                  # release or diag

win() { echo "Z:${1//\//\\}"; }

# --- The fork's fast settings (its scripts/play.ps1, non-safe mode) -----------
for kv in VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1 \
    VICTORY_JOIN_COPIES=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2 \
    VICTORY_INDIRECT_BATCH=2 VICTORY_MAP_THREADS=1 VICTORY_TITLE_PRIORITIES=0 VICTORY_CARD_AWAKE=1 \
    VICTORY_SHOW_AFTER=1 VICTORY_AMBIENT=0.8 VICTORY_D3D12_DRED=light VICTORY_SKIP_PIPELINES=none \
    VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1 \
    VICTORY_PORT_DESCRIPTORS=1 VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1 VICTORY_PORT_CBV=1 \
    VICTORY_PORT_CONSTANTS=1 VICTORY_COPY_COMPARE=1 VICTORY_ONE_PAD=0 VICTORY_FRAME_INTERVAL_MS=0; do
    k=${kv%%=*}
    [ -z "${!k+x}" ] && export "$kv"
done

# NOT RADV_DEBUG=hang: it turns on syncshaders, and the card becomes the
# bottleneck (~205 ms GPU a frame, ~4 fps). Set it yourself only to get a hang
# report (~/radv_dumps_*) when the GPU hangs.
export RADV_DEBUG="${RADV_DEBUG-}"

# Port 1 (VICTORY_PORT_CONSTANTS) needs this build's patch under Proton: without
# it the picture is black (vkd3d-proton's heap-from-address only works on
# anonymous memory; the patch backs the title's 32-bit heap with private pages).
# VICTORY_FRAME_INTERVAL_MS=0: the title's own 30 fps cap off (frame and sim
# rate patched to "no limit"; plays normally). 33.333 puts the cap back.
# VICTORY_ONE_PAD=0: the fork's keyboard stand-in pad is not merged with real
# pads (an analog keyboard's gamepad mode would get every key twice).
# Devices that pretend to be joysticks can steer on their own: list them as
# SDL_GAMECONTROLLER_IGNORE_DEVICES=0xVID/0xPID,... (e.g. OpenTabletDriver's
# virtual tablet is 0x0000/0x0000).
export SDL_GAMECONTROLLER_IGNORE_DEVICES="${SDL_GAMECONTROLLER_IGNORE_DEVICES-0x0000/0x0000}"
export SDL_JOYSTICK_IGNORE_DEVICES="${SDL_JOYSTICK_IGNORE_DEVICES-$SDL_GAMECONTROLLER_IGNORE_DEVICES}"

# --- Proton -------------------------------------------------------------------
export STEAM_COMPAT_DATA_PATH="$PREFIX"
export STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
export WINEDLLOVERRIDES="d3dcompiler_47=n,b${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
# vkd3d-proton keeps its pipeline cache in the working directory otherwise.
export VKD3D_SHADER_CACHE_PATH="$PREFIX/vkd3d-cache"
mkdir -p "$PREFIX" "$VKD3D_SHADER_CACHE_PATH" "$LOGDIR"

# Optional overlay: VICTORY_HUD=1 with MangoHud installed.
if [ "${VICTORY_HUD:-0}" != 0 ]; then export MANGOHUD=1; fi

# --- Run ----------------------------------------------------------------------
stamp=$(date +%Y%m%d-%H%M%S)
log="$LOGDIR/run-$stamp.txt"
echo "log: $log ($PROFILE build)"

# --- Huge pages for renderer port 1 (AMD) ---------------------------------------
# amdgpu checks the port-1 heap (a userptr, ~185 MB) page by page on every
# submission; as 2 MiB pages that is 512 times shorter: ~9 -> ~18-19 fps here.
# See linux/thp/README.md. Needs the helper built and given CAP_SYS_NICE once.
# VICTORY_THP=0 turns it off.
thp_watch() {
    set +eu +o pipefail
    local helper="$REPO/linux/thp/victory-thp-collapse" out="${log%.txt}.thp" pid ranges r last=""
    if ! getcap "$helper" 2>/dev/null | grep -q cap_sys_nice; then
        echo "huge pages: off (see linux/thp/README.md: build $helper, then sudo setcap cap_sys_nice+ep on it)"
        return
    fi
    while kill -0 "$1" 2>/dev/null; do
        sleep 15
        # the Windows process: the one of them with the most threads
        pid=$(for p in $(pgrep -f 'victory\.exe run'); do echo "$(ls /proc/$p/task 2>/dev/null | wc -l) $p"; done \
            | sort -rn | awk 'NR==1{print $2}')
        [ -n "$pid" ] || continue
        ranges=$(grep -aoE "32-bit heap are private memory.*at=0x[0-9a-f]+ len=[0-9]+" "$log" 2>/dev/null \
            | sed -E 's/.*at=0x([0-9a-f]+) len=([0-9]+)/\1 \2/' | sort -u \
            | while read -r a l; do printf "%x %x " $((16#$a)) $((16#$a + l)); done)
        [ -n "$ranges" ] || continue
        # shellcheck disable=SC2086
        r=$("$helper" "$pid" $ranges 2>&1)
        if [ "$r" != "$last" ]; then echo "$(date +%T) $r" >> "$out"; last="$r"; fi
    done
}
if [ "${VICTORY_THP:-1}" != 0 ]; then
    thp_watch $$ &
fi
cd "$REPO"
cp -u "$D3DCOMPILER" "target/x86_64-pc-windows-msvc/$PROFILE/"
# Without a terminal the exe dies on its first println: its output goes
# through cmd /c into the log file instead.
exec "$PROTON" run cmd /c "target\\x86_64-pc-windows-msvc\\$PROFILE\\victory.exe run $(win "$GAME_EXE") --execute $* > $(win "$log") 2>&1"
