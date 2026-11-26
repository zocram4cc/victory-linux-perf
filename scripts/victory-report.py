#!/usr/bin/env python3
"""A playtest run's folder (play-linux.sh), summed up in text.

    scripts/victory-report.py [run]                  the newest run when none is given
    scripts/victory-report.py <run> --against <run>  two runs side by side (an A/B)

Reads run.json, frames.csv, system.csv, mangohud.csv, marks.csv and
victory.log; any of them may be missing. Plain Python 3, no packages.

play-linux.sh also calls it to write run.json (--write-run-json).
"""

import argparse
import csv
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

RUNS = Path(os.environ.get("VICTORY_RUNS_DIR", Path.home() / ".local/share/victory/runs"))


# ----------------------------------------------------------------- run.json


def sh(*cmd: str) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=10).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def write_run_json(args: argparse.Namespace) -> None:
    run = Path(args.write_run_json)
    path = run / "run.json"
    info = json.loads(path.read_text()) if path.exists() else {}
    if args.start:
        repo = args.repo or "."
        meminfo = Path("/proc/meminfo").read_text()
        total_kb = int(re.search(r"MemTotal:\s+(\d+)", meminfo).group(1))
        proton = os.environ.get("PROTONPATH", "")
        version_file = Path(proton) / "version"
        info.update(
            started=time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            started_epoch=time.time(),
            profile=args.profile,
            bench=args.bench,
            exe=args.exe,
            git_rev=sh("git", "-C", repo, "rev-parse", "--short", "HEAD"),
            git_dirty=bool(sh("git", "-C", repo, "status", "--porcelain", "--untracked-files=no")),
            proton=version_file.read_text().split()[-1] if version_file.exists() else Path(proton).name,
            umu=sh("umu-run", "--version").splitlines()[0] if sh("umu-run", "--version") else "",
            gpu=sh("nvidia-smi", "--query-gpu=name,driver_version,memory.total", "--format=csv,noheader"),
            cpu=next((l.split(":", 1)[1].strip() for l in Path("/proc/cpuinfo").read_text().splitlines()
                      if l.startswith("model name")), ""),
            logical_cpus=os.cpu_count(),
            ram_gb=round(total_kb / 1024 / 1024, 1),
            kernel=platform.release(),
            env={k: v for k, v in sorted(os.environ.items())
                 if k.startswith(("VICTORY_", "VKD3D_", "MANGOHUD", "PROTON_", "DXVK_"))},
        )
    if args.end:
        info.update(ended=time.strftime("%Y-%m-%dT%H:%M:%S%z"), exit_code=args.exit_code, seconds=args.seconds)
        log = run / "victory.log"
        if log.exists():
            text = log.read_text(errors="replace")
            m = re.search(r"card memory the CPU maps[^\n]*supported=(\w+)", text)
            info["rebar_supported"] = m.group(1) if m else None
    path.write_text(json.dumps(info, indent=2) + "\n")


# ------------------------------------------------------------------ reading


def rows(path: Path) -> list[dict]:
    if not path.exists():
        return []
    with path.open(newline="", errors="replace") as f:
        lines = [l for l in f if l.strip()]
    # MangoHud writes a few lines of system info before its header.
    for i, line in enumerate(lines):
        if line.lower().startswith(("frame", "fps,", "t_s", "time")):
            return list(csv.DictReader(lines[i:]))
    return list(csv.DictReader(lines))


def num(row: dict, key: str) -> float | None:
    try:
        v = row.get(key, "")
        return float(v) if v not in ("", None) else None
    except ValueError:
        return None


def pct(values: list[float], p: float) -> float:
    if not values:
        return float("nan")
    s = sorted(values)
    return s[min(len(s) - 1, int(p / 100 * len(s)))]


def fmt(x: float | None, digits: int = 1) -> str:
    return "-" if x is None or x != x else f"{x:.{digits}f}"


class Run:
    def __init__(self, path: Path):
        self.path = path
        self.info = json.loads((path / "run.json").read_text()) if (path / "run.json").exists() else {}
        self.frames = rows(path / "frames.csv")
        self.system = rows(path / "system.csv")
        self.mango = rows(path / "mangohud.csv")
        self.marks = rows(path / "marks.csv")
        self.log = (path / "victory.log").read_text(errors="replace").splitlines() if (path / "victory.log").exists() else []

    # Frame times past the first frame (loading shows as one long one).
    def frame_ms(self) -> list[float]:
        return [v for r in self.frames[1:] if (v := num(r, "frame_ms")) is not None]

    def column(self, key: str) -> list[float]:
        return [v for r in self.frames[1:] if (v := num(r, key)) is not None]


# ---------------------------------------------------------------- the report


COLUMNS = [
    ("submit_busy_ms", "submitting thread busy"),
    ("waits_for_gpu_ms", "waits for the card"),
    ("gpu_idle_ms", "card idle"),
    ("gpu_idle_behind_us_ms", "card idle behind us"),
    ("title_copy_ms", "title memory copies"),
    ("texture_refresh_ms", "texture refreshes"),
    ("copy_back_ms", "copy-back"),
    ("card_up_mb", "MB up to the card"),
    ("draws", "draws"),
    ("draws_dropped", "draws dropped"),
]


def summary(run: Run) -> dict:
    ms = run.frame_ms()
    out = {"frames": len(ms)}
    if ms:
        med = statistics.median(ms)
        out.update(median_ms=med, fps=1e3 / med if med else 0, p1_low_fps=1e3 / pct(ms, 99), p99_ms=pct(ms, 99),
                   worst_ms=max(ms), stutters=sum(1 for v in ms if v > 2 * med))
        for key, _ in COLUMNS:
            col = run.column(key)
            out[key] = statistics.median(col) if col else None
    rss = [v for r in run.system if (v := num(r, "victory_rss_mb")) is not None]
    swap = [v for r in run.system if (v := num(r, "swap_used_mb")) is not None]
    avail = [v for r in run.system if (v := num(r, "mem_available_mb")) is not None]
    vram = [v for r in run.system if (v := num(r, "vram_used_mb")) is not None]
    out.update(rss_peak_gb=max(rss) / 1024 if rss else None, swap_peak_gb=max(swap) / 1024 if swap else None,
               ram_available_min_gb=min(avail) / 1024 if avail else None,
               vram_peak_gb=max(vram) / 1024 if vram else None)
    mango = [v for r in run.mango if (v := num(r, "frametime")) is not None]
    out["mangohud_median_ms"] = statistics.median(mango) if mango else None
    return out


def report(run: Run) -> None:
    i = run.info
    print(f"run {run.path.name}")
    if i:
        print(f"  {i.get('profile', '?')} build {i.get('git_rev', '?')}{' (dirty)' if i.get('git_dirty') else ''}"
              f", {i.get('proton', '?')}, {i.get('gpu', '?')}, {i.get('ram_gb', '?')} GB RAM"
              f", exit {i.get('exit_code', '?')} after {i.get('seconds', '?')} s"
              f"{', bench ' + i['bench'] if i.get('bench') else ''}, ReBAR {i.get('rebar_supported', '?')}")
        changed = {k: v for k, v in i.get("env", {}).items() if k in (
            "VICTORY_WINDOWS_BAR_MB", "VICTORY_CARD_WINDOWS_MB", "VICTORY_CULL_NONE", "VICTORY_FOLD_BUFFERS",
            "VICTORY_WAVE_LDS_LOOSE", "VICTORY_INDIRECT_REST", "VKD3D_CONFIG")}
        print("  " + ", ".join(f"{k}={v}" for k, v in changed.items()))
    s = summary(run)
    ms = run.frame_ms()
    print()
    if not ms:
        print("no frames (frames.csv missing or empty)")
    else:
        print(f"frames {s['frames']}: median {fmt(s['median_ms'])} ms ({fmt(s['fps'])} fps), "
              f"99th {fmt(s['p99_ms'])} ms (1% low {fmt(s['p1_low_fps'])} fps), worst {fmt(s['worst_ms'], 0)} ms, "
              f"{s['stutters']} frames over twice the median")
        if s["mangohud_median_ms"] is not None:
            print(f"  MangoHud's median: {fmt(s['mangohud_median_ms'])} ms")
        print("a frame's median:")
        for key, label in COLUMNS:
            print(f"  {label:<26} {fmt(s.get(key), 2)}")
        print("per minute (median ms / 99th ms):")
        t0 = num(run.frames[1], "t_ms") or 0
        minutes: dict[int, list[float]] = {}
        for r in run.frames[1:]:
            t, v = num(r, "t_ms"), num(r, "frame_ms")
            if t is not None and v is not None:
                minutes.setdefault(int((t - t0) / 60000), []).append(v)
        for m, vals in sorted(minutes.items()):
            print(f"  {m:>3}: {fmt(statistics.median(vals))} / {fmt(pct(vals, 99))}")
        med = s["median_ms"]
        worst = sorted(((num(r, "frame_ms") or 0, r) for r in run.frames[1:]), key=lambda x: -x[0])[:8]
        print("longest frames:")
        for v, r in worst:
            if v <= 2 * med:
                break
            print(f"  frame {r['frame']} at {fmt((num(r, 't_ms') or 0) / 1e3)} s: {fmt(v, 0)} ms"
                  f" (submit {r.get('submit_busy_ms')}, waits {r.get('waits_for_gpu_ms')},"
                  f" pipelines queued {r.get('pipelines_queued')}, made {r.get('pipelines_made')})")
    print()
    print(f"memory: title peak {fmt(s['rss_peak_gb'])} GB, RAM available low {fmt(s['ram_available_min_gb'])} GB,"
          f" swap peak {fmt(s['swap_peak_gb'])} GB, VRAM peak {fmt(s['vram_peak_gb'])} GB")
    levels = Counter()
    messages = Counter()
    for line in run.log:
        m = re.search(r"\b(ERROR|WARN)\b\s+(\S+?):\s*(.*)", line)
        if m:
            levels[m.group(1)] += 1
            messages[(m.group(1), m.group(2), re.sub(r"\d+", "#", m.group(3))[:110])] += 1
    print(f"log: {len(run.log)} lines, {levels['ERROR']} errors, {levels['WARN']} warnings")
    for (lvl, target, msg), n in messages.most_common(12):
        print(f"  {n:>6} {lvl} {target}: {msg}")
    if run.marks:
        print("marks:")
        for m in run.marks:
            print(f"  #{m.get('mark')} at {m.get('t_s')} s, frame {m.get('frame')}: {m.get('note') or ''}"
                  f"{' [' + m['screenshot'] + ']' if m.get('screenshot') else ''}")
            frame = int(m.get("frame") or 0)
            near = [r for r in run.frames if abs(int(r["frame"]) - frame) <= 30]
            vals = [v for r in near if (v := num(r, "frame_ms")) is not None]
            if vals:
                print(f"      frames around it: median {fmt(statistics.median(vals))} ms, worst {fmt(max(vals), 0)} ms")
            t = float(m.get("t_s") or 0)
            srow = min(run.system, key=lambda r: abs((num(r, "t_s") or 0) - t), default=None)
            if srow:
                print(f"      system: title {srow.get('victory_rss_mb')} MB, RAM free {srow.get('mem_available_mb')} MB,"
                      f" swap {srow.get('swap_used_mb')} MB, VRAM {srow.get('vram_used_mb')} MB,"
                      f" CPU {srow.get('cpu_busy_pct')}% (busiest core {srow.get('cpu_busiest_core_pct')}%)")


def compare(a: Run, b: Run) -> None:
    sa, sb = summary(a), summary(b)
    print(f"{'':<28}{a.path.name:>22}{b.path.name:>22}{'change':>10}")
    keys = [("median_ms", "median frame ms"), ("fps", "fps"), ("p99_ms", "99th ms"), ("p1_low_fps", "1% low fps"),
            ("stutters", "stutters"), *COLUMNS, ("rss_peak_gb", "title peak GB"), ("swap_peak_gb", "swap peak GB"),
            ("vram_peak_gb", "VRAM peak GB")]
    for key, label in keys:
        x, y = sa.get(key), sb.get(key)
        change = f"{(y - x) / x * 100:+.0f}%" if isinstance(x, (int, float)) and isinstance(y, (int, float)) and x else ""
        print(f"{label:<28}{fmt(x, 2):>22}{fmt(y, 2):>22}{change:>10}")
    ea, eb = a.info.get("env", {}), b.info.get("env", {})
    diff = sorted(k for k in set(ea) | set(eb) if ea.get(k) != eb.get(k) and not k.endswith(("_CSV", "_DIR", "CONFIG")))
    if diff:
        print("settings that differ:")
        for k in diff:
            print(f"  {k}: {ea.get(k, '(unset)')} -> {eb.get(k, '(unset)')}")


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("run", nargs="?")
    p.add_argument("--against")
    p.add_argument("--write-run-json", help=argparse.SUPPRESS)
    p.add_argument("--start", action="store_true", help=argparse.SUPPRESS)
    p.add_argument("--end", action="store_true", help=argparse.SUPPRESS)
    p.add_argument("--profile", help=argparse.SUPPRESS)
    p.add_argument("--exe", help=argparse.SUPPRESS)
    p.add_argument("--repo", help=argparse.SUPPRESS)
    p.add_argument("--bench", help=argparse.SUPPRESS)
    p.add_argument("--exit-code", type=int, help=argparse.SUPPRESS)
    p.add_argument("--seconds", type=int, help=argparse.SUPPRESS)
    args = p.parse_args()
    if args.write_run_json:
        write_run_json(args)
        return
    path = Path(args.run) if args.run else RUNS / "latest"
    if not (path / "run.json").exists() and not (path / "frames.csv").exists():
        sys.exit(f"no run at {path}")
    run = Run(path.resolve())
    if args.against:
        compare(Run(Path(args.against).resolve()), run)
    else:
        report(run)


if __name__ == "__main__":
    main()
