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

**P.4 (K.3): slot 0x218 is an FSPX program taken apart; first build of it makes 2,689 pipelines, the picture goes black**
- `VICTORY_NATIVE_PSO_DUMP` (ported from fxceless `52ac4ec`), one run: **12,886 distinct descriptions, 10,206 of 0x1c8 and 2,680 of 0x218** (598 MB in `~/Games/victory-data/pso-dump`). 0x1c8: word 0 a root signature, (1,2) a 0x1d8 header and (3,4) 0x50 in the title image, then per stage (code in GPU memory, metadata in the image) at (5..8), (9..12), (13..16): graphics.
- **0x218 = the pieces of an `FSPX` program** (the format `victory_shader::native::fspx_program` already reads for `ACCC` parts): (1,2) 0x1c pipeline registers (thread-group size at +0x0c, RSRC1/RSRC2 at +0x14/+0x18), (3,4) 0x14 pipeline metadata (a hash), (5,6) the GFX10 code, (7,8) the program metadata (`0x403`, size − 0x14, code dwords; 0x41/0x42/0x43 records).
- `VICTORY_NATIVE_COMPUTE=1` (`device::native_compute`, off): rebuilds an `FSPX` blob in a one-part (`ACCC`) container and makes the pipeline as `create_compute` would. One run: **2,689 made, 0 refused**; 460 do not translate (`malformed`) and 331 fail in the converter; the rest run, and the scene goes black (~4,000 draws a frame instead of ~9,000): programs that did not run before now run with something wrong (resource mapping most likely). Off.
- Next: an offline harness over the dump (assemble, `fspx_program`, translate; errors grouped) to fix the assembly without the game; then check what the running ones bind.
- New P.4 census line (release): work dropped with no pipeline set (slots 0x1c8/0x218) against a pipeline set but not made.

**Belobug/VIctory-Belobug-AMD (read 26 Nov):** the same base as kroeeee's (`c9c932c`) plus three commits: FSR 4.1.2 to 4K, **texture uploads in the submission prelude "to fix frame-order & flickering"** (`5d6bd84`, a candidate for the flicker while streaming), and an RX 9070 XT set with a guard for AMD's compiler crashing in `amdxc64.dll` (a `warm-bad.txt` list of pipelines left out). No 0x1c8/0x218.

- P.4 bisect, user live: with `VICTORY_NATIVE_COMPUTE_MAX=675` the picture was **all white, Jason's hair and beard black** (other broken runs: black). At least two bad programs: one blowing the picture out (exposure/lighting?), one emptying the scene (draws ~9,000 → ~5,900 below call 1,350, → ~4,000 from call 1,352).

**P.4 bisect: call 169 (program `0f0039f000`) blows the picture out to white**
- `scripts/p4-bisect.sh` by draws a frame: up to call 168 the scene holds (~10,650 draws); with 169, ~6,000 and the picture white (screenshot `bench/k-…-p4-call169`). (A first bisect by fps was wrong: broken runs went from 29 to 36 fps.)
- The program (`pso-dump/218_0000000f0039f000`): 16×16 groups, code 0x3200 B, ~25 SRVs, 3 immediate samplers, 3 UAVs and 6 CBVs in two tables (user data 4 and 5): a lighting or exposure pass, by its shape.
- Now `scripts/p4-hunt.sh` (with `VICTORY_NATIVE_COMPUTE_SKIP`) finds the next ones the same way, round after round, into `bench/p4-bad.txt`, until the scene holds with every other program made.
- **Call numbers are not stable between runs** (pipelines are made on several threads): 168 calls held in one run, 87 broke in another. Program code addresses are stable (call 169's was in another run's dump). `p4-hunt.sh` now halves the sorted list of addresses (`VICTORY_NATIVE_COMPUTE_ONLY=<file>`), judging by draws only (fps is skewed by loading: one run at 2.3 fps was still streaming in, the user said).
