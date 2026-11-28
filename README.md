# clankerVI (`victory-proton` Performance Fork)

Performance-focused Linux/Proton fork of `victory-proton` targeting native **30+ fps** playable performance without frame generation on AMD Ryzen 7 9700X + Radeon RX 7900 XTX (RADV, NixOS).

## Key Optimizations Implemented
- **Wine `ntdll.so` patch** ([linux/wine-virtual-writewatch.patch](linux/wine-virtual-writewatch.patch)):
  - Enables Linux kernel write-watch (`UFFDIO_REGISTER_MODE_WP` + `PAGEMAP_SCAN`) on `MapViewOfFile3` section views (`VirtualAlloc(..., MEM_COMMIT | MEM_WRITE_WATCH, ...)`), so `VICTORY_SECTION_ALLOC=wc` backs the 2.15 GB write-combined GPU heap with a section + `VICTORY_LAND_BESIDE=1` without page faults, cutting `GetWriteWatch` dirty pages by **76%** (6,323 → 1,518) and `VICTORY_SEND_OWN` upload volume by **45%** (39 MB → 21 MB/frame).
  - Releases Wine's global `virtual_mutex` before `ioctl(pagemap_fd, PAGEMAP_SCAN)` in `NtGetWriteWatch`, enabling true parallel kernel write-watch scans under `VICTORY_ASK_SPREAD=1`.
- **Submission lane & worker pool (`victory-gpu`)**:
  - Zero-allocation scoped `EachPool` with chunked atomic dispatch for `wide_each` and parallel `copy_all` / `take_snapshots` (`crates/victory-gpu/src/shadow.rs`).
  - `Shadow::landed` (`VICTORY_LAND_TAKEN=1`) extended to kernel write-watched sections (`watched == 1`), eliminating ~15–18 MB/frame of redundant copy-back re-uploads.
  - Lock-free `KEPT` fast-path for `ask_once` / `ask_ahead`, per-shard `max_written: [AtomicU64; SHARDS]` + single-page lookup fast-path in `watch::Notes::all_unordered_since`, and 64 MiB chunked parallel `ask_ahead_held` (`crates/victory-gpu/src/watch.rs`).
  - Per-thread `TLS_VIEWS` / `TLS_SHAPES` caches (`descriptor.rs`) and lock-free `HostHeap::keep` fast-path (`descriptor/host.rs`).
  - AVX2 4×32B vectorized `texture::digest` (`texture.rs`), 128B AVX2 fast-path in `victory_safe_merge` (`safecopy.rs`), and inline small-copy fast-path in `fastmem.rs` (`VICTORY_FAST_MEMCPY=1`).
  - `VICTORY_SLEEP0_YIELD=1` in `victory-hle/src/modules/kernel.rs` to throttle tight `Sleep(0)` spin loops (eliminates a 68% CPU spin thread).
  - Thin LTO (`lto = "thin"`) and `x86-64-v3` (AVX2/BMI2/FMA) codegen.

## Measured Progress (Native, Zero Frame Gen, `VICTORY_UPSCALER=6`)
| Milestone | Console / Res | FPS | Frame Time | `preparing` |
|---|---|---:|---:|---:|
| Baseline (`f353c97`) | Series X @ 2560×1440 | 24.7–25.3 fps | 39.6–40.4 ms | 32–34 ms |
| Waves 1–5 (TLS views, `HostHeap::keep`, `EachPool`, `Sleep(0)`, `shard_max`, AVX2 digest) | Series X @ 2560×1440 | 26.0–27.7 fps | 36.0–38.4 ms | 28.5–31.8 ms |
| Wave 8c/8d (Wine `UFFD_WP` section write-watch + `LAND_TAKEN` + parallel `PAGEMAP_SCAN` + `JOIN_BARRIERS=1`) | Series X @ 2560×1440 | 28.5–28.8 fps | 34.8–35.1 ms | 23.6–26.3 ms |
| Wave 11 (`PAGE_MAX`, AVX2 `safecopy`, worker priority, `THIN_THREADS=6`, `GUARD_HOT=500`) | Series X @ 2560×1440 | **29.5–30.8 fps** | **32.5–33.9 ms** | **26.7–27.6 ms** |
| Wave 11 (same + full `GATE=1 GATE_REACH=1` correctness) | **Series S** | **30.3–31.7 fps** | **31.6–33.0 ms** | **23.2–25.4 ms** |

## Play
```bash
# Series S (sustained 30.3-31.7 fps with full GATE_REACH=1 correctness)
./linux/play-nixos.sh

# Series X @ 2560x1440 (29.5-30.8 fps)
CONSOLE=series-x ./linux/play-nixos.sh
```

## Build
```bash
CARGO_TARGET_DIR="$HOME/.cache/vp-upstream-win" \
RUSTFLAGS="-C target-cpu=x86-64-v3" \
cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
```

---

# victory-proton

[VIctory](https://github.com/Marvisak/VIctory), the compatibility layer that runs
an Xbox Series title from files the user supplies, running on **Linux under
Proton**, ray tracing included.

Built on [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)
(its original README: [docs/README-7900XTX-fork.md](docs/README-7900XTX-fork.md)),
with the changes Linux needed and a much faster ray-tracing path.

Since 2026-11-27 the runtime is a faster tree tuned for Linux under Proton on
the RX 9070 XT: the info panel (`VICTORY_PANEL=1`, F3), a launcher and S70 preset
runs (`linux/launcher.sh`, `linux/s70.sh`), FSR 4 and frame generation with AMD's
FidelityFX DLLs in `linux/keep/fidelityfx/`. The tree before it is on the
`linux-proton` branch.

### Keyboard and mouse

On by default (`VICTORY_KBM_PC=0`: the old controller-style keys). The mouse
looks around as soon as the game window has the focus; Esc, P or F2 let it go,
a click in the window takes it again (`VICTORY_MOUSE_SENS`,
`VICTORY_MOUSE_INVERT_Y=1`, `VICTORY_MOUSE_AUTO=0` for F2 / `~` only).

The pad cannot tell walking from driving, so the driving keys are a mode:
**F** (enter / leave a vehicle) turns them on and off. **E** sends the same
button (Y) to interact without turning them over, and **F5** turns them over by
hand if they are out of step.

| Key | On foot | Driving keys (after F) |
|---|---|---|
| W / S | move | throttle (RT) / brake, reverse (LT), and the stick |
| A / D | move | steer |
| Numpad 8 2 / 4 6 | — | pitch / roll (left stick: aircraft) |
| Space | jump, climb (X) | handbrake (RB) |
| Mouse | look | look |
| Left / right button | attack (RT) / aim (LT) | same |
| F / E | enter, leave vehicle (Y, turns the mode over) / interact (Y) | same |
| Shift | sprint (A) | |
| R | reload, melee (B) | |
| Q / Tab | cover (RB) / weapon wheel (LB) | |
| Ctrl / C, middle button | crouch (L3) / look behind (R3) | |
| Arrows / G / M / Z | D-pad / D-pad left / phone / D-pad down | |
| V / Esc, P / Enter, Backspace | camera / pause / confirm, back (menus) | |

**This repository contains no game files, code or assets.** You need your own
copy of the game. It is not affiliated with or endorsed by the game's developer
or publisher.

## Where it stands (2026-11-26)

Tested on one machine: Arch Linux, KDE Plasma (Wayland), Ryzen 9 9950X3D,
30 GB RAM, Radeon RX 9070 XT (RADV, Mesa 26.3), GE-Proton10-34, 4K output,
the Series X profile.

| | Ray tracing off | Ray tracing on |
|---|---|---|
| Prologue, first scene (prison) | ~27 fps | ~17-20 fps |
| Open world, first boat ride | | ~20-25 fps |
| First in-world cinematic | ~21 fps | ~13 fps |

At the start of the day ray tracing ran at 2-3 fps in the world and ~5 in the
cinematic, and streaming stalled the cinematic at 2-8 fps even without it.
With ray tracing off some ~340 draws a frame that need it are left out, so
ray tracing on is the complete picture.

What changed:

- **Streaming:** Wine's server kept a process's memory views in a list that
  every map walked under Wine's memory lock; the title has tens of thousands
  of streamed 64 KiB views. [A wineserver patch](linux/wineserver-views-rbtree.patch)
  (as upstream Wine `3217449b1f`, in no Proton release yet): a page map
  294 -> 13 us, the cinematic 2-8 -> 15-28 fps. The runtime also records its
  own placeholder reservations (`473d815`): Wine has no
  `NtQueryVirtualMemory` class 7, and the failing test cost 10,000 queries a
  frame under the same lock.
- **Ray tracing**, where the console's hardware node test is emulated in
  shaders:
  - `5e013ab`: a node is read in four 16-byte loads instead of 28 single
    dwords (vkd3d-proton only vectorizes a raw load whose offset is visibly
    `(x << 4) + 16k`).
  - `8eaee79`, `91184ce`: a node, and each access to title memory by address,
    runs once per copy the wave's lanes name, with that copy's view the
    wave's. Given per lane, RADV put every load in a loop of its own around a
    descriptor read, one after another.
  - `2baa1dd`, `f81abde`: walks through ray-tracing structures stop after
    1,024 steps (rays sent into broken structures walked 4,096 and held their
    waves) and end with what they found instead of leaving the program
    without writing their pixel.
- `6eb38e3`: programs that trace rays store a NaN as 0 (black blocks spread
  over reflections on windows, framed pictures and floors).
- Ray-traced light (2026-11-27): rooms were near black with ray tracing on
  and their light pulsed from frame to frame. The structure builder found
  200-700 pages of title memory a frame without a copy (read as zeros,
  stores dropped) once a cap of 2,048 pages was spent:
  - `c831644`: the builder compacts a structure by copying it; only 4 MiB
    were readied at the copy, so structures over 4 MiB (17 of 937, up to
    48 MiB) were cut off. Now readied whole from the size the title asked
    for; a dropped store gets its page. The interrogation room went from
    near black to lit.
  - `edf7e74`: pages that keep faulting (3 frames) are taken past the cap,
    up to 1 GiB (`VICTORY_FAULT_PERSIST`). 300 s with ray tracing at 1080p:
    22.3 fps, no device loss.
  - Fault diagnostics: what found a page (load or store, ray program or
    builder), `VICTORY_FAULT_LIST`, `VICTORY_WINDOW_LIST`.
- Merged avatarmanz's main again (the interrogation room now lit as by
  day) and kroeeee-eeee's Linux fork (renderer port 1 under Wine, loops
  dxil-spirv can take, far fewer `VirtualQuery`, `VICTORY_COPY_COMPARE`, a
  huge-page helper in `linux/thp/`). Measured here with ray tracing, 240 s:
  port 1 without huge pages 18.4 fps against 22.6, the other ports and
  `VICTORY_COPY_COMPARE=1` no change; all off by default. `VICTORY_PROFILE`
  (CPU stacks) works under Wine (upstream PR #343); `VICTORY_KBM_GTA=1`
  maps keyboard and mouse as GTA V on PC (PR #295).
- An old `d3dcompiler_47.dll` (6.3.9600, Windows 8.1) gives a black world:
  the run script refuses it, the log says so loudly (`3de0fac`, issue #1).
- Output 1080p rendered at 1080p by default in the run script
  (`VICTORY_OUTPUT`, `VICTORY_RENDER_SCALE`): 22.5 fps against 20.0 at 4K,
  heavy scenes 22.2 against 14.6.
- Merged avatarmanz's main again and, from [Belobug's AMD fork](https://github.com/Belobug/VIctory-Belobug-AMD)
  (an RX 9070 XT build like this one): streamed textures capped only when VRAM
  is over 92 % full, a pooled indirect buffer, descriptor slots given back
  sooner (scenes of 10,000+ draws), the texture upload fix for flicker, its HUD.
- `1390a72`: 16-bit data of integer views in descriptor tables keeps its bits.
- Diagnostics: `VICTORY_GPU_PROFILE=1` (GPU time by pipeline),
  `VICTORY_LOCK_PROBE=1`, `VICTORY_RT_SKIP=1`.

Known wrong: NPCs' mouths do not move when they talk; the splash video
judders; clear plastic and eyes sparkle (forward-lit "Eight" shaders, with
or without ray tracing); some rooms are still darker in some shots. Memory
is tight: a run reaches ~15 GB of VRAM and 11-13 GB of the PC's memory
shared with the card (copies of title memory outside buffers alone are
5-8 GB); with swap on it holds, without it the PC's memory runs out. The CPU side of the memory emulation (~30 ms preparing,
~24 ms copying back a frame) limits both modes now.

## How to run it

See [linux/README.md](linux/README.md): what to build and install
(this code built for Windows with `cargo xwin`, GE-Proton with the wineserver
patch, vkd3d-proton from master, Microsoft's `d3dcompiler_47.dll`), and
[linux/run-victory.sh](linux/run-victory.sh), which sets up the rest.
Ray tracing: `VICTORY_BVH_BUILDER=1 ./run-victory.sh`.

## License

GPL-3.0-or-later, as VIctory ([LICENSE](LICENSE)).
