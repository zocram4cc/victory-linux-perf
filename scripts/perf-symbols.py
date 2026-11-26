#!/usr/bin/env python3
"""Symbolise the layer's own code in a `bench.sh --perf` profile.

perf sees Wine's PE code as "[JIT]" with raw addresses. victory.exe is
relocated to a different base on every run, so this finds the base first:
the 64 KiB-aligned base that puts the most samples inside victory.exe's
.text, and whose top addresses resolve to the layer's own functions. It then
sums samples per function (llvm-addr2line reads the PDB next to the exe).

    scripts/perf-symbols.py bench/<run>/perf.data [target/.../victory.exe] [-n 40]

The title's own code shows up as "[JIT]" too; it is counted apart (samples
outside victory.exe), not symbolised.
"""
import argparse
import collections
import os
import struct
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def text_range(exe):
    f = open(exe, "rb").read()
    pe = struct.unpack_from("<I", f, 0x3C)[0]
    sections = struct.unpack_from("<H", f, pe + 6)[0]
    optional = struct.unpack_from("<H", f, pe + 20)[0]
    at = pe + 24 + optional
    for i in range(sections):
        name = f[at + 40 * i : at + 40 * i + 8].rstrip(b"\0")
        size, va = struct.unpack_from("<II", f, at + 40 * i + 8)
        if name == b".text":
            return va, va + size
    sys.exit("no .text section in " + exe)


def symbolise(exe, addresses):
    out = subprocess.run(
        ["llvm-addr2line", "-f", "-C", "--obj=" + exe],
        input="\n".join(hex(a) for a in addresses),
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split("\n")
    return out[0::2][: len(addresses)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("perf_data")
    ap.add_argument("exe", nargs="?", default=os.path.join(ROOT, "target/x86_64-pc-windows-msvc/release/victory.exe"))
    ap.add_argument("-n", type=int, default=40, help="functions to list")
    ap.add_argument(
        "--callers",
        metavar="PATTERN",
        help="for samples whose function contains PATTERN (e.g. Mutex), list the first victory_ function up the stack "
        "(needs a profile with BENCH_PERF_G=lbr, or BENCH_PERF_G=1 and a frame-pointer build)",
    )
    a = ap.parse_args()

    script = subprocess.run(["perf", "script", "-i", a.perf_data, "-F", "ip,dso"], capture_output=True, text=True).stdout
    total = 0
    jit = collections.Counter()
    for line in script.splitlines():
        parts = line.split()
        if len(parts) < 2:
            continue
        total += 1
        if "perf-" in parts[1] and parts[1].endswith(".map)"):
            jit[int(parts[0], 16)] += 1
    if not jit:
        sys.exit("no [JIT] samples")
    tstart, tend = text_range(a.exe)
    span = tend - tstart
    # The window of .text's length holding the most samples: victory.exe.
    ips = sorted(jit)
    best, j = (0, 0), 0
    for i, ip in enumerate(ips):
        while ips[j] < ip - span:
            j += 1
        n = sum(jit[x] for x in ips[j : i + 1])
        if n > best[0]:
            best = (n, i)
    hi = ips[best[1]]
    lo = min(x for x in ips if hi - span <= x <= hi)
    candidates = [b for b in range((hi - tend) // 0x10000 * 0x10000, (lo - tstart) // 0x10000 * 0x10000 + 0x10000, 0x10000) if b + tstart <= lo and hi < b + tend]
    if not candidates:
        sys.exit("no base fits")
    top = [ip for ip, _ in jit.most_common() if lo <= ip <= hi][:150]
    own = ("victory_", "blake3", "hashbrown", "std::", "core::", "alloc::")

    def score(base):
        names = symbolise(a.exe, [0x140000000 + ip - base for ip in top])
        return sum(1 for n in names if n.startswith(own))

    base = max(candidates, key=score)
    if a.callers:
        callers(a, base, tstart, tend)
        return
    ours = {ip: n for ip, n in jit.items() if base + tstart <= ip < base + tend}
    in_exe = sum(ours.values())
    title = sum(jit.values()) - in_exe
    print(f"samples {total}; victory.exe {in_exe} ({in_exe * 100 / total:.1f} %), other PE code (the title) {title} ({title * 100 / total:.1f} %); base {base:#x} of {len(candidates)} candidates")
    ranked = sorted(ours, key=ours.get, reverse=True)[:8000]
    names = symbolise(a.exe, [0x140000000 + ip - base for ip in ranked])
    by = collections.Counter()
    for ip, name in zip(ranked, names):
        by[name] += ours[ip]
    for name, n in by.most_common(a.n):
        print(f"{n:7d} {n * 100 / total:5.1f}%  {name[:130]}")


def callers(a, base, tstart, tend):
    """Attributes samples whose top function matches a.callers to the first
    victory_ function up their stack."""
    stacks = []
    branches = []
    lbr = subprocess.run(["perf", "report", "-i", a.perf_data, "--header-only"], capture_output=True, text=True).stdout
    if "BRANCH_STACK" in lbr:
        # One line per sample: the ip, then from/to/... pairs, newest first.
        # The "stack" is the ip followed by the call sites (the froms).
        out = subprocess.run(["perf", "script", "-i", a.perf_data, "-F", "ip,brstack"], capture_output=True, text=True).stdout
        for line in out.splitlines():
            parts = line.split()
            if not parts:
                continue
            try:
                stack = [int(parts[0], 16)]
            except ValueError:
                continue
            pairs = []
            for entry in parts[1:]:
                fields = entry.split("/")
                if len(fields) >= 2:
                    try:
                        pairs.append((int(fields[0], 16), int(fields[1], 16)))
                    except ValueError:
                        pass
            branches.append((stack[0], pairs))
            stacks.append(stack + [f for f, _ in pairs])
    else:
        out = subprocess.run(["perf", "script", "-i", a.perf_data, "-F", "ip"], capture_output=True, text=True).stdout
        stack = []
        for line in out.splitlines():
            line = line.strip()
            if not line:
                if stack:
                    stacks.append(stack)
                stack = []
                continue
            try:
                stack.append(int(line.split()[0], 16))
            except ValueError:
                pass
        if stack:
            stacks.append(stack)
    inside = lambda ip: base + tstart <= ip < base + tend
    wanted = {ip for s in stacks for ip in s if inside(ip)} | {t for _, p in branches for _, t in p if inside(t)}
    names = dict(zip(sorted(wanted), symbolise(a.exe, [0x140000000 + ip - base for ip in sorted(wanted)])))
    if branches:
        # Exact call sites: the newest branch into a function matching the
        # pattern, made from one that does not match (the caller).
        matched, by = 0, collections.Counter()
        for ip, pairs in branches:
            if not inside(ip) or a.callers not in names.get(ip, ""):
                continue
            matched += 1
            site = next(
                (names.get(f, "?") for f, t in pairs if inside(t) and a.callers in names.get(t, "") and a.callers not in names.get(f, a.callers)),
                "(no call into it in the last 16 branches)",
            )
            by[site] += 1
        print(f"{matched} samples in functions matching {a.callers!r}; the call site that entered them:")
        for name, n in by.most_common(a.n):
            print(f"{n:7d} {n * 100 / max(matched, 1):5.1f}%  {name[:130]}")
        return
    matched, by = 0, collections.Counter()
    for s in stacks:
        if not s or not inside(s[0]) or a.callers not in names.get(s[0], ""):
            continue
        matched += 1
        caller = next((names[ip] for ip in s[1:] if inside(ip) and names.get(ip, "").startswith("victory_")), "(no victory_ frame)")
        by[caller] += 1
    print(f"{matched} samples in functions matching {a.callers!r} (of {len(stacks)} stacks); first victory_ caller:")
    for name, n in by.most_common(a.n):
        print(f"{n:7d} {n * 100 / max(matched, 1):5.1f}%  {name[:130]}")


if __name__ == "__main__":
    main()
