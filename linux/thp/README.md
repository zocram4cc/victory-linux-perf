# Huge pages for renderer port 1 (AMD, Linux)

**Measured: ~9 -> ~18-19 fps in the same scene, live, by this alone** (RX 7900 XT,
Proton Experimental, kernel 7.2). In 8,000-10,000 draw views: 16-17 fps.

## Why

Renderer port 1 (`VICTORY_PORT_CONSTANTS`) opens the title's 32-bit heap
(~185 MB of private memory) as a GPU heap from its address. vkd3d-proton hands
it to the amdgpu kernel driver as a *userptr*, and amdgpu checks every page of
a userptr on **every submission** (`hmm_range_fault`). With 4 KiB pages that is
~47,000 page-table entries, ~18 times a frame.

A `perf` profile showed the `vkd3d_queue` thread at 100 % of one core, almost
all of it in the kernel (`hmm_vma_walk_pmd`, `vm_normal_page`), and the GPU
idle behind it. As 2 MiB transparent huge pages the same memory is ~93
entries.

Whether the kernel gives that memory huge pages by itself is luck (free 2 MiB
blocks, `defrag` settings, `khugepaged`, which is far too slow for a process
with ~45 GB of reserved address space). That is why frame rates differed so
much from one run to the next.

## What the script does

`linux/victory-proton.sh` reads the port-1 ranges from the run's log ("pages
of the 32-bit heap are private memory at=... len=...") and, every 15 s,
collapses them into huge pages from outside the process with
`process_madvise(MADV_COLLAPSE)` (Linux 6.1 or newer). When they are huge
already, a round costs next to nothing. The result goes to `run-*.thp` next to
the log. `VICTORY_THP=0` turns it off.

It touches nothing but those ranges: their bytes stay the same, only the
pages under them change.

## Setup (once)

`process_madvise` on another process needs `CAP_SYS_NICE`. Build the helper and
give it that one capability (not root):

```
cc -O2 -o linux/thp/victory-thp-collapse linux/thp/victory-thp-collapse.c
sudo setcap cap_sys_nice+ep linux/thp/victory-thp-collapse
```

Without it the script says so at start and runs as before.

## Check

```
grep -A20 '^<start of the ~200 MB rw-p region below 4 GiB>' /proc/<pid>/smaps | grep AnonHugePages
```

or simply the `.thp` file: `88 collapsed, 0 not` at the first round.
