//! Experiments on a list's commands: the same list with something left out
//! or put otherwise.

use std::collections::{HashMap, HashSet};

use super::wire::{ALL, Barrier, Cmd, NONE, Va, slot};

/// The buffer the clamped arguments of all clamp passes lie in, in lists
/// edited by [`clamps`].
pub const RESULTS: u32 = u32::MAX - 1;
/// The buffer texels are read back through (`play::pixel`).
pub const PROBE: u32 = u32::MAX - 2;
/// A buffer of the card's that depth goes through on the way there.
pub const SCRATCH: u32 = u32::MAX - 3;

const INDIRECT_ARGUMENT: i32 = 0x200;
const UNORDERED_ACCESS: i32 = 0x8;
const COPY_SOURCE: i32 = 0x800;

/// Drops what runs a pipeline of `psos`; how many commands that were.
pub fn skip_pipelines(cmds: Vec<Cmd>, psos: &HashSet<u32>) -> (Vec<Cmd>, u32) {
    let mut bound = NONE;
    let mut dropped = 0;
    let out = cmds
        .into_iter()
        .filter(|c| {
            if let Cmd::Pipeline { pso } = c {
                bound = *pso;
            }
            let drop = c.runs_shaders() && psos.contains(&bound);
            dropped += u32::from(drop);
            !drop
        })
        .collect();
    (out, dropped)
}

/// Drops unordered-access barriers; how many.
pub fn no_uav_barriers(cmds: Vec<Cmd>) -> (Vec<Cmd>, u32) {
    let mut dropped = 0;
    let out = cmds
        .into_iter()
        .filter_map(|c| match c {
            Cmd::Barriers { mut all } => {
                let n = all.len();
                all.retain(|b| !matches!(b, Barrier::Uav { .. }));
                dropped += (n - all.len()) as u32;
                (!all.is_empty()).then_some(Cmd::Barriers { all })
            }
            c => Some(c),
        })
        .collect();
    (out, dropped)
}

/// Barriers only: what a list left out still does to the states of
/// resources.
pub fn barriers_only(cmds: Vec<Cmd>) -> Vec<Cmd> {
    cmds.into_iter()
        .filter(|c| matches!(c, Cmd::Barriers { .. }))
        .collect()
}

/// Joins the barrier calls with no work between them into one; how many
/// calls that saved.
pub fn merge_barriers(cmds: Vec<Cmd>) -> (Vec<Cmd>, u32) {
    let mut out: Vec<Cmd> = Vec::with_capacity(cmds.len());
    let mut open: Option<usize> = None;
    let mut saved = 0;
    for c in cmds {
        match c {
            Cmd::Barriers { all } => {
                if let Some(i) = open
                    && let Cmd::Barriers { all: have } = &mut out[i]
                {
                    for b in all {
                        join(have, b);
                    }
                    saved += 1;
                } else {
                    open = Some(out.len());
                    let mut have = Vec::new();
                    for b in all {
                        join(&mut have, b);
                    }
                    out.push(Cmd::Barriers { all: have });
                }
            }
            c => {
                if c.works() || matches!(c, Cmd::BeginQuery { .. } | Cmd::EndQuery { .. }) {
                    open = None;
                }
                out.push(c);
            }
        }
    }
    out.retain(|c| !matches!(c, Cmd::Barriers { all } if all.is_empty()));
    (out, saved)
}

/// Adds a barrier to a call's.
fn join(have: &mut Vec<Barrier>, b: Barrier) {
    match b {
        Barrier::Uav { res, .. } => {
            let covered = have
                .iter()
                .any(|h| matches!(h, Barrier::Uav { res: r, .. } if *r == res || *r == NONE));
            if covered {
                return;
            }
            if res == NONE {
                have.retain(|h| !matches!(h, Barrier::Uav { .. }));
            }
            have.push(b);
        }
        Barrier::Transition {
            flags: 0,
            res,
            sub,
            before,
            after,
        } => {
            // The last one of this resource, if it is of the same part of it.
            let last = have
                .iter()
                .rposition(|h| matches!(h, Barrier::Transition { res: r, .. } if *r == res));
            if let Some(i) = last
                && let Barrier::Transition {
                    flags: 0,
                    sub: s,
                    before: first,
                    after: until,
                    ..
                } = have[i]
                && s == sub
                && until == before
            {
                if first != after {
                    have[i] = Barrier::Transition {
                        flags: 0,
                        res,
                        sub,
                        before: first,
                        after,
                    };
                } else {
                    have.remove(i);
                    if after == UNORDERED_ACCESS {
                        // Still a wait for the writes before.
                        join(have, Barrier::Uav { flags: 0, res });
                    }
                }
                return;
            }
            have.push(b);
        }
        b => have.push(b),
    }
}

/// A clamp pass of ours found in a list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clamp {
    /// The command that dispatches it.
    pub at: usize,
    /// The scratch buffer it writes the arguments to, from its start.
    pub scratch: u32,
    pub bytes: u64,
    /// Where its arguments are in [`RESULTS`].
    pub result: u64,
}

/// The clamp passes of a list (`psos`: the clamp's pipelines), given places
/// in the results from `next` on.
pub fn find_clamps(cmds: &[Cmd], psos: &HashSet<u32>, next: &mut u64) -> Vec<Clamp> {
    let (mut bound, mut constants, mut to) = (NONE, [0u32; 6], None);
    let mut found = Vec::new();
    for (at, c) in cmds.iter().enumerate() {
        match c {
            Cmd::Pipeline { pso } => bound = *pso,
            Cmd::Constants {
                graphics: false,
                param: 0,
                offset,
                values,
            } => {
                for (i, v) in values.iter().enumerate() {
                    if let Some(c) = constants.get_mut(*offset as usize + i) {
                        *c = *v;
                    }
                }
            }
            // The clamp's parameters: constants (kind, stride, records,
            // ...), arguments, count, then where it writes.
            Cmd::RootView {
                graphics: false,
                param: 3,
                va: Va::At { res, offset: 0 },
                ..
            } => to = Some(*res),
            Cmd::Dispatch { .. } if psos.contains(&bound) => {
                if let Some(scratch) = to {
                    let bytes = u64::from(constants[1]) * u64::from(constants[2]);
                    found.push(Clamp {
                        at,
                        scratch,
                        bytes,
                        result: *next,
                    });
                    *next += bytes.next_multiple_of(256);
                }
            }
            _ => {}
        }
    }
    found
}

/// The list with a copy of what each clamp wrote into the results.
pub fn keep_clamped(cmds: Vec<Cmd>, clamps: &[Clamp]) -> anyhow::Result<Vec<Cmd>> {
    let mut out = Vec::with_capacity(cmds.len() + 3 * clamps.len());
    let mut open = None;
    let mut copied = 0;
    for (at, mut c) in cmds.into_iter().enumerate() {
        if let Some(clamp) = clamps.iter().find(|c| c.at == at) {
            open = Some(*clamp);
        }
        let (Some(clamp), Cmd::Barriers { all }) = (open, &mut c) else {
            out.push(c);
            continue;
        };
        // BEGIN_ONLY leaves the buffer inaccessible until END_ONLY.
        let Some(i) = all.iter().position(|b| {
            matches!(b,
                Barrier::Transition { flags, res, after: INDIRECT_ARGUMENT, .. }
                    if flags & 1 == 0 && *res == clamp.scratch
            )
        }) else {
            out.push(c);
            continue;
        };
        // Later barriers in the same call can change its state again.
        let rest = all.split_off(i + 1);
        out.push(c);
        let moved = |before, after| Cmd::Barriers {
            all: vec![Barrier::Transition {
                flags: 0,
                res: clamp.scratch,
                sub: ALL,
                before,
                after,
            }],
        };
        out.push(moved(INDIRECT_ARGUMENT, COPY_SOURCE));
        out.push(Cmd::CopyBuffer {
            dst: RESULTS,
            dst_at: clamp.result,
            src: clamp.scratch,
            src_at: 0,
            len: clamp.bytes,
        });
        out.push(moved(COPY_SOURCE, INDIRECT_ARGUMENT));
        if !rest.is_empty() {
            out.push(Cmd::Barriers { all: rest });
        }
        open = None;
        copied += 1;
    }
    anyhow::ensure!(
        copied == clamps.len(),
        "clamp capture has no completed scratch transition in its command list"
    );
    Ok(out)
}

/// The list without its clamp passes and the barriers of their scratch
/// buffers; what they bounded is read from the results instead.
pub fn skip_clamps(cmds: Vec<Cmd>, clamps: &[Clamp]) -> Vec<Cmd> {
    let scratches: HashSet<u32> = clamps.iter().map(|c| c.scratch).collect();
    let by_at: HashMap<usize, &Clamp> = clamps.iter().map(|c| (c.at, c)).collect();
    let mut last: HashMap<u32, u64> = HashMap::new();
    let mut out = Vec::with_capacity(cmds.len());
    for (at, c) in cmds.into_iter().enumerate() {
        if let Some(clamp) = by_at.get(&at) {
            last.insert(clamp.scratch, clamp.result);
            continue;
        }
        match c {
            Cmd::Barriers { mut all } => {
                let of_scratch = |b: &Barrier| matches!(b, Barrier::Transition { res, .. } if scratches.contains(res));
                if all.iter().any(of_scratch) {
                    // The waits made with the scratch buffer's moves are the
                    // clamp's too.
                    all.retain(|b| !of_scratch(b) && !matches!(b, Barrier::Uav { res: NONE, .. }));
                }
                if !all.is_empty() {
                    out.push(Cmd::Barriers { all });
                }
            }
            Cmd::Indirect {
                signature,
                most,
                args,
                at,
                count,
                count_at,
            } if last.contains_key(&args) => {
                let result = last.get(&args).copied().unwrap_or(0);
                out.push(Cmd::Indirect {
                    signature,
                    most,
                    args: RESULTS,
                    at: result + at,
                    count,
                    count_at,
                });
            }
            c => out.push(c),
        }
    }
    out
}

/// The root signature, pipeline and table of the batched clamp
/// (`crate::indirect::batch`), and the scratch buffer it writes in the
/// first list of the frame (the next lists' are the ids before it), in lists
/// edited by [`batch_clamps`].
pub const BATCH_ROOT: u32 = u32::MAX - 2;
pub const BATCH: u32 = u32::MAX - 3;
pub const BATCH_JOBS: u32 = u32::MAX - 4;
pub const BATCH_SCRATCH: u32 = u32::MAX - 16;

/// What [`batch_clamps`] did to a list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Batched {
    /// Clamp passes before; after; the records the latter clamp.
    pub clamps: u32,
    pub batches: u32,
    pub jobs: u32,
    /// The bytes of the scratch buffer.
    pub scratch: u64,
}

/// A clamp batch being filled by [`batch_clamps`].
struct Batch {
    /// Where its table is in the jobs, and its root views in the list.
    table: usize,
    at: usize,
    sources: Vec<u32>,
    /// By source, offset and kind: the job, and how often it is read.
    known: HashMap<(u32, u64, u32), (u32, u32)>,
    n: u32,
    base: u64,
}

/// The list with its clamps done in batches, as `crate::draw` does with
/// `VICTORY_INDIRECT_BATCH`: one pass at the first execution after a
/// point where arguments may have changed (a wait for shader writes, with
/// dispatches since the batch before it; or a draw, with dispatches since
/// the last wait). `careful`: before indirect dispatches too, with
/// dispatches since the last wait. `compute`: the pipelines that are compute
/// ones. The tables go to `jobs` (the contents of [`BATCH_JOBS`]).
pub fn batch_clamps(
    cmds: Vec<Cmd>,
    psos: &HashSet<u32>,
    compute: &HashSet<u32>,
    scratch: u32,
    careful: bool,
    jobs: &mut Vec<u8>,
) -> (Vec<Cmd>, Batched) {
    use crate::indirect::batch::{JOBS, SLOT, SOURCES, TIMES, job};
    const DISPATCH: u32 = 2;
    let mut done = Batched::default();
    let scratches: HashSet<u32> = {
        let mut next = 0;
        let found = find_clamps(&cmds, psos, &mut next);
        found.iter().map(|c| c.scratch).collect()
    };
    if scratches.is_empty() {
        return (cmds, done);
    }
    let of_clamp =
        |b: &Barrier| matches!(b, Barrier::Transition { res, .. } if scratches.contains(res));
    let moved = |before, after| Barrier::Transition {
        flags: 0,
        res: scratch,
        sub: ALL,
        before,
        after,
    };
    // What the last clamp into a scratch buffer clamped: source, offset,
    // kind.
    let mut clamps: HashMap<u32, (u32, u64, u32)> = HashMap::new();
    let (mut bound, mut constants, mut views) = (NONE, [0u32; 6], [(NONE, 0u64); 5]);
    let mut out: Vec<Cmd> = Vec::with_capacity(cmds.len());
    let mut open: Option<Batch> = None;
    let (mut written, mut dirty, mut readable) = (true, true, false);
    let mut cmds = cmds.into_iter().peekable();
    while let Some(c) = cmds.next() {
        match &c {
            // A clamp's own commands: its root signature and all it sets.
            Cmd::RootSignature {
                graphics: false, ..
            } if matches!(cmds.peek(), Some(Cmd::Pipeline { pso }) if psos.contains(pso)) => {}
            Cmd::Pipeline { pso } => {
                bound = *pso;
                if !psos.contains(pso) {
                    out.push(c);
                }
            }
            Cmd::Constants {
                graphics: false,
                values,
                offset,
                ..
            } if psos.contains(&bound) => {
                for (i, v) in values.iter().enumerate() {
                    if let Some(c) = constants.get_mut(*offset as usize + i) {
                        *c = *v;
                    }
                }
            }
            Cmd::RootView {
                graphics: false,
                param,
                va: Va::At { res, offset },
                ..
            } if psos.contains(&bound) => {
                if let Some(v) = views.get_mut(*param as usize) {
                    *v = (*res, *offset);
                }
            }
            Cmd::Dispatch { .. } if psos.contains(&bound) => {
                done.clamps += 1;
                let (source, offset) = views[1];
                clamps.insert(
                    views[3].0,
                    (source, offset + u64::from(constants[3]), constants[0]),
                );
                // What it was clamped for follows: a batch is open by then.
                let full =
                    |o: &Batch| !o.sources.contains(&source) && o.sources.len() == SOURCES;
                if dirty && (constants[0] != DISPATCH || careful) || open.as_ref().is_some_and(full)
                {
                    open = None;
                }
                if open.is_some() {
                    continue;
                }
                done.batches += 1;
                let table = jobs.len();
                jobs.resize(table + 16 + 16 * 2048, 0);
                out.push(Cmd::Barriers {
                    all: vec![
                        Barrier::Uav {
                            flags: 0,
                            res: NONE,
                        },
                        moved(if readable { INDIRECT_ARGUMENT } else { 0 }, UNORDERED_ACCESS),
                    ],
                });
                readable = true;
                out.push(Cmd::RootSignature {
                    graphics: false,
                    root: BATCH_ROOT,
                });
                out.push(Cmd::Pipeline { pso: BATCH });
                let at = out.len();
                let mut view = |kind, param: usize, res, offset| {
                    out.push(Cmd::RootView {
                        graphics: false,
                        kind,
                        param: param as u32,
                        va: Va::At { res, offset },
                    });
                };
                for i in 0..SOURCES {
                    view(slot::SRV, i, source, 0);
                }
                view(slot::SRV, SOURCES, BATCH_JOBS, table as u64);
                view(slot::UAV, SOURCES + 1, scratch, 0);
                view(slot::UAV, SOURCES + 2, views[4].0, views[4].1);
                out.push(Cmd::Dispatch {
                    groups: [JOBS.div_ceil(64), 1, 1],
                });
                out.push(Cmd::Barriers {
                    all: vec![moved(UNORDERED_ACCESS, INDIRECT_ARGUMENT)],
                });
                open = Some(Batch {
                    table,
                    at,
                    sources: Vec::new(),
                    known: HashMap::new(),
                    n: 0,
                    base: done.scratch,
                });
                (written, dirty) = (false, false);
            }
            Cmd::Barriers { all } => {
                if all.iter().any(of_clamp) {
                    continue;
                }
                let waits = |b: &Barrier| matches!(b, Barrier::Uav { res: NONE, .. });
                if all.iter().any(waits) {
                    if written {
                        open = None;
                    }
                    dirty = false;
                }
                out.push(c);
            }
            Cmd::Indirect {
                signature,
                most,
                args,
                at,
                count,
                count_at,
            } if clamps.contains_key(args) => {
                let (source, offset, kind) = clamps.get(args).copied().unwrap_or_default();
                let Some(o) = &mut open else {
                    // Not clamped by any batch: left out, which shows.
                    continue;
                };
                let slot = match o.sources.iter().position(|&s| s == source) {
                    Some(i) => i,
                    None => {
                        o.sources.push(source);
                        let i = o.sources.len() - 1;
                        if let Some(Cmd::RootView {
                            va: Va::At { res, .. },
                            ..
                        }) = out.get_mut(o.at + i)
                        {
                            *res = source;
                        }
                        i
                    }
                };
                let offset = offset + at;
                let entry = o.known.entry((source, offset, kind)).or_insert((o.n, 0));
                entry.1 += 1;
                let (i, times) = *entry;
                if i == o.n {
                    o.n += 1;
                    done.jobs += 1;
                    done.scratch = o.base + u64::from(o.n * SLOT);
                    // Wider than the live pass where its table is longer.
                    if let Some(Cmd::Dispatch { groups }) = out.get_mut(o.at + SOURCES + 3) {
                        groups[0] = groups[0].max(o.n.div_ceil(64));
                    }
                }
                let place = o.base + u64::from(i * SLOT);
                let words = [
                    job(slot as u32, kind, times.min(TIMES)),
                    offset as u32,
                    place as u32,
                    0,
                ];
                let mut put = |at: usize, w: u32| {
                    jobs[o.table + at..o.table + at + 4].copy_from_slice(&w.to_le_bytes());
                };
                for (k, w) in words.into_iter().enumerate() {
                    put(16 + 16 * i as usize + 4 * k, w);
                }
                put(0, o.n);
                out.push(Cmd::Indirect {
                    signature: *signature,
                    most: *most,
                    args: scratch,
                    at: place,
                    count: *count,
                    count_at: *count_at,
                });
                if compute.contains(&bound) {
                    (written, dirty) = (true, true);
                }
            }
            Cmd::Dispatch { .. } => {
                (written, dirty) = (true, true);
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    if readable {
        out.push(Cmd::Barriers {
            all: vec![moved(INDIRECT_ARGUMENT, 0)],
        });
    }
    (out, done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pso(pso: u32) -> Cmd {
        Cmd::Pipeline { pso }
    }
    fn go() -> Cmd {
        Cmd::Dispatch { groups: [1, 1, 1] }
    }
    fn uav(res: u32) -> Barrier {
        Barrier::Uav { flags: 0, res }
    }
    fn moved(res: u32, before: i32, after: i32) -> Barrier {
        Barrier::Transition {
            flags: 0,
            res,
            sub: ALL,
            before,
            after,
        }
    }
    fn barriers(all: &[Barrier]) -> Cmd {
        Cmd::Barriers { all: all.to_vec() }
    }

    #[test]
    fn pipelines_are_skipped() {
        let list = vec![
            pso(1),
            go(),
            pso(2),
            go(),
            go(),
            Cmd::CopyResource { dst: 1, src: 2 },
            pso(1),
            go(),
        ];
        let (out, n) = skip_pipelines(list.clone(), &HashSet::from([2]));
        assert_eq!(n, 2);
        assert_eq!(
            out,
            vec![
                pso(1),
                go(),
                pso(2),
                Cmd::CopyResource { dst: 1, src: 2 },
                pso(1),
                go()
            ]
        );
        assert_eq!(skip_pipelines(list.clone(), &HashSet::new()), (list, 0));
    }

    #[test]
    fn uav_barriers_are_dropped() {
        let list = vec![
            barriers(&[uav(0)]),
            go(),
            barriers(&[uav(1), moved(1, 8, 4)]),
        ];
        assert_eq!(
            no_uav_barriers(list.clone()),
            (vec![go(), barriers(&[moved(1, 8, 4)])], 2)
        );
        assert_eq!(
            barriers_only(list.clone()),
            vec![list[0].clone(), list[2].clone()]
        );
    }

    #[test]
    fn barriers_are_merged() {
        let list = vec![
            barriers(&[moved(1, 0, 8)]),
            pso(1),
            barriers(&[uav(2), moved(1, 8, 64)]),
            barriers(&[uav(0), uav(3)]),
            go(),
            // There and back again: nothing is left but the wait.
            barriers(&[moved(1, 8, 64)]),
            barriers(&[moved(1, 64, 8)]),
            go(),
            barriers(&[moved(4, 4, 128)]),
            barriers(&[moved(4, 128, 4)]),
        ];
        let (out, saved) = merge_barriers(list);
        assert_eq!(saved, 4);
        assert_eq!(
            out,
            vec![
                barriers(&[moved(1, 0, 64), uav(0)]),
                pso(1),
                go(),
                barriers(&[uav(1)]),
                go()
            ]
        );
        // Parts of a resource are not mixed up.
        let part = |sub, before, after| Barrier::Transition {
            flags: 0,
            res: 1,
            sub,
            before,
            after,
        };
        let list = vec![
            barriers(&[part(0, 4, 8)]),
            barriers(&[part(1, 4, 8)]),
            barriers(&[part(ALL, 8, 64)]),
        ];
        assert_eq!(
            merge_barriers(list).0,
            vec![barriers(&[part(0, 4, 8), part(1, 4, 8), part(ALL, 8, 64)])]
        );
    }

    #[test]
    fn clamp_copy_waits_for_the_end_of_a_split_transition() {
        let clamp = Clamp {
            at: 0,
            scratch: 7,
            bytes: 16,
            result: 256,
        };
        let transition = |flags| {
            barriers(&[Barrier::Transition {
                flags,
                res: clamp.scratch,
                sub: ALL,
                before: UNORDERED_ACCESS,
                after: INDIRECT_ARGUMENT,
            }])
        };
        let begin = transition(1);
        let end = transition(2);
        let pending = vec![
            go(),
            begin,
            barriers(&[moved(8, UNORDERED_ACCESS, INDIRECT_ARGUMENT)]),
            pso(3),
        ];
        assert!(keep_clamped(pending.clone(), &[clamp]).is_err());

        let mut list = pending;
        list.push(end);
        let mut expected = list.clone();
        expected.extend([
            barriers(&[moved(7, INDIRECT_ARGUMENT, COPY_SOURCE)]),
            Cmd::CopyBuffer {
                dst: RESULTS,
                dst_at: 256,
                src: 7,
                src_at: 0,
                len: 16,
            },
            barriers(&[moved(7, COPY_SOURCE, INDIRECT_ARGUMENT)]),
        ]);
        assert_eq!(keep_clamped(list.clone(), &[clamp]).unwrap(), expected);
        let next = moved(7, INDIRECT_ARGUMENT, UNORDERED_ACCESS);
        if let Some(Cmd::Barriers { all }) = list.last_mut() {
            all.push(next.clone());
        }
        expected.push(barriers(&[next]));
        assert_eq!(keep_clamped(list, &[clamp]).unwrap(), expected);
    }

    #[test]
    fn clamps_are_found_kept_and_skipped() {
        let clamp = |scratch, stride, records| {
            vec![
                barriers(&[uav(0), moved(scratch, 0, 8)]),
                pso(9),
                Cmd::Constants {
                    graphics: false,
                    param: 0,
                    offset: 0,
                    values: vec![10, stride, records, 0, 0, 0],
                },
                Cmd::RootView {
                    graphics: false,
                    kind: 3,
                    param: 3,
                    va: Va::At {
                        res: scratch,
                        offset: 0,
                    },
                },
                go(),
                barriers(&[uav(0), moved(scratch, 8, 0x200)]),
            ]
        };
        let run = |args, at| Cmd::Indirect {
            signature: 1,
            most: 1,
            args,
            at,
            count: 0,
            count_at: 0,
        };
        let mut list = clamp(5, 12, 2);
        list.extend([pso(1), run(5, 12), run(5, 0), run(6, 0)]);
        list.extend(clamp(5, 12, 100));
        list.extend([pso(1), run(5, 24), barriers(&[moved(5, 0x200, 0)])]);
        let mut next = 0;
        let found = find_clamps(&list, &HashSet::from([9]), &mut next);
        assert_eq!(
            found
                .iter()
                .map(|c| (c.scratch, c.bytes, c.result))
                .collect::<Vec<_>>(),
            [(5, 24, 0), (5, 1200, 256)]
        );
        assert_eq!(next, 256 + 1280);
        let kept = keep_clamped(list.clone(), &found).unwrap();
        assert_eq!(kept.len(), list.len() + 6);
        assert_eq!(
            kept[7],
            Cmd::CopyBuffer {
                dst: RESULTS,
                dst_at: 0,
                src: 5,
                src_at: 0,
                len: 24
            }
        );
        assert_eq!(
            kept.iter()
                .filter(|c| matches!(c, Cmd::CopyBuffer { .. }))
                .count(),
            2
        );
        let skipped = skip_clamps(list, &found);
        assert!(
            !skipped
                .iter()
                .any(|c| matches!(c, Cmd::Dispatch { .. } | Cmd::Barriers { .. }))
        );
        let runs: Vec<(u32, u64)> = skipped
            .iter()
            .filter_map(|c| {
                if let Cmd::Indirect { args, at, .. } = c {
                    Some((*args, *at))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            runs,
            [(RESULTS, 12), (RESULTS, 0), (6, 0), (RESULTS, 256 + 24)]
        );
    }

    #[test]
    fn clamps_are_batched() {
        use crate::indirect::batch::job;
        let clamp = |kind, offset, records, source| {
            let view = |param, res| Cmd::RootView {
                graphics: false,
                kind: 2,
                param,
                va: Va::At { res, offset: 0 },
            };
            vec![
                barriers(&[uav(0), moved(5, 0x200, 8)]),
                Cmd::RootSignature {
                    graphics: false,
                    root: 8,
                },
                pso(9),
                Cmd::Constants {
                    graphics: false,
                    param: 0,
                    offset: 0,
                    values: vec![kind, 32, records, offset, 0, 0],
                },
                view(1, source),
                view(2, source),
                view(3, 5),
                view(4, 6),
                go(),
                barriers(&[uav(0), moved(5, 8, 0x200)]),
            ]
        };
        let run = |at| Cmd::Indirect {
            signature: 1,
            most: 1,
            args: 5,
            at,
            count: 0,
            count_at: 0,
        };
        // A dispatch, two indirect ones with a wait of the title's between
        // them, then draws: a window of two and another buffer's.
        let mut list = vec![pso(1), go()];
        list.extend(clamp(2, 0, 1, 70));
        list.extend([pso(1), run(0)]);
        list.extend(clamp(2, 64, 1, 70));
        list.extend([pso(1), run(0), barriers(&[uav(0)])]);
        list.extend(clamp(2, 64, 1, 70));
        list.extend([pso(1), run(0)]);
        list.extend(clamp(10, 128, 2, 70));
        list.extend([pso(2), run(0), run(32), run(32)]);
        list.extend(clamp(10, 256, 1, 71));
        list.extend([pso(2), run(0)]);
        let (clamps, compute) = (HashSet::from([9]), HashSet::from([1]));
        let table = 16 + 16 * 2048;
        for (careful, batches) in [(false, 3), (true, 4)] {
            let mut jobs = Vec::new();
            let (out, done) = batch_clamps(list.clone(), &clamps, &compute, 99, careful, &mut jobs);
            assert_eq!((done.clamps, done.batches, done.jobs), (5, batches, 6), "careful {careful}");
            assert_eq!(jobs.len(), table * batches as usize);
            let passes = out.iter().filter(|c| matches!(c, Cmd::Pipeline { pso: BATCH })).count();
            assert_eq!(passes, batches as usize);
            // Nothing of the clamps is left; the title's wait is.
            assert!(!out.contains(&pso(9)) && out.contains(&barriers(&[uav(0)])));
            let names_5 = |c: &Cmd| matches!(c, Cmd::Barriers { all } if all.iter().any(|b| matches!(b, Barrier::Transition { res: 5, .. })));
            assert!(!out.iter().any(names_5));
            let places: Vec<u64> = out
                .iter()
                .filter_map(|c| match c {
                    Cmd::Indirect { args: 99, at, .. } => Some(*at),
                    _ => None,
                })
                .collect();
            assert_eq!(places, [0, 32, 64, 96, 128, 128, 160]);
            // The last table: two records, one read twice, then one of the
            // other buffer.
            let word = |i: usize| {
                let at = jobs.len() - table + 4 * i;
                u32::from_le_bytes(jobs[at..at + 4].try_into().unwrap())
            };
            assert_eq!(word(0), 3);
            let all: Vec<u32> = (4..16).map(word).collect();
            assert_eq!(
                all,
                [
                    [job(0, 10, 1), 128, 96, 0],
                    [job(0, 10, 2), 160, 128, 0],
                    [job(1, 10, 1), 256, 160, 0]
                ]
                .concat()
            );
            let sources: Vec<u32> = out
                .iter()
                .filter_map(|c| match c {
                    Cmd::RootView { param: 1, va: Va::At { res, .. }, .. } => Some(*res),
                    _ => None,
                })
                .collect();
            assert_eq!(sources.last(), Some(&71));
            // It ends common, as it began.
            assert_eq!(out.last(), Some(&barriers(&[moved(99, 0x200, 0)])));
        }
        // A list without clamps is left as it is.
        let plain = vec![pso(1), go(), barriers(&[uav(0)])];
        let (out, done) = batch_clamps(plain.clone(), &clamps, &compute, 99, false, &mut Vec::new());
        assert_eq!((out, done), (plain, Batched::default()));
    }
}
