# GUI launcher specification (v1)

Design for a later Windows launcher. It does not change the runtime by itself. Verified where it describes files that exist today. Corrections are marked **[branch]**.

- **[branch]** On Wave 11 `main`, `presets/4_Safe_Mode_8GB_12GB_VRAM.bat` sets frame generation off. The copy of that preset on `integration/upstream-port-2` currently sets frame generation **on**. When this spec says the official Safe Mode preset forces frame generation off, that is the Wave 11 file.
- **[branch]** `docs/en/` and the branch named in the sources table are not in this checkout. If that documentation pull request is not on the branch you are editing, use the five `README*.md` files here.
- **[branch]** `launcher_ui.ps1` lives on Wave 11 `main` (UTF-8 BOM). This integration branch wires a BOM-free copy so the experimental profile can be selected. The spec below still describes the Wave 11 dialog, plus that extra profile once the wiring commit is present.
- Names no Rust file reads are written without the `VICTORY_` prefix, as `NAME (not a switch in this tree)`.

---

# RAZDA4A XEN GUI Launcher — Implementation Spec

**Audience:** an AI coding agent running on Google Gemini, on a Windows 10 PC with an AMD GPU.  
**Product:** a new GUI starter/launcher for [RAZDA4A XEN](https://github.com/crossducking/RAZDA4A-XEN).  
**This document is a task brief, not an invitation to change the GitHub repo from the machine that wrote it.** Implement in your own working copy. Do not invent runtime switches. Do not bundle or download game files.

| Field | Value |
|---|---|
| Spec date | 2026-11-28 |
| Target OS | Windows 10 (19045) and Windows 11; the project's own test bench is Windows 10 |
| Target GPU | AMD RDNA2 / RDNA3 / RDNA4 (RX 6000 / 7000 / 9000) |
| Repo | `https://github.com/crossducking/RAZDA4A-XEN` |
| Branches studied (read-only) | `main` (Wave 11 prebuilt + current bat/ps1 launcher), `dev` (Rust source of the runtime), `cursor/readme-docs-split-9702` (PR #4 docs split: `docs/en/running.md`, `hardware.md`, `performance.md`, and siblings) |
| License | GPL-3.0-or-later (the launcher must be the same license if it ships in this repo) |
| Priority | **v1 is a news-worthy release milestone.** Ship a polished, minimal, explainable launcher that ordinary players can use. Do not stall v1 on later extras. |

Naming rules used by this project (also in `dev` `AGENTS.md`): never use trademarked game or studio names in the UI, README, or logs. Call the title **your game** or, if a proper noun is needed, *The Great Car-Borrowing Simulator from the Starry Studio*. The executable placeholder is `your_executable.exe`.

---

## 1. Goal and non-goals

### Goal

Replace the current `RAZDA4A XEN.bat` + `launcher_ui.ps1` flow **for normal users** with a modern, native-feeling Windows app that:

1. Asks where the user's game files are, validates them, and explains what is missing.
2. Detects hardware and suggests a safe preset.
3. Lets the user pick a preset or customize settings, with a plain-language explanation of every option (visible description panel, not tooltips alone).
4. Launches the **runtime** (`RAZDA4A_XEN.exe`) with the same environment variables and command line the bat scripts use today.
5. Offers tools (warm shaders, collect/save a sanitized bug report, backup saves, pagefile helper, open logs, check for a new wave).
6. Shows the installed **Wave number** prominently, and always labels **native FPS** vs **frame-generation FPS**.

The existing `.bat` / `.ps1` scripts must **keep working**. The GUI is an additional, better front end, not a deletion of the CLI path. External launchers and `RAZDA4A XEN.bat --no-gui` remain supported.

### Non-goals

- Do not rewrite or replace `RAZDA4A_XEN.exe` (that is the game runtime, built from `apps/victory` on `dev`).
- Do not download, host, or bundle game files, assets, or the game executable.
- Do not auto-download runtime updates that include game content. A "new wave" check is a notice plus a link, not a silent updater.
- Do not invent `VICTORY_*` switches, profiles, or hidden FPS unlocks. If a setting is not a real switch the runtime already reads, it does not appear as a control — mark it `TODO` and verify in `dev` source.
- Do not enable `VICTORY_SCALAR_SPLITS=1` (forbidden in `AGENTS.md`).
- Do not remove the RDNA4 crash-shader shield (`VICTORY_SKIP_PIPELINES=240cb516c1af0bc3`) or casually lower `VICTORY_COMPUTE_WAIT_MS` (character faces).
- Do not add telemetry, analytics, crash beacons, or any network call that sends machine data without an explicit, previewed, opt-in action.
- Do not build a Linux/Proton GUI in v1 (`launch_linux_proton.sh` stays as-is).
- Do not require a Microsoft Store / MSIX install in v1. The app must run unpackaged from the extracted repo folder.
- Do not tell users to open GitHub Issues. Issues are disabled on this repository (see PR #4 docs).

---

## 2. v1 (releasable) vs later extras

v1 is the news-worthy release. It must look finished, explain itself, and cover the real failure modes people hit. Everything in the **v1** column is required for the first public GUI. Everything in **later** is specified so you can leave hooks, but it must not block the release.

| Area | v1 (ship this) | Later (not required for release) |
|---|---|---|
| First-run wizard | Game folder/exe picker, validation, GPU/VRAM/RAM/pagefile detection, NVIDIA/unsupported messaging, suggested preset | Auto-discover game installs across disks |
| Settings | Four official profiles + Frame Gen toggle + a customizer for the switches those profiles actually change; save/load/share custom presets; reset to defaults; visible explanation panel | Full expert matrix of every `VICTORY_*` switch in the runtime (hundreds exist; most are debug) |
| Tools | Warm shaders, Export/save sanitized bug report to a zip on disk, backup/restore saves, pagefile helper (with admin consent), open logs folder, "check for a new wave" notice+link | In-app update of the runtime, remote bug-report upload |
| Bug report | One-click **Report a bug**: short form, collect metrics + logs + optional screenshot, sanitize, **preview**, save a `.zip` locally (works offline) | Opt-in send to a server, with consent |
| FPS labeling | Launcher always says native vs frame-gen; recommend a small companion patch to the F3 overlay (see the overlay section) | Live in-launcher FPS meter while the game runs |
| Languages | English UI is enough for v1. Keep the door open for the repo's five languages (en, ru, zh, es, pt-BR) | Localized UI |
| Linux | Out of scope | Proton GUI |
| Audio quality sliders | Do not invent them. There is no `MUTE (not a switch in this tree)` / `VOLUME (not a switch in this tree)`. If a user reports missing TV/radio/phone audio, the bug form should collect that — it is not a launcher setting today | Whatever audio fix lands in the runtime, then expose it |
| Resolution / upscaler extras | Optional **Advanced** row only if you verify `VICTORY_OUTPUT`, `VICTORY_RENDER_SCALE`, `VICTORY_UPSCALER`, `VICTORY_TITLE_FPS` against `dev`. If unverified, omit from v1 | Full display-mode UI |
| Visual design | Polished dark theme, butterfly identity, unslop rules, screenshots of every screen | Motion design, extra themes |

**Definition of done for v1:** a person who has never used the project can download `main`, double-click the launcher, point it at their game folder, understand the suggested preset, launch successfully (or get a human explanation of why not), see which Wave they are on, and export a sanitized bug report zip — without ever touching a `.bat` file or running the game exe directly.

---

## 3. Current launch flow (facts — implement against these)

Studied read-only from `main` (and the same scripts on the docs-split branch). Do not "improve" the launch command or drop environment variables in v1.

### 3.1 What each file is

| File on `main` | Role |
|---|---|
| `RAZDA4A_XEN.exe` | **The runtime**, not a GUI. It is the release build of the `victory` Cargo package (`apps/victory` on `dev`), renamed at packaging. CLI front end (`clap`): `run`, `warm`, `config`, and related commands. Wave 11 SHA-256: `4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622` (see `SHA256SUMS.txt`). Source exists on `dev` only; `main` ships the prebuilt exe and DLLs. |
| `RAZDA4A XEN.bat` | Current Windows entry point. Optionally opens `launcher_ui.ps1`, then sets a large block of `VICTORY_*` variables and starts the runtime. Supports `--no-gui`. |
| `launcher_ui.ps1` | Dark WinForms dialog: hardware banner, Browse for exe, Frame Gen checkbox, profile combo, Warm Shaders, Export Logs, Launch/Cancel. Writes `config.bat` and `launcher_prefs.bat` as UTF-8 **without BOM**. |
| `config.bat` | Only `GAME_DIR` and `GAME_EXE`. Default placeholder paths. |
| `launcher_prefs.bat` | Created by the UI: `XEN_FRAMEGEN` (`0`/`1`) and `XEN_PROFILE` (`balanced` / `low_commit` / `cbv_exp` / `safe`). |
| `presets\1_Low_Commit_NVMe_16GB.bat` | Balanced + `VICTORY_FILE_PHYSICAL=1`. |
| `presets\2_Max_FPS_Experimental_CBV.bat` | Balanced + `VICTORY_PORT_CBV=1`, `VICTORY_PORT_CBV_PLACE=1`, `VICTORY_PORT_CBV_SHADOW=2`. |
| `presets\3_Native_No_FG_Zero_Latency.bat` | Balanced with `VICTORY_FIDELITYFX_FRAMEGEN=0` (and no framegen ahead/hudless vars). |
| `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` | 8–12 GB VRAM live-budget fit (PR #1). FG off, VSync on, tighter budgets. |
| `warm_shaders.bat` | `RAZDA4A_XEN.exe warm --used --jobs 6` after a short env setup. Must not run while the runtime is already running. |
| `collect_logs.bat` | Sanitized hardware + prefs + last 250 lines of `logs\last_run.log`. Currently only redacts `C:\Users\<USERNAME>`. Writes `logs\bug_report_YYYYMMDD_HHMMSS.txt` and opens it in Explorer. Text still tells users to attach it to GitHub Issues — **do not copy that sentence** (Issues are disabled). |
| `backup_saves.bat` | Backup/restore `%ROOT%\data` to `%ROOT%\saves-backup\latest` plus a timestamped snapshot. |
| `pagefile_16gb_ram.ps1` | Admin-only: set `C:\pagefile.sys` to 16384–32768 MB, then asks for a reboot. |
| `launch_linux_proton.sh` | Linux path. Out of v1 GUI scope. |
| `D3D12\D3D12Core.dll` | Bundled Agility SDK 620 (needed on Windows 10). |
| `fidelityfx\amd_fidelityfx_framegeneration_dx12.dll`, `amd_fidelityfx_loader_dx12.dll`, `amd_fidelityfx_upscaler_dx12.dll` | AMD FidelityFX PC DLLs. Required when `VICTORY_FSR_PC=1` or frame gen is on. |
| `docs/en/*.md` (PR #4 branch) | How to run, hardware FAQ, performance. Prefer these over the long README still on `main` if PR #4 has been merged by the time you work. |

### 3.2 Exact runtime command used today

From `RAZDA4A XEN.bat` and every preset (working directory = repo root):

```bat
start "" /WAIT /B "%ROOT%\RAZDA4A_XEN.exe" run --console series-s "%GAME_EXE%" --execute
```

The GUI must spawn the equivalent:

- Program: `<root>\RAZDA4A_XEN.exe`
- Arguments: `run --console series-s "<full path to GAME_EXE>" --execute`
- Working directory: `<root>` (the folder that contains `RAZDA4A_XEN.exe`, `D3D12\`, `fidelityfx\`)
- Environment: first **clear every inherited `VICTORY_*` variable**, then set the block in the switch catalog for the chosen profile. This matches `for /f "delims==" %%V in ('set VICTORY_ 2^>nul') do set "%%V="` in the bats.

The current short README (PR #4) says users may start `RAZDA4A_XEN.exe` or the bat. That is misleading: `RAZDA4A_XEN.exe` with no arguments is a CLI (clap help), and it will not set the `VICTORY_*` block. **The new GUI is the thing users should double-click.** Never start the game executable directly.

### 3.3 Known bugs the GUI must not repeat

From community findings (`menedzheo_findings_wave13.md`) and repo inspection:

1. **UTF-8 BOM in `.bat` files** on Wave 11 `main` (`warm_shaders.bat`, `config.bat`, `RAZDA4A XEN.bat`) causes `'ï»¿@echo off' is not recognized`. `launcher_ui.ps1` already writes `config.bat` **without BOM**. The GUI must write every text file it creates (`config.bat`, `launcher_prefs.bat`, presets, reports) as UTF-8 **without BOM**, CRLF for `.bat`.
2. People launch the **game exe** directly and get `xmem.dll` / `d3d12_xs.dll not found`. Those DLLs are provided by the runtime layer; they are not files the user is supposed to place in the game folder. The GUI must prevent this mistake and explain it.
3. People do not know which Wave they are on, so results come in as "old wave." Show the Wave number prominently.
4. People confuse **frame-gen displayed FPS** with **native FPS** (the 62–63 figure is FG; native on the 7800X3D reference is ~31). Label both, always.
5. An incomplete extracted RAZDA4A-XEN folder (missing `D3D12`, `fidelityfx`, or `presets`) will fail in confusing ways. Validate the runtime folder before launch.

---

## 4. Tech stack recommendation

### Recommendation: .NET 8 + WPF, self-contained single-file exe (preferred)

Build a Windows unpackaged WPF app targeting `net8.0-windows`, published as:

```text
dotnet publish -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -p:EnableCompressionInSingleFile=true
```

Output name: **`RAZDA4A XEN Launcher.exe`** (spaces allowed; do **not** name it `RAZDA4A_XEN.exe` — that name is taken by the runtime).

**Why this, for a Gemini agent on Windows 10:**

- Runs on the project's test bench (Windows 10 19045) without a Microsoft Store package, without WinUI 3's Windows App SDK bootstrapper pain, and without depending on a specific WebView2 runtime.
- One exe the user drops next to `RAZDA4A_XEN.exe`. No installer required for v1.
- Folder/file pickers, process launch, registry/CIM hardware queries, UAC elevation for the pagefile helper, and zip export are all straightforward in .NET.
- WPF can look modern if you follow the design rules (custom chrome, not default gray Aero).
- Many coding agents are reliable at C# / XAML.

### Acceptable alternative: Tauri 2 + vanilla HTML/CSS/TypeScript

Choose Tauri only if you are clearly faster and more confident producing a beautiful UI in HTML/CSS than in WPF, **and** you verify WebView2 is present on Windows 10 19045 (it usually is, via Edge). Still ship a single launcher exe beside the runtime. Do not pull in a heavy UI kit (no generic dashboard template). Same design rules.

### Do not use

- Electron (huge, hard to justify as a "single exe").
- WinUI 3 / MSIX as the v1 vehicle (unpackaged Win10 support is a time sink).
- A PowerShell or WinForms restyle of `launcher_ui.ps1` (that is what we are replacing).
- A console-only TUI.

The launcher is a **separate binary**. It must not be compiled into `RAZDA4A_XEN.exe`. If you add the project to the `dev` tree, keep it in something like `apps/launcher/` (or a sibling folder on `main` packaging), GPL-3.0-or-later, no game assets inside.

---

## 5. Coexistence with `RAZDA4A_XEN.exe` and the bat scripts

Layout after a user extracts `main`:

```text
RAZDA4A-XEN\
  RAZDA4A XEN Launcher.exe     ← new; the double-click target
  RAZDA4A_XEN.exe              ← runtime (unchanged)
  RAZDA4A XEN.bat              ← keep; may open the new GUI if present, else launcher_ui.ps1
  launcher_ui.ps1, config.bat, presets\, warm_shaders.bat, collect_logs.bat, ...
  D3D12\D3D12Core.dll
  fidelityfx\*.dll
  cache\, data\, logs\         ← created at first launch
  WAVE.json                    ← add this small sidecar if it does not exist
  SHA256SUMS.txt
```

Rules:

- The launcher locates `<root>` as the directory of `RAZDA4A XEN Launcher.exe`. If the runtime is not next to it, say so and refuse to launch. Do not search the whole disk for a game dump without asking.
- Read and write the **same** `config.bat` and `launcher_prefs.bat` the bats use, so a user can mix the GUI and `--no-gui` / preset bats without losing paths.
- Optionally, if `RAZDA4A XEN.bat` is started without `--no-gui` and `RAZDA4A XEN Launcher.exe` exists, the bat may start the new GUI and exit. Keep a fallback to `launcher_ui.ps1` if the new exe is absent. This is a small, welcome bat change; if you cannot touch the bat (BOM concerns), it is enough that users double-click the new exe.
- Do not delete or overwrite `RAZDA4A_XEN.exe`, FidelityFX DLLs, or `D3D12Core.dll`.
- `cache\`, `data\`, and `logs\` stay under `<root>`, matching the bats (not `%LOCALAPPDATA%\victory\...`, even though that is the runtime default when env vars are unset).

---

## 6. Architecture of the app

Three areas, always available after first run:

1. **Home** — Wave badge, hardware summary, game path, suggested/selected preset, Frame Gen toggle, primary **Launch** button, short status.
2. **Settings** — profiles + customizer, with a persistent description panel.
3. **Tools** — Warm shaders, Report a bug, Backup saves, Pagefile helper, Open logs, Check for a new wave.

A **first-run wizard** runs when `config.bat` is missing, `GAME_EXE` is the placeholder, or the saved path does not exist. The user can re-open the wizard from Settings.

Suggested window size: about 960×640, resizable down to a readable minimum (~800×520), not a tiny 540×365 dialog like today's WinForms UI. One window, not a stack of modal popups (modals only for destructive confirms and the bug-report preview).

---

## 7. Show the installed Wave number (v1, prominent)

Community request: people do not know which Wave they have, so almost every result arrives as "old wave."

### 7.1 Source of truth

There is **no** Wave field inside `RAZDA4A_XEN.exe` today. Do not invent a number from marketing copy in memory.

Add (or consume) a small sidecar next to the runtime, **`WAVE.json`**, UTF-8 no BOM, for example:

```json
{
  "product": "RAZDA4A XEN",
  "wave": 11,
  "channel": "main",
  "runtime": "RAZDA4A_XEN.exe",
  "sha256": "4ab8fcf7e2614cb669ad54069011d8cb0bed74b3eac7b994854c278f2a862622",
  "git": null,
  "notes": "Wave 11 (main). Wave 12 is in progress on dev — not released, no date."
}
```

If you are also preparing the packaging commit: generate this file when copying the release exe to `main`, and keep `sha256` in sync with `SHA256SUMS.txt`.

### 7.2 What the UI shows

- A persistent badge in the header, large enough to be in a screenshot: **Wave 11** (or whatever `wave` is), plus channel if not `main` (e.g. `dev`).
- A details line or "About" row: SHA-256 of the actual `RAZDA4A_XEN.exe` on disk (compute it), and whether it **matches** `WAVE.json` / `SHA256SUMS.txt`.
- If `WAVE.json` is missing: show **Wave unknown**, the computed SHA-256, and "This build has no WAVE.json. Compare SHA-256 with SHA256SUMS.txt." Never guess "Wave 11" just because the README on GitHub says so — the user's folder might be older.

This badge belongs on Home, on the bug-report form, and in the exported report.

---

## 8. First-run wizard (v1)

Steps, in order. The user can go back. No dark patterns, no "I agree to telemetry."

### Step A — Welcome

One short paragraph: this launcher starts the RAZDA4A XEN runtime with your own copy of the game. This repo does not include game files. You will pick the folder that already contains your game. AMD RDNA2+ is required today.

### Step B — Where are your game files?

Provide **both**:

- **Choose folder…** (primary, folder picker).
- **Choose executable…** (file picker, `*.exe`).

If the user picks a folder, look for a reasonable executable inside it (see validation below). If they pick an exe, set `GAME_DIR` to that exe's parent and `GAME_EXE` to the exe.

**Common mistakes to detect immediately** (do not wait for a crash):

| What they picked | What you say |
|---|---|
| `RAZDA4A_XEN.exe` or `RAZDA4A XEN Launcher.exe` or any exe inside the RAZDA4A-XEN runtime folder | "That is the RAZDA4A XEN runtime/launcher, not your game. Choose the game executable in your unpacked game folder." |
| An exe named like a setup/installer, or a 0-byte file | "This does not look like the game executable." (be conservative; only block on certainty) |
| Folder is the RAZDA4A-XEN repo/extract itself (`RAZDA4A_XEN.exe` + `fidelityfx` present, no `MicrosoftGame.config`) | "This is the RAZDA4A XEN folder, not your game folder." |
| Path is the placeholder `D:\Path\To\Your\GameFolder` | Treat as unset. |

Validate the game package and list **exactly** what is missing, in plain language, with a way to choose again.

### Step C — Hardware

Run the detection in the next section. Show GPU name, VRAM, RAM / free virtual memory, pagefile, OS. Then the compatibility verdict (AMD RDNA2+, NVIDIA message, 8 GB Safe Mode, 6 GB experimental, too little VRAM, low commit, HDD warning if you can detect it).

### Step D — Suggested preset

Suggest one official profile and a Frame Gen default using the same rules as `launcher_ui.ps1`. Explain why in the description panel. The user can override before finish.

### Step E — Confirm and save

Write `config.bat` and `launcher_prefs.bat` (UTF-8 no BOM). Create `cache`, `data`, and `logs` if missing. Land on Home.

---

## 9. Hardware detection (reuse the current method)

Match `launcher_ui.ps1` so advice stays consistent:

- GPU name and VRAM: registry `HKLM\SYSTEM\ControlSet001\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*`, `DriverDesc`, `HardwareInformation.qwMemorySize` (fallback `HardwareInformation.MemorySize`, including the `byte[]` case).
- AMD if description matches `AMD` or `Radeon`.
- RDNA1 or older if description matches `RX 5xxx` / older `RX 4xx/5xx` / `Vega` / `R9` / `R7` (same regex as the ps1). These lack DX12 Ultimate mesh shaders required by this runtime.
- RAM / free virtual memory: `Win32_OperatingSystem` (`TotalVisibleMemorySize`, `FreeVirtualMemory`).
- Pagefile: `Win32_PageFileUsage` (allocated and peak), as `collect_logs.bat` does.
- OS: caption + build number.
- CPU: `Win32_Processor` (name, cores, threads) — show on Home and in bug reports.
- Driver version from the same display-class registry key.

Optional (nice in v1 if reliable, otherwise TODO): detect whether the system drive is HDD vs SSD vs NVMe (`Win32_DiskDrive` / `MSFT_PhysicalDisk`). HDD is unsupported for this runtime. `VICTORY_FILE_PHYSICAL=auto` also refuses to auto-enable on SATA SSD/HDD.

### 9.1 NVIDIA, Intel Arc, and other unsupported GPUs

If no AMD GPU is present, or a non-AMD GPU is the one with the VRAM reading:

- Do **not** launch.
- Explain clearly: this build is **AMD RDNA only for now**. An NVIDIA version is planned after the AMD build is finished and optimized. **No date.** Until then, a community NVIDIA fork exists: [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA). Intel Arc is not supported.
- Offer a link button (opens the default browser). Do not auto-download that fork.

If the GPU is AMD but RDNA1 or older: explain that DX12 Ultimate mesh shaders (RDNA2, RX 6000 or newer) are required, and block launch.

### 9.2 Preset suggestion (same thresholds as `launcher_ui.ps1`)

| Condition | Suggestion |
|---|---|
| VRAM less than 5.5 GB (and VRAM was detected) | Unsupported / very likely OOM or black screen. Do not recommend a "make it work" preset. |
| VRAM 5.5–6.5 GB | Safe Mode, Frame Gen **off**. Experimental / rough. |
| VRAM 8.5 GB or less | Safe Mode (`safe`). Frame Gen default **off** on first run (ps1 default: FG off when low VRAM). User may turn FG on; explain the trade-off. |
| Free virtual memory less than 12 GB | Suggest Low-Commit NVMe (`low_commit`) and mention the pagefile helper. |
| Otherwise, VRAM detected | Balanced (`balanced`), Frame Gen on. |

If the user already has `launcher_prefs.bat`, respect it and do not silently overwrite on upgrade.

---

## 10. Path and package validation (v1, hard guards)

Two different folders must be valid. Mixing them up is the most common user error.

### 10.1 Runtime folder (`<root>`) — must be present before launch or warm

Require at least:

- `RAZDA4A_XEN.exe`
- `D3D12\D3D12Core.dll`
- `fidelityfx\amd_fidelityfx_upscaler_dx12.dll`
- `fidelityfx\amd_fidelityfx_loader_dx12.dll`
- `fidelityfx\amd_fidelityfx_framegeneration_dx12.dll` (required if Frame Gen will be on; still warn if missing even when FG is off)
- `presets\` (the four official preset bats, or a clear warning if this is a source-only `dev` checkout)

If any of these are missing, name the missing path and say: "This does not look like a complete RAZDA4A XEN extract. Re-download the official zip and keep the D3D12, fidelityfx, and presets folders next to RAZDA4A_XEN.exe." Do not start the runtime.

### 10.2 Game folder — user-supplied, never downloaded

Minimum checks:

- The chosen executable exists and is a file.
- It is **not** the runtime or the launcher (compare full paths and file names with `<root>\RAZDA4A_XEN.exe` and the launcher exe).
- The parent folder is **not** `<root>`.
- `MicrosoftGame.config` exists next to the exe. The runtime reads this for the title id and storage quota (`apps/victory/src/run.rs`). If it is missing, say: "The game's config file (MicrosoftGame.config) was not found next to the executable. The folder is probably incomplete — copy every file of your unpacked game next to the executable, not just the .exe."
- Optional but valuable: after identifying the exe, you may peek at imports / sibling DLLs the way the runtime does. `run.rs` will refuse to `--execute` if title DLLs imported by the exe are missing (`the title's own DLLs are missing: … copy every file of the package next to the executable`). If you implement a preflight, list missing **sibling** DLLs in ordinary language. **Do not** tell the user to obtain `xmem.dll` or `d3d12_xs.dll` — those are implemented by the runtime, not shipped in the game folder.

Asset-policy note (`docs/asset-policy.md`): the runtime also wants game paths **outside the source repository** when running from a `dev` checkout. For a `main` extract this is usually a non-issue (the extract has no Rust source). Still, keep `GAME_DIR` pointing at the user's game, never at `<root>`.

### 10.3 "I launched the game exe and got `xmem.dll not found`"

This is not a missing-file bug in the game folder. It means the game was started **without** the runtime.

In the wizard, Home, and the error glossary:

> Do not double-click your game's .exe. That starts the game without RAZDA4A XEN, and Windows will look for console libraries such as xmem.dll and d3d12_xs.dll that only the runtime provides. Always start the game with this launcher (or RAZDA4A XEN.bat).

The Launch button must only ever spawn `RAZDA4A_XEN.exe` as specified in the launch-command section.

---

## 11. Home / launch screen

Contents:

- Header: product name, **Wave badge**, small butterfly mark.
- Hardware one-liner: GPU, VRAM, RAM, OS. Compatibility chip (OK / Safe Mode recommended / blocked).
- Game path, with Change… (re-opens picker / wizard).
- Profile selector and Frame Gen toggle (same four official profiles).
- Primary **Launch** button. Disabled with an explanation when validation fails or the GPU is blocked.
- Secondary: Report a bug, Open logs, Warm shaders.
- A status region: idle / launching / running / exited / failed, plus the last human-readable error.
- A short controls reminder: **F2** or **~** toggles mouse-look (Esc unlocks the cursor); **F3** toggles the FPS overlay. When mentioning the overlay, say that the large number may be a **frame-gen** figure unless the overlay has been patched.

While the game is running, disable Launch, keep the window usable for "Open logs" and "Report a bug," and show "Runtime is running." When it exits, show the process exit code and, if non-zero, a pointer to the log and the bug-report tool.

---

## 12. Settings screen and preset customizer

### 12.1 Official profiles

Map 1:1 to today's `XEN_PROFILE` + Frame Gen. Present them with plain names and a one-sentence promise. Details go in the description panel when the row is selected.

| UI name | `XEN_PROFILE` | What it is | Risk |
|---|---|---|---|
| Balanced (default) | `balanced` | Auto-VRAM, FG optional, tearing, helper priority on, 4 pipeline workers. The default for 12–16+ GB cards. | Still heavy on 8 GB cards. |
| Low-Commit NVMe (16 GB RAM) | `low_commit` | Same as Balanced, but `VICTORY_FILE_PHYSICAL=1` (file-backed physical pages on NVMe, delete-on-close, typically under `%LOCALAPPDATA%\victory\physical`). Cuts commit charge on 16 GB RAM machines. | Do not use on HDD or a slow DRAM-less SATA SSD (`AGENTS.md`). Prefer NVMe. |
| Max FPS Experimental (CBV) | `cbv_exp` | Balanced plus in-place constant-buffer reads (`VICTORY_PORT_CBV=1`, `VICTORY_PORT_CBV_PLACE=1`, `VICTORY_PORT_CBV_SHADOW=2`). Can shave CPU submit time. | Experimental. Visual or stability issues possible. Easy to turn off. |
| Safe Mode (8–12 GB VRAM) | `safe` | Live-budget fit (PR #1, verified on RX 7600 8 GB: 3.67 → 12.7 native / 24.86 FG). VSync on, tearing off, helper priority off, 2 pipeline workers, tighter texture/staging/card-buffer budgets, `VICTORY_PORT_DESCRIPTORS=0`, `VICTORY_FILE_PHYSICAL=1`. Official Safe Mode preset bat also forces Frame Gen **off**; the main bat's `safe` profile leaves FG to the checkbox. **Follow the main bat:** FG is an independent toggle. In the description, mention that the standalone preset bat turns FG off, and that 8 GB users who want FG can try it but should watch VRAM. | Lower texture headroom. VSync adds latency but avoids tearing and some 8 GB stalls. |

The Native No-FG preset (`presets\3_...`) is **not** a fourth `XEN_PROFILE` in the main bat — it is Balanced with Frame Gen off. In the GUI, that is simply Balanced (or any profile) with the Frame Gen checkbox cleared. You may offer "Native, no frame generation" as a shortcut that selects Balanced + FG off.

### 12.2 Frame Generation toggle

Sets `XEN_FRAMEGEN` / `VICTORY_FIDELITYFX_FRAMEGEN` to `1` or `0`.

Plain language (description panel):

- **On:** AMD FidelityFX Frame Generation (2× displayed frames, HUD-less colour so the HUD is not interpolated). The number you *see* is roughly double the native FPS. Input still follows native frames, so latency is higher than native. The project's reference on RX 9060 XT + 7800X3D is about **31 FPS native / 62–63 FPS with frame generation**. Always write it that way.
- **Off:** only real frames, lowest input latency. Use this for testing, for 8 GB troubleshooting, or if FG artifacts bother you.

Related vars the bat sets when FG is on (still set them when FG is off in the main bat, except preset 3 which omits ahead/hudless): `VICTORY_FRAMEGEN_AHEAD=1`, `VICTORY_FRAMEGEN_HUDLESS=1`. Keep the same behavior as `RAZDA4A XEN.bat` for the profile in use. HUD-less is on by default in the runtime even if unset (`VICTORY_FRAMEGEN_HUDLESS=0` disables it).

### 12.3 Customizer

A "Customize" mode starts from the currently selected official profile (copy its env block) and lets the user change **grouped** options. Every control maps to a real switch from the catalog below. Changing a value marks the preset **Custom**.

Groups, left side; **description panel** on the right (required, not a tooltip):

1. **Graphics / FSR / Frame generation** — FG, VSync, allow tearing, FSR PC (normally left on), HUD-less FG, panel (F3 overlay) on/off and corner, ambient values if you expose them.
2. **Memory / VRAM** — Safe Mode budget fields, `FILE_PHYSICAL`, segment tight, staging/window budgets, card buffers, texture room/keep percents.
3. **Performance** — helper priority, submit frames, pipeline workers, port descriptors, experimental CBV trio, join parts.
4. **Audio** — v1: no fake sliders. A short note: "This build has no launcher audio quality switches. If TV, radio, or phones are silent, use Report a bug." If you verify `VICTORY_XMA_REFILL` and believe it is user-safe, it may go under Advanced with the source comment (off by default; do not turn it on just to fill the tab).
5. **Debug / logs** — `VICTORY_LOG` (`info` default), `VICTORY_LOG_FILE` (default `<root>\logs\last_run.log`), open logs folder. Do not expose diagnostics-only switches (`VICTORY_WATCH`, `VICTORY_POKE`, `VICTORY_CRASH_SAVE`, `VICTORY_D3D12_DEBUG`, A/B toggles) in v1.

Each control in the description panel shows:

- The plain-language effect.
- The performance / quality trade-off.
- The risk (and when to leave the default).
- The real variable name, in secondary text (`VICTORY_VSYNC`), so a power user can confirm it.

**Reset to defaults:** restore the official block for the selected profile (or Balanced if custom-from-scratch). Confirm if the user has unsaved custom values.

### 12.4 Save / load / share custom presets

File format: UTF-8 JSON, no BOM, extension **`.razda4a-preset.json`**. Version the schema.

```json
{
  "format": "razda4a-preset",
  "format_version": 1,
  "name": "My 8 GB city attempt",
  "based_on": "safe",
  "framegen": false,
  "env": {
    "VICTORY_VSYNC": "1",
    "VICTORY_FILE_PHYSICAL": "1"
  },
  "notes": "Optional user note"
}
```

Rules:

- `env` keys must be `VICTORY_*` names from the known catalog. Unknown keys: warn and ignore; never set an unknown variable silently.
- Prefer writing the **full applied block** so a shared file is self-contained.
- Also offer **Export as .bat** in the same shape as `presets\*.bat`, so existing `--no-gui` users can drop it into `presets\`. UTF-8 no BOM, CRLF.
- Load: file picker. Share: "Show in folder" / copy path. This is how users share presets (no cloud).
- Store the last custom preset path in `launcher_prefs.bat` or a new `launcher_state.json`. Safer for v1: keep `XEN_PROFILE` as the base official profile, and let the GUI apply extra env on top only when launching from the GUI (so unknown `XEN_PROFILE=custom` does not break the bats).

---

## 13. Switch catalog — only real switches

The runtime on `dev` reads on the order of **four hundred** `VICTORY_*` names (debug, benches, shader translator, diagnostics). **The GUI must not list them all.**

A switch is allowed in the UI only if:

1. It is set by `RAZDA4A XEN.bat` or an official preset, or
2. It is a documented path/log/display switch you have confirmed in `dev` source, or
3. You personally verified the read site in `dev` and wrote an explanation from that comment — not from guesswork.

If you are unsure, mark **TODO: verify in `dev`** and leave it out of v1.

Semantics of on/off (from `crates/victory-gpu/src/switch.rs`):

- Many switches are **off when unset**, and on for any value other than `0`. **[branch]** The original notes used a placeholder switch name here; that placeholder is not a real switch, so it is not repeated.
- Some are **on unless set to `0`** (`toggled!`, or unset-or-not-zero) — e.g. `VICTORY_HELPER_PRIORITY`, `VICTORY_FRAMEGEN_HUDLESS`.
- Some require the exact string `1` (`VICTORY_FSR_PC`, `VICTORY_ALLOW_TEARING`, `VICTORY_KBM_PC`).
- `VICTORY_FILE_PHYSICAL` is `1` / `0` / `auto` (not a boolean).
- `VICTORY_VSYNC` is `1` / `0` / unset (`None` = runtime default).

### 13.1 Paths and logging (always set, like the bat)

| Variable | Bat value | Meaning |
|---|---|---|
| `VICTORY_GAME_DIR` | `%GAME_DIR%` | User's game folder. |
| `VICTORY_CACHE_DIR` | `%ROOT%\cache` | Shader / pipeline cache. Safe to delete; will be rebuilt (long first launch). |
| `VICTORY_DATA_DIR` | `%ROOT%\data` | Persistent saves / title storage. This is what Backup saves copies. |
| `VICTORY_FIDELITYFX_DIR` | `%ROOT%\fidelityfx` | Folder with the three AMD FidelityFX DX12 DLLs. |
| `VICTORY_LOG` | `info` | Tracing filter. Release builds cannot go below info (no `diagnostics` feature). |
| `VICTORY_LOG_FILE` | `%ROOT%\logs\last_run.log` | Mirrors logs to a file (plain text, no ANSI). |

Also known, not needed in the v1 form: `VICTORY_CONFIG` (alternate `victory.toml`), `VICTORY_TEST_TITLES`, `VICTORY_FILE_PHYSICAL_DIR` (override directory for file-backed pages; default `%LOCALAPPDATA%\victory\physical`).

### 13.2 Set by every official Windows launch (balanced base)

Use these exact values as the Balanced baseline (copied from `RAZDA4A XEN.bat` on `main`, 2026-11-28). Then apply the profile overrides.

**Graphics quality**

| Variable | Balanced | Notes |
|---|---|---|
| `VICTORY_FSR_PC` | `1` | Load the PC FSR upscaler from `VICTORY_FIDELITYFX_DIR`. Required for the intended picture. Leave on. |
| `VICTORY_AMBIENT` | `0.75` | Ambient lighting scalar. |
| `VICTORY_AMBIENT_AIR` | `1.5` | Air / atmosphere related ambient. |
| `VICTORY_AMBIENT_REFLECT` | `1.0` | Reflections related ambient. |
| `VICTORY_SCALAR_SPLITS` | `0` | **Always 0. Never offer a control that sets this to 1.** |

**Frame gen / presentation**

| Variable | Balanced | Notes |
|---|---|---|
| `VICTORY_FIDELITYFX_FRAMEGEN` | `XEN_FRAMEGEN` | `1` or `0`. |
| `VICTORY_FRAMEGEN_AHEAD` | `1` | Set by the main bat whenever it sets the FG block. |
| `VICTORY_FRAMEGEN_HUDLESS` | `1` | HUD-less colour for FG (avoids HUD ghosting). Runtime default is on if unset. |
| `VICTORY_ALLOW_TEARING` | `1` | Exact `1` enables tearing. Safe profile sets `0`. |
| `VICTORY_VSYNC` | `0` | `1` / `0`. Safe profile sets `1`. |
| `VICTORY_FRAME_INTERVAL_MS` | `16.667` | Host frame clock (~60 Hz). The title's own 30 FPS limiter can be overridden via `VICTORY_TITLE_FPS` (advanced, verify before exposing). |

**CPU / faces / queues** (keep as in the bat; only expose a subset in the customizer)

| Variable | Balanced | Notes |
|---|---|---|
| `VICTORY_COMPUTE_WAIT_MS` | `5000` | Wait budget for compute pipelines (character face / head blend-shape). Runtime default if unset is 10000 ms. **Do not lower casually.** |
| `VICTORY_COMPUTE_LATE_WAIT_MS` | `400` | Late compute wait. |
| `VICTORY_HELPER_PRIORITY` | `1` | On unless `0`. Raises helper threads to above-normal so 6-core CPUs do not stall submit. Safe sets `0` so background apps are less likely to freeze. Source: `crates/victory-gpu/src/cores.rs`. |
| `VICTORY_SUBMIT_FRAMES` | `2` | Safe: `1`. |
| `VICTORY_CARD_STEPS` | `2` | |
| `VICTORY_PLACED_ORDER` | `1` | |
| `VICTORY_FETCH_CHECK` | `1` | |
| `VICTORY_TEXTURE_NO_WAIT` | `1` | |
| `VICTORY_TEXTURE_LOOK_MS` | `350` | |
| `VICTORY_RELAY` | `2` | |
| `VICTORY_WIDE_BACK` | `1` | On by default in runtime; `0` turns it off. |
| `VICTORY_WIDE_SPIN` | `100` | |
| `VICTORY_CARD_REHASH_RUNS` | `1` | |
| `VICTORY_JOIN_COPIES` | `1` | |
| `VICTORY_JOIN_BARRIERS` | `1` | |
| `VICTORY_UP_QUEUE` | `1` | |
| `VICTORY_INDIRECT_WINDOW` | `0` | |
| `VICTORY_RELAY_COPY` | `1` | |
| `VICTORY_INDIRECT_BATCH` | `1` | |
| `VICTORY_MAP_THREADS` | `1` | |
| `VICTORY_CARD_AWAKE` | `1` | |
| `VICTORY_SHOW_AFTER` | `1` | |
| `VICTORY_KEEP_SHOWN` | `2` | |
| `VICTORY_TEXTURE_AWAY_MS` | `10000` | |
| `VICTORY_THIN` | `1` | |
| `VICTORY_THIN_THREADS` | `4` | |
| `VICTORY_LAND_THREADS` | `3` | |
| `VICTORY_LAND_SKIP` | `1` | |
| `VICTORY_JOIN_PARTS` | `16` | Safe: `12`. |
| `VICTORY_UPLOAD_AHEAD` | `1` | |
| `VICTORY_HOLES_ONCE` | `30` | |
| `VICTORY_FILE_CACHE` | `1` | |
| `VICTORY_WC_NO_BACK` | `1` | |
| `VICTORY_CLOSED_PLAN` | `1` | |
| `VICTORY_PORT_JOURNAL` | `1` | |
| `VICTORY_PORT_GUARD` | `1` | |
| `VICTORY_PORT_DESCRIPTORS` | `1` | Renderer port #2: faster descriptor-heap sync. Off by default in the runtime. Safe profile sets `0` (full page scans, less flicker risk). Linux script also uses `0`. Source: `crates/victory-gpu/src/port_descriptors.rs`. |
| `VICTORY_SWEEP_ASYNC` | `1` | |

**PSO / pipeline cache**

| Variable | Balanced | Notes |
|---|---|---|
| `VICTORY_LIBRARY` | `1` | Pipeline library on. |
| `VICTORY_LIBRARY_STORE_LATER` | `1` | |
| `VICTORY_LIBRARY_SAVE_S` | `60` | |
| `VICTORY_PIPELINE_BOOK` | `1` | Records pipelines so `warm` can precompile them. |
| `VICTORY_PIPELINE_WORKERS` | `4` | 1–64 if set; else two thirds of logical cores (2–16). Safe: `2`. |

**Memory / OOM / VRAM (balanced leaves several "auto")**

| Variable | Balanced | Notes |
|---|---|---|
| `VICTORY_RESIDENCY` | `1` | |
| `VICTORY_SEGMENT_TIGHT` | `92` | Percent of shared (NON_LOCAL) memory at which idle lists are trimmed. Default 92 if unset. Safe: `85`. `0` disables. |
| `VICTORY_STAGING_IDLE_MS` | `1500` | Safe: `1000`. |
| `VICTORY_WINDOW_IDLE_MS` | `2500` | Safe: `2000`. |
| `VICTORY_SCRATCH_SPARE_MB` | `16` | |
| `VICTORY_READ_KEEP_MB` | `16` | |
| `VICTORY_WAIT_RESIDENT` | `textures` | |
| `VICTORY_KEEP_FREED` | `0` | |
| `VICTORY_LIST_IDLE_MOST` | `1024` | Safe: `128`. |
| `VICTORY_LIST_IDLE_TRIM_S` | `60` | Safe: `30`. |
| `VICTORY_LATE_WAIT_LISTS` | `160` | Safe: `128`. |
| `VICTORY_BURY_LAG` | `4` | |
| `VICTORY_SNAPSHOT_POOL` | `32` | Safe: `24`. |
| `VICTORY_MEMORY_MAP` | `0` | |
| `VICTORY_FILE_PHYSICAL` | `auto` | `1` force on, `0` force off, `auto` = on only if RAM is 20 GiB or less **and** the target volume is a verified NVMe SSD. Never auto-enables on SATA/HDD. Low-commit and Safe force `1`. Source: `crates/victory-hle/src/modules/xmem.rs`. |
| `VICTORY_STAGING_BAR` | `0` | |
| `VICTORY_SEND_BAR` | `0` | |
| `VICTORY_CARD_BUFFERS_MIN_KB` | **unset** | Leave unset on Balanced so 8 GB cards can auto-use a safer minimum. Safe sets `128`. Do not "helpfully" write `64`. |

**Always set**

| Variable | Value | Notes |
|---|---|---|
| `VICTORY_SKIP_PIPELINES` | `240cb516c1af0bc3` | RDNA4 crash-shader shield. Always set. Not a user toggle in v1. |
| `VICTORY_KBM_PC` | `1` | PC keyboard/mouse bindings. F2 / `~` mouse-look, etc. Keep on. Source: `crates/victory-hle/src/modules/gameinput.rs`. |
| `VICTORY_PANEL` | `1` | F3 overlay. Off in the runtime if unset. Keep on so users can screenshot metrics. |
| `VICTORY_PANEL_POS` | `top-right` | `top-right` (default), `top-left`, `bottom-right`, `bottom-left`. Default avoids covering top-left objectives. Source: `crates/victory-gpu/src/host/panel.rs`. |

### 13.3 Official profile overrides only

**`low_commit`:** start from Balanced, then `VICTORY_FILE_PHYSICAL=1`.

**`cbv_exp`:** start from Balanced, then:

- `VICTORY_PORT_CBV=1` (off by default in the runtime; experimental raw/in-place constant buffers)
- `VICTORY_PORT_CBV_PLACE=1`
- `VICTORY_PORT_CBV_SHADOW=2` (allow card-backed shadow CBVs with no pending GPU writes)

**`safe`:** start from Balanced, then (from the main bat; confirm still matches `dev` if the bat has moved):

- `VICTORY_ALLOW_TEARING=0`
- `VICTORY_VSYNC=1`
- `VICTORY_HELPER_PRIORITY=0`
- `VICTORY_SUBMIT_FRAMES=1`
- `VICTORY_JOIN_PARTS=12`
- `VICTORY_PORT_DESCRIPTORS=0`
- `VICTORY_PIPELINE_WORKERS=2`
- `VICTORY_SEGMENT_TIGHT=85`
- `VICTORY_TEXTURE_KEEP_PERCENT=40`
- `VICTORY_TEXTURE_ROOM_PERCENT=65` (runtime clamps 50–97; default otherwise ~90 or the live VRAM policy)
- `VICTORY_TEXTURE_STAGE_MB=64`
- `VICTORY_STAGING_MAX_MB=96`
- `VICTORY_STAGING_IDLE_MS=1000`
- `VICTORY_WINDOW_BUDGET_MB=96`
- `VICTORY_WINDOW_IDLE_MS=2000`
- `VICTORY_SEND_SPARE_MB=24`
- `VICTORY_CARD_BUFFERS_MIN_KB=128`
- `VICTORY_CARD_BUFFERS_MB=768`
- `VICTORY_CARD_ROOM_FOR_TARGETS_MB=2048`
- `VICTORY_LIST_IDLE_MOST=128`
- `VICTORY_LIST_IDLE_TRIM_S=30`
- `VICTORY_LATE_WAIT_LISTS=128`
- `VICTORY_SNAPSHOT_POOL=24`
- `VICTORY_FILE_PHYSICAL=1`

Note: official `presets\4_Safe_Mode_8GB_12GB_VRAM.bat` also forces `VICTORY_FIDELITYFX_FRAMEGEN=0`. The GUI follows the **main bat**: Safe Mode + independent FG checkbox.

### 13.4 Advanced / optional (expose only after verification)

Confirm in `dev` before adding a control. If not in v1, list as TODO:

| Variable | Read site (starting point) | Intent |
|---|---|---|
| `VICTORY_OUTPUT` | `crates/victory-loader/src/resolution.rs` | Title output mode: only `1920x1080`, `2560x1440`, `1280x720`, `1664x936`, `3840x2160`. |
| `VICTORY_RENDER_SCALE` | same | 0.5–1.0, Series X upscaler scale. |
| `VICTORY_UPSCALER` | same | TODO: read the allowed values from source; do not invent names. |
| `VICTORY_TITLE_FPS` | `apps/victory/src/run.rs` | Override the title's 30 FPS setting to match `VICTORY_FRAME_INTERVAL_MS`, or `off` to leave it. |
| `VICTORY_DISPLAY_MODE` | set by the FSR 4K path | Do not set yourself unless you implement that path. |
| FSR 4K quality presets | `apps/victory/src/fsr_4k.rs` | Requires Series X console mode; the Windows bats all use `--console series-s`. **Do not switch v1 to Series X** just to expose 4K FSR. Treat as later. |
| `VICTORY_AMBIENT_RT` / `VICTORY_AMBIENT_SHADE` / `VICTORY_AMBIENT_SETTLE` | `crates/victory-gpu/src/ambient.rs` | Ray tracing is officially off while 30 FPS CPU work continues. Do not present an "enable RT" marketing toggle unless you have verified it is safe and intended. |
| `VICTORY_SKIP_CPU_CHECK` | `apps/victory/src/run.rs` | Bypass AVX2 check. Not for normal users. |
| `VICTORY_WARM_BAD` | warm path | `0` ignores `warm-bad.txt`. Leave to later. |

**Do not invent:** `MUTE (not a switch in this tree)`, `VOLUME (not a switch in this tree)`, `RT (not a switch in this tree)`, `LOG_GPU (not a switch in this tree)`, `PANEL_DETAIL (not a switch in this tree)`, or any "FPS unlock" variable.

### 13.5 Customizer: which of the above are actual controls in v1

Keep the customizer usable. Recommended v1 controls (everything else stays at the profile default, still *applied* at launch):

- Frame gen, VSync, allow tearing, HUD-less FG, overlay on/off, overlay corner.
- File-physical: Auto / On / Off, with the NVMe warning.
- Safe Mode budget cluster: texture room %, texture keep %, card buffers MB, card room for targets MB, staging max MB, window budget MB, pipeline workers, submit frames, helper priority, port descriptors.
- Experimental CBV on/off (when on, set the official trio; when off, unset/remove those three).
- Log level (info only in practice) and "open log file."

If a user is on an official profile and has not customized, launch with the official block even for variables that have no control.

---

## 14. Tools (v1)

### 14.1 Warm shaders

Equivalent to `warm_shaders.bat`:

- Require `config.bat` / a valid `GAME_EXE`.
- Require the runtime **not** already running. If it is, explain: "Close the game first. Warm shaders cannot run at the same time."
- Set at least: `VICTORY_GAME_DIR`, `VICTORY_CACHE_DIR`, `VICTORY_DATA_DIR`, `VICTORY_SCALAR_SPLITS=0`, `VICTORY_SKIP_PIPELINES=240cb516c1af0bc3`, `VICTORY_LIBRARY=1` (same as the bat).
- Run: `RAZDA4A_XEN.exe warm --used --jobs 6` with working directory `<root>`.
- Show stdout/progress in the UI (the command prints counts and timings). On failure, show the output and point at Report a bug.
- Explain: first play session compiles shaders and stutters for 1–3 minutes; warming after that session makes the next launch smoother. It uses pipelines recorded with `VICTORY_PIPELINE_BOOK=1`.

### 14.2 Backup saves

Same as `backup_saves.bat`:

- Source: `<root>\data`
- Destination: `<root>\saves-backup\latest` plus `saves-backup\snapshot-YYYYMMDD-HHMMSS`
- Actions: Backup, Restore from latest (confirm: restore overwrites `data`).
- If `data` does not exist: "Play at least once first."
- Do not touch saves inside the game folder unless you later discover a second official location — and then ask.

### 14.3 Pagefile helper

Wraps `pagefile_16gb_ram.ps1` behavior:

- This changes **Windows system settings** (`C:\pagefile.sys` to 16–32 GB) and requires Administrator and a reboot.
- Show a clear explanation and a confirm dialog. Do nothing until the user agrees.
- If not elevated, relaunch this action with a UAC prompt (or tell the user how). Never silently fail.
- After success: "Reboot Windows for this to take effect."
- Recommended for 16 GB RAM PCs. Do not run it "just in case" on 32 GB+ machines without the user asking.

### 14.4 Open logs folder

Open `<root>\logs` in Explorer. If missing, create it.

### 14.5 Check for a new wave (notice only)

- In v1: query the GitHub API for the default branch (or the latest release, if the project later adds GitHub Releases) of `crossducking/RAZDA4A-XEN`, or simply open `https://github.com/crossducking/RAZDA4A-XEN` in the browser.
- If you compare SHAs, show: "Your runtime SHA-256 is … / remote main is … / a newer Wave may be available" plus a link. **No auto-download.** No patching of game files.
- If the network is down, fail quietly with "Could not check. Open the repository in a browser."
- This is the only unsolicited network use allowed, and only when the user clicks the button (you may also check when the user opens the Tools page, but then it must be clearly labeled and easy to skip; prefer on-click for v1).

---

## 15. Launch procedure and status

When the user clicks Launch:

1. Re-run runtime-folder and game-folder validation. Abort with a human message if anything fails.
2. Re-read or apply the current profile + FG + custom overrides.
3. Create `cache`, `data`, `logs` if needed.
4. Write `config.bat` and `launcher_prefs.bat` (UTF-8 no BOM) so the bats stay in sync.
5. Clear inherited `VICTORY_*` from the child environment.
6. Set the full env block from the switch catalog.
7. Start `RAZDA4A_XEN.exe` with arguments `run --console series-s "<GAME_EXE>" --execute`, working directory `<root>`. Do not use `shell execute` on the game exe.
8. Show status **Launching…** then **Running**. Capture the process stderr/stdout if you can without deadlocking (the runtime also writes `VICTORY_LOG_FILE`).
9. On exit: show the code. If it failed quickly, offer Report a bug and Open logs. Map common failures in the glossary below.

Do not hide a console if that is how Wave 11 currently surfaces useful errors; you may show a "show runtime console" checkbox (default on for v1, or off if you reliably tee logs into the UI). TODO: verify which is less confusing on a clean Win10 box and pick one.

---

## 16. Human error messages (glossary)

Translate raw failures. Examples:

| Symptom / message | What to tell the user |
|---|---|
| `xmem.dll` or `d3d12_xs.dll` not found | They started the game exe directly, or something else skipped the runtime. Start only from this launcher. Those DLLs are not missing game files. |
| Executable not found | The path in `config.bat` is wrong. Browse again. |
| `MicrosoftGame.config` missing / "title's own DLLs are missing" | Incomplete game folder. Copy the whole unpacked game, not just the exe. |
| `VICTORY_FSR_PC=1 requires amd_fidelityfx_upscaler_dx12.dll` | Incomplete RAZDA4A XEN extract; `fidelityfx` folder missing or incomplete. |
| Device / D3D12 creation fails on Windows 10 | `D3D12\D3D12Core.dll` missing (Agility SDK). Re-extract the official zip. |
| `this CPU does not support AVX2` | This CPU cannot run the title. (Do not advertise `VICTORY_SKIP_CPU_CHECK` in the UI.) |
| `0x8007000E` / OOM in the log | Out of memory / commit. Close browsers and other apps; use Low-Commit NVMe; run the pagefile helper on 16 GB RAM; prefer Safe Mode on 8 GB VRAM. |
| NVIDIA / black screen | This build is AMD-only. NVIDIA version later, no date. Link the community fork. |
| Non-zero exit with empty log | "The runtime stopped before it could write a log. Try Launch once more, then use Report a bug even if the log is short." |
| Warm: `'ï»¿@echo off'` | BOM in a bat file (known Wave 11 issue). The GUI warm path should not go through a BOM-broken bat; call the exe directly. |

---

## 17. Native vs frame-generation FPS (v1)

This is a product requirement, not a caption nicety. People already quote 62–63 as "native."

### 17.1 In the launcher (required)

Any time you mention performance (Home blurb, description panel, bug form, about screen), use both numbers and both words, e.g.:

- "Reference (RX 9060 XT + Ryzen 7 7800X3D): **~31 FPS native**, **62–63 FPS with frame generation**."
- Frame Gen checkbox subtitle: "Doubles *displayed* frames. Native FPS is unchanged."

Never show a single FPS figure when FG is on without saying it is displayed/FG.

### 17.2 Companion overlay patch (strongly desired in the same wave)

The F3 overlay lives in the **runtime**, `crates/victory-gpu/src/host/panel.rs` on `dev` (`paint_hud`). Today, when FG is active, the **large** header is `shown_fps = s.fps * 2` labeled only `"NN FPS"`, with a small badge `"{native:.1}X2 FG"`. Screenshots therefore look like a native 62 FPS.

Request a small, safe patch (separate commit on `dev`, can ship with the Wave that includes the launcher):

- Large header should show both, for example: `31 native` on one line and `62 FG` on the second, or `31 native / 62 FG`.
- When FG is off: `31 native` only (do not invent a FG number).
- Keep the sparkline and hardware rows.
- Do not change how FPS is measured; only labeling.

If you are launcher-only and cannot patch `dev`, document this exact change in your handoff and still ship the launcher labels. Do not wait on the overlay patch to release v1 of the GUI.

---

## 18. In-GUI bug report and metrics sharing

Community and maintainer need structured, sanitized reports. GitHub Issues are disabled, so the launcher must make sharing easy without sending anything in the background.

### 18.1 Principles

- **No silent telemetry, ever.** No counters, no heartbeat, no "anonymous usage," no automatic crash upload.
- Nothing leaves the machine until the user has seen a **preview** of the exact payload and has chosen an action.
- Default action is **save a zip on disk** (offline, no server). That is **v1**.
- **Send to a server** is **later**, opt-in only, with a second explicit consent after the same preview.

### 18.2 Entry point

A primary button **Report a bug** (Home and Tools). Also offer it after a failed launch.

### 18.3 Short form (required before export)

Ask in plain fields (all except description can be optional, but encourage them):

- **What happened?** (required, multiline)
- **Where / which scene?** (e.g. prison window, city street, open water, intro, menu)
- **Can you reproduce it?** (yes / sometimes / once / not sure)
- **Native FPS you saw** and **frame-gen / displayed FPS you saw** (optional numbers; two labeled fields, not one)
- **Did you launch with this launcher?** (default yes)
- Attach **screenshot** (optional): file picker. v1 may also offer "Capture the game / overlay in 5 seconds" if you can do it reliably (PrintWindow / DXGI); if not, tell the user to press F3 so the overlay is visible and to paste a screenshot. Never capture or upload a screenshot without it appearing in the preview.

Pre-fill, do not ask the user to type: Wave, SHA-256, OS, CPU, GPU, driver, VRAM, RAM, pagefile, preset, FG on/off, custom settings, whether Safe Mode was on.

### 18.4 What to collect (then sanitize)

Mirror and extend `collect_logs.bat`:

- Wave + `WAVE.json` + computed SHA-256 of `RAZDA4A_XEN.exe` + match/mismatch with `SHA256SUMS.txt`
- OS caption and build, CPU, GPU, VRAM, driver, total RAM, free virtual memory, pagefile allocated/peak
- `XEN_PROFILE`, `XEN_FRAMEGEN`, and the full applied `VICTORY_*` block that the last (or next) launch would use — values only, not secrets (these are not secrets)
- `launcher_prefs.bat` and a redacted `config.bat` (paths sanitized)
- Last **250 lines** of `logs\last_run.log` (or the whole file if shorter). If you include more, say so in the preview. Prefer the tail plus any lines containing error / OOM / device-removed.
- User form answers
- Optional screenshot file
- A `MANIFEST.txt` listing every included file and a **redaction log** (what classes of data were removed, with counts — not the secrets themselves)

If the runtime log contains FPS / frame times / VRAM local vs budget / commit, parse and copy those into `report.json` metrics. If it does not, leave metrics as "user-supplied" + "not found in log" — **TODO: verify log format on a real run** rather than inventing parsers for fields that are not there.

### 18.5 Sanitization (required, before preview)

Apply to all text (logs, paths, form attachments that are text, JSON strings). At least:

| Remove or replace | Replacement |
|---|---|
| `C:\Users\<name>\` and `C:\Users\<name>` | `C:\Users\<USER>\` |
| `/home/<name>/`, `/Users/<name>/` | `/home/<USER>/`, and so on |
| The actual Windows username, if known (`%USERNAME%`) | `<USER>` |
| Computer name (`%COMPUTERNAME%`, and any "PC-NAME" fields you collect) | `<PC>` |
| IPv4 and IPv6 addresses | `<IP>` |
| MAC addresses | `<MAC>` |
| Disk / volume / SMB serial-looking identifiers if you collect them | `<SERIAL>` |
| Strings that look like tokens, bearer secrets, API keys, or URLs with query `token=` / `key=` / `sig=` | `<REDACTED>` |
| Email addresses | `<EMAIL>` |
| Game folder and other absolute paths | Keep **structure**, strip the user directory prefix. Example: `C:\Users\<USER>\Games\MyTitle\your_executable.exe` |

Do not include Windows product keys, stored credentials, or other users' profiles.

`collect_logs.bat` only redacts `C:\Users\<USERNAME>` today. The GUI must be stricter.

After sanitization, scan again; if the raw username or computer name still appears, strip that occurrence and note it in the redaction log.

### 18.6 Notice + preview (required)

Before any save or send, show a modal that states, in substance:

> You are about to export a diagnostics package. It includes hardware (CPU, GPU, VRAM, RAM, OS, driver), your Wave version, launcher settings, recent logs, and what you typed in the form. Personal paths, your user name, PC name, IP addresses, and tokens are stripped. Check the preview. Nothing is uploaded unless you later choose Send (not available in v1 / clearly disabled).

Then show:

- A list of files and the sanitized `report.json`
- A "Removed" summary (e.g. "14 user-path occurrences, 2 IPv4 addresses, computer name")
- A scrollable preview of the sanitized log excerpt
- If a screenshot is attached, a thumbnail (screenshots may still show a desktop username in the image; warn: "Images are not auto-redacted. Crop or avoid showing personal files before including.")

Buttons: **Cancel**, **Save zip to disk** (default, focused), and later **Send…** (hidden or disabled in v1 with "Coming later").

### 18.7 v1 action: save a sanitized zip

- Default filename: `RAZDA4A-XEN-bug-Wave<N>-YYYYMMDD-HHMMSS.zip` in a folder the user chooses (default: `<root>\logs\`).
- Open the zip in Explorer (`/select`) like `collect_logs.bat`.
- Tell the user they can attach it when they post a report in the place the project currently uses (X thread or whatever the README says at ship time — **not** GitHub Issues). If the project names a place, put that sentence in the success dialog and make it easy to update.

### 18.8 Structured format (JSON + logs)

Zip layout:

```text
report.json
MANIFEST.txt
logs/last_run.redacted.log
logs/collect_logs.txt
settings/applied_env.json
settings/launcher_prefs.redacted.bat
screenshot.png          (optional)
```

`report.json` sketch (all strings already sanitized):

```json
{
  "format": "razda4a-bug-report",
  "format_version": 1,
  "created": "2026-11-28T12:00:00+03:00",
  "wave": {
    "number": 11,
    "channel": "main",
    "sha256": "...",
    "sha256_matches_sums": true,
    "wave_json_present": true
  },
  "hardware": {
    "os": "Microsoft Windows 10 Pro (Build 19045)",
    "cpu": "AMD Ryzen 7 7800X3D (8C/16T)",
    "gpu": "AMD Radeon RX 9060 XT",
    "vram_gb": 16.0,
    "driver": "...",
    "ram_gb": 16.0,
    "free_virtual_gb": 12.3,
    "pagefile": { "allocated_mb": 32768, "peak_mb": 20000 }
  },
  "launch": {
    "used_this_launcher": true,
    "profile": "safe",
    "framegen": false,
    "custom_preset_name": null
  },
  "metrics": {
    "fps_native_user": 12,
    "fps_fg_user": 24,
    "fps_native_log": null,
    "fps_fg_log": null,
    "frame_times_ms_log": null,
    "vram_local_mb_log": null,
    "vram_budget_mb_log": null,
    "ram_commit_gb_log": null,
    "source": "user and/or parsed log; null if unknown"
  },
  "user": {
    "what_happened": "...",
    "scene": "...",
    "repro": "sometimes"
  },
  "redaction": {
    "policy": "razda4a-sanitize-1",
    "counts": { "user_paths": 14, "ipv4": 2, "computer_name": 1, "tokens": 0 }
  }
}
```

Keep field names stable so triage can parse them. Missing data is `null`, never a made-up number.

### 18.9 Later: send to a server (not v1)

When built, it must:

1. Use the same sanitize + preview path.
2. Require a second explicit checkbox: "I want to upload this sanitized zip to the RAZDA4A XEN report inbox."
3. Show the destination (URL) and retention period.
4. Fail visibly on network errors; never retry forever in the background.

**Proposed low-cost inbox** (for the maintainers to stand up later, not a hard dependency of the v1 app):

- A **Cloudflare Worker** + **R2** bucket (or equivalent: a single HTTPS endpoint that accepts a multipart zip + the JSON, writes object `reports/<year>/<id>.zip`).
- Front with **Turnstile** (or a one-time upload token the launcher fetches after the user clicks Send).
- Abuse limits: max zip size **12 MB**; **3 uploads per IP per hour**; reject if `report.json` is missing or `format` is wrong; re-run a sanitizer on the server and drop the object if a raw `C:\Users\<likely real name>` or token pattern remains; no listing API; no public bucket.
- Retention: 90 days, then delete.
- Id is a random UUID, returned to the user so they can cite `report id: …` in a public post without revealing the zip.
- Authentication: none for the first version of upload (Turnstile + rate limit only), to avoid accounts. If abuse appears, add a shared project password or signed tokens.

Do not implement this endpoint as a hidden URL in v1. Leave a `IReportSink` interface and a disabled "Send" button.

---

## 19. Design direction (modern, not slop)

The implementing agent **must use unslop / anti-slop design skill(s)** when designing the UI. If your environment has a skill named `unslop`, `anti-slop`, or similar, load it and follow it. If it does not, follow the rules below as the skill.

This is a small utility people will screenshot. It should feel like a considered app, not a generated dashboard.

### 19.1 Identity

- Dark theme, cool graphite / ink backgrounds, one accent (a clear blue is fine; the current Launch button uses approximately `#2E86DE` — you may refine, but stay consistent).
- A discreet **butterfly** mark (the project's identity) as a simple monochrome or two-tone graphic, not a cartoon sticker and **not an emoji glyph in the icon font**.
- Product name set in a real typographic hierarchy: "RAZDA4A XEN" as the title, "Launcher" or the Wave badge as secondary.

### 19.2 Unslop rules (mandatory)

- No gratuitous gradients, glassmorphism, neon glow, mesh backgrounds, or hero illustrations.
- No emoji icons in buttons or headings. If you need an icon, use a small, consistent set (Lucide, Fluent System Icons, or similar) in one weight and one size scale.
- No filler copy ("Welcome to your ultimate next-gen experience," "Unlock your journey," "Powered by AI"). Write only sentences that help the user do a thing.
- A spacing scale (for example 4 / 8 / 12 / 16 / 24 / 32 / 48) used consistently. No accidental 13 px gaps.
- One typeface (Segoe UI Variable / Segoe UI is correct on Windows; do not mix three display fonts). Distinct sizes for title, section, body, caption, mono (paths and SHA).
- Real states for every screen: **loading**, **empty**, **error**, **success**, **disabled with a reason**. No blank white panels, no infinite spinners without a cancel.
- Contrast that meets WCAG AA for body text on the dark background. Visible focus rings. Keyboard: Tab order, Enter launches only when Launch is focused, Esc closes dialogs.
- Do not dummy-up charts or fake FPS meters with random numbers.

### 19.3 Screens to design and photograph

Produce **screenshots of every screen** for review, on Windows 10, 96 and 150 DPI if you can, including:

1. First-run welcome
2. Folder picker + validation error (wrong folder: runtime mistaken for the game)
3. Incomplete game folder (missing `MicrosoftGame.config`)
4. Incomplete runtime folder (missing `fidelityfx` or `D3D12`)
5. Hardware step, AMD OK
6. Hardware step, NVIDIA blocked (fork link visible)
7. Home, ready to launch, Wave badge visible
8. Home, Safe Mode suggested on 8 GB
9. Settings / customizer with the description panel showing a real explanation
10. Tools
11. Warm shaders in progress and finished
12. Launch error: game exe started directly / xmem explanation
13. Bug report form
14. Bug report sanitize **preview** (the important one)
15. Bug report saved successfully
16. About / Wave details (SHA match)

Name files clearly and include them in your PR description. Do not put screenshots of the **game** into the repo (`docs/asset-policy.md`). Launcher chrome is fine.

---

## 20. Safety, privacy, legal

- Never download or bundle game files. Never link to piracy or "dumps."
- Never touch files outside: `<root>` (config, cache, data, logs, presets the user saves), the user-chosen game folder (read-only except you do not write into it in v1), and — only after an explicit confirmed request — the Windows pagefile setting.
- Do not scan the user's whole disk for the game unless they ask a "search" later feature (out of v1).
- No telemetry (see the bug-report section).
- Antivirus: unsigned exe + scripts already get `Trojan:Script/Wacatac.H!ml` false positives. Keep the launcher boring (no packers, no process hollowing, no injecting into other processes). Document how you build it. Optionally add the launcher hash to `SHA256SUMS.txt` when maintainers package a wave.
- GPL-3.0-or-later; source must be offered with the binary when this ships in the repo.

---

## 21. Packaging and how it ships

- v1 ships on `main` next to the existing runtime, or in a clearly named zip the README points to. Prefer one folder, one new exe.
- Build from source must be documented in one paragraph (for example: "install .NET 8 SDK, `dotnet publish …`").
- Keep `RAZDA4A XEN.bat --no-gui` working for VILauncher and scripts.
- Update **How to run** docs: "Double-click `RAZDA4A XEN Launcher.exe`. Do not start your game .exe. Do not start `RAZDA4A_XEN.exe` by itself if you want the official presets." Coordinate language with PR #4 if it has merged.
- Add `WAVE.json` to the packaging checklist for every future Wave.

---

## 22. Acceptance criteria (v1 checklist)

The implementing agent should not call v1 done until all of the following are true:

- [ ] A new user can complete first-run, pick a valid game folder, and launch without editing a bat file.
- [ ] Bat scripts and `--no-gui` still work; `config.bat` / `launcher_prefs.bat` stay in sync; files are UTF-8 **without BOM**.
- [ ] Launch command and env block match the catalog for all four official profiles and the FG toggle (spot-check against `RAZDA4A XEN.bat` and the four preset bats).
- [ ] The game exe is never started directly. Choosing the runtime/launcher as the "game" is refused with a clear message. `xmem.dll not found` is explained in the UI glossary.
- [ ] Incomplete runtime extracts (missing `D3D12`, `fidelityfx`, or the runtime exe) are refused with named files. Incomplete game folders (missing `MicrosoftGame.config`) are explained.
- [ ] NVIDIA / non-RDNA2 AMD / too-little VRAM cases are explained; NVIDIA copy includes "version coming later, no date" and the community fork link. Launch is blocked where this build cannot work.
- [ ] Wave number (or "unknown" + SHA-256) is visible on Home and in bug reports.
- [ ] Native vs frame-gen language is correct everywhere in the launcher.
- [ ] Customizer has a visible description panel for every exposed setting; no invented switches; `VICTORY_SCALAR_SPLITS` stays 0; skip-pipeline shield stays on; compute wait is not quietly lowered.
- [ ] Custom presets save/load/share as specified; reset to defaults works.
- [ ] Warm shaders, backup saves, pagefile helper (with consent + UAC), open logs, and check-for-wave (link/notice only) work.
- [ ] Report a bug: form, collection, sanitization, preview, save zip. No upload in v1. No background network with machine data.
- [ ] Pagefile helper and any other system change ask first.
- [ ] Unslop design rules followed; screenshots of every screen attached for review.
- [ ] Keyboard / contrast / focus basics work; high-DPI is not clipped on 150% scaling (test on Windows 10).
- [ ] No game assets in the commit. No trademarked game name in the UI.

---

## 23. Step-by-step build plan (milestones you can pause and resume)

Work in a branch off `dev` if the launcher source should live with the Rust tree, or a clearly named branch for `main` packaging files. Do not force-push. Do not commit game files.

### Milestone A — Skeleton (half day)

1. Create the app project (WPF / .NET 8 or Tauri 2 as chosen).
2. Single-file publish produces `RAZDA4A XEN Launcher.exe`.
3. Window chrome, dark theme tokens, header with a placeholder Wave badge.
4. App finds `<root>` and shows whether `RAZDA4A_XEN.exe` is present.

**Resume checkpoint:** the exe runs from a copy of a `main` extract.

### Milestone B — Paths, wizard, validation (core UX)

1. Read/write `config.bat` and `launcher_prefs.bat` (UTF-8 no BOM, CRLF).
2. First-run wizard steps A–E.
3. All guards in the validation section, with the exact user-facing sentences.
4. Hardware detection and preset suggestion.
5. NVIDIA / RDNA1 / VRAM messaging.

**Resume checkpoint:** you can point the wizard at a real game folder and at the wrong folders, and the messages are correct, without launching yet.

### Milestone C — Launch parity

1. Implement the env builder from the switch catalog. Unit-test: for each official profile × FG on/off, the map of variables matches a fixture harvested from the bats (you can parse the bats and the override tables).
2. Spawn the runtime with the official command line.
3. Status / exit codes / error glossary.
4. Home screen wired to Launch.

**Resume checkpoint:** launching from the GUI plays the same as `RAZDA4A XEN.bat` for Balanced and Safe Mode (confirm with a short run on your AMD Windows 10 box).

### Milestone D — Customizer and tools

1. Settings + description panel + official profiles.
2. Customizer v1 controls, save/load JSON, export bat, reset.
3. Warm shaders, backup saves, pagefile helper, open logs, check-for-wave.

**Resume checkpoint:** a custom preset round-trips and a warm run starts only when the game is not running.

### Milestone E — Wave badge, FPS language, bug report

1. `WAVE.json` support + SHA-256 verify.
2. Copy pass: every FPS mention is native vs FG.
3. Report a bug: form, collect, sanitize, preview, save zip. Disabled Send button.
4. Optional: overlay label patch on `dev` (`panel.rs`) in a **separate** commit.

**Resume checkpoint:** you can generate a zip, open it, and confirm the username does not appear in any text file.

### Milestone F — Design polish and screenshots

1. Apply unslop rules; fix spacing, type, empty/error states, focus, 150% DPI.
2. Screenshot every screen listed in the design section.
3. Write a short "How to run" paragraph for the maintainers to drop into docs.

### Milestone G — Acceptance and handoff

1. Walk the v1 acceptance checklist on Windows 10 with an AMD GPU.
2. Manual regression: old bats, `--no-gui`, preset 4, BOM-safe writes.
3. Open a PR with source, build instructions, screenshots, and a note on anything still TODO (especially unverified advanced switches, overlay patch, and server upload).
4. Do not merge to `main` unless the project owner asks. The tester friend may want to run the same playtest protocol as for other `dev` work first.

If a session dies, resume at the last **Resume checkpoint**, not at Milestone A.

---

## 24. TODOs for the implementing agent to verify (do not guess)

- [ ] Confirm `RAZDA4A XEN.bat` and presets on the tip of `main` / `dev` still match the env tables in this spec (they were accurate on 2026-11-28).
- [ ] Confirm whether log lines contain parseable native FPS, FG FPS, frame times, VRAM local/budget, and commit; if yes, parse them into `report.json`; if no, leave nulls.
- [ ] Decide whether a console window should stay visible during launch on Windows 10.
- [ ] `VICTORY_OUTPUT` / `VICTORY_RENDER_SCALE` / `VICTORY_UPSCALER` / `VICTORY_TITLE_FPS`: read allowed values from `dev` before exposing.
- [ ] Do not expose RT ambient switches without a current maintainer decision (RT is officially off).
- [ ] Overlay text layout in `panel.rs` after the dual FPS label — check it still fits the HUD box.
- [ ] If `WAVE.json` is not yet in the repo, add it as part of packaging; do not hardcode Wave 11 forever.
- [ ] Re-read the current "where to send reports" sentence (Issues are disabled) before writing the success dialog.

---

## 25. References (read these before coding)

- `main`: `RAZDA4A XEN.bat`, `launcher_ui.ps1`, `config.bat`, `presets\*.bat`, `warm_shaders.bat`, `collect_logs.bat`, `backup_saves.bat`, `pagefile_16gb_ram.ps1`, `SHA256SUMS.txt`
- `dev`: `apps/victory/src/main.rs`, `apps/victory/src/run.rs`, `apps/victory/src/warm.rs`, `crates/victory-core/src/config.rs`, `crates/victory-core/src/logging.rs`, `crates/victory-gpu/src/switch.rs`, `crates/victory-gpu/src/host/panel.rs`, `docs/asset-policy.md`, `AGENTS.md`, `victory.example.toml`
- PR #4 branch `cursor/readme-docs-split-9702`: `docs/en/running.md`, `docs/en/hardware.md`, `docs/en/performance.md`, `docs/en/known-limits.md`
- Community launcher requests: `/workspace/razda4a-team/menedzheo_findings_wave13.md` (Wave badge, native vs FG, BOM, direct-exe `xmem.dll`)

End of spec.
