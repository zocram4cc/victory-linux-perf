# Reference scene and `scripts/bench.sh`

This covers phases 0.2 and 0.3 of the [README](../README.md).

## The scene

No save is needed:

1. Start a new game.
2. Skip the first cutscene.
3. The scene is the first moment you control the protagonist.

After updating the shader translator, warm the cache first:

```sh
victory-proton -p royale victory warm --no-pipelines
```

It takes about 100 s. The variant that also builds pipelines hangs on RADV (phase 3.4).

## Measuring

| Scene | What to do | Command |
|---|---|---|
| A, static | reach the scene, keep the camera still, wait ~20 s, press Enter | `scripts/bench.sh run -l base-A` |
| B, moving | from the same spot, press Enter and walk straight for 30 s | `scripts/bench.sh run -l base-B -s 30` |

**What `run` does:**

1. It launches the game with `victory-proton -p royale run --no-update` and `VICTORY_FRAMETIME_DUMP=1`.
2. It waits for Enter, then measures the window.
3. It closes the game when the window ends. With `--keep` the game stays open, and `scripts/bench.sh mark -l ...` measures again without relaunching.

**Testing a switch:** append it to the command, for example:

```sh
scripts/bench.sh run -l ceiling-no-copies VICTORY_COPY_MODE=none
```

An empty value (`VICTORY_SKIP_PIPELINES=`) overrides a value the profile would set.

**Results** go to `bench/<date>-<label>/`, which is not committed:

| File | Contents |
|---|---|
| `frametime.csv` | the measured window only |
| `summary.json` | mean, p50, p95, min and max of every column, plus the cores used by the game and by wineserver |
| `meta.json` | commit, the game's own `VICTORY_*` and `VKD3D_*` variables, profile |
| `log.txt` | the window's log |

**Comparing runs:**

```sh
scripts/bench.sh compare bench/<A> bench/<B> [...]   # table with the change against A
```

That table goes into every performance commit message.

## Unattended runs

`--auto` needs nobody at the keyboard:

1. It presses A (`VICTORY_KEYS`, Enter) through the start screen, from 8 s to 50 s.
2. It lets the intro cutscene play; holding A does not skip it.
3. It waits for the world: four CSV rows in a row with more than 3,000 draws.
4. It waits 20 s more, then measures.

A run takes about 3 minutes. `VICTORY_KEYS` now works in release builds.

```sh
scripts/bench.sh run -l my-test --auto VICTORY_SOMETHING=1
```

## Stability trials

`scripts/trials.sh` repeats one configuration unattended and classifies each run: `lost` (the log reports `0x887A0005`), `world` (the window was measured), `loading` (the world did not come up) or `closed`. It defaults to the open-world save (`royale-save`) and a 20 s window. `-o DP-1` takes a screenshot 8 s into the window, to check for the white picture.

```sh
scripts/trials.sh -n 5 -l heap-sync-on -o DP-1 VICTORY_HEAP_SYNC=1
scripts/trials.sh -n 5 -l heap-sync-off -o DP-1 VICTORY_HEAP_SYNC=0
```

Results go to `bench/trials-<date>-<label>/`: `trials.tsv` (one row per run, with fps and the heap-sync counter), `tally.txt`, each run's output and screenshot. Run the configurations being compared back to back, with the same binary.

## Profiling

`--perf` records a user-space CPU profile (`perf record -e cycles:u`) of the game and of wineserver during the window. `nsenter` steps into bubblewrap's user namespace so that `perf` can attach.

```sh
scripts/bench.sh run -l profile-A --perf
```

The window's folder then also has `perf.data`, `perf-wineserver.data` and `perf.txt` (top modules and symbols). Profiling costs a little time, so do not compare its frame times with a run without it.

## Rules for figures that count

- **Three runs per configuration.** If `ms_per_frame` varies by more than 10 % between them, the measurement does not count. Find the cause first: a cold shader cache, other load on the PC, a different scene…
- **The first run after changing the shader translator warms the cache: discard it**, or run `warm` first.
- **Same PC, same GE-Proton, patched wineserver.** `log.txt` says which was used.
- **No other games or heavy programs open.**

## Diagnostics build

Some reports only exist in the diagnostics build. `copystat` is one: copies by source range and kind of memory, write-watch queries, and how much of what shaders write actually changes. Build it as its own profile and run it through the launcher with `VP_EXE`:

```sh
cargo xwin build --profile diag -p victory --features diagnostics --target x86_64-pc-windows-msvc
VP_EXE=$PWD/target/x86_64-pc-windows-msvc/diag/victory.exe scripts/bench.sh run -l census-diag
```

Its frame times are not comparable with the release baseline: it does extra bookkeeping. Use it for *what* and *how much*, not for *how fast*.
