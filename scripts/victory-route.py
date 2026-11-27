#!/usr/bin/env python3
"""victory-route.py: a recording of VICTORY_KEYS_RECORD made a route.

    scripts/victory-route.py RECORDING [-o scripts/bench/NAME.keys] [options]

The runtime, with VICTORY_KEYS_RECORD=<file>, appends what the player presses
to <file>, in seconds from the moment the title first reads input (the clock
VICTORY_KEYS plays a route by):

    <from>-<to>:<key>                keyboard key held
    <from>-<to>:pad<n>:<key>         pad button held, by the stand-in pad's key
                                     (Enter = A, Back = B, Esc = menu, Tab = view,
                                     X, Y, arrows = d-pad, Q/E = shoulders,
                                     F/H = stick clicks)
    <t> pad<n> axes lx ly rx ry lt rt  sticks -1..1 (up +), triggers 0..1

This writes a route for `--bench` (scripts/bench/<name>.keys): `duration=<s>`
and `<from>-<to>:<key>` lines. A route holds keys only. The keyboard's stand-in
pad turns W/A/S/D into the left stick, I/J/K/L into the right one and Z/C into
the triggers, always fully: a stick is replayed as full deflection in one of
eight directions, a trigger as fully pressed or not.

  --sticks threshold (default): a key is held while its axis is past
      --threshold (0.5), let go below --release (0.3). Direction is kept, the
      amount is not: a half tilt walks as fast and turns the camera as fast as
      a full one.
  --sticks pulse: each --pulse window (0.2 s) holds the key for the axis's
      mean deflection in it times the window, so a half tilt turns at about
      half the speed on average, in steps. Titles that ease the camera in
      make this rougher still.

Keyboard keys come out of the recorder as many short presses when the X
server repeats a held key: presses of one keyboard key at most --gap (0.45 s)
apart are joined. Keys the route cannot name (vkXX lines) are left out and
counted.
"""

import argparse
import math
import re
import sys

HOLD = re.compile(r"^\s*([0-9.]+)-([0-9.]+):(?:(pad\d+):)?([A-Za-z0-9]+)\s*$")
AXES = re.compile(r"^\s*([0-9.]+)\s+(pad\d+)\s+axes((?:\s+-?[0-9.]+){6})\s*$")

# Axis index, sign, key: what the stand-in pad makes of the keys.
AXIS_KEYS = [
    (0, +1, "D"), (0, -1, "A"), (1, +1, "W"), (1, -1, "S"),
    (2, +1, "L"), (2, -1, "J"), (3, +1, "I"), (3, -1, "K"),
    (4, +1, "Z"), (5, +1, "C"),
]

NAMED = {"Enter", "Esc", "Tab", "Back", "Up", "Down", "Left", "Right", "Space", "Shift", "Ctrl"}


def known(key):
    return key in NAMED or (len(key) == 1 and key.isalnum())


def read(path):
    """Keyboard holds, pad button holds, per pad its axis samples, the last
    time seen, and the keys left out."""
    keyboard, buttons, axes, end, unnamed = [], [], {}, 0.0, {}
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            m = HOLD.match(line)
            if m:
                a, b, pad, key = float(m[1]), float(m[2]), m[3], m[4]
                end = max(end, b)
                if not known(key.upper() if len(key) == 1 else key):
                    unnamed[key] = unnamed.get(key, 0) + 1
                    continue
                key = key.upper() if len(key) == 1 else key
                (buttons if pad else keyboard).append((a, b, key))
                continue
            m = AXES.match(line)
            if m:
                t = float(m[1])
                end = max(end, t)
                axes.setdefault(m[2], []).append((t, [float(v) for v in m[3].split()]))
                continue
            print(f"victory-route: not understood, left out: {line.rstrip()}", file=sys.stderr)
    for samples in axes.values():
        samples.sort(key=lambda s: s[0])
    return keyboard, buttons, axes, end, unnamed


def threshold_holds(samples, end, on, off):
    """Holds of the stick's keys while an axis is past `on` (let go below `off`)."""
    holds = []
    for axis, sign, key in AXIS_KEYS:
        since = None
        for t, values in samples:
            v = values[axis] * sign
            if since is None and v >= on:
                since = t
            elif since is not None and v < off:
                holds.append((since, t, key))
                since = None
        if since is not None:
            holds.append((since, end, key))
    return holds


def pulse_holds(samples, end, window, least=0.1):
    """Per window, the key held for the axis's mean deflection times the window."""
    holds = []
    if not samples:
        return holds
    first = samples[0][0]
    for axis, sign, key in AXIS_KEYS:
        # The axis is what its last sample said, until the next one.
        pieces = []
        for i, (t, values) in enumerate(samples):
            until = samples[i + 1][0] if i + 1 < len(samples) else end
            v = max(0.0, values[axis] * sign)
            if v > 0 and until > t:
                pieces.append((t, until, min(v, 1.0)))
        if not pieces:
            continue
        k = math.floor(first / window)
        while k * window < end:
            a, b = k * window, (k + 1) * window
            mean = sum(max(0.0, min(b, y) - max(a, x)) * v for x, y, v in pieces) / window
            if mean >= least:
                holds.append((a, a + min(mean, 1.0) * window, key))
            k += 1
    return holds


def joined(holds, gap):
    """Holds of one key at most `gap` apart (or overlapping) made one."""
    by_key = {}
    for a, b, key in holds:
        by_key.setdefault(key, []).append([a, b])
    out = []
    for key, spans in by_key.items():
        spans.sort()
        cur = spans[0]
        for a, b in spans[1:]:
            if a - cur[1] <= gap:
                cur[1] = max(cur[1], b)
            else:
                out.append((cur[0], cur[1], key))
                cur = [a, b]
        out.append((cur[0], cur[1], key))
    return out


def read_route(path):
    """The holds and duration of an existing route (for --append)."""
    holds, duration = [], 0.0
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip().replace(" ", "")
            if not line or line.startswith("#"):
                continue
            if line.startswith("duration="):
                duration = float(line.split("=", 1)[1])
                continue
            at, _, key = line.partition(":")
            if "-" in at:
                a, b = (float(x) for x in at.split("-", 1))
            else:
                a = float(at)
                b = a + 0.2
            holds.append((a, b, key))
    return holds, duration


def main(argv=None):
    p = argparse.ArgumentParser(
        description="Turn a VICTORY_KEYS_RECORD recording into a --bench route (scripts/bench/<name>.keys).",
        epilog="Replay: scripts/play-linux.sh --bench <name> (keys from scripts/bench/<name>.keys).",
    )
    p.add_argument("recording")
    p.add_argument("-o", "--out", help="the route to write (default: standard output)")
    p.add_argument("--append", metavar="ROUTE", help="merge into this route's keys (and keep its duration if longer)")
    p.add_argument("--gap", type=float, default=0.45, help="join presses of one keyboard key this close (s, 0.45)")
    p.add_argument("--sticks", choices=["threshold", "pulse"], default="threshold")
    p.add_argument("--threshold", type=float, default=0.5, help="stick/trigger counts as pushed from here (0.5)")
    p.add_argument("--release", type=float, default=0.3, help="and as let go below this (0.3)")
    p.add_argument("--pulse", type=float, default=0.2, help="window of --sticks pulse (s, 0.2)")
    p.add_argument("--start", type=float, default=0.0, help="leave out what began before this (s)")
    p.add_argument("--end", type=float, default=math.inf, help="leave out what began after this (s)")
    p.add_argument("--shift", type=float, default=0.0, help="move every key this much later (s; negative: earlier)")
    p.add_argument("--tail", type=float, default=10.0, help="seconds the run goes on after the last key (10)")
    p.add_argument("--duration", type=float, help="the run's length instead (s)")
    p.add_argument("--no-pad", action="store_true", help="leave out the pads")
    p.add_argument("--no-keyboard", action="store_true", help="leave out the keyboard")
    a = p.parse_args(argv)

    keyboard, buttons, axes, end, unnamed = read(a.recording)
    holds = [] if a.no_keyboard else joined(keyboard, a.gap)
    if not a.no_pad:
        holds += buttons
        for samples in axes.values():
            if a.sticks == "pulse":
                holds += pulse_holds(samples, end, a.pulse)
            else:
                holds += threshold_holds(samples, end, a.threshold, a.release)
    holds = [(x, y, k) for x, y, k in holds if a.start <= x <= a.end]
    holds = [(max(0.0, x + a.shift), max(0.0, y + a.shift), k) for x, y, k in holds]
    holds = [(x, max(y, x + 0.05), k) for x, y, k in holds if y > 0]
    duration = 0.0
    if a.append:
        old, duration = read_route(a.append)
        holds += old
    # Keyboard and pad may hold one key at once: one hold.
    holds = sorted(joined(holds, 0.0))
    last = max((y for _, y, _ in holds), default=0.0)
    duration = max(duration, math.ceil(last + a.tail)) if a.duration is None else a.duration

    lines = [
        f"# A route made by scripts/victory-route.py from {a.recording.split('/')[-1]}",
        f"# (sticks: {a.sticks}"
        + (f", threshold {a.threshold}/{a.release}" if a.sticks == "threshold" else f", window {a.pulse} s")
        + f"; keyboard presses joined {a.gap} s apart). Seconds from the title's first input read.",
        "# Sticks replay as full deflection on W/A/S/D and I/J/K/L, triggers as Z/C fully pressed.",
        f"duration={duration:g}",
    ]
    lines += [f"{x:.2f}-{y:.2f}:{k}" for x, y, k in holds]
    text = "\n".join(lines) + "\n"
    if a.out:
        with open(a.out, "w", encoding="utf-8") as f:
            f.write(text)
    else:
        sys.stdout.write(text)
    left = ", ".join(f"{k} x{n}" for k, n in sorted(unnamed.items()))
    span = f"{holds[0][0]:.0f} s to {last:.0f} s" if holds else "nothing"
    print(
        f"victory-route: {len(holds)} holds, {span}, duration {duration:g} s"
        + (f"; left out (no name in VICTORY_KEYS): {left}" if left else ""),
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
