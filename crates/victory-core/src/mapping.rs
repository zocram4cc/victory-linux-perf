//! The title's memory map as a generation.
//!
//! What of the title's memory is committed or readable changes only when a
//! call that commits, decommits, frees, maps, unmaps or protects it returns.
//! Such calls run in [`changing`], which records the change once they have
//! returned. An answer about the map holds while [`answering`] gives the
//! generation it was kept under, and no answer is kept while a change is in
//! flight.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

/// Calls run in [`changing`] that have not returned.
static IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
/// Changes recorded so far.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Runs `change`, a call that changes the title's memory map, and records the
/// change when it has returned.
pub fn changing<T>(change: impl FnOnce() -> T) -> T {
    IN_FLIGHT.fetch_add(1, SeqCst);
    let out = change();
    changed();
    IN_FLIGHT.fetch_sub(1, SeqCst);
    out
}

/// Records a change made by a call that did not run in [`changing`].
pub fn changed() {
    GENERATION.fetch_add(1, SeqCst);
}

/// The generation so far.
pub fn generation() -> u64 {
    GENERATION.load(SeqCst)
}

/// The generation an answer may be kept under, or `None` while a change is in
/// flight. Ask this before the kernel, and keep the answer under it.
pub fn answering() -> Option<u64> {
    (IN_FLIGHT.load(SeqCst) == 0).then(generation)
}

/// Whether answers are kept: `VICTORY_MAPPED_CACHE=1`, read once; off by default.
pub fn cached() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("VICTORY_MAPPED_CACHE").is_some_and(|v| v == "1"));
    *ON
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_moves_the_generation_and_nothing_is_kept_during_it() {
        let before = generation();
        changing(|| assert_eq!(answering(), None));
        assert_eq!(generation(), before + 1);
        assert_eq!(answering(), Some(before + 1));
    }
}
