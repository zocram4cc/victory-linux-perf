# Checkpoints

States to play and compare, each a git tag `cp/<name>` and a ready-to-run build in `~/Games/victory-data/checkpoints/<name>/` (release `victory.exe`, this tree's `linux/` launcher, a link to the huge-page helper). Made with `scripts/checkpoint.sh NAME "what" [COMMIT]`.

**Play one:**

```sh
K_REPO=~/Games/victory-data/checkpoints/<name> ~/Games/victory-data/run-kroeeee.sh
```

**Measure one unattended** (3 runs, each from the original save; results in `bench/k-<date>-<label>/runs.tsv`, screenshots beside):

```sh
cd ~/src/victory-royale-k7900
K_LAUNCH=~/Games/victory-data/run-kroeeee.sh K_REPO=~/Games/victory-data/checkpoints/<name> scripts/ktrials.sh -n 3 -l cp-<name> -s 40
```

**Go back to one in the source:** `git checkout cp/<name>` (or `git switch -c try-<name> cp/<name>`); every checkpoint is also pushed to the private repo.

All of them run with GE-Proton11-14 and the view-tree wineserver (`~/Games/victory-data/proton-vt/`), profile `kroeeee`. P.4 (native 0x218 pipelines) is off in all of them unless `VICTORY_NATIVE_COMPUTE=1` is set.

| name | commit | made | what this state is |
|---|---|---|---|
| `2026-11-27-translator-1105` | `70318d12` | 2026-11-27 04:51 | Current: translator at 1,105 of 2,680 0x218 programs (steps 1-13), huge pages, P.4 off by default. Played by the user 27 Nov ~04:30: like the best runs (15-21 fps). |
| `2026-11-27-hugepages` | `3e80060d` | 2026-11-27 04:51 | kroeeee's huge pages for renderer port 1 merged, the watcher reading ANSI logs; translator at 715 (P.4 off). Played 27 Nov ~03:20: 15-19 fps with the original save. |
| `2026-11-26-viewtree` | `81252bb6` | 2026-11-27 04:51 | First good state on this machine: kroeeee's tree + K.1 + the view-tree wineserver (outside the repo), no huge pages, no translator work. Measured 26 Nov ~19:45: 17.0-18.5 fps. |
| `2026-11-27-minkb64` | `b55c8720` | 2026-11-27 15:37 | CARD_BUFFERS_MIN_KB=64 + COMPUTE_WAIT_MS=500 by default, DYNAMIC_BITS ported (on), translator at 1,105 (P.4 off). Measured: 25.5-25.8 fps (3 runs; 20.5-21.6 before). The user, playing: better, also moving away from the house. |
