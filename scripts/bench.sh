#!/usr/bin/env bash
# bench.sh: repeatable measurements of a scene, from Linux, through the
# victory-proton launcher (phase 0.3 of the README).
#
#   scripts/bench.sh run [-l LABEL] [-s SECONDS] [-p PROFILE] [--keep] [--perf] [--auto] [VAR=VALUE ...]
#       Launches the game with VICTORY_FRAMETIME_DUMP=1 (and VAR=VALUE...),
#       waits for Enter once you are in the scene, measures SECONDS (60),
#       and writes bench/<date>-<LABEL>/: frametime.csv (the window),
#       summary.json, meta.json, log.txt (the window's log), and closes
#       the game unless --keep. --perf also records a user-space CPU profile
#       of the game and of wineserver during the window (perf.data,
#       perf-wineserver.data, and perf.txt with the top symbols).
#       --auto needs nobody at the keyboard: it presses A through the
#       start screen (VICTORY_KEYS, unless set), lets the intro play, waits
#       until the world is drawn (more than 3,000 draws a frame for four
#       rows in a row), waits 20 s more, and measures.
#   scripts/bench.sh mark [-l LABEL] [-s SECONDS] [-p PROFILE]
#       The same on a game that is already running (started by `run --keep`
#       or by hand with VICTORY_FRAMETIME_DUMP=1).
#   scripts/bench.sh summary DIR       recompute DIR/summary.json
#   scripts/bench.sh compare DIR_A DIR_B [...]
#       a table of the main figures, with the change of each against A
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
OUT_ROOT=${BENCH_DIR:-$ROOT/bench}
DATA=${VP_DATA:-$HOME/Games/victory-data}
LAUNCHER=${VP_LAUNCHER:-victory-proton}

die() { printf '\e[1;31mbench:\e[0m %s\n' "$*" >&2; exit 1; }
say() { printf '\e[1;36mbench:\e[0m %s\n' "$*" >&2; }

game_pid() { pgrep -f '^Z:.*(release|diag).victory\.exe run' | head -n1; }
# The wineserver the game uses: the process whose executable is a
# wineserver (Proton's, or the patched *-viewtree build).
server_pid() {
    local p exe
    for p in $(pgrep -f 'wineserver|viewtree'); do
        exe=$(readlink "/proc/$p/exe" 2>/dev/null) || continue
        [[ $exe == */wineserver || $exe == *-viewtree ]] && { echo "$p"; return; }
    done
}
# VICTORY_*/VKD3D_*/RADV_* the game runs with, from its own environment.
game_env() {
    [[ -r /proc/$1/environ ]] || return 0
    tr '\0' '\n' <"/proc/$1/environ" | grep -E '^(VICTORY_|VKD3D_|RADV_|WINEFSYNC|DXVK_)' || true
}
# utime + stime of a process, in clock ticks.
ticks() { [[ -n ${1:-} && -r /proc/$1/stat ]] || { echo 0; return; }; awk '{print $14 + $15}' "/proc/$1/stat"; }

# -------------------------------------------------------------- summary
summary() {
    local dir=$1
    [[ -f $dir/frametime.csv ]] || die "no frametime.csv in $dir"
    python3 -I - "$dir" <<'PY'
import csv, json, os, statistics, sys
d = sys.argv[1]
rows = [r for r in csv.DictReader(l for l in open(os.path.join(d, "frametime.csv")) if not l.startswith("#"))]
if not rows:
    sys.exit("bench: the window has no rows (was the game in the scene, and long enough?)")
def num(v):
    try: return float(v)
    except (TypeError, ValueError): return None
cols = {}
for k in rows[0]:
    vals = [x for x in (num(r[k]) for r in rows) if x is not None]
    if not vals or k in ("t_ms", "frame"):
        continue
    s = sorted(vals)
    pick = lambda q: s[min(len(s) - 1, int(round(q * (len(s) - 1))))]
    cols[k] = {"mean": statistics.fmean(vals), "p50": pick(0.5), "p95": pick(0.95), "max": s[-1], "min": s[0]}
out = {"rows": len(rows), "columns": cols}
# A window whose scene changed (a pause menu, a load) does not count.
dr = cols.get("draws")
if dr and dr["min"] < 0.5 * dr["p50"]:
    out["invalid"] = f"draws fell to {dr['min']:.0f} (median {dr['p50']:.0f}): the scene changed during the window"
    print(f"WARNING: {out['invalid']}")
meta = os.path.join(d, "meta.json")
if os.path.exists(meta):
    m = json.load(open(meta))
    hz = os.sysconf("SC_CLK_TCK")
    secs = m.get("seconds_measured") or 0
    if secs:
        for k in ("game", "wineserver"):
            t = m.get(f"{k}_ticks")
            if t is not None:
                out[f"{k}_cores"] = t / hz / secs
json.dump(out, open(os.path.join(d, "summary.json"), "w"), indent=1)
c = cols
def g(k, f="mean"): return c.get(k, {}).get(f)
line = lambda name, k, unit="": print(f"  {name:<22} {g(k):10.1f}{unit}   p95 {g(k,'p95'):8.1f}") if g(k) is not None else None
print(f"{d}: {len(rows)} rows")
for name, k, u in [("fps", "fps", ""), ("ms per frame", "ms_per_frame", " ms"), ("worst frame", "frame_ms_max", " ms"),
                   ("prepare (CPU)", "cpu_prepare_ms", " ms"), ("GPU", "gpu_ms", " ms"), ("GPU busy", "gpu_pct", " %"),
                   ("copies", "copies_ms", " ms"), ("copies MB", "copies_mb", " MB"), ("textures", "textures_ms", " ms"),
                   ("copy-back", "copyback_ms", " ms"), ("copy-back MB", "copyback_mb", " MB"),
                   ("hash passes", "card_passes", ""), ("process CPU", "cpu_pct", " %")]:
    line(name, k, u)
for k in ("game_cores", "wineserver_cores"):
    if k in out: print(f"  {k:<22} {out[k]:10.2f} cores")
PY
}

# -------------------------------------------------------------- compare
compare() {
    (($# >= 2)) || die "usage: bench.sh compare DIR_A DIR_B [...]"
    local d; for d in "$@"; do [[ -f $d/summary.json ]] || summary "$d" >/dev/null; done
    python3 -I - "$@" <<'PY'
import json, os, sys
dirs = sys.argv[1:]
S = [json.load(open(os.path.join(d, "summary.json"))) for d in dirs]
keys = ["fps", "ms_per_frame", "frame_ms_max", "cpu_prepare_ms", "gpu_ms", "gpu_pct", "copies_ms", "copies_mb",
        "textures_ms", "copyback_ms", "copyback_mb", "card_passes", "cpu_pct"]
names = [os.path.basename(os.path.normpath(d)) for d in dirs]
print("| measure | " + " | ".join(names) + " |")
print("|---" * (len(dirs) + 1) + "|")
for k in keys + ["game_cores", "wineserver_cores"]:
    vals = [(s["columns"].get(k, {}).get("mean") if k in s.get("columns", {}) else s.get(k)) for s in S]
    if all(v is None for v in vals): continue
    a = vals[0]
    cells = []
    for i, v in enumerate(vals):
        if v is None: cells.append("-"); continue
        txt = f"{v:.1f}" if abs(v) >= 1 or v == 0 else f"{v:.2f}"
        if i and a: txt += f" ({(v - a) / a * 100:+.0f} %)"
        cells.append(txt)
    print(f"| {k} | " + " | ".join(cells) + " |")
PY
}

# -------------------------------------------------------------- measure
# Measures SECONDS on the running game; $dir is made here.
measure() {
    local profile=$1 label=$2 secs=$3 dir=$4
    local pdata=$DATA/perfiles/$profile csv log pid srv
    csv=$pdata/cache/frametime.csv
    pid=$(game_pid); [[ -n $pid ]] || die "the game is not running"
    [[ -f $csv ]] || die "no $csv yet: is VICTORY_FRAMETIME_DUMP=1 set, and has the game presented a frame?"
    log=$(readlink -f "$pdata/logs/latest.log")
    if ((AUTO)); then
        wait_world "$csv" "$pid"
    elif [[ -t 0 ]]; then
        say "get into the scene, then press Enter here (the window is ${secs} s)"
        read -r _
    fi
    local csv0 log0 g0 s0 t0
    csv0=$(wc -l <"$csv"); log0=$(stat -c %s "$log")
    srv=$(server_pid); g0=$(ticks "$pid"); s0=$(ticks "$srv"); t0=$(date +%s.%N)
    say "measuring ${secs} s..."
    mkdir -p "$dir"
    local perfs=()
    if ((PERF)); then
        # bubblewrap's user namespace keeps a profiler outside it from
        # attaching: enter it first. User space only (perf_event_paranoid=2).
        # Small buffers: the game has 40+ threads, and the default ones for
        # each go past perf_event_mlock_kb (the first profile came out empty).
        nsenter -t "$pid" -U --preserve-credentials perf record -q -m 4 -F 999 -e cycles:u -p "$pid" -o "$dir/perf.data" -- sleep "$secs" >"$dir/perf.err" 2>&1 &
        perfs+=($!)
        if [[ -n $srv ]]; then
            nsenter -t "$srv" -U --preserve-credentials perf record -q -m 4 -F 999 -e cycles:u -p "$srv" -o "$dir/perf-wineserver.data" -- sleep "$secs" >>"$dir/perf.err" 2>&1 &
            perfs+=($!)
        fi
    fi
    sleep "$secs"
    local p; for p in ${perfs[@]+"${perfs[@]}"}; do wait "$p" 2>/dev/null || true; done
    local g1 s1 t1; g1=$(ticks "$pid"); s1=$(ticks "$srv"); t1=$(date +%s.%N)
    game_env "$pid" >"$OUT_ROOT/.env"
    game_pid >/dev/null || die "the game closed during the window"
    mkdir -p "$dir"
    { grep '^#' "$csv" || true; head -n "$(grep -n '^t_ms' "$csv" | head -n1 | cut -d: -f1)" "$csv" | tail -n1; tail -n +"$((csv0 + 1))" "$csv"; } >"$dir/frametime.csv"
    tail -c +"$((log0 + 1))" "$log" | sed 's/\x1b\[[0-9;]*m//g' >"$dir/log.txt"
    python3 -I - "$dir/meta.json" <<PY
import json, os, sys
json.dump({
  "label": "$label", "profile": "$profile", "seconds_measured": $t1 - $t0,
  "commit": "$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null)",
  "dirty": $( [[ -n $(git -C "$ROOT" status --porcelain 2>/dev/null) ]] && echo True || echo False ),
  "switches": dict(l.rstrip("\\n").split("=", 1) for l in open("$OUT_ROOT/.env") if "=" in l),
  "game_ticks": $g1 - $g0, "wineserver_ticks": $s1 - $s0,
  "log": "$log",
}, open(sys.argv[1], "w"), indent=1)
PY
    summary "$dir"
    if ((PERF)) && [[ -f $dir/perf.data ]]; then
        {
            echo "== game, by module"
            perf report -q -i "$dir/perf.data" --stdio --sort dso 2>/dev/null | head -25
            echo; echo "== game, by symbol"
            perf report -q -i "$dir/perf.data" --stdio --sort dso,sym 2>/dev/null | head -60
            if [[ -f $dir/perf-wineserver.data ]]; then
                echo; echo "== wineserver, by symbol"
                perf report -q -i "$dir/perf-wineserver.data" --stdio --sort sym 2>/dev/null | head -25
            fi
        } >"$dir/perf.txt"
        say "profile: $dir/perf.txt"
    fi
}

# Waits until the frametime dump shows the world (4 rows in a row of more
# than 3,000 draws a frame), then 20 s more for it to settle.
wait_world() {
    local csv=$1 pid=$2 i
    say "waiting for the world (the intro plays through; up to 6 minutes)"
    for ((i = 0; i < 360; i++)); do
        [[ -n $(game_pid) ]] || die "the game closed before reaching the world"
        if python3 -I - "$csv" <<'PY'
import csv, sys
rows = [r for r in csv.DictReader(l for l in open(sys.argv[1]) if not l.startswith("#"))]
last = rows[-4:]
sys.exit(0 if len(last) == 4 and all(float(r["draws"]) > 3000 for r in last) else 1)
PY
        then
            say "in the world; settling for 20 s"
            sleep 20
            return
        fi
        sleep 1
    done
    die "the world did not come up in 6 minutes"
}

parse_opts() {
    LABEL=run SECS=60 PROFILE=${VP_PERFIL:-royale} KEEP=0 PERF=0 AUTO=0 EXTRA=()
    while (($#)); do
        case $1 in
            -l) LABEL=$2; shift ;;
            -s) SECS=$2; shift ;;
            -p) PROFILE=$2; shift ;;
            --keep) KEEP=1 ;;
            --perf) PERF=1 ;;
            --auto) AUTO=1 ;;
            *=*) EXTRA+=("$1") ;;
            *) die "unknown option: $1" ;;
        esac
        shift
    done
    [[ $LABEL =~ ^[A-Za-z0-9._-]+$ ]] || die "a label may only have letters, digits, '.', '_' and '-'"
}

run() {
    parse_opts "$@"
    [[ -z $(game_pid) ]] || die "the game is running already; close it (victory-proton kill) or use: bench.sh mark"
    local dir; dir=$OUT_ROOT/$(date +%Y%m%d-%H%M%S)-$LABEL
    local e; for e in ${EXTRA[@]+"${EXTRA[@]}"}; do export "${e?}"; done
    export VICTORY_FRAMETIME_DUMP=1
    # Unattended: press A (Enter) through the start screen, and nothing
    # after (the intro plays through; A in the world would act).
    if ((AUTO)) && [[ -z ${VICTORY_KEYS:-} ]]; then
        # Taps through the start screen only. Holding A through the intro
        # did not skip it (the world came at ~103 s either way) and changed
        # the scene measured afterwards.
        VICTORY_KEYS=$(seq 8 3 50 | sed 's/$/:Enter/' | paste -sd,)
        export VICTORY_KEYS
    fi
    rm -f "$DATA/perfiles/$PROFILE/cache/frametime.csv"
    mkdir -p "$OUT_ROOT"
    say "launching profile $PROFILE${EXTRA[*]:+ with ${EXTRA[*]}}"
    "$LAUNCHER" -p "$PROFILE" run --no-update >"$OUT_ROOT/.launcher.out" 2>&1 &
    local launcher=$!
    local i; for ((i = 0; i < 120; i++)); do [[ -n $(game_pid) ]] && break; sleep 1; done
    [[ -n $(game_pid) ]] || { tail -n 20 "$OUT_ROOT/.launcher.out" >&2; die "the game did not start"; }
    for ((i = 0; i < 600; i++)); do [[ -f $DATA/perfiles/$PROFILE/cache/frametime.csv ]] && break; sleep 1; done
    measure "$PROFILE" "$LABEL" "$SECS" "$dir"
    cp "$OUT_ROOT/.launcher.out" "$dir/launcher.out" 2>/dev/null || true
    if ((KEEP)); then say "the game keeps running (bench.sh mark measures again)"
    else "$LAUNCHER" kill >/dev/null 2>&1 || true; wait "$launcher" 2>/dev/null || true; fi
    say "results: $dir"
}

mark() {
    parse_opts "$@"
    measure "$PROFILE" "$LABEL" "$SECS" "$OUT_ROOT/$(date +%Y%m%d-%H%M%S)-$LABEL"
}

cmd=${1:-help}; shift || true
case $cmd in
    run) run "$@" ;;
    mark) mark "$@" ;;
    summary) summary "${1:?a results folder}" ;;
    compare) compare "$@" ;;
    *) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; [[ $cmd == help ]] ;;
esac
