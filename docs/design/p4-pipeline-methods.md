# P.4: the two pipeline creation methods the layer does not implement

Roadmap task P.4, piece **P**. Notes started 26 Nov 2026, from the play log of that morning (`royale-save/logs/20261126-050658-run.log`) and the title image VIctory keeps in its cache (`victory-data/perfiles/royale-save/cache/images/victory-image-b99f876b74d4a77f.dll`, not the game folder).

## What happens today

- Device methods `0x1c8` and `0x218` are `unknown_pipeline` (`crates/victory-gpu/src/device.rs`): they return `E_NOTIMPL` with `out` null, and the title goes without the pipeline, silently. No fork implements them (fxceless, wineless, upstream `63bcca4` checked on 26 Nov).
- In play, minutes into the open world: **`dropped 342 without a host pipeline (3 being made) and 981 indirect`** per frame. These draws never get a pipeline. Missing passes are a candidate for S.9 (dark interiors, black silhouettes) and for missing objects.

## What the title passes (disassembly of the cached image)

**`0x1c8`**, called at title+0x2477578:
- `rcx` is the device, `rdx = r12 + 0x40` (the description), `r8` is the IID at `0x1461e06b8`, and `r9` is a stack slot (`out`).
- The IID is `765a30f3-f624-4c6f-a828-ace948622445`, **`ID3D12PipelineState`**. The result is tested with `js` (a failure skips the rest).

**`0x218`**, called at title+0x24800d5: the same IID and the same shape, with `rdx` an element of a linked list (`+0x18`).

**The description (logged words, `0x1c8`):**
```
[0]  pointer P to a table (below)
[1..12]  six (pointer, length) pairs, e.g.
     (0x145969d48, 0x184) (0x145969ecc, 0xbc) (0xf002b3400, 0x1c0)
     (0x145969f88, 0xd0)  (0xf002b3600, 0x180) (0x14596a058, 0x2f0)
```
The pairs alternate between the title image's data (`0x14…`, lengths 0xbc-0x2f0) and title heap memory (`0xf00…`, lengths 0x180-0x200). A guess to test: the heap blocks are the console's compiled shader code (ISA), and the image blocks their metadata. Some pairs are `0, 0` (absent stages).

**The table P:** entries of 6 qwords:
```
{ root signature (0x6ffffdc9c3d0, one of ours),
  u32 0 | u32 index << 32 (0x27, 0x07, 0x03 ...),
  length, pointer, length (again),
  tag 0x8075_0008_0000_0003 / 0x8146_000a_0000_0003 }
```
These are shader containers by (pointer, length), like the PC description's `D3D12_SHADER_BYTECODE`, with an index and a tag. Lengths seen: 0x70, 0x530, 0x1e04.

**Checked offline, 26 Nov:** in the cached image the `0x14…` addresses of the pairs fall in code (x86 bytes). So either they are not image data, or the image's data there differs at run time. The blocks must be dumped from the running process (step 2), not read from the file.

## Steps

1. **Census:** count the calls of each method per run, and the draws and indirect draws dropped for want of a pipeline. Is it these pipelines, or other ones (untranslated shaders, slow compiles)? Log `missing()` reasons for the top dropped pipelines in a diagnostics build.
2. **Decode:** dump the six blocks and the table's containers of a few calls. Compare the containers with what `create_graphics` receives (the same DXIL/console container format?). Disassemble the code that fills the description (from title+0x24774c0 back) to name the fields.
3. **Implement** behind `VICTORY_PIPELINE_X=1`: build the same `Shader` list `create_graphics` builds, then `hand_out`. Off until a run shows the dropped counts fall and the picture is unchanged or better.
