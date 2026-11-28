# Testing handoff

For an agent or a person on a Windows machine with an AMD RDNA GPU. Test two trees:

1. **`dev`** at or after `46723850`. That squash already contains community PRs #1, #2, and #3.
2. **`integration/upstream-port-2`**. Same as `dev` plus the shader and pipeline-library port in `docs/upstream-sync-analysis.md`.

Do not merge this branch, and do not push to `main`, unless every box in the checklist is pass and the report at the bottom is filled in. `main` today is Wave 11 (`fd806e1e`).

## What is already on `dev`

| Piece | What to look for |
|---|---|
| PR #1, 8 GB fit | On an 8 GB card, textures admitted at 65%, 2048 MB kept for targets, card buffers capped at 768 MB. Preset `presets/4_Safe_Mode_8GB_12GB_VRAM.bat` sets the same limits and turns VSync on. 12/16/24 GB tiers are unchanged |
| PR #2, AMD fork fixes | Open water on a 32-lane GPU should stop losing tiles (dark 16×16 patches). Static samplers keep one order so the second launch recompiles less. Both are on unless turned off (below) |
| PR #3, upstream sync | Every real launch reserves free address space below 4 GiB except the lowest 256 MB, for the title's 32-bit allocations. Password entry is masked. Failed async cleanup does not hold the completion lock |

## Build and run

Windows 10 or 11, MSVC toolchain (never the gnu toolchain). From a fresh clone:

```powershell
git fetch origin
git checkout dev
# or: git checkout integration/upstream-port-2
cargo xtask agility
cargo xtask policy
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --offline --locked -p victory
```

The release binary is `target\release\victory.exe`. The Wave 11 package names that file `RAZDA4A_XEN.exe`. For a source run, point the usual launcher at this `victory.exe`, or copy it over a spare copy of the package (do not overwrite the only known-good Wave 11 exe until the checklist passes).

One GPU run at a time. If `heavy.lock` is held, wait.

Turn a single change off without rebuilding, if a scene gets worse:

- Water-tile rewrite: set `VICTORY_WATER_TILE_SLOTS=0`
- Static-sampler order: set `VICTORY_SAMPLER_ORDER=0`

## Scenes

Run each scene on **`dev`** and on **`integration/upstream-port-2`**, same settings, against the Wave 11 exe from `main` as the baseline. Record the overlay for the whole run, not a single frame.

| Scene | How long | What it stresses |
|---|---|---|
| Prison window | A few minutes, camera held on the window and then turned | Small scene, characters, lighting. Catches shader correctness (med3, cube faces, 64-bit compares) |
| Biggest city, driving | Cross the busiest district without pausing | Stutter, texture upload, VRAM. This is the frame-time test |
| Open water | A minute over water, including boats or piers if the scene has them | PR #2 water tiles. Look for dark 16×16 patches |
| Long session | **22 minutes** in the city, do not restart | RAM and commit growth, pipeline-library save, the low-4 GiB reservation |
| 16 GB RAM machine | The city drive and the 22-minute session | The machine this tree is built for. Watch commit and the working set |
| 8 GB card, if you have one | The safe preset, 3 minutes in the city | PR #1. VSync on is expected for that preset only |

Default preset on the 16 GB card: tearing allowed, VSync off, the usual frame-generation settings. Do not use the 8 GB preset for the 16 GB numbers.

## What to write down

From the overlay, for each run: native FPS, FPS with frame generation, GPU percent, VRAM, RAM (working set and commit if the overlay shows them), and any stutter or dropped frames. Note device removal, OOM, and a hang.

Wave 11 baseline on an RX 9060 XT and a 7800X3D, from `main`: about **31 FPS native** and **62–63 FPS with frame generation**. A pass on that class of machine is "not worse than that on native FPS, and no new stutter in the city." A different CPU or GPU gets its own baseline from the Wave 11 exe on that same PC. Do not compare an 8 GB card's safe-preset numbers to the 31 FPS figure.

## Pass / fail

Pass only if all of these are true on `integration/upstream-port-2`:

- [ ] Builds with `cargo xtask policy` and the Zen 3 release command above
- [ ] Prison window: no missing faces, no black props, no device removal
- [ ] City drive: native FPS at or above the Wave 11 run on the same PC, no new multi-second freezes
- [ ] Open water: no dark 16×16 patches that Wave 11 or `dev` did not already have
- [ ] 22 minutes: commit and working set do not climb without bound, no crash on a pipeline-library save
- [ ] 16 GB RAM: no OOM dialog, no silent exit
- [ ] 8 GB card, if tested: stays inside the live budget, no 200 ms stalls
- [ ] Password box, if it appears, does not show the characters
- [ ] `dev` alone (without this branch) is no worse than Wave 11 on the city drive. If `dev` is already worse, say so and do not blame the shader port

Fail if any scene crashes, loses the device, or drops native FPS by more than a frame or two against Wave 11 on the same PC with the same preset.

## If something regresses

`dev`'s community work is **one squash**, `46723850`. Reverting that one commit drops PRs #1, #2, and #3 together:

```powershell
git checkout -b revert-community-squash origin/dev
git revert --no-edit 46723850
```

The branch `integration/community-prs-1-2-3` still has the three merges separately, if you need only one of them:

| Stop after | Commit on that branch | Contents |
|---|---|---|
| PR #1 only | `41dd487` | 8 GB policy and the safe preset |
| PRs #1 and #2 | `8780ab0` | plus AMD fork fixes |
| All three | `2f4e4b9` | plus the 17 upstream commits |

This port branch is two commits on top of `dev`:

| Commit | What | Revert if |
|---|---|---|
| `93c2fb2` | med3 hardware test only | the test fails on the GPU and you want `dev`'s tests |
| `67e866a` | med3, cube denormals, legacy multiply, i64 compares, fallible library buffer | any of those shader changes look wrong, or a save fails |

```powershell
git checkout integration/upstream-port-2
git revert --no-edit 67e866a
```

Bisect between Wave-equivalent source and the port:

```powershell
git bisect start integration/upstream-port-2 origin/dev
# build, run the scene that broke, then:
git bisect good
# or
git bisect bad
```

There are only two commits on top of `dev`, so bisect will land on one of them. To separate the water-tile rewrite from the sampler order, use the environment switches above instead of bisect.

## When it passes: merge `dev` into `main`

Only after the checklist is pass and the report is filled in. `main` does not build from this source tree; it stores the exe.

1. Merge `integration/upstream-port-2` into `dev` (pull request, no force-push). Do this only if the port itself passed. If only `dev` passed, leave the port branch unmerged.
2. Rebuild `victory.exe` on the 16 GB machine with the commands above.
3. From a worktree of `origin/main`, copy **only** the new exe (as `RAZDA4A_XEN.exe`), any changed `presets\*.bat`, and the five `README*.md` files if the numbers changed. Do not copy the `dev` source tree onto `main`.
4. Regenerate `SHA256SUMS.txt` in that worktree. The file is a comment header plus `sha256sum` lines: the exe, `D3D12\D3D12Core.dll`, and the three `fidelityfx\*.dll` files. Replace the exe line with `Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256`. Leave the DLL lines unless those DLLs actually changed.
5. Put the new native and frame-generation numbers into the five READMEs, commit, and open a pull request into `main`. No force-push.

## Report

Fill this in and leave it with the run:

```
PC:
GPU:
RAM:
Build: dev @ <sha>    port @ <sha>    Wave 11 exe sha256:

Scene            Wave11 native/FG   dev native/FG   port native/FG   VRAM   RAM   notes
prison window
city drive
open water
22 min city
8 GB preset

Checklist: pass / fail
Worst stutter:
Crashes or device removal:
Which commit or switch you reverted, if any:
Merge to main: yes / no
```
