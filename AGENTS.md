# AGENTS.md

Read this first if you are a coding agent (or a person) changing this repository, or a fork of it.

## What this is

RYUKI: VIctory, a compatibility layer that runs an Xbox Series title natively on Windows from
files the user supplies, tuned on an RX 7900 XT with 32 GB of RAM. Base:
[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) `a-publier`
(avatarmanz/VIctory-7900XTX's latest with Belobug's AMD changes). What RYUKI added on top is in
[CHANGELOG.md](CHANGELOG.md); the older RYUKI line is the branch `xtx-2026-11-27`.

Rust workspace: `apps/victory` (the program), `crates/victory-loader` (PE loading, patches),
`crates/victory-hle` (the console's runtime services: memory, files, audio, input, ...),
`crates/victory-gpu` (Direct3D 12 back end), `crates/victory-shader` (RDNA2 to DXIL translator).
`docs/architecture.md` has the map.

## The one rule that is never bent

**No game files, code or assets, in any form** (see [docs/asset-policy.md](docs/asset-policy.md)):
no dumps, extracted tables, shader dumps, screenshots of the game, decompiled code. Addresses
inside the user's own executable and byte-checked patches of it are fine, as the existing code
does. No credentials, machine-specific paths or personal data either: `victory.toml` stays local.

## Every change gets a changelog entry

Whatever you change (a fix, a switch, a port from another fork, a launcher default, a revert),
add an entry at the top of [CHANGELOG.md](CHANGELOG.md) in the same commit or pull request:

- **What** changed, and the switch that controls it (`VICTORY_...`, with its default).
- **Why**: the problem it addresses, as observed.
- **Measured how and with what result**: the PC, the build (release or diagnostics), the route,
  the numbers before and after (rendered fps median, slowest 1 % or 10 %, ms per frame, faults).
  Say plainly when something was not measured, or measured no gain.
- **Where it came from**: a port names the fork, commit and author; keep their credit.
- **Tried and not kept** belongs in the changelog too, with the reason: it saves the next person
  the same hours.

Forks: keep this file and the changelog, and keep adding to them; that is how fixes travel
between forks. If your fork prefers its own format, keep at least one dated entry per change.

## Building and testing

```powershell
cargo build --release --locked -p victory
cargo test -p victory-gpu --release --lib          # ~400 tests
cargo test -p victory-hle --release --lib          # runtime services
cargo clippy --workspace --release                  # no new warnings
```

Before measuring a change in the game:
- Translate shaders with the same settings the run uses (`VICTORY_PORT_*` change them):
  `target\release\victory.exe shaders --jobs 14 <game folder>`; then `warm` after a first run.
- Compare runs of the same route from the same save, warm (pipelines built ahead), with the PC
  idle. Single runs differ by a frame or two a second: compare pairs.
- `VICTORY_QUIT_AFTER_S=<seconds>` ends an unattended run by itself.
- Switches that change memory or guards have a `verify` mode where possible: run it before
  turning a change on by default. A verify count that is not 0 means off by default.

## Things that cost hours here

- The title stopping at its own assert about 15 s in, or a loading screen that never ends, on
  every build: the audio output (see the changelog), not the code.
- Frame generation needs the Series S profile; in Series X the title has no FidelityFX upscale.
- More list threads than cores starve the title's own threads: 6 was best on 8 cores, 8 fell to
  about 3.5 fps.
- Runs launched by a script must not kill processes by name: someone may be playing.

## Style

Follow the code around you: comments say what and why in plain sentences, switches are read
once into a `OnceLock`, new behaviour is off by default until measured, and every `unsafe` block
says why it is sound (`// SAFETY:`).
