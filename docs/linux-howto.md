# VIctory under Linux (Proton): build and run — Ubuntu, Arch, Fedora

Covers VIctory-family trees (upstream avatarmanz/VIctory-7900XTX and forks) running your **own legally
dumped** Xbox Series copy of the game. Tested class: AMD RDNA 2+ GPU with Mesa RADV, Proton Experimental.
NVIDIA works with other trees' settings but is not covered here.

## 1. System packages

Needs: git, a C toolchain, **clang + lld** (cargo-xwin uses `clang-cl`/`lld-link`), python3 (the Proton
script), Mesa Vulkan (RADV) + 32-bit Vulkan for Steam, vulkan-tools (to check the GPU), Steam.

**Ubuntu / Debian-based** (enable `multiverse` for Steam):
```sh
sudo dpkg --add-architecture i386 && sudo apt update
sudo apt install build-essential git curl pkg-config clang lld llvm python3 \
  mesa-vulkan-drivers mesa-vulkan-drivers:i386 vulkan-tools steam-installer
# Ubuntu ships versioned names; make the plain ones visible if missing:
command -v clang-cl || sudo ln -s "$(ls /usr/bin/clang-[0-9]* | sort -V | tail -1)" /usr/local/bin/clang-cl
command -v lld-link || sudo ln -s "$(ls /usr/bin/lld-link-[0-9]* 2>/dev/null | sort -V | tail -1)" /usr/local/bin/lld-link
```

**Arch-based** (CachyOS, EndeavourOS, Manjaro; enable `[multilib]` in /etc/pacman.conf):
```sh
sudo pacman -S --needed base-devel git clang lld llvm python vulkan-radeon lib32-vulkan-radeon vulkan-tools steam
```

**Fedora-based** (enable RPM Fusion nonfree for Steam):
```sh
sudo dnf install @development-tools git clang lld llvm python3 mesa-vulkan-drivers \
  mesa-vulkan-drivers.i686 vulkan-tools steam
```

Check the driver: `vulkaninfo --summary` must list your card with `driverName = radv`. Mesa ≥ 24 is fine.

## 2. Rust and the Windows cross-compiler

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
rustup target add x86_64-pc-windows-msvc
cargo install --locked cargo-xwin      # downloads the MSVC CRT + Windows SDK on first build
```
The tree's `rust-toolchain.toml` pins the compiler; rustup fetches it on the first `cargo` call.

## 3. Get and build the runtime

```sh
git clone <the VIctory tree you use> victory && cd victory
# Play build (the 'scripted' feature only adds VICTORY_STEP_OVER):
cargo xwin build --profile release -p victory --features scripted --target x86_64-pc-windows-msvc
# Diagnostics build (frame captures, VICTORY_AB, profiling; ~3 % slower):
cargo xwin build --profile diag -p victory --features diagnostics --target x86_64-pc-windows-msvc
```
Results: `target/x86_64-pc-windows-msvc/release/victory.exe` and `.../diag/victory.exe`.
Optional checks: `cargo test -p victory-shader --release` (native); the other crates' tests are Windows
executables (`cargo xwin test -p victory-gpu --target x86_64-pc-windows-msvc --no-run`, then run the test
exe with `proton runinprefix <exe> --test-threads 1`). Under Wine a few tests fail by environment
(WSAConnectByList missing, temp folder not NTFS, unchecked DXIL digest) — not regressions.

## 4. Proton

Use **Proton Experimental** (Steam → Library → Tools → install "Proton Experimental") or **GE-Proton
11-14 or newer** (unpack into `~/.steam/steam/compatibilitytools.d/`). vkd3d-proton must be ≥ 3.2.0:
3.1.0 crashes at the first world pipeline.

| Install | Path |
|---|---|
| Steam (native) | `~/.steam/steam/steamapps/common/Proton - Experimental` |
| Steam (Flatpak) | `~/.var/app/com.valvesoftware.Steam/.steam/steam/steamapps/common/Proton - Experimental` |
| GE-Proton | `~/.steam/steam/compatibilitytools.d/GE-Proton<version>` |

Optional, CPU-bound Wine fix: a wineserver with the "view tree" patch (CachyOS/wine-cachyos PR #50) makes
map/unmap of views O(log n) instead of O(n). Build it against the exact Wine your Proton ships and
replace only `files/bin/wineserver` in a **copy** of the Proton folder.

## 5. Game files and extras

- **Game**: your dump folder, with `game_scarlett_master.exe`, `shaders.rpf` and the rest of the package
  next to it. VIctory refuses to start if DLLs of the package are missing.
- **Shader compiler for runtime helper shaders**: either Microsoft `d3dcompiler_47.dll` in the prefix
  (`winetricks d3dcompiler_47`, prefix from step 6) or a folder with `dxcompiler.dll` + `dxil.dll` from
  Microsoft's DirectXShaderCompiler release, named by `VICTORY_DXC`. Without either, indirect draws are
  dropped (`E5005 ... RWByteAddressBuffer`).
- **Series S mode, GPU FSR (optional)**: `amd_fidelityfx_loader_dx12.dll` + `amd_fidelityfx_upscaler_dx12.dll`
  from AMD's FidelityFX SDK 2.x (`Kits/FidelityFX/signedbin`) in a folder named by `VICTORY_FFX_DIR`
  (trees that carry the FFX module).

## 6. Launcher

Save as `~/victory/run.sh`, edit the four paths at the top, `chmod +x`.
```sh
#!/usr/bin/env bash
set -euo pipefail
PROTON_DIR="$HOME/.steam/steam/steamapps/common/Proton - Experimental"
EXE="$HOME/src/victory/target/x86_64-pc-windows-msvc/release/victory.exe"
GAME="$HOME/Games/my-dump"                      # folder with game_scarlett_master.exe
DATA="$HOME/.local/share/victory"               # prefix, caches, extras
win() { printf 'Z:%s' "${1//\//\\}"; }          # Linux path -> Wine path

mkdir -p "$DATA/prefix" "$DATA/cache"
export STEAM_COMPAT_CLIENT_INSTALL_PATH="$HOME/.steam/steam"
export STEAM_COMPAT_DATA_PATH="$DATA/prefix"
export VICTORY_GAME_DIR="$(win "$GAME")" VICTORY_CACHE_DIR="$(win "$DATA/cache")"
[ -d "$DATA/dxc" ] && export VICTORY_DXC="$(win "$DATA/dxc")"
# Load vkd3d-proton/DXVK, not Wine's own d3d12 (else device creation fails: E_INVALIDARG):
export WINEDLLOVERRIDES="d3d12=n,b;d3d12core=n,b;dxgi=n,b"
export VKD3D_CONFIG="${VKD3D_CONFIG:-descriptor_heap}"
export MESA_SHADER_CACHE_MAX_SIZE="${MESA_SHADER_CACHE_MAX_SIZE:-16G}"   # default 1 GiB evicts warmed pipelines
export GLIBC_TUNABLES=glibc.pthread.rseq=0     # kernels 6.19+/7.x: avoids rare SIGSEGV exits under Wine
export WINEDEBUG="${WINEDEBUG:--all}"
cd "$DATA/prefix"                               # vkd3d-proton writes its cache to the working dir
[ -d pfx ] || "$PROTON_DIR/proton" run "$EXE" --version     # creates the prefix once
if [ $# -eq 0 ]; then
  args=(run)
  [ "${CONSOLE:-}" = series-s ] && args+=(--console series-s) && \
    { [ -d "$DATA/ffx" ] && export VICTORY_FFX_DIR="$(win "$DATA/ffx")"; true; }
  set -- "${args[@]}" "$(win "$GAME/game_scarlett_master.exe")" --execute
elif [ "$1" = prebuild ]; then
  set -- shaders "$(win "$GAME/shaders.rpf")"
fi
# runinprefix keeps the console output (proton run goes through steam.exe, which swallows it):
exec "$PROTON_DIR/proton" runinprefix "$EXE" "$@"
```

## 7. First start: caches

The settings below that change translation (e.g. `VICTORY_PORT_CBV`) are part of the shader cache key:
prebuild with the **same** settings you play with.
```sh
VICTORY_PORT_CBV=1 ./run.sh prebuild        # translates every shader once (~2 min on 16 threads)
CONSOLE=series-s VICTORY_PORT_CBV=1 ./run.sh   # play into the world once (pipelines get listed)
VICTORY_PORT_CBV=1 ./run.sh warm --used     # makes the host pipelines the run drew with
VICTORY_PORT_CBV=1 ./run.sh warm            # optional: every pipeline any run met (long; needs the 16G cache)
```
After a runtime update that changes the shader translator, repeat the steps (the old entries are ignored).

## 8. Settings that helped on RDNA 2 under Proton

```sh
CONSOLE=series-s VICTORY_PORT_CBV=1 VICTORY_MAP_LOCK_FINE=1 VICTORY_THIN=1 VICTORY_SKIP_MOVIES=end \
VICTORY_STEP_OVER=5149aec,63966f VICTORY_LATE_WAIT=default VICTORY_PIPELINE_WAIT_MS=2500 \
VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1 VICTORY_PARTIAL_DELTA=1 VICTORY_REFRESH_ONCE=1 ./run.sh
```
- Series S mode is much lighter than Series X (720p render + FSR).
- Check each switch exists in **your** tree before trusting it (`grep -rn VICTORY_<NAME> crates`); unknown
  ones are silently ignored. Read the `effective-env:` lines at the top of the console output.
- `WINE_CPU_TOPOLOGY` mapping SMT siblings in Windows order (`16:0,8,1,9,...` on an 8-core/16-thread
  CPU) helped ~3 % where the title pins threads.

## 9. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Starts, no picture, `E_INVALIDARG` creating the device | `WINEDLLOVERRIDES` missing (Wine's d3d12 loaded) |
| Indirect draws dropped, `E5005` | no d3dcompiler_47 / `VICTORY_DXC` |
| Black world, HUD drawn, 11 draws/frame | renderer port 1 (`VICTORY_PORT_CONSTANTS=1`) under Wine/RADV: leave it off |
| Crash `write 0x0` at title offset `+0x2215c35` | the game stopping itself (watchdog / fatal error); slow start (cold caches, heavy background load) is a common trigger: prebuild first, don't build while playing |
| Crash at once in Series S with `VICTORY_OUTPUT` | don't change the output size in Series S mode |
| "Silent" exits, game freezes then disappears | a native SIGSEGV being dumped by systemd-coredump (`coredumpctl list`); try `GLIBC_TUNABLES=glibc.pthread.rseq=0` |
| Very slow first minutes | cold shader / pipeline caches: steps of section 7 |

Stopping a stuck run: `pkill -f 'release\\victory\.exe'` (matches the Wine process only), then
`WINEPREFIX="$DATA/prefix/pfx" wineserver -k`. Avoid broad patterns like `pkill -f runinprefix`: they
also kill unrelated Proton processes.
