# clankerVI

Performance-focused Linux/Proton fork of `victory-proton`, targeting native **30+ fps** without frame generation on AMD Ryzen 7 9700X + Radeon RX 7900 XTX (RADV, NixOS).

> [!WARNING]
> **Known limitations, stability & visual trade-offs:**
> - **Cinematics do not really work** yet.
> - **Limited gameplay scope** — tested mostly idle in the starting prison cell and walking out into corridor B35; later areas, driving, and combat are untested.
> - **Current preset (`c0d8e31` / `c7288eb`, Wave 33 peak FPS)** prioritizes maximum frame rate (`~35.5–36.6 fps` Series S, `~33.5–34.6 fps` Series X @ 1440p) and includes settings with known trade-offs:
>   - **Stretched NPC cloth / flickering geometry in corridor B35**: triggered by `VICTORY_LEARNED_ON_CARD=1` (mapping CPU `learned` constant pages in place so GPU fills run without waiting while the CPU overwrites those pages after sending). Running with `VICTORY_LEARNED_ON_CARD=0` eliminates this artifact cleanly (`diag-nolearn`), at the cost of `~15–21 ms/frame` waiting on GPU flights (`~24–25 fps` Series S on `fast`).
>   - **Intermittent Breakpad crash on world load** (`0xc0000005` `read 0x18` at `+0x4f43970`): observed on builds with `VICTORY_FAST_MEMCMP=1` and Waves 27–33 (`play-134751`).
>   - **`VICTORY_WINDOWS_BAR_MB=4096`**: can cause CPU overwrites of live BAR windows while GPU draws are in flight (`20c48ca`).
> - This repository contains **no game files, code, or assets**.

## Key Optimizations

- **Wine `ntdll.so` patch** ([linux/wine-virtual-writewatch.patch](linux/wine-virtual-writewatch.patch)):
  - Enables Linux kernel write-watch (`UFFDIO_REGISTER_MODE_WP` + `PAGEMAP_SCAN`) on `MapViewOfFile3` section views via `VirtualAlloc(..., MEM_COMMIT | MEM_WRITE_WATCH, ...)`. Combined with `VICTORY_SECTION_ALLOC=wc` and `VICTORY_LAND_BESIDE=1`, GPU copy-back on the 2.15 GB write-combined heap lands through an unwatched mapping with zero extra page faults, cutting `GetWriteWatch` dirty pages by **76%** (`6,323 → 1,518`) and `VICTORY_SEND_OWN` upload traffic by **45%** (`39 → 21 MB/frame`).
  - Releases Wine's global `virtual_mutex` before `ioctl(pagemap_fd, PAGEMAP_SCAN)` in `NtGetWriteWatch`, enabling true parallel kernel write-watch scans under `VICTORY_ASK_SPREAD=1`.
- **Submission lane, memory & worker pool (`victory-gpu`)**:
  - Zero-allocation scoped `EachPool` with chunked atomic dispatch, `THREAD_PRIORITY_ABOVE_NORMAL` matching `victory-submit`, and parallel `copy_all` / `take_snapshots` (`crates/victory-gpu/src/shadow.rs`).
  - Extended `Shadow::landed` (`VICTORY_LAND_TAKEN=1`) to kernel write-watched sections (`watched == 1`), eliminating ~15–18 MB/frame of redundant copy-back re-uploads, plus `VICTORY_TOUCHED_LIST=1`, `VICTORY_SUBMIT_SPLIT=1`, and `VICTORY_OFF_LINE=1` (Waves 18–26).
  - Lock-free `KEPT` fast-path for `ask_once`/`ask_ahead`, unified single-wave `ask_once` (Wave 31), `AHEAD_MAX` 64 MiB coalescing, per-shard `max_written`, and L2-resident `PAGE_MAX` direct-mapped table (`crates/victory-gpu/src/watch.rs`).
  - Per-thread `TLS_VIEWS` / `TLS_SHAPES` caches (`descriptor.rs`), cross-frame `ListViews` CBV cache with batched `CopyDescriptors` and per-list dynamic slot slab (`descriptor/host.rs`, `draw.rs`, Waves 27–33), lock-free `HostHeap::keep`, `VB/IB` bind cache, and `direct_ready` table cache (`draw.rs`).
  - AVX2 4×32B vectorized `texture::digest` (`texture.rs`), AVX-512 fault-safe `victory_safe_copy` & `victory_safe_merge` (`safecopy.rs`, Wave 29), write-combining `copy_wc` streaming stores (`host/send.rs`), and inline fast-paths in `fastmem.rs` (`VICTORY_FAST_MEMCPY=1`, `VICTORY_FAST_MEMCMP=1`).
- **Pipelines, HLE & build**:
  - `VICTORY_BVH_INPUT_SKIP` (Wave 16) and deterministic `VICTORY_SAMPLER_ORDER` (Wave 17) to avoid cross-run shader recompiles.
  - `VICTORY_SLEEP0_YIELD=1` in `crates/victory-hle/src/modules/kernel.rs` to throttle tight `Sleep(0)` spin loops (eliminated a 68% CPU spin thread).
  - `--profile fast` (`lto = "fat"`, `codegen-units = 1`) with `RUSTFLAGS="-C target-cpu=native"` (Zen 5 / AVX-512) and `--profile release` (`lto = "thin"`).
  - Runtime tuning in [linux/play-nixos.sh](linux/play-nixos.sh): `VICTORY_GUARD_REST=500000`, `VICTORY_GUARD_HOT=10000`, `VICTORY_JOIN_BARRIERS=1`, `VICTORY_THIN_THREADS=8`, `VICTORY_MOUSE_SENS=1.35`, `VICTORY_WAIT_RESIDENT=0`.

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
| Waves 27–33 + `THIN_THREADS=8` (`4e80b7d` / `c7288eb` / `c0d8e31`) | **Series X @ 2560×1440** | **~33.5–34.6 fps** | — | — | Peak FPS; load crash / cloth risk |
| Waves 27–33 + `THIN_THREADS=8` (`4e80b7d` / `c7288eb` / `c0d8e31`) | **Series S** | **~35.5–36.6 fps** | — | — | Peak FPS; load crash / cloth risk |
| Stable baseline (`aad91b7` + `LEARNED_ON_CARD=0` w/ Waves 27–31) | **Series S** (`fast`) | 23.9–25.3 fps | 39.6–42.7 ms | 17.0–20.0 ms | Clean visuals; ~21 ms GPU wait |

### Git Checkpoints

- `c0d8e31` / `c7288eb` (**current `HEAD`**): Wave 33 peak FPS (`36.6 fps` Series S, `34.6 fps` Series X @ 1440p).
- `decd3ef` (Wave 25) / `cbaa2c5` (Wave 26): pre-Waves 27–33 baseline (`36.0 fps` Series S, without `VICTORY_FAST_MEMCMP=1`).
- `aad91b7`: stable preset with `SUBMIT_SPLIT=0`, `OFF_LINE=0`, `TOUCHED_LIST=0`, no `WINDOWS_BAR_MB=4096`, no `FAST_MEMCMP=1`, and no Waves 27–33 (run with `VICTORY_LEARNED_ON_CARD=0` to avoid B35 NPC cloth stretching).

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
# Series S (~35.5-36.6 fps sustained on current HEAD)
./linux/play-nixos.sh

# Series X @ 2560x1440 (~33.5-34.6 fps sustained on current HEAD)
CONSOLE=series-x ./linux/play-nixos.sh

# Override to prevent B35 NPC cloth stretching (trades ~15-21 ms/frame GPU wait)
VICTORY_LEARNED_ON_CARD=0 ./linux/play-nixos.sh
```
