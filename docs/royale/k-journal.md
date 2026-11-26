# Journal of branch k7900 (victory-royale on kroeeee's tree)

## 2026-11-26

**K.0 baseline (kroeeee's tree as it is, Jason's house, by hand):** fps median 5.5-7.2 over a run, ~140-180 ms a frame; preparing ~80-110, copies ~36-53, GPU ~6-16, copy-back ~20; "asking the write watch and the guards" median 20-31 ms (the author: 2-4). Tested with the author and ruled out: zram (21.5 → 22.9 with disk swap only), one CCD (20.5), THP shmem (already `advise`); ntsync off is worse (31.4).

**K.2 tooling:** `scripts/ktrials.sh` (unattended runs, medians per run, window-only screenshots; closes the game by pid: `pkill -x victory.exe` misses it under Wine and once left a game open under the next run, which then suspended the title). `VICTORY_KEYS` works in release builds now.

**K.1 pipeline library (null blob):** works: ~2,230-2,275 pipelines loaded from disk a run (`loaded=`), the log no longer says "pipelines are not kept". **No measurable gain here** (`bench/k-…-k1-library-b` against `k-…-k1-control-nolib`, `VICTORY_PIPELINE_LIBRARY=0`): world entry 89-99 s either way (the intro), fps 5.4/7.4 against 9.0/6.2, write watch the same. This tree builds pipelines off the title's threads and Mesa's shader cache already holds the compiled code. Kept on (less work on early runs). **Runs vary a lot (6.2 vs 9.0 fps for the same build): decide on 4+ runs.**

Seen: the intro flickers before the loading screen (the author lists it as known); Jason's face showed only eyes and mouth in one run, fine in the next (intermittent, as S.9 in the old tree); a black picture once at start, fine on relaunch.
