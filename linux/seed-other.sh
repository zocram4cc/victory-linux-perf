#!/usr/bin/env bash
# seed-other.sh <seed-name> <exe> <shaders|warm> [VAR=value ...]: a seed of
# its own for another fork's build: an empty cache, every title shader
# translated by that binary (shaders), or its host pipelines made from the
# book a first run left in <seed>/cache/pipelines (warm). The extra
# VAR=value are the switches the translation is keyed on, as the run has them.
set -euo pipefail
name=$1; exe=$2; phase=$3; shift 3
port=@PORTING@
profile=@PROFILE@
seed=$profile/$name
proton=$profile/team-diagnostics/server-index-ab/indexed
wine=$proton/files/bin/wine
ws=$proton/files/bin/wineserver
if [[ ! -d $seed ]]; then
    staging=$(mktemp -d "$profile/.seed-other-XXXXXX")
    mkdir -p "$staging/data" "$staging/cache" "$staging/vkd3d-cache"
    cp -a --reflink=always "$profile/loop/prefix" "$staging/"
    install -m 644 "$profile/team-diagnostics/server-index-ab/d3dcompiler_47.dll" "$staging/prefix/pfx/drive_c/windows/system32/d3dcompiler_47.dll"
    mv -T -- "$staging" "$seed"
fi
winpath() { printf 'Z:%s' "${1//\//\\}"; }
export WINEPREFIX="$seed/prefix/pfx"
export VICTORY_CACHE_DIR="Z:$seed/cache" VICTORY_DATA_DIR="Z:$seed/data"
game=$(realpath "$port/../title-work")
export VICTORY_GAME_DIR="$(winpath "$game")"
export VKD3D_SHADER_CACHE_PATH="$seed/vkd3d-cache" MESA_SHADER_CACHE_MAX_SIZE=10G
export VICTORY_WINDOWS_BAR_MB=4096 VICTORY_BVH_BUILDER=0 VICTORY_CARD_SPANS=1
export VICTORY_BVH_EMPTY=0 VICTORY_LEGACY_FTZ=0 VICTORY_COMPACT_VIEW_HEAP=1 VICTORY_DYNAMIC_SLOTS=1048576
export WINEDEBUG=-all RUST_LOG=info MANGOHUD=0
export WINEDLLOVERRIDES='d3d12,d3d12core,dxgi,d3d11,d3d10core,d3d9=n,b;d3dcompiler_47=n,b'
for kv in "$@"; do export "$kv"; done
cd "$seed"
if [[ $phase == shaders ]]; then
    echo "=== shaders $(date +%T)"
    timeout 3600 "$wine" "$exe" shaders "$(winpath "$game/shaders.rpf")" --jobs 12 > "$seed/shaders.log" 2>&1 || echo "shaders exit $?"
    grep -v -E "vkd3d|^info:|^warn:|ntsync" "$seed/shaders.log" | tail -5
else
    echo "=== warm $(date +%T)"
    timeout 3600 "$wine" "$exe" warm --jobs 8 > "$seed/warm.log" 2>&1 || echo "warm exit $?"
    grep -E " of [0-9]+, | made in " "$seed/warm.log" | tail -3
fi
"$ws" -k || true
echo "DONE $(date +%T)"
