# Why `dev` is behind Marvisak/VIctory

Checked against upstream `main` at `63bcca43` (2026-11-26, "update readme") and our `dev` at `46723850` (the community-PR squash).

## No shared history

`git merge-base origin/dev` and upstream `main` fails. The two repositories do not have a single commit in common.

| History | Root | Commits | Tip |
|---|---|---|---|
| Upstream `main` | `f4504f64` 2026-11-23 "Initial commit" | **692** | `63bcca43` |
| Our `dev` | `453597e7` 2026-11-27 "RAZDA4A XEN: Rust source…" | **13** | `46723850` |
| Our `main` | `3e1f2bcd` 2026-11-27, a different root from `dev` | **10** | `fd806e1e` |

`dev` and `main` also have no merge base. `main` is the binary release line (Wave 11 exe, presets, READMEs, `SHA256SUMS.txt`). `dev` is the source line. Upstream is a third history.

So "about 600 behind" is the whole upstream history: **692 commits** are not in `dev`, because none of them are ancestors of `dev`. It is not 692 commits of `git log dev..upstream` on top of a shared parent. A normal `git merge` of upstream `main` cannot run.

## The import is a snapshot, then a rewrite

The previous port (community PR #3) called upstream `ffe310eb` (2026-11-24 10:53, "Shaders: the console's waterfall loop…", commit 213 of 692) the import base. That commit is real. It is not a tree parent of our root:

| Compared with our root `453597e7` | Identical file blobs | Shared paths |
|---|---|---|
| `ffe310eb` | 36 | 154 |
| Best overlap, a band of commits through 25 Nov | 59 | about 167 |

Our root contains every path from `ffe310eb` plus **129** paths of our own, and **118 of the 154 shared paths already differ**. Scanning every upstream commit, nothing matches more than 59 files. `crates/victory-gpu/src/host.rs` is the clearest rewrite:

| Tree | `host.rs` lines |
|---|---|
| Upstream `ffe310eb` | 2,055 |
| Upstream `main` today | 4,133 |
| Our `dev` | 13,306 |

The 13 `dev` commits after the import are our own waves, not upstream commits: auto VRAM, submit and descriptor work, open-world recovery, waves 4 through 11 (list pool, residency, raw twins for list views), then the community-PR squash. `feat/auto-vram`, `feat/phase2-30fps`, `feat/wave8-tex-trees`, `feat/frame-pacing`, `feat/submit-30fps`, `feat/community-fixes`, and `feat/open-world-fix` are ancestors of `dev` (their tips are older commits on this same line). `feat/cpu-pipeline` has no merge base with `dev`; its perf work was recommitted on `dev` as `4b16c62`. `feat/open-world-fix` has one commit object that is not in `dev`, with the same message as `7e9c3b9`, which is in `dev` and already cites upstream issues #2143, #2234, #2462, #2844, and #3540.

## Upstream since the import

**479** commits are strictly after `ffe310eb` (`ffe310eb..upstream/main`). By subject prefix:

| Prefix | Count |
|---|---|
| GPU | 124 |
| Shaders | 72 |
| no short prefix (merges and one-off titles) | 155 |
| HLE | 13 |
| Shader tests | 7 |
| Shader | 6 |
| Loader | 6 |
| Formats | 5 |
| XAsync | 4 |
| CLI, README | 4 each |

By what they are about: GPU submit, heaps, title-memory copies, and texture upload; shader lowering (float mode, compares, descriptors, discards); HLE and xmem; crash and device-removal handling; tests and diagnostics. A large share of the GPU commits after 24 Nov replace the title-memory copy and heap design. That design is not the one in our `host.rs`.

## Already on `dev` before this branch

Community PR #3 cherry-picked **17** of those 479, all of them ancestors of upstream `main`. They are inside squash `46723850`, so they are on `dev` now: low-4 GiB reservation, XAsync cleanup without the completion lock, anisotropic point mips, the pipeline-cache publish race, masked password entry, crash-log records, placed-heap test fix, and the smaller test and diagnostic commits. See `docs/integration-community-prs.md`.

These upstream fixes are also already in our sources, reimplemented rather than cherry-picked: `clwb` length at the end of a page (`60d2cf9c`, `crash.rs` already takes a slice), `GetDeviceRemovedReason` (`13185f50`, `device.rs`), and the constant-table placement that was using gigabytes of video memory (`e04c0781`, comment in `lower.rs`).

## Ported on this branch

Six commits. One applied with `git cherry-pick -x`. The other five conflicted in rewritten files; the missing behaviour was copied by hand and the original commits are named in the message.

| Upstream | Author | What landed | Why it should help |
|---|---|---|---|
| `b2a97b19` | Pablo González | Hardware test: `v_med3` orders -0 below +0. Clean cherry-pick | Locks the bug the next row fixes. Windows-only test |
| `77053a0c` | Marvis | `v_med3` flushes denormals, then uses the min/max that order -0 below +0. Not the rest of the commit | Wrong median shows up as speckle and bad lighting on AMD. DXIL's plain FMin/FMax chain returns either zero |
| `77fd8195` | Deliu Alexandru-Darius | Cube-map face coordinates flush denormals to a signed zero | Faces and reflections pick the wrong axis when a denormal survives. Measured upstream as thousands of mismatches on an RX 9070 |
| `704d25ed` | Senchsus | `v_mul_legacy_f32` treats a denormal factor as zero, on our existing multiply | Legacy multiplies otherwise depend on the host's denormal mode. RDNA2 flushes them |
| `cc7ec2a5` | Deliu Alexandru-Darius | 64-bit integer compares run on the high dword, then the low dword | AMD's compiler orders one i64 compare as if the dwords were swapped. Equality was already fine. Affects any shader that compares 64-bit values |
| `321a4bc1` | Kevin | If the pipeline library cannot be mapped, the fallback buffer is reserved with `try_reserve` | An infallible multi-gigabyte allocation aborted the process mid-run. Our save already prefers a file mapping; only the fallback changes |

## Skipped

**456** of the 479 after `ffe310eb` are not in this branch (479 minus 17 already on `dev` minus 6 ported). Plus the **213** commits up through `ffe310eb`, which are the history of the snapshot we already imported in rewritten form.

Not taken, and why:

- **The rest of `77053a0c`.** Discards, f16 rounding modes, and a new `host` feature on `victory-shader` (`Cargo.toml`). AGENTS.md: a new feature flag needs a reason we can stand behind. Our lowering already has its own f16 and min/max paths. Pulling the whole commit would fight those.
- **`f3578e38` (f16 constants in whole-wave `v_add_f16`).** PR #3 already left it out. It needs wave code this tree does not have.
- **GPU title-memory, heap, and upload commits** (`host.rs`, `memory.rs`, `watch.rs`, `texture.rs`, `descriptor.rs`). They are the 25 Nov redesign. Our `host.rs` is a different 13k-line submit path with the 16 GB residency, list pool, and raw-twin work. Cherry-picks of `219878a0`, `136e39d0`, `4fc9d70f`, `53126284`, and `65062e13` all conflicted. Resolving them without a GPU run would mean guessing which side owns the copy and the alias. Not done.
- **`4a53da72` (a loop leaving to a no-output end is not a discard)** and **`e815cef2` (straddling memory writes).** Real correctness fixes. `structure.rs` and `lower.rs` conflicted with our wave64 and water-tile edits. Left for a later port that can run the shader tests on a device.
- **`42961e8e` (TDR log).** Adds a module and edits `victory-gpu/Cargo.toml`. Skipped with the feature/dependency rule.
- **Merge commits and test-only commits** that do not change a result we are missing.
- **Anything already reimplemented** on the waves listed above.

## Risks

- `v_med3` now emits more min/max instructions, including the signed-zero fix, on every median. That is a few extra ALU ops in shaders that use it, not a frame-time rewrite. The hardware test is in the branch and has not been run here (no Windows GPU).
- Flushing denormals in cube coordinates and legacy multiplies changes pixels on purpose. If a scene was accidentally depending on the host's denormal mode, it will look different. That is the console's mode.
- The dword compare changes every 64-bit integer compare, on AMD and on WARP. Equality is unchanged. Ordered compares should start matching RDNA. A bad split of a literal would show up as a compare that is always true or always false.
- The library fallback only matters when the file mapping fails. A failed reserve skips that save and tries later. Pipelines stay counted as added.

None of this was run in a game. The Windows handoff is `docs/TESTING-HANDOFF.md`.
