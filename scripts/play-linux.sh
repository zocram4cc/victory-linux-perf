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
#   --new-game      start at the opening scene: the story save and player record are
#                   moved to ~/.local/share/victory/save-backups/<time> first (settings stay),
#                   then ~/.local/share/victory/save-templates/new-game is copied in (a player
#                   record from just after the terms of service were accepted)
#   --levels-on-demand  texture mip levels as draws need them
#                   (VICTORY_LEVELS_ON_DEMAND=1; off by default since it used more memory)
#   --no-texture-waits  the title's shorter waits for textures in the first frames and
#                   the first cutscene (VICTORY_EARLY_TEXTURES_MS=12, VICTORY_BAKE_TEXTURES_MS=1500)
#   --stock-wineserver  Proton-GE Latest as it is, even where the view-tree copy is
#                   installed (by default the launcher uses that copy when it is there:
#                   ~/.local/share/victory/proton/GE-Proton11-14-viewtree, GE-Proton11-14
#                   with GE PR #1016's wineserver, memory views kept in a tree)
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
profile=release safe=0 bench="" proton_log=0 mangohud=0 stock_wineserver=0 new_game=0 args=()
while (( $# )); do
  case "$1" in
    --diag) profile=diag ;;
    --safe) safe=1 ;;
    --ray-tracing) export VICTORY_BVH_BUILDER="${VICTORY_BVH_BUILDER:-1}" ;;
    --bench) shift; bench="${1:?--bench needs a route name}" ;;
    --proton-log) proton_log=1 ;;
    --new-game) new_game=1 ;;
    --levels-on-demand) export VICTORY_LEVELS_ON_DEMAND="${VICTORY_LEVELS_ON_DEMAND:-1}" ;;
    --no-texture-waits)
      export VICTORY_EARLY_TEXTURES_MS="${VICTORY_EARLY_TEXTURES_MS:-12}"
      export VICTORY_BAKE_TEXTURES_MS="${VICTORY_BAKE_TEXTURES_MS:-1500}" ;;
    --skip-intro) export VICTORY_KEYS="${VICTORY_KEYS-12:Enter,16:Enter,20:A,25:Enter,30:A,35:Enter,40:A,50:Enter}" ;;
    --stock-wineserver) stock_wineserver=1 ;;
    --view-tree) ;; # the default where the copy is installed
    --mangohud) mangohud=1 ;;
    --no-mangohud) mangohud=0 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d'; exit 0 ;;
    *) args+=("$1") ;;
  esac
  shift
done
set -- "${args[@]+"${args[@]}"}"

command -v umu-run >/dev/null || { echo "umu-run not found (README: install the umu-launcher zipapp)" >&2; exit 2; }
# GE-Proton11-14 with GE PR #1016's wineserver where it is installed: the
# title maps memory views by the hundred thousand, and the stock wineserver
# looks each one up in a list. Over the same 305 s of the intro (runs
# 20261126-221648 against -223848) frames went from 7.7 to 22.2 a second on
# average, 2349 to 6801 drawn, and the card's idle time a frame from
# 67-200 ms to 9-28 ms. How the copy is built: VIEW-TREE-NOTES.txt in it.
view_tree="$HOME/.local/share/victory/proton/GE-Proton11-14-viewtree"
if [[ -z "${PROTONPATH:-}" ]] && (( ! stock_wineserver )) && [[ -x "$view_tree/files/bin/wineserver" ]]; then
  export PROTONPATH="$view_tree"
fi
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
# Every texture's mip levels as the title loads it. On demand (1) used more
# memory, stuttered more and left Jason's skin black in the first cutscene
# (runs 20261126-220716 against -221207: title peak 23.0 -> 21.0 GB, swap
# 11.2 -> 9.7 GB, 99th percentile frame 802 -> 578 ms); the willy build's
# setting. --levels-on-demand turns it back on.
d VICTORY_LEVELS_ON_DEMAND 0
# Longer waits for textures in the first frames and the first cutscene (the
# willy build's): with them run 20261126-221648 reached 305 s of the intro
# with clothes and faces lit right and a lower memory peak than without
# (-221207). --no-texture-waits: the shorter waits.
d VICTORY_EARLY_TEXTURES_MS 40
d VICTORY_BAKE_TEXTURES_MS 10000
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

# The title holds 19-21 GB in the world on a 30 GB PC; with less than that
# free it swaps, whatever else is set. Other programs held 12 GB and 9 GB of
# swap at the start of the 2026-11-26 runs.
avail_gb=$(awk '/^MemAvailable:/ {printf "%d", $2 / 1048576}' /proc/meminfo)
if (( avail_gb < 20 )); then
  echo "note: ${avail_gb} GB of RAM is free; the title wants about 20 GB in the world, so closing other programs first cuts its swapping and stutter." >&2
fi

# The title fills about 15 GB of a 16 GB card. A program holding GPU memory
# of its own makes the driver page the title's: a dictation model's 4.9 GB
# took the opening from 24 to 3 frames a second (runs 20261126-225925,
# -231332), the card busy all the time.
if command -v nvidia-smi >/dev/null; then
  others=$(timeout 10 nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits 2>/dev/null \
    | awk -F', ' '$3 >= 1024 {printf "  %s (pid %s): %d MB\n", $2, $1, $3}')
  if [[ -n "$others" ]]; then
    printf 'warning: other programs hold GPU memory the title needs; quit them for full speed:\n%s\n' "$others" >&2
  fi
fi

# The title's shaders translated, and the pipelines earlier runs drew made,
# before the first frame: once per build and set of VICTORY_* switches. On a
# cold cache a first run otherwise drew a black screen for minutes while
# they were made on demand. About 70 s for the whole set the first time
# (and after a translator change), a few seconds when they are cached.
# VICTORY_PREBUILD=0 skips it; the log is in the cache folder.
if [[ "${VICTORY_PREBUILD:-1}" != 0 ]]; then
  switches=$(env | grep -E '^VICTORY_' | grep -vE '^VICTORY_(KEYS|FRAMES_CSV|RUNS_DIR|PREBUILD)=' | sort)
  stamp="$(sha256sum "$exe" | cut -c1-16) $(printf '%s' "$switches" | sha256sum | cut -c1-16)"
  stamp_file="$cache/prebuilt"
  if [[ "$(cat "$stamp_file" 2>/dev/null)" != "$stamp" ]]; then
    log="$cache/prebuild.log"
    echo "New build or switches: translating the title's shaders first (about 70 s the first time; log: $log)..." >&2
    if umu-run "$win_exe" shaders "$(winpath "$game/shaders.rpf")" > "$log" 2>&1; then
      sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -E 'shaders in the cache, .*translate' >&2 || true
      if [[ -s "$cache/pipelines/early-pipelines.txt" ]]; then
        echo "making the pipelines earlier runs drew..." >&2
        VICTORY_LOG=warn umu-run "$win_exe" warm --used >> "$log" 2>&1 || true
        sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -E ' made in ' | tail -1 >&2 || true
      fi
      printf '%s\n' "$stamp" > "$stamp_file"
    else
      echo "warning: translating the shaders failed (see $log); the run translates them as it goes" >&2
    fi
  fi
fi

# --new-game: the title's story saves (SARC*) and player records (Player~*)
# moved aside, never deleted, so every run starts at the opening scene and
# runs can be compared frame for frame. Settings stay.
if (( new_game )); then
  backup="$HOME/.local/share/victory/save-backups/$(date +%Y%m%d-%H%M%S)"
  moved=0
  while IFS= read -r -d '' save; do
    rel="${save#"$data"/}"
    mkdir -p "$backup/$(dirname "$rel")"
    mv "$save" "$backup/$rel" && moved=$((moved + 1))
  done < <(find "$data" -mindepth 5 -maxdepth 5 -path '*/saves/*' \( -name 'SARC*' -o -name 'Player~*' \) -print0)
  (( moved )) && echo "new game: $moved saves moved to $backup" >&2 || echo "new game: no saves to move" >&2
  # A player record from just after the terms of service were accepted, so
  # a new game does not ask again.
  template="$HOME/.local/share/victory/save-templates/new-game"
  if [[ -d "$template" ]]; then
    cp -a "$template/." "$data/" && echo "new game: player record from $template" >&2
  fi
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
  > >(python3 -I -u -c 'import re, signal, sys, time
# Runs until the title closes its output: Ctrl-C and a closed terminal are
# for the script above, which stops the title.
signal.signal(signal.SIGINT, signal.SIG_IGN)
signal.signal(signal.SIGHUP, signal.SIG_IGN)
colour = re.compile(r"\x1b\[[0-9;]*m")
screen = True
with open(sys.argv[1], "a", buffering=1, errors="replace") as log:
    for line in sys.stdin:
        if screen:
            try:
                sys.stdout.write(line)
            except OSError:
                screen = False  # the terminal is gone: the log goes on
        log.write(time.strftime("%H:%M:%S ") + colour.sub("", line))' "$run/victory.log") 2>&1 &
title=$!
"$repo/scripts/victory-sampler.sh" "$run" $$ "$title" &
sampler=$!

stopping=0
stop() {
  (( stopping )) && return
  stopping=1
  echo "stopping the title (SIGTERM)..." >&2 || true
  # The title's Wine process first (found below umu-run, never by a name
  # match across the system), then umu's own group; never SIGKILL.
  local wine
  wine=$("$repo/scripts/victory-title-pid.sh" "$title") && kill -TERM "$wine" 2>/dev/null || true
  kill -TERM -- -"$title" 2>/dev/null || true
}
# A closed terminal (SIGHUP) stops the title as Ctrl-C does, and the run's
# end is still written: output goes nowhere from then on (a write to the
# closed terminal would end this script under set -e before the end).
trap stop INT TERM
trap 'exec >/dev/null 2>&1; stop' HUP
if (( bench_secs > 0 )); then
  ( sleep "$bench_secs"; kill -TERM $$ 2>/dev/null ) &
  timer=$!
fi

code=0
while kill -0 "$title" 2>/dev/null; do
  wait "$title" && code=0 || code=$?
done
trap - INT TERM HUP
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
echo "run finished (exit $code, $(( $(date +%s) - started )) s): $run" || true
echo "summary: scripts/victory-report.py $run" || true
exit "$code"
