#!/bin/bash
source "$(dirname "$(readlink -f "$0")")/env.sh"
mode=$1; shift
case "$mode" in
  shaders|warm|play) ;;
  *) echo "usage: run.sh shaders|warm|play [VAR=value...]"; exit 1;;
esac
if [ ! -d "${GAME:-}" ]; then echo "no game: write GAME=/path/to/your/dump in $root/run.conf"; exit 1; fi
exe=${EXE:-$root/target/x86_64-pc-windows-msvc/diag/victory.exe}
if [ ! -f "$exe" ] || [ ! -f "$VK_DRIVER_FILES" ]; then echo "not built yet: run $root/setup.sh"; exit 1; fi
for dev in /sys/bus/pci/devices/*; do
  if [ "$(cat "$dev/vendor")" = 0x1002 ] && [[ $(cat "$dev/class") = 0x03* ]]; then
    echo on | sudo -n tee "$dev/power/control" >/dev/null 2>&1
  fi
done
for k in $(env | grep -o '^VICTORY_[A-Z0-9_]*'); do unset "$k"; done
mkdir -p "$state/cache" "$state/data" "$state/run" "$state/mesa-cache" "$state/vkd3d-cache"
export WINEDLLOVERRIDES='mmdevapi=b;d3d12,d3d12core,dxgi=n;d3dcompiler_47=n,b;winemenubuilder.exe=d'
export VICTORY_GAME_DIR=$(win "$GAME") VICTORY_CACHE_DIR=$(win "$state/cache") VICTORY_DATA_DIR=$(win "$state/data")
export VICTORY_DXC=$(win "$root/deps/dxc") VICTORY_DXIL=$(win "$root/deps/dxc/dxil.dll")
if [ -f "$root/deps/fsr4/amd_fidelityfx_upscaler_dx12.dll" ]; then
  export VICTORY_FIDELITYFX_DIR=$(win "$root/deps/fsr4") FSR4_UPGRADE=1
fi
export VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1
export VICTORY_JOIN_COPIES=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2
export VICTORY_INDIRECT_BATCH=3 VICTORY_MAP_THREADS=1 VICTORY_TITLE_PRIORITIES=0
export VICTORY_SHOW_AFTER=1 VICTORY_AMBIENT=0.8
if [ "${SAFE:-0}" = 1 ]; then
  export VICTORY_UNDER_WIDE=0 VICTORY_LAND_ECHO=0
else
  export VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1
  export VICTORY_LAND_ECHO=0
  export VICTORY_PORT_CBV=1 VICTORY_PORT_CONSTANTS_ORDER_ALONE=0
  export VICTORY_PORT_DESCRIPTORS=1 VICTORY_PORT_COPIES=1 VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1
fi
export VICTORY_WAIT_RESIDENT=0
if [ "${MUTE:-0}" = 1 ]; then source "$root/tools/mute.sh"; mute; fi
if [ "$mode" = play ]; then
  export VICTORY_UNIFIED_VRAM=1 VICTORY_FRAME_INTERVAL_MS=0
  export VICTORY_ALIAS_RUNS=1
  export RADV_VUM_MIRROR_ARENA=1 VICTORY_MIRROR_ARENA=1
  export VICTORY_TEXTURE_STREAM=1
  export VICTORY_PREPARE_PIPELINES=1 VICTORY_PREPARE_PASSES=1 VICTORY_READY_COMPUTE=1
  export VICTORY_CARD_TEXTURES=1 VICTORY_CARD_STAMPS=1 VICTORY_CARD_ARENAS=1
  export MANGOHUD="${MANGOHUD:-1}"
  export MANGOHUD_CONFIG="${MANGOHUD_CONFIG:-fps,ram,vram}"
fi
if [ "$mode" = warm ]; then
  export VICTORY_PREPARE_PIPELINES=1 VICTORY_PREPARE_PASSES=1
fi
for kv in "$@"; do case "$kv" in *=*) export "$kv";; esac; done
if [ "$mode" = play ] && [ "${VICTORY_BVH_BUILDER:-0}" = 1 ] && [ -z "${VICTORY_AMBIENT_RT+x}" ]; then
  export VICTORY_AMBIENT_RT=0.7,1.5
fi
avail=$(awk '/MemAvailable/{print int($2/1024)}' /proc/meminfo)
kept=$(sudo -n cat /sys/kernel/debug/ttm/page_pool 2>/dev/null | awk '/^total/{for(i=1;i<NF;i++) if($(i+1)=="of"){print int($i/256); exit}}')
avail=$((avail+${kept:-0}))
cap=$((avail-${KEEP_FREE:-8192}))
if (( cap < 2048 )); then
  if [ "$mode" != play ]; then echo "not enough free memory: $avail MiB available"; exit 1; fi
  cap=2048
fi
if [ ! -f /proc/self/fd/1 ]; then
  log=$state/run/$mode-$(date +%Y%m%d-%H%M%S).log
  ln -sfn "$log" "$state/run/last.log"
  exec > >(tee "$log") 2>&1
fi
cd "$root"
case "$mode" in
  shaders) args=(shaders "$(win "$GAME/shaders.rpf")");;
  warm) args=(warm --used);;
  play) args=(run "$(win "$GAME/game_scarlett_master.exe")" --execute --console "${CONSOLE:-series-x}");;
esac
exec systemd-run --user --scope --collect -q --unit "um-$mode-$$" -p "MemoryMax=${cap}M" -p MemorySwapMax=2G -- \
  unshare -rnm --propagation private "$root/tools/play-wine.sh" "$exe" "${args[@]}"
