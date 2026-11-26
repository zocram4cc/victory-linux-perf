# S.6: host textures in pre-mapped memory

Roadmap task S.6. Pieces **M** (memory placement), **D** (descriptors) and **Q** (submission). Written 25 Nov 2026, after S.1 ([journal](../journal.md)).

## Problem

The open-world device loss (README, *Known blocker*) is a GPU read from a texture made moments before. Its address is mapped, but its page-table update has not reached the GPU yet:
1. `descriptor/host.rs` `make()` writes host slots lazily, from the title's threads, into the shader-visible heap, while a submission may run (15,600-58,300 slots a frame in the open world).
2. A slot can be pointed at a texture made after the running submission was sent. Each new committed texture gets fresh memory, and the kernel updates the GPU page tables for it asynchronously. Only a submission sent *after* the mapping waits for that update.
3. Work in flight that reads the slot faults (`VKD3D_CONFIG=fault`: *"Fault address is currently mapped"*).

`VICTORY_SLOT_WAIT=1` (the minimal fix) makes slot writes wait until no submission runs. Open world: 5 of 5 held, about 90 writes a frame wait.

## Why the wait is not the end state

- **Phase 3 (frames in flight)** means a submission is almost always running. The wait would then serialise the title's recording threads behind the GPU, which undoes the overlap.
- **Staging slot writes to the next submission boundary is not enough either.** With frames in flight, an *earlier* submission can still be running and read the slot. The kernel makes only the new submission wait for the page-table update.

The race is not between the descriptor write and the GPU. It is between **a fresh memory mapping** and **GPU work sent before it**. Removing fresh mappings from the hot path removes the race in any submission model.

## Design

Host textures (`resource.rs` `host_texture`, `twin_texture`) are made as **placed resources in heaps the layer creates ahead of time**, instead of committed resources with their own memory.

- **Pool:** `ID3D12Heap`s of 256 MiB (default heap type, resource heap tier 2 allows buffers and textures together; the probe reported tier 2), created at start and grown in whole heaps. A heap is mapped once. Placing a texture in it binds an image to memory already in the GPU's page tables, so no page-table update is needed.
- **Allocator:** per heap, a free list of 64 KiB blocks (the placement alignment). Fit: first fit by size class (64 KiB, 1 MiB, 4 MiB, larger). Large textures (more than 64 MiB) stay committed.
- **Growth:** a new heap is made *at a submission boundary* (in `submit`, before `ExecuteCommandLists`), when the free space falls below a threshold (default 512 MiB). The heap is made before any texture placed in it can be named by a slot, so its mapping precedes every submission that can read it.
- **Release:** a texture's blocks return to the free list only through `retire()` (after the submissions running at release have finished), keeping design rule 7 (exact lifetimes). The sweep keeps its policy; what changes is that its frees no longer return memory to the kernel.
- **Switch:** `VICTORY_TEXTURE_POOL=1`, off until measured; `VICTORY_TEXTURE_POOL_MB` sets the initial size (default 2,048).

### What it fits with

| Piece | Effect |
|---|---|
| M | the first piece of "memory the layer owns and places": the same allocator later serves buffer copies (phase 2), ReBAR heaps (2.2), and tile mappings (2.3) |
| D | slot writes no longer need to wait: `VICTORY_SLOT_WAIT` can go once the pool covers every texture a slot can name |
| L | releases go through `retire()`; no texture memory returns to the kernel while the GPU may read it |
| Q | removes the race for frames in flight (3.1) |
| T | the sweep's churn (1-1.5 GB of copies let go and remade per minute in the prologue) stops costing kernel allocations |

## Steps

1. **Measure first:** count host textures made per frame in the open world, their sizes, and the time spent in `CreateCommittedResource` (frame line). This sizes the pool.
2. A `pool.rs` module: heaps, size classes, free lists, `place(desc) -> (heap, offset)`, `free(heap, offset, size)` through `retire()`. Unit tests for the allocator (host side, no GPU).
3. `resource.rs`: `host_texture` and `twin_texture` use `CreatePlacedResource` when the switch is on and the size fits; otherwise committed, as today. `pin()` covers placed resources too.
4. Growth at the submission boundary (`host.rs` `submit`).
5. Frame line: pool size, in use, placed and committed counts, growth events.
6. **Measure:** `scripts/trials.sh -n 5 -l pool VICTORY_TEXTURE_POOL=1` (Jason, **without** `VICTORY_SLOT_WAIT`): the exit is 5 of 5 held. Then scene A and the open world for speed, against the wait.

## Risks

- **Placed textures and RADV metadata:** compressed render targets need their metadata inside the placement; D3D12 accounts for it in `GetResourceAllocationInfo`, so sizes come from there.
- **Fragmentation:** the open world makes many sizes. Size classes plus whole-heap growth bound the waste; measure it (step 5).
- **Aliasing bugs:** two textures in one block would corrupt silently. The allocator gets unit tests, and a debug check that a block is free before placing.
- **The fault may also come from buffers** (host copies of title buffers are committed too). The reports so far named textures only. If faults remain with the pool, the same pool serves buffers.
