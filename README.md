# victory-memory-stack

A memory stack for **any [VIctory](https://github.com/Marvisak/VIctory) fork**: patches that cut
VRAM and private commit with **no change to what is drawn**. Every change sits behind a `VICTORY_*`
switch that is **off by default**; a launcher (`play.ps1`) turns them on.

This repository holds only the docs and the patches. To browse the same commits in the emulator's
tree, see the branch
[`skornuta/victory-fork` @ `memory-portable`](https://github.com/skornuta/victory-fork/tree/memory-portable).

## Results

RTX 5080 16 GB, Ryzen 7 9800X3D, 32 GB RAM; Series X path at 4K; first area, minute 3 in the
world, warm shader and pipeline cache.

| | Before | After |
|---|---:|---:|
| VRAM | ~12.6 GB | **8.55 GB** |
| Private commit | ~26.0 GB | **~20.3-20.6 GB** |
| Working set | 15.1 GB | **~12.85 GB** |
| FPS | ~17 | 16.5 median / 17.8 p90 |
| Pop-in, flicker, device losses | | **none seen** |

## Apply directly (forks close to the base)

The series is cut against
[`e94251a`](https://github.com/skornuta/victory-fork/tree/e94251a). On a fork whose code is close to
it, the patches apply as they are:

```sh
git clone https://github.com/skornuta/victory-memory-stack
cd your-victory-fork
git checkout -b memory-stack             # from your own branch
git am --3way ../victory-memory-stack/patches/*.patch
cargo build --release
cargo test --release -p victory-gpu
```

Then launch with `play.ps1`, or set the switches yourself (the table in
[MEMORY_STACK.md](MEMORY_STACK.md)).

## Port into any other fork

Where `git am` does not apply (a fork that has moved further from the base, such as Vice-Tay or
AnyXboxX), the patches are the specification. Port feature by feature, in the order and with the
invariants in [AGENTS.md](AGENTS.md). It is written so an AI coding agent (Claude Code, Codex,
Cursor) can follow it: *"port the memory stack from victory-memory-stack into my fork"*.

## Contents

| | |
|---|---|
| [MEMORY_STACK.md](MEMORY_STACK.md) | What each switch does, the cherry-pick order, the hook points |
| [AGENTS.md](AGENTS.md) | How to port: path A (apply directly) and path B (port feature by feature), invariants, verification |
| [patches/](patches) | `0001`-`0027`, `git format-patch` of `e94251a..memory-portable` |
