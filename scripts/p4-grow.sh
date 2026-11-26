#!/usr/bin/env bash
# P.4: grows a set of 0x218 programs that work together, instead of hunting
# the bad ones (several bad programs interact: a half that holds in one
# combination breaks in another, and some programs leave the scene whole
# but at ~2 fps).
#
# Candidates (K_ALL, default bench/p4-translate-ok.txt, minus K_SKIP_FILE if
# given) are tried in batches (K_BATCH, default 64) on top of the good set.
# A run is good when the scene is whole (>= K_MIN_DRAWS draws a frame,
# default 7,500) AND not crawling (>= K_MIN_FPS median fps, default 10). A
# good batch joins the set; a bad one is split in two and both halves are
# queued; a bad single program goes to bench/p4-grow/bad.txt.
#
# State is kept in bench/p4-grow/ (good.txt, bad.txt, queue/): a second
# start resumes where it stopped.
#
#   scripts/p4-grow.sh
set -uo pipefail
cd "$(dirname "$0")/.."
W=bench/p4-grow
mkdir -p "$W/queue"
touch "$W/good.txt" "$W/bad.txt"
say() { printf '\e[1;32mp4-grow:\e[0m %s\n' "$*"; }
winpath() { local p; p=$(readlink -f "$1"); echo "Z:${p//\//\\}"; }
min_draws=${K_MIN_DRAWS:-7500} min_fps=${K_MIN_FPS:-10}
if [[ -z $(ls "$W/queue") && ! -s $W/good.txt && ! -s $W/bad.txt ]]; then
    all=${K_ALL:-bench/p4-translate-ok.txt}
    sort "$all" | { if [[ -n ${K_SKIP_FILE:-} ]]; then grep -vxFf "$K_SKIP_FILE"; else cat; fi; } |
        split -l "${K_BATCH:-64}" -d -a 4 - "$W/queue/b"
fi
n=0
while :; do
    next=$(ls "$W/queue" | sort | head -n1)
    [[ -z $next ]] && break
    n=$((n + 1))
    batch=$W/queue/$next
    cat "$W/good.txt" "$batch" >"$W/try.txt"
    label=p4-grow-$n-$(wc -l <"$batch")
    scripts/ktrials.sh -n 1 -l "$label" -s 15 VICTORY_NATIVE_COMPUTE=1 "VICTORY_NATIVE_COMPUTE_ONLY=$(winpath "$W/try.txt")" >/dev/null 2>&1
    d=$(ls -d bench/k-*"$label" | tail -1)
    log=$(awk -F'\t' 'NR==2 {print $13}' "$d/runs.tsv" 2>/dev/null)
    draws=$( [[ -n $log ]] && sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -oE '\), [0-9]+ draws/frame' | tail -4 | grep -oE '[0-9]+' | sort -n | sed -n 2p)
    fps=$(awk -F'\t' 'NR==2 {print $5}' "$d/runs.tsv" 2>/dev/null)
    if [[ -z $draws || -z $fps || $fps == - ]]; then
        say "test $n: no measurement; stopping (the batch stays queued)"
        exit 1
    fi
    if ((draws >= min_draws)) && awk -v f="$fps" -v m="$min_fps" 'BEGIN {exit !(f >= m)}'; then
        cat "$batch" >>"$W/good.txt"; rm "$batch"
        say "test $n: batch $next kept (draws=$draws fps=$fps)"
    elif (($(wc -l <"$batch") == 1)); then
        cat "$batch" >>"$W/bad.txt"; rm "$batch"
        say "test $n: program $(cat "$W/bad.txt" | tail -1) is bad alone on the good set (draws=$draws fps=$fps)"
    else
        half=$(($(wc -l <"$batch") / 2))
        head -n "$half" "$batch" >"$batch.a"; tail -n +"$((half + 1))" "$batch" >"$batch.b"; rm "$batch"
        say "test $n: batch $next bad (draws=$draws fps=$fps): split in $half + $(wc -l <"$batch.b")"
    fi
    say "  good $(wc -l <"$W/good.txt"), bad $(wc -l <"$W/bad.txt"), batches queued $(ls "$W/queue" | wc -l)"
done
say "done: $(wc -l <"$W/good.txt") good, $(wc -l <"$W/bad.txt") bad (bench/p4-grow/good.txt, bad.txt)"
