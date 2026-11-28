# Handoff: what still gates the big families (lane-mask / wave-section area)

Written 28 Nov ~08:30, translator at 1,447 of 2,680. The three big families: `0x2996b5a0` 478/525, `0x1cd48770` 306/330, `0x2996b900` 192/246. Their 125 untranslated programs (harness on just them, with the game's host settings) are blocked by:

| blocker | programs | area | example |
|---|---|---|---|
| a whole-wave section step the simulation does not follow | 48 | `emit/wave.rs` (section simulator) | `218_0000000f00382b00` at 0x7d0 |
| a lane mask whose lanes depend on its own value | 36 | `structure.rs` / lane masks | `218_0000000f002b9300` |
| a descriptor's address or format read as data | 25 | mostly waterfall loops over a per-lane heap index (see `p4-shortcuts.md`, "The biggest wall") | `218_0000000f00416300` |
| a lane mask holding lanes elsewhere read as data | 8 | lane masks | `218_0000000f003af100` |
| a compare with DPP | 3 | lowering | `218_0000000f00a3a500` |
| others | 5 | | |

**1. The whole-wave step** (`218_0000000f00382b00`, ~0x780). Inside a section the compiler narrows EXEC to one lane (`s_mov_b64 exec, 1`) and writes a reduction's result to LDS from SCC-derived values:

```
s_and_b64 s[20:21], vcc, exec
s_mov_b64 exec, 1
s_mov_b32 s17, src_scc
s_and_b64 s[18:19], s[18:19], exec
s_mul_i32 s18, 2, src_scc
s_mov_b32 s19, src_scc
s_or_b32 s17, s17, s18
v_mov_b32 v6, s17
ds_write_b32 v5, v6
…
s_mov_b64 exec, -1
s_barrier
```

The simulator leaves the values these steps write unknown: `src_scc` moves and arithmetic, and the single-lane EXEC.

**2. The waterfall loops** (`218_0000000f00416300` ~0x306c):
- `v_readfirstlane` of a per-lane heap index;
- `v_cmpx_eq`;
- an 8-dword descriptor load at index × 32;
- `image_load`;
- `s_andn2_wrexec_b32` + `s_cbranch_scc1`.

`resource::Tracer::waterfall` now accepts the wrexec retry and scalar accesses (step 46). It still stops because `structure.rs` gives no lane mask for the EXEC the loop's `v_cmpx` / wrexec write: select checks 1 and 3, `VICTORY_WHY_WATERFALL=1`. With those masks the loop is proven and the join's back-edge value is dead by construction.

**How to check a fix:**
1. `cargo test --release -p victory-shader` (216 tests).
2. The harness (it now uses the game's host settings by default: RDNA, waves 32/64).
3. `scripts/diff-tests.sh` (29 compute cases; it needs the game closed).

A new differential case for each new shape is welcome: the console compiler emits these from HLSL wave reductions (`WaveActiveSum` etc.) and from `NonUniformResourceIndex` indexing.
