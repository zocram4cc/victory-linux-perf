#!/usr/bin/env bash
# P.4: finds every 0x218 program that breaks the picture, one after another,
# by halving the list of programs (their code addresses, from the native PSO
# dump; stable between runs, unlike the order of the calls, which differs
# because pipelines are made on several threads).
#
# A run makes only the programs listed in a file (VICTORY_NATIVE_COMPUTE_ONLY)
# and is broken when the scene empties: fewer than K_MIN_DRAWS (default 7,500)
# draws a frame (second lowest of its last four frame lines; a good one has
# ~9,000-11,000). Its fps is noted, not judged (loading skews it).
#
# Each round: all candidates on; if the scene holds, done. Otherwise halve:
# the half that breaks it alone is kept, until one program is left; it goes
# to bench/p4-bad.txt and out of the candidates.
#
#   scripts/p4-hunt.sh [ROUNDS]     (default 6; K_DUMP= the dump directory)
set -uo pipefail
cd "$(dirname "$0")/.."
DUMP=${K_DUMP:-$HOME/Games/victory-data/pso-dump}
BAD=bench/p4-bad.txt
WORK=bench/p4-hunt
mkdir -p "$WORK"
touch "$BAD"
rounds=${1:-6}
say() { printf '\e[1;35mp4-hunt:\e[0m %s\n' "$*"; }
winpath() { local p; p=$(readlink -f "$1"); echo "Z:${p//\//\\}"; }
# Draws a frame of run LABEL (second lowest of the last four lines), and fps.
measure() {
    local d log
    d=$(ls -d bench/k-*"$1" 2>/dev/null | tail -1)
    log=$(awk -F'\t' 'NR==2 {print $13}' "$d/runs.tsv" 2>/dev/null)
    echo "$(sed 's/\x1b\[[0-9;]*m//g' "$log" 2>/dev/null | grep -oE '\), [0-9]+ draws/frame' | tail -4 | grep -oE '[0-9]+' | sort -n | sed -n 2p) $(awk -F'\t' 'NR==2 {print $5}' "$d/runs.tsv" 2>/dev/null)"
}
# Runs with only the addresses in FILE made; prints "draws fps".
try() { # LABEL FILE
    scripts/ktrials.sh -n 1 -l "$1" -s 15 VICTORY_NATIVE_COMPUTE=1 "VICTORY_NATIVE_COMPUTE_ONLY=$(winpath "$2")" >/dev/null 2>&1
    measure "$1"
}
# K_ALL=<file>: the candidates (e.g. the programs that translate, from
# `p4_fspx --ok`); default: every program in the dump.
if [[ -n ${K_ALL:-} ]]; then sort "$K_ALL" >"$WORK/all.txt"; else ls -d "$DUMP"/218_* | sed 's|.*/218_||' | sort >"$WORK/all.txt"; fi
for ((round = 1; round <= rounds; round++)); do
    grep -vxFf "$BAD" "$WORK/all.txt" >"$WORK/cand.txt" || true
    n=$(wc -l <"$WORK/cand.txt")
    read -r draws fps < <(try "p4-hunt-r$round-all" "$WORK/cand.txt")
    say "round $round: $n programs on (skipping $(wc -l <"$BAD")): draws=${draws:-?} fps=${fps:-?}"
    [[ -z $draws ]] && { say "no measurement; stopping"; break; }
    ((draws >= ${K_MIN_DRAWS:-7500})) && { say "the scene holds with all of them: done"; break; }
    cp "$WORK/cand.txt" "$WORK/set.txt"
    step=0
    while (($(wc -l <"$WORK/set.txt") > 1)); do
        step=$((step + 1))
        half=$(($(wc -l <"$WORK/set.txt") / 2))
        head -n "$half" "$WORK/set.txt" >"$WORK/a.txt"
        tail -n +"$((half + 1))" "$WORK/set.txt" >"$WORK/b.txt"
        read -r draws fps < <(try "p4-hunt-r$round-s$step" "$WORK/a.txt")
        [[ -z $draws ]] && { say "no measurement; stopping"; exit 1; }
        if ((draws < ${K_MIN_DRAWS:-7500})); then
            cp "$WORK/a.txt" "$WORK/set.txt"; verdict="first half breaks it"
        else
            cp "$WORK/b.txt" "$WORK/set.txt"; verdict="first half holds"
        fi
        say "  step $step: first $half of $((half + $(wc -l <"$WORK/b.txt"))): draws=$draws fps=$fps, $verdict"
    done
    bad=$(cat "$WORK/set.txt")
    echo "$bad" >>"$BAD"
    say "round $round: program $bad breaks the picture (now $(wc -l <"$BAD") left out)"
done
say "left out: $(paste -sd, "$BAD")"
