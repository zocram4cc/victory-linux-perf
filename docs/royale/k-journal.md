# Journal of branch k7900 (victory-royale on kroeeee's tree)

## 2026-11-26

**K.0 baseline (kroeeee's tree as it is, Jason's house, by hand):** fps median 5.5-7.2 over a run, ~140-180 ms a frame; preparing ~80-110, copies ~36-53, GPU ~6-16, copy-back ~20; "asking the write watch and the guards" median 20-31 ms (the author: 2-4). Tested with the author and ruled out: zram (21.5 → 22.9 with disk swap only), one CCD (20.5), THP shmem (already `advise`); ntsync off is worse (31.4).

**K.2 tooling:** `scripts/ktrials.sh` (unattended runs, medians per run, window-only screenshots; closes the game by pid: `pkill -x victory.exe` misses it under Wine and once left a game open under the next run, which then suspended the title). `VICTORY_KEYS` works in release builds now.

**K.1 pipeline library (null blob):** works: ~2,230-2,275 pipelines loaded from disk a run (`loaded=`), the log no longer says "pipelines are not kept". **No measurable gain here** (`bench/k-…-k1-library-b` against `k-…-k1-control-nolib`, `VICTORY_PIPELINE_LIBRARY=0`): world entry 89-99 s either way (the intro), fps 5.4/7.4 against 9.0/6.2, write watch the same. This tree builds pipelines off the title's threads and Mesa's shader cache already holds the compiled code. Kept on (less work on early runs). **Runs vary a lot (6.2 vs 9.0 fps for the same build): decide on 4+ runs.**

Seen: the intro flickers before the loading screen (the author lists it as known); Jason's face showed only eyes and mouth in one run, fine in the next (intermittent, as S.9 in the old tree); a black picture once at start, fine on relaunch.

**K.6 found: the wineserver's view list. A view-tree wineserver doubles the frame rate (8.3 → 17.7 fps)**
- Profile (`K_PERF=1`): the wineserver took 11-24 % of all the game's CPU samples, in `is_valid_view_addr` and `req_unmap_view`: every map/unmap request walks all of the process's views (tens of thousands: GPU memory mapped 64 KiB at a time).
- The old tree's view-tree wineserver (built 24 Nov) did not help here: the server still walked the list. Rebuilt from the patch's current source (with a line printed if it ever falls back to the list), on GE-Proton11-14: **17.0, 17.6, 18.5 fps median per run (54-59 ms) against 8.4, 8.2 with GE's own wineserver; write watch 4.6 ms against 17-26; wineserver ~1 % of samples against 11-16 %; world entry 43-60 s against 112-120.** No fallback line in any run. The user, live: "nunca vi tantos fps"; a slow start while the area loads, then smooth.
- Patch and how to build it: `linux/wineserver/` (`wineserver-view-tree.patch`, README). Our build: `~/Games/victory-data/proton-vt/GE-Proton11-14-vt` (a reflinked GE-Proton11-14 with that wineserver); prefix `perfiles/kroeeee/prefix-ge`.
- Why the author does not need it (write watch 2-4 ms on Proton Experimental): to find out (fewer views on his runs, or a different server path). Next: build it for Proton Experimental too, and compare there.
