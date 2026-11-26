#!/usr/bin/env python3
"""Phase 2.1: how much the title's CPU reads and writes its GPU memory.

An AMD IBS op profile of the whole system (`perf record -a -e
ibs_op/cnt_ctl=1/ -c PERIOD -d`, needs kernel.perf_event_paranoid <= 0)
gives, for sampled micro-ops, the process, the instruction and the data
address of each load or store. The layer writes the title memory the GPU
uses to a file (`VICTORY_REGIONS_FILE`): buffers, windows, textures the
title fills. This classifies the game's sampled loads and stores by that
map and scales them by the sampling period:

    scripts/cpu-reads.py perf.data regions.txt PID PERIOD SECONDS FRAMES

Through ReBAR the CPU reads VRAM at ~0.08 GB/s, about 0.8 us a 64-byte
line, uncached (tools/rebar-bench): every load of GPU memory would pay it.
"""
import bisect
import collections
import subprocess
import sys


def main():
    data, regions_path, pid, period, seconds, frames = sys.argv[1:7]
    pid, period, seconds, frames = int(pid), int(period), float(seconds), float(frames)
    spans = []
    for line in open(regions_path):
        parts = line.split()
        if len(parts) == 3:
            spans.append((int(parts[0], 16), int(parts[1], 16), parts[2]))
    spans.sort()
    # Joined per kind into disjoint runs for lookup (kinds may overlap: the
    # first match by start wins, buffers before windows before textures).
    starts = [s[0] for s in spans]
    total_bytes = collections.Counter()
    for start, length, kind in spans:
        total_bytes[kind] += length

    def kind_of(addr):
        i = bisect.bisect_right(starts, addr) - 1
        # Look back over a few spans: big ones may start well before.
        for j in range(i, max(i - 64, -1), -1):
            start, length, kind = spans[j]
            if start <= addr < start + length:
                return kind
        return None

    out = subprocess.run(
        ["perf", "script", "-i", data, "-F", "pid,addr,data_src"], capture_output=True, text=True
    ).stdout
    counts = collections.Counter()
    pages = collections.Counter()
    for line in out.splitlines():
        parts = line.split(None, 3)
        if len(parts) < 4 or not parts[0].isdigit() or int(parts[0]) != pid:
            continue
        op = "load" if "OP LOAD" in parts[3] else "store" if "OP STORE" in parts[3] else None
        if op is None:
            continue
        try:
            addr = int(parts[1], 16)
        except ValueError:
            continue
        kind = kind_of(addr) if addr else None
        counts[(op, kind or "other")] += 1
        if kind:
            pages[(op, addr >> 12)] += 1
    print(f"GPU memory in the map: " + ", ".join(f"{k} {v / 1e6:.0f} MB" for k, v in total_bytes.items()))
    print(f"sampled loads and stores of pid {pid} (period {period}, {seconds:.0f} s, {frames:.0f} frames):")
    for op in ("load", "store"):
        all_ops = sum(n for (o, _), n in counts.items() if o == op)
        for kind in ("buffer", "window", "texture", "other"):
            n = counts[(op, kind)]
            if not n:
                continue
            per_frame = n * period / frames
            line = f"  {op:5} {kind:8} {n:7d} samples ({n * 100 / max(all_ops, 1):5.1f} % of {op}s): ~{per_frame:,.0f} a frame"
            if op == "load" and kind != "other":
                line += f", ~{per_frame * 0.8e-3:,.0f} ms a frame if each were an uncached VRAM read"
            print(line)
    hot = collections.Counter({p: n for (o, p), n in pages.items() if o == "load"})
    print(f"  pages of GPU memory with sampled loads: {len(hot)}; the hottest: "
          + ", ".join(f"{p << 12:#x} ({n})" for p, n in hot.most_common(8)))


if __name__ == "__main__":
    main()
