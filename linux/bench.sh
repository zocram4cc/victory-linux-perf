#!/usr/bin/env bash
# Scripted benchmark: starts the diagnostics build with VICTORY_SKIP_INTRO=30
# (Enter every two seconds for the first half minute: through the intro and
# the menus into the world, with no one at the keyboard), stops after
# <seconds> and prints what the world's frames cost. Build first: linux/build.sh --profile diag --features diagnostics
#
# Usage: linux/bench.sh <name> <seconds> [VAR=value ...]
#   linux/bench.sh base 170
#   linux/bench.sh nojoin 170 VICTORY_JOIN_VIEWS=0
# Logs are kept under $STATE_DIR/bench/<name>.log.
set -uo pipefail
name=$1; secs=$2; shift 2
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$ROOT/linux/local.env" ]] && source "$ROOT/linux/local.env"
OUT="${STATE_DIR:-$HOME/.local/share/victory-linux}/bench"
mkdir -p "$OUT"
log="$OUT/$name.log"
# A log left by an earlier run of this name ends in EXIT: the loop below would
# take the new run for over in its first second.
rm -f "$log"
(
    env VICTORY_SKIP_INTRO=30 "$@" PROFILE=diag VICTORY_LOG_FILE="$log" \
        PROTON_LOG_FILE="$OUT/$name.proton.log" "$ROOT/linux/run.sh" >/dev/null 2>&1
    echo "EXIT $?" >> "$log"
) &
for _ in $(seq 1 "$secs"); do
    /usr/bin/sleep 1
    grep -aq '^EXIT' "$log" 2>/dev/null && break
done
ended="by itself"
grep -aq '^EXIT' "$log" 2>/dev/null || ended="stopped at ${secs} s"
pkill -f 'victory[.]exe' 2>/dev/null
/usr/bin/sleep 3
python3 -I - "$log" "$name" "$ended" <<'PY'
import re
import statistics
import sys
from datetime import datetime

path, name, ended = sys.argv[1], sys.argv[2], sys.argv[3]
ansi = re.compile(r"\x1b\[[0-9;]*m")
stamp = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z")
frame = re.compile(
    r"([\d.]+) fps, ([\d.]+) ms/frame \(([\d.]+) preparing: copies ([\d.]+) ms [\d.]+ MB, "
    r"textures ([\d.]+) ms [\d.]+ MB; ([\d.]+) on the GPU; ([\d.]+) copying back.*?(\d+) draws/frame, "
    r"dropped (\d+) without a host pipeline \((\d+) being made\) and (\d+) indirect"
)
refresh = re.compile(
    r"texture refreshes per frame, all threads: ([\d.]+) ms, of them ([\d.]+) ms waiting for a texture.*?"
    r"([\d.]+) ms asking the write watch and the guards, ([\d.]+) ms waiting for title memory"
)


def when(line):
    m = stamp.match(line)
    return datetime.fromisoformat(m.group(1)[:26]) if m else None


start = None
frames, refreshes = [], []
exit_line = crash = None
with open(path, errors="replace") as f:
    for raw in f:
        line = ansi.sub("", raw)
        t = when(line)
        if t and start is None:
            start = t
        if line.startswith("EXIT"):
            exit_line = line.strip()
        if "unhandled exception" in line or "BacktraceBreakpad" in line and crash is None:
            crash = (t - start).total_seconds() if t and start else -1
        m = frame.search(line)
        if m and t:
            frames.append(((t - start).total_seconds(), [float(x) for x in m.groups()]))
        m = refresh.search(line)
        if m and t:
            refreshes.append(((t - start).total_seconds(), [float(x) for x in m.groups()]))

world = [(t, v) for t, v in frames if v[7] >= 1000]
print(f"== {name}: {ended} ({exit_line}); title crash handler at {crash} s")
if not world:
    last = frames[-1] if frames else None
    print("   never reached the world; last frame line:", last)
    sys.exit(0)
t0, t1 = world[0][0], world[-1][0]
med = lambda i: statistics.median(v[i] for _, v in world)
print(f"   world from {t0:.0f} s to {t1:.0f} s ({len(world)} frame lines)")
print(
    f"   median ms/frame {med(1):.0f} (preparing {med(2):.0f}: copies {med(3):.0f}, textures {med(4):.0f}; "
    f"GPU {med(5):.0f}; copying back {med(6):.0f}); draws {med(7):.0f}, dropped {med(8):.0f}+{med(10):.0f} indirect, being made {med(9):.0f}"
)
print("   ms/frame over time:", " ".join(f"{v[1]:.0f}" for _, v in world))
rw = [v for t, v in refreshes if t0 <= t <= t1 + 1]
if rw:
    m = lambda i: statistics.median(v[i] for v in rw)
    print(f"   texture refreshes median {m(0):.0f} ms: write watch {m(2):.0f}, waiting for title memory {m(3):.0f}")
PY
