#!/usr/bin/env python3
"""Prints the thread-trace bytes (SQTT data chunks) of RGP captures, largest first.

Usage: scripts/rgp-size.py FILE.rgp...  The trace grows with the GPU work a
capture holds; the files' sizes do not (code objects dominate them).
"""
import struct, sys

def sqtt_bytes(path):
    with open(path, 'rb') as f:
        data = f.read()
    if len(data) < 32:
        return 0
    off = struct.unpack_from('<i', data, 16)[0]
    total = 0
    while 0 < off and off + 16 <= len(data):
        ident, _minor, _major, size = struct.unpack_from('<IHHi', data, off)
        if size <= 0:
            break
        if ident & 0xff == 2:  # SQTT_DATA
            total += size - 16
        off += size
    return total

rows = sorted(((sqtt_bytes(p), p) for p in sys.argv[1:]), reverse=True)
for n, p in rows:
    print(f'{n:>12} {p}')
