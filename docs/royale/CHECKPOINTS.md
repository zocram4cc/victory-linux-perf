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
