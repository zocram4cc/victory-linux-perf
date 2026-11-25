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

## Rules for figures that count

- **Three runs per configuration.** If `ms_per_frame` varies by more than 10 % between them, the measurement does not count. Find the cause first: a cold shader cache, other load on the PC, a different scene…
- **The first run after changing the shader translator warms the cache: discard it**, or run `warm` first.
- **Same PC, same GE-Proton, patched wineserver.** `log.txt` says which was used.
- **No other games or heavy programs open.**
