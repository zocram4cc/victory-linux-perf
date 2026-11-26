# wineserver with a view tree

`wineserver-view-tree.patch` (against Wine's `server/mapping.c`) keeps every process's mapped views in a red-black tree by (process, base), next to the per-process list. Without it, every `map_view` / `unmap_view` request walks all of the process's views (`is_valid_view_addr`, `find_mapped_addr`). The title maps and unmaps GPU memory 64 KiB at a time and has tens of thousands of views, so each request costs about a millisecond and the server is busy all the time. Every thread that maps, unmaps or asks the write watch waits on it.

If two views ever share a base in one process, the tree is no longer trusted and the server falls back to the list for good. The patch then prints, once: `wineserver view tree: two views at base … ; falling back to the list`.

## Measured (26 Nov 2026, RX 7900 XT, Ryzen 9 9900X, 32 GB, Jason's house, this branch's settings, GE-Proton11-14)

| | stock wineserver | view-tree wineserver |
|---|---|---|
| fps (median per run) | 8.4, 8.2 | 17.0, 17.6, 18.5 |
| ms a frame | 119-122 | 54-59 |
| preparing / copies | 76-90 / 35-39 | 48-50 / 21-23 |
| "asking the write watch and the guards" | 17-26 ms | 4.6-4.7 ms |
| wineserver's share of the game's CPU samples | 10.6-16.4 % | ~1 % |
| world entry (launch to world) | 112-120 s | 43-60 s |

Runs: `bench/k-20261126-193334-k6-ge-stock`, `bench/k-20261126-194548-k6-viewtree-new` (`scripts/ktrials.sh`, `K_PERF=1`).

## Building it for a Proton

The wineserver must come from the **same Wine source as the Proton it replaces** (the server protocol version has to match exactly), so the patch is applied to that Proton's Wine and only `wineserver` is built:

1. Get the Wine source of your Proton: for GE-Proton, its release's `wine` submodule after its `patches/protonprep-valve-staging.sh` (or `prep-wine.sh`); for Proton Experimental, ValveSoftware/wine at the commit of your Proton build.
2. `patch -p1 < wineserver-view-tree.patch` in that Wine tree.
3. Configure and build just the server: `./configure --enable-win64 && make -j"$(nproc)" server/wineserver` (or in a separate build directory).
4. Copy the Proton (`cp -a --reflink=auto`) and replace `files/bin/wineserver` in the copy with the new one; start the game with that copy (`PROTON=…/proton` for `linux/victory-proton.sh`).

Check: the game's log should not show the fallback line, and the frame lines' "asking the write watch and the guards" should be a few ms.
