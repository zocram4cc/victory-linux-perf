#!/usr/bin/env python3
"""The blink number of a run's screenshots: how much the picture flickers.

    scripts/victory-blink.py <shots folder> [--from <s>] [--to <s>] [--window <n>]
                             [--step <n>] [--threshold <x>] [--out <dir>] [--frames]

The folder holds captures named by the milliseconds since the run started
(0318283.png is 318.283 s), as victory-leg.sh and the A/B shots folders have
them. Needs PIL and numpy.

What is measured
----------------
Every capture is shrunk to 240 pixels wide (each pixel the average of an 8x8
block, so dithering and noise average out). A pixel of capture B *blinks*
when, in any colour channel, it lies more than 24 levels outside the range of
values its 5x5 neighbourhood has in BOTH neighbouring captures A and C (each
shifted first by the whole-picture motion found between it and B), and A and C
themselves agree at that pixel (within 24 levels in every channel). Isolated
blinking pixels and slivers one pixel thin are removed (a 3x3 opening). The
same is done for a span of two captures (B and B' both outside the range of A
and of the capture after B'), so a fault that shows in two captures running
counts too. A capture's *blink share* is the largest share (in percent of the
picture) of blinking pixels over the spans it is part of.

A triple is *not judged* when A and C agree on less than 60 % of the picture:
a cut, a cross-fade's middle or a fast pan. Cuts and smooth motion therefore
do not count as blinking (the picture does not come back); a cross-fade does
not either (its middle lies between its ends).

The *blink number* of a window of n captures is 1 + the largest blink share
among its judged captures: 1.0 for a still or smoothly changing picture,
about 1.0-1.5 in calm scenes of the intro, 3 to 11 where stretched-triangle
shards come and go (A/B shots 2026-11-27, about 314 s and 320-322 s). A
window whose captures are all not judged prints "-".

What it cannot tell apart (look at the crops, --out): an object that crosses
a large part of the view between two captures and is gone in the next (a
plane past the camera in the montage scores 20+), and fast motion close to
the camera (a dancer filling the view scores 2-3). The number says where to
look; a person decides what it is. It is not the other fork's number (the
mean grey difference between neighbours), only named after it.

Captures that are all one colour (gamescope sometimes returns black) are
counted, reported and left out, as if they had not been taken.
"""

import argparse
import statistics
import sys
from pathlib import Path

try:
    import numpy as np
    from PIL import Image
except ImportError as e:  # pragma: no cover
    sys.exit(f"victory-blink.py needs numpy and PIL ({e})")

WIDTH = 240          # analysis width in pixels
LEVEL = 24           # levels (0..255) a pixel must lie outside its neighbours' range
REACH = 2            # neighbourhood half-size: 5x5 at 240 wide, about 40 px at 1920
JUDGED = 0.60        # share of the picture A and C must agree on
FLAT = 6             # a capture whose every channel spans fewer levels is "all one colour"


# ------------------------------------------------------------------ loading


def capture_time(path: Path) -> float | None:
    """Seconds since the run started, from a name like 0318283.png."""
    return int(path.stem) / 1000 if path.stem.isdigit() else None


def small(path: Path) -> np.ndarray:
    im = Image.open(path).convert("RGB")
    w, h = im.size
    factor = max(1, w // WIDTH)
    if factor > 1 and w % factor == 0 and h % factor == 0:
        im = im.reduce(factor)
    else:
        im = im.resize((WIDTH, max(1, round(h * WIDTH / w))), Image.BOX)
    return np.asarray(im, dtype=np.float32)


def is_flat(arr: np.ndarray) -> bool:
    """All one colour (black included): every channel spans fewer than FLAT levels."""
    span = arr.reshape(-1, arr.shape[-1]).max(0) - arr.reshape(-1, arr.shape[-1]).min(0)
    return bool((span < FLAT).all())


# ------------------------------------------------------------------ measure


def neighbourhood(a: np.ndarray, k: int, op) -> np.ndarray:
    """Min or max over a (2k+1)x(2k+1) square, separably, edges repeated."""
    h, w = a.shape[:2]
    pad = [(k, k), (0, 0)] + [(0, 0)] * (a.ndim - 2)
    p = np.pad(a, pad, mode="edge")
    rows = op(np.stack([p[d:d + h] for d in range(2 * k + 1)]), axis=0)
    pad = [(0, 0), (k, k)] + [(0, 0)] * (a.ndim - 2)
    p = np.pad(rows, pad, mode="edge")
    return op(np.stack([p[:, d:d + w] for d in range(2 * k + 1)]), axis=0)


def opened(mask: np.ndarray) -> np.ndarray:
    m = mask.astype(np.uint8)
    return neighbourhood(neighbourhood(m, 1, np.min), 1, np.max).astype(bool)


def motion(b: np.ndarray, a: np.ndarray) -> tuple[int, int]:
    """The whole-picture shift (rows, columns) that lays a over b (phase correlation);
    (0, 0) when the best shift is over an eighth of the picture (a cut, not motion)."""
    g1, g2 = b.mean(2), a.mean(2)
    f = np.fft.fft2(g1 - g1.mean()) * np.conj(np.fft.fft2(g2 - g2.mean()))
    f /= np.abs(f) + 1e-6
    r = np.fft.ifft2(f).real
    h, w = r.shape
    y, x = np.unravel_index(int(np.argmax(r)), r.shape)
    y = y - h if y > h // 2 else y
    x = x - w if x > w // 2 else x
    return (0, 0) if abs(y) > h // 8 or abs(x) > w // 8 else (int(y), int(x))


def shifted(a: np.ndarray, s: tuple[int, int]) -> np.ndarray:
    return np.roll(a, s, axis=(0, 1))


def wrapped(shape, *shifts) -> np.ndarray:
    """Where a roll by any of `shifts` brought in pixels from the other edge."""
    m = np.zeros(shape[:2], bool)
    for y, x in shifts:
        if y > 0: m[:y] = True
        if y < 0: m[y:] = True
        if x > 0: m[:, :x] = True
        if x < 0: m[:, x:] = True
    return m


class Shots:
    def __init__(self, pics: list[np.ndarray]):
        self.pics = pics
        self.lo = [neighbourhood(p, REACH, np.min) for p in pics]
        self.hi = [neighbourhood(p, REACH, np.max) for p in pics]
        self._motion: dict[tuple[int, int], tuple[int, int]] = {}

    def motion(self, b: int, a: int) -> tuple[int, int]:
        if (b, a) not in self._motion:
            self._motion[(b, a)] = motion(self.pics[b], self.pics[a])
        return self._motion[(b, a)]

    def span(self, before: int, mids: list[int], after: int):
        """(share %, mask) of the pixels of `mids` that blink against `before` and
        `after`, or None when those two do not agree on enough of the picture."""
        ref = mids[0]
        sa, sc = self.motion(ref, before), self.motion(ref, after)
        a, c = shifted(self.pics[before], sa), shifted(self.pics[after], sc)
        agree = (np.abs(a - c) < LEVEL).all(2) & ~wrapped(a.shape, sa, sc)
        if agree.mean() < JUDGED:
            return None
        lo = np.minimum(shifted(self.lo[before], sa), shifted(self.lo[after], sc)) - LEVEL
        hi = np.maximum(shifted(self.hi[before], sa), shifted(self.hi[after], sc)) + LEVEL
        out = agree.copy()
        for m in mids:
            p = self.pics[m]
            out &= ((p < lo) | (p > hi)).any(2)
        out = opened(out)
        return 100.0 * out.mean(), out

    def shares(self):
        """Per capture: (blink share %, mask) or None when no span with it was judged."""
        n = len(self.pics)
        found = []
        for i in range(n):
            spans = []
            if 0 < i < n - 1:
                spans.append((i - 1, [i], i + 1))
            if 0 < i and i + 2 < n:
                spans.append((i - 1, [i, i + 1], i + 2))
            if 1 < i < n - 1:
                spans.append((i - 2, [i - 1, i], i + 1))
            got = [r for s in spans if (r := self.span(*s)) is not None]
            found.append(max(got, key=lambda r: r[0]) if got else None)
        return found


# ------------------------------------------------------------------ report


def pct(values: list[float], p: float) -> float:
    s = sorted(values)
    return s[min(len(s) - 1, int(p / 100 * len(s)))] if s else float("nan")


def windows(times: list[float], shares: list, n: int, step: int):
    """(first time, last time, blink number or None, judged, captures) per window."""
    out = []
    last_start = max(0, len(times) - n)
    starts = list(range(0, last_start + 1, step))
    if starts and starts[-1] != last_start:
        starts.append(last_start)
    for s in starts:
        part = shares[s:s + n]
        judged = [r[0] for r in part if r is not None]
        out.append((times[s], times[min(s + n, len(times)) - 1], 1 + max(judged) if judged else None,
                    len(judged), len(part)))
    return out


def crop(paths: list[Path], mask: np.ndarray, dest: Path) -> None:
    """The blinking area of a capture between its two neighbours, side by side."""
    ys, xs = np.nonzero(mask)
    with Image.open(paths[1]) as im:
        w, h = im.size
    scale = w / mask.shape[1]
    pad = 24
    box = (max(0, int(xs.min() * scale) - pad), max(0, int(ys.min() * scale) - pad),
           min(w, int((xs.max() + 1) * scale) + pad), min(h, int((ys.max() + 1) * scale) + pad))
    tiles = [Image.open(p).convert("RGB").crop(box) for p in paths]
    fit = min(1.0, 480 / max(1, box[3] - box[1]), 640 / max(1, box[2] - box[0]))
    tiles = [t.resize((max(1, int(t.width * fit)), max(1, int(t.height * fit)))) for t in tiles]
    sheet = Image.new("RGB", (sum(t.width for t in tiles) + 8 * (len(tiles) - 1), tiles[0].height), (255, 0, 255))
    x = 0
    for t in tiles:
        sheet.paste(t, (x, 0))
        x += t.width + 8
    sheet.save(dest)


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("folder", type=Path)
    p.add_argument("--from", dest="start", type=float, default=None, help="seconds since the run started")
    p.add_argument("--to", dest="end", type=float, default=None)
    p.add_argument("--window", type=int, default=12, help="captures a window (default 12)")
    p.add_argument("--step", type=int, default=None, help="captures between windows (default window/2)")
    p.add_argument("--threshold", type=float, default=3.0, help="windows over this are counted (default 3)")
    p.add_argument("--out", type=Path, help="crops of the worst captures go here")
    p.add_argument("--crops", type=int, default=6, help="how many crops (default 6)")
    p.add_argument("--frames", action="store_true", help="also every capture's blink share")
    p.add_argument("--summary-only", action="store_true", help="only the summary lines")
    a = p.parse_args(argv)
    if a.window < 3:
        p.error("--window must be 3 or more")
    step = a.step or max(1, a.window // 2)

    named = sorted((t, f) for f in a.folder.glob("*.png") if (t := capture_time(f)) is not None)
    named = [(t, f) for t, f in named
             if (a.start is None or t >= a.start) and (a.end is None or t <= a.end)]
    flat, times, paths, pics = [], [], [], []
    for t, f in named:
        try:
            arr = small(f)
        except OSError as e:
            print(f"unreadable: {f.name} ({e})", file=sys.stderr)
            continue
        if is_flat(arr):
            flat.append(f.name)
            continue
        times.append(t)
        paths.append(f)
        pics.append(arr)
    print(f"{a.folder}: {len(named)} captures"
          + (f" from {named[0][0]:.1f} to {named[-1][0]:.1f} s" if named else "")
          + (f", {len(flat)} all one colour (left out: {', '.join(flat[:5])}{' ...' if len(flat) > 5 else ''})"
             if flat else ", none all one colour"))
    if len(pics) < 3:
        print("too few captures to measure")
        return 1

    shares = Shots(pics).shares()
    if a.frames and not a.summary_only:
        for t, r in zip(times, shares):
            print(f"  capture {t:9.3f} s  " + ("not judged" if r is None else f"{r[0]:5.2f} %"))
    rows = windows(times, shares, a.window, step)
    if not a.summary_only:
        print(f"window of {a.window} captures, every {step}: time (s), blink number, captures judged")
        for t0, t1, b, judged, total in rows:
            mark = "  <" if b is not None and b > a.threshold else ""
            print(f"  {t0:8.1f}-{t1:6.1f}  {'-' if b is None else f'{b:5.1f}'}  {judged}/{total}{mark}")
    values = [b for _, _, b, _, _ in rows if b is not None]
    unjudged = sum(1 for r in shares if r is None)
    over = [(t0, t1) for t0, t1, b, _, _ in rows if b is not None and b > a.threshold]
    if values:
        print(f"blink number: median {statistics.median(values):.2f}, 90th percentile {pct(values, 90):.2f}, "
              f"{len(over)} of {len(values)} windows over {a.threshold:g}"
              + (f" ({', '.join(f'{t0:.0f}-{t1:.0f} s' for t0, t1 in over[:8])}{' ...' if len(over) > 8 else ''})"
                 if over else "")
              + f"; {unjudged} of {len(shares)} captures not judged (cuts, fast change)")
    else:
        print(f"blink number: no window could be judged ({unjudged} captures not judged)")
    worst = sorted(((r[0], i) for i, r in enumerate(shares) if r is not None and r[0] > 0), reverse=True)
    if worst:
        print("worst captures: " + ", ".join(f"{times[i]:.3f} s {s:.1f} %" for s, i in worst[:6]))
    if a.out and worst:
        a.out.mkdir(parents=True, exist_ok=True)
        for s, i in worst[:a.crops]:
            mask = shares[i][1]
            if not mask.any():
                continue
            near = [paths[j] for j in (i - 1, i, i + 1) if 0 <= j < len(paths)]
            crop(near if len(near) == 3 else [paths[i]] * 3, mask,
                 a.out / f"{paths[i].stem}-blink{s:.1f}.png")
        print(f"crops (before | capture | after): {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
