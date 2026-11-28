# What to do next, in order (for the next session, unattended)

Written 27 Nov 2026, ~05:20, when the user ran out of usage. The user asked: **continue automatically; between big changes leave checkpoints ready to test the next day, so that a break can be undone and compared; document everything in detail.**

Read first: [STATUS.md](STATUS.md) (setup, how to play, every tool), [CHECKPOINTS.md](CHECKPOINTS.md) (the states saved so far), [k-journal.md](k-journal.md) (the dated log), README section 6 (the translator work, step by step).

## Rules

1. **Never write to the game folder**; never start the game while the user plays (`scripts/ktrials.sh` refuses if a game runs). Close games by pid, never `pkill -f` with a pattern that may match your own shell.
2. **After every big change: a checkpoint** — `scripts/checkpoint.sh NAME "what, measured how" [COMMIT]` (tag `cp/NAME`, a ready-to-run build). Big = anything that changes what normal play runs (the layer, the launcher, the wineserver) or a P.4 family turned on by default. Translator-only steps (P.4 off) are covered by the harness and the unit tests; checkpoint them every few steps anyway.
3. **Measure the same way every time:** `scripts/ktrials.sh -n 3 -l LABEL -s 40` (each run from the original save, `K_SAVE`), compare medians; P.4 runs with `K_CACHE=…/cache-p4` (the P.4 scripts set it) so normal play's pipeline library stays clean. Decide on 3+ runs; judge broken pictures by screenshot contrast and the world-entry window region, never by draws a frame.
4. **Translator steps:** `cargo test --release -p victory-shader` green, then the harness (`cargo run --release -p victory-shader --example p4_fspx -- ~/Games/victory-data/pso-dump --ok bench/p4-translate-ok.txt`), then commit with the count in the message, then a row in README section 6 and a line in the journal.
5. **Nothing public** (no PR, issue or comment) unless the user asks; the private repo is fine. Commits with `228639058+datingesp-hash@users.noreply.github.com` where public.
6. Talk to the user in Spanish; repository text in English.

## Queue

1. **Checkpoint the current state** (done if `CHECKPOINTS.md` lists `2026-11-27-translator-1105`).
2. **The 3 translator panics** ("forward reference outside a phi": `218_0000000f003ccf00`, `…3f7a00`, `…3fc100`). The writer prints `FORWARD-REF in <op> reads <op>` before panicking: an `LShr` reads a `Shl` emitted after it. Find which lowering emits that pair (search `Bin::LShr` uses whose operand is built in another block or later), fix, keep the harness at 0 panics.
3. **Next blockers by size** (run the harness for the current list): "a descriptor joined too often" (249; raising `MAX_JOINS` to 12 did not help and tripled harness time — find what the joins are first), "an opaque lane mask" (206), "a descriptor changed after loading" (147, `VICTORY_WHY_DESCRIPTOR=1`), "a descriptor joined in several blocks" (118), "a descriptor's address or format read as data" (105), "a descriptor outside the heaps" (101), "a descriptor built around a table in the code" (80), "a typed buffer store of some channels or at an offset" (80). For each: census, read two real programs (`K_FULL_ERRORS=1` on a one-program directory, offsets mapped with `llvm-mc -show-encoding`), implement exactly that shape, refuse variations.
4. **Descriptor tables addressed from constant data**: done 27 Nov, but **its 46 programs were a misreading (step 18, README): it is now limited to addresses loaded from memory (1,202)**. First fix (see `p4-house-bug.md`, "What remains") the root constants' SGPR count and the heap's user SGPR (read from the root signature), which would bring them back properly. Every newly whole family: check in game with `scripts/p4-family-bisect.sh` before trusting it; next big ones: "a descriptor joined too often" (317: de-duplicate sources at joins instead of raising MAX_JOINS), "an opaque lane mask" (208), "a descriptor joined in several blocks" (146), "an image store of some channels" (136, mostly the deferred alpha-only stores). (Was: designed, README section 6): the layer passes the view heap's console start as a host word (`RootType::Other(USER_VIEW_HEAP_START)` in `host_word`), the translator computes `(address − start) / 32` where a descriptor is loaded through an address read from memory. Measure: view heap `0x0b3c0000`-`0x0d9c0000`.
5. **Families:** after each few steps recompute whole families (README section 6 has the script idea; `bench/p4-full-families.txt`). When a large family (`0x2996b5a0` 525, `0x1cd48770` 330, `0x2996b900` 246) becomes whole: test it alone in game (`VICTORY_NATIVE_COMPUTE=1 VICTORY_NATIVE_COMPUTE_ONLY=<its list>`, 3 runs with `K_EARLY_SHOT=1 K_SHOTS=2`, screenshots compared with plain runs), and if it holds, a checkpoint with it on.
6. **Turning P.4 on by default** only for families that hold, through a list file in the repo; never all translatable programs (partial chains break the picture).
7. Then **0x1c8** (graphics, 10,206 descriptions) the same way.
8. **Verify the shortcuts in game (`p4-shortcuts.md`), 1 and 2 first.** Also done (steps 24-28, 1,304): the high-half wall and partial image stores; 1,239.** Next wall: "a descriptor's address or format read as data" (481). Earlier note: **Next translator wall (27 Nov evening): 459 programs at "an address with an untracked high half"**: 64-bit XOR register swaps between misaligned pairs across joins. XOR swaps resolve now (step 24); the high dword comes back from an SGPR spill lane written on several paths: prove the high half only over the paths that reach the access, as `classify` does with `known`/`lanes.reach` (README section 6, steps 24-25). Then "an opaque lane mask" (208).
9. **Re-test family 0x33cb3490 (the culling, `docs/royale/p4-house-bug.md`)** whenever its chain's families grow.
10. **Image stores of the alpha channel alone** (`dmask:0x8`, 69 programs), deferred on purpose: README section 6 "Deferred on purpose" has why and how (read-modify-write only where each texel has one writing thread; raw atomics for linear textures).

## Other open threads (lower)

- `VICTORY_THP=0` against on, 3 runs each (do huge pages help on this machine?).
- Belobug's texture uploads in the submission prelude (`5d6bd84`) for the load-time flicker.
- The author's next fixes (known_holes under Wine, PORT_DESCRIPTORS with port 1, COPY_COMPARE on BAR): `git fetch kroeeee`, merge, checkpoint, measure.
- The world-entry load varies run to run (20-30 s of flicker and black patches in some runs, clean in others).

## Reference material (28 Nov)

The user has legitimate access to the console SDK and keeps it locally. Rule: **reference only**. Never copy its code, headers, internal names or documentation text into this repository, PRs or anything public. Findings are written in our own words, as observations about how the console behaves, and our own code implements them. Nothing from the SDK is committed.


## State after the unattended run of 28 Nov (to ~11:00)

**Translator: 1,552 of 2,680 0x218 programs** (1,432 when the user left).
- Unit tests: 216.
- Differential tests against the console's own compiler: **33/33 compute cases equal**.
- **13 good families, 228 native programs**, checked in game by automatic runs; the newest is `0x302dbfc0`, 125 programs.
- Checkpoints: `2026-11-28-translator-1447`, `2026-11-28-translator-1530`.

**Done in the run** (README section 6, steps 45-51; each with real data or a proof):
- Heaps placed from the title's root signature flags. Collected in game for all 2,680 programs and checked against the code both ways.
- The friend's lane-mask patch, reviewed and merged.
- Waterfall proof: the RDNA2 wrexec retry and scalar accesses.
- `ds_append` / `ds_consume`, checked with the console compiler.
- DPP compares.
- Buffer loads from tables in the code (+80; console-compiler case).
- `buffer_atomic_sub` (console-compiler case).
- Constant BVH descriptors.
- `saddr` global accesses (by the ISA).
- Integer `clamp` (console-compiler case).
- The harness now measures with the game's host settings (RDNA, waves of 32 and 64).

**What blocks most of the rest is the lane analysis** (`structure.rs`, `emit/wave.rs`): a value that is a descriptor dword on one path and a lane mask on another, joined. See `p4-handoff-lane-masks.md`.

| blocker | programs |
|---|---|
| a descriptor's address or format read as data | 765 (mostly waterfall loops whose EXEC has no mask, and joins with dead descriptor dwords) |
| a sampler's descriptor read as data | 90 (EXEC restores through reused registers) |
| a whole-wave section step | 63 |
| lane masks | 60 + 57 + 45 |
| a constant buffer's descriptor read as data | 34 (EXEC masks through reused registers again) |

**Next, in order:**
1. **Lane masks for the EXEC the waterfall loops write** (`structure.rs`). That proves those loops and kills the dead back-edge values by construction: the largest single gain.
2. **A proof that a dead descriptor dword reaching an EXEC restore or merge through a join is never read** on the paths that use EXEC. The runtime marker (`VICTORY_DEAD_MARKERS`) is ready to check any candidate rule in game.
3. **The whole-wave section steps** with `src_scc` and a one-lane EXEC (`wave.rs`).
4. **Then 0x1c8** (graphics, 10,206 descriptions).
