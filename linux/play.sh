#!/usr/bin/env bash
# play.sh [stable|today|fast|quality|rt|steady] [minutes] [route]: a player session of the fps20-ports
# build on the precompiled seed.
#   stable   today's tree with renderer port 1 off: about 24 FPS, steady
#            light, no white square in the corner, floor and phone fixed
#            (NOTES 172); the recommended one to play
#   today    today's tree, ray tracing off, floor and phone fix: 29-30 FPS.
#            With a third word the keys held are recorded to
#            captures-fix/routes/<route>.keys, for leg.sh to play back
#   fast     (default) ray-tracing structure builder off: 22-26 FPS, stand-in
#            light, noisy phone and glass indoors, hair missing in some starts
#   quality  builder on: about 9 FPS, the room, glass and aquarium lit as the
#            title means them
#   rt       builder on, ray tracing through memory windows (bin/victory.rt6.keep,
#            seed-rt-windows): 19-20 FPS measured on a quiet machine
#   steady   the binary of 2026-11-26 21:19 (before the write-guard changes):
#            about 18 FPS, hair in 4 of 4 starts
# One game at a time: refuses to start while another runs.
set -euo pipefail
mode=${1:-fast}; minutes=${2:-30}; route=${3:-}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
profile=${VICTORY_PROFILE:-@PROFILE@}
if pgrep -af '[v]ictory.exe run'; then echo "a game is running" >&2; exit 3; fi
exe=$here/bin/victory.fix4.keep; seed=seed-precompiled-full; overrides=""
case $mode in
    stable) exe=$here/bin/victory.new4.keep; seed=seed-rt-windows; export VICTORY_PORT_CONSTANTS=0 ;;
    today) exe=$here/bin/victory.new4.keep; seed=seed-rt-windows
        if [[ -n $route ]]; then
            keys=@PORTING@/captures-fix/routes/$route.keys
            : > "$keys"
            overrides="VICTORY_KEYS_RECORD=Z:$keys"
            echo "recording keys to $keys" >&2
        fi ;;
    fast) ;;
    quality) overrides="VICTORY_BVH_BUILDER=1" ;;
    rt) exe=$here/bin/victory.rt6.keep; seed=seed-rt-windows; overrides="VICTORY_BVH_BUILDER=1" ;;
    steady) exe=$here/bin/victory.release.exe.keep; seed=seed-shadow-off ;;
    *) echo "usage: $0 [stable|today|fast|quality|rt|steady] [minutes] [route]" >&2; exit 2 ;;
esac
[[ -f $exe ]] || { echo "missing $exe" >&2; exit 4; }
cp -f "$exe" "$here/bin/victory.exe"
(cd "$here" && sha256sum bin/victory.exe > SHA256SUMS)
n=1; while [[ -e $profile/harbor-x-play-$mode-$n ]]; do n=$((n + 1)); done
cd "$here"
VICTORY_OVERRIDES="$overrides" SUPERVISOR_RESERVE_GIB=${SUPERVISOR_RESERVE_GIB:-1.0} PORTS_LONG=1 \
    PORTS_WORLD_SECONDS=$((minutes * 60)) VICTORY_PIPELINE_WORKERS=4 \
    python3 run-ports.py "harbor-x-play-$mode-$n" "$seed" series-x
