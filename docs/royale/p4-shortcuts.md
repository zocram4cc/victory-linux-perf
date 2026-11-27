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
