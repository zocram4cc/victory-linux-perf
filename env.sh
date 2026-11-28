root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
[ -f "$root/run.conf" ] && source "$root/run.conf"
state=$root/local
export WINEPREFIX=$state/prefix
export WINEDEBUG=${WINEDEBUG:--all}
export VK_DRIVER_FILES=$root/deps/mesa-inst/share/vulkan/icd.d/radeon_icd.x86_64.json
export MESA_SHADER_CACHE_DIR=$state/mesa-cache
win() { printf 'Z:%s' "${1//\//\\}"; }
export VKD3D_SHADER_CACHE_PATH=$(win "$state/vkd3d-cache")
