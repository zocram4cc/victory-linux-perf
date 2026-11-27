# P.4 translator shortcuts to verify in game

Every translator step that rests on an assumption about the title's code, rather than on exact semantics. A program that translates is not proven correct. Each shortcut has a switch that turns it off, so it can be checked by A/B runs.

**How to check one:**
1. Run a family that becomes whole thanks to it, with and without the switch (`VICTORY_NATIVE_COMPUTE=1 VICTORY_NATIVE_COMPUTE_ONLY=<list>`, `K_EARLY_SHOT=1 K_SHOTS=2`, several runs, the same save).
2. Compare the screenshots, and what the user sees, with plain play.
3. Write the result here.

| # | shortcut | where | switch to turn it off | assumption | risk if wrong | status |
|---|---|---|---|---|---|---|
| 1 | **Partial image stores merged** (step 27): read the texel, replace the masked channels, write all four | `emit/lower.rs`, image stores (`merge`) | `VICTORY_PARTIAL_IMAGE_STORES=0` | each texel has one writing thread per dispatch (the programs take one entry each of a list of texels) | two threads writing different channels of one texel at once lose one write: wrong alpha or colour on some texels | **to verify** |
| 2 | **Dead descriptor dwords in joins are 0** (step 28) | `emit/lower.rs`, phi lowering | `VICTORY_DEAD_DESCRIPTOR_JOINS=0` | a join that mixes a descriptor's address/format dword with another value is read only on paths where the other value is live (the compiler reuses the register) | a program that really reads an address or format through the join gets 0: wrong data, or reads at address 0 (the log's "shaders touched pages that had no copy … 0x…") | **to verify** |
| 3 | **Buffer dword 1 = stride << 16** (step 28) | `emit/lower.rs`, `descriptor_data` | none (always on) | the stride from the FSPX record is the descriptor's | wrong element size: wrong record counts in raw views | to verify with 1-2 |
| 4 | **High half of table addresses proven 0** over reaching paths, XOR multisets, 0 + carry (steps 22-26) | `resource.rs`: `is_zero`, `xor_leaves`, `edge_reaches`, `one_leaf` | none | heap and table addresses fit in 32 bits (already assumed by `high_half`) | none expected: console heaps are below 4 GB | low |
| 5 | **View heap SGPR from the code** (step 19) | `native::view_heap_sgpr` | none | the user SGPR used as a 32-bit base with index × 32 is the view heap | a wrong SGPR filled with the heap's start: wrong descriptors | checked: family 0x33cb3490 loads its pyramid view right (desc trace) |
| 6 | **Gather at a level read as four texel loads** (upstream) | `emit/lower.rs`, gathers | none | level rounded to nearest, edges clamped | the occlusion culling (0x33cb3490) tested against wrong texels | read checked by hand (`p4-house-bug.md`): right for the culling's integer levels |
| 7 | **Descriptor tracing work bound** (step 21) | `resource.rs`, `WORK` | none | none (it refuses past 3,000 steps) | none (a refusal, not a wrong translation) | safe |

**Order to verify:** 1 and 2 first (they made the most programs translate: 11 and 65). When a family whole only because of them is tested, run it twice, with the switch on and off. If off changes nothing (the family no longer translates, so those dispatches are dropped) but on looks wrong, the shortcut is the suspect.
