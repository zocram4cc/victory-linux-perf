# Running under Proton on Linux

`victory.exe` is a Windows program; these scripts cross-build it on Linux
and run it through `umu-run` under a Proton build, which supplies Direct3D 12
by way of vkd3d-proton (3.2.0 or newer: see `run.sh`).

| Script | What it does |
|---|---|
| `build.sh` | Cross-builds for `x86_64-pc-windows-msvc` with lld-link and the SDK that `xwin` unpacked. `build.sh` is the release build, `build.sh --profile diag --features diagnostics` the one with counters, the F3 panel, the timeline and the sampler. |
| `run.sh` | Sets up the prefix and starts the title, or runs any other command of `victory.exe` (`shaders <dump>`, `warm`). |
| `play.sh [name] [VAR=value ...]` | Starts the title with no time limit; the log goes to `$STATE_DIR/bench/<name>.log`. Diagnostics build unless `PROFILE=release`. |
| `bench.sh <name> <seconds> [VAR=value ...]` | Starts it with the intro skipped, stops it after so many seconds and prints what the frames cost. |

Machine settings go into `linux/local.env` (git-ignored): `GAME_DIR`,
`PROTONPATH`, `STATE_DIR`, `D3DCOMPILER` as `run.sh` describes them, and any
`VICTORY_*` switches. (An example file with the switches measured on an
RX 7900 XTX is not in the repository yet: the policy check does not allow
its file type.)

Measuring a switch: runs from the same save land in scenes of 1,500 to 7,000
draws, so two runs do not compare. `VICTORY_AB=<seconds>,<SWITCH>,...`
(diagnostics build) flips switches read with `toggled!` inside one run and
logs an "A/B phase" line for each phase; fit frame time against draws and
the switches over thirty or more phases. Ten phases have shown a 3 ms effect
that forty then showed to be the other way.

What a frame waits for: `VICTORY_TIMELINE='60+85+110,4,Z:\path\tl.json'`
writes a trace of four frames at each of those seconds, and a `.txt` beside
it with the chain of work and waits each present came out of. Where threads
are: `VICTORY_SAMPLE=<start s>,<seconds>,<interval ms>,<thread name>|...`
(`*` for all).

`wine/` holds two changes to Wine and how to build them.
