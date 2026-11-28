# The black picture of 28 Nov: renderer port 1 split in two mappings

**Status: fixed** (commit `715fa0378`, branch `k7900`). Six unattended runs after
the fix: picture right in all six, 28.9-30.1 fps.

## What it looked like

- The title's picture is black **from the start screen on**, for the whole run.
  The layer's own drawing over it (the pipeline loading bar, the panel) shows,
  so our present works; what the title draws is black.
- About **7 runs in 10**. It is not tied to one build: the build of 08:53 that
  had drawn right in the morning drew black in the afternoon too.
- A black run has its own signature in the log: around 4,000 draws a frame
  instead of 5,500-6,800, about 34 fps instead of ~25, and an extra block of
  12 compute shaders bound on the first frame of the world.
- With `VICTORY_DUMP_TARGETS` (diagnostics build), a black frame has **every
  target of every pass empty**: G-buffer, depth, shadows. Only the clears show.
  The commands and the draw state are the same as in a good frame, draw for draw:
  viewport, scissor, blend, write masks, shaders. The card simply read zeros
  for constants, vertices and arguments.

## What was ruled out, each by runs (not by reasoning)

| suspect | test | result |
|---|---|---|
| the kroeeee merge, the loading bar | base build; merge without bar; merge exe with base script | black in all of them at times |
| today's translator changes | the 08:53 build; code-table change reverted; waterfall change reverted | black |
| the P.4 native families | 4 runs with none | 4 black |
| the save and the profile data | `ktrials` restores both whole every run | black |
| the host pipeline library | emptied | black |
| the shader translation cache | `VICTORY_SHADER_FRESH=1` | black |
| vkd3d-proton's and Mesa's caches | emptied (`play-fresh.sh`) | black in the first run |
| system state | reboot | black |
| the clock | `TZ` shifted by 8 hours | black |
| transparent huge pages | off for the process (`prctl PR_SET_THP_DISABLE`) | black |
| the kernel's zero page under the user pointer | every page written once before the card got it | black (and worse, see below) |
| **renderer port 1** | `VICTORY_PORT_CONSTANTS=0` | **4 of 4 right** (20 fps) |

Two leads that looked good and were wrong, kept here so nobody follows them again:

- **The huge-page helper's output** (`run-*.thp`): "31 collapsed, 57 not: Invalid
  argument" matched every black run and "88 collapsed, 0 not" every good one. It
  was a symptom: a 2 MiB piece across the boundary between two mappings cannot be
  collapsed. Turning huge pages off changed nothing.
- **Writing every page once at allocation** ("prefault") made it worse: black in
  every run. Each piece then had its anonymous memory set up at once, so Linux
  could never join them.

## The cause

Renderer port 1 (`VICTORY_PORT_CONSTANTS=1`, `crates/victory-gpu/src/port.rs`)
opens the title's 32-bit graphics heap (185 MB) as a host heap from its address.
vkd3d-proton gives it to amdgpu as a user pointer, which needs anonymous private
memory, so `xmem::private_backing` replaces each run of fresh pages the title maps
there with private memory.

Grand Theft Auto VI maps the five pieces of that heap with **five calls, not in
address order** (47 MB, 100 MB, 10 MB, 1 MB, then the 27 MB piece between the two
groups). Made private one by one, each piece was a mapping of its own. Linux joins
neighbouring anonymous mappings only while at most one side has had a page
written. When the title had written into **both** groups before the last piece
came (microseconds), that piece could join only one side: the heap stayed **two
mappings**. Then **the card read zeros from all of it**.

Measured with the checks off, `/proc/<pid>/maps` against the screenshot:

| run | mappings over the heap | picture |
|---|---|---|
| 1 | 2 (`49c32000-4c630000`, `4c630000-55d30000`) | black |
| 2 | 2 | black |
| 3 | **1** (`49c32000-55d30000`) | right |
| 4 | 2 | black |
| 5 | 2 | black |

With the check at open (below), every two-mapping run showed `seen=0 marked=30`:
the card read none of the 30 marks the PC had written. Opening again three times
did not help: the split stays for the run.

Why the driver reads nothing from a two-mapping range is not established (amdgpu's
user pointer path, or RADV's import). The measurement is what the fix rests on: one
mapping, read; two, not.

## The fix (`crates/victory-hle/src/modules/xmem.rs`, `whole_reservation`)

- **The first fresh run** mapped into a 32-bit graphics reservation makes **the
  whole reservation one private mapping** at once: `VirtualAlloc2` replacing the
  whole placeholder (coalesced first with `MEM_COALESCE_PLACEHOLDERS` if it was
  split). Every page is written once with its zero, so the kernel gives the memory
  at once, in huge pages where it can.
- **Later runs** mapped into it find their pages there (`in_whole`): nothing new
  is mapped, so nothing can split.
- **Unmapping a run inside** writes zeros over it instead of giving it back
  (`release_private`): the next fresh run mapped there reads zeros, as fresh pages
  must.
- **Pages holding data** cannot be mapped into it. The title never does that
  (`not_fresh=0` in every log); if it ever happens, the log has an error line, and
  `VICTORY_PORT_WHOLE=0` goes back to piece by piece.
- Fresh pages read zeros, so which memory holds them is not observable to the
  title.

Six runs: one mapping every time (`/proc/<pid>/maps`), marks 30/30, picture right,
28.9-30.1 fps.

## The checks that stay in (`crates/victory-gpu/src/port.rs`)

- **`sees_the_pages`, at open:** the PC writes up to 32 marks where the memory is
  still zero, with an atomic exchange from zero and back, so a write of the title's
  in between stays. The card reads them on a queue of ours, then they are cleared.
  - All seen: in use.
  - None seen: reopened up to three times, then refused. Refused means the memory
    is copied as without the port: **about 2 fps**, but drawn.

  Log lines: "marks the card read back on the pages just opened seen=N marked=M",
  "the card never read the private pages".
- **`check`, every frame-rate line:** 12 pieces of 256 bytes holding something are
  compared, card against PC. "renderer port 1 check: the card reads other bytes than
  the PC has" means the card lost the memory while the run was going.
- `VICTORY_PORT_CHECK=0` turns both off.

## If it comes back

1. **Look at the log:** "made private memory whole"; `seen=30 marked=30`; no "card
   reads other bytes" warnings.
2. **Look at the mappings while the game runs:**

   ```sh
   pid=$(pgrep -f 'release\\victory.exe run' | tail -1)
   awk '{split($1,a,"-"); s=strtonum("0x" a[1]); if (s>=0x49000000 && s<0x56600000) print}' /proc/$pid/maps
   ```

   The heap must be one `rw-p` line.
3. **If the heap is one mapping and the card still reads zeros,** the cause is
   elsewhere. Next: the import itself (`crate::inplace::open`, `From::Part`) and
   whether vkd3d-proton fell back to memory of its own.
4. **The tools used,** all in `~/Games/victory-data/`:
   - `vma-watch.sh`: mappings and the mark check for each run;
   - `shotstat.py`: screenshot brightness per run;
   - `no-thp.py`: a process without huge pages;
   - `play-fresh.sh`: launch with the caches emptied.

   Diagnostics build: `VICTORY_CAPTURE`, `VICTORY_CAPTURE_WORLD`,
   `VICTORY_DUMP_TARGETS`, `VICTORY_CAPTURE_FRAMES` (each pass's targets and the
   frame's operations).

## Relation to translation work

None, as measured: it happened with no native family, with yesterday's
translator, and with today's. When more programs are translated (P.4 families,
0x1c8 graphics), this check stays useful. A black picture with `seen=30` and no
"other bytes" warnings is a translation problem, not this one.
