//! Pieces of card copies that came down while a later flight was writing
//! them (`VICTORY_FETCH_CHECK`, with `VICTORY_RELAY_COPY`).
//!
//! On the copy queue a fetch runs beside later submissions (`relay_copy`),
//! so what comes down of a piece may be what it held in the middle of one:
//! a counter cleared and not counted up again, a list half filled. Such
//! bytes went into title memory and the copy's CPU side, to be put right by
//! that submission's own landing, which fetches what its compare flagged.
//! But the compare flags a piece only if it is not, at the submission's end,
//! what it was at the end of the one before. A piece cleared and filled
//! again with the same is not flagged, is not fetched, and the CPU's side
//! keeps the bytes from in between: until they go up with a refresh of
//! their 64 KiB, over the right ones.
//!
//! So the compare gives each piece's hash with its flag, and what came down
//! is hashed here too. The same hash: it is what the flight ended with. Not
//! the same: the piece is suspect (another flight was at it, or the title's
//! bytes went up to it meanwhile), and the next flight that may write it
//! fetches it whatever its compare says, until a fetch is found whole.
//!
//! What it costs: pieces are fetched whole where a flight declared a part
//! of one, and 64 KiB of what came down are hashed (some 4 us) just before
//! they are merged, on the thread that merges them (`crate::land`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU8;

use super::*;

/// `VICTORY_FETCH_CHECK=1`; off by default until a run has shown how often a
/// piece is caught (the frame-rate line says so) and what the hashes cost
/// (3-4 ms for 96 MB beside the merge, spread over the threads that land).
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_FETCH_CHECK").is_some_and(|v| v == "1"))
}

/// A piece: its copy's number, and the piece.
pub(super) type Piece = (u64, u32);

/// Since the last frame-rate line: pieces fetched and hashed, those found
/// suspect, and those fetched because an earlier fetch was.
pub(super) static STATS: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

/// The pieces whose last fetch was not what a flight ended with.
#[derive(Default)]
pub(super) struct Suspects(HashSet<Piece>);

impl Suspects {
    /// Whether a flight that may write `piece` fetches it, its compare
    /// having flagged it or not.
    pub(super) fn wanted(&self, piece: Piece, flagged: bool) -> bool {
        flagged || (!self.0.is_empty() && self.0.contains(&piece))
    }

    /// `piece` was fetched; `same`: it was what the last of the flights
    /// landed ended with.
    pub(super) fn fetched(&mut self, piece: Piece, same: bool) {
        STATS[0].fetch_add(1, Ordering::Relaxed);
        if same {
            if self.0.remove(&piece) {
                STATS[2].fetch_add(1, Ordering::Relaxed);
            }
        } else {
            self.0.insert(piece);
            STATS[1].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// What is landed: its copy, its offsets from and to, and where it comes
/// down.
pub(super) type Landed = (std::sync::Arc<crate::shadow::Shadow>, usize, usize, u64);

/// The pieces of a landing's fetch, whole, and what is known of them.
pub(super) struct Check {
    /// What is copied down: whole pieces, as the compare hashes them.
    pub(super) copies: Vec<Landed>,
    /// The bytes of them.
    pub(super) total: u64,
    /// Of each: the piece, and the hash the last flight that may have
    /// written it ended with.
    of: Vec<(Piece, Option<u32>)>,
    /// Of what is landed, the piece (in `copies`) it is of.
    within: Vec<usize>,
    /// Of each piece: 0 not looked at yet, 1 as that flight ended with, 2
    /// not so.
    found: Vec<AtomicU8>,
}

impl Check {
    /// For `landed`, parts of pieces that `plans` may have written, whose
    /// compares' results are in `differs`: the parts are given their places
    /// in the pieces that come down.
    pub(super) fn new<'a>(plans: impl Iterator<Item = &'a CardPlan>, differs: &[u32], landed: &mut [Landed]) -> Check {
        let mut index: HashMap<Piece, usize> = HashMap::new();
        let (mut copies, mut of, mut within, mut total) = (Vec::new(), Vec::new(), Vec::new(), 0u64);
        for (shadow, lo, _, at) in landed {
            let number = shadow.card_piece_of(*lo);
            let piece = (shadow.id(), number);
            let i = *index.entry(piece).or_insert_with(|| {
                let (from, to) = shadow.card_hashed(number);
                copies.push((shadow.clone(), from, to, total));
                of.push((piece, None));
                total += (to - from) as u64;
                copies.len() - 1
            });
            *at = copies[i].3 + (*lo - copies[i].1) as u64;
            within.push(i);
        }
        // (The last flight's stands.)
        for r in plans.flat_map(|plan| &plan.ranges).filter(|r| r.compared && !r.picked) {
            for p in 0..r.span.pieces {
                if let Some(&i) = index.get(&(r.shadow.id(), r.span.first + p))
                    && let Some(&d) = differs.get((r.at + p) as usize)
                {
                    of[i].1 = Some(folded(d));
                }
            }
        }
        let found = of.iter().map(|_| AtomicU8::new(0)).collect();
        Check { copies, total, of, within, found }
    }

    /// Looks at the piece that part `i` of what is landed is of, if none of
    /// its parts was looked at: on the thread that lands it, just before, so
    /// that its bytes are near.
    ///
    /// # Safety
    /// What was fetched is at `data`, and stays as it is during the call.
    pub(super) unsafe fn look(&self, i: usize, data: usize) {
        let i = self.within[i];
        if self.found[i].load(Ordering::Relaxed) != 0 {
            return;
        }
        let Some(hash) = self.of[i].1 else {
            return;
        };
        let (_, from, to, at) = &self.copies[i];
        // SAFETY: the caller's.
        let bytes = unsafe { std::slice::from_raw_parts((data + *at as usize) as *const u8, to - from) };
        self.found[i].store(if self::hash(bytes) == hash { 1 } else { 2 }, Ordering::Relaxed);
    }

    /// All is landed: what was found is kept for later flights.
    pub(super) fn done(&self, suspects: &mut Suspects) {
        for ((piece, _), found) in self.of.iter().zip(&self.found) {
            match found.load(Ordering::Relaxed) {
                // Not compared on the card: fetched by every flight.
                0 => {}
                found => suspects.fetched(*piece, found == 1),
            }
        }
    }
}

/// Of a result of the compare: whether the piece differs.
pub(super) fn flagged(result: u32) -> bool {
    result & 1 != 0
}

/// Of a result of the compare: the piece's hash, 31 bits of it.
pub(super) fn folded(result: u32) -> u32 {
    result >> 1
}

/// The hash the compare on the card ([`CARD_COMPARE`]) makes of a piece
/// that holds `bytes` (a multiple of 16 of them), as [`folded`].
pub(super) fn hash(bytes: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        // SAFETY: the feature is there.
        return unsafe { hash_avx2(bytes) };
    }
    hash_plain(bytes)
}

const MUL: [u32; 4] = [0x0100_0193, 0x9E37_79B1, 0x85EB_CA77, 0xC2B2_AE3D];
const SEED: [u32; 4] = [0x9E37_79B9, 0x85EB_CA6B, 0xC2B2_AE35, 0x27D4_EB2F];

fn step(h: &mut [u32; 4], with: [u32; 4]) {
    let x: [u32; 4] = std::array::from_fn(|i| (h[i] ^ with[i]).wrapping_mul(MUL[i]));
    *h = std::array::from_fn(|i| x[(i + 1) % 4] ^ (x[i] >> 15));
}

/// The 64 threads' hashes as one, folded to 31 bits.
fn finish(parts: &[[u32; 4]; 64]) -> u32 {
    let mut h = parts[0];
    for part in &parts[1..] {
        step(&mut h, *part);
    }
    let f = h[0] ^ h[1].wrapping_mul(3) ^ h[2].wrapping_mul(5) ^ h[3].wrapping_mul(7);
    f & 0x7FFF_FFFF
}

fn seeds() -> [[u32; 4]; 64] {
    std::array::from_fn(|thread| SEED.map(|s| s.wrapping_add(thread as u32)))
}

fn tail(parts: &mut [[u32; 4]; 64], bytes: &[u8]) {
    for (thread, chunk) in bytes.chunks_exact(16).enumerate() {
        let with = std::array::from_fn(|i| u32::from_le_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]));
        step(&mut parts[thread], with);
    }
}

fn hash_plain(bytes: &[u8]) -> u32 {
    let mut parts = seeds();
    for row in bytes.chunks(1024) {
        tail(&mut parts, row);
    }
    finish(&parts)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn hash_avx2(bytes: &[u8]) -> u32 {
    use std::arch::x86_64::*;
    let mut parts = seeds();
    let rows = bytes.chunks_exact(1024);
    let rest = rows.remainder();
    let mul = _mm256_setr_epi32(
        MUL[0] as i32, MUL[1] as i32, MUL[2] as i32, MUL[3] as i32, MUL[0] as i32, MUL[1] as i32, MUL[2] as i32, MUL[3] as i32,
    );
    let state = parts.as_mut_ptr().cast::<__m256i>();
    for row in rows {
        let from = row.as_ptr().cast::<__m256i>();
        for k in 0..32 {
            // SAFETY: two threads' hashes of 16 bytes each, and their 32 of
            // the row's 1,024 bytes.
            unsafe {
                let h = _mm256_xor_si256(_mm256_loadu_si256(state.add(k)), _mm256_loadu_si256(from.add(k)));
                let x = _mm256_mullo_epi32(h, mul);
                // y, z, w, x of each.
                let turned = _mm256_shuffle_epi32::<0b00_11_10_01>(x);
                _mm256_storeu_si256(state.add(k), _mm256_xor_si256(turned, _mm256_srli_epi32::<15>(x)));
            }
        }
    }
    tail(&mut parts, rest);
    finish(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wide_hash_is_the_plain_one() {
        let bytes: Vec<u8> = (0..65_536u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for n in [0usize, 16, 32, 1008, 1024, 1040, 5000 & !15, 65_536] {
            assert_eq!(hash(&bytes[..n]), hash_plain(&bytes[..n]), "{n} bytes");
        }
        let mut other = bytes.clone();
        other[40_000] ^= 1;
        assert_ne!(hash(&bytes), hash(&other));
    }

    /// A model of one piece. The card runs flights in order; each writes the
    /// piece several times and its compare flags it if it ends otherwise
    /// than the flight before. Each landing fetches what is wanted, on a
    /// queue beside the drawing one: the fetch sees the piece as it is while
    /// a later flight is at some step. Returns what the CPU's side holds in
    /// the end, and the fetches.
    fn landed(flights: &[&[u32]], seen: &[(usize, usize)], check: bool) -> (u32, u32) {
        let piece: Piece = (1, 0);
        let mut suspects = Suspects::default();
        let (mut held, mut ended, mut fetches) = (0, 0, 0);
        for (f, steps) in flights.iter().enumerate() {
            let before = ended;
            ended = *steps.last().expect("a step");
            let flagged = ended != before;
            if if check { !suspects.wanted(piece, flagged) } else { !flagged } {
                continue;
            }
            fetches += 1;
            // While flight `at` is after its step `step`.
            let (at, step) = seen[f];
            assert!(at >= f);
            held = flights[at][step];
            suspects.fetched(piece, held == ended);
        }
        (held, fetches)
    }

    /// A counter that each flight clears and counts up to the same: the
    /// first flight's fetch sees it cleared by the second, whose compare
    /// flags nothing.
    #[test]
    fn a_piece_caught_in_between_is_fetched_again() {
        let count: &[u32] = &[0, 3, 5];
        // The first is fetched while the second has cleared it.
        assert_eq!(landed(&[count, count], &[(1, 0), (1, 2)], false), (0, 1), "the CPU's side keeps what was in between");
        assert_eq!(landed(&[count, count], &[(1, 0), (1, 2)], true), (5, 2));
        // And again, while the third was at it; the fourth finds it whole.
        let four = [count; 4];
        assert_eq!(landed(&four, &[(1, 0), (2, 1), (3, 0), (3, 2)], true), (5, 4));
        // Fetched whole: no more fetches than without the check.
        assert_eq!(landed(&four, &[(0, 2), (1, 2), (2, 2), (3, 2)], true), (5, 1));
        assert_eq!(landed(&four, &[(0, 2), (1, 2), (2, 2), (3, 2)], false), (5, 1));
    }

    /// What the hash costs (ignored: a measurement): pieces that just came
    /// down, hashed alone, compared with what the CPU's side holds (as the
    /// merge begins), and both.
    #[test]
    #[ignore]
    fn hash_costs() {
        let (bytes, held) = (vec![0x5Au8; 96 << 20], vec![0x5Au8; 96 << 20]);
        let pieces = || bytes.chunks(64 << 10).zip(held.chunks(64 << 10));
        let compare = |a: &[u8], b: &[u8]| a.chunks(4096).zip(b.chunks(4096)).filter(|(a, b)| a != b).count() as u32;
        for _ in 0..5 {
            let mut all = 0u32;
            let mut time = |f: &dyn Fn(&[u8], &[u8]) -> u32| {
                let t = crate::clock::Instant::now();
                for (a, b) in pieces() {
                    all ^= f(a, b);
                }
                t.elapsed().as_secs_f64() * 1e3
            };
            let (hashed, compared, both) = (
                time(&|a, _| hash(a)),
                time(&compare),
                time(&|a, b| hash(a) ^ compare(a, b)),
            );
            println!("96 MB: hashed {hashed:.2} ms, compared {compared:.2} ms, both {both:.2} ms ({all:#x})");
        }
    }
}
