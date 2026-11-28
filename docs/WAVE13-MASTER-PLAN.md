# Wave 13 master plan

One order of work for the Windows agent. Bench: Windows 10, AMD RX 9060 XT 16 GB, Ryzen 7 7800X3D. Baseline to beat is **Wave 11 `main`**, about **31 native** and **62–63 with frame generation**, measured separately. Do not call the frame-generation number "native".

This file is the order. `docs/TESTING-HANDOFF.md` is how to build, how to resume a dead session, and how to land on `main`. When they disagree about what to try next, this file wins. When they disagree about the build or the merge commands, the handoff wins. Do not paste either document into the other.

Do not merge, do not push `main`, and do not force-push. The owner says when a result may land.

## Step 0 — confirm the finding still applies

Before editing code for a bug or an FPS idea, reproduce it on the **Wave 11 executable** (or show the source path is absent and mark the item stale). A note that was true of an older wave is not a Wave 13 task.

Write the result in the next checkpoint: applies, does not apply, or stale. Sources to walk:

- `docs/WAVE12-13-FIX-PLAN.md` (B01–B10)
- `docs/WAVE13-FPS.md` (section 3 top 10, then section 4)
- The 28 Nov community notes summarized below

| Item | What was claimed | What this repo shows without a game | Do this before coding |
|---|---|---|---|
| B01 BOM | First line of the warm-up bat is not a command | Wave 11 `main` still has a UTF-8 BOM on `RAZDA4A XEN.bat`, `warm_shaders.bat`, `config.bat`, `backup_saves.bat`, and `launcher_ui.ps1` (bytes `EF BB BF`). This branch's four bats are already clean (`ef4616d`). The copy of `launcher_ui.ps1` added here is clean. Preset 4 on `main` has no BOM. | Fixed in source, not in the Wave 11 zip. Confirm `cmd` on the zip still shows the bad first line, then the fixed scripts are the patch. Not stale. |
| B02, B03 direct exe / unpacked folder | Missing console DLLs, or a folder with no `D3D12` | User launch, not a runtime bug. READMEs on this branch already say so (`0fa4e09`). | No code. Confirm the zip's README still tells people to start `RAZDA4A XEN.bat`. |
| B04 unlit black, 8 GB | 5–7 FPS, local VRAM over budget, RX 9060 XT 8 GB | Not fixed. One report with a screenshot, Wave 11. | Reproduce on that class of PC, Safe Mode, before any residency change. |
| B05 pitch black + driver crash | `f8bbae3` on an RX 6600 XT | That commit is older than current `dev`. | Retest current `dev` with the safe preset. If it does not crash, mark the `f8bbae3` case stale for this wave. |
| B06 commit growth | +3 GB class climb, FPS 22 to 16 | The cited session was **Wave 5**, not Wave 11. | 22 minutes on Wave 11. If commit stays flat, mark stale for Wave 11. |
| B07 Ally X "VRAM" | Shared memory called VRAM | Unsupported APU. | Do not work it. |
| B08 intro / house / cutscenes | About 5–14 FPS on strong AMD PCs | Not fixed. Several reports, mixed waves. | One Wave 11 run after `warm_shaders.bat`, native and frame generation written separately. |
| B09 city freeze | A few FPS, second launch better | Mostly older waves. Open. | Same as B08, city scene, Wave 11 vs this branch. |
| B10 silent TVs and phones | Car radio works | Not fixed. One report. | Log with a TV on and the car radio on. No mixer sliders (the launcher spec forbids inventing them). |
| 7800X3D + 9070 XT crash | Asked about in the notes | The notes' author did not find it in the replies. The only "crash" they found was an NVIDIA card, which this fork does not support. | Stale until someone reproduces it on Wave 11. Do not start from it. |
| Water / texture rewrite | No measured fix in those notes | Textures on Wave 11 were described as better by one 8 GB report. No water method was collected. | P2 only, and only after a before/after on this bench. |

FPS ideas get the same gate: grep the switch on this branch (the table at the top of `docs/WAVE13-FPS.md` is the grep result as of this plan). If the name is marked "not a switch in this tree", it is a port, not a launcher line.

## Priorities

### P1 — Framerate

A/B every item in `docs/WAVE13-FPS.md` against the Wave 11 baseline, one change at a time, same save, same camera, at least 3 minutes after a warm run. Keep a change only when native **or** frame generation improves and the other does not regress, with no new black meshes, no missing faces, and no commit climb. Revert the rest.

Record native and frame generation as two numbers. Scene name, preset name, and the exact variable under test go in the checkpoint.

Order for the cheap switches (section 4 of the FPS notes), each against baseline, not stacked, until that one switch has a keep/revert:

1. `VICTORY_HEAP_RECHECK=0`
2. `VICTORY_PIPELINE_WAIT_MS` at 24, then 0, then 100. Leave `VICTORY_SCREEN_WAIT_MS` unset (code default 10 000). Leave `VICTORY_COMPUTE_WAIT_MS` at 5000. Do not lower the compute budget (character faces). Run `warm_shaders.bat` first (`RAZDA4A_XEN.exe warm --used --jobs 6` is already that file).
3. `VICTORY_LANE_CORE=1`
4. `VICTORY_THIN_THREADS` 6 and 8, and `VICTORY_JOIN_PARTS` 8 / 12 / 16, one at a time
5. `VICTORY_LEVELS_ON_DEMAND=1` and a 20-minute memory curve (this one is also the start of P2 if the picture is wrong)

The opt-in bundle is preset 5 and launcher profile `wave13_fps`: heap recheck off, pipeline wait **16** ms, lane core on, on the balanced 16 GB block. It is not the default (`XEN_PROFILE` stays `balanced`). Measure the three switches separately first. Use the bundle only as a follow-up run of the ones you kept. 16 ms is the preset value; the A/B list is still 0 / 24 / 100. If 16 was never the winner, do not ship 16.

### P1 gated ports — do not start these until the five config steps above have keep/revert results

Each is one commit, behind the switch named, default off. Revert if the bench regresses. Exact sources from the FPS notes (re-fetch before cherry-pick; forks rewrite history):

| Gate | What is in this tree | Source commits to port, in this order | Do not |
|---|---|---|---|
| Renderer port 1 ordering | `VICTORY_PORT_CONSTANTS` exists and is off unless the value is `1`. There is no order switch. | `631400f5`, `15c3d0ea`, `3e04ca0a`. Copperframe `8ba4f34d` ships the ordering off; match that until an A/B says otherwise. | Do not turn the port on in a default preset. |
| In-place buffers | `VICTORY_INPLACE` exists (`0` / `1` / `2`, default `0`). The port-buffers name from the notes is not a switch. | A/B the existing `VICTORY_INPLACE` values first. Only then `b2917b48`, `4487275b`, `5e6fec2d`, `4c210deb`, `540dc6cc`. The published comparison is `730f8e82`. | Do not combine with late-wait and the port in the first pass. 16 GB commit is the risk. |
| GPU detile | No such switch. | fxceless `3461f52f` (`gpudetile.rs` and the texture format helper), CPU fallback left on. | Not a launcher line today. |
| Title affinity ignore | No such switch. Upstream pull request 3635. | Port the hooks only after the config steps. `cores.rs` already places threads; the two must not fight. `CCD_SPLIT` does not apply to the single-CCD 7800X3D. | Do not invent the env name in a preset. The experimental preset does not set it. |

Also out of P1 until the frame is no longer CPU-bound: GPU overlap, a VRAM shadow of buffers, and shader-flag bundles. They target a full GPU.

### P2 — Textures and water, secondary

Only after P1 has a measured keep/revert for the config steps. Water-tile rewrite, on-demand mip levels, and any other texture path land only when a before/after on this bench is obviously right (the lit scene, the 8 GB unlit scene if you have that PC, and a 20-minute commit curve). If it is ambiguous, revert. Do not let P2 block P3's escape hatch.

### P3 — GUI launcher v1

Implement `docs/GUI-LAUNCHER-SPEC.md`. v1 does not change the runtime. It may be released to `main` **on its own, earlier**, if P1 is still in A/B. That early release is the escape hatch: scripts, the dialog, and docs only. It does not replace `RAZDA4A_XEN.exe` and it does not bump the wave number of the executable.

The current dialog (`launcher_ui.ps1`) already has a fifth profile, `wave13_fps`. Keep that entry when you replace the dialog.

## Required tasks (do them inside the priority that fits)

### (a) Branding

Everywhere the perf overlay, the stats, or the graphs show `VIctory`, show `RAZDA4A XEN`. Add a small-font watermark `github.com/crossducking/RAZDA4A-XEN` on the stats overlay so a screenshot identifies the build.

Places, as of this branch:

- `crates/victory-gpu/src/host.rs` around 11211 and 11213. The title bar is the original caption plus ` | VIctory {fps}`. That is the string screenshots show. Use `RAZDA4A XEN` there. With frame generation the line is ` | VIctory {fps*2} fps ({native}x2 FG)`.
- `crates/victory-gpu/src/host/panel.rs` `paint_hud` (starts around line 573). The canvas is 540×354. It does **not** print a product name today. The large header is `{shown} FPS` (`s.fps * 2` when frame generation is on) and a scale-2 badge `{native}X2 FG`. Add `RAZDA4A XEN` on that header. `Canvas::text` scale `1` is already used for the graph's 33 ms / 66 ms labels; use scale 1 for the watermark along the bottom edge.
- Win32 class name `VIctory` at `host.rs` 12248, 12271, and 12294 is not the title text. Leave it unless you have checked nothing finds the window by class.

Do not rename in the same commit:

- `crates/victory-gpu/src/dxgi.rs` line 215, the adapter string `VIctory console GPU` (the title can see it).
- Log and crash prefixes `VIctory:`. `crates/victory-loader/src/thunks.rs` asserts that prefix. `load_probe.rs` asserts `VIctoryProbeDelayed` / `VIctoryProbeNotImplemented`.

One commit for the title bar and the HUD wordmark plus watermark. Screenshot the overlay with frame generation on and off.

### (b) Cleanup of other forks

This tree is a VIctory fork that cherry-picks. Remove leftover fork names from code, paths, and scripts when they read as if that fork were us. Keep credits in docs for people who actually helped. Belobog / Belobug may stay in those credits.

No Belobog install path was found on this branch. What is here are comments. Known sites: `crates/victory-shader/src/emit/lower.rs` around 6121 (`VIctory-Belobug-AMD`), `crates/victory-gpu/src/pipeline/host.rs` around 160, `crates/victory-gpu/src/pipeline/make.rs` around 21, `crates/victory-gpu/src/ambient.rs` around 12, `crates/victory-gpu/src/residency.rs` header, `draw.rs` comments that name codec-d/cool_fox, `crates/victory-hle/src/modules/audio.rs`, `crates/victory-hle/src/modules/dstorage.rs`.

Replace a fork name in a code comment with the commit hash, or move the credit to `docs/integration-community-prs.md`. Do not change behavior in that commit. NVIDIA readers still need the README pointer to a community NVIDIA fork; that pointer stays.

`docs/integration-community-prs.md` is the credit file. Do not strip it.

### (c) Step 0

Already the first section. Do not skip it when a new note arrives.

### (d) Checkpoint reports

After every 5–6 commits, or after any change you are keeping because the numbers moved, write `docs/reports/checkpoint-NN.md` (01, 02, …). Commit it. The short summary at the bottom is what the user forwards. Do not make them read the log.

```
# Checkpoint NN

Branch:
HEAD:
Date:
Bench: Windows 10, RX 9060 XT 16 GB, 7800X3D, 16 GB RAM
Wave 11 exe SHA:

## Tried
- item:
  change (commit):
  scene:
  settings / preset:
  native FPS:
  frame generation FPS:
  frame ms:
  GPU %:
  VRAM local / budget:
  RAM:
  commit:
  visual (faces, lights, water, pop-in):

## Kept
- 

## Reverted
- commit:
  command:
  why:

## Stale / does not apply
- 

## Next
- 

## Forward this
One short paragraph: what changed, the two FPS numbers versus Wave 11, and whether to keep going.
```

Also append one line to `logs\handoff-checkpoint.txt` (gitignored) as the handoff describes, so a crashed session can resume.

### (e) Resume, revert, merge

Resume:

```
git fetch origin
git checkout integration/upstream-port-2
git pull origin integration/upstream-port-2
git log -1 --oneline
```

Read the newest `docs/reports/checkpoint-*.md` and `logs\handoff-checkpoint.txt`. Continue at that checkpoint's "Next". Rebuild with `docs/TESTING-HANDOFF.md` section 1 if the last kept commit touched Rust.

Revert one commit (no force-push):

```
git revert --no-edit <sha>
```

Revert a file to a known good blob:

```
git checkout <good-sha> -- path\to\file
```

If a stacked A/B was reverted, the next A/B starts from baseline again.

Merge to `main`, only after the owner says so, and only for what passed:

1. Follow `docs/TESTING-HANDOFF.md` section 6 for the executable path, the worktree of `main`, and `SHA256SUMS.txt`.
2. Rebuild `RAZDA4A_XEN.exe` from the commit you are shipping. `Get-FileHash .\RAZDA4A_XEN.exe -Algorithm SHA256` replaces the exe line. Leave the `D3D12\` and `fidelityfx\` lines unless those DLLs changed.
3. Wave number: read the wave line on `main` first. An executable release increments it by one (Wave 11 is current as of this plan). A launcher-only early release (P3 escape hatch) does not increment it and does not replace the exe or its hash.
4. Launcher-only ship copies scripts and docs: `launcher_ui.ps1` (no BOM), `RAZDA4A XEN.bat`, `presets\5_Wave13_FPS_Experimental.bat` if that profile is still a measured keep, and the README lines that mention the profile. Do not copy this branch's preset 4 over Wave 11's preset 4 without looking: Wave 11 preset 4 has frame generation **off**; this branch's copy has it **on**.
5. Full ship also copies the new exe, any kept preset, the five READMEs with the measured numbers, and the checkpoint summary's native and frame-generation pair.
6. Commit on the `main` worktree and fast-forward push. No force-push. Record `git rev-parse HEAD`.

## What this branch already added for P1

- `docs/WAVE13-FPS.md` — community notes, with corrections for this tree.
- `docs/GUI-LAUNCHER-SPEC.md` — launcher v1.
- `presets/5_Wave13_FPS_Experimental.bat` — opt-in bundle.
- `RAZDA4A XEN.bat` profile `wave13_fps` and `launcher_ui.ps1` (BOM removed, fifth combo entry). Default profile is still `balanced`.

Nothing in that list has been run in a game.
