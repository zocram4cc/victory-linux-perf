# M.1: one host copy for each byte of title memory

Piece **M** (memory placement); it unlocks 1.3 step 2 (piece **B**) and so 3.1 (**Q**). Written 26 Nov 2026 from that morning's measurements ([journal](../journal.md)).

## Why

- **Ceiling** (`VICTORY_BACK_CEILING=1`): without the copy-back the frame is 362-374 ms against 477-486, and the card waits for us ~60 ms a frame against 173-228. But the picture goes black.
- **CPU census** (`VICTORY_BACK_CENSUS=2`): the title's CPU reads about 1 of the ~10,600 pages written back each frame before the next submission.
- **Overlap census** (`VICTORY_BACK_OVERLAP=1`): per frame, 253-263 of the ~465 ranges shaders write have **another host copy over the same title bytes** (about 267 copies, 67-85 MB of overlap; aliases not counted).

So the copy-back exists mostly to carry what the GPU wrote in one host copy to another host copy of the same memory, through title memory: GPU → card → PCIe → CPU snapshot compare → title memory → write watch → CPU → PCIe → card. One copy per byte removes that loop. The copy-back is then needed only for the CPU's few reads, which 1.3 step 2 fetches on demand.

## Where the overlapping copies come from (`shadow.rs`)

1. **Nested buffers.** `buffers()` holds every title buffer object by (start, resource); `lookup` hands out the copy of the **innermost** buffer holding the ask. A buffer inside a heap-sized buffer gets its own copy, and the big one has another of the same bytes.
2. **Windows over buffers.** `LOOSE` holds 4 MiB windows (`WINDOW`) for asks outside any buffer. A window made before a buffer is created over the same memory stays, and a later ask finds the buffer.
3. **Window variants.** Tails (`VICTORY_WINDOW_TAILS`) and fine windows (`fine()`, `holder`) can make more than one window per cell.
4. **Aliases.** The same pages at two addresses (the page pool's 64 KiB mappings). They are not seen by address and are out of scope here.

## Design

**Rule:** every title byte has at most one host copy at a time: the **outermost** one. Asks inside it get the outer copy and an offset. A view, a root view or a vertex buffer is a GPU address, so an offset into a bigger buffer is free (`Shadow::gpu(address)` already maps address → GPU address).

- `lookup` returns the outermost buffer with a copy that holds the ask, not the innermost. A new copy is made only where none holds the range.
- **A new buffer over an existing copy** (case 2, or a big buffer created after small ones) **adopts** it:
  - if one copy holds the new buffer, asks for the new buffer go to that copy;
  - if the new buffer is bigger, its copy is made, and the copies it covers are retired through `retire()` once nothing holds them (exact lifetimes, rule 7). Their bytes are not copied over: the new copy is refreshed from title memory as any new copy is.
  - Until then, the covered copies stay in use by the lists and slots that hold them. A **coherence window** remains for those few submissions; they keep the copy-back.
- **Windows** are made only where no buffer copy holds the ask (today too), and a window under a later buffer is retired as above.
- **Limits:** a copy has a size limit (heaps of hundreds of MB, `card_limits` for card copies). Outermost wins only up to it. Past it, the cell split stays and overlaps are counted (`VICTORY_BACK_OVERLAP`).

## Steps

1. **Census, extended:** for each overlapping pair, which of cases 1-3 it is, and the sizes. If most is case 1 (nested buffers), step 2 alone removes most overlap.
2. **`VICTORY_ONE_COPY=1`:** outermost lookup for nested buffers.
   - Measure `VICTORY_BACK_OVERLAP` (target: fewer than 10 overlapped ranges a frame), the frame and the picture, then 5 runs of Jason and of the prologue.
3. Windows retired under later buffers (case 2).
4. **Then 1.3 step 2:** with no overlap, the ceiling's switch becomes a design. The copy-back is skipped for ranges no CPU read touched, and fetched when the CPU touches them. The guards need a safer form than the census's, since its guard on a page the title does read faulted once. That gives the ~22 % of the ceiling with the picture intact, and opens 3.1.

## Risks

- **Bigger copies, bigger refreshes:** an outer copy refreshes by write watch per page, so the cost follows the pages written, not the copy's size. Uploads must be watched for it (`copies … MB` on the frame line).
- **Views at offsets past 4 GiB or alignment rules:** root CBVs need 256-byte alignment. The scratch path (`root_view`) already handles a misaligned address.
- **Descriptor slots** that show an inner copy (`descriptor::host::buffer_copy`) must be remade when it is retired. Slot staleness is a known source of faults (S.1), so retirement must go through the same path as the sweep.
