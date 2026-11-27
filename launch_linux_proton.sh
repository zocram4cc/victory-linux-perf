#!/usr/bin/env bash
# ============================================================================
#  RAZDA4A XEN — Linux (Proton / Wine / VKD3D) Direct Launcher
#  Skips PowerShell/WinForms GUI and invokes RAZDA4A_XEN.exe directly.
#
#  Usage:
#    GAME_DIR="/path/to/unpacked" GAME_EXE="/path/to/unpacked/your.exe" ./launch_linux_proton.sh
#  Optional env overrides:
#    XEN_PROFILE=safe|balanced|low_commit  (default: safe for 8 GB VRAM compatibility)
#    XEN_FRAMEGEN=0|1                      (default: 0 for VKD3D stability)
#    RUNNER="wine64" or "umu-run" or "/path/to/proton run"
# ============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
XEN_PROFILE="${XEN_PROFILE:-safe}"
XEN_FRAMEGEN="${XEN_FRAMEGEN:-0}"
RUNNER="${RUNNER:-wine64}"

if [[ -z "${GAME_DIR:-}" || -z "${GAME_EXE:-}" ]]; then
  echo "[!] Set GAME_DIR and GAME_EXE before running:"
  echo "    GAME_DIR=\"/path/to/folder\" GAME_EXE=\"/path/to/folder/your_executable.exe\" ./launch_linux_proton.sh"
  exit 1
fi

mkdir -p "$ROOT/cache" "$ROOT/data"

export VICTORY_GAME_DIR="$GAME_DIR"
export VICTORY_CACHE_DIR="$ROOT/cache"
export VICTORY_DATA_DIR="$ROOT/data"
export VICTORY_FIDELITYFX_DIR="$ROOT/fidelityfx"
export VICTORY_LOG="info"

export VICTORY_FSR_PC=1
export VICTORY_AMBIENT=0.75
export VICTORY_AMBIENT_AIR=1.5
export VICTORY_AMBIENT_REFLECT=1.0
export VICTORY_SCALAR_SPLITS=0

export VICTORY_FIDELITYFX_FRAMEGEN="$XEN_FRAMEGEN"
export VICTORY_FRAMEGEN_AHEAD=1
export VICTORY_FRAMEGEN_HUDLESS=1
export VICTORY_ALLOW_TEARING=0
export VICTORY_VSYNC=0
export VICTORY_FRAME_INTERVAL_MS=16.667

export VICTORY_COMPUTE_WAIT_MS=5000
export VICTORY_COMPUTE_LATE_WAIT_MS=400
export VICTORY_HELPER_PRIORITY=0
export VICTORY_SUBMIT_FRAMES=1
export VICTORY_CARD_STEPS=2
export VICTORY_PLACED_ORDER=1
export VICTORY_FETCH_CHECK=1
export VICTORY_TEXTURE_NO_WAIT=1
export VICTORY_TEXTURE_LOOK_MS=350
export VICTORY_RELAY=2
export VICTORY_WIDE_BACK=1
export VICTORY_WIDE_SPIN=100
export VICTORY_CARD_REHASH_RUNS=1
export VICTORY_JOIN_COPIES=1
export VICTORY_JOIN_BARRIERS=1
export VICTORY_UP_QUEUE=1
export VICTORY_INDIRECT_WINDOW=0
export VICTORY_RELAY_COPY=1
export VICTORY_INDIRECT_BATCH=1
export VICTORY_MAP_THREADS=1
export VICTORY_CARD_AWAKE=1
export VICTORY_SHOW_AFTER=1
export VICTORY_KEEP_SHOWN=2
export VICTORY_TEXTURE_AWAY_MS=10000
export VICTORY_THIN=1
export VICTORY_THIN_THREADS=4
export VICTORY_LAND_THREADS=3
export VICTORY_LAND_SKIP=1
export VICTORY_JOIN_PARTS=12
export VICTORY_UPLOAD_AHEAD=1
export VICTORY_HOLES_ONCE=30
export VICTORY_FILE_CACHE=1
export VICTORY_WC_NO_BACK=1
export VICTORY_CLOSED_PLAN=1
export VICTORY_PORT_JOURNAL=1
export VICTORY_PORT_GUARD=1
export VICTORY_PORT_DESCRIPTORS=0
export VICTORY_SWEEP_ASYNC=1

export VICTORY_LIBRARY=1
export VICTORY_LIBRARY_STORE_LATER=1
export VICTORY_LIBRARY_SAVE_S=60
export VICTORY_PIPELINE_BOOK=1
export VICTORY_PIPELINE_WORKERS=2

export VICTORY_RESIDENCY=1
export VICTORY_SEGMENT_TIGHT=85
export VICTORY_WAIT_RESIDENT=textures
export VICTORY_KEEP_FREED=0
export VICTORY_LIST_IDLE_MOST=256
export VICTORY_LIST_IDLE_TRIM_S=30
export VICTORY_LATE_WAIT_LISTS=128
export VICTORY_BURY_LAG=4
export VICTORY_SNAPSHOT_POOL=24
export VICTORY_MEMORY_MAP=0
export VICTORY_FILE_PHYSICAL=0
export VICTORY_STAGING_BAR=0
export VICTORY_SEND_BAR=0
export VICTORY_SKIP_PIPELINES=240cb516c1af0bc3

if [[ "$XEN_PROFILE" == "safe" ]]; then
  export VICTORY_TEXTURE_KEEP_PERCENT=40
  export VICTORY_TEXTURE_STAGE_MB=64
  export VICTORY_STAGING_MAX_MB=96
  export VICTORY_WINDOW_BUDGET_MB=96
  export VICTORY_CARD_BUFFERS_MIN_KB=256
  export VICTORY_CARD_BUFFERS_MB=1280
  export VICTORY_CARD_ROOM_FOR_TARGETS_MB=1536
elif [[ "$XEN_PROFILE" == "low_commit" ]]; then
  export VICTORY_FILE_PHYSICAL=1
fi

export VICTORY_KBM_PC=1
export VICTORY_PANEL=1
export VICTORY_PANEL_POS=top-right

echo "Launching RAZDA4A XEN (profile=$XEN_PROFILE, FG=$XEN_FRAMEGEN, runner=$RUNNER)..."
cd "$ROOT"
# shellcheck disable=SC2086
exec $RUNNER "$ROOT/RAZDA4A_XEN.exe" run --console series-s "$GAME_EXE" --execute
