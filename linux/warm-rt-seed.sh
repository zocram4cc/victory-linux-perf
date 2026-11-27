#!/usr/bin/env bash
set -euo pipefail
coord=@SCRATCH@
work=$coord/fps20-ports
port=@PORTING@
profile=@PROFILE@
src=$profile/seed-precompiled-full
seed=$profile/seed-rt-windows
proton=$profile/team-diagnostics/server-index-ab/indexed
wine=$proton/files/bin/wine
ws=$proton/files/bin/wineserver
exe=$work/bin/victory.rt.keep

exec 9<>"$coord/game.lock"
flock -x 9
if pgrep -af '[v]ictory.exe run'; then echo "game running"; exit 3; fi
(cd "$work" && sha256sum -c SHA256SUMS)

rm -rf "$seed" "$profile/.seed-rt-stage"
mkdir -p "$profile/.seed-rt-stage/data"
cp -a --reflink=always "$profile/loop/prefix" "$profile/.seed-rt-stage/"
cp -a --reflink=always "$src/cache" "$src/vkd3d-cache" "$profile/.seed-rt-stage/"
install -m 644 "$profile/team-diagnostics/server-index-ab/d3dcompiler_47.dll" "$profile/.seed-rt-stage/prefix/pfx/drive_c/windows/system32/d3dcompiler_47.dll"
mv -T -- "$profile/.seed-rt-stage" "$seed"

# Remove cached VSH1 entries that use SLOT_MEMORY (254), and drop their
# pipelines from warm-done.txt + host-pipelines.bin so only RT/builder shaders
# and PSOs are rebuilt by victory.rt.keep.
python3 - "$seed" <<'PY'
import os, struct, sys
from pathlib import Path

seed = Path(sys.argv[1])
cache = seed / "cache"
removed_vsh = 0
for root, _, files in os.walk(cache):
    for f in files:
        p = Path(root) / f
        try:
            b = p.read_bytes()
        except Exception:
            continue
        if len(b) < 16 or b[:4] != b"VSH1":
            continue
        i = 8
        dxil_len = struct.unpack_from("<I", b, i)[0]; i += 4 + dxil_len
        ud_len = struct.unpack_from("<I", b, i)[0]; i += 4 + ud_len * 16
        samp_len = struct.unpack_from("<I", b, i)[0]; i += 4 + samp_len * 16
        ss_len = struct.unpack_from("<I", b, i)[0]; i += 4 + ss_len * 40
        cbv_len = struct.unpack_from("<I", b, i)[0]; i += 4 + cbv_len
        srv_len = struct.unpack_from("<I", b, i)[0]; i += 4
        srvs = b[i:i+srv_len]
        if 254 in srvs:
            p.unlink()
            removed_vsh += 1
print(f"Removed {removed_vsh} SLOT_MEMORY VSH1 files from {cache}")
PY

winpath() { printf 'Z:%s' "${1//\//\\}"; }
export WINEPREFIX="$seed/prefix/pfx"
export VICTORY_CACHE_DIR="Z:$seed/cache"
export VICTORY_DATA_DIR="Z:$seed/data"
VICTORY_GAME_DIR="$(winpath "$(realpath "$port/../title-work")")"
export VICTORY_GAME_DIR
export VKD3D_SHADER_CACHE_PATH="$seed/vkd3d-cache"
export VICTORY_WINDOWS_BAR_MB=4096 VICTORY_BVH_BUILDER=0 VICTORY_CARD_SPANS=1
export VICTORY_PORT_CBV=0 VICTORY_PORT_CONSTANTS=1 VICTORY_PORT_DESCRIPTORS=0
export VICTORY_PORT_GUARD=1 VICTORY_PORT_JOURNAL=1
export VICTORY_BVH_EMPTY=0 VICTORY_LEGACY_FTZ=0
export VICTORY_COMPACT_VIEW_HEAP=1 VICTORY_DYNAMIC_SLOTS=1048576
export WINEDEBUG=-all RUST_LOG=info MANGOHUD=0
export WINEDLLOVERRIDES='d3d12,d3d12core,dxgi,d3d11,d3d10core,d3d9=n,b;d3dcompiler_47=n,b'
userptr_lib=$port/tools/libdrm-amdgpu-shared-userptr
[[ -f $userptr_lib/libdrm_amdgpu.so.1 ]] && export LD_LIBRARY_PATH="$userptr_lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

cd "$seed"
echo "=== victory.rt.keep shaders (jobs 16) $(date +%T)"
timeout 600 "$wine" "$exe" shaders --jobs 16 --list "$(winpath "$seed/shaders-failed.txt")" 9>&- > "$seed/shaders.log" 2>&1 || echo "shaders exit $?"
grep -v -E "vkd3d|^info:|^warn:|ntsync" "$seed/shaders.log" | tail -20
"$ws" -k || true

# Find which shader container hashes in known-shaders.bin correspond to the
# newly translated VSH1 files (or contain SLOT_MEMORY), and drop only their
# pipeline keys from warm-done.txt so `victory warm` compiles just those PSOs.
python3 - "$seed" <<'PY'
import os, struct, sys
from pathlib import Path

seed = Path(sys.argv[1])
pdir = seed / "cache/pipelines"
ks = (pdir / "known-shaders.bin").read_bytes()
kp = (pdir / "known-pipelines.bin").read_bytes()

# Scan all VSH1 files that now exist with recent mtime or 254 in root_srvs?
# Even simpler: check known-shaders.bin containers against VSH1 files? Wait:
# we know the VSH1 cache key is blake3( converter_header || build || container )!
# Instead of computing cache keys, look at which containers in known-shaders.bin
# contain RDNA2 BVH / flat / smem raw-address ops OR even simpler: check mtime!
# All 438 newly translated VSH1 files were JUST written in the last 60s, while
# the 42,846 reflinked VSH1 files have mtimes from hours ago!
# Wait: how do we map VSH1 -> container hash? We don't even need VSH1 -> container!
# Look at `shaders.log` or compute `blake3`? No: in `known-shaders.bin` there are
# only ~3,000 shaders! Let's check how many shaders are in known-shaders.bin!
i = 0
shaders = {}
while i + 37 <= len(ks):
    stage = ks[i]
    h = ks[i+1:i+33]
    ln = struct.unpack_from("<I", ks, i+33)[0]
    if i + 37 + ln > len(ks):
        break
    shaders[h] = ks[i+37:i+37+ln]
    i += 37 + ln

pipes = {}
i = 0
while i + 4 <= len(kp):
    ln = struct.unpack_from("<I", kp, i)[0]
    if i + 4 + ln > len(kp) or ln < 17:
        break
    ent = kp[i+4:i+4+ln]
    key_hex = ent[:16].hex()
    n_sh = ent[16]
    pos = 17
    sh_hashes = []
    for _ in range(n_sh):
        if pos + 33 <= len(ent):
            sh_hashes.append((ent[pos], ent[pos+1:pos+33]))
        pos += 33
    pipes[key_hex] = sh_hashes
    i += 4 + ln

# Compute compute-only pipelines in known-pipelines.bin (all SLOT_MEMORY shaders
# in gameplay are compute shaders: stage == 5!). How many compute pipelines are
# in known-pipelines.bin total? Let's print!
comp_keys = {k for k, shs in pipes.items() if any(st == 5 for st, _ in shs)}
print(f"known shaders={len(shaders)}, known pipelines={len(pipes)}, compute pipelines={len(comp_keys)}")

done_path = pdir / "warm-done.txt"
if done_path.exists():
    lines = done_path.read_text().splitlines()
    kept = [l for l in lines if not (len(l.split()) >= 2 and l.split()[1] in comp_keys)]
    done_path.write_text("\n".join(kept) + ("\n" if kept else ""))
    print(f"warm-done.txt: {len(lines)} -> {len(kept)} (re-warming {len(lines) - len(kept)} compute pipelines)")
PY

echo "=== victory.rt.keep warm (jobs 8) $(date +%T)"
timeout 900 "$wine" "$exe" warm --jobs 8 9>&- > "$seed/warm.log" 2>&1 || echo "warm exit $?"
grep -E "pipelines known| of [0-9]+, | made in |^pipeline " "$seed/warm.log" | tail -20
"$ws" -k || true
echo "SEED_RT_WINDOWS_READY $(date +%T)"
