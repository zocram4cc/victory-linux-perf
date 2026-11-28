## What and why

<!-- What changed, the switch that controls it (VICTORY_..., default), and the problem it addresses. -->

## Measured

<!-- PC, build (release/diagnostics), route and save, before and after: rendered fps median,
     slowest 1 % or 10 %, ms per frame, faults. "Not measured" or "no gain" are fine, said plainly. -->

## Where it came from

<!-- A port: the fork, commit and author. -->

## Checklist

- [ ] An entry at the top of `CHANGELOG.md` (see `AGENTS.md`)
- [ ] No game files, code, assets or screenshots of the game (`docs/asset-policy.md`)
- [ ] `cargo test` for the crates touched, no new clippy warnings
- [ ] New behaviour off by default unless measured; a `verify` mode where it changes memory or guards
