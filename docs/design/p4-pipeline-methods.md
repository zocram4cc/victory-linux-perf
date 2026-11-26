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

## Dumped at run time (26 Nov, `royale-save/logs/20261126-053724-run.log`, `P.4 block` / `P.4 table entry` lines)

- **The table is root signatures, not shaders.** Every entry's container starts with `DXBC` and holds an `RTS0` part: a serialized root signature. Entry words: `{device object, index << 32, length, pointer, length, id 0x8075_00NN_0000_0003}` with consecutive ids. So word 0 of the description names the root signature(s) the pipeline uses (or a list they live in).
- **The heap blocks are console GPU code.** `0xf00220200` (0x200 bytes) and `0xf00220000` (0x180) begin `920c80d5 33d7fe03 … 0300a0bf 040388be 800389be`: RDNA instructions (`s_mov_b32 s8, s4`, `s_mov_b32 s9, 0`, …). These pipelines are made from **precompiled ISA in GPU memory**, not from a DXBC/DXIL container.
- **The image blocks are small metadata structures** next to each code block (0x14-0x184 bytes: counts, register numbers, `03040000 04010000 60000000…`), likely the shader's register and user-data layout, which a DXIL container gives through its own parts.
- Some later "pairs" are not (pointer, length) at all (lengths like `0x147ecf7c0`): the description is a structure with more fields than six pairs, and its exact layout is still to be read from the code that fills it.

**What this means:** implementing them needs the translator to start from raw ISA plus this metadata, instead of from a container. Next: compare with what `victory_shader` extracts from a container (it translates console ISA carried inside the containers `create_graphics` receives). If the container's ISA part and these blocks are the same format, a container can be built from them and handed to the existing path.

**The existing path, for comparison** (`crates/victory-shader/src/native.rs`): `create_graphics` receives `DXIL` parts whose `XDAT` index holds an `XBCH` (zlib) blob, unpacked to `FSSM`. The `FSSM` payload has one **record** per stage (code offset and size, flags with wave32, root layout and used-resource tables with counts, register counts), followed by stage data (PS inputs and export patches, VS inputs and outputs, CS group size, mesh outputs). The translator works from `NativeProgram` (code + record + tables + stage data), not from the container as such.

**Hypothesis to test next:** each image block of a `0x1c8`/`0x218` description is one stage's record plus its stage data (sizes 0xbc-0x2f0 fit: a 0x54-byte record and the VS/PS/CS data after it), with absolute pointers where `FSSM` has payload-relative offsets, and the heap block next to it is that stage's code. If so, a `NativeProgram` can be built from each (metadata, code) pair and handed to the existing translator, and the pipeline made as `create_graphics` makes it, with the root signature from the table.

## Steps

1. **Census:** count the calls of each method per run, and the draws and indirect draws dropped for want of a pipeline. Is it these pipelines, or other ones (untranslated shaders, slow compiles)? Log `missing()` reasons for the top dropped pipelines in a diagnostics build.
2. **Decode:** dump the six blocks and the table's containers of a few calls. Compare the containers with what `create_graphics` receives (the same DXIL/console container format?). Disassemble the code that fills the description (from title+0x24774c0 back) to name the fields.
3. **Implement** behind `VICTORY_PIPELINE_X=1`: build the same `Shader` list `create_graphics` builds, then `hand_out`. Off until a run shows the dropped counts fall and the picture is unchanged or better.
