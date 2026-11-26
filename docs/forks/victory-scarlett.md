# victory-scarlett (Op-lite/victory-scarlett): notes for track U

Read on 26 Nov 2026 at `7c8f019` (630 commits): codec-d/cool_fox `f4b48ad` with a local fork ("Scarlett") ported on top. Windows, Ryzen 7 9800X3D, 32 GB, **RTX 4080 SUPER**, 1440p, Series X. Clone: `~/src/victory-scarlett` (read only).

## Their numbers (6-minute opening route, diagnostics build)

41 ms a frame at 1,000-2,500 draws, 50 ms at 2,500-4,000, 61 ms above 4,000; a heavy scene (5,500-8,600 draws) ~70 ms. Commit ~36 GB.

## Their run set (run-118, README)

`VICTORY_ASYNC_SUBMIT=1 VICTORY_QUEUE2=1 VICTORY_RELAY=2 VICTORY_WIDE_BACK=1 VICTORY_RELAY_COPY=1 VICTORY_CARD_STEPS=2 VICTORY_INDIRECT_BATCH=2 VICTORY_INDIRECT_WINDOW=1 VICTORY_JOIN_PARTS=8 VICTORY_UPLOAD_AHEAD=1 VICTORY_SHOW_AFTER=1 VICTORY_REFRESH_ONCE=1 VICTORY_LEVELS_ON_DEMAND=1 VICTORY_PIPELINE_WAIT_MS=20`; the three that "made the difference": `VICTORY_POOL_DISCARD=1 VICTORY_FILL_TITLE=1 VICTORY_PARTIAL_DELTA=1`. Run-128 adds memory trims (`VICTORY_BAKE_DELTA`, `VICTORY_MAP_LOCK_FINE`, staging and window trims).

## The clue for us: `VICTORY_SHOW_AFTER` instead of our slot wait

- **There is no slot wait in their tree.** For the same race as our S.1 (a texture made and shown while lists already on the card read the slot by index: their losses n08, n11, n15 were page faults in a texture made and shown in the last tenth of a second), `VICTORY_SHOW_AFTER=1` makes a title slot **show a texture that is new to it only once the submission with its upload has finished**; until then the slot shows what it showed (`descriptor/host.rs` `texture_slot_show_after`, `texture_slot_flip`).
- **Ours waits instead:** `descriptor::host::slot_wait` makes every host slot write wait until no submission runs. Measured on 26 Nov (journal, C.1): **~1 s a frame of the recording threads' time, ~15-25 writes a frame each waiting for a whole submission**. Off (`VICTORY_SLOT_WAIT=0`), the picture broke the way an early-shown texture would (black sky, white specks).
- **So the port to try:** their show-after rule in place of the wait (pool on). It is the C.1 gain (frame ~400 → ~330-360 ms in our runs without the wait) with the picture kept.

## Also

- `VICTORY_INDIRECT_BATCH=2` exists there too (indirect clamps batched; ours is I.1, on by default).
- On this machine cool_fox ran ~2 fps with the card ~11 % busy ([cool-fox.md](cool-fox.md)): the Windows figures of this family do not carry over to Wine as they are; the CPU-side waits are the difference to look at.
