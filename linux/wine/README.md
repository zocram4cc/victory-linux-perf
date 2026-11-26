# Two changes to Wine for running the title under Proton

Both are optional. The layer runs on an unchanged Proton; these make it
faster, or are groundwork that is not used by default. They go into a private
copy of a Proton build, never into Steam's own.

The two patch files and the test program named below are in this folder.
The patches are changes to Wine's own source, so they carry Wine's licence
(LGPL 2.1 or later), not this repository's.

Measured on Proton Hotfix `hotfix-20261110` (wine-11.0), kernel 7.3, RADV, an
RX 9070 XT.

## 1. `views-tree.patch`: the server finds a process's views through a tree

The title maps and unmaps its memory in 64 KiB views, 1,000 to 14,000 times a
second while it streams, with 20,000 to 70,000 views alive. Wine's server
keeps a process's views in a list: `map_view` walks all of it
(`is_valid_view_addr`), `unmap_view` walks it to the view
(`find_mapped_view`), and ntdll makes the request holding its process-wide
lock on virtual memory, so every other memory call of the process waits
behind it.

| Views alive | A map call, before | With the patch |
|---|---|---|
| under 5,000 | 10 us | 7 us |
| 10,000 to 15,000 | 95 us | 19 us |
| 15,000 to 20,000 | 148 us | 17 us |
| 20,000 to 25,000 | 203 us | 14 us |

The title's mapping thread went from 45-76 % busy in mapping calls to 1-14 %.
The patch adds a red-black tree beside the list (`server/mapping.c`,
`process.c`, `process.h`); the list and its order stay, and a view that
could not go into the tree (a second one at the same base, which the checks
rule out) makes the lookups fall back to the list.

Not settled: on the patched server the title has crashed three times in
about fifteen runs with a read through a null pointer in its own code (twice
while loading the world, once 95 s in), and never in twenty-odd runs on the
unpatched one. The patch has been read again without a fault being found;
the guess is that the much faster mapping changes timing and shows a race in
the layer.

## 2. `views-writewatch.patch`: the kernel tracks writes to mapped views

Proton's ntdll tracks writes to `MEM_WRITE_WATCH` allocations in the kernel
(userfaultfd in asynchronous write-protect mode and `PAGEMAP_SCAN`), with no
fault to user space. The patch (17 lines in `dlls/ntdll/unix/virtual.c`)
offers the same for a view of a section:

- `MapViewOfFile3` with `MEM_WRITE_WATCH` in its allocation type turns the
  tracking on for that view (`STATUS_NOT_SUPPORTED` where the kernel's
  tracking is not there);
- `GetWriteWatch` with flag `0x80000000` answers for a range of several
  views and holes. With the reset flag the kernel passes over memory it does
  not track in silence, so only ask so about ranges that are tracked views
  and holes all through. Without the flag an asking about one view works as
  for an allocation, and is refused for a view that is not tracked.

`writewatch-test.c` shows it working, and what it costs: three writes give
those three 4 KiB pages; a first write after a reset 0.25 us (a write fault
through the layer's guards is about 130 us); an asking over 64 views (4 MiB)
4.5 us; mapping with the flag no dearer. A write through one view of a page
is not seen at another view of the same page: the kernel tracks by address.

The layer can use it (`VICTORY_VIEW_WATCH=1`), and so far gains nothing from
it: see `crate::watch::views_hot` and `views_whole` in
`crates/victory-gpu/src/watch.rs` for the two ways tried and their numbers.
It is off by default.

## Building them

The source must be the one the Proton build was made from. For
`hotfix-20261110` that is ValveSoftware/wine, branch `experimental_11.0`, the
commit before `d59f8494e5` ("server: Rename ymm registers to vector"): one
commit later the thread context the client sends has another layout, the
server answers the first thread's `select` with `STATUS_INVALID_PARAMETER`,
and `wineboot` hangs with no window. Proton's build runs
`tools/make_requests`, which is why its protocol version is one higher than
the header in the tree says.

```bash
git clone --filter=blob:none --single-branch -b experimental_11.0 https://github.com/ValveSoftware/wine.git wine
(cd wine && git checkout 'd59f8494e5^' && perl tools/make_requests && perl tools/make_specfiles && autoreconf -f)
mkdir build && (cd build && ../wine/configure --enable-win64 --without-x --without-wayland --without-freetype --without-mingw --disable-tests)
```

`configure` ends with an error about a generated Vulkan header: it has
written `build/include/config.h` by then, which is all that is needed here.

```bash
(cd wine && git apply ../views-tree.patch ../views-writewatch.patch)
./build-server.sh   # obj/wineserver
./build-ntdll.sh    # obj-ntdll/ntdll.so (config.h with HAVE_LIBUNWIND off, as Proton's)
```

Then copy the Proton build (`cp -a --reflink=auto`), and in the copy replace
`files/bin/wineserver` and, for the second patch,
`files/lib/wine/x86_64-unix/ntdll.so` with stripped copies of what was built.
`linux/local.env.example` shows how the scripts pick such a copy.

Check before starting the title:

- the built `ntdll.so` exports and imports the same symbols as the one it
  replaces (`nm -D`; 289 and 203 here);
- `wine wineboot --init` in a fresh prefix ends, and `wine cmd /c echo ok`
  prints;
- `writewatch-test.exe` (build it with `clang --target=x86_64-pc-windows-msvc`
  against the xwin headers and `-lonecore`) ends with "works".
