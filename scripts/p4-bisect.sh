#!/usr/bin/env bash
# P.4: finds, by halves, the first 0x218 call whose program breaks the
# picture (VICTORY_NATIVE_COMPUTE_MAX). A run is broken when the scene
# empties: fewer than K_MIN_DRAWS (default 7,500) draws a frame (second lowest
# of its last four frame lines), where a good one has ~9,000. (fps was a poor
# test: broken runs went from 29 to 36 fps.) Usage: scripts/p4-bisect.sh [LOW] [HIGH]
set -uo pipefail
# P.4 runs make native pipelines: a cache of their own, so that the pipeline
# library of normal play does not start them early every run (27 Nov).
export K_CACHE=${K_CACHE:-$HOME/Games/victory-data/perfiles/kroeeee/cache-p4}
cd "$(dirname "$0")/.."
lo=${1:-0} hi=${2:-2700}   # lo: known good (0 = none made), hi: known broken
while ((hi - lo > 1)); do
    mid=$(((lo + hi) / 2))
    out=$(scripts/ktrials.sh -n 1 -l "p4-bisect-$mid" -s 15 VICTORY_NATIVE_COMPUTE=1 "VICTORY_NATIVE_COMPUTE_MAX=$mid" 2>&1)
    d=$(ls -d bench/k-*"p4-bisect-$mid" | tail -1)
    log=$(awk -F'\t' 'NR==2 {print $13}' "$d/runs.tsv")
    draws=$(sed 's/\x1b\[[0-9;]*m//g' "$log" 2>/dev/null | grep -oE '\), [0-9]+ draws/frame' | tail -4 | grep -oE '[0-9]+' | sort -n | sed -n 2p)
    if [[ -z $draws ]]; then echo "max=$mid: no measurement, stopping"; break; fi
    if ((draws < ${K_MIN_DRAWS:-7500})); then hi=$mid; verdict=broken; else lo=$mid; verdict=good; fi
    echo "max=$mid draws=$draws $verdict  -> [$lo, $hi]"
done
echo "first breaking call: $hi"
