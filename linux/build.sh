#!/usr/bin/env bash
# Cross-builds victory.exe on Linux for the Windows target the project is
# written for (x86_64-pc-windows-msvc), linking with lld-link against the
# Microsoft CRT and Windows SDK libraries that `xwin` unpacked.
#
# One-time setup (accepts Microsoft's Build Tools licence, ~1.7 GB on disk):
#   cargo install xwin
#   xwin --accept-license --arch x86_64 --cache-dir ~/.cache/xwin/dl \
#        splat --output ~/.cache/xwin/splat
#
# Usage: linux/build.sh [extra cargo arguments]
#   linux/build.sh                                        release build
#   linux/build.sh --profile diag --features diagnostics  the debugging build
set -euo pipefail
cd "$(dirname "$0")/.."

XWIN="${XWIN_SPLAT:-$HOME/.cache/xwin/splat}"
if [[ ! -d "$XWIN/crt/lib/x86_64" ]]; then
    echo "Windows SDK libraries not found under $XWIN (see the top of this script)" >&2
    exit 1
fi
command -v lld-link >/dev/null || { echo "lld-link not found (install lld)" >&2; exit 1; }

export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="\
-Lnative=$XWIN/crt/lib/x86_64 \
-Lnative=$XWIN/sdk/lib/um/x86_64 \
-Lnative=$XWIN/sdk/lib/ucrt/x86_64"
# blake3 otherwise assembles MASM files with ml64.exe, which does not exist
# here; this selects its Rust SSE/AVX2 code instead (no AVX-512 path).
export CARGO_FEATURE_PURE=1

profile=(--release)
for a in "$@"; do [[ $a == --profile* ]] && profile=(); done
exec cargo build "${profile[@]}" -p victory --target x86_64-pc-windows-msvc "$@"
