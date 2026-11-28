# Wave 12–13 fix plan

Deduped from the 28 Nov team list, the bug cards, and the last-12-hour posts. One row per bug, not per person. Nothing here was run in a game. Hypotheses are labeled.

## Bug table

| Id | Symptom | Reporters | Hardware | Wave / commit | Severity | Suspected area | Status |
|---|---|---|---|---|---|---|---|
| B01 | First line of a `.bat` is not a command (`@echo` with a hidden character). Warm-up then says 0 pipelines to make | [@FernVarens](https://x.com/FernVarens/status/2126576538925572219) | RX 7900 XTX, 7800X3D, 48 GB | After Wave 11 | Cosmetic | Launcher text encoding | **FIXED** |
| B02 | System error: a console DLL was not found (`d3d12_xs.dll` and the others in the same dialog) | [@Y30v0](https://x.com/Y30v0/status/2126584160374419856) | Not given | Unknown | Does not start | User launched the game executable | **USER ERROR** |
| B03 | "Do I put all these files here?" Folder listing has no `D3D12`, `fidelityfx`, or `presets` | [@SBut67](https://x.com/SBut67/status/2126587681886884220) | Later: RX 9060 XT, 5–8 FPS | Wave 11 zip | Install | Unpack / launcher | **USER ERROR** (the FPS part is B08) |
| B04 | Places with no sun go partly black. Textures otherwise better. 5–7 FPS. Overlay: local VRAM above budget, RAM 15.2/15.7, commit 19.1 | [@xuebaiqian](https://x.com/xuebaiqian/status/2126571403701109203) | RX 9060 XT 8 GB, R5 7500F, 16 GB | Wave 11 | FPS + image | 8 GB residency / missing lights | **NOT FIXED** |
| B05 | On `dev` `f8bbae3`, textures pitch black on scene load. Then `amdxc64.dll` crash or `0x80000003` at `victory-image` + `0x4dcbc9d` about 14 s in. Older `659d074` + Safe Mode was fine | [@Fooudub](https://x.com/Fooudub/status/2126596340071444591) | RX 6600 XT 8 GB, R7 5700G, 16 GB, pagefile on NVMe | `f8bbae3` (before the community squash) | Crash + image | 8 GB eviction / a debug break | **NOT FIXED** |
| B06 | Commit and RAM climb over a long session (about +3 GB in 54 min), FPS 22 to 16, audio stutters. "VRAM" keeps rising | [@brpt369](https://x.com/brpt369/status/2126576256929980491) | RX 6700 XT, R5 5600, 16 GB. Was on Wave 5 after Wave 7 black textures | Wave 5 | Stutter | Commit not returned after reclaim | **NOT FIXED** |
| B07 | "VRAM 23 and it keeps shooting up" on a handheld | [@YoungTeddy97](https://x.com/YoungTeddy97/status/2126582758717960243) | ROG Xbox Ally X (shared-memory APU) | Unknown | Unsupported | Shared memory reported as VRAM | **USER ERROR** |
| B08 | Intro, early house, and cutscenes stay at about 5–14 FPS on strong AMD PCs. One 9060 XT never reaches the prison scene | [@Coltxkila](https://x.com/Coltxkila/status/2126593024365150695), [@Mrbl_ue](https://x.com/Mrbl_ue/status/2126598112508879124), [@Akshaykalkal93](https://x.com/Akshaykalkal93/status/2126604961110962296), [@SBut67](https://x.com/SBut67/status/2126597441357070362), [@CyrusTheDoggo](https://x.com/CyrusTheDoggo/status/2126570633886630167) | 6900 XT + 5800X; 9070 XT + 9700X + 32 GB; 9060 XT 16 GB + 7600X + 16 GB; 9060 XT | Mixed, some after Wave 11 | FPS | CPU submit / shader compile, not the GPU percent | **NOT FIXED** |
| B09 | City and crowds drop to a few FPS or freeze the frame. Second launch worse than the first until shaders are warm | [@maxiip96](https://x.com/maxiip96/status/2126562377231409482), [@AlimanSyahrul](https://x.com/AlimanSyahrul/status/2126557760833892490), [@bearerobadnews1](https://x.com/bearerobadnews1/status/2126565255014924405), [@kwii8h](https://x.com/kwii8h/status/2126496974862274837), [@aironside3](https://x.com/aironside3/status/2126565158365675552) | 9070 XT, 7900 GRE, 7900 XTX, 6600, mostly 16–64 GB | Mostly before Wave 11 | FPS / freeze | City CPU path. Team note: a root cause is believed found, not landed here | **NOT FIXED** |
| B10 | TVs, radios, and phones are silent. The car radio has sound | [@prisonmaique](https://x.com/prisonmaique/status/2126600960231490030) | Not given | Unknown | Audio | A world emitter path, not the file the car plays | **NOT FIXED** |

Out of scope, already answered, not given a fix commit: RDNA1 and NVIDIA are unsupported (README). Antivirus false positives: verify `SHA256SUMS.txt`. Linux 1 FPS / blank 3D: Wave 11 `launch_linux_proton.sh`.

## Per bug

### B01 — FIXED, untested

- Commit: `ef4616d` `fix(launcher): drop the UTF-8 BOM from the command scripts`
- Change: `RAZDA4A XEN.bat`, `warm_shaders.bat`, `config.bat`, and `backup_saves.bat` are the main copies with the three BOM bytes removed. They now start at `@echo off`.
- Verify: `cmd /c warm_shaders.bat` from that folder. The first line must not say the command is not recognized. "0 to make" still means the shader cache is already full.
- Revert: `git revert --no-edit ef4616d` and rebuild is not required (scripts only). When promoting to `main`, copy these four files over the copies there.

### B02, B03 — USER ERROR

- Commit: `0fa4e09` `docs(readme): start from the launcher, and keep the unpacked folder whole`
- The five READMEs now say: do not double-click the game executable, and keep `D3D12\`, `fidelityfx\`, and `presets\` in the unpacked folder.
- Verify: read Quick Start step 2. A double-clicked game executable still cannot work; the text is the fix.
- Revert: `git revert --no-edit 0fa4e09`

### B04 — NOT FIXED

Hypothesis, not a measurement: the two frames show local VRAM (6.5) above the live budget (6.2–6.4) and a 16 GB machine with commit already at 19.1. Unlit areas may be the meshes whose light or shadow data was the first thing dropped. The frames themselves still show textures, so this is not the full pitch-black of B05. PR #1 (already on `dev`) admits 8 GB textures at 65% instead of 78%. That may be enough, or it may not. Do not tighten residency again until a Safe Mode run on this PC is logged.

Need: `collect_logs.bat` output, the preset name, and one shot where the dark patch is actually in frame.

### B05 — NOT FIXED

Hypothesis: `f8bbae3` is the wave 6–7 descriptor change, older than the 8 GB fit and older than this branch. Pitch-black on load plus a driver crash in `amdxc64.dll` or a breakpoint (`0x80000003`) fits a descriptor or resource the 8 GB eviction left invalid. The same PC on `659d074` + Safe Mode did not do this. Current `dev` is not `f8bbae3`. Retesting that commit is the first step, not another code change.

Need: the sanitized `logs\bug_report_*.txt` he said he attached (it is not in this repo), and a repeat on current `dev` with the safe preset.

### B06 — NOT FIXED

Hypothesis: reclaim runs (he says it does) but committed memory is not released back to Windows, so a 16 GB PC slowly runs out of standby and the frame time climbs. This branch only makes the pipeline-library fallback allocation fallible (`67e866a`); it does not free commit over a 22-minute session.

Need: overlay commit and RAM at 0, 10, and 22 minutes on current `dev`, plus whether a restart clears it. Do not treat the Ally X report (B07) as the same bug.

### B07 — USER ERROR

The Ally X has no discrete VRAM. The number he calls VRAM is shared RAM. Handhelds and iGPUs are not a supported target. No code change.

### B08 — NOT FIXED

Hypothesis: overlay GPU percent in the 8 GB shots is 24–37% while the frame is 150–200 ms, so the wait is on the CPU submit/compile side, not on the Radeon being full. A first launch compiles pipelines (minutes). A 9060 XT that never reaches the prison scene may still be on that first compile, or on the install mistake in B03. No FPS patch in this branch. Changing submit without a trace would risk the 31 FPS Wave 11 path.

Need: native FPS (not the frame-generation number), GPU percent, and whether a second launch after `warm_shaders.bat` is faster. Wave name from the exe they actually started.

### B09 — NOT FIXED

Same family as B08, in the city. The team list says a root cause for the city stall is known and belongs to a later wave. It is not in this branch. A blind change to the submit loop is the opposite of low-risk.

Need: a city drive overlay (native FPS, SUB ms, GPU ms) on Wave 11 versus current `dev`.

### B10 — NOT FIXED

Hypothesis: the car radio is a file Media Foundation is allowed to open (`modules/media.rs` only forwards local files). TVs, phones, and room radios may be a different voice or a URL the resolver refuses on purpose (it must not fetch the network). No change until a log shows which call fails.

Need: a few seconds of log with a TV on and the car radio on, same session.

## Internal CI, same branch

Not a player bug. Windows CI on `dev` (`46723850`) already fails, and this branch failed the same way until the two commits below.

| Commit | What | Revert |
|---|---|---|
| `9f33ef0` | `lower` / `lower_counted` allowed as dead in the non-test build. Water-tile length check uses `is_multiple_of` | `git revert --no-edit 9f33ef0` |
| `721b263` | Diagnostics feature: bitset read, and the UAV closure does not borrow `self` | `git revert --no-edit 721b263` |
| `fix(ci): clippy on victory-gpu and the stale keep-line tests` | Collapsible conditions and a derivable `Default`, no behavior change except the two tests. HDR dumps of formats 9–10 and 26 already use a cube root, so a half-bright channel is not 127. Past 93% of the budget the keep line trims to 76%, which is 19 GiB of a 100 GiB example, not 11 | `git revert` that commit if a keep-line number looks wrong in game |

Checked here: Windows-target `cargo check --workspace --all-targets`, the same check with `--all-features`, shader/format/pipeline/xtask tests, `cargo xtask policy`. The only test failure left is the pre-existing `documented_switches_are_read` hit for a switch named in `AGENTS.md` and not read by Rust. No new failures from these commits. GitHub CI has not been re-run yet.

## Wave 12 (next, still no code until a log says so)

1. Retest B05 on current `dev` with the safe preset before touching residency again.
2. One 22-minute city session for B06. If commit climbs with reclaim already firing, that is the leak to chase.
3. B04 dark unlit areas: only if they remain on the safe preset with a screenshot that shows them.
4. B10: one TV-on log, then a targeted audio fix.

## Wave 13

1. B08/B09 city and intro CPU time, after Wave 12 has a clean baseline. The submit and descriptor hot path is the place, and it needs a trace, not another guess.
2. Whatever B06's log names (cache, staging, or pipeline library) gets a bound. The fallible library buffer is already in `67e866a`.
