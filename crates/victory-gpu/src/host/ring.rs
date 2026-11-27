//! The landing ring (`crate::host::land_ring`, `VICTORY_LAND_RING=1`): the
//! compare after a flight's lists copies the pieces it finds differing into
//! PC memory of ours in the same submission, and the landing merges them
//! from there with no fetch. From olivertgwalton/victory-rx6600 fd407bf.

/// [`crate::host::CARD_COMPARE`] with the ring: the same hash and flags,
/// plus, told `compare & 4`, a piece that differs is copied whole (its
/// hashed bytes, `lo..hi` below) into `ring` at a place taken with an
/// atomic bump of `head`, and that place + 1 (0: no room) is written at the
/// piece's dword of `copied`. Without `& 4` it stores what the compare
/// without the ring stores, and nothing else.
pub(crate) const CARD_COMPARE_RING: &str = r#"
#define RS "RootConstants(num32BitConstants=7, b0), SRV(t0), UAV(u0), UAV(u1), UAV(u2), UAV(u3), UAV(u4)"
cbuffer Span : register(b0) {
    uint first;    // the first piece
    uint skew;     // where the copy starts in its piece 0
    uint size;     // the copy's buffer, in bytes (a multiple of 64 KiB)
    uint pieces;
    uint out_at;   // the first piece's dword in `differs`, in bytes
    uint compare;  // 0: the hashes are stored; else compared with those stored (2: and then stored);
                   // +4: a piece that differs is copied into `ring`
    uint ring_size; // the ring's bytes
};
ByteAddressBuffer now : register(t0);
// 16 bytes per piece of the copy: the hash of its bytes when they were
// last sent up (or hashed whole, the first time).
RWByteAddressBuffer hashes : register(u0);
RWByteAddressBuffer differs : register(u1);
// PC memory, mapped: the pieces that differ, one after another; where each
// went (+1, 0: not copied) at its `differs` dword; the bytes taken so far.
RWByteAddressBuffer ring : register(u2);
RWByteAddressBuffer copied : register(u3);
RWByteAddressBuffer head : register(u4);
groupshared uint4 parts[64];
groupshared uint copy_to;
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_GroupID, uint thread : SV_GroupIndex) {
    uint piece = first + id.x;
    uint lo = piece * 65536u;
    lo = lo > skew ? lo - skew : 0u;
    uint hi = min((piece + 1u) * 65536u - skew, size);
    lo = lo & ~15u;
    hi = (hi + 15u) & ~15u;
    uint4 h = uint4(0x9E3779B9u, 0x85EBCA6Bu, 0xC2B2AE35u, 0x27D4EB2Fu) + thread;
    uint at = lo + thread * 16u;
    [loop]
    for (; at + 7168u < hi; at += 8192u) {
        uint4 v0 = now.Load4(at + 0u);
        uint4 v1 = now.Load4(at + 1024u);
        uint4 v2 = now.Load4(at + 2048u);
        uint4 v3 = now.Load4(at + 3072u);
        uint4 v4 = now.Load4(at + 4096u);
        uint4 v5 = now.Load4(at + 5120u);
        uint4 v6 = now.Load4(at + 6144u);
        uint4 v7 = now.Load4(at + 7168u);
        h = (h ^ v0) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
        h = (h ^ v1) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
        h = (h ^ v2) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
        h = (h ^ v3) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
        h = (h ^ v4) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
        h = (h ^ v5) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
        h = (h ^ v6) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
        h = (h ^ v7) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
    }
    [loop]
    for (; at < hi; at += 1024u) {
        h = (h ^ now.Load4(at)) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
    }
    parts[thread] = h;
    GroupMemoryBarrierWithGroupSync();
    if (thread == 0u) {
        h = parts[0];
        [loop]
        for (uint i = 1u; i < 64u; ++i) {
            h = (h ^ parts[i]) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
            h = h.yzwx ^ (h >> 15);
        }
        uint to = 0u;
        if ((compare & 3u) != 0u) {
            // The flag, and above it 31 bits of the hash (`torn`).
            uint folded = h.x ^ (h.y * 3u) ^ (h.z * 5u) ^ (h.w * 7u);
            bool differ = any(h != hashes.Load4(piece * 16u));
            differs.Store(out_at + id.x * 4u, (folded << 1u) | (differ ? 1u : 0u));
            // With the copy-back on its own thread: the next compare of the
            // piece finds what is written from here on.
            if ((compare & 3u) == 2u) hashes.Store4(piece * 16u, h);
            if ((compare & 4u) != 0u) {
                if (differ) {
                    uint at0;
                    head.InterlockedAdd(0, hi - lo, at0);
                    to = at0 + (hi - lo) <= ring_size ? at0 + 1u : 0u;
                }
                copied.Store(out_at + id.x * 4u, to);
            }
        } else {
            hashes.Store4(piece * 16u, h);
        }
        copy_to = to;
    }
    GroupMemoryBarrierWithGroupSync();
    uint dst = copy_to;
    if (dst != 0u) {
        dst -= 1u;
        [loop]
        for (uint a = lo + thread * 16u; a < hi; a += 1024u) {
            ring.Store4(dst + (a - lo), now.Load4(a));
        }
    }
}
"#;

/// Pieces of one landing to merge from the ring, by (copy, piece): the
/// newest flight's copy of each wins, and the part merged grows to hold
/// every part any flight's ranges hold of it.
///
/// Why the union and not the newest flight's part alone: two flights may
/// declare different parts of one piece. The newest flight's ring copy
/// holds the whole hashed piece as it was at the end of that flight (the
/// earlier flight's writes included: the copies run in queue order), so
/// merging the union from it lands both. Taking only the newest part, as
/// the source fork did, would drop the earlier flight's writes outside it.
pub(crate) struct Ringed<T> {
    pieces: std::collections::HashMap<(usize, u32), RingedPiece<T>>,
}

impl<T> Default for Ringed<T> {
    fn default() -> Self {
        Ringed { pieces: std::collections::HashMap::new() }
    }
}

/// One piece from the ring ([`Ringed`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RingedPiece<T> {
    pub(crate) copy: T,
    /// The part to merge (offsets in the copy).
    pub(crate) lo: usize,
    pub(crate) hi: usize,
    /// The hashed piece's first offset, which `src` holds.
    pub(crate) base: usize,
    /// Where the hashed piece is in the ring (an address).
    pub(crate) src: usize,
    /// The send number of the flight it came from.
    pub(crate) sent_as: u64,
    /// The flight's place in the landing (later: newer).
    pub(crate) flight: usize,
}

impl<T> RingedPiece<T> {
    /// The part to merge as `Shadow::land_card` takes it: (lo, hi, the
    /// address of the byte at `lo`).
    pub(crate) fn addressed(&self) -> (usize, usize, usize) {
        (self.lo, self.hi, self.src + (self.lo - self.base))
    }
}

impl<T> Ringed<T> {
    /// A flight ringed `key`'s piece, of which its range holds `part`.
    pub(crate) fn add(&mut self, key: (usize, u32), part: RingedPiece<T>) {
        match self.pieces.get_mut(&key) {
            Some(e) => {
                let (lo, hi) = (e.lo.min(part.lo), e.hi.max(part.hi));
                if part.flight >= e.flight {
                    *e = part;
                }
                e.lo = lo;
                e.hi = hi;
            }
            None => {
                self.pieces.insert(key, part);
            }
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// The pieces, by copy (`order` of it), then by send number, then in
    /// order in the copy: runs of one copy and one send number follow one
    /// another.
    pub(crate) fn into_sorted(self, order: impl Fn(&T) -> usize) -> Vec<RingedPiece<T>> {
        let mut list: Vec<_> = self.pieces.into_values().collect();
        list.sort_by_key(|e| (order(&e.copy), e.sent_as, e.lo));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::{Ringed, RingedPiece};

    fn part(lo: usize, hi: usize, src: usize, sent_as: u64, flight: usize) -> RingedPiece<u8> {
        RingedPiece { copy: 1, lo, hi, base: 0, src, sent_as, flight }
    }

    /// Two flights ringed one piece through different parts: the newest
    /// copy is merged, over both parts, whichever came first.
    #[test]
    fn the_newest_copy_wins_over_every_part() {
        for newest_first in [false, true] {
            let mut ringed = Ringed::default();
            let (old, new) = (part(0, 100, 5000, 3, 0), part(200, 300, 9000, 4, 1));
            if newest_first {
                ringed.add((1, 7), new.clone());
                ringed.add((1, 7), old.clone());
            } else {
                ringed.add((1, 7), old.clone());
                ringed.add((1, 7), new.clone());
            }
            let list = ringed.into_sorted(|&c| usize::from(c));
            assert_eq!(list, [part(0, 300, 9000, 4, 1)], "newest first {newest_first}");
            assert_eq!(list[0].addressed(), (0, 300, 9000));
        }
    }

    /// One flight's two ranges of a piece: the part grows; other pieces
    /// stay apart, and come out by copy, send number and offset.
    #[test]
    fn parts_of_one_flight_join_and_pieces_stay_apart() {
        let mut ringed = Ringed::default();
        ringed.add((1, 7), part(64, 96, 1000, 3, 0));
        ringed.add((1, 7), RingedPiece { src: 2000, ..part(16, 32, 2000, 3, 0) });
        ringed.add((1, 8), RingedPiece { base: 65536, ..part(65536 + 10, 65536 + 20, 3000, 2, 0) });
        let list = ringed.into_sorted(|&c| usize::from(c));
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].lo, list[0].hi, list[0].sent_as), (65546, 65556, 2));
        assert_eq!(list[0].addressed(), (65546, 65556, 3010));
        assert_eq!((list[1].lo, list[1].hi, list[1].src), (16, 96, 2000));
        assert_eq!(list[1].addressed(), (16, 96, 2016));
    }
}
