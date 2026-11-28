# clankerVI

Performance-focused Linux/Proton fork of `victory-proton`, targeting native **30+ fps** without frame generation on AMD Ryzen 7 9700X + Radeon RX 7900 XTX (RADV, NixOS).

> [!WARNING]
> **Known limitations & testing scope:**
> - **Cinematics do not really work** yet.
> - **Only tested in the starting zone** — all benchmarks were measured idle in the starting area; **no gameplay inputs** (walking, driving, camera movement, combat, etc.) were sent or tested outside the starting zone.
> - This repository contains **no game files, code, or assets**.

## Key Optimizations

- **Wine `ntdll.so` patch** ([linux/wine-virtual-writewatch.patch](linux/wine-virtual-writewatch.patch)):
  - Enables Linux kernel write-watch (`UFFDIO_REGISTER_MODE_WP` + `PAGEMAP_SCAN`) on `MapViewOfFile3` section views via `VirtualAlloc(..., MEM_COMMIT | MEM_WRITE_WATCH, ...)`. Combined with `VICTORY_SECTION_ALLOC=wc` and `VICTORY_LAND_BESIDE=1`, GPU copy-back on the 2.15 GB write-combined heap lands through an unwatched mapping with zero extra page faults, cutting `GetWriteWatch` dirty pages by **76%** (`6,323 → 1,518`) and `VICTORY_SEND_OWN` upload traffic by **45%** (`39 → 21 MB/frame`).
  - Releases Wine's global `virtual_mutex` before `ioctl(pagemap_fd, PAGEMAP_SCAN)` in `NtGetWriteWatch`, enabling true parallel kernel write-watch scans under `VICTORY_ASK_SPREAD=1`.
- **Submission lane & worker pool (`victory-gpu`)**:
  - Zero-allocation scoped `EachPool` with chunked atomic dispatch, `THREAD_PRIORITY_ABOVE_NORMAL` matching `victory-submit`, and parallel `copy_all` / `take_snapshots` (`crates/victory-gpu/src/shadow.rs`).
  - Extended `Shadow::landed` (`VICTORY_LAND_TAKEN=1`) to kernel write-watched sections (`watched == 1`), eliminating ~15–18 MB/frame of redundant copy-back re-uploads.
  - Lock-free `KEPT` fast-path for `ask_once`/`ask_ahead`, per-shard `max_written`, and an L2-resident `PAGE_MAX` direct-mapped table (`crates/victory-gpu/src/watch.rs`).
  - Per-thread `TLS_VIEWS` / `TLS_SHAPES` caches (`descriptor.rs`), lock-free `HostHeap::keep` fast-path (`descriptor/host.rs`), and thread-local `ByCopy` HashMap reuse (`host.rs`).
  - AVX2 4×32B vectorized `texture::digest` (`texture.rs`), fault-safe 128B AVX2 `victory_safe_copy` & `victory_safe_merge` (`safecopy.rs`), and inline small-copy fast-path in `fastmem.rs` (`VICTORY_FAST_MEMCPY=1`).
- **HLE & build**:
  - `VICTORY_SLEEP0_YIELD=1` in `crates/victory-hle/src/modules/kernel.rs` to throttle tight `Sleep(0)` spin loops (eliminated a 68% CPU spin thread).
  - Thin LTO (`lto = "thin"`) and `x86-64-v3` (AVX2/BMI2/FMA) codegen.
  - Runtime tuning: `VICTORY_GUARD_HOT=500`, `VICTORY_JOIN_BARRIERS=1`, `VICTORY_THIN_THREADS=6`, `VICTORY_WAIT_RESIDENT=0` (disables a Windows WDDM residency fence wait that caused multi-second stalls and texture upload backlogs on Linux/RADV during cutscenes).

## Measured Progress (Starting Zone Idle, Native, Zero Frame Gen, `VICTORY_UPSCALER=6`)

Test hardware: AMD Ryzen 7 9700X (8C/16T), AMD Radeon RX 7900 XTX (RADV 26.3.0), 32 GB RAM, NixOS Linux 7.3.4, patched Proton Experimental Wine.

| Milestone | Console / Resolution | Sustained FPS | Frame Time | `preparing` |
|---|---|---:|---:|---:|
| Baseline (`f353c97`) | Series X @ 2560×1440 | 24.7–25.3 fps | 39.6–40.4 ms | 32–34 ms |
| Waves 1–5 (TLS views, `HostHeap::keep`, `EachPool`, `Sleep(0)`, `shard_max`, AVX2 digest) | Series X @ 2560×1440 | 26.0–27.7 fps | 36.0–38.4 ms | 28.5–31.8 ms |
| Wave 8c/8d (Wine `UFFD_WP` section write-watch + `LAND_TAKEN` + parallel `PAGEMAP_SCAN` + `JOIN_BARRIERS=1`) | Series X @ 2560×1440 | 28.5–28.8 fps | 34.8–35.1 ms | 23.6–26.3 ms |
| Wave 11 (`PAGE_MAX`, AVX2 `safecopy`, worker priority, `THIN_THREADS=6`, `GUARD_HOT=500`) | Series X @ 2560×1440 | **29.5–30.8 fps** | **32.5–33.9 ms** | **26.7–27.6 ms** |
| Wave 11 (same + full `GATE=1 GATE_REACH=1` correctness) | **Series S** | **30.3–31.7 fps** | **31.6–33.0 ms** | **23.2–25.4 ms** |

## Build

```bash
CARGO_TARGET_DIR="$HOME/.cache/vp-upstream-win" \
RUSTFLAGS="-C target-cpu=x86-64-v3" \
cargo xwin build --release -p victory --target x86_64-pc-windows-msvc
```

To get the `VICTORY_SECTION_ALLOC=wc` and `VICTORY_ASK_SPREAD=1` speedups, apply [linux/wine-virtual-writewatch.patch](linux/wine-virtual-writewatch.patch) to Proton Experimental's Wine `dlls/ntdll/unix/virtual.c` and rebuild `ntdll.so`.

## Run

```bash
# Series S (30.3–31.7 fps in starting zone, full GATE_REACH=1 correctness)
./linux/play-nixos.sh

# Series X @ 2560x1440 (29.5–30.8 fps in starting zone)
CONSOLE=series-x ./linux/play-nixos.sh
```
