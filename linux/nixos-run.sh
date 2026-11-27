#!/usr/bin/env bash
# Run this tree's victory.exe (built into ~/.cache/vp-upstream-win by
#   cargo xwin build --release -p victory --target x86_64-pc-windows-msvc, CARGO_TARGET_DIR=~/.cache/vp-upstream-win)
# on this NixOS machine: gamescope, steam-run, the patched Proton Wine (wineserver view tree), the game's
# own prefix, upstream's settings (linux/victory-proton.sh). Paths below are this machine's.
# Usage: nixos-run.sh <name> <seconds> [--console series-s] [KEY=VAL]...  (log: ~/.cache/scarlettport-autotest/<name>/run.log)
set -uo pipefail
NAME="$1"; DUR="$2"; shift 2
CONSOLE=()
if [[ "${1:-}" == "--console" ]]; then CONSOLE=(--console "$2"); shift 2; fi
T=~/.cache/scarlettport-autotest; OUT=$T/$NAME; rm -rf "$OUT"; mkdir -p "$OUT"
EXE_DIR=~/.cache/vp-upstream-win/x86_64-pc-windows-msvc/${PROFILE:-diag}
GAME=/mnt/nvme/GTA6/GTA6/GTA6.Xbox.v1.0.2.1-CYBERLEEK/game_scarlett_master.exe
PROTON_DIR=~/.cache/scarlettport-autotest/proton-vmutex
PREFIX=~/.local/share/victory-proton
win() { echo "Z:${1//\//\\}"; }

# Microsoft's d3dcompiler_47 (Wine's builtin cannot compile the layer's helper shaders).
D3DC=$(ls -S "$PREFIX/pfx/drive_c/windows/system32/d3dcompiler_47.dll" /mnt/hdd/Nix/ScarlettPort-Linux/engine/target/release/d3dcompiler_47.dll 2>/dev/null | head -1)
cp -u "$D3DC" "$EXE_DIR/" 2>/dev/null

export STEAM_COMPAT_DATA_PATH="$PREFIX" STEAM_COMPAT_CLIENT_INSTALL_PATH="$HOME/.local/share/Steam" SteamAppId=0 SteamGameId=0
export WINEPREFIX="$PREFIX/pfx" WINESERVER="$PROTON_DIR/files/bin/wineserver" WINEESYNC=1 WINEFSYNC=1 WINEDEBUG="${WINEDEBUG:--all}"
export WINEDLLOVERRIDES="d3d12,d3d12core,dxgi,d3dcompiler_47=n"
export VKD3D_FILTER_DEVICE_NAME="7900 XTX" VKD3D_SHADER_MODEL=6_6 VKD3D_FEATURE_LEVEL=12_2 VKD3D_CONFIG="${VKD3D_CONFIG:-dxr}"
export VKD3D_SHADER_CACHE_PATH="$(win "$T/upstream-cache/vkd3d")"; mkdir -p "$T/upstream-cache/vkd3d"
export MESA_SHADER_CACHE_MAX_SIZE=12G
# Upstream's settings (linux/victory-proton.sh, 2026-11-27).
for kv in VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1 \
    VICTORY_JOIN_COPIES=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2 \
    VICTORY_INDIRECT_BATCH=2 VICTORY_MAP_THREADS=1 VICTORY_TITLE_PRIORITIES=0 VICTORY_CARD_AWAKE=1 \
    VICTORY_SHOW_AFTER=1 VICTORY_AMBIENT=0.8 VICTORY_D3D12_DRED=light VICTORY_SKIP_PIPELINES=none \
    VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1 \
    VICTORY_PORT_DESCRIPTORS=1 VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1 VICTORY_PORT_CBV=1 \
    VICTORY_PORT_CONSTANTS=1 VICTORY_COPY_COMPARE=1 VICTORY_ONE_PAD=0 VICTORY_FRAME_INTERVAL_MS=0 \
    VICTORY_PIPELINE_WAIT_MS=30; do
    k=${kv%%=*}; [ -z "${!k+x}" ] && export "$kv"
done
# Enter tapped every 2 s from 8 to 44 s (start screen: press A), then held 3 s every 6 s until
# KEYS_UNTIL (a cutscene is skipped by holding A). KEYS=0: no scripted keys (your own input works).
if [ "${KEYS:-1}" != 0 ]; then
    taps=$(for t in $(seq 8 2 44); do printf '%s:Enter,' "$t"; done)
    holds=$(for t in $(seq 46 6 ${KEYS_UNTIL:-130}); do printf '%s-%s:Enter,' "$t" "$((t + 3))"; done)
    export VICTORY_KEYS="${VICTORY_KEYS:-${taps}${holds%,}}"
fi
export RUST_LOG="${RUST_LOG:-info}"
for kv in "$@"; do export "$kv"; done

GS="$(ls -d /nix/store/*-gamescope-[0-9]*/bin/gamescope 2>/dev/null | sort -V | tail -1)"
cd "$EXE_DIR"
echo "==> upstream run '$NAME' ${DUR}s ${CONSOLE[*]}"
timeout --signal=INT --kill-after=10 "${DUR}s" "$GS" -W 1920 -H 1080 -w 1920 -h 1080 -- \
    steam-run "$PROTON_DIR/files/bin/wine" "$(win "$EXE_DIR/victory.exe")" run "$(win "$GAME")" --execute "${CONSOLE[@]}" \
    > "$OUT/run.log" 2>&1
echo "==> ended rc=$?"
sed 's/\x1b\[[0-9;]*m//g' "$OUT/run.log" | grep -a "fps" | grep -aE "[1-9][0-9]{3,} draws" | tail -5 | cut -c1-220
