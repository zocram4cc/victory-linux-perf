#!/usr/bin/env python3
"""route-merge.py <recorded.keys> <out.route> [gap]: a recording of
VICTORY_KEYS_RECORD made playable. A held key comes out of the recorder as
many short presses (the X server's key repeat releases and presses it):
presses of one key at most `gap` seconds apart (0.45) become one hold."""
import sys

gap = float(sys.argv[3]) if len(sys.argv) > 3 else 0.45
by_key = {}
for line in open(sys.argv[1]):
    line = line.strip()
    if ':' not in line:
        continue
    span, key = line.rsplit(':', 1)
    a, b = (float(x) for x in span.split('-'))
    by_key.setdefault(key, []).append([a, b])
holds = []
for key, spans in by_key.items():
    spans.sort()
    cur = spans[0]
    for a, b in spans[1:]:
        if a - cur[1] <= gap:
            cur[1] = max(cur[1], b)
        else:
            holds.append((cur[0], cur[1], key)); cur = [a, b]
    holds.append((cur[0], cur[1], key))
holds.sort()
with open(sys.argv[2], 'w') as out:
    for a, b, key in holds:
        out.write(f'{a:.2f}-{max(b, a + 0.2):.2f}:{key}\n')
print(f'{sum(len(s) for s in by_key.values())} presses -> {len(holds)} holds, {holds[0][0]:.0f} s to {holds[-1][1]:.0f} s')
