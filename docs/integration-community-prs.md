# Integration of community PRs #1, #2, and #3

This branch merges three open community pull requests into current `dev`.
It is a source integration for review and in-game testing. Nothing here was
measured on a GPU in this pass.

## Where the changes came from

| PR | Branch | Submitted by | What it brings in |
|---|---|---|---|
| [#1](https://github.com/crossducking/RAZDA4A-XEN/pull/1) | `feat/rx7600-8gb-vram-fit` | [@nathanbzrr](https://github.com/nathanbzrr) | 8 GB VRAM policy and the safe-mode preset |
| [#2](https://github.com/crossducking/RAZDA4A-XEN/pull/2) | `feat/amd-fixes` | [@conongnang](https://github.com/conongnang) | AMD/RDNA fixes cherry-picked from community forks |
| [#3](https://github.com/crossducking/RAZDA4A-XEN/pull/3) | `feat/upstream-sync` | [@conongnang](https://github.com/conongnang) | 17 commits that apply cleanly from upstream [Marvisak/VIctory](https://github.com/Marvisak/VIctory) `main` (`63bcca4`) |

PR #2 keeps the original authors on each commit (`git cherry-pick -x`):

| Fork | Author | Change |
|---|---|---|
| [fxceless/victory-fork](https://github.com/fxceless/victory-fork) | V.V. (`t90`) | Scalar codes past the physical registers no longer panic the unspill pass |
| [fxceless/victory-fork](https://github.com/fxceless/victory-fork) | fxceless | On 32-lane hosts, the water-light tile lists take one slot per lane (`VICTORY_WATER_TILE_SLOTS=0` turns it off) |
| [fxceless/victory-fork](https://github.com/fxceless/victory-fork) | fxceless | Static samplers keep one order in the driver's table (`VICTORY_SAMPLER_ORDER=0` turns it off) |
| [DexNom/VIctory](https://github.com/DexNom/VIctory) | DexNom | A wave64 mask copied through scalar moves is recognized again (voxel visibility) |
| [DexNom/VIctory](https://github.com/DexNom/VIctory) | DexNom | A uniform branch may choose between two immediate samplers. The integer-target blend half of that commit was not brought over; `dev` already strips blending on integer targets |
| [ToLIManl/VIctory-Hydra](https://github.com/ToLIManl/VIctory-Hydra) | ToLIManl | The command-list shim is put back after the Agility SDK 1.620+ `Close()` vtable swap |
| [ToLIManl/VIctory-Hydra](https://github.com/ToLIManl/VIctory-Hydra) | ToLIManl | Replay drops aliasing barriers that name a committed resource (D3D12 error 528) and raises the info-queue limit while recording |

PR #3 keeps each upstream author. The commits cover XAsync cleanup without the completion lock (#114, Andrew), reserving address space below 4 GiB for 32-bit title memory (#1009, Marvis), the Windows pipeline-cache publish race (#1324, Maledetto), heap samplers keeping point mips when anisotropic (#2031, Mehmet Güneş), crash reports appended as single records (#2033, Manuel Pérez), masked password text entry (#881 / #2067, Nuraad and 给伊莉雅补魔), placed descriptor heaps released before their test memory is reused (#2262, BaggyALPHA), plus diagnostics and tests from Daniel, DoubleD17, Flare, Fuzeofficial, Pablo González, prism, and wilu222.

## What changed

- **8 GB cards.** Textures are admitted at 65% of the budget instead of 78%, 2048 MB is reserved for render targets instead of 1024 MB, and card-buffer copies cap at 768 MB instead of up to 1024 MB. The 12, 16, and 24 GB tiers are unchanged. `presets/4_Safe_Mode_8GB_12GB_VRAM.bat` sets the same limits (`TEXTURE_ROOM_PERCENT=65`, `CARD_ROOM_FOR_TARGETS_MB=2048`, `CARD_BUFFERS_MB=768`) and turns VSync on.
- **Shaders on RDNA (wave32).** One known water-light compute program is rewritten, before translation, so each lane appends its own tile. Other programs are left alone (container digest and code hash must both match). A uniform branch between two immediate samplers lowers to two samples and a select. Scalar spill codes above the register file no longer index off the end of the parking table. A wave64 mask rebuilt from scalar copies stays a mask.
- **Pipelines.** Before the first root signature that uses static samplers, each sampler already seen gets a root signature of its own, in a saved order, so the driver's cache can hit on the next launch.
- **Replay.** `Close()` reinstalls the command-list hook. Aliasing barriers that touch a committed resource are skipped. A failed `Close()` prints debug-layer messages when the info queue is on.
- **Host memory and HLE.** On every real launch, free address space below 4 GiB is reserved except the lowest 256 MB, and a 32-bit allocation that does not fit releases guards and retries. A failed async call runs provider cleanup without holding the completion lock. Password text entry uses the password edit style. Heap samplers follow the same anisotropic point-mip rule as static samplers.
- **Diagnostics and tests.** Crash logs append one record at a time. Unimplemented GPU methods log the four argument registers. HDR captures keep NaNs. Inspect names the remaining DLL characteristic bits. New unit tests cover BC1 endpoint prediction, XMA table invariants, fence waits after a rewind, and the cache publish race.

## What should improve, in theory

- An 8 GB card whose live budget falls to about 5.9 GB during play should stop sitting 0.5–1.3 GB over budget, which is the stall the PR #1 author measured on an RX 7600 (about 200 ms frames, native FPS 3.67 to 12.7 in their 180 s run). Those numbers are the author's, already written into the READMEs on `dev`; this pass did not re-measure them.
- Open water on a 32-lane GPU should stop losing tiles from the second half of a 64-lane group (the 16×16 dark patches the fork author traced).
- Starts after the first should spend less time recompiling pipelines whose static samplers had landed in a different driver slot.
- A title that needs a large 32-bit reservation at startup should get that address space instead of failing because the host filled everything below 4 GiB.
- Password prompts should not show the typed characters. A failed async call should not deadlock its cleanup against the completion lock.

## Materials used

The three pull requests and their diffs, the fork commits named above, and upstream Marvisak/VIctory `main` at `63bcca4` as cited by PR #3. No game assets, captures, or shader dumps were added. The water-light change identifies one program by a 16-byte container digest and a hash of its code, then rewrites that program in memory. Verification on this machine used the locked dependency set already in `Cargo.lock`.

## Verification done here

Linux cannot link a Windows executable (no `link.exe` / `ml64.exe`). The project’s CI target is `x86_64-pc-windows-msvc`, so that triple was typechecked with `cargo check --workspace --all-targets --locked --offline --target x86_64-pc-windows-msvc` (clang + `llvm-lib` only so the `blake3` build script can run). A host `cargo check` / `cargo test --workspace` does not compile the Windows-only crates; that failure is the same on plain `dev`.

| Check | `dev` | This branch |
|---|---|---|
| Merge into `dev` | — | Clean for #1, #2, and #3, including all three together. The only overlapping file is `pipeline/host.rs`; git merged it and both edits are present |
| Windows `cargo check --workspace --all-targets` | Pass. Dead-code warnings: `lower`, `lower_counted`, `part_ranges` | Pass. The same three warnings, no errors |
| `victory-shader` tests | 214 passed, 2 ignored | 219 passed, 2 ignored (the five new tests pass) |
| `victory-formats` tests | 43 passed, 5 ignored | 51 passed, 5 ignored (the eight new tests pass) |
| `victory-pipeline` tests | 7 passed | 7 passed (the new cache race test is `cfg(windows)` and did not run) |
| `cargo xtask policy` | 286 files ok | 289 files ok |
| `xtask` `documented_switches_are_read` | Failed while `AGENTS.md` named a lock-path variable the runtime does not read | That name is no longer written as a switch token |
| `victory-pe` image tests that parse the test binary | Fail on Linux (`Unknown file magic`, the binary is ELF) | Not re-run; the PR only adds DLL-flag constants |

No unexpected network calls, binaries, obfuscated payloads, or download-and-execute steps. The new preset only sets variables and starts `RAZDA4A_XEN.exe`.

## Still needs an in-game run

Do this on the target machine (Ryzen 5 5600, 16 GB RAM, and an 8 GB card if the safe preset is in the run) before treating the result as a verified win:

- 180 s on the 8 GB safe preset, compared with the README’s RX 7600 numbers.
- 180 s on the 16 GB card with the default preset. PR #2’s sampler order and water-tile rewrite are on by default and were not benched by their author. PR #3’s low-4 GiB reservation runs on every launch and changes the address space the 16 GB path starts in; 256 MB of slack may be tight once the device and pipeline cache allocate.
- Open water on a wave32 GPU, a cold start and a second start (pipeline cache), a password prompt, and a 32-bit title allocation at startup.
- Watch for device removal, OOM, and the solved guards (character faces, the RDNA4 pipeline skip, FSR frame generation, tearing on the main path). The safe preset itself sets `VICTORY_VSYNC=1`, `VICTORY_ALLOW_TEARING=0`, and `VICTORY_SEGMENT_TIGHT=85`; that is opt-in and does not change the 16 GB defaults.
