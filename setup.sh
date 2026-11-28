#!/bin/bash
set -euo pipefail
source "$(dirname "$(readlink -f "$0")")/env.sh"
deps=$root/deps
sys=$WINEPREFIX/drive_c/windows/system32

MESA=(https://gitlab.freedesktop.org/mesa/mesa.git mesa-26.3.1)
VKD3D=(https://github.com/HansKristian-Work/vkd3d-proton 71cae091b06571e51446cf2a937f0b04c237c04d)
WINE=(https://gitlab.winehq.org/wine/wine.git)
XWIN=(https://github.com/Jake-Shadle/xwin/releases/download/0.10.0/xwin-0.10.0-x86_64-unknown-linux-musl.tar.gz d870eb4b2f390878af6da1ccd3cf321d22fcb72720984853b4be732ae597fc88)
DXC=(https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.9.2609/dxc_2026_09_29.zip ad31b1fc8443175d204f77a611fdb3ef2ec42759bdc2f1167368de24a4a7e7f1)
DXVK=(https://github.com/doitsujin/dxvk/releases/download/v3.1.1/dxvk-3.1.1.tar.gz 40565b4a724aadc4433fa4e010b4b23916d9b1f1baeee64e17186db94f54e608)
D3DCOMPILER=(https://raw.githubusercontent.com/mozilla/fxc2/9aba9b11079303d5577e0e3eb455f4d00f3b5946/dll/d3dcompiler_47.dll 4432bbd1a390874f3f0a503d45cc48d346abc3a8c0213c289f4b615bf0ee84f3)
FSR4=(https://raw.githubusercontent.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/v2.4.0/Kits/FidelityFX/signedbin/amd_fidelityfx_upscaler_dx12.dll aad4fccccb29084a655f9a4f9f613bd22c8c03fb4a0f8840148cc6543b4ccace)
FSR4_DRIVER=(https://loathingKernel.github.io/proton-upscalers/amdxcffx64_v4.1.1_398EA93C15D554EFB7ECE1F4CD057554.xz 4e7dc37aebea3a90e3d3cc43e24cb2b54176b2535315f20dbe63b3b7cfc56b1e)

logged() {
  local log=$deps/$1.log
  shift
  if ! "$@" >>"$log" 2>&1; then
    tail -n 30 "$log"
    exit 1
  fi
}

fetch() {
  local url=$1 sum=$2 to=$3
  if [ -f "$to" ] && echo "$sum  $to" | sha256sum -c --status; then return; fi
  mkdir -p "$(dirname "$to")"
  echo "fetching $(basename "$to")"
  curl -fL --retry 3 -sS -o "$to.part" "$url"
  if ! echo "$sum  $to.part" | sha256sum -c --status; then echo "$url is not the file expected"; exit 1; fi
  mv "$to.part" "$to"
}

source_of() {
  local name=$1 url=$2 ref=$3 dir=$deps/$1 patch=$root/patches/$1.patch
  local stamp="$ref $(sha256sum < "$patch")"
  if [ ! -d "$dir/.git" ]; then
    git init -q "$dir"
    git -C "$dir" remote add origin "$url"
  fi
  if [ "$(cat "$dir/.git/um-stamp" 2>/dev/null)" = "$stamp" ]; then return; fi
  if [ -f "$dir/.git/um-patch" ] && ! git -C "$dir" diff HEAD | cmp -s - "$dir/.git/um-patch"; then
    echo "deps/$name has changes the patch does not: keep them with tools/patch.sh $name"
    exit 1
  fi
  echo "fetching $name $ref"
  git -C "$dir" fetch -q --depth 1 origin "$ref"
  git -C "$dir" reset -q --hard
  git -C "$dir" checkout -q --detach FETCH_HEAD
  git -C "$dir" submodule update -q --init --recursive --depth 1
  git -C "$dir" apply --index "$patch"
  git -C "$dir" diff HEAD > "$dir/.git/um-patch"
  echo "$stamp" > "$dir/.git/um-stamp"
}

step_tools() {
  local missing=()
  for t in git curl unzip tar xz python3 ninja glslangValidator clang-cl lld-link llvm-lib x86_64-w64-mingw32-gcc x86_64-w64-mingw32-strip widl cargo rustup wine wineboot make flex bison systemd-run unshare; do
    command -v "$t" >/dev/null || missing+=("$t")
  done
  if (( ${#missing[@]} )); then echo "install first: ${missing[*]}"; exit 1; fi
  if ! "$deps/venv/bin/python3" -c "import mesonbuild, mako, yaml, packaging" 2>/dev/null; then
    python3 -m venv "$deps/venv"
    "$deps/venv/bin/pip" install -q --disable-pip-version-check meson mako pyyaml packaging
  fi
}

step_sdk() {
  if [ -d "$deps/msvc/VC" ]; then return; fi
  fetch "${XWIN[@]}" "$deps/xwin.tar.gz"
  tar -xzf "$deps/xwin.tar.gz" -C "$deps" --strip-components 1 --wildcards '*/xwin'
  echo "fetching the windows sdk"
  "$deps/xwin" --cache-dir "$deps/xwin-cache" splat --use-winsysroot-style --preserve-ms-arch-notation --output "$deps/msvc"
  rm -rf "$deps/xwin-cache" "$deps/xwin.tar.gz" "$deps/xwin"
}

step_dlls() {
  if [ ! -f "$deps/dxc/dxil.dll" ]; then
    fetch "${DXC[@]}" "$deps/dxc.zip"
    unzip -q -o -j "$deps/dxc.zip" 'bin?x64?dxcompiler.dll' 'bin?x64?dxil.dll' -d "$deps/dxc" 2>/dev/null || [ -f "$deps/dxc/dxil.dll" ]
    rm "$deps/dxc.zip"
  fi
  if [ ! -f "$deps/dxgi.dll" ]; then
    fetch "${DXVK[@]}" "$deps/dxvk.tar.gz"
    tar -xzf "$deps/dxvk.tar.gz" -C "$deps" --strip-components 2 --wildcards '*/x64/dxgi.dll'
    rm "$deps/dxvk.tar.gz"
  fi
  fetch "${D3DCOMPILER[@]}" "$deps/d3dcompiler_47.dll"
}

step_mesa() {
  source_of mesa "${MESA[@]}"
  local b=$deps/mesa/build
  if [ ! -f "$b/build.ninja" ]; then
    logged mesa meson setup "$b" "$deps/mesa" --prefix "$deps/mesa-inst" --buildtype release -Db_ndebug=true \
      -Dvulkan-drivers=amd -Dgallium-drivers= -Dplatforms=x11,wayland -Dllvm=disabled \
      -Dgles1=disabled -Dgles2=disabled -Dopengl=false -Dgbm=disabled -Dglx=disabled -Degl=disabled \
      -Dvideo-codecs= -Dvulkan-layers= -Dtools=
  fi
  echo "building mesa"
  logged mesa meson install -C "$b"
}

step_vkd3d() {
  source_of vkd3d "${VKD3D[@]}"
  local b=$deps/vkd3d/build
  if [ ! -f "$b/build.ninja" ]; then
    logged vkd3d meson setup "$b" "$deps/vkd3d" --cross-file "$deps/vkd3d/build-win64.txt" --buildtype release \
      -Denable_tests=false -Denable_extras=false
  fi
  echo "building vkd3d"
  logged vkd3d meson compile -C "$b"
}

step_wine() {
  local ver
  ver=$(wine --version)
  source_of wine "${WINE[@]}" "${ver%% *}"
  local b=$deps/wineserver
  if [ "$(cat "$b/version" 2>/dev/null)" != "$ver" ]; then rm -rf "$b"; fi
  mkdir -p "$b"
  if [ ! -f "$b/Makefile" ]; then
    (cd "$b" && logged wineserver "$deps/wine/configure" --enable-win64 --without-x --without-wayland --without-freetype --without-vulkan --disable-tests)
  fi
  echo "building wineserver"
  logged wineserver make -C "$b" -j"$(nproc)" server/wineserver
  echo "$ver" > "$b/version"
}

step_prefix() {
  if [ ! -f "$WINEPREFIX/system.reg" ]; then
    echo "creating the prefix"
    mkdir -p "$state"
    WINEDLLOVERRIDES='mscoree,mshtml=d;winemenubuilder.exe=d' wineboot -i
    wineserver -w
  fi
  x86_64-w64-mingw32-strip -o "$sys/d3d12.dll" "$deps/vkd3d/build/libs/d3d12/d3d12.dll"
  x86_64-w64-mingw32-strip -o "$sys/d3d12core.dll" "$deps/vkd3d/build/libs/d3d12core/d3d12core.dll"
  install -m 644 "$deps/dxgi.dll" "$deps/d3dcompiler_47.dll" "$sys/"
  if [ -d "$deps/fsr4/system32" ]; then install -m 644 "$deps/fsr4/system32"/*.dll "$sys/"; fi
}

step_victory() {
  echo "building victory"
  "$root/build.sh"
}

step_fsr4() {
  local bridge=${PROTON:+$PROTON/files/lib/wine/x86_64-windows/amdxc64.dll}
  local proton
  for proton in "$HOME/.local/share/Steam/steamapps/common/Proton - Experimental" "$HOME"/.local/share/Steam/steamapps/common/Proton*; do
    if [ -n "$bridge" ]; then break; fi
    if [ -f "$proton/files/lib/wine/x86_64-windows/amdxc64.dll" ]; then bridge=$proton/files/lib/wine/x86_64-windows/amdxc64.dll; fi
  done
  if [ ! -f "$bridge" ]; then echo "amdxc64.dll comes from proton: point at one with PROTON=/path/to/proton"; exit 1; fi
  fetch "${FSR4[@]}" "$deps/fsr4/amd_fidelityfx_upscaler_dx12.dll"
  if ! echo "${FSR4_DRIVER[1]}  $deps/fsr4/system32/amdxcffx64.dll" | sha256sum -c --status 2>/dev/null; then
    mkdir -p "$deps/fsr4/system32"
    echo "fetching amdxcffx64.dll"
    curl -fL --retry 3 -sS "${FSR4_DRIVER[0]}" | xz -d > "$deps/fsr4/system32/amdxcffx64.dll.part"
    if ! echo "${FSR4_DRIVER[1]}  $deps/fsr4/system32/amdxcffx64.dll.part" | sha256sum -c --status; then echo "${FSR4_DRIVER[0]} is not the file expected"; exit 1; fi
    mv "$deps/fsr4/system32/amdxcffx64.dll.part" "$deps/fsr4/system32/amdxcffx64.dll"
  fi
  install -m 644 "$bridge" "$deps/fsr4/system32/amdxc64.dll"
  if [ -d "$sys" ]; then install -m 644 "$deps/fsr4/system32"/*.dll "$sys/"; fi
}

mkdir -p "$deps"
export PATH=$deps/venv/bin:$PATH
step_tools
if (( $# == 0 )); then set -- sdk dlls mesa vkd3d wine prefix victory; fi
for step in "$@"; do
  case "$step" in
    sdk|dlls|mesa|vkd3d|wine|prefix|victory|fsr4) "step_$step";;
    *) echo "usage: setup.sh [sdk|dlls|mesa|vkd3d|wine|prefix|victory|fsr4]..."; exit 1;;
  esac
done
