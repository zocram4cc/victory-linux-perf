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

**Steps 13-16 (27 Nov, 05:00-16:00):** 13, immediate samplers built in code with `s_bfm_b64` of constants folded (`resource::bfm_folded`): 931 → 1,105, and the 723 "address not from a root parameter" were almost all these; 14, typed buffer stores of one to three channels (`buffer_store_format_x/xy/xyz`, the rest 0; at an offset refused): → 1,164; 15, the thread-group info SGPR (TG_SIZE: waves in the group in bits 0-5, the wave's index in bits 6-11, from the console's flat thread index and wave size): → **1,202**. Families: 9 whole (40 programs); 43/1023, 413/525, 282/330, 165/246. Not done on purpose: image stores of the alpha channel alone (`dmask 0x8`, 69 programs: would need a read-modify-write racing other threads). Three programs panic in the bitcode writer ("forward reference outside a phi"), guarded by `catch_unwind` in game. `MAX_JOINS` 12 tried and reverted (no gain, three times slower).

**Steps 16-17 (27 Nov afternoon):** 16, XOR swap chains followed 32 deep (8 was not enough: "a descriptor changed after loading" 147 → 26); 17, **descriptor tables addressed from constant data**, as designed: where a descriptor is loaded through an address read from memory (`MEMORY`), the translator takes `(address − heap start) / size`, the heap start a host word (`USER_VIEW_HEAP_START` / `USER_SAMPLER_HEAP_START`, `emit/shader.rs`) that the layer fills in `draw::host_word` with the bound heap's console address (`descriptor::Heap::start`). "A descriptor outside the heaps" (117) is gone; **1,202 → 1,248**. Families: 12 whole (77 programs); 43/1023, 413/525, 282/330, 165/246. In game this is only reached with `VICTORY_NATIVE_COMPUTE=1` (off). Tried and dropped: collapsing trivial phis in descriptor words (no change: the tracer's origins already skip them; the 317 "joined too often" are real multi-path joins in very large programs, e.g. `218_0000000f005db600`, ~18,000 instructions, a constant-buffer descriptor `s[80:83]` reaching a load through many paths: the sources multiply per join, so raising `MAX_JOINS` is not the fix; a de-duplicated join of sources is).

**Step 18 (27 Nov evening): step 17 undone in practice.** In game, the 12 whole families made half of Jason's house invisible (walls and deck floor; `bench/p4-12families-house-missing.png`). Halving the families (`scripts/p4-family-bisect.sh`) and running each suspect alone found family `0x33cb3490`: HiZ occlusion culling whose descriptor loads went through step 17's new path from a user SGPR that is really the view heap's base (s7), which our layout had wrongly given to the root constants (their record's `e[20]=4` is a descriptor size, not an SGPR count). The layer leaves it 0, so the loads read near address 0 and the culling dropped the house. Now a descriptor is taken through a memory address only when that address was loaded from memory (`resource::MEMORY_LOADED`); none of the 46 programs step 17 gained was, so **1,248 → 1,202**. Lesson: a new translator path that makes a family whole must be checked in game before that family is trusted; the bisect script and the per-family lists (`bench/p4-fam/`) make that a 15-minute job. Next: read the root constants' SGPR count and the heap's place from the root signature instead of guessing (n-1 against n-2). **The whole investigation, the code, the cause and what remains: [docs/royale/p4-house-bug.md](docs/royale/p4-house-bug.md).**

**Steps 19-23 (27 Nov evening): clearing the way through the descriptor tracer** (each moved programs to the next blocker; the count stays 1,228, none lost, but the blockers ahead are now few and large):
- 19, **the view heap's SGPR read from the code** (`native::view_heap_sgpr`: s[n-1] in 187 programs, s[n-2] in 59): 1,202 → 1,228.
- 20, **words joined in several blocks** traced from the block the others dominate: 146 → 0.
- 21, **descriptor tracing memoized** per (words, lane facts), joins with one source collapsed, and a work bound (3,000 steps) instead of the join depth: "joined too often" 433 → 0. Raising the depth alone to 32 ran out of memory and killed the user's terminal: never again without a work bound.
- 22, **table addresses through phi chains that bring one value** (`one_leaf`, through cancelled XOR swaps): "outside the heaps" 307 → 0.
- 23, **zero high halves through phis and XORs** (`is_zero`): not enough. The 459 programs left at "an address with an untracked high half" use 64-bit XOR swaps between misaligned register pairs across joins (the table's high dword traded with another address's low dword, `s_add_u32`). Next: resolve XOR swaps as permutations of registers through phis, not term by term. `VICTORY_WHY_HIGH=1` prints where the proof stops.
- 24, **XORs that cancel are copies in copy propagation** (`Tracer::xor_copy`, `xor_terms`):
  - `x ^ 0` is `x`, and `(p ^ q) ^ q` is `p`: the second and third steps of a register swap `a ^= b; b ^= a; a ^= b`;
  - so joins of swapped registers settle to one origin.

  Correct and kept, but not enough: "untracked high half" went 459 → 511 (programs moved up from other blockers; none lost, still 1,228).

  **What these programs really do** (`218_0000000f002d6300` at its line ~5460):

  ```
  v_readlane_b32 s84, v142, 13     ; SGPRs back from a spill slot: lanes of a VGPR
  v_readlane_b32 s85, v142, 12
  s_xor_b64 s[24:25], s[24:25], s[84:85]   ; swap s[24:25] <-> s[84:85]
  s_xor_b64 s[84:85], s[24:25], s[84:85]
  s_xor_b64 s[24:25], s[24:25], s[84:85]
  s_xor_b32 s92, s92, s24                  ; then swap s92 <-> s24
  s_xor_b32 s24, s92, s24
  s_xor_b32 s92, s92, s24
  ```

  The table address's high dword is one of those spilled values. The swaps now resolve; what does not is **the spill slot**: lane 12 of v142 is written (`v_writelane_b32`) on several paths, so the value read back is a join that `ir::unspill` / `parked` does not turn into a phi.

  **Next:** in `unspill`, when a lane's writes on all paths into the read write the same origin (or 0 for a table's high half), take that origin. Otherwise make the read a phi of the written values at the join, so that `one_leaf` / `is_zero` can see through it. `VICTORY_WHY_UNSPILL=1` exists for this.
- 25, **checked, and corrected**: `unspill` does succeed on the example (only v140, an ordinary VGPR, is not parked), so the spill lanes are already copies and phis. The zero proof stopped at two other things:
  1. **`s_add_u32 sH, 0, src_scc`**: the high half as 0 plus the low add's carry, which `high_half` already accepts (offsets below 4 GB). Now `is_zero` accepts it too (`plus_scc`). Correct, kept.
  2. **A lane mask in the same register on another path** (`218_0000000f002d6300` near 0x6580: `s_and_b32 s90, s90, exec_lo` … then the XOR swaps). The compiler reuses the register: on paths where the address is not used it holds a lane mask, and the join mixes that dead value in.

  **Next:** prove the high half only over the paths that reach the access, as `classify` does for descriptors (`known`, `lanes.reach`, `only_edge`). A phi edge no lane that uses the address comes by does not count. `VICTORY_WHY_HIGH=1` prints the offset where the proof stops.

- 26, **the high-half proof over the paths that matter** (`Tracer::edge_reaches`: a phi edge no lane that reaches the access can come by is ignored), and **XOR trees cancelled as multisets** (`Tracer::xor_leaves`: the leaves that appear an odd number of times are what the XOR is, so a swap partner cancels out even when it is a lane mask). "An address with an untracked high half" **511 → 0**.
- 27, **image stores of any channel mask** (the deferred item below, now done): the texel is read (typed UAV load), the masked channels replaced (the data registers hold them packed in order), and all four written. 553 of the 566 refused stores were alpha alone (`dmask 0x8`).

  The real program (`218_0000000f00431000`) takes one entry per thread of a list of texels (x, y packed in 16 bits each). Then either it stores RGBA with alpha 1, or it stores alpha alone as half 1.0 (`0x3c00`, `d16`) into the same texture, leaving RGB.

  *shortcut*: right where each texel has one writing thread per dispatch, as there. `VICTORY_PARTIAL_IMAGE_STORES=0` refuses them as before. **1,228 → 1,239**, none lost.
- 28, **dead descriptor dwords in joins are 0** (joins are per register; the compiler reuses a register whose descriptor dword is dead, here as a lane mask: `218_0000000f002d6300` at 0x14e50, `s_and_b32 s37, s37, exec_lo`), and **buffer dword 1 carries the stride** (`s_bfe_u32 x, d1, 0xe0010`). **1,239 → 1,304**, none lost.
- **Shortcuts that rest on assumptions are listed, with their switches and how to verify them in game, in [docs/royale/p4-shortcuts.md](docs/royale/p4-shortcuts.md).** The user rightly asked that the programs get correct data: a program that translates is not proven right.
- Next wall: "a descriptor's address or format read as data" (405 left). Census of where it is read (first 60 programs, by the instruction at the error): `s_and_b32 sX, sX, exec_lo` 27, `s_andn2_b32 exec_lo, sX, exec_lo` 5, `v_cmpx_ne_i32 -1, vX` 4, `s_mov_b32 exec_lo, sX` 4, a few real uses (`v_mul_f32`, `v_mul_lo_u32`, an `s_load` base).

  The first kind is the same dead-register case as step 28, but read directly by the compiler's lane-mask merge (`s16 = (s16 & ~exec) | (s37 & exec)`) and not through a join. Taking it as 0 in `operand()` had no effect: these instructions go through the lane-mask simulation (`Val` of masks, `get()` → `Val::Missing`), which refuses first. **Next: apply the same rule there**: a dead descriptor dword read as a lane mask counts as no lanes, under `VICTORY_DEAD_DESCRIPTOR_JOINS`.
- 29, **done**: in `atom()` (the lane-mask simulation's leaves), an opaque leaf whose value is a dead descriptor dword (`Val::Missing("a descriptor's address or format read as data")`) is *no lanes* (`false`).

  Why it is right: the console compiler's lane-mask merge is `s16 = (s16 & ~exec) | (s37 & exec)`. On this path, the variable s37 stands for was never set (to the compiler it is undefined, any value goes), and the register still holds a dead descriptor dword. That falls under shortcut #2 in `p4-shortcuts.md`, with the same switch `VICTORY_DEAD_DESCRIPTOR_JOINS=0`.

  The wall is gone from the top; **1,304 → 1,324**, none lost.
- 30, **cube images sized from their descriptor** (kind 5, DEPTH 0 cubes less one as RADV computes it, type 11), as the image lowering already takes cubes. Then the dead-dword rule extended to sampler and constant-buffer dwords: **1,324 → 1,340**.
- 31, **the dead-dword rule tightened from an assumption to a proof** (default): **1,265** proven; the assumption (1,340) behind `VICTORY_DEAD_DESCRIPTOR_JOINS=assume`, its 75 extra programs to be checked with real values. Details: [p4-shortcuts.md](docs/royale/p4-shortcuts.md).
- 42-43, `s_sext_i32_i16`, `s_absdiff_i32`, `s_bitreplicate_b64_b32`, uninitialized VCC and trap temporaries read as 0, `v_readfirstlane` destination in the heap scan: 1,332.
- 44, **the friend's lane-mask patch** (reviewed, merged with conflicts resolved): exact mask pairs, more mask ops, 4 new diff cases; 1,432, diff tests 28/28.
- 46, **the waterfall wall analysed** (`p4-shortcuts.md`, "The biggest wall"): the waterfall proof now accepts RDNA2's `s_andn2_wrexec` + `s_cbranch_scc1` retry and scalar accesses; the rest needs `structure.rs` to give a mask for the EXEC these loops write (the friend's area).
- 47, **`ds_append` / `ds_consume`** (an LDS counter bumped by the active lanes). Its meaning and addressing were checked with the console compiler: an `InterlockedAdd(shared, 1, old)` compiles to `ds_append` at the counter's offset, with M0 untouched. A new differential case checks it ("lds append", equal). It is refused only where a console wave runs as two host halves. **The harness now measures with the game's host settings** (RDNA, waves of 32 and 64, as the layer passes them). 1,434 → **1,447**, none lost; diff tests 29/29.
- 49, **buffer loads through a descriptor built around a table in the program's code**. The console compiler keeps a `static const` table indexed per lane in the code and reads it with `s_getpc` + `buffer_load_dword … offen`, as the new differential case shows. They now read the code's bytes, known at translation time (`code_dword`), with 0 past the descriptor's record count as on the console. Stores and typed or indexed loads are still refused. The whole blocker is gone: **1,450 → 1,530**, none lost; diff tests 31/31.
- **The big family `0x302dbfc0` (125 programs) is whole**: 13 good families, **228 native programs** (103 before). Four automatic runs: the first after the build showed black throughout (a cold pipeline cache with 125 new pipelines to compile; GPU 4 ms a frame, nothing drawn yet); the next three show the picture right, at 25.0-25.7 fps. The first launch after a large change can sit black for a while. Checkpoint `2026-11-28-translator-1530`.
- 48, **compares with DPP** (3 programs of the big families): lowered through the same `dpp()` every DPP VALU op uses, only where every lane writes its bit. A new differential case (float compares with quad neighbours, which the console compiler emits as VOPC with DPP) is equal. Its compares go through the lane-mask path, which already handled DPP; the plain path changed here shares `dpp()` with the verified ops. 1,447 → **1,450**; diff tests 30/30.
- **New whole family `0x33cb34f0`** (2 programs, the culling family's sibling): checked in two automatic runs, house whole, 24.7-25.0 fps. Checkpoint `2026-11-28-translator-1447` (12 good families, 103 programs).
- 45, **heaps from the title's own root signatures** (real data): the layer logs and passes each program's root signature flags; the rule (heaps in the last user SGPRs) is checked against the code both ways over all 2,680 programs; the code scan is now only a fallback. 1,434. Details: [p4-shortcuts.md](docs/royale/p4-shortcuts.md), "Heaps from the title's root signatures".
- 40, **three missing instructions** that gated the big families: `v_ldexp_f32` (2^n built from exponent bits as three normal factors), `s_flbit_i32_b64`, `s_bfm_b64`. 1,259 → 1,302.
- 41, **three more bugs in the view-heap scan** (`native::view_heap_sgpr`), found by tracing which instruction "overwrote" s6/s7 (`VICTORY_WHY_HEAP=1`):
  1. scalar loads marked **16 SGPRs** as written instead of the dwords they load, so an `s_buffer_load_dword` into s2 hid s6/s7;
  2. the scan stopped at the first match even when that SGPR is a listed table (s5 hid s6);
  3. an **8-dword load** from `(sX, 0)` at any offset was not taken as proof of the view heap (8 dwords is an image descriptor; samplers are 4).

  Buffer loads traced to the "sampler heap" (the misplaced view heap) went from ~2,030 to 586. **This is mostly a correctness fix**: programs that translated were binding those buffers from the wrong heap. 1,302 → 1,314.
- 39, **an outside soundness review**: a friend's patch closed seven holes in the proofs of steps 22-34 (loops, wide branches, carries, XOR folding, descriptor-only uses, view identity, wave64 masks), each with a test. 1,274 → 1,259 (15 programs translated only through the holes); diff tests 24/24. Details: [p4-shortcuts.md](docs/royale/p4-shortcuts.md), "An outside soundness review".
- 38, **the differential tests at work**: four alpha/half cases are all equal, and the console compiler never emits the title's mask-0x8 `d16` stores for them (that construct is still to be found). They also caught **a real translator bug**: spilled programs with root parameters in registers got negative user-data indices (a 4 GiB constant buffer). Fixed. Compute diff tests 24/24. Details: [diff-tests.md](docs/royale/diff-tests.md).
- 37, **differential tests running on Linux** (`scripts/diff-tests.sh`, [docs/royale/diff-tests.md](docs/royale/diff-tests.md)): console compiler under Wine, translated, run on the GPU under the game's Proton, against the PC compiler. **Compute: 21 of 21 equal.** Mesh: one real difference (a loop's second-pass vertices lost). Graphics: a test case one compiler rejects. From now on, shortcuts and blockers get HLSL cases checked against real console code.
- 36, **a bug in the heap scan**: it took the opcode bits of SOPP/SOPC instructions (branches, waits, compares) for a destination SGPR, so an `s_cbranch` of opcode 10 "overwrote" s10 and hid the view heap in later code. Fixed (only SOP1/SOP2/SOPK/SMEM have one). 1,268 → 1,274. 479 buffer-d1 loads still traced to the sampler heap remain to be looked at (`VICTORY_WHY_STALE=1`, `VICTORY_WHY_HEAP=1`).
- 35, **the stride step, and what it turned up**. Looking up the stride for table-picked descriptors too (`slot_word` now takes `Table` roots) changed nothing. A census of where descriptor dwords are left without value (`VICTORY_WHY_STALE`) showed why: the buffer descriptors whose stride was missing were traced to the **sampler heap**, i.e. the view heap placed wrongly again (s[n-1]) in waterfall loops. There the heap address is computed by an add (`s_add_u32 s20, s10, s14` with the first lane's byte offset, then `s_add_u32 s21, 0, src_scc`), a form `native::view_heap_sgpr` did not know.

  Now it knows it: a user SGPR that is the base of such an added address, whose 4-dword descriptor a `buffer_*` or `s_buffer_*` takes as its resource (or an 8-dword load), is the view heap. This is a correctness fix too: such loads were bound from the wrong heap. 1,267 → 1,268; ~480 such loads remain in other shapes.
- 34, **real data instead of assumptions**: the view's identity for descriptor dwords 0-1 where they are only compared with other descriptors' (proven), and the buffer stride from the host's slot table. 1,265 → 1,267. The census shows the next real-data step: the buffer stride for descriptors not picked by heap slot (47,385 reads). Details in `p4-shortcuts.md`, "Real data instead of assumptions".
- 33, **correction to step 32**: the two hits were image descriptor dwords only copied to spill lanes and back, so they were *false alarms*. My first reading of them was wrong. Now a proven case (`descriptor_only`). The unproven readers that remain include real numeric uses (waterfall comparisons), so `=assume` stays unsafe. Default 1,265. Details in `p4-shortcuts.md`, "Correction".
- 32, **the runtime marker tested in game** (`VICTORY_DEAD_MARKERS=1`): of ~1,744 assumed-dead edges watched, 2 ran, and **both are real counterexamples**:
  - a descriptor's address compared as a number, to see whether a texture changed. Fix: give dwords 0-1 the view's identity;
  - EXEC restored from a register our analysis wrongly takes for a descriptor dword. Fix: the analysis.

  The proven default stays; `=assume` must not be used in play. Details: [p4-shortcuts.md](docs/royale/p4-shortcuts.md), "The runtime marker in game".
- (Before step 31:) **The dead-dword rule is being tightened from an assumption to a proof.** Why, the risk, the plan and how to test it with real values: [p4-shortcuts.md, "Shortcut #2 in depth"](docs/royale/p4-shortcuts.md).
- Next walls (before step 30):
  - "an image dimension (cube or MSAA)" (261, first);
  - "a lane mask holding lanes elsewhere read as data" (224);
  - "an opaque lane mask" (207);
  - "lane conditions too large" (128);
  - "a descriptor built around a table in the code" (80)., then "an opaque lane mask" (209), "a lane mask holding lanes elsewhere read as data" (150), "lane conditions too large" (128).

### Deferred on purpose (done 27 Nov, step 27): image stores of the alpha channel alone (69 programs then, 553 later)

**What the console code does.** `image_store v0, v[2:3], s[16:23] dmask:0x8 dim:SQ_RSRC_IMG_2D unorm d16` (e.g. `218_0000000f00419900` at 0x175ec): it writes **only the fourth channel (A)** of a texel of an RGBA texture and leaves R, G and B as they are. The same program also stores all four channels elsewhere (`dmask:0xf`), so the format does have four.

**Why it is not done yet.** On the console the hardware writes only the channels the mask names. D3D12/DXIL has no partial-channel typed store: `textureStore` writes all four (its mask must be 0xF on typed UAVs), and a texture's typed view cannot pick one channel of an RGBA8 format. Writing 0 or anything else in R, G and B would destroy them.

**Ways to do it, in order of preference:**
1. **Read-modify-write per texel** (`textureLoad` the texel, replace A, `textureStore` all four), allowed only when the analysis proves **each texel is written by one thread** in the dispatch: the coordinates are a one-to-one function of the thread id (e.g. `x = group.x * 8 + tid.x`, `y = group.y * 8 + tid.y`, as in the per-texel passes we saw). Then nothing else in that dispatch touches the texel, and the earlier `dmask:0xf` stores of the same program are ordered before it within the thread. Needs: a test in the lowering that the store's coordinates come from the thread and group ids alone, typed UAV loads of that format (supported for RGBA8_UNORM/R16G16B16A16 on RDNA via typed UAV load), and the d16 half conversion.
2. **Raw access** when the texture is linear in memory (a buffer-backed texture): write the A byte/half with an atomic AND/OR on its dword (the translator already does that for 8/16-bit raw buffer stores). Only for linear layouts; tiled textures are not addressable so.
3. Otherwise refuse, as now.

**Where:** `emit/lower.rs`, image stores, the `prefix` test next to "an image store of some channels".

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
