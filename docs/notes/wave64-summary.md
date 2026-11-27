# Wave64 / cross-lane census of the title's programs

Measured by parsing the 19,139 shader containers the title created (kept in a
warm seed's `known-shaders.bin`) and the translations cached for them; no
program was run. Scripts beside this file: `census.py`, `forms.py`,
`summarize.py`, `blake3_ref.py` (run with `python3 -I`); full tables in
`summary-tables.md`, one row per program in `wave64-census.tsv`.

## How the flag is read

DXBC `DXIL` (or `ACCC`) part → `XDAT` entry of kind 0x0d → `XBCH` (zlib) →
`FSSM` payload → the stage's record: flags at +0x14, **bit 5 set = wave32,
clear = wave64** (`victory-shader/src/native.rs`, `FLAG_WAVE32`). The emitted
form is read from the cached DXIL's PSV0 runtime info (minimum and maximum
wave lanes: (n, n) is `WaveSize(n)`, (0, 0xFFFFFFFF) is none).

## Numbers

| | All | Compute | Pixel | Vertex | Mesh |
|---|---:|---:|---:|---:|---:|
| Programs | 19,139 | 2,763 | 8,525 | 1,166 | 6,685 |
| Wave64 | 16,638 | 1,166 | 7,621 | 1,166 | 6,685 |
| Wave64 with a cross-lane op | 11,214 | 276 | 4,252 | 1 | 6,685 |
| Wave64, op can reach across the 32-lane halves | 7,551 | 253 | 612 | 1 | 6,685 |
| Wave32 with a cross-lane op | 1,746 | 892 | 854 | 0 | 0 |

- Every compute program with a cross-lane op is emitted with `WaveSize` equal
  to its console wave (276 at 64, 892 at 32): no exception.
- No title program is refused and no lesser form is used on a host whose
  largest wave is 64: the wave64-to-32 retry chain only runs when the host's
  largest wave is 32.
- **Graphics stages declare no wave size** (DXIL `WaveSize` is for compute):
  4,252 wave64 pixel, 6,685 wave64 mesh and 854 wave32 pixel programs with
  cross-lane code run at whatever lane count the driver picks. This is the
  one structural place a wave mismatch can arise on this host.

## The groups asked about

**Hair.** The strand programs are not wave64 programs with cross-lane code:
`CS_InterpolateStrands` (80 variants) is wave32 with none; `CS_VisHair`,
`CS_InterpolateStrandStyle`, `PS_HairStrandGBuffer`, `VS_HairStrandGeometry`
are wave64 with only `v_readfirstlane`; `PS_HairStrandVBuffer` has no lane
instruction. Only `CS_HairTileClassify` has real cross-lane code (wave64,
emitted `WaveSize(64)`).

**Lighting, denoising, reflections (ray tracing off).** The compute passes
with cross-lane code all get their console wave size. The lane count is not
pinned for the pixel programs: the light-loop material programs
(`PS_PBR_MATERIAL_*`, `PS_NoDir_PBR_MATERIAL_*`, `PS_STUDIOSHOT_*_LIGHTING`)
are wave32 with real lane reads; `PS_AlphaTransmission_Fire`,
`PS_AlphaTransmission_BackFace_Fire` and `PS_PedAlphaTransmission_Fire` are
wave64 with lane reads and counts that can cross halves.

**Ray-tracing traces.** All 18 are compute and get their console wave size:
`CS_RaytraceVBuffer_*` and the `TraceRays_RayBinning_*` variants are wave32,
`CS_ScreenProbes_TraceRays_BVH_*` and `_Fallback` are wave64.

## Not determined

The lane count RADV actually gives pixel, vertex and mesh programs; whether
the lowering of each wave operation is correct (see `code-audit.md`); which
variants are bound in a scene by hash (the bound list has names only).

## A first test (2026-11-27)

`RADV_PERFTEST=pswave32` (pixel programs in waves of 32) on a seed warmed
under it, three legs: 39.2, 38.7 and 42.6 ms, hair in 2 of 3 starts, the
phone as before. No visible change, so nothing shown either way.
