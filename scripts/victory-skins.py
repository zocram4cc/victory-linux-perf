#!/usr/bin/env python3
"""The skinned vertices a captured frame's character draws read, counted for
spikes: vertices thrown out of their mesh (the stretched "sheets").

    scripts/victory-skins.py <survey log> <dump folder> [--start <dump folder>] [--all]

The survey log is what `victory replay <capture> --root-reach 0..2000` prints:
its "skin" lines name, for each draw of a pre-skinned character mesh program
(MS_VS_PedTransformPreSkinD*), the buffer and place of the draw's record and
the two buffers of skinned positions. The dump folder holds those three
buffers as the card has them before the first such draw:

    victory replay <capture> --images <folder> --texture <record buffer>
        --texture <positions-0> --texture <positions-1> --raw --until <list>:<command before the first draw>

(The draws of one frame come after the skinning dispatches and the copies
before them, and do not write these buffers, so one dump serves them all.)

Per draw: the range word (dword 0x54 of the record), the range it names
(bits 24-25: 0 positions-0, else positions-1; the low 24 bits x 16 its
offset; a 32-byte header {count, 0xdeadbeef}; positions float3 with bit 29,
10:10:10 with bit 28, else 24:24:16, the packed ones within +-3 of the
character), and the vertices that are both
  far:  a coordinate more than 1.0 from the range's median, and
  lone: more than 0.5 (in some coordinate) from both of its neighbours in
        the range (the vertex before and the one after);
and, apart, those whose record differs from a neighbour's in one byte only
(the signature of the title's bytes landing over a skinned vertex).

With --start, a second dump of the position buffers taken at the frame's
start (`--until 0:0`) says for each draw whether its range was written in
the frame before the draw ("written this frame") or holds the bytes it held
when the frame began ("as at the start": skinned in an earlier frame, or
skinned again to the very same bytes). Lists on queues other than the drawing
one are not in a capture: a range written there shows as at the start.
"""
import os
import re
import struct
import sys

FAR, LONE, JUMP = 1.0, 0.5, 0.5
SKIN = re.compile(r'skin (\d+:\d+) (.*?): record (\d+)\+(0x[0-9a-f]+) positions-0 (\d+)\+\S+ positions-1 (\d+)\+')


def load(folder, res):
    path = os.path.join(folder, '%05d.bin' % res)
    return open(path, 'rb').read() if os.path.exists(path) else None


def vertices(buf, base, n, kind):
    """(record bytes, position or None for a vertex hidden on purpose)."""
    stride = {'float3': 12, '10:10:10': 4, '24:24:16': 8}[kind]
    out = []
    for i in range(n):
        raw = buf[base + 32 + stride * i:base + 32 + stride * (i + 1)]
        if kind == 'float3':
            p = struct.unpack('<3f', raw)
            hidden = raw == b'\xff' * 12
        elif kind == '10:10:10':
            d = struct.unpack('<I', raw)[0]
            p = tuple(-3 + 6 * ((d >> s) & 1023) / 1023 for s in (0, 10, 20))
            hidden = d == 0xffffffff
        else:
            d0, d1 = struct.unpack('<2I', raw)
            p = (-3 + (d0 >> 8) * 6 / 2**24,
                 -3 + ((d0 & 0xff) | (d1 & 0xffff) << 8) * 6 / 2**24,
                 -3 + (d1 >> 16) * 6 / 65535)
            hidden = d0 & d1 == 0xffffffff
        out.append((raw, None if hidden else p))
    return out


def apart(a, b):
    return max(abs(a[k] - b[k]) for k in range(3))


def draw(folder, m, start=None):
    rec = load(folder, int(m.group(3)))
    if rec is None:
        return 'no dump of the record buffer %s' % m.group(3)
    word = struct.unpack_from('<I', rec, int(m.group(4), 16) + 0x54)[0]
    res = int(m.group(5)) if word & 0x03000000 == 0 else int(m.group(6))
    buf = load(folder, res)
    if buf is None:
        return 'no dump of %d' % res
    base = (word & 0xffffff) * 16
    n, magic = struct.unpack_from('<2I', buf, base)
    if magic != 0xdeadbeef or not 0 < n < 70000:
        return 'word %08x: %d+%#x holds no skinned range (%08x %08x)' % (word, res, base, n, magic)
    kind = 'float3' if word & 0x20000000 else '10:10:10' if word & 0x10000000 else '24:24:16'
    span = 32 + {'float3': 12, '10:10:10': 4, '24:24:16': 8}[kind] * n
    fresh = None
    if start:
        old = load(start, res)
        fresh = None if old is None else old[base:base + span] != buf[base:base + span]
    vs = vertices(buf, base, n, kind)
    pos = [p for _, p in vs]
    shown = [p for p in pos if p]
    if not shown:
        return (word, res, base, kind, n, n, [], [], fresh)
    med = [sorted(p[k] for p in shown)[len(shown) // 2] for k in range(3)]
    bad, byte = [], []
    for i, (raw, p) in enumerate(vs):
        if not p:
            continue
        near = [j for j in (i - 1, i + 1) if 0 <= j < n and pos[j]]
        if apart(p, med) > FAR and near and all(apart(p, pos[j]) > LONE for j in near):
            bad.append(i)
        if any(sum(x != y for x, y in zip(raw, vs[j][0])) == 1 and apart(p, pos[j]) > JUMP for j in near):
            byte.append(i)
    return (word, res, base, kind, n, n - len(shown), bad, byte, fresh)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    log, folder, every = sys.argv[1], sys.argv[2], '--all' in sys.argv
    start = sys.argv[sys.argv.index('--start') + 1] if '--start' in sys.argv else None
    draws, broken, spikes, failed = 0, 0, 0, 0
    stale, stale_bad = [], 0
    for line in open(log):
        m = SKIN.match(line)
        if not m:
            continue
        draws += 1
        r = draw(folder, m, start)
        if isinstance(r, str):
            failed += 1
            print(m.group(1), m.group(2), r)
            continue
        word, res, base, kind, n, hidden, bad, byte, fresh = r
        broken += bool(bad)
        spikes += len(bad)
        if fresh is False:
            stale.append(m.group(1))
            stale_bad += bool(bad)
        when = '' if fresh is None else ', written this frame' if fresh else ', AS AT THE START'
        if bad or every or fresh is False:
            print('%s %s word %08x %d+%#x %s: %d vertices, %d hidden, %d spikes, %d one-byte%s; first spikes %s' % (
                m.group(1), m.group(2), word, res, base, kind, n, hidden, len(bad), len(byte), when, bad[:8]))
    print('%d draws, %d not decoded, %d with spikes, %d spike vertices' % (draws, failed, broken, spikes))
    if start:
        print("%d draws of ranges as at the frame's start (%d of them with spikes)" % (len(stale), stale_bad))


if __name__ == '__main__':
    main()
