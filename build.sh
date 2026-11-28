#!/bin/bash
source "$(dirname "$(readlink -f "$0")")/env.sh"
sdk=$root/deps/msvc
if [ ! -d "$sdk/VC" ]; then echo "no sdk: run $root/setup.sh sdk"; exit 1; fi
export CARGO_BUILD_TARGET=x86_64-pc-windows-msvc
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target}
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-Clink-arg=/winsysroot:$sdk"
export CC_x86_64_pc_windows_msvc=clang-cl CXX_x86_64_pc_windows_msvc=clang-cl AR_x86_64_pc_windows_msvc=llvm-lib
export CFLAGS_x86_64_pc_windows_msvc="--target=x86_64-pc-windows-msvc -Wno-unused-command-line-argument -fuse-ld=lld-link /winsysroot$sdk"
export CXXFLAGS_x86_64_pc_windows_msvc="$CFLAGS_x86_64_pc_windows_msvc"
ram=$(awk '/MemTotal/{print int($2/1024)}' /proc/meminfo)
cd "$root"
exec systemd-run --user --scope --collect -q -p "MemoryHigh=${MEM_HIGH:-$((ram*2/5))M}" -p "MemoryMax=${MEM_MAX:-$((ram*9/20))M}" -p MemorySwapMax=4G -- \
  nice -n 12 timeout -k 8s "${BUILD_TIMEOUT:-1800}s" cargo "${@:-build}" --locked --profile diag --features diagnostics -j "${JOBS:-$(nproc)}" -p "${PKG:-victory}"
