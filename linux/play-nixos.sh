#!/usr/bin/env bash
# Play clankerVI (victory-proton) on this NixOS machine: release build, your own input (no scripted
# keys), patched Proton Wine (with UFFD_WP section write-watch + lock-free PAGEMAP_SCAN), gamescope.
#   ./play-nixos.sh                   Series S (30.3-31.7 fps sustained, GATE_REACH=1)
#   CONSOLE=series-x ./play-nixos.sh  Series X @ 2560x1440 (29.5-30.8 fps)
# Log: ~/.cache/scarlettport-autotest/play-<time>/run.log
CONSOLE="${CONSOLE:-series-s}"
args=()
extra=()
if [ "$CONSOLE" != series-x ]; then
    args=(--console "$CONSOLE")
else
    extra=(VICTORY_OUTPUT=2560x1440)
fi
PROFILE=release KEYS=0 exec "$(dirname "$0")/nixos-run.sh" "play-$(date +%H%M%S)" 86400 \
    "${args[@]}" "${extra[@]}" \
    VICTORY_PIPELINE_WAIT_MS=40 \
    VICTORY_PIPELINE_WAIT_EACH_MS=100 \
    VICTORY_TEXTURE_STAGE_MS=12 \
    VICTORY_TEXTURE_STAGE_MB=64 \
    VICTORY_TEXTURE_CATCHUP=0 \
    VICTORY_PORT_PRIVATE=1 \
    VICTORY_UPSCALER=6 \
    VICTORY_CARD_STEPS_GATE=1 \
    VICTORY_CARD_STEPS_GATE_REACH=1 \
    VICTORY_FAST_MEMCPY=1 \
    VICTORY_SLEEP0_YIELD=1 \
    VICTORY_ASK_SPREAD=1 \
    VICTORY_JOIN_BARRIERS=1 \
    VICTORY_THIN_THREADS=6 \
    VICTORY_GUARD_HOT=500 \
    VICTORY_WAIT_RESIDENT=0
