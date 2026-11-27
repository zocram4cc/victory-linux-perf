# Where things stand (branch `k7900`), and how every piece works

Written 26 Nov 2026, evening. Read this first; [PLAN.md](PLAN.md) has the plan, [k-journal.md](k-journal.md) the dated log of each step.

## 1. In one paragraph

Work moved on 26 Nov from our old base (joshuu/victory-amd, ~2.7 fps at Jason's house, now tag `archive/victory-amd-2026-11-26`) to **kroeeee's tree** (avatarmanz/VIctory-7900XTX plus Linux fixes) as branch **`k7900`**. On this machine it ran at 5-9 fps, against the author's ~16. The difference was **the wineserver**: with tens of thousands of memory views, the stock server walks a list on every map/unmap. With our **view-tree wineserver** on GE-Proton11-14, the game runs at **17-19 fps** standing at Jason's house. That patch went to the author as **PR #1**. What still looks broken when moving into new areas comes from **pipelines the layer cannot make (device methods 0x1c8 and 0x218, task P.4)**: up to ~1,000 draws and dispatches a frame are dropped. P.4 is in progress.

## 2. Measured, same scene (Jason's house, release build, this machine)

| setup | fps (median per run) | ms a frame | write watch | notes |
|---|---|---|---|---|
| old tree (archive) | ~2.7 | ~360-400 | - | GPU ~165 ms a frame |
| kroeeee's tree, Proton Experimental, stock wineserver | 5.5-7.2 | 138-183 | 20-31 ms | by hand, 26 Nov |
| `k7900`, GE-Proton11-14, stock wineserver | 8.2-8.4 | 119-122 | 17-26 ms | wineserver 11-16 % of CPU samples |
| **`k7900`, GE-Proton11-14, view-tree wineserver** | **17.0-19.8** | **51-59** | **3.8-4.7 ms** | wineserver ~1 %; world entry 43-61 s |

Moving into new areas: 2-6 fps for a few seconds while the title streams (its main thread; outside the layer), and objects missing or black (P.4).

## 3. How to play the current state

```sh
~/Games/victory-data/run-kroeeee.sh
```

- Runs `~/src/victory-royale-k7900/linux/victory-proton.sh` (the author's launch script, unchanged) with:
  - `PROTON=~/Games/victory-data/proton-vt/GE-Proton11-14-vt/proton`: a reflinked copy of GE-Proton11-14 whose `files/bin/wineserver` is our view-tree build;
  - `PREFIX=~/Games/victory-data/perfiles/kroeeee/prefix-ge`: its own prefix;
  - `VICTORY_GAME_DIR` the game folder (read only), `VICTORY_CACHE_DIR` / `VICTORY_DATA_DIR` in `perfiles/kroeeee/` (the data is a copy of `royale-save`'s saves);
  - logs in `perfiles/kroeeee/logs/run-*.txt`.
- Overrides: `PROTON=…` and `PREFIX=…` (e.g. Proton Experimental with `PREFIX=$P/prefix`), `K_REPO=~/src/victory-linux-7900` for the author's tree as it is.
- The start screen can stay black until A/Enter is pressed. The intro can flicker (known upstream).
- To close a hung game: `ps -eo pid,args | grep '[r]elease\\victory.exe run'` and kill those pids (**`pkill -x victory.exe` does not find it under Wine**).

## 4. Pieces and where they live

| piece | where | what it is |
|---|---|---|
| branch `k7900` | `~/src/victory-royale-k7900` (a worktree of `~/src/victory-royale`; remote `kroeeee` = the author's repo) | the author's tree + our commits; pushed to `origin/k7900` (our private repo) |
| author's tree as it is | `~/src/victory-linux-7900` | a plain clone, for comparisons |
| view-tree wineserver patch | `linux/wineserver/wineserver-view-tree.patch` (+ README) | ~150 lines in `server/mapping.c`; prints one line if it ever falls back to the list |
| its source and build | `~/src/wineserver-ge/ge/wine` (GE-Proton11-14's prepared Wine, patched), build in `~/src/wineserver-ge/build` (`make -j16 server/wineserver`) | rebuild here, copy `build/server/wineserver` into the Proton copy |
| the Proton copy | `~/Games/victory-data/proton-vt/GE-Proton11-14-vt` | only `files/bin/wineserver` differs from GE-Proton11-14 |
| PR to the author | kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX **#1**, from fork `datingesp-hash/…`, branch `wineserver-view-tree` (worktree `~/src/k7900-pr`) | `linux/wineserver/` + a line in `LINUX.md` |
| old tree | `~/src/victory-royale` (branches `s5-heap-sync`, `main`) | archive; its README section 0 points here |

## 5. Our commits on `k7900` (on top of the author's `6c970fe`)

| what | switch | state |
|---|---|---|
| K.1 pipeline library made with a null blob (vkd3d refuses a non-null pointer of length 0) | `VICTORY_PIPELINE_LIBRARY=0` turns the library off | on; ~2,250 pipelines loaded a run; no measurable fps gain in this tree |
| K.2 `VICTORY_KEYS` in release builds | `VICTORY_KEYS=<s>:<key>,<from>-<to>:<key>` | for unattended runs |
| K.6 view-tree wineserver (outside the layer) | the Proton copy | the big gain |
| P.4 dump of the 0x1c8/0x218 descriptions (from fxceless) | `VICTORY_NATIVE_PSO_DUMP=Z:\…\dir` | off unless set |
| P.4 census on the frame line | always | `P.4: work without a host pipeline per frame: N with no pipeline set, M with a pipeline not made` |
| P.4 0x218 programs rebuilt as FSPX and made | `VICTORY_NATIVE_COMPUTE=1`, `VICTORY_NATIVE_COMPUTE_MAX=N` | **off**: the scene goes black with it (bisecting) |
| FSPX root constants in `victory_shader::native::fspx_program` | always | needed by 0x218's programs |

## 6. Tools

- **`scripts/ktrials.sh -n RUNS -l LABEL [-s SECS] [-w SETTLE] [VAR=value …]`**: unattended runs. Taps Enter through the start screen, waits for the world (three frame lines over 3,000 draws), settles, measures, screenshots the game window, closes the game by pid, and writes `bench/k-<date>-<label>/runs.tsv` (median fps, ms, preparing, copies, GPU, copy-back, write watch, dropped, world entry). It refuses to start while a game runs. Extras: `K_PERF=1` (a `perf` profile of the game and wineserver over the window, kernel included; needs `perf_event_paranoid` ≤ 1), `K_EARLY_SHOT=1`, `K_SHOTS=N`, and `PROTON`/`PREFIX` through the environment. **Runs vary a lot (e.g. 6.2 vs 9.0 fps for the same build): decide on 3+ runs.**
- **`scripts/p4-bisect.sh [LOW] [HIGH]`**: finds the first 0x218 call that breaks the picture (`VICTORY_NATIVE_COMPUTE_MAX`, a run is broken when its fps is above 30).
- **`crates/victory-shader/examples/p4_fspx.rs`**: offline P.4 harness. `cargo run --release -p victory-shader --example p4_fspx -- ~/Games/victory-data/pso-dump` rebuilds every dumped 0x218 description as FSPX, translates it natively on Linux in seconds, and groups the errors.
- **The dump**: `~/Games/victory-data/pso-dump` (one run, 598 MB): `1c8_*` (10,206) and `218_*` (2,680) directories with `desc.bin`, `root.bin` and `w<word>_<size>.bin` pieces.

## 7. P.4 as of now

- **0x218 = compute programs as the pieces of an `FSPX` program**: (1,2) 0x1c pipeline registers (thread-group size, RSRC1/RSRC2), (3,4) 0x14 metadata (hash), (5,6) GFX10 code in title GPU memory, (7,8) program metadata (`0x403` header, 0x41 resource / 0x42 view / 0x43 hash records). Rebuilt as FSPX inside a one-part `ACCC` container, they go through the normal compute path.
- Offline: **542 of 2,680 translate**. The rest: 1,366 "an address with two computed terms", 226 "an address not from a root parameter", 166 "an image store of some channels", then small groups (unsupported scalar instructions, unknown structured-buffer stride, thread-group info SGPR, lane masks read as data).
- In game with `VICTORY_NATIVE_COMPUTE=1`: all 2,689 are accepted, but **the scene goes black when the world starts** (the HUD shows a moment), even with only the 542 that translate running. Not a few bad programs: ~30 break the scene even made alone (`p4-grow.sh`), so the fault is systematic, most likely the mapping of FSPX records onto the translator's bindings. Next step offline (see the journal's stop point).
- **0x1c8 (10,206 descriptions) is graphics**: word 0 root signature, (1,2) a 0x1d8 header, (3,4) 0x50, then per stage (code, metadata) pairs at words 5-8, 9-12, 13-16. Not started; likely FSPX-like stage metadata plus a graphics packet.

## 8. Open threads, in order

1. **P.4 0x218**: find the systematic binding fault offline (compare with a regular container holding the same code, or check one small program by hand); then the translator's "two computed terms" (1,366 programs).
2. **P.4 0x1c8** (graphics): decode the header and per-stage metadata against FSSM's records; same approach (rebuild a container the existing path takes).
3. **Streaming flicker**: Belobug's texture uploads in the submission prelude (`5d6bd84`, "fix frame-order & flickering") are a candidate to port and measure.
4. **The author's three fixes** (known_holes under Wine, PORT_DESCRIPTORS with port 1, COPY_COMPARE on BAR) once he pushes them: merge and measure.
5. **The view-tree wineserver for Proton Experimental**, if wanted (the author already runs one there).

## 9. Rules kept

- The game folder is never written. The game is never started while the user plays (ktrials refuses).
- Public messages (PRs, comments) in the first person singular, in English; commits with the noreply address.
- Nothing goes to other people's repositories unless the user asks (PR #1 was asked for).
