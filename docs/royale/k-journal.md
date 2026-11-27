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
- **First good sign in game:** a run with ~43 of the 0x218 programs made (hunt run `r1-1-43`, by call count): full scene (10,568 draws a frame); the user: slow only while it loaded, then with Jason in close-up "very sharp and correct, nothing odd", fps up. So programs rebuilt as FSPX run correctly; only some translate wrong.
- Only translatable 0x218 programs get a pipeline now (542 made, 2,147 left without): **still broken** (~6,000 draws; the user: grey, only the money and health HUD). Hunting among the 542 (`K_ALL=bench/p4-translate-ok.txt`, `p4_fspx --ok`); 6 of the 7 earlier culprits are among them.
- **Two kinds of bad program, interacting:** some subsets empty the scene (grey/black/white, ~4,000-6,000 draws), others leave it whole but crawl at 1.7-2.1 fps and look bad (the user; runs `r3-s1`, `r4-s3` of the hunt among translatable programs, which the draws-only test took for good). Halves that hold in one combination break in another, so hunting culprits by halves does not converge. **Switched to growing a good set** (`scripts/p4-grow.sh`): batches of 64 on top of the good set, kept when the scene is whole (≥ 7,500 draws) and not crawling (≥ 10 fps), split when not; state in `bench/p4-grow/`.

**P.4 stop point (27 Nov, ~00:30): the fault is systematic, not a few bad programs**
- `p4-grow.sh` (31 tests): **no batch kept; 30 programs break the scene even made alone** (`bench/p4-grow/bad.txt`), either emptying it (~6,000 draws) or crawling (1.5-3 fps with the scene whole). In those runs the program is made and runs (no work dropped for want of a host pipeline), so the translated programs themselves compute wrong.
- So hunting or growing sets does not lead anywhere: almost every rebuilt 0x218 program misbehaves. The earlier "sharp and correct" run (~43 calls made, before the translatable-only rule) most likely had few of them actually running.
- **Most likely cause: how the FSPX records are mapped** onto the translator's bindings (table offsets, user-data dwords, root slots, descriptor sizes), which was written for the AMD FidelityFX `ACCC` shaders and may not mean the same for the title's own programs. To check offline next:
  1. Find a 0x218 program whose GFX10 code also appears in a regular container (`create_compute`, FSSM record) in the shader cache, and compare the two sets of bindings field by field.
  2. Otherwise, disassemble a small one (`0f00220000`: 8×8 groups, 1 sampler, 1 SRV, 1 UAV, 1 CBV) and check each binding the code reads (user SGPRs 4.., table offsets) against what `fspx_program` produced.
- `VICTORY_NATIVE_COMPUTE` stays off (default). Normal play is unaffected.

**P.4, checked by hand on the smallest bad program (`0f00220000`, 384 B)**
- Disassembled (`llvm-mc -arch=amdgcn -mcpu=gfx1030`): table address in user SGPR s4 (`s_mov_b32 s8, s4; s_mov_b32 s9, 0`), `s_load_dwordx16 s[24:39]` at table+0 (SRV, 8 dwords, then UAV, 8 dwords), `s_load_dwordx4 s[12:15]` at table+0x40 (CBV); an immediate sampler built in code (`0x92`, `s_bfm_b32`, `0x14500000`); group ids in s5/s6/s7 (RSRC2 0xb8a: 5 user SGPRs, TGID x/y/z, TG size); `image_sample_l` from a 2D array with the CBV's scale, `image_store` of the result at (x, y, slice): a per-layer copy/downsample.
- **The metadata matches the code exactly:** SRV table offset 0 (8 dwords), UAV 8 (8 dwords), CBV 16 (4 dwords), all user data 4, root slot 0; sampler root type 4 (immediate). So the FSPX record mapping is not the fault, at least here.
- **Lost UAV writes ruled out:** `placed unordered texture view not mirrored (null: writes … lost)` is asked as often without native programs (`times` 2,048, 12 textures) as with them (512, 10).
- **Next (needs the game):** trace one native pipeline's dispatch: which root signature and root parameter values the list holds at that moment, and what user data the translated shader gets. Suspect: the title binds these pipelines' resources through a console path the layer does not carry over, so they read empty or wrong tables.

**P.4 correction (27 Nov, ~01:40): draws a frame were a wrong test; the "bad" programs are fine**
- Plain runs (no native programs) go from ~11,000 draws a frame outside the house to ~6,000 (sometimes ~4,000) when the camera cuts indoors, at a different moment each run. Every bisect/hunt/grow verdict by draws is unreliable (`bench/p4-grow-by-draws-invalid`, `bench/p4-bad.txt`).
- **Screenshots tell it:** luminance standard deviation of the game window ~72 for a real scene, ~1-4 for the grey screen (mean ~50), 0.7 for the white one (mean 247). The ~30 "bad" programs made alone (`bench/k-…-p4-trace`): **a perfect picture** (Jason on the porch, sea, light, HUD). What is grey: all 542 translatable made together (`p4-translatable`, `p4-uav-all`). The white one (call 169, `0f0039f000`) is real.
- `VICTORY_PLACED_UAV=1` made no difference (its counter never showed); left off.
- `p4-grow.sh` now judges by screenshots (std ≥ 15) and fps ≥ 5, from scratch.
- p4-grow tightened after the user's point (many programs are bad for real: worse look, lower fps, flicker): good only if contrast ≥ 15 on three screenshots AND fps ≥ 14 (17-19 without native programs). Flicker still needs the user's eyes.

**P.4: one program turns the picture grey; the other 508 run at 20 fps (27 Nov, ~02:00)**
- All 542 translatable minus the 33 earlier suspects (509): grey, 19.9 fps. Hunting by screenshot contrast (`p4-hunt.sh`, 9 steps): **`0f01a73b00` alone turns it grey**. With the other 508 made: contrast 69 (a real picture), **20.3 fps** (17-19 without native programs).
- Still wrong (the user, live): flicker in the intro before the loading screen, half the house missing and artifacts in the first, outdoor view of the house (fine once the view cuts to Jason), and some sets slow there (63 programs: 10 fps). Screenshots after settling miss it: next, compare the world-entry (outdoor) screenshot against plain runs. User notes per run with their program sets: `bench/p4-user-notes.txt`.
