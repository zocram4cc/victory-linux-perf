# AGENTS.md — porting workspace

Instructions for anyone (human or agent) working in this directory. Read this
before touching anything.

## What this is

A compatibility layer project (`victory`) that runs an Xbox Series X|S title on
a Linux host through Wine. Our tree is `vfork/` — a fork of
[fxceless/victory-fork](https://github.com/fxceless/victory-fork) with our own
Wine/RADV fixes and a curated patch series on `main`.

Everything here is GPL-3.0-or-later source code. **No game files, no derived
assets, no keys, ever** — not in the repo, not in a commit, not in a bundle.

## Hard rules

1. **Never modify the original dump.** Work only against the reflink copy
   (`../title-work`). `check-data.py` must still report that every file is written out.
2. **No game data in git.** Not in `vfork/`, not in `mirrors/`, not in
   `bundles/`. If a tool needs a path to the title, it lives outside the
   repository.
3. **One game instance at a time.** Two concurrent runs corrupt saves and
   crash. Kill leftovers before starting a new run:
   ```sh
   pgrep -af '[v]ictory.exe run' | awk '{print $1}' | xargs -r kill
   tools/GE-Proton11-14-x86_64/files/bin/wineserver -k
   ```
   Never `pkill -f victory.exe run` — it kills its own shell.
4. **Do not edit `run-victory.sh` while a run is using it.** bash reads the
   script as it executes; editing mid-run kills the run with a parse error.
5. **No `sudo`, no system changes without asking.** Building the patched
   wineserver needed packages; that was approved explicitly.
6. **No PRs, pushes or comments upstream without explicit go-ahead.** Our
   remote `origin` is the private server; `fxceless`, `upstream`, `triippy`,
   `chapterii`, `vitoriabr` are read-only sources.
7. **Measure before and after.** A change without a measurement is not done.
8. **Never delete the shader cache.** `victory-data/cache` (shaders, pipelines,
   images; ~4.3 GB, ~70k files today) is the single biggest saving at start-up:
   translating the pack costs tens of minutes and the game stutters while it
   happens. Prune it only with an explicit instruction, and never as part of
   "cleaning up space". A copy is kept on the backup host (see §Backups).

## Layout

| Path | What |
|---|---|
| `vfork/` | Our tree. `main` = the patch series; worktrees live in `@WORKTREES@/`. |
| `vfork/crates/victory-gpu/src/` | GPU backend: `host.rs`, `shadow.rs`, `watch.rs`, `inplace.rs`, `arena.rs`, `resident.rs`, `draw.rs`, `descriptor/host.rs` |
| `run-victory.sh` | Launcher (presets below). Builds the EXE path from `VICTORY_BUILD`. |
| `victory-work.toml` | Config: `game_dir` points at the reflink copy. |
| `NOTES.md` | Running log with every measurement. Read the last section before starting. |
| `NOTES-SUMMARY.md` | The readable overview: state, measurements, root causes, next steps. |
| `HANDOFF.md` | Start here in a new session: state, branch map, next three tasks. |
| `compare-runs.py` | Summarises `run-*.log`: fps, frame, copies, textures, askings, marked pages. |
| `copy-cost.py` | Shows the costliest single title-memory copies in a diag log. |
| `captures<N>/` | Frame dumps per run (`VICTORY_CAPTURE`). |
| `check-data.py` | Completeness gate for the dump. |
| `mirrors/`, `bundles/` | Offline repo mirrors and git bundles + `SHA256SUMS`. |
| `backup-repos.sh` | Refreshes every mirror, re-bundles, keeps the curated PR refs. |
| `tools/GE-Proton11-14-x86_64/` | Wine used to run the title. |
| `tools/wine-patched*/` | Wineserver built with the fork #184 patch (see NOTES §22). |
| `wineprefix/` | The Wine prefix with vkd3d-proton 3.2 and DXVK. |

## Build

```sh
cd vfork
cargo xwin build --profile diag -p victory --features diagnostics --target x86_64-pc-windows-msvc
cargo xwin build --release --locked -p victory --target x86_64-pc-windows-msvc
```

Copy the D3D12 DLLs next to the binaries after a build (the launcher does not):

```sh
p=../tools/GE-Proton11-14-x86_64/files/lib/wine/vkd3d-proton/x86_64-windows
cp -f "$p/d3d12.dll" "$p/d3d12core.dll" target/x86_64-pc-windows-msvc/{diag,release}/
```

## Test

The suites only run under Wine:

```sh
export WINEPREFIX=$PWD/../wineprefix WINEDEBUG=-all
export WINEDLLOVERRIDES="d3d12,d3d12core,dxgi,d3d11,d3d10core=n,b"
W=../tools/GE-Proton11-14-x86_64/files/bin/wine
cargo xwin test --release --no-run -p victory-gpu -p victory-shader --target x86_64-pc-windows-msvc
f=$(ls -t target/x86_64-pc-windows-msvc/release/deps/victory_gpu-*.exe | head -1)
timeout 900 "$W" "$f" --test-threads=2
```

Known Wine-only failures (they fail on `main` too, ignore them):
`resource::tests::a_depth_planes_twin_is_drawn_into_the_plane`,
`emit::tests::stub_pipeline_is_created`,
`emit::valu_tests::v_cvt_f32_f64_matches_ieee754`, and
`victory-hle::modules::net::tests::only_loopback_is_reachable` (this host has
internet), and `drvwatch::tests::the_system_takes_the_subscription` — Wine's
`EvtSubscribe` neither subscribes nor sets an error, so **the driver watch is
inert under Wine** and its test cannot pass. Anything else is a regression.

## Run and measure

```sh
./run-victory.sh 240          # default: VICTORY_FAST=1, 30 fps menus/intro
```

| Variable | Meaning |
|---|---|
| `VICTORY_FAST=1` (**default**) | 1080p, relay/late-wait, card buffers, `TEXTURE_NO_WAIT`. Best intro; world ~0.5 fps. |
| `VICTORY_PLAIN=1` | No preset at all. |
| `VICTORY_INPLACE_RUN=1` | **Experimental.** Title memory in place (`INPLACE=3`, 4 GiB arenas): world 6–7 fps, copies ~10 ms, but it lost the device in 3 of 6 runs. Never combine with `VICTORY_FAST`. |
| `VICTORY_BUILD=` | Which tree/worktree under `porting/` to run (`vfork`, `ip`, `fx`, `sp`, `marked`). |
| `VICTORY_WS=patched` | Start the patched wineserver first (protocol 938). Currently refused by `wineboot` — see NOTES §22. |
| `VICTORY_KEYS="20:Enter,40:Enter,…"` | Scripted input at second offsets. |
| `VICTORY_CAPTURE=<dir>`, `VICTORY_CAPTURE_EVERY_SECS=20` | Frame dumps. |
| `VICTORY_SKIP_PIPELINES=<hash>` | Drop a pipeline (AMD device-loss workaround). |

Then:

```sh
./compare-runs.py 3            # last three runs, side by side
./copy-cost.py run-<hhmmss>.log
```

Always state which build, which switches and which phase a number comes from.
Menu frames and cutscene frames are different worlds; `compare-runs.py` splits
them at 2 fps.

## Code conventions for patches

- One change per commit. The message says what it fixes, the measurement that
  shows it helps, and the run it came from.
- Cherry-picks keep `git cherry-pick -x` trailers and the original author.
- Our own additions go where the job lives in this tree; if upstream's file
  does not exist here (`bindless.rs`), say so in the message and put the code
  next to the equivalent job.
- Everything we write is in English: `README.md`, `docs/`, `NOTES.md` and
  `NOTES-SUMMARY.md`. Never take upstream's README over ours.
- After an accepted change: update `NOTES.md`, refresh `README.md` if a switch
  or a status number changed, and re-run `./backup-repos.sh`.

## Backups

```sh
./backup-repos.sh                        # mirrors + bundles + SHA256SUMS
rsync -a bundles/ @BACKUP_HOST@:@BACKUP_DIR@/some-game-porting-bundles/
ssh @BACKUP_HOST@ 'cd @BACKUP_DIR@/some-game-porting-bundles && sha256sum -c SHA256SUMS'
```

Tag checkpoints on `main` (`git tag -a checkpoint-<what>-<date>`) and push
`--tags` to `origin`.

## State of play

As of 2026-11-27 (RX 7900 XTX, Series X profile 3840x2160, tree `fps20-ports`,
branch `fps20-wip`; measurements in `NOTES.md` §155–§167). The sections above
describe the older `vfork` workflow; the current launcher is
`fps20-ports/launch-ports.sh`, started through `run-ports.py` or `play.sh`.

- Menus and the intro: 50–60 fps.
- World with the ray-tracing structure builder off (`VICTORY_BVH_BUILDER=0`,
  the default): **22–26 fps** (37–43 ms; copies ~12 ms, copying back ~20 ms),
  290–320 draws a frame dropped (the builder's stages and six shadow-ray
  programs on the bad list). Light is a stand-in (`ambient.rs`).
- World with the builder on: **about 9 fps**, 50–85 ms on the GPU; room,
  glass and aquarium are lit as the title means them.
- Open, picture: the phone's screen and glass are covered by blue-white noise
  indoors; strand hair is missing in some starts and can go during a run; the
  head is sometimes untextured. None of the upstream changes taken in §166
  moved these.
- Open, correctness: `VICTORY_UPCOPY_CHECK=1` reports older values sent over
  card-written words every frame, and one 24 MB buffer written by a program
  nobody declared (§167). Console wave32 pixel programs declare no wave size
  (`backups/wave64-audit-20261127/code-audit.md`).
- Keep build trees out of `/tmp`: it is a RAM disk here, and 7.9 GB of them
  starved the game of memory (§166).
- The title's own GPU-progress watchdog aborts at `+0x2215c35` when GPU work
  does not finish; treat it as a stall, not memory corruption.

Read `NOTES.md` §155–§167 for the measurements behind all of the above.
