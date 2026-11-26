# CPU prepare time: what was measured, kept and dropped (2026-11-26)

Machine: Windows 11, Ryzen 7 9800X3D (SMT off, 8 logical CPUs), 31 GB RAM, RX 9070 XT 16 GB
(RDNA 4), Series X profile (no FidelityFX SDK here), `scripts\play.ps1` settings, diagnostics build.
Branch `perf/prepare-time` = upstream `main` c9c932c + #55 (needed on RDNA 4, see below) + the
benchmark tooling below. Tags: **[fact]** measured or read, **[deduction]** follows from facts,
**[hypothesis]** not verified.

## The benchmark

- `scripts\bench.ps1`: starts the title with `play.ps1`'s settings and a fixed key script
  (`Enter` every 5 s from 20 to 55 s) into the first mission's intro, the same scene every run;
  stops only the processes it started, by id, after 200 s.
- `VICTORY_FRAME_CSV=<file>` (new, diagnostics builds): one line per present: frame ms, the
  submitting thread's preparing ms, copies and texture refreshes (ms, MB), copy-back (ms, MB),
  write-tracker lock waits of all threads, draws. Test: `a_line_holds_what_each_counter_grew_by`.
- Summary per run: the window 90-190 s, frames with ≥ 100 draws: median, p10, p90 and the median
  of each second's worst frame, of frame ms and prepare ms.
- `scripts\bench-parts.ps1`: the runtime's own 5 s counters averaged over world blocks (≥ 2000
  draws); `scripts\bench-ab.ps1`: per arm, mean and min..max of the runs' medians.
- Arms interleaved (A, B, C, ... A, B, C ...), 4 runs each, one build, warm shader and pipeline caches.
  Run-to-run spread of the baseline's frame median: 54.2-58.8 ms, so under ~5 % is noise.

## Where prepare time goes (baseline, [fact], 4 runs, ms a frame, submitting thread)

Frame 55.6 ms, prepare 46.7 ms: the submitting thread is the wall (the card is ~72 % busy).

| Part | ms |
|---|---|
| Lists' steps | 20.4, of it refreshes of host copies **14.2** (≈6,700 spans a frame, ≈5,700 known to copy nothing) |
| Waiting at the end for the heaps' descriptor records | **10.2** (the records job itself 19.6, on another thread) |
| Snapshots of title buffers before shader writes | 5.0 |
| Title bytes put under flights' shader writes | 4.2 |
| Copies in the card's memory | 3.8 |
| Asking the write watch | 3.5 |
| Planning steps done on the card | 3.2 |
| Lock waits, **all** threads (mostly the guards' lock) | 37-41 (not on the submitting thread: its own lock wait is 0.0) |

No on-CPU sampler is installed on this PC; attribution is from the runtime's own spans and
counters and one `VICTORY_TIMELINE` trace (critical chain with waits and wakers).

## Ranked avenues

The mission's seven avenues, ranked by measured share of the submitting thread's prepare time
(baseline 46.7-47.9 ms). n=4 per arm, interleaved; values are mean (min..max) of run medians.

| Rank | Avenue | Share of prepare | Mechanism (measured) | Fix tried | A/B result | Decision |
|---|---|---|---|---|---|---|
| 1 | 3. Uploads and refreshes | refreshes 14.2 ms (**30 %**) | ≈6,700 spans a frame, ≈5,700 known to copy nothing; flushes run at ~1.8× on 9 threads; one big span (~17 MB) per flush on one thread, its longest update 5-6 ms a frame of 12-13 ms wall | `VICTORY_REFRESH_PIECE_KB=2048` (new: long spans cut into pieces) | **4 of 4 runs crashed** in the title (write to 0x0) within 46 s; 0 of 8 for base/jc | **dropped, reverted** |
| 2 | 2. Copy-back, and the steps it causes before submission | under-flights 4.2 + card copies 3.8 + planning 3.2 = 11.2 ms (**24 %**); copy-back itself 30 ms on the relay thread | the 174 MB constants heap is a host copy: its writes sent up (17 MB a frame), shader writes fetched back and merged, ~6,000 constant fills a frame waited for or done on the card | `VICTORY_PORT_CONSTANTS=1` (upstream #1: heap used in place) | frame **56.4 (54.2..58.4) → 51.3 (49.4..52.3), −8.9 %**; prepare −3.4 %; under-flights 4.4 → 1.5, card copies 4.5 → 1.8, planning 3.3 → 0.5, copy-back 30.8 → 24.4 ms | **keep** |
| 3 | 1. Lock waits on the submit path | end-of-submission wait for the heaps' records 10.2 ms (**22 %**); the thread's own lock waits ~0 | the submitting thread waits for the records job and the journal's single lock | `VICTORY_PORT_JOURNAL=1` (upstream #7) | prepare **47.1 (45.5..48.7) → 44.3 (42.9..45.4), −6.0 %**; frame −3.5 %; end wait 10.0 → 5.1 ms; journal note waits 1.65 → 0.24 ms | **keep** |
| 3b | 1. Lock waits, other threads | guards' lock 37-41 ms a frame on all threads, **≈0 %** of prepare | one global write lock for every guard change | `VICTORY_PORT_GUARD=1` (upstream #6) | lock waits 41.6 → 12.7 ms (−70 %); frame +1.4 %, prepare +1.9 % (noise) | drop: off the critical path [deduction] |
| 4 | 4. Descriptor and table work | part of the 10.2 ms wait above; records job 19.6 ms on its thread | ~430,000 slots looked at a frame, ~17,600 mirrored | `VICTORY_PORT_DESCRIPTORS=1` (upstream #2) | frame −1.1 %, prepare −0.9 % (noise); records 19.6 → 17.3 ms | drop |
| 5 | 5. Kernel and OS calls | asking the write watch 3.4 ms (**7 %**) | all threads: 316 `GetWriteWatch` askings a frame over 1,982 MB, 2.8 ms (~9 µs each); **0 `VirtualQuery` a frame**; 759 blocks made read-only, 4.4 ms changing protections; remap moves 6.9 ms | none | — | not pursued: 7 % share, ~9 µs a call on native Windows; the cost the mission warns of is a Wine/Proton one [deduction] |
| 6 | 6. Allocation and page-fault churn | not attributable to prepare: no named part of the submitting thread is allocation | all threads: 5,638 page faults and 378 guard faults a frame; host lists made 0.5 a frame (1.0 ms); kernel time 55 ms a frame (16 % of the process's CPU) | none | — | not pursued: no measured share of prepare; would need a sampler to place the faults |
| 7 | 7. Command-list translation hot spots | **0 %** directly: translation runs on 8 recording threads beside the submitting thread | 31,782 list calls a frame, 215 ms of those threads' time; on the critical chain only 1.4 ms a frame ("recording: inside our list calls", timeline) | none | — | not pursued: off prepare's path; no on-CPU sampler on this PC to find hot spots |
| — | (extra) CPU oversubscription | "woken, not running yet" 8-18 ms on the chain in the timeline | 6.6 of 8 logical CPUs busy; pools sized for 24 threads | `VICTORY_THIN_THREADS=4 VICTORY_UNDER_THREADS=3 VICTORY_VIEW_THREADS=3` | one screening run: frame 53.7, prepare 45.1 vs 54.4 / 45.2 | drop (n=1, no sign of a gain) |

Avenues 5-7 counters: `scripts\bench-parts.ps1` over the 4 baseline runs of batch ab2 (100
world blocks); with journal + constants: 280 askings (2.8 ms), 4,885 page faults, 295 guard
faults, kernel time 46 ms.
## Kept changes

Both are upstream's renderer patches, **off by default** upstream and here; this work verified
them on this machine, it did not write them.

| Switch | Default | Tests | Before → after |
|---|---|---|---|
| `VICTORY_PORT_JOURNAL=1` | off | upstream `watch::port7_tests::no_write_is_lost_with_port_7` (writers and askers with the switch on in all threads and in every other one): passes | end-of-submission wait 10.0 → 5.1 ms; prepare −6.0 % |
| `VICTORY_PORT_CONSTANTS=1` | off | upstream `port::tests::only_memory_below_4_gib_is_wanted_with_the_switch`, `warp_a_stretch_of_a_reserved_section_is_used_in_place_through_a_view_of_ours`: pass | copy-back 30.8 → 24.4 ms; frame −8.9 % |

Red-then-green tests of the switches' mechanism (new, `#[ignore]`d so the default suite with the
switches off stays green; the switch is read once per process, so each runs on its own):

- `port::tests::the_constants_heap_is_used_in_place_with_the_switch`. Unset, red:
  "the title's constants heap is not used in place (a host copy is tracked)"; with
  `VICTORY_PORT_CONSTANTS=1`, green.
- `watch::port7_tests::an_asking_notes_its_pages_under_the_shared_lock_with_the_switch` (and the
  written page is told). Unset, red: "the asking took the journal alone to note its pages (not
  shared)"; with `VICTORY_PORT_JOURNAL=1`, green.

```powershell
cargo test -p victory-gpu --release --lib -- --ignored --exact port::tests::the_constants_heap_is_used_in_place_with_the_switch watch::port7_tests::an_asking_notes_its_pages_under_the_shared_lock_with_the_switch
```

Counters per change: journal: the end-of-submission wait and the journal's note waits (above);
constants: under-flights, card copies, planning steps and copy-back ms (above).

Changes of mine kept in the branch, with tests:
- The per-frame record (`VICTORY_FRAME_CSV`) and running totals; `a_line_holds_what_each_counter_grew_by`.
- Diagnostics: refreshes on several threads (wall vs threads' time, spans, copies).
- `VICTORY_TITLE_DIR`: the package folder when the executable lives outside the dump.
- #55 merged onto main: its record counts now follow main's view addresses in the slot table
  (they collided). Without #55 the AMD compiler crashed this card in the world (run setup-3,
  `amdxc64.dll`, read of 0x8) and `size_tests` crashed the shader suite. Test fix
  `view_addresses_follow_the_strides`: red ("left: [305419776, 66, 3735928559, ...]"), then green.

## Not worth it

- `VICTORY_PORT_GUARD=1`: −70 % lock waits, no frame or prepare change (4 runs each).
- `VICTORY_PORT_DESCRIPTORS=1`: −1 %, inside noise (4 runs each).
- Smaller thread pools for 8 CPUs: no change in one screening run.
- Cutting refresh spans (`VICTORY_REFRESH_PIECE_KB`): title crashes 4/4. Pieces of one span put
  several threads in one host copy at once; until then every span of a flush was its own copy
  (3,424 spans in 3,424 copies, [fact]), so its per-copy state is not made for that [deduction].
  The splitting test passed (byte coverage) and could not catch it. Reverted.
- Shared locks inside the refresh checks ([hypothesis] before measuring): every span is in its
  own copy, so no such contention.

## Final configuration

`VICTORY_PORT_JOURNAL=1 VICTORY_PORT_CONSTANTS=1`, as a whole against the baseline, interleaved,
same build, 4 runs each ([fact]):

| | baseline | journal + constants | change |
|---|---|---|---|
| frame median | 55.6 ms (54.4..58.8) | **48.6 ms (47.5..50.6)** | **−12.6 %** (18.0 → 20.6 fps) |
| prepare median | 46.7 ms (45.4..49.1) | **41.5 ms (40.9..42.7)** | **−11.2 %** |
| frame p90 | 118.3 (98.7..168.2) | 100.8 (72.1..139.5) | −14.8 % (wide spread) |
| worst frame per second (median) | 155.1 | 151.6 | −2.3 % (noise) |
| copy-back | 30.3 ms | 24.2 ms | −20 % |

```powershell
scripts\play.ps1 -Exe <exe> -More @{VICTORY_PORT_JOURNAL='1'; VICTORY_PORT_CONSTANTS='1'}
```

Not measured here: `VICTORY_PORT_CBV=1` (it changes translated shaders: needs its own translate
and warm run), Series S with FidelityFX, the street-chase scene, longer sessions, other CPUs.
