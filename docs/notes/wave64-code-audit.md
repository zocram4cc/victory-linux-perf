# Wave64 handling: code audit of the fps20-ports tree (2026-11-27)

Tree audited: `fps20-ports/source` (branch `fps20-wip`, HEAD `4f6f9d3`, with uncommitted changes). This was a read-only
audit. Nothing was built or run. Paths below are relative to the tree root unless they start with `porting/`.
Tags: **[V]** means verified by reading the code. **[I]** means inferred and not checked on hardware.

## Verdict

The claim is **partly supported.**

- **The NVIDIA path (32-lane host waves) has deficiencies.** The code itself documents them as `shortcut:`
  comments and "APPROXIMATE" forms. The approximate forms are on by default, and I found two further defects. None of
  this code runs on our RX 7900 XTX: every split-wave path is gated on `host_max_wave() == 32`, and our device
  reports a maximum of 64. [V]
- **Wave64 compute on our host.** Programs whose lane layout matters declare `WaveSize(64)` and run natively, so
  they are exact. [V]
- **What is actually at risk on our host** is a different case. Console **wave32** pixel programs read a lane mask
  as a number (`s_ff1`, `s_bcnt1`, `v_mbcnt`, computed `v_readlane`) through a host ballot of which only the low
  dword is used. If RADV runs pixel shaders in 64-lane waves, lanes 32–63 see the mask of lanes 0–31. This
  affects many programs (see §3 D1). Three other forks have fixes, and we do not have any of them.
- **The `VICTORY_HOST_WAVE` override is documented but does not exist in this tree.** The WIP commit `dc6a8e1`
  deleted `apps/victory/src/translate/host_wave.rs`, and the integration test that includes that file was left in
  place. [V]

## 1. How a wave64 console program runs on the host

- **Probe.** `crates/victory-gpu/src/host.rs:1044-1056` reads `D3D12_OPTIONS1` and passes `WaveLaneCountMax` to
  `set_host_max_wave()`. It sets "RDNA" if the device is AMD with a range of 32..64. There is no runtime override. [V]
  - On our device this gives `host_max_wave = 64`.
  - `host_settings()` returns `[host_max_wave, view_heap_limit, raw_views, host_rdna]` (`lower.rs:1123`), and the
    shader-cache key includes it (`shader_cache.rs:103`).
- **The CLI.**
  - `victory translate` reads only `VICTORY_HOST_MAX_WAVE` (`apps/victory/src/translate.rs:248-252`). [V]
  - The help text in `main.rs:78-80,100` describes a `VICTORY_HOST_WAVE` override with precedence over the old
    name. Nothing reads that variable. [V]
  - `apps/victory/tests/translate_host_wave.rs:1` includes `../src/translate/host_wave.rs`, which `dc6a8e1`
    deleted. That test target cannot compile. [V]
- **Compute: decided at translation time** (`lower.rs:1406-1536`).
  - The console wave is `native.wave64 ? 64 : 32`. [V]
  - **`console_lanes`** is set by any of the following (`lower.rs:1412-1423`) [V]:
    - `ds_swizzle`, `*permute*`, `v_permlane*`, `v_mbcnt*`, `v_readlane`, `v_writelane`, or any DPP;
    - a mask read as data (`reads_mask_as_data`);
    - lane-constant masks;
    - a wave-wide branch.
  - **`WaveSize(n)` is emitted only if `console_lanes`, or if the group is one console wave and uses LDS**
    (`lower.rs:1534-1535`). The DXIL entry property (tag 11) is written at `shader.rs:887-890` and the PSV0 min/max
    at `container.rs:194`. [V]
  - Every other compute program has no wave size, and the driver chooses the width. [V]
  - **`v_readfirstlane` is not in the `console_lanes` list.** [V]
  - **Thread ids.** With `console_lanes`, the thread is relabelled as `WaveReadLaneFirst(flat) & ~(wave-1) + lane`
    (`lower.rs:4696-4740`). On RADV, where a wave holds a consecutive run of flat ids, this is the identity [I].
    Without `console_lanes`, the plain `SV_GroupThreadID` is used. Compute derivatives are never emitted
    (`lower.rs:11444`), so a quad-ordered lane layout does not arise. [V]
  - **On a host with a 32-lane maximum** (`lower.rs:1435-1533`): a wave64 program whose cross-lane code stays
    inside 32-lane halves becomes `wave = 32` with a relabelling through an LDS counter (`halves`, `wave_base`). If
    the code crosses halves, it gets the `exchange` form, which exchanges through LDS dwords plus group barriers.
    Otherwise it stays at 64, the device refuses it, and `emit()` retries (`mod.rs:457-596`). [V]
- **Graphics.**
  - Pixel and vertex programs never declare a wave size (SM 6.6 has none). They use `MaskData::Host`
    (`lower.rs:414-429, 1706-1710`): ballots are over the host wave, and only counts, first-set-bit, `v_mbcnt` and
    `s_cmp_eq/lg` may read a mask (`lower.rs:5452-5462`). Cross-lane operations other than quad operations are
    refused in pixel programs (`cross_lane`, `lower.rs:4676-4689`). [V]
  - Mesh programs have no wave size either. Cross-lane operations are refused, and only min/max reductions are
    allowed (`lower.rs:1999-2007`). [V]
- **On our RADV host:**
  - A wave64 compute program with lane-dependent code gets `WaveSize(64)`. vkd3d-proton should translate that into
    a required subgroup size of 64 [I]. This path is exact.
  - A wave64 compute program without lane-dependent code (417 programs in the colleague's census snapshot) gets
    whatever width RADV chooses. Mesa's documented defaults are wave64 for compute, pixel and geometry on GFX10+
    unless `RADV_PERFTEST=cswave32/pswave32/gewave32` is set [I, not measured in our Proton build]. The launcher
    does not set those flags. [V]
  - Pixel programs run at the driver's width; wave64 is likely [I].
  - None of the following can run here, because each requires `host_max_wave() == 32`: the retry ladder, `widen`
    for 32-lane hosts, `exchange`, `halves`, local waves, `own_bits`, `VICTORY_FIRSTLANE_OWN`, `VICTORY_WAVE_LOCAL`
    and `VICTORY_WAVE_DEEP_SKIPS`. [V]
  - `VICTORY_WAVE_WHY` prints only inside that branch. [V]
  - One exception runs on any host: a refusal for `LANE_MASK` in compute triggers a retry with `widen()`
    (`mod.rs:600-613`). It is on unless `VICTORY_WAVE_WIDE=0`. [V]

## 2. Cross-lane constructs when the host wave is narrower than the console's (32-lane host, wave64 program)

In this section, "exch" means the two host waves of 32 lanes exchange through LDS with two group barriers per
exchange. This requires every lane to reach the code: `all_lanes`.

| Construct | Handling | Exactness |
|---|---|---|
| DPP rows / DPP8, `ds_swizzle`, `v_permlane16/x16` | `halves`: each host wave is one console half (`lower.rs:1428-1446`) | exact [V]; `permlane` with fetch-inactive refused outside an all-lane block (`lower.rs:8873-8879`) |
| Whole-wave sections (WaveActive sum/min/max/and/or/xor, prefix sum) | simulated (`wave.rs`), then `waveActiveOp` per half plus exchange (`lower.rs:2078-2135`) | exact where all lanes run; else **refused** ("a wave reduction where not every lane runs", `lower.rs:2089-2091`) → retry ladder |
| Wave OR steering a loop over its bits | `own_bits.rs` proves each lane's run is the same with its host wave's OR (`lower.rs:2084-2087`) | exact by proof [V, not re-proven] |
| 16-bit float wave sum | balanced tree + exchange (`lower.rs:2240-2320`) | bit-exact; refused if not all lanes run or group not a multiple of 64 |
| f64 wave sum | exchange (`lower.rs:2186-2206`) | sum order differs from the console's tree: **approximated** (rounding) [V] |
| Ballot / mask as number | ballot plus exchange of the low words (`lower.rs:3609-3622`) | exact where all lanes run; else refused → `need()` → retry |
| `s_ff1`, `s_bcnt1`, `v_mbcnt` | from the exchanged ballot, using the console lane | exact (as above) |
| `v_readlane` (computed lane) | lane holder writes LDS, barrier, all read (`lower.rs:8964-8982`) | exact where all lanes run; else refused |
| `v_writelane` | console-lane compare (`lower.rs:8987-9005`) | exact; refused in "local" form |
| `v_readfirstlane` | exchange `any` plus value (`lower.rs:9025-9075`) | exact where all lanes run. Where not, refused, or with `VICTORY_FIRSTLANE_OWN=1` the host wave's own first lane is used (**approximate**). **Defect D3: no exchange is made in `halves`-only programs** |
| `ds_bpermute` | refused across two host waves (`lower.rs:5126-5130`) | refused |
| Wave-wide branches (`execz` / `vccz` skips) | `waveAnyTrue` per half plus exchange (`lower.rs:4410-4451`) | exact; `widen` only for a 64-thread group (`mod.rs:648-659`) |
| SCC of a mask operation (`scc_some`) | per half plus exchange (`lower.rs:4380-4408`) | exact where all lanes run; else refused |
| LDS shared without barriers inside a console wave | a barrier before each LDS access (`split_lds`), or meetings around lane-branch regions (`lds_meetings`, `lower.rs:940-1002`) | exact except `shortcut:` "two writes in one region are taken not to hit the same dword from both halves" (`lower.rs:939`) |
| The title's `s_barrier` | `VICTORY_GROUP_SYNC` (`mod.rs:160-185`). Default: always waits | waits are undefined if some threads never arrive (documented) |
| Derivatives / helper lanes (pixel) | quad reads include helpers (`lower.rs:4896-4958`). A helper lane's first-lane read takes its own value (`lower.rs:4965-4974`, `VICTORY_HELPER_OWN`) | intent-preserving approximation [V]. "Some lane of the quad" is taken as true (`lower.rs:4209-4212`, `shortcut:`) |

**What happens after a refusal** (`mod.rs:444-596`). The program is retried in these forms, in order:

1. without early-outs;
2. without skips;
3. the exact "local" form if the allocation idiom is proven (`wave_local`, `lower.rs:634`);
4. without deep skips;
5. the wave-wide branches (`widen`) form, only with `VICTORY_WAVE_WIDE=1` because it caused device loss in
   runs 121–124;
6. **APPROXIMATE local waves**: each host wave on its own, giving half-tile results (`lower.rs:1062-1090`);
7. **APPROXIMATE "any mask"** local waves, the last resort.

Steps 6 and 7 are **on by default** (`VICTORY_WAVE_LOCAL` unset). If every form fails, the error goes back to
`victory-gpu`, the pipeline has no host shader, and its draws and dispatches are dropped (`pipeline.rs:277`, counted
as skipped). [V]

## 3. Where results can be silently wrong (suspected defects)

**D1. Console wave32 pixel programs on a 64-lane host.** This is the relevant defect for our host.

- **Code** [V]:
  - `mask_dword` returns the ballot's low dword only (`lower.rs:3604-3627`).
  - `s_bcnt1_i32_b32` and `s_ff1_i32_b32` use that dword.
  - `v_mbcnt_lo` under `MaskData::Host` uses the host lane, 0..63 (`lower.rs:10355-10388`).
  - `first_live_lane` uses `waveReadLaneFirst` over all 64 lanes (`lower.rs:4982-5007`).
  - Whole-wave sections reduce over all 64 lanes.
- **Condition:** the console program is wave32 and RADV runs pixel shaders in waves of 64 [I: Mesa's default].
- **Effect:** for a lane in host half 32–63:
  - counts and the first lane come from half 0–31;
  - "no lane left" can be true while that lane is still active;
  - an `s_ff1` → `v_readlane` scalarisation loop reads lane −1 once the lower half is served. This is undefined:
    it either loops to the pixel iteration cap (the pixel is discarded) or reads a stale value;
  - `v_mbcnt` prefixes all equal the lower half's count, so append slots collide;
  - sums and counts disagree (a mean is wrong).
- **On screen:** blocks of 32 pixels inside each 64-pixel wave get a wrong material layer or texture, holes or
  flicker in alpha-tested or transparent surfaces, or broken per-pixel lists.
- **Scale:** the colleague's census snapshot (`wave64-census.tsv`, 19,139 rows, read at 03:0x; the file was being
  regenerated during the audit) lists 904 wave32 pixel programs. Of these, 105 use `s_bcnt1`/`s_ff1` on a mask,
  146 use `v_mbcnt`, and 814 use `v_readlane`/`v_readfirstlane`. They are mostly the
  `PS_Textured_Eight_*` and `PS_MirrorReflectionTextured_Eight_*` families. All 1,166 vertex programs are wave64.
- **Fixes elsewhere:** victory-fork `507c307a4e` (per half), klbf-lab-beta `8b0bed5483` + klbf-lab `b01bce99af`
  (per half, with `VICTORY_PS_HALVES`), Scarlight `588a0e24a8`, Belobug-AMD `ecc4eb668d` (both ballot dwords).

**D2. Compute programs with no wave size whose code assumes "one wave = the console's".** [V for the code; I for
whether the driver ever picks the other width]

- **Code:** `v_readfirstlane` is missing from `console_lanes` (`lower.rs:1412-1421`).
- **Condition:** a wave64 group of 64 broadcasts thread 0's value with `WaveReadLaneFirst`, or LDS is shared within a
  wave without barriers in a group larger than 64 (the `one_wave` test, `lower.rs:1426, 1534`), and the driver picks
  wave32 (for example `RADV_PERFTEST=cswave32` or a future default). The reverse case is a wave32 program running
  in a host wave of 64.
- **Effect:** lanes 32–63 take lane 32's value, or a race in LDS.
- **On screen:** half-tile seams in light culling or tile classification, garbage in a denoiser pass.
- **Scale (census snapshot):** 66 wave64 and 48 wave32 compute programs with no wave size use `v_readfirstlane`.
  Some read uniform values; those are harmless.
- **Fix elsewhere:** VIctory `200e3bde8f` and `73d2c90d6a`.

**D3. 32-lane hosts only: `halves` without `exchange`.**

- **Code:** `v_readfirstlane` is not in `crosses` (`lower.rs:1436-1444`), so `halves` is chosen without an
  exchange, and `lower.rs:9036` then returns the host wave's own first lane. [V]
- **Effect:** in a wave64 program with DPP or swizzle plus a non-uniform first-lane read, lanes 32–63 read lane 32.
- **Fix elsewhere:** VIctory `4a365e3a73`; Scarlight `dd3c08c105`.

**D4. 32-lane hosts only: partial waves in `wave_base`.**

- **Code:** each host wave adds 32 to the counter whatever its lane count (`lower.rs:4256-4262`). [V]
- **Condition:** a group that is not a multiple of 32 (for example 48).
- **Effect:** if the short wave arrives first [I], console thread ids skip 16–31 and run past the group size, so
  indexing goes wrong.
- **Fix elsewhere:** victory-fork `745305ce7b` and `489fd6b8d7`.

**D5. 32-lane hosts only: exchange slots are never initialised.**

- **Code:** `exchange_halves` stores and loads the pair, and nothing zeroes it (`lower.rs:4312-4349`). [V]
- **Condition:** a group with a console wave that has no upper half (`group % 64` in 1..32 and group > 32, for
  example 96).
- **Effect:** the lower half adds uninitialised LDS to sums, ORs and ballots.
- **Fix elsewhere:** VIctory `611184eedc`.

**D6. 32-lane hosts only: approximate forms are on by default.**

- `with_local_waves`, the "APPROXIMATE" and "any mask" forms (`mod.rs:555-595`), give half-wave results with no
  warning beyond `VICTORY_WAVE_WHY`. Examples: a tile maximum or a light count over half a tile.
- VIctory made them opt-in (`f896d26e89`, `VICTORY_WAVE32_APPROX`).

**D7. Graphics on a 32-lane host.**

- `s_cmp_eq/lg_u64` of a ballot against a constant compares both dwords (`lower.rs:5753-5770`), and the high dword
  is always 0, so `== -1` ("all lanes") is never true. [V]
- This does not apply on our host if pixel shaders run at 64.

**Minor.** The section shortcuts `Atom::Quad → true` (`lower.rs:4209`) and the `ds_bpermute` read of a lane that
has left (`lower.rs:5122`) are documented approximations, not specific to wave64.

## 4. Switches (defaults as coded)

| Switch | Default | Effect / trade |
|---|---|---|
| `VICTORY_WAVE_WIDE` | unset: wide form **off** on 32-lane hosts (`mod.rs:514`), but the `LANE_MASK` widen retry **on** for every host (`mod.rs:604`) | `=1`: wave-wide branches with barriers inside branches (exact, but device loss seen); `=0`: both off |
| `VICTORY_WAVE_DEEP_SKIPS` | on (`mod.rs:524`) | exact form "without skips, scalars followed"; `=0` off |
| `VICTORY_WAVE_OWN_BITS` | on (`lower.rs:1115-1118`) | host-wave OR for proven own-bit loops; `=0` refuses as before |
| `VICTORY_WAVE_LOCAL` | on, including the **approximate** forms (`mod.rs:470-476`) | `=0`: no local forms; `=idiom`: exact idiom only |
| `VICTORY_FIRSTLANE_OWN` | off (`lower.rs:9044`) | `=1`: host wave's own first lane where not all lanes run (approximate) |
| `VICTORY_HELPER_OWN` | on (`mod.rs:155-158`) | a pixel helper lane's first-lane read takes its own value; `=0` off |
| `VICTORY_F16_NEAREST` | off (`lower.rs:1098-1105`) | f16 results rounded as the console does (the f16 wave sum always is) |
| `VICTORY_FIRST_HALF` | on, as a retry only (`lower.rs:1662`, `mod.rs:395-402`) | interpolation first halves evaluated as a pair; `=0` refuses |
| `VICTORY_STRICT_JOINS` | off (`lower.rs:12860`) | marks joins read through joins (40 programs stop with it) |
| `VICTORY_GROUP_SYNC` | 2: every barrier waits (`mod.rs:178-185`) | 1: only all-lane barriers wait; 0: none (breaks the exchange forms) |
| `VICTORY_SHADER_OPT` bit 32 | on (`mod.rs:221-224`) | an unguarded first-lane read in halves skips the "which half has a lane" round |
| `VICTORY_WAVE_WHY` | off | diagnostics; only reached on 32-lane hosts |
| `VICTORY_HOST_MAX_WAVE` | CLI only (`translate.rs:250`) | translate as for that width. `VICTORY_HOST_WAVE`: documented, **not implemented** |

All of the above are in the shader-cache key (`shader_cache.rs:57-104`, together with `host_settings()`). [V]

## 5. Other forks (log and diff search from 2026-11-10; commits checked by object id and identifiers in our tree)

| Repo | Commit | Date | Subject / change | In our tree? | Correctness fix we lack? |
|---|---|---|---|---|---|
| victory-fork (PR #318 only) | `507c307a4ed9c3894974f397d7eb99376c1e1701` | 11-25 | Wave32 pixel/vertex ballots, `v_mbcnt`, first lane and `v_readlane` taken per 32-lane half on 64-lane hosts; 242 lines of tests | no (`mask_dword` has no half select) | **Yes, D1** (it does not cover reductions) |
| klbf-lab-beta (`klbf-amd`, `win10-test`) | `8b0bed548341991f7087835b10dca069c55287e5` | 11-24 | Same as above, plus `set_host_pixel_wave`, `VICTORY_PS_HALVES=0`, and a runtime `VICTORY_HOST_WAVE=32` in `host.rs` | no | **Yes, D1**; the runtime override would let us reproduce the NVIDIA path |
| klbf-lab (`win10-test`) | `b01bce99afeaac126b7d7a61c74fde96712a0490` | 11-24 | Whole-wave reductions and prefixes per half for wave32 pixel programs (`wave_reduce`) | no | **Yes, D1** (the reduction half) |
| Belobug-AMD (`main`) | `ecc4eb668db7e61b42e90a7bf06517fe34cc062a` | 11-24 | `mask_ballot` / `host_mask_pair`: wave32 host-mask operations read both ballot dwords | no | **Yes, D1** (other semantics: the whole 64-lane wave) |
| VIctory (PR #189 / #3200) | `200e3bde8f391eff2aa50e7769cb5909fa4c3af8` (+ `73d2c90d6a`) | 11-25 | A non-waterfall, non-uniform `v_readfirstlane` sets `console_lanes` (WaveSize), wave64 only | no | **Yes, D2** |
| VIctory (PR #192) | `4a365e3a736a92af82947dbfb1c6617ca1a4311d` | 11-24 | Keep `WaveSize(64)` when lane-local code also reads the whole wave (first lane, reductions) | no | Yes (D3; 32-lane hosts only) |
| victory-fork (PR #1261) | `489fd6b8d7e15a651a155acc9eeeea5656cb9898` | 11-26 | Thread relabelling from an LDS ticket (partial waves, 16×2 tiles); `VICTORY_THREAD_TICKET` | no | Yes (D4; our relabel also assumes linear waves, true on RADV [I]) |
| Scarlight (PR #599) | `38234626c7c17cb6966a51c2ecba89e0bd1f78a7` | 11-23 | Lane-indexed operations on a wide host wave use the own console wave | no | Possibly (wave32 program on a 64-lane host; not reviewed in depth) |
| Scarlight | `28796f946447f25daf2b9fb27d9e1b7b87ad5202` | 11-25 | SCC of a mask operation is wave-wide, not a lane bit | no (`lower.rs` has no `scc_write` guard) | Unclear: it touches a different code shape |
| triippywhale | `0babbce70fa471d23113059d8fc775a35f22a562` | 11-25 | `s_bcnt1` SCC keeps the narrowed EXEC; width suffix must match the wave | partly (our `structure.rs:1250-1265` differs: only a uniform `own`, no width check) | Width check: likely yes (low impact) |

Also checked: VIctory `5fb2d6d930` / CGBJ `cc1f10ba03` add `WaveOpsIncludeHelperLanes` (SM 6.7), an alternative to
our `VICTORY_HELPER_OWN`. We do not have it. It is a design choice, not a clear fix. NOTES line ~116 skips
#189/#233/#961 as "NVIDIA only", which is correct for the wave64→32 split.

## 6. Tests

- **`emit/wave64_tests.rs`** (9 tests). Five lower for a 32-lane host; when the device accepts the shader, they also
  run on the GPU (RDNA3 takes WaveSize(32)):
  - f16 sum tree, bit-exact;
  - f16 round to nearest;
  - wave OR in a lane branch with `widen`;
  - mask joined with a number (a wave32 console program);
  - SCC skip (a wave32 console program).

  Four use a 64-lane host: `scc_some`, the f16 sum where not every lane runs, the f16 sum with absent lanes, and the
  counted dispatch. Only three of the 32-lane tests are wave64-on-32 tests.
- **`emit/permlane_tests.rs`**:
  - Runs with `host_wave = WaveLaneCountMax` (64 on RADV), so the halves form of `permlane` is never run on the GPU
    here.
  - The mask is the same in both halves (`mask >> (t % 32)`), so a half mix-up is not detected.
- **Others:** `own_bits.rs` has 6 unit tests; `wave.rs` has 8 tests on the section simulator; `mod.rs:2591` has one
  test of a helper lane in a pixel first-lane loop.
- **Untested:**
  - Any **wave32 pixel or vertex program on a 64-lane host** (D1).
  - Unsized compute under the other wave width (D2).
  - Partial groups (48, 96) with `halves` or `exchange` (D4, D5).
  - The APPROXIMATE local-wave forms (their output is never compared with the console's).
  - `VICTORY_FIRSTLANE_OWN`.
  - Barrier meetings around lane-branch LDS regions on the GPU.
  - f64 sum order.
  - `ds_bpermute` from absent lanes.

## Ranked checks and fixes

1. **Measure the widths RADV actually uses.** Write a probe in the existing GPU test harness (`Host` in
   `emit/host.rs`): a compute PSO with no WaveSize and a pixel PSO, each writing `WaveGetLaneCount()` to a UAV.
   Run it under our Proton, with and without `RADV_PERFTEST=cswave32,pswave32`. The observable is 32 or 64 per
   stage. This decides whether D1 and D2 apply today.
2. **Fix D1 in one small patch.** Port victory-fork `507c307a4e` (ballot, first lane, `v_readlane`, `v_mbcnt`) plus
   the `wave_reduce` per-half part of klbf-lab `b01bce99af`, behind a switch (`VICTORY_PS_HALVES`, default on when
   the host maximum is 64).
   - Unit check: a wave32 pixel program doing `s_bcnt1 vcc_lo`, drawn over 64+ pixels. Today lanes 32–63 report the
     lower half's count; with the fix each half reports its own.
   - Game check: A/B captures of scenes using `PS_Textured_Eight_*` with `VICTORY_RETRANSLATE`, looking for
     32-pixel block artifacts and for changes in the pixel-loop-limit discards.
3. **Fix D2.** Add a non-waterfall `v_readfirstlane` of a non-uniform value to `console_lanes` (VIctory
   `200e3bde8f`), so that those compute programs get WaveSize.
   - Observable: the census count of "no WaveSize" programs drops by about 66+48, and the DXIL changes for exactly
     those.
   - Behaviour check: a synthetic 64-thread wave64 broadcast test under `RADV_PERFTEST=cswave32`; lanes 32–63 are
     wrong before the fix and right after.
4. **Restore the `VICTORY_HOST_WAVE` override in `host.rs`** (the klbf-lab-beta `8b0bed5483` hunk), so the
   NVIDIA forms can be reproduced on the 7900 XTX, which accepts WaveSize(32). Also fix or remove the stale
   `translate_host_wave.rs` test and the help text.
   - Observable: the log line `host wave lanes … compute=32`, `WAVE64-RETRY` lines with `VICTORY_WAVE_WHY=1`, and
     `cargo xwin test -p victory` compiles.
5. **For 32-lane forms only:** fix D4 and D5 (initialise the pair slots or reduce over the halves present; use a
   ticket in `wave_base`), and make the APPROXIMATE local forms opt-in (D6).
   - Observable: new GPU tests with groups of 48 and 96 in `wave64_tests.rs` that fail today.
