#!/usr/bin/env bash
# Unattended measured runs of this tree (victory-royale, branch k7900).
#
#   scripts/ktrials.sh -n RUNS -l LABEL [-s SECS] [-w SETTLE] [VAR=value ...]
#
# Each run: starts the game with ~/Games/victory-data/run-kroeeee.sh (this
# tree's linux/victory-proton.sh settings; K_LAUNCH= another launcher), taps
# Enter through the start screen (VICTORY_KEYS), waits for the world (three
# frame lines in a row with more than 3,000 draws), settles SETTLE s (default
# 10), takes the frame lines of SECS s (default 40), screenshots the game's
# window only, and closes the game. Results: bench/k-<date>-<label>/:
# runs.tsv (medians per run), run-N.log (the window's lines), run-N.png.
#
# It refuses to start while a game is running (the user may be playing).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
LAUNCH=${K_LAUNCH:-$HOME/Games/victory-data/run-kroeeee.sh}
LOGS=${K_LOGS:-$HOME/Games/victory-data/perfiles/kroeeee/logs}
RUNS=1 LABEL=run SECS=40 SETTLE=10 VARS=()
while (($#)); do
    case $1 in
        -n) RUNS=$2; shift ;;
        -l) LABEL=$2; shift ;;
        -s) SECS=$2; shift ;;
        -w) SETTLE=$2; shift ;;
        *=*) VARS+=("$1") ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
say() { printf '\e[1;36mktrials:\e[0m %s\n' "$*" >&2; }
running() { ps -eo args | grep -q '[r]elease\\victory.exe run'; }
close_game() {
    local i
    for sig in TERM KILL; do
        running || return 0
        # By pid: under Wine the process's name is not victory.exe (pkill -x
        # missed it and left the game open, 26 Nov).
        ps -eo pid,args | grep '[r]elease\\victory.exe run' | awk '{print $1}' | xargs -r kill -s "$sig" 2>/dev/null || true
        for ((i = 0; i < 10; i++)); do running || return 0; sleep 1; done
    done
}
shoot() {
    local g
    g=$(hyprctl clients -j 2>/dev/null | python3 -c 'import json, sys
c = [c for c in json.load(sys.stdin) if "VIctory" in c.get("title", "") and c.get("class") == "steam_proton"]
if c:
    x, y = c[0]["at"]; w, h = c[0]["size"]; x2 = max(x, 0)
    print("%d,%d %dx%d" % (x2, y, w - (x2 - x), h))') || true
    [[ -n $g ]] && grim -g "$g" "$1" 2>/dev/null || true
}
running && { echo "the game is running: close it first" >&2; exit 1; }
OUT=$ROOT/bench/k-$(date +%Y%m%d-%H%M%S)-$LABEL
mkdir -p "$OUT"
printf 'run\tresult\tentry_s\tlines\tfps\tms\tprep\tcopies\tgpu\tback\twrite_watch\tdropped\tlog\n' >"$OUT/runs.tsv"
keys=$(seq 8 3 50 | sed 's/$/:Enter/' | paste -sd,)
for ((r = 1; r <= RUNS; r++)); do
    say "run $r of $RUNS${VARS[*]:+ with ${VARS[*]}}"
    before=$(ls -t "$LOGS"/run-*.txt 2>/dev/null | head -n1 || true)
    t0=$(date +%s)
    env VICTORY_KEYS="${VICTORY_KEYS:-$keys}" ${VARS[@]+"${VARS[@]}"} setsid "$LAUNCH" >"$OUT/launch-$r.out" 2>&1 </dev/null &
    log=""
    for ((i = 0; i < 120; i++)); do
        l=$(ls -t "$LOGS"/run-*.txt 2>/dev/null | head -n1 || true)
        [[ -n $l && $l != "$before" ]] && { log=$l; break; }
        sleep 1
    done
    result=ok entry=-
    if [[ -z $log ]]; then result=nolog; fi
    # The world: three frame lines in a row of more than 3,000 draws.
    if [[ $result == ok ]]; then
        result=noworld
        for ((i = 0; i < 480; i++)); do
            running || { result=closed; break; }
            if sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -o '), [0-9]* draws/frame' | tail -n3 | awk '{n++; if ($2+0 > 3000) k++} END {exit !(n == 3 && k == 3)}'; then
                entry=$(($(date +%s) - t0)); result=ok; break
            fi
            sleep 1
        done
    fi
    if [[ $result == ok ]]; then
        say "in the world after ${entry} s; settling ${SETTLE} s, measuring ${SECS} s"
        from0=$(wc -l <"$log")
        # K_EARLY_SHOT=1: a screenshot right at world entry (what streaming in looks like).
        [[ ${K_EARLY_SHOT:-0} == 1 ]] && shoot "$OUT/run-$r-entry.png"
        sleep "$SETTLE"
        from=$(wc -l <"$log")
        # K_PERF=1: a profile of the game and the wineserver over the window,
        # kernel included (needs kernel.perf_event_paranoid <= 1).
        if [[ ${K_PERF:-0} == 1 ]]; then
            gp=$(ps -eo pid,args | grep '[r]elease\\victory.exe run' | awk '$2 !~ /cmd.exe/ {print $1}' | tail -n1)
            sp=$(pgrep -x wineserver | head -n1 || true)
            perf record -q -m 8 -F 499 -g -p "$gp${sp:+,$sp}" -o "$OUT/run-$r.perf" -- sleep "$SECS" >"$OUT/run-$r.perf.err" 2>&1 || true
        else
            sleep "$SECS"
        fi
        shoot "$OUT/run-$r.png"
        # K_SHOTS=N: N more screenshots, 5 s apart, after the window.
        for ((k = 1; k <= ${K_SHOTS:-0}; k++)); do sleep 5; shoot "$OUT/run-$r-$k.png"; done
        running || result=closed
        tail -n +"$((from + 1))" "$log" | sed 's/\x1b\[[0-9;]*m//g' >"$OUT/run-$r.log"
    fi
    close_game
    running && { say "the game is still running after run $r: stopping here"; exit 1; }
    row=$(python3 -I - "$OUT/run-$r.log" <<'PY' 2>/dev/null || echo "-	-	-	-	-	-	-	-	-"
import re, statistics as st, sys
rows = []
try:
    lines = open(sys.argv[1], errors="replace").read().split("\n")
except OSError:
    lines = []
for l in lines:
    m = re.search(r"host: ([0-9.]+) fps, ([0-9.]+) ms/frame \(([0-9.]+) preparing: copies ([0-9.]+) ms.*; ([0-9.]+) on the GPU; ([0-9.]+) copying back.*\), ([0-9]+) draws/frame, dropped ([0-9]+)", l)
    if m:
        rows.append([float(x) for x in m.groups()] + [None])
    m = re.search(r"([0-9.]+) ms asking the write watch", l)
    if m and rows:
        rows[-1][8] = float(m.group(1))
rows = [r for r in rows if r[6] > 2000]
def med(i, f="%.1f"):
    v = [r[i] for r in rows if r[i] is not None]
    return f % st.median(v) if v else "-"
print("\t".join([str(len(rows)), med(0), med(1, "%.0f"), med(2, "%.0f"), med(3, "%.0f"), med(4, "%.0f"), med(5, "%.0f"), med(8), med(7, "%.0f")]))
PY
)
    printf '%s\t%s\t%s\t%s\t%s\n' "$r" "$result" "$entry" "$row" "$log" >>"$OUT/runs.tsv"
done
column -t -s $'\t' "$OUT/runs.tsv" | cut -c1-150
say "results: $OUT"
