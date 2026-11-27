#!/usr/bin/env python3
"""flicker.py <out-dir> [frames] : how much the picture changes between
captures of the game's window while nothing moves.

Grabs `frames` (default 14) captures of the window as fast as `import`
gives them, scaled to 480 px wide, and prints for every pair of neighbours
the mean absolute difference (0..255) and the share of pixels that differ by
more than 24. A still scene with the character idle gives a mean under ~1.5
and a share under ~2 %; surfaces that blink give several times that. The
captures stay in <out-dir> (f00.png ...) for looking at.
"""
import re
import subprocess
import sys
import time
from pathlib import Path


def window():
    clients = subprocess.run(['xprop', '-root', '_NET_CLIENT_LIST'], capture_output=True, text=True).stdout
    found = []
    for handle in re.findall('0x[0-9a-fA-F]+', clients):
        v = subprocess.run(['xprop', '-id', handle, '_NET_WM_NAME', 'WM_NAME'], capture_output=True, text=True).stdout
        if re.search(r'VIctory [0-9]+ fps [0-9]+(?:\.[0-9]+)? ms/frame', v, re.I):
            found.append(handle)
    if len(found) != 1:
        raise SystemExit(f'game windows found: {found}')
    return found[0]


def pixels(path):
    raw = subprocess.run(['magick', str(path), '-colorspace', 'Gray', '-depth', '8', 'gray:-'], capture_output=True, check=True).stdout
    return raw


def main():
    out = Path(sys.argv[1])
    frames = int(sys.argv[2]) if len(sys.argv) > 2 else 14
    out.mkdir(parents=True, exist_ok=True)
    handle = window()
    shots, stamps = [], []
    for n in range(frames):
        p = out / f'f{n:02d}.png'
        subprocess.run(['import', '-silent', '-window', handle, '-resize', '480x', str(p)], check=True, timeout=15)
        shots.append(p)
        stamps.append(time.time())
    data = [pixels(p) for p in shots]
    means, shares = [], []
    for a, b in zip(data, data[1:]):
        n = min(len(a), len(b))
        diff = [abs(x - y) for x, y in zip(a[:n], b[:n])]
        means.append(sum(diff) / n)
        shares.append(100.0 * sum(d > 24 for d in diff) / n)
    means_sorted, shares_sorted = sorted(means), sorted(shares)
    mid = len(means) // 2
    gap = (stamps[-1] - stamps[0]) / max(1, frames - 1)
    print(f'flicker {out.name}: {frames} captures {gap:.2f} s apart; mean difference median {means_sorted[mid]:.2f} '
          f'max {means_sorted[-1]:.2f}; pixels over 24: median {shares_sorted[mid]:.1f} % max {shares_sorted[-1]:.1f} %')
    print('  pairs: ' + ' '.join(f'{m:.1f}/{s:.0f}%' for m, s in zip(means, shares)))


if __name__ == '__main__':
    main()
