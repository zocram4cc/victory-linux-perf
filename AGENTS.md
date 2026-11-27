# AGENTS.md

Instructions for coding agents and humans working in this repository. Terse by design. Facts come from the repo and its notes; check the named file before you act on a claim.

## Read first

- Personal scratch fork, private (notes/TASKS.md T362). Do not open issues or pull requests here: they are closed unread (README.md). Upstream work goes to the upstreams (see Upstreams).
- The repo holds no game files, code or assets. Users run their own copy of the title. Nothing from the game may be committed (`cargo xtask policy`).
- Never force-push or rewrite pushed history without the owner's OK. Commits already pushed (trailers included) stay as they are. README.md's "rebased and rewritten without notice" warning is about using this repo as a base.
- `main` is an integration branch. A round lands only after one A/B against main that is equal or faster, with the picture as good or better (notes/PLAN-integration.md).
- `notes/` (upstream registry, triage, risks, play script, launchers) is rig-local and outside this repo.

## What this is

- A fork of VIctory, a compatibility layer that runs an Xbox Series title natively on Windows from files the user supplies (README.md). Root upstream: Marvisak/VIctory.
- Built on avatarmanz/VIctory-7900XTX (its commit c9c932c is an ancestor of HEAD; that repo is gone, see Upstreams), plus round branches from other forks.
- Goal: a stable 30 fps or more without ray tracing (optional, off by default), then higher frame rate and lower frame times. Picture comes after (README.md).
- Method: port the title's renderer to behave like a PC renderer; translate shaders ahead of time into a versioned cache.
- New behaviour goes behind a `VICTORY_*` switch, off by default. Plain bug fixes keep the source's default.
- Target users: people running their own copy of the title on AMD RDNA2/RDNA3 GPUs. Measured on RX 6900 XT; README.md targets RX 7900 XTX.
- Crates (per-crate `Cargo.toml` descriptions): `victory-loader` runs the title's x86-64 image in-process; `victory-hle` is the Xbox system API it imports; `victory-gpu` is D3D12.X command streams and GPU memory on host D3D12; `victory-shader` is the translator; `victory-formats`, `victory-pe`, `victory-pipeline` (offline conversion, content-addressed cache), `victory-core` (config, logging, asset-policy guard), `victory-game` (title-specific code). `apps/victory` is the CLI; `xtask` holds the asset policy and git hooks.
- Licence: GPL-3.0-or-later (LICENSE). OPEN QUESTION: `crates/victory-loader/src/console_insn.rs` says it is ported from XCL (GPL-2.0-only), but README.md says no XCL code is here. Give-backs to klbf-lab must contain no GPL-2.0-only code (notes).

## Test record

- Measured: Linux 7.3.5, Mesa RADV 26.3.1, Ryzen 7 5800X3D, RX 6900 XT (16 GB), Proton Experimental 11.0-20261112 with its `wineserver` replaced by a view-tree build (CachyOS/wine-cachyos PR #50), `CONSOLE=series-s`. Source: CURRENTLY_TESTING.md and the victory-proton-run notes. Without the view tree, the wineserver's view list dominates map/unmap cost.
- Windows: we have recorded no Windows run of our rounds. README.md's "Where it stands" (Windows 11, i9-12900K, RX 7900 XTX, 2026-11-26) is the upstream base's own test record, inherited with the README: the base ran on Windows.
- Nothing in our rounds is Linux-only by design: the crates have no `target_os = "linux"` or `cfg(unix)` gates, and Wine-specific switches are gated on Wine (see Wine and Windows). Windows reports are welcome. Measure before assuming a gain carries over.

## Upstreams

Registry: `notes/upstreams.tsv`. GONE = removed upstream, archived privately (the README's avatarmanz link is dead).

| name | repo | we take | status |
|---|---|---|---|
| Marvisak | Marvisak/VIctory | root upstream; PR #3124 (SDWA), #3333 (straddling buffer loads) | watched |
| avatarmanz | avatarmanz/VIctory-7900XTX | our base (c9c932c); port 1/2 ordering fixes; PR #460 | GONE, archived privately |
| royale | datingesp-hash/victory-royale-private `k7900` | sister fork (we have push); P.4 native 0x218/0x1c8 translator | watched |
| kroeeee | kroeeee-eeee/VIctory-Linux-Proton-Wine-7900XT-XTX | THP MADV_COLLAPSE (47fd862c); PR #2 | watched |
| coolfox | developer/victory-coolfox-linux (Forgejo) | correctness picks (r9, r12, translator batch, guards) | watched |
| Slaktarn | Slaktarn700/SlaktarnVI | JOIN_VIEWS, EARLY_TABLES, LOOK_GUARDS, CARD_STEPS_LEARN, REFRESH_CHECK | watched |
| eikkapine | eikkapine/victory-amd | HOLES_ONCE, UPLOAD_POOL, PORT_ALL | watched |
| LibsMQG | LibsMQG/hle-d3d12-7900xtx | SKIP_WC_BACK (in play set), PORT_BUFFERS; its base was ryuki | GONE, archived privately |
| Belobug | Belobug/VIctory-Belobug-AMD | magenta probe-light filter (AMBIENT_MAGENTA); #565 taken | GONE, archived privately |
| Fayberr | Fayberr/Victory-United | Wine SSE2 memcmp, idle trims, worker rescue, AVX2 detiler | triaged |
| AntoninMichelq | AntoninMichelq/VIctory-forkAMD | copies, guards, textures (256 commits) | triaged |
| fxceless | fxceless/victory-fork | big PR pool (#927, #1353, #1376, #1407); faces, FSR | watched |
| klbf | PastiiJay/klbf-lab | origin; WC_BACK (e0a91ea); give-back PRs go to its `beta` | watched |
| myokex | myokex/victory-amd | guards over every view of a page | watched |
| DexNom | DexNom/VIctory | FSR4, RT fixes, debug HUD | watched |
| zeex64 | zeex64/VIctory-7900XTX | views-writewatch.patch | watched |
| DoubleGhosty | DoubleGhosty/cool_fox | source of many coolfox picks | watched |
| breakwater | bonaza/breakwater-victory-7900xtx | REFLECT_SCREEN, WC_JOIN_BYTES, userptr-thp preload | watched |
| rx6600 | olivertgwalton/victory-rx6600 | LAND_RING, KEEP_RELAY, BURY_ASIDE, KEEP_SNAPSHOTS, WOKEN_WAITS, PORT_WAITS | watched |
| scarlett | Op-lite/victory-scarlett | MAP_LOCK_FINE (our port); PROBE_RAYS not taken | GONE, archived privately |
| megafork | rorodev6/victory-megafork-amd | commit af875f26 (Filip) | taken |
| vibr | edsuuu/VI-BR-PORT | RT_MISS consumer (off) | taken |
| ryuki | YukiRyuu/RYUKI-7900XT | base of LibsMQG | watched |
| joshuu, neptune, victoryfps | JoshuuDev/victory-amd; zm0kiemedia/Neptune_VI; clankenjoyer/victory-fps | surveyed; Neptune base rejected (RISKS R17) | watched |

Credits and provenance:

- Ported commits keep the source author (`GIT_AUTHOR_NAME`/`EMAIL` from the source commit). Message: `From <owner>/<repo> <sha>.`, plus `Here: ...` when adapted.
- Credit record: `git log --format='%h %an | %s' -400`. Top authors there: local 113 (commits made here), zocram4cc 66, developer 44 (coolfox), archie 37 (royale), Giovanni 18, Beqren 11 (avatarmanz PR #129).
- New commits carry no `Co-Authored-By`. 116 of the last 400 already do: history; do not rewrite it.

## Build and test

- Toolchain: `rust-toolchain.toml` pins 1.98.1 and `x86_64-pc-windows-msvc`. Use `--locked`. Give each worktree its own `CARGO_TARGET_DIR`.
- Measurement build: `cargo xwin build --profile release -p victory --features scripted --target x86_64-pc-windows-msvc` (output `target/x86_64-pc-windows-msvc/release/victory.exe`).
- Diagnosis build: `cargo xwin build --profile diag -p victory --features diagnostics --target x86_64-pc-windows-msvc`. Frame capture needs it. The release build is about 3 % faster.
- Feature facts (apps/victory/Cargo.toml): `VICTORY_KEYS` works in every build; `VICTORY_STEP_OVER` needs `scripted`.
- `victory-gpu` and `victory-hle` are Windows-only (round rules; `cfg(windows)` gates in their lib.rs). Test: `cargo xwin test -p <crate> --target x86_64-pc-windows-msvc --no-run`, then `proton runinprefix <exe> --test-threads 1 <filter>` in the same prefix. Set `WINEDLLOVERRIDES="d3d12=n,b;d3d12core=n,b;dxgi=n,b"` yourself; the prefix needs Microsoft d3dcompiler_47 (docs/linux-howto.md).
- Native: `cargo test --release -p victory-shader -p victory-formats`.
- CI (`.github/workflows/ci.yml`, windows-latest): clippy with `-D warnings` (default and `--all-features`); `cargo test --workspace --locked` (includes the asset-policy test); `cargo test --workspace --all-features --locked -- --test-threads=1`.
- Asset policy: `cargo xtask policy`; the pre-commit hook runs `policy --staged`. Limits: allowlisted file types, 640 KiB per file, no PE or DXBC magic. `cargo xtask install-hooks` sets `core.hooksPath`, a git config change: not without the lead's OK.
- Known failing under Proton (ignore; round rules): gpu `watch::many_askers_tests::holes_are_known_until_the_runtime_tells`; shader `emit::tests::stub_pipeline_is_created`; hle `net::tests::only_loopback_is_reachable`, `xgamesave::tests::a_kept_blob_past_the_link_limit_is_copied`.
- Known failing at HEAD (inferred from code, not run): `repository_passes_policy`. `crates/victory-shader/src/emit/lower.rs` is 659,114 bytes; the limit is 655,360. Triage notes name upstream split commits as the fix path.

## Changing code

- Every env var the translator reads must be in the key list in `crates/victory-gpu/src/shader_cache.rs` (risk R24). `build.rs` hashes `victory-shader/src`, `shader_cache.rs`, `Cargo.lock` and `rust-toolchain.toml` into the cache key: any translator edit means full re-translation and rewarm.
- Bump `TRANSLATOR_VERSION` (`crates/victory-shader/src/lib.rs`, now 7) once per round, and only when translator output changes.
- Tests: red, then green. Revert the fix by hand, keep the test, see it fail, restore, see it pass. Quote the failing assertion.
- One commit per source commit (`git cherry-pick -x <sha>` when it applies, else hand-port). Squashed sources: one commit per feature.
- Messages go in a file: `git commit -F <file>`. No trailers. Don't change git config. Match the surrounding style; add no comments beyond provenance and what the source had.
- Merging another fork: list what is ON with no env set on our stack (Wine, THIN, INDIRECT_BATCH, CARD_STEPS, ASYNC_SUBMIT). Each default-on change needs an off switch before the build (RISKS R23: two older ones had none).
- Do not take (triage-1900): `VICTORY_PLANE_PASS=written` (walls go missing, per its author), `ALIAS_AFTER_UPLOAD` (device removals, per author), LATE_WAIT variants, upstream AGENTS/CLAUDE/GEMINI and cleanup files, DLSS, VRS.

## Measuring

- Scripted run from a fresh save to the world, then 150 world rows (one per second; world = `draws > 100`, not `fps < 5`). Figure: median `ms_per_frame`; prep: median `cpu_prepare_ms`. Compare the first N world rows of each run.
- Release build, warm caches: prebuild, then full `warm`. Set `MESA_SHADER_CACHE_MAX_SIZE=16G` for warm and play; RADV's 1 GiB default evicts the cache.
- A/B: bundle round branches into one integration branch and A/B it once against main: 2 interleaved runs per arm, B (the candidate) first (user rule, notes/TASKS.md T367). Picture captures at world frames 1400, 1500, 2500, 3000. Slower or worse picture: bisect by round, then by commit. Perf switches get one on/off sweep per bundle; single switches only if the sweep moves.
- A new base gets one unmeasured warm-up run. No builds and no Proton test runs during a measured run (`/tmp/i19/measuring` marks a run).
- Noise: one run is ±5 ms. Series S is bimodal (about 72–74 vs 80–86 ms, same settings): use at least 4 runs an arm for effects under ~5 %. Cold vs warm cache and DRED (`VICTORY_D3D12_DRED`, ~2.5× slower) confound timings: state both.
- Clean env first: unset `PROTON CONSOLE WINEDEBUG WINEDLLOVERRIDES STEAM_COMPAT_DATA_PATH` and every `VICTORY_*`, then pass settings explicitly. A leaked export once invalidated a comparison.
- Stop only your own PIDs. Never `pkill -f` by name: one such kill stopped live runs.
- Repo harness (Windows PowerShell): `scripts\bench.ps1 -Exe <exe> -TitleDir <dump> -Label <l> -Run <n>` (one run; frames 90–140 s; world = 100+ draws). `scripts\bench-ab.ps1 -Prefix <p> -Arms a,b` (per-arm mean and range, change against the first arm).
- Latest: all new switches off 77.7 ms; combined set 48.2 ms (CURRENTLY_TESTING.md, more runs pending). Newest single run of main 61b2d80f1: 46.4 ms (notes/TASKS.md T365).

## Recommended switch set

From the play script (rig-local). `CONSOLE` defaults to `series-s`. Every `VICTORY_*` name below exists in the tree (grep).

- Renderer port: `VICTORY_PORT_CBV=1 VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1 VICTORY_PORT_CONSTANTS=1 VICTORY_PARTIAL_DELTA=1 VICTORY_REFRESH_ONCE=1`
- Wine-only (inert on native Windows): `VICTORY_WINE_MAP=1 VICTORY_PORT_HOME=1 VICTORY_CLAMP_SYNC=0 VICTORY_PIPELINE_WATCH=0`
- Measured wins: `VICTORY_CARD_BUFFERS_MIN_KB=64 VICTORY_TABLE_MEMO=1 VICTORY_DYNAMIC_BITS=1 VICTORY_COMPUTE_WAIT_MS=500`
- Copies and landings (round r20): `VICTORY_SKIP_WC_BACK=1 VICTORY_LAND_RING=1 VICTORY_KEEP_SNAPSHOTS=1 VICTORY_BURY_ASIDE=1 VICTORY_WOKEN_WAITS=1` (BURY_ASIDE is off in code; the script sets 1)
- Picture: `VICTORY_AMBIENT_MAGENTA=1` (violet fix, Belobug's filter), `VICTORY_AMBIENT=0.8`
- Threads and pacing: `VICTORY_THIN=1 VICTORY_JOIN_PARTS=8 VICTORY_JOIN_COPIES=1 VICTORY_UPLOAD_AHEAD=1 VICTORY_PIPELINE_BOOK=1 VICTORY_LAND_SKIP=1 VICTORY_RELAY=2 VICTORY_RELAY_COPY=1 VICTORY_WIDE_BACK=1 VICTORY_CARD_STEPS=2 VICTORY_CARD_REHASH_RUNS=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_INDIRECT_BATCH=2 VICTORY_CARD_AWAKE=1 VICTORY_SHOW_AFTER=1 VICTORY_MAP_THREADS=1 VICTORY_TITLE_PRIORITIES=0 VICTORY_MAP_LOCK_FINE=1 VICTORY_LATE_WAIT=default VICTORY_PIPELINE_WAIT_MS=2500 VICTORY_TEXTURE_NO_WAIT=1`
- Run: `VICTORY_SKIP_MOVIES=end VICTORY_FRAMEGEN=0 VICTORY_STEP_OVER=<title addresses, in the script> GLIBC_TUNABLES=glibc.pthread.rseq=0 WINEDEBUG=-all WINE_CPU_TOPOLOGY=<sibling order of this CPU>`. `VICTORY_JOIN_PARTS_DRAWS` only when set. FSR needs `VICTORY_FFX_DIR`, which the script does not set.

## Deliberately off

- `VICTORY_INPLACE=0`: +200 ms; off until in-place Track A (play script). RISKS R16: not for in-game use.
- `VICTORY_HOME_NOHUGE=0`: +19 ms (play script).
- `VICTORY_CLAMP_SYNC=1` (49.0 ms), `VICTORY_WAKE_ONE=1` (50.0), `VICTORY_JOIN_PARTS_DRAWS=500` (49.8), `VICTORY_CARD_BUFFERS_MB=4096` (49.8): within noise of 49.7 ms.
- `VICTORY_GRAPHICS_WATCH`, `VICTORY_SMALL_ALIGN`, `VICTORY_PIPELINE_WATCH` (all =1 in r4): +2 ms, within noise. The last is recovery only.
- `VICTORY_SLOT_COUNTS=0`: off in every recorded run; no reason recorded.
- `VICTORY_RT_MISS=0`: not a violet fix (PLAN-integration.md). `VICTORY_RT_SKIN`, `VICTORY_HOST_EARLY`, `VICTORY_FRAME_INTERVAL_MS` (unset): not tested yet; the last matters below 33 ms.
- `VICTORY_TEXTURE_STORE=1`: costs about 8 ms a frame (README.md).
- `VICTORY_FRAMEGEN=0`: no measurement recorded. `VICTORY_PORT_DESCRIPTORS`: not in the set; no A/B recorded.
- `VICTORY_BATCH_PAGE_MAPPINGS=1`: fails under Wine (error 487); keep off (RISKS R7).
- Flat or worse, not retried without a new reason: `TITLE_PRIORITIES=1`, `ASK_TEXTURES=1`, `GUARD_RUNS=1`, `COPY_BESIDE=1`, `SUBMIT_FRAMES=2`, guard shards.

## Wine and Windows

- Wine-only switches are inert on native Windows (code): `VICTORY_CLAMP_SYNC` (only under Wine), `VICTORY_NULL_BUFFER_ZEROS`, `VICTORY_NULL_ROOTS`, `VICTORY_NO_HEAP_GUARD`, `VICTORY_PUBLISH_SAFE`, `VICTORY_PIPELINE_WATCH` (each gated on Wine), and `VICTORY_WINE_MAP` (victory-hle, gated on Wine). `VICTORY_COMPUTE_WAIT` is on under Wine, and on Windows only when set to 1.
- `VICTORY_PORT_CONSTANTS=1` works on Windows. Under Wine it also needs `VICTORY_PORT_CONSTANTS_WINE=1` or the home path (`VICTORY_PORT_HOME=1` with `VICTORY_WINE_MAP=1`).
- The hole-query fallback in `victory-gpu/src/watch.rs` has no switch. It runs only where `NtQueryVirtualMemory` class 7 fails, which is Wine.
- The Linux set is not the Windows set. Measure the Windows set on Windows before assuming any gain.

## Pitfalls

- Tuned settings in README and launchers may not be read by the pinned commit: grep each `VICTORY_*` before trusting it.
- Launchers override settings. Check the `effective-env:` lines in the console log, and a switch's own log line, before crediting it.
- `crashes.log` misses crashes inside Vulkan driver calls: read the console log tail. Silent exits are native SIGSEGVs: `coredumpctl`.
- Frame captures fire only on listed or power-of-two frames, and run on the submit thread: keep them off in timing runs.
- `VICTORY_OUTPUT`, `RENDER_SCALE` or `FFX_SCALE` other than the title's own size crash under Series S (RISKS R21; untested with `VICTORY_UPSCALER=6`).
- Known picture issues: purple vertical blinds and violet trees on the first balcony without `VICTORY_AMBIENT_MAGENTA=1`; sparkle on glass and speckle on floors; `VICTORY_PORT_CONSTANTS` without `VICTORY_PORT_HOME` under Wine gives a black world.

## Docs

`docs/findings.md` (measurements and reasons), `docs/architecture.md`, `docs/asset-policy.md`, `docs/proton.md`, `docs/linux-howto.md`, `docs/replay.md`, `docs/README-fxceless.md` (upstream's own description).
