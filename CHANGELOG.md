# Changelog (RYUKI)

Newest first. Each entry: what changed and its switch, why, how it was measured and the result,
and where it came from. See [AGENTS.md](AGENTS.md) for what an entry needs. Measurements: RX 7900
XT 20 GB, Ryzen 7 7800X3D, 32 GB RAM, Windows 11 26300, release builds, free roam from a fixed
save (standing, then walking), pipelines built ahead unless said. "Shown" counts frame
generation's frames, "rendered" the title's own.

Changes before this base (the older RYUKI line on avatarmanz's tree) are in CHANGELOG.md on the
branch `xtx-2026-11-27`.

## 2026-11-28

### Placed-view leak fixed (`VICTORY_PLACED_PAD_IGNORE=1`, `VICTORY_OWNERLESS_VIEWS=1048576`, `-FrameGen`)
From trustno1-coder/VIctory-NVIDIA (4eead67, 144aca0, 89fd936; their PR #58), by way of
[Op-lite/OP-lite](https://github.com/Op-lite/OP-lite) e5e7120e; ported by hand, with their three
tests. Views the title places in memory are named by a "shape" (the description without the
address), at most 4,194,304 of them. The title writes the same view with other garbage in the
description's padding words and in the view words its kind does not use, so every write was a new
shape; once they were full (about half an hour of play), every placed view became a table entry
kept for good, ~15,000 a second (16.7 GB after 70 minutes on trustno1's runs, the frame rate
halved). Now those words are left out of the shape, and entries of views no resource releases are
capped at a million, oldest freed first (a descriptor names its view for a frame or a few).
Both off by default in the code. Not measured over a long session here; an hour-long session
of 2026-11-28 here fell from 33 rendered fps in its first minutes to a median of 27 after ~50.

### mimalloc as the allocator (`--features mimalloc`, build time, off): no gain here
From Op-lite/OP-lite e5e7120e (they measured about -5 ms a frame in busy scenes on an RTX 4080
SUPER): mimalloc (MIT) in place of the Windows heap. Release builds, the user's save at 70 % (a heavy
spot: 8,500-9,200 draws a frame), `-FrameGen` settings: mimalloc 24.8 standing / 21.4 walking
(rendered median over the run 23.3) and 24.8 / 22.9 (24.0); the Windows heap 25.6 / 25.6 (25.4),
its walk through a lighter scene (6,700 draws). No gain within run-to-run spread; left as a
build feature, not the default.

### Tried, not kept: helper threads above normal priority
trustno1-coder/VIctory-NVIDIA 80c70b71 (`VICTORY_HELPER_PRIORITY`): the threads the submitting
thread waits for (wide pool, landing pools, heap records, the pass beside the lists) raised above
normal; they measured +1 fps on a six-core i5-12400F. Ported and measured here in an in-run A/B
(diagnostics build, the user's 70 % save, `-FrameGen` settings, 20 s phases, 8,400-9,400 draws):
frames 42.3 against 43.6 and 43.0 ms, 43.4 against 43.0 and 45.4, 47.2 against 45.4 and 47.4,
47.5 against 47.4 and 48.4; about -0.3 ms, within noise. Here the list threads already run below
normal, so these helpers get their turns. Not kept.

### Tried, not ported: OP-lite's faster guards and lists kept open
`VICTORY_GUARDS_FAST` takes the title's write faults out of the guards' lock: here renderer port 6
(`VICTORY_PORT_GUARD`, in `-FrameGen`) already shares that lock, and the lock waits are 1.6 ms a
frame over all threads. `VICTORY_LISTS_OPEN` spares the title's thread a close and reset of the
fresh host list at each submit: with list threads (`VICTORY_THIN=1`) that is not on its path.

### Ten list threads below normal priority (`VICTORY_THIN_PRIORITY=-1`, `-FrameGen`)
The threads that translate list calls run at the Windows priority given (-2 to 2; unset: as
before). Eight at normal priority fell to about 3.5 fps: they took the title's own threads' turns.
Below normal they wait for them, and more of them use the cores the title leaves idle. Release
build, `-FrameGen` settings with list holds and the table memo, same route: six at normal 29.9
standing / 24.7 walking (rendered median over the run 25.2); below normal: six 28.0 / 23.9 (24.6),
eight 30.6 / 26.4 (26.7) and again 29.6 / 25.9 (26.4), **ten 30.5 / 27.2 (28.8)**, twelve 28.6 /
26.0 (26.4). No faults; shown 30+ in 99.6-100 % of seconds. `-FrameGen` now sets ten below normal,
and the table memo (`VICTORY_TABLE_MEMO=1`: every run above had it on).

### List holds (`VICTORY_LIST_HOLDS=1`, off; on in `-FrameGen`)
A host list holds the resource of each view it makes for its tables once (again in each part of a
list cut at waits), and the dynamic descriptor slots it copies the view into hold nothing: no lock,
reference and retirement for each of ~2.2 million copies a second. Measured with the list meter
(below): 0.24 us of a copy's 0.73 us was that. In-run A/B (diagnostics build, 20 s phases, same
scenes): frames about 1 ms shorter in every matched pair (walking 45.2 against 47.0 and 45.8 ms,
45.2 against 45.8 and 46.6; standing 36.0 against 35.9 and 37.3), no faults.

### List meter (`VICTORY_LIST_METER=1`, diagnostics builds)
Where the list threads' time goes, a line every 5 s: list calls by op; indirect executions, binds
and descriptor tables by step. What it found in the city (six threads, ~5.4 s of CPU a second):
indirect executions are ~175,000 a second at 23 us, 21 of them the bind; of a bind, 10 us a stage
are its descriptor tables; of those, tables holding a constant buffer, made afresh for each draw
(~380,000 a second at 6.9 us), most of it copying ~2.2 million views a second.

### Tried, not kept: constant-buffer tables read in place
With renderer port 5 a title slot's constant buffer is a raw view of its bytes in place, so a table
whose constant buffers are all in place could be read where the title has it instead of made
afresh. In-run A/B: 3-6 ms a frame slower (the title writes those slots anew for its draws, and
making each title slot again costs more than the copies it spared). Removed.

## 2026-11-27

### Descriptor-table memo, ported (`VICTORY_TABLE_MEMO=1|verify`, off)
From [Op-lite/victory-scarlett](https://github.com/Op-lite/victory-scarlett) (their "Fox T"): a
descriptor-table entry, root view or root buffer made ready earlier in the same list with nothing
changed since is not made ready again. Here the list threads are the frame's limit (about 200 ms of
CPU a frame across six threads, ~45,000 list calls). Verify: 85.8 % of ~128,000 entries a frame
are hits, every must-be-zero count 0. Same binary, same settings, `-FrameGen`: off 24.6 rendered
median (27.2 standing, 23.9 walking), on 27.9 (28.0 / 27.9), shown median 48.9 against 54.5. The
on run's view had fewer draws and the list threads' total time barely moved, and runs of one
setup differ by about 2 fps: likely 1-3 fps, not proven. Off until an in-run A/B settles it.

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
