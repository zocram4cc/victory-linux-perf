# Running on Linux through Proton

The layer is Windows code. It also runs on Linux under Wine: Proton (Wine plus
vkd3d-proton, Direct3D 12 on Vulkan) on an AMD card with Mesa's RADV driver.
This page says what that needs, how to build and run, and what goes wrong
there, with the cause and the switch for each fix.

**Measured on one machine only**: CachyOS (kernel 7.3), Radeon RX 9070 16 GB,
Mesa RADV 26.3.1, Proton Experimental with the patched wineserver (below),
2026-11-25. The frame times were confirmed on mainline Wine 11.17 built from
source: the same, so Proton itself is not a factor. Nothing here has been
tried on another Linux machine, another card or another driver.

The fixes below for what only Wine shows are active only under Wine (ntdll
exports `wine_get_version`) or behind a switch that is off by default: on
Windows they change nothing. They are untested on Windows and on other cards.

## Requirements

- **Proton Experimental** (or a Wine of the 11 series with vkd3d-proton). Not
  GE-Proton11-12: its vkd3d-proton turns some translated shaders into invalid
  SPIR-V and RADV 26.2 dies on a null pointer at the first world pipeline (exit
  code 3).
- **For speed, a wineserver with the view tree** (next section). Stock works,
  about three times slower.
- **Microsoft's `d3dcompiler_47.dll`, build 10.0.26100**, in the prefix's
  `system32`, with the override `WINEDLLOVERRIDES=d3dcompiler_47=n`. The layer
  compiles its own helper shaders at run time with `D3DCompile`, some as
  `cs_5_1`. Wine's built-in `d3dcompiler_47` refuses some of them (for example
  `Method 'InterlockedMax' is not defined`), and so does the older Microsoft build
  winetricks installs (6.3.9600). Without them the world's indirect draws are
  dropped and the screen is mostly black.
- **cargo-xwin** and a Rust toolchain with the `x86_64-pc-windows-msvc` target,
  for the Windows build on Linux.
- A launcher that runs a Windows program with Proton outside Steam, such as
  umu-run (`GAMEID=0`, `PROTONPATH=<the Proton>`, `WINEPREFIX=<a prefix of its
  own>`).

## Building

    cargo xwin build --release -p victory --target x86_64-pc-windows-msvc

The binary is `target/x86_64-pc-windows-msvc/release/victory.exe`. A
diagnostics build is `--profile diag --features diagnostics`, under
`target/x86_64-pc-windows-msvc/diag/`.

The shader crate's tests run natively (`cargo test -p victory-shader --lib`).
`victory-gpu` and `victory-hle` are Windows-only: build their tests with
`cargo xwin test --no-run` and run the test executables under Proton.

## The patched wineserver

wineserver keeps a process's memory views in a linked list and walks it on
every view call: `map_view` checks the address against every view and
`unmap_view` searches from the head. The title keeps about 125,000 views and
the layer remaps 1,000 to 4,000 single 64 KiB pages a frame, and ntdll holds
its process-wide lock across each server call, so every `VirtualProtect`,
`VirtualQuery` and `GetWriteWatch` of every thread waits behind it. One page
remapped cost 23.6 us with 100 views alive and 552 us with 125,000.

The fix is a published wineserver patch: Wine bug 60868, CachyOS/wine-cachyos
pull request 50. It keeps an rb-tree of views by base address beside the list,
so each lookup is O(log n), and touches only `server/` (`file.h`, `mapping.c`,
`process.c`, `process.h`). Only `wineserver` differs from stock.

| views alive | stock, one page remapped | patched |
|---:|---:|---:|
| 10,000 | 113.6 us | 11.5 us |
| 50,000 | 249.8 us | 12.0 us |
| 125,000 | 520.9 us | 12.2 us |

To use it with Proton, build `server/wineserver` from Wine's source at exactly
the commit that Proton release's `wine` submodule names, with the patch applied,
and put it in a copy of that Proton (`files/bin/wineserver`). It must match
that Proton's own `ntdll.so`: the protocol version is checked at start, so
never put it into another Proton release, and rebuild it when Proton updates.
Give the copy a prefix of its own. A world frame of 2.9 to 3.2 s on stock fell
to about 1 s in the first runs, before the fixes below.

## Running

    WINEPREFIX=<prefix> PROTONPATH=<Proton> GAMEID=0 WINEDLLOVERRIDES=d3dcompiler_47=n \
      umu-run victory.exe run 'Z:\path\to\game_scarlett_master.exe' --execute

Paths given to the layer are Windows paths (`Z:` is the Linux root). Set
`VICTORY_GAME_DIR`, `VICTORY_CACHE_DIR` and `VICTORY_DATA_DIR` (or
`victory.toml`) to Windows paths too, and `VKD3D_SHADER_CACHE_PATH`, since
vkd3d-proton otherwise writes its cache into the working directory. Proton
does not pass a console program's output on: run it under `cmd /c` with the
output redirected to a file.

The launch settings of `scripts/play.ps1` apply on Linux as on Windows.
`VICTORY_KEYS` presses the pad's A on the start screen in every build, so an
unattended run gets past it. On a build without the single-latch fix (see
"Every frame waits 500 ms") add `VICTORY_PIPELINE_WAIT_MS=0`.

## Problems seen under Proton

Each entry gives what was seen, the cause, and what to do about it.

### Every frame waits 500 ms for six compute pipelines

Seen: in the world 285 ms a frame; 55 ms with `VICTORY_PIPELINE_WAIT_MS=0`
(RX 9070, the same scene).

Cause: six compute pipelines are never made under vkd3d-proton (shaders
17829ac71aab4e8d, 32ed072b9f845048, afeea48bbb9913ad, 559d9f0184e15fe3,
a1b20bc47e3c9597, e8c962358d862e3f). Its DXIL to SPIR-V converter recurses
without end (`CFGNode::find_common_dominator`,
`recompute_immediate_dominator`) on a loop with two back edges, a `continue`.
On Windows a fault in pipeline creation ends the process; under Wine a stack
overflow in the host call ends only the worker thread, without a word, and the
pipeline stays "being made". Every frame that asks for one of them then spends
the whole pipeline-wait budget (`VICTORY_PIPELINE_WAIT_MS`, 500 ms a frame)
waiting for it.

The fix, on under Wine (`VICTORY_ONE_LATCH`): DXC writes loops with one latch,
so the translation gives every loop one (the blocks that branched back branch
to a new block that branches back alone). With it the six convert, nothing
stays "being made", and the default budget costs 0.0 to 0.4 ms a frame
(median frame 53 to 60 ms in the same scenes). On a build without it,
`VICTORY_PIPELINE_WAIT_MS=0` avoids the wait: draws whose pipeline is not
ready are dropped instead.

### Characters without heads (or heads without skin)

Cause: a compute dispatch the title issues once (a skinning or set-up pass)
was dropped because its pipeline did not exist yet; the frame went on without
what it writes, and nothing runs it again.

Switch: `VICTORY_COMPUTE_WAIT`: compute dispatches wait for their pipeline.

### Faces still, eyes glowing, or faces torn

The face network (65 `compute_graph_ops` programs, all wave64, tensors in
R16_UINT and R16G16B16A16_UINT views) wrote zeros: a descriptor table's copy
into dynamic slots never got its integer-kind word in the table by slot, so
its 16-bit integers were read as floats, then 0. The eyes glow because the rig
drives the eye meshes too. Switch: `VICTORY_TABLE_SLOT_WORDS`.

With that, faces moved but were torn. On RADV through vkd3d-proton a typed
(texel buffer) load after a group barrier can return a value from before
another wave's store that came before the barrier; a raw (storage buffer)
load sees the store. Found in the face controls' interpreter (0aa7934094794dbf)
by replaying it alone: one dispatch per stage matched a CPU model at every
stage, one dispatch with the barrier did not. In compute programs that store to
UAVs and use `s_barrier`, 16-bit integer typed loads read the raw view.
Switch: `VICTORY_RAW_TYPED_LOADS`.

### GPU page faults, the device lost

- Inside a texture made a moment ago: a view of a fresh texture is written into
  its heap slot only when no submission is in flight. Switch:
  `VICTORY_PUBLISH_SAFE`.
- At addresses near 0: passes handed address 0 read zeros there, and null
  buffer views read zeros. Switches: `VICTORY_MEMORY_RUN_FIRST`,
  `VICTORY_NULL_BUFFER_ZEROS`.

### Everything sinks in water

Seen: past the prologue things fall to the bottom of the water, and the first
mission with the boat fails at once.

Cause: the ocean's compute passes write a 256x256 height map (R32_FLOAT) and a
displacement map (R32G32_FLOAT), and the title's CPU physics reads those
textures' memory. The texture read-back (`crates/victory-gpu/src/texback.rs`,
`VICTORY_TEXTURE_BACK`, on; `=0` off) copies them into title memory after the
list that wrote them has run. Under Wine it is hardened:

- a mark behind the copies (`VICTORY_FETCH_MARK`, `=0` off): the card can
  finish a list's fence on a lost device before the loss is reported, and the
  read-back buffer then holds zeros; a landing without its mark writes nothing;
- only the 64-byte pieces that changed are written, not the whole texture over
  a title write that landed meanwhile;
- only textures of one subresource, of the title's format, in a layout the
  formats crate can tile, are taken (the other mips' card contents would go
  over the title's texels);
- a log line per texture taken and every 10 s the counts (recorded, landed,
  unchanged, stale, unmarked, failed, bytes).

### A read fails with 0x800703e6 mid-run

Seen: a DirectStorage request failed with `ERROR_NOACCESS`, mid-run, and the
title stopped at its own breakpoint. Every such failure was a raw request (a
file read by the kernel straight into title memory, 1 to 45 KB).

Cause: a race. The kernel's write does not fault into the layer's guard
handler, so the guarded blocks are opened first; a refresh on another thread
could guard them again before the read landed. The layer opens them for the
length of the read (`kernel_writing`: no refresh guards them again until the
read is done, as upstream's lease), for DirectStorage, `ReadFile` and `fread`.
A related failure, data written into pages the title had remapped, came from
a queue that submitted itself at half capacity before the title had filled in
its page lists; here a queue waits for the title's Submit
(`VICTORY_DSTORAGE_HALF=1` brings the half-capacity submit back).

### Exit code 3 at the first world pipeline

GE-Proton11-12's vkd3d-proton makes invalid SPIR-V of some translated shaders.
Use Proton Experimental.

## Not known

- Long sessions on Linux with this tree have not been tested.
- vkd3d-proton's converter may fail the same way on programs not yet seen.
- The patched wineserver goes stale whenever Proton updates: rebuild it.
