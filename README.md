# clankerVI

Performance-focused Linux/Proton fork of `victory-proton`, targeting native **30+ fps** without frame generation on AMD Ryzen 7 9700X + Radeon RX 7900 XTX (RADV, NixOS).

> [!WARNING]
> **Known limitations & testing scope:**
> - **Cinematics do not really work** yet.
> - **Limited gameplay scope** — tested idle in the starting prison cell and walking/looking into corridor B35; later areas, driving, and combat are untested.
> - **Root cause of NPC cloth stretching / frame flickering (`VICTORY_LEARNED_ON_CARD=1` + `VICTORY_WINDOWS_BAR_MB=4096`)**:
>   - `VICTORY_LEARNED_ON_CARD=1` executed card steps by reading live CPU memory in place (`in_place_block`) when the GPU ran `own.list` — *after* the game CPU thread had already overwritten those shared 4 KiB `learned` pages for subsequent draws/frames (`~0.96 fills/frame found written again`), corrupting NPC skinning/cloth buffers into full-screen triangle spikes.
>   - `VICTORY_WINDOWS_BAR_MB=4096` allowed CPU overwrites of live BAR vertex/cloth buffers while GPU draws were in flight.
>   - **Fix (`VICTORY_LEARNED_ON_CARD=2`)**: snapshots `learned` fills synchronously at submission time into a pooled per-flight upload buffer (`Own::snap` in `crates/victory-gpu/src/host/card_steps.rs`) before copying on the card, keeping `0.00` learned-page GPU waits without live-memory overwrite races (`0.00 found written again`), alongside rolling back unstable Waves 28–33 (`VICTORY_FAST_MEMCMP=1` / Breakpad load crashes) to the stable Wave 27 (`9b5e3c3`) baseline.
> - This repository contains **no game files, code, or assets**.

## Key Optimizations

- **Wine `ntdll.so` patch** ([linux/wine-virtual-writewatch.patch](linux/wine-virtual-writewatch.patch)):
  - Enables Linux kernel write-watch (`UFFDIO_REGISTER_MODE_WP` + `PAGEMAP_SCAN`) on `MapViewOfFile3` section views via `VirtualAlloc(..., MEM_COMMIT | MEM_WRITE_WATCH, ...)`. Combined with `VICTORY_SECTION_ALLOC=wc` and `VICTORY_LAND_BESIDE=1`, GPU copy-back on the 2.15 GB write-combined heap lands through an unwatched mapping with zero extra page faults, cutting `GetWriteWatch` dirty pages by **76%** (`6,323 → 1,518`) and `VICTORY_SEND_OWN` upload traffic by **45%** (`39 → 21 MB/frame`).
  - Releases Wine's global `virtual_mutex` before `ioctl(pagemap_fd, PAGEMAP_SCAN)` in `NtGetWriteWatch`, enabling true parallel kernel write-watch scans under `VICTORY_ASK_SPREAD=1`.
- **Submission lane, memory & worker pool (`victory-gpu`)**:
  - Zero-allocation scoped `EachPool` with chunked atomic dispatch, `THREAD_PRIORITY_ABOVE_NORMAL` matching `victory-submit`, and parallel `copy_all` / `take_snapshots` (`crates/victory-gpu/src/shadow.rs`).
  - Extended `Shadow::landed` (`VICTORY_LAND_TAKEN=1`) to kernel write-watched sections (`watched == 1`), eliminating ~15–18 MB/frame of redundant copy-back re-uploads, plus `VICTORY_TOUCHED_LIST=1`, `VICTORY_SUBMIT_SPLIT=1`, and `VICTORY_OFF_LINE=1` (Waves 18–26).
  - Submission-time upload snapshots for `learned` card steps (`VICTORY_LEARNED_ON_CARD=2` in `crates/victory-gpu/src/host/card_steps.rs`) preventing CPU/GPU overwrite races with `0.0 ms` learned-page stalls.
  - Lock-free `KEPT` fast-path for `ask_once`/`ask_ahead`, `AHEAD_MAX` 64 MiB coalescing, per-shard `max_written`, and L2-resident `PAGE_MAX` direct-mapped table (`crates/victory-gpu/src/watch.rs`).
  - Per-thread `TLS_VIEWS` / `TLS_SHAPES` caches (`descriptor.rs`), per-list `ListViews` CBV cache and zero-COM `constant_raw_at` (`descriptor/host.rs`, Wave 27), lock-free `HostHeap::keep`, and `VB/IB` bind cache (`draw.rs`, Wave 27).
  - AVX2 4×32B vectorized `texture::digest` (`texture.rs`), fault-safe 128B AVX2 `victory_safe_copy` & `victory_safe_merge` (`safecopy.rs`), and inline small-copy fast-path in `fastmem.rs` (`VICTORY_FAST_MEMCPY=1`).
- **Pipelines, HLE & build**:
  - `VICTORY_BVH_INPUT_SKIP` (Wave 16) and deterministic `VICTORY_SAMPLER_ORDER` (Wave 17) to avoid cross-run shader recompiles.
  - `VICTORY_SLEEP0_YIELD=1` in `crates/victory-hle/src/modules/kernel.rs` to throttle tight `Sleep(0)` spin loops (eliminated a 68% CPU spin thread).
  - `--profile fast` (`lto = "fat"`, `codegen-units = 1`) with `RUSTFLAGS="-C target-cpu=native"` (Zen 5 / AVX-512) and `--profile release` (`lto = "thin"`).
  - Runtime tuning in [linux/play-nixos.sh](linux/play-nixos.sh): `VICTORY_LEARNED_ON_CARD=2`, `VICTORY_GUARD_REST=500000`, `VICTORY_GUARD_HOT=10000`, `VICTORY_JOIN_BARRIERS=1`, `VICTORY_THIN_THREADS=8`, `VICTORY_MOUSE_SENS=1.35`, `VICTORY_WAIT_RESIDENT=0`.

## Measured Progress (Starting Area, Native, Zero Frame Gen, `VICTORY_UPSCALER=6`)

Test hardware: AMD Ryzen 7 9700X (8C/16T), AMD Radeon RX 7900 XTX (RADV 26.3.0), 32 GB RAM, NixOS Linux, patched Proton Wine (`wineserver` view tree + `ntdll` `UFFD_WP` write-watch).

| Milestone | Console / Resolution | Sustained FPS | Frame Time | `preparing` | Notes |
|---|---|---:|---:|---:|---|
| Baseline (`f353c97`) | Series X @ 2560×1440 | 24.7–25.3 fps | 39.6–40.4 ms | 32–34 ms | — |
| Waves 1–5 (`TLS views`, `EachPool`, `Sleep(0)`, AVX2 digest) | Series X @ 2560×1440 | 26.0–27.7 fps | 36.0–38.4 ms | 28.5–31.8 ms | — |
| Wave 8c/8d (`UFFD_WP` section write-watch + `LAND_TAKEN` + `JOIN_BARRIERS=1`) | Series X @ 2560×1440 | 28.5–28.8 fps | 34.8–35.1 ms | 23.6–26.3 ms | — |
| Wave 11 (`PAGE_MAX`, `THIN_THREADS=6`, `GUARD_HOT=500`) | Series X @ 2560×1440 | 29.5–30.8 fps | 32.5–33.9 ms | 26.7–27.6 ms | `GATE_REACH=1` |
| Waves 16–25 (`decd3ef`: `BVH_INPUT_SKIP`, `fast` LTO, `GUARD_HOT=10000`) | **Series S** | **36.0 fps** | — | — | `LEARNED_ON_CARD=1` |
| Wave 27 (`9b5e3c3`: `ListViews` CBVs, VB/IB bind cache) | **Series S** | **35.2 fps** (`release`) | — | — | `LEARNED_ON_CARD=1` |
| Waves 27–33 + `THIN_THREADS=8` (`c7288eb` / `c0d8e31`) | **Series S** | **~33.7–36.6 fps** | 27.9–33.5 ms | 18.7–25.0 ms | `LEARNED_ON_CARD=1` cloth/load-crash risk |
| **Current (`Wave 27` + `LEARNED_ON_CARD=2` snapshots, no `WINDOWS_BAR`/`FAST_MEMCMP`)** | **Series S** (`fast`) | **32.7–33.4 fps** | **29.9–30.6 ms** | **22.7–24.9 ms** | `0.0–1.7 ms` GPU wait, `0.00` written again, 105s clean |

### Git Checkpoints

- **Current `HEAD`**: Wave 27 (`9b5e3c3`) + `VICTORY_LEARNED_ON_CARD=2` submission-time snapshots (`32.7–33.4 fps` Series S `fast`, no `WINDOWS_BAR_MB=4096` / `FAST_MEMCMP=1`).
- `c0d8e31` / `c7288eb`: Wave 33 peak FPS (`~35.5–36.6 fps` Series S, `~33.5–34.6 fps` Series X @ 1440p; `LEARNED_ON_CARD=1`).
- `decd3ef` (Wave 25) / `cbaa2c5` (Wave 26): pre-Waves 27–33 baseline (`36.0 fps` Series S, without `VICTORY_FAST_MEMCMP=1`).
- `aad91b7`: conservative fallback with `SUBMIT_SPLIT=0`, `OFF_LINE=0`, `TOUCHED_LIST=0`.

## Build

```bash
# Fast profile (fat LTO + native CPU tuning, auto-selected by play-nixos.sh)
CARGO_TARGET_DIR="$HOME/.cache/vp-upstream-win" \
RUSTFLAGS="-C target-cpu=native" \
cargo xwin build --profile fast -p victory --target x86_64-pc-windows-msvc

# Release profile (thin LTO, x86-64-v3)
CARGO_TARGET_DIR="$HOME/.cache/vp-upstream-win" \
RUSTFLAGS="-C target-cpu=x86-64-v3" \
cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
```

To get the `VICTORY_SECTION_ALLOC=wc` and `VICTORY_ASK_SPREAD=1` speedups, apply [linux/wine-virtual-writewatch.patch](linux/wine-virtual-writewatch.patch) to Proton Experimental's Wine `dlls/ntdll/unix/virtual.c` and rebuild `ntdll.so`.

## Run

```bash
# Series S (~32.7-33.4 fps sustained, submission-time snapshot card steps)
./linux/play-nixos.sh

# Series X @ 2560x1440
CONSOLE=series-x ./linux/play-nixos.sh

# Fallback without learned card snapshots (trades ~15-21 ms/frame GPU wait)
VICTORY_LEARNED_ON_CARD=0 ./linux/play-nixos.sh
```
