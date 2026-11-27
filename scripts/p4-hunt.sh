#!/usr/bin/env bash
# P.4: finds every 0x218 program that breaks the picture, one after another,
# by halving the list of programs (their code addresses, from the native PSO
# dump; stable between runs, unlike the order of the calls, which differs
# because pipelines are made on several threads).
#
# A run makes only the programs listed in a file (VICTORY_NATIVE_COMPUTE_ONLY)
# and is broken when a screenshot shows no picture: contrast under K_MIN_STD
# (default 15). Its fps is noted, not judged.
#
# Each round: all candidates on; if the scene holds, done. Otherwise halve:
# the half that breaks it alone is kept, until one program is left; it goes
# to bench/p4-bad.txt and out of the candidates.
#
#   scripts/p4-hunt.sh [ROUNDS]     (default 6; K_DUMP= the dump directory)
set -uo pipefail
# P.4 runs make native pipelines: a cache of their own, so that the pipeline
# library of normal play does not start them early every run (27 Nov).
export K_CACHE=${K_CACHE:-$HOME/Games/victory-data/perfiles/kroeeee/cache-p4}
cd "$(dirname "$0")/.."
DUMP=${K_DUMP:-$HOME/Games/victory-data/pso-dump}
BAD=bench/p4-bad.txt
WORK=bench/p4-hunt
mkdir -p "$WORK"
touch "$BAD"
rounds=${1:-6}
say() { printf '\e[1;35mp4-hunt:\e[0m %s\n' "$*"; }
winpath() { local p; p=$(readlink -f "$1"); echo "Z:${p//\//\\}"; }
# The lowest screenshot contrast of run LABEL (luminance standard deviation:
# ~70 for a real scene, under 5 for a grey, black or white screen), and fps.
# (Draws a frame were a wrong test: the camera cuts indoors by itself.)
measure() {
    local d
    d=$(ls -d bench/k-*"$1" 2>/dev/null | tail -1)
    echo "$(python3 -I - "$d" <<'PY'
import glob, sys
from PIL import Image, ImageStat
v = []
for p in glob.glob(sys.argv[1] + "/run-1*.png"):
    im = Image.open(p).convert("L"); w, h = im.size
    v.append(ImageStat.Stat(im.crop((int(w * .05), int(h * .15), int(w * .95), int(h * .95)))).stddev[0])
print(int(min(v)) if v else "")
PY
) $(awk -F'\t' 'NR==2 {print $5}' "$d/runs.tsv" 2>/dev/null)"
}
# Runs with only the addresses in FILE made; prints "draws fps".
try() { # LABEL FILE
    K_SHOTS=2 scripts/ktrials.sh -n 1 -l "$1" -s 15 VICTORY_NATIVE_COMPUTE=1 "VICTORY_NATIVE_COMPUTE_ONLY=$(winpath "$2")" >/dev/null 2>&1
    measure "$1"
}
# K_ALL=<file>: the candidates (e.g. the programs that translate, from
# `p4_fspx --ok`); default: every program in the dump.
if [[ -n ${K_ALL:-} ]]; then sort "$K_ALL" >"$WORK/all.txt"; else ls -d "$DUMP"/218_* | sed 's|.*/218_||' | sort >"$WORK/all.txt"; fi
for ((round = 1; round <= rounds; round++)); do
    grep -vxFf "$BAD" "$WORK/all.txt" >"$WORK/cand.txt" || true
    n=$(wc -l <"$WORK/cand.txt")
    read -r draws fps < <(try "p4-hunt-r$round-all" "$WORK/cand.txt")
    say "round $round: $n programs on (skipping $(wc -l <"$BAD")): std=${draws:-?} fps=${fps:-?}"
    [[ -z $draws ]] && { say "no measurement; stopping"; break; }
    ((draws >= ${K_MIN_STD:-15})) && { say "the scene holds with all of them: done"; break; }
    cp "$WORK/cand.txt" "$WORK/set.txt"
    step=0
    while (($(wc -l <"$WORK/set.txt") > 1)); do
        step=$((step + 1))
        half=$(($(wc -l <"$WORK/set.txt") / 2))
        head -n "$half" "$WORK/set.txt" >"$WORK/a.txt"
        tail -n +"$((half + 1))" "$WORK/set.txt" >"$WORK/b.txt"
        read -r draws fps < <(try "p4-hunt-r$round-s$step" "$WORK/a.txt")
        [[ -z $draws ]] && { say "no measurement; stopping"; exit 1; }
        if ((draws < ${K_MIN_STD:-15})); then
            cp "$WORK/a.txt" "$WORK/set.txt"; verdict="first half breaks it"
        else
            cp "$WORK/b.txt" "$WORK/set.txt"; verdict="first half holds"
        fi
        say "  step $step: first $half of $((half + $(wc -l <"$WORK/b.txt"))): std=$draws fps=$fps, $verdict"
    done
    bad=$(cat "$WORK/set.txt")
    echo "$bad" >>"$BAD"
    say "round $round: program $bad breaks the picture (now $(wc -l <"$BAD") left out)"
done
say "left out: $(paste -sd, "$BAD")"
