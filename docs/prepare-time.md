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

| # | Avenue (share of prepare) | Mechanism | Tried | A/B (n=4 per arm, mean (min..max) of run medians) | Decision |
|---|---|---|---|---|---|
| 1 | Heap records wait at submission end (10.2 ms, 22 %) | the submitting thread waits for the descriptor records job | `VICTORY_PORT_JOURNAL=1` (upstream #7) | prepare **47.1 (45.5..48.7) → 44.3 (42.9..45.4), −6.0 %**; frame 56.4 → 54.4, −3.5 %; end wait 10.0 → 5.1 ms, journal note waits 1.65 → 0.24 ms | **keep** |
| 2 | Under-flights, card copies, planning (11 ms, 24 %) and copy-back (30 ms, relay thread) | constants heap uploaded and filled every frame | `VICTORY_PORT_CONSTANTS=1` (upstream #1) | frame **56.4 (54.2..58.4) → 51.3 (49.4..52.3), −8.9 %**; prepare −3.4 %; under-flights 4.4 → 1.5, card copies 4.5 → 1.8, planning 3.3 → 0.5, copy-back 30.8 → 24.4 ms | **keep** |
| 3 | Refreshes (14.2 ms, 30 %) | flushes run at ~1.8× on 9 threads: one big span (~17 MB) per flush on one thread; its longest update 5-6 ms a frame of 12-13 ms wall | `VICTORY_REFRESH_PIECE_KB=2048` (new: long spans cut into pieces) | **4 of 4 runs crashed** in the title (write to 0x0) within 46 s; 0 of 8 for the other arms | **dropped, reverted** |
| 4 | Guard lock waits (37-41 ms all threads) | one global write lock for every guard change | `VICTORY_PORT_GUARD=1` (upstream #6) | lock waits 41.6 → 12.7 ms (−70 %), but frame +1.4 %, prepare +1.9 % (noise) | drop: not on the critical path [deduction] |
| 5 | Heap records work (19.6 ms, other thread) | ~430,000 slots scanned a frame | `VICTORY_PORT_DESCRIPTORS=1` (upstream #2) | frame −1.1 %, prepare −0.9 % (noise); records 19.6 → 17.3 ms | drop |
| 6 | CPU oversubscription ("woken, not running yet" 8-18 ms on the chain; 6.6 of 8 CPUs busy) | fixed pools sized for a 24-thread CPU | `VICTORY_THIN_THREADS=4 VICTORY_UNDER_THREADS=3 VICTORY_VIEW_THREADS=3` | one screening run: frame 53.7, prepare 45.1 vs 54.4 / 45.2 | drop (n=1, no sign of a gain) |

## Kept changes

Both are upstream's renderer patches, **off by default** upstream and here; this work verified
them on this machine, it did not write them.

| Switch | Default | Tests | Before → after |
|---|---|---|---|
| `VICTORY_PORT_JOURNAL=1` | off | upstream `watch::port7_tests::no_write_is_lost_with_port_7` (writers and askers with the switch on in all threads and in every other one): passes | end-of-submission wait 10.0 → 5.1 ms; prepare −6.0 % |
| `VICTORY_PORT_CONSTANTS=1` | off | upstream `port::tests::only_memory_below_4_gib_is_wanted_with_the_switch`, `warp_a_stretch_of_a_reserved_section_is_used_in_place_through_a_view_of_ours`: pass | copy-back 30.8 → 24.4 ms; frame −8.9 % |

No red-then-green test of mine exists for these two: the code is upstream's and was not changed.

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
