# Optional: a wineserver with a view tree

`wineserver-view-tree.patch` (against Wine's `server/mapping.c`) keeps every process's mapped views in a red-black tree keyed by (process, base), next to the per-process list, which still keeps their order.

## Why

Without it, every `map_view` and `unmap_view` request makes the server walk all of the process's views (`is_valid_view_addr`, `find_mapped_addr`). The title maps and unmaps GPU memory 64 KiB at a time and ends up with tens of thousands of views, so each request takes about a millisecond and the server is busy all the time. Every thread that maps or unmaps memory, or asks the write watch, waits for it.

In a `perf` profile of the game and its wineserver (stock server), the wineserver took 11-24 % of all CPU samples, almost all of it in `is_valid_view_addr` and `req_unmap_view`.

If two views ever share a base in one process, the tree is no longer trusted and the server falls back to the list for good. The patch then prints, once, to the server's stderr: `wineserver view tree: two views at base …; falling back to the list`.

## Measured

RX 7900 XT, Ryzen 9 9900X, 32 GB, Mesa/RADV 26.3.1, GE-Proton11-14, this repository's `linux/victory-proton.sh` settings, release build, Jason's house, unattended runs with a 40 s window each:

| | stock wineserver | view-tree wineserver |
|---|---|---|
| fps (median per run) | 8.4, 8.2 | 17.0, 17.6, 18.5 |
| ms a frame | 119-122 | 54-59 |
| preparing / copies | 76-90 / 35-39 | 48-50 / 21-23 |
| "asking the write watch and the guards" | 17-26 ms | 4.6-4.7 ms |
| wineserver's share of CPU samples | 10.6-16.4 % | about 1 % |
| launch to world | 112-120 s | 43-60 s |

Not measured yet on Proton Experimental.

## Building it

A wineserver only works with the Wine it was built from: the server protocol version has to match exactly. So the patch goes into the Wine source of the Proton you use, and only the server is built:

1. Get the Wine source of your Proton: for GE-Proton, its release's `wine` tree after its prep script (`patches/protonprep-valve-staging.sh`); for Proton Experimental, ValveSoftware/wine at the commit of your Proton build.
2. In that tree: `patch -p1 < wineserver-view-tree.patch`.
3. Build only the server, e.g. in a separate directory: `../wine/configure --enable-win64 && make -j"$(nproc)" server/wineserver`.
4. Copy your Proton (`cp -a --reflink=auto`), replace `files/bin/wineserver` in the copy with the new one, and start the game with that copy: `PROTON=/path/to/copy/proton ./linux/victory-proton.sh`.

To check it took: the log has no fallback line, and the frame lines' "asking the write watch and the guards" is a few milliseconds.
