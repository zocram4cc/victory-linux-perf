#!/usr/bin/env bash
# P.4: finds, by halves, the first 0x218 call whose program breaks the
# picture (VICTORY_NATIVE_COMPUTE_MAX). A run is broken when its median fps
# is above 30: the scene empties (~4,000 draws a frame, ~40 fps) where a good
# one runs at ~17. Usage: scripts/p4-bisect.sh [LOW] [HIGH]
set -uo pipefail
cd "$(dirname "$0")/.."
lo=${1:-0} hi=${2:-2700}   # lo: known good (0 = none made), hi: known broken
while ((hi - lo > 1)); do
    mid=$(((lo + hi) / 2))
    out=$(scripts/ktrials.sh -n 1 -l "p4-bisect-$mid" -s 15 VICTORY_NATIVE_COMPUTE=1 "VICTORY_NATIVE_COMPUTE_MAX=$mid" 2>&1)
    fps=$(awk -F'\t' 'NR==2 {print $5}' "$(ls -d bench/k-*"p4-bisect-$mid" | tail -1)/runs.tsv")
    if [[ $fps == - || -z $fps ]]; then echo "max=$mid: no measurement, stopping"; break; fi
    if awk -v f="$fps" 'BEGIN {exit !(f > 30)}'; then hi=$mid; verdict=broken; else lo=$mid; verdict=good; fi
    echo "max=$mid fps=$fps $verdict  -> [$lo, $hi]"
done
echo "first breaking call: $hi"
