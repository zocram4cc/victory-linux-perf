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

**K.7 streaming: what breaks when moving into new areas is P.4 (slot 0x218); the fps drop is the title's main thread**
- World entry streams in fine: 18.6-19.2 fps in the first 30 s (`bench/k-…-k7-streaming`).
- Moving away by hand (`bench/k-…-k7-manual`; the user: things appear and disappear, some black, Jason black and white, blurry): **draws dropped "without a host pipeline (0 being made)" rise from ~200-350 to 600-1,058 a frame** as new areas come in. Those are the pipelines the title creates through device method slot 0x218 from the console's precompiled shader binaries (~2,048 calls, all at load), which no fork implements: their draws are not drawn. That is the visible breakage; TAA blending frames with missing objects fits the blur.
- In the same moments the frame grows to 200-660 ms while the layer's own parts shrink (preparing ~50 → ~30 ms); the profile has `title-main` at 33 % of samples: the title's own streaming work and waits, outside the layer.
- So P.4 (K.3) moves up: it is what the player sees. The fps dip while streaming is the title's.
