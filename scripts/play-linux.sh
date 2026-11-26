#!/usr/bin/env bash
# Linux launcher: runs the Windows build under Proton-GE through umu-launcher,
# with the same settings as Play-VIctory.cmd (pareto-residency, Series S).
#
#   VICTORY_GAME_DIR=~/Games/MyDump scripts/play-linux.sh [pareto-residency] [options]
#   scripts/play-linux.sh [--diag] shaders <the shader archives>   # translate every shader ahead of time
#   scripts/play-linux.sh [--diag] warm --used                     # make pipelines earlier runs drew with
#   scripts/play-linux.sh [--diag] exe <args...>                   # any other victory.exe command
#
# Options for a game run:
#   --diag          the diagnostics build (scripts/build-linux.sh --diag)
#   --safe          without the newest, faster ways (scripts/play.ps1 -Safe)
#   --ray-tracing   the title's ray-traced indirect light
#   --bench <route> a scripted run: keys from scripts/bench/<route>.keys, stopped when it ends
#   --skip-intro    press through the boot screens and logos (meeyao's timings, same
#                   game build): Enter/A at 12-50 s, so the run reaches the load sooner
#   --proton-log    Wine's own log into the run folder (large)
#   --no-levels-on-demand  every texture's mip levels as the title loads it
#                   (VICTORY_LEVELS_ON_DEMAND=0, as the willy build runs; for a memory A/B)
#   --texture-waits wait longer for textures in the first frames and the first
#                   cutscene (VICTORY_EARLY_TEXTURES_MS=40, VICTORY_BAKE_TEXTURES_MS=10000,
#                   the willy build's; against faces of noise)
#   --mangohud      also MangoHud's frame log (off by default: its NVIDIA thread
#                   aborted the title under umu, `free(): invalid pointer`)
#
# Every game run leaves a folder in ~/.local/share/victory/runs/ (VICTORY_RUNS_DIR):
# run.json, victory.log, frames.csv, system.csv, marks.csv, marks/ (mangohud.csv with --mangohud).
# scripts/victory-report.py summarises one; scripts/victory-mark.sh marks a moment.
# Ctrl-C stops the title cleanly (SIGTERM, never SIGKILL: killing it with its
# mappings live can wedge the NVIDIA driver until a reboot).
#
# Every VICTORY_* default below can be overridden from the environment.
# Runner settings (overridable):
#   PROTONPATH      Proton build to use (default: Proton-GE Latest from Steam's compatibilitytools.d)
#   WINEPREFIX      prefix for VIctory alone (default: ~/.local/share/victory/prefix)
#   VICTORY_EXE     victory.exe to run (default: the cross-built build, then a release exe beside this repo)
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="$repo/target/x86_64-pc-windows-msvc"

# Linux path -> path Wine sees (its Z: drive is the Linux root).
winpath() { local p; p="$(realpath -m -- "$1")"; printf 'Z:%s' "${p//\//\\}"; }
usage() { sed -n '2,8p' "$0" >&2; exit 2; }

# Options that apply to every command, taken out first.
profile=release safe=0 bench="" proton_log=0 mangohud=0 args=()
while (( $# )); do
  case "$1" in
    --diag) profile=diag ;;
    --safe) safe=1 ;;
    --ray-tracing) export VICTORY_BVH_BUILDER="${VICTORY_BVH_BUILDER:-1}" ;;
    --bench) shift; bench="${1:?--bench needs a route name}" ;;
    --proton-log) proton_log=1 ;;
    --no-levels-on-demand) export VICTORY_LEVELS_ON_DEMAND="${VICTORY_LEVELS_ON_DEMAND:-0}" ;;
    --texture-waits)
      export VICTORY_EARLY_TEXTURES_MS="${VICTORY_EARLY_TEXTURES_MS:-40}"
      export VICTORY_BAKE_TEXTURES_MS="${VICTORY_BAKE_TEXTURES_MS:-10000}" ;;
    --skip-intro) export VICTORY_KEYS="${VICTORY_KEYS-12:Enter,16:Enter,20:A,25:Enter,30:A,35:Enter,40:A,50:Enter}" ;;
    --mangohud) mangohud=1 ;;
    --no-mangohud) mangohud=0 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d'; exit 0 ;;
    *) args+=("$1") ;;
  esac
  shift
done
set -- "${args[@]+"${args[@]}"}"

command -v umu-run >/dev/null || { echo "umu-run not found (README: install the umu-launcher zipapp)" >&2; exit 2; }
export PROTONPATH="${PROTONPATH:-$HOME/.local/share/Steam/compatibilitytools.d/Proton-GE Latest}"
export WINEPREFIX="${WINEPREFIX:-$HOME/.local/share/victory/prefix}"
export GAMEID="${GAMEID:-umu-0}"
[[ -d "$PROTONPATH" ]] || { echo "No Proton at $PROTONPATH; set PROTONPATH" >&2; exit 2; }
mkdir -p "$WINEPREFIX"

# vkd3d-proton older than v3.1-83 (GE-Proton11-12 and before) converts DXIL
# with dxil-spirv's legacy structurizer, which turned ~6% of the title's
# pipelines into invalid SPIR-V that NVIDIA's compiler crashed on (the title
# died as it loaded). GE-Proton11-13 and later have the rewritten one.
d3d12core="$PROTONPATH/files/lib/wine/vkd3d-proton/x86_64-windows/d3d12core.dll"
if command -v strings >/dev/null && [[ -f "$d3d12core" ]]; then
  vkd3d_version=$(strings "$d3d12core" | grep -m1 -oE '^v[0-9]+\.[0-9]+(\.[0-9]+)?(-[0-9]+)?-g[0-9a-f]+' || true)
  if [[ "$vkd3d_version" =~ ^v([0-9]+)\.([0-9]+)(\.[0-9]+)?(-([0-9]+))? ]]; then
    major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} since=${BASH_REMATCH[5]:-0}
    if (( major < 3 || (major == 3 && minor < 1) || (major == 3 && minor == 1 && since < 83) )); then
      echo "warning: $PROTONPATH has vkd3d-proton $vkd3d_version, older than v3.1-83: its legacy" \
        "DXIL structurizer crashes NVIDIA's shader compiler on this title. Use GE-Proton11-13 or later." >&2
    fi
  fi
fi

exe="${VICTORY_EXE:-}"
if [[ -z "$exe" ]]; then
  for c in "$target/$profile/victory.exe" \
           "$repo/victory-3d-rtx-pareto-residency.exe" \
           "$repo/target/release/victory.exe"; do
    [[ -f "$c" ]] && { exe="$c"; break; }
  done
fi
[[ -n "$exe" && -f "$exe" ]] || {
  echo "No victory.exe for the $profile build. Build it: scripts/build-linux.sh$([[ $profile == diag ]] && echo ' --diag')" >&2
  exit 2
}

# Caches and data (saves, settings) live on the Linux side, not inside the
# prefix, so a fresh prefix keeps them.
cache="${VICTORY_CACHE_DIR:-$HOME/.cache/victory/cache-pareto}"
data="${VICTORY_DATA_DIR:-$HOME/.local/share/victory/data}"
mkdir -p "$cache" "$data"
export VICTORY_CACHE_DIR="$(winpath "$cache")"
export VICTORY_DATA_DIR="$(winpath "$data")"
if [[ -n "${VICTORY_CONFIG:-}" ]]; then export VICTORY_CONFIG="$(winpath "$VICTORY_CONFIG")"; fi

# Microsoft's DXC (dxcompiler.dll, dxil.dll) beside the exe: under Wine our
# shaders only compile with it (no 3D without). Copied in from
# VICTORY_DXC_DIR when missing, so a rebuild keeps working.
dxc_dir="${VICTORY_DXC_DIR:-$HOME/.local/share/victory/dxc}"
for dll in dxcompiler.dll dxil.dll; do
  [[ -f "$(dirname "$exe")/$dll" ]] || { [[ -f "$dxc_dir/$dll" ]] && cp -f "$dxc_dir/$dll" "$(dirname "$exe")/"; } || true
done
[[ -f "$(dirname "$exe")/dxcompiler.dll" ]] || echo "warning: no dxcompiler.dll beside $exe or in $dxc_dir;" \
  "get dxcompiler.dll and dxil.dll from Microsoft's DirectXShaderCompiler release (bin/x64)." >&2

# vkd3d-proton settings from meeyao/victory-8gb-linux (tested on NVIDIA under Proton).
export VKD3D_CONFIG="${VKD3D_CONFIG-descriptor_heap}"
export VKD3D_DISABLE_EXTENSIONS="${VKD3D_DISABLE_EXTENSIONS-VK_NV_raw_access_chains}"
export VKD3D_SHADER_CACHE_PATH="${VKD3D_SHADER_CACHE_PATH:-$HOME/.cache/victory/vkd3d}"
mkdir -p "$VKD3D_SHADER_CACHE_PATH"

cd "$repo"   # victory.toml is looked for in the working directory

# The exe goes in as a Wine path: given a Linux path, Proton starts it through
# its umu.exe wrapper and its output never reaches this terminal. (umu warns
# it cannot find the file; Proton finds it.) Linux paths in the arguments
# work as they are, since the working directory is on Wine's Z: drive.
win_exe="$(winpath "$exe")"

case "${1:-}" in
  shaders|warm) exec umu-run "$win_exe" "$@" ;;
  exe) shift; exec umu-run "$win_exe" "$@" ;;
esac
for a in "$@"; do
  [[ "$a" == pareto-residency ]] || usage
done

game="${VICTORY_GAME_DIR:-}"
[[ -n "$game" && -f "$game/game_scarlett_master.exe" ]] || {
  echo "Set VICTORY_GAME_DIR to your own dump folder containing game_scarlett_master.exe." >&2
  exit 2
}

# The same defaults as Play-VIctory.cmd.
d() { [[ -n "${!1:-}" ]] || export "$1=$2"; }
d VICTORY_AMBIENT_AIR 1.5
d VICTORY_TEXTURE_NO_WAIT 1
d VICTORY_RELAY 2
d VICTORY_WIDE_BACK 1
d VICTORY_CARD_REHASH_RUNS 1
d VICTORY_JOIN_COPIES 1
d VICTORY_INDIRECT_WINDOW 1
d VICTORY_RELAY_COPY 1
d VICTORY_CARD_STEPS 2
d VICTORY_INDIRECT_BATCH 2
d VICTORY_MAP_THREADS 1
d VICTORY_CARD_AWAKE 1
d VICTORY_SHOW_AFTER 1
d VICTORY_AMBIENT 0.8
if (( safe )); then
  # scripts/play.ps1 -Safe: without the newer, faster ways.
  d VICTORY_UNDER_WIDE 0
  d VICTORY_LAND_ECHO 0
else
  d VICTORY_THIN 1
  d VICTORY_JOIN_PARTS 8
  d VICTORY_UPLOAD_AHEAD 1
fi
d VICTORY_LEVELS_ON_DEMAND 1
d VICTORY_CARD_BUFFERS_MIN_KB 64
# Both faces: under Proton character heads and hair are culled away otherwise.
d VICTORY_CULL_NONE 1
# Game memory in the card's memory through Resizable BAR (Zarina35's RTX 5080
# numbers; 0 turns either off). vkd3d-proton has the GPU upload heap this needs.
d VICTORY_WINDOWS_BAR_MB 2048
d VICTORY_CARD_WINDOWS_MB 1024
# Remember the pipelines a run used, so `warm --used` can make them ahead.
d VICTORY_PIPELINE_BOOK 1
[[ -n "${VICTORY_LOG:-}${RUST_LOG:-}" ]] || export VICTORY_LOG=warn,victory_gpu=info

# A scripted run: the route's keys, and how long it lasts.
bench_secs=0
if [[ -n "$bench" ]]; then
  route="$repo/scripts/bench/$bench.keys"
  [[ -f "$route" ]] || { echo "No route $route" >&2; exit 2; }
  keys=$(grep -vE '^[[:space:]]*(#|$)' "$route" | grep -v '^duration=' | tr -d ' ' | paste -sd, -)
  bench_secs=$(sed -n 's/^duration=\([0-9]*\).*/\1/p' "$route" | head -1)
  [[ -n "$bench_secs" ]] || { echo "$route has no duration=<seconds> line" >&2; exit 2; }
  export VICTORY_KEYS="$keys"
fi

# The run's folder.
runs="${VICTORY_RUNS_DIR:-$HOME/.local/share/victory/runs}"
run="$runs/$(date +%Y%m%d-%H%M%S)${bench:+-bench-$bench}"
mkdir -p "$run/marks"
ln -sfn "$run" "$runs/latest"
: > "$run/marks.csv"
export VICTORY_FRAMES_CSV="$(winpath "$run/frames.csv")"
if (( proton_log )); then export PROTON_LOG=1 PROTON_LOG_DIR="$run"; fi
if (( mangohud )) && command -v mangohud >/dev/null; then
  export MANGOHUD=1
  export MANGOHUD_CONFIG="${MANGOHUD_CONFIG:-no_display,output_folder=$run,autostart_log=1,log_interval=0,toggle_hud=Shift_L+F12,toggle_logging=Shift_L+F2}"
fi
# (No gamemoderun: its preload cannot load inside umu's Steam Runtime
# container and only adds errors there.)

started=$(date +%s)
python3 -I "$repo/scripts/victory-report.py" --write-run-json "$run" --start \
  --profile "$profile" --exe "$exe" --repo "$repo" ${bench:+--bench "$bench"} || true

# The title in a process group of its own, so that Ctrl-C here reaches this
# script only, which then asks the title to end.
echo "run folder: $run"
setsid umu-run "$win_exe" run "$(winpath "$game/game_scarlett_master.exe")" --console series-s --execute \
  > >(python3 -I -u -c 'import re, sys, time
colour = re.compile(r"\x1b\[[0-9;]*m")
with open(sys.argv[1], "a", buffering=1, errors="replace") as log:
    for line in sys.stdin:
        sys.stdout.write(line)
        log.write(time.strftime("%H:%M:%S ") + colour.sub("", line))' "$run/victory.log") 2>&1 &
title=$!
"$repo/scripts/victory-sampler.sh" "$run" $$ "$title" &
sampler=$!

stopping=0
stop() {
  (( stopping )) && return
  stopping=1
  echo "stopping the title (SIGTERM)..." >&2
  # The title's Wine process first (found below umu-run, never by a name
  # match across the system), then umu's own group; never SIGKILL.
  local wine
  wine=$("$repo/scripts/victory-title-pid.sh" "$title") && kill -TERM "$wine" 2>/dev/null || true
  kill -TERM -- -"$title" 2>/dev/null || true
}
trap stop INT TERM
if (( bench_secs > 0 )); then
  ( sleep "$bench_secs"; kill -TERM $$ 2>/dev/null ) &
  timer=$!
fi

code=0
while kill -0 "$title" 2>/dev/null; do
  wait "$title" && code=0 || code=$?
done
trap - INT TERM
[[ -n "${timer:-}" ]] && kill "$timer" 2>/dev/null || true
kill "$sampler" 2>/dev/null || true
wait 2>/dev/null || true

# MangoHud names its files after the program; give them fixed names.
for f in "$run"/*_summary.csv; do [[ -e "$f" ]] && mv -f "$f" "$run/mangohud-summary.csv"; done
for f in "$run"/*.csv; do
  case "$(basename "$f")" in frames.csv|system.csv|marks.csv|mangohud*.csv) ;; *) mv -f "$f" "$run/mangohud.csv" ;; esac
done
python3 -I "$repo/scripts/victory-report.py" --write-run-json "$run" --end --exit-code "$code" \
  --seconds $(( $(date +%s) - started )) || true
echo "run finished (exit $code, $(( $(date +%s) - started )) s): $run"
echo "summary: scripts/victory-report.py $run"
exit "$code"
