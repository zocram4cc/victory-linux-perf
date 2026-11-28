//! What the indirect executions of a list ran, for the report of a removed
//! device: with `VICTORY_D3D12_DRED=light` the lists carry no markers, and
//! the two device losses of 2026-11-25 (sessions play-1125-0203 and n02)
//! said only that a list of 814 (954) commands stopped at an indirect
//! execution. The pipeline of each was found hours later by finding the
//! same run of commands in a capture.
//!
//! Per list, in the order recorded: the pipeline (by its host object, whose
//! name is kept once) and the first root constants (user data) it had. The
//! report finds an indirect execution by how many the breadcrumbs' history
//! has before it. Nothing is kept without device-removed data.
//! `VICTORY_D3D12_DRED=faults` and no data at all: nothing is noted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The kind of a direct mesh dispatch.
pub(crate) const DIRECT: u32 = u32::MAX;

/// Words of user data kept of each execution.
pub(crate) const WORDS: usize = 16;

/// One indirect execution.
#[derive(Clone, Copy)]
pub(crate) struct Ran {
    pipeline: usize,
    kind: u32,
    count: u32,
    words: [u32; WORDS],
    used: u8,
}

#[derive(Default)]
pub(crate) struct Indirects {
    names: HashMap<usize, String>,
    ran: Vec<Ran>,
    /// The direct mesh dispatches, likewise.
    meshes: Vec<Ran>,
}

/// A list's, shared with the report's table of lists.
pub(crate) type Shared = Arc<Mutex<Indirects>>;

impl Indirects {
    /// An indirect execution of `kind` (the argument's type) of at most
    /// `count` records with `pipeline` (a host object) and user data.
    /// A direct mesh dispatch of `count` groups is of kind [`DIRECT`].
    pub(crate) fn note(&mut self, pipeline: usize, name: &str, kind: u32, count: u32, user: &[u32]) {
        self.names.entry(pipeline).or_insert_with(|| name.to_owned());
        let used = user.len().min(WORDS);
        let mut words = [0; WORDS];
        words[..used].copy_from_slice(&user[..used]);
        let ran = Ran { pipeline, kind, count, words, used: used as u8 };
        if kind == DIRECT {
            self.meshes.push(ran);
        } else {
            self.ran.push(ran);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.ran.clear();
        self.meshes.clear();
        if self.names.len() > 512 {
            self.names.clear();
        }
    }

    /// The `nth` execution (from 0), or direct mesh dispatch: its text and
    /// its user data.
    pub(crate) fn nth(&self, nth: usize, direct: bool) -> Option<(String, &[u32])> {
        let ran = if direct { &self.meshes } else { &self.ran }.get(nth)?;
        let name = self.names.get(&ran.pipeline).map_or("a pipeline", String::as_str);
        let words = &ran.words[..usize::from(ran.used)];
        Some((
            if direct {
                format!("{name} (a mesh dispatch of {} groups, the list's {nth}, user data {words:x?})", ran.count)
            } else {
                format!("{name} (indirect kind {} max {}, the list's indirect execution {nth}, user data {words:x?})", ran.kind, ran.count)
            },
            words,
        ))
    }
}

/// How many indirect executions come before command `at` of a history of
/// breadcrumb operations (`is` tells them): which of a list's it is.
pub(crate) fn before<T>(history: &[T], at: usize, is: impl Fn(&T) -> bool) -> usize {
    history[..at.min(history.len())].iter().filter(|op| is(op)).count()
}

/// The heap slots the user data may name, for the report to describe: the
/// words that are slots as they are, and the slots from where a word is a
/// table's byte offset (32 bytes a descriptor), `table` of them.
pub(crate) fn slots(words: &[u32], count: u32, table: u32) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    for (i, &word) in words.iter().enumerate() {
        if word > 0 && word < count && word % 32 != 0 {
            out.push((i, word));
        }
        if word % 32 == 0 && word / 32 > 16 && word / 32 < count {
            out.extend((0..table).map(|k| (i, word / 32 + k)).filter(|s| s.1 < count));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The execution a breadcrumb history stops at is found by the ones
    /// before it, and its pipeline and user data are told.
    #[test]
    fn an_indirect_execution_is_found_by_its_place_in_the_history() {
        let mut all = Indirects::default();
        all.note(1, "[compute a]", 2, 1, &[0x2549f40, 0x20, 0x11]);
        all.note(2, "[compute b]", 2, 1, &[7; 20]);
        all.note(1, "another name of the same is not taken", 10, 4, &[]);
        // dispatch, indirect, barrier, indirect, indirect
        let history = [6, 5, 15, 5, 5];
        let nth = before(&history, 3, |op| *op == 5);
        assert_eq!(nth, 1);
        let (text, words) = all.nth(nth, false).unwrap();
        assert!(text.starts_with("[compute b] (indirect kind 2 max 1"), "{text}");
        assert_eq!(words, [7; WORDS]);
        all.note(2, "[mesh b]", DIRECT, 9, &[1]);
        let (text, _) = all.nth(0, true).unwrap();
        assert!(text.starts_with("[compute b] (a mesh dispatch of 9 groups"), "{text}");
        assert!(all.nth(1, true).is_none());
        let (text, words) = all.nth(2, false).unwrap();
        assert!(text.starts_with("[compute a] (indirect kind 10 max 4"), "{text}");
        assert!(words.is_empty());
        assert!(all.nth(3, false).is_none());
        assert_eq!(before(&history, 99, |op| *op == 5), 3);
        all.clear();
        assert!(all.nth(0, false).is_none() && all.nth(0, true).is_none());
    }

    /// A table's byte offset names the slots from it on; a small word that
    /// is no multiple of a descriptor's bytes names its own slot.
    #[test]
    fn user_data_names_slots() {
        let words = [0, 0x2549f40, 0x20, 0x11, 0x26f1, 0xffff_ffff];
        let slots = slots(&words, 1_245_184, 2);
        assert_eq!(slots, [(1, 0x2549f40 / 32), (1, 0x2549f40 / 32 + 1), (3, 0x11), (4, 0x26f1)]);
    }
}
