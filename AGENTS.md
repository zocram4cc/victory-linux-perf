# Agents working on this repository

This file is read automatically by coding agents ([pi](https://github.com/earendil-works/pi), and others that read `AGENTS.md`). It holds the rules every agent follows and how the work is split between them. Humans: it is also a short map of the project.

## What this project is

VIctory with a Linux focus: running an Xbox Series title smoothly and stably under GE-Proton, vkd3d-proton and RADV, tuned for one machine: RX 7900 XT (RDNA3, ReBAR on), Ryzen 9 9900X. Read these before changing anything:

1. [README.md](README.md) section 4: the target architecture (pieces M, W, B, L, T, Q, D), the phases and the tracks.
2. [README.md](README.md), *Known blocker*: the timing-dependent device loss.
3. [docs/memory.md](docs/memory.md) section 10: the design rules drawn from measurements.
4. [docs/journal.md](docs/journal.md): what was tried, newest first.

## Hard rules

- **Never touch the game folder** (`~/Games/...`), not even to read it. No game data in the repository.
- **Never run the game, the launcher (`victory-proton`), `scripts/bench.sh run|mark`, wine or the sandbox.** The machine has one GPU, and the user may be playing: starting our wineserver kills a running game. Ask the maintainer for a run instead (a "Run needed" section in your report, with the exact command).
- **Never push, and never use `gh` or the GitHub API.** Commit locally on your own branch.
- **English** for everything in the repository: code, comments, docs, commit messages.
- **Every number names its source:** a `bench/` folder, a journal entry, or a `file:line`.
- **New behaviour sits behind a `VICTORY_*` switch**, off by default, readable in release builds (not only under `feature = "diagnostics"`).
- **Do not run `cargo fmt` on a whole crate.** Format only what you touched.

On the maintainer's machine, the pi extension `~/.pi/agent/extensions/victory-guard.ts` enforces most of these rules.

## Roles

The agent definitions live in `~/.pi/agent/agents/`. The lead session (the maintainer, with Claude Code) dispatches them, runs the game and decides.

| Agent | Does | Tools | Never |
|---|---|---|---|
| `vr-scout` | finds the code for a task; returns a `file:line` map | read-only, `rg`, `git log/show` | builds, edits |
| `vr-architect` | designs one roadmap piece from the measured data: switch, steps, risks, the runs that decide it | read-only | edits |
| `vr-worker` | implements a design in its own worktree, builds it, commits locally | all, under the guard | runs the game, pushes |
| `vr-reviewer` | reviews a branch for the bugs this project has had: GPU reading freed or remapped memory, heuristic lifetimes, out-of-range raw reads, diagnostics-only switches | read-only git | edits |
| `vr-analyst` | reads existing `bench/` results and profiles, compares runs, writes conclusions | `bench.sh summary/compare`, `perf report` | launches anything |
| `vr-upstream` | evaluates other forks' commits for track U | `git fetch`, read-only | merges |

Workflows (pi prompt templates in `~/.pi/agent/prompts/`):

| Command | Chain |
|---|---|
| `/vr-design <task>` | scout → architect |
| `/vr-build <task>` | scout → architect → worker → reviewer (→ worker once more if the review finds critical issues) |
| `/vr-upstream <forks or ranges>` | `vr-upstream` agents in parallel, merged into one table |
| `/vr-analyze <bench folders>` | analyst |

## How the work flows

1. **Pick a task** from the README task board, in roadmap order. Phase S comes first.
2. **Design** (`/vr-design`). The design goes into the journal or into `docs/design/<task>.md` once the maintainer accepts it.
3. **Build** (`/vr-build`) in a worktree:
   ```sh
   git -C ~/src/victory-royale worktree add ~/src/victory-royale-wt/<branch> -b <branch> p1.8-fast-reads
   CARGO_TARGET_DIR=~/src/victory-royale-wt/target cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
   ```
   The shared target directory keeps worktrees from rebuilding everything, and from overwriting the binary the maintainer runs.
4. **Measure:** the maintainer runs the "Run needed" commands, with `VP_EXE` pointing at the worktree's binary:
   ```sh
   VP_EXE=~/src/victory-royale-wt/target/x86_64-pc-windows-msvc/release/victory.exe scripts/bench.sh run -l <label> --auto VICTORY_<SWITCH>=1
   ```
5. **Decide** (`/vr-analyze`): three valid runs per configuration ([docs/bench.md](docs/bench.md)). A gain keeps the switch, and it becomes the default once it passes the gates. No gain: the result still goes into the journal, and the switch stays off or is removed.
6. **Record:** a dated journal entry, figures in `docs/memory.md` or `docs/baseline.md`, and the task board updated.

## Branches

| Branch | Use |
|---|---|
| `main` | our line; advanced to `p1.8-fast-reads` on 25 Nov 2026 |
| `p<phase>.<task>-<name>` | one task each |
| `fx` | fxceless/main plus our Linux pieces; reference for track U, not a base |
