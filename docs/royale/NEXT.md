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
4. ~~**Descriptor tables addressed from constant data**~~ **done 27 Nov (1,248)**; next big ones: "a descriptor joined too often" (317: de-duplicate sources at joins instead of raising MAX_JOINS), "an opaque lane mask" (208), "a descriptor joined in several blocks" (146), "an image store of some channels" (136, mostly the deferred alpha-only stores). (Was: designed, README section 6): the layer passes the view heap's console start as a host word (`RootType::Other(USER_VIEW_HEAP_START)` in `host_word`), the translator computes `(address − start) / 32` where a descriptor is loaded through an address read from memory. Measure: view heap `0x0b3c0000`-`0x0d9c0000`.
5. **Families:** after each few steps recompute whole families (README section 6 has the script idea; `bench/p4-full-families.txt`). When a large family (`0x2996b5a0` 525, `0x1cd48770` 330, `0x2996b900` 246) becomes whole: test it alone in game (`VICTORY_NATIVE_COMPUTE=1 VICTORY_NATIVE_COMPUTE_ONLY=<its list>`, 3 runs with `K_EARLY_SHOT=1 K_SHOTS=2`, screenshots compared with plain runs), and if it holds, a checkpoint with it on.
6. **Turning P.4 on by default** only for families that hold, through a list file in the repo; never all translatable programs (partial chains break the picture).
7. Then **0x1c8** (graphics, 10,206 descriptions) the same way.
8. **Image stores of the alpha channel alone** (`dmask:0x8`, 69 programs), deferred on purpose: README section 6 "Deferred on purpose" has why and how (read-modify-write only where each texel has one writing thread; raw atomics for linear textures).

## Other open threads (lower)

- `VICTORY_THP=0` against on, 3 runs each (do huge pages help on this machine?).
- Belobug's texture uploads in the submission prelude (`5d6bd84`) for the load-time flicker.
- The author's next fixes (known_holes under Wine, PORT_DESCRIPTORS with port 1, COPY_COMPARE on BAR): `git fetch kroeeee`, merge, checkpoint, measure.
- The world-entry load varies run to run (20-30 s of flicker and black patches in some runs, clean in others).
