//! The states textures are in, as lists that ran left them: from the
//! barriers, the copies that promote a texture out of the common state, and
//! the decay at the end of a submission. Buffers are in the common state
//! between submissions, always, and are not kept here.

use std::collections::HashMap;

use super::wire::{ALL, Barrier, Cmd, Loc};

pub const COMMON: u32 = 0;
pub const RENDER_TARGET: u32 = 0x4;
pub const UNORDERED_ACCESS: u32 = 0x8;
pub const DEPTH_WRITE: u32 = 0x10;
pub const COPY_DEST: u32 = 0x400;
pub const COPY_SOURCE: u32 = 0x800;
pub const RESOLVE_DEST: u32 = 0x1000;

/// A change of state a list makes, to apply when it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub res: u32,
    pub sub: u32,
    pub after: u32,
    /// A copy into it: it is in the copy-destination state afterwards if it
    /// was in the common state, else as it was.
    pub promoted: bool,
    /// An aliasing barrier made it the one in use.
    pub alias: bool,
}

#[derive(Clone, Debug)]
struct One {
    subs: u32,
    /// Decays to the common state after each submission.
    decays: bool,
    all: u32,
    each: Vec<u32>,
}

/// The states of textures, by id.
#[derive(Clone, Debug, Default)]
pub struct States {
    map: HashMap<u32, One>,
    touched: Vec<u32>,
    /// When a placed resource was last the one in use (made, or named by an
    /// aliasing barrier), by id; higher is later.
    pub used: HashMap<u32, u64>,
    clock: u64,
}

impl States {
    /// A texture of `subs` subresources starts in `state`.
    pub fn add(&mut self, res: u32, subs: u32, state: u32, decays: bool) {
        self.map.insert(
            res,
            One {
                subs: subs.max(1),
                decays,
                all: state,
                each: Vec::new(),
            },
        );
    }

    /// A placed resource is the one in use now.
    pub fn in_use(&mut self, res: u32) {
        self.clock += 1;
        self.used.insert(res, self.clock);
    }

    pub fn remove(&mut self, res: u32) {
        self.map.remove(&res);
        self.used.remove(&res);
    }

    /// One state, or one for each subresource.
    pub fn get(&self, res: u32) -> Option<Vec<u32>> {
        let one = self.map.get(&res)?;
        Some(if one.each.is_empty() {
            vec![one.all]
        } else {
            one.each.clone()
        })
    }

    pub fn change(&mut self, c: &Change) {
        if c.alias {
            self.in_use(c.res);
            return;
        }
        let Some(one) = self.map.get_mut(&c.res) else {
            return;
        };
        let new = |old: u32| match (c.promoted, old) {
            (true, COMMON) => COPY_DEST,
            (true, old) => old,
            (false, _) => c.after,
        };
        if c.sub == ALL || one.subs == 1 {
            one.all = new(one.all);
            if c.promoted {
                one.each.iter_mut().for_each(|s| *s = new(*s));
            } else {
                one.each.clear();
            }
        } else if c.sub < one.subs {
            if one.each.is_empty() {
                one.each = vec![one.all; one.subs as usize];
            }
            let s = &mut one.each[c.sub as usize];
            *s = new(*s);
            if one.each.iter().all(|s| *s == one.each[0]) {
                one.all = one.each[0];
                one.each.clear();
            }
        }
        if one.decays {
            self.touched.push(c.res);
        }
    }

    /// The end of a submission (one `ExecuteCommandLists`).
    pub fn decay(&mut self) {
        for res in std::mem::take(&mut self.touched) {
            if let Some(one) = self.map.get_mut(&res) {
                one.all = COMMON;
                one.each.clear();
            }
        }
    }
}

/// The changes a command makes.
pub fn changes(cmd: &Cmd, out: &mut Vec<Change>) {
    let promoted = |res, sub| Change {
        res,
        sub,
        after: COPY_DEST,
        promoted: true,
        alias: false,
    };
    match cmd {
        Cmd::Barriers { all } => {
            for b in all {
                match *b {
                    // The first half of a split barrier changes nothing yet.
                    Barrier::Transition {
                        flags,
                        res,
                        sub,
                        after,
                        ..
                    } if flags & 1 == 0 => {
                        out.push(Change {
                            res,
                            sub,
                            after: after as u32,
                            promoted: false,
                            alias: false,
                        });
                    }
                    Barrier::Alias { after, .. } if after != 0 => {
                        out.push(Change {
                            res: after,
                            sub: ALL,
                            after: 0,
                            promoted: false,
                            alias: true,
                        });
                    }
                    _ => {}
                }
            }
        }
        Cmd::CopyTexture {
            dst: Loc::Sub { res, sub },
            ..
        } => out.push(promoted(*res, *sub)),
        Cmd::CopyResource { dst, .. } => out.push(promoted(*dst, ALL)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(res: u32, sub: u32, after: u32) -> Change {
        Change {
            res,
            sub,
            after,
            promoted: false,
            alias: false,
        }
    }

    #[test]
    fn states_are_followed() {
        let mut s = States::default();
        s.add(1, 4, COPY_DEST, false);
        s.add(2, 1, COMMON, false);
        s.add(3, 2, COMMON, true);
        assert_eq!(s.get(1), Some(vec![COPY_DEST]));
        s.change(&set(1, 2, 0x80));
        assert_eq!(s.get(1), Some(vec![COPY_DEST, COPY_DEST, 0x80, COPY_DEST]));
        for sub in [0, 1, 3] {
            s.change(&set(1, sub, 0x80));
        }
        assert_eq!(s.get(1), Some(vec![0x80]), "equal states are one again");
        // A copy into a texture in the common state leaves it a copy
        // destination; one into a texture in another state changes nothing.
        let copy = |res| Change {
            res,
            sub: ALL,
            after: COPY_DEST,
            promoted: true,
            alias: false,
        };
        s.change(&copy(2));
        s.change(&copy(1));
        s.decay();
        assert_eq!(s.get(2), Some(vec![COPY_DEST]));
        assert_eq!(s.get(1), Some(vec![0x80]));
        // Simultaneous access decays.
        s.change(&set(3, 1, UNORDERED_ACCESS));
        assert_eq!(s.get(3), Some(vec![COMMON, UNORDERED_ACCESS]));
        s.decay();
        assert_eq!(s.get(3), Some(vec![COMMON]));
        s.remove(3);
        assert_eq!(s.get(3), None);
    }

    #[test]
    fn commands_change_states() {
        let mut out = Vec::new();
        let t = |flags| Barrier::Transition {
            flags,
            res: 1,
            sub: 0,
            before: 4,
            after: 8,
        };
        changes(
            &Cmd::Barriers {
                all: vec![
                    t(1),
                    t(2),
                    Barrier::Alias {
                        flags: 0,
                        before: 3,
                        after: 4,
                    },
                    Barrier::Uav { flags: 0, res: 1 },
                ],
            },
            &mut out,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], set(1, 0, 8));
        assert!(out[1].alias && out[1].res == 4);
    }
}
