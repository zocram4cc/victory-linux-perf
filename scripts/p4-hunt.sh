#!/usr/bin/env bash
# P.4: finds every 0x218 program that breaks the picture, one after another.
# Each round: a run with all of them made except those found so far; if the
# scene holds (K_MIN_DRAWS, default 7,500 draws a frame), done. Otherwise the
# first breaking call is found by halves (VICTORY_NATIVE_COMPUTE_MAX), its
# code address read from the log (VICTORY_NATIVE_COMPUTE_LOG), and it joins
# the skip list. Found programs go to bench/p4-bad.txt (one address a line,
# kept between hunts).
#
#   scripts/p4-hunt.sh [ROUNDS]     (default 6)
set -uo pipefail
cd "$(dirname "$0")/.."
BAD=bench/p4-bad.txt
touch "$BAD"
rounds=${1:-6}
say() { printf '\e[1;35mp4-hunt:\e[0m %s\n' "$*"; }
# Draws a frame (second lowest of the last four lines) of run LABEL, and its log.
draws_of() {
    local d log
    d=$(ls -d bench/k-*"$1" 2>/dev/null | tail -1)
    log=$(awk -F'\t' 'NR==2 {print $13}' "$d/runs.tsv" 2>/dev/null)
    sed 's/\x1b\[[0-9;]*m//g' "$log" 2>/dev/null | grep -oE '\), [0-9]+ draws/frame' | tail -4 | grep -oE '[0-9]+' | sort -n | sed -n 2p
}
log_of() { awk -F'\t' 'NR==2 {print $13}' "$(ls -d bench/k-*"$1" | tail -1)/runs.tsv"; }
run() { # LABEL MAX
    scripts/ktrials.sh -n 1 -l "$1" -s 15 VICTORY_NATIVE_COMPUTE=1 "VICTORY_NATIVE_COMPUTE_MAX=$2" \
        "VICTORY_NATIVE_COMPUTE_SKIP=$(paste -sd, "$BAD")" VICTORY_NATIVE_COMPUTE_LOG=0-100000 >/dev/null 2>&1
}
lo=0
for ((round = 1; round <= rounds; round++)); do
    tag=r$round-$(wc -l <"$BAD")
    run "p4-hunt-$tag-all" 100000
    all=$(draws_of "p4-hunt-$tag-all")
    say "round $round: skipping $(wc -l <"$BAD"), all others made: draws=${all:-?}"
    [[ -z $all ]] && { say "no measurement; stopping"; break; }
    ((all >= ${K_MIN_DRAWS:-7500})) && { say "the scene holds with every other program: done"; break; }
    hi=2800
    while ((hi - lo > 1)); do
        mid=$(((lo + hi) / 2))
        run "p4-hunt-$tag-$mid" "$mid"
        d=$(draws_of "p4-hunt-$tag-$mid")
        [[ -z $d ]] && { say "no measurement at $mid; stopping"; exit 1; }
        if ((d < ${K_MIN_DRAWS:-7500})); then hi=$mid; else lo=$mid; fi
        say "  max=$mid draws=$d -> [$lo, $hi]"
    done
    addr=$(sed 's/\x1b\[[0-9;]*m//g' "$(log_of "p4-hunt-$tag-$hi")" | grep -oE "P\.4: 0x218 call call=$hi code=[0-9a-f]+" | grep -oE '[0-9a-f]+$' | head -1)
    [[ -z $addr ]] && { say "no address for call $hi; stopping"; break; }
    echo "$addr" >>"$BAD"
    say "round $round: call $hi breaks it: program $addr (now $(wc -l <"$BAD") left out)"
    lo=$((hi - 1))
done
say "programs left out: $(paste -sd, "$BAD")"
