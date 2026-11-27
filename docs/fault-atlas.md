# Fault atlas

A map from what a fault looks like on screen to what is known to cause it. Find the
symptom below; each entry names the mechanisms seen behind it, the switch that tests each
one without a code change, the commit that fixed it (with the fork or PR it came from, as
the commit message cites it) and how well the fix is confirmed. The first part holds what
past fixes taught about the title, so a new fault can start from there. Measurements and
the faults found on the RX 7900 XTX are in [findings.md](findings.md); the Windows-side
residency work is in [bug-journal.md](bug-journal.md); the crates and the shader path are
in [architecture.md](architecture.md).

## What counts as evidence

Every fix below carries one of these labels, strongest first:

1. **run**: a game run judged against how the title should look.
2. **definition**: a primary definition (a header, a spec) or a measurement on real hardware.
3. **independent**: forks that reached the same answer separately, with their lineage checked.
4. **test**: a unit test written with the port. It guards against regressions; it does not show the fix is right.
5. **none**: copies agreeing. A line or patch inherited or copied between forks, or copied from this repository, counts once, and as nothing.

A review of a port shows it matches its source and fits this code, not that the source is
right. A test that compares a translated shader with "the console's expected answer" uses
this project's model of the console, unless the expected values were taken on hardware.
When a fix appears in several forks, the atlas names it once, with its origin.

## What the title assumes

**The card reads memory when the draw runs, not earlier.** On the console the CPU and the
card share memory, and the title writes constant buffers and textures up to the moment
the card uses them. Any speed-up that takes the bytes earlier can read a stale copy.
Card steps sent a submission at once, and the title wrote some constant buffers again
before it landed: skinned characters threw vertices (`1daf742`, which learns those pages
and waits there). A list that uploaded a texture while the copy queue was still taking it
read a half-refreshed staging buffer (`362f04e`). A view copied from a host-written
texture was skipped after an upload from title memory earlier in the same list
(`7544a37`). A constant buffer written by a copy earlier in the list was filled on the CPU
at submission (`00bf4e4`). When a fault goes away under `--strict`, look here first.

**Who wrote the memory last, the CPU or the card, decides what a view must read.** There is
no single owner record; it is tracked case by case. Placed views over render targets,
over textures the host copied or resolved into, and over unordered-access writes are
found by alias lookups (`e84d2d0`, `7a2b82a`, `7544a37`). A texture whose contents go back
to title memory must be registered again when its memory is reused (`b6bbcbd`). A placed
texture whose mip span runs into its neighbour reads the neighbour's bytes unless cut
there (`9481860`). A new path that reads title memory for a texture must ask whether the
card wrote it since.

**An address is not an identity.** The title releases heaps, textures and buffers and makes
new ones at the same address. Dynamic descriptor slots were given back by heap address,
so an old list freed a new heap's slots (`2ae6fef`, now keyed by a heap identity). The
texture behind a placed view could be released between lookup and copy (`609ed4b`, now
held). A buffer remade at the same place dropped a host copy still in flight (`5312d45`).
A texture the title streams into the same address later kept its first upload for good
(`288d69a`). Key host objects by an identity, and hold what you found while you use it.

**Some work is done once and never again.** The title bakes character skin once while a
scene loads (`eaeaf6c`), and some one-off dispatches reset or clear a buffer when a scene
changes (`1d52b86`). A dropped draw is drawn again next frame; a dropped one-off is not,
and the whole scene stays wrong. Such work must be delayed or replayed, never dropped.
The delay must still be bounded: `1d52b86`'s unbounded wait froze the title (`5752ad3`).

**Console waves are 64 lanes, the host's 32.** A decision the console makes once for a
wave must stay one decision when the wave is split in two. Examples on main: a wave64
program that uses LDS inside a lane branch, translated with the two host waves meeting
only where the branch is entered and left (`4d745d5`, marked approximate); a sampler
chosen by a uniform branch (`d903b12`); wave-sum shapes misread from 16-bit constants
(`1bc8ec7`). Running a translated shader on the host card against known answers finds
these; mind the evidence note above on where the answers come from.

**Host-side failures are silent.** NVIDIA does not store to some formats through a typed
unordered view, and making one removed the device (`eceafac`). Raw views off 16 bytes were
null views that read zeros, so trees and props vanished for single frames (`b5bc76f`; a
test on the host card, `14761c1`, now shows NVIDIA reads them correctly). Under Wine the
system shader compiler dropped root signatures without an error (`e667ac1`, `0a69a6b`).
The driver trimmed its shader cache and the title recompiled while it drew (`46555ae`). A
failed texture conversion left a half-new buffer marked as converted (`df0e9e2`). Make
each such case visible: a log line, a counter, or a refusal that names the cause.

**Forks that copied the same line are not evidence; forks that worked it out separately
are.** The table gave format 191 four bytes a texel; DXGI defines it as A4B4G4R4_UNORM,
two bytes (`b588802`). Several forks carried the same wrong line and comment, so their
agreement showed nothing. Before counting a fork as confirmation, trace where its line
came from (`git log -S` or blame in that fork, or text and comment identical to ours).
Check tables against primary definitions and cover them with tests.

## Symptoms

`--strict` means `scripts/play-linux.sh --strict`: `--safe` plus every speed-up off that
changes when title memory is read or lets the title run on without waiting (relay, card
steps, upload ahead, texture no-wait, thin lists, skipped re-reads; `e1dbfef`). A switch
given in the environment stays as given, so one speed-up at a time can be turned back on
(for example `VICTORY_CARD_STEPS=2 scripts/play-linux.sh --strict`). A fault gone under
`--strict` is a timing fault. Launcher defaults are set in `scripts/play-linux.sh`; code
defaults are where the switch is read.

### One-frame geometry spikes on skinned characters

Vertices thrown far off for a single frame. **Open.**

- Card steps' fills read before the title's late writes: `1daf742` (avatarmanz PR #434), *test*. `VICTORY_CARD_STEPS=0` (launcher default 2, code default 0), or `VICTORY_CARD_STEPS_LEARN=0` to drop the late-write check. An A/B over the 600 s intro (runs 20261127-121700 with card steps and the late-write check, 20261127-122838 with card steps off) showed about 8 and 7 spikes by frame review, most of them in the crowd scene at 318-327 s: not the sole cause (*run*).
- Tables kept across a list cut at a memory wait, pointing at an old bone palette: `84454e1` (LooperSalty/AnyXboxX ad9b80d), behind `VICTORY_CUT_FORGETS_TABLES=1` (off). *run*: spikes still showed with it on, at 523.8 and 524.9 s in the prison-window scene (run 20261127-024858).
- Series S device only, per a report from Windows on NVIDIA: `VICTORY_CONSOLE=series-x` (`f3049be`). A headless Series X opening ran to its end at a 32.7 GB title peak, against about 23-26 GB on Series S (run 20261127-020738).
- Spikes cluster in a crowd scene (a queue outside a club, then dancers) at about 318-327 s of `--bench intro --new-game`; that scene is the test for any fix here. The frame recorder cannot catch them: a capture stands the title still for seconds a frame and its clock runs on (run 20261127-125825).

### Mosaic, checkerboard or missing skin

- Two lists' descriptors written over each other after a heap was remade at the same address: `2ae6fef` (trustno1 47747d6), *test*. No switch.
- A skin bake draw ran before its source textures were on the card: `eaeaf6c` (fxceless PR #1264, #1422). `VICTORY_BAKE_SOURCES=0` only counts (code default on). *test*: the run after it showed no fault, but neither did its baseline.
- BC7 dictionary entry 0 read two bytes short (wrong 4x4 blocks): `4132103` (xcl-dev/xcl 7881dbc9), *test*; the run after it was clean. BC7 rotation read for modes other than 4 and 5: `12d6674` (DoubleGhosty e7a40a4), *test*.
- Skin black with mip levels on demand: `0e3995c`, *run*. `VICTORY_LEVELS_ON_DEMAND=1` brings it back (launcher default 0).
- Black faces for 30 frames on partly mapped textures: `e3ecd00` (DoubleGhosty c34104617), *run*.
- Blotched skin and walls from NaNs spreading through passes: `d45675d` (willy build), *run* in that build. `VICTORY_NAN_GUARD=0` turns the guard off (code default on).
- Boxes of an older picture on body and shirt, a decoration atlas the host copies into: `7a2b82a` (Legendary 65498fd, 54ebfc1), *test*. `VICTORY_ALIAS_HOST_WRITTEN=0`, `VICTORY_UAV_RECHECK=0` (both code default on).

### A face or clothing that differs between two runs of the same scene

- A texture reused at the same address kept its first upload, and looks compared three windows only: `288d69a` (Legendary f085a93, c2ab4b5), *test*. `VICTORY_HEAP_RECHECK_FOREVER=0`, `VICTORY_TEXTURE_FULL_RECHECK=0` (both code default on).
- A view of a host-written texture read the CPU's bytes after an upload in the same list: `7544a37` (fxceless 19a45d1, Vice-Tay 7993c1b), *test*. No switch.
- The texture behind a placed view released while copied: `609ed4b` (trustno1 PR #25, fxceless PR #1336), *test*. No switch.
- Noisy faces from a neighbour's bytes in packed mip levels: `9481860` (fxceless #1401), behind `VICTORY_PACKED_LEVELS=1` (code default off), not judged in a run.
- Timing: compare two `--strict` runs first; if they agree, bisect with single switches.

### Rainbow foliage

Plants rainbow-coloured right after they are drawn, fading over about 0.4 s.

- `CS_ScreenProbes_ActivateRays` left out with the ray-tracing structure builder off: `0d6bf3a`, *run* (run 20261127-000808). `VICTORY_ACTIVATE_RAYS=1` is the launcher default; `=0` brings the fault back.

### Lights that light only patches

- Indirect light is a stand-in (`VICTORY_AMBIENT`, launcher default 1.2). It read the stencil plane as depth after a variant of the resolve (flat floors): `081ef1d` (trustno1 d226814, bda6e59), *test*.
- Stale estimate after a camera cut, the probes' old memory as hard pale blocks: `103b173`, `VICTORY_AMBIENT_CUT` (code default 2, 0 off). Not judged in a run.
- Test by unsetting `VICTORY_AMBIENT` to see the title's own path; `--ray-tracing` turns on the title's traced light.

### Sky reflected in eyes

Eyes glowing blue-white, blue blotches on floors indoors. **Open** (both switches off by default).

- Without ray-tracing structures every reflection ray misses and takes the sky's light: `VICTORY_AMBIENT_SPECULAR=<share>[,<sky>]` (`e0b22bf`, trustno1 2db59d4), *test*.
- `VICTORY_AMBIENT_REFLECT` caps the reflection history (`a1e91ed`, `50ceb20`, Neptune_VI's pass); it did not engage in the opening (run 20261127-013149).

### Z-fighting of overlays

- An overlay one bias step in front of the mesh under it; the two pipelines round depth differently: `1c9bb8d`, `bdd64d4`, *run* (run 20261127-003552). `VICTORY_DEPTH_BIAS_SCALE` (launcher default 8, code default 1).

### Props or large polygons flashing

- Trees and props vanish for single frames: the raw twin of a view off 16 bytes was null: `b5bc76f` (avatarmanz PR #437), *test*; NVIDIA's reads checked on the host card in `14761c1`, *definition*. `VICTORY_TWIN_EXACT=0` gives the null view back (code default on).
- Flat half-transparent rectangles around smoke and sprites: the neutral stand-in texture was half opaque: `c341b22`, `VICTORY_NEUTRAL_ALPHA=128` for the old texel. Not judged in a run.
- Draws dropped for want of dynamic descriptor slots on NVIDIA's capped view heap: `4f04dbd` (trustno1). `VICTORY_DYNAMIC_SLOTS=524288` drops fewer, but leaves fewer of the title's slots in place (`46555ae`); not a launcher default.
- Typed unordered views in a format NVIDIA cannot store to: `eceafac` (DoubleGhosty 32aceeb), *test*.

### Pop-in and memory growth

- New textures got no first look while retries were due: `933d6f7` (Legendary 70e4cba), *test*. `VICTORY_FAIR_DUE=0` (code default on).
- A full `VICTORY_TEXTURE_BUDGET_MB` refused new copies instead of letting idle ones go: `a95f4cd` (Belobug PR #512), *test*. `VICTORY_TEXTURE_BUDGET_SWEEP=0`.
- The sweep let copies go by age, not those whose title memory was gone: `2d3ebd7` (zeex64 502e905), *test*. `VICTORY_SWEEP_GONE_FIRST=0`.
- Idle staging buffers and read buffers kept for minutes: `73ca0dc` (Ortomanic linux-mix cf3f615), `VICTORY_STAGING_IDLE_MS`, `VICTORY_READ_KEEP_MB`, both unset by default.
- Mip levels on demand used more memory and the road popped in: `0e3995c`, *run*.
- The title's own short texture waits: start black for about 5 s, faces pop in soon after; `--texture-waits` gives longer ones (`f15f8bf`, *run*).

### A freeze about 30 s into the world, then a crash

The render loop stops; some 35 s later the title's watchdog ends it with a write to 0.

- `1d52b86` (fxceless PR #1312) made a dispatch wait for its compute pipeline without a bound; under Proton on NVIDIA that held the title's thread. `5752ad3` turned the wait off, *run* (runs 20261127-005324 and -005721 froze, -010017 played on). `VICTORY_PIPELINE_COMPUTE_WAIT=1` brings the wait back (code default off).
- An exe older than its sources reran the fixed fault: the launcher warns (`779471f`).

### Black screen on a fresh clone

- Shaders and pipelines made on demand on a cold cache, for minutes: `e0d9806` translates them before the first frame, once per build and switch set. *run*. `VICTORY_PREBUILD=0` skips it.

### Very low frame rate

- Another program holds GPU memory the title needs (it fills about 15 GB of a 16 GB card): the launcher warns (`2a1dbd7`).
- NVIDIA's 1 GB shader disk cache trimmed the title's pipelines; 1,100-1,300 recompiled in the first 20-30 s, the first car scene black: `46555ae`, *run*. The launcher sets `__GL_SHADER_DISK_CACHE_SIZE` to 12 GB and `__GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1`.
- Pipelines warmed by another exe build are not reused: `a6056e6`.
- Less than 20 GB of RAM free: the launcher notes it (`e0d9806`).

## How to add an entry

1. Put the symptom as the reader sees it, not as the cause.
2. Per mechanism: one line with the fixing commit, its source as the commit cites it, the switch and its default as read in the code or launcher.
3. Label the fix by the evidence levels above; name a run by its id.
4. Mark the symptom open until a run confirms the fix.
5. A new lesson about the title goes in "What the title assumes", with its commits.
