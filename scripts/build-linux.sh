#!/usr/bin/env bash
# Cross-builds victory.exe on Linux with cargo-xwin (README, "Run on Linux").
#
#   scripts/build-linux.sh            release:  target/x86_64-pc-windows-msvc/release/victory.exe
#   scripts/build-linux.sh --diag     diagnostics build (debug logging, GPU timing switches, panel):
#                                     target/x86_64-pc-windows-msvc/diag/victory.exe
#
# Needs rustup (with the pinned toolchain), cargo-xwin, clang/lld/llvm, and
# Microsoft's licence for the CRT/SDK cargo-xwin downloads accepted
# (XWIN_ACCEPT_LICENSE=1, set here). Copies dxcompiler.dll and dxil.dll from
# VICTORY_DXC_DIR (default ~/.local/share/victory/dxc) beside the exe.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target=x86_64-pc-windows-msvc
profile=release
features=()
for a in "$@"; do
  case "$a" in
    --diag) profile=diag; features=(--features diagnostics) ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

export PATH="$HOME/.cargo/bin:$PATH"   # rustup's cargo, which honours rust-toolchain.toml
command -v cargo-xwin >/dev/null || { echo "cargo-xwin missing: cargo install --locked cargo-xwin" >&2; exit 2; }
export XWIN_ARCH=x86_64 XWIN_ACCEPT_LICENSE=1

cd "$repo"
cargo xwin build --profile "$profile" "${features[@]}" --locked -p victory --target "$target"

out="$repo/target/$target/$profile"
dxc="${VICTORY_DXC_DIR:-$HOME/.local/share/victory/dxc}"
for dll in dxcompiler.dll dxil.dll; do
  if [[ -f "$dxc/$dll" ]]; then
    cp -f "$dxc/$dll" "$out/"
  else
    echo "warning: no $dll in $dxc; under Wine there is no 3D without DXC (README)" >&2
  fi
done
echo "built: $out/victory.exe ($profile)"
