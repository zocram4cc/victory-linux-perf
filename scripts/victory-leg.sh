#!/usr/bin/env bash
# One labelled leg, headless and unattended: a scripted route from a new game
# with screenshots, then its victory-legs.py show line and its blink summary.
#
#   scripts/victory-leg.sh [--shots <from>-<to>[,<from>-<to>...]] [--diag] <label> <route> [NAME=value ...]
#
#   --shots   seconds since the run started to take gamescope screenshots in
#             (as fast as they come, about 7 a second), into <run>/shots/,
#             named by the milliseconds since the start (default 300-330)
#   --diag    the diagnostics build
#   NAME=value  switches for this leg (VICTORY_*, VKD3D_*, ...)
#
# The leg runs scripts/play-linux.sh --headless --bench <route> --new-game
# with VICTORY_DATA_DIR at a data folder of its own (default
# ~/.local/share/victory/data-auto; never the player's data folder, which it
# refuses). It writes <run>/leg.json (label, route, switches, the exe and its
# sha256, the shots asked for) and <run>/leg.out (the launcher's output).
# Every wait is bounded; at the end or on Ctrl-C it sends SIGTERM to the
# launcher it started and nothing else, never SIGKILL. Leg method after the
# breakwater fork's.
#
# The functions below can be sourced for tests (scripts/tests/).
{
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
share="$HOME/.local/share/victory"

die() { echo "victory-leg: $*" >&2; exit 2; }

# "300-330,400-410" -> lines "300 330" and "400 410"; fails on anything else.
parse_shots() {
  local spec=$1 part from to
  local -a parts
  [[ "$spec" =~ ^[0-9]+-[0-9]+(,[0-9]+-[0-9]+)*$ ]] || return 1
  IFS=, read -ra parts <<< "$spec"
  (( ${#parts[@]} )) || return 1
  for part in "${parts[@]}"; do
    [[ "$part" =~ ^([0-9]+)-([0-9]+)$ ]] || return 1
    from=${BASH_REMATCH[1]} to=${BASH_REMATCH[2]}
    (( 10#$from < 10#$to )) || return 1
    echo "$((10#$from)) $((10#$to))"
  done
}

# The data folder, or a failure when it is the player's own.
data_dir() {
  local dir player
  dir=$(realpath -m -- "${1:-$share/data-auto}")
  player=$(realpath -m -- "$share/data")
  [[ "$dir" != "$player" ]] || return 1
  echo "$dir"
}

# Waits (at most $2 s) for "run folder: <path>" in the launcher's output $1.
wait_for_run_folder() {
  local out=$1 limit=$2 waited=0 line
  while (( waited < limit )); do
    line=$(grep -m1 '^run folder: ' "$out" 2>/dev/null) && { echo "${line#run folder: }"; return 0; }
    [[ -n "${launcher:-}" ]] && ! kill -0 "$launcher" 2>/dev/null && return 1
    sleep 1; waited=$((waited + 1))
  done
  return 1
}

# Waits (at most $2 s) until <run>/run.json holds started_epoch; prints it.
wait_for_start() {
  local run=$1 limit=$2 waited=0 epoch
  while (( waited < limit )); do
    epoch=$(python3 -I -c 'import json, sys
try:
    print(json.load(open(sys.argv[1]))["started_epoch"])
except (OSError, ValueError, KeyError):
    sys.exit(1)' "$run/run.json" 2>/dev/null) && { echo "$epoch"; return 0; }
    sleep 1; waited=$((waited + 1))
  done
  return 1
}

# Counts the screenshots in $1 that are all one colour (black included); prints
# "<flat> <all>" and the flat ones' names. Needs PIL.
flat_shots() {
  python3 -I - "$1" <<'PY'
import sys
from pathlib import Path
try:
    from PIL import Image
except ImportError:
    print("? ?\nno PIL: the black test was not made"); sys.exit(0)
shots = sorted(Path(sys.argv[1]).glob("*.png"))
flat = []
for p in shots:
    try:
        with Image.open(p) as im:
            lo_hi = im.convert("RGB").reduce(8).getextrema()
    except OSError:
        flat.append(p.name + " (unreadable)"); continue
    if all(hi - lo < 6 for lo, hi in lo_hi):
        flat.append(p.name)
print(len(flat), len(shots))
for name in flat:
    print(name)
PY
}

# The gamescope socket that was not there before ($1: the earlier list).
new_gamescope() {
  local before=$1 s
  for s in "${XDG_RUNTIME_DIR:-/run/user/$UID}"/gamescope-*; do
    [[ -S "$s" && "$s" != *.lock ]] || continue
    grep -qxF "$(basename "$s")" <<< "$before" || { basename "$s"; return 0; }
  done
  return 1
}

gamescopes() {
  local s
  for s in "${XDG_RUNTIME_DIR:-/run/user/$UID}"/gamescope-*; do [[ -S "$s" ]] && basename "$s"; done
  return 0
}

# Takes screenshots while the seconds since $2 (epoch) are in the ranges $3,
# into $1, while the launcher lives; display $4.
take_shots() {
  local dir=$1 epoch=$2 ranges=$3 display=$4 from to now ms start_ms
  start_ms=$(python3 -I -c 'import sys; print(int(float(sys.argv[1]) * 1000))' "$epoch")
  mkdir -p "$dir"
  while read -r from to; do
    while kill -0 "$launcher" 2>/dev/null; do
      now=$(( $(date +%s%3N) - start_ms ))
      (( now >= to * 1000 )) && break
      if (( now < from * 1000 )); then sleep 0.5; continue; fi
      printf -v ms '%07d' "$now"
      GAMESCOPE_WAYLAND_DISPLAY=$display timeout 5 gamescopectl screenshot "$dir/$ms.png" >/dev/null 2>&1 \
        || echo "screenshot at $ms ms failed" >&2
    done
  done <<< "$ranges"
}

stop_launcher() {
  kill -0 "${launcher:-0}" 2>/dev/null || return 0
  echo "victory-leg: SIGTERM to the launcher ($launcher)" >&2
  kill -TERM "$launcher" 2>/dev/null
}

# Waits at most $1 s for the launcher to end.
wait_launcher() {
  local limit=$1 waited=0
  while kill -0 "$launcher" 2>/dev/null && (( waited < limit )); do sleep 1; waited=$((waited + 1)); done
  ! kill -0 "$launcher" 2>/dev/null
}

main() {
  local shots_spec=300-330 diag=() label route
  while (( $# )); do
    case "$1" in
      --shots) shots_spec=${2:?--shots needs <from>-<to>[,...]}; shift ;;
      --diag) diag=(--diag) ;;
      -h|--help) sed -n '2,/^# The functions below/p' "$0" | sed '$d'; exit 0 ;;
      -*) die "unknown option $1" ;;
      *) break ;;
    esac
    shift
  done
  (( $# >= 2 )) || die "usage: victory-leg.sh [--shots <from>-<to>,...] [--diag] <label> <route> [NAME=value ...]"
  label=$1 route=$2; shift 2
  [[ "$label" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] || die "label: letters, digits, . _ - (at most 64)"
  [[ -f "$here/bench/$route.keys" ]] || die "no route $here/bench/$route.keys"
  local ranges switch data duration
  ranges=$(parse_shots "$shots_spec") || die "--shots: <from>-<to>[,<from>-<to>...] in seconds, from < to"
  for switch in "$@"; do
    [[ "$switch" =~ ^[A-Z_][A-Z0-9_]*=.*$ ]] || die "not a NAME=value switch: $switch"
  done
  data=$(data_dir "${VICTORY_DATA_DIR:-}") || die "VICTORY_DATA_DIR is the player's data folder ($share/data); a leg starts a new game and would move the player's saves"
  duration=$(sed -n 's/^duration=\([0-9]*\).*/\1/p' "$here/bench/$route.keys" | head -1)
  [[ -n "$duration" ]] || die "$route.keys has no duration= line"
  command -v gamescopectl >/dev/null || die "gamescopectl not found"

  local tmp out before
  tmp=$(mktemp -d) && out="$tmp/leg.out" && : > "$out"
  before=$(gamescopes)
  echo "victory-leg: $label, route $route ($duration s), data $data, switches: ${*:-(launcher defaults)}"
  env "$@" VICTORY_DATA_DIR="$data" "$here/play-linux.sh" "${diag[@]}" --headless --bench "$route" --new-game \
    > "$out" 2>&1 < /dev/null &
  launcher=$!
  trap 'stop_launcher; trap - INT TERM' INT TERM

  # The prebuild can take minutes after a new build: up to 15 for the folder.
  local run epoch exe sha display
  run=$(wait_for_run_folder "$out" 900) || { stop_launcher; wait_launcher 120; tail -20 "$out" >&2; die "no run folder"; }
  epoch=$(wait_for_start "$run" 60) || { stop_launcher; wait_launcher 120; die "$run/run.json has no started_epoch after 60 s"; }
  exe=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1])).get("exe", ""))' "$run/run.json")
  sha=$( [[ -f "$exe" ]] && sha256sum "$exe" | cut -d' ' -f1 )
  python3 -I -c 'import json, sys
label, route, shots, exe, sha, data, *switches = sys.argv[2:]
json.dump({"label": label, "route": route, "shots": shots, "exe": exe, "exe_sha256": sha,
           "data_dir": data, "switches": switches}, open(sys.argv[1], "w"), indent=2)' \
    "$run/leg.json" "$label" "$route" "$shots_spec" "$exe" "$sha" "$data" "$@"
  echo "victory-leg: run $run, exe sha256 ${sha:0:16}"

  # The headless gamescope this launcher started (waits up to 120 s for it).
  local waited=0
  until display=$(new_gamescope "$before"); do
    (( waited++ < 120 )) && kill -0 "$launcher" 2>/dev/null || break
    sleep 1
  done
  [[ -n "$display" ]] || { display=gamescope-0; echo "victory-leg: no new gamescope socket; trying $display" >&2; }
  take_shots "$run/shots" "$epoch" "$ranges" "$display"

  # The launcher stops the route itself after $duration s; 5 minutes more
  # at most (the title can take long to give memory back), then SIGTERM.
  wait_launcher $(( duration + 300 )) || { stop_launcher; wait_launcher 180 || echo "victory-leg: the launcher is still running (pid $launcher); not killed" >&2; }
  cp -f "$out" "$run/leg.out"; rm -rf "$tmp"

  local flat nflat nall
  flat=$(flat_shots "$run/shots")
  read -r nflat nall <<< "${flat%%$'\n'*}"
  echo "black test: $nflat of $nall screenshots all one colour $(tail -n +2 <<< "$flat" | head -5 | paste -sd' ')"
  python3 -I "$here/victory-legs.py" show "$run"
  python3 -I "$here/victory-blink.py" "$run/shots" --summary-only || true
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then main "$@"; fi
}
