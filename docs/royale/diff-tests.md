# Differential tests on Linux (28 Nov 2026)

**What they are.** `crates/victory-shader/src/emit/diff_tests.rs`, written upstream:
1. Each case is a small HLSL shader, compiled twice: by the **console's own compiler** (its native-code variant, which writes the same `ACCC` part the title ships) and by the **PC compiler**.
2. The console build is **translated by our translator**.
3. Both run on the GPU with the same inputs (D3D12), and the results are compared.

A pass means our translation of real console code computes what the HLSL says. This is the "real data" check the user asked for.

**How to run them** (no game needed, but the game must be closed: same GPU, and vkd3d-proton):

```sh
scripts/diff-tests.sh                                  # all diff tests
scripts/diff-tests.sh translated_compute_matches_pc    # one test
VICTORY_DIFF_ONLY="cube sampling" scripts/diff-tests.sh translated_compute_matches_pc   # one case
```

What the script does:
1. Builds the Windows test binary (`cargo xwin test --no-run`, in `target-diff/`, apart from the game's `target/`).
2. Runs it under the game's Proton (GE-Proton11-14-vt, vkd3d-proton on the RX 7900 XT), in its own prefix `~/Games/victory-data/perfiles/difftest`.
3. Writes the output to `diff-tests.log` there (Proton does not pass console output through), waits for that prefix's wineserver, and prints the log.

**What it needs** (local, never in the repository):
- `K_CONSOLE_DXC`: the console SDK's native-code compiler, from the user's own SDK copy. By default the current generation's `xdxc.exe` under `~/Games/xbox/…/261000/xbox/bin/gen9/`. The plain `dxc.exe` there writes DXIL only, with no `ACCC` part, so it does not exercise the translator.
- `K_PC_DXC`: the public DirectXShaderCompiler release `v1.9.2609` (`dxc.exe`, `dxil.dll`) in `~/Games/victory-data/tools/dxc-pc/unpacked/x64`, downloaded from Microsoft's GitHub releases. `dxil.dll` is also the validator (`VICTORY_DXIL`).

**A harness change.** Since step 35 the translator also reads the layer's table of strides (`SLOT_STRIDES`, root SRV `t255`) for table-picked buffers. The compute test now builds that table for its own views by the layer's rule:
- structured: its stride;
- raw: 1;
- typed: the format's bytes, with `VIEW_INTEGER` for integer formats.

It binds that table where the translation asks for it. This mirrors `view_stride`/`view_kind` in `victory-gpu`.

## Results, first run (28 Nov, ~02:10)

| test | result |
|---|---|
| `translated_compute_matches_pc` | **21 of 21 cases equal.** The cases: ids and stores, shared memory and barrier, sample and divergent store, buffers and root views, conditional memory access, select on joined compares, lane reads, sub-dword operations, carries and gradients, comparison sampling, 16-bit texture reads, sizes from descriptors, typed loads, wave operations, aggregated and texture atomics, cube sampling, indexed registers and bit fields, size queries, gather with offsets, constant tables, 16-bit and 64-bit arithmetic |
| `translated_mesh_shaders_draw_as_pc_ones` | 4 equal (one triangle; groups, counts and culling; two waves; quad reads). **1 differs: "grid through shared memory"**: the translated draw misses part of the grid |
| `translated_shaders_draw_as_pc_ones` | 6 equal (constant; interpolated; sample and branch, within 2e-7; wave range; uniform loop; bit loop). Then **one compiler rejects the case "integer loop"** (`Int type 'i33' has an invalid width`, its own validator): a compiler issue with that HLSL, not the translator. To find which compiler and skip or reword the case |
| `float_semantics_are_the_consoles` | passes |

**The mesh difference, a real translator bug.** The case runs 32 threads that write 36 vertices: `for (v = i; v < 36; v += 32)`, so threads 0-3 take a second pass. In the translated draw the cells that use vertices 32-35 are missing, so the second pass's writes are lost. The title's graphics programs (device method 0x1c8, P.4's next part) include mesh shaders, so this matters later. Next: dump the case (`VICTORY_EMIT_DUMP`) and compare the console code's loop with our lowering.

**What this changes for P.4.** Every shortcut or blocker can now be checked against real console code:
1. write an HLSL case that makes the console compiler emit that pattern;
2. add it to `CS_CASES`;
3. run `scripts/diff-tests.sh`.

Candidates now:
- the alpha-only image store (step 27);
- a waterfall loop over descriptors compared lane against first lane (step 34);
- a descriptor kept in spill lanes around a loop (the marker hits, step 33);
- the occlusion test with level gathers (the house bug).

## Checking shortcut #1 (partial image stores) with console code (28 Nov, ~02:40)

Two cases added to `CS_CASES`, both **equal**:
- "alpha-only store": each thread writes its texel; after a barrier, half the threads set the alpha of their neighbour's texel;
- "alpha-only store, read and written back": the texel is read, its alpha changed, and the texel written back.

**But neither exercises our merge path.** Dumping the console code (`VICTORY_EMIT_DUMP`, then `translate_file FILE --code OUT` and `llvm-mc`) shows that for both, the console compiler emits `image_load … dmask:0x7` (RGB) then `image_store … dmask:0xf` (all four). It does not emit a partial store, and `VICTORY_WHY_MERGE=1` stays silent.

**What we learn (real data):** the console compiler does not turn `tex[p].w = x`, or a read-modify-write of a float4 texel, into a mask-0x8 store. The title's `image_store … dmask:0x8 … unorm d16` (half 1.0 into alpha, e.g. `218_0000000f00431000`) must come from something else. The SDK's list of console intrinsics has no partial texture store (checked: typed buffer loads and stores, global memory, LDS and lane intrinsics, nothing for texture channel masks).

**Next candidates:**
- **a half-precision texture** (`RWTexture2D<half4>` / `min16float4` with 16-bit types): the `d16` points there, and the harness needs a 16-bit texture UAV for it;
- **a `unorm` texture format** whose view the compiler knows.

Until a case makes the console compiler emit it, shortcut #1 stays **not verified by console code** (it is still the only way to translate those stores at all).

## Half precision, and a translator bug the tests found (28 Nov, ~03:00)

**The harness gained an RGBA16F texture UAV** (`View::HalfTextureUav`, register `u4`, the fifth entry of the compute UAV table; the typed views of `raw` moved from heap index 9 to 10). There is a new case, "half alpha-only store": a `half4` texel read, its alpha set to 1.0 and written back, the others written whole. It is **equal**.

The console code again shows no partial store: `image_load … dmask:0x7`, then `image_store … dmask:0xf`, not even `d16`. **The title's mask-0x8 `d16` stores are still unexplained**, and shortcut #1 stays unverified by console code.

**A real translator bug, found on the way.** A first try added the half texture as an eighth root parameter. With eight, the console compiler moves root parameters into a "spilled" user-data table in memory (index 16 on), while the root constants stay in registers (s2).

The translator numbered user data from 16 for every spilled program (`Lower::user_first`). That held for the only spilled programs seen before (the ray-tracing structure builder, nothing in registers), but here the root constants came out at index 2 - 16, wrapped to 4294967284. The DXIL then declared a constant buffer of 4 GiB, and the validator refused it.

Fixed: a spilled program that also has root parameters in registers numbers its user data from 0 (the host fills each dword by its index anyway).

**Still open on that path** (case not kept): with the fix, the spilled variant validated but its half-texture writes came out 0 while the PC's did not. Spilled programs with parameters in registers too need their own test. To do: a case with eight root parameters, checked word by word.

**State:**
- compute diff tests **24 of 24 equal**;
- harness 1,274 of 2,680 (none lost);
- 209 unit tests pass.
