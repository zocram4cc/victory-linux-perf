#!/usr/bin/env bash
# Play clankerVI (victory-proton) on this NixOS machine: release build, your own input (no scripted
# keys), patched Proton Wine (with UFFD_WP section write-watch + lock-free PAGEMAP_SCAN), gamescope.
#   ./play-nixos.sh                   Series S (~35.5-36.6 fps sustained)
#   CONSOLE=series-x ./play-nixos.sh  Series X @ 2560x1440 (~33.5-34.6 fps)
# Log: ~/.cache/scarlettport-autotest/play-<time>/run.log
CONSOLE="${CONSOLE:-series-s}"
if [ -z "${PROFILE+x}" ]; then
    if [ -x "$HOME/.cache/vp-upstream-win/x86_64-pc-windows-msvc/fast/victory.exe" ]; then
        PROFILE=fast
    else
        PROFILE=release
    fi
fi
args=()
extra=()
if [ "$CONSOLE" != series-x ]; then
    args=(--console "$CONSOLE")
else
    extra=(VICTORY_OUTPUT=2560x1440)
fi
PROFILE="$PROFILE" KEYS=0 exec "$(dirname "$0")/nixos-run.sh" "play-$(date +%H%M%S)" 86400 \
    "${args[@]}" "${extra[@]}" \
    VICTORY_PIPELINE_WAIT_MS=40 \
    VICTORY_PIPELINE_WAIT_EACH_MS=100 \
    VICTORY_TEXTURE_STAGE_MS=12 \
    VICTORY_TEXTURE_STAGE_MB=64 \
    VICTORY_TEXTURE_CATCHUP=0 \
    VICTORY_PORT_PRIVATE=1 \
    VICTORY_WINDOWS_BAR_MB=4096 \
    VICTORY_CARD_BUFFERS_MIN_KB=64 \
    VICTORY_WIDE_SPIN=200 \
    VICTORY_UPSCALER=6 \
    VICTORY_LEARNED_ON_CARD=1 \
    VICTORY_PLACED_ORDER=1 \
    VICTORY_SUBMIT_SPLIT=1 \
    VICTORY_OFF_LINE=1 \
    VICTORY_FAST_MEMCPY=1 \
    VICTORY_SLEEP0_YIELD=1 \
    VICTORY_ASK_SPREAD=1 \
    VICTORY_JOIN_BARRIERS=1 \
    VICTORY_THIN_THREADS=8 \
    VICTORY_GUARD_REST=500000 \
    VICTORY_GUARD_HOT=10000 \
    VICTORY_MOUSE_SENS="${VICTORY_MOUSE_SENS:-1.35}" \
    VICTORY_WAIT_RESIDENT=0
