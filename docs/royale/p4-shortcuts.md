# P.4 translator shortcuts to verify in game

Every translator step that rests on an assumption about the title's code, rather than on exact semantics. A program that translates is not proven correct. Each shortcut has a switch that turns it off, so it can be checked by A/B runs.

**How to check one:**
1. Run a family that becomes whole thanks to it, with and without the switch (`VICTORY_NATIVE_COMPUTE=1 VICTORY_NATIVE_COMPUTE_ONLY=<list>`, `K_EARLY_SHOT=1 K_SHOTS=2`, several runs, the same save).
2. Compare the screenshots, and what the user sees, with plain play.
3. Write the result here.

| # | shortcut | where | switch to turn it off | assumption | risk if wrong | status |
|---|---|---|---|---|---|---|
| 1 | **Partial image stores merged** (step 27): read the texel, replace the masked channels, write all four | `emit/lower.rs`, image stores (`merge`) | `VICTORY_PARTIAL_IMAGE_STORES=0` | each texel has one writing thread per dispatch (the programs take one entry each of a list of texels) | two threads writing different channels of one texel at once lose one write: wrong alpha or colour on some texels | **to verify** |
| 2 | **Dead descriptor dwords in joins are 0** (step 28), **and read as lane masks are no lanes** (step 29): **now only where proven**; the assumption behind `=assume` | `emit/lower.rs`, phi lowering (`edge_unread`) and `atom()` | `VICTORY_DEAD_DESCRIPTOR_JOINS=0` (off), `=assume` (the assumption) | a join that mixes a descriptor's address/format dword with another value is read only on paths where the other value is live (the compiler reuses the register) | a program that really reads an address or format through the join gets 0: wrong data, or reads at address 0 (the log's "shaders touched pages that had no copy … 0x…") | **to verify** |
| 3 | **Buffer dword 1 = stride << 16** (step 28) | `emit/lower.rs`, `descriptor_data` | none (always on) | the stride from the FSPX record is the descriptor's | wrong element size: wrong record counts in raw views | to verify with 1-2 |
| 4 | **High half of table addresses proven 0** over reaching paths, XOR multisets, 0 + carry (steps 22-26) | `resource.rs`: `is_zero`, `xor_leaves`, `edge_reaches`, `one_leaf` | none | heap and table addresses fit in 32 bits (already assumed by `high_half`) | none expected: console heaps are below 4 GB | low |
| 5 | **View heap SGPR from the code** (step 19) | `native::view_heap_sgpr` | none | the user SGPR used as a 32-bit base with index × 32 is the view heap | a wrong SGPR filled with the heap's start: wrong descriptors | checked: family 0x33cb3490 loads its pyramid view right (desc trace) |
| 6 | **Gather at a level read as four texel loads** (upstream) | `emit/lower.rs`, gathers | none | level rounded to nearest, edges clamped | the occlusion culling (0x33cb3490) tested against wrong texels | read checked by hand (`p4-house-bug.md`): right for the culling's integer levels |
| 7 | **Descriptor tracing work bound** (step 21) | `resource.rs`, `WORK` | none | none (it refuses past 3,000 steps) | none (a refusal, not a wrong translation) | safe |

**Order to verify:** 1 and 2 first (they made the most programs translate: 11 and 65). When a family whole only because of them is tested, run it twice, with the switch on and off. If off changes nothing (the family no longer translates, so those dispatches are dropped) but on looks wrong, the shortcut is the suspect.


## Shortcut #2 in depth: dead descriptor dwords (27 Nov night)

**What the rule does today** (steps 28-29 and their extension): a register that holds a descriptor's dword with no host value is treated as 0 on that edge, or as no lanes. "No host value" means the address or format of a view, a sampler's or a constant buffer's dword: the layer cannot give the console's bits. This applies where such a register reaches a join (phi) or the lane-mask simulation (`atom()`). Set: `DEAD_DESCRIPTOR_WORDS` in `emit/lower.rs`; switch `VICTORY_DEAD_DESCRIPTOR_JOINS=0`. It made about 100 programs translate (1,239 → 1,340 with the steps around it).

**Why the idea is right.** The console compiler allocates *variables* to registers. On a path where a variable is undefined, the code it generates gives the right result whatever the register holds: the compiler only relies on the variable where it is defined. On the console the register really does hold the old descriptor dword, and the program still works. So *if* the dword is dead at that read, any value we give, 0 included, yields exactly the console's result.

Seen in `218_0000000f002d6300` at 0x14e50. The lane-mask merge `s16 = (s16 & ~exec) | (s37 & exec)` reads s37, which on that path last held a descriptor dword.

**Where the risk is.** Today the rule *assumes* deadness as soon as such a dword reaches a join or a mask; it does not prove it. If a program really used that dword as a number on a path where it is live (an address to compute with, a format to test), the console would read the real bits and we read 0. The result is wrong data with no error.

**The tighter rule (being implemented):** take the dword as 0 only where it provably cannot influence the result.
1. **In a join:** only on an edge that no lane reading the join can have come by. This is the same test as `resource::Tracer::edge_reaches` for the high half of addresses, done in the lowering with the lanes' paths (`Structured`: `reach`, `exits`, `bools.meet`).
2. **As a lane mask:** only where the leaf is ANDed with EXEC in a block that no lane reaches with that value (the same reachability).
3. Elsewhere: refused, as before the shortcut.

Some of the ~100 programs may stop translating; those that stay are right by construction.

**Testing with real values (the user's request: the programs must get correct data).** Planned, as soon as possible:
- **A/B in game** of any family whole thanks to this rule: on, and off (`VICTORY_DEAD_DESCRIPTOR_JOINS=0`).
- **Read the real values on the console's side of our layer.** The layer knows each view's description; a debug switch can log, for a native dispatch, the dwords such a register would have held, i.e. the view's console descriptor bits from the title's heap record. If any program then reads them as a number on a live path, the trace would show it.
- **A runtime check:** a debug build of the translation can write, where the rule substitutes 0, a marker into a debug buffer when the substituted value is actually used (an edge taken by an active lane). Any hit in game means the rule was wrong there.


### Done (27 Nov, late): the rule is now proven by default

**Measured** (harness over the 2,680 dumped 0x218 programs):

| version | translate |
|---|---|
| assumption everywhere (steps 28-30) | 1,340 |
| lane-mask leaf proven, joins assumed | 1,334 |
| **both proven (default now)** | **1,265** |

**What "proven" means in the code (`emit/lower.rs`):**
- **Lane-mask leaf** (`atom()`, `Atom::Opaque`): the register must *be* the descriptor dword (`self.stale`), not a join that brings it on some paths. Then no path ever set the mask's variable here: it is undefined on every path, so the compiler's code cannot depend on it, and "no lanes" gives the console's result. Costs 6 programs.
- **Join** (phi lowering): only on an edge that no lane reading the join can have come by (`Lower::edge_unread`). It collects the blocks that read the join; a read inside another join → not proven. For each reader, it asks the lanes' paths (`Structured`: `reach`, `exits`, `bools.meet`) whether a lane can both reach the reader and have come by that edge; it must be no for every reader. Costs 69 programs: the proof rarely succeeds, so those programs did depend on the assumption.

**What did not change.** The translator's original rule, `Lower::dead`: number joins with at least one real number, where a dead dword is 0. It rests on the upstream authors' reasoning: "compiled code joins numbers with numbers". It was already there before our work; it stays as it was and is listed here for completeness.

**The 75 programs that need the assumption** are behind `VICTORY_DEAD_DESCRIPTOR_JOINS=assume`. Before they are accepted, check them with real values:
1. An A/B in game of any family that becomes whole with `=assume` (screenshots against plain play and against the default).
2. A runtime marker: a debug translation that writes to a debug buffer whenever an assumed-dead value is actually used by an active lane. Any hit means the assumption was wrong there. To be built: emit `atomicrmw or` into a host debug UAV at each substituted edge, guarded by "this lane came by that edge".

### The runtime marker in game (27 Nov, ~22:40): the assumption is wrong in places

**Run** (`bench/k-…-dead-markers`, cache `cache-marker`): all 1,340 programs that translate with the assumption (`bench/p4-assume/all-assume.txt`), with `VICTORY_DEAD_DESCRIPTOR_JOINS=assume VICTORY_DEAD_MARKERS=1`. The picture was broken (black, grey, the HUD, ~1 fps at first), as expected with partial chains. It did not matter: the run only asked whether the assumed-dead edges execute.

**Result:**
- ~1,744 assumed-dead join edges were watched; 69 could not be (their block branches elsewhere too).
- **Only 2 marker hits**: code `0x2014` and `0x2214`.
- Found with `VICTORY_WHY_DEAD=1` in the harness, which prints each assumed edge with its program.

**Both hits are real counterexamples:**

1. **`218_0000000f05456500`, the join near 0x2200: a descriptor's address compared as a number.** A loop loads an image descriptor (`s[100:107]`, from spill lanes of v71) and reads a 3D texture with it (`image_load … dim:SQ_RSRC_IMG_3D` at 0x224c). On the next pass it moves the joined `s[100:101]` (the descriptor's dwords 0-1, its address) into `s[26:27]` and compares (`s_cmp_eq_i32 s27, s67`, `s_bfe_u32 s26, …`) to tell whether the texture changed. **The address is used as a number.** With 0 every comparison says "same texture", and needed reloads would be skipped.

   **A correct fix exists:** an equality test does not need the console address, only a value that *identifies the view*. Giving dwords 0-1 the view's identity (its heap slot or the layer's view entry number, unique per view) makes "same descriptor?" answer as on the console.

2. **`218_0000000f05499700`, the join at 0x2014: EXEC restored from the joined register** (`s_mov_b32 exec_lo, s100`). A correct program never loads EXEC from a descriptor dword, so on that edge s100 (`= s53`) must really be a saved lane mask, and our analysis marks it as a descriptor dword. 0 there would switch every lane off for the rest of the program. Besides refuting the assumption here, **this points at a wrong tracking in our analysis** (likely the spill-lane lives of v79 in `ir::unspill`, or the register's origin across the join). To investigate.

**Conclusions:**
- The default (proven) rule stays. **`=assume` is unsafe and must not be used in play.**
- The marker method works: it singled out the two places out of ~1,744 where the assumption is tested in practice.
- The 75 programs that need the assumption need, instead:
  - the view-identity values for dwords 0-1 (case 1);
  - a fix of the analysis (case 2);
  - then this test again.


### Correction (27 Nov, ~23:15): the two marker hits were false alarms, misread by me

My reading of the two hits (a comparison of descriptor addresses; EXEC restored) came from the disassembly around the offsets, **not from the registers the translator had marked**, and it was wrong.

`VICTORY_WHY_DEAD=1` now prints the register and its readers. In both programs (`…05456500` at 0x2214, `…05499700` at 0x2014), the marked joins are **`s80`, `s85`, `s86`, `s87`, dwords of an image descriptor `s[80:87]`**. Their only readers are `s_mov` copies, the spill saves that `ir::unspill` turns into moves: a descriptor kept in spill lanes and restored on the next loop pass to be used again as a descriptor. No number depends on those values. The descriptor use itself is traced to its load, not read.

**Made a proof** (`Lower::descriptor_only`): a dead dword is 0 by right when every use, followed through copies (`s_mov_b32`/`s_mov_b64`) and joins, is the resource or sampler operand of a memory instruction. `soffset` is excluded: it is a number. Both hits are now proven cases.

No program gained: the 75 need other joins too. **What those other joins' readers are** (all assumed-dead edges, with `=assume`):

| reader | count | what it means |
|---|---|---|
| `s_mov_b32` | 777 | copies whose chain ends in some other use |
| `image_load` | 123 | descriptor uses: fine, but the same value also has other readers |
| `v_readfirstlane_b32` | 63 | **waterfall loops**: lanes compare their descriptor with the first lane's. A real numeric use; 0 for all would wrongly say "uniform" |
| `v_cmpx_eq_i32` | 63 | the same |
| `s_and_b32` | 34 | lane-mask merges |
| `s_mul_i32` | 19 | real numeric uses (sizes from descriptor bits) |
| `s_bfe_u32` | 11 | the same |
| float ops (`v_max_f32`, `v_add_f32`, `v_mul_f32`, `v_subrev_f32`, `v_fma_f32`) | 45 | a float op on a descriptor dword cannot be meaningful: the register is dead there at run time (reused), but the analysis cannot prove it path by path |

**Status:**
- Default: proven only, **1,265**.
- `=assume` stays unsafe: the waterfall comparisons (63) are real numeric uses where 0 is wrong.
- **To gain these programs correctly:**
  - give descriptor dwords values that answer comparisons as on the console: the view's identity in dwords 0-1, for waterfall and change tests;
  - prove the float-op cases dead per path.


### Real data instead of assumptions (27 Nov, ~23:45; the user: "the assumptions do not help us; we should rely on real data")

Two steps that use only real values or proofs:

1. **View identity** (`Lower::compare_only` / `ident_dword`, `VICTORY_VIEW_IDENTITY=0` turns it off). Descriptor dwords 0-1 get the view's heap slot in dword 0 and 0 in dword 1, only where every use is proven to be one of:
   - a use as a descriptor;
   - an equality comparison (`s_cmp_eq/lg`, `v_cmp(x)_eq/ne`, 32-bit integers) whose other side is also dwords 0-1 of a traced descriptor load. A comparison with a constant (a null check, say) is not accepted.

   Equality answers as on the console: the same view gives the same number. Two slots naming one texture compare unequal; a waterfall loop then takes one more pass and a change test reloads, both still right.
2. **The buffer stride in dword 1 from the host's table** (`SLOT_STRIDES` via `slot_stride`), where the program's metadata does not give it.

Result: **1,265 → 1,267.** The census (`VICTORY_WHY_IDENTITY=1`) of what still blocks:

| use | count | note |
|---|---|---|
| `s_bfe_u32 x, d1, 0xe0010` | 47,385 | **the buffer stride**, of descriptors not picked by heap slot (tables, built descriptors) |
| `s_and_b32` | 6,374 | mostly the raw-view conversion `d1 & 0xc000ffff` |
| `s_nand_b32` | 2,932 | |
| `s_xor_b32` | 432 | |
| `v_cvt_f32_ubyte0` | 392 | a byte of a descriptor dword as a float: a real numeric use |
| others | < 300 each | |

**Next with real data: the stride for every buffer descriptor, not only heap-picked ones.** The FSPX metadata record of the table entry gives it (`e[16..20]`, step 9) where the load is from a table at a known entry, and the layer knows every view's stride. Pass it through the same host word mechanism (`RootType::Other`) per table entry, or look it up by the descriptor's slot computed from the table address (`heap_index`), which is the same as for heap-picked ones once the table base is known.

## Verifying with real data: console-compiled test shaders (28 Nov, ~02:00)

The user has legitimate access to the console SDK, kept locally. Rule: reference only, nothing copied into the repository (see `NEXT.md`, "Reference material").

**What works now, offline on Linux:**
- The console's own shader compiler runs under Wine (a plain prefix, `~/.wine-xdxc`; no game, no GPU).
- Its native-code variant writes the same `ACCC` part the title ships, beside the DXIL.
- `cargo run --release -p victory-shader --example translate_file -- FILE` translates such a file. First test: a compute shader that loads a texture and writes a structured buffer translates (2,540 bytes of DXIL).

**What this gives us.** Console code from HLSL we wrote, so we know exactly what it must compute. For each shortcut or blocker we can:
- write a small HLSL case that makes the console compiler emit the same pattern (a waterfall over descriptors, an alpha-only image store, an occlusion test with gathers, a lane-mask merge, a descriptor kept in spill lanes);
- check that our translation of it is correct;
- add it to the tests.

**Executing both and comparing results** is what `emit/diff_tests.rs` already does (it wants `VICTORY_CONSOLE_DXC` and a PC `dxc.exe`), but those tests are Windows-only (D3D12). Plan:
1. Build the test binary for Windows (`cargo xwin test --no-run -p victory-shader --target x86_64-pc-windows-msvc`).
2. Run it under the game's Proton, whose D3D12 is vkd3d-proton on the real GPU.
3. Point `VICTORY_CONSOLE_DXC` at a small wrapper that runs the console compiler under Wine.

That gives real side-by-side runs of console-compiled-and-translated against PC-compiled shaders: the strongest check we can have short of a console.

## An outside soundness review of the proofs (28 Nov, ~03:30)

A friend of the user, working from the helper prompt, sent a patch (`p4-proof-soundness.patch`). I reviewed it in full and applied it as one commit. It found real holes in the proofs of steps 22-34, each shown by a hand-assembled program in new unit tests:

| hole | what was unsound | fix |
|---|---|---|
| loops | `edge_unread`, `edge_reaches` and `lane_phi` met lane conditions across a loop's back edge, where the atoms belong to an earlier iteration | back edges are never pruned |
| wide branches | a branch the whole wave takes together carries no per-lane condition | `wide` blocks are not pruned |
| `0 + carry` | `plus_scc` took any `s_add_u32 x, 0, src_scc` as a 0 high half; SCC from an unrelated compare makes it 1 | only SCC from an add on the same base, at most once per path (`scc_from_base_add`) |
| XOR folding | `xor_terms` could take the SCC write as a result dword; 64-bit inline float constants were folded as integers | width-checked; float constants not followed |
| descriptor-only uses | a register that is also `soffset`, store data, or a BVH memory base was taken as a pure descriptor use; only the first read in an instruction was looked at | `is_pure_descriptor_use`, every read |
| view identity | comparisons mixing dword 0 and 1, non-view-heap loads, guarded or SDWA `v_mov_b32`, mixed joins | per-dword `rel`, view-heap loads only, those moves refused; dword 1 gets the identity too |
| wave64 masks | the "no lanes" rule looked only at the low dword of a 64-bit mask | the high dword must be a dead descriptor dword too |

**Measured:**
- 211 unit tests pass, the patch's 2 new ones included;
- the harness goes 1,274 → **1,259**: 15 programs had translated only through these holes, none in the families used in game;
- the compute differential tests stay **24/24 equal**.

This is the right trade: the user wants correct data over coverage.

## Heaps from the title's root signatures (28 Nov, ~06:00): real data, not deduction

Until now the translator *deduced* where a program's descriptor heaps sit:
- first with a fixed rule (view heap in s[n-2], samplers in s[n-1]);
- then from the code (`native::view_heap_sgpr`), which had to be fixed five times.

Now it **reads them from the title's own root signature**.

**Collecting the data.** `VICTORY_NATIVE_ROOT_LOG=1` makes the layer log, for each 0x218 description, the flags and parameter count of the root signature that description names (its word 0). That root signature is the title's serialized blob: a DXBC container whose `RTS0` part has the flags at byte 20 (`root::rts0`, `root::blob_flags`). One automatic run with P.4 on and an empty program list (no program made native, play unchanged) logged all 2,680 programs. They are in `bench/p4-root-flags.txt`:

| flags | programs | heaps indexed directly |
|---|---|---|
| `0xc00` | 1,164 | views and samplers |
| `0x0` | 1,026 | none |
| `0x400` | 483 | views |
| `0x800` | 7 | samplers |

(The two flag bits are the standard ones for directly indexing the view and sampler heaps; the SDK's reference for console root signature flags, read locally, names the same two.)

**Checked both ways:**
1. **Against the code scan:** wherever it had found the view heap (1,416 programs), it agrees with the flags without one exception: `0xc00` → s[n-2] (1,093), `0x400` → s[n-1] (323). It had missed the heap in 231 programs (`0x400` 160, `0xc00` 71).
2. **Against the code's heap loads** (a census of loads from a user SGPR by index stride): in `0xc00` programs, view descriptors (index × 32) come from s[n-2] and samplers (index × 16) from s[n-1]; in `0x400` programs, views come from s[n-1].

**The rule:** the heaps take the last user SGPRs. Both indexed: views in s[n-2], samplers in s[n-1]. One alone: it in s[n-1].

**How it is used:**
- The layer adds the flags to the program it rebuilds, as an FSPX piece of ours (`native::FSPX_ROOT_FLAGS = 0x7F`). The harness adds them from `bench/p4-root-flags.txt` (`K_ROOT_FLAGS=0` leaves them out).
- `fspx_program` places the heaps by the flags and clamps root constants below them. A listed table where the flags put a heap is refused, not guessed.
- The code scan stays only as the fallback when no flags come.
- No more phantom sampler heap in programs that index none.

**Result:** 1,432 → 1,434, none lost. The main gain is that every program's heaps now come from the title's data.

## The biggest wall: waterfall loops over a per-lane heap index (28 Nov, ~07:00), in progress

**"A descriptor's address or format read as data" (751 programs).** A sample of 300 failure sites, each disassembled at its offset: **166 are the back-edge branch of a loop** (`s_cbranch_scc1` backwards). Example: `218_0000000f00416300` near 0x306c:

```
v_readfirstlane_b32 s10, v6        ; the first active lane's heap index
v_cmpx_eq_i32_e32  s10, v6         ; keep the lanes with that index
s_lshl_b32         s10, s10, 5
s_load_dwordx8     s[32:39], s[42:43], s10   ; its image descriptor from the view heap
image_load         v[6:8], v9, s[32:39] …    ; results over v6..v8, active lanes only
s_andn2_wrexec_b32 s16, s16        ; drop the served lanes; SCC = some lane left
s_cbranch_scc1     <loop>
```

A **waterfall over a per-lane heap index**. The translator runs such a loop once for every lane, each lane with its own descriptor. Here, though, the resource analysis did not prove the loop (`Waterfall::compares` empty). Each lane's index in v6 meets the image_load's results on the back edge, and the join carries a value with no host meaning.

**Found and fixed so far** (`VICTORY_WHY_WATERFALL=1`, `VICTORY_WHY_DEFER=1`, `WHY-DEAD` with the joined argument's definition):
1. The proof only knew loops that retry with `s_cbranch_execnz`. The console compiler uses RDNA2's **`s_andn2_wrexec_b*` + `s_cbranch_scc1`**. It is now accepted when the branch reads exactly the SCC that wrexec writes, and the retrying lanes are the EXEC it writes.
2. **Scalar accesses** in the loop (`s_buffer_load`) read no EXEC. They now take the EXEC last written before them in the block.
3. `descriptor_only` follows `v_readfirstlane_b32` and accepts the waterfall compares the analysis recognized.

**What still stops it:** the lane analysis (`structure.rs`) has no lane mask for the EXEC these loops write (select checks 1 and 3 in `Tracer::waterfall`, 17 accesses in the example). No count change yet; 1,434, none lost.

**Next:** have `structure.rs` give a mask for the EXEC written by the loop's `v_cmpx` and wrexec. This is the lane-mask area the friend took, so it is worth asking him: the example and the diagnostics above reproduce it.
