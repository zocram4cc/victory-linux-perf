# Memory stack

Cuts VRAM and private commit with no change to what is drawn. Every switch is off by
default in the code; `play.ps1` turns them on.

## Before and after

RTX 5080 16 GB, Ryzen 7 9800X3D, 32 GB RAM. First area, 3 minutes in the world.

| | Before | After |
|---|---:|---:|
| VRAM used | ~12.6 GB | **8.55 GB** |
| Private commit | ~26.0 GB | **~20.3–20.6 GB** (20.26–20.61) |
| Rust heap (`rust_heap`) | | **1,102 MB** (−254 MB from the last three switches) |
| Our D3D12 buffers (`d3d12_ours_unranged`) | | **982 MB** (−190 MB from the last three switches) |
| Working set | 15.1 GB | 12.85 GB (measured before the last three switches) |
| Frame rate | ~17 fps | 16.5 fps median, 17.8 p90 |

Textures keep their native quality. In these runs no pop-in, flicker or black surfaces were
seen, and the device was never lost.

## Switches (as `play.ps1` sets them)

| Switch | What it does |
|---|---|
| `VICTORY_GPU_DETILE=1` | Detiles textures on the card with a compute shader instead of on the CPU. |
| `VICTORY_GPU_DETILE_VERIFY=0` | Turns off the CPU cross-check of the detile (a debugging aid). |
| `VICTORY_GPU_DETILE_OUT_MB=512`, `VICTORY_GPU_DETILE_RAW_MB=512` | Sizes of the two detile rings used for loading bursts. |
| `VICTORY_GPU_DETILE_TRIM=1` | Shrinks the rings to 64 + 64 MB once loading is over, and grows them back for a burst. |
| `VICTORY_SLOT_EVICT=1` | Frees the card copies of idle textures when VRAM use passes a line. |
| `VICTORY_SLOT_EVICT_PERCENT=60`, `VICTORY_SLOT_EVICT_LOW_PERCENT=50` | Eviction starts above 60 % of the card's budget and stops at 50 %. |
| `VICTORY_BINDLESS_FEEDBACK=1` | Shaders record which heap textures they read, so a texture no shader has read for 60 s can be evicted too. |
| `VICTORY_ALLOCATOR_TRIM=1` | Replaces each command allocator every 30 s (an allocator keeps its largest recording), and frees spare list scratch. |
| `VICTORY_PRELUDE_POOL=1` | Host lists share one pool of preludes instead of keeping one each (~0.85 MB per list on NVIDIA). |
| `VICTORY_STAGE_POOL_MB=64` | Keeps at most 64 MB of spare texture staging buffers (an existing switch; the default is 256). |
| `VICTORY_CARD_WINDOW_UPLOAD_MB=256` | Caps the pool of the card windows' upload buffers at 256 MB (an existing switch; the default is 512). |
| `VICTORY_BUDGET_PACK=1` | A host list's 256-byte indirect-work budget is a slot of a shared 64 KiB buffer, not a 64 KiB committed buffer of its own (602 of them held 37 MB). |
| `VICTORY_COMPARE_PIECES=1` | Refreshes compare the title's range 1 MiB at a time through one 1 MiB buffer per thread, instead of a buffer as large as the largest range (~270 MB across 12 threads). Same bytes copied, same runs sent. |

## Cherry-pick order (onto `e94251a`)

```
d8a3df8 49dd4c7 0ab9ab8 f879d92 df8839d 6497c6e 406ff5f 3599b52 7c274de 9c0c61b
876456c 79dc3be cd5ae44 390d949 2f94a58 be0f84c 1626109 66cee8f 98a4e60 4b8ff9e 0c78932
8172480 75d1f4f efe723a
```

(Hashes are those on `gpu-detile`.)

| Group | Commits | Hook points |
|---|---|---|
| GPU detile + ring trim | `d8a3df8` `0ab9ab8` `390d949` `2f94a58` | `formats/texture.rs` (`copy_table`); new `gpu/detile.rs`; `texture.rs` (upload route); `host.rs` (`Step::GpuTexture`, a pre-list at `lists[0]` in `submit`, `detile::sent`) |
| Memory telemetry | `f879d92` `df8839d` | new `gpu/memtel.rs`; `ramline.rs` (memory CSV); `core/heapcount.rs`. Later commits add counters to `ramline.rs`: take these first. |
| Slot eviction | `6497c6e` `406ff5f` | `resource.rs`, `descriptor.rs`, `descriptor/host.rs`. `406ff5f` fixes a deadlock: never take one without the other. |
| Bindless feedback | `7c274de` `9c0c61b` `876456c` `79dc3be` | new `gpu/feedback.rs`; the shader emitter (`emit/shader.rs`); `descriptor/host.rs` (parking); `draw.rs::heap_textures`; `pipeline/library.rs` (a separate library file); a pre-list in `host.rs` |
| Allocator, scratch and prelude trims | `cd5ae44` `1626109` `4b8ff9e` | `host.rs`: allocator swap at list and prelude resets, `Spare::trim`, the prelude pool tagged at both send sites; `list.rs` |
| Budget pack | `8172480` | `host.rs`: budget slots taken at list creation, given back at retire |
| Compare in pieces | `75d1f4f` | `shadow.rs` (`Shadow::copy_now`), with a unit test against the whole compare |
| Launcher | `be0f84c` `66cee8f` `98a4e60` `0c78932` `efe723a` | `play.ps1` only |
| Optional | `49dd4c7` `3599b52` | test target compile fix; hang watch (`VICTORY_HANG_WATCH_S`, a diagnostic) |

The patches also apply with `git am` (series `0001` to `0027`; `0022` and `0026` only touch this file, `0027` adds `AGENTS.md`). For AI coding agents, or a fork that has moved away from `e94251a`: [AGENTS.md](AGENTS.md).

## Left out on purpose

- `5bc8998`, `3469541`: the copy profile, a diagnostic.
- `bd8a047`: the heap profile (`VICTORY_HEAP_PROFILE`), a diagnostic.
- `9e6e2db`, `857a1ac`: `VICTORY_VIEW_NO_ASK`, worth about 1 ms a frame.
- `0e7e3e3`, `a44a9c3`: a change and its revert.
- `VICTORY_GUARD_HOT` and `VICTORY_INPLACE` (both existing switches) made things worse in testing: keep them off.
