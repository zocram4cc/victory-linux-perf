#!/usr/bin/env bash
# Repeated unattended runs of one configuration, each classified, for
# stability questions ("does this switch change the device-loss rate?").
#
#   scripts/trials.sh -n 5 -l LABEL [-p PROFILE] [-s SECS] [-o OUTPUT] [VAR=value ...]
#
# Each run is `bench.sh run --auto` (default profile royale-save, the open
# world, 20 s window). Every run ends as one of:
#   lost     the log says the host device was removed (0x887A0005)
#   world    the world came up and the window was measured
#   loading  the world did not come up in time
#   closed   the game closed for another reason
# With -o (a monitor, e.g. DP-1) screenshots are taken 8 s into the window
# and near its end, to check the picture (white or washed-out frames are a known symptom).
#
# Results: bench/trials-<date>-LABEL/{trials.tsv,run-N.out,shot-N.png}, and a
# one-line tally at the end. Like bench.sh, never while someone is playing.
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
BENCH=$ROOT/scripts/bench.sh
OUT_ROOT=${BENCH_DIR:-$ROOT/bench}
DATA=${VP_DATA:-$HOME/Games/victory-data}
LAUNCHER=${VP_LAUNCHER:-victory-proton}

die() { printf '\e[1;31mtrials:\e[0m %s\n' "$*" >&2; exit 1; }
say() { printf '\e[1;36mtrials:\e[0m %s\n' "$*" >&2; }
game_pid() { pgrep -f '^Z:.*(release|diag).victory\.exe run' | head -n1; }

N=5 LABEL= PROFILE=royale-save SECS=20 OUTPUT= EXTRA=()
while (($#)); do
    case $1 in
        -n) N=$2; shift ;;
        -l) LABEL=$2; shift ;;
        -p) PROFILE=$2; shift ;;
        -s) SECS=$2; shift ;;
        -o) OUTPUT=$2; shift ;;
        *=*) EXTRA+=("$1") ;;
        *) die "unknown option: $1" ;;
    esac
    shift
done
[[ $LABEL =~ ^[A-Za-z0-9._-]+$ ]] || die "usage: trials.sh -n N -l LABEL [-p PROFILE] [-s SECS] [-o OUTPUT] [VAR=value ...]"
[[ -z $(game_pid) ]] || die "the game is running; close it first"

dir=$OUT_ROOT/trials-$(date +%Y%m%d-%H%M%S)-$LABEL
mkdir -p "$dir"
printf 'run\tresult\tfps\tms_per_frame\tinvalid\theap_sync\tlog\n' >"$dir/trials.tsv"
declare -A tally=()

for ((i = 1; i <= N; i++)); do
    say "run $i of $N: $LABEL${EXTRA[*]:+ (${EXTRA[*]})}"
    out=$dir/run-$i.out
    "$BENCH" run -p "$PROFILE" -l "$LABEL-$i" --auto -s "$SECS" ${EXTRA[@]+"${EXTRA[@]}"} >"$out" 2>&1 &
    bench=$!
    shot=0
    while kill -0 "$bench" 2>/dev/null; do
        if [[ -n $OUTPUT ]] && ((!shot)) && grep -q 'measuring' "$out"; then
            sleep 8
            grim -o "$OUTPUT" "$dir/shot-$i.png" 2>/dev/null || true
            shot=1
            # And one near the window's end, to see whether the scene changed.
            ( sleep $((SECS > 12 ? SECS - 12 : 1)); grim -o "$OUTPUT" "$dir/shot-$i-end.png" 2>/dev/null ) &
        fi
        sleep 2
    done
    wait "$bench"; rc=$?
    # bench.sh leaves the game open when it gives up waiting.
    [[ -n $(game_pid) ]] && { "$LAUNCHER" kill >/dev/null 2>&1 || true; sleep 5; }
    log=$(readlink -f "$DATA/perfiles/$PROFILE/logs/latest.log")
    clean=$(sed 's/\x1b\[[0-9;]*m//g' "$log" 2>/dev/null)
    # The layer's log names the loss when it sees it first; otherwise the
    # driver's fault is only in the launcher's output (stderr).
    if grep -q '887A0005' <<<"$clean" || grep -qs 'GPUVM fault\|VK_ERROR_DEVICE_LOST' "$OUT_ROOT/.launcher.out"; then result=lost
    elif ((rc == 0)); then result=world
    elif grep -q 'did not come up' "$out"; then result=loading
    else result=closed
    fi
    cp "$OUT_ROOT/.launcher.out" "$dir/launcher-$i.out" 2>/dev/null || true
    heap=$(grep -o 'per frame [0-9.]* changed heap pages, [0-9]* slots looked at, [0-9.]* whose record was new' <<<"$clean" | tail -n1)
    res=$(grep -o "$OUT_ROOT/[^ ]*-$LABEL-$i" "$out" | tail -n1)
    read -r fps ms invalid < <(python3 -I - "${res:-/nonexistent}/summary.json" <<'PY'
import json, sys
try:
    s = json.load(open(sys.argv[1]))
    c = s.get("columns", {})
    print(f'{c.get("fps", {}).get("mean", 0):.2f} {c.get("ms_per_frame", {}).get("mean", 0):.0f} {"yes" if s.get("invalid") else "no"}')
except Exception:
    print("- - -")
PY
)
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$i" "$result" "$fps" "$ms" "$invalid" "${heap:--}" "$log" >>"$dir/trials.tsv"
    tally[$result]=$((${tally[$result]:-0} + 1))
    say "run $i: $result${heap:+ ($heap)}"
done

line="$LABEL:"; for k in world lost loading closed; do line+=" $k ${tally[$k]:-0}"; done
echo "$line" | tee "$dir/tally.txt"
column -t -s $'\t' "$dir/trials.tsv" | cut -c1-200
say "results: $dir"
