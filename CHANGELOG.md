# Changelog (RYUKI)

Newest first. Each entry: what changed and its switch, why, how it was measured and the result,
and where it came from. See [AGENTS.md](AGENTS.md) for what an entry needs. Measurements: RX 7900
XT 20 GB, Ryzen 7 7800X3D, 32 GB RAM, Windows 11 26300, release builds, free roam from a fixed
save (standing, then walking), pipelines built ahead unless said. "Shown" counts frame
generation's frames, "rendered" the title's own.

Changes before this base (the older RYUKI line on avatarmanz's tree) are in CHANGELOG.md on the
branch `xtx-2026-11-27`.

## 2026-11-28 (Linux / Proton fork)

Measured under Linux: RX 7900 XT 20 GB, Ryzen 7 5800X3D, 32 GB RAM, Proton Experimental 11 with
the view-tree wineserver and huge pages for renderer port 1, diagnostics builds, unattended runs
from the same save in the same house (`victory-abtest`; standing, or turning the camera all the
time). In-run A/B: phases of 20 s alternating inside one run (`VICTORY_AB`), compared with the
scene's draw count taken out.

### Placed textures found by hash; the filler count only in diagnostics builds
`texture_at` runs for every view of a placed texture a draw's tables name. Its map was an ordered
map with 120-byte keys compared along the tree, under the lock every recording thread takes; it
is a hash map now (only looked up, filled and counted). And on every call whose description had
other filler words than the key (the title leaves stack bytes there) it inserted into a growing
ordered set under a second lock and scanned a range of it, for a count in the log; that count is
kept in diagnostics builds only now. 2+2 alternating standing runs (diagnostics builds, so with
the count in both): -0.2 +- 0.2 ms, list calls 5.68 against 5.73 us; release builds save the
count's work besides (not measured on its own).

### mimalloc as the Rust allocator (cargo feature `mimalloc`, on by default)
The layer's Rust allocations went through the process heap it shares with the title and the
drivers, under Wine Wine's own. With mimalloc (per-thread caches; after upstream VIctory, as
carried by aszterrr/VIctory-NVIDIA-Azter 2653166, which measured 24.7 -> 26.9 fps on an RTX
4090) the frame here is 0.8 ms shorter standing: 2+2 alternating runs, +0.8 +- 0.3 ms without it
(medians 37.1/36.6 with, 37.3/37.5 without). Building it needs clang-cl (cargo-xwin's usual
toolchain); `--no-default-features` builds without it.

### DirectStorage: decoded results kept (`VICTORY_DSTORAGE_CACHE_MB`, 1536 MB; 0: none)
Turning the camera the title asked for about 5,900 decompressing requests a second (360 MB of
output), and 99.7 % of them were the same as one asked before: same file, offset, sizes and
options; about 81,000 distinct in a run, 10-20 new per 5 s. Each was read and decoded again
(BCPack in software, half of it; zlib, a third), and the DirectStorage workers used 1.9 cores
while turning (0.15 standing), beside the title's 3.5 and our list threads. Results of requests
from the title's package files (which never change while it runs; requests from memory are not
kept) are kept in two generations up to the budget and copied again. In-run A/B turning: about
6,000 copied a second against 11-100 decoded; frame median 47.9 -> 44.6 ms (-6.9 %), slowest
10 % 64.0 -> 57.6 ms; regression +1.3 ms at a scatter of 6.7 ms (noise). Also some hashed sets
of the recording path use the address hasher instead of SipHash (not measured on its own).
The budget: with 1024 MB about 1,200 requests a second still missed while turning (the data the
title asks for again and again is larger); with 1536 MB 11-100, with 2048 MB 1-33 (RAM left
available at least 5 GB of 32). 1536 is the default; 2048 for PCs with RAM to spare.

### Turning the camera: the page-mapping copies' views kept in the books (launch script, `VICTORY_SOFT_VIEWS=copies`)
Op-lite's freeze fix as RYUKI ported it (left off there: no clear gain standing on Windows, and
`verify` found 8 differing pages in 6 minutes). Turning the camera the title remaps about 5,000
pages a second (`CopyPageMappingsBatchX`); each became a `MapViewOfFile3` and an
`UnmapViewOfFile2`, and under Wine those calls took about 2.4 s of thread time per 5 s (longest
over 100 ms). In the timeline the title's `CopyPageMappings` were 3.5 ms of each frame's chain.
Kept in the runtime's books until touched, almost none ever is (0-1 per 5 s made real). 2+2
alternating turning runs: -5.1 ms a frame (+-1.4 by lines; medians 56.7 and 57.1 against 52.4
and 50.1 ms). `verify`, 5 minutes turning: 0 differing pages. Set by the start script; the code's
default stays off.

### Our QueryPerformanceCounter from the TSC (`VICTORY_FAST_CLOCK`, on; `=0` the host's)
Proton Experimental 11's `RtlQueryPerformanceCounter` is always a syscall (`NtQueryPerformanceCounter`
through `__wine_syscall_dispatcher`, the whole CPU state saved and restored, for a
`clock_gettime`): every `std::time::Instant::now` of ours was one, about 7 % of the list threads'
time and 6 % of the submitting thread's in a CPU profile. At start victory.exe's own import of
`kernel32!QueryPerformanceCounter` is pointed at the invariant TSC scaled to the host counter's
ticks (calibrated over 20 ms, anchored to a real reading every 100 ms, the scale refined over the
whole run); only with an invariant TSC (CPUID). The title's imports keep the host's counter.
In-run A/B standing: -1.0 ms a frame (scatter 0.4 ms), median 40.0 -> 39.3 ms. A first version
took the TSC delta unsigned: a reading just before another thread's re-anchor became 2^64 ticks
in the future, durations of 10^12 ms in the log and a crash of the title; it is signed now.

### Textures by index only up to 72 % of the VRAM budget (launch script)
`VICTORY_TEXTURE_ROOM_PERCENT=72` (code default 90). Turning the camera filled VRAM to 18.2-18.9
of 20.5 GB (a desktop compositor can crash when VRAM runs out); with 72 the peak was 15.7 GB, the
frame time the same (72.8 against 72-75 ms) and the textures sharp.

### Faces: bake draws wait longer for their sources (launch script; not measured yet)
`VICTORY_BAKE_TEXTURES_MS=10000` (1500), `VICTORY_EARLY_TEXTURES_MS=40` (12), from a tester's set
on Discord. The title bakes its characters' skin from textures picked by index while it loads;
what is not on the card then is baked as nothing for the session. Effect on the faces not yet
checked in play; loads may take longer.

### Diagnostics: where the recording threads' time goes
- The guards' lock taken alone, by line of `watch.rs`: times, ms waited, ms held (frame line
  "the guards' lock taken alone"). Turning: the takers hold it briefly but wait long for shared
  holders; under Wine a waiting writer holds back new readers (the title's write faults waited
  12.5 ms a frame for the lock turning, 6.6 standing).
- Timeline spans inside `ExecuteIndirect`, `prepare` and `bind`; counters of fresh tables met
  again and of what moves a host list's epoch (frame lines "tables met again", "host lists'
  epochs moved").
- Findings (perf, `kernel.perf_event_paranoid=1`, and the timeline): our 8 list threads use as
  much CPU as the title (4.3 cores each on 8 physical cores). Of their time 58 % is
  `State::tables`: about 5,500 binds a frame walk 434,000 table entries, 168 ms of CPU, because
  the fresh-table cache hits 10 % of the time: the list's epoch moves about 3,700 times a frame,
  nearly once a draw, and a table's readers are cleared at each walk. 3,800 `ExecuteIndirect` a
  frame cost 32 us each, nearly all of it binding state again. Proton Experimental 11's
  `RtlQueryPerformanceCounter` is always a syscall: every `Instant::now` enters the kernel.

### Experimental, off: fresh tables kept over other textures' moves (`VICTORY_TABLE_TEXTURES=1`)
The epoch moves as before; each list logs why (a texture's state, or anything else), and a fresh
table met again after moves of other textures only is taken as up to date. In-run A/B standing:
-2.0 ms (noise level); the log rescued few tables because readers are cleared at every walk.
Off until that part is done.

### Tried, not kept
- `VICTORY_GUARD_HOT=200` turning: +0.8 ms (few blocks qualify).
- The guards' lock taken shared for remaps/unmaps of untracked memory: +0.2 %, remaps are rare
  per frame (reverted).
- Short-lived upload/shared/UAV buffers made resident without waits (after "VIctory AMD",
  `VICTORY_BUFFER_WAIT`): -0.3 ms turning; the "waited until mapped" time (13 ms a frame turning)
  is texture-pool blocks, off the frame's path (reverted).
- A list's first use of a texture and unchanged `settle`s leaving the epoch as it is: moves
  3,457 -> 1,371 a frame, table hits unchanged (reverted).
- Fresh tables' readers told apart by what they read (entries and kinds) instead of by the
  plan, with the move log on: -1.1 ms, table hits 860 -> 1,313 of ~7,000 a frame; the ready list
  is emptied by walks that change the table's own textures, and fresh tables live per list
  (~23 binds). Not kept.
- Telling the GPU side of unmapped and remapped pages once per run of neighbouring pages instead
  of page by page (`change` in `xmem.rs`), with the soft views on, turning: +1.3 ms. Not kept.
- The title's own `QueryPerformanceCounter` (its import from `api-ms-win-core-profile-l1-1-0`)
  from the same TSC clock: -0.1 ms, no crash; not kept (the title's timing stays the host's).
- A descriptor slot's kept resource taken by reference (no reference count change for the same
  one again) and two slot maps on the address hasher: +0.6 +- 0.3 ms, worse (callers that passed
  theirs by value now cloned and dropped it). Reverted.
- Built with `-C target-cpu=x86-64-v3` (AVX2, BMI, FMA everywhere; as Azter's fork does): standing
  2+2 runs -0.3 +- 0.2 ms, partly fewer draws in its runs. Not adopted: it would require AVX2 of
  every player's CPU for about nothing.
- `VKD3D_CONFIG=descriptor_heap` (vkd3d-proton on RADV's `VK_EXT_descriptor_heap`), standing, 2+2
  runs: -0.7 +- 1.2 ms, no clear gain (not set).
- Our own writes ([`writing`]) opening their guarded blocks with the guards' lock shared, block
  by block as a write fault does: turning +6.9 ms, median +22.8 % (reverted).
- `VICTORY_WINDOWS_BAR_MB=2048` with `VICTORY_CARD_WINDOWS_MB=1024` (a tester's NVIDIA set):
  copy-back 150 ms a frame instead of 7, 4.5 fps. Title windows in card memory are read back over
  PCIe here.

## 2026-11-27

### Tried, not kept: eight list threads
`VICTORY_THIN_THREADS=8` fell to about 3.5 rendered fps for long stretches (shown 30+ in 57 % of
seconds): the threads that translate list calls crowd out the title's own on 8 cores. Six stay.

### Freeze fix, ported; left off (`VICTORY_SOFT_VIEWS=copies|verify`, off)
From [Op-lite/victory-scarlett](https://github.com/Op-lite/victory-scarlett) `bc264a4`. Views of
the title's batch page-mapping copies are kept in our books and mapped only if touched, sparing a
`MapViewOfFile3` and an `UnmapViewOfFile2` per 64 KiB page (slow behind the kernel's address-space
lock under memory pressure; Scarlett measured long frames 2.7 % to 0.8 %). Ported by hand: this
tree's guards are shared (`RwLock`, `try_read` for making a view real), the fault's origin is not
told apart. Unit tests pass (Scarlett's two soft-view tests among them).
In the game: `copies` ran 6 minutes with no fault, every second at 30+ shown, about 6,000 map and
6,500 unmap calls spared per 5 s, and no clear frame-rate gain (rendered 24.4 median against 26.7
without, within run-to-run spread). `verify` found 8 pages in 6 minutes that differed from what
the books would have read, or changed between the two reads. Left off until that is understood.

### Window caption
`RYUKI | fps | ms | draws`; `VICTORY_CAPTION_GAME=1` adds the game's name.

### `-FrameGen`: the measured settings and six list threads
Every frame-generation figure was measured with the older line's RX 7900 XT settings over the
base's; `-FrameGen` now sets them (memory windows of 1 MiB with 12 MB at roots, card-side compares
for buffers of 1 MiB and more, title memory in place, texture steps of 80 ms, joined lists, ...)
and `VICTORY_THIN_THREADS=6`: rendered 26.7 fps median against 21-23 with four (two: 13.9);
shown median 51.6, slowest 1 % 37.6, every second at 30+.

### Resizable BAR windows in `-FrameGen` (`VICTORY_WINDOWS_BAR_MB=4096`)
Windows of title memory in the card's memory, written through Resizable BAR (it must be on in
the firmware). With the copy-back skip: shown median 46.3, slowest 1 % 30.4, 99.4 % of seconds at
30+, no second under 25 (against 52.7 / 27.1 / 98.7 % without). The steadiest of the runs.

### Copy-back skip (`VICTORY_SKIP_WC_BACK`, on)
From [PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab) `e0a91ea`, after
fxceless/victory-fork #868 (Chipsraine, reservation fix by iGoldenNinja) and #1268 (Proudnoob4,
the 1 MiB gate). Shader writes to memory the title reserved write-combined (CPU-written, never
read back) are not fetched back from the card, for buffers of 1 MiB and more. Copy-back 9.6 to
4.4 ms a frame; rendered 21 to 25 fps in heavy views. Pictures checked: no dark rooms.
`VICTORY_SKIP_WC_BACK=0` fetches everything.

### Frame generation (`VICTORY_FRAMEGEN=1`, `-FrameGen`)
From [gleaf-6/VIctory-7900XTX-](https://github.com/gleaf-6/VIctory-7900XTX-) (after
MrSkelee/VIctory-MrSkele-AMD): AMD's frame-generation swap chain, fed by the title's own FidelityFX
upscale (depth, motion vectors, jitter, camera), answered by the PC FidelityFX SDK
(`VICTORY_FIDELITYFX_DIR`). The host upscaler serves the title whenever frame generation is asked
for (`VICTORY_FFX_HOST=1`: the upscaler alone). Only the **Series S profile** has a FidelityFX
upscale: in Series X shown equals rendered. First measured: shown median 40, 97 % of seconds at
30+ from about 20 rendered; the short drops were pipelines made mid-play (build them ahead).
Fast motion ghosts at these base rates. The very first attempt removed the device at the start
screen and the PC stopped with bugcheck 0x1E in `amdkmdag.sys`; every run since (well over an
hour) has been clean.

### Audio as a console has it (`modules::endpoint`)
The title's sound engine reaches Windows audio itself (device enumerator, then
`ISpatialAudioClient`) and expects a console's output: always there, mixing at 48 kHz. With a
monitor's HDMI audio as the default (it will not open), a 44.1 kHz Bluetooth speaker, or no output
at all, its audio never started, its loading screen waited forever and the title stopped at its
own `int3` (`+0x4dcbc9d`) about 15 s in, on every build: for a whole evening it looked like a code
regression and was a pair of headphones switched off. The layer now gives the title an output
that opens (the default, else another active one), read as 48 kHz with Windows converting
(`AUTOCONVERTPCM`), else a silent one with a silent spatial client; the list of outputs always
holds that one. `VICTORY_AUDIO_DEVICE=<part of a name>` or `silent` picks one;
`VICTORY_AUDIO_CONSOLE=0` turns it off. Measured: a 44.1 kHz speaker and no output at all both
reach the world (they did not before).

### Unattended test runs (`VICTORY_QUIT_AFTER_S=<seconds>`)
A run exits by itself after that long, so measurements need not kill the process.

### Tried, not kept (on the older line, same day)
- Scarlett's descriptor-table memo (`VICTORY_TABLE_MEMO`): exact (84 % of entries skipped,
  verify 0) but no frame-rate gain here; the skipped work is beside the frame's long path.
- The title's GPU memory through Resizable BAR (`VICTORY_CARD_MEMORY`): 19.4 to 12.7-15.5 fps and
  black faces (the CPU reads that memory at about 30 MB/s).
