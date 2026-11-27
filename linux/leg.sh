#!/usr/bin/env bash
# leg.sh <tag> <label> [VAR=value ...]: one labelled, scripted leg into the
# room of the world, with captures, a blink measurement and a row in the
# index of the day's legs (captures-fix/index.html).
#
#   LEG_EXE   binary to run (default bin/victory.fix4.keep); never touches
#             bin/victory.exe or SHA256SUMS
#   LEG_SEED  seed (default seed-precompiled-full)
#   LEG_WORLD seconds in the world (default 80)
#   LEG_KEYS  scripted keys after the menus (default 92:Up, the phone);
#             <from>-<to>:<key> holds a key, W A S D walk
#   LEG_KEYS_FILE a route recorded by a player (play.sh today <minutes>
#             <name>): every key of the run, the menus' too; replaces
#             LEG_KEYS and the scripted Enters
#   LEG_SAVE  none: a start without the saved game (the opening's cut
#             scenes, then the prison); default: the save in the house
#   LEG_VIDEO seconds of the window recorded after the blink measure
#             (default 10; 0: none): video.mp4, 1280 wide, 20 frames a second
#   LEG_SHOTS seconds in the world of the captures (default 28,40,58,68;
#             the blink measure follows the first)
#
# Captures: f0 at 40 s in the world, twelve quick ones for flicker.py right
# after, f1 at 52 s (both before the phone), f2 and f3 at 62 and 70 s (phone
# up; the scripted Up is at 92 s of the run). "In the world" is the first
# frame line with over 1000 draws, so legs of other forks' builds work too.
set -uo pipefail
tag=$1; label=$2; shift 2
[[ $tag =~ ^[a-z][a-z0-9-]{1,40}$ ]] || { echo "tag: lower-case letters, digits and dashes"; exit 2; }
coord=@SCRATCH@
work=$coord/fps20-ports
profile=@PROFILE@
ws=$profile/team-diagnostics/server-index-ab/indexed/files/bin/wineserver
root=@PORTING@/captures-fix
out=$root/$tag
seed=${LEG_SEED:-seed-precompiled-full}
exe=${LEG_EXE:-$work/bin/victory.fix4.keep}
world=${LEG_WORLD:-80}
keys=${LEG_KEYS-92:Up}
shots=${LEG_SHOTS:-28,40,58,68}
all_keys="12:Enter,18:Enter,26:Enter,38:Enter,52:Enter,65:Enter,80:Enter${keys:+,$keys}"
if [[ -n ${LEG_KEYS_FILE:-} ]]; then
    all_keys=$(grep -E '^[0-9.]+(-[0-9.]+)?:[A-Za-z0-9]+$' "$LEG_KEYS_FILE" | paste -sd, -)
    keys="route $(basename "$LEG_KEYS_FILE") ($(grep -c . "$LEG_KEYS_FILE") keys)"
fi

# One leg at a time, whoever starts it: the next waits here for the lock
# (half an hour at most; the game does not inherit it).
exec 8<>"$coord/leg.lock"
flock -x -w 1800 8 || echo "leg lock not had after half an hour: going on" >&2
save_base=$profile/loop
[[ ${LEG_SAVE:-} == none ]] && save_base=$profile/loop-nosave
for _ in $(seq 1 240); do pgrep -f '[v]ictory[^ ]*[.]exe run ' >/dev/null || break; sleep 5; done
if pgrep -f '[v]ictory[^ ]*[.]exe run ' >/dev/null; then echo "game still running"; exit 3; fi
"$ws" -k 2>/dev/null || true
# The launcher starts start.exe /exec <exe>: the name must end in .exe.
run_exe=$work/bin/victory.leg.victory.exe
cp -f "$exe" "$run_exe"
rm -rf "$out" "$profile/harbor-x-$tag" "$profile/harbor-x-$tag-system.tsv"; mkdir -p "$out"
{
    echo "tag: $tag"
    echo "label: $label"
    echo "when: $(date '+%F %T')"
    echo "binary: $(basename "$exe") sha256 $(sha256sum "$exe" | cut -c1-12)"
    echo "seed: $seed"
    echo "switches: ${*:-(launcher defaults)}"
    echo "keys: ${keys:-none}"
    echo "save: ${LEG_SAVE:-the house}"
    echo "shots: $shots"
} > "$out/label.txt"
cd "$work"
(env "$@" HARBOR_SAVE_BASE="$save_base" VICTORY_OVERRIDES="VICTORY_EXE=$run_exe $*" PORTS_LONG=1 PORTS_WORLD_SECONDS="$world" \
  VICTORY_KEYS="$all_keys" \
  VICTORY_PIPELINE_WORKERS=4 timeout 600 python3 run-ports.py "harbor-x-$tag" "$seed" series-x > "$out/run.out" 2>&1 8>&- &)
log=$profile/harbor-x-$tag/logs/victory.log
python3 -I - "$out" "$log" "$work" "$shots" "${LEG_VIDEO:-10}" <<'PY'
import re, subprocess, sys, time
from pathlib import Path
out, log, work = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
sys.path.insert(0, '@SCRATCH@/fulladaptive-player')
import capture
line = re.compile(r'host.*?: [0-9.]+ fps, [0-9.]+ ms/frame.*?(\d+) draws/frame')
t0 = time.time()
def in_world():
    if not log.exists():
        return False
    text = re.sub(r'\x1b\[[0-9;]*m', '', log.read_text(errors='replace'))
    return any(int(m.group(1)) > 1000 for m in line.finditer(text))
while time.time() - t0 < 260 and not in_world():
    time.sleep(1)
if not in_world():
    print('never reached the world'); sys.exit(0)
w = time.time()
def shot(n, at):
    time.sleep(max(0, at - (time.time() - w)))
    d = out / f'f{n}'; d.mkdir(exist_ok=True)
    try:
        capture.save(d)
        subprocess.run(['magick', str(d / 'room-view.png'), '-resize', '1100x', str(out / f'f{n}.jpg')])
    except Exception as e:
        print('capture failed', n, str(e)[:80])
times = [float(x) for x in sys.argv[4].split(',')]
shot(0, times[0])
try:
    r = subprocess.run(['python3', '-I', f'{work}/flicker.py', str(out / 'flicker'), '12'], capture_output=True, text=True, timeout=60)
    (out / 'flicker.txt').write_text(r.stdout + r.stderr)
except Exception as e:
    (out / 'flicker.txt').write_text(f'flicker failed: {e}\n')
video = float(sys.argv[5])
recorder = None
if video > 0:
    try:
        clients = subprocess.run(['xprop', '-root', '_NET_CLIENT_LIST'], capture_output=True, text=True).stdout
        handle = next(h for h in re.findall('0x[0-9a-fA-F]+', clients)
                      if re.search(r'VIctory [0-9]+ fps', subprocess.run(['xprop', '-id', h, '_NET_WM_NAME'], capture_output=True, text=True).stdout))
        (out / 'video.txt').write_text(f'{time.time() - w:.0f} s in the world, {video:.0f} s\n')
        recorder = subprocess.Popen(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'x11grab', '-window_id', handle,
                                     '-framerate', '20', '-t', str(video), '-i', ':0', '-vf', 'scale=1280:-2',
                                     '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '27', '-pix_fmt', 'yuv420p',
                                     '-movflags', '+faststart', str(out / 'video.mp4')])
    except Exception as e:
        print('no video', str(e)[:80])
for n, at in enumerate(times[1:], 1):
    shot(n, at)
if recorder is not None:
    try:
        recorder.wait(timeout=video + 30)
    except Exception:
        recorder.kill()
PY
for _ in $(seq 1 80); do pgrep -f '[v]ictory[^ ]*[.]exe run ' >/dev/null || break; sleep 3; done
"$ws" -k 2>/dev/null || true
sed -r 's/\x1b\[[0-9;]*m//g' "$log" > "$out/victory.log" 2>/dev/null
python3 -I - "$out" <<'PY'
import re, statistics as st, sys
from pathlib import Path
out = Path(sys.argv[1])
rows = []
for l in (out / 'victory.log').read_text(errors='replace').splitlines():
    m = re.search(r'host: ([0-9.]+) fps, ([0-9.]+) ms/frame \(([0-9.]+) preparing: copies ([0-9.]+) ms.*?([0-9.]+) copying back.*?, (\d+) draws/frame, dropped (\d+) without a host pipeline \((\d+) being made\)(?: and (\d+) indirect)?', l)
    if m:
        rows.append([float(x or 0) for x in m.groups()])
room = [r for r in rows if r[5] > 3000][2:] or [r for r in rows if r[5] > 3000]
phone = [r for r in rows if 1000 < r[5] <= 3000]
def med(rs, i): return st.median(r[i] for r in rs) if rs else float('nan')
text = (out / 'victory.log').read_text(errors='replace')
lost = len(re.findall(r'device removed|panicked', text))
s = (f"room: {med(room,1):.1f} ms ({1000/med(room,1):.1f} fps) at {med(room,5):.0f} draws, {len(room)} reports; "
     f"phone up: {med(phone,1):.1f} ms; dropped {med(room,6):.0f} draws ({med(room,7):.0f} pipelines being made), "
     f"{med(room,8):.0f} indirect refused; device-lost or panic lines {lost}") if room else f"no world frames; device-lost or panic lines {lost}"
(out / 'summary.txt').write_text(s + '\n')
print(out.name + ': ' + s)
PY
cat "$out/flicker.txt" 2>/dev/null | head -2
python3 -I "$work/leg-index.py" "$root"
