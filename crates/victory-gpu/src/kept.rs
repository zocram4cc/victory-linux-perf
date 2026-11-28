// Author: libs (hle-d3d12-amd)
//! Room that only a burst needed, given back (`VICTORY_SHRINK_KEPT=1`, off by default).
//!
//! A table that is cleared and a buffer that is used again keep the room of the most they
//! ever held. Each host list's tables of texture states and of twins are cleared at every
//! reset and nowhere else, so every host list keeps, for the whole run, the room of its
//! biggest recording; each thread's buffer for reading a texture's memory keeps the bytes of
//! the biggest texture it ever read. With the switch:
//!
//! - at a host list's reset, a table whose room is more than four times what it held and
//!   more than 1024 entries is made anew at the size it held ([`table_size`]); the others are
//!   only cleared, as before, so a list that records about the same each time makes no table
//!   anew and moves nothing;
//! - a read buffer grows to the size a use asks and no more, and after a use that left it
//!   over 8 MiB it is given back ([`given_back`]): the next use makes one anew.
//!
//! What it costs: a list whose recordings swing by more than four times grows its table
//! again after a small one, and a thread that reads a texture over 8 MiB makes and fills its
//! buffer anew for each such read, where it reused it before.
//!
//! What was given back is counted for the memory housekeeping line ([`text`]). Diagnostics
//! builds can turn it on and off while the title runs (`VICTORY_AB=<s>,VICTORY_SHRINK_KEPT`).

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// A cleared table is made anew only with room for more entries than this.
const TABLE_ROOM: usize = 1024;

/// ...and only with more than this many times the room of what it held.
const TABLE_TIMES: usize = 4;

/// A read buffer with more room than this (8 MiB) is given back after its use.
const BUFFER_ROOM: usize = 8 << 20;

/// Since the start: tables made anew smaller and the bytes of room they gave back (about:
/// entries of room times an entry's size; the table's control bytes are not counted), read
/// buffers given back and their bytes. A table or buffer that grows again and is given back
/// again counts again.
static GIVEN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// `VICTORY_SHRINK_KEPT=1` (or the in-run A/B's choice in a diagnostics build).
fn on() -> bool {
    #[cfg(feature = "diagnostics")]
    {
        static BIT: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
        if let Some(bit) = *BIT.get_or_init(|| crate::ab::bit("VICTORY_SHRINK_KEPT")) {
            return crate::ab::on(bit);
        }
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_SHRINK_KEPT").is_ok_and(|v| v.trim() == "1"))
}

/// The size a cleared table with room for `room` entries, which held `held` before the
/// clear, is made anew at: `held`, when its room is more than [`TABLE_ROOM`] entries and
/// more than [`TABLE_TIMES`] times what it held (most of it was a burst's). None: it keeps
/// its room.
fn table_size(room: usize, held: usize) -> Option<usize> {
    (room > TABLE_ROOM && room > held.saturating_mul(TABLE_TIMES)).then_some(held)
}

/// Whether a read buffer with room for `room` bytes is given back after its use.
fn given_back(room: usize) -> bool {
    room > BUFFER_ROOM
}

/// Clears a host list's table at its reset. With the switch, one whose room only a burst
/// needed is made anew at the size it held ([`table_size`]). A list is reset twice for each
/// use, and the second reset finds its tables empty: an empty table says nothing of what the
/// list needs, so it is only cleared (else every table would be made anew at each recording).
pub(crate) fn clear_table<K: Eq + Hash, V, S: BuildHasher>(table: &mut HashMap<K, V, S>) {
    let held = table.len();
    table.clear();
    if on() && held > 0 {
        shrink_cleared(table, held);
    }
}

/// Makes the cleared `table`, which held `held` entries, anew at that size if [`table_size`]
/// says so, and counts it. Returns the bytes of room given back (about).
fn shrink_cleared<K: Eq + Hash, V, S: BuildHasher>(table: &mut HashMap<K, V, S>, held: usize) -> Option<u64> {
    let room = table.capacity();
    let size = table_size(room, held)?;
    // Empty, the table is only allocated anew at the smaller size: nothing is moved.
    table.shrink_to(size);
    let bytes = (room.saturating_sub(table.capacity()) * size_of::<(K, V)>()) as u64;
    GIVEN[0].fetch_add(1, Relaxed);
    GIVEN[1].fetch_add(bytes, Relaxed);
    Some(bytes)
}

/// Makes this thread's read buffer `size` bytes long for a use, if it is shorter (what it
/// holds is whatever the last use left).
pub(crate) fn grow(buffer: &mut Vec<u8>, size: usize) {
    grow_to(buffer, size, on());
}

/// `exact` (the switch): to the size asked and no more. Grown by doubling, a buffer could be
/// over [`BUFFER_ROOM`] after a use under it, and be given back and made anew again and again.
fn grow_to(buffer: &mut Vec<u8>, size: usize, exact: bool) {
    if buffer.len() < size {
        if exact {
            buffer.reserve_exact(size - buffer.len());
        }
        buffer.resize(size, 0);
    }
}

/// After a use of this thread's read buffer: with the switch, one with more room than
/// [`BUFFER_ROOM`] is given back ([`given_back`]).
pub(crate) fn after_use(buffer: &mut Vec<u8>) {
    if on() {
        give_back(buffer);
    }
}

/// Gives `buffer` back if [`given_back`] says so, and counts it. Returns its bytes.
fn give_back(buffer: &mut Vec<u8>) -> Option<u64> {
    let room = buffer.capacity();
    if !given_back(room) {
        return None;
    }
    *buffer = Vec::new();
    GIVEN[2].fetch_add(1, Relaxed);
    GIVEN[3].fetch_add(room as u64, Relaxed);
    Some(room as u64)
}

/// For the memory housekeeping line: what was given back since the start.
pub(crate) fn text() -> String {
    let given: [u64; 4] = std::array::from_fn(|i| GIVEN[i].load(Relaxed));
    let mb = |bytes: u64| bytes as f64 / f64::from(1u32 << 20);
    format!(
        "room only a burst needed, given back (VICTORY_SHRINK_KEPT={}): host list tables made smaller {} \
         (about {:.1} MB), read buffers {} ({:.1} MB)",
        u8::from(on()),
        given[0],
        mb(given[1]),
        given[2],
        mb(given[3])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_big_table_that_held_a_quarter_or_less_is_made_anew() {
        // Small tables keep their room, whatever they held.
        assert_eq!(table_size(1024, 0), None);
        assert_eq!(table_size(896, 3), None);
        // A big one keeps it while what it held needs more than a quarter of it.
        assert_eq!(table_size(1792, 448), None);
        assert_eq!(table_size(1792, 1000), None);
        assert_eq!(table_size(1792, 447), Some(447));
        assert_eq!(table_size(7168, 0), Some(0));
        // No overflow for what no table holds.
        assert_eq!(table_size(usize::MAX, usize::MAX / 2), None);
    }

    #[test]
    fn a_burst_table_is_made_anew_at_what_it_held_and_a_steady_one_kept() {
        let entry = size_of::<(usize, u64)>() as u64;
        let mut table: HashMap<usize, u64> = HashMap::new();
        table.extend((0..5000).map(|i| (i, 0)));
        let room = table.capacity();
        // As big as what it held: only cleared, its room kept.
        table.clear();
        assert_eq!(shrink_cleared(&mut table, 5000), None);
        assert_eq!(table.capacity(), room);
        // Then a small recording: made anew for what it held.
        table.extend((0..10).map(|i| (i, 0)));
        assert_eq!(table.capacity(), room);
        table.clear();
        let bytes = shrink_cleared(&mut table, 10).expect("made anew");
        assert!(table.is_empty());
        assert!((10..64).contains(&table.capacity()), "{}", table.capacity());
        assert_eq!(bytes, (room - table.capacity()) as u64 * entry);
        // It works as before: entries go in and are found.
        table.extend((0..100).map(|i| (i, i as u64 * 3)));
        assert_eq!(table.get(&99), Some(&297));
        // A burst followed by nothing gives back the whole room.
        table.extend((0..5000).map(|i| (i, 0)));
        let room = table.capacity();
        table.clear();
        assert_eq!(shrink_cleared(&mut table, 0), Some(room as u64 * entry));
        assert_eq!(table.capacity(), 0);
    }

    #[test]
    fn clear_table_clears_with_the_switch_off() {
        let mut table: HashMap<usize, u64> = (0..3000).map(|i| (i, 0)).collect();
        let room = table.capacity();
        clear_table(&mut table);
        assert!(table.is_empty());
        if !on() {
            assert_eq!(table.capacity(), room);
        }
    }

    #[test]
    fn a_read_buffer_over_8_mib_is_given_back_after_its_use() {
        assert!(!given_back(0));
        assert!(!given_back(8 << 20));
        assert!(given_back((8 << 20) + 1));
        let mut small = vec![7u8; 8 << 20];
        assert_eq!(give_back(&mut small), None);
        assert_eq!(small.len(), 8 << 20);
        let mut big = vec![7u8; 9 << 20];
        let room = big.capacity() as u64;
        assert_eq!(give_back(&mut big), Some(room));
        assert_eq!(big.capacity(), 0);
    }

    #[test]
    fn grown_to_the_size_asked_a_buffer_under_8_mib_stays() {
        // Grown to the size asked (by doubling, the 7 MiB use after the 5 MiB
        // one would leave more than 8 MiB of room), no use of 8 MiB or less
        // has its buffer given back...
        let mut buffer = Vec::new();
        for size in [1 << 20, 5 << 20, 7 << 20, 8 << 20, 2 << 20] {
            grow_to(&mut buffer, size, true);
            assert!(buffer.len() >= size);
            assert_eq!(give_back(&mut buffer), None, "{size}");
        }
        // ...and one over it has, the next use making one anew.
        grow_to(&mut buffer, 12 << 20, true);
        assert_eq!(buffer.len(), 12 << 20);
        assert!(give_back(&mut buffer).is_some());
        grow_to(&mut buffer, 3 << 20, true);
        assert_eq!(buffer.len(), 3 << 20);
        assert!(!given_back(buffer.capacity()));
    }

    #[test]
    fn the_line_names_the_switch() {
        assert!(text().contains("(VICTORY_SHRINK_KEPT="));
    }
}
