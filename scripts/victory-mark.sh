#!/usr/bin/env bash
# Marks a moment of the run that is going (or the newest one): the time, the
# frame, a screenshot into marks/, a short sound. Bound to a global shortcut
# (Ctrl+Alt+M; scripts/victory-mark.sh --install sets up the KDE entry).
#
#   scripts/victory-mark.sh [note]              mark now
#   scripts/victory-mark.sh --note <n> <text>   add a note to mark n afterwards
#   scripts/victory-mark.sh --install           the desktop entry for the shortcut
set -euo pipefail

runs="${VICTORY_RUNS_DIR:-$HOME/.local/share/victory/runs}"
run="$(readlink -f "$runs/latest" 2>/dev/null || true)"

if [[ "${1:-}" == --install ]]; then
  apps="$HOME/.local/share/applications"
  mkdir -p "$apps"
  cat > "$apps/victory-mark.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=VIctory: mark this moment
Comment=Marks the running playtest (time, frame, screenshot)
Exec=$(readlink -f "$0")
Icon=flag
Terminal=false
NoDisplay=false
Categories=Utility;
EOF
  echo "installed $apps/victory-mark.desktop"
  echo "Bind it: System Settings > Keyboard > Shortcuts > Add New > Application > 'VIctory: mark this moment', key Ctrl+Alt+M"
  exit 0
fi

[[ -n "$run" && -d "$run" ]] || { echo "no run folder at $runs/latest" >&2; exit 1; }
marks="$run/marks.csv"
[[ -s "$marks" ]] || echo "mark,wall,t_s,frame,screenshot,note" > "$marks"

if [[ "${1:-}" == --note ]]; then
  n="${2:?--note needs a mark number}"
  shift 2
  python3 -I - "$marks" "$n" "$*" <<'EOF'
import csv, sys
path, n, note = sys.argv[1:]
rows = list(csv.DictReader(open(path, newline="")))
for r in rows:
    if r["mark"] == n:
        r["note"] = note
with open(path, "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=["mark", "wall", "t_s", "frame", "screenshot", "note"])
    w.writeheader()
    w.writerows(rows)
EOF
  echo "note added to mark $n"
  exit 0
fi

n=$(( $(wc -l < "$marks") ))   # the header is line 1: marks start at 1
started=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1])).get("started_epoch", 0))' "$run/run.json" 2>/dev/null || echo 0)
t=$(python3 -I -c 'import sys, time; print(f"{time.time() - float(sys.argv[1]):.1f}")' "$started")
frame=""
[[ -s "$run/frames.csv" ]] && frame=$(tail -n 1 "$run/frames.csv" | cut -d, -f1)
[[ "$frame" =~ ^[0-9]+$ ]] || frame=""
shot=""
if command -v spectacle >/dev/null; then
  shot="marks/mark-$n.png"
  spectacle --background --nonotify --fullscreen --output "$run/$shot" >/dev/null 2>&1 || shot=""
fi
note="${*//,/;}"
echo "$n,$(date +%H:%M:%S),$t,$frame,$shot,$note" >> "$marks"
for s in /usr/share/sounds/freedesktop/stereo/camera-shutter.oga /usr/share/sounds/freedesktop/stereo/bell.oga; do
  [[ -f "$s" ]] && { (paplay "$s" >/dev/null 2>&1 &); break; }
done
echo "mark $n at $t s, frame ${frame:-?}${shot:+, $shot}"
