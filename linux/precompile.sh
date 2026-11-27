#!/usr/bin/env bash
# precompile.sh: a new seed (reflink of seed-shadow-off) with every title
# shader translated again by the current binary (`victory shaders`) and every
# known pipeline made (`victory warm`). The source seed is left untouched.
set -euo pipefail
coord=@SCRATCH@
work=$coord/fps20-ports
port=@PORTING@
profile=@PROFILE@
src=$profile/seed-shadow-off
seed=$profile/seed-precompiled-full
proton=$profile/team-diagnostics/server-index-ab/indexed
wine=$proton/files/bin/wine
ws=$proton/files/bin/wineserver
exe=$work/bin/victory.exe
phase=${1:-all}

exec 9<>"$coord/game.lock"
flock -x 9
if pgrep -af '[v]ictory.exe run'; then echo "game running"; exit 3; fi
(cd "$work" && sha256sum -c SHA256SUMS)

if [[ ! -d $seed ]]; then
    staging=$(mktemp -d "$profile/.seed-precompiled-XXXXXX")
    mkdir -p "$staging/data"
    cp -a --reflink=always "$profile/loop/prefix" "$staging/"
    cp -a --reflink=always "$src/cache" "$src/vkd3d-cache" "$staging/"
    install -m 644 "$profile/team-diagnostics/server-index-ab/d3dcompiler_47.dll" "$staging/prefix/pfx/drive_c/windows/system32/d3dcompiler_47.dll"
    mv -T -- "$staging" "$seed"
fi

winpath() { printf 'Z:%s' "${1//\//\\}"; }
export WINEPREFIX="$seed/prefix/pfx"
export VICTORY_CACHE_DIR="Z:$seed/cache"
export VICTORY_DATA_DIR="Z:$seed/data"
VICTORY_GAME_DIR="$(winpath "$(realpath "$port/../title-work")")"
export VICTORY_GAME_DIR
export VKD3D_SHADER_CACHE_PATH="$seed/vkd3d-cache"
# The translation's key as the player's launcher (launch-ports.sh) has it.
export VICTORY_WINDOWS_BAR_MB=4096 VICTORY_BVH_BUILDER=0 VICTORY_CARD_SPANS=1
export VICTORY_PORT_CBV=0 VICTORY_PORT_CONSTANTS=1 VICTORY_PORT_DESCRIPTORS=0
export VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1
export VICTORY_BVH_EMPTY=0 VICTORY_LEGACY_FTZ=0
export VICTORY_COMPACT_VIEW_HEAP=1 VICTORY_DYNAMIC_SLOTS=1048576
export WINEDEBUG=-all RUST_LOG=info MANGOHUD=0
export MESA_SHADER_CACHE_MAX_SIZE="${MESA_SHADER_CACHE_MAX_SIZE:-10G}"
export WINEDLLOVERRIDES='d3d12,d3d12core,dxgi,d3d11,d3d10core,d3d9=n,b;d3dcompiler_47=n,b'
userptr_lib=$port/tools/libdrm-amdgpu-shared-userptr
[[ -f $userptr_lib/libdrm_amdgpu.so.1 ]] && export LD_LIBRARY_PATH="$userptr_lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

cd "$seed"
if [[ $phase == all || $phase == shaders ]]; then
    echo "=== victory shaders (jobs 16) $(date +%T)"
    timeout 3600 "$wine" "$exe" shaders --jobs 16 --list "$(winpath "$seed/shaders-failed.txt")" 9>&- > "$seed/shaders.log" 2>&1 || echo "shaders exit $?"
    grep -v -E "vkd3d|^info:|^warn:|ntsync" "$seed/shaders.log" | tail -25
    "$ws" -k || true
fi
if [[ $phase == all || $phase == warm ]]; then
    echo "=== victory warm (jobs ${WARM_JOBS:-8}) $(date +%T)"
    timeout 14400 "$wine" "$exe" warm --jobs "${WARM_JOBS:-8}" ${WARM_ARGS:-} 9>&- > "$seed/warm.log" 2>&1 || echo "warm exit $?"
    grep -E "pipelines known| of [0-9]+, | made in |^pipeline " "$seed/warm.log" | tail -30
    "$ws" -k || true
fi
echo "PRECOMPILE_DONE $(date +%T)"
