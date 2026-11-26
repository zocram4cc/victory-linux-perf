# Journal

What was tried, what came out of it, and what was decided. Newest first. The Spanish journal before 25 Nov 2026 is in [history/es/diario.md](history/es/diario.md).

## 2026-11-26

**Playing it: the load is slow and black, and two pieces were missing (P, A)**
- The user played the Jason save (our build, pool and S.7 guard not yet built). Seen: the world comes in black (Jason's house included) with many textures missing for a long time, and ~3 fps. Screenshot in play (`bench/user-play-20261126/shot-now.png`): S.9 again, a dark interior, Jason as a black silhouette, white frost-like speckles on the floor and table, windows blown out.
- Play log (`royale-save/logs/20261126-050658-run.log`): ~3,000 pipelines first bound in the first 2-3 minutes of the world (1,795 in the first minute), **one frame of 9,953 ms** at 04:10:19, then 2.1-3.0 fps. The shader translation hits its disk cache; the pipelines are created on first bind, on the submitting thread.
- **Two pieces added to the target (README §4):** **P** pipelines (a journal of what each run creates, replayed on spare cores at the loading screen; a pipeline still missing built on a worker), modelled on KytyPS5's shader journal, pipeline journal and driver-cache saver; **A** asynchronous uploads (a copy queue of its own, fed by workers). Neither is in the 1.3 → 3.1 chain, so they do not wait for it; the steady ~2-3 fps is the serial chain, which only B, D.1 and Q fix.

**S.7 traced: a root view left at address 0; the 1.3 read census built**
- The only SQC fault on record (kernel log, 25 Nov 23:30:38; prologue run `royale/logs/20261125-232915-run.log`): *SQC (data) reading address 0x0*, `PERMISSION_FAULTS 0x3`. The SQC is the scalar cache: constant buffer loads. 100 ms before, the sweep had let go of 252 texture copies and a texture staging buffer.
- `draw.rs`: a root CBV or SRV whose title address does not resolve to a host copy (`root_view` returns `None`, or the title never set the root) was **skipped with `continue`**. vkd3d then leaves the root descriptor at 0, and the shader's constant loads read address 0: that fault exactly.
- **Fix (`9e3e8e5`):** such a root view is bound to 64 KiB of zeros in the list's scratch; the first 8 are logged with their address, and the frame line keeps the count (`root views bound to zeros so far`). If the count shows up in prologue runs, the next question is why the address had no host copy (a window the sweep let go?).
- **1.3 step 1 (`5395625`), `VICTORY_BACK_CENSUS=1`, off:** `census.rs`. `write_mapped` (all of its callers are copy-backs) notes pages; after the bottom-of-pipe writes they get `PAGE_GUARD` (private read-write pages only); at the next submission's start the guards left are lifted, before we read title memory. A guard fault from the title's code counts as touched (read or write first). Faults at fastcopy's copy instruction (it resumes the copy) or in our own image are counted apart, as the layer's own. Kernel reads of a guarded page would fail, so the census needs the fast reads (on by default) and is off with the relay.

**The open world is a serial chain; the order of the roadmap holds**
- The layer's card clock, open world outside: **the card has nothing of ours queued 212-284 ms a frame (busy 48-57 %), 173-228 ms of that while our submitting thread is working**; the thread then waits for the card ~265 ms. CPU and card never overlap. Overlapped, the frame would fall towards ~270 ms (phase 3, ~1.8×).
- Within our thread's work: **408-437 MB a frame snapshotted on the CPU** to find what shaders wrote in title buffers' copies (copy-back, piece B), plus 72 ms / 46 MB of copy-back.
- `VICTORY_CARD_BUFFERS_MIN_MB=1` (the card compares copies of 1 MiB or more): 491-494 ms against 491-504; prepare 119 → 107 ms but GPU 265 → 276 ms, and the CPU still snapshots 408 MB. No gain; stays off (`bench/trials-…-world-out-card1`).
- **Next:** 1.3, measured first: how much of what is copied back does the CPU read before the GPU writes it again ([design](design/p1.3-copyback-on-demand.md)). Lesson 1 (from `fx`) says the copy-back must shrink before frames in flight.

**S.4: the open-world baseline (outside the house), and where its time goes**
- **Baseline** (`BENCH_SETTLE=5 scripts/trials.sh -n 3 -l world-out -s 30`, all windows valid, none lost): **491-504 ms a frame (2.0 fps)**, prepare 116-124 ms, GPU 262-269 ms (card busy 53 % of it), copies 49-54 ms / 133 MB, copy-back 71-74 ms / 46 MB, ~13,100 draws. Recorded in [baseline.md](baseline.md).
- **Correction:** with the pool off (and the wait on), the same: 485-502 ms. The pool's earlier "~6 % faster" came from invalid windows of different scenes. Its value is stability without waits (frames in flight), not speed.
- **Unlike the prologue, the GPU phase dominates** (53 % of the frame). And a once-a-frame pass over the heap's buffer views (`draw.rs` `heap_buffers`, for shaders that pick buffers by index) takes **133 ms a frame**: ~110,000 views over 4,085 host copies spanning 2.4 GB; the full pass every 16th frame takes 389 ms, the others ~23 ms.
- **Profiles** (`bench.sh --perf`, new `scripts/perf-symbols.py`, which finds victory.exe's base by itself and symbolises it from the PDB): victory.exe 45 % of the process's samples, the title's own code 19 %. **Mutex lock/unlock/spin is ~14 % of all samples**, the same with the pool off, so it predates it. Branch records (`BENCH_PERF_G=lbr`, AMD LBR v2) place it near `shadow::find`, the descriptor slot paths and `Shadow::refresh`, but not exactly: under Wine an unlock enters ntdll, which fills the 16 records. Frame-pointer stacks (`BENCH_PERF_G=1` with a `force-frame-pointers` build) stop at Rust's std, built without them.
- `bench.sh --perf` leaves the game running at the end (seen 3 times); closed by hand with `victory-proton kill`. To fix.

**The open-world scene is two scenes: outside, then indoors**
- Draws in the world windows stepped down partway (16,000 → 2,400, 14,000 → 8,100, 14,000 → 6,100) with no input: the A taps stop at 50 s and the world comes at ~78 s.
- A screenshot at the window's end shows why: after ~45 s of the house from outside, the save **cuts indoors to Jason at the TV** (prompts "PHONE", "TV (X)"). The TV's video plays right. The interior shows **white speckles on the floor and blown-out areas**: a lighting fault for S.3 / phase 5, not stability.
- `bench.sh` takes `BENCH_SETTLE` (seconds after the world is detected, default 20). The open-world baseline measures the outside: `BENCH_SETTLE=5 scripts/trials.sh -n 3 -l world-out -s 30` ([bench.md](bench.md)). `trials.sh` now also takes a screenshot near the window's end.

**S.8 first step: the pool had stopped the sweep; fixed**
- The sweep lets a texture copy go only when `crate::host::references` says its title texture alone holds it (1). The pool's own reference made that 2 for every pool texture, so **nothing was ever let go**: that, not the title, is why the gate run reached 37,700 live textures and 18.9 GB of VRAM.
- Fix: the pool keeps a set of what it holds, and `references()` leaves its reference out. A copy the sweep lets go then reaches the pool's `collect` with no other holder, goes through `retire()`, and its block is reused.
- **Jason save, 2 runs of 120 s** (`bench/trials-…-s8-sweep`): both held, with blocks given back and reused (5-28 a frame). 25,800 live textures, 9.2 GB in use (against 37,700 and 13.3 GB), VRAM at most 17.8 GB. The heaps themselves are never released (11.5 GB made); trimming empty heaps is the rest of S.8.
- **Rule 7 holds:** the sweep's "unused for 5 s" now only chooses *which* copy to let go. Whether it is safe comes from exact holders (no list, no slot) and `retire()` (no running submission), and the block reaches the allocator only after that.

**Stability gate: 10 minutes in the open world with the new defaults (pool and slot wait), no loss; the picture more complete**
- Jason save, one run with a 600 s window (`bench/trials-20261126-002900-gate10`): held to the end. Frame 363 ms on average (2.77 fps), window marked `invalid` because the draws varied; not a speed figure yet.
- **The picture is more complete than with committed textures:** the house shows its painted boards, towels on the railing and wood detail, where the screenshots with committed textures and the wait showed it dark and flat. Textures that failed or went stale on fresh memory now hold. To be checked against S.3 (texture coherence).
- **VRAM:** 18.8-18.9 GB of 20.4 GB from the first minute on, GTT 4.5-5.9 GB (sampled every 5 s). The pool holds 37,700 textures, 13.3 GB in 14.6 GB of heaps. It did not grow during the static 10 minutes, but walking will: S.8 (bound texture memory) is the next task.

**The texture pool on by default; what holds 35,000 textures**
- 4 KiB small-resource alignment: 98 % of pool textures take it, yet use barely falls (12.3 GB for 34,700 textures, ~360 KB each on average). The bytes are real.
- **Live textures by references besides the pool's** (`texture pool:` frame line): none with 0, ~12,000-13,000 with 1 (their title texture), ~22,000 with 2 (also a descriptor slot, which keeps its last view alive by design), ~930 with more. Nothing is given back because the title never lets these textures go, not because the count is wrong. The committed path holds the same set.
- **New task (T/L): bound texture memory.** The live set grows as the world streams in (12-14 GB after a few minutes); a texture held only by its title texture and unread for a long time is the candidate, which is what the sweep does today by time. It needs an exact rule (rule 7), measured over a long run.
- `collect` took ~1 ms per call over 35,000 textures, at every submission; now at most every 250 ms.
- **Pool on by default** (`VICTORY_TEXTURE_POOL=0` turns it off; 4 GiB to start). The slot wait stays on as a safety net; it costs nothing measurable today, and goes once frames in flight need it gone.

**S.6 confirmed: textures in pre-mapped heaps remove the device loss at its root, with no wait, and faster**
- The first pool run (2 GiB, grown only when full) still lost 5 of 5: the pool filled, 16-60 textures a frame fell back to committed memory. A fault report placed the culprit in a 16 MB vkd3d block, not a pool heap.
- Fixed: the pool grows ahead of need (a new heap when ready free space falls under 1 GiB) and counts every texture it does not take. With 8 GiB to start: 0 committed, 0 not taken.
- **Jason save, no slot wait (`VICTORY_SLOT_WAIT=0`):** without the pool **8 of 8 lost** (`bench/trials-20261126-000514-control-nowait` and earlier); **with the pool 5 of 5 held** (`bench/trials-…-pool2-nowait`), at **406-453 ms a frame** (2.21-2.50 fps) against 469-489 ms with the wait and committed textures. Two of the five windows are valid (444, 453 ms); the others saw the scene change.
- **The cause is settled:** fresh memory mappings of textures made while work that can read them is in flight. The slot wait hides it by serialising; the pool removes it, which is what frames in flight (3.1) need.
- **Open before it becomes the default:** 32,000 textures live, 12.4 GB "in use", none ever given back. Small textures take a whole 64 KiB block each (95 % are under 1 MiB, many far smaller); D3D12's 4 KiB small-resource alignment would cut this. And why none is given back (held by descriptor slots that keep their last view, or a wrong count) has to be measured.

## 2026-11-25

**S.6 step 1 measured: the open world makes hundreds of host textures a frame, all over title memory; the pool built**
- New frame lines: host textures made per frame, by size and by the title texture's backing (`resource.rs` `TEXTURES_MADE`, `MADE_KINDS`).
- Jason save, settled in the world (`bench/trials-…-texsize`, `…-texkinds`): **60-635 host textures made a frame, 15-200 MB a frame**, 0.3-3.5 ms creating. About 95 % are under 1 MiB, the largest 10 MB. **All of them are textures the title makes "at a title address"** (over its own memory: streaming); none placed in a title heap, none committed. 10-50 % repeat the address and description of one made before. The sweep let none go in that window, so this is the title's own churn, not ours.
- Each of those got fresh committed memory: each was a chance for the S.1 race. A console texture is a view over memory; ours was an allocation.
- **`pool.rs` (`VICTORY_TEXTURE_POOL=1`, off):** host textures up to 64 MiB are placed in 256 MiB heaps made up front (2 GiB by default). A heap is used only after a submission sent after its creation has finished. First fit with alignment from `GetResourceAllocationInfo`. The pool holds one reference to each texture; at each submission boundary, a texture only the pool holds goes through `retire()` and its block comes back afterwards (design rule 7, exact lifetimes). The allocator's tests pass (run natively; the Windows test binary needs vkd3d's DLLs in its prefix).

**`VICTORY_SLOT_WAIT` on by default; the prologue had the bug too; S.6 designed**
- **Open world:** 5 of 5 held with the wait (`bench/trials-20261125-232200-slot-wait5` makes the fifth). Fast writes with the wait: 3 of 3 held (they lost the device before), but no gain in the open world (2.03-2.15 fps against 2.10), so they stay off.
- **Prologue, 3 runs each:** without the wait 1 of 3 lost, with a TCP fault on a texture (the S.1 kind); with the wait 0 of 3. Frame 358-364 ms with the wait against 359-372 without: no measured cost. Both sets of windows are marked `invalid` because the intro cutscene pulls the draws down during the window, so this is a rough comparison.
- **On by default** from here (`VICTORY_SLOT_WAIT=0` turns it off), per the rule that a switch becomes the default when it wins.
- **New, rare (S.7):** one prologue run with the wait lost the device on a different fault: **SQC (scalar data) reading address 0**, right after the sweep let ~5,000 texture copies go. 1 of 6 prologue runs with the wait. Not the S.1 mechanism (a null pointer, not a fresh texture). Watched; next step when it repeats is `VKD3D_CONFIG=fault` on it.
- **S.6 designed** ([design/s6-premapped-textures.md](design/s6-premapped-textures.md)): the wait does not survive frames in flight, and staging writes at submission boundaries is not enough either. The race is between a fresh memory mapping and GPU work sent before it. Host textures placed in heaps mapped ahead of time remove it in any submission model, and are the first piece of memory the layer owns (piece M).

**S.1 found: host descriptor slots written while the GPU runs (`VICTORY_SLOT_WAIT=1` holds the open world, 4 of 4)**
- **`VICTORY_LEAK_ALL=1`** (new: every resource and host descriptor heap the layer makes is held forever, `crate::host::pin` at all 18 creation sites): still **4 of 4 lost**. No title heap with a host heap was released in any run. So the invalid memory is not a use-after-free of anything we make (`bench/trials-20261125-225445-leak-all`).
- **VRAM is not short:** sampled every 0.5 s through a losing run, at most 8,252 of 20,464 MiB of VRAM and 4,344 MiB of GTT.
- **`VKD3D_CONFIG=fault` names the faulting resource** (vkd3d's address-binding tracker with `VK_EXT_device_fault`). In both reports:
  - the address is **inside a texture made about 2 s before** the loss was reported (512×512 BC1 with 8 levels at offset 0; 512×512×32 layers with 10 levels at offset 0x1e2000);
  - vkd3d: *"Fault address is currently mapped. This should not normally page fault."*
  - vkd3d reports a loss about 2 s after it happens, so **the GPU faults right after the texture is made**.
- **Mechanism:** host slots are made lazily (`descriptor/host.rs` `make()`), from the title's threads, into the shader-visible heap, **while a submission may be running**. Counted: **15,600-58,300 slots a frame written while a submission ran** in the open world. If the running work reads such a slot, it sees a resource made after it was submitted, whose page-table update only the *next* submission waits for. That fits every known fact: timing-dependent, hidden by anything that serialises the GPU, the open world (texture streaming) far more than the prologue, and a mapped address that faults.
- **`VICTORY_SLOT_WAIT=1`** (new): `make()` waits until no submission runs (at most 2 s). **Jason save: 4 of 4 reached and held the world** (13,000-13,500 draws a frame, 2.05-2.13 fps, valid windows, the picture right: house, light, water), against 1 of 2 with the counter only and 10 of 12 lost earlier tonight. Only 82-97 writes a frame had to wait; none gave up (`bench/trials-20261125-231520-slot-wait`).
- **Architectural reading (piece D, guarantee "a view never reaches past its resource"):** a shader-visible slot is GPU state. It may only change at a point where no work that can read it is in flight, as on the console, where the title's own fences guard it. The wait is the minimal form. Frames in flight (phase 3) need the proper form: slot writes staged and applied at submission boundaries.

**S.5 measured: mirroring byte-copied descriptors does not stop the open-world loss**
- Ported the descriptor part of fxceless `a26859f` (`VICTORY_HEAP_SYNC=1`, off by default; branch `s5-heap-sync`). New `scripts/trials.sh` runs one configuration N times unattended and classifies each run.
- **The title does copy descriptors as bytes, a lot:** per frame in the open world, 260-320 changed heap pages, 530,000-650,000 slots looked at, **~225,000 records new and mirrored**.
- **Loss rate, Jason save, same binary:** sync on **5 of 5 lost**; sync off 4 of 5 lost. The one survivor drew 2,935 draws a frame with an `invalid` window, so it was likely not in the world yet. In practice **every run loses the device within about a minute once the world draws 12,000-25,000 draws a frame**: a fast, reliable reproducer (`bench/trials-20261125-223829-hs-on`, `…-224417-hs-off`).
- **Kernel fault details** (`journalctl -k`): `GCVM_L2_PROTECTION_FAULT_STATUS 0x00701031`, client TCP, a read, `PERMISSION_FAULTS=0x3`, `MAPPING_ERROR=0`. The page has a page-table entry marked invalid: memory freed or never bound, not a wild address.
- vkd3d breadcrumbs (non-sync) print nothing on loss with GE-Proton's vkd3d-proton: the config is parsed but no report is made. A breadcrumb report needs our own vkd3d-proton build.
- **Reasoning for the next step:** the bug is timing-dependent (anything that slows or serialises the GPU hides it), the page is invalid, and it lies in no live copy of ours. That fits **memory released while the GPU still reads it** better than a view out of range, which would fault whatever the timing. `VICTORY_LEAK_RETIRED` covered only `retire()`.
- **Found in the code:** when the title releases a shader-visible heap, its `HostHeap` (the `ID3D12DescriptorHeap` and every resource its slots hold in `kept`) is dropped at once, with no fence waited for. `HostHeap` has no `Drop` and does not go through `retire()`.
- Next: `VICTORY_LEAK_ALL=1` (every resource and host descriptor heap the layer makes is held forever) and a log line when a title heap with a host heap is released.

**Decision: back to our base; the roadmap rebuilt around pieces that fit this machine**
- **Base:** `p1.8-fast-reads` (joshuu `003beef` plus phases 0, 1.7 and 1.8), not `fx`. On this machine it is twice as fast (317 against 610 ms per frame), and the stability bug is in both bases. `main` is advanced to it. `fx` stays as a reference for track U.
- Carried over from `fx`: the `bench.sh` check that marks a window `invalid` when its draws fall below half their top, and the journal entries.
- **README section 4 now opens with the target architecture:** seven pieces (M memory placement, W write tracking, B copy-back, L lifetimes, T textures, Q submission and fences, D descriptors and views), each with its guarantee, today's state, the target on this machine and its phases. Lessons from `fx` fix the order: **B (1.3) comes before Q (3.1)**, because overlapping while the copy-back is large puts it on the title's critical path.
- New: task S.5 (evaluate fxceless's descriptor mirroring `a26859f` as a cause of the wrong reads), track U (upstream intake, one measured change at a time), track L (soft-dirty / `userfaultfd` prototype, moved out of phase 4), track G (gates).
- **This machine:** ReBAR is on (BAR0 32 GiB over 20 GB of VRAM), so CPU-visible VRAM is not limited to 256 MB. Phase 2.1 also measures CPU write bandwidth into it.
- **Agents:** [AGENTS.md](../AGENTS.md) sets out the rules and roles of the pi subagents (scout, architect, worker, reviewer, analyst, upstream). They write code in worktrees and read results; game runs stay with the maintainer.

**fx on Linux: why it is slower, and a measurement trap**
- **The bisect was wrong in its premise.** The title stalls (~280-460 ms a frame) at every fxceless commit tested, back to `3bff182`. What made the old base fast is not in fxceless's line at all; it came from joshuu/myokex commits fxceless did not take. joshuu's remapping commits `948e9d3` and `476a020`, ported, change nothing.
- **fxceless's new defaults** (`c96b4f2`):
  - relay on (copy-back by its own thread, stage 3 sending ahead);
  - wide copy-back;
  - card copies from 1 MiB to 1,024 MB;
  - 256 lists in the air.
- **With the relay, the title waits for the relay thread's landing.** Per frame it waits 55-72 ms for the card and 100-122 ms for fetches of card copies' pieces, 158-176 ms landing in all. The card is idle while the title waits.
- With the old defaults (`RELAY=0`, `WIDE_BACK=0`, card 16-512 MB, 128 lists): **7.5 s a frame** (copy-back 6.3 s). fxceless rebuilt the copy-back around the relay, so it cannot simply be turned off.
- `VICTORY_QUEUE2=1`: 623 ms. `VICTORY_ASYNC_SUBMIT=1`: 660 ms. No gain.
- **Trap:** a `CARD_BUFFERS_MIN_MB=16` run read "140 ms" because the game went to a 199-draw screen (pause?) during the window. In the world it was 1.5 fps, the same. `bench.sh` now marks a window `invalid` when draws fall below half their top.
- `waitstat` on fx: the same waits as the old base (one thread ~700 ms/s at `title+0x4e7f358`). The title's `mwaitx` wait is native on this Ryzen and not seen.

**Branch `fx`: the base moved to fxceless/main (`d510ee4`)**
- Carried over by hand (the old commits do not apply):
  - `VICTORY_SKIP_PIPELINES` and `VICTORY_KEYS` in release builds; fxceless still has both diagnostics-only;
  - `crate::fastcopy` (reads only; writes off);
  - docs and `bench.sh`, which now reads the log's frame-rate lines when there is no frametime CSV (fxceless has none).
- Not carried yet: `medir` (waitstat, CSV columns), `F1_OWN` / `F1_CARD_MERGE` (fxceless has `VICTORY_BACK_OWN_VIEW` for the same purpose), and the diagnostic switches of the device-loss hunt.
- fxceless has no `victory warm`. The first run after a translator change is the warm-up.
- **Scene A on Linux, fxceless defaults: 610 ms per frame (1.7 fps)**, against 317 ms on our old base. The layer is faster (prepare 62 ms, GPU 52-66 ms), copy-back slower (145 ms). But the card is idle ~420 ms a frame **"while the title had not submitted"**: the title itself stalls, where on the old base it waited ~2 ms.
- fxceless's 10 fps switch set (`JOIN_PARTS`, `BACK_OWN_VIEW`, `INDIRECT_GATHER`, `CARD_REHASH_RUNS`) changes nothing here: 603 ms, title idle ~427 ms.
- fxceless also reports washed-out frames in ~6 of 21 runs with no switch (our white picture) and an open-world device loss "with the card out of room for new textures". Both are base bugs, not Linux ones.
- **Bisect under way** (`2119b0e`, good per the old base, .. `fxceless/main`) on how long the title waits per frame. It runs in a worktree, unattended (`scratchpad/bisect/step.sh`).

**Our base is behind: fxceless moved a long way today**
- `RADV_DEBUG=allbos`: still lost 2 of 2, so not memory missing from a submission.
- Release with the skip fixed and `VICTORY_DRED_DEVICE=both`: 1 of 2 lost; the other loaded with the 3D picture blown out to white (HUD right; screenshot). That looks like auto-exposure fed a wrong value. Forced held submissions do not correlate with the losses.
- **fxceless/main has 15 commits past our base (joshuu `003beef`, which is fxceless + myokex + joshuu).** It now carries the myokex view guard and joshuu's DirectStorage fix itself. Among them:
  - `a26859f` *"The first room renders right: byte-copied descriptor records are mirrored"* (descriptor records the title copies as bytes were not mirrored: wrong data in slots);
  - `f85101b` a submit thread of the layer's own (`VICTORY_ASYNC_SUBMIT`) and a copy queue for fetches (`VICTORY_QUEUE2`), i.e. our phase 3;
  - `465b94f` / `3973179` **9.6-10.3 fps in the first rooms** on an RTX 4070.
- Upstream VIctory has 402 commits past our base: barriers, memory-value waits, texture work.
- **Consequence:** phases 1-3 duplicate work the forks are doing faster, on a base a day old. The descriptor fix may be exactly our wrong reads. Proposed: rebase onto fxceless/main, carry over the Linux-specific pieces (fast reads under Wine, the skip fix, `VICTORY_KEYS` in release, bench tooling), then test both scenes on Linux.

**Roadmap reordered: phase S (stability foundation) before phases 2-3**
- The user's point: the prologue is a closed space, and the game has to work anywhere. Every speed-up ran into the same correctness bug, and phase 2 rewrites the paths where it lives.
- Phase 1 stays (small, measured, harmless), except fast writes, which stay off.
- New phase S: wrong reads / device loss (Jason as reproducer), exact lifetimes, texture coherence, and an open-world reference scene. Exit: release loads Jason 5 of 5 with the right picture.

**Found: `VICTORY_SKIP_PIPELINES` only worked in diagnostics builds (fixed)**
- `pipeline::is_skipped` and the three checks in `draw.rs` were `cfg(feature = "diagnostics")`. **Every release run of ours, and of the launcher's fork profiles, never skipped `fc5e66f8f873d3bb`**, although the profile set it. Fixed: the skip works in every build. The 24 Nov finding that the GPU faulted ~9 minutes in without the skip came from release builds, where the skip did nothing.
- It does **not** fix the saved game's loss alone: 2 runs after the fix, one lost the device, one did not (it was still loading at 1,503 draws).
- Found by listing every diagnostics-only statement that does more than measure. The other diag-only paths are notes, traces and counters.
- Also: texture views from `make()` never reach past their host texture (0 in a run). `VKD3D_CONFIG=fault` gave no report and no loss in that run, but a white-and-black picture, like release with DRED.

**Device loss: the fault is not in a buffer copy; likely a texture read out of range**
- `VICTORY_PAD_KB=256` (256 KiB more at the end of every host buffer made in `host.rs`): still lost 2 of 2. The faults were at `0x8003bb7f0000` and `0x800410e00000`.
- **`VICTORY_ALLOC_DUMP=<file>`** writes, on device loss, every live host copy of a title buffer and every window: GPU address, bytes, title address, kind.
  - vkd3d's addresses are sign-extended (`0xffff8001…`); RADV prints the low 48 bits. Compare `addr & (2^48 − 1)`.
- One run: fault at `0x80035fac4000`. The 4,070 live copies (3,402 buffers, 658 windows, 10 card copies) span `0x800112200000`-`0x800332200000`. **The fault is ~740 MB above the highest, in no copy of ours.**
- The faulting client is TCP (texture unit), so it is most likely a **texture read**: images live elsewhere in the GPU's address space. Retired textures were kept alive in the leak runs, so not a freed texture.
- **Leading hypothesis:** a shader resource view that describes more mips or array slices than the host texture has (textures streamed in part, mips picked from the first one present). On the console those reads land in title memory; here they run past the image's allocation. Next: check every texture view made against its host texture and count the mismatches.

**Device loss: two more hypotheses ruled out (saved game, release build)**

| Switch | Lost | Furthest draws a frame |
|---|---|---|
| `VICTORY_LEAK_RETIRED=1`: nothing passed to `retire()` is dropped, nor texture staging (×3) | **3 of 3** | 12,246-14,399 |
| `VICTORY_SERIAL_OPS=all`: a global UAV barrier before every draw, dispatch and indirect command (×2) | **2 of 2** | 7,375-13,498 |

- Not a host object freed through `retire()` (shadows, texture copies, slot views, staging).
- Not a missing barrier between commands on the GPU.
- **Not caused by phase 1:** with `VICTORY_F1_OWN=0 VICTORY_F1_CARD_MERGE=0 VICTORY_F1_FAST_READ=0` the save still lost 2 of 2. The base has the bug.
- Not the copies in the card's memory: `VICTORY_CARD_BUFFERS=0` still lost 2 of 2.
- Not DirectStorage's parallel loading: `VICTORY_DSTORAGE_WORKERS=1` (one worker instead of 24) still lost 2 of 2.
- What did avoid it: **the CPU waiting for the GPU to go idle after each submission** (`RADV_DEBUG=hang`, `breadcrumbs_sync`), and the diag build with the runtime's DRED. So something the CPU side touches while the GPU still works. Candidates:
  - DirectStorage's worker threads (`victory:disk`) writing destinations during loading;
  - work vkd3d keeps in flight past our wait;
  - a GPU read out of range on data not complete yet.
- **Fault addresses so far:** `0x80038cc00000`, `0x80036a000000`, `0x8003b080e000`, `0x8002f0c73000`, `0x800335e04000`, `0x8003378e1000`, `0x80038cc00000`. Several sit exactly on 1-2 MB boundaries: a shader reading past the end of an allocation into unmapped address space.
  - Reads through descriptors are bounds-checked (D3D12 robustness), but raw addresses are not: root descriptors, and title memory read by address (`memory.rs`).
  - Whether a read past the end faults depends on what lies after the allocation, which changes with timing and allocation order. That would explain why DRED or the diag build hide it, and why the one release run that lived drew an almost white picture (garbage instead of a fault).
  - **Leading hypothesis now: an out-of-range raw read.**

**The saved game's device loss: what keeps it alive (all runs load the save, `royale-save`)**

| Build and switches | Device lost | Notes |
|---|---|---|
| release (×4, also with every sweep off) | **4 of 4** | ~45-60 s in, while loading the world |
| release, `VKD3D_CONFIG=…,breadcrumbs` | yes | vkd3d printed no breadcrumb report |
| release, `RADV_DEBUG=hang` | no in 108 s | killed from outside |
| release, `VKD3D_CONFIG=…,breadcrumbs,breadcrumbs_sync` | no | 0.2 fps, reached 11,878 draws a frame |
| release, `VICTORY_DRED_DEVICE=faults` or `=breadcrumbs` (DRED setting alone) | yes, both | |
| release, `VICTORY_DRED_DEVICE=both` | **1 of 2** | the run that lived: 2.9 fps, 13,405 draws, **picture almost white, Jason a shadow** (user) |
| diag, no DRED | yes | |
| diag, `VICTORY_D3D12_DRED=1` (×4, with or without our timestamps, markers or object names) | **0 of 4** | 2.3-2.7 fps, **picture right**: face, skin, water, clouds, minimap (screenshot and user) |
| diag, DRED without the runtime settings (`VICTORY_DRED_SETTINGS=0`) | yes | |

- Every release-only code path (`cfg(not(feature = "diagnostics"))`) was read: all equivalent (counters, captures, log text). The builds differ in timing, not in what they draw.
- **Conclusion:** the loss depends on timing, as with fast writes and the forks' late wait. What helps is slowing things down or serialising the GPU (`breadcrumbs_sync`, `RADV_DEBUG=hang`), plus the extra work of the diag build with the runtime's DRED on. It is the main blocker for any speed-up. Whether it is a missing GPU barrier or a use after free is not settled.
- The white picture in the release run that lived is a second symptom to look at once it stops crashing.
- **New switches:**
  - `VICTORY_DRED_DEVICE=breadcrumbs|faults|both` (any build);
  - `VICTORY_DRED_SETTINGS=0`, `VICTORY_DRED_NAMES=0`, `VICTORY_NOTE_STAMPS=0`, `VICTORY_NOTE_MARKERS=0` (diag).
- **Tooling:** `bench.sh` sees diag builds. In diag runs the frametime CSV is not where `--auto` looks; to fix.

**Unattended runs, and a reproducible device loss**
- **`bench.sh --auto` works:** it presses A at the start screen (`VICTORY_KEYS`, now in release builds), the intro plays through, the world is reached at ~105 s, and it measures with nobody at the keyboard. Current defaults: **327 ms per frame** (auto-keys run, ~5,080 draws, stable). Holding A does not skip the intro.
- **The user's saved game (Jason, in the world)**, installed in a separate launcher profile `royale-save` (own data folder, same build):
  - **loses the device while loading, 3 of 3** (GPUVM fault ~45 s in, then the title's own crash handler), with no fast writes and no texture freeing in the trail;
  - vkd3d's `breadcrumbs` gave no report in this GE build;
  - with `RADV_DEBUG=hang` (a wait after every submission) it **ran 108 s without a fault**, until something outside sent SIGTERM (another session was using the launcher at the same time).
  - The waits remove the loss, so this points to a race in timing again, not a broken command. It is a reliable test case for the lifetime work, in a heavier scene than the prologue.
- **Caution:** two sessions launching the game at once kill each other's wineserver. Coordinate before measuring.

**Freeing vs fast writes: three more runs**

| Run | Lost device | Picture (user) | ms |
|---|---|---|---|
| fast writes, `VICTORY_TEXTURE_FREE=0` (a) | no | lighting missing, **Lucia's orange shirt black** | 289 |
| fast writes, `VICTORY_TEXTURE_FREE=0` (b, moving) | no | textures more broken and flat | 327 |
| no fast writes, freeing aggressive (`IDLE_MS=500`, `IDLE_FRAMES=1`), breadcrumbs | **no** | — | 314 |

- **Freeing alone does not lose the device, even when aggressive.** The losses need fast writes **and** freeing together.
- **With fast writes and no freeing, the picture breaks.** With freeing on, it looked as usual: textures recreated from title memory come back right.
- Open question: does the picture break because freeing is off (textures go stale, and freeing was hiding it), or because fast writes write something wrong? To settle it: `VICTORY_TEXTURE_FREE=0` **without** fast writes.
- Note: the sweep's trail lines reach the log only when the device is lost, so a run without a loss does not show what was freed.

**Correction: `VICTORY_TEXTURE_SWEEP=0` turns off two things**
- It stops `resource::let_go_idle` (freeing host texture copies), **and** `HostHeap::texture_sweep`, a repair pass. That pass finds descriptor slots left showing nothing (a texture a shader would read as black) and puts them back in the upload queue.
- So the missing lighting, window and models in the sweep-off runs were most likely the repair pass being off, **not** stale textures. "Bug 2" as written above is unproven.
- What texture.rs does say about staleness: *"Only textures read through descriptor tables are brought up to date (directly indexed views are not known per draw)"*. Textures picked by index get rechecks at 4-400 frames after upload (joshuu, `VICTORY_HEAP_RECHECK`), plus remap notices.
- **New switches:**
  - `VICTORY_TEXTURE_FREE=0` keeps every host copy alive and leaves the repair pass on;
  - `VICTORY_TEXTURE_IDLE_MS` and `VICTORY_TEXTURE_IDLE_FRAMES` (5000 and 4) make the freeing aggressive, to reproduce the lifetime bug on purpose.
- The `crumbs-a` run (fast writes plus vkd3d breadcrumbs) got through without a loss, so there is nothing to read yet.

**Sweep test: two texture bugs, not one**

| Run (fast writes on) | ms per frame | Device | Picture |
|---|---|---|---|
| sweep on (3 runs) | 308 (when it got through) | **lost 2 of 3** | — |
| `VICTORY_TEXTURE_SWEEP=0` a | 307 | ok | — |
| `VICTORY_TEXTURE_SWEEP=0` b | 303 | ok | **lighting missing** |
| `VICTORY_TEXTURE_SWEEP=0` c | **294** (3.4 fps) | ok | **lighting, a window and many models missing** |

- **Same draws in every run** (~4,980 per frame, ~340 dropped), so the game emits the same work. What is missing is wrong **data** (stale textures or buffers), not missing draws.
- **Bug 1 (lifetime):** with the sweep on, a faster frame lets it free host texture copies that the GPU still reads. With the sweep off, no device loss in 3 runs. Every slot path I read (`HostHeap::keep`, `mirror`, `make_waiting`) holds its texture, so the unheld reference is somewhere else. Next: let the GPU name it (`VKD3D_CONFIG=descriptor_heap,breadcrumbs`, and the trail).
- **Bug 2 (staleness):** with the sweep off, textures go stale and stay so. The sweep was hiding it: copies it frees are made again later from title memory, fresh. So change detection for textures misses some updates. This is a phase 1-2 correctness problem (`texture.rs`, `VICTORY_HEAP_RECHECK`), and probably related to the flicker and missing pieces seen before.
- Fast reads, own write-back and card merge stay on. Fast writes stay off until bug 1 is fixed.

**Fast writes: 2 device losses in 3 runs; a lead in the texture sweep**
- `VICTORY_F1_FAST_WRITE=1`, three runs:
  - **lost the device twice**, as loading ended and a few seconds into the first cutscene (GPUVM faults on host GPU addresses, TCP client: a texture read);
  - **when it gets through, copy-back falls from 69 to 53 ms** (−23 %) and the frame from 317 to 308 ms.
  - Without fast writes: no device loss in ~25 runs.
- **The crash log's trail:** 1.4 ms before the loss, the sweep (`resource::let_go_idle`) let go of **2,196 host copies of textures the title fills (1.58 GB)**, *"each held by its title texture alone (no list, no slot)"*. The fork's late-wait crash of 24 Nov also had a sweep letting go a few ms before.
- **Why speed would expose it:**
  - a texture copy is let go after 5 s **and** 4 frames without use;
  - during loading, frames take seconds, so a faster copy-back reaches the 4 frames sooner;
  - textures loaded long before become eligible right as the cutscene starts using them.
  - In `Host::submit`, `sweep()` runs before `descriptor::make_waiting()` makes the slots the title wrote since the last submission. A texture a pending slot is about to show may still count as held by no slot.
- **Decisive test:** `VICTORY_TEXTURE_SWEEP=0` with fast writes on. If the losses stop, fix the sweep (ordering, or count pending slots as holders) instead of hiding it. This lifetime bug would also hit phase 3.

**Phase 1.8 measured: fast reads work, −10 % frame time**

| Scene A | baseline | 1.7 (own write-back) | 1.8 (fast reads) |
|---|---|---|---|
| ms per frame | 354 | 348 | **317** (3.2 fps) |
| prepare | 124 | 129 | **102** |
| copies | 63 ms | 66 | **49** |
| GPU | 159 | 143 | 142 |
| copy-back | 67 ms | 71 | 69 |

- **17,190 reads of title memory per frame** (288 MB), each one a system call before. 20 per frame meet a missing or guard page and take the kernel path.
- No crash after the guard-page fix.
- **Copy-back did not move.** It writes with `NtWriteVirtualMemory` (`host.rs::write_mapped`), the same per-call cost in the other direction. **Next: fast writes**, the same technique.
- **Fast writes (1.9):** `write_mapped` tries the same routine first (`VICTORY_F1_FAST_WRITE=0` turns it off). Unit tests under wine:
  - a write to a missing page and to a read-only page fails to the kernel path;
  - **a fast write into write-watched memory is reported by `GetWriteWatch`**, so Wine tracks our writes like the title's and the journal stays correct.
- **First run with fast writes: device lost as loading ended.** A GPUVM fault at `0x80038cc00000` (a host GPU address, not title memory), TCP client, ring timeout. It happened where the forks' late-wait switches also faulted.
  - **Ruled out:** a write-watch difference. A test shows Wine's kernel write (`WriteProcessMemory`) also marks the page written.
  - The bytes written are the same either way. Leading hypothesis: the faster copy-back changes timing and exposes an existing GPU-side race (resource lifetime around loading).
- **Decision:** fast writes off by default (`VICTORY_F1_FAST_WRITE=1` turns them on); fast reads stay on. To settle it: two more runs with fast writes on. If they fault again at the same point, take a diagnostics build with `VICTORY_D3D12_DEBUG=1` to name the command.

**Phase 1.8: profile of the submitting thread, and fast reads (branch `p1.8-fast-reads`)**
- **Profile** (`bench.sh --perf`, scene A, 30 s). The submitting thread is busy about half the time (~170 ms per frame):

  | What | Share of the thread |
  |---|---|
  | `memcpy` / `memcmp` in ucrtbase (copies, and the `F1_OWN` compare) | ~29 % |
  | **Wine's `ReadProcessMemory` path:** `virtual_check_buffer_for_write` ~10 %, its libc copy ~10 %, syscall dispatch ~3 % | **~23 %** |
  | `RtlQueryPerformanceCounter` (clock reads) | **~8 %**, caller unknown (no call graphs) |
  | VIctory's own code: `watch::writes`, B-tree walks of the journal, blake3, merges | ~22 % |

  - Wineserver: 0.08 cores, no memory-read requests. Reads stay in-process.
  - Symbolising PE code: victory.exe was at `0x6ffffd670000` in that run; `llvm-symbolizer --obj=victory.exe` takes `0x140000000 + (address − base)`. Find the base by scoring candidate bases on plausible names.
- **`crate::fastcopy`:** title memory is read with `rep movsb` in a small assembly routine. A read of a missing page faults, and our vectored handler (first in the chain, ahead of the crash reporter) moves the thread to the routine's failure exit; `read()` then falls back to `ReadProcessMemory` as before. This is the kernels' technique for copying from user memory. `VICTORY_F1_FAST_READ=0` turns it off. A unit test (mapped page, reserved page, reuse after a fault) passes under wine.
- New CSV columns: `fast_reads`, `fast_mb`, `fast_faults`.
- **First run in the world: crash** `0x80000001` (`STATUS_GUARD_PAGE_VIOLATION`) at `victory_fast_copy_at`, reading `0x11000`. Some title range holds a guard page. That raises its own one-shot exception, not an access violation, and the fault clears the guard. **Fix:** the handler also takes guard-page faults at our instruction, puts `PAGE_GUARD` back on the page, and fails the copy to the kernel path. A second unit test covers it, and both pass under wine.
- **Next:** measure scene A. Then find who reads the clock so often.

**Phase 1.8 started: fast reads of title memory**
- **Why:** half the bytes, same copy time, so the cost is per read, not per byte. All reads of title memory go through `ReadProcessMemory` (`host.rs::read`), one call per piece, with fallbacks to 64 KiB and 4 KiB pieces and `VirtualQuery` when a page is missing.
- **Plan** (README 1.8): profile first, then plain `memcpy` for ranges known to be readable, then a fault safety net, then batching.
- First `--perf` run: **wineserver used 0.08 cores, with no memory-read request among its hot symbols**: title memory reads do not go through the server, so their cost is inside the game process. The game's own profile came out empty; perf errors now go to `perf.err`.
- `bench.sh run --perf` records the game's and wineserver's user-space profiles during the window.

**Phase 1: own write-back measured, both switches on by default**

| Scene A | baseline | `F1_OWN` | `F1_CARD_MERGE` | both (×2) |
|---|---|---|---|---|
| ms per frame | 354 | 344 | 365 | 348 |
| MB uploaded per frame | 216 | 108 | 230 | 110-113 |
| MB sent up to the card | 93 | 54 | 59 | 18 |
| hash passes | 558 | 536 | 144 | 102-104 |
| GPU ms | 159 | 149 | 155 | 143-144 |
| copy time | 63 ms | 64 | 67 | 66 |

- **About 100 MB a frame of uploads were our own write-back.** 95 % of those pages compare equal and are now skipped (`own_skip_mb` 100-104, `own_copy_mb` 5).
- The picture is unchanged, by the user's eye.
- **Copy time did not drop with half the bytes.** The cost is reading title memory, which the compare has to do too. Every read goes through the kernel (`ReadProcessMemory` / `NtReadVirtualMemory`) so that pages with no backing do not crash. This confirms design rule 1 of [memory.md](memory.md) by measurement.
- **Decision:** both switches are on by default (`=0` turns them off). Next: a fast path for reading title memory.

**Phase 1: first real change (branch `p1.1-census`, both switches off by default)**
- **Finding from reading the code:** the copy-back writes what shaders wrote into title memory. Write watch, or the view guards, then report those pages as written by the title, so the next refresh copies them again. For copies in the card's memory, `card_merge` also marks the whole merged range to go back up to the card and be rehashed, although the card already holds those bytes. The code says so itself: *"Our write shows as a title write at the copy's next refresh, which copies those pages once more (some dozens of MB a frame)"*.
- **`VICTORY_F1_OWN=1`:** a copy remembers the pages it wrote back. At its next refresh it compares each such page with title memory (one read) instead of copying it again. If they are equal, nothing is copied or sent up. Correct by construction: a page the title wrote meanwhile differs and is copied as before. New CSV columns `own_skip_mb` and `own_copy_mb`.
- **`VICTORY_F1_CARD_MERGE=1`:**
  - after a merge, only the bytes where the merge kept the title's bytes go back up to the card;
  - the compare after the lists stores its hashes (`compare = 2`, as the relay already does), so pieces that were not sent up are not found differing again.
- Builds. The `shadow::` unit tests pass under wine. `watch::tests::writes_in_partly_committed_view_blocks_are_reported` fails only when run together with other tests (shared state); alone it passes, and the file was not touched.
- **To measure:** scene A with each switch and with both, against the baseline, checking the picture for corruption.

**Repository**
- The documentation moved to English. The Spanish originals are in `docs/history/es/`.
- A collaborator joins, working on a separate branch.

**Heads investigation: parked**
- 18 runs in total, heads in 3. It is all or nothing per run, and the game does not emit the head draws.
- Ruled out: camera distance (H1), skipping the cutscene (H3), the `fc5e66f8` skip (H4a). In-place reads gave faces in only 1 of 3 runs.
- Full record in [memory.md](memory.md) §7. To be re-checked after each phase.

**Phase 0 measurements**
- **Ceilings:** no copies gives 5.9 fps and 170 ms per frame (×2). Plain copies crash on a page with no backing.
- **The probe:**
  - private memory in place works on vkd3d-proton with RADV;
  - the page pool through sections does not;
  - reading in place over PCIe doubles GPU time.
- **Census:** 72 % of the bytes shaders read are in the page pool.
- **Baseline A:** 2.8 fps, 356 ms per frame, less than 1 % variation between three runs. Baseline B: 2.6 fps, 386 ms.
- All in [memory.md](memory.md) and [baseline.md](baseline.md).

**Phase 0 tooling**
- **`VICTORY_MEDIR=1`** turns on, in the release build:
  - the xmem remap counters;
  - `waitstat`, the title's waits by thread and caller;
  - the CSV columns `remap_ms`, `maps` and `unmaps`.
- **`VICTORY_COPY_MODE`** now works in any build (`medir::copy_mode`).
- **`scripts/bench.sh`** and the `royale` launcher profile, which builds `~/src/victory-royale` itself.
- **`warm --no-pipelines`:** 25,700 shaders translated, 58 refused, 101 s. The variant with pipelines hangs on RADV (seen with joshuu).

**Base**
- `main` is JoshuuDev/victory-amd `003beef`.
- victory-royale (ligmaSec) is kept as `royale-base`: merging it gave 494 conflicts in 63 files.
