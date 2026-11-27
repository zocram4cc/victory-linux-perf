#!/usr/bin/env bash
# Play upstream victory-proton on this NixOS machine: release build, your own input (no scripted
# keys), patched Proton Wine, gamescope window. Series S by default (faster here: ~18.5 vs ~16.5 fps).
#   ./play-nixos.sh               Series S
#   CONSOLE=series-x ./play-nixos.sh
# Log: ~/.cache/scarlettport-autotest/play-<time>/run.log
CONSOLE="${CONSOLE:-series-s}"
args=()
[ "$CONSOLE" != series-x ] && args=(--console "$CONSOLE")
PROFILE=release KEYS=0 exec "$(dirname "$0")/nixos-run.sh" "play-$(date +%H%M%S)" 86400 "${args[@]}" VICTORY_PIPELINE_WAIT_MS=2 VICTORY_PORT_PRIVATE=1 VICTORY_UPSCALER=6
