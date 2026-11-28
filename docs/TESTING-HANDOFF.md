# Testing handoff

One procedure for a tester agent on **Windows 10**, MSVC, an AMD RDNA GPU. Power may be out; do not invent results. If the session dies, resume from the checkpoint section at the bottom.

Do not push to `main` until the last stage says pass. Pull request #6 stays the branch `integration/upstream-port-2`.

Write progress to `logs\handoff-checkpoint.txt` in the clone (that folder is gitignored). One line: stage name, commit SHA, pass or fail.

## 0. Prerequisites

- Windows 10, Visual Studio Build Tools with the MSVC x64 toolchain. Do not use the gnu Rust toolchain.
- Rust comes from `rust-toolchain.toml` (`cargo xtask` will ask rustup to install it).
- The game folder you already use with Wave 11. Do not move it.
- A spare copy of the Wave 11 folder so a bad exe cannot be the only copy.
- One GPU process at a time. If `heavy.lock` exists, wait.

## 1. Clone and build

```powershell
git clone https://github.com/crossducking/RAZDA4A-XEN.git
cd RAZDA4A-XEN
git fetch origin
git checkout main
git rev-parse HEAD
```

Record that SHA. It should be Wave 11 (`fd806e1` or a later `main` that is only a release). Do not build `main`. Run the Wave 11 `RAZDA4A_XEN.exe` that is already in that checkout. That is the baseline.

Build helper, used at every later stage. From the clone root:

```powershell
cargo xtask agility
cargo xtask policy
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --offline --locked -p victory
```

The new binary is `target\release\victory.exe`. Point a **copy** of the launcher at it, or copy `victory.exe` over `RAZDA4A_XEN.exe` in the spare folder only. Launch with `RAZDA4A XEN.bat`, Browse to the game executable, Launch. Do not double-click the game executable.

## 2. Stage A — Wave 11 baseline (`main`)

Scenes, same preset you will use later (tearing on, VSync off, frame generation on, unless the row says the 8 GB preset):

| Id | What | Length |
|---|---|---|
| A1 | Prison window: hold the camera, then turn | 3 min |
| A2 | Biggest city, drive without pausing | 5 min |
| A3 | Open water, including a pier or boat if one is there | 1 min |
| A4 | Intro / early house / a cutscene | 3 min |
| A5 | 22 minutes in the city, no restart | 22 min |
| A6 | Only if the card is 8 GB: preset 4 Safe Mode, city | 3 min |

From the overlay write native FPS, frame-generation FPS, GPU percent, VRAM local and budget, RAM, commit, and any stutter. Known Wave 11 reference on an RX 9060 XT and a 7800X3D: about **31 native** and **62–63 with frame generation**. Any other PC uses its own Wave 11 numbers as the bar.

Checkpoint: `stage A pass <sha>`.

Decision: if Wave 11 itself crashes, stop and report. Do not blame later branches.

## 3. Stage B — `dev` as it is (community PRs #1, #2, #3)

```powershell
git checkout origin/dev
git rev-parse HEAD
```

Expect `46723850` or a descendant that still contains that squash. Rebuild with the helper in section 1.

Repeat A1–A6.

What this stage added:

- PR #1: 8 GB textures at 65%, 2048 MB for targets, 768 MB card buffers. Preset 4 turns VSync on. Use preset 4 only for A6.
- PR #2: water-tile rewrite and static-sampler order, both on. Off switches, no rebuild: `VICTORY_WATER_TILE_SLOTS=0`, `VICTORY_SAMPLER_ORDER=0`.
- PR #3: on every real launch, free address space below 4 GiB is reserved except the lowest 256 MB. Password entry is masked.

Decision:

- City native FPS within about 2 FPS of stage A, no new device removal, no pitch-black world: **pass**. Checkpoint `stage B pass`.
- Fail: revert the whole community squash, rebuild, rerun the scene that failed.

```powershell
git checkout -B stage-b-revert origin/dev
git revert --no-edit 46723850
```

That drops #1, #2, and #3 together. To drop only one, use the unmerged integration branch:

| Keep | Checkout |
|---|---|
| #1 only | `git checkout 41dd487` |
| #1 and #2 | `git checkout 8780ab0` |
| all three | `git checkout 2f4e4b9` |

Those commits are on `origin/integration/community-prs-1-2-3`. Rebuild after each checkout. Checkpoint `stage B fail <scene> reverted <sha>`.

## 4. Stage C — PR #6 upstream port, before the bugfix commits

```powershell
git checkout origin/integration/upstream-port-2
git checkout 8ef7ca1
```

`8ef7ca1` is the docs commit at the end of the port, before the bugfix commits. Rebuild.

Repeat A1–A3 (prison, city, water). You do not need another 22-minute run unless stage B's long run was clean and you have time.

This stage is the med3 signed-zero test (`93c2fb2`) and the shader/library port (`67e866a`): median ordering, cube-face denormals, legacy multiply, 64-bit compares, fallible pipeline-library buffer.

Decision:

- No new sparkle, wrong cube faces, or compare-looking flicker, and FPS still matches stage B: **pass**.
- Fail: `git revert --no-edit 67e866a` (the test commit `93c2fb2` can stay). Rebuild. Checkpoint `stage C fail reverted 67e866a`.

## 5. Stage D — bugfix commits, one group at a time

Stay on `integration/upstream-port-2` at the tip (`git checkout origin/integration/upstream-port-2`). Rebuild once; the later commits are scripts, docs, and compile fixes.

Full table: `docs/WAVE12-13-FIX-PLAN.md`.

| Order | Commit | Bugs | Repro | Pass looks like |
|---|---|---|---|---|
| D1 | `ef4616d` | B01 | Run `warm_shaders.bat` from the folder that contains these scripts | No "'@echo' is not recognized". "0 to make" is a full cache, not a failure |
| D2 | `0fa4e09` | B02, B03 | Read Quick Start step 2. Double-click the game executable once, on purpose | The error dialog still appears if you skip the launcher. The README now says not to. Launch again with `RAZDA4A XEN.bat` and confirm `D3D12\`, `fidelityfx\`, `presets\` are present |
| D3 | `9f33ef0`, `721b263` | CI only | `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --all-features --locked -- --test-threads=1` if you have the time | Clippy gets past `victory-shader`. Diagnostics tests compile. The two old `host::batch` failures may remain |

D3 does not change gameplay. If clippy fails on something else, stop and write the error into the checkpoint. Do not revert D1 or D2 because of clippy.

Bugs with no commit (B04–B06, B08–B10) still get one attempt each, on this tip, so the report has a result:

| Bug | Repro | Record |
|---|---|---|
| B04 | 8 GB card, safe preset, stand in a shaded street | Are the unlit walls black or just dark? VRAM local vs budget |
| B05 | Only if you are on an 8 GB RX 6600 XT. Boot current tip, safe preset, 2 minutes | Pitch black or not. Any `amdxc64` / `0x80000003` |
| B06 | A5, 22 minutes | Commit at 0, 10, 22 min |
| B07 | Do not test a handheld | Mark unsupported |
| B08 | A4 intro / early house, second launch after warm_shaders | Native FPS and GPU percent |
| B09 | A2 city | Native FPS, longest freeze |
| B10 | Turn on a TV or phone, then the car radio | Which has sound |

If D1 or D2 misbehave:

```powershell
git revert --no-edit ef4616d
# or
git revert --no-edit 0fa4e09
```

Scripts and READMEs need no rebuild. Checkpoint after each group: `stage D1 pass` or `stage D1 fail reverted ef4616d`.

## 6. Merge, only if the decisions above passed

1. Merge pull request **#6** into `dev` (the GitHub merge button, no force-push). Skip this if stage C or a gameplay commit in D failed and was reverted; push the reverts to the PR branch first and merge that.
2. On the 16 GB machine, check out the new `dev` and rebuild with section 1.
3. Worktree of `origin/main`. Copy onto it only:
   - `target\release\victory.exe` renamed to `RAZDA4A_XEN.exe`
   - changed `presets\*.bat`
   - the four BOM-fixed scripts if D1 passed: `RAZDA4A XEN.bat`, `warm_shaders.bat`, `config.bat`, `backup_saves.bat`
   - the five `README*.md` if the numbers or the launch text changed
4. Refresh `SHA256SUMS.txt`. Keep the header. Replace the `RAZDA4A_XEN.exe` line with the output of `Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256`. Leave the `D3D12\` and `fidelityfx\` lines unless those DLLs changed.
5. Commit on `main` and push with a normal fast-forward. No force-push. Record `git rev-parse HEAD`.

## 7. Report

```
PC:
GPU:
RAM:
Wave 11 SHA:
dev SHA:
port tip SHA:
final main SHA (or "not merged"):

Stage A  native/FG  GPU%  VRAM  RAM  commit  notes
A1 prison
A2 city
A3 water
A4 intro
A5 22 min
A6 8 GB

Stage B vs A:
Stage C vs B:
Stage D1 B01:
Stage D2 B02 B03:
Stage D3 clippy/diagnostics:

B04 unlit:
B05 pitch black / crash:
B06 commit growth:
B08 intro FPS:
B09 city:
B10 sound:

Reverted commits:
Merged PR #6: yes/no
Merged to main: yes/no
```

## Checkpoint and resume

`logs\handoff-checkpoint.txt` example:

```
stage A pass fd806e1
stage B pass 46723850
stage C pass 8ef7ca1
stage D1 pass ef4616d
```

On a new session: clone or `git fetch`, read that file, check out the SHA on the last pass line, rebuild if the last stage was B or C, and start at the next stage. Do not rerun a stage marked pass unless the binary was lost. If the last line is a fail with a revert, stay on that revert and continue to the next stage only for bugs that do not depend on the reverted commit.
