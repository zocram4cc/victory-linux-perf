#!/usr/bin/env python3
"""Runs compared properly: like with like, enough starts, and no frame rate
taken from a capture nobody looked at.

    scripts/victory-legs.py show <run>...
    scripts/victory-legs.py compare --reference <run> <run | label=<run>,<run>...>... [--window 60]
    scripts/victory-legs.py groups <label>=<run>,<run>,... <label>=... [--count <name>=<file.csv>]...

Common options: --viewed <file> (run folders whose captures a person has
looked at, one per line, # comments; default ~/.local/share/victory/viewed.txt
when it exists), --hitch-ms <ms> (default 250).

show: one line per run: its label (label in leg.json written by
  victory-leg.sh, else the folder's name), the exe's build (profile, commit,
  sha256 when the leg recorded it), the VICTORY_* switches that differ from the
  FIRST run given (+NAME=value set or changed, -NAME unset), seconds, frame
  time median with the 1st/99th percentile, hitches (frames over 250 ms), the
  log's fault lines and the bake's last totals; then the same frame times per
  time window of the route.

compare: per window of --window seconds of the title's clock (frames.csv
  t_ms), each run against the reference, but only where both show the same
  view: the same route (run.json's bench) and median draws per frame within 3 %
  of the reference's in that window. Other windows print "not comparable".
  Runs given as label=<run>,<run> are one group: their differences are given
  as the median and the spread (lowest..highest) over the group's runs.

groups: for faults decided at load time. Per group: its starts (runs) and,
  for hitches, every fault line and each --count <name>=<file.csv> (columns
  run,count; run is the folder's name or path), the per-run values, median and
  range. No verdict when a group has fewer than 4 starts; with 4 or more, for
  every pair of groups, whether the ranges overlap. No p-values.

The rules, after the breakwater fork's leg method: a fault decided at load
time is counted over at least 4 starts; a frame rate from a capture nobody
looked at is void (printed with "(unviewed)"); compare only like with like.
Plain Python 3, no packages.
"""

import argparse
import csv
import json
import os
import re
import statistics
import sys
from pathlib import Path

HOME = Path.home()
DEFAULT_VIEWED = HOME / ".local/share/victory/viewed.txt"
SAME_VIEW = 0.03        # draws per frame within 3 %
MIN_STARTS = 4

# The log's fault lines: name -> pattern (victory.log, colour codes stripped).
FAULTS = {
    "dispatch_dropped": r"a compute dispatch was dropped",
    "changed_unannounced": r"changed after its upload with nothing to tell",
    "stale_host_copy": r"is stale after its refresh",
    "placed_over_host": r"a placed texture lies where a texture the host writes",
    "untranslated": r"shader does not translate",
    "exception": r"exception 0x",
    "device_removed": r"DEVICE_REMOVED|device removed",
    "panicked": r"panicked",
}
# Lines the layer stops logging after a number of them: name -> (the running
# count's field, the lines logged in full). draw.rs logs the first 200 of
# "changed ... nothing to tell", then every 1000th, with changed=<count>.
CAPPED = {"changed_unannounced": (re.compile(r"\bchanged=(\d+)"), 200)}
BAKE = re.compile(r"decoration bake: textures picked by index.*?\bbakes=(\d+).*?\buploaded_so_far=(\d+)")
COLOUR = re.compile(r"\x1b\[[0-9;]*m")
# Switches that name places, not behaviour.
NOT_SWITCHES = re.compile(r"_(CSV|DIR)$|^VICTORY_(KEYS|CONFIG|EXE)$")


# ------------------------------------------------------------------ a run


def pct(values, p):
    s = sorted(values)
    return s[min(len(s) - 1, int(p / 100 * len(s)))] if s else float("nan")


def fnum(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


class Run:
    def __init__(self, path, hitch_ms=250.0):
        self.path = Path(path).expanduser()
        if not self.path.is_dir():
            raise SystemExit(f"no run folder {self.path}")
        self.path = self.path.resolve()
        self.name = self.path.name
        self.hitch_ms = hitch_ms
        self.info = self._json("run.json")
        self.leg = self._json("leg.json")
        self.label = self.leg.get("label") or self.name
        self.frames = self._frames()
        self._faults = None
        self.capped = set()

    def _json(self, name):
        p = self.path / name
        try:
            return json.loads(p.read_text()) if p.exists() else {}
        except (OSError, ValueError):
            return {}

    def _frames(self):
        """(t in s of the title's clock, frame ms, draws) per frame after the first."""
        p = self.path / "frames.csv"
        if not p.exists():
            return []
        out = []
        with p.open(newline="", errors="replace") as f:
            for row in list(csv.DictReader(f))[1:]:
                t, ms = fnum(row.get("t_ms")), fnum(row.get("frame_ms"))
                if t is None or ms is None:
                    continue
                out.append((t / 1000, ms, fnum(row.get("draws")) or 0.0))
        return out

    @property
    def route(self):
        return self.info.get("bench") or self.leg.get("route")

    def build(self):
        i = self.info
        parts = [i.get("profile", "?"), (i.get("git_rev") or "?") + ("+dirty" if i.get("git_dirty") else "")]
        if self.leg.get("exe_sha256"):
            parts.append("sha " + self.leg["exe_sha256"][:12])
        return " ".join(parts)

    def switches(self):
        return {k: v for k, v in self.info.get("env", {}).items()
                if k.startswith("VICTORY_") and not NOT_SWITCHES.search(k)}

    def frame_ms(self, t0=None, t1=None):
        return [ms for t, ms, _ in self.frames if (t0 is None or t >= t0) and (t1 is None or t < t1)]

    def draws(self, t0, t1):
        return [d for t, _, d in self.frames if t0 <= t < t1]

    def hitches(self):
        return sum(1 for ms in self.frame_ms() if ms > self.hitch_ms)

    def faults(self):
        """Counts of the fault lines, and the bake line's last totals. A capped
        line counts by its running count; self.capped names those whose count
        is only a lower bound (the log stopped at its cap)."""
        if self._faults is None:
            counts = {k: 0 for k in FAULTS}
            running = {k: 0 for k in CAPPED}
            pats = {k: re.compile(v) for k, v in FAULTS.items()}
            bake = None
            p = self.path / "victory.log"
            if p.exists():
                with p.open(errors="replace") as f:
                    for line in f:
                        line = COLOUR.sub("", line)
                        for k, pat in pats.items():
                            if pat.search(line):
                                counts[k] += 1
                                if k in CAPPED and (m := CAPPED[k][0].search(line)):
                                    running[k] = max(running[k], int(m.group(1)))
                        m = BAKE.search(line)
                        if m:
                            bake = (int(m.group(1)), int(m.group(2)))
            for k, (_, cap) in CAPPED.items():
                if counts[k] >= cap:
                    self.capped.add(k)  # every 1000th after the cap: the true count lies higher
                counts[k] = max(counts[k], running[k])
            self._faults = (counts, bake)
        return self._faults

    def end(self):
        return max((t for t, _, _ in self.frames), default=0.0)


def windows_of(runs, width):
    end = max((r.end() for r in runs), default=0.0)
    return [(w * width, (w + 1) * width) for w in range(int(end // width) + 1)]


# ------------------------------------------------------------------ viewed


def read_viewed(path):
    """Run folder names (and resolved paths) on the viewed list; None: no list."""
    if path is None:
        path = DEFAULT_VIEWED if DEFAULT_VIEWED.exists() else None
        if path is None:
            return None
    names = set()
    for line in Path(path).expanduser().read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            p = Path(line).expanduser()
            names.add(p.name)
            names.add(str(p.resolve()) if p.exists() else line)
    return names


def is_viewed(run, viewed):
    return viewed is not None and (run.name in viewed or str(run.path) in viewed)


def fps_text(run, viewed, text):
    return text if is_viewed(run, viewed) else f"{text} (unviewed)"


# ------------------------------------------------------------------ show


def ms_text(ms):
    if not ms:
        return "no frames"
    return f"{statistics.median(ms):.1f} ms ({pct(ms, 1):.1f}/{pct(ms, 99):.0f})"


def switch_diff(base, other):
    out = []
    for k in sorted(set(base) | set(other)):
        if k not in other:
            out.append(f"-{k}")
        elif base.get(k) != other[k]:
            out.append(f"+{k}={other[k]}")
    return out


def show(runs, viewed, width):
    base = runs[0].switches()
    print("label | build | switches against the first | s | frame median (p1/p99) | hitches | faults | bake")
    for r in runs:
        diff = switch_diff(base, r.switches()) if r is not runs[0] else []
        counts, bake = r.faults()
        faults = " ".join(f"{k}={v}{'+' if k in r.capped else ''}" for k, v in counts.items() if v) or "none"
        print(f"{r.label} | {r.build()} | {' '.join(diff) or ('(first)' if r is runs[0] else 'same')} | "
              f"{r.info.get('seconds', '?')} s exit {r.info.get('exit_code', '?')} | "
              f"{fps_text(r, viewed, ms_text(r.frame_ms()))} | {r.hitches()} over {r.hitch_ms:g} ms | {faults} | "
              + (f"bakes={bake[0]} uploaded={bake[1]}" if bake else "no bake line"))
        per = [f"{t0:.0f}s {statistics.median(ms):.0f}/{pct(ms, 99):.0f}"
               for t0, t1 in windows_of([r], width) if (ms := r.frame_ms(t0, t1))]
        if per:
            print(f"    per {width:g} s window, median/p99 ms: " + ", ".join(per))


# ------------------------------------------------------------------ compare


def comparable(ref, run, t0, t1):
    """(True, note) when both show the same view in [t0, t1): same route, draws within 3 %."""
    if ref.route != run.route:
        return False, f"route {run.route} against {ref.route}"
    dr, dx = ref.draws(t0, t1), run.draws(t0, t1)
    if not dr or not dx:
        return False, "no frames"
    mr, mx = statistics.median(dr), statistics.median(dx)
    if mr == 0 and mx == 0:
        return True, "0 draws"
    if mr == 0 or abs(mx / mr - 1) > SAME_VIEW:
        return False, f"draws {mx:.0f} against {mr:.0f}" + (f" ({(mx / mr - 1) * 100:+.0f} %)" if mr else "")
    return True, f"draws {mx:.0f} against {mr:.0f}"


def parse_groups(items, hitch_ms):
    """label=<run>,<run> items (or a plain run: its own group) -> [(label, [Run])]."""
    groups = []
    for item in items:
        if "=" in item and not Path(item).exists():
            label, paths = item.split("=", 1)
            runs = [Run(p, hitch_ms) for p in paths.split(",") if p]
        else:
            r = Run(item, hitch_ms)
            label, runs = r.label, [r]
        if not runs:
            raise SystemExit(f"group {label} has no runs")
        groups.append((label, runs))
    return groups


def compare(ref, groups, viewed, width):
    print(f"reference {ref.label} ({ref.name}), route {ref.route}, {ref.build()}"
          f"{'' if is_viewed(ref, viewed) else ' (unviewed)'}")
    for label, runs in groups:
        names = ", ".join(r.name for r in runs)
        diff = switch_diff(ref.switches(), runs[0].switches())
        print(f"{label}: {len(runs)} run{'s' if len(runs) > 1 else ''} ({names}); switches: {' '.join(diff) or 'same'}")
        if any(not is_viewed(r, viewed) for r in runs + [ref]):
            print("  (unviewed): a frame rate from a capture nobody looked at is void; the numbers below are not results")
    print(f"per {width:g} s window: reference median ms, then each group's median difference [spread over its runs]")
    for t0, t1 in windows_of([ref] + [r for _, rs in groups for r in rs], width):
        ref_ms = ref.frame_ms(t0, t1)
        if not ref_ms:
            continue
        ref_med = statistics.median(ref_ms)
        cells = []
        for label, runs in groups:
            diffs, why = [], []
            for r in runs:
                ok, note = comparable(ref, r, t0, t1)
                ms = r.frame_ms(t0, t1)
                if ok and ms:
                    diffs.append(statistics.median(ms) - ref_med)
                else:
                    why.append(note)
            if not diffs:
                cells.append(f"{label}: not comparable ({why[0]})")
                continue
            text = f"{label}: {statistics.median(diffs):+.1f} ms ({statistics.median(diffs) / ref_med * 100:+.0f} %)"
            if len(runs) > 1:
                text += f" [{min(diffs):+.1f}..{max(diffs):+.1f}, {len(diffs)}/{len(runs)} comparable]"
            cells.append(text)
        print(f"  {t0:5.0f}-{t1:<5.0f} ref {ref_med:6.1f} ms | " + " | ".join(cells))


# ------------------------------------------------------------------ groups


def read_counts(spec):
    name, file = spec.split("=", 1)
    values = {}
    with open(Path(file).expanduser(), newline="") as f:
        for row in csv.DictReader(f):
            run, count = (row.get("run") or "").strip(), fnum(row.get("count"))
            if run and count is not None:
                values[Path(run).name] = count
    return name, values


def measures(groups, extra):
    names = ["hitches"] + list(FAULTS) + [n for n, _ in extra]
    table = {}
    for label, runs in groups:
        for r in runs:
            counts, _ = r.faults()
            row = {"hitches": r.hitches(), **counts}
            for n, values in extra:
                row[n] = values.get(r.name)
            table[(label, r.name)] = row
    return names, table


def groups_report(groups, extra):
    names, table = measures(groups, extra)
    for label, runs in groups:
        print(f"{label}: {len(runs)} start{'s' if len(runs) != 1 else ''} ({', '.join(r.name for r in runs)})")
    ranges = {}
    if any(r.capped for _, runs in groups for r in runs):
        print("(a value with + is a lower bound: the log stops writing that line after its cap)")
    for n in names:
        print(f"{n}:")
        for label, runs in groups:
            vals = [table[(label, r.name)][n] for r in runs]
            known = [v for v in vals if v is not None]
            shown = " ".join("-" if v is None else f"{v:g}{'+' if n in r.capped else ''}" for v, r in zip(vals, runs))
            if known:
                ranges[(n, label)] = (min(known), max(known), any(n in r.capped for r in runs))
                print(f"  {label}: {shown}  median {statistics.median(known):g}, range {min(known):g}..{max(known):g}")
            else:
                print(f"  {label}: {shown}  (no values)")
    short = [(label, len(runs)) for label, runs in groups if len(runs) < MIN_STARTS]
    if short:
        print("no verdict: " + ", ".join(f"{label} has {n} start{'s' if n != 1 else ''}" for label, n in short)
              + f"; a fault decided at load time is counted over at least {MIN_STARTS} starts per group")
        return 1
    if len(groups) < 2:
        print("one group: nothing to set against it")
        return 0
    print(f"ranges (every group has {MIN_STARTS} or more starts):")
    for n in names:
        for i, (la, _) in enumerate(groups):
            for lb, _ in groups[i + 1:]:
                ra, rb = ranges.get((n, la)), ranges.get((n, lb))
                if ra is None or rb is None:
                    continue
                if ra[2] or rb[2]:
                    word = "unknown: a count is only a lower bound (the log stops at its cap)"
                elif ra[1] < rb[0]:
                    word = f"apart: every start of {la} lower than every start of {lb}"
                elif rb[1] < ra[0]:
                    word = f"apart: every start of {lb} lower than every start of {la}"
                else:
                    word = "overlap"
                print(f"  {n}: {la} {ra[0]:g}..{ra[1]:g} against {lb} {rb[0]:g}..{rb[1]:g}: {word}")
    return 0


# ------------------------------------------------------------------ main


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--viewed", help="file of run folders whose captures were looked at")
    p.add_argument("--hitch-ms", type=float, default=250.0)
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("show")
    s.add_argument("runs", nargs="+")
    s.add_argument("--window", type=float, default=60.0)
    c = sub.add_parser("compare")
    c.add_argument("--reference", required=True)
    c.add_argument("runs", nargs="+")
    c.add_argument("--window", type=float, default=60.0)
    g = sub.add_parser("groups")
    g.add_argument("groups", nargs="+")
    g.add_argument("--count", action="append", default=[], help="name=file.csv with columns run,count")
    for sp in (s, c, g):
        sp.add_argument("--viewed", default=argparse.SUPPRESS)
        sp.add_argument("--hitch-ms", type=float, default=argparse.SUPPRESS)
    a = p.parse_args(argv)
    viewed = read_viewed(a.viewed)
    if a.cmd == "show":
        show([Run(r, a.hitch_ms) for r in a.runs], viewed, a.window)
    elif a.cmd == "compare":
        compare(Run(a.reference, a.hitch_ms), parse_groups(a.runs, a.hitch_ms), viewed, a.window)
    else:
        bad = [x for x in a.groups if "=" not in x]
        if bad:
            p.error(f"groups are label=<run>,<run>,...: {bad[0]}")
        return groups_report(parse_groups(a.groups, a.hitch_ms), [read_counts(x) for x in a.count])
    return 0


if __name__ == "__main__":
    sys.exit(main())
