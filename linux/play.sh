#!/usr/bin/env bash
# play.sh [name] [VAR=value ...]: starts the game with no time limit and no
# scripted keys, for playing into a scene by hand. The log goes to
# $STATE_DIR/bench/<name>.log (default name: play); close the game yourself.
set -uo pipefail
name=${1:-play}; (($#)) && shift
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$ROOT/linux/local.env" ]] && source "$ROOT/linux/local.env"
OUT="${STATE_DIR:-$HOME/.local/share/victory-linux}/bench"
mkdir -p "$OUT"
exec env "$@" PROFILE="${PROFILE:-diag}" VICTORY_LOG_FILE="$OUT/$name.log" \
    PROTON_LOG_FILE="$OUT/$name.proton.log" "$ROOT/linux/run.sh"
