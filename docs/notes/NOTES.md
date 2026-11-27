# Xbox Series title on Linux — working notes

The running log of this work, oldest first: every measurement, also the ones
that led nowhere. Sections about the machine's own files and backups are left
out, and every machine-specific path is a token (`@PORTING@`, `@PROFILE@`,
`@SCRATCH@`, ...; see `README.md` in this folder).

Updated 2026-11-25. All work happens on **our own copy**; the original dump is
never modified. No game assets, no large code excerpts — offsets, signatures,
counters and our own conclusions only (the same policy VIctory and XCL keep).

---

## 5. Crash analysis (our own disassembly, `objdump`)

Addresses are RVAs in the title image (base `0x140000000` when analysed).

- `+0x13308e0` — the streaming decoder's loop. Takes instance `[ctx+0x10020]`,
  sets `[inst+0x18]=dst`, `[inst+0x20]=len`, then loops:
  `if [inst+0x30] != 0 → done`, `if [inst+8] == 0 → feed (call +0x1330b60)`,
  else **call +0x1330a50** (dispatcher), `test al,al; je +0x1330a44`.
- `+0x1330a44` — `int3` (0x80000003). That is where we land.
- `+0x1330a50` — the dispatcher. Return in `AL`:
  1. `[inst+0x50] == 0` → `xor eax,eax` (no codec provider)
  2. `[inst+0x58] == 0xFFFF` → `xor eax,eax` (invalid codec id)
  3. otherwise: provider call through vtbl `+0x3f0` with hash `0xe0c12d7a5d4884c5`,
     then `+0x3f8` → `+0x1330cd0`, or `+0x2f8` → `+0x1331380`
- `+0x1330b60` — feeds the decoder (called when input runs out).

Register snapshot at the crash: `rax=0 rcx=0xfffffffd rsi=0x6ffffa816a50`
(ctx in title memory) `rbp=0x10000` (64 KiB block boundary).

**Conclusion:** the provider *exists* (otherwise we would have fallen through at
`+0x50`), so the content/chunk is being rejected — consistent with incomplete
data (#686). VIctory #1063 hits the same offset but in the world phase (after
150–288 frames).

## 6. Imports and DLL inventory

Exe: 587 imports from 68 DLLs, 10 sections, 2 TLS callbacks, `.text` ≈ 100 MB.

**Console DLLs the exe wants that are NOT in the dump** (must be HLE'd):
`d3d12_xs.dll`, `xmem.dll`, `AcpHal.dll`, `xapu.dll`, `chad.dll`, `xg_xs.dll`,
`XCRDAPI.dll` (and `GameInput.dll`).

**Present in the dump (16):** `Bink2Scarlett`, `XboxBvhBuilder`, `XCurl`,
`libHttpClient.GDK`, `amd_fidelityfx_loader_xbox`, `amd_fidelityfx_upscaler_xbox`,
`vcruntime140(_1)`, `msvcp140(_1/_2/_atomic_wait/_codecvt_ids)`, `concrt140`,
`vccorlib140`, `vcomp140`.

Binding at boot: 88 hle / 427 host / 72 title / **0 missing** for the exe.

## 7. PR review (Linux/Wine)

| PR | Assessment | Decision |
|---|---|---|
| #523 build/test on Linux | enables the native build and tests | ✅ in `sl-prep` |
| #994 (15 commits) | only 2 matter to us now | ✅ `e5a8c2f` null blob, `45aceea` one return block |
| #1279 clamp | superseded by #190 | ⏭ |
| #526 native runtime | a different route | ⏸ |
| Scarlight #14 speed stack | the world's fps | ⏸ |
| Scarlight #15 = #994+speed | later | ⏸ |
| #189/#233/#961 wave64→32 | NVIDIA only | ⏭ |
| #1009 xmem <4 GiB | may bite at boot | 👀 |

`sl-prep` verified: Windows build ✅, native build ✅, **272 tests, 0 failed**.
Local follow-up fix: `target()` in the #523 Linux stub (1 line) — a candidate to
report upstream.

## 8. Plan

**Phase 0 — data (blocks everything):** download complete → `check-data.py` →
reflink copy → run.

**Phase 1 — boot/menus:** `VICTORY_DIAG=1 ./run-victory.sh 240`.
Gate: first window + 30 fps in menus. If we still land on `+0x1330a44` with
*complete* data, then it is code, and §5 plus Ghidra is the next step.

**Phase 2 — the menu chain:** account picker → logo → start screen. Known
non-fatal warnings: `rpcrt4.dll` outside the host policy (#510 closed without a
merge), `view the host does not allow`, `method not implemented`.

**Phase 3 — loading/world:** ~50 s of loading; expect a black screen and
0.5–2 fps. That is where Scarlight #14 (write-watch/copy) and #994's world fixes
are worth testing.

**Phase 4 — measure and compare:** `scarlight` vs `sl-prep`, log diffs, and only
then more patches.

## 10. New opportunity: `fxceless/victory-fork`

Found 2026-11-24 (568★, GPL-3.0, created the same day). **Furthest ahead of
every variant:**

| Stage | State in the fork |
|---|---|
| Boot, menu, start screen | 30 fps (as upstream) |
| Opening scene | plays in full, lit |
| Autosave + load | works |
| First gameplay scene (prologue) | renders: characters, textures, sky, HUD, pause menu, pad |
| Shadows / RT light | missing — everything in shadow is black |
| World fps | 2–4 fps (240–450 ms/frame), ~24 GB RAM |

**Fresh measurements (2026-11-24, the fork's own issues):**

| Measurement | Value |
|---|---|
| Lit opening scene | **8.5–8.7 fps** (#107, RTX 5080, `4e17478`) |
| Gameplay | **~3.9 fps** (#31) |
| GPU time | 63 ms/frame, **40.5 ms of it in 3 227 indirect calls** (~12 µs each) |

The switches behind those numbers (packaged as `VICTORY_FAST=1` in
`run-victory.sh`): `VICTORY_DISPLAY_MODE=1920x1080`, `VICTORY_RELAY=2`,
`VICTORY_LATE_WAIT=3`, `VICTORY_WIDE_BACK=1`, `VICTORY_CARD_BUFFERS_MIN_MB=1`,
`VICTORY_CARD_BUFFERS_MB=1024`.

Fork PRs of interest: #28 (Linux/Proton: DXC fallback for the clamp, watchdog
hooks, 32 MiB pipeline), #37 (Linux build support), #32 (R16G16B16A16 standing in
for R9G9B9E5).

**AMD/Linux state:** upstream #1711 = RX 7800 XT, RADV 26.3.1, vkd3d 3.2 →
**black world** even with the integrated shader fixes. So our RADV outcome is
uncertain; the fork's numbers are NVIDIA/Windows. Tested on **Windows 11 +
RTX 4070**, not Linux/AMD.

**Scope:** base `ffe310e` (upstream 2026-11-24 10:53) + **12 commits, 92 files,
+37 502/−1 484**: meshlet culling, descriptor ceilings, light read-back per
mip/layer, fog/shadow/hair/skinning shaders, mip levels in gathers, 730 compute
programs with invalid DXIL fixed, plus performance switches and shader
optimisation (−43 % instructions).

**Gaps in it:** it does *not* have our two Wine fixes — it still sends an empty
non-null blob to `CreatePipelineLibrary` (vkd3d-proton refuses) and lacks "one
return block per program" (RADV crash). Both are added locally when we test it.

**Risks for us:** developed on NVIDIA/Windows; `VICTORY_INPLACE` (heap over title
memory) and card-memory copies are less proven on RADV; 24 GB RAM against our
32 GB (23 free) is tight.

**Plan:** A/B/C once the data is complete — A = `vfork` + our fixes (best
rendering), B = `scarlight`, C = `sl-prep`. Licence-wise all can be combined
(GPL-3.0), but merging two large diverging diffs waits until we know which one
actually runs.

### Our patch series on top of vfork (2026-11-24)

| Patch | Why | Verified |
|---|---|---|
| `e17a4bc` one return block per program (#994) | RADV crash at pipeline creation | builds |
| `9fc87ce` empty pipeline library with a null blob (#994) | vkd3d-proton refuses an empty non-null blob | builds |
| ours: clamp without byte-address methods | Wine's `d3dcompiler_47` lacks `ByteAddressBuffer.Load/Store/Interlocked*` and aborts on `[allow_uav_condition]` → **every indirect call was dropped** | `argument_clamp_builds` + `host_list_reuses…` OK; `victory_gpu` **109/110** |

Still failing in the GPU suite: `a_depth_planes_twin_is_drawn_into_the_plane` →
`E_FAIL` from the host (a vkd3d gap in the fork's depth-plane twin; not blocking).

## 12. Strategy shift (2026-11-25 00:30)

fxceless works **locally** (too many AMD PRs while he runs NVIDIA) and intends to
publish a new repo once he has results. Consequences:

- His `main` (`2119b0e4`) will stand still → **do not wait for it**.
- Our mirrors/bundles are the insurance; when the new repo appears we mirror again.
- **The AMD contributions matter most to us.** AMD PRs reviewed:
  - `#134`/`#98` view-heap clamp on all GPUs (device removal on RDNA3) → **have**
    (through triippywhale)
  - `#135` i64 comparisons, `#317`/`#195` AMD compiler crashes → **Windows driver
    specific**, probably irrelevant on RADV
- `ref/amd-skip-pipelines.txt`: **43 pipelines** AMD runs had to skip (#3,
  RX 9070 XT). `VICTORY_AMD_SKIPS=1` in `run-victory.sh` sets them as
  `VICTORY_SKIP_PIPELINES` — the emergency lever if we get "device removed".

## 13. More patches + diag build (2026-11-25 00:55)

fxceless merges (locally, through triippywhale's `test/amd-134-203-156`):
#134 AMD view-heap clamp, #203 shader gaps, #156 RT on 32 lanes. We have all
three (the first two in `main`, #156 on the `lit` branch).

New cherry-pick in `main`: `5cc1c71` **VICTORY_CAPTURE_EVERY_SECS** (saves a
frame every N seconds — a timeline independent of fps). Behind the `diagnostics`
feature, so diag builds only.

Builds for vfork: `release` (fastest) and `diag` (same code generation plus
diagnostics: `VICTORY_CAPTURE*`, `VICTORY_PEEK`, `VICTORY_GPU_TIMING`). The diag
binary has the vkd3d DLLs next to it.

Backup plan adjusted: copy the **contents** of the nested folder (no nested
structure at the destination) plus `game_scarlett_master.exe`; delete the
duplicates locally after verification.

## 14. First real run — MILESTONE (2026-11-25 01:00)

Run `run-010231.log`, vfork + 7 patches, `VICTORY_FAST`, diag build, capture
every 5 s. **No device removal, no crashes.**

| Phase | Result |
|---|---|
| First boot (screen size, brightness, subtitle size) | ✅ 30 fps, UI correct |
| Onboarding "NAVIGATION EXPERIENCE" | ✅ (frame-1024) |
| Loading (spinner) | ✅ (frame-2753) — a few minutes, black background |
| **Opening cutscene** | ✅ **renders with subtitles and audio** (frame-3775: "It's a new car.") |

Cutscene performance: 0.1–0.4 fps; per frame copies 1.6–2.6 s (140–220 MB) +
textures 2–5 s (20–190 MB), GPU 1.3 s, 26 000–42 000 list calls. Dropped draws:
1 352/frame = the ray-tracing builder (off).

Known artifacts: purple tint, iridescent hair, black in shadow — matching
chapter-ii's report (exposure swings, 12-byte views, RT off).

Next: a longer run (900 s) to get past the cutscene; then the Series S profile
and/or Scarlight's write-watch batching (#393/#124).

## 15. Performance chase in the cutscene (2026-11-25 01:30)

| Step | ms/frame | fps | Action |
|---|---:|---:|---|
| Baseline | 5 148 | 0.27 | card compare fell back to host copies |
| + structured buffers in `CARD_COMPARE` | 5 148 | 0.27 | CPU compare 515 → 31 MB (fix proven, not the bottleneck) |
| + **`VICTORY_TEXTURE_NO_WAIT=1`** | **1 068–1 958** | **0.5–0.9** | textures 1 773 → 108–224 ms |
| `VICTORY_COPY_MODE=plain` | – | – | **unsafe: crashes** (reads uncommitted memory / bring-up failure). Do not use. |

Left in the late phase: copies 1 096 ms/67 MB (61 MB/s, page by page), copy-back
293 ms, GPU 93 ms. Next candidates: `VICTORY_INPLACE=1`,
`VICTORY_CARD_WINDOWS_MB`, and Scarlight's write-watch batching (#393/#124).

**Important:** the crash at `+0x2215c35` is the title's **bring-up reporter**
(XCL #746: references `E_OUTOFMEMORY`, reached when an init check fails; it calls
an unset callback → writes to 0). Therefore **never run two instances at once** —
running out of memory triggers it.

## 16. The 30 fps cap (2026-11-25 01:55)

**The cap is the title's own, not victory's.** Evidence:

- `victory-data/data/7952987A/storage/settings/settings.xml`: `desiredFPS=30`,
  `frameRateLimit=30`, `vSync=2` — written by the title, never by victory.
- The title calls the console's `SetFrameIntervalX(33333 µs)` → `frame.rs:45-69`
  logs `frame interval interval=33.333ms`. Victory has **no** limiter; the only
  constant is `SIXTIETH = 16 667 µs` (`frame.rs:26`).
- Present is vsync-free (`Present(0, …)`, 2 buffers, no vblank).
- The "94 ms GPU time" we saw was queue-empty waiting while the title slept; the
  `on the GPU` field is 0.0 ms and the GPU is ~2 % busy in menus.

**Attempt 1:** `desiredFPS=60`, `frameRateLimit=0`, `vSync=0` in settings.xml.
→ the title still asks for 33.333 ms. Those fields do not drive the interval.

**Attempt 2 (in progress):** `VICTORY_DISPLAY_HZ` (our switch,
`dxgi.rs::display_hz`). `modes()` only advertises 60 Hz, and the code's own
comment says the title "looks for a 120 Hz one to allow 120 Hz frame intervals" —
so we announce 120 Hz.

**Assessment:** 60 fps in menus/loading is realistic (GPU ~2 %, prepare 0.1 ms);
in gameplay it is nowhere near (0.2–5 fps). Hacking `frame.rs` to force 16 667 µs
is not advised — the title uses the interval as its time base.

## 17. VItoriaBr merge (2026-11-25 02:20)

`uiliannn/VItoriaBr` (GPL-3.0, Brazil, 19★) integrates fxceless + triippywhale's
clamp + its own work, tested on an RTX 5060 Ti (Windows).

**Their fixes that matter to us** (README + commits):
- `279c90a1` Shadow buffers' D3D12 resource states across lists (barrier errors id=527)
- `a65d0547`/`bd33acc6` descriptor-slot recycling + `MIN_DYNAMIC=2^18`
  ("16 384+ dropped draws" in the world)
- `6f99f751` submit one command list per fence wait + capped mesh dispatches (TDR)
- shader coverage: 64-bit image atomics, per-row interpolation, `v_cvt_f32_f64`

**Merge:** branch `try-vitoriabr` merged into `main` (commit `d4b5421`). Only two
files conflicted (both `v_cvt_f32_f64`, which we already had tested) — our side
was kept, their other lowering came in.

**Tests after the merge:** `victory_gpu` 112/1 (from 110/1), `victory_shader`
214/2 — a new failure `v_cvt_f32_f64_matches_ieee754` ("64-bit operand
modifiers", their lowering path), plus the known Wine digest.

**Visual result (captures9/frame-2366):** a night city silhouette with lit
windows, neon, vehicles and three characters; subtitles correct. **The first
time buildings appear** in our runs. Left: close-up geometry/transparent
surfaces, 0.4–2 fps, no RT light (BVH off).

**Next:** a deterministic menu sequence (Enter→Down→Enter) so we reliably reach
gameplay; then measure buildings/lighting again.

## 18. New fixes 02:30 (triippywhale + fxceless)

| Patch | What | Why |
|---|---|---|
| `611bee8` (triippy `eea9951f`) | Pages a copy never took count as **stale** under shader writes (`shadow.rs`) | Without it shaders read **zeros** there → **missing walls** with `VICTORY_LATE_WAIT`/`RELAY` |
| `761cfc3` (triippy `0cdf9317`) | Our own writes move `ahead_asked` together with `asked` (`watch.rs`) | Without it the same page was copied on **every refresh** → 1–2 s per frame in copies |
| `d139050` (fxceless `c96b4f2`) | Depth bounds test, faster defaults (relay 2, wide copy-back, card copies 1–1024 MB, 256 lists), textures before the bake | Gives ~230 ms/frame against ~400; depth bounds lit the lamp in the first room on an RTX 4070 |

**Crash pattern:** `+0x2215c35` (the bring-up reporter) appeared in the run with
depth bounds (02:24). At the crash there were **12.6 GiB free** — so not a system
memory shortage. Runs without depth bounds reached 2.6 fps without crashing. If
the crash repeats we isolate `d139050`.

## 19. The clzero experiment (2026-11-25 02:15)

**Measurement:** the intro is CPU-bound. In the video phase there are 2 draws,
GPU time 0.0 ms, copies 0.1 ms — but the frame takes 947 ms. `console_insn`
emulates ~25 000 `clzero` loops per second (1 048 576 in 40 s), each loop a VEH
exception.

**Implemented** (`fca83d9`, opt-in `VICTORY_CLZERO_STUB=1`):
- On the first exception for a site, the loop's 12 bytes are patched to
  `jmp rel32` against a generated stub that calls our region-aware zeroing and
  jumps past the loop.
- Stub page: `VirtualAlloc2` with an address window, else a scan in 1 MB steps
  (Wine takes address hints literally).
- **Mechanically verified:** 5 sites patched in one run, and the handler's own
  `zeroed_mb` stops growing (the stub does the work).

**But:** the title crashed right after the patch (`+0x4e8ab0a`, reading at -1).
An deviation in the loop's `rax`/flags probably remains (the loop leaves rax on a
64-byte boundary and flags from `cmp rax,end`; two variants tried). Therefore
**off by default**; with it off the run is clean again (0 crashes).

**Next for the intro:** a unit test that runs the loop in a synthetic function
and compares registers/flags against the real instruction, before the switch is
turned on.

## 20. The guard work is the bottleneck — not the copies (2026-11-25 03:30)

The cutscene frame (3 473 ms) split by the diag counters:

| Item | Before | After `59a1238` |
|---|---|---|
| `guards` (inside recording) | 1 677 ms | **123 ms** |
| texture refresh waiting on write-watch/guards | 635 ms | 256 ms |
| copies | 1 214 ms | 50 ms (phase dependent) |
| `write-watch askings ≥1 ms` | 465/frame, 1 855 ms | 1 179/frame, 2 386 ms (all threads) |
| GPU | 49 ms | 0–60 ms |

Conclusion: the frame is CPU-bound keeping host copies of console memory in
sync, not on the GPU and no longer on the copies. 441 ms of the frame is
**before the first list call**, 407 ms until the first submission.

What was done (our own work, `59a1238` + `watch.rs`):

1. `protect()`: every block of a guarded run is looked up in **one** pass under
   **one** lock (`inplace::alias_blocks`), deduplicated, and then protected with
   **one `VirtualProtect` per contiguous run** instead of one per 64 KiB block.
2. `protect_here()`: when the whole range cannot be protected (view memory
   always has holes), **one run of committed blocks at a time** is protected
   (`committed_until` through VirtualQuery) instead of block by block.
3. `open_here()`: opens runs, not blocks.

**A mistake I made and fixed:** the first version of (3) removed the blocks from
`g.protected` before it knew the range call had succeeded. If the run contained
an unmapped page the whole `VirtualProtect` call failed, the pages stayed
read-only and were no longer tracked → `0xc0000005 (write … read-only) at
victory.exe+0x5a046e`. Fixed with a per-block fallback that only forgets the
blocks when the opening succeeded.

## 21. What the repos said 03:30 (no new commits, new PRs)

Nothing new in main since the night. New open PRs that hit our bottleneck:

| PR | Contents | Relevance |
|---|---|---|
| fxceless #571 | DirectStorage `PAGE_DESTINATION` calls `kernel_will_write` | **taken** (`d3436d8`): the writes went through our view, so the title's addresses never saw them → no write-watch entries, no remap of index-picked textures → *missing cell walls/ceiling/floor* and black blocks |
| VIctory #1986 | fixes #1976 (a withheld range swallowed valid writes on the same 64 KiB page) | **unlocks** `cd94d353`+`940d32f6` = shader-marked pages, the only known way off "copy everything every frame" |
| fxceless #570 | `LATE_WAIT` ceiling + remap lock | conflicts in 3 files; mostly `pipeline/host.rs` (+488) — not our priority |
| VIctory #1980 | the heap lock held through the whole detile | `bindless.rs` does not exist in the fxceless tree → cannot be picked structurally |
| VIctory #1979/#1978 | reuse dead copies; no texture layouts per draw | require the VIctory tree |

Conclusion: the remaining big wins live in the VIctory tree (marked pages,
per-draw work) and cannot be cherry-picked directly — they must be ported.

### A/B: does #571 cost anything? (run-033230 with, run-033637 without)

| Cutscene (points ≤2 fps) | #571 ON (14 p) | #571 OFF (22 p) |
|---|---|---|
| fps / frame | 0.51 fps / 2 906 ms (median 2 585) | 0.44 fps / 3 090 ms (median 2 784) |
| copies, median | 905 ms | 838 ms |
| textures, median | **174 ms** (max 4 825) | 396 ms (max 4 630) |
| write-watch askings | 784 ms/frame | 335 ms/frame |
| crashes | 1 (the title's `int3`) | 0 |

Conclusion: #571 stays. It costs no performance in the cutscene, textures got
cheaper (correct remapping instead of endless retries), and it gives back the
cell walls/ceiling/floor. That askings rise is expected: more writes are now
tracked.

The cutscene sits at 0.3–1.3 fps with a median frame of 2.6 s. Breakdown: copies
0.8–1.9 s, textures 0.2–4.8 s, GPU 40–90 ms. The structural fix is shader-marked
pages (`cd94d353`+`940d32f6`+`#1986`) and copy pooling (`#1979`) — both in the
VIctory tree, both needing a port.

## 22. Wine's wineserver is an O(views) bottleneck — patched (fork #184)

fxceless/victory-fork#184 (reporter on RX 7900 XT, RADV 26.3.1, GE-Proton11-14):
Wine's `wineserver` walks `process->views`, a linked list, on **every** map/unmap
(`is_valid_view_addr` 54.7 % + `req_unmap_view` 44.3 % of the server's CPU). With
22 552 `memfd:wine-mapping` views in the process, each `MapViewOfFile3` cost
450–900 µs and each `UnmapViewOfFile2` 300–500 µs; the remap lock was held
460–930 ms per second. That is the "waiting for title memory to stand still" we
see.

| | stock wineserver | patched |
|---|---|---|
| map / unmap | 450–900 / 300–500 µs | **9–12 / 10–15 µs** |
| remap lock held | 460–930 ms/s | **12–43 ms/s** |
| "textures" per frame | 0.2–3 s | **30–57 ms** |
| preparing per frame | 0.5–3 s | **110–150 ms** |
| wineserver CPU | 57–91 % | 3 % |

The patch (~197 lines, `server/mapping.c` + `file.h`/`process.[ch]`, no protocol
change) indexes the views in a red-black tree per (process, base) as well.

Here: taken from the comment (UwUCammie, wine-11.22), applied to
**ValveSoftware/wine @ a47c73f** (exactly GE-Proton11-14's wine-11.0 Staging).
One hunk (`is_valid_view_addr`) differed — 11.0 uses `host_page_mask` where 11.22
uses `process->page_size` — adapted by hand. Built with
`./autogen.sh && ./configure --enable-win64 && make server/wineserver`.

Requires (installed 2026-11-25): `flex autoconf automake libtool mingw64-gcc`.

Usage: `VICTORY_WS=patched ./run-victory.sh <sec>` — Wine 11 ignores
`WINESERVER=`, so the launcher starts the patched server first (`wineserver -p`)
and lets the client connect.

Also from #184: `VKD3D_CONFIG=descriptor_heap` (our vkd3d-proton 3.2 has
descriptor heaps on by default, only `no_descriptor_heap` remains → probably
already satisfied), Microsoft's `d3dcompiler_47` (our own clamp copes without
it), `VICTORY_SKIP_PIPELINES=fc5e66f8f873d3bb` on AMD, and **the late-wait
switches (`LATE_WAIT`/`WIDE_BACK`/`CARD_BUFFERS`/`RELAY=2`) lost the device in 3
of 4 AMD runs** — our `VICTORY_FAST` contains exactly those.

### The wineserver: the protocol must match the client exactly

First attempt (ValveSoftware/wine @ a47c73f, Wine 11.0) gave **protocol 931**
while the GE-Proton11-14 client requires **938** → `version mismatch 931/938`.

Second attempt (wine-mirror @ 521c3e8d, Wine 11.9) gave **938, i.e. correct** —
the client accepts the server (no mismatch), but `wineboot`/`start.exe` fail with
`c000000d`. The tree is not the client's (Valve/Proton patches), so server and
client share the protocol number but not everything else.

Facts for the next attempt: ValveSoftware/wine `bleeding-edge` sits on 930 until
2026-11-18 and jumps to 965 by 2026-11-23. Upstream wine: 11.5=930, 11.8=931,
**11.9=934**, 11.10=942, 11.22=979. The right tree is therefore GE-Proton's own
Wine build (wine-staging + Proton patches) at the commit that gave 938 — it must
be taken from GE's build chain, not from a guessed tag.

Built and ready when the right tree appears: `tools/wine-patched/server/wineserver`
(931) and `tools/wine-patched-938/server/wineserver` (938). The launcher's
`VICTORY_WS=patched` prefers the 938 build when it exists.

**Lesson:** never edit `run-victory.sh` while it runs — bash reads the script as
it executes and dies mid-way ("unexpected EOF"). One measurement run was lost
that way.

### mrowsnezh/Linux-AMD-Nv (same fxceless base)

A single commit on top of `c96b4f2` (the same base as ours): `VICTORY_PIPELINE_WORKERS`
(one pipeline worker per CPU, below the title's thread priority),
`VICTORY_NO_HOST_GPU=1` for CPU-only diagnostics, and `docs/linux-proton.md`.
Their README reports 420–450 ms/frame (2–4 fps) on Windows/RTX 4070 with the same
base — so our 2.6 s/frame is a Linux thing (the wineserver) plus our remaining
copies. Commit taken in branch `pipeline-workers` (worktree
`@WORKTREES@/vfork-pw`), builds green.

## 23. What actually costs: the same 182 MB buffer, 40 times a frame

The diag log's `copied from …` lines (tool: `copy-cost.py`) show it plainly:

    240 ms  7480 MB/frame  41.0x  0xb580000+0xae00000

The same 182 MB buffer is copied **40–41 times per frame**, ~240 ms for that row
alone, ~1 400 ms in total. The title makes ~40 submissions a frame and writes
into the buffer between them, so every submission refreshes the whole buffer
again.

Why the whole thing? `Shadow::update` uses write-watch and copies only *stale*
pages (`copy_stale`) — but `watch::writes()` builds that list by scanning the
journal's `written: BTreeMap<page, asking>`, which never shrinks. After a while
all 44 000 pages of the buffer are in it, so every question scans 44 000 entries
(~5 ms), 40 times a frame.

The right fix is per-copy watermarks (upstream's `Tracked`/`Changed`), so a
question returns only pages newer than the copy's last question, plus pruning of
the journal. That is the next real piece of work.

**An attempt that did not hold:** merging refresh spans only when they overlap
(instead of taking the union) — device removal (`0x887A0005`) in two runs in a
row, and the 182 MB was still compared 40 times. Reverted.

**Measured, no gain:** branch `marked-pages` (copy-back only for shader-marked
pages): copies 812 ms vs 905 ms median (noise), median frame 3 550 ms vs 2 585,
0 crashes. Marked pages work (470 compared, 6.2 MB written back) but copy-back
was never the bottleneck — the snapshot reads are.

## 24. Catching up upstream 08:20 (three new fxceless commits)

fxceless/victory-fork got three commits on top of our base:

| Commit | Contents | Ours |
|---|---|---|
| `310c6eb6` | Packed textures stay in the upload queue; a texture on the card is refreshed when the card is full. Removes tens of thousands of slot nullings per run | **taken** (`db32a21`), measured: 0.46 fps / 2 380 ms median, 0 crashes — performance unchanged (as expected), quality better |
| `8dd30ab4` | View guard (same as our `6576f4dd`) + **page writes counted** (`VICTORY_PAGE_WRITES`), levels on demand (off), warm-up, card memory numbers, kernel reads beside | ported by subagent (7 files conflicted) |
| `a26859fc` | **The first room renders right**: byte-copied descriptor records mirrored (≈40 000/frame), views off their copy's elements get their own bytes, **card room for the world's first-frame burst** (98 buffer copies, 945 MB, where earlier runs made 1) | ported (13 files conflicted) — the one that could touch our 182 MB × 40 |

Other repos the same hour: triippywhale 20 (shader optimizer,
`VICTORY_WIDE_MERGE`), chapter-ii 13 (page pool through a view, in-place opt-in),
VItoriaBr 20 (immediate samplers), xcl 16 (README, "60 %, ~5 fps"), Scarlight 4
(docs, Linux-first), VIctory 0.

Measured today, same scene, `VICTORY_FAST=1`:

| Build | fps | median frame | copies | textures | crashes |
|---|---|---|---|---|---|
| `main` (`571107e`) | 0.51 | 2 585 ms | 905 ms | 174 ms | 0 |
| `marked-pages` | 0.51 | 3 550 ms | 812 ms | 307 ms | 0 |
| `open-aliases` | 0.48 | 2 726 ms | 1 575 ms | 651 ms | 0 |
| Batch split | – | – | – | – | **2 device removals** |
| default switches | 0.39 | 4 578 ms | 1 521 ms | 1 901 ms | 0 |
| `310c6eb6` | 0.46 | 2 380 ms | 1 458 ms | 151 ms | 0 |

Conclusion: none of these touch the real item — the 182 MB buffer compared 40
times a frame. The next real work is per-copy watermarks in `watch::writes` plus
journal pruning.

## 25. Upstream commits taken: the picture lifted, the time did not (08:40)

`8dd30ab4` + `a26859fc` ported (subagent, 24 + 13 files, +3 697/+965) and
measured: **0.46 fps / median 2 420 ms, 0 crashes**. The 182 MB buffer is still
compared 38–41 times a frame, so the copies stay at ~1 000–1 500 ms.

But **the picture took a big step**: faces/skin, a street lamp, wet asphalt with
reflections, city lights, correct subtitles (captures28/frame-2134). That is
"the first room renders right" plus the texture fix in `310c6eb6`.

`main` = `936fda3` (pushed). Switches that came with it: `VICTORY_HEAP_SYNC` (on,
mirrors byte-copied descriptor records ≈40 000/frame),
`VICTORY_CARD_ROOM_FOR_BUFFERS` (on), `VICTORY_PAGE_WRITES=guards`,
`VICTORY_VIEW_OWN_COPY` (on).

In progress: `#772` (in-place/VRAM: `VICTORY_INPLACE=3`, `POOL=1`, `4`) ported by
a subagent — the only change that removes the host copies entirely (upstream:
−75 % copy-back, frame 395→357 ms on Windows). `#781` (staging pool for
textures, 2 files) taken and measured.

Measurement method: `./compare-runs.py [count]` and `./copy-cost.py <log>`.

## 26. BREAKTHROUGH: in-place removes the copies (1 000+ ms → 9 ms)

`#772` ported (17 commits on top of `main`, branch `inplace-772`, 172 GPU tests
green). Run with `VICTORY_INPLACE=3 VICTORY_INPLACE_MB=4096` + `VICTORY_FAST=1`:

- **"title memory in place, arenas: 2 parts of private memory and 0 sections
  open, 3.29 GiB locked"** — private title memory opened as D3D12 heaps.
- **copies: median 9 ms** (from 994–1 458 ms), askings 3/frame (from 478–560),
  texture refresh 16 ms/frame.
- Frames **110–260 ms → 4–7.5 fps** in the points we reached.
- BUT: **device removal** (`0x887A0005`) after ~1 380 frames — exactly #184's AMD
  trap ("the remaps get fast → work in flight reads pages the title remaps").

Sections (the page pool) cannot be imported on our host: `VICTORY_INPLACE_POOL=1`
is gated and falls back to copies (otherwise shaders would read memory that only
*looks like* the pool — silent corruption). `VICTORY_INPLACE=4` (hot memory in
VRAM) is blocked by Wine's built-in `d3dcompiler_47`: the merge shader uses
`Load`/`InterlockedAnd` on `RWByteAddressBuffer` and gets `E5005`. The same gap
that fxceless #20 (DXC fallback) solves and Microsoft's `d3dcompiler_47` solves
(our prefix has Wine's 370 KB variant).

Next test: in-place **without** the late-wait/relay switches (only
`VICTORY_TEXTURE_NO_WAIT=1` + `VICTORY_PIPELINE_WAIT_MS=5`), because #184 pointed
at them as what tips AMD into GPUVM faults once the remaps get fast.

### In-place without late-wait: stable and 6–7 fps

`VICTORY_INPLACE=3 VICTORY_INPLACE_MB=4096 VICTORY_TEXTURE_NO_WAIT=1
VICTORY_PIPELINE_WAIT_MS=5 VICTORY_DISPLAY_MODE=1920x1080` (no RELAY/LATE_WAIT/
WIDE_BACK/CARD_BUFFERS):

- **0 device removals** over 230 s (against 1 with `VICTORY_FAST=1`).
- Typical frames **130–300 ms → 6–7.7 fps**; median in the later half ~150 ms.
- copies 0–30 ms, textures 1–70 ms, GPU 0–40 ms.
- Spikes remain: one frame 5 127 ms with **textures 2 373 ms** (streaming), one
  1 972 ms with 646 ms textures. That is exactly what `#781` (staging pool) is for.
- The menus got slower than before (30 fps → 7–10 fps): in-place adds per-frame
  work that did not exist when everything was host copies.

Against the `main` baseline (`936fda3`): **0.46 fps → 6–7 fps**, about 14×.

### In-place + staging pool (main = a10a46d)

`VICTORY_INPLACE=3 + TEXTURE_NO_WAIT + PIPELINE_WAIT_MS=5`, 260 s: **0 crashes,
median frame 176 ms**, a 20+ point stretch at **5.7–7.0 fps**, copies 2–116 ms,
texture refresh 12 ms/frame, askings 2/frame. The cutscene part (8 p) 1.04 fps /
984 ms — still heavy, but twice the baseline, and the spikes are texture
streaming (max 1 675 ms).

The launcher now has `VICTORY_INPLACE_RUN=1` as a preset (the same switches).

## 27. Honest state 10:12: in-place wins in the world, loses in the intro

| Configuration | Intro/menus | World | Stability |
|---|---|---|---|
| `VICTORY_FAST=1` | **30 fps** | 0.5 fps | 0 crashes |
| `VICTORY_INPLACE_RUN=1` | 11 fps (81 ms) | **6–7 fps** | **2 device removals of 4** |
| in-place + `INPLACE_MIN_KB=65536` | 6 fps (148 ms) | 1 fps | 0 crashes, but worse on everything |
| in-place + `9376fd60` (fingerprint) | 11.4 fps | 0.63 fps | 1 device removal |

Conclusion: `INPLACE_MIN_KB` was a dead end (the overhead stays, the copies come
back). The launcher's **default is now `VICTORY_FAST=1`** (smooth 30 fps intro);
in-place is opt-in through `VICTORY_INPLACE_RUN=1` and counts as experimental
until the device loss is solved.

Cause of the device loss: with in-place, shaders read the title's own pages, and
Wine allows (unlike Windows) the title to free/protect memory inside an open
heap. Safety rests on the HLE wrappers notifying before the call
(`title_memory_released`). Auditing every free/protect path is the next piece of
work.

On "no gameplay": the title's cutscene plays in real time, so at 0.5 fps it takes
the same wall-clock time but our 4-minute runs end in the middle of it. Longer
runs (10–15 min) are needed to reach the prologue.

## 28. CHECKPOINT 2026-11-25 10:40 — the intro flows (30 fps)

The user: "the intro flows really nicely now". Tagged
`checkpoint-intro-smooth-20261125` on `main` = `2a343e1`, pushed to origin.

State:
- `./run-victory.sh 240` (default `VICTORY_FAST=1`) → **30 fps in menu and intro**,
  0 crashes, no device removals.
- World/cutscene: 0.44–0.55 fps without in-place; **6–7 fps with
  `VICTORY_INPLACE=3`** but it loses the device in 3 of 6 runs (`0x887A0005`) —
  experimental.
- `LATE_WAIT=0` + in-place: 0.1 fps (9–12 s/frame) and a crash → overlap is both
  the speed and the fault. Closed road until the memory safety is solved (Wine
  allows freeing inside an open heap).

Textures (the next bottleneck, and the black blocks):
- `VICTORY_TEXTURE_STAGE_MB=768` + `STAGE_MS=600` did **not** help (0.44 fps,
  texture pass 457 ms): the queue grows anyway — 66 709 uploaded (26.9 GB),
  20 689 waiting, 3 982 passed over unmapped.
- The pass costs ~750 ms/frame in "pass over textures picked by index" with a
  budget of 4 096 slots; ~180 µs per slot despite the staging pool (#781).
- Actionable: find out whether the time sits in the `texture_slots_due`
  enumeration or in the staging per slot (diag only splits "texture_us" against
  "slots").

Backups: bundle + the backup host refreshed after the tag.

## 29. #824 merged: guards as intervals, and a real crash found (2026-11-25 11:30)

`#824` ported (8 commits, `guards-824`): the four per-block `BTreeSet`s in
`Guards` become coalescing intervals under an `RwLock`, `writes()` short-circuits
on `last_write`, the relay merges fetched card bytes in runs on up to eight
threads (`VICTORY_WIDE_MERGE`, now on), texture checks ask the write watch once
instead of twice, and `TEXTURE_NO_WAIT` is the default. Tests: gpu 184/1,
shader 214/2, hle 99/1 — the documented Wine-only failures.

Measured (same scene, default `VICTORY_FAST`):

| | `main` before | `#824` |
|---|---|---|
| cutscene, median frame (28 points) | 2 585 ms | **2 243 ms** |
| copies, median | 905 ms | **144 ms** |
| askings ≥1 ms | 158/frame, 784 ms | **64/frame, 388 ms** |
| textures, median | 174 ms | 112 ms |
| texture refresh | 102 ms | 41 ms |

### The crash we had all day: `EXCEPTION_GUARD_PAGE`

`#824` crashed deterministically after ~3 minutes:
`unhandled exception 0x80000001 at victory.exe+0x46d1c0`. Disassembly shows the
address is a `rep movsb` inside our guarded copy (`direct_copy`), and 0x80000001
is `EXCEPTION_GUARD_PAGE` — not an access violation. The title places guard
pages (XMem growable blocks, stacks), and `on_fault` accepted only
`EXCEPTION_ACCESS_VIOLATION` and `EXCEPTION_IN_PAGE_ERROR`, so a guard page took
the exception out of our handler and into the title's crash reporter. The same
code explains the earlier `0x80000001 at victory.exe+0x3b1618`.

Fix (`d5fc385`): a guard page takes the same path as an unmapped page — the copy
fails and the per-page fallback retries (the guard is one shot, so the retry
reads the page). Verified: **0 crashes over 330 s**, 44 points, the whole
cutscene.

`main` = `d5fc385`, pushed.

### Where the texture pass' time goes: staging, not the enumeration

Instrumented the pass (`HEAP_PASS_PARTS[2]`): finding the slots that are due
costs **0.7–1.7 ms** of the ~90 ms the part takes. The rest is staging (detile,
upload buffers, descriptor writes) — so the fix is cheaper staging, not a queue.

Staging pool (`#781`) reuses well — 17 455 made, 36 142 taken, 53 457 given back
— but **15 856 were released because the pool was full** (cap 512 MB), so the cap
is the next knob.

Best stable run so far (`main` = `d5fc385` + diag, default preset, 260 s):
**0.81 fps / 1 231 ms median**, copies 104 ms, textures 89 ms, GPU 73 ms,
askings 31/frame 50 ms, **0 crashes**. Against the morning baseline: 2 585 ms →
1 231 ms, i.e. 2.1×.

## §29 The `(3 being made)` bug: three compute pipelines kill their worker

`(3 being made)` sat in the frame line of every world frame of every run.
Three compute pipelines were queued, a worker took each, and the worker died
before answering. `WAITING` is lowered only by a worker that *returns* from
`make`, so the count never fell; every draw needing one of the three was
dropped for the whole run. Nothing in the tree noticed.

**Bracketing logs pin the death** (run-141201, `HP n` probes since removed):
translation finishes, `HostPipeline::new`'s root signature finishes, then
`CreateComputePipelineState` never returns. That is **vkd3d-proton** recursing
while it compiles the DXIL we hand it — not our stack, not
`victory-shader::structure::ite`. The same shader kills every worker it is
given, so retrying is pointless; the three are always the same:

```
compute a1b20bc47e3c9597
compute e8c962358d862e3f
compute 559d9f0184e15fe3
```

**What landed** (`7c0ce61`):

- A claimed pipeline that stops moving for `CLAIM_STALL` (5 s) is given up on:
  it is recorded as refused, so `missing()` names it and its draws are dropped
  at once rather than each waiting a stall first. `is_finished` is polled too,
  and would catch a thread that really ended — it does not fire under Wine,
  where the abort after a stack overflow *wedges* the thread instead of ending
  it.
- It lowers `WAITING` for the pipeline it gives up on, and starts a new batch
  of workers when none is left, so one death cannot stop the run.
- `Pending::made` is written with `OnceLock::set`, never `get_or_init`: a worker
  aborted inside `get_or_init` leaves the lock half-initialised and that
  pipeline can then never be made by anyone.
- `WORKER_STACK` 256 MB, for the recursion that is ours.

**Measured**, run-142034 against run-141613 (same diag build, `VICTORY_FAST`,
same scripted keys, 90 s):

| | before | after |
|---|---|---|
| `being made` | 3 on every frame line | 0 on 12 of 15, 1–2 only while dying |
| overflows | 6 | 3 |
| refused | 0 | 3, by name (above) |
| cutscene median | 1 416 ms | 964 ms |
| menu/loading | 23.7 fps / 33 ms | 25.0 fps / 34 ms |

The cutscene number is 3 points against 2 and both runs are inside the
0.6–1.1 fps spread this machine has shown all week, so it is **not** claimed as
a gain. What is claimed is the mechanism: the three pipelines are now named and
dropped instead of silently holding draws for the whole run.

`victory-gpu` 208 passed / 2 failed — the same two Wine-only failures as before.

## 30. The data gate was pointed at the wrong folder (2026-11-25 14:30)

`check-data.py` defaulted to the dump's root folder
(`~@DUMP@`), which is an **incomplete** copy
(61–89 % zero blocks), while the copy we run —
`@DUMP@/title-work`, the one `victory-work.toml` names — is
**complete**: 0.0 % zero blocks over 200 random 1 MiB reads per file, for
`levels`, `packs`, `shaders`, `textures`, `navmeshes`, `audio`, `anim`, `data`
and `movies`.

So the "OSKRIVEN" alarm from the agent's session was a false positive, and no
run has ever used incomplete data. The script now takes `game_dir` out of
`victory-work.toml` when no path is given, and prints that every file is written out.

Copies on disk, for the record:

| Folder | State |
|---|---|
| `@DUMP@/` | **complete** (the original set) |
| `@DUMP@/title-work/` | **complete** (the reflink copy we run) |
| `@DUMP@/` (root) | incomplete, 61–89 % zeros |
| `~/Downloads/title-work/` | incomplete, 90–96 % zeros — ~320 GB, a trap |

## 31. Indirect windows on RADV: no gain here, and the pass is still textures

`main` = `7c0ce61` (the agent's six commits: `f85101b` + `a2537066` + the Wine
clamp root signature + Belobug's guard hysteresis + the frame log + the pipeline
stall refusal). Tests 208/1 + the new `drvwatch` failure, which is Wine's
`EvtSubscribe` stub returning 0: **the driver watch is inert under Wine**, so a
window run here has no early warning.

Run `run-143850.log`, 240 s, `VICTORY_INDIRECT_WINDOW=1`, diag:

| | value |
|---|---|
| crashes / device removals | **0** |
| menus | 21.5 fps / 35 ms |
| cutscene (18 points) | **0.73 fps / 1 505 ms median** |
| copies | 162 ms median, 387 max |
| textures | 74 ms median, 760 max |
| GPU | 30 ms |
| askings ≥1 ms | 94/frame, 152 ms |
| best frames in the run | 215–220 ms (**4.5 fps**) |

So the windows cost nothing in stability here, but they do not move the median
on RADV (the same run class without them measured 1 092–1 231 ms). The frames
that do run at 215 ms show what is left once textures are not streaming.

Inside the same window the diag line reports the parts apart:

```
1508.3 ms in its pass over textures picked by index, 0.9 ms of that finding
the slots that are due; 81000 buffer slots looked at by its pass over buffer views
```

- Finding the due slots: **0.9 ms** (never the problem).
- The texture part: up to **1 508 ms** when the queue streams, ~90 ms when it
  does not — the spikes are streaming, not bookkeeping.
- The buffer-view pass still walks **81 000–114 500 slots** a frame; this is
  where `MrSkele`'s incremental spans (HANDOFF §5.5) and their stack-buffer
  user-data belong.

## 32. Manual save: straight into the world, and the entry burst is now the wall

The community's manual save (`Manual_Save.zip`, installed into
`victory-data/data/7952987A/`, old data backed up in `backups/`) works: the
title goes from the account screen into **Jason's part of the world** with no
cutscene. That makes gameplay measurement possible for the first time.

Three runs with the same keys (`25/45/65/85/120/160/200`):

| Run | Switches | Result |
|---|---|---|
| `run-144815` | default (`VICTORY_FAST`) | never left the menu — 30 fps for 300 s |
| `run-145550` | default | **reached the world** (10 cutscene points, 0.47 fps / 2 416 ms) then **device removed** (`0x887A0005`) with 10 submissions ahead |
| `run-145935` | `VICTORY_SKIP_PIPELINES=fc5e66f8f873d3bb` | no device removal, but after the first load burst it sat on the **loading screen at 30 fps** for two minutes with the card 1 % busy — a skipped pipeline is likely one the load needs |

What the world looked like before it died: copies 313 ms, textures 67 ms, GPU
15 ms, **askings 254/frame = 1 885 ms** — the entry burst is ask-heavy.

Next: drop the skip list and serialise instead (`LATE_WAIT=0`, `RELAY=0`,
`PLAIN` + `TEXTURE_NO_WAIT`): the device loss is work in flight reading pages the
title remaps, so nothing in flight is the direct mitigation.

## 33. `CGBJ-VIctory` (`integration`) benchmarked on RADV + stock Proton Experimental (2026-11-25)

Cloned `JordanFromIT/CGBJ-VIctory` (`integration`, updated to `85e8470`) into
`porting/cgbj/` with an isolated runtime in `porting/cgbj-runtime/` (copied
`Manual_Save` from `victory-data/data/7952987A/{saves,storage}`, leaving `vfork/`
and `victory-data/cache` untouched). Built for `x86_64-pc-windows-gnu` and run
under stock Steam `Proton - Experimental` (`experimental-11.0-20261112`,
`vkd3d-proton 3.2.0`, `ntsync`).

### Measured runs (`Manual_Save`, 1080p, `SKIP_MOVIES=1 RT_STANDIN=1 GI_STANDIN=0.5 FAST_QPC=1`)

| Run | Build / switches | Cutscene / entry | Gameplay (Jason balcony) | `outside` | GPU | Copies | Crashes |
|---|---|---:|---:|---:|---:|---:|---|
| `cgbj-run1-baseline` | `diag`, baseline | 0.8–2.6 fps | **3.9–4.0 fps (250–255 ms)** | 17–24 ms | 96 ms | 51 ms | **0** |
| `cgbj-run2-speed` | `diag`, `BATCH=1 WAIT_BATCH=1 PIPELINE=3 PARPREP=1 VRAM=1`, `taskset 0-11` | **2.6–3.3 fps** (307–384 ms) | 3.4–3.5 fps (286–295 ms) | 101–123 ms | **48–51 ms** | **24 ms** | **0** |
| `cgbj-run5-pipe2-allcores` | `diag`, `BATCH=1 WAIT_BATCH=1 PIPELINE=2 PARPREP=1 VRAM=1` | — | 3.4–3.5 fps (282–296 ms) | 90–107 ms | 60–65 ms | **23 ms** | **0** |
| `cgbj-run6-vram1-warm` | `diag`, `VRAM=1` (no `BATCH`/`PIPELINE`) | 1.9–2.2 fps | **3.8–4.0 fps (253–263 ms)** | 19–26 ms | 92–95 ms | 50–52 ms | **0** |
| `cgbj-run8-int-parprep` | `diag` (`85e8470`), `BATCH+PIPELINE=2+PARPREP(_ASK,_SNAP)+VRAM=1` | — | 3.0–3.2 fps (311–334 ms) | 125–149 ms | 57 ms | **21–22 ms** | **0** |
| `cgbj-run10-release` | **`release` (`ec5ffe4`)**, `VRAM=1` | 1.5–1.7 fps (6.1–7.1 trans.) | **3.8–4.0 fps (252–263 ms)** | **14–21 ms** | 95–101 ms | 57–60 ms | **0** |
| `cgbj-run11-release-speed` | **`release` (`ec5ffe4`)**, `BATCH+PIPELINE=2+PARPREP(_ASK,_SNAP)+VRAM=1` | — | 3.4–3.6 fps (281–290 ms) | 89–98 ms | 54 ms | **24 ms** | **0** |

**Key findings:**
- **0 crashes and 0 `0x887A0005` device removals** across all 1080p runs (vs 0.47–0.81 fps / device loss in `vfork` on `Manual_Save` without `INPLACE=3`).
- `VICTORY_LAZY_VIEWS=1` (default under Wine) + `VICTORY_FAST_QPC=1` (20k–30k QPC/s → ~620 host asks/s) eliminate the `wineserver` O(views) remapping wall without needing a patched `wineserver`.
- `CGBJ_CUTSCENE_FAST=1` (`BATCH=1 + PIPELINE=2 + PARPREP`) is **3× faster during cutscenes/world entry** (2.6–3.3 fps, ~13–29 dropped draws), while plain `VRAM=1` gives **3.8–4.0 fps** in gameplay (`outside` stays 14–21 ms without batch gate/fence waits).
- **Local patch on `cgbj` branch `perf-stack` (`ec5ffe4`):** removed `#[cfg(feature = "diagnostics")]` from `bink` (`VICTORY_SKIP_MOVIES`) and `gameinput::scripted()` (`VICTORY_KEYS`) so `--release` builds skip intro videos in 1 frame and auto-confirm the grey Player/Account screen (`run-cgbj.sh` defaults `VICTORY_KEYS="12:Enter,15:Enter,18:Enter,22:Enter,26:Enter"`, saving ~15–20 s of dead time while letting the ~2,100 cached pipelines start prebuilding).
- **Why `zlib`/`BCPack` is not the FPS bottleneck:** `vfork` already uses AVX2 `zlib-rs 0.6.8`; `cgbj` uses pure-Rust `miniz_oxide` and reaches 4.0 fps. Disk decompression runs across 20 worker threads and drops to `<1 MB/frame` (`44–73 worker ms`) once streamed; early ~1 fps and black/transparent geometry are on-demand pipeline builds + texture streaming.
- **`--console series-s` on `cgbj`:** currently crashes at startup (`amd_fidelityfx_upscaler_xbox.dll` refused by host DLL policy → `0xc0000005` at `+0x4e8f487`).
- **Wine / direct-Vulkan summary:** `CGBJ-VIctory` (`PROTON-STATUS.md` §6) evaluated rewriting on Vulkan (`vkprobe` on `census`) and rejected it: `vkd3d-proton 3.2` + RADV already takes only 48–96 ms/frame; the remaining bottleneck is CPU-side shadow/write-watch coordination.
- **`DexNom/VIctory` evaluated (read-only):** Windows-only fork of `ZSlayerHQ`/`Marvisak` (`3.52–4.05 fps` non-RT, `1.03 fps` with experimental RT on RX 7900 XTX). Lacks Wine/Proton optimizations (`LAZY_VIEWS`, `FAST_QPC`), so not a drop-in speedup on Linux, though its shader fixes (40 shaders via static-sampler branch selection, 18 water vertex shaders, 5 shadow shaders) and RT read-only write-tracking fix (`bc3cd4b0`) are clean cherry-pick candidates later.

### Env-switch sweep + `VICTORY_WATCH_ONCE` port (2026-11-25, "get FPS up")

Profiled the `--release` steady-state breakdown on the Jason balcony
(`cgbj-run-162733`): **3.5–3.6 fps / ~277 ms** = `gate` 71–74 ms (batch
thread waiting on GPU fences) + fence waits 44–46 ms + copy-back 33–35 ms
+ GPU 56 ms + copies 21 ms. GPU is only 56 ms, so the frame is pinned by
the CPU↔GPU serial dependency, not by raw work. Swept the existing knobs:

| Switch | Gameplay | Verdict |
|---|---:|---|
| `VICTORY_COPY_DEDUP=3` | 3.4–3.6 fps | neutral |
| `VICTORY_GPU_HAZARDS=1` | 3.6 fps | neutral (GPU 56→48, gate 72→76 absorbs it) |
| `VICTORY_ASYNC_SUBMIT=1` | 1.9–3.9 fps | **worse** — breaks the title's GPU-wait fences (`given_up` loop) |
| `VICTORY_BATCH_PREDICT=1` | 3.5–3.6 fps | neutral |

Ported `prep` (write-watch reuse, `VICTORY_WATCH_ONCE=1`) as `fa5f54e`
(adapted for the `VICTORY_PIPELINE` path upstream's 75-commit-old branch
lacked; 4 unit tests pass). Verified active: host write-watch askings fell
**~43 %** (6 306 vs 11 117/frame, ~5 000 answered by an earlier batch), but
the frame stayed **~284 ms / 3.5 fps** — prep 92→88 ms and copies 21→19 ms
were taken back by the gate 71→79 ms.

**Conclusion:** five independent data points all show the bottleneck is the
**gate (CPU waiting for the GPU to finish writing title memory the next
batch reads)**, not askings, copies, textures or `zlib`. This is the
write-watch/shadow model's serialization, and no fork breaks it: `DexNom`
reports the same ~3.5–4 fps non-RT ceiling on the same card. The remaining
levers are (a) cut GPU time (DexNom shaders / RT stand-in), (b) cut
copy-back, or (c) a deeper multi-frame overlap — none is a one-line switch.

## 34. CHECKPOINT 2026-11-25 17:45 — stable 3.5–4 fps; in-place is a dead end on RADV

`cgbj` (`perf-stack` = `origin/integration` `85e8470` + `ec5ffe4` + `fa5f54e`)
is the stable baseline: **30 fps menu (SKIP_MOVIES+KEYS), 2.2–2.9 fps world
entry, 3.5–3.6 fps gameplay, 0 crashes, 0 device loss.** Detailed world
breakdown (`cgbj-run-183453`, diag): **288 ms/frame = gate 73 ms + copy-back
39 ms + GPU 54 ms + copies 23 ms + prefetch views 17 ms + prefetch watched
10 ms + early scan 10 ms + textures 8 ms**, 6 459 draws. The gate's fence
waits are `title memory ~15 ms` + `host copy written ~20 ms` + `host copy
stale ~8 ms` + `in flushes ~12 ms`.

What was tried and ruled out this session:
- **Env-var sweep** (`COPY_DEDUP`, `GPU_HAZARDS`, `ASYNC_SUBMIT`,
  `BATCH_PREDICT`) — all neutral; `ASYNC_SUBMIT` broke the title's GPU-wait
  fences (`given_up` loop).
- **`VICTORY_WATCH_ONCE`** (ported `prep`, commit `fa5f54e`, 4 tests pass) —
  verified active (43 % fewer write-watch askings) but neutral on FPS: the
  gate takes the savings back.
- **vfork `INPLACE=3`** — the only known 6–7 fps path, but it is a dead end
  here: it no longer reproduces (runs show **0 MB placed in place**, "turned
  down" for every range), never reached the world, slowed the intro to 16 fps
  with audio clipping, and the historical device loss (3/6) stands. The
  device-loss fix (`settle_all` in `title_memory_released`/`title_memory_freeing`,
  vfork `host.rs`/`arena.rs`/`inplace.rs`) is implemented but **uncommitted**
  and untestable until the world is reached.
- **Belobug-AMD `VICTORY_STRETCH`** (read-only, 1 519★) — reached stable
  5–7 fps on Windows, but it is **section in-place**, which vfork's
  `inplace::aliases()` already measured as **not imported by vkd3d-proton on
  RADV 26.2** (the GPU reads another allocation, silently). So it cannot be
  ported; only Belobug's CPU-side changes (read-write-lock descriptor views,
  async submit) are portable, and they target different bottlenecks.

`vfork` carries an uncommitted `settle_all` WIP (the INPLACE device-loss fix)
and commit `edec23f` (VICTORY_KEYS in release, measured: advances past the
Account Picker/Player screen).

**Next (chosen):** attack the copy-back that is on the critical path. The
frame line shows copy-backs "forced by a wait ~6/frame" (synchronous, on the
gate) beside "completion thread ~7.6/frame" (already async). Move more
copy-backs off the wait path onto the completion thread, measure before/after.

## 35. Repos checked + VICTORY_HOST_BAR + in-place white-screen (2026-11-25 18:00)

Checked the tracked repos for new fixes relevant to the CPU<->GPU memory
problem:

- **CGBJ-VIctory `rebar` work** (`VICTORY_HOST_BAR=1`, `999eb3e`): places host
  copies the CPU never reads back into BAR VRAM (CUSTOM WRITE_COMBINE/L1).
  Merged into `perf-stack` (`d1dc40e`, with the `keys` feature that supersedes
  our `ec5ffe4`). **Measured: neutral** — it placed 5.3 GB of loose windows +
  208 MB of buffers in the BAR with 0 wrong CPU reads, but the frame stayed
  2.8-3.6 fps / ~285 ms (the gate absorbs the 25-39x faster GPU reads).
  Confirms bandwidth is not the bottleneck; the copy-back *dependency* is.
- **fxceless `0b0074e0` "copies in place"** (7-8 fps on RTX 4070): its
  `VICTORY_COPY_PLAIN=1` (plaincopy.rs, a rep-movsb copy with a fault handler)
  is **already in vfork**; `VICTORY_FAST_CORES` (submit/copy-back on P-cores) is
  not, but is equivalent to the `taskset -c 0-11` already measured (neutral).
- **vfork `INPLACE=3` second run (warm cache)**: reached the world load, placed
  **84 MB in place (16 windows, 2 arenas, 3.29 GiB locked)**, but 1,524
  turn-downs (738 buffers crossing the 2 GiB part boundary, 558 section views
  not SEC_COMMIT). Then at world render: **white screen, then fades back to the
  loading screen and crashes** (device loss). The white screen is the missing
  RT light (vfork has no RT stand-in), and the crash is the free/protect race —
  both RADV-specific blockers that make in-place unworkable on this stack for now.

**Verdict:** `cgbj` (2-3 fps, fast load, WASD works, correct rendering via
`VICTORY_RT_STANDIN`) remains the best obtainable result on this hardware. The
in-place 6-7 fps path is blocked by vkd3d-proton not importing section pages and
the white-screen/device-loss, both of which need upstream or large RADV work.

## 36. RT light on cgbj: the shaders already translate — the stand-in is the mask (2026-11-25)

Task `TASK-RT.md` assumed the ray-traced passes are dropped because their shaders
**do not translate** (the `vfork`-era state). That is not true in `cgbj`
(`perf-stack`): the trace shaders translate and run. The "lit but flat" world is
not dropped RT work — it is the GI stand-in (`VICTORY_GI_STANDIN=0.5`)
overwriting the real, already-computed indirect light with a constant.

### What the logs show (release `cgbj-run-190902`, diag `cgbj-run1-baseline`)

- **No translation failure.** 2 487 shaders translated, 0 `does not translate` /
  `refused` lines. The trace hashes all translate and make host pipelines:
  `CS_ScreenProbes_TraceRays` (`f22a76fcb967ee59`), `CS_IrradianceCache_ShadowTrace`
  (`f607780aa8e502d0`), `CS_LaserLightShadowRaytracing` (`32ed072b9f845048`), and
  the shadow masks (`559d9f0184e15fe3`, `a1b20bc47e3c9597`, `e8c962358d862e3f`) —
  the three that killed a pipeline worker in `vfork` §29 compile fine here.
- **No RT dispatch is dropped for translation.** The RT stand-in
  (`VICTORY_RT_STANDIN=1`) never fires: **0** `RT stand-in: a dropped … cleared`
  events in every release and diag run. `standin.rs`'s texture clears are dead
  code on cgbj — the passes it lists all bind and run.
- **The BVH plumbing works.** The trace passes bind their window:
  `RT window bound … pipeline=[compute f22a76fcb967ee59] … held_mib=514
  structures=[3 TLAS]`. `Known`/`rtwindow.rs` reads real top-level structures:
  `top-level structure read … tlas=0xda245ee00 instances=1642
  bottom_levels=140` (up to 2 793 instances / 367 BLAS in run 190902). The
  builders (`XboxBvhBuilder.dll`, 52 hashes in `rtwindow.rs::BVH_BUILDERS`)
  translate and run; their writes are `gpu_keeps` (kept in the arena, not copied
  back — the DexNom "distinguish guest reads from writes" lesson, already in
  `draw.rs::root_view`/`bind_window` via `kept` and `window_writes`).
- **The flat light is `VICTORY_GI_STANDIN=0.5`.** It fires after every
  `CS_FinalGather_*` and `CS_ScreenProbes_TemporalResolve_*` dispatch *runs* and
  fills u38/u40 (R11G11B10) and u32/u33 (R9G9B9E5) with `0.5`
  (`draw.rs::ambient_stand_in`). Those passes gather from the trace's ray records,
  so their own output is overwritten.

### Measurement: real indirect light is free (`VICTORY_GI_STANDIN=0`)

Ran the diag build, `Manual_Save`, 220 s, `VICTORY_RT_STANDIN=1 VICTORY_GI_STANDIN=0`
(`cgbj-run-201540`, capture `cgbj-runtime/frames`). World entered frame 698.

| | GI stand-in 0.5 (baseline) | GI stand-in 0 (real) |
|---|---|---|
| fps / frame | 3.6 / ~279 ms | **3.5 / ~286 ms** |
| GPU | ~58 ms | **53–55 ms** |
| copy-back | ~37 ms | **35–38 ms / ~12 MB** |
| device loss / crash | 0 | **0** |

No regression — the trace passes already ran under the stand-in, so unmasking
them costs nothing. Frame statistics (PIL, half-size PNGs): world frames are lit
and varied (`lum ~92–155`, colour σ ~80–102), **no magenta** (Belobug's broken-
resolve symptom), and a small true-black tail (4.9 % vs 0.1 % under the constant
fill) consistent with real shadowing rather than a uniform 0.5 wash. Frames
`frame-{728,758,878,938,998}.png` are saved for eyeballing; I could not view
them myself (no image input), so "correct" vs "plausibly-lit" is not yet judged.

### What remains (blocked vs. done)

- **Done:** translate, run, bind the BVH window, serve the builders, no device
  loss with the stand-in off — i.e. the "definition of done" machinery is in
  place and opt-in already exists as `VICTORY_GI_STANDIN=0 VICTORY_RT_STANDIN=0`.
- **Open:** (a) a visual A/B to confirm the indirect light is *correct* (the
  saved frames need a human eye or a second run at the same pose);
  (b) a single `VICTORY_RAYS=1` convenience switch that maps to those two
  stand-ins off, so "real RT" is one knob; (c) Belobug's two device-loss causes
  (lists/descriptors naming already-destroyed copies, and a read at an address no
  resource held) have not been exercised here because no device loss reproduced
  yet — watch for them once RT is on by default and the runs get longer.

## 37. RT enabled: VICTORY_HOST_BAR=1 frees the RAM for real ray-traced light (2026-11-25 ~20:00)

The RT work (TASK-RT.md) found that `cgbj` already translates the ray-traced
passes and serves the BVH builder, so `VICTORY_GI_STANDIN=0 VICTORY_RT_STANDIN=0`
runs real traced light at **3.2-3.6 fps (no FPS cost)** with 0 device loss. The
blocker was memory: the RT working set plus our ~7 GB of host copies peaked at
**31.2 GB** of 31.8 GB and OOM'd.

Fix measured: `VICTORY_HOST_BAR=1` places the loose windows (the 5.3 GB streaming-
heap copies) in the BAR (VRAM) instead of system RAM — `loose windows 924 (5304 MiB)
in BAR, 29 (177 MiB) in system memory` — which drops the peak system RAM to
**~25.5 GB** (6 GB headroom), VRAM to 17.2 of 23.3 GB. 0 OOM, 0 device loss over
280 s. `VICTORY_LOOSE_BUDGET_MB` does NOT help (the loose windows are held by
title lists, not evictable).

Recipe (now a `run-cgbj.sh` preset): `CGBJ_RT=1` = `VICTORY_HOST_BAR=1
VICTORY_GI_STANDIN=0 VICTORY_RT_STANDIN=0`. Needs human visual confirmation that
the traced light is *correct* (the layer can only prove it runs lit and stable).

## 38. Missing draws on the RX 7900 XTX at the world transition (2026-11-25 evening)

CGBJ perf-stack at `d1dc40e`, diagnostic GNU Windows build, Intel i5-13600K,
RX 7900 XTX, stock Proton Experimental, vkd3d-proton 3.2 and RADV. The
swapchain in this experiment was 3840 × 2160. PNG captures are half-size.
Each launch used its own reflinked prefix, cache and save and the same scripted
input against the working title copy. Host BAR, VRAM, FAST_QPC, BATCH,
WAIT_BATCH, PIPELINE=2 and all three PARPREP switches were on. RT_STANDIN=0.
Private logs and PNGs are in the Codex work directory under
`amd-frame-profile/`. The public patch sources contain no title data.

**Cold scene entry.** In `control-1`, one reporting interval reached 856 ms
per frame with 1,356 host pipelines still building. Later in the same shot,
at 12,442 draws per frame, the queue was empty and a frame still took 595 ms
(67 ms in the GPU timing, 403 ms outside submissions). The latter counter is
a residual. Ghidra confirmed a title semaphore wait and the worker's release
path. Nine of ten main-thread world stack samples took the same wait from
title +0x23cf735. Four samples of the releasing worker were in the Intel
exception path for CLZERO, five were in a child-job wait and one reached the
page-mapping flush. The loader experiment reached 388 ms in its one completed
heavy-scene trial against 462 and 512 ms in the controls. Its repeat ended in
a title exception before the world. The page-bounded CLZERO patch is kept
separately from the GPU candidate. These timings use median five-second
reporting windows 5–35 seconds after world entry, ≥10,000 draws/frame, with no
pipeline still building at the report. A low-priority build overlapped part of
the second control. The counters do not measure time in Wine alone.

**Descriptor exhaustion.** The small and large pools ran the same instrumented
executable from identical seeds, with GI_STANDIN=0. At offsets +5, +24 and +60
from world entry the 262,144-slot pool dropped 1,285, 794 and 1,196 operations.
Every one hit the allocation failure in `State::tables` at `host.dynamic(n)`.
The affected shaders include textured mesh and skin, the alpha prepass,
shadows, light-probe shading and tiled lighting. The larger pool had zero
descriptor refusals in all six traced offsets (+5, +24, +60, +100, +140,
+180). At +60 the small-pool frame 827 shows rectangular holes in the dock
and buildings. Large-pool frame 867 shows continuous geometry at the same
camera location. The number of commands recorded at the sampled offsets
varied between launches. These traces establish failure causes, while
performance comparisons use separate launches.

**Lighting.** Even with the larger pool, the sun-facing character view
retains an intense star and rainbow ghost. The final-gather passes execute,
but u38 and u40 of FinalGather_0_1_1_0 and u33 of the sampled temporal resolve
reported 0.0% nonzero in large-pool frames 832, 896 and 960. The t246 flags
texture had bit 2 set in 33.1%, 0.8% and 0.3% of sampled pixels respectively.
These are measurements of specific intermediate resources. The correctness
of the exposure and the source of the missing indirect light remain open.

**Upstream work checked against this build.** triippywhale/victory-fork
`43b65e2` doubles the dynamic tail where an uncapped heap can take it. The
CGBJ port opts in with `VICTORY_DYNAMIC_SLOTS=524288` and retries the previous
budget if the larger heap cannot be created. VItoriaBr `a91ef70`/`009856f`
also fixes a free span straddling the search cursor. Belobug `788ea46`
avoids repeated full scans when the heap is exhausted. Neither change was
included in the measured pool-size experiment. CGBJ integration now has
`f00b8e8` and `55035b8` for DispatchX thread counts in RT builders, and
`a7ee697` for ordering their atomics. They are opt-in candidates for a
separate GI test. A second GPU candidate normalizes integer-target blending
before pipeline cache keys are computed. The original vkd3d-proton rejected
R8_UINT at RT2 although both shaders translated. REA/Ghidra confirmed the
validation path in the loaded d3d12core.dll. The patched run created the mesh
pipeline f5e50eb2108c425b / ce3f0432a08f1d35.

**Archive coverage.** Offline enumeration of the working shaders.rpf found
25,889 distinct, parsable containers. The final diagnostic translator could
serve 25,587 (98.83%). Of the 302 failures, 195 had no supported vertex,
pixel, compute or mesh program, 40 needed a descriptor selected by control
flow and 18 joined a descriptor across multiple blocks. The baseline game
log reported 2,659 distinct shader hashes translating successfully and no
translation refusal. The archive census is wider than this save and does
not measure whether a translated shader draws correctly. The two GPU
patches passed the blend-state cases and the dynamic allocation/release
case at both pool sizes under Wine.

At world offset +5 in the dock shot, with 524,288 slots, 6.3% of screenshot
pixels had 8-bit luma ≥245 with native GI, against 37.7% with GI_STANDIN=0.5.
World offsets align the first substantial recorded frame, so exposure can be
at different stages. With the fill on, the sampled final-gather u38 reported
100% nonzero and mean RGB (0.5, 0.5, 0.5). The ambient run still recorded
five descriptor refusals at +60. The character camera in that run faced a
different direction, so its later frames are excluded from the lighting
comparison.

**Warm timing without per-operation traces or captures.** One launch per
budget, large then small. Both used the same final GPU-only executable
(SHA-256 `1fef9f3e01fb2791b74c4aade4c33785ce5f4e64040f80c77cae3872e2910a27`),
the original loader, GI_STANDIN=0, identical precompiled shader caches,
2,267 pipeline recipes, and clones of the same vkd3d cache and save.
Both reported 2,487 shader-cache hits and 36 misses at the same cache
checkpoint. The five qualifying reporting windows per launch used the
same selection as the loader experiment.

| Dynamic slots | FPS | Frame ms | Draws/frame | Prepare ms | GPU ms | Copy-back ms | Outside submissions ms |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 262,144 | 1.1 | 902.7 | 11,932 | 101.8 | 66.2 | 22.6 | 712.7 |
| 524,288 | 2.9 | 348.4 | 13,572 | 111.9 | 78.7 | 25.8 | 128.6 |

All five large-pool windows reported zero drops. Each small-pool window
reported at least 564 drops in the host-pipeline counter and 1,118 in the
indirect counter. Those counters can overlap. The source of the remaining
348 ms includes substantial preparation, GPU work, copy-back and title-side
waits. The large-pool median waits for present and page mapping were 81.8
and 71.8 ms; waits overlap other timings. More trials would be needed for
a run-to-run uncertainty estimate. The ordinary launcher still selects
its previous binary and pool budget.

**Same-camera post-processing comparison.** An instrumented large-pool build
omitted only PS_FlareMask (4273011b11446435) during world offsets +125 to +144
and restored it at +145. Frame 699 (+124) has sharp star rays, frames 701–719
(+126 to +144) lose them and frame 721 (+146) regains them. The broad white
area and the rainbow ghost remain in both states. This change was diagnostic
only. Cross-run screenshots and the on/off/on sequence are stored as
`descriptor-gi-comparison.jpg` and `flare-mask-comparison.jpg` beside the
private run logs.

In a second same-camera trial the PS_CalculateGhosts draw (9b75e0fbdd13c3aa)
was suppressed for +125 to +144, enabled for +145 to +159, then suppressed
together with the star mask for +160 to +179. Frames 669/675/691 show the
rainbow ring disappearing and returning, while the star rays remain. Frames
703/713/727 show both effects disappearing and returning. Two broad white
patches at the windows persisted with both shaders omitted. The log records
61 diagnostic omissions across the two intervals. This localizes the visible
effects to their post-processing passes. Their intensity and the bright
inputs that drive them still need an exposure or reference-image comparison.
See `lens-reflection-comparison.jpg` in the private result directory.

**Published fxceless shader translator on Radeon.** The supplied boat
close-up shows 6.7 FPS in a later cutscene and reportedly came from an
unpublished fxceless build on Windows 11 and an RTX 4070 after 15–20
minutes. Our dock and balcony captures use another camera and checkpoint.
The settings and private source of the reference could not be compared.

We streamed the exact 302 distinct inputs CGBJ refused from the working
archive into the public fxceless translator at
`085ee7c4c3d1dfaed41d683fa9f465a6f9dac8c3`. It emitted DXIL for the same
52 at both maximum wave sizes, 32 and 64. With the wave-64 setting an
independent D3D12 device test on RADV accepted all 52 test pipelines
(22 pixel, 15 mesh, 10 compute, 5 vertex). Graphics programs used a
matching test stage. This checks translation and pipeline creation.
Rendering the boat scene and checking its inputs requires its checkpoint.

All 11 rejected `TransformSkin` variants became valid test pipelines,
as did `PS_RasterizeWaterVolumeExposedSurface`. CGBJ had refused the 11
when a descriptor was merged across several blocks or through too many
choices. The water-volume program had used computed barycentric weights.
The list also includes deferred materials, mirror reflections and
directional shadow tracing. None of these 52 hashes appeared in the
baseline or the warm large-pool game logs. The shader comparison matrix
is in `prebuild-replay/fxceless-085ee7c4-radeon.tsv` in the private result
directory. The public translator was tested separately from the CGBJ
GPU candidate.

In a separate warm replay on the Radeon, CGBJ loaded all 2,314 recorded
pipeline recipes twice (1.5 and 0.8 seconds for the pipeline phase after
the archive scan). Both runs reported zero new compilations, refusals or
unusable recipes. This measures the recorded workload. The later boat
cutscene could request other recipes.

**Independent shader-pair control.** The
[asset format notes](https://github.com/jaylikesbunda/GTA6-Asset-Info/blob/648739f9e9a434cf3538379fdea38bea794f0eef/structure/06-models-shaders.md)
identify the compressed payload offsets 0x1b2392 and 0x5176e02 in shaders.rpf.
The local working archive has the same VS_Clear and PS_Clear at those
offsets, with local hashes e9dfb322e336c536 and 77d0ca9010e94769. CGBJ
translated both and RADV created the paired graphics pipeline. The
pixel shader reads four words from a constant buffer through the descriptor
table. Using the unmodified CGBJ translator and a scratch readback harness,
we bound a test constant buffer, drew a full-screen triangle into a 64 × 64
RGBA32F target, and read back two independently selected colours.
All 4,096 pixels matched (0.125, 0.25, 0.5, 1) exactly, and all 4,096
matched (0.75, 0.5, 0.25, 0.125) on the second draw. This exercises a
real vertex/pixel pair and one constant-buffer binding. Texture sampling,
skinning and indirect lighting are outside its coverage. See
`prebuild-replay/asset-info-clear-pixels.log` in the private results.
The repo also documents distinct hashes for base-colour and normal-map
bindings. Its reported pipeline-cache layout has no confirmed location
in the archives the authors scanned.

## 39. fxceless shader port and a warm harbor-to-house replay on AMD

CGBJ branch `perf-stack`, shader commit `4eece1b`. The port adapts the
public fxceless translator at `085ee7c4` to CGBJ's root layout. It handles
computed pixel interpolation, descriptors carried through vector lanes, and
values selected on control-flow edges. It preserves the CGBJ address window,
descriptor representation and stride table. The archive, executable and DLLs
yielded 25,985 distinct shader containers. Translation improved from 25,651
to 25,703. All 52 additional programs passed a D3D12 pipeline creation test
on the RX 7900 XTX with CGBJ's host signature, including all 11 additional
skinning variants and the water-volume pixel program. The water test used
the corrected barycentrics signature reader. Of the additions, 15 were mesh,
5 vertex, 22 pixel and 10 compute shaders. The translator suite passed 181
tests with one ignored. The remaining 282 include containers of unsupported
stage types. None of the 52 hashes appeared in the recorded harbor/house
shader binds or translation logs.

The Windows shader suite on Wine passed 207, with one ignored and the
previously documented `emit::tests::stub_pipeline_is_created` failure.
The normal release executable with scripted key support also precompiled
25,703 shaders from a cold build-specific cache and loaded all 2,314 recipes.
One release start hit the same title exception at +0x35a44d5 as two baseline
starts. A repeat with a fresh copy of the initial save reached the harbor
and bound the foliage pixel shader at frame 880.

### Matched performance runs

The four successful runs used the GNU Windows diagnostic build, i5-13600K,
RX 7900 XTX, RADV 26.2.6, Proton Experimental, a 3840 × 2160 swapchain, and
the same working save and scripted input. Both variants used 524,288 dynamic
descriptors, native indirect lighting and RT, host BAR, batching, two-level
pipelining and parallel preparation as in §38. Each run had a separate
reflinked runtime. Both builds prewarmed shaders and replayed the same 2,314
pipeline recipes. Timed runs had captures and per-draw traces off.

| Build and run order | Harbor ms/frame | Harbor FPS | House ms/frame | House FPS |
|---|---:|---:|---:|---:|
| Before, first | 354.6 | 2.8 | 288.0 | 3.5 |
| Port, second | 361.9 | 2.8 | 300.2 | 3.3 |
| Port, third | 352.1 | 2.9 | 313.2 | 3.2 |
| Before, fourth | 359.3 | 2.8 | 279.1 | 3.5 |

Each number is the median of the eligible reporting windows in that phase.
Harbor windows are 5–35 seconds after world entry with at least 10,000
draws/frame. House windows are 48–85 seconds after entry. Both require zero
pending compilations. Two baseline timing starts and one baseline visual
start failed before world entry and were excluded. The camera diverged
indoors, where the two ported runs reported medians of about 6,461 and 9,012
draws/frame. The exterior is the better matched comparison. The first
fully exterior reporting window averaged 388–402 ms/frame before and
393–431 ms/frame after.

At the dock the ported runs spent 111–112 ms/frame preparing work, 78–79
in the GPU timing, about 28 copying back and 138–146 outside submissions.
The GPU counter includes host synchronization. The outside counter includes
title work and waits. In the first ported run the median page-mapping wait
was about 72 ms/frame. The present path also waited. These span overlapping
intervals. One 361.9 ms window performed 2,818 GetWriteWatch calls (34.55 ms)
and moved 302.7 MB into host copies per frame. Ghidra confirms that the
semaphore sampled in nine of ten earlier main-thread stacks waits for job
completion. The worker can wait on a child before signaling. The counters do
not assign the entire residual to Wine. During both ported runs the median
hardware samples at the dock showed 39% GPU busy, 16.9–17.0 GiB VRAM and
39–40% host CPU. Hardware sampling began after the first baseline.

### Image and lighting checks

In separate visual runs, frames +5, +24 and +60 from world entry showed the
same sparse branches and dark facade before and after. The median facade
luma was 39, 40 and 40 on both builds (8-bit values, crop x=920..1119,
y=405..456 in the 1920 × 1080 capture). The later house captures showed
different views, so a pixel-by-pixel lighting comparison there would be
misleading. Five ported samples covering frames 832–1088 showed nonzero data
in about 79.4–79.5% of the sampled Primary_GatherVBuffer/u36 output.
FinalGather/u38, FinalGather/u40 and TemporalResolve_ReflectiveCaustics/u33
all reported 0.0% nonzero in all five. The first incorrect producer or
input binding remains unknown.

In a separate same-run control, the diagnostic build suppressed the four
`PS_PropFoliageDeferredTo_GBuffer` pixel variants only during world frames
+12..23 and +36..47. It counted 5,770 and 5,316 suppressed graphics binds,
with zero suppressed in the three intervening or surrounding on periods.
The two crowns in front of the sky kept their bare outlines in all five
periods. The mean absolute 8-bit luma difference in the left crown at the
four switches was 1.51, 1.82, 1.65 and 1.17; the neighboring +10 to +11
pair with the draws enabled measured 2.23. The camera and temporal image
history move slightly between frames. These four material passes produced
no visible leaf canopy in the inspected view. The earlier trace at +24 had
208 of 255 unambiguously paired foliage indirect records with positive group
counts along all three axes. Group counts alone cannot establish raster
coverage. See `fxport-foliage.jpg` and `fxport-foliage.tsv` in the private
run directory.

Hardware occlusion queries in `fxport-foliage-coverage-3` bracketed real
indirect mesh draws at offsets +5, +12, +24, +36, +48 and +60. The four
foliage materials produced just 41 passing samples across 239 queried draws.
Only 18 draws produced any samples. Two deferred textured control materials
produced 36,354 passing samples across 180 queried draws (60 nonempty).
Those counts cover the whole render target and a fixed subset of draws.
The queries establish how little visible coverage those foliage passes
generate. They do not identify which sampled draw belongs to a given tree.

At 22:48:26 CET the host kernel recorded a global OOM with only 128 KiB
free in 8 GiB of swap. It killed PID 2069517, named `main`, with about
5.1 GiB anonymous and 6.2 GiB shared-memory RSS. The six query frames were
already logged before the kill. The kernel recorded a system RAM shortage.
The victim's process name alone does not establish the source of all memory
pressure. Two earlier diagnostic starts failed before world entry and are
excluded from the coverage figures.

After the OOM, the standard D3D12 pipeline statistics query returned pixel
shader invocations alongside occlusion counts. At offsets +5, +12 and part of
+24, 87 foliage draws produced 2,236,596 pixel shader invocations and nine
passing samples. Of these draws, 42 ran pixel shaders and produced zero
passing samples. The 78 control draws had 53,085 invocations and 50,243
passing samples. The counts come from fences completed before the test
stopped, using the same set of draw ordinals for each material. Shader
invocations and raster samples are different units, including helper lanes
in the former on some hardware. All four foliage recipes use the same
front/back stencil comparison (NOT_EQUAL, read mask 0x80), depth GREATER_EQUAL,
full sample mask and alpha-to-coverage disabled. The most visible control
uses stencil ALWAYS. See `fxport-std-first-queries.tsv` and
`fxport-foliage-fixed-state.tsv`. This run used a transient user scope
limited to 12 GiB of RAM and 256 MiB of swap. Peak sampled scope RAM was
11.72 GiB and host MemAvailable stayed at or above 9.33 GiB through collection.
The ordinary launcher accepts the same per-run ceilings through systemd.

```sh
systemd-run --user --scope -p MemoryMax=12G -p MemorySwapMax=256M -- \
  env CGBJ_RT=1 VICTORY_DYNAMIC_SLOTS=524288 ./run-cgbj.sh
```

The diagnostic runner additionally watched host MemAvailable and stopped
after obtaining its readbacks. These short runs do not establish how much
RAM a full play session needs.

Changing only the four foliage stencil comparisons to ALWAYS preserved the
bare outlines at +5. The sampled foliage draws gave 12 passing samples from
2,208,612 pixel shader invocations. Allowing both depth and stencil to pass
also preserved the bare outlines. Across frames +5, +12, +36, +48 and +60,
that test returned 90 passing samples from 9,651,562 foliage pixel shader
invocations. The textured controls returned 151,872 from 156,646 invocations
in the same run. The sample counts differ between launches. For the eight
foliage commands that matched across all three by offset, material, ordinal
and guest argument address, all three runs measured zero passing samples.
Reading the emitted DXIL symbol tables confirmed a texture-sample and
discard intrinsic in each foliage pixel shader. See `fxport-foliage-depth.jpg`
and the `fxport-std-*-queries.tsv` tables.

The decisive control preserved the original depth, stencil, blend and
texture bindings. The experimental compiler forced the predicate of
`dx.op.discard` to false in just the four foliage pixel programs.
Prebuild compiled exactly four replacement PSOs and loaded the other
2,310 from the library. At world offset +5 leaf crowns appeared on both
sides of the house. They were dark, with angular silhouettes and opaque
patches. Normal discard is necessary for a real leaf's cutout. With discard
suppressed, 94 sampled foliage draws produced 485,088 passing samples.
With the normal shaders, 87 draws produced nine. The four commands with
matching guest argument addresses, hashes and ordinals in both runs went
from zero passing samples to 58. Sample totals across runs include
different draws. One start of the discard test failed before the harbor.
The repeat collected the counters and screenshot inside the memory-limited
scope. There was no second global OOM. See `fxport-foliage-discard.jpg`,
`fxport-std-discard-retry-queries.tsv` and `fxport-foliage-discard-common.tsv`.
The bypass belongs only to the diagnostic executable. The CGBJ release at
`4eece1b` retains the ordinary discard behavior.

Offline recompilation traced the emitter call stack for each of those four
programs. Each emitted exactly one discard, at
`cgbj/crates/victory-shader/src/emit/lower.rs:9110`. In `Lower::export`,
the final pixel export discards a lane when its guard is false. All four
used this site. Neither the loop-budget exit nor the missing-export exit
emitted a discard for them. This identifies the decision that removes the
canopy. The experiment does not yet distinguish an incorrect input
texture/alpha value from incorrect EXEC mask translation upstream of the
export. `fxport-foliage-emission-sites.tsv` records the source site for
each shader and the two controls.

### Upstream and memory-path checks

The later fxceless patch for texture readback addresses water data that the
CPU reads. Another commit adds opt-in coalesced scalar loads in the shader
translator and experiments with placement of small buffers. DexNom's
separate changes cover pixel quad sampling, ray buffer views and sparse
textures. Their effects on this AMD save remain unmeasured. [fxceless water
readback](https://github.com/fxceless/victory-fork/pull/999) · [fxceless buffer
and scalar-load changes](https://github.com/fxceless/victory-fork/commit/c36971b)
· [DexNom patch](https://github.com/DexNom/VIctory/commit/921d8d3b43b44f3a6ca4b44095edd3c4baf10a87)

The installed Vulkan driver exposes buffer device addresses and external
host memory with 4,096-byte minimum imported pointer alignment. A direct
libdrm userptr probe accepted an aligned private anonymous mapping and
returned EPERM for the corresponding shared anonymous and shared memfd
mappings on this host. A Vulkan port would still have to map title virtual
addresses and maintain coherence. Both live instrumented game processes
examined held userfaultfd and pagemap descriptors with asynchronous write
protection negotiated by Wine. The number and extent of write-watch queries
are relevant even when that facility is available.

The private run directory is
`~/Documents/Codex/2026-11-25/agent-work/work/amd-frame-profile/`.
The shader cases and driver results are in `fxport-shader-comparison.tsv`,
the four-run medians in `fxport-frame-times.tsv`, the hardware samples in
`fxport-system-samples.csv`, and the matched exterior views and indoor
camera differences in `fxport-fixed-camera.jpg`.

From the porting directory, the regular launcher selects the release build
and native lighting with `CGBJ_RT=1 VICTORY_DYNAMIC_SLOTS=524288 ./run-cgbj.sh`.
The machine-local `victory-work.toml` paths were corrected after the project
directory moved. Its title path resolves to the verified `../title-work`
working copy. The regular launcher uses its own runtime. The measured runs
used the private runtime copies listed in the TSV and the common GPU
changes saved in `fxport-common-gpu.patch`.
The two foliage probes have separate source patches alongside their captures.
The local CGBJ bundle for `4eece1b` and its checksum were verified. The
backup refresh used the existing Zettrox mirror when its remote was
unavailable. Four historical pull refs were unavailable during the refresh.

## 40. Shader inventory against the AMD harbor and house captures

On CGBJ `perf-stack` at `4eece1b`, a native Linux pass ran the same
translator over all 25,985 distinct containers found by the project's
precompiler in the working shader archive, main executable and DLLs. It
accepted 25,703 and rejected 282, matching the earlier Wine release
precompile. The archive itself accounts for 250 of those rejections.

| Containers | Reason |
|---:|---|
| 195 | `GS_VoxelizeGeneric` variants, all standalone geometry shaders. The translator has no entry point for this stage. |
| 40 | A descriptor chosen by control flow. Named examples include 10 `PS_TexturedDeferred`, seven `PS_MirrorReflectionTexturedDeferred`, 14 lit texture variants, six `PS_PuddleDetect`, and three mesh variants for terrain or cascaded shadows. |
| 10 | `PS_PedTexturedCutOutTint_Zero` variants requiring an unsupported hardware scalar input (SGPR). |
| 5 | `CS_TraceShadowRays` shadow-mask variants that read their descriptor as data. |
| 29 | Containers in `amd_fidelityfx_upscaler_xbox.dll` with an `ACCC` section and no `DXIL` section. |
| 3 | Unnamed containers with a `DXIL` section and no supported native program. |

The ported visual and two timed diagnostic runs used the 524,288-slot pool,
prebuilt recipes and native indirect lighting. Joined with the release smoke
log, they recorded 2,462 distinct shader hashes in first-bind events. Every
one translated, and none of the 282 rejects appeared anywhere in these four
logs. All 47 reporting windows logged at least five seconds after world
entry in the three diagnostic runs printed zero drops and zero pipelines
being made. Each of the three also had a transition window with eight to 20
reported draw drops and no logged shader translation or PSO refusal. The
reason for those transition drops was not identified. Pipeline first-bind
events do not identify an individual object or establish correct output.

- **Character and hair.** `PS_DeferredPedTexturedNC` alternatives and
  `PS_HairStrandVBuffer` (`2d0cace65bc05bb2`) bound in the ported build.
  The older large-pool trace records those families executing at house
  offsets +100, +140 and +180. The ten rejected character cutout variants
  above have different hashes and no logged bind. A material G-buffer
  readback at character pixels could separate its output from the
  downstream lighting fault.
- **Indirect lighting.** `CS_ScreenProbes_TraceRays` (`f22a76fcb967ee59`),
  `CS_FinalGather_0_1_1_0` (`2dc364df5d3278b8`) and
  `CS_ScreenProbes_TemporalResolve_ReflectiveCaustics` (`34fb51b8a0312e82`)
  translated and bound. The older large-pool trace recorded the two latter
  passes executing. In the ported visual readback, FinalGather u38/u40 and
  TemporalResolve u33 reported 0.0% nonzero at each of five sampled
  frames, with nonzero data in other resources. Follow the producer data
  into FinalGather. These snapshots do not identify the first bad pass.
- **Window artifacts.** `PS_FlareMask` and `PS_CalculateGhosts` translate.
  The targeted on/off tests in §38 identify the star rays and rainbow
  ring. The white window patches persisted through both tests.
- **Foliage.** All 20 named `PS_PropFoliageDeferredTo_GBuffer` variants
  translate. Five bound in the audited logs. The discard control isolated
  four variants and recovered visible leaf coverage as described in §39.

The three inventories in `shader-audit/` in the private run directory
list the rejected containers, scenes' shader hashes, and named family
coverage. The lighting readbacks include the min/max nonzero percentage
across the five frames for each sampled register. This audit tests which
programs translate and correlates existing AMD captures. It does not
estimate the fraction of game materials rendered correctly or cover
checkpoints outside the captured harbor-to-house sequence.

## 41. First rendering and frame-time checks to run on AMD

The older 524,288-slot trace at world +100 links
`CS_GenerateMaterialMasks` (`f0c91b4d2578f5bf`) UAVs u52/u53 to
`CS_FinalGather_0_1_1_0` and
`CS_ScreenProbes_TemporalResolve_ReflectiveCaustics` SRV t246.
The trace also links FinalGather u40 to TemporalResolve t378. The
resource identities match within that frame. The commands are recorded
on different lists, so the logged order alone does not establish
execution order or visibility of the producer's writes.

In the later ported capture, FinalGather's t246 had the tested flag set
in 9.5–33.2% of texels at frame end. FinalGather u38/u40 and
TemporalResolve u33 each reported 0.0% nonzero. The existing buffer
summary reads title memory, so the zero reported for TraceRays u45
does not establish that the corresponding GPU buffer was zero at the
consumer. The useful follow-up is to sample the same tile at the
material mask producer, the FinalGather consumer and output, and the
TemporalResolve consumer. Record the actual host resource, view,
subresource, execution order and fence on the path. Separately sample
character G-buffer base color and the same pixels after tiled lighting.
For foliage compare the sampled alpha and the final export's EXEC
predicate on the same fragments.

The archive scan found 55 rejected programs in already supported stages.
Cover those by reproducing the shader operation that fails and by
verifying the corrected pipeline on Radeon. Prioritize any confirmed
bind at the tested checkpoint. Geometry stage support and the
FidelityFX containers need separate work when their paths are exercised.

In the heavy harbor windows, median Present-triggered waits were 77.5
and 83.0 ms/frame across the two ported runs. Page-mapping waits were
71.8 and 64.1 ms. The counters can overlap, and the host-reported GPU
timing includes synchronization. Pair the existing D3D12 timestamp
queries with CPU timestamps for title waits, batch submission, page
mapping and fence completion to identify which intervals are actually
on the frame's critical path. Repeat changes at the same save and camera
with the warm cache and large descriptor pool. Compare actual frame
percentiles, fence time and image correctness. Keep diagnostic readbacks
out of the timing runs.

## 42. Coordinated AMD diagnostics

All title runs used one shared launch lock, a private copy of the working
save, and a systemd scope capped at 12 GiB RAM and 256 MiB swap. The
runner monitors free host memory and terminates only the active test
scope. Synthetic and offline tests use separate scratch trees.

The foliage translator probe kept the original pixel discards and
changed only the last `v_cmpx_gt_f32` coverage comparison in each of
the four shaders. Three use offset 0x0cdc and one uses 0x155c.
Four changed pipelines compiled and the same 2,310 others loaded.
The +5 screenshot recovered crowns to the right of the house.
The bare crown on the left recovered only in the earlier run that
disabled all foliage discard. Four groups of sampled commands in
`team-foliage-dither` returned 726,469 passing foliage samples across
152 draws. Two variants accounted for almost the entire total.
The probe localizes one cause of missing coverage to the final
comparison. Correct input coverage and dither values still need
verification. The scope peaked at 11.70 GiB and the minimum available
host memory observed was 5.66 GiB.

The translator candidate accepts all 40 shaders previously rejected
because a sampler descriptor was selected by control flow. It emits a
sample with each immediate sampler at the original sample site and
selects the numeric result by predecessor. This preserves the original
derivative context. Radeon created all 40 standalone test PSOs (37 pixel,
three mesh). The synthetic test exercises both paths. The native suite
reported 182 passed and one ignored. In the original 302-container test
set, 52 previously accepted outputs stayed byte-identical, 210
rejections stayed identical, and the 40 now translate. These 40 were
absent from the audited harbor and house logs, so the fixture validates
pipeline creation and does not measure scene rendering. All ten
rejected character variants use primitive-ordered pixel shading
(POPS), including a collision wave ID from s18 and a poll of the
exiting wave ID. That requires equivalent host fragment ordering.

The `team-gi-original` run completed the +100 lighting snapshots before
the cgroup limit killed its title process. The original material mask
producer and both consumers ran within fence 3992, with the producer
in list 7 and the consumers in list 15. Both FinalGather variants
returned zero in all sampled u38 and u40 tiles. TemporalResolve saw
those same zero bits via t378, with the same resource identity. The
t246 SRV requested DXGI format 47 on a resource of format 53. The
descriptor log reports the incompatible format family. The probe
could not copy a stencil subresource from that 16-bit depth backing.

With the candidate enabled in `team-gi-stencil`, the view maps the native
stencil component to the physical R8_UINT twin. At world +100, the
GenerateMaterialMasks u53 producer and all t246 readers used the same
host resource. The three ordered tile checksums matched from producer
to each consumer across fence 4106. The TemporalResolve u33 counts of
nonzero pixels went from (0, 0, 0) to (7, 205, 193), each out of 256.
Both FinalGather u38/u40 outputs and TemporalResolve t378 were still
zero in the sampled tiles. Two of the sampled R8 tiles were uniformly
0x01 and thus lacked the bit 2 flag used by FinalGather. A targeted
sample with bit 2 set is needed before interpreting the zero output.
Another resolve output, u32, had data in the third baseline tile and
was zero in the candidate capture. The captures used the same save and
world offset in separate starts. The snapshots bracket temporary copy
barriers and do not establish the effect on the final image.

The patches and per-run summaries are in `team-diagnostics/` in the
private AMD evidence directory. The candidate remains a scratch build.

The first timing launch failed before world entry with title exception
+0x35a3ce4. The warmed retry recorded 136 Presents and 2,957 matched
host fences, with no image readbacks. It used the same 524,288 dynamic
slots and 2,314 cached pipeline recipes. Across harbor offsets +5 to
+90, the 86 actual frame intervals had a median of 378.34 ms and
p95 of 415.10 ms. The union of page-mapping-triggered flush time had a
median of 76.57 ms and p95 of 110.48 ms. The mapping operation itself
took a median of 0.07 ms and p95 of 1.81 ms. Present-triggered flush
time had a median of 79.62 ms. DXGI Present itself took 0.01 ms.
In the 27 captured house intervals starting at +110, median/p95 were
282.70/294.38 ms.

These are CPU-observed intervals aligned to Present and linked to the
host fences. Their categories can overlap. The run had no calibrated
GPU timestamp envelopes. The large mapping cost was in draining the
preceding work, before the small mapping operation. Queue optimization
can now start at `queue::in_order` and `batch::flush`. Establish
which prior ranges can alias each mapping and where a full drain is
required before narrowing the wait. The scoped limit killed the title
after capture of the full harbor range and part of the house range.
No global OOM occurred and no title instance was left running.

## 43. AMD fork audit, POPS port and ordered light readback

With the same instrumented `perf-stack` executable and warm cache as the
harbor baseline in §42, changing `VICTORY_PIPELINE` from 2 to 3 gave 379.61
ms/frame median and 412.86 ms p95 at world frames +5 through +90, against
378.34 and 415.10 ms. Both runs used 524,288 dynamic descriptors. The outer
`Present` call took 83.51 versus 81.62 ms median. Draining for mapping took
78.66 versus 76.57 ms, and the mapping operation after that drain took
0.08 versus 0.07 ms. The intervals overlap. Level 2 remains the reference.
Before changing the queue, assign physical page ranges and aliases to
outstanding batches so a read/write hazard can be distinguished from a
conservative drain. See `team-diagnostics/next-p3-harbor.txt` in the private
result directory.

The ordered stencil probe on the RX 7900 XTX, using the existing CGBJ
stencil SRV test build with 2,319 warm pipeline recipes and both RT and GI
stand-ins off, reached harbor frame 904 (world +100). It sampled 77 tiles
of 16 × 16 pixels. FinalGather's t246 had bit 2 set in 3,089 of the 19,712
sampled pixels. Both dispatched FinalGather variants had zero nonzero
samples in u38 and u40 before and after their recorded commands. The
stencil mask alone cannot explain those zero outputs. TemporalResolve
subsequently had 16,495 nonzero pixels in u33 and zero in u32 across the
same sample locations. The pass read zero in t378 at those locations.
These are ordered, sparse readbacks from one frame. The indirect dispatch
group counts and the remaining shader inputs needed direct readback.
See `team-diagnostics/next-gi-active.tsv`.

The public heads of nine upstream repositories were checked on 2026-11-25.
Five default heads advanced relative to the local mirrors. The AMD-specific
projects Belobug (`4f62c2a`), MrSkelee (`edb370c`) and DexNom
(`2e01ab4`) matched their local mirrored default heads. Belobug and
MrSkelee expose only main. DexNom also exposes the older
`backup/main-before-intro-2026-11-25` and `snapshot/intro-tested` branches.

- [Belobug `4f62c2a`](https://github.com/Belobug/VIctory-Belobug-AMD/commit/4f62c2a5e27e7beebf3e8d68f77898e43dc7b77c)
  uses a sky-driven ambient fallback. Earlier commits add depth-based shading
  (`cd9f78c`) and explain why the sky dome is sampled instead of a brighter
  intermediate (`e48eb83`). Belobug separately implemented depth UAV twins
  (`e52106f`) and skips a screen-probe ray activation pass when rays are
  unavailable (`6986a57`). The sky-driven fallback is an option for a separate
  visual comparison. CGBJ already measured that a constant 0.5 fill could
  overexpose the house in §38.
- [MrSkelee `edb370c`](https://github.com/MrSkelee/VIctory-MrSkele-AMD/commit/edb370c34de799b7412c7dcc341a1e4737e2fbc0)
  groups parallel texture refresh, upload pooling, ambient shading and UI.
  The fork reports performance on Windows.
- [DexNom `921d8d3`](https://github.com/DexNom/VIctory/commit/921d8d3b43b44f3a6ca4b44095edd3c4baf10a87)
  carries pixel helper-lane quad operations, a cube LOD correction and sparse
  texture recovery. Marvisak separately changed inactive-lane DPP16, DPP8
  and `ds_swizzle_b32` semantics in
  [`b51011aa`](https://github.com/Marvisak/VIctory/commit/b51011aa6502ef349b63f71e7cf27c6f87a14200).
  These are candidates for testing the remaining foliage coverage gap.
- [Scarlight #134](https://github.com/pablogonz12/Scarlight/pull/134) uploads
  the interleaved stencil plane for compatible typed host depth-stencil
  textures. It leaves the R16_TYPELESS/twin route to a separate fix.
- The new public CGBJ commit (`4327b73`) changes crash diagnostics.
  triippywhale's new range-queue and texture diagnostics do not provide a
  benchmark for the mapping wait measured here.

The CGBJ scratch port of
[fxceless #215](https://github.com/fxceless/victory-fork/pull/215),
using [VItoriaBr's port](https://github.com/uiliannn/VItoriaBr/commit/213beb9c661f1e80a0c50e223e39c1b67d869f2e)
as its source, lowers console primitive-ordered pixel sections to
rasterizer-ordered UAVs. All ten `PS_PedTexturedCutOutTint_Zero` rejects
translated and created a D3D12 PSO with a synthetic companion stage on
RADV 26.2.6 under Proton Experimental. The shader suite reported 182
passed and one ignored. A differential 302-program replay found 52
previously accepted shaders byte-identical and 240 rejects with identical
errors. The ten changes were exactly the ten targeted failures. The
separate test covered translation and pipeline creation. In-game output
and frametime remain unverified with these shaders active. The candidate is
`team-diagnostics/next-pops-cgbj.patch` and applies to CGBJ `4eece1b`.

The sources behind those forks were checked separately. MrSkelee's
texture changes came from rorodev6. Commit
[`6f07e7b`](https://github.com/rorodev6/victory/commit/6f07e7b5752c7f5ed8e3e259574d79352651e765)
holds title memory stable once per group of texture refreshes. Their
Windows cutscene reported 60–230 ms in the texture step, down from
400–1,800 ms before batching. Commit
[`0781f61`](https://github.com/rorodev6/victory/commit/0781f61a67e1b44910362f4e174f01747d3ecc59)
pools staging allocations. CGBJ's early-prep path already distributes
texture jobs over a pool. Measure how often those jobs wait for the
same remapping epoch before adapting the shared memory guard. ZSlayerHQ's
`next` has an RTX-tested wall fix at `99f8834`. Its recent updates after
that concern CI and documentation. XCL's separate backend remains a
comparison point for shader and command execution.

### FinalGather dispatch bounds

A second probe at world +100 (frame 903) had 6,455 stencil-qualified
pixels of 19,712. Both FinalGather variants again left u38 and u40 zero
across the samples. That trace recorded the two dispatch calls but missed
their GPU argument records because the VRAM diagnostic samples one in
16 finished lists. A scratch diagnostic now reads the lists selected for
tracing. Two follow-up launches had title access violations before
reaching the sample, and a third reached the world and was stopped by
the 12 GiB scope OOM limit. No global OOM was observed in those runs.

Older AMD traces have the missing GPU records. In `visual-real`, frame
791 (world +24, both stand-ins off), the clamp flagged FinalGather
`2dc364df5d3278b8` and passed `[65535, 1, 1]` to the host. The title-side
snapshot was `[93704, 1, 1]`; the two snapshots are taken at different
points. The other FinalGather variant received `[21576, 1, 1]` with no
cut flag. In `visual-ambient`, frame 792 (world +100), the same shader was
flagged and capped at 65,535. The other variant ran with `[43648, 1, 1]`.
The clamp therefore discards work beyond the per-axis host limit, and
both passes had positive GPU group counts in those earlier traces.
This establishes a dispatch coverage defect. How much it contributes
to the empty light buffers needs a matched readback of the same frame
with the extra work preserved. The next implementation can reshape an
oversize 1-D dispatch into 2-D groups for these shaders, reconstruct the
original group ID and mask the padded tail. Keep the existing budget.
See `team-diagnostics/visual-real-finalgather.tsv` and
`team-diagnostics/visual-ambient-finalgather.tsv`.

## 44. Wide FinalGather work on RX 7900 XTX (2026-11-26 local time)

[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)
was audited at `0a43566`. Its
[findings](https://github.com/avatarmanz/VIctory-7900XTX/blob/0a43566532c57b34a5397d6fdc91e419de32e8e9/docs/findings.md)
report 46–53 ms/frame on the first boat mission on Windows 11 with a
12900K, 64 GB RAM and the same Radeon GPU. They measured 48 ms on the
submission-preparation thread in a 55 ms boat frame. The README reports
40–46 GB of private memory, remaining outdoor flicker, and dark interiors
without `-RayTracing`. The Linux harbor measurements and these Windows
boat measurements represent different scenes and settings.

Their [wide-X change](https://github.com/avatarmanz/VIctory-7900XTX/commit/84eae352a2daa0539f3e1e8e8a719bb03bac1ed8)
independently identified the same two FinalGather shaders and the
65,535-group cut. Their captured title records split 115,200 tiles into
67,440 and 47,760 groups. Their Windows AMD test executes X directly
beyond 65,535. Local Vulkan reports 4,294,967,295 groups along X and
65,535 along Y and Z for RADV on NAVI31. The CGBJ experiment applies the
fork's approach only to these two FinalGather programs on an AMD
adapter, behind `VICTORY_FINAL_GATHER_LINEAR=1`. It preserves the existing
per-dispatch and cumulative budgets.

The independent tiled-dispatch prototype was built before this fork
was identified. It sends a root constant with the original count,
reconstructs the 1-D group index in DXIL, and returns for the padded tail.
The native shader suite passed 182 tests and ignored one. On RADV at
world +24 (frame 915), the host argument record was
`[93760, 1024, 92, 1]` (original count, then dispatch dimensions). The
first FinalGather pass changed u38 from 80 to 94 nonzero samples of
19,712, with 16 newly nonzero samples in stencil-qualified pixels. u40
remained zero. TemporalResolve u32 reached 176 nonzero samples. The
Tiled +24 and baseline +24 images both show a dark facade and bare
trees, with facade median luma 31 and 30 respectively. The tiled
run was stopped by the isolated 12 GiB memory limit after capturing
world +24.

For a performance port, test
[the list-local view reuse](https://github.com/avatarmanz/VIctory-7900XTX/commit/4e77a8795b99566ac347dca66c2df31c3652bf64)
in isolation. Their trace found a slow command list with 2,500 indirect
dispatches whose tables mostly differ in one constant-buffer binding.
CGBJ already caches heap bindings, tracks preparation hazards and
parallelizes part of submission preparation. Reuse must respect
descriptor edits and lifetimes. The fork's more invasive
[thin list recorder](https://github.com/avatarmanz/VIctory-7900XTX/commit/3a99ee610cc706d0e7a28dde6a4f5d82303ce70f)
is a separate candidate. The
[texture readback](https://github.com/avatarmanz/VIctory-7900XTX/commit/23925a4aa29a6bcb13b29959ec3eddec481f5e00)
addresses incorrect boat physics, and the
[RT thread-count interpretation](https://github.com/avatarmanz/VIctory-7900XTX/commit/810efe5829e80d998b9b606a610a1df346c35045)
points to a separate test for the still missing indirect light.

The native wide-X shader suite passed 182 tests with one ignored. The
D3D12 indirect command was also tested under Proton Experimental on
RADV 26.2.6. A synthetic compute shader counted writes for every group
ID. All IDs were visited exactly once at 65,535, 65,536, 93,760 and
115,200 groups, with zero writes beyond each count. The test used one
ordinary indirect dispatch record and no root-constant changes.

In the first in-world wide-X sample (frame 858, world +24), RADV
received `[93600, 1, 1]` and `[21600, 1, 1]`, both with zero clamp
cuts or budget empties. The diagnostic build enabled the experiment
only after world entry after two earlier runs hit title exceptions
during startup. The earliest comparison baseline recorded
`[65535, 1, 1]` with its cut flag set. Both shots show nearly the same
dark facade and leafless crowns. Ordered readbacks still had zero
samples in FinalGather u40. Its u38 had 46 nonzero samples of 19,712
after both passes in the wide-X run. The independent tiled version
and the wide-X version cover the full work and both leave the
larger lighting defect visible.

Two CGBJ traces also show the scale of repeated bindings. At harbor
world +24, 91,526 of 96,997 logged compute SRV/UAV references repeated
a view on the same command list. At world +100 in a separate run the
count was 62,763 of 65,986. These are counts of logged view descriptions,
including direct views. They are an estimate of repetition, with the
cost and reuse eligibility of real descriptor creation still to be
measured. CGBJ's current cache keys complete tables by title address and
contents. Rebuilding a table after one constant-buffer entry changes
can recreate its otherwise identical views. That is the narrow port
to evaluate first from the 7900 XTX fork.

The second baseline and the in-world wide-X run used the same diagnostic
executable (SHA-256 `550b4ec11cae12e07108f957d8cfbf018a10e6fdebfb1bfd792e06b25b134036`),
the same cache source and 524,288 dynamic descriptors, with the
stencil SRV correction enabled. The +24 baseline (frame 687) again
reported a hard cut to `[65535, 1, 1]`. It had zero nonzero samples in
u40, as did the wide-X sample. Harbor reporting windows with at least
10,000 draws and zero missing-pipeline drops had medians 396.1 ms
(two windows, clamp on) and 391.4 ms (three windows, wide X). These
short samples did not establish a frame-time improvement. The matching
binary's baseline shows substantially more flare than the earlier
baseline, so pixel differences between the two runs cannot establish
the lighting patch's effect on exposure. The earlier darker baseline
and both test variants also appear in the contact sheet, with bare
crowns visible across all variants.

Private artifacts in `team-diagnostics` include
`finalgather-linear-radeon.patch` (opt-in candidate against CGBJ
`4eece1b`, with the targeted GPU regression), the two game traces,
`finalgather-run-timing.tsv`, `finalgather-harbor-comparison.jpg`, and
`linear-radeon-test.log`. The release worktree has no new shader or
runtime edits from this test. The original title data and the shader
cache were preserved. The working title copy passed `check-data.py`.

## 45. Running the 7900 XTX fork directly under Proton (2026-11-26 local time)

The standalone [7900 XTX fork](https://github.com/avatarmanz/VIctory-7900XTX)
at `0a43566532c57b34a5397d6fdc91e419de32e8e9` cross-built under Linux
with Cargo stable, MinGW and its locked dependencies. The Windows GNU
diagnostic executable ran directly under Proton Experimental and RADV
26.2.6 on the RX 7900 XTX. The CGBJ shell launcher supplied the isolated
Wine prefix and storage paths. The executable and graphics backend were
the fork's source. Both tests ran against `../title-work` with separate
prefix, save, and cache copies, and a 14 GiB memory scope. The fork's
published launch settings were used, including its 0.8 ambient fallback
and the default with ray tracing disabled.

The fork produced menu and loading images at about 30 fps. In the first
world start its two indirect argument clamp shaders failed in Wine:
the bundled HLSL compiler rejected the byte-address-buffer
`InterlockedOr` call. At world +7 to +13 seconds the game reported
11,788–12,510 dropped indirect operations per frame. The captured
world frames +2, +6 and +8 were black.

The second start used Microsoft's HLSL compiler 10.0.28000.2705 from
the Windows SDK, installed only in that copied Wine prefix. The
64-bit DLL matched SHA-256
`3e56c1a0867aa04cbca948b3664fcac5d51275865cd044a45a3fd6ce636ee914`
and Wine logged it as native. Both clamp compilation warnings
disappeared. World reporting windows ending 6.5–21.3 seconds after
entry reported 2,048–4,864 dropped indirect operations per frame,
681–751 pending pipelines and 0.2–0.7 fps. The same three world
offsets were still black. The tests reached the renderer's 500-draw
world marker and ended at the requested world duration. The second
test copied the first run's cache. These early, partly cold runs
establish the working launch and the compiler fix, with world
rendering and comparable in-world performance still unverified.

The fork keeps title pipeline recipes only with `VICTORY_PIPELINE_BOOK=1`.
Both pilot runs used its default and left no recorded pipeline book.
Its warm tool will need a run with book recording enabled before a
controlled warm world comparison. CGBJ's recipe and shader cache formats
differ, so use independent caches. CGBJ remains the working comparative
baseline for Linux. The fork's published 19–22 fps boat result uses Windows
and a different scene and should remain a separate comparison.

## 46. Dynamic descriptor materialization on RADV

The descriptor instrumentation in `goal-desc-measure` measured
104,050–106,595 placed descriptors per harbor frame. Realizing these views
took 52.05–52.78 ms of aggregate CPU time per frame. The timer includes
resource lookup, descriptor creation, and backing-resource retention.

The 7900 XTX fork's `4e77a87` suggested testing descriptor copies. The CGBJ
prototype creates CPU-only templates per command list and looks up each
template by the current host resource and the normalized D3D12 view. Two
sequential starts (`goal-desc-copy-0`, `goal-desc-copy-1`) used the same
diagnostic executable, the same initial cache, RADV on RX 7900 XTX, the
524,288-entry descriptor pool, and the linear FinalGather and stencil
experiments. Neither start hit the memory limit. Each row below is the
median of three harbor reporting windows beginning after entry and
ending 7–20 seconds into the world, with at least 10,000 draws per frame
and zero pending or missing host pipelines in the reported windows.

| Template switch | Frame time | Placed views | Placed CPU time | Templates created / copied |
| --- | ---: | ---: | ---: | ---: |
| Off | 375.2 ms | 107,654 | 61.21 ms | 0 / 0 |
| On | 414.3 ms | 108,849 | 63.77 ms | 37,439 / 78,965 |

The ordinary SRV and UAV paths were faster with the switch on. The net
descriptor CPU time was nearly level, and wall time in the measured run
was 39.1 ms higher. There was no visible improvement at harbor frame +24.
The prototype stays outside the main build. Reducing table invalidation
before descriptor materialization is a separate experiment. Raw windows,
the frame comparison, and the experimental patch are saved in the
private `amd-frame-profile/team-diagnostics` directory.

The focused test for backing-resource identity, buffer offset, and cache
reset passed under Proton on RX 7900 XTX. An existing `clamped` test call
needed the `wide_x` argument to build the test binary. This test-only
adjustment was made in the scratch tree.

## 47. The 7900 XTX fork's pipeline warmup under Proton

The stock fork under Proton Experimental made a book of 83,555 title
pipeline recipes. It also logged `no host pipeline library` and never
wrote `early-pipelines.txt`. The early-list writer returned before both
files if `ID3D12PipelineLibrary1` creation failed. The driver's own cache
still logged disk writes. The source-only test patch moves the early/slow
list persistence outside the library dependency for that case and
preserves the ordinary save path when a host library is available.

The first patched run wrote the startup list and was stopped by the host
RAM reserve before transition. A follow-up with `VICTORY_PIPELINE_WORKERS=4`
entered the world and saved 1,735 unique pipeline keys. All 1,735 had
recipes in the book. It rendered at 0.9–3.6 fps in the first six
full-world reporting windows. Four normal pipeline workers reported stack
overflows and 860–987 pipelines were reported pending in those windows.
Six back-buffer captures, world +2 through +64, had zero pixels with
8-bit luma of at least 16. The reserve stopped the run about 48 s after
the world marker. The allocation limit recorded zero OOM kills.

Ghidra confirmed that the title calls the device vtable at offsets
`0x1c8` and `0x218` as `(device, descriptor, IID, out)`, using the standard
`ID3D12PipelineState` IID in both cases. At `+0x24800d5`, the caller first
looks up a cached record, passes a descriptor starting 0x18 bytes into
that record, and returns the object only after a successful result. The
`+0x2477578` caller walks a sequence of records and takes a different
success branch to store the result. The source under test logs and refuses
both slots. Those methods are independent candidates for missing pipelines.
The call-site evidence establishes their ABI and use of the results.
CGBJ also reports both slots as unimplemented at the same return
addresses. In the patched fork's runtime observations, each of the first
two descriptors sampled for either slot began with an object whose
vtable symbol in that exact executable was `victory_gpu::root::FUNCTIONS`.
This identifies the first member as a root signature. The other fields
need a mapping before either entry point can build a real pipeline.

`warm --used --jobs 4` processed the 1,735 selected recipes in 404 s:
1,729 created host pipelines, and six compute pipelines hit the 180 s
child watchdog. The shader IDs were `32ed072b9f845048`,
`a1b20bc47e3c9597`, `17829ac71aab4e8d`, `e8c962358d862e3f`,
`afeea48bbb9913ad`, and `559d9f0184e15fe3`. The CGBJ
`goal-desc-copy-0` diagnostic on the same Radeon logged both host PSO
creation and an in-world bind for each hash. This narrows the difference
to code or state produced by the two compatibility layers on the same
driver stack. It does not prove that each bound shader gave correct output.
The warm process logged a sharing conflict when more than one child
attempted to open vkd3d-proton's `.cache.write` file. Its 1,729 successful
creations therefore do not establish full disk persistence.

The warmed replay used the same executable and four pipeline workers. It
loaded the recorded list of 1,735 keys and moved four worker stack
overflows to startup. The record and replay are a before/after workflow
comparison, with the early list and caches both changed. From world +5 to
+36.1 s, median reporting windows fell from 375.5 ms (six windows) to
263.4 ms (five windows). Median draws fell from 11,296 to 307 and median
indirect drops grew from 1,958 to 6,066 per frame. Median pending
pipelines grew from 910 to 1,727. The replay showed HUD only at world
+24, +40 and +64. The differences in rendered work preclude claiming a
performance improvement. The replay completed the requested 55 s in the
world with zero scoped OOM kills.

CGBJ commit `e59aa23d4750003c6be32a54ed2f9685277f1625` identifies the
same six ray/shadow shaders among a group of DXIL programs that entered
unbounded recursion in dxil-spirv's structurizer. It redirects multiple
back edges through one latch and joins their phi operands there. The
7900 XTX fork's emitter left the edges separate. This gives a targeted
translator port to validate against the timed-out pipelines.

The isolated 7900 XTX translator port joins each affected loop's counter,
scalar and mask phi operands at the latch. The regression constructs a
compute loop with two back edges and checks that the lowered graph has
one. The native shader suite was 203 passed, zero failed and two ignored.
With an independent empty translation and vkd3d cache, a direct replay of
the six previously timed-out pipeline recipes created all six D3D12
objects on RX 7900 XTX under Proton Experimental and RADV 26.2.6. Calls
took 64.31–496.28 ms. The result covers compilation of these six PSOs.

The matched `goal-fork7900-latch-world` title run used the same warm
cache source, launch options and four pipeline workers as the replay.
It started all six timed-out compute PSOs, recorded zero worker stack
overflows, and reported zero pending pipelines in seven of eight
post-transition windows. The one other window reported seven. The first
harbor shot already shows the facade, foliage and water. All inspected
shots through world +88 retained the rendered harbor. In the two earlier
runs the scene was black, with the warm replay showing HUD only in the
later captures. Comparing CGBJ `goal-desc-copy-0` with this patched fork
at world +24 shows different facade lighting and visible tree crowns.
Both runs used the working save, and their frame markers count different
minimum draw thresholds, so the shots compare the scene visually.
The fork used its ambient fallback at 0.8 with its RT builder disabled.

The first full reporting window was 1,645.1 ms (0.6 fps). The seven
following windows had median 460.7 ms with a range of 252.4–703.9 ms
(1.4–4.0 fps). Their median time attributed to preparing copies was
219.0 ms, the largest named component. The scene still logged 390–1,328
dropped indirect operations per frame in those seven windows. The RAM
reserve stopped the title about 50 s after world entry. Scoped peak RAM
was 10.94 GiB with zero OOM kills. Interior lighting and sustained play
remain unmeasured in this configuration.

The source-only patches, Radeon fixture summary, native shader test
output, before/after timing and luminance TSVs, and screenshots are
in private `team-diagnostics`. The compatibility changes are in the
scratch fork. No title data or pipeline books are included with the
patches. The working title copy passed `check-data.py`.

### Saved checkpoint

`checkpoints/7900xtx-harbor-rendered-20261126` records the two source
patches and replay instructions at commit `affd21d`, tagged
`checkpoint-7900xtx-harbor-rendered-20261126`. The independent Git bundle
contains those source patches and instructions. Both patches applied
cleanly to the recorded upstream revision. The private checkpoint
directory contains the exact measured executable, launcher and evidence.
Its executable SHA-256 matches `goal-fork7900-latch-world/binary.sha256`.

## 48. Harbor memory-window test and split compute descriptors

Two runs from the tested loop-fix binary and the same `goal-fork7900-latch-world`
cache seed compared the default grid (`VICTORY_WINDOW_KB=4096`, tails off)
with `VICTORY_WINDOW_KB=1024` and `VICTORY_WINDOW_TAILS=1`. The same 14 GiB
scope and host RAM reserve applied. At the first world report the standard
grid had 704 windows outside known buffers, covering 3,192 MiB of host
allocations. The smaller grid with tails had 2,651 windows consuming
4,353 MiB at the analogous report. This combination increased memory
pressure. The host reserve stopped it roughly five seconds into the world,
after capture +2. The standard grid reached +64 and recorded five windows
from world +10 to +36 s with 468.2 ms median frame time and 228.7 ms median
copy preparation. Both runs recorded zero scoped OOM kills. Scene captures
from the standard grid reproduced the lit harbor and foliage. These runs
do not isolate the smaller grid from the tail-window switch.

A diagnostic for the `0x218` device entry point assembled a temporary
in-memory FSPX directory from three descriptor pointer/length pairs at
offsets `0x08`, `0x28`, and `0x38`. They identify the compute packet, code,
and program metadata. This used the leading packet size of `0x1c`, the
`0x403` metadata signature, and the program's declared dword count. Ten
of 13 sampled calls parsed with the existing compute FSPX parser, including
8x8x1, 32x2x1, 16x16x1 and 64x1x1 group shapes. Three rejected an unknown
root type. The diagnostic kept the runtime return value and retained only
summary metadata. Recreating the compute PSOs requires decoding the
remaining root records and verifying the reconstructed shader bindings.

`goal-fork-copy-watch-ab` alternated a guarded-range prefetch bypass in
six-second phases on the 4 MiB grid. The counter for avoided queries was
zero in all 114 split reports, including the world phases with the switch
on. The existing `Shadow::asked_ahead` path already excludes guarded
copies when light refresh is enabled, as it was here. The short frame-time
differences have no demonstrated connection to this switch. The source
experiment remains outside the checkpoint. Instrumenting the fallback
write-watch queries before a range is classified is a more direct way to
explain the refusals that remain.

## 49. Fresh upstream review after the Radeon checkpoint

Read the public repository heads on 2026-11-26 UTC. `fxceless/victory-fork`
and its `victory-title` alias both pointed to
`dc4c0fcd71bdb1233cca5b55befb77e4f8c2a344`. Its README reports
9–12 fps on a Windows RTX 4070 in rooms and light outdoor scenes, 5–6 fps
in heavy outdoor scenes, play past the first boat mission, and eventual
device loss. Indirect lighting in shadow is still listed as missing.

- `d03859a` fixes invalidation of cached holes for a fixed commit when
  Windows rounds the returned pointer down across the caller's requested
  range. The 7900 XTX source still passes the rounded pointer and original
  length to `title_memory_allocated`. The port should adapt the notification
  semantics and exercise a two-byte commit straddling a 64 KiB boundary.
- `84c5dc0` indexes three passes over submission steps. The 7900 XTX source
  already indexes `stale_under_flights` and deduplicates `declared_writes`.
  The independent linear membership scans in `card_before` still exist.
  `de4146a` avoids kernel calls for tiny upload-buffer fills and depends on
  fxceless's `plaincopy` probing, absent as a module in the 7900 XTX tree.
  The published estimates for these changes are small compared to the
  219 ms measured copy-preparation median on our harbor checkpoint.
- `94ed363` is a port from the 7900 XTX fork of the protected-block fast
  path, already on by default in our fork. The 64 KiB CBV limit in `2ba6e1f`
  is also already enforced by the checkpoint's `constant_bytes`. Treat
  both as equivalents when comparing the current code.
- `iNiKKo/victory-universal` at `3c34f8f` measured the whole-section Wine
  view in `07b1082` on RX 9070 XT. It removed roughly 44,000 temporary
  unmaps per run and moved the three-run range from 0.7–0.8 to 1.1–1.3 fps.
  The 7900 XTX fork already attempts a whole-section view with
  `VICTORY_SECTION_WINDOW` enabled and falls back to per-range views.
  Its Wine section commit and fallback behavior must be measured before
  transferring the reported gain. CGBJ separately uses lazy guest mappings.
- `mrowsnezh/Linux-AMD-Nv` at `73ef8ab` documents the known native
  `d3dcompiler_47` requirement and Wine view-tree experiment. The former is
  already in the checkpoint. Its texture recheck after upload (`a803c87`)
  is relevant to visible skin/hair after streaming. Compare its requeue
  behavior with the 7900 XTX descriptor `VICTORY_SHOW_AFTER` path before
  porting it. The 7900 XTX public head was still `0a43566` and the
  public CGBJ head was still `4327b73` in this review.

## 50. Radeon split-compute experiment and a fresh AMD performance source

Public refs still point to the heads in section 49. The additional
`myokex/victory-amd` default branch is `vi-fx3` at `1544bd5`.
Its `c3723f3` records one six-minute Windows run per configuration on
an RX 7800 XT. On the terrace the median was 202 ms before
`VICTORY_TABLE_BUFFERS` and 179 ms with it. Moving runs combined texture
heaps, staging blocks and later build changes, with different routes.
The descriptor shortcut suppresses the global mirror for buffer and
constant views recently used as draw-table entries. The code documents
that a shader using a computed index can read the old view in that
interval. Preserve the heap mirror for such reads. Texture staging
suballocation can be measured separately from the descriptor shortcut.
The 7900 XTX branch already enables the four water copy-back kinds that
`myokex` added in `1544bd5` and already rechecks bindless textures after
upload, as in `mrowsnezh` commit `a803c87`.

In a scratch build of the 7900 XTX checkpoint, ported the XMem fixed
commit notification semantics from fxceless `d03859a`. A two-byte
commit crossing a 64 KiB boundary reports the requested interval.
A commit at a caller-unspecified address reports the address returned
by Windows. Both targeted tests passed under Proton Experimental.
Added diagnostics to count selection of the DirectStorage whole-section
view. In `goal-fork-split-off` the last sample was 40,960 calls, all
using that view, with zero range, map or commit failures. In
`goal-fork-split-on` the last sample was 36,864 calls, all using the view.
This covers the route targeted by the Wine optimization in iNiKKo
`07b1082`. These two game runs both contain the XMem change, so they do
not measure its FPS effect.

The experimental device method at slot `0x218` reconstructs a normal
compute container from the split descriptor and uses the existing
translator. A synthetic padded shader created a host PSO on the RX
7900 XTX with RADV 26.2.6. The game A/B ran the same diagnostic executable
with the switch off/on, Proton Experimental and vkd3d-proton
`2576c0b4adfae4a`, the same warm cache seed
`goal-fork7900-latch-world`, four pipeline workers, the 4 MiB title-memory
window, the same descriptor heap settings, ambient 0.8 and the RT builder
off. Both runs ended at the host RAM reserve with zero observed OOM
kills. The off run lasted about 26 seconds after the world marker,
and the on run about 14 seconds.

With split compute disabled, the harbor had foliage and visible daylight
through captured world frame +40. With it enabled, all three matched
frames (+2, +6 and +8) were visually black. The first heavy window took
2,543 ms/frame at 25,741 draws/frame with split compute off and
3,118 ms/frame at 25,707 draws/frame with it on. Later windows were short,
different in coverage and rendered different images. The off run had a
536 ms median across three windows, with 245 ms median preparing
title-memory copies. The on run had one later sample at 434 ms, with
331 ms preparing those copies.

The reconstructed compute containers added 1,211 distinct hashes to
the book. In the game log, 452 of those were requested by a list,
139 had a successfully created or loaded host PSO, and 312 had an
explicit translation failure. The largest groups were 116 unsupported
addresses not derived from a root parameter, 53 unsupported partial
image stores, and 25 unsupported addresses with two computed terms.
The device also rejected at least 1,024 descriptor calls during decode.
Sampled rejects contain resource root type 0 (inline constants), which
the current FSPX decoder does not accept. Successful guest creation,
successful host creation and execution of a dispatch are different
events. The unfiltered experiment is unsuitable as a default.

A third run, `goal-fork-split-verified`, used the same cache and launcher
settings with split creation limited to the 139 hashes that the second run
both requested and made on the host. It added those 139 to the book,
made 138 during the run, and logged zero compute translation failures.
The harbor still did not render. World +8 had an 8-bit luminance range of
51–55, compared with 11–255 in the control and 0–2 in the unfiltered
experiment. The flat gray output persisted through captured world +64.
This rules out the rejected translations as a sufficient explanation for
the visual regression. The split route uses the FSPX metadata to
reconstruct host bindings. The descriptor's root signature and the
first affected dispatch's output still need to be traced.

Across windows from 10 to 25 seconds after the world marker, the filtered
run had a median of 404 ms/frame across three windows, with a median
227 ms preparing title-memory copies. These intervals had different
rendering output and draw counts, so the difference from the control
is not an FPS improvement claim. All three runs used host heaps of
2,097,152 views. The filtered run reached the RAM reserve after about
40 seconds in the world, again with zero observed OOM kills. Both split
compute variants remain scratch experiments.

The capture comparison, luminosity and frame-time tables, test logs,
and source-only candidate patches are in the private
`amd-frame-profile/team-diagnostics/split-compute-20261126` directory.

## 51. Harbor dispatch tracing and startup frame pacing

A run with only the 137 previously accepted split-compute shaders from
the world group removed the two shaders first bound at intro frame 184.
The harbor still came out flat gray, including world +6 and +8. The trace
recorded 132 distinct host-list dispatches across the sampled hashes
(121 direct and 11 indirect), starting at frame 2192, one before the
world marker. The 661 sampled guest references matched their declared
classes. This establishes guest descriptor presence and recording, not
the contents of the resources consumed by a GPU dispatch.

A separate probe followed pixel shader `a78d088fa55aa495` and checked
the actual dynamic heap slot passed in the shader's user data. In the
control and filtered split runs, that slot retained the host resource
corresponding to the guest view in all sampled draws. The sampled
4x4 grid read at Present was zero in both variants, even when the
control screenshot was lit. That sample cannot locate the first
incorrect GPU output. An in-list readback at the relevant operation
would be needed before interpreting those pixels as the shader's input.
The broad timeline instrumentation run crashed in `dbghelp.dll`
before the world. The passive Present timestamps avoided that failure.

For FPS, the whole card-copy preparation phase in the three prior
control windows took 5.1, 6.8 and 10.9 ms/frame. Deferred title-memory
refreshes took 336.9, 195.9 and 235.1 ms/frame. A pilot with
`VICTORY_GUARD_REST=12000` used the exact prior executable and warm
cache, but the host RAM reserve stopped it before the world marker.
There is no comparable world result for that setting. Moving the
generated Cargo target tree from tmpfs to disk preserved the exact
executable hash and recovered about 1 GiB of available host memory.

### Startup presentation and termination

The intro periodically stalled with the diagnostic launcher taking
automatic screenshots. The matched test kept the same executable,
Proton Experimental profile, 2,097,152-view heap, cache seed, input
sequence and split-compute-off setting. Between title frames 192 and
416, 12 of 225 consecutive intervals exceeded 50 ms with intro
screenshots on and one exceeded 50 ms with them off. Eleven of the
12 slow intervals immediately followed an automatic capture at a
multiple of 16. The interval P95 fell from 70.25 to 33.53 ms, with
a 33.33 ms median in both runs. Available host RAM stayed above 14 GiB
in both samples, and host CPU pressure was below 0.4 percent.
The remaining 119.67 ms interval was outside Present and coincided
with the first DirectStorage section reads. The correlation does
not establish which title wait produced it.

In world-capture mode, automatic pre-world screenshots are disabled
by default in the tested Radeon build. Set `VICTORY_CAPTURE_PREWORLD=1`
to restore the earlier startup capture cadence. Explicitly listed
frames and automatic world captures still work. The no-intro-capture
run rendered the normally lit harbor and saved world +2, +6, +8 and +16.
It was stopped by the runner after its requested 16-second world
measurement. The matched intro-only capture-on control was stopped at
frame 448. The two earlier RAM-reserve stops recorded zero cgroup OOM
kills. The `dbghelp.dll` fault in the broad timeline attempt was a
separate unexpected process exit.

The paired intro plot, frame and system telemetry, source-only patch
and shader evidence are stored in the private
`amd-frame-profile/team-diagnostics/harbor-intro-pacing-20261126` directory.

## 52. Playable harbor, character rendering and descriptor allocator

`goal-harbor-character-watch` used the Radeon RX 7900 XTX checkpoint build
with split compute off, the world-only capture change, four pipeline
workers, the usual 4 MiB window and the warm harbor cache. It reached
world +160 and the runner stopped it after its requested 104 seconds
in the world. No cgroup OOM kill or unexpected exit was observed.
The run benefited from moving three older generated Cargo target
directories from tmpfs to disk. Each was left accessible through its
original path and its test executable checksum matched before and after.

World +96 and +100 show visible face, body, beard and cloth, with missing
or streaked scalp hair over the pillow. Hair was visible by +104 and
clearer by +112. The +96 to +112 captures were 21.76 seconds apart.
The named hair vertex/pixel host pipeline completed 22.62 ms after
first bind. Its output is the R32G32_UINT hair visibility target.
Its compute consumers include `CS_HairTileClassify` and the eight
`CS_ResolveToGBufferTiled` variants. Those compute host PSOs were ready
before the first exterior use, so the isolated hair vertex/pixel
translation time cannot account for the whole delayed appearance.

`goal-harbor-character-warm` replayed the same build with the previous
run's pipeline cache seed, mostly the same input sequence, and later
scripted movement. The hair host PSO was ready 63 seconds before the
world marker. Hair was visible in the first sampled interior shot, which
was a later pose in the animation than the previous run's first shot.
In the four near-cut windows, recorded pipeline wait fell from
375–511 ms/frame to 9–41 ms/frame. The warm run stayed active through
world +456, drew the player HUD, and responded to movement input,
including walking through the doorway and turning. It ended at the
requested 153-second world cutoff with no unexpected exit or OOM kill.
The photos alone do not validate longer stability beyond that branch.

The floor highlight persists in warm screenshots around world +220
through +260, and after the camera turns. The screen-space reflection
prepass `7168dd781ca7efb9`, resolve `130d3d04172e5456` and ray-miss
fallback `06bb6e728c7c99bf` were recorded. The last two write the same
half-resolution f26 reflection targets as the interior material passes.
The runtime used diffuse ambient fallback 0.8 with the BVH builder off.
The trace establishes the reflection path but not which pass writes the
excess values. The face and torso also differ in final color. Comparing
their base-color buffers with the same pixels after lighting is needed.
Representative unaltered captures and a contact sheet are in the private
`amd-frame-profile/team-diagnostics/rendering-highlights-20261126` directory.

In the warm exterior, twelve to thirteen thousand draws still prepared
hundreds of milliseconds of copies. Descriptor allocation search and
mutex wait reached 2,505 ms/frame when summed across recording threads,
with 2,260 refused tables. A candidate free-extent index preserves
next-fit order, physical heap boundaries and command-list ownership.
Two standalone tests passed, including differential randomized
allocation and release against the bitmap. A separate 1,048,576-slot
fragmentation fixture took 80.22 ms versus 0.013 ms for 128 forced
refusals interleaved with releases. That fixture probes the worst-case
failure path. Game timing and picture parity decide adoption.

The matched allocator runs used the exact same diagnostic executable,
shader and pipeline cache from `goal-harbor-character-watch`, Proton Experimental,
2,097,152-view RADV heaps, switches and scripted entry. The sole
experimental setting was `VICTORY_INDEX_DYNAMIC_SLOTS=0`/1.
Across four outdoor reporting windows per setting between the world
+16 and +88 capture timestamps, median frame time was 377.1 ms with
the bitmap and 290.6 ms with the extent index. Median descriptor
allocation time summed across recording threads was 194.6 versus
37.8 ms per frame. Median refused tables per frame were 1,013 versus
475. The newer run recorded a median of 13,253 draws versus 11,225.
Because the allocation order is identical for a fixed sequence of
operations, the differing refusals and draws are runtime scheduling
observations rather than a claim that the index increases pool capacity.
Title-memory copy preparation remained 179.9 ms/frame in the indexed
run's median outdoor window.

The matched daylit harbor screenshots retain their building, vegetation
and camera framing. The runner intentionally ended both experiments
after 51 seconds in the world. The Radeon scratch launcher now uses the
validated index by default; `VICTORY_INDEX_DYNAMIC_SLOTS=0` selects
the bitmap for comparison. A patch, text summary, raw timing table and
before/after contact sheet are in the private diagnostic directory
`amd-frame-profile/team-diagnostics/descriptor-index-20261126`.

## 53. Capture-free Radeon harbor timing

The scratch Radeon pacing launcher now omits capture and listed-frame traces
by default. FORK_VISUAL_RUN=1 opts into its previous visual profile. The runner
recognizes the world from a host report averaging at least 500 draws per frame.
For the comparison itself, use the first pixel shader binds of 40ffcb23278729db
and 2d0cace65bc05bb2. Both runs traversed 88 presented-frame intervals between
those CPU-side recording events. These are elapsed sequence times, not per-present
percentiles. Both used the same 7900 XTX diagnostic executable, indexed descriptor
allocator, four pipeline workers, split compute off and the same private warm seed
(goal-harbor-character-watch). No screenshot was saved or frames directory created.

| Run | Guard rest | Seconds | Mean ms/frame | FPS | Median copy preparation ms | Median copy MB |
|---|---:|---:|---:|---:|---:|---:|
| fps-nocap-guard-3000 | 3 ms | 34.254 | 389.25 | 2.57 | 211.4 | 104.7 |
| fps-nocap-guard-12000 | 12 ms | 31.302 | 355.71 | 2.81 | 191.1 | 121.3 |

Copy metrics cover four complete reporting windows inside each marker pair.
The 12 ms setting also had a 4107 ms/frame reporting window. With one run
per setting, a shorter interval overall does not establish repeatability or
better pacing through the transition. Keep 3 ms as the default. Both runs
were intentionally stopped after the measurement.

## 54. GPU-resident title buffers and allocator windows on the RX 7900 XTX

The clean-range word predicate passed the extracted equivalence test and the
Windows diagnostics build. Its initial capture-free harbor run (3 ms guard
rest) took 393.47 ms per interval against 389.25 before. The faster predicate
in isolation did not establish a frame-time gain in the game.

On the resulting executable (SHA-256 c7b54c0a63586e3192e0f1d7e766b09aa25283e6c4cfd86953e0f7903ea2a411),
keep the controls and warm seed in section 53. Paired endpoints are the two
first pixel binds there, separated by 89 frame IDs in all three runs below.
No screenshots were collected. Resident statistics are medians across the
complete logging windows within the interval.

| Run | Minimum named buffer | Window budget | Seconds | ms/frame | FPS | Card copies (MB) |
|---|---:|---:|---:|---:|---:|---:|
| fps-nocap-covers-on | 1 MiB | 0 | 35.019 | 393.47 | 2.54 | 1021 |
| fps-nocap-card64 | 64 KiB | 0 | 34.438 | 386.94 | 2.58 | 1255 |
| fps-nocap-windows512 | 1 MiB | 512 MiB | 28.472 | 319.91 | 3.13 | 1531 |

Small named buffers and windows over the title allocator use different placement
paths. The former increased the median number of card copies from 102 to 3435
with little end-to-end difference. The latter adds a VRAM copy of eligible
windows and still tracks updates from title memory. The fxceless mirror at
9c184b35b43de3455b74ac1770e1c8734a047810 documents a 512 MiB window run on
the RTX 4070. Its published rooms and cutscene results are different workloads.

Repeated in reverse order with VICTORY_MEMORY_MAP=0 for both participants,
to remove the periodic diagnostic address-space walk. Identical executable
and seed, no frame captures or memory-map reports, 88 intervals in both:
fps-nocap-nomap-window-512 took 36.817 s (418.37 ms/frame, 2.39 FPS);
fps-nocap-nomap-window-0 took 36.360 s (413.18 ms/frame, 2.42 FPS).
Median card-copy footprint increased from 1022 to 1531 MB as intended.
The first pair's apparent FPS benefit did not repeat. Keep the existing
placement defaults. A long startup stall remained visible with the walk
disabled. In the 512 MiB repeat the submitting title thread spent 1961
ms/frame between submissions in a two-frame reporting window with only
about 10 ms/frame of its own CPU time there. Stream requests and mappings
were active. A per-request DirectStorage timeline is needed to establish
the dependency. These no-capture tests establish timing only.

## 55. DirectStorage at the entry to the harbor

The first tracing attempt ended with status 143 after 63 seconds, before
the harbor, with no scope OOM kill recorded. The cause of termination is
unknown. The repeat reached the harbor and stopped at its requested world
deadline. Use the same diagnostic binary, warm seed, placement defaults and
capture-free settings as section 54 with VICTORY_MEMORY_MAP=0. The new
observation is VICTORY_DSTORAGE_LOG. Run fps-nocap-stream-repeat traversed
89 marker intervals in 37.151 s (417.43 ms/frame).

Over the first 40 seconds after the harbor marker, choose the reporting
window with the largest mean time from Present to first graphics submission.
In that 8.190 s window (01:20:26.964–01:20:35.154 UTC), 3909 intersecting
requests described 409.3 MB of destination data and all returned success.
The per-request median/p95 times in ms were 632/1191 from enqueue to batch
submit, 12/35 from submit to worker start, and 0/6 executing (1 ms clock).
For 13 status entries the median enqueue-to-ready interval was 35 ms. The
13 first successful polls occurred a median 573 ms after ready, max 1179 ms.
Polling does not identify what the main thread was waiting for. The eight
mapping-counter buckets ending in the interval reported 2637 calls, 2031
aggregate ms and a 4 ms longest call; bucket endpoints are approximate.
Thread 520 spent 701.7 ms/frame between graphics submissions with 6.7 ms/frame
of its own CPU time there. These measurements prioritize title batch submission
and scheduling over adding storage workers. They do not establish how much
of the delay is on the frame's critical path. Half-full queue auto-submit is
incompatible with this title's deferred destination-page setup (dstorage.rs).

## 56. Which thread waits during the harbor entry

Correction to section 55: the thread in copystat's "submitted x/frame"
report can be the compatibility layer's asynchronous submission worker.
Its between-submission time alone cannot identify a wait in title code.
An ordinary stack-profile attempt (fps-nocap-submit-stacks) exited with
code 5 at world entry. Symbolization of victory.exe+0x1da530/+0x1f9274
put the first exception in RtlVirtualUnwind from unwind_into / frames_of
on the profiler thread; the subsequent exception was in dbghelp.dll.
No stack sample file was produced.

For fps-nocap-title-waits the scratch diagnostics build added an opt-in
VICTORY_TIMELINE_TITLE_WAITS flag. It retains imported guest waits/signals,
host spans and GPU timestamps, skips the host import-table hooks, live stack
walks, submitting-thread suspension and symbol initialization. Four traces
of five frames completed without event loss, without capturing images, and
the run ended at the requested deadline. Times below are within those traces
and can include tracing overhead.

In waits.109.json, five harbor frames took 3342 ms. Thread 488 made Present
calls and spent 3072 ms in WaitForSingleObject. Thread 512, the async worker
named in the submission statistics, spent 2502 ms in joined submissions,
including 1837 ms refreshing copies. Do not add overlapping thread times.
Of the waits on 488, 1799 ms were paired with the last observed signal on
the same object from thread 484. The intersection of these wait intervals
with thread 484 executing CopyPageMappingsBatchX was 1639 ms. This is a
temporal dependency observed at a real present. The signal pairing is a
heuristic and mapping may overlap other independent work. Another later
five-frame window had 385 ms of the same mapping. These observations single
out xmem::map_aliases for a targeted test without removing synchronization.

Without timeline or screenshots, on the same binary and seed (3 ms guard
rest), fps-harbor-page-workers-4 ran the marker interval in 30.337 s for
87 frames (348.70 ms, 2.87 FPS); fps-harbor-page-workers-1 took 34.051 s
for 88 (386.94 ms, 2.58 FPS). The former logged three helper threads plus
the caller. One pair; leave one as default pending repeats and visual
comparison. Both runs ended at their requested world deadline.

Ported the serial map_alias overlap fix from fxceless df06bf7 on the
scratch tree, separately from worker tuning. Before the fix the regression
test read 0xA1 instead of 0xB2 after shifting two aliased pages. With the
fix the Wine xmem suite passed 19 tests, including preservation of prior
destinations when a source is missing; one benchmark was ignored. The
grouped multi-worker path is unchanged. A capture-free harbor run with
VICTORY_ALIAS_CENSUS=1 reached its deadline and reported at least 65,536
serial alias calls. No overlap or missing-source event occurred (first
occurrence of either would be logged). Its 89 marker intervals took
37.610 s; this diagnostic run does not isolate performance of the fix.
No evidence for this specific bug causing the harbor's rendering errors.
The correctness patch, wait-profiling patch and both regression logs
are archived under team-diagnostics/harbor-mapping-20261126.

## 57. Bedroom diffuse lighting A/B, one capture per run

The Radeon scratch launcher defaults to VICTORY_AMBIENT=0.8. A per-run
override was used for this experiment; the cache seed, descriptor pool,
single mapping thread, scripted account input and diagnostic binary
(a15f52e02f5082876b92b3a4b1b4f2cc70f401f69f700c879ec976913829d761)
were the same. The scratch VICTORY_WORLD_SHOT=240 selected one presented
frame after entry, with all other screenshots and operation dumps disabled.
Each run stopped on confirmation that its one PNG was saved.

| Run | Diffuse override | Absolute frame | Mean luma | Median luma | Pixels with luma >=240 |
|---|---:|---:|---:|---:|---:|
| room-ambient-08-single | 0.8 | 2471 | 67.75 | 62.43 | 2.31% |
| room-ambient-0-single | 0 | 2538 | 233.73 | 244.79 | 57.44% |

Luma is computed on final 8-bit RGB (Rec.709 coefficients); the HUD is
included. The dim wall, torso and dark hair remain visible at 0.8; at 0 the
frame is dominated by a white flare. These observations agree with the
user's live report. Equal world offsets do not fix the camera pose across
runs; the off frame obscures the boards. A matched per-pixel floor
comparison is therefore not supported. At 0.8 the visible bright floor
in x=595..719, y=770..841 averaged 237.1, versus 101.3 for the shaded
wood sample x=550..659, y=942..1001. The diffuse fallback is retained.

A third run, room-ambient-restored-probe, explicitly unset the override
before launching. The log confirmed Resolve and Radiance level 0.8.
Frame 2558 (world +240) again showed the bedroom, boards, daylight and
character, with 2.29% of pixels at luma >=240. A pale outline/ghost remains
around the character, and the boards remain glossy. The camera differed
from both other shots. The run ended at the requested world deadline.

For this run a diagnostics-only probe sampled nine coordinates on each of
two verified 1280x720 R11G11B10_FLOAT UAVs before and after the indirect SSR
resolve (hash 130d3d04172e5456). Readbacks were recorded in the same command
list and read after GPU completion. At (1115,540), resource 30207 changed
from RGB (20.25,28.5,39.0) to (0.00806,0.00537,0.00220). Its other eight
samples were unchanged; all nine on resource 30212 stayed zero. This
establishes that SSR resolve executed and modified one of the two reflection
resources. The high value was already present at input to that dispatch;
this does not establish which stage causes the visible highlight.

## 58. Bedroom reflection pass samples at world +240

The scratch 7900XTX diagnostic build ran room-reflections-all-passes under
Proton Experimental with ambient 0.8, one mapping worker, split compute off,
dynamic slot indexing on and the goal-harbor-character-watch seed. The
single screenshot (frame 2328) shows skin, hair and wood grain. The floor
near the far window still looks extremely glossy. 6.44% of the full image
had 8-bit luma >=240, including the windows. Camera framing varies by run.

Nine points were read before and after each of 16 dispatches across four
shader hashes and three verified reflection resources. Only resource 29509
changed at those points. CS_RayMiss raised RGB (24.5,27.75,33) to
(34.5,35.5,37) at (1050,528) and changed (20,30,39) to (23.5,27,33)
at (1115,540). CS_ResolveScreenSpace slightly reduced a third, much darker
sample at (1180,550). The high HDR values thus already existed before the
sampled ray-miss dispatch. Sparse samples cannot identify the final floor
compositor or establish whether the radiance is wrong before tone mapping.
No FPS comparison was made with this probe.

## 59. Local intervention in the screen-space reflection result

The scratch 7900XTX build now has an opt-in intervention after verified
CS_ResolveScreenSpace indirect dispatches on the selected frame. It copies
32x128 strips from a retained upload into the typed 1280x720 texture,
restores the UAV state and reads selected pixels after the write. Default
runs do not record this copy. Test both placements because the room camera
can end up on either side of the character at world +240.

In room-reflection-stripes (same seed and switches as section 58), the zero
strips were on the right and the brightly lit floor appeared on the left.
The three sampled locations inside strips were already zero at every
observed dispatch. The left-side control points did contain radiance.
The screenshot and readback therefore cannot establish a connection to
the visible floor. The runner intentionally stopped at its world deadline.

In room-reflection-marker the probe instead wrote HDR RGB (16,0,16) in
eight narrow strips, on both sides. Readback confirmed the value at five
in-strip points per sampled dispatch. In frame 2558 the floor itself shows
magenta strips. Of 3171 pixels where min(R-G,B-G)>30, 3170 were inside
the projected strips (zero across the full image in both preceding controls).
This directly connects the tested screen-space reflection outputs to the
visible floor. It does not establish whether the source radiance, material
roughness, or blend weight is wrong. Do not change ambient for this effect.

## 60. Waiting in the bedroom for skin and fabrics

Idle tests use the 7900XTX diagnostic build, native compiler, warm
goal-harbor-character-watch seed, ambient 0.8, one mapping worker, indexed
dynamic slots and split compute off. No reflection intervention, movement or
camera inputs after entering the world. The title itself enters an idle camera
with different framing, exposure and visible surfaces. Compare recognizable
details and persistent defects, not registered pixel deltas between poses.

room-idle-five-min saved the bedroom at world +240 and ran another 297.8 s
before the 3 GiB host reserve stopped it, just short of the scheduled image.
There was no scope OOM kill. During the interval the log reported 131 shader
translations, 21 non-mesh and 98 mesh host pipeline records. Most cited
host-pipeline drops were attributed to the disabled ray-tracing builder.
The global upload counters advanced from 3.35 to 6.26 GB; these cannot
identify the body/cloth resources. First and last minute median report times
were 225 and 434 ms/frame respectively (not matched-camera benchmarks).

room-idle-repeat at world +210 and +150.1 s showed the rear and front views
of the same character. The wall curtain had a detailed leaf print. In the
front view the face and hair were recognizable and the crossed arms had
little shape separation. The automatic reserve stopped this second run
after the middle image. Relocated 90 compiled scratch build products to
disk, preserving their paths via symlinks after verifying copies with SHA-256
(901565314 bytes of tmpfs files replaced). No game data or shader caches
were moved. room-idle-headroom repeated the middle capture and exited with
status 143 after about 219 s in the bedroom. Last sampled host availability
was above reserve with zero scope OOM kills. The scope journal does not
identify who sent the signal.

room-idle-soak completed 305.3 s of idle time after the first bedroom shot,
with zero observed scope OOM kills. The supervisor intentionally terminated
the game after completing the observation. Frames 2225, 2901, 3398 were
saved at 0, 135.4 and 270.2 s. The first rear shot shows a grid pattern on the
shorts and texture on the back. The middle close-up shows a detailed beard and
leaf-print curtain, while the chest and crossed arms still have comparatively
flat shading. The last wide rear view still shows an unusually bright floor
through the doorway and very bright bedside fabric. The varying camera does
not provide a same-angle skin or fabric quality score. 191 shader-translation
log entries occurred between first and last images. The run-wide indexed
texture upload counter increased from ~3.26 GB near the first shot to ~6.21
GB near the end. Median report rates in the first and last minute were 7.0
and 4.4 FPS respectively. This is an idle sequence, not a controlled FPS
benchmark. A source-only diagnostic has been applied to the scratch tree
to sample the color targets around direct character mesh draws; it has
not been run or validated yet.

## 61. Character mesh material readbacks in the bedroom

The scratch 7900XTX diagnostic build sampled the actual six OM color targets
immediately before and after two direct DispatchMesh commands with mesh
51ff151f75cd5759 and pixel 7fe75645c406cac6. It verifies list-local state,
retained host resource identity and the typed RTV. The format was sRGB RGBA8
for slot 0 and UNORM RGBA8 for slots 1-5. Raw values below are bytes in RGBA
order. Samples were read back only after completion.

room-ped-material at +240 produced no image or landed probe before the world
deadline. room-ped-material-210 reached frame 2227 and ended at the requested
world deadline. Same warm seed, ambient 0.8, one mapping worker, split compute
off, indexed dynamic slots. Nine points x six targets x two draws produced
108 before/after pairs; none changed. In the paired image points 6, 7 and 8
lie on the character's shoulder/back and arm. Before both draws, slot 0 was
B5 87 6F 1D at all three, slot 2 was 1E 1E 1E 00 and slots 1 and 3 varied.
The nearby background points had different slot-0 values. These sparse
samples neither identify the actual surface writer nor assign channel
semantics. The two named ped draws cannot explain a change at these points.

room-ped-three-pairs reproduced the unchanged named ped draws (216 sample
pairs). Four attempted readbacks on another pair were rejected. The warm
operation trace showed these draws using a partial scissor with the same
full-size viewport. For room-ped-producers, accept a nonempty scissor and
log its bounds; retain the full viewport requirement. Sample only slot 0
for the additional skinned mesh pairs so the unrelated auxiliary target
format does not prevent reading the first color. Include indirect meshes.

room-ped-producers reached frame 2410, with conspicuous angular patches of
flat-looking skin next to detailed skin on the same back and shoulder. All
33 selected draw calls from eight mesh/pixel pairs returned slot-0 samples
(nine per draw), and no before/after word changed at those locations. This
includes the two previously skipped direct draws and 28 indirect draws.
Slot 0 used the same retained resource across all 33. At the point (992,736)
inside the shoulder/upper arm in this pose, the observed RGBA was AC 7B 63 1D
in 32 readbacks and AD 7C 64 1D in one. Distinct draws can see intervening
writes; log order across lists cannot identify those writes. Both runs
ended at the requested world deadlines. A draw name alone is insufficient
to identify the producer; the body patches are visible in the saved frame.

## 62. Early out for redundant write-watch range collection

On the scratch 7900XTX build, VICTORY_LAZY_ASK_RANGES=1 checks whether the
submission-wide journal is kept before enumerating and sorting ranges for a
flush. The serial submission already holds that asking. Actual writes and
dirty-page checks retain the original fallback. ABBA test: same executable
64f0076490e5, same warm goal-harbor-character-watch seed, no screenshots,
one mapping worker, 3 ms guard rest, index dynamic slots on, split compute
off, ambient 0.8. Compare first pixel shader bind 40ffcb23278729db to first
2d0cace65bc05bb2. All four stopped intentionally at the latter marker and
recorded zero scope OOM kills. After A the next launch's RAM preflight
refused an immediate restart, before making a game instance. Releasing the
previous runtime's memory allowed the other runs to proceed.

| Run | Skip range enumeration | Frames | Seconds | ms/frame | FPS |
|---|---|---:|---:|---:|---:|
| fps-lazy-range-0-a | off | 88 | 36.023 | 409.36 | 2.44 |
| fps-lazy-range-1-a | on | 89 | 29.169 | 327.74 | 3.05 |
| fps-lazy-range-1-b | on | 89 | 31.733 | 356.55 | 2.80 |
| fps-lazy-range-0-b | off | 89 | 33.846 | 380.29 | 2.63 |

Pooled marker times fell from 394.74 to 342.15 ms/frame (13.3%). Median
skipped candidate spans per frame across full diagnostic reporting windows
were 8385 and 8753 in the on runs, zero in both off runs. Counts describe
range metadata; bytes still needed by the game are copied as before. The
two pairs favor enabling the candidate for further testing in this build.
This test does not attribute all elapsed time saved to enumeration or cover
other scenes, long-run stability, or image equivalence.

The tested source-only patch and run summarizer are archived in the private
diagnostics directory team-diagnostics/harbor-lazy-ranges-20261126. Patch
SHA-256: 572559ab7bd4591534b0c5b437faa7c58836ea223d7267f10260ffa2f6863c2d.

## 63. A spatial sample of the character's six color targets

The scratch Radeon diagnostic now captures four consecutive rows every
40 rows across the six verified 2560x1440 G-buffer targets, after the first
matching ped draw. The readback is 7.5 MiB per selected frame, gathered
after the submission fence. The launcher saves one final image from that
frame. The contents include preceding writers regardless of shader name;
the method does not itself identify a writer.

With the same seed and switches as section 62, including lazy range checks
on, room-ped-region-lazy reached the bedroom at world +210 (frame 2381)
and ended at the requested deadline. At target x=1280..1519, y=800..1040,
the sampled central back patch contained 5760 values. Slot 0 held 692
distinct RGBA words. Its 5th and 95th percentile RGB bytes were (163,111,85)
and (176,128,102), with alpha 29 throughout. The most frequent word
occurred 93 times. Slot 1 had 5757 distinct words, slot 3 had 130 and slots
4 and 5 were uniform. These are storage values; slot 0 is sRGB. The back
shows detail in the final image. This successful sample establishes that
the active configuration can deliver a spatially varying body color buffer.
The earlier angular skin patches were not apparent on this frame.

room-ped-region-early at +185 showed the character in frame 2372 but no
qualifying regional readback. Neither capture establishes the material
values on opposite sides of a visible angular skin patch.

## 64. Time spent rechecking absent guard blocks at harbor entry

fps-harbor-hole-timing used lazy range checks plus VICTORY_HOLE_TIMING=1,
otherwise the capture-free section 62 settings. The same marker interval
took 34.554 s for 88 frames; the 24 complete reporting intervals within it
cover 84 frames. Zero recorded OOM kills; requested marker stop.
Counters are elapsed time accumulated across threads, not additive frame
latencies. Enumerating unknown blocks while the guards lock was held took
262.57 ms total (3.13 ms/frame, excluding time acquiring the lock). The
unlocked loop verifying them took 63.42 s (755.01 aggregate ms/frame).
It reported 1,992,107 unknown blocks, 649,659 VirtualQuery calls from
uncommitted_end and only two calls to is_view for potentially committed
blocks. The separate failed-protection code made 3,899 region queries and
4,670 placeholder queries. The 1-in-64 samples estimate only 0.28 and 0.014
aggregate ms/frame for those two sites respectively. Focus first on why
already-seen uncommitted regions need revalidation; preserving detection of
ordinary guest commits and concurrent remaps is required. The bitset
enumeration alone is a small fraction of this path's elapsed time.

A two-allocation probe ran under the same installed Proton Experimental
client, in the isolated timing prefix, without starting the game. It
reserved 64 KiB once normally and once with MEM_RESERVE_PLACEHOLDER using
VirtualAlloc2. For both, NtQueryVirtualMemory class 7 returned C0000003
(STATUS_INVALID_INFO_CLASS) and zero bytes. Class 3 succeeded with 48
bytes and type bits zero for both. watch::placeholder uses class 7 bit 8.
Thus this path cannot mark even a known fresh placeholder as held on this
Wine client. This test does not show how many of the harbor queries were
for actual placeholders. Measure the address overlap with live HLE-created
placeholder reservations and successful replacement/unmap transitions
before skipping any recheck of guest-committable or reused addresses.

## 65. Live placeholder classification on a private Proton client

Built the Unix ntdll from the local Proton Wine source used for the earlier
wineserver study. A freshly built baseline returned the same class 3/7
answers as installed Proton. A separate client directory contains its own
ntdll with a private diagnostic query. On the current process only, it
classifies an aligned 64 KiB block under Wine's virtual-memory mutex. A
positive result requires the entire block to fit the same file_view with
VPROT_FREE_PLACEHOLDER set. No change to NT information class 3 or 7. In
the stand-alone test ordinary reservation, free placeholder, replacement
with committed private pages, restored placeholder and released address
returned 0, 1, 0, 1, 0 respectively, all with success and a four-byte
response. No changes to the installed Proton or the title.

fps-actual-placeholder, no screenshots, opt-in caching OFF, used the warm
section 62 harbor preset and the private client. Across 23 complete intervals
covering 85 frames, the usual recheck loop made 581017 VirtualQuery calls.
Of 8971 sampled lookups, 8969 were confirmed wholly within a currently
free Wine placeholder, two were not, and zero queries were unsupported.
The independent historical counts showed 8969 inside the original HLE
reservation spans and two outside. The histories alone do not prove current
type. The validation loop accumulated 726.6 ms/frame across threads. The
run reached the second harbor marker and stopped intentionally.

With VICTORY_WINE_FREE_PLACEHOLDERS=1, the scratch diagnostic build uses
the private query for the existing held-block optimization. The tested
binary falls back to the normal client query if the private operation is
unavailable. Both Wine GPU tests ran and passed individually:
watch::many_askers_tests::holes_are_known_until_the_runtime_tells and
watch::many_askers_tests::no_mapping_is_lost_between_askers.

ABBA comparison fps-wine-held-{1-a,0-a,0-b,1-b}, same private ntdll
75fd2c2b490d and exe 6410f7604220 in all four. No screenshots, provenance
or extra hole timing. Lazy range enumeration already enabled in both
variants. The median column counts the existing query counter per frame
across complete reporting windows; it excludes the held-block audit at
Present. Times compare the same section 62 first-bind markers.

| Order | Use actual free placeholders | Frames | Seconds | ms/frame | FPS | Median hole queries/frame |
|---|---|---:|---:|---:|---:|---:|
| 1-a | yes | 88 | 30.185 | 343.01 | 2.92 | 18 |
| 0-a | no | 88 | 36.012 | 409.23 | 2.44 | 8732.5 |
| 0-b | no | 88 | 30.480 | 346.37 | 2.89 | 7345 |
| 1-b | yes | 89 | 28.340 | 318.42 | 3.14 | 13 |

Pooled time was 377.80 ms/frame off and 330.65 on (12.5% lower). Both
ordered pairs favor the client extension. Zero held-block audit warnings,
zero scope OOM kills, four planned marker stops. The counters show a major
reduction in rechecks. They are not evidence of an equal reduction in
frame latency. Both source patches, the diagnostic C test, private ntdll,
tested exe and results are archived in team-diagnostics/wine-free-placeholder.

One-shot bedroom visual comparison on that same executable and private
client, with six verified MRT readbacks at world +210. In room-wine-held-0
(frame 2537) the character's back shows texture. In room-wine-held-repeat
(frame 2468, switch on) the central back is visibly flat despite hair and
patterned shorts rendering. Both ended at the planned world deadline. An
earlier on attempt terminated with status 143 at startup before the
bedroom, cause unknown, without a recorded scope OOM kill.

The center-back ROI was selected separately on each image because the
camera positions differ (120 columns, y=780..1024 sampled in strips; target
x=[1390,1510) off, x=[1070,1190) on). Out of 3360 pixels, slot 0 showed
380 distinct RGBA words in the textured view. All 3360 pixels in the flat
view were exactly B5 87 6F 1D. Slot 1 in the flat view still had 3189
distinct words. Thus the observed loss of spatial color variation is
already in the first color attachment before final lighting/postprocessing.
It is not proof of which material input or draw caused it. The older
room-ped-material-210 on unmodified Proton showed the same flat appearance
and exact value at three skin points (section 61). One image per variant,
with different camera pose and streaming state, does not establish that
the new optimization caused, fixed or increased the rate of this bug.
Do not promote the experimental client to the default on this evidence.

## 66. Per-present timing and guard rest after the placeholder change

Private Wine client, VICTORY_WINE_FREE_PLACEHOLDERS=1, lazy range enumeration,
one mapping worker, same warm seed and switches as section 65. Pacing-only
diagnostic binary 85ada65e0aea. Four sequential runs, no screenshots, zero
scope OOM kills. The window uses the same first shader binds as before.
Each run had 88 guest frames and 87 complete successive Present-return
intervals inside the marker boundaries. CPU-side Present intervals include
work on both sides of the API, and do not measure scan-out. P95 uses nearest
rank; with 87 samples the maximum is also p99.

| Guard rest / run | Whole window ms/frame | Median interval ms | P95 ms | Max ms |
|---|---:|---:|---:|---:|
| 3 ms / A | 328.98 | 247.01 | 722.87 | 2724.02 |
| 12 ms / A | 352.70 | 286.53 | 1055.39 | 2276.31 |
| 12 ms / B | 313.89 | 214.63 | 860.17 | 3951.95 |
| 3 ms / B | 336.19 | 228.96 | 727.07 | 3605.65 |

Mean frame time pooled across runs was 332.6 ms at 3 ms and 333.3 ms
at 12 ms. The longer rest did not repeat its previous mean improvement,
and its p95 was worse in both pairs. Retain the 3 ms experimental preset.
The worst returns began 5.2–8.6 s after the first marker. No shader translation
or pipeline creation record appeared inside any of these four intervals.
Median host GPU load samples in these intervals ranged from 4 to 8 percent.
Aside from the Windows process and host GPU being different measurement
domains, utilization does not establish what the presenting thread waited on.

A subsequent no-capture run (fps-marker-stall-waits) armed the
wait/signal recorder four Present completions after the first harbor
bind, recording 26 intervals, with VICTORY_TIMELINE_TITLE_WAITS=1.
The trace and pacing stream agree on the single longest interval:
4089.48 ms, between Present returns 2313 and 2314; no timeline
events were lost. TID 488 spent 4089.30 ms in WaitForSingleObject
on handle 0x66c. Pairing waits with the last signal observed on the
same handle attributed 4065.93 ms of waits to TID 484. Six
CopyPageMappingsBatchX calls on that thread lay entirely inside this
interval: 1211.50, 617.74, 659.58, 603.97, 362.35, and 538.63 ms.
Their total overlap with the paired waits was 3993.77 ms. This is a
temporal correlation with a heuristic signal pairing, not a sum of
independent frame costs. The archived trace is stall.json in the run
directory. It records neither page counts nor the cost split inside
the six mapping calls.

## 67. A measured color writer on the character in the bedroom

The single-frame room-raster-line probe targeted world +210 (frame 2200,
2560 × 1440), recording a 640-pixel row before and after raster commands
with a verified sRGB render target. The row was at y=880, x=960..1599.
Of 6639 draw candidates, 2663 yielded paired readbacks on resource 30511.
One indirect mesh draw wrote alpha 29 on 200 distinct sampled pixels
(x=960..1159), across the visible torso and arm. Its mesh hash was
5bd95fdec765c0d6 (MS_VS_PedTransformPreSkinD) and its pixel hash
398971d1222d8f95 (PS_DeferredPedTexturedNC), both variant b1a27fc2.
This identifies a writer in the detailed-body run only; the faulty input
in the flat run has not yet been identified. In the six-target spatial
sample from that detailed run, the most common slot-0 RGBA value among
32089 alpha-29 samples occurred 199 times (0.62%). In the prior
room-wine-held-repeat flat run, B5 87 6F 1D occurred in 38506 of 42318
such samples (91.0%). The shorts showed their grid pattern in both images.

The full translated shader cache entry (DXIL payload 120424 bytes) for
398971d1222d8f95 compared byte-for-byte equal in the detailed and flat runs.
Both logs recorded creation and binding of this mesh/pixel pipeline. This
rules out differing cached code for that variant between these two runs;
actual sampled descriptor indices and texels in the flat frame are unknown.

## 68. Five-minute idle test on the current AMD scratch build

Binary d41c5a6c93a6, initial sample at world +210, next captures requested at
150 and 300 seconds from that room frame. No movement or camera input in the
world. All attempts used the same warm seed, and ran sequentially.

* room-held-five-min (experimental Wine free placeholders and lazy ask ranges)
  showed texture on the back and a grid on the shorts at frame 2469. The
  session ended with exit status 143 at +59.2 s. Scope OOM kills stayed at
  zero. The signal source is unknown.
* room-held-idle-repeat (same binary and switches) showed a flat-looking back
  and patterned shorts at frame 2549. The last diagnostic was at +17.6 s.
  Neither subsequent timed image arrived, although the process stayed alive
  for more than five minutes after the first capture. A live stack sample
  found all 160 game-process threads in wait, poll or read paths and the GPU
  was at low single-digit utilization in later samples. We intentionally
  stopped that scope at +320 s. The lock owner has not been established.
  This run cannot measure how the skin changes during five minutes of
  actively rendered game time.
* room-stock-idle-current used the same executable on stock Proton Experimental
  with both experimental placeholder/range switches unset. It rendered for
  300.3 s between the first and third screenshots: frames 2235, 3025 and 3487
  (0, 150.6 and 300.3 s). The back already had detail in the first image. The
  shorts had a grid pattern and the curtain retained a leaf print. At +300 s
  the chest had visible detail, but the face was very bright relative to the
  torso and the brightly reflective blue-white bedding remained conspicuous.
  The automatic camera and lighting changed. This does not establish recovery
  from a uniformly flat body. The game advanced 1252 frames, 4.17 FPS overall.
  Median periodic FPS reports were 4.6 in the first minute and 2.25 in the
  last. There were 226 shader translation records between shots. No scope OOM
  kill or device-loss record. These runs do not isolate a cause of the hang.

## 69. Disjoint column timings inside the harbor mapping stall

fps-marker-mapping-stages used the section 66 marker-centered timeline, one
map worker, experimental Wine placeholder classification, lazy ranges and
3 ms guard rest. No captures. The longest interval in both pacing and the
26-interval trace was Present 2315 to 2316, 2991.08 ms. No trace events
were lost. TID 488 spent 2990.84 ms waiting on handle 0x678. Four complete
CopyPageMappings calls on TID 484 totaled 2919.55 ms; their detail lines
matched the traced durations and endpoints. Within those calls, 2919
MapViewOfFile3 calls consumed 2115.99 ms; 1741 unmaps consumed 752.64 ms.
The two OS operations occupied 95.9% of this particular frame. There
were no identical backing/page candidates or already-empty destination
unmaps among 4660 compared pages. Physical lock acquisition totaled 0.27 ms.
A short 5.33 ms mapping call on another thread fell below the detail
threshold. This is a frame-specific wall-time breakdown; it does not
attribute the time inside the OS calls to the Wine server alone.

## 70. Indexed wineserver doubles harbor frame rate on the current AMD build

The same diagnostic executable (2aff0f8e298f), native compiler, warm seed,
descriptor limits, private placeholder-aware ntdll, lazy ask ranges, one
mapping worker and 3 ms guard rest were used for four independent starts.
No screenshots or wait tracing. Pacing and outer-call timing were enabled
in all four runs. The two locally built servers use the same Wine revision
5b557605, protocol 931. Only the server binary differed. Both its SHA-256
and the live server executable in each run's cgroup were verified.
Two setup attempts were excluded before measurement: one selected the
installed server, another could not load NLS data via a symlinked binary.

| Sequential run | Marker frames | ms/frame | FPS | p95 ms | Worst ms |
|---|---:|---:|---:|---:|---:|
| Control A | 89 | 369.63 | 2.71 | 718.03 | 4594.11 |
| Indexed A | 89 | 186.86 | 5.35 | 270.26 | 418.21 |
| Indexed B | 89 | 188.16 | 5.31 | 277.36 | 309.63 |
| Control B | 88 | 378.91 | 2.64 | 568.04 | 3570.33 |

Median/worst interval estimates use complete inter-Present intervals
inside both markers; mean frame time uses marker elapsed time divided by
guest frame advance. All runs reached the exit marker and were stopped
intentionally with zero scope OOM kills. This validates the server index
for this harbor phase and build; the older CGBJ build tested in a previous
session regressed, so the result must be scoped to the current combination.
The reproducible local files live under amd-frame-profile/team-diagnostics/
server-index-ab: victory-harbor-tested.exe, indexed/files/bin/wineserver
(SHA-256 ba3d4a9e5ad7d368df7c59ae6b39a3a87cb44be7a4d1c20478c7c8b68259dd4d),
the matched baseline and the same private Wine client. The four evidence
directories are fps-held-server-bin-{1-baseline,2-indexed,3-indexed,4-baseline}.

With the indexed server and the original executable, fps-indexed-marker-waits
captured the same 26-interval diagnostic window with zero lost events.
Median Present interval was 198.4 ms. The title wait occupied 95.4 ms in
median and the Present queue-depth gate 79.8 ms. Waking-thread alias mappings
overlapped title waits by only 1.0 ms in median. The submit worker's joined
submissions overlapped the depth gate by 42.3 ms in median. These times can
overlap. Source and trace do not establish that another queue slot would
increase actual display throughput. The next depth test must measure real
swapchain Present completions and remaining backlog at the markers.

## 71. Reading the detailed body draw's material record on the GPU

room-body-record-indexed on the indexed server used a separate diagnostic
binary and one frame at world +210. The probe gated on both shader hashes,
the exact compiled pixel DXIL digest, the list-owned descriptor fingerprint
and the underlying ordinary shadow buffer identity. The GPU read the 48-byte
material record through the raw SRV and wrote to private diagnostic storage.
The original body draw's pre/post readbacks show an indirect mesh writing
484 changed alpha-29 pixels on the scanned row (x=960..1443). In that same
event the material record began at byte 53460752 in a 182452224-byte view.
The index word at offset +32 in the record was 36395. This is the index
used by the first unconditional image sample in this exact translated
program. It does not identify the bound image or prove its texel value.
The PNG shows texture on the back and the shorts. The most common value in
the alpha-29 spatial sample occupied 209 of 42157 samples (0.50%).
See room-body-record-indexed/frames/frame-1169.png and its paired
raster-1169-s6092-l10-c130.body.txt sidecar.

A second independent indexed run (room-body-record-repeat) also showed a
textured back, with a different camera pose. The same shader pair wrote
516 tagged pixels on the row. Its verified material record had an index
word of 35161 at +32 and no flat-color writes on those 516 pixels.
The most frequent RGBA value accounted for 385 of 60123 tagged samples
(0.64%). Both good renders therefore used different numeric slots. A slot
number alone is not a texture identity, and neither of these runs explains
the earlier flat color. The bright floor is visible in both captures.

## 72. Actual swapchain Present completions at two queue depths

The indexed diagnostic binary recorded host API returns and submit-queue
occupancy using one monotonic clock; logs were flushed after the two shader
bind markers. Same executable within this four-run comparison. Successful
returns are not a measurement of compositor presentation or physical scanout.

| Run order / queued frames allowed | Successful host returns | Seconds | Returns/s | Host interval p95 ms | Queue at markers |
|---|---:|---:|---:|---:|---|
| A / 1 | 89 | 13.197 | 6.744 | 218.47 | 1 to 1 |
| A / 2 | 89 | 13.296 | 6.694 | 260.80 | 1 to 1 |
| B / 2 | 89 | 13.168 | 6.759 | 234.77 | 1 to 1 |
| B / 1 | 89 | 14.003 | 6.356 | 243.46 | 1 to 2 |

All observed API returns succeeded. Depth two was slightly slower in the
first pair and faster in the second. Retain the default depth of one.
The binary used here includes additional diagnostics versus section 70;
absolute results across sections are not a controlled depth comparison.

## 73. A real descriptor at the detailed body draw's first sample

On the shipped server with the private placeholder client and the same warm
seed, room-body-identity-control frame 2355 showed a detailed back. Two
verified writer events in lists submitted at 10092 and 10109 had the same
48-byte material record and each changed 186 tagged pixels. The GPU read
index 35719; it matched the submit-time prediction and the watched slot
was not rewritten through either completed fence. The actual binding was
a retained 256 × 512 texture, one layer, seven mip levels, DXGI format 83
in both resource and SRV. This excludes neutral and null descriptors for
this one sample in this detailed image. It does not identify the texel,
mip actually read, source of body color, or the cause of the intermittent
flat render. The qualified per-event output is in the two .slot.txt files
next to frame-2355.png. The run was stopped at its planned world deadline.

## 74. Helper concurrency while the indexed Present queue is waiting

Intersected the indexed timeline's Present queue-depth waits with
victory-submit's "work spread over threads" spans over 26 complete,
non-synthetic return intervals. These windows totaled 603.18 ms.
Five distinct victory-wide helper threads appeared. In 62.97% of that
time all five were inside helper jobs; in 8.03% none was. Mean was
3.67 helpers inside jobs. This counts elapsed job spans, including waits,
not CPU utilization. Across helpers, nested texture refresh accounted for
1103.3 thread-ms and buffer refresh 433.9 thread-ms; these figures must
not be added to elapsed frame time. This motivates a controlled pool-size
comparison using real swapchain completions with the queue limit kept at one.

Four sequential no-capture runs used the same binary, same indexed server,
warm seed and queue limit. Host completions and p95 inter-return times were
7.038/s and 207.04 ms (5 workers), 6.811/s and 227.78 ms (8), 6.242/s and
248.03 ms (8), and 6.337/s and 243.82 ms (5). All 355 observed swapchain
returns succeeded. Pooled rates were 6.67/s at five and 6.51/s at eight.
No improvement from adding workers; retain the five-helper default on
this machine. These are API completions, not a display scanout measurement.

## 75. Where texture refresh time goes at the harbor

room-phase-scan-indexed used five helpers and an indexed server. Approximately
one operation in sixteen was randomly sampled per thread between the harbor
markers; numbers below are the sampled totals summed across threads.
The 2247 sampled full-buffer hashes read 1328.7 MiB and took 170.5 ms,
of which 163.3 ms (95.8%) produced the previous digest. The 1947 mapping
checks took 72.2 ms, 259 separate reads took 22.4 ms and 521 detiles took
15.1 ms. Direct mapped reads as part of hashing cannot be separated.
Of the sampled hash time, 146.5 ms was on wide helpers and 23.4 ms on the
submitter. This identifies a candidate for a faster full-content hash
without relaxing texture validation. These thread times are not frame savings.

The same run checked 640 pixels on row y=900 at world offsets +180,+210,
+240,+270 without PNGs. All four frames had at least 522 tagged skin pixels
in one of the completed events and no exact B5 87 6F 1D pixels. The row test
was separately calibrated against an earlier flat frame (490 of 490) and
six earlier detailed frames (zero exact matches). No failure was captured
on the indexed run; this does not establish that the defect is fixed.

## 76. Full staging hash experiment and the effect of a compatible cache

An opt-in XXH3-128 implementation checks the full staging input wherever
the previous 128-bit hash was checked. Resource holds are process-local;
the separate fingerprint used for stream identity keeps the original
algorithm. Both modes passed the Wine GPU crate test for changed bytes,
lengths, swaps and repeatability. The existing cold_texture_costs single-
thread microbenchmark measured 0.034 ms/MB for the original hash and
0.028 ms/MB for XXH3 in this pair of measurements.

Adding the single cached Rust dependency changes Cargo.lock. victory-gpu's
build.rs includes that entire file in VICTORY_TRANSLATOR, so identical seed
files do not imply that translated shaders are reusable by the new build.
In fps-staging-hash-{1-xxh0,2-xxh1,3-xxh1,4-xxh0}, cloned from the earlier
warm seed, host API completions/s were 2.844, 2.844, 2.862 and 2.893. The
harbor reports in the second run still showed roughly 140–160 pipelines
being made. The pre-existing seed contained 272 MiB of translated shaders;
the second run ended with 378 MiB. Both modes shared the same executable,
so only between-build comparisons are confounded by the changed key.

Reusing caches from the second game run alone did not establish a warm
baseline: fps-warm-staging-1-xxh0 still logged pipelines being made and
terminated before the closing marker. It exited 5 at the title's known
GPU-progress watchdog (+0x2215c35); there was no scope OOM kill. Its
incomplete window was excluded. In a separate clone, the runtime's
warm --used reported zero recorded completions for the new fingerprint
a72ce777d08c746b. Of 83556 known recipes, it selected 1994 and created
all 1994 in 35 s with four worker processes, zero refused and zero worker
crashes. Six previously marked bad recipes were excluded before selection.

From four independent clones of staging-hash-precompiled, same binary,
hardware, seed prefix and switches except for the staging hash:

| Hash, run order | Successful host presents/s | p95 ms | Hash ms/MB (sampled) |
|---|---:|---:|---:|
| Original, A | 6.538 | 234.68 | 0.126 |
| XXH3, A | 6.529 | 232.55 | 0.112 |
| XXH3, B | 6.619 | 232.17 | 0.114 |
| Original, B | 6.703 | 246.97 | 0.125 |

Every observed Present returned success, with queue depth 1 at both markers
in all four runs. The final in-window frame reports in these runs showed
zero draws waiting on pipeline creation. XXH3 reduced sampled hash time per
byte in both comparisons but did not raise total throughput; retain the
original staging hash. The high-impact difference was the state of the
cache prepared for this exact build. The precompiled seed derives from a
completed pass with the same executable plus explicit warm --used, so
the four-versus-four result measures the combined seed preparation.

## 77. Five minutes stationary in the bedroom on the precompiled build

RX 7900 XTX, indexed Wine server, five refresh workers, original staging
hash, ambient 0.8 and the section 76 private seed. Three screenshots per
completed run, at 0, 150 and 300 seconds after world +210. Both room runs
launched the archived binary as victory-tested.exe. The VIctory translation
fingerprint matched the seed, but the vkd3d-proton driver cache used a new
basename. Harbor checkpoint replays use victory.exe to match both.

The first attempt, room-precompiled-five-min, reached the host RAM reserve
after 144 seconds (no scope OOM kill). Its first-minute median was 11.75 FPS.
Archived 413 old tmpfs compiler objects with verified hashes and retained
their original paths through symlinks, recovering 1.1 GiB for the repeat.
No shader cache was removed.

room-precompiled-five-repeat saved frames 1164, 2669 and 4082 over
300.024 seconds (9.73 FPS by frame advancement). First and last minutes
had 11 periodic reports apiece; their medians were 11.1 and 9.7 FPS,
90.3 and 103.3 ms/frame, and 30.2 and 31.2 ms/frame for copies.
The supervisor stopped the game after capture completion. No device loss.
There were 230 shader translation log entries during the observation.
The early skin probe found 492–561 tagged pixels at peak per sampled
frame and zero exact flat-color hits in those rows.

The first shot shows skin variation on the back, patterned shorts and a
highly reflective floor. The last shows facial hair, a warm but low-detail
torso, patterned curtains and shiny pale purple bedding. The idle camera
moved and the lighting and shot scale changed. The images establish what
was present after five minutes, but cannot isolate material improvement
over time or identify a particular shader as the cause.

A single short room trial used the same executable, vkd3d basename and
seed, with VICTORY_TEXTURE_KEEP=0. Around +58 s, total live host textures
were 4,678 MiB vs 5,390 MiB in the five-minute repeat, and title-filled
copies were 2,172 vs 2,884 MiB. In the short-retention run 5,587 textures
(136 MiB) were evicted during that minute and 3,324 (65 MiB) were recreated.
The control had no evictions or recreations. Room report medians were
11.45 vs 11.1 FPS. Scope memory at +60 s was 11,681 vs 11,634 MiB.
Short retention reduced live texture bytes, with no demonstrated process
group memory or frame-time benefit. Keep the default. Run and evidence:
room-precompiled-keep-short, frame 1160 and its adjacent system TSV.

The archived harbor replay precompiled-harbor-1795671480 subsequently ran
with the original victory.exe basename and the expected driver cache. All
89 observed host Present calls succeeded over 12.813 s (6.95/s, p50 149 ms,
p95 213 ms). Queue depth changed from one to two. The supervisor stopped
at the requested exit marker.

## 78. Paired material and final color samples from the bedroom

Seven existing captures contained a full first material attachment and a
same-frame final PNG. Sampled 210 interior back locations per image and
scaled pixel positions from 2560x1440 to 1920x1080 for the final image.
All material samples had alpha 29. The flat frame (room-wine-held-repeat,
2468) had one material RGB value in the sampled region and 34 final RGBs.
The similar camera in room-body-record-indexed (1169) had 139 material
RGBs and 90 final RGBs. At the same locations the second attachment had
209 and 210 distinct RGBA values respectively. The six detailed frames
had 118–144 distinct material colors. Resampling can affect individual
final pixels. This comparison localizes the older flat-color defect to
the material output or its inputs, before final lighting. It does not
identify the responsible texture and cannot generalize to bedding.

On the same flat frame, two rows on the bare lower back had 1 RGB across
136 tagged samples. Two rows on the shorts had 113 RGBs in 136 samples.
The subject's skin and shorts therefore need separate pass-level checks.
Shader def-use analysis of the exact body pixel program found the texture
index from material byte +28 feeds a sample whose R/G/B all reach first-
attachment color. The previously probed +32 supplies a different sample;
its R/G show no direct value path to first-attachment RGB. This static
analysis excludes branch dependencies and treats other texture operations
as leaves. Private scripts and data: team-diagnostics/room-material-light.

## 79. Separating graphics queue time from the driver's GPU-busy counter

On the precompiled RX 7900 XTX bedroom build, the standard launcher sets
VICTORY_CARD_AWAKE=1. This submits repeated small compute dispatches on a
separate queue between rendering submissions. In the first minute of the
section 77 repeat, 215 of 239 system samples reported 100% GPU busy and
the others 99%. Over 57 queue reports the tracked queue by fence was 37%
busy in median, with 56.3 ms/frame without pending work, including 42.3 ms
while preparing submissions. Those counters overlap. Copy preparation took
30.2 ms/frame across the eleven longer frame reports.

room-card-spans added VICTORY_CARD_SPANS=1 to the matched run and used one
shot at world +210. With the normal submission path and two GPU timestamps
per submitted group, the 13 complete room reports had median 29.2 ms
between submission start and end, 57.4 ms between submissions, 17.4
groups and 86.9 ms/frame (11.5 FPS). The stamp intervals exclude the
separate keep-awake compute queue.

room-card-profile enabled per-command timing with the default light
breadcrumbs. This disables deferred waits but produces no command labels.
Its 5.4 FPS is diagnostic overhead. Repeating with full breadcrumbs is
required to rank commands and changes the timing of the normal renderer.

room-command-times used full breadcrumbs and GPU_TIMING. In the six
complete post-shot room reports, median frame time was 317.7 ms and the
median sum of timed commands was 37.75 ms/frame. Per-command means included
2.33 ms for all alias copies, 1.58 ms for compute 5cf00dc39042edaf,
1.15 ms for indirect compute 87df99711607dd60 and 1.10 ms for compute
34fb51b8a0312e82 (screen probe temporal resolve for reflective caustics).
The ranking applies to this serialized diagnostic path. No single measured
shader explains the normal queue gaps. Do not add timings across these
two runs. Both stops were supervisor time limits after reaching the room.

## 80. Following the material record's RGB sample to a stable descriptor

The opt-in color-slot probe keeps the ordinary material identity check's
heap, descriptor-write counter, submission fence and GPU word validation.
VICTORY_BODY_IDENTITY_COLOR=1 uses record byte 28 and GPU readback word
11. Without that switch the probe uses byte 32 as before. The shader and
its translation are unchanged. Diagnostic executable SHA-256 starts with
ee54996e1df4. Patch in team-diagnostics/room-material-light.

In room-color-slot frame 1157, event 13 changed 519 alpha-29 skin pixels
on target row y=880. Those pixels had 242 material RGB values, zero exact
flat signature hits and 165 RGBs at corresponding final-image coordinates.
The CPU-predicted slot and GPU-read slot both equaled 36253. The version
stayed at zero through completion of the owning fence. The resource and
SRV used format 99 (BC7 sRGB), 512x1024, eight mips. Events 11 and 12 in
the same chunk used different slots and changed no pixels on this row.
This is a detailed-skin reference, not a sample of the old flat fault.
Evidence in room-color-slot/frames/raster-1157-s6086-l10-c45.slot.txt and
its same-stem .tsv, .body.txt and .rgba and frame-1157.png.

## 81. Keep-awake crossover in the stationary bedroom

room-clock-turns used the archived section 76 binary, indexed server,
unchanged room capture recipe, CARD_SPANS=1 and VICTORY_CARD_AWAKE=turns.
The mode alternated every 20 seconds. Three complete post-shot epochs per
state were available. Trimmed one second off both edges and only included
reports starting and ending inside a remaining interval.

| Median | Helper off | Helper on |
|---|---:|---:|
| System GPU busy | 48% | 100% |
| GPU timestamp work (ms/frame) | 39.5 | 31.3 |
| GPU timestamp gap (ms/frame) | 67.9 | 76.9 |
| CPU submit worker (ms/frame) | 73.9 | 73.5 |
| Periodic FPS | 9.4 | 9.1 |

There were 47/48 complete GPU time reports, 211/214 system samples and
7/8 periodic FPS reports off/on. Keeping clocks active shortened queued
GPU work in this run while the gaps grew; the camera and scene continued
to evolve. These are medians of reports. No FPS improvement from changing
the default is established. The six intervals and analysis script are in
team-diagnostics/room-material-light/clock-turns.txt and room-clock-segments.py.

For the CPU lead, section 77 control reports 6–60 s after the first room
capture (51 reports) had median 20.4 ms of refresh processing in 46 flushes
per frame. The median count was 7,403 refresh requests, with 6,325 proven
to need no copy. The median per-report ratio was 85.3%. This does not
attribute 85.3% of refresh time to unchanged spans. Time the no-change
checks and work scheduling separately before changing their semantics.

The subsequent room-color-slot-control used the previous non-indexed Wine
control with the same color-probe executable and four requested sparse
row offsets. Three completed frames (+210,+240,+270) had maximum tagged
counts 226, 203, 194 and zero exact flat-signature matches. Offset +180
yielded no completed row. Frame 2352 shows back variation and patterned
shorts. No flat-trigger descriptor was retained. The supervisor stopped
the run at the intended world duration. The negative sample is insufficient
to attribute the intermittent issue to either Wine variant.

## 82. Cost of checking unchanged buffers in a warmed bedroom

room-buffer-check-profile sampled about one call in 32 to Shadow::nothing_new
on each worker thread. Between 2 and 60 seconds after the bedroom screenshot,
54 reports covered 415 frames and 102,082 samples. Of those, 87,299
returned unchanged in the write journal, 10,129 returned false there,
4,654 returned false earlier and none returned true earlier. Weighting
by frames and the sampling rate gives 5.63 ms/frame before the journal
(including shadow metadata locks) and 10.20 ms/frame inside the journal
check. These are sums over threads; they include blocked time and do
not measure frame latency saved by changing a check. Runtime defaults
remain unchanged.

## 83. Restoring the ray-tracing builder in the bedroom

In three earlier runs, the last twelve reports consistently attributed
the persistent missing-pipeline counter to the disabled RT builder. The
median in each run was 304–333 skipped builder dispatches/frame. The
same counters include both compute and graphics. The intermittent flat
skin captured in section 78 was already flat before final lighting.

In room-bvh-real the archived executable reported all 53 builder PSOs
available and activated the builder. At scene entry, the descriptor
allocator also reported a failed table request for 29 contiguous slots,
and up to 1,384 refused requests/frame in the reported intervals. A
generic indirect prepare failure has several possible causes. At world
+180 only a sparse row was saved before the 3 GiB host reserve stopped
the run. It did not reach the comparison screenshot. The separate
VICTORY_WINDOWS_BAR_MB setting allows CPU-writable title-memory windows
in local VRAM. Check the driver support report, room marker, host reserve
and image before concluding that this combination restores lighting.

room-bar-control and room-bar-bvh both used a 4,096 MiB limit on BAR
windows, confirmed GPU_UPLOAD support, the matched seed and identical
indexed server, binary, room offset and capture options. Only the builder
permission changed. Both completed the requested world duration.
The +210 shots were frames 1159 and 1156 respectively. Both show the same
doorway and planks. The BVH run shows more legible wood, a brighter
detailed back and window detail. In the shared unobstructed doorway floor
band x=[1497,1896), y=[831,873), 16,758 pixels, median displayed luma was
220.8 vs 92.7 and the fraction at or above 240 was 7.53% vs 0%. Camera
pose differs; these are material-class comparisons, not identical texels.
The default ambient fallback stops when the RT builder runs, so changes
to global illumination and reflections cannot be separated in this pair.
Over the first room minute the medians were 11.5 vs 6.9 FPS, 87.1 vs
144.5 ms/frame and 30.2 vs 30.15 ms/frame of copy preparation. Median
reported missing host pipeline operations/frame fell from 314 to zero.
See checkpoints/7900xtx-lit-bedroom-20261126.md for the saved recipe.

The checkpoint replay lit-bedroom-1795673416 then rendered five minutes
without input (frames 1176, 2332 and 3452 over 300.094 s, 7.58 FPS overall).
First/last minute rolling medians were 7.9/7.85 FPS. The final front view
shows facial hair, torso shading, patterned curtain, pink bedding and
directional light. 128 translation log entries followed the initial shot.
The supervisor stopped after the captures. No device loss was recorded.

Across the first minutes of room-bar-control vs room-bar-bvh, the submit
worker's median time increased from 70.35 to 133.2 ms/frame. Median
waiting for outstanding GPU writes before filling upload memory increased
from zero to 55.4 ms. Copy preparation stayed at about 30 ms. In 57
fine-grained BVH reports, the 0xd60-byte fill against a declared 0x2160-
byte write range accounted for 40.6 ms/frame averaged over those reports.
The other large group concerns a 0x1e0-byte fill inside a declared
0xae00000-byte range. These are declared write ranges, not measured bytes
actually modified. The reported refusal concerns order relative to a
copy in a list. Never skip the producer/consumer dependency.

## 84. Bedroom floor zero tiles and journal cross-check

With the fallback lighting, room-floor-tiles captured world +240, frame
1194. The probe zeroed 16x16 regions of the screen-space reflection at
four rows on the left and right. Thirty of 96 resource/pair observations
had nonzero input reduced to zero; all paired control samples retained
their values. On the visibly bright left-hand floor, for instance, at
final x=684,y=786 the treated patch still had median RGB (239,243,247)
vs (241,245,248) in its neighbour. Other pairs were darker or brighter
than their controls. Disabling this single frame's local SSR contribution
leaves a bright floor; source radiance, temporal history and later lighting
remain possible contributors. The probe is diagnostic only.

The same run compared the optional per-group journal epoch bound against
the existing exact journal while returning the exact answer. 2,637,842
candidate checks agreed, zero mismatches. Another 5,083,476 checks needed
the exact path. This verifies the observed decisions in this run, with no
end-to-end performance claim.

## 85. Contiguous dynamic descriptor allocation at the lit harbor

At the first refusal in room-floor-tiles the owning heap had 1,245,184 title
slots and 851,967 dynamic slots, filling the available range below its
null slot. A request for 181 contiguous slots saw 18,934 free, with the
largest run only 135. Raising the setting above its current cap cannot
change that heap layout. The opt-in VICTORY_DYNAMIC_BEST_FIT=1 test adds
a length/start-ordered index alongside the indexed allocator. Splits and
merges update both indices; the old next-fit policy remains the default.
Three stand-alone Rust tests passed including the mixed-extent regression.

lit-bestfit-{a1,b1,b2,a2} ran sequentially, same new binary ded125ac80f1,
4 GiB BAR windows, BVH enabled and same precompiled seed. Actual successes/s
by harbor markers were 5.942, 6.227, 6.163 and 5.994. P95 intervals were
227.67, 217.72, 223.46 and 224.81 ms. The two high-load periodic allocation
reports per run (>20,000 tables/frame) had frame-weighted refused counts
of 285, 631, 0 and 424 per frame respectively. In B1's first refusal
only 727 slots remained free and the largest run was five. The new policy
used the pool more densely before its first refusal but did not reliably
eliminate refusals. Keep it experimental. Changing which draws are dropped
also complicates interpreting a small FPS difference. No visual preset
was changed.

room-lit-placed-order tried the existing VICTORY_PLACED_ORDER=1 on the
archived lit-bedroom recipe. It entered the world but the periodic frame
logs ended 23 seconds after the marker. No world +210 image arrived
by the 105-second world deadline. During the stall gdb observed the main
thread in Wine's NtWaitForSingleObject, and 156 of 157 sampled task wait
channels were futex, ntsync or poll. Zero scope OOM kills. The supervisor
deliberately stopped the scope at its deadline. No valid room FPS window;
keep the change disabled. These stack samples do not identify the lock
owner or prove the ordering switch caused the stall.

## 86. Bedroom queue stamps, shader command times and demand paging

On the archived AMD bedroom executable, the top-five wait reports over
the five-minute replay tell a different story as the camera moves. In
lit-bedroom-1795673416 the median total wait for outstanding GPU work
was 14.3, 12.0, 11.2, 20.9 and 18.4 ms/frame in successive minutes.
The visible contribution from the 3,424-byte fill was 12.7, 8.4, 0.0,
0.0 and 0.0 ms/frame. An absent entry contributes zero to this last
measure, so it is a lower bound. Each median uses 53–55 periodic
reports, excluding the first two seconds of each minute. In the earlier
room-bar-bvh first minute the corresponding medians were 56.3 and 39.4.
The transfer's wait is scene-dependent and can overlap the GPU work.

room-lit-queue-stamps used the checkpoint recipe, 4 GiB BAR windows,
the BVH builder and queue timestamps. Across 11 complete periodic
reports starting more than six seconds after the +210 room image,
medians were 125.1 ms/frame, 17.4 submissions, 85.7 ms from start to
finish of GPU submissions and 33.9 ms between submissions. Separate
medians do not sum to frame latency. The +210 image shows a shaded
back, patterned shorts, windows and reflective planks.

room-bar-control-stamps used the same binary, BAR budget, seed, room
offset and timestamp mode, with the builder off. Thirteen reports had
medians of 10.9 FPS, 91.8 ms/frame, 24.3 ms of queue work and 66.6 ms
between submissions. The matched lit run above was 8.0 FPS. Enabling
the builder also turns off fallback ambient lighting. This pair isolates
the cost of that combined rendering path.

In the separate serialized room-lit-command-times profile, 25 complete
bedroom reports had a median 134.6 ms/frame across timed GPU commands.
The median of each of the five largest individual passes was 36.16 ms
(sr_tracing, 6d7fe7745ab9c6d8), 22.23 ms (screen-probe trace,
f22a76fcb967ee59), 14.50 ms (primary probe gather, 3592426e3e9142d8),
10.96 ms (irradiance-cache gather, b58eb3ca6ee46427) and 9.08 ms
(alpha gather, 36f04a4ed0ad1d9b). Names are from the local archive
hash inventory. Full per-command profiling changes submission order
and pacing. Its times rank shaders under instrumentation and cannot
be added to the regular 125.1 ms frame. The five passes were active
in all 25 reports.

room-lit-identify-writer instrumented declarations overlapping guest address
0xe0c6ddb00 after clamping to the shadow's extent. Both observed call sites
declared 3,424 bytes starting at that exact address. They were GPU copies
in host/card_steps.rs and host/place.rs. This establishes the producer
classes that can cause this wait. Recording declarations once per site
does not identify the last writer before any particular fill.

room-lit-budget-4096 doubled the aggregate budget for successful page
fault mappings, retaining the 128-per-frame limit and memory checks. Same
instrumented binary 353a4b6057e2, seed, BAR and BVH setup as the preceding
control. Both reached the room. Median periodic reports after +210 showed
7.55 vs 7.5 FPS, 132.6 vs 133.6 ms/frame, 760.5 vs 916 MiB of pages and
zero missing host-pipeline drops. The reported missing pages remained
variable. Both pictures show back shading and patterned shorts, with
different camera poses and lighting. The local repository backup started
during the experimental run and could influence its timing. This pair
did not establish an FPS gain. The default stays at 2,048 mappings.

AMD repository PR 87 (1ef70513704c2e4578a6d84e93edd65cb95072f3)
corrects the array slice supplied as a spatial coordinate to calculateLOD.
Wine, Microsoft DXIL validator from DXC v1.8.2505.1, current AMD fork:
the 1D-array and 2D-array tests both failed before the change with the
out-of-bounds coordinate error. Both passed after. The upstream commit
patch is in checkpoints/7900xtx-array-lod-1ef7051.patch and applies to
the AMD source tree. This test establishes DXIL correctness for these
queries; it does not identify a particular material that uses them.

Repository preservation during this series: backups/repos-20261126-0935
contains 89 restored and fsck-verified bare archives of project mirrors,
working repos and alternate cached versions. Fresh public branches/tags
were available for 24 of 26 project sources contacted. CGBJ and ztrx
were preserved from local copies. The two Wine builds are additional
shallow local source snapshots. Source history containing upstream save
archives or gameplay images was filtered; original ref IDs and excluded
paths are recorded. All 278 SHA-256 entries passed locally and on
the designated backup server in the directory of the same name.

The first room-lit-array-lod run with the previous build's cache gave
2.9 FPS and 343.5 ms/frame in four late room reports. Its +210 image
was nearly black with bright windows, with many pipelines still being
made. On replay from that run's cache, room-lit-array-lod-warm gave
8.0 FPS, 125.4 ms/frame and zero median missing host-pipeline drops
over 21 reports. Frame 1146 shows hair, skin shading, patterned shorts
and a lit doorway. The supervisor stopped at the scheduled 150 seconds
in-world with zero scope OOM kills. This confirms warm-cache operation.
One pair cannot establish an FPS gain from changing the LOD operand.

## 87. Cold and warm shader coverage by identity in the bedroom replay

In room-lit-array-lod, 2,341 distinct shader hashes were bound at least
once. For 57 there was no successful translation or host pipeline record
before termination. In room-lit-array-lod-warm all 57 had a host pipeline.
All 2,457 unique hashes bound in that replay had both a successful
translation and a host compute, graphics or mesh pipeline somewhere in
the log. These sets do not establish availability at first use, every
pipeline state combination, or correctness of shader outputs.

After the room screenshot, 143 identities first appeared. In one of
22 periodic reports there were drops (38 without a host pipeline and
60 indirect per frame). At 08:45:39 UTC 99 previously unseen shader
hashes were first bound. The subsequent 373.6 ms/frame report also had
43 pipelines being made and 92 ms/frame in the heap-buffer pass. The
other 21 room reports had no drops in either counter. This isolates a
transient workload distinct from the regular cost of rendering the room.

room-lit-array-lod-repeat ran the same executable, preset and deadlines
from a reflink of the first warm run's caches. After the +210 image and
six-second exclusion, the median of 20 reports was 135.65 ms/frame
(7.4 FPS), and the slowest report averaged 189.4 ms/frame. Only one
reported dropped work (2 missing-pipeline draws and 1 indirect per frame).
The earlier warm run had 125.4 and 373.6 ms respectively. Shaders used
differed: 86 new identities first appeared in the room in the repeat,
65 of them seen somewhere in the previous run. Its image shows hair,
skin shading and floor reflections from a different angle. A single pair
does not prove a cache-induced improvement in latency.

Of 2,477 unique bound hashes, 2,476 had translation logs and 2,475 had
host-pipeline logs. The outstanding mesh 29545c7adbb4e5d4 and pixel
69ef3d2231f5f202 were bound in the last frame; the mesh translation is
the final log line. The supervisor stopped at the 150-second world
deadline. Their host-pipeline creation remains unverified in that run.

## 88. Verification of the last bedroom shader pair

The earlier five-minute lit bedroom log records both stages bound together
on frame 2127 and the matching host mesh pipeline created (line 17406).
On the current LOD-fixed binary 30ce8b4e3f5b, the repeat run's recorded
pipeline book contains one recipe combining both hashes: index 73150,
key 4754ea36b84b4e51a6acee25bc77bb26. It was absent from the early-use
list. Replaying just that recipe on the RX 7900 XTX translated both stages
and produced the host mesh pipeline in 420.87 ms. Together with the repeat
log this confirms host-pipeline coverage for every bound shader identity
in that run. It does not time an in-game draw or validate its pixel output.

Separately, warm --used using that build and a fresh clone of the repeat
cache found 83,565 recipes and 2,220 distinct early-use keys. 2,214
successfully built in eight seconds with four processes, no refusals and
no worker crashes. Six keys with historic 180-second timeouts were excluded
by the warmer. The main process exited successfully; the service manager
later timed out shutting down a Wine helper. Neither test measured FPS.

## 89. A stale timeout list suppressed six valid bedroom pipelines

The old warm command read the first token in warm-bad.txt as a key
irrespective of the translator and driver that produced the timeout.
All six excluded used recipes created host pipelines on the current
RX 7900 XTX, one fresh worker per recipe (208–287 ms each). Indices
in the recorded book were 50818, 51600, 51601, 51602, 51603 and 51785.

checkpoints/7900xtx-warm-fingerprint.patch uses the existing translator
and driver fingerprint for new timeout records. Old unversioned failures
are eligible to retry. Workers now record successful host pipeline
creation before marking a recipe done. On a copy of the original cache,
the normal warm --used command reused 2,214 prior successes, selected
all six exclusions and created all six in one second. All 2,220 used
recipe keys now have successful creation records for fingerprint
655be648a7c7d9cc. The book includes other recipes the scene did not use.

The six recipes are compute passes for laser lighting shadows, distant
directional shadows, their temporal upsampling and checkerboard shadow
masks. Their names come from the archive hash inventory. Exclusion from
offline warming did not prevent their creation during normal execution.

room-fingerprint-ready ran the lit bedroom configuration with the updated
cache and binary 391e175c6a37. All 2,432 distinct bound hashes had logged
translations and host pipelines. Nine room reports after the six-second
exclusion had median 112.9 ms/frame (8.9 FPS); the slowest interval was
180.3 ms/frame. One interval reported indirect drops. The earlier repeat
had 135.65 ms/frame over twenty reports. The new run was stopped by the
3 GiB host RAM reserve about 80 seconds after entering the world, with no
scope OOM kills. These unequal windows do not establish a steady FPS
gain. Frame 1176 shows back shading and hair, a bright doorway and
reflective floorboards.

## 90. cool_fox Windows fork and early release of descriptor tables

Inspected carlm125/cool_fox main f4b48ad766ddbec1a1867022cd42f7645b0dd55e.
The launcher selects Series S; the README's Series X RX 7900 XTX table
is explicitly about the upstream AMD fork. The fork journal reports an
RTX 5090 opening-scene median of 64.2 to 54.8 ms across 15 heavy caption
intervals per build. Capture and scene progression differed. These are
not measurements of the AMD Linux setup. The fork uses optional Wave32
lowering, staged memory-pressure recovery, special material-bake uploads
and earlier reuse of dynamic descriptor slots. The transmission prefilter
2da4beb386509b77 already translates and gets a host pipeline in the local
room-fingerprint-ready log. This does not validate its output.

Ported the descriptor release from codec_d's f4b48ad into the scratch AMD
tree behind VICTORY_RELEASE_COMPLETED_TABLES. Both completion paths now
can return dynamic tables from detached lists after GPU completion while
retaining the previous delay for recycling command allocators. Patch:
checkpoints/7900xtx-cool-fox-completed-tables.patch. Local and backup-server
copies of the complete source bundle are under cool-fox-20261126, SHA256
e8cefe7153badcf7b60b73f01bd626e1696234f9b0e100aa19eeb111ede517c8.

room-cool-fox-tables-0 and -1 ran sequentially with the same binary
25c23cdf857b, warm-fingerprint-pass1 cache seed, enlarged descriptor
pool, 4 GiB BAR and active BVH builder. Only early table release changed.
In the first three reports above 20,000 table requests/frame, the
baseline refused 300, 1,467 and 1,706 allocations/frame; the candidate
refused zero in all three. No candidate report recorded a refusal or a
list rerun after table release. Between the same two harbor markers both
presented 88 frames. Throughput was 6.03 vs 6.37 FPS and P95 present
interval 233.45 vs 215.91 ms. Over 13 later bedroom reports each the
medians went the other way: 8.4 vs 7.5 FPS and 119.3 vs 132.8 ms.
Neither scope recorded OOM. The two world +210 images show hair, skin
and patterned shorts; pose and apparent skin tone differ. Keep this
an opt-in descriptor-pressure fix; the single pair does not establish
a general frame-rate improvement.

## 91. Reusing box bounds in the ray-intersection shader translator

The five costly compute shaders in §86 contain respectively 7, 5, 8, 8
and 4 static BVH64 intersection instructions. Those counts do not measure
how often each instruction executes. The experiment shares the first 12
bounds words between the half- and full-precision box decoding paths.
The additional 48 bytes for full-precision boxes retain the original
address lookup. The reference and candidate each validated all five shader
translations and created all five compute pipelines. The candidate passed
all six intersection tests under Wine on the RX 7900 XTX.

RADV compiled the following code sizes (bytes) with its shader cache
disabled. These are the five compute shaders, excluding driver helpers.

| Shader | Reference | Shared bounds |
|---|---:|---:|
| RaytraceVBuffer | 191784 | 182632 |
| Screen probes | 94880 | 88416 |
| Primary probe gather | 172768 | 162144 |
| Irradiance gather | 168580 | 158276 |
| Alpha gather | 77100 | 71852 |

All ten compilations reported 96 VGPRs, 108 SGPRs and zero spilled VGPRs.

Serial Proton/Wine runs room-bvh-box-reference and room-bvh-box-shared
used the indexed server, 4 GiB BAR, enabled BVH builder, 2,097,152-view
heap and GPU queue timestamps. Both started from copies of the same seed
and ran warm --used first. The reference reused 2,220 successes. The
candidate rebuilt all 2,220 recipes in 65 seconds, with zero failures.
Both supervisors stopped the game after 105 seconds in the world. Neither
scope recorded an OOM kill. Each run captured one image at world +210.

Between the first two harbor markers the reference presented 89 frames
at 6.07 FPS (P95 interval 228.05 ms) and the candidate 88 at 6.37 FPS
(P95 234.21 ms). Across complete later bedroom reports (12 vs 11), median
FPS was 8.65 vs 6.8 and frame time 115.8 vs 147.7 ms. GPU submission
time was 80.4 vs 110.3 ms, gaps 34.15 vs 34.8 ms. Both +210 images show
hair and back shading, but the reference faces a window and the candidate
faces the interior doorway. Workloads differed: 2,343 vs 2,402 bound
shader hashes. Every bound hash had a translation and a host pipeline.
The photos and logs do not establish pixel correctness of all materials.
The smaller code has no validated FPS benefit. Keep the patch experimental.

A separate follow-up patch reuses the first page mapping when the second
half of a wide box remains on that page. It applies on top of shared bounds.
Seven intersection tests passed on RADV, including a new test covering
missing and noncontiguous pages, the last two aligned offsets and 4 GiB
carry. In-game performance of this follow-up is unmeasured.

## 92. Native console profile and the second BVH page lookup

The comparison uses the same RX 7900 XTX, indexed Wine server, 4 GiB BAR
budget, enabled BVH builder, 2,097,152-view heap and queue timestamps as
§91. Each isolated run starts from the same save copy and stops after
105 seconds in the world. Each picture is world +210. Room medians below
use periodic reports from 6 through 55 seconds after the picture.

The previous shared-bounds run faced the doorway and submitted 9,020
draws/frame, versus 6,488 in the window-facing reference run. Two older
images with those respective views correspond to 8,846 and 6,518 draws.
Thus comparing equal frame offsets alone does not isolate shader cost.

With the additional same-page patch and Series X, room-bvh-page-v1 faced
the doorway (8,951 draws/frame). Frame time was 108.3 ms and GPU submission
time 64.45 ms, versus 147.7 and 110.55 ms in the shared-bounds doorway run.
The screenshots differ in illumination. Both activated 53 builder programs;
all five costly tracing shaders had host pipelines. Median reported missing
page entries/frame were 89.7 vs 22.4. Older doorway runs ranged from 54.7
to 110.4. Those counters have no per-shader identity or hit-rate attribution.

The Series S test passes the existing --console series-s launcher option
to the game, using the same reference binary as Series X. The upscaler
DLL was loaded as a title DLL and bound with zero missing imports. The
guest recorded a 1920x1080 viewport in S where X recorded a 2560x1440
viewport. Both also used 3840x2160, 1280x720 and square viewports. This
records distinct viewport sizes, not the dimensions of every dispatch.

| Native profile and build | Harbor presents | Harbor FPS | P95 present interval |
|---|---:|---:|---:|
| X, reference | 89 | 6.07 | 228.05 ms |
| S, reference | 89 | 7.73 | 195.15 ms |
| X, shared bounds plus same-page lookup | 89 | 6.12 | 224.99 ms |
| S, shared bounds plus same-page lookup | 89 | 7.31 | 183.75 ms |

The interval uses the same two marker hashes across profiles. Series S
improved harbor throughput with both compilers. The shader change alone
did not give consistent harbor throughput. No change to the default.

In the two Series S window views, the left/right background edge cosine
scores were 0.914/0.853. The reference and page candidate had 5,871 vs
5,938 draws/frame across nine room reports each. Median frame time was
96.2 vs 93.0 ms, GPU submission time 69.8 vs 55.55 ms. Across 13 complete
reports the candidate spent 36.6 ms between submissions. The X reference
had 117.1 ms/frame and 79.95 ms of GPU work in its nine-report window.
The Series S reference bound 2,260 distinct shader hashes and the candidate
2,268. Each hash had a translation and a host pipeline. Both images show
hair, skin and floor reflections, but the character's color and floor
brightness visibly differ. Neither Series S run recorded an OOM kill.

## 93. CPU preparation, submission grouping and refresh workers in Series S

In the 6–55 seconds after the +210 room capture of §92 (46 CPU reports per
build), the reference and page candidate respectively spent 18.55 and
23.90 ms/frame with the GPU queue empty while the submitter was working;
another 5.55 and 7.10 ms elapsed with no queued work while the guest had
not submitted. The submitter waited for the GPU for 15.60 and 8.15 ms.
List-step refresh took 21.90 and 22.85 ms of elapsed time; roughly 6,070
of 7,340 examined spans per frame required no copy. The concurrent
readback thread worked about 30 ms/frame in each. Its elapsed work is
not additive with the submitter or queue gaps.

room-console-s-join-study used the same page candidate and Series S recipe
and alternated between 1 and 8 joined parts every 15 s in the same launch.
Fifteen complete post-shot phases remained before the supervising shell
received SIGTERM (exit 143, scope inactive, no scope OOM). It did not
reach the scheduled five-minute room capture. The midpoint picture shows
the idle camera framing the character from the front. Across all seven
adjacent pairs the median on-minus-off phase mean was +5.3 ms. The four
pairs within 5% in draw count, all without a screenshot, gave -2.7 ms
in phase mean, -2.95 ms in phase median and -1.65 ms in phase P90. Keep
the existing maximum of eight; the switch provides a modest benefit in
the comparable phases. The game's idle camera changes its workload.

A 1 Hz schedstat sample over 89 seconds in the subsequent Series S study
measured victory-submit on CPU for 49.5% of wall time and runnable
waiting for a CPU for 2.8%. The relay used 15.3% and was runnable waiting
for 0.5%. These are per-thread scheduler counters, independent of GPU
queue timing. One-second wait-channel samples found the submitter in
futex waits, but do not identify the specific user-space lock.

In room-console-s-join-study, 35 stage combinations were first bound
between +83.08 and +88.57 s after the first room image. Seven already
had matching host pipelines. The other 28 appeared later, corresponding
to 29 used recipe keys in the completed run. The original book already
contained all 29, but the seed used list and its current-fingerprint
success records contained none. One mesh/pixel combination took 1.278 s
from first bind to the first matching host pipeline. At +88.6 s the
runtime reported 25.3 dropped draws/frame with pipelines being created.
The runtime does not log a recipe key for each dropped draw.

room-console-s-claim-study alternated atomic work claims of one and four
for refresh batches of at least 256 entries. Across 12 adjacent phase
pairs the median on-minus-off mean frame time was -1.5 ms, phase median
-0.15 ms and P90 +3.05 ms. Four pairs within 5% in draws and with no
screenshot had -1.5, -0.15 and -3.95 ms respectively. This does not
establish an improvement in overall pacing. The opt-in code was left
in the isolated tree. A second isolated code experiment combined the
two locks guarding per-buffer page and range stamps. Four targeted Wine
tests passed, including concurrent title writes and readback. Its effect
on FPS is unmeasured.

This five-minute capture run stopped normally after the +300 s image.
The three images show the back near the doorway, the idle camera facing
the character by the bed, then the face and hair in close-up. Every one
of 2,451 bound shader hashes had a translation and each of 1,943 bound
stage combinations eventually had a matching host pipeline. Two room
reports had pipeline-creation drops (0.2 and 1.2 draws/frame). A matching
stage combination does not prove every render-state variant or material.

The separate room-console-s-output720 run offered 1280x720 in the DXGI
mode query, verified in the title log. It still created the same
1920x1080 swapchain as room-console-s-page-v1 and recorded the same
3840x2160, 1920x1080, 1280x720 and square viewports. At the two harbor
markers it presented 89 frames at 7.70 FPS, P95 172.82 ms, versus 89
at 7.31 FPS and 183.75 ms. The short run ended with supervisor SIGTERM
about 68 s after launch and has no full room interval. A lower guest
display-mode answer did not demonstrate a reduction in render resolution.

warm-console-s-extra29 was reflinked from warm-bvh-page-v1. Only the
29 observed missing recipe identifiers were appended to its private
used list. The offline warm command reused all 2,220 prior successes
and created the additional 29 host pipelines in 2 s, zero failures.
All 29 have success records for fingerprint 84afa9f4affa17b9. For the
same 28 shader-stage combinations, two preceding runs without these
entries saw the first host stage match a median 1,019 and 1,134 ms
after first bind, respectively. Those times include asynchronous queueing.

room-console-s-warm29-move used the expanded 2,249-recipe seed. Of the
previously late 28 stage combinations, 18 appeared in this run. All 18
had a matching host pipeline before first bind; the other ten were not
exercised. Other recipes still produced two bursts of asynchronous drops.
The game remained running through the scripted movement and camera inputs
and was stopped after the final planned room image. The final picture
shows the character in the adjoining area, along with a bright purple
light. No reference image establishes the intended color there.

Combining distinct used recipe keys from the join, work-claim and movement
runs produced 2,628. The older book was missing 261 of their recipes.
The most recent movement run's 88,596-recipe book contained all 2,628,
so the new private cache was cloned from that run. It needed 379 more
pipelines after the previous 2,249. All 379 were created in three seconds;
all 2,628 used keys have success records for fingerprint 84afa9f4affa17b9.
The reusable private seed is warm-console-s-room-2628. The exact Series S
binary (SHA-256 1556c0fc234297f0b3b2550fde855a2900cf7521ac5f2695ae97cb2d19aa488c)
and its serialized room replay are in team-diagnostics/series-s-room-2628.

room-console-s-union-move replayed the same scripted inputs with the
2,628-key cache and same binary. All 28 targeted stage combinations
were exercised and had a matching host pipeline before their first bind.
Across all 42 room reports there was one fractional pipeline-creation
drop report (0.4/frame at +112 s). The 2,249-key run had 20.8/frame and
2.0/frame in two reports. Each run was stopped by its supervisor after
the scheduled third image; no scope OOM kill. Skin tone and scene lighting
vary between shots, including a yellow-orange back in the first image
of the union run. Cache coverage does not establish material correctness.

The fixed early room window was 102.45 vs 100.7 ms/frame, 8,276.5 vs
8,355 draws/frame. Harbor marker rates were 7.29 vs 7.43 FPS and P95
present intervals 194.64 vs 196.61 ms. None demonstrates a sustained
FPS improvement. The union run bound 2,507 shader hashes, all translated,
and 1,975 stage combinations, all eventually seen in a host pipeline.

## 94. Buffer metadata lock and the Series S material probe

room-console-s-stamps-candidate and room-console-s-stamps-control ran
sequentially with unchanged Series S settings, the 2,628-recipe seed,
the same descriptor heap and VICTORY_BUFFER_CHECK_PHASES=1. The candidate
combines the per-buffer page and range generations under one existing
mutex. At the harbor, both runs presented 88 frames, with candidate vs
control 7.21 vs 6.72 FPS and P95 intervals 180.86 vs 211.04 ms.
The room pictures faced different directions and the corresponding
early windows drew 5,956 vs 8,245 times/frame. Their 89.2 vs 119.3 ms
therefore do not isolate the change.

Across 55 sampled room reports per build, the estimated sum of worker
time before the journal check was 1.84 vs 1.99 ms/frame, or roughly 234
vs 241 ns per examined span (160,500 and 124,229 samples respectively,
1-in-32 sampling). The journal itself cost 7.32 vs 7.23 worker-ms/frame.
This measured lock change does not explain the roughly 60 ms/frame still
needed to reach 24 FPS. The default executable is the existing checkpoint.

In the two preceding movement runs, the five complete periodic reports
inside the scripted input sequence (186–218 s after GameInputCreate)
had medians of 10.9 and 9.3 FPS, at 3,981 and 7,507 draws/frame. Identical
input schedules do not guarantee matching camera or scene workload.

The prior material probe required a 2560x1440 slot-zero attachment, so
the absence of Series S scan files was a diagnostic blind spot. The
updated probe positions its readback in target coordinates, supports
2560x1440, 1920x1080 and 1280x720, and records target size and row origin.
It also retains changing tagged material samples. A coordinate test passed
under Wine. The material remained rendered by the existing pipeline.

In room-console-s-material-row, the actual skin attachment was 1280x720.
At frame 1164 the tracked indirect body draw changed 255 pixels carrying
alpha 29 on row y=450. Their raw material RGB had 162 distinct values
and median (160,111,86). Scaled to the same-frame final image, the sample
had 177 distinct RGBs and median (103,80,76). Three later records had
152–167 distinct material RGBs and medians within one unit of the first.
The CPU-predicted and GPU-read texture indices agreed at 34511 and
the slot version stayed zero. This sample shows a detailed body texture;
it does not explain the separate orange-back image, or the head material.

An isolated translator experiment moved the BVH page lookup after the
guest EXEC guard. For the five costly compute shaders, 22 of 32 static
intersection sites were within 80 instructions of a mask change and
empty-wave branch. This is a static pattern, not a dynamic mask count.
Eight GPU correctness tests passed, including inactive-lane register
preservation and first/second page fault checks. All five real shaders
validated and created host compute pipelines. All 2,628 recorded recipes
were prepared successfully for the new fingerprint (69e834d17bb776ae).

room-console-s-exec2628 reached the room on the same indexed Wine
server. It presented 88 frames at 7.30 FPS in the harbor, P95 190.73 ms.
The earlier same-page build with the 2,628-recipe seed presented 89 at
7.43 FPS, P95 196.61 ms. The new window-facing room run had a median
111.45 ms/frame (88.25 ms GPU submission time and 6,015 draws/frame).
An earlier same-page window view with the smaller seed had 93.0 ms
(55.55 ms GPU time and 5,938 draws/frame). The output images differ in
illumination. No reason to change the checkpoint translator on these data.

The independent RADV comparisons disabled the Mesa and vkd3d disk caches
and compiled each of the five captured compute containers on both
translators. Every pair had the same CLI-emitted shader hash, RADV
compiler hash, instruction count and code size. In particular RaytraceVBuffer
was 188,968 bytes and 33,220 instructions on both. Thus the mask change
does not produce different compiled code for these five offline cases.
The synthetic masked-lane test exercises a case they need not use.

The repeat of the unchanged material probe captured the orange back at
frame 1162. The tracked body draw wrote 377 alpha-29 pixels on the same
normalized row, all exactly (181,135,111). The other three probes in the
repeat also had one material RGB across 367–387 changed skin pixels.
Same-frame final RGB at +210 varied across 225 values, median (165,127,78).
The first run and repeat each bound a stable 512x1024, eight-mip BC7
sRGB resource at record offset +28. The repeat reported predicted and
actual slot 36192, version 0 to 0. Slot numbers and object addresses are
process-local. Both sides had an actual descriptor; the data do not show
whether the texture contains the intended bytes or whether sampling
coordinates, mip selection or other material inputs are responsible.

Two subsequent room-console-s-body-content runs used the same binary and
2,628-key seed. In each +210 image the same body draw changed 375 skin
pixels on row y=450. The first had 194 material RGB values, median
(168,117,91); the second had one, (181,135,111). The title memory backing
address and numerical descriptor slot differed between processes, but
the source SRV words matched (first mip 0, eight mips, clamp zero,
component mapping 0x1688). The retained COM resource matched the host
copy; the CPU-predicted and GPU-read descriptor agreed in each run.

All 155 sampled BC7 blocks across eight guest mips were readable and
their per-mip hashes matched between runs. The last submitted source
digest also matched (41970910092250393369371863574544705696). No live
CPU staging object remained to sample. This locates the visible defect
before final lighting while ruling out a difference in the observed
+28 source blocks/view configuration. It does not establish which UV,
mip, or texel value the pixel shader actually consumed, its other material
inputs, or GPU texture residency. The source probe is separate from the
material-row patch. Both are optional diagnostics; the saved play
checkpoint was left intact.

## 95. Series S bedroom GPU command timings on RADV

The indexed Proton run room-console-s-cmd-timing used the Series S
2,628-pipeline warm seed, 4 GiB BAR setting, card spans, BVH builder,
GPU command timing and full DRED. Two complete reporting intervals after
the bedroom capture averaged 95.45 ms per frame of measured GPU commands.
The indirect visibility dispatch 6d7fe7745ab9c6d8 averaged 31.30 ms and
the light-probe dispatch f22a76fcb967ee59 averaged 15.75 ms. The next
three dispatches accounted for 8.50, 5.76 and 3.79 ms respectively.
Timing serializes commands. The 339.65 ms diagnostic frame time is
unsuitable as a play-mode baseline, and savings need an uninstrumented
comparison in the same scene. At 24 FPS the entire frame budget is
41.67 ms, and at 30 FPS it is 33.33 ms.

The CPU translator classified 32/32 BVH instructions across the five
timed compute shaders as memory-mode with no instruction-local EXEC
guard. Moving the guard did not change their DXIL. A second candidate
read each 64-byte triangle leaf in four fixed chunks before selecting
the vertices. It passed all seven AMD reference tests and created all
five pipelines. On all five the generated machine instruction count grew
by 2.46–3.52% and VMEM instruction count by 24–48. VGPR usage stayed at
96 with zero spills. This candidate was set aside without a gameplay FPS
claim; these static counts are not elapsed GPU time.

The CLI defaults to VICTORY_HOST_RDNA=0 while the rendering host
auto-selects RDNA float lowering on the applicable AMD wave range.
The static prefetch figures above describe that CLI configuration.

Repeated the 10 offline PSO builds with VICTORY_HOST_RDNA=1,
VICTORY_RDNA_FLOAT=15 and maximum wave width 64. All passed. Full-node
prefetch again raised machine instructions 2.49–3.49% on every shader,
with 24–48 extra VMEM instructions and unchanged registers/occupancy.
The offline heap and raw-view settings still differed from the game.

In the uninstrumented-for-per-command-timing union-move report at
09:51:15.458622 UTC, 8,341 draws took 100.7 ms/frame with zero missing
host-pipeline drops. Overall queue-idle time was 43.4 ms, including
34.5 ms while the submit worker was working. Refreshes took 26.1 ms
on that worker. The timeline overlap of refresh specifically with an
empty queue is unknown. Host preparation included 34.8 ms copying and
the concurrent return path reported 25.1 ms; these are not additive
frame savings. The single largest whole copy was the 21 MB range at
0x5cb50000, copied twice per frame for 5.12 ms. 321 pieces of the
range were both uploaded and fetched during the traffic sample.

An earlier frame-2164 command trace in goal-fork-copy-grid1m ties the
three RaytraceVBuffer calls to different surfaces. The nearest named
prepasses are water (root word u6=2), opaque (u6=0) and alpha (u6=1).
They bind different UAVs and reuse the same indirect argument address
at separate points. The Series S bedroom command timing did not capture
this root word, so its largest individual call cannot yet be assigned
to one of these surfaces.

## 96. Bedroom skin detail and the UV received by the body pixel shader

A strip probe kept the exact body mesh/pixel pair and replaced only RGB
on one 640-pixel material-target row. It preserved alpha, included an
original-color lane, validated with Microsoft DXIL validation and created
the recorded mesh PSO on RADV. Series S, indexed Proton, same enlarged
descriptor heap, same 2,628-recipe seed. The strip was observed in actual
draws; measurements below are across complete adjacent eight-pixel groups
on the body, so each diagnostic represents a neighboring pixel.
At world +210, room-console-s-body-strip-1 had 29 groups with 29 distinct
encoded fractional UV pairs, 29 albedo samples and 29 final material RGBs.
room-console-s-body-strip-4 had 46 groups: fractional UV encoded as (0,0)
throughout, albedo (185,143,125) throughout, and material (181,135,111)
throughout. Across all four measured draws in that bad run, the albedo
sample and fractional UV stayed constant, even when final material varied
slightly. The +28 texture lookup uses interpolated TEXCOORD0.xy with
implicit LOD. The branch selector stayed zero in both pictured draws and
the blend weight was zero throughout the flat sample. This places the
first observed loss of spatial variation at the UV input, before lighting.
The strip encodes fractional coordinates to 8-bit sRGB; this does not
establish the original floating-point values or why the coordinates vary
between launches. Trace TEXCOORD0 back through the mesh stage.

Run 3 also showed detailed skin. Run 2 was stopped before capture by the
3 GiB host-RAM reserve, with no scope OOM kill. Finished temporary build
outputs were relocated from tmpfs to private disk before runs 3 and 4.

## 97. Direct overlap between refresh and an empty queue in the bedroom

room-console-s-refresh-idle used indexed Proton, the Series S 2,628-recipe
seed, 4 GiB BAR, BVH builder, card spans, five refresh workers, enlarged
descriptor pool and VICTORY_REFRESH_IDLE=1. Command serialization was off.
The overlap probe measures submitter refresh scopes and one host fence
observer with a common monotonic clock. Its reporting-boundary test
passed under Wine. Over 63 reports starting at least five seconds after
the +210 bedroom screenshot, the median refresh elapsed time was 24.75
ms/frame, the median overlap with the observed-empty drawing queue was
8.24 ms, and the median total observed-empty time was 28.01 ms.
Across 12 longer frame reports in that window the medians were 94.85
ms/frame, 10.55 FPS, 5,885.5 draws/frame and zero drops for lack of a
host pipeline. Medians from different reports are not additive.
The overlap measures exposure of the refresh phase and is not a
speedup prediction. Refresh work can also compete with other work.
One screenshot, scheduled supervisor termination 120 s after the
world marker, zero scope OOM kills. The playable checkpoint is unchanged.

## 98. Mesh UV backing measured in five Series S bedroom launches

The body texture's TEXCOORD0 originates in the mesh shader's root SRV
t3, space1. An opt-in pass reads back the same list-owned bounded raw
view just after the indirect mesh draw. The synthetic AMD test covers
packed and float-pair formats and an out-of-bounds read. The pixel strip
remains active to classify the observed draw. Identical seed, executable,
indexed Proton server, BVH, descriptor heap and 4 GiB BAR across runs
room-console-s-mesh-uv-1 through -5. Each saved one image.

All five runs produced detailed skin, including the camera view with
46 complete body groups. Across 18 paired draws the encoded fractional
UV had 29–46 distinct pairs per row. GPU header 0x03543c28 selected packed
format 8, stride 4 and base byte 436344. Scale 0x40800000 means 4.0.
Every one of the twelve fixed GPU words at each observation agreed with
the corresponding CPU source word read during recording. The smallest
view was 1,900,544 bytes; at that layout all 65,536 possible u16 vertex
indices are in range. These were fixed sampled indices, not the actual
visible triangle indices. These measurements do not establish the t3
contents during a flat run or a repair. A camera angle alone does not
identify a bad draw. The prior flat run remains valid evidence at the
pixel shader input. No runtime change was promoted.

## 99. Per-surface ray visibility on the actual Series S room frame path

room-console-s-vis-timing ran the indexed Series S recipe using the
2,628-key seed, 4 GiB BAR, BVH builder, full DRED, GPU_TIMING and
VISIBILITY_TIMING. The verified root constant in slot 2 distinguished
three dispatches of the same visibility shader. Earlier command-stream
prepass names associate values 0/1/2 with opaque/alpha/water. Over nine
complete bedroom reports taken after world +210, each label accounted
for one command per frame. Median GPU command times were 7.61 ms opaque,
13.80 ms alpha and 2.44 ms water. The corresponding 15 complete earlier
world reports were 6.74, 7.48 and 2.32 ms. Serialized diagnostic mode
changes frame pacing; no gameplay speedup is claimed. Earlier estimates
that assigned the longest call to opaque from slow-list samples alone
were not justified. The supervisor stopped normally after 150 world
seconds. The surface labels describe the work item; the alpha pass here
does not exclusively identify hair, floor reflections or cast shadows.

## 100. Series S floor reflection reconstruction and shadow probe coverage

room-console-s-light-chain reached the bedroom under indexed Proton with
the same 2,628-recipe seed, enlarged heap, 4 GiB BAR, BVH builder and
Series S console profile. Four targeted Wine tests passed. Frame 1205
(+240) yielded one 960x540 PNG and a 6.5 MB partial recording-time
command list. Reflection readbacks were tagged 1206, including two
observations of the opaque temporal chain. Counters label CPU recording
windows; these samples cannot be claimed as exact pixels of PNG 1205.

The opaque reflection source was 640x360 R11G11B10, host resource 25067.
Spatial filtering wrote a 1280x720 R11G11B10 surface (25666); temporal
opaque wrote two alternating 1280x720 R32_UINT surfaces (471, 473).
The two left point pairs were on bright floorboards in the neighboring
photo. At normalized (456,524)/1280x720, source linear HDR RGB was
(26,32,43) and (24.5,30.75,41) in the two observations; the spatial
output was (20.25,28.25,41) and (18.5,27.25,41). At (472,548) nearly
black input in one observation became (21,28.25,40) in the spatial
output. This identifies spatial redistribution, not a defect by itself.
At the former point the opaque temporal output, interpreted as RGB9E5
based on an observed 42->67 alias path, was roughly (7.5,10.7,15.5)
and (8.1,11.6,17.0). The final consumer of that specific packed texel
was not observed. The bright input is present before temporal filtering.

The floor photo's two left points (342,393) and (354,393) had displayed
median RGB (219,226,234) and (216,225,234) in 5x5 regions. The right
points were wall, not shaded floor. The selected ops window did not
contain a directional trace/upsample, leak-plug or RaytraceVBuffer
dispatch. No matching shadow samples or dispatch dimensions landed.
No conclusion about missing shadows follows from their absence in this
CPU recording window. The game stopped at its requested world deadline.

A separate opt-in visibility shader experiment coalesced a sequence of
seven guest reads (nine adjacent dwords) on the already mapped same-page
path. Other cases retain the original reads and fault behavior. Eight AMD
reference tests passed. All five offline baseline DXIL blobs matched
recorded bedroom cache bytes, and all ten variant pipelines validated and
were created. Only 6d7fe7745ab9c6d8 changed: six guarded fast paths use
one directory lookup and three payload loads. The extra branches and
fallback grew the RADV code from 33,033 to 33,898 static instructions
and 1,394 to 1,448 VMEM instructions, with 96 VGPRs and zero spills in
both. Neither dynamic throughput nor gameplay improvement is established.

The bounded retry room-console-s-light-retry allowed one sample per pass
within world +240..259. It saved one image and no full ops dump. In fact
all seven samples landed on frame 1208, +240, matching the screenshot.
The four indirect directional passes shared one 640x360 R16_UINT surface.
All 15 pre/post locations read 0x00ff on all four passes. These included
different visibly shaded points on the back and different parts of the
wall. The same raw value therefore cannot by itself account for those
differences. This does not show that the whole texture is constant or
that each tested thread wrote a value. The exact stored shader signal
packs a square-root-encoded low byte and a separate high field; raw
16-bit numbers should not be interpreted as grayscale brightness.

Visibility dispatch dimensions read from the actual bounded indirect
records were (249,1,1) opaque, (336,1,1) alpha and (128,1,1) water.
All used the same guest argument address at distinct calls. The group
size of 32 gives 7,968, 10,752 and 4,096 launched threads respectively
on this frame; those are not counts of rays that actually hit geometry.
The GPU timing comparison in section 99 was a separate run, so it does
not measure time per group for these exact records. Five targeted Wine
tests passed. The diagnostic build was kept separate from the checkpoint.

A separate R16 store test on RADV 26.2.6 passed all eight CPU-supplied
bit patterns (including 0x0380 and 0x8001) via both the integer store
and a float4/asfloat store matching the trace's value representation.
It passed for an R16_UINT UAV on R16_UINT and R16_TYPELESS resources.
An R16_FLOAT view converted the low-pattern floating values to zero.
The R32_FLOAT bit-preservation control passed. Both DXILs validated.
The initial vectorized CPU-input control repeated its first four
values and was discarded. The replacement fixture sent one explicit
root word and column per dispatch. The available stack lacked the
D3D12 debug layer. This removes a generic integer-bit-loss theory
for this typed view on this driver, without validating the entire pass.

## 101. Isolated GPU throughput for the grouped visibility reads

The opt-in grouped reads from section 100 were timed in an interleaved
65536-lane dispatch fixture with output verification, 80 pairs for each
address distribution. Median GPU time with the switch off/on was
28.48/19.84 microseconds for aligned nodes, 28.84/19.54 for random
dwords, 27.14/18.82 for one in eight crossing a page, and 30.12/28.32
with eight divergent backing allocations. Enabled was faster in all
320 paired observations. This hot 8 MiB fixture executes one sequence
per lane and does not measure a frame or the complete visibility shader.

## 102. Windows fork reports versus the Series S Radeon runtime

The trustno1-coder/VIctory-NVIDIA README at 033be27f reports 20–22 fps
indoors on an RTX 5070, 24–31 in lighter views. Its speed preset uses
Series S at 960x540 with the game's upscaler and ray misses. GPU-side
memory comparison raised its reported 11.0 fps to 21.8 on that setup.
These are author measurements; no matched harbor capture is provided.

randomguy12000/VIctorious (d3b2d666f2) reports 51–55 ms at ~4,000 draws
driving on a Windows RX 9070 XT. Its descriptor record lock change
(ff7159b2d5) starts from a single lock; our bvh-samepage-tree already
has atomic keys and 512 locks in descriptor/host.rs. Its heap texture
worker (a408b3c011) is a candidate for the indexed path. In the local
room-console-s-light-retry report at victory.log:14778 the texture part
took 5.7 ms; slot scanning already overlaps recording. The 13.7 ms
wait at list close deserves profiling by worker phase.

fxceless/victory-fork at 06702fe8 also carries PR 1026. Our lower.rs
still lowers v_mad_legacy_f32 to legacy_mul followed by add. The fork
flushes denormal inputs and uses a fused operation. This is an untested
correctness candidate for highlights. There is no attribution to the
floor material without a matched pixel or shader test.

## 103. Room list-close waits and GPU comparison eligibility

Indexed Series S, RX 7900 XTX, 4 GiB BAR, BVH builder and card spans.
During seconds 6–55 after the room capture, median counters across 47
light-retry and 46 floor-stock reports respectively were 17.0/18.8 ms
scanning slots, 3.7/3.65 ms collecting buffer spans, 6.4/7.85 ms on
textures and 10.4/10.85 ms waiting at list close. In host/aside.rs the
list waits for the buffer-view pass. The texture counter is separate;
it would be incorrect to assign the close wait to texture uploads.
Guard-lock waits summed across threads were 12.1/10.8 ms. These counters
overlap and cannot be added to predict a frame-rate improvement.

The same reports show a median 37.3/37.0 MB per frame compared on the
CPU for title-buffer copies. Of these, a median 0.0 MB (log precision)
qualified for the Windows fork's 1 MiB minimum. Enabling comparison at
that threshold alone therefore cannot reproduce the RTX 5070 result in
these windows. GPU timestamps showed 57.4/70.85 ms executing queued
groups and 39.5/33.5 ms between groups. These are captured diagnostic
runs; timings include any instrumentation active in that run.

## 104. Grouped visibility reads in the actual harbor and bedroom

First pair of the off/on/on/off sequence. RX 7900 XTX, Series S,
indexed Proton, 4 GiB BAR, BVH builder, card spans, enlarged descriptor
pool. Identical executable (4c2c385f0c0d); only TRACE_GLOBAL_RUN and its
corresponding prewarmed cache namespace differ. Both game logs recorded
a host compute pipeline for 6d7fe7745ab9c6d8. The matching cached DXIL
digests start with 4f25e09f (off) and ce0b4ab2 (on).

Between the same two harbor markers, each leg had 88 presents. Throughput
rose from 6.84 to 7.24 fps, P95 present interval fell from 195.15 to
186.99 ms. Matching the 87 interior consecutive present intervals by
guest frame relative to the first marker gave a median on-minus-off
difference of -9.23 ms; 55/87 intervals shortened. These observations
share runs and do not count as 87 independent trials.

The +210 screenshots (off frame 1183, on frame 1167) both show hair and
the bright floor reflection. The back has larger sharply bordered areas
in the control and more uniform shading in the candidate; the pose
also differs. Earlier stock runs already produced both flat and detailed
skin, so this pair cannot attribute the visual difference to the reads.

The first two run directories independently contained the expected bytes
for all five checked cache entries and a host compute pipeline for each
guest hash. The bound-stage audit found 2,270/2,279 distinct bound hashes
(off/on), all translated, and 1,813/1,821 bound stage combinations, all
observed as host pipelines. No missing-host-pipeline or indirect draw
drops appeared in the 13/14 post-capture frame reports. One new stage
combination in the enabled run first logged its host pipeline 24 ms
after first bind. These counts cover the observed path and do not test
the calculations or bound resources of every stage.

In the fixed room window the first pair gave 9.7/10.85 median FPS,
103.1/92.2 ms, and 6,047/6,109.5 draws per frame. On the tracked GPU
queue, median working time fell from 73.9 to 49.6 ms while the median
between-submission gap rose from 29.2 to 42.1 ms. These are separate
medians from asynchronous counters.

### Follow-up on memory placement

The eight largest CPU comparison rows logged per report in the first
three runs each covered at least 896 KiB. Across the per-run medians,
they accounted for 11.52–11.60 MB/frame examined and only 0.320–0.366
MB/frame written back. This is a lower bound on bytes examined in
large ranges; an individual range is not an allocation identifier.
The active runtime explicitly logged a 1,024 KiB placement minimum.
Its existing VICTORY_CARD_BUFFERS_MIN_KB switch allows a 896 KiB trial
while leaving much smaller buffers alone. The earlier 64 KiB harbor
test in section 54 moved thousands of buffers for little frame-rate
change. The 1,024/896 comparison uses the same warmed control seed and
waits for the floor and legacy-MAD tests to relinquish the GPU.

### Crossover result

| Run order | Harbor presents | Harbor FPS | Harbor P95 ms | Room median FPS | Room ms | Draws/frame | Queue working / gap ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| off-1 | 88 | 6.84 | 195.15 | 9.7 | 103.1 | 6047 | 73.9 / 29.2 |
| on-1 | 88 | 7.24 | 186.99 | 10.85 | 92.2 | 6109.5 | 49.6 / 42.1 |
| on-2 | 89 | 7.02 | 196.71 | 8.6 | 115.7 | 6176 | 91.0 / 26.4 |
| off-2 | 88 | 7.28 | 183.10 | 11.1 | 89.8 | 5922 | 53.3 / 38.6 |

The harbor and room differences reversed in the second pair. The second
enabled run also had a substantially brighter floor and wall, despite
using the same seed as the first enabled run. The grouped reads remain
an opt-in experiment. The microbenchmark advantage is real but a
repeatable gameplay improvement was not demonstrated. No positive room
draw-drop report or absent host stage combination in any of the four runs.

## 105. The opaque spatial reflection filter supplies bright neighboring samples

Stock and center-only checkerboard test on the indexed Series S Radeon
path, frames 1208 and 1209 at world +240. Both on-disk run caches were
checked byte-for-byte against their respective DXIL files for spatial
shader 63381c3250b287fa; both created a host compute pipeline. On the
right floor, center-only points (1128,524) and (1112,548) fell from
22.679 and 14.214 linear HDR luma to 0.001 and 0.000. Interleaved
unmodified points (1112,524) and (1128,548) stayed close: 22.749 and
10.266 in stock, 23.013 and 10.388 in the experiment. Displayed median
luma on the right control/treated tiles was 205.91/229.65 in stock,
149.34/100.01 in the experiment. Camera/scene lighting differs; the
within-frame alternation in the linear output is the stronger evidence.
The left surface had a different response. Neighbor reconstruction is
supplying the strong radiance at these two floor samples. We have not
established whether the originating ray, weighting or material is wrong.
The center-only intervention deliberately leaves gaps and is diagnostic.

## 106. Legacy fused multiply-add crossover on RADV

At the same opcode locations, AMD's RDNA2 ISA specifies
V_FMA_LEGACY_F32 and V_FMAC_LEGACY_F32. Its MODE register controls
single-precision input and output denorm handling separately (sections
“Instruction Changes” and 6.4):
https://www.amd.com/content/dam/amd/en/documents/radeon-tech-docs/instruction-set-architectures/rdna2-shader-instruction-set-architecture.pdf

On RX 7900 XTX with vkd3d-proton, independent control/candidate shader
test binaries (83927b00/fb2cd7a8) both returned +0 for each opcode on
a cancellation case whose exact fused result is -2^-46 (0xa8800000).
An earlier candidate launch reused the control binary; that result
was discarded. The upstream lowering therefore fails this fixture on
our stack. Candidate also changed signed-zero results. Do not promote
it as a verified fix. The four retail stages whose arithmetic was
examined use XDAT; their precise denorm mode has not been established.

## 107. Lowering the card placement minimum to 896 KiB

Indexed Series S, same control executable and warm cache, unchanged
large descriptor pool, RT builder enabled. The runtime confirmed the
1,024 and 896 KiB limits. In the fixed room window CPU comparison fell
from 37.3 to 23.7 MB/frame and CPU snapshotting from 31.8 to 21.65 MB.
The return path still took 30.4 versus 30.0 ms. The rooms had 6,220
versus 5,843 median draws and 101.0 versus 106.3 ms/frame (9.9 versus
9.4 FPS). The marked harbor took 6.97 versus 7.32 FPS; P95 192.59
versus 186.25 ms. The change demonstrably moved comparisons, without
a consistent gameplay speedup; keep the 1,024 KiB baseline.

## 108. Normal-queue visibility timings and the 21 MB buffer

room-console-s-vis-normal used indexed Series S, the warm off seed and
the section 104 settings. Only the three verified indirect RaytraceVBuffer
calls were bracketed, at seven world offsets (+180 through +360, every
30 frames). GPU query pairs enclosed only ExecuteIndirect. Results and
actual bounded XYZ records were mapped after the normal list fence, with
the normal deferred-wait path active. All 21 pairs were valid, from a
100 MHz timestamp domain. Median opaque/alpha/water durations were
3.54/5.32/1.82 ms at 236/359/77 groups respectively. Across the seven
complete frames the median sum was 10.45 ms. The previously serialized
31.30 ms visibility measurement (§95) is a different execution mode.
Post-capture median wall frame time here was 91.65 ms. Even removing
the full observed visibility interval leaves a substantial gap to 24 FPS.

The persistent 21,045,760-byte whole copy is a committed guest D3D
buffer with allocator attributes 0x30808000. Its backing covers
21,102,592 bytes and is part of a larger private read/write allocation.
The exact and rounded source queries both refused GetWriteWatch and a
query on the one VirtualQuery region also failed. No traced XMem
reservation enclosing it was seen. Ten late reports gave a median 5.41
ms/frame for the two copies together. The runtime must account for
possible guest CPU writes until the owning allocator supplies tracking.

## 109. Native placed maps and guest host-memory import on the RX 7900 XTX

Host RADV reports VK_EXT_map_memory_placed, memoryMapPlaced=true,
memoryMapRangePlaced=false, memoryUnmapReserve=true and 4 KiB placement
alignment. A standalone native test reserved its own virtual address,
placed an entire host-coherent, device-local VkDeviceMemory there,
unmapped with reserve, filled half the buffer on the GPU, and remapped
at exactly the same address. Both halves (16,384 words) matched.

The 64-bit Windows enumeration utility on the exact indexed Proton
runtime returned 237 RADV device extensions: VK_EXT_map_memory_placed
absent, VK_KHR_map_memory2 and VK_EXT_external_memory_host present.
The latter was also tested through the actual Windows Vulkan loader.
An aligned interior pointer 64 KiB into a fresh VirtualAlloc reservation
was imported as a Vulkan buffer on memory type 5 (host visible,
coherent, cached; not device-local). After a GPU copy, GPU fill, host
barrier and queue completion, 4,096 copied CPU-origin words, 4,096 GPU
fill words and 8,192 original words matched in the original allocation,
and its 64 KiB prefix was intact. Sources: checkpoints/7900xtx-vulkan-memory.
These are single-process synthetic correctness tests with owned allocations;
D3D resource integration, remapping lifetime, fences during concurrent
use and gameplay throughput are separate questions.

## 110. Legacy fused arithmetic and bedroom reflection samples on RADV

The RX 7900 XTX passed 390/390 individual legacy FMA and MAC cases
with the native fused lowering. The existing RDNA regression also passed,
and the emitted module passed DXIL validation. The source-only candidate
is checkpoints/7900xtx-rdna2-legacy-fma.patch (SHA-256
f0ef78d53d782a088780cad62869dc92d1ae051b756680088d2a67a9b9bdf9b7).
It retains a precise FMad through vkd3d-proton. This establishes arithmetic
correctness for the exercised inputs. There is no gameplay FPS claim.

In room-console-s-spatial-weights, world +240, the floor reflection at
(1112,548) received almost all its spatial numerator from the half-resolution
neighbor (554,269), whose incoming HDR RGB was (16.5,20.25,27). Its normal
dot product was 0.9999 and its plane weight 0.99932. The center was near zero.
The contributing value was already present in the resolve output bound as the
spatial input. A diagnostic for this sample should follow the resolve hit
and its color source before changing spatial weights or global exposure.
Only three of eight selected sample points executed the probed shader.

## 111. Pipeline pack progress versus observed Series S coverage

The screenshot's progress text matches apps/victory/src/prebuild.rs in the
Marvisak/VIctory mirror at bde8e7decb675d740b3ae75d2466aafdd221fdef.
The progress denominator is the remaining job list after preparation and
previously listed jobs are filtered. Made and loaded count host pipeline
creations and library hits. Skips are reported separately at completion.
This progress line alone does not identify any failed shader.

The indexed stock scale replay recorded 2,345 distinct bound shader hashes
and 1,867 bound stage combinations. Every stage had a translation, every
combination had a matching host pipeline, and all 12 room reports had zero
reported pipeline drops. A stage combination can belong to several PSOs
with different fixed-function state. Checking exact keys separately, all
2,628 stock early keys have warm successes on fingerprint 76cc7008efca850a.
The six old timeout entries also have successes on that fingerprint.
Across 41 Series S run directories including the scale tests, 2,677
distinct keys had been registered by 11:52 UTC. All 49 extra keys are
present in the original 88,596-recipe book. The manifest’s 2,628 keys
still describe the original private warm seed. pipeline-coverage.py
reproduces the offline key comparison without modifying any cache.

## 112. Render resolution and queue gaps in the Series S bedroom

room-console-s-scale-stock/own/half ran serially with the same executable,
2,628-recipe warm seed, enlarged descriptor pool, BVH builder, 4 GiB
BAR and indexed Proton. Matched mesh viewport and bound depth probes
at world +4 and +210 confirmed 1280x720 for stock and own-upscaler,
960x540 for own-upscaler at scale 0.5. All three reached the timed stop.
In 87 aligned harbor intervals per run, frame times averaged
144.11/135.57/131.24 ms. After the bedroom image the median report
was 109.2/102.8/102.25 ms. One run per setting, with different skin
tone and lighting between the three images. Even the smaller workload
remained far from the 33.33 ms frame budget.

Over the common 6-60 seconds after the image, there were 50/52/53
short diagnostic reports respectively. Median elapsed GPU timestamp
intervals inside submissions were 63.15/57.0/59.4 ms per frame, with
45.05/45.95/41.8 ms of timestamp gaps between submissions on that queue.
The submitting worker reported 83.4/74.0/75.8 ms of work. By the
separate host fence observer the queue was empty while the submitter
worked for 29.6/30.75/28.2 ms. These clocks overlap. Submission spans
include synchronization inside the submitted work. They measure neither
shader ALU utilization nor an unavoidable lower bound for a native backend.
The comparisons prioritize queue construction, memory dependencies and
GPU work in tandem. There is no matched native reference frame yet.

The expanded private seed warm-console-s-scale-union includes the subsequent
shader regression replay in its observed list, bringing the total to 2,678
keys from 42 run directories. All 50 additional recipes built in three
seconds with zero failures. A second warm on fingerprint 76cc7008efca850a
reported 2,678 already made, zero bad and zero left. A separate offline
manifest/hash check matched all 2,678 used keys against successes. Five
previously validated DXIL blobs retained their hashes. No global archive
coverage or steady-state FPS gain follows from warming these scene keys.

## 113. Native Vulkan addressing and D3D12 alias controls

An isolated native RADV benchmark compared a 2,048-entry descriptor array
with buffer device addresses on the same 128 MiB device-local allocation.
Both shaders looked up the same 64 KiB page table and read eight vec4s per
lane. All 524,288 output words per mode and workload matched CPU reference.
For 32 timestamp pairs each, descriptor/BDA median microseconds were
46.76/47.30 for coherent accesses and 177.02/171.72 for random accesses.
BDA won 16 and 21 pairs respectively. For random access, paired improvement
changed sign with execution order (-3.99% when BDA ran first, +10.09%
when it ran second). This does not support a large speedup from changing
the lookup alone. Host copies, remapping and first-touch residency were
outside this fixture.

The D3D12 private import on the RX 7900 XTX retained 65,536 bytes across
two CPU patterns and 8,192 bytes written back by GPU, including a separate
compute result of 42. A separate clean-device fixture for mapped sections
passed bidirectional CPU alias checks before and after import and after
release, but GPU reads returned zero where both CPU views held byte 31.
Both SEC_COMMIT and committed SEC_RESERVE subranges showed this failure.
Protecting an already-imported private range with PAGE_NOACCESS in a
separate fixture led to DXGI_ERROR_DEVICE_REMOVED. Keep gameplay on the
validated memory path until synchronization and lifetime are established.

## 114. Release versus diagnostics, first controlled pair

The Series S diagnostic and release executables used the same scene inputs,
shader translator fingerprint and 2,678-recipe warm seed. A shared DXGI
Present observer recorded actual completed presents, with no missing frame
IDs or failed calls in either measured interval. In 85 harbor intervals,
diagnostics/release throughput was 7.46/9.23 FPS and P95 was
177.49/138.22 ms. In 240 bedroom intervals it was 10.08/11.16 FPS and
P95 was 127.27/121.74 ms. Median bedroom frame time was 99.0/89.8 ms.
The common camera waypoint reached the same furnished room. Skin and
lighting differed between captures. Bedroom work was 6,043/5,956 draws
and 2,852/2,940 dispatches per frame. This first pair demonstrates why
the release build needs its own baseline. Order reversal and repeatability
remain to be checked before attributing the full difference to diagnostics.

## 115. Two CPU aliases with a native GPU-owned DMA-BUF on RADV

A separate 256 KiB driver allocation was exported as a DMA-BUF and
reimported onto the same Vulkan device. The CPU mapped a whole-allocation
view and a second view starting at +64 KiB. Both the GTT allocation
(type 2, flags 0x6) and the host-visible device-local allocation
(type 3, flags 0x7) passed two fenced passes. Each verified 65,536
CPU-sourced bytes copied by the GPU, 65,536 GPU fill bytes, the complete
262,144-byte owner including untouched bytes, and the 131,072-byte
interior alias. In the second pass the interior view was unmapped before
submission and recreated after its fence. Physical residency throughout
CPU access was not measured. This is a working driver-owned alternative
to importing ordinary mapped Wine sections. The next integration boundary
is guest reservation, mapping offsets and fences on the runtime queue.
There is no game FPS measurement for this path.
Source: checkpoints/7900xtx-vulkan-memory/native-dmabuf-alias.c.

## 116. Bedroom reflection input before spatial reconstruction

room-console-s-resolve-source instrumented the authenticated Series S
resolve shader on its live compute pipeline at world +240. The instrument
left its original texture store intact; a separate private buffer recorded
inputs, path and outputs in two 32x24-pixel windows. DXIL validation and
four host tests passed. In observation 8, resource 25468 was both the
resolve target and the subsequent opaque spatial filter input. At (556,262)
it already contained packed 0x9da66cb5 before resolve and retained that
value. No lane wrote (554,269), the dominant neighbor in the earlier run.
Of 201 observed writes in the right window seven had an output channel
>= 10. Their path and nonzero blue inputs trace to a sample of table s1+80
multiplied by an earlier scalar. The final multiplier was 0.00390653.
At (554,264) the final RGB was (52.32,47.80,40.02). This distinguishes
bright values inherited from earlier passes from bright values actually
written in this dispatch. It does not establish a faulty texture, a bad
hit coordinate, or an incorrect exposure.

## 117. Rechecking old translation failures against the current recipe book

The current private pack book contains 88,596 recipes and references
19,135 distinct shader hashes; 2,740 occur in the 2,678-key used set.
Comparing the older 282 rejected containers against the recorded shaders
identified 34 matches, each a single-stage compute recipe. The current
translator handles all 29 ACCC-only FidelityFX inputs. Twenty match the
active fingerprint's exact cached DXIL byte for byte, have a verified warm
recipe and have host pipeline and first-bind records in both stock Series S
runs examined. The other nine translated offline without a recorded use in
this set. The five CS_TraceShadowRays shaders (Full, FullAdaptive, Half,
Quarter and Eighth) still fail on a descriptor read as scalar data. None
of their recipe keys was used or warmed in this set. No visual blame for
the indoor reflections follows from these five failures. Logs and hashes:
private old-reject-audit/summary.txt, native-results.tsv and auth.tsv.

### Warm fingerprint correction for the lean FPS comparison

All four lean-pair launches use independent clones of the same 2,678-key
input. The 2,678 completed warm recipes in that source seed belong to the
global-run translator fingerprint 76cc7008efca850a. The lean binaries
use the stock translator, with 2,628 prior successes on 84afa9f4affa17b9.
Across the 2,740 shaders referenced by the 2,678 keys, the source seed has
2,740 exact global-run shader entries and 2,696 exact stock entries. Thus
the compared launches have equal starting state but only 2,628 matching
prior warm records. A separately verified stock seed is needed to call
all 2,678 recipes prewarmed for the production binary.

## 118. Order-reversed release performance on indexed Proton

Series S, identical executable hashes per variant as section 114, same
shader settings, enlarged descriptor pool and identical private starting
caches. Sequence diagnostics, release, release, diagnostics. Harbor FPS
was 7.46, 9.23, 9.50, 7.56. The means of the two per-run throughputs
were 7.51 and 9.37 FPS, +24.7% for release; mean complete present
interval fell from 133.15 to 106.79 ms. Mean draws and dispatches per frame
differed by about 0.5% between variants. Bedroom FPS was 10.08, 11.16,
12.28 and 9.73. The last diagnostic run drew 8,225 draws/frame versus
about 6,000 in the other three, limiting a causal percentage for that room.
The recipe fingerprint qualification in section 117 applies to all four.
These are real gameplay present intervals; both phases are still well
short of 24 FPS. Use release for player-facing measurements.

## 119. Bedroom reflection resolve and GPU counters on the 7900 XTX

Series S, room-console-s-resolve-hit, frame 1203. A shader-specific diagnostic
captured both coordinates, filtered source RGB, scale and final RGB in the
same invocation. Four resolve dispatches were observed. One wrote the probe
area (62 pixels). On 61 filtered-path writes the source measured 640 x 360,
the UV adjustment moved less than 0.000923 source texels, and the final RGB
matched the filtered source within 1.2e-7. The two scale factors multiplied
to 1.00000006. The largest sampled filtered color was (0.915, 1.226, 1.647).
The screen landmark for its normalized source coordinates lies near the bright
far window, while the output pixel lies on the glossy floor. These measurements
leave surface roughness, reflection hit selection and other passes open. No
sample in this capture repeated the previous frame's values above 10.

Half-second amdgpu sysfs samples in this diagnostic run recorded 100% median
GPU busy both before the screenshot (58 samples) and after the screenshot
plus five seconds through the final log (113 samples). The latter window had
99-100% busy, 3098 MHz median gfx clock, 197 W, 67 C junction, 11.38 GiB
device-wide VRAM usage and a Gen4 x16 link. This is a single instrumented
room run. Busy percentage does not identify the expensive GPU commands.

The private stock Series S cache subsequently warmed the 50 outstanding
recipes from section 117. The second pass reported zero outstanding and
confirmed all 2740 referenced shader artifacts for the original 2678 recipes.
The four-run ABBA audit has one additional used key present in the recipe book.

In the same room window, 53 complete fence reports had a median 29.1 ms
with the tracked queue empty per frame, including 24.2 ms while the
submitting worker prepared work. Across 53 GPU timestamp reports the
medians were 17.4 groups, 60.2 ms inside groups and 34.4 ms between
groups per frame. GPU stamp readback trails recording by 512 groups.
The launcher enabled the separate keep-awake compute queue (section 81).
Device-wide busy therefore includes deliberate background work. The
longest single group in each reporting window had a median of 40.5 ms;
this is a window maximum, not a typical per-frame group duration.

## 120. Release descriptor batch and completed gameplay warm set

One Series S release off/on pair used one binary, the enlarged descriptor
pool and the same frozen 2678-recipe cache. Batching the host constant-slot
updates in groups of at most 128 changed harbor FPS from 9.285 to 8.616
(107.696 to 116.060 ms mean, 85 intervals each). In the room FPS changed
from 12.703 to 12.235 (78.720 to 81.731 ms, 240 intervals each). No
missing intervals, failed presents or OOM kills in either leg. Leave the
candidate off. The single pair does not establish an exact regression size.

The separate player cache now covers all 2679 distinct recipes used across
the four release/diagnostics runs on its stock translator and driver
fingerprint. All 2741 shader artifacts were authenticated. The second
warm had zero outstanding. The original 2678 cache was retained.

## 121. Wine section views of a Vulkan-owned Radeon allocation

Synthetic cross-process tests on the indexed Wine runtime, vkd3d not involved
in these transfers. A native process gave its 256 KiB Radeon DMA-BUF fd to
a Windows executable. Wine converted it to an NT file handle; creating a file
mapping and mapping full and interior views succeeded. The Windows child
rewrote bytes, and a native Vulkan copy and fill passed the complete byte
checks. A second Wine child saw the GPU copy after the fence. Both GTT and
device-local host-visible memory passed. A plain memfd control passed too.

A second run used VirtualAlloc2, MapViewOfFile3 with REPLACE_PLACEHOLDER
and UnmapViewOfFile2 with PRESERVE_PLACEHOLDER (as used in XMem). Both
memory classes passed remapping one 128 KiB view at the same guest address
across section offsets 64 KiB, zero and 64 KiB. Full byte comparisons and
the native owner guard passed. This establishes that indexed Wine can host
guest CPU aliases for this backing today. A binding consumed by the title's
D3D12 queue and a workload FPS comparison are still required.
Reproducible synthetic source: checkpoints/7900xtx-vulkan-memory/nt-section.

## 122. D3D12 resource sharing on the indexed Radeon run

The stock vkd3d-proton 3.2 DLL accepted CreateCommittedResource and
CreateSharedHandle for a 262144-byte shared D3D12 buffer, but the handle
output was null. Wine trace shows D3DKMTShareObjects declining the object,
followed by vkGetMemoryWin32HandleKHR returning a null NT shared handle.
There was no alias or D3D12 copy to validate in this case.

An otherwise matched 256 x 256 RGBA8 shared texture on the same device
produced a live handle. Opening the D3DKMT resource, obtaining its Unix
file descriptor, and converting the descriptor to an NT file handle all
succeeded; the backing measured 262144 bytes. The test ended before
mapping the texture. This isolates the failure to the resource path.
In vkd3d-proton revision cae8353ed, d3d12_resource_create_committed
requests exportable Vulkan memory for shared textures; the buffer branch
calls vkd3d_allocate_heap_memory without an export request. A private
candidate requests a dedicated exportable allocation and an external
buffer at creation. Its acceptance criterion is byte-exact D3D12 queue
copies in both directions with a removed and reconstituted Wine guest
alias. It is outside the player build.

## 123. Splitting the large private CPU copy in normal gameplay

Series S diagnostic, frozen 2678-recipe seed, same binary with split off,
on, on. The 21 MB source passed sparse/read-only-page and guard tests.
The stationary-room workload copied about 41 MB/frame in each leg. CPU
time for those copies fell from 5.42 to 3.75 and 3.65 ms/frame. Median
room frame times were 100.85, 94.95 and 100.50 ms. Harbor mean intervals
were 137.76, 133.74 and 153.94 ms over the common 86-frame sequence.
Time with an empty queue during refresh stayed close to 8 ms/frame. All
three captures showed the body, hair and floor; exposure varied. The
subroutine saving reproduced, while a gameplay throughput gain did not.
The split remains an opt-in experiment.

The packaged observer-free release subsequently reached the room using
the 2679-recipe cache, an isolated save and the indexed Proton runtime.
Eight consecutive steady room reports ranged from 11.1 to 13.3 FPS,
with median 12.35 FPS and 80.75 ms/frame. No missing-pipeline drops
in that sequence. The character and hair are visible, and the floor is
still glossy. The supervisor sent SIGTERM at the requested 56 seconds
in the world. The smoke run recorded no OOM kill. The production
launcher's own window reported 13 FPS / 78.8 ms at the screen capture.

## 124. Direct D3D12 shader writes through a Wine-mapped buffer

With local vkd3d-proton cae8353ed changes, a 256 KiB buffer allocated by
D3D12 on a CPU write-back custom L0 heap passed the cross-API test on
RX 7900 XTX. Wine mapped two aliases. The D3D12 queue read the CPU
pattern exactly, wrote into the backing and returned a fence. A newly
placed Wine view at the same address saw the GPU bytes. A compute
shader read and modified 16384 32-bit values using a root UAV; both
views and the untouched guards matched. The default heap produced a
valid export handle but its CPU view failed with access denied. The
stock buffer control returned a null shared handle. No title-run FPS
measurement exists for this experimental path. Reproducible source:
checkpoints/7900xtx-vulkan-memory/d3d12-shared-buffer.

## 125. GPU time spent comparing title-memory buffers

The Series S diagnostic run `room-console-s-card-check-stamps-warm` used
the frozen 2678-recipe seed and disabled global-run tracing. Discarding
the delayed timestamp ring across each phase boundary left 177 frame
equivalents before the bedroom capture and 391 afterward. Frame-weighted
GPU time for the comparison at the end of submission averaged 1.222 ms
out of 53.711 ms in submitted groups before the capture, and 1.402 ms
out of 55.240 ms afterward. All timestamp pairs used were valid.
The comparison accounted for 2.54% of measured group time in the room.
CPU bookkeeping, copy operations elsewhere and waits between groups
remain outside this interval.

## 126. Physical pages behind CPU shadow copies in the bedroom

Series S diagnostic run `room-console-s-xmem-submit-2`, warm 2678-recipe
seed, world frames +224 through +231. All five observation channels covered
all eight frames. Of 12670 observed XMem physical pages, 566 had a submitted
read and an actual CPU shadow copy with no recorded fetch or declared write
during the interval. They accounted for 13568092 bytes copied per frame.
The busiest page accounted for 130608 bytes/frame in a 4 MiB shadow window.
This is one scene sample. It measures traffic, not time, and does not prove
that these pages can be bound directly across a remap or later GPU write.
The separate shared-section factory passed 32 create/map/release cycles
and a fenced compute test on the same card.

The observer-free release with the optional backing compiled. On the
indexed Proton runtime, the HLE physical page test passed in stock mode
and with pages 1 and 2 of a three-page allocation backed by D3D12.
The second test asserted that the runtime really selected that backing.
Both exercised writes across aliases, the DirectStorage physical write
helper, sparse unmap, scatter remap and reconstruction of a CPU view.
The release comparison uses the same private D3D12 libraries in both legs.

The Series S release reached the room both ways using the indexed Proton
runtime, the same private vkd3d and 2679-recipe source seed. The candidate
allocated exactly page 28017 of the first 63632-page request using D3D12.
The last eight periodic reports gave median 105.5 ms/frame (9.45 FPS) in
room-console-s-owner-control and 111.5 ms/frame (8.95 FPS) in
room-console-s-owner-shared. Zero dropped draws for missing pipelines in
these intervals, zero scope OOM kills, both stopped at 56 seconds in the
world by the supervisor. The character and hair are visible and the floor
is glossy in both images. The light on the back and upper wall differs.
The ambient CPU shader audit and varying draw counts limit throughput
attribution. This proves game compatibility for a single physical page.
The patch still populates ordinary GPU shadow buffers; direct shader
binding and skipping their copies require separate tests.

## 127. Bedroom replay checkpoint and physical window geometry

The annotated vfork tag checkpoint-7900xtx-bedroom-render-20261126
identifies the replay recipe for the shared-page leg in §126. The
private checkpoint contains both original full-resolution room images,
the separate garment reference, executable and DLLs, measured source,
indexed Proton, both cache layers and a clean starting prefix/save.
21,169 regular files passed SHA-256 verification. The target visual has
dark shadows and pale reflections. The tagged recipe uses one shared
page and preserves the ordinary shadow copies.

A repeat of the eight-frame physical-page census identified 351 pages
with 12,820,164 bytes of eligible actual CPU copies per frame. None of
192 measured or previously selected shadow spans had contiguous physical
backing across its full extent. The earlier top 4 MiB span had 16 of its
64 pages mapped across nine runs. The other had two mapped pages in two
runs. Generation 4 is the first 63,632-page XMem allocation in the run.

An incremental hardware fixture, outside the tagged build, resolved
ranges through the runtime guest-view table and tested direct D3D12
reads across a contiguous shared two-page region. A raw UAV dispatch
through the second alias at a nonzero offset wrote the expected sum,
visible in both CPU aliases with the adjacent sentinel preserved.
Resolution rejected mixed stock/shared memory and the former shared
address after remapping it to a stock page. Both stock and shared test
processes passed. These checks use a quiescent guest and explicit fences.

A fresh replay from the saved package reached the bedroom and selected
the intended shared page. The screenshot shows the player, hair and
shorts, with the dark doorway and bright floor reflection still visible.
The supervisor stopped it at 45 world seconds as requested (exit SIGTERM).
The private run directory is room-console-s-bedroom-checkpoint-replay.

## 128. Pending GPU work at upload fills on the measured route

Two indexed Series S diagnostic runs with the 2678-recipe seed,
room-console-s-xmem-submit-2 and room-console-s-xmem-window-1, reported
median submit-thread waits for outstanding GPU work before guest-memory
reads of 8.3 and 8.9 ms/frame in the harbor. The corresponding medians
after the room capture were 30.15 and 15.95 ms/frame. These are medians
of periodic reports spanning complete intervals within each phase:
11 and 13 harbor reports, 62 and 60 room reports. The waits can overlap
necessary GPU work. In the harbor a recurring refusal reason was the
12,288-step planning limit. In the bedroom the common refusal named
a conflicting copy recorded inside a command list. That reason applies
to the whole batch; identifying the specific dependency requires the
placement and order of its individual operations. Neither refusal alone
establishes the FPS that could be recovered.

## 129. Comparison with fxceless main at ceb9e1f

Source: https://github.com/fxceless/victory-fork/blob/ceb9e1f7a0180e61961bfde85d69d24e2dd84b39/README.md .
The supplied boat screenshot reads 11.5 FPS and 87 ms. Its build, preset,
and averaging window are unknown. The repository reports a separate desktop
RTX 4070 Series X first-boat test at 3,000–5,000 draws: 6.2–6.7 FPS with
the heap worker off and 8.2–8.6 with it on. Parallel texture checks later
saved 8.7 and 9.4 ms of preparation at equal draw counts. With the larger
outstanding-work limits, GPU time was 84–128 ms against about 60 ms of CPU
preparation in that scene.

In the Radeon lean-pair source, Batch::refresh_textures already runs independent
staging checks through shadow::wide_each and flush joins them. Its local
switch is VICTORY_WIDE_TEXTURES, enabled by default when wide_refresh is on.
The latter defaults on from four logical CPUs. The local late_most list limit
is separately configurable via VICTORY_LATE_WAIT_LISTS. The local changed-run
budget is 4,096 with card steps enabled and 512 otherwise. Consequently simply
copying the upstream switch names would not establish a new optimization.
Our measured Series S release harbor results (9.23 and 9.50 FPS) cover a
different scene. No matched boat comparison or new Radeon gain is established
by this source audit.

## 130. Moving shadow report and release thread scheduling

A pale outline was previously observed around the character in a single
bedroom frame (§57). Existing consecutive harbor captures show 0–1 pixels
of landmark movement per axis and no visible player shadow. Older scripted
room captures are more than 1,000 frames apart. Neither set distinguishes a
shadow retained at a previous position from an outline or lighting error.
The current route's recipe set contains both shadow temporal upsampling
and whole-scene temporal AA. A recipe's presence does not show which stage
first produces the trail. A separate short game-window capture is queued
so visual readback does not contaminate the A/B performance recordings.

During 42.04 seconds of room-console-s-heap-early-b1, the eight named
victory-list threads accumulated 144.47 CPU seconds and 5.35 seconds runnable
waiting for CPU. All eight had affinity 0–19. In 613 of 1,277 active polling
intervals their last CPU was an efficiency core. That is a sampled last-CPU
count, not time spent on the efficiency cores. The data do not support treating
scheduler queue time as the main cost for these threads. Mutex, GPU and other
blocked time are not included in runnable waits.

## 131. KLBF Lab source and Proton measurements

Reviewed public main 427bb782, beta-2 8187780 and the heads of PRs #34
and #54 in the local KLBF mirror (refs/scan/20261126). Lab 61 reports
14.0 FPS mean, 14.5 median and a 0.4 FPS 1% low on Windows 11, RX 9070 XT
and Ryzen 7 9800X3D, Series S with FSR 4, over 385 gameplay samples.
Its 27 FPS figure is a sky-facing peak. The Lab 62 README still quotes
Lab 61's measurements. Lab 62 changes the launcher to cap selected
textures at 1024 pixels and records missing water and the earlier
flickering shadows in the Lab 61 notes. These are different workloads
from the Radeon harbor and room comparison.

KLBF PR #35 reports a 2068 to 509 ms median frame improvement from
indexing Wine views on an RX 6900 XT. Our measured lean release already
runs a compatible indexed wineserver. PR #34 proposes guarded copies
within the process. The lean build already takes that path by default
through safecopy. KLBF PR #54 reports 289 to 239 ms when restoring late
wait and 257 to 206 ms for copy queue off to on in separate RX 6900 XT
Series X comparisons. Our release launcher enables VICTORY_RELAY=2
and VICTORY_RELAY_COPY=1. KLBF’s copy-compare test cut uploads from
170 to 32 MB/frame with frame time at 206 vs 207 ms.

A cross-check of distinct logged first shader binds across
room-console-s-heap-early-a1, b1, b2 and room-console-s-private-refresh-delta
found 2,422 hashes, including nine of the 48 hashes on beta-2's
docs/klbf/skip-amd.txt. Each of the four runs bound those nine.
The intersection was 072f5fe96efd27e1, 1c02f8131c508c75, 39f0e9aeb156a61c,
3fa1fda35402c93f, 76cfc8733e521f35, 7b956e1f6d56609f, a1266cc7391ae0d6,
a4df980a1c65fdbe and a73284b60aa303ed. The log gives bound hashes,
with visual stage attribution still open. The KLBF launcher settings
were not adopted. The local texture sweep already orders victims by age
when over budget. A Windows driver skip list or a texture dimension cap
would change the rendered workload of our quality comparisons.

## 132. A repeatedly identical private buffer refresh

The RX 7900 XTX Series S private-refresh-delta probe sampled every 16th
eligible full refresh of a 21,045,760-byte carded buffer. All 101 samples
of shadow 4087 were byte-identical to the CPU staging data. Every sample
copied the same snapshot and scheduled the existing whole transfer.
The supervisor stopped at 95 world seconds with status 0. The observation
bounds a candidate for eliminating redundant uploads. It establishes
neither write-tracking coverage nor equality with the live GPU buffer.

## 133. Heap synchronization schedule ABBA on the Series S route

All four RX 7900 XTX release legs used the same binary, 2679-recipe seed,
enlarged descriptor pool, indexed Proton and launcher settings. Only
VICTORY_HEAP_SYNC_EARLY changed: A=4 and B=0. Each leg contains 85 harbor
and 240 steady bedroom Present intervals. There were no failed Presents,
missing or reordered frame IDs or scope OOM kills within these windows.

| Leg | Harbor FPS | Harbor p95 (ms) | Bedroom FPS | Bedroom p95 (ms) |
| --- | ---: | ---: | ---: | ---: |
| A1 | 10.012 | 122.791 | 10.939 | 127.281 |
| B1 | 9.965 | 125.826 | 9.289 | 152.272 |
| B2 | 8.886 | 132.451 | 9.679 | 150.130 |
| A2 | 9.070 | 128.753 | 6.949 | 199.778 |

The sign of the bedroom throughput and tail changes reverses relative
to the two A legs. Draws per frame in the bedroom were 5956–6120, so
the slower repeat cannot be explained from total draw count alone.
No scheduling change is justified by these runs.

## 134. Consecutive bedroom pictures and GPU buffer follow-up

room-console-s-shadow-motion-1 recorded 48 full-window video samples
at 24 Hz (22 distinct images) from the Series S release at world +224.
The doorframes moved less than one native pixel while the tracked
forearm edge moved 20 pixels. At samples 7 and 18, the arm edge was
within one pixel of its prior position. The strip alongside it had
luma 147 then 100 on an 8-bit scale. The opposite wall stayed at 75,
the lit floor at 199–200 and the torso at 119. The narrow bright ridge
stayed close to the same wall feature. The result establishes a
localized lighting change near the silhouette. A separate moving cast
shadow was not resolved. Stage-level attribution needs a comparison of
the shadow output, motion vectors and the final temporal resolve.

The follow-up to §132 reuses the existing at-rest GPU comparison and
restricts it to the 21,045,760-byte allocation size. Each report gives
the exact compared byte count, differing dwords, and whether a shader
write was declared. It skips ranges with outstanding CPU dirty data.
The diagnostic executable is built, checksum verified and launched
under the one-instance supervisor. Equality after settling cannot on
its own prove equality immediately before a send or justify skipping one.

The GPU diagnostic completed with status 0. In all 24 observations of
shadow 4098, the full 21,045,760 bytes matched the staging copy. A shader
write was declared for the allocation in every observation. Keep uploads
until the same equality can be tested at the send boundary or enforced
by dependency tracking.

## 135. Upstream RX 7900 XTX renderer changes

Reviewed local source mirror avatarmanz-7900xtx-20261126 at c9c932c,
including merges for raw constant buffers (0dec5dc), guarded refresh
planning (cc761bf), and heap and journal scheduling (3406192).
The upstream merge-5 result is 44.9 ms in a Series S city route, with
ray tracing off, on Windows 11, an RX 7900 XTX, and a 64 GB host. It is
not a same-scene comparison to our Series S release under Proton. The
reported 36 ms of GPU work in that workload is itself above 33.3 ms.

Port 5 reports about 22,000 fewer fills and 4.7 ms off the frame. It
requires the constants backing from port 1 and raw descriptor twins
and uses a fallback where the original allocation is not GPU-accessible
or its raw view would be misaligned. That maps to our current memory
placement problem. The local hardware probe has already verified both
uploaded and GPU-produced versions of the same 32-, 240- and 544-byte
constant ranges. Gameplay A/B is still pending. Port 6 and port 7 modify
guard ownership and descriptor ordering in addition to parallelizing
work. The local ABBA in §133 tests a different, narrower scheduling
change. Upstream's ambient time-constant change applies only when the
ambient stand-in runs. The lean release ghosting capture had no
stand-in invocation logged, so it is not evidence that the change
would affect this artifact.

REA 6.8.0 over its local MCP stdio server and Ghidra 12.1.4 decompiled
req_map_view at 0x4576b0 and req_unmap_view at 0x457ce0 in the exact
indexed wineserver (SHA-256 ba3d4a9e5ad7d368df7c59ae6b39a3a87cb44be7a4d1c20478c7c8b68259dd4d).
Map calls add_process_view. Unmap calls find_mapped_view and then
free_memory_view. This confirms the native entry points for the
existing Wine path. It does not locate the rendering artifact.

## 136. FullAdaptive release smokes and the Series X draw drops

The private AMD release with both shadow shader changes warmed all
2,685 selected recipes at fingerprint 16ea5145672d1998. Validation
authenticated 2,747 cached stages and confirmed the 2,741 stock DXIL
payloads were byte-identical. Both Series S room smokes ran for
56 world seconds with zero reported missing-pipeline drops. Both
Series X smokes reached their 48-second deadline. Neither mode
reported device loss, translation error or scope OOM. This scope
does not cover all game shaders or prove the recovered shadow
variants executed during these runs.

Series X control and candidate reported 140.8 and 113.5 ms median
from ten periodic reports and different scene progression. Both
reported descriptor table failures during the early transition.
The logs show "no free dynamic descriptor slots" immediately
followed by "a draw whose descriptor table found no free slots
is dropped". Control first requested 255 contiguous slots at the
failure, candidate 26. Each log called the host heap 2,097,152
views with raw twins. That total includes the twin region and is
not the allocation budget available to dynamic descriptor tables.
A warmed recipe did not prevent this failure. The aggregate
counter can include multiple causes, so these events establish
one cause rather than characterizing every drop.

The isolated capacity build exposes the existing first-refusal
measurement in release. It records the title slot count, dynamic
capacity, request size, total free space and largest free
contiguous run. Its replay uses the same candidate source and
warm seed as the Series X smoke. The capacity comparison is
queued under the single-instance supervisor.

## 137. Descriptor fragmentation in the Series X release

The Series X capacity replay completed normally with the FullAdaptive
candidate and the same frozen shader seed. At the first dynamic-table
allocation failure, the host heap contained 1,245,184 title slots and
851,967 dynamic slots. It had 48,740 slots free in total and its largest
free run was 109 slots. A 133-slot table could not be placed and the
draw was dropped immediately. The allocated table is required to use
one contiguous run. This reproduces the fragmentation class observed
in §85. Both its array and raw-twin ABI and the configured heap partition
constrain simply raising the slot count.

The constant versioning experiment completed its B-A-A-B sequence
using the indexed Series S diagnostic build. In the two controls the
median title-read wait was 12.1 and 12.65 ms. It reached 0.0 ms in both
candidate legs. The comparable reverse pair ran at 102.0 and 100.1
ms/frame with 6,043 and 6,094 draws. One other camera view drew 8,507
draws/frame and the harbor comparisons reversed direction. The GPU
fixture passed with each of 32, 240 and 544 byte versions. Keep the
candidate opt-in while the broader FPS benefit is unresolved.

The Series S pre-upload probe compared the entire 21,045,760-byte
shadow in 30 sampled prelude submissions. It found zero GPU words
different from the old staging bytes among words unchanged in the
actual upload snapshot. The CPU-side sample reported identical staging
and guest bytes in all 87 snapshots. The GPU probe excludes samples
without the needed resource state or with prior dirty ranges, so the
observation does not imply that all full refreshes may be omitted.
Existing transfers remained active.

## 138. The meeyao 8 GB Linux fork and the pipeline-library fix

Source: meeyao/victory-8gb-linux main eaf72729. The README reports about
2 FPS at 1080p and 4.9 GB VRAM on an RTX 3070 with a Ryzen 7 3700X,
including 229 ms preparing and 149 ms copying back. Its preset drops
two texture mips and uses several NVIDIA-specific workarounds. The
supplied image shows 1.9 FPS and rectangular blending artifacts.
The workload is distinct from our run.

Commit 3e42a23 carries viyourmom 69cca8a for creating an empty D3D12
pipeline library. In a no-title fixture with the exact indexed Proton
vkd3d-proton DLLs, CreatePipelineLibrary returned E_INVALIDARG
(0x80070057) for a non-null pointer plus zero length. Passing a null
pointer and zero length succeeded and reported a 56-byte serialized
header. The working source path already preserves the early key list
when a host library is unavailable. The isolated release candidate
adapts only the empty-library invocation. The cold/warm replay copies
only the resulting library into a clone of the same original cache
seed for the second leg; its purpose is to establish that real
pipelines are saved and subsequently loaded, then measure any effect.

The Series X cold/warm candidate completed both 42-world-second legs.
The cold leg serialized a 292,664-byte library. With only that file
overlaid into a copy of the starting seed, the warm leg loaded the
identical library and reached 1,585 cached loads and one new creation
at the first main save. This verifies persistence on our Proton stack.
The main source checkout already uses a null pointer for an empty
library; the archived release replay source lacked that change.
Both legs still refused a fragmented dynamic table. This pair does not
establish a steady-state FPS gain.

A separate D3D12 fixture with the same vkd3d-proton DLLs successfully
created and released shader-visible heaps of 4,194,304, 5,242,880,
6,291,456 and 8,388,608 descriptors. Larger heaps are feasible on this
machine in isolation. Gameplay requires matching raw-twin and
view-metadata ranges on both the host and shader sides, a matching
translation cache fingerprint and a memory-usage check.

## 139. gleaf RX 7600 fork and Series X visual reference

Reviewed gleaf-6/VIctory-7900XTX- main d6432d2. Its README reports
6–8 rendered FPS on Windows with the Series S preset on an RX 7600.
About 15 displayed FPS includes generated frames, and the README
reports fast-motion ghosting. The current top commit changes the
8 GB buffer cap and retains a quarter-of-local-budget cap up to 8 GiB
for budgets at or above 20 GiB. Our Radeon candidate already applies
the same rule in that budget tier. The fork's FidelityFX and two-plane
composition are useful reference code; they are not evidenced fixes
for the artifact in our current game frames.

**Preferred bedroom visual reference: `harbor-x-pipeline-library-warm`.**
At 15:03:33 local time the warmed pipeline-library Series X release
replay captured the bedroom with detailed hair and skin, lit floorboards
and a pale rim above the hair. The window title reported 120.7 ms/frame
(about 8.3 FPS) and 9,374 draws/frame at capture. The user selected this
image as the visual reference for later FPS changes. Match the camera
and scene state before comparing skin, hair, indoor lighting and floor
reflections.

The RX 7900 XTX run used indexed Proton/vkd3d-proton on RADV, the
`fulladaptive-player/isolated-launch.sh` preset with `FORK_VISUAL_RUN=0`,
2,097,152 base host views with raw twins, and 851,967 dynamic slots.
The exact release executable is archived as `reference-victory.exe`
next to the image in the private run directory. Its SHA-256 is
`cff75f28d653222025820e6a1f1ff526f2650d76b44745453da067d6f2e513d2`.
The private seed is `warm-console-x-private-library-only` under
`amd-frame-profile`. Its 292,664-byte host pipeline library has SHA-256
`8376090521c92e22d3aaba6ee0ce917e8ae5bae1657909677ddd51b4ebf82d91`.
The original image and window metadata remain under
`amd-frame-profile/harbor-x-pipeline-library-warm` outside Git.
The image SHA-256 is
`c5e99890b37b1db683383ea658126a3143a5a60aaaff30c4e2e0bd13128bcd7d`.

## 140. Source of the bedroom wall luminance change

The Series S matched-wall diagnostic captured 26 ordered intervals
between adjacent hardware Presents. The four directional shadow passes
left the same packed value (0x00ff) in the selected wall patch. The
whole-scene temporal pass c89f7bd38af4f529 read the spatial colour from
slot s1+48 as 1280x720 RGBA16F and velocity from s1+32 as RG16F.
At frames 1200 and 1203 the arm edge differed by one input pixel.
Scene-linear wall luma was already 0.2091 vs 0.1259 before TAA and
0.2001 vs 0.1401 at its output. The static control's output was
0.0455 in both. This pair has the greatest incoming difference among
31 near-equal-edge pairs. It shows the colour variation is present
upstream of TAA. The source texture was resource 26659 in this run.
Only one native final-colour row capture and the +224 final image
landed, so exact displayed trail persistence remains unresolved.
The capture uses diagnostic readbacks and is unsuitable for FPS
attribution.

## 141. Wider host descriptor heap and matched replay

The isolated release branch can request a shader-visible heap of
8,388,608 entries. Both raw-twin base and view metadata extent
cover 4,194,304 base entries. The twin offset remains a power of
two because the translator uses a bit OR to form the twin index.
The wider mode falls back to the previously probed heap when
creation fails, and its replay requires proof of the wider mode.

AMD/RADV selected 4,194,304 base views with raw twins enabled. The
new translation fingerprint is 4dca61019d2b5496. Warming started
with 2,686 selected recipes, including a newly used Series X recipe
found by the pipeline-library pair. The matched release replay waits
for a second warm with no bad or outstanding recipes. Both legs
then use the same binary, seed, enlarged heap layout and shader
ABI, with 851,967 and 1,572,864 dynamic slots respectively. Each
leg captures one room image and logs allocation refusals and FPS.
No gameplay or performance result has been established yet.

The wider-layout warm finished all 2,686 selected recipes in 414 s.
An independent second warm reported 2,686 ready, zero bad and zero
outstanding, and the completed-key manifest contains every selected
key. The original shell gate returned status 1 because ANSI was
embedded in the raw-twin field. The corrected verifier stripped ANSI,
confirmed the exact layout and fingerprint, and started the replay
recovery without repeating the warm. The reusable warm script now
uses that verifier. The replay still uses the single game lock.

## 142. Ordered producers of the colour entering TAA

The Series S wall-source replay ended at its planned world duration
with status 0. It identified the TAA spatial-input texture by guest
object and host serial (25349 in this run) and retained command
submission order for two adjacent Present intervals. In each interval
there were 337 candidate write commands before the exact whole-scene
TAA consumer, with no event truncation. At observed frame 1190 the
candidates included direct material lighting, four deferred resolves,
then scattering_transmittance_system:CS_ApplyCombinedDeferred
(577248728544d74b), followed by transparent and light-volume draws.
The compute pass read 1280x720 depth and 480x272 scattering inputs and
bound the target colour texture as its UAV. Resource binding proves
only potential writes at the selected wall pixel. The next probe
needs bounded before/after pixel values across these passes.

The initial control/large pair completed both 48-second Series X legs
using the same binary, raw-twin layout, starting recipe cache and
descriptor allocations of 851,967 and 1,572,864 respectively. No
extra recipes appeared and both scopes recorded zero OOM kills. The
control's first refusal requested 185 slots with 10,074 free and 166
in the largest run. The larger setting logged no pool refusal, no
draw or indirect drops and no pipelines pending in the sampled
world reports. The control reported drops in the opening and room and
had 109–324 pipelines pending until the last reports. Periodic
opening medians were 311.25 and 125.1 ms, with median 13,292 and
12,988 draws. The final control reports after the pending count
fell to 3 and then zero were 118.8 and 115.7 ms. The larger pool
ran at 124.3 ms in the late window with a different camera and
about 6,414 rather than 9,239.5 draws. The opening timing
difference cannot be attributed to pool capacity alone. The photos
confirm a brighter in-transition control image and a more advanced
room scene in the larger-pool capture.

The control ran to completion even though the report script failed
afterward on a missing recipe manifest. A verified sidecar manifest
and resume-only second leg recovered the complete result without
another control run. A reverse-order pair has been queued using
the same serialized host pipeline library in both seeds to reduce
runtime compilation imbalance.

## 143. Descriptor refusals with the same warmed pipeline library

The reverse-order Series X replay used a byte-identical 436,856-byte
host pipeline library for both legs. The binary, heap layout, selected
recipes and settings matched except for the dynamic pool allocation.
All three opening reports per leg recorded zero pending pipelines.

| Opening reports | 851,967 slots | 1,572,864 slots |
| --- | ---: | ---: |
| Median frame time | 144.0 ms | 129.7 ms |
| Median draws per frame | 12,463 | 13,086 |
| Draw drops | 377 | 0 |
| Indirect drops | 654 | 0 |

The first control refusal requested 255 contiguous slots. The pool
had 20,190 free and the largest free run held 190. The larger pool
reported no refusal. This reproduces the draw loss from fragmentation
at a scene transition with no pending pipelines in the sampled reports.
The sampled opening median was 9.9% lower in the larger-pool leg.

The memory supervisor ended the larger-pool leg after its 15.806 s
world report when host MemAvailable fell to 2.972 GiB. Scope peaks
were 11.778 GiB with the larger pool and 11.465 GiB in the control.
Both recorded zero OOM kills. The control reached 47.764 s and its
six late reports had a 146.55 ms median with zero draw or indirect
drops. A repeat of the larger-pool leg was queued with the same library
and memory guard to measure the late window.

The larger-pool repeat then reached the planned stop. Its last complete
interval ended at 42.978 s and it captured the room. It recorded zero
descriptor refusals, draw or indirect drops, pending pipelines, device
loss matches and OOM kills. The peak game scope was 12.279 GiB and the
lowest host MemAvailable was 3.331 GiB. In the first two complete reports
of the repeated leg, the median was 127.75 ms with 13,370 draws per frame.
The same reports in the control measured 135.95 ms with 12,726.5 draws
per frame. In the common late window the medians were 127.0 and 146.9 ms,
with 6,625 and 9,083 draws per frame respectively. The late scenes had
different workloads and do not isolate an FPS gain from the heap size.
The room captures show different camera angles. Use the Series X visual
reference in §139 when collecting a matching view.

## 144. Material-lighting writer at the bedroom wall

The Series S stage replay sampled 20 complete Present intervals at the
same wall locations. The material mask, directional standard draw and
four deferred resolves preserved the sampled wall pixels. The
`TL_PS_NoDir_PBR_MATERIAL_STANDARD` pixel shader (48e74d16cbfa45e3)
changed all sampled wall pixels. Its output equaled the value entering
atmospheric scattering, which changed mean near-wall luma by a median
of -0.0020. The scattering output equaled the spatial input to TAA.

At frames 1184 and 1198 the spatial-colour arm edge was within one
input pixel. Near-wall scene-linear luma after NoDir rose from 0.976
to 1.695 and far-wall luma from 1.165 to 1.482. Nearby displayed RGB
also changed. The diagnostic identifies the wall's writer. Whether
the variation comes from its inputs, the translation or ordinary
scene lighting remains open. Readbacks affect frame times.

## 145. Submitted constant-slot fill census

The Series S census completed eight consecutive world frames (+224
through +231) without truncation. It counted 171,020 submitted
constant-slot fills across 135 submissions. The CPU route accounted
for 80,752 and the GPU preamble route for 90,268. Of all requested
addresses, 156,969 met 16-byte alignment without meeting 256-byte
alignment. The diagnostic resolved 162,666 fills to full physical
intervals across 470 pages.

Adjacent 64 KiB physical pages 335 and 336 of allocation generation
3 appeared in every sampled frame. They had 4,850 fill-page touches,
1,590 of which had no recorded writer, pending-flight or mapping
uncertainty flags. This gives a ceiling of 198.75 CPU copy elisions
per frame, or 0.93% of all fills. The counters do not isolate the
CPU-route subset of these touches. No elision is certified until the
alias and per-flight lifetime checks are in place. The experiment
would keep the scratch fallback, GPU preamble, ordinary shadow
refresh and copy-back paths active.

## 146. Intermediate dynamic pool and smaller physical heap

The Series X release repeated the §143 replay with the same verified
binary, 2,686-recipe seed, serialized pipeline library and 8,388,608
physical heap entries. With 1,048,576 dynamic slots it reached the
scheduled stop with no descriptor refusals, draw or indirect drops,
pending pipelines, device loss matches or OOM kills. Its last complete
report ended at 43.124 s of world time. The opening median was 134.9 ms
with 11,703.5 draws per frame. The late window measured 137.5 ms with
9,163 draws per frame. The full-run scope peak was 13.04 GiB and the
lowest host MemAvailable was 3.62 GiB. The smaller dynamic allocation
proved sufficient for this replay. The scope did not show a measured
memory saving from reducing the dynamic range inside the same heap.

The five late intermediate-pool reports recorded medians of 28.1 ms
and 155.4 MB per frame for title-memory preparation copies, 47.6 ms
and 30.2 MB for copying back, and 44.1 ms for GPU intervals. The clocks
overlap. Descriptor allocation and pipeline creation had no reported
drops in this window, leaving synchronization and data movement as
separate performance work.

The intermediate-pool room capture nearly matches §139's camera.
Horizontal-gradient matching of three static background patches
found zero integer-pixel shift at quarter-size, with correlations
0.900, 0.959 and 0.995. Both images retain visible hair strands,
skin detail and the lit floorboards. On the captured display images
the selected floor patch's median 8-bit code-value luma moved from
147.8 to 120.8, while the left wall moved from 142.3 to 156.6. The
pair establishes a visible lighting difference in these two images.
Scene animation, exposure and temporal history were not synchronized.

The compact-layout candidate sets a 3,145,728 base index and creates
6,291,456 physical entries, 25% fewer than the wide heap. The host already
uses addition for twin handles. The translator now selects addition
after the existing unsigned clamp when the base is not a power of two.
The ordinary and wide power-of-two layouts keep the OR instruction.
The shader fixture exercises a compact offset whose set bits overlap
the guest index and an out-of-range index that must read the last twin.
The compact mode is opt-in and the changed bounds have their own
translation fingerprint. Hardware fixture, warm cache and gameplay
measurements must pass before adopting the layout.

Both raw-twin GPU fixtures passed under the indexed vkd3d-proton
build on the RX 7900 XTX. The compact compute fixture verified the
raw SRV, the output UAV and the last twin on clamped access. The
warm selected 3,145,728 base views and the new fingerprint
`492c51ef804e98ae`. All 2,686 selected recipes built in 418 s,
with zero missing host pipelines and zero subprocess losses. The
verification pass found 2,686 ready, zero bad and zero outstanding.
Two serial gameplay legs use the same 1,048,576-slot partition as
the intermediate control. The first serializes a native library for
the compact layout. The second uses that library from startup.

Both compact legs reached the scheduled world duration with no
descriptor refusals, device loss matches or OOM kills. The cold leg
still had 282 pipelines pending in an opening report and dropped
470 draws and 825 indirect commands in the two opening reports.
The 397,688-byte native pipeline library from the cold leg loaded
in the second leg. The warmed compact leg had zero pending and
zero draw or indirect drops across all world reports.

| Same 1,048,576-slot allocation | Wide with native library | Compact with native library |
| --- | ---: | ---: |
| Opening median, two reports | 134.9 ms | 140.05 ms |
| Opening median draws per frame | 11,703.5 | 9,374.5 |
| Room median, five reports | 137.5 ms | 132.1 ms |
| Room median draws per frame | 9,163 | 9,141 |
| World scope peak | 11.76 GiB | 11.72 GiB |
| Full-run minimum host available | 3.62 GiB | 3.88 GiB |

Both warmed runs had their last complete timing report around world
+43 s. They used layout-specific native libraries. One pair supports
rendering through this route with the smaller heap. Opening workload
differences and run-to-run spread limit FPS attribution. The compact
image retains visible hair, skin, indoor light and floor reflections.
Static features aligned at zero integer shift at quarter size, but
one of three patch correlations missed the preselected 0.85 threshold
(0.825, 0.899 and 0.997). Keep the user-selected §139 image as the
visual reference.

An independent no-title D3D12 fixture used order wide, compact, compact,
wide on the same Proton/RADV stack. The local segment usage increased
256 MiB for 8,388,608 slots and 192 MiB for 6,291,456 slots after heap
creation, unchanged after filling every slot with copied null
descriptors. Both repeats matched exactly. This demonstrates 64 MiB
less local memory per shader-visible heap. It does not measure total
game RAM. The compact layout remains an isolated opt-in release.

## 147. Operands of the bedroom wall's material-lighting pass

The Series S probe authenticated and reassembled the exact NoDir
shader and preserved the original MRT store instructions and operands.
Strict DXV validation passed and the instrumented graphics recipe
57072 created on the RX 7900 XTX. The shader wrote private diagnostic
UAV samples while the host captured the corresponding render target
before and after the draw. The probe only accepts unique per-pixel hits
with a changed target and exact output reconstruction. The diagnostic
UAV can alter early-depth optimization, so timing is excluded.

The 20-frame run ended at its scheduled stop. It reconstructed 43,438
changed, single-hit pixels exactly after using observed toward-zero
RGBA16F export rounding. A nearest-rounding assumption matched only
2,713. On the new camera, frames 1190 and 1206 had arm edges within two
input pixels. Mean scene-linear luma at row 445, x=603..626, fell from
0.62485 to 0.57440. The multiplicative lighting term contributed
-0.03902 (77%), additive lighting -0.01054 (21%), sampled material
-0.00089 (2%), and the shared scale stayed at 0.0039062519. On the
farther wall (x=704..727), the lighting multiplier accounts for
-0.09817 of the -0.10274 difference. This identifies the changing
lighting inputs for these frames. It does not establish a defect
in their producer, or a root cause of all ghosting.

Across 83 qualifying overlapping pairs, multiplication had the
largest absolute effect in 76 near-wall pairs and in all 83 far-wall
pairs. The shared scale and chroma term stayed fixed across the
958 changed pixels in the two measured regions. Export rounding
was selected from 2,163 first-frame pixels and reproduced all
41,275 changed pixels held out in later frames.

## 148. Exact packed upload reuse and the faster Series S image

The guarded Series S diagnostic stopped at its planned 95 world
seconds. It captured 64 actual packed, 21,045,760-byte images of the
same shadow at world +195 through +258. None of the 63 adjacent full
images matched. Only the 8,704-byte tail matched in each pair, covering
548,352 of 1,325,882,880 bytes (0.0414%). All 321 full 64 KiB pieces
changed on every adjacent send. The exact comparison took 1.107 ms
total (17.57 us per pair) and snapshot collection took 70.075 ms
total (1.095 ms per capture). The existing upload remained active.
These results rule out reuse of the previous full upload or unchanged
64 KiB pieces for this measured buffer and scene.

The later room reports at about 10–11 FPS belong to this Series S
diagnostic, with about 6,000 draws per frame. They reported zero
draw drops and zero pipelines pending. The +210 image shows a dark
wall and stronger, fragmented reflections on the floor compared
with the preferred Series X room image. The camera and capture paths
differ. The current observation is insufficient to attribute the
visual variation to the console preset, diagnostic or lighting
history independently. The same Series X executable and starting
pipeline library from §139 were queued for a reference replay.

## 149. Intermittent dark sun rays on the Series X reference

Both reference replays used the archived §139 executable and the
exact same 292,664-byte starting native pipeline library. The
16:00 user observation reported intermittent black sun rays.
The single saved room image was taken at 16:00:26 and faced a
different direction than the preferred §139 screenshot. There
is no timed capture of the reported dark transition.

The 16:03 repeat added only an asynchronous desktop capture of the
game window. It captured 215 samples at 1280x720 over 18 seconds
starting at 16:03:02 local time, covering world +22 to about +40.
The user observed clean sun lighting during this replay, and the
chronological contact sheet shows no obvious black beam. Complete
room reports had zero draw or indirect drops and zero pending
pipelines. The host did record descriptor fragmentation earlier
in each launch. The timing does not connect those earlier refusals
to the observed indoor lighting. Both replays reached their
scheduled stop with zero OOM kills.

The new run retains `rays-burst.mkv` and the timestamped window
metadata in the private `harbor-x-reference-sun-rays` directory.
The 16:03 replay is a negative observation for the intermittent
artifact. A positive frame and its preceding render-pass state are
needed to establish the source.

An older ordered Series X capture links the flare-geometry merge to
the flare mask and then to the masked glare pass (d141233c6ef29641).
The glare pass and the lens-ghost pass (9b75e0fbdd13c3aa) used
additive blending into the same 640x360 R11G11B10F target. A zero
mask contribution leaves a positive destination intact. Sampling
that target before and after glare, as well as the later scene
composite, can distinguish missing flare from a darkened scene on
a positive replay.

## 150. Series X immutable constant versions and landing pressure

The Series S positional-copy candidate from §137 applied to the compact
Series X release with no shader changes. Four serial off/on/on/off legs
used one executable, 1,048,576 dynamic slots, the same 397,688-byte
starting native pipeline library, indexed Proton and the normal BVH
path. The release observer logged completed DXGI Presents for 85
equal relative harbor intervals and 120 room intervals per leg.
There were no missing indices, failed Presents or reported draw drops
in those windows. Each image was requested at world +224.

| Leg | Harbor FPS | Room FPS | Room P95 | Room draws/frame | Read wait median | Admitted slots |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Off A | 7.240 | 8.614 | 154.530 ms | 9,031.4 | 32.2 ms | 0 |
| On B | 7.088 | 6.982 | 225.377 ms | 9,002.4 | 53.7 ms | 392 |
| On C | 6.816 | 8.216 | 203.097 ms | 6,580.4 | 19.9 ms | 368 |
| Off D | 7.707 | 6.466 | 172.751 ms | 6,448.7 | 83.5 ms | 0 |

The FPS and title-read effects reverse between pairs. Room P95 was
higher in both enabled legs. A and B match static background geometry
within a quarter-resolution pixel. The floor patch median displayed
luma was 151.5 vs 35.6, with differences in exposure, pose and floor
reflections. C and D have different camera views. Keep the original
player preset for quality and the positional candidate opt-in.

The combined queue pressure counter had room medians of 0, 12.7,
16.4 and 0 ms. Source review found it counts both the guest list cap
and full drains when the 1 MiB GPU compare-result area exceeds half
of its linear allocation. That metric alone gives no basis to raise
the list cap. The isolated follow-up measures those two waits
separately without changing the settle predicates.

The last complete off-A card-traffic report recorded 1,725 fetched
64 KiB pieces, 110.6 MB and 30.5 ms of relay fetch waits per frame.
The nearby landing output combined card and host jobs. The follow-up
separates their exact 4 KiB equality, change and uncertain buckets
for 32 frame indices while keeping every original transfer.

The four legs selected three additional recipes already present in
the book. A second warm of the private, expanded Series X seed
reported all 2,689 selected recipes ready, zero bad, zero outstanding
for fingerprint `492c51ef804e98ae`. All used keys across the four legs
are covered by this seed.

## 151. Separate Series X list pressure and card readback bytes

The release probe added separate counters to the existing settle
predicates. In nine fully contained room reports with immutable constants
enabled, there were 44 waits at the 256-list cap, with up to 268 live
lists and a 10.08 ms per-frame median. The GPU result-area threshold
did not trigger. The wait arises at the list limit in this replay.
An isolated 320/256 pair uses the same executable, warm seed and
capture marker to measure the effect, with page collection disabled
for both legs and the ordinary 3 GiB host reserve in force.

Across 32 room frame indices (+246 through +277), the device-local
fetches transferred 3,839,392,316 bytes (119.98 MB per frame).
Of those, 1,951,053,788 bytes (50.82%) covered eligible 4 KiB
fragments that already matched the reference at landing, and
1,878,282,088 bytes covered pages with at least one changed byte.
The remaining 10,056,440 bytes were excluded by the existing
CPU-dirty or later-send checks. There were no uncertain fragments.
The host-visible jobs contributed 376,145,856 separate bytes.
Mean fetch wait was 40.02 ms per sampled frame. It includes work
overlapping on other threads and does not scale directly with bytes.

This is a bandwidth opportunity, subject to proving equality for
the exact version read. A mask computed on an earlier graphics
submission does not establish the contents seen by a later copy
queue read of the mutable source. An eligible selective transfer
needs the mask and immutable bytes captured in the same ordered
GPU work, with held-page generations and the existing fallback.

The no-census follow-up ran 320 then 256 on the same attributed
release build with the positional candidate enabled. Both matched
windows contained 85 harbor and 120 room Present intervals. In the
room the 320 leg recorded zero list waits and the 256 leg recorded
41, with a 5.95 ms per-frame median. Neither reached the result-area
threshold. Room throughput was 8.731 and 8.756 FPS, P95 177.14
and 171.17 ms, with 6,486 and 8,962 draws per frame. Harbor was
7.517 and 6.230 FPS with 13,018 and 13,709 draws per frame. The
room images have different camera views. Minimum host available
memory was 4.03 and 4.67 GiB. Both stopped at their requested
Present marker with zero OOM kills and zero descriptor refusals.
The higher cap removes the observed list wait. This pair does
not establish an end-to-end room FPS improvement.

The census and cap replays selected four more recipes already in
the book. The expanded private landing seed verifies all 2,693
observed recipes on fingerprint `492c51ef804e98ae`; there are
zero bad or outstanding recipes in the selected set.

## 152. Immutable sparse readback fixture on the RX 7900 XTX

The no-title D3D12 fixture validated both compute shaders with DXV,
then ran on the indexed Proton/RADV stack. It compared exact bytes
with a known private baseline and captured selected 4 KiB pages
in a packet owned by the earlier graphics generation. A separate
graphics submission deliberately overwrote and restored the live
buffer before the independent copy queue fetched anything. The
CPU reconstructed the earlier image bit for bit. It also verified
the distinct overwrite and the restored live buffer and rejected
the wrong owner or generation.

Sparse, overflow-to-whole, unknown-baseline-to-whole and zero-change
cases all passed. The short final page is clipped. In three runs
with approximately 50% of a 16 MiB buffer changed, the header and
payload carried 8,409,120 bytes, versus 16,777,213 bytes for a
separate full copy. GPU timestamp medians were 0.03572 ms for
classification and 0.01972 ms for packing. The host intervals
from closing each copy list to observing its fence had medians
of 0.5454 ms for the header, 1.5684 ms for the payload and
2.7628 ms for the separate full copy. The header includes the
queued graphics wait. These are fixture intervals. The normal
game relay, lifetime, merge and recovery paths still need a
bounded integration and a matched gameplay replay.

Source fixture: `checkpoints/7900xtx-readback-prototype`.

## 153. Scarlett comparison and Bayside Morning preparation

Inspected Op-lite/victory-scarlett `scarlett-fox` at `7c8f019`,
based on codec-d/cool_fox `f4b48ad`. The reported 41 ms case was
on Windows with an RTX 4080 SUPER and 1,000–2,500 draws per frame.
The project reports substantially more memory commit than our
guarded Linux scope. Its settings and frame rates are evidence for
that machine and workload.

The local compact Series X release already has the opt-in
`VICTORY_FILL_TITLE` heuristic. It bypasses some in-flight waits for
small ranges that the CPU wrote or that recently had no shader
landing. This does not prove that a pending shader cannot write the
requested bytes. Tested off and then on with the same binary and
2,693-recipe warm seed, 1,048,576 dynamic slots, 256-list limit and
positional snapshot candidate disabled.

| Series X measurement | Off | On |
| --- | ---: | ---: |
| Harbor FPS, 85 intervals | 8.389 | 8.494 |
| Harbor P95 frame time | 136.778 ms | 161.942 ms |
| Harbor median title-read wait | 11.8 ms | 3.25 ms |
| Room FPS, 120 intervals | 8.380 | 7.561 |
| Room draws per frame | 6,576.4 | 6,568.1 |
| Room median title-read wait | 52.5 ms | 70.7 ms |

Both legs completed, with no reported draw drops in the sampled
windows and zero scope OOM kills. The user judged the 16:37 on
replay's picture acceptable. The original room image is in the
private `harbor-x-scarlett-fill-title-on` record at 16:37:02.
The images retain hair, skin and floor reflections with visibly
different lighting. A single pair does not establish a rendering
change caused by the switch. Leave it off for the comparison
baseline. The partial-mip delta and bake revision are separate
texture-streaming candidates in Scarlett. They require isolated
tests of the resource version and the startup or streaming workload.

KLBF Lab 63 beta-2 `21c4f7d` uses the 9-bit primitive index fix
`7209bdd`. The current compact translator already strips the
edge flag with `0x1FF` and a stride of ten bits. The local Bayside
launcher and scorer are in `checkpoints/bayside-morning`. Its
isolated release compiled and the scorer passed checks for the
duration, slow tails, one-second windows and failed Presents.
It measures host DXGI completions. KLBF Lab measures at its title
Present callback with a black-frame check and its own certificate.
The local build still needs a gameplay validation from the
specified private slot-1 morning save before publishing a score.

## 154. 4 KiB sub-page hash masks in `CARD_COMPARE` (`VICTORY_CARD_PAGE_MASK`)

To cut the 50.8% equal-page readback overhead measured in §151 without adding
extra VRAM snapshot buffers or a second copy-queue fence round trip, the
isolated Series X candidate (`series-x-page-mask`, patch in
`checkpoints/7900xtx-card-page-mask.patch`) adds an opt-in 4 KiB sub-page
variant of `CARD_COMPARE` (`VICTORY_CARD_PAGE_MASK=1`, automatically disabled
when `VICTORY_FETCH_CHECK=1`). The same 64-thread compute group per 64 KiB
piece hashes all 16 sub-pages (4 threads per 4 KiB page, 3-step per-page
reduction), stores 16-byte hashes per 4 KiB sub-page (`256 B` per 64 KiB
piece, adding `3.0 MB` of VRAM across ~5.8 GB of card copies), and writes
`(mask << 1) | (mask != 0)` into the existing 32-bit `differs` slot. In
`relay_card_back` (and `card_back`), masks for multiple ranges sharing a piece
within one plan are ORed together, and each flagged piece expands on the stack
into at most 8 contiguous 4 KiB runs intersected with `[lo, hi)` before
`land::ordered` merges adjacent runs across flights.

Verification on the RX 7900 XTX (`victory-gpu` release tests under Wine/RADV)
passed 4/4 tests (`0.09 s`), including `warp_card_compare_pages_tracks_4k_page_masks`
(aligned and skewed starts, mode 0/1/2 hash updates and flip-back detection)
and `page_masks_or_across_ranges_and_clip_to_subpages`.

Controlled four-leg ABBA Series X replay (`off`, `on`, `on2`, `off2`) used
one binary (`e680fc6bd728d9c5d888d0290fb99b07cb8e9ddfedf3e26228462be5d8d13ec1`),
the 2,693-recipe compact warm seed (`warm-console-x-compact-landing-union`),
3,145,728 base views, 1,048,576 dynamic slots, 256-list cap, `VICTORY_FILL_TITLE=0`,
`VICTORY_PLACED_CONSTANTS=0`, and indexed Proton. All four legs completed with
zero draw/indirect drops, zero descriptor refusals, zero device removals, and
zero OOM kills (scope peak 11.94–13.22 GiB, min host available 3.78–4.64 GiB,
peak VRAM 15.33–15.64 GiB).

| Leg (`off`/`on`) | Harbor FPS (85) | Room FPS (120) | Room median | Room P95 | Room draws/f | Read wait | Card fetch MB/f | Landed 4 KiB pages | CPU merge |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `off` (A1) | 8.492 | 7.590 | 131.72 ms | 150.38 ms | 6,593.7 | 60.70 ms | 124.60 MB | 33,899 | 8.39 ms |
| `on` (B1) | 7.981 | 9.187 | 108.14 ms | 131.98 ms | 6,378.4 | 43.90 ms | 56.60 MB | 17,179 | 6.00 ms |
| `on2` (B2) | 6.796 | 9.569 | 104.04 ms | 131.07 ms | 6,584.7 | 41.25 ms | 60.45 MB | 18,105 | 6.95 ms |
| `off2` (A2) | 6.451 | 8.316 | 117.29 ms | 148.78 ms | 6,504.1 | 52.55 ms | 120.45 MB | 32,855 | 7.94 ms |

Across both forward and reversed pairs, `VICTORY_CARD_PAGE_MASK=1` cut card
readback bandwidth by **52.2%** in the room (124.6/120.5 → 56.6/60.5 MB/frame)
and **55.6%** in the harbor (108.1/113.5 → 49.8/48.7 MB/frame), raising
useful changed-page density from ~50% to **90.6–91.6%** and cutting room CPU
merge time by ~21% (8.39/7.94 → 6.00/6.95 ms). Both enabled legs beat both
controls in the room: mean room throughput rose from **7.953 to 9.378 FPS
(+17.9%)**, with P95 falling from 150.4/148.8 ms to **132.0/131.1 ms** and
title-read wait dropping by 11.3–16.8 ms. Harbor throughput was neutral
(mean 7.472 vs 7.389 FPS, draw-dominated at ~13.3k draws/frame). On the
matched `on2`/`off2` pair (both origin frame 970, static-feature alignment
dx=0, dy=0, correlations 0.885/0.884/0.935), `on2` reached **9.569 vs 8.316 FPS
(+15.1%)** with clean room rendering (skin, hair, blinds, floor reflections
and HUD intact). Note that relay copy-queue wait time itself remained ~27–29 ms
despite half the bytes because contiguous-run splitting increased copy calls
(~1,900 → ~3,550 pieces/frame), leaving copy-queue batching or sparse-packet
packing as a further follow-up.

## 155. Checkpoint: Series X room at 19 FPS with a picture; renderer port 1 renders black

Continues the session after §154 (MSVC target, `keep_ref`, GPU phase meter,
then the `fps20-ports` tree built on `avatarmanz-7900xtx` `c9c932c`). All
numbers are RX 7900 XTX, Series X profile at 3840x2160, indexed Proton,
single runs, room window of 367 presents from `room-presents.tsv`.

| Run | Tree | Renderer ports | Picture | Median | Median FPS | P95 | Dropped draws/frame |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: |
| `harbor-x-ambient-placed` | `series-x-msvc` | none in tree | clean | 59.01 ms | 16.95 | 104.4 ms | 304–312 |
| `harbor-x-ports-bisect-none` | `fps20-ports` | all off | visible, skin patches | 52.41 ms | 19.08 | 81.3 ms | 329–331 |
| `harbor-x-ports-bisect-p1` | `fps20-ports` | port 1 only | black | 45.19 ms | (22.13) | 82.7 ms | 305–320 |

- The step from ~9 to 17–19 FPS is `VICTORY_BVH_BUILDER=0`: per-command GPU
  timing (`harbor-x-gpu-timing`) put 73.4 ms/frame in six software ray-tracing
  compute pipelines (`6d7fe7745ab9c6d8` 44.0 ms, `f22a76fcb967ee59` 14.9 ms).
  It is a lighting trade-off, not a free win.
- Every `fps20-ports` run with renderer port 1 on was black with sound
  (`harbor-x-ports-world`, `-nocbv-1`, `-20fps`, `-bisect-p1`; mean luma 3/255,
  "0.0 on the GPU"). The 25–28 FPS and the `preparing` −67 % reported from
  those runs are void. With all `VICTORY_PORT_*` at 0 the same binary shows
  the room (mean luma 106–111). Port 1 alone is enough to lose the picture;
  ports 2, 6, 7 are untested alone.
- `fps20-ports` carries `victory-shader`, `shader_cache.rs`, `translate.rs`
  and `Cargo.lock` from `series-x-msvc` and a hard-coded `VICTORY_TRANSLATOR`
  (`5e3c62c3…`) so the old seed `492c51ef804e98ae` loads. That defeats the
  cache key; revert it and warm a seed for the tree before judging pictures.
- 300–330 draws/frame are dropped "without a host pipeline" in the room even
  in the clean `series-x-msvc` run. Cause open.
- `series-x-msvc/bin/victory.exe` was a symlink into the shared target
  directory and now resolves to the `fps20-ports` binary (`99da1391…`);
  `e642971e…` must be rebuilt to be re-measured.

Patches, launchers and hashes: `vfork/checkpoints/7900xtx-room-19fps-20261126/`.
Not an ABBA result and not yet 20 FPS. Next: why port 1 presents nothing
under Wine/RADV, an honest warm seed for the ports tree, then ABBA.

## 156. The black picture: amdgpu userptr refuses Wine section views (fixed)

Renderer port 1 maps the title's 32-bit graphics memory once more and opens
it with `OpenExistingHeapFromAddress1`. With `VKD3D_DEBUG=warn`
(`harbor-x-ports-p1-vkd3dwarn`) vkd3d-proton 3.2 logs `Failed to import host
memory, hr 0x8007000e` for the 193,986,560-byte stretch, then gives the heap
a fresh empty allocation and returns success; the card reads zeroes for the
title's constants (black picture, sound, "0.0 on the GPU").

The refusal is `AMDGPU_GEM_USERPTR_ANONONLY`, hard-coded in
`libdrm_amdgpu`'s `amdgpu_create_bo_from_user_mem` (flags 0xe). A direct
ioctl probe on kernel 7.3.4 / RX 7900 XTX: with 0xe private anonymous OK,
shared anonymous / shared memfd EPERM (as in §60); with 0xc all four OK,
including a second view of the same memfd. A private copy of
`libdrm_amdgpu.so.1` (2.4.134) with that one byte changed, first in
`LD_LIBRARY_PATH` for the title only, removes the failure: 0 import failures
and a picture in `harbor-x-ports-p1-userptr` (port 1) and
`harbor-x-ports-1267-userptr`, `-userptr-2` (ports 1, 2, 6, 7). This also
lifts the "vkd3d-proton not importing section pages" blocker of §34–35.

Room, Series X 3840x2160, single runs, different cameras: 58.48 ms median
(17.10 FPS; second half 18.74; log 18.7 and 19.3 fps) and 62.10 ms (16.10;
second half 17.76). No measured gain from the ports over the ports-off
19.08 FPS of §155: copies ~19–24 ms and copying back ~21–25 ms per frame
are unchanged. The CPU savings reported from the black runs were an artefact.
Small bright moving specks seen in the dim view are in the pre-ports
reference and ports-off pictures too.

Script, probe and launcher: `vfork/checkpoints/7900xtx-room-19fps-20261126/`
(`make-libdrm-shared-userptr.py`, `userptr-probe.c`, `launch-ports.sh`).
Next: why port 1 leaves copies and copy-back where they were; a warm seed
for `VICTORY_PORT_CBV`; ABBA.

## 157. Checkpoint: Series X room holds 20+ FPS with a picture (off-on-on-off)

Four 90-second legs in the world (`PORTS_LONG=1`), `fps20-ports` binary
`95067db4…`, 2,693-recipe seed, Series X 3840x2160, `VICTORY_BVH_BUILDER=0`,
private libdrm (§156) in all legs, same camera (6.5k draws/frame). Title log
reports from the room view on, first two left out.

| Leg | Ports 1, 2, 6, 7 | Reports | Median | FPS | P90 | Reports ≥ 20 FPS | Copies | Copying back |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `harbor-x-steady-off1` | off | 13 | 46.2 ms | 21.65 | 47.4 ms | 13 | 17.0 ms | 21.3 ms |
| `harbor-x-steady-on1` | on | 12 | 49.5 ms | 20.20 | 54.2 ms | 7 | 18.8 ms | 22.1 ms |
| `harbor-x-steady-on2` | on | 12 | 48.6 ms | 20.58 | 52.5 ms | 9 | 17.8 ms | 21.7 ms |
| `harbor-x-steady-off2` | off | 13 | 47.8 ms | 20.92 | 49.3 ms | 12 | 18.3 ms | 22.0 ms |

No device loss, no failed host-memory import, a picture in every leg, ~310
draws/frame dropped in every leg. Both off legs beat both on legs: the
renderer ports cost about 1 FPS on this stack, so ports off is the default.
The earlier 17–19 FPS windows (§155–156) included the texture streaming
right after the load.

Negative results: port 1 widened to all title memory
(`VICTORY_PORT_BELOW_GB=131072`, 72 regions, 1028 MB) ran at 7–8 FPS with
41–63 ms on the GPU; `VICTORY_INPLACE_PROBE=1` removes the device on RADV.

Open: the lighting is flat and washed out without the ray-tracing passes
(user's console reference: warm, contrasty room); 310 dropped draws; the
forced cache key in `fps20-ports`.

## 158. Ports one at a time; reflections held (`VICTORY_AMBIENT_REFLECT`)

Series X 3840x2160, `fps20-ports`, `VICTORY_BVH_BUILDER=0`, 90 s legs, title
log reports from the room view on. The scripted start lands in view A
(~6.5k draws/frame) or view B (~9.0k); only legs of one view are compared.

View A: none 46.2 ms (21.65 FPS); port 1 51.6 ms (19.38); port 2 46.1 ms
(21.69); port 6 45.4 ms (22.03); ports 2+6 44.6 ms (22.42); ports 2+6+7
43.6 ms (22.94). View B: none 51.0 and 51.5 ms; port 7 50.3 ms; ports 2+6
49.5 ms. Port 1 alone costs ~5 ms: with the userptr import working (§156)
the card reads the constants from the PC's memory. Ports 2, 6, 7 without it
gain 1.5–2.6 ms. Single legs; same direction in every pair. Launcher default:
2, 6, 7 on; 1 and 5 off.

Free roam with ports off, 9 min 15 s (`harbor-x-play-3`): 49.7 ms median
(20.1 FPS), p90 57.1 ms, no device loss. The supervisor's 3 GiB host reserve
ended two earlier sessions at 2.5 and 4.7 min (`SUPERVISOR_RESERVE_GIB`,
new, lowers it; the user chose 1.5 for play sessions).

Where the frame goes (ports off, 47.9 ms): the submitting thread works
44 ms; the card has nothing queued 18.5 ms of the frame; copies 20.8 ms
(86.7 MB), descriptor heap records 14.5 ms (578,560 slots looked at),
landing 21.9 ms on its own thread, write watch and guards 8.2 ms.

`VICTORY_AMBIENT_REFLECT=<multiple>` (new, in `ambient.rs`; after
VIctory-Belobug-AMD 44dcd43): the title's reflections are held to a multiple
of the stand-in's slow mean of the room's light after `CS_UpdateHistory`.
The program is dispatched through `ExecuteIndirect` here, so the stand-in's
hook now runs after indirect dispatches as well. 0.2 too dark, 0.5 removes
the white glaze from the wooden floor (launcher default), 1.0 leaves some.
`VICTORY_AMBIENT_AIR=1|3` showed no visible change. Screenshots only.

Open: 300–330 dropped draws a frame (peaks of 1,552 while walking), objects
appearing late at close range, 5,490 textures waiting for memory (mean wait
7.7 s), the forced cache key.

## 159. Port 2 flickers; soft reflection bound; garbage-coloured light dropped

- `harbor-x-play-4` (ports 2, 6, 7; hard reflection cut 0.5): 46.2 ms median
  (21.65 FPS) walking, but 10,903 port-2 fallback records and 200 textures
  changed after upload; the player saw "disco" on glossy surfaces and
  blinking small shadows, and a floor with no sheen at all.
- `harbor-x-play-5` (ports 6, 7; soft bound 1.0): 45.9 ms median (21.79 FPS,
  p90 49.0), 0 fallback records, 9 texture changes. Port 2 defaults off.
- The hold is now `c / (1 + luma / cap)` with cap = multiple × the stand-in's
  slow mean of the room's light; launcher default 1.0.
- Magenta and near-pure-colour light is dropped from the resolve's leftover
  and from the reflections (after VIctory-Belobug-AMD e942896 for magenta).
  Before: specks on paper, and a single purple pixel blooming into a star.
- Open: white glass table top, pale skin patches after the load, the
  character gains clothes on leaving the house (cause unknown), ~300 dropped
  draws. Belobug's `rx9070xt` branch is this same base plus three commits
  (driver compiler guard and HUD, title textures uploaded in the submission
  prelude against flicker, FSR 4); not ported.

## 160. Dips while driving are pipelines made on the way; Belobug's prelude uploads

- `harbor-x-play-6` (driving, 9 min 44 s): 50.1 ms median, p90 67.7 ms, 5 of
  112 reports under 10 FPS. In slow seconds the title was not submitting
  (card idle 174 ms/frame, title threads 144 ms at presents) while 604 new
  host pipelines were made and 78–104 textures/frame read in. PSI showed no
  memory stall (scope 13 of 14 GiB, VRAM 21.5 of 24 GB).
- Runs start from a fresh copy of the seed, so made pipelines were lost each
  time. `harbor-x-play-7`, seeded from `harbor-x-play-6`
  (`run-ports.py <tag> <previous tag> series-x`): 49.7 ms median at 10.4k
  draws, p90 53.5 ms, 0 of 82 reports under 10 FPS, 2 pipelines made. Single
  sessions, routes differ, build changed too.
- VIctory-Belobug-AMD `e942896` and `5d6bd84` cherry-picked onto the tree
  (`-x`; one conflict, the magenta filter). `VICTORY_TEXTURE_MAX=0` in the
  launcher. The player called the intro the smoothest so far; the five-second
  reports show 33.3 ms before and after.
- Picture, open: glass (aquarium, coffee pot, table top) shows noise or white;
  car paint on one door only; black ovals on a road; blinking small shadows;
  the character gains clothes on leaving the house.

## 161. The 300 dropped draws are the ray-tracing builder; six shadow-ray programs hang in the compiler

Diagnostics build of `fps20-ports`, room (`harbor-x-diag-drops`): 308 drops a
frame = 302 stages of the ray-tracing structure builder (off by
`VICTORY_BVH_BUILDER=0`) + 6 compute pipelines on `warm-bad.txt` ("hung for
180 s" in a warm on RADV). No graphics draw is dropped in the room.
`VICTORY_SHADER_NAMES=1` (new) logs name and hash of every program; the six:
`CS_TraceShadowRays_Checkerboard` (559d9f01…),
`CS_LaserLightShadowRaytracing` (32ed072b…),
`CS_RTDistantShadowTrace32_Tiled` (a1b20bc4…), `…LeakPlug32_Tiled`
(e8c96235…), `…TemporalUpsample4X32_Tiled` (17829ac7…),
`…LeakPlugTemporalUpsample4X32_Tiled` (afeea48b…). Their outputs are never
written: candidate cause of blinking small shadows and black ovals on roads.

Glass (aquarium: sparkling noise, worse up close; coffee pot: coloured
noise; table top: white) is not a dropped draw: `CS_RayPrePass_Alpha`,
`CS_ShadeAlpha`, `CS_AlphaTransmissionDenoise/HistoryResolve/Blur*` and
`CS_RefractionRayEstimation` all run and read ray results nothing produced.
Name lists: `@SCRATCH@/fps20-profile/shader-names.tsv` (19,104)
and `bound-program-names.txt` (1,184 bound in the room); not in git.

## 162. Glass noise is the alpha ray pre-pass; Linux fork's write guards; DualShock 4

- New switches: `VICTORY_VIEWS_OF` (logs a named program's table images),
  `VICTORY_CLEAR_AFTER` (zeroes a 2D image after a named program),
  `VICTORY_SHADER_NAMES`. Glass: `CS_AlphaTransmission*` work from scene
  colour; the noise is `CS_RayPrePass_Alpha` (outputs (1,112) R32F, (1,120)
  RG16F, (1,128) R16UI, (1,136) R32UI, 1280x720), zeroed by default now; the
  sky-probe fallback `CS_DeferredFallbacksAlpha_SkyProbe` (1,48) R11G11B10F
  is bounded softly like the opaque reflections. Player: noise gone, glass
  white after step one; step two unjudged.
- kroeeee-eeee `f5c30ef` cherry-picked: write watch and guards 8.2 → 1.8
  ms/frame; 44.5 ms (22.47 FPS) at 6.6k draws with ports 6, 7.
  `VICTORY_COPY_COMPARE=1` crashes the title at world load in this tree.
  Their port 1 under Wine uses private pages instead of a patched libdrm.
- `VICTORY_AMBIENT_SHADE=0.4,0.3` against black knuckles (unjudged).
- DualShock 4: `PROTON_DISABLE_HIDRAW=<Sony ids>` in the launcher (probe
  only).
- `harbor-x-play-10`: 52.4 ms median, p90 87.2 ms, 6 of 62 reports under
  10 FPS, 126 pipelines made: dips again, cause open.

## 163. Shadow-ray programs compile with the single-latch change; own A/B of the picture fixes

- `VICTORY_RETRANSLATE=<hash>,...` (new) translates named shaders again
  whatever the cache holds. With it and `VICTORY_WARM_BAD=0` the six
  shadow-ray programs of §161 all get host pipelines (`harbor-x-shadowrays`,
  49.1 ms at 8.0k draws, no hang). They hung because dxil-spirv recursed on
  loops with several back edges (kroeeee-eeee `f5c30ef`). Launcher defaults.
- Scripted walk (`VICTORY_KEYS` with `W` held) into the living room with
  captures: fixes on (`harbor-x-walk1`) wood floor with a sheen, aquarium
  glass over water; fixes off (`harbor-x-hair-plain`) whitewashed floor,
  sparkling aquarium, white table top. Glass passes ≈ 0.7 ms/frame.
- Open: dark patch on the elbow (weaker crease shade did not help; player),
  a glass bowl's noise, hair missing once in `harbor-x-walk1`, pastel skin
  indoors, first-visit dips while pipelines are made (126 in
  `harbor-x-play-10`; the title knows 88,596).
- DualShock 4 confirmed working in the title by the player.

## 164. The phone's noise: `VICTORY_ABSENT_CACHE` (kroeeee `f5c30ef`) loses writes in this tree

`phone-shot.sh` scripts `Up` (the phone) and captures. Phone is noise with
launcher defaults, with `VICTORY_AMBIENT_REFLECT=0`, with
`VICTORY_CLEAR_AFTER=` and with both; with `VICTORY_ABSENT_CACHE=0` the
screen's clock, reminders and icons are drawn (some sparkle left). The
remembered uncommitted regions (8 frames) miss memory this tree commits
another way. Launcher: `VICTORY_ABSENT_CACHE=0`; write watch goes back to
~8 ms. Probably also behind hair missing in two captures and glass
flickering good/noisy since §162; not shown.

Shadow-ray programs (§163) back off: all six made the phone and glass worse
while small shadows blinked less; `CS_TraceShadowRays_Checkerboard` alone
made arms darker (player). Seeds `seed-shadow-off` (all six on the bad
list) and `seed-shadow-checker`.

rorodev6/victory-megafork-amd is Belobug `cd8dfc0` plus docs, Windows 10
Agility loading and a worker-panic fix (`af875f2`); nothing for this stack's
picture. Its queue names fxceless #1226 (prebuilt pipelines), #1031/#1210
and Cool Fox's character upload order (hair, faces).
`VICTORY_FRAME_INTERVAL_MS=16.667` (in this tree) would lift the 30 FPS cap
of menus and intro; untested here.

## 165. Port 1 with huge pages from inside the process: 39.3 ms (25.45 FPS), one leg

`tools/userptr-thp/libuserptr-thp.so` (source in the vfork checkpoint): an
`LD_PRELOAD` shim around `amdgpu_create_bo_from_user_mem` that collapses the
userptr range into 2 MiB pages (`MADV_COLLAPSE`) when it is made and every
15 s; no `CAP_SYS_NICE`, no sudo. Idea from kroeeee-eeee `47fd862`.

Legs of 90 s, heavier view (8.0–8.4k draws), ports 6+7: port 1 off 51.2 ms
(19.53 FPS); port 1 on 49.0 ms (20.41); port 1 on with the shim **39.3 ms
(25.45 FPS)**, p90 40.3 ms, copies 12.6 ms, picture present, 0 failed imports
(`VKD3D_DEBUG=warn`), 92 of 92 pieces collapsed. A first version of the shim
returned an error for want of the real symbol and gave 33.3 ms on a black
picture: void (the trap of §155 again; always look at the capture).

Player on `harbor-x-play-14` (`VICTORY_ABSENT_CACHE=0`): phone fine outdoors,
wrong indoors; aquarium and glass still flicker, without the "disco"; hair
gone after a while outdoors; elbow black when bent. Open.

## 166. Precompiled seed; card memory; host RAM in the RAM disk; upstream survey; hair is strands and comes and goes by the run

Night of 2026-11-26/27, RX 7900 XTX, Series X 3840x2160, `fps20-ports`,
`VICTORY_BVH_BUILDER=0` unless said. Test harness `shot.sh` (in the export's
`scripts/fps20-ports/`): scripted start, captures at world +40, +52, +62 and
+70 s (phone up with `Up` at 92 s of the run), medians over the title's
five-second reports of over 3,000 draws. The scripted start lands in view A
(~6.4k draws/frame) or view B (~9.0k); only legs of one view compare.

- **Precompiling is not the cure for the picture.** `precompile.sh` makes
  `seed-precompiled-full` from `seed-shadow-off`: `victory shaders` 25,787
  translations in 85 s (198 containers hold no vertex, pixel, compute or mesh
  program), `victory warm` 85,933 host pipelines in 234 s, none refused. A
  player session on it (`harbor-x-play-15`) had 0 shader misses and the same
  phone indoors and the same lost hair.
- **Card memory.** `harbor-x-play-15` (`VICTORY_TEXTURE_MAX=0`): 4 → 18 GB in
  the first minute in the world, peak 22.0 GB of a 22.3 GB budget, 4.2 GB in
  shared memory at three minutes. `harbor-x-play-16`
  (`VICTORY_TEXTURE_MAX=2048`): settles at 20.7–21.0 GB, nothing shared, and
  the character had no hair, no eyebrows and an untextured head at 18 min.
  A full card is not why hair goes.
- **Host RAM was in the RAM disk.** `/tmp` (tmpfs) held 7.9 GB of build trees
  and prefixes, most of it pushed into the 8 GB zram swap; available RAM
  fell to 1.3–3.3 GB in sessions and the supervisor's reserve or the
  session's memory reaper ended them. `@SCRATCH@` and
  `@WORKTREES@` now live on disk (`../ramdisk-moved/`, symlinked from
  `/tmp`): tmpfs 2.0 GB, 21 GB available at rest, 8.8–9.5 GB at the lowest
  in a test leg.
- **Baseline.** `base-1` 42.1 ms (23.8 FPS, view B), second leg 42.1 ms
  (23.7); view A `base-2` 38.2 ms (26.2 FPS), `r-base-1..3` 37.4–38.5 ms.
  Copies ~12 ms, copying back ~20 ms, 290–320 draws dropped.
- **Builder on** (`VICTORY_BVH_BUILDER=1`, `bvh-on`, `harbor-x-play-17`):
  9.1–9.6 FPS, 50–85 ms on the GPU, copying back 58–74 ms, 1 draw dropped;
  room lit, floor reflects, the aquarium looks right (player). Two programs
  carry the cost: `CS_RaytraceVBuffer_*` (`6d7fe7745ab9c6d8`) and
  `CS_ScreenProbes_TraceRays_*` (`f22a76fcb967ee59`). Every access of theirs
  goes through `memory.rs`'s page directory.
- **Upstream survey** (26 mirrors refreshed; patches under
  `backups/upstream-survey-20261127/`, 12 more forks in `extra.git` there).
  Taken into the tree, each with a switch:
  fxceless #1212 `d03859a2` (a fixed commit's rounded address: the watch is
  told from the rounded start to the caller's end); triippywhale `70656d56`
  (remap and page-write queues 2^18; `harbor-x-play-16` had dropped 163,840
  ranges); after DoubleGhosty/cool_fox `8c23f185`
  (`VICTORY_HEAP_RECHECK_FOREVER`: a slot stays on the 400-frame look) and
  `c3410461` (a partly mapped texture that read zeros is looked at again
  after 2 frames); fxceless #1246 `faf709b5` (`VICTORY_FAIR_DUE`,
  `VICTORY_RELOOK_MS=20000`); gleaf `12ce337b` (aliased view copied again);
  klbf `e0a91ea2` (`VICTORY_SKIP_WC_BACK`; it skipped nothing in these runs);
  avatarmanz `631400f`/`3e04ca0`/`15c3d0e`
  (`VICTORY_PORT_CONSTANTS_ORDER`). The last costs 53.1 ms against 37.2 ms
  (`fix1-a`, `off-order`; copying back 32.6 against 19.4 ms): the launcher
  sets it to 0. With it off the tree runs as before (39.2 against 38.2 ms,
  `off-all`, `base-2`). **None of them changed the hair or the phone.**
- **Tried and worse:** `VICTORY_BVH_EMPTY=1 VICTORY_LEGACY_FTZ=1
  VICTORY_PORT_CBV=1` on `seed-opt-cbv` with `CS_RayMiss:1:104` zeroed
  (`emptymiss`: washed-out white, red flare on the phone);
  `VICTORY_TEXTURE_GUARD=0` (`texguard0`: untextured head, 19.6 FPS).
  `VICTORY_CULL_NONE=1` (new diagnostic, after meeyao `3e42a23`): hair in
  0 of 3, winding is not it here. `VICTORY_TEXTURE_NO_WAIT=0`: hair in 3 of
  4 at no measurable cost (39.0 ms), not told from chance.
- **Hair.** The room captures of 2026-11-26 (+13 s) show hair in every one
  of ~95 runs up to 23:54. From the binary of 23:36 on it is there in some
  runs and not in others, usually from the start, once lost between +40 and
  +52 s (`fix1-a`); three states: long hair and beard; a cropped head
  (strands gone); a flat untextured head. Counts at +40/+52 s, same binary
  and switches: 1 of 4, 3 of 4, 3 of 4, 0 of 4, 7 of 8. Runs are streaky,
  so four legs tell nothing. The binary of 21:19 (`victory.release.exe.keep`,
  before kroeeee `f5c30ef` was picked): 4 of 4 with hair at 17.6–18.7 FPS.
  Hair is not cards: `CS_InterpolateStrands` (80 variants),
  `VS_HairStrandGeometry`, `PS_HairStrandVBuffer`/`GBuffer`, `CS_VisHair`,
  `CS_HairTile*`.
- **The title's own `VirtualAlloc`** (imported beside `XMemVirtualAlloc`) is
  now intercepted and told to the watch (`VICTORY_TITLE_VIRTUAL_ALLOC`); it
  is not called once on the way into the room.
- **The phone** is drawn right underneath: clock, a reminder and icons show
  in some captures (`off-order` f3, `r-fix3-4` f3), then the blue-white noise
  covers them again. The same in the 21:19 binary.
- XCL (GPL-2.0-only: ideas only) is behind on all of this: Windows only,
  4–5 FPS in the world on an RTX 5090, ray tracing gated off or forced to
  miss. Its design for title memory in shaders (fixed 4 GiB windows, an
  arithmetic window index instead of a directory look) is the lead for
  making the traces affordable; no measurement of theirs shows it.

## 167. The up-copy check fires in every binary; `play.sh`

`VICTORY_UPCOPY_CHECK=1` (0.3–0.7 FPS while it is on; counts, not times),
one leg each, `upmetric.py` over the log:

| Leg | Older values put over card-written words (median per sample, most) | Words at rest not the CPU side's |
| --- | ---: | ---: |
| `upcheck-1` (fix build, defaults) | 3,099 / 107,737 | 780,459 |
| `up-old` (binary of 21:19) | 1,286 / 48,522 | 845,169 |
| `up-prev` (binary of 22:18) | 1,106 / 80,248 | 872,758 |
| `up-nop1` (`VICTORY_PORT_CONSTANTS=0`) | 0 / 58,206 | 535,561 |
| `up-nop67` (`VICTORY_PORT_GUARD=0 VICTORY_PORT_JOURNAL=0`) | 0 / 76,669 | 848,788 |
| `up-noports` (all three off) | 0 / 195,250 | 510,762 |
| `up-noskip` (`VICTORY_WRITE_SKIP=0`) | 2,464 / 116,686 | 806,386 |

- Sends of older values over what the card wrote ("declared; last fetched
  0–1 frames ago") hit a handful of buffers every frame:
  `0x100a330000+0xa8c000`, `0x1013690000+0xa8c000`, `0x100eb20000+0xa8c000`,
  `0x100c840000+0x3c0000`, `0x1019760000+0x708000`. In every binary, with
  every port setting: old, not last night's regression.
- One buffer, `0x1028b20000+0x1800000` (24 MB), holds 0.5–0.9 M words on the
  card that are zero in title memory and on the CPU's side, "no shader was
  declared to write the buffer"; the values change between looks.
  `VICTORY_WRITE_SKIP=0` does not change it, and host copies are not made
  `CREATE_NOT_ZEROED`. Who writes it is open: a diagnostics build's
  `wide_write` names the program. A later send would put zeros over it.
- Whether either is what takes the hair is not shown.
- `play.sh fast|quality|steady [minutes]` beside the launcher: the fix build
  with the builder off (22–26 FPS), with it on (~9 FPS, right light), and
  the 21:19 binary (~18 FPS, hair in 4 of 4 starts). Test legs pin their
  binary (`SHOT_EXE`) since a second agent builds in the same tree.

## 168. Wave sizes: compute is pinned, graphics is not; `pswave32` shows nothing

Two read-only audits, in `backups/wave64-audit-20261127/` (`code-audit.md`,
`summary.md`, `wave64-census.tsv`).

- 16,638 of the title's 19,139 programs are wave64. Every compute program
  with cross-lane code is emitted with `WaveSize` equal to its console wave
  (276 at 64, 892 at 32); the wave64-to-32 forms (`own_bits`, the retry
  ladder, `VICTORY_WAVE_LOCAL`, …) only run on a host whose largest wave is
  32 and never here (`host wave lanes min=32 max=64`).
- Graphics stages declare no wave size: 4,252 wave64 pixel, 6,685 wave64
  mesh and 854 wave32 pixel programs with cross-lane code run at the
  driver's lane count. In a wave32 pixel program a lane mask read as a
  number uses the low half of the host's ballot while `v_mbcnt` and whole-
  wave reductions span 64 lanes (`lower.rs` 3604–3627, 10355): wrong in
  lanes 32–63 if the driver runs pixel programs in waves of 64. Fixed in
  fxceless #318 `507c307a`, klbf `8b0bed54`/`b01bce99`, Belobug `ecc4eb66`;
  not in this tree.
- The strand programs are not the case: `CS_InterpolateStrands` is wave32
  with no cross-lane code; only `CS_HairTileClassify` has any.
- `v_readfirstlane` alone does not make a compute program declare a wave
  size (about 114 programs); VIctory `200e3bde`/`73d2c90d` fix it.
- `VICTORY_HOST_WAVE` is described by the CLI's help but its code was
  removed in `dc6a8e1`; `apps/victory/tests/translate_host_wave.rs` still
  includes the deleted file.
- `RADV_PERFTEST=pswave32` on `seed-precompiled-ps32` (88,620 pipelines made
  again under it in 288 s): `ps32-1..3` 39.2, 38.7, 42.6 ms; hair in 2 of 3;
  the phone as before. Which lane count RADV gives a pixel program was not
  measured, so the defect is neither shown nor ruled out.

## 169. Mesa's cache limit raised; the state the archive holds

- A community guide for the kroeeee profile says the title fills Mesa's
  1 GB shader cache and pipelines then evict each other. Here
  `~/.cache/mesa_shader_cache` stood at 1.1 GB. The launcher and
  `precompile.sh` now export `MESA_SHADER_CACHE_MAX_SIZE=10G`; all 88,620
  pipelines made again (`warm --again`, 160 s) left it at 1.1 GB, so it was
  not being evicted here: head-room, not a fix.
- `victory-proton` (datingesp-hash; a wrapper that installs forks under
  GE-Proton with the patched wineserver and kroeeee's huge-page helper) read:
  its default profile sets what this launcher sets plus
  `VICTORY_COPY_COMPARE=1`, ports 2 and 5. Nothing to take.
- Final legs, fix build (`victory.fix4.keep`), final launcher, view A:
  `final-1` 40.7 ms (24.6 FPS), `final-2` 37.2 ms (26.9 FPS); copies 11.8–12.7
  ms, copying back 19.4–21.9 ms, 313–316 draws dropped, no device loss. Hair
  in one of the two; the phone as before.

## 170. Ray tracing through 2 GiB memory windows: builder on, 8.8 -> 19-20 FPS

Tree `fps20-ports/source` (uncommitted, on `4f6f9d3`; diff in
`backups/fps20-ports-20261127/rt-windows-rt6-on-4f6f9d3.diff`), binary
`bin/victory.rt6.keep`, seed `seed-rt-windows`, `play.sh rt`. All legs:
`VICTORY_BVH_BUILDER=1`, Series X profile, 80 s in the world. The first
legs were made by a second agent; `rt-win-5` on are ours.

- What changed: title memory by address is bound as 64 reserved 2 GiB
  windows (`CreateReservedResource` + `UpdateTileMappings`, `memory.rs`)
  and the trace and builder programs compute the window from the address
  instead of looking every access up in the page directory
  (`emit/intersect.rs`, `emit/lower.rs`). Read-only programs read the
  windows as `ByteAddressBuffer`; when every lane of a wave is in one window
  the loads share one uniform descriptor.
- `rt-win-1` hung at the first world frame: `Memory::another()` returned
  true without making a heap while `0 < free < 1024`; fixed.
- Legs, view A (~6.5k draws): `bvh-on` (before) 113.6 ms, 8.8 FPS;
  `rt-win-3` 56.0-57.3 ms (17.5-17.9 FPS); `rt-win-4` 48.7-52.8 ms
  (18.9-20.5 FPS; phone up 41.0 ms). Between 3 and 4: fills read through
  placed copies of the same list (no `BEHIND_PLACED` wait, 17 ms), 512 lists
  in the air (8 ms), eight landing threads with `VICTORY_LAND_SKIP`,
  dirty-range sync of the windows once a submission, write-combined memory
  left on the card in the page-mask branches.
- `rt-win-4` had the phone's bezel patchy and the wall dark: the
  write-combined skip left shader writes on the card but whole pages still
  went up from the CPU's side over them. Taking the skip out again
  (`rt-win-5`) costs it all back (60-77 MB fetched a frame, 87 ms on a loaded
  machine). Kept instead with `Shadow::leave_on_card()`: such a copy sends
  only the dwords the title changes, also of pages never taken (`rt-win-6`:
  bezel clean, 2 MB fetched a frame).
- From about 10:06 a remote-desktop process was using the card: the control
  leg `rt-win-4b` (the `rt-win-4` binary) made 84.4 ms where it had made
  52.8, and `rt-win-6` 70.2 ms, both in view B (~9.0k draws). Absolute
  numbers of `rt-win-5`, `-6`, `-4b` do not compare with the earlier legs;
  `rt-win-6` against `rt-win-4b` does (same view, same load): 14.2 against
  11.8 FPS.
- GPU time by command (`rt-gpu-prof`, diagnostics build,
  `VICTORY_GPU_TIMING=1 VICTORY_D3D12_DRED=1`): about 30 ms a frame of noted
  title commands in ~1390 kinds, none over 2.4 ms in a steady report. The
  two trace programs `6d7fe774` and `f22a76fc` are 0.4-1.7 and 0.2-1.2 ms;
  the builder's `caa78b8c` and `28918b71` about 1 ms each at 2-3 dispatches
  a frame (one report of a burst: 239 and 113 dispatches, 5.2 and 5.1 ms);
  `CS_GenerateShadowMasks` 1.6-2.0 ms; our alias copies 2.4 ms in 86.
  By list: preludes (refreshes of copies) 18 ms in 71 lists, compares 4 ms
  in 27, each list on its own at least 94 us. So the trace is no longer the
  cost; what is left over the builder-off frame is spread thin.
- Not done: a clean final leg (the machine must be quiet), `rt-win-6` in
  view A, and the 30 FPS the second agent's goal names.
- Later legs on a quiet machine, view A: `rt-win-4c` (the `rt-win-4`
  binary again) 52.5 ms, 19.0 FPS, so that leg repeats. `rt-win-7`
  (`leave_on_card`, every changed run on its own: 2.8 MB a frame in ~9,600
  ranges against ~450) 64.4 ms, 15.5 FPS; no yellow patches, the bezel not
  as clean as in `rt-win-3`. `rt-win-9` (`VICTORY_WC_JOIN_BYTES=256`: ~600
  ranges) 80.5 ms and the patches and the dark wall back: shader-written
  words lie within 256 bytes of the title's, so joining is not safe; the
  switch stays, default 0. `rt-win-8` (same binary) lost the device while
  the world loaded, once. `rt-win-10` (`VICTORY_SKIP_WC_BACK=0`, all fetched
  back) 95.3 ms with 70 MB fetched a frame: this build no longer makes
  `rt-win-3`'s 56 ms that way. Single legs differ by tens of ms here; none
  of these is a settled number.

## 171. The floor's glaze and the phone's noise are one fault: reflections held after CS_ResolveScreenSpace

Labelled legs from here on: `fps20-ports/leg.sh <tag> <label> [VAR=value]`
writes `captures-fix/<tag>/` (label, four captures, twelve quick captures and
`flicker.py`'s measure of how much the picture changes while nothing moves,
a summary) and `captures-fix/index.html`. All legs Series X, 3840x2160,
ray tracing off unless said.

- `VICTORY_REFLECT_SCREEN=1` (new; default on in the launcher): the
  reflection hold the stand-in already ran after `CS_UpdateHistory` also runs
  right after `CS_ResolveScreenSpace`, on its R11G11B10 image at table
  (1, 128), at `VICTORY_AMBIENT_REFLECT_SCREEN` (0.5) times the light around.
  After Trexies' Whiplash patch for cool_fox f4b48ad; here it is one more
  role (`Role::ReflectedScreen`) on the existing `hold_f` pass.
- `n1-newbuild-ref` (today's tree, without): 34.0 ms, floor glazed white,
  phone screen noise, room black around the phone. `f1-floor` 34.5 ms and
  `f2-floor-again` 33.0 ms (30.3 FPS), both view A: the floor reads as wood,
  **the phone's screen shows its lock screen and icons cleanly**, the room
  behind it is dim instead of black. The phone's noise (§164, §166) was the
  open sky's reflection on the screen's glass.
- Today's tree with ray tracing off is faster than last night's build:
  `a1-ours-fast` (`victory.fix4.keep`) 37.4 ms, `n1` 34.0 ms.
- `b1-coolfox` (cool_fox 30e54b4 built from source, own seed, 83,523
  pipelines made ahead): black picture, 3.7 FPS, 956 pipelines never made,
  2,232 indirect records refused a frame. Not usable on this stack.
- `a3-air` (`VICTORY_AMBIENT_AIR=1.5`): no change. `a2-rest0`
  (`VICTORY_INDIRECT_REST=0`): view B, not comparable; no effect shown.
- `o1-1440p` (`VICTORY_OUTPUT=2560x1440`): view B 37.4 ms against 42.9 ms
  at 4K in that view (`a2-rest0`, last night's binary); the HUD is larger.
- The light flickers by the frame in some starts (blink median 1.3 in calm
  legs, 13 in `a2-rest0`, 32 in `n1`): a white or blue veil one frame, dark
  the next; in `n1` one capture has the top-left quarter of the screen
  covered by a blocky white picture at the wrong size, and the hair gone in
  the next. Target dumps (`d1-targets`, `d3-writers`, diagnostics build with
  `VICTORY_DUMP_TARGETS`): the 1280x720 R32_UINT image the opaque rays are
  kept in holds a solid 80x45 block of the largest value in its top-left
  corner in two frames of three; its writers are `CS_Tiling_Generate_Opaque`,
  `CS_RayPrePass_Opaque_SSR`, `CS_RayBudgeting_Cull_RayBinning` and
  `CS_RaytraceVBuffer`. Zeroing that corner after each (`n2-corner-clear`,
  `VICTORY_CLEAR_AFTER` now takes `:<w>x<h>`) shows it as a white rectangle
  with the phone up: that corner reaches the screen, zero is not its right
  value, the cause is open.
- Upstream read today: Belobug PRs #206 (BVH hits from unmapped memory
  rejected), #541 (`VICTORY_AMBIENT_RT`, faces), #139; fxceless `9a9091d1`
  (instance nodes all ones), `3221ac42` (masked patch); zocram4cc
  `perf/merge-round-4-fixes` (translator version 5). None taken yet.

## 172. Renderer port 1 is what breaks the picture indoors; off by default

All legs: today's tree (`bin/victory.new4.keep`), seed `seed-rt-windows`, ray
tracing off, Series X 3840x2160, four captures at 20, 30, 40 and 50 s in the
room, no phone. "Corner" is the mean grey of the capture's top-left 44x24
pixels against the same patch beside it.

| Legs | Setting | ms (FPS) | Corner against beside | Blink median | Hair |
|---|---|---|---|---|---|
| `h1`-`h4` | port 1 on (the default until now) | 34.3-37.8 | up to 251 against 125 in 2 of 4 legs | 1 to 13 | 2 of 4 full, 1 cropped, 1 blank head |
| `p1`-`p4` | `VICTORY_PORT_CONSTANTS=0` | 40.9-42.0 (24) | equal in 16 of 16 captures | 1.0-1.6 | 3 of 4 full, 1 cropped |
| `q1`-`q3` | on + `VICTORY_PORT_CONSTANTS_ORDER=1` | 44.6-47.8 (21-22) | white square in 6 of 12 | 1.7-1.9 | 1 blank head, 1 cropped, 1 with square skin patches |
| `v1`-`v3` | on + `VICTORY_LATE_WAIT=0` | 85.6-92.2 (11) | white in 3 of 12 | 1.5-3.1 | 1 cropped; veil, skin patches, view B flicker |
| `t1`-`t3` | on + `USERPTR_THP=0` | 47.1-49.0 (20-21) | white square in 2 of 12 | 1.3-1.8 | changes between captures, veil in 2 legs |

- With port 1 off the white square in the corner (§171) is gone in every
  capture and the light is steady in all four starts; the start also landed
  in view A four times of four. The port costs it back: 5 FPS.
- Neither the order of the in-place indirect arguments, nor the delay
  between the title's write and the card's read (every list waited for), nor
  the huge-page helper (§165) is the cause: the faults stay in all three.
  Without the helper the port is slower than no port at all.
- So it is the sharing of that heap between the PC and the card itself
  (here: a userptr import of a file-backed mapping through the patched
  libdrm) that does not hold. avatarmanz PR #440 reaches the same port
  under Wine another way (`VICTORY_WINE_MAP`, `VICTORY_PORT_HOME`: the heap
  as one anonymous mapping of the process's own, "amdgpu takes memory by
  address only as one anonymous mapping"); not in this tree, the next thing
  to try for the port's 5 FPS.
- The launcher now has `VICTORY_PORT_CONSTANTS` 0 by default; `play.sh
  stable` is that mode. Hair is a fault of its own: cropped in one start of
  four with the port off.
- The title's render scale (Ghidra: the upscaler's size method at +0x7e9440
  clamps `+0x90` of the object at +0x7b99040 to 0.2..1.0; the method before
  it, +0x28f1e70, sets it to 2/3) read every second in `s1-scale-watch`
  (`VICTORY_IMAGE_WATCH=scale=7b990d0:4`): 2/3 for the whole leg. Not the
  flicker's cause.
- `VICTORY_COMPUTE_WAIT_MS` (avatarmanz d9ee5f2 + 1bf01b5, merged into
  `pipeline/make.rs`) never acts here: with every pipeline made ahead no
  compute pass waits. `VICTORY_KEYS_RECORD=<file>` records a player's keys
  in `VICTORY_KEYS` form; `route-merge.py` joins the key repeat's taps.
