# AGENTS.md — RAZDA4A XEN (`dev` source branch)

Autonomous engineering manual for AI coding agents working in parallel chats on this repo.
**Target hardware baseline:** Ryzen 5 5600 (6C/12T) · Radeon RX 9060 XT 16 GB · **16 GB RAM** · **Windows 10 (19045)**.

## Autonomous End-to-End Protocol (Zero-Touch User Flow)

The user simply opens a chat and gives a goal (e.g. "raise CPU FPS", "cut VRAM below 7.5 GB", "fix open-world lighting"). **Execute the entire lifecycle autonomously without asking the user to run manual git/build commands:**

1. **Self-isolate in a fresh Git worktree (FIRST step, before any edit):**
   ```powershell
   git fetch xen dev
   git worktree add ../wt-<short-slug> -b feat/<short-slug> xen/dev
   cd ../wt-<short-slug>
   ```
   Check `git worktree list` first to avoid colliding with another active chat's folder, branch, or hot file (`host.rs`).
2. **Build & test using shared `CARGO_TARGET_DIR`:**
   Use the shared `CARGO_TARGET_DIR` so parallel Rust builds queue safely instead of exhausting 16 GB RAM.
3. **Wait politely for `heavy.lock` before running GPU tests/benchmarks:**
   Only ONE chat may run the runtime binary or `warm` at a time. Check the workspace's `heavy.lock`, or the environment variable that overrides that path (the `VICTORY` prefix plus `HEAVY_LOCK`), and active GPU runtime processes. If it is busy, wait (never kill another chat's benchmark!).
4. **Verify via 180 s benchmark:**
   Confirm `0` errors, `0` OOMs, and measurable win vs baseline (`29.2–31.0 FG FPS`, `64.3–69.6 ms` real frame, `8.18 GB` VRAM, `5.45 GB` shared, `19.3 GB` commit / `6.2 GB` RAM WS). If it regresses, fix or revert autonomously.
5. **Update all 5 READMEs immediately on every verified win:**
   In the same commit, update the verified wins list and benchmark table in `README.md`, `README.ru.md`, `README.zh.md`, `README.es.md`, and `README.pt-BR.md` with exact measured numbers (FPS, real frame ms, `preparing` ms, VRAM, Shared, Commit/RAM).
6. **Land cleanly (parallel-safe, NO force-push!) & clean up:**
   - Run `cargo xtask policy` (must pass).
   - Commit as `RAZDA4A XEN <crossducking@users.noreply.github.com>`.
   - `git fetch xen dev && git rebase xen/dev` → push `feat/<short-slug>` (`git push -u xen feat/<short-slug>`). If rebase is conflict-free and tests + 180 s benchmark are green, fast-forward merge into `dev` (`git push xen HEAD:dev`). Never `git push --force` to `dev` or `main`.
   - When promoting a verified release to `main`, open a temporary worktree from `xen/main`, copy ONLY `RAZDA4A_XEN.exe`, updated `presets\*.bat`, and the 5 updated `README*.md`, commit non-force, and push (`git push xen HEAD:main`).
   - Remove temporary worktrees (`git worktree remove` + `git worktree prune`) and report exact before/after metrics to the user.

## Commands

```powershell
# 1. Windows 10 D3D12 Agility SDK (once per fresh checkout)
cargo xtask agility

# 2. Asset-policy verification (MUST pass before every commit)
cargo xtask policy

# 3. Targeted fast unit tests (MSVC toolchain ONLY, NEVER -gnu)
cargo test --offline --locked -p victory-formats --lib texture::
cargo test --offline --locked -p victory-hle --lib -- xmem:: gameinput::
cargo test --offline --locked -p victory-gpu --lib -- framegen:: wc:: merge_piece_tests:: detile_retry:: prelude_upload_tests:: upq::tests:: present_status_tests:: land:: upload:: queue::behind::tests:: placed_lookup_checks_the_description_on_cached_address_hits

# 4. Pure-release Zen 3 build
$env:RUSTFLAGS = "-C target-cpu=znver3"
cargo build --release --offline --locked -p victory

# 5. Pre-warm newly recorded shaders (requires heavy.lock)
target\release\victory.exe warm --used --jobs 6
```

## Boundaries

### Always
- Follow the Autonomous End-to-End Protocol above from start to finish in a single chat session.
- Run targeted `cargo test --offline --locked` + `cargo xtask policy` before every commit; keep zero new warnings.
- Update all 5 language READMEs immediately whenever a verified win, new preset, or improved benchmark result lands.
- Preserve `LICENSE` (`GPL-3.0-or-later`) and full source availability on `dev`.

### Ask First
- Modifying `Cargo.toml` or `Cargo.lock` (breaks `--offline --locked` builds).
- Deleting repositories, force-pushing shared branches, or changing `.github/workflows/ci.yml`.
- Resolving non-trivial merge conflicts if another parallel chat heavily modified the same lines in `host.rs`.

### Never
- Commit proprietary game dumps, executables, shaders, textures, captures, screenshots, or local user paths (`docs/asset-policy.md`, Rule #1).
- Use trademarked game/studio names, `game_scarlett`, or unrelated third-party fork names in READMEs, launchers, or docs (always use `your_executable.exe` / `ваш_запускной_файл.exe`).
- Develop source code directly on `main`, build with `-gnu`, enable `VICTORY_SCALAR_SPLITS=1`, or put `VICTORY_FILE_PHYSICAL` temp files on slow DRAM-less SATA SSDs.
- Break solved invariants: character faces (`VICTORY_COMPUTE_WAIT_MS=5000`), RDNA4 crash shield (`240cb516c1af0bc3`), FSR 4.1.2 ML (`VICTORY_FSR_PC=1`), HUD-less FSR 3.1.6 FG (`VICTORY_FRAMEGEN_HUDLESS=1`), tearing swapchain (`VICTORY_ALLOW_TEARING=1`, `VICTORY_VSYNC=0`), and 16 GB RAM OOM guards (`PRELUDES`, `recycle_landed`, `VICTORY_SEGMENT_TIGHT=92`).

## Architecture & Hot Files

| Area | Path | Notes |
|---|---|---|
| Submit loop, swapchain, OOM & preludes | `crates/victory-gpu/src/host.rs` | Shared hot file — check active worktrees first |
| FSR 3.1.6 FG & `HUDLessColor` ring | `crates/victory-gpu/src/framegen.rs` | 3-slot RGBA8 HUD-less capture + fallback |
| Command lists & recycling | `crates/victory-gpu/src/list.rs` | `recycle_landed`, stack `Few<Barrier, 16>` |
| Copy-back & AVX2 diffing | `crates/victory-gpu/src/shadow.rs`, `land.rs` | 128 B AVX2 `vptest`, lock-free work-stealing `Pool` |
| Descriptors & placed textures | `crates/victory-gpu/src/descriptor.rs`, `descriptor/host.rs`, `resource.rs` | Lock-free `mirror`, safe async flip, 64-sharded `PLACED_TEXTURES` |
| Async submit queue & CBV | `crates/victory-gpu/src/queue/behind.rs`, `port_cbv.rs` | Unlocked `unfold`, lock-free `wake()`, TLS stats |
| Physical memory & NVMe backing | `crates/victory-hle/src/modules/xmem.rs` | `VICTORY_FILE_PHYSICAL=1` (`FILE_FLAG_DELETE_ON_CLOSE`) |
| PC KBM controls | `crates/victory-hle/src/modules/gameinput.rs` | `VICTORY_KBM_PC=1`, `F2`/`~` mouse-look |
