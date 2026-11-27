# Running on Linux through Proton

The layer is Windows code. It also runs on Linux under Wine: Proton (Wine plus
vkd3d-proton, Direct3D 12 on Vulkan) on an AMD card with Mesa's RADV driver.
This page says what that needs, how to build and run, and what goes wrong
there, with the cause and the switch for each fix. A step-by-step install
for Ubuntu-, Arch- and Fedora-based distributions: [linux-howto.md](linux-howto.md).

**Measured on one machine only**: CachyOS (kernel 7.3), Radeon RX 9070 16 GB,
Mesa RADV 26.3.1, Proton Experimental 11.0-20261112 (its vkd3d-proton is
3.2.0, build 2576c0b4) with the patched wineserver (below), 2026-11-25/26;
some results also with vkd3d-proton master (71cae09, 2026-11-25) built from
source and placed beside `victory.exe` (`WINEDLLOVERRIDES` with
`d3d12,d3d12core=n`). Nothing here has been tried on another Linux machine,
another card or another driver.

The changes below are on by default only under Wine (ntdll exports
`wine_get_version`), each with a switch: `0` turns it off, `1` turns it on
on any host (`VICTORY_PUBLISH_SAFE` and the null-page switches,
`VICTORY_NULL_BUFFER_ZEROS` and `VICTORY_NULL_ROOTS`, only turn off). That
is a choice, not a finding: the compute dispatch wait is for a cause that is
not Wine-specific. None of them has been tried on Windows or on other cards.
The null-blob pipeline library and the faces fix (the d16 accesses through
tables) are the same on every host.

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
unattended run gets past it (it does nothing unless set). With
`VICTORY_ONE_LATCH=0` on a vkd3d-proton older than 71cae09 (below), add
`VICTORY_PIPELINE_WAIT_MS=0`.

## Problems seen under Proton

Each entry gives what was seen, the cause, and what to do about it.

### Pipelines were never kept between sessions

Seen: "no host pipeline library: pipelines are not kept" at every start; no
early list was written, so every session made its pipelines from nothing.

Cause: the layer asked for an empty pipeline library with an empty Rust slice,
which has a non-null address. vkd3d-proton returns `E_INVALIDARG` for a
non-null blob of length 0 (`libs/vkd3d/cache.c`,
`d3d12_pipeline_library_init`). The empty library is now asked for with a null
blob (as Marvisak/VIctory#347 by AugustoZFJ, and fxceless/victory-fork#885):
the first start logs "no host pipeline library yet; starting an empty one",
the next one "host pipeline library loaded" and starts the listed pipelines
early. This is the same on every host. Microsoft's page says an empty blob
gives an empty library, not what the pointer may be; it has not been tried on
Windows.

### Every frame waits 500 ms for six compute pipelines (vkd3d-proton 3.2.0)

Seen, with the vkd3d-proton of Proton Experimental 11.0-20261112 (3.2.0,
dxil-spirv 114e664): in the world 285 ms a frame; 55 ms with
`VICTORY_PIPELINE_WAIT_MS=0` (the same scene).

Cause: six compute pipelines are never made (shaders 17829ac71aab4e8d,
32ed072b9f845048, afeea48bbb9913ad, 559d9f0184e15fe3, a1b20bc47e3c9597,
e8c962358d862e3f; each hangs when built alone with `victory translate
--pipelines`). From reading dxil-spirv's source, not from a debugger: a
pre-pass that runs before its structurizer mishandles a loop with two back
edges (a `continue`) and recurses. The pipeline stays "being made", and every
frame that asks for one of them spends the whole pipeline-wait budget
(`VICTORY_PIPELINE_WAIT_MS`, 500 ms a frame) on it. dxil-spirv fixed this on
2026-11-24/25 (694cc67, "Only run fixup passes after structurizer has run",
and 3bc8276); with vkd3d-proton 71cae09, which has them, all six are made.

Stopgap, on under Wine (`VICTORY_ONE_LATCH`): DXC writes loops with one latch,
so the translation gives every loop one (the blocks that branched back branch
to a new block that branches back alone). With it the six are made on 3.2.0
too, and a world frame takes 53-60 ms here, the same as with
`VICTORY_PIPELINE_WAIT_MS=0`. It is to be removed once Proton ships a
vkd3d-proton with the dxil-spirv fix. Without it, `VICTORY_PIPELINE_WAIT_MS=0`
avoids the wait for draws: those whose pipeline is not ready are dropped
instead. Compute dispatches still wait for the six once (below): one frame of
10 s at the world's start, then 54-60 ms frames (one cold run on 3.2.0).

### Heads without skin (a floating cap, eyes and teeth)

Seen with a cold driver shader cache (a fresh `MESA_SHADER_CACHE_DIR`), on
vkd3d-proton 3.2.0 and on 71cae09 alike; with a warm cache the heads had
skin.

Cause: the head blend-shape map's compute pipeline (61e54e90adccaf6a) was not
made yet at its one dispatch (made 1 to 21 s later, behind hundreds of
graphics pipelines). A dispatch whose pipeline is not ready is dropped, and
the title issues this one once, so its output is missing for the session. A
timing race, not a Wine one: the fork's issue #75 saw heads without skin on
Windows.

Fix, on under Wine (`VICTORY_COMPUTE_WAIT`: `0` off, `1` on any host): a
compute dispatch whose pipeline is not made yet waits for it, outside the
frame's 500 ms budget, and its pipeline goes before the requested graphics
pipelines (after the passes over the screen). The waits are bounded: a frame's
compute waits end at most 10 s after its first one began, and one pipeline is
waited for only until a worker has had it for 10 s; past that a dispatch is
dropped as before (logged once per pipeline). Without the bound, the first
world frame on a cold cache waited for some 670 compute pipelines one after
another, 33 s, and the title's own watchdog (about 30 s) ended the run. With
it, in two cold starts (one on 3.2.0 with the single latch off, one on
71cae09), and four more with earlier forms of the bounds, the heads had skin,
the head map was made within 15 ms of its dispatch, and the title ran on. With
`VICTORY_COMPUTE_WAIT=0` on this tree the heads had no skin again.

### Faces still, eyes glowing, or faces torn

Seen: no face moved, and the eyes glowed (the rig drives the eye meshes too).
Once faces moved, some were torn: mouths agape, teeth under the nose.

Cause of the still faces: the face network's programs (`compute_graph_ops`)
keep 16-bit integers in typed views of R16_UINT and R16G16B16A16_UINT in
descriptor tables. Their `_d16` accesses took the data as halves of a float
view, which gave zeros, a face at rest. The translation could ask the host's
table of strides for the integer kind of a view picked from the heap (upstream
9a6c829), not of one in a table, and the host wrote that word only for the
title's own slots. Fix, on every host and without a switch (PR #114): the d16
accesses ask for the kind of a view in a table too, and the host writes the
word for the dynamic slots of a table made afresh. It changes the
translations: `TRANSLATOR_VERSION` is 2. Seen working on an RTX 4080 SUPER
(`docs/findings.md`); not run on a Radeon on this tree.

Cause of the torn faces, on an RX 9070 (Beqren's report, not run here): on
RADV through vkd3d-proton a typed (texel buffer) load after a group barrier
can return a value from before another wave's store that came before the
barrier; a raw (storage buffer) load of the same bytes sees it. Found in the
face controls' interpreter (0aa7934094794dbf) by replaying it alone. Fix, on
under Wine (`VICTORY_RAW_TYPED_LOADS`: `0` off, any other value on any host):
in compute programs that store to UAVs and use `s_barrier`, a 16-bit integer
typed load also reads the slot's raw view, and the table of strides picks at
run time: only views of an integer format with 2, 4 or 8 bytes an element take
the raw value. When it is on, the shader cache's key has it.

### GPU page faults, the device lost

Seen by Beqren on amdgpu (not run here): the device lost on reads at GPU
addresses near 0, or inside a texture made a moment before. Fixes, each on
under Wine only and off with `0`:

- `VICTORY_PUBLISH_SAFE`: the slot of every host texture waits for the
  submissions begun before the texture was made (`VICTORY_SHOW_NEW_LATER` did
  so for the pooled ones), and a first upload on the copy queue waits for them
  too. On amdgpu a memory object newer than a running submission is not valid
  for it.
- `VICTORY_NULL_BUFFER_ZEROS`: views of buffers that read nothing (null
  shader, constant and unordered ones, the title-memory run's slot 0) are views
  of 64 MiB of real zeros, unordered ones of a second 64 MiB written into and
  never read back; a null typed buffer UAV is a typed view of them. On
  vkd3d-proton's path a null buffer view holds a load to no bound.
- `VICTORY_NULL_ROOTS`, with the zeros above: a root constant or shader view
  the title never set, with no host copy, or at a host address under 1 MiB is
  bound to the zeros instead of address 0 (each pipeline and cause is logged
  once). The bind goes through the same cache as the title's own root binds.

## Not known

- Long sessions on Linux with this tree have not been tested.
- vkd3d-proton's converter may fail the same way on programs not yet seen.
- The patched wineserver goes stale whenever Proton updates: rebuild it.
- Nothing here has been run on Windows.
