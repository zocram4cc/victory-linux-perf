#!/usr/bin/env bash
# P.4: which whole family (bench/p4-fam/<root signature>.txt) breaks the
# house, by halves. A run's verdict: the world-entry screenshot's window
# region (the house's interior: dark when drawn) against K_LIMIT (default
# 66: plain runs 54-60, broken 73-85). Each run from the original save.
set -uo pipefail
cd "$(dirname "$0")/.."
export K_CACHE=${K_CACHE:-$HOME/Games/victory-data/perfiles/kroeeee/cache-p4}
winpath() { local p; p=$(readlink -f "$1"); echo "Z:${p//\//\\}"; }
window() { # the window region's mean luminance of run LABEL's entry shot
    python3 -I - "$(ls -d bench/k-*"$1" | tail -1)" <<'PY'
import glob, sys
from PIL import Image, ImageStat
p = sorted(glob.glob(sys.argv[1] + "/run-1-entry.png"))
if not p: print(""); sys.exit()
im = Image.open(p[0]).convert("L"); w, h = im.size
print(int(ImageStat.Stat(im.crop((int(w*.02), int(h*.18), int(w*.26), int(h*.85)))).mean[0]))
PY
}
run() { # LABEL FILE (empty: no native programs)
    if [[ -n $2 ]]; then
        K_EARLY_SHOT=1 scripts/ktrials.sh -n 1 -l "$1" -s 10 VICTORY_NATIVE_COMPUTE=1 "VICTORY_NATIVE_COMPUTE_ONLY=$(winpath "$2")" >/dev/null 2>&1
    else
        K_EARLY_SHOT=1 scripts/ktrials.sh -n 1 -l "$1" -s 10 >/dev/null 2>&1
    fi
    window "$1"
}
echo "plain: window $(run fam-plain '')"
ls bench/p4-fam/*.txt | sort > bench/p4-fam/set.list
cat $(cat bench/p4-fam/set.list) > bench/p4-fam/all.txt
echo "all $(wc -l < bench/p4-fam/set.list) families: window $(run fam-all bench/p4-fam/all.txt)"
step=0
while (($(wc -l < bench/p4-fam/set.list) > 1)); do
    step=$((step + 1)); n=$(wc -l < bench/p4-fam/set.list); half=$((n / 2))
    head -n $half bench/p4-fam/set.list > bench/p4-fam/a.list; tail -n +$((half + 1)) bench/p4-fam/set.list > bench/p4-fam/b.list
    cat $(cat bench/p4-fam/a.list) > bench/p4-fam/a.txt
    v=$(run "fam-s$step" bench/p4-fam/a.txt)
    if [[ -n $v ]] && ((v > ${K_LIMIT:-66})); then cp bench/p4-fam/a.list bench/p4-fam/set.list; r="first half breaks it"; else cp bench/p4-fam/b.list bench/p4-fam/set.list; r="first half holds"; fi
    echo "step $step: first $half of $n families: window ${v:-?}: $r"
done
echo "family: $(cat bench/p4-fam/set.list)"
