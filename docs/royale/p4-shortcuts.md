# P.4 translator shortcuts to verify in game

Every translator step that rests on an assumption about the title's code, rather than on exact semantics. A program that translates is not proven correct. Each shortcut has a switch that turns it off, so it can be checked by A/B runs.

**How to check one:**
1. Run a family that becomes whole thanks to it, with and without the switch (`VICTORY_NATIVE_COMPUTE=1 VICTORY_NATIVE_COMPUTE_ONLY=<list>`, `K_EARLY_SHOT=1 K_SHOTS=2`, several runs, the same save).
2. Compare the screenshots, and what the user sees, with plain play.
3. Write the result here.

| # | shortcut | where | switch to turn it off | assumption | risk if wrong | status |
|---|---|---|---|---|---|---|
| 1 | **Partial image stores merged** (step 27): read the texel, replace the masked channels, write all four | `emit/lower.rs`, image stores (`merge`) | `VICTORY_PARTIAL_IMAGE_STORES=0` | each texel has one writing thread per dispatch (the programs take one entry each of a list of texels) | two threads writing different channels of one texel at once lose one write: wrong alpha or colour on some texels | **to verify** |
| 2 | **Dead descriptor dwords in joins are 0** (step 28), **and read as lane masks are no lanes** (step 29) | `emit/lower.rs`, phi lowering and `atom()` | `VICTORY_DEAD_DESCRIPTOR_JOINS=0` | a join that mixes a descriptor's address/format dword with another value is read only on paths where the other value is live (the compiler reuses the register) | a program that really reads an address or format through the join gets 0: wrong data, or reads at address 0 (the log's "shaders touched pages that had no copy … 0x…") | **to verify** |
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
