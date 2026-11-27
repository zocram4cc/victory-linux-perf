> **victory-royale (private), branch `k7900`:** this tree plus our pieces. The plan, what carries over and in what order: [docs/royale/PLAN.md](docs/royale/PLAN.md); where things stand and how every piece works: [docs/royale/STATUS.md](docs/royale/STATUS.md); next steps: [docs/royale/NEXT.md](docs/royale/NEXT.md); checkpoints to play and compare: [docs/royale/CHECKPOINTS.md](docs/royale/CHECKPOINTS.md).

## victory-royale: what the tests of 26 Nov 2026 found

Measured on an RX 7900 XT 20 GB, Ryzen 9 9900X (two CCDs), 32 GB, CachyOS (kernel 7.3), Mesa/RADV 26.3.1, Jason's house, release builds, unattended runs (`scripts/ktrials.sh`). Day log: [docs/royale/k-journal.md](docs/royale/k-journal.md); setup and tools: [docs/royale/STATUS.md](docs/royale/STATUS.md).

### 1. The wineserver was what held this machine at half the author's frame rate

| | fps (median per run) | ms a frame | write watch a frame | wineserver's share of CPU samples |
|---|---|---|---|---|
| this tree, GE-Proton11-14, stock wineserver | 8.2-8.4 | 119-122 | 17-26 ms | 10.6-16.4 % |
| this tree, GE-Proton11-14, **view-tree wineserver** | **17.0-19.8** | **51-59** | **3.8-4.7 ms** | ~1 % |

- **The cause:** the title maps GPU memory 64 KiB at a time and has tens of thousands of views. Every `map_view` / `unmap_view` made the stock server walk them all (`is_valid_view_addr`, `req_unmap_view` in the profile). Every thread that maps memory or asks the write watch waits on the server.
- **The fix:** keep the views in a red-black tree by (process, base), in [linux/wineserver/](linux/wineserver/). It went upstream as kroeeee's PR #1; the author already ran a similar patch on Proton Experimental, which is why he saw 17-19 fps and we did not.
- **Ruled out first, with the author:** zram vs disk swap (21.5 vs 22.9 ms write watch), one CCD only (20.5), THP for shmem (already `advise`), ntsync off (worse: 31.4). A stale build of our own old view-tree patch (24 Nov) still walked the list. Rebuilt from source, it fell back to the list in no run (the patch now prints a line if it ever does).
- **World entry** dropped from 112-120 s to 43-61 s with it.

### 2. What else was measured

- **Pipeline library (K.1):** vkd3d-proton refuses `CreatePipelineLibrary` with a non-null pointer of length 0, and an empty Rust slice is non-null, so this tree ran with no library ("pipelines are not kept"). A null blob fixes it (~2,250 pipelines loaded a run), but **no measurable fps gain in this tree**: it makes pipelines off the title's threads and Mesa's cache holds the compiled code.
- **Noise:** the same build gave 6.2 and 9.0 fps in two runs. **Decide on 3+ runs.**
- **Moving into new areas:** the frame grows to 200-660 ms for a few seconds while the layer's own parts shrink; the profile has `title-main` at 33 %. That is the title's own streaming, outside the layer. What looks broken meanwhile (objects appearing and vanishing, black patches, Jason black and white, blur) matches the work dropped for want of a pipeline, which rises from ~200-350 to 600-1,058 a frame there: section 3.
- **The author's tree on Proton Experimental:** a black picture once at start (fine on relaunch), the intro flickers (known), the start screen waits for A.

### 3. P.4: the pipelines no fork makes (device methods 0x1c8 and 0x218)

The title asks for ~12,900 pipelines through two console device methods the layer does not implement; the title then goes without them. One dump (`VICTORY_NATIVE_PSO_DUMP`, ported from fxceless): **10,206 distinct 0x1c8 descriptions (graphics) and 2,680 of 0x218 (compute)**.

- **0x218 is an `FSPX` program taken apart**, the format this tree already reads for `ACCC` parts (`victory_shader::native::fspx_program`):

  | description words | FSPX piece |
  |---|---|
  | (1, 2), 0x1c bytes | kind 0, pipeline registers: thread-group size at +0x0c, RSRC1 / RSRC2 at +0x14 / +0x18, flags (bit 4 wave32) at +0x12 |
  | (3, 4), 0x14 bytes | kind 1, pipeline metadata (a hash) |
  | (5, 6), in title GPU memory | kind 6, the GFX10 code |
  | (7, 8) | kind 7, program metadata: `0x403`, size − 0x14, code dwords; 0x41 resource, 0x42 view, 0x43 hash records |

  Put back together as an FSPX blob inside a one-part container, it goes through the normal compute path (`device::native_compute`, `VICTORY_NATIVE_COMPUTE=1`, off by default).
- **Root constants:** 1,469 programs declare a constant buffer as root constants (root type 0); `fspx_program` refused them. Fixed: as many user SGPRs as the record's descriptor dwords.
- **Offline: 542 of 2,680 translate** (`crates/victory-shader/examples/p4_fspx.rs` over the dump, natively on Linux, in seconds). The rest: 1,366 "an address with two computed terms", 226 "an address not from a root parameter", 166 "an image store of some channels", then small groups (`s_pack_*_b32_b16`, `s_bfm_b64`, unknown structured-buffer stride, the thread-group info SGPR, lane masks read as data). 1,529 programs are wave32, 1,151 wave64; the translator declares `WaveSize` where a program depends on the lane layout.
- **In game, all of them made: the scene goes black or white** when the world starts (~4,000-6,000 draws a frame instead of ~9,000-11,000). A run with ~43 of them made looked "very sharp and correct" once loaded (the user).
- **The cause, found by the hunt:** a program that does not translate still got a pipeline object. Its dispatches were then dropped, so **the title believed the pass ran and used what it never wrote**. Without a pipeline (E_NOTIMPL) the title goes its own way round, as before. The pass that blew the picture to white (`0f0039f000`, 16×16 groups, ~25 SRVs) does not translate at all. **Fix: a 0x218 program gets a pipeline only if it translates** (checked when the title asks for it). Needed but not enough: with the 542 that translate made, the scene is still broken (~6,000 draws a frame; the user: all grey but the HUD). Then growing a set of working programs (`scripts/p4-grow.sh`) kept none: **~30 programs break the scene even made alone**, while they do run. So the fault is systematic (most likely how FSPX records are mapped onto the translator's bindings for the title's own programs), not a few bad programs. Checked by hand on the smallest one (`0f00220000`, a per-layer copy/downsample): its metadata matches what its code reads exactly, and lost UAV writes are not the difference either. Next: trace one native pipeline's dispatch in game (root signature and root values the list holds, user data the shader gets); the suspect is a console path for binding these pipelines' resources that the layer does not carry over. `VICTORY_NATIVE_COMPUTE` stays off.
- **Correction and conclusion (27 Nov, 02:15):** draws a frame were a wrong test (the camera cuts indoors by itself); judged by screenshots, **one program (`0f01a73b00`) turns the picture grey and the other 508 that translate run at ~20 fps with a picture**, but the world-entry view is wrong with them every time (the house's interior not drawn, far islands missing). The title's compute programs work in chains, and **only 2 of the 30 families (by root signature) translate whole**: a consumer made without its producer reads nothing. With none made the title's fallback looks right. **So P.4 needs translator coverage of whole families first** (offline, `p4_fspx`: "an address with two computed terms", 1,366 programs, is the first target); a family goes to the game only when it translates whole.
- **0x1c8 (graphics)**: word 0 a root signature, (1, 2) a 0x1d8 header, (3, 4) 0x50, then (code, metadata) pairs per stage at words 5-8, 9-12, 13-16. Not started.

### 4. How to test this kind of thing (lessons)

- **Judge a broken picture by draws a frame, not fps.** Broken runs ran at 29-42 fps (nothing drawn), good ones at ~17. `scripts/p4-bisect.sh` first went wrong on fps.
- **The order of pipeline calls differs between runs** (they are made on several threads): bisect by call number gave contradictory answers (168 good in one run, 87 broken in another). **Code addresses are stable:** `VICTORY_NATIVE_COMPUTE_ONLY=<file>` and `scripts/p4-hunt.sh` halve the sorted list of addresses instead.
- **fps while streaming says little:** one run at 2.3 fps was still loading the area.
- Seven programs the hunt marked bad (`bench/p4-bad.txt`); six of them translate and need a second look now that untranslatable ones no longer get pipelines.
- Unattended runs under GE-Proton: the game window's class is `victory.exe` (`steam_proton` under Proton Experimental); close the game by pid (`pkill -x victory.exe` misses it under Wine).

### 5. Other trees read

- **kroeeee** (this tree's upstream): see section 1; the author's next fixes (known_holes under Wine, PORT_DESCRIPTORS with port 1, COPY_COMPARE on BAR memory) to merge when published.
- **Belobug/VIctory-Belobug-AMD**: the same base, plus texture uploads in the submission prelude "to fix frame-order & flickering" (a candidate for the flicker while streaming), a guard for AMD's compiler crashing (a list of pipelines left out), FSR 4.1.2. No 0x1c8/0x218.
- **fxceless**: `VICTORY_NATIVE_PSO_DUMP` (dump only).
- **victory-scarlett**: `SHOW_AFTER` (a slot shows a new texture only after its upload ran) in place of a wait for running submissions.

### 6. P.4 translator work, in detail (27 Nov 2026)

**Goal.** Make device method 0x218's compute programs (2,680 in one dump) translate, family by family, so that whole chains of the title's GPU work can be switched on (`VICTORY_NATIVE_COMPUTE=1`). A family made in part breaks the picture (section 3), so coverage has to reach whole families before anything is turned on by default.

**How it is measured (no game needed).**

```sh
# every dumped 0x218 program rebuilt as FSPX and translated, errors grouped; writes the list of those that translate
cargo run --release -p victory-shader --example p4_fspx -- ~/Games/victory-data/pso-dump --ok bench/p4-translate-ok.txt
# the unit tests, after every change
cargo test --release -p victory-shader
# what blocks the next step (printed per attempt, grouped with sort | uniq -c)
VICTORY_WHY_DESCRIPTOR=1 cargo run --release -p victory-shader --example p4_fspx -- ~/Games/victory-data/pso-dump 2>&1 | grep WHY-DESCRIPTOR
VICTORY_WHY_ADDRESS=1    cargo run --release -p victory-shader --example p4_fspx -- ~/Games/victory-data/pso-dump 2>&1 | grep WHY-ADDRESS
```

The dump comes from one game run with `VICTORY_NATIVE_PSO_DUMP=Z:\…\pso-dump` (598 MB; `218_<code address>/` directories with the description's pieces). A whole run of the harness takes well under a minute on Linux.

**Method.** Census the most frequent blocker → read real programs that hit it (`llvm-mc --disassemble -arch=amdgcn -mcpu=gfx1030` on `w5_*.bin`, the metadata records in `w7_*.bin`) → implement exactly that shape, refusing variations → tests green → harness count up → commit. A program translates only when every blocker on its path is gone, so the count rises slowly while blockers move.

**Steps done** (all in `crates/victory-shader`, each its own commit on `k7900`):

| step | what the console code does | what the translator does now | where | effect |
|---|---|---|---|---|
| 0 | declares a constant buffer as root constants (root type 0) | as many user SGPRs as the record's descriptor dwords | `native::fspx_program` | 1,469 programs were refused as malformed before |
| 1 | adds an offset register to an address that already had a computed term | `resource::Address::dynamic2`, a second computed term, added where the address is used (global loads, built buffer offsets); single-term interpretations skip it; scalar loads from code tables refuse it | `resource.rs`, `emit/lower.rs` | the 1,366 programs blocked there pass it |
| 2 | turns a loaded structured-buffer descriptor into a raw view: `[d0, d1 & 0xc000ffff, d2 * n, (d3 & m) \| 0x30000000]` | that is the loaded descriptor; accesses without an index are raw already | `resource::raw_view_of_loaded` | 664 of 979 pass "a descriptor changed after loading" |
| 3 | swaps registers with three `s_xor_b32` | XOR chains that cancel are the word they come back to | `resource::xor_cancelled` | "changed after loading" 315 → 114 |
| 4 | fetches descriptors from the heap by index: `s_add_u32 addr, s[n-2], readfirstlane(index << 5)` (a waterfall over bindless descriptors); the heap bases are in the last two user SGPRs and the metadata does not list them | `ResourceHeap` at user SGPR n-2 and `SamplerHeap` at n-1 where nothing listed covers them (the host binds heap bases as 0) | `native::fspx_program` | "a descriptor outside the heaps" (362) gone; 544 → 574 |
| 5 | writes an address's high half as `s_add_u32 hi, 0, src_scc` (the low add's carry) | accepted as the high half plus carry | `resource::plus_scc`, `high_half` | 574 → 587 |

**More steps (27 Nov, 03:00-04:15):**

| step | what the console code does | what the translator does now | where | effect |
|---|---|---|---|---|
| 6 | stores to an image only its first one, two or three channels (`dmask` 1, 3, 7) | the data in those channels, 0 in the rest (a format with that many channels has no others); other partial masks refused | `emit/lower.rs` image stores | 587 → 715 |
| 7 | packs 16-bit halves: `s_pack_ll/lh/hh_b32_b16` | implemented (SCC untouched) | `emit/lower.rs` scalar ALU | → 798 |
| 8 | `v_mad_u32_u16` / `v_mad_i32_i16`, 50 programs with **op_sel** (high halves) | implemented with op_sel; clamp refused | `emit/lower.rs` integer VALU | → 809 |
| 9 | FSPX buffer records carry the **element size** in the word after the register (a multiple of 4, or 1 for raw: true of all 44,749 buffer records in the dump); textures carry the view dimension there | used as the stride for buffers | `native::fspx_program` | → 925 |
| 10 | swaps 64-bit register pairs with `s_xor_b64` | XOR chains cancelled half by half | `resource::xor_operands` | "neither loaded nor built" 298 → 180 |
| 11 | parks **VCC** in a VGPR lane beside other SGPRs | VCC (codes 106/107) counts as a parked scalar | `ir::unspill` (`parked`) | → 928 |

**Where it stands: 928 of 2,680 translate** (542 before the steps); **6 families whole** (18 programs); the large ones at 362/525 and 235/330. 209 `victory-shader` tests pass. In game nothing changes by default.

**The real wall: scalars parked in VGPR lanes.** When the console compiler runs out of SGPRs it saves one in a lane of a spare VGPR (`v_writelane_b32 vN, sX, K`) and takes it back later (`v_readlane_b32 sX, vN, K`). `ir::unspill` turns such saves and restores into plain scalar moves so that later stages see through them, but only when a VGPR's uses meet its conditions (every use one of the two forms with a constant lane, or from the first save on within one block, a pair saved and restored together…). Where it does not apply, **the analysis loses where a value came from**: in `218_0000000f002d6300` the table pointer `{s4, 0}` is parked in `v142` lanes 28/29 (~300 parked saves and restores in that VGPR) and brought back as `s[84:85]` → `s_mov_b64 s[20:21], s[84:85]`; an image descriptor loaded through `s[20:21]` then has "an address not from a root parameter". This is behind most of the 683 programs with that error and likely the 116 descriptors read back with four `v_readlane_b32`. Finding which condition such VGPRs fail, and lifting it, should unblock them together.

**Step 12 (04:30): an off-by-one in `unspill`.** `parked` took SGPR codes `< 105`, but GFX10 has s0-**s105**: one `v_writelane_b32 vN, s105, K` made the whole VGPR an ordinary one, and nothing parked in it was seen through. Fixed (`<= 105`, plus VCC). With `VICTORY_WHY_UNSPILL=1` the harness prints, per VGPR that holds parked scalars, its first other use and whether it was unspilled. 928 → 931; "a descriptor neither loaded nor built" leaves the top of the list.

**What is behind the 723 "address not from a root parameter" after that:** in `218_0000000f002d6300` the image descriptor at 0x3640 is loaded through `s[20:21]`, which at that point is `s[84:85]` **loaded from a constant buffer** (`s_buffer_load_dwordx2 s[84:85], s[84:87], 0xc0`): the title keeps the address of a descriptor table in its constant data and the shader loads descriptors from there. The translator has no notion of a descriptor table reached through an address read from memory (the `MEMORY` fallback is for data loads, not descriptors). Next: decide how such an address maps to the host heap and add that source kind.

**Measured in game (27 Nov 04:45, `VICTORY_HEAP_TRACE=20`, `bench/k-…-heap-trace`):** the view heap is at console addresses **`0x0b3c0000`-`0x0d9c0000`** (1,245,184 descriptors of 32 bytes, 32-bit addresses), and **every table value the lists set lies in that range** (`0x0bddde00`, `0x0c7bf340`, …): tables are console addresses in the heap, which the layer turns into byte offsets by subtracting the heap's start. A table address the title keeps in constant data is then of the same kind. **Design:** where a descriptor is loaded through an address that came from memory, the translator computes `(address − view heap start) / 32` at run time and reads the host heap at that index; the layer passes the view heap's console start to the shader as one more user-data value (as it passes table offsets), since the translation is cached across runs and must not bake it in.

How a blocking site is found: put one program in a directory of its own, `K_FULL_ERRORS=1 cargo run --release -p victory-shader --example p4_fspx -- that-dir` (the error with its code offset), then `llvm-mc --disassemble -arch=amdgcn -mcpu=gfx1030 -show-encoding` over its `w5_*.bin` and sum the encoding lengths to reach that offset.

**Next blockers** (programs whose first remaining error it is):

| programs | blocker | what is known |
|---|---|---|
| 618 | an address not from a root parameter | addresses made by `s_or_b32`, `v_or_b32`, `v_cndmask_b32`, a pair of `s_lshl_b64`, `v_add_co_u32`, `s_bfm_b64` (`VICTORY_WHY_ADDRESS`) |
| 376 | an image store of some channels | stores with a partial `dmask` |
| 298 | a descriptor neither loaded nor built | to census |
| 114 | a descriptor changed after loading | other shapes (`VICTORY_WHY_DESCRIPTOR`) |
| 96 + 69 | `s_pack_hh_b32_b16`, `s_pack_ll_b32_b16` | scalar 16-bit packing, not implemented |
| 95 | a structured buffer of unknown stride | FSPX records carry no stride |
| 69 | a typed buffer store of some channels or at an offset | |
| 49 | `v_mad_u32_u16` | not implemented |

**Families** (programs sharing the root signature of the description's word 0): 30. Recompute which are whole with the `--ok` list against the dump (`bench/p4-full-families.txt` was made so). When one becomes whole, test it in game alone: `VICTORY_NATIVE_COMPUTE=1 VICTORY_NATIVE_COMPUTE_ONLY=Z:\…\family.txt`, judged by screenshot contrast, the world-entry window region (the house's interior must show; mean luminance ~55-60 in plain runs) and fps against plain runs, plus the user's eyes.

**Known bad in game:** `0f01a73b00` turns the picture grey even alone with the others; keep it out until understood.

---

> **Linux / Proton fork.** This is avatarmanz/VIctory-7900XTX with the changes it
> takes to run under Proton on AMD (RADV): ~16-17 fps in the open world on an
> RX 7900 XT / Ryzen 5800X3D / 32 GB. **Start with [LINUX.md](LINUX.md).**
> No game files are included.

# VIctory on an RX 7900 XTX

A fork of [VIctory](https://github.com/Marvisak/VIctory), a compatibility layer
that runs an Xbox Series title natively on Windows from files the user supplies.

## The goal

**A stable 30 fps or more without ray tracing, then as high a frame rate and as
low frame times as the PC allows**, on an AMD RX 7900 XTX first and on PCs with
32 GB (later 16 GB) of RAM.

The way there is **porting, not emulating**: patching the title's renderer so it
behaves the way a PC renderer does, rather than having the layer watch and copy
the console's shared memory every frame. Shaders are translated ahead of time
into a versioned store that is kept between builds. Ray tracing stays optional
and off by default. The picture comes after the frame rate: texture pop-in and
lighting that is too bright or too dark when walking indoors are next in line.

**This repository contains no game files, code or assets.** You need your own
copy of the game. It is not affiliated with or endorsed by the game's developer
or publisher.

## Where it stands (2026-11-26)

Tested on one machine only: Windows 11, Intel i9-12900K, 64 GB RAM, Radeon
RX 7900 XTX, ray tracing off.

| | |
|---|---|
| City street, Series S profile, renderer patches 1, 2, 5, 6 and 7 on | about 45 ms a frame (about 22 fps, 44.9 ms in run merge-5) |
| Same, patches 1, 2, 5 and 6 | 45.6 ms, p95 47 ms |
| Same, patches 1, 2 and 5 | 47.3 ms (about 21 fps), p95 49 ms |
| Series X profile, patches 1+2 | about 53 ms (about 19 fps) |
| Same scene before the patches | 57-75 ms (13-17 fps) |
| Shaders the title uses that translate | all of them (25,790 of 25,985; the rest are unused geometry shaders) |
| Memory | about 30 GB of private memory, 10.5 GB of VRAM: **64 GB of RAM recommended** for now |

It started at 280 ms a frame in the same scenes three days earlier. What sets
the frame now is the one thread that submits the title's work to the card
(about 40 ms); the card itself needs about 36 ms and now waits about 7 ms a frame with nothing queued.

### Renderer patches (off by default while they are tested)

| Switch | What it does |
|---|---|
| `VICTORY_PORT_CONSTANTS=1` | The title's 174 MB constants heap is read by the card where the title writes it: no uploads, no fills |
| `VICTORY_PORT_DESCRIPTORS=1` | The title's descriptor-table stores are patched (byte-checked) to report the slots they write, instead of a scan of ~600,000 slots a frame |
| `VICTORY_PORT_CBV=1` | Translated shaders read constant buffers in place as raw bytes (~22,000 fills a frame to none; -4.7 ms) |
| `VICTORY_PORT_GUARD=1` | The write guards' lock is shared by faults and refreshes, with a claim per 64 KiB block; the submission's planning runs beside it (title threads' waits 29 ms to ~2 ms a frame; -2 ms) |
| `VICTORY_PORT_JOURNAL=1` | The heaps' records are asked for at the title's submit, and the journal's notes are split into 64 parts with locks of their own (the submit thread's waits ~5 ms + 1 ms to ~0.3 ms; -1 to -3 ms) |

### Other switches

| Switch | What it does |
|---|---|
| `--console series-s` | The Series S profile: 1080p output, about 5 ms faster, a softer picture |
| `VICTORY_OUTPUT=1920x1080` | The title renders to that output size itself (Series X profile; about 16 ms less card time) |
| `VICTORY_FRAME_INTERVAL_MS=16.667` | Lets the title go above 30 fps (menus and videos reach 60; the game's clock stays right) |
| `VICTORY_AMBIENT_SETTLE` | How fast light settles after walking indoors (on by default, `250,3`; `0` turns it off) |
| `VICTORY_TEXTURE_STORE=1` | Converted textures kept between runs (about 8 ms a frame for now: off) |

What is known to be wrong:

- Trees, signs and far objects flicker in the distance in some places.
- Lighting is too bright or too dark after walking indoors, until the title has
  run for a while; rooms with little direct light are too dark without `-RayTracing`.
- Textures of what just came into view appear late while the title streams.
- The first run of a new build stutters while the driver makes pipelines
  (`victory warm` makes them ahead).
- Recording a frame in the world makes the title's own watchdog end it.

## How to run it

You need Windows 10/11 x64, a CPU with AVX2, a Direct3D 12 GPU with shader
model 6.6 and mesh shaders, Rust (the toolchain is pinned), the MSVC build
tools, and your own dump of the title.

```powershell
git clone https://github.com/avatarmanz/VIctory-7900XTX
cd VIctory-7900XTX
copy victory.example.toml victory.toml      # then set your paths, outside this folder
cargo build --locked --profile diag -p victory --features diagnostics
scripts\play.ps1 -Exe <path of your copy's executable>
```

`scripts\play.ps1 -Safe` leaves out the newest, faster ways; try it first when
something is wrong. `-RayTracing` turns the title's ray-traced indirect light on.

On a hybrid-graphics PC, the runtime and offline pipeline checks prefer a
high-performance adapter with Shader Model 6.6. The runtime logs its adapter
name, PCI vendor and device IDs, and dedicated VRAM. Set
`VICTORY_D3D12_ADAPTER` to a zero-based DXGI index or part of the adapter's
name to select it explicitly. With the launcher, pass
`-More @{VICTORY_D3D12_ADAPTER='NVIDIA'}`.

Before the first run, ahead of time (both can be stopped and go on later):

```powershell
target\diag\victory.exe shaders <the shader archives>   # every shader translated, minutes
target\diag\victory.exe warm --used                     # pipelines earlier runs drew with, for a new driver or build
```

Commands, debugging switches and upstream's own description:
[docs/README-fxceless.md](docs/README-fxceless.md). Every `VICTORY_...` switch is
described in a comment where it is read.

## What is different here

In short; the reasons and the measurements are in [docs/findings.md](docs/findings.md).

- **List calls are written down, not carried out, on the title's threads**
  (`VICTORY_THIN`): eight threads of ours translate them.
- **Submissions are prepared by a thread of ours and joined**
  (`VICTORY_ASYNC_SUBMIT`, `VICTORY_JOIN_PARTS`), with what can be spread over
  threads spread: refreshes, compares (AVX2), descriptor records, landings.
- **What shaders wrote comes back beside the write guards**, by the dwords that
  changed, on a thread of its own (`VICTORY_RELAY`, `VICTORY_LAND`).
- **Buffer fills that need bytes still on the card are done on the card**
  (`VICTORY_CARD_STEPS`) instead of waiting for it.
- **The title's writes are caught more cheaply**: one view opened at a fault,
  blocks that rest, places in the title's code that look before they write.
- **Textures go up on a copy queue of their own** and a slot shows one only after
  its upload ran, which also ended a device loss that came every 15 minutes or so.
- **Boats float**: textures that shaders write and the title's CPU reads go back
  to title memory (from fxceless and Geckomacabre, on by default here).
- **Frame record and replay** with per-pass times and shader variants compared
  turn by turn ([docs/replay.md](docs/replay.md)).

## Credits

This is a tree of other people's work with more on top. The history is kept, and
commits taken from elsewhere name the commit and its author.

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors: the loader, the runtime services, the Direct3D 12 back end and
  the shader recompiler.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**: the fork
  this one started from, and the people whose pull requests it carries.
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)**: ray tracing fixes,
  cube gradients, sparse textures.
- **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**:
  the ambient light idea, pipeline workers, and work by romainhedouin.
- **[FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY)**.
- **[myokex/victory-amd](https://github.com/myokex/victory-amd)**: the guard over
  every view of a page (with Spexivus, joroyt and tightfall), and Joshua Odom's
  reads into title memory.
- **Geckomacabre** and **cxy0072019-sketch**: the texture read-back.
- **[XCL](https://github.com/ivall/xcl)** (GPL-2.0-only) was read for how the same
  title behaves under another layer; no code of it is here.
- This fork's changes were made with [Claude Code](https://claude.com/claude-code).

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file that is not
covered is named in [docs/asset-policy.md](docs/asset-policy.md).
