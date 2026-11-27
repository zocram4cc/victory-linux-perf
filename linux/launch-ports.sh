#!/usr/bin/env bash
set -euo pipefail
here=${HARBOR_PLAYER:?HARBOR_PLAYER must point to the verified player package}
port=@PORTING@
profile=@PROFILE@
seed=${HARBOR_SEED:?HARBOR_SEED must point to the verified warm seed}
save_base=${HARBOR_SAVE_BASE:-$profile/loop}
export VICTORY_RUNTIME="${VICTORY_RUNTIME:-$profile/player-fps20-ports}"
export VICTORY_EXE=$here/bin/victory.exe
export PROTON_DIR=$profile/team-diagnostics/server-index-ab/indexed
export WINEDEBUG=-all MANGOHUD=0 RUST_LOG=info
export WINEDLLOVERRIDES='d3dcompiler_47=n,b'
# Proton's winebus takes Sony pads over hidraw and shows them as raw HID, which
# XInput (all gameinput.rs reads) does not list. Through SDL they are XInput pads
# (DualShock 4 v1, v2 and its dongle; checked with a probe, not in the title).
export PROTON_DISABLE_HIDRAW="${PROTON_DISABLE_HIDRAW-0x054C/0x05C4,0x054C/0x09CC,0x054C/0x0BA0}"
export VICTORY_KEYS="${VICTORY_KEYS-12:Enter,18:Enter,26:Enter,38:Enter,52:Enter,65:Enter,80:Enter}"
export VICTORY_WINE_FREE_PLACEHOLDERS=1 VICTORY_MEMORY_MAP=0 VICTORY_GUARD_REST=3000
export VICTORY_PIPELINE_WORKERS="${VICTORY_PIPELINE_WORKERS:-8}" VICTORY_PIPELINE_BOOK=1 VICTORY_WINDOW_KB=4096 VICTORY_WINDOW_TAILS=0
export VICTORY_REFRESH_WORKERS=5 VICTORY_WINDOWS_BAR_MB=4096 VICTORY_BVH_BUILDER=0 VICTORY_CARD_SPANS=1
export VICTORY_TEXTURE_NO_WAIT=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_CARD_REHASH_RUNS=1 VICTORY_JOIN_COPIES=1
export VICTORY_INDIRECT_WINDOW=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2 VICTORY_INDIRECT_BATCH=2 VICTORY_MAP_THREADS=1
export VICTORY_TITLE_PRIORITIES=0 VICTORY_CARD_AWAKE=1 VICTORY_SHOW_AFTER=1 VICTORY_AMBIENT=0.8 VICTORY_AMBIENT_SETTLE=250,3 VICTORY_D3D12_DRED=light
export VICTORY_SKIP_PIPELINES=none VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1
export VICTORY_CARD_PAGE_MASK="${VICTORY_CARD_PAGE_MASK:-1}"
# Renderer port 1 opens Wine section views as a host heap; stock libdrm_amdgpu
# asks for anonymous-only userptr and the import fails silently (black picture),
# while libuserptr-thp collapses those ~185 MB userptr pages into 2 MiB THPs
# inside the process without sudo (51.2 -> 39.3 ms on RX 7900 XTX).
userptr_lib="${VICTORY_USERPTR_LIB-$port/tools/libdrm-amdgpu-shared-userptr}"
thp_lib="${VICTORY_USERPTR_THP_LIB-$port/tools/userptr-thp/libuserptr-thp.so}"
preload=""
if [[ ${VICTORY_PORT_CONSTANTS:-0} == 1 && ${USERPTR_THP:-1} != 0 && -f $thp_lib ]]; then
    preload=$thp_lib
fi
if [[ -n $userptr_lib && -f $userptr_lib/libdrm_amdgpu.so.1 ]]; then
    export LD_LIBRARY_PATH="$userptr_lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
elif [[ ${VICTORY_PORT_CONSTANTS:-0} == 1 ]]; then
    echo "renderer port 1 needs tools/libdrm-amdgpu-shared-userptr (make-libdrm-shared-userptr.py)" >&2
    exit 4
fi
if [[ -n $preload ]]; then
    export LD_PRELOAD="$preload${LD_PRELOAD:+:$LD_PRELOAD}"
fi
# Renderer port 1 is off by default from 2026-11-27 (NOTES 172): with it the
# card reads the title's constants in place, 5 FPS faster, but the picture
# flickers, a white square sits in the top-left corner and parts of the figure
# go wrong in some starts. VICTORY_PORT_CONSTANTS=1 turns it on again.
export VICTORY_PORT_CONSTANTS="${VICTORY_PORT_CONSTANTS:-0}"
export VICTORY_FRAME_INTERVAL_MS="${VICTORY_FRAME_INTERVAL_MS-16.667}"
# Reflections bounded softly by the light around: no white glaze on glossy
# floors, the sheen stays.
export VICTORY_AMBIENT_REFLECT="${VICTORY_AMBIENT_REFLECT-1.0}"
# Reflections held right after CS_ResolveScreenSpace too (NOTES 171): the
# floor's white glaze and the noise on the phone's screen. Needs a binary
# from 2026-11-27 12:10 on (victory.new2.keep); older ones ignore it.
export VICTORY_REFLECT_SCREEN="${VICTORY_REFLECT_SCREEN-1}"
# Port 2 misses descriptor stores on this title build (10,903 records found
# late by the fallback in harbor-x-play-4; glossy surfaces flickered): off.
export VICTORY_PORT_DESCRIPTORS="${VICTORY_PORT_DESCRIPTORS:-0}"
# Port 5 changes the translation: it needs a warm seed of its own.
export VICTORY_PORT_CBV="${VICTORY_PORT_CBV:-0}"
export VICTORY_PORT_GUARD="${VICTORY_PORT_GUARD:-1}"
export VICTORY_PORT_JOURNAL="${VICTORY_PORT_JOURNAL:-1}"
# Glass: the rays prepared for see-through surfaces find nothing without the
# structures and left sparkling noise; their four images are set to zero, and
# the sky-probe fallback that then lights the glass is bounded (ambient.rs).
export VICTORY_CLEAR_AFTER="${VICTORY_CLEAR_AFTER-CS_RayPrePass_Alpha:1:112;CS_RayPrePass_Alpha:1:120;CS_RayPrePass_Alpha:1:128;CS_RayPrePass_Alpha:1:136;CS_RefractionRayEstimation:1:40;CS_RefractionRayEstimationBlur:1:48}"
# The six shadow-ray programs hung in the compiler for 180 s before the
# single-latch change (kroeeee-eeee f5c30ef) and were on warm-bad.txt; the
# seed's cache still holds their old translations, so they are translated
# again each start and the list is left unread (harbor-x-shadowrays: all six
# pipelines made).
export VICTORY_RETRANSLATE="${VICTORY_RETRANSLATE-559d9f0184e15fe3,32ed072b9f845048,a1b20bc47e3c9597,e8c962358d862e3f,17829ac71aab4e8d,afeea48bbb9913ad}"
# (The list is read again: all six running broke the phone's picture in
# harbor-x-play-11 while small shadows blinked less. Which of them a run
# makes is set by the seed's warm-bad.txt.)
# kroeeee-eeee's cache of uncommitted regions (write watch 8.2 -> 1.8 ms a
# frame) still misses phone UI commits in this tree (stage1-absent-thp); off.
export VICTORY_ABSENT_CACHE="${VICTORY_ABSENT_CACHE-0}"
# Creases: full strength turned knuckles, fingers and elbows black up close.
export VICTORY_AMBIENT_SHADE="${VICTORY_AMBIENT_SHADE-0.4,0.3}"
# beloooo's cap on streamed textures is for 16 GB cards (1024 px by default):
# uncapped on this 24 GB card unless asked for.
export VICTORY_TEXTURE_MAX="${VICTORY_TEXTURE_MAX-0}"
export VICTORY_COMPACT_VIEW_HEAP="${VICTORY_COMPACT_VIEW_HEAP:-1}"
export VICTORY_DYNAMIC_SLOTS="${VICTORY_DYNAMIC_SLOTS:-1048576}"
# avatarmanz 631400f/3e04ca0/15c3d0e clamp in-place indirect arguments alone,
# right before they run (against palms and trees missing for runs of frames
# with port 1): 53.1 ms against 37.2 ms here (fix1-a, off-order; copying back
# 32.6 against 19.4 ms). Off unless asked for.
export VICTORY_PORT_CONSTANTS_ORDER="${VICTORY_PORT_CONSTANTS_ORDER:-0}"
# Mesa keeps at most 1 GB of compiled pipelines by default; the title's
# 85,933 fill it (1.1 GB found on 2026-11-27) and then evict each other, so
# warmed pipelines were compiled again in the world.
export MESA_SHADER_CACHE_MAX_SIZE="${MESA_SHADER_CACHE_MAX_SIZE:-10G}"
# VICTORY_OVERRIDES="A=1 B=2": set last, over everything above (for A/B legs
# of switches this launcher fixes).
if [[ -n ${VICTORY_OVERRIDES:-} ]]; then
    for kv in $VICTORY_OVERRIDES; do export "$kv"; done
    echo "overrides: $VICTORY_OVERRIDES" >&2
fi
if [[ $# -ne 0 ]] && ! [[ $# -eq 2 && $1 == --seconds && $2 =~ ^[0-9]+$ ]]; then
    echo "Usage: $0 [--seconds N]" >&2
    exit 2
fi
mkdir -p @SCRATCH@
[[ ${SMOKE_SUPERVISOR_LOCK:-0} == 1 ]]
if pgrep -af '[v]ictory.exe run'; then exit 3; fi
(cd "$here" && sha256sum -c SHA256SUMS)
if [[ ! -d "$VICTORY_RUNTIME" ]]; then
    staging=$(mktemp -d "$profile/.lean-player-XXXXXXXX")
    cp -a --reflink=always "$save_base/"{prefix,data} "$seed/"{cache,vkd3d-cache} "$staging/"
    install -m 644 "$profile/team-diagnostics/server-index-ab/d3dcompiler_47.dll" "$staging/prefix/pfx/drive_c/windows/system32/d3dcompiler_47.dll"
    mv -T -- "$staging" "$VICTORY_RUNTIME"
fi
"$port/cgbj/run-victory-proton.sh" "$port/../title-work" --console "${HARBOR_CONSOLE:-series-x}" --release "$@"
