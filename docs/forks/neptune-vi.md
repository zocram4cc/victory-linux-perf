# Neptune VI (zm0kiemedia/Neptune_VI): notes for track U

Read on 26 Nov 2026 at `b900c64` (22 commits on a base of 25 Nov evening, "Neptune (VIctory Edit.)" by Traffic & imgone, on fxceless/myokex). Windows 11, Ryzen 7 9800X3D, RTX 5070 Ti, 32 GB. Clone: `~/src/Neptune_VI` (read only).

## Their numbers

- **Open world: about 6,500 draws, 90 ms a frame, the GPU busy 35 %** (README "Where a frame goes", an upstream measurement): roughly **30 ms of GPU a frame**. They report up to 8.4 fps at one save, 10 fps in places.
- Ours (26 Nov): 13,000-15,000 draws, ~480 ms a frame, **~265 ms of GPU** on an RX 7900 XT, not resolution-bound (`VICTORY_DISPLAY_MODE=1920x1080` changed nothing). So our GPU work is ~8× theirs per frame, about 20 µs per draw.

## What they do that matters for us

| What | Their switch / commit | Why it matters here |
|---|---|---|
| **Indirect draws run on the title's own arguments**, checked on the CPU (`VICTORY_INDIRECT_RAW=3`, `fb0c93c`). Argument records the GPU writes are sanitised **once, right after the writer** (`VICTORY_CLAMP_AFTER_WRITER`, `09222fa`, `ce39c72`), and clamps are batched per window (`VICTORY_CLAMP_BATCH`, `61f2206`) | 4.6 → 8.4 fps for them | We clamp **every** indirect draw with a dispatch between two **global UAV barriers** (`indirect.rs` `record_clamp_as`): ~1,000 full GPU drains a frame. G.1 measures it next |
| **GPU time profiling** (`VICTORY_GPU_TIME`, `6cc42d5`): the 25 costliest passes every 5 s | the same idea as our G.1 | compare outputs |
| `VICTORY_THIN` (upstream `ab7c9cf`): the title's draw calls translated by more threads | CPU side | not in our base |
| `VICTORY_WATCH_4K=1`: 4 KiB write tracking (upstream evening build) | finer pages = less of the page-granular merge problem (1.3) | ours tracks how? check |
| `VICTORY_LEVELS_ON_DEMAND`, `VICTORY_VRAM_IDLE_MS=15000,2500,1000` | textures keep only mips in use; unused textures stay 15 s ("less black-then-back") | piece T/A, VRAM |
| `VICTORY_PIPELINE_WAIT_MS=2500` | a frame waits at most 2.5 s for a compile | we use 500 ms |
| `-Warm`: translate every shader before playing (`victory.exe warm`, ~7 min) | the user's "compile shaders before entering" | piece P: we have the library now; a full warm pass is P.2 |
| `VICTORY_AMBIENT=0.6`: an ambient pass in place of ray-traced light | ray tracing does not run there either | S.9 (dark interiors, black silhouettes) may be the missing RT light |
| `VICTORY_RELAY=2`, `VICTORY_RELAY_OWN_QUEUE=1`, `VICTORY_QUIET_BACK=1`, `VICTORY_COPY_CHANGED=1`, `VICTORY_WIDE_*` | overlap and copy-back on Windows/NVIDIA | our lesson 1: the relay made Linux slower; measure each alone |
| `docs/unified-memory/` (3 documents: architecture, techniques, design) | a map of the same copy/merge model and of zero-copy options (`OpenExistingHeapFromAddress`, GPU upload heap, tiled resources) | input for phase 2; vkd3d-proton implements `OpenExistingHeapFromAddress` through `VK_EXT_external_memory_host` |

## Their Series S path (from `docs/NEPTUNE-README.txt`)

Their 9-11 fps is the **Series S** path (720p upscaled to 1080p) with the title's FidelityFX calls handed to AMD's PC DLL (`VICTORY_FSR_DLL`, module `fidelityfx.rs`, optionally through OptiScaler). The Series X 4K path was "not measured again" (README: "about 14 fps there" in play.ps1's comment). Here, `--console series-s` without that module gives a black picture (the console upscaler DLL is refused), GPU −10 %.

## To do here (track U)

1. **G.1 with the clamps counted apart** (running), then a switch that runs indirect draws raw where safe, after their model (`CLAMP_AFTER_WRITER`): one sanitising pass after the writer, no barrier pair per draw.
2. Check our write-watch granularity against `VICTORY_WATCH_4K`.
3. P.2: a full shader warm pass, as their `-Warm`.
