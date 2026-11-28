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
