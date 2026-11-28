//! Where identical runs of a frame part: every list is run several times
//! from the very same memory (what the frame writes is put back before each,
//! and everything before it was run once), so what differs after it was made
//! different by it; then the first command of the list that does so, by
//! halving the list. The state after the last run is what the rest goes on
//! from, so each finding is a root, not a consequence.

use super::*;

/// A part of memory compared as one.
const PIECE: usize = 64 << 10;

/// Where a run's contents come down: for each resource (and subresource) the
/// frame writes, its place in read-back memory.
pub(super) struct Reader {
    blocks: Vec<(ID3D12Resource, usize)>,
    pub(super) parts: Vec<Part>,
}

#[derive(Clone)]
pub(super) struct Part {
    pub(super) id: u32,
    /// `None` for a buffer.
    sub: Option<(u32, D3D12_PLACED_SUBRESOURCE_FOOTPRINT)>,
    block: usize,
    at: u64,
    pub(super) len: u64,
}

/// One way to run commands `from..to` of a list.
struct Unit<'a> {
    submit: usize,
    cmds: &'a [Cmd],
    from: usize,
    to: usize,
    /// With the copies of what the CPU wrote before the submission.
    delta: bool,
}

/// What differed between runs: part and piece.
type Pieces = Vec<(usize, usize)>;

/// What to look at.
#[derive(Clone, Debug)]
pub struct Search {
    /// Runs of each list.
    pub runs: u32,
    /// Only these lists (the others are run once).
    pub lists: Option<(u32, u32)>,
    /// Roots looked for in one list.
    pub roots: u32,
    /// A full wait after each command.
    pub wait_all: bool,
    /// Experiments on the frame.
    pub options: Options,
}

impl Player {
    /// Looks for what makes identical runs differ; `say` gets the findings
    /// as they are made.
    pub fn diverge(&mut self, s: &Search, say: &mut dyn FnMut(String)) -> Result<()> {
        let mut report = Report::default();
        let clamps = if s.options.skip_clamps {
            self.clamp_results(&mut report)?
        } else {
            Vec::new()
        };
        let submits = self.edited(&s.options, &mut report, &clamps);
        self.restore(true)?;
        self.fresh = false;
        let ids: Vec<u32> = self.masters.iter().map(|m| m.0).collect();
        let reader = self.reader(&ids)?;
        let mb: u64 = reader.parts.iter().map(|p| p.len).sum::<u64>() >> 20;
        say(format!(
            "{} resources the frame writes ({mb} MB in {} parts) compared after each list, {} runs of each",
            self.masters.len(),
            reader.parts.len(),
            s.runs
        ));
        let mut states = self.start_states();
        let mut global = 0u32;
        for (i, lists) in submits.iter().enumerate() {
            for (j, cmds) in lists.iter().enumerate() {
                let here = global;
                global += 1;
                let delta = j == 0;
                let wanted = s.lists.is_none_or(|(a, b)| (a..=b).contains(&here));
                let mut from = 0;
                let mut roots = 0;
                loop {
                    let unit = Unit {
                        submit: i,
                        cmds,
                        from,
                        to: cmds.len(),
                        delta: delta && from == 0,
                    };
                    let looks = wanted && cmds[from..].iter().any(Cmd::works);
                    let runs = if looks { s.runs } else { 1 };
                    let pieces = self.trials(&reader, &unit, &states, runs, s.wait_all)?;
                    if pieces.is_empty() || roots >= s.roots {
                        self.commit(&unit, &mut states)?;
                        break;
                    }
                    roots += 1;
                    // The shortest start of the rest that differs.
                    let works: Vec<usize> =
                        (from..cmds.len()).filter(|&c| cmds[c].works()).collect();
                    let (mut lo, mut hi) = (0, works.len() - 1);
                    while lo < hi {
                        let mid = (lo + hi) / 2;
                        let cut = Unit {
                            to: works[mid] + 1,
                            ..unit
                        };
                        self.put_back(&states)?;
                        if self
                            .trials(&reader, &cut, &states, s.runs, s.wait_all)?
                            .is_empty()
                        {
                            lo = mid + 1;
                        } else {
                            hi = mid;
                        }
                    }
                    let at = works[lo];
                    let cut = Unit { to: at + 1, ..unit };
                    self.put_back(&states)?;
                    let pieces = self.trials(&reader, &cut, &states, s.runs.max(6), s.wait_all)?;
                    say(format!(
                        "list {here} (submission {i}, its list {j}), command {at} of {}: {}",
                        cmds.len(),
                        self.describe(cmds, at)
                    ));
                    self.put_back(&states)?;
                    for line in self.compare(&reader, &cut, &states, &pieces, s.wait_all)? {
                        say(format!("    {line}"));
                    }
                    self.commit(&cut, &mut states)?;
                    from = at + 1;
                    if from >= cmds.len() {
                        break;
                    }
                }
            }
            states.decay();
            if s.lists.is_some_and(|(_, b)| global > b) {
                break;
            }
            if i % 5 == 4 {
                say(format!("submission {i} done"));
            }
        }
        self.restore_all()?;
        Ok(())
    }

    /// Looks for where the picture's variants are decided: at each list (or,
    /// of one list, each `step` commands in `commands`) the rest of the frame
    /// is run several times from the same memory. Where the runs' pictures
    /// agree from a point on but not from the one before, what lies between
    /// decides.
    pub fn diverge_picture(
        &mut self,
        s: &Search,
        commands: Option<(u32, u32)>,
        step: usize,
        say: &mut dyn FnMut(String),
    ) -> Result<()> {
        let mut report = Report::default();
        let submits = self.edited(&s.options, &mut report, &[]);
        let flat: Vec<(usize, usize, &Vec<Cmd>)> = submits
            .iter()
            .enumerate()
            .flat_map(|(i, lists)| lists.iter().enumerate().map(move |(j, c)| (i, j, c)))
            .collect();
        self.restore(true)?;
        self.fresh = false;
        let mut states = self.start_states();
        for (here, &(i, j, cmds)) in flat.iter().enumerate() {
            let inside = s.lists.is_some_and(|(a, b)| a == b && a as usize == here);
            let looks = s
                .lists
                .is_none_or(|(a, b)| (a as usize..=b as usize).contains(&here));
            let works: Vec<usize> = (0..cmds.len()).filter(|&c| cmds[c].works()).collect();
            // Where the list is cut.
            let mut cuts: Vec<usize> = vec![0];
            if inside {
                let (a, b) = commands.unwrap_or((0, u32::MAX));
                let some = works
                    .iter()
                    .map(|&c| c + 1)
                    .filter(|c| (a as usize..=b as usize).contains(c));
                cuts.extend(some.step_by(step.max(1)));
            }
            for (k, &from) in cuts.iter().enumerate() {
                if looks && !works.is_empty() {
                    let mut pictures: Vec<(f64, String, Vec<u8>)> = Vec::new();
                    for run in 0..s.runs {
                        if run > 0 {
                            self.put_back(&states)?;
                        }
                        let mut now = states.clone();
                        let first = Unit {
                            submit: i,
                            cmds,
                            from,
                            to: cmds.len(),
                            delta: j == 0 && from == 0,
                        };
                        self.go(&first, s.wait_all)?;
                        now = self.after(&first, &now);
                        for (n, &(i, j, cmds)) in flat.iter().enumerate().skip(here + 1) {
                            if flat[n - 1].0 != i {
                                now.decay();
                            }
                            let unit = Unit {
                                submit: i,
                                cmds,
                                from: 0,
                                to: cmds.len(),
                                delta: j == 0,
                            };
                            self.go(&unit, s.wait_all)?;
                            now = self.after(&unit, &now);
                        }
                        self.at = now;
                        if let Some((db, _, hash, rgb)) = self.picture_now()? {
                            pictures.push((db, hash, rgb));
                        }
                    }
                    self.put_back(&states)?;
                    let least = pictures
                        .iter()
                        .enumerate()
                        .flat_map(|(a, x)| pictures[a + 1..].iter().map(|y| gpu::psnr(&x.2, &y.2)))
                        .fold(f64::INFINITY, f64::min);
                    let kinds: HashSet<&String> = pictures.iter().map(|p| &p.1).collect();
                    let db: Vec<String> = pictures.iter().map(|p| format!("{:.1}", p.0)).collect();
                    say(format!(
                        "from list {here} (submission {i}, its list {j}), command {from}: {} pictures of {}, the least alike {least:.1} dB; against the recorded: {}",
                        kinds.len(),
                        pictures.len(),
                        db.join(" ")
                    ));
                }
                // On to the next cut.
                let to = cuts.get(k + 1).copied().unwrap_or(cmds.len());
                let unit = Unit {
                    submit: i,
                    cmds,
                    from,
                    to,
                    delta: j == 0 && from == 0,
                };
                self.go(&unit, s.wait_all)?;
                self.commit(&unit, &mut states)?;
                self.at = states.clone();
                if inside && looks {
                    let last = (from..to).rev().find(|&c| cmds[c].works());
                    if let Some(c) = last {
                        say(format!("  command {c}: {}", self.describe(cmds, c)));
                    }
                }
            }
            if flat.get(here + 1).is_none_or(|n| n.0 != i) {
                states.decay();
                self.at = states.clone();
            }
            if s.lists.is_some_and(|(_, b)| here >= b as usize) {
                break;
            }
        }
        Ok(())
    }

    /// The states of textures at the frame's start.
    pub(super) fn start_states(&self) -> States {
        let mut states = States::default();
        for (id, all) in &self.start {
            let Some(o) = self.objects.get(id) else {
                continue;
            };
            let flags = self.frame.resources[o.index].desc.flags;
            let decays = flags & D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS.0 != 0;
            states.add(*id, o.subs, all[0], decays);
            for (i, s) in all.iter().enumerate().filter(|_| all.len() > 1) {
                states.change(&states::Change {
                    res: *id,
                    sub: i as u32,
                    after: *s,
                    promoted: false,
                    alias: false,
                });
            }
        }
        states
    }

    fn after(&self, unit: &Unit, states: &States) -> States {
        let mut after = states.clone();
        let mut all = Vec::new();
        for c in &unit.cmds[unit.from..unit.to] {
            states::changes(c, &mut all);
        }
        all.iter().for_each(|c| after.change(c));
        after
    }

    /// Says which resources that are not put back before a run are changed
    /// by one all the same.
    pub fn check_written(&mut self, say: &mut dyn FnMut(String)) -> Result<()> {
        // What is put back, is as it was.
        {
            let ids: Vec<u32> = self.masters.iter().map(|m| m.0).collect();
            let reader = self.reader(&ids)?;
            let start = self.start_states();
            let before = self.hashes_now(&reader, &start)?;
            let mut again = self.hashes_now(&reader, &start)?;
            say(format!(
                "read twice, {} parts differ",
                before.iter().zip(&again).filter(|(a, b)| a != b).count()
            ));
            for i in 0..3 {
                self.run(&Options {
                    loops: 1,
                    ..Options::default()
                })?;
                let ran = {
                    let end: Vec<(u32, Vec<u32>)> =
                        self.end.iter().map(|(k, v)| (*k, v.clone())).collect();
                    let mut s = start.clone();
                    for (id, v) in end {
                        for (sub, st) in v.iter().enumerate() {
                            let sub = if v.len() == 1 { ALL } else { sub as u32 };
                            s.change(&states::Change {
                                res: id,
                                sub,
                                after: *st,
                                promoted: false,
                                alias: false,
                            });
                        }
                    }
                    self.hashes_now(&reader, &s)?
                };
                self.restore(true)?;
                again = self.hashes_now(&reader, &start)?;
                let differ: Vec<usize> = (0..before.len())
                    .filter(|&i| before[i] != again[i])
                    .collect();
                let changed = (0..before.len()).filter(|&i| before[i] != ran[i]).count();
                say(format!(
                    "run {i}: {changed} parts changed by the frame, {} not put back as they were",
                    differ.len()
                ));
                for &d in differ.iter().take(20) {
                    let pieces = before[d]
                        .iter()
                        .zip(&again[d])
                        .filter(|(a, b)| a != b)
                        .count();
                    say(format!(
                        "    {} {:?}: {pieces} of {} pieces",
                        self.called(reader.parts[d].id),
                        reader.parts[d].sub.map(|s| s.0),
                        before[d].len()
                    ));
                }
            }
        }
        let put_back: HashSet<u32> = self.masters.iter().map(|m| m.0).collect();
        let others: Vec<u32> = self
            .objects
            .keys()
            .copied()
            .filter(|id| !put_back.contains(id))
            .collect();
        let reader = self.reader(&others)?;
        self.restore(true)?;
        let start = self.start_states();
        let before = self.hashes_now(&reader, &start)?;
        let o = Options {
            loops: 1,
            ..Options::default()
        };
        self.run(&o)?;
        self.restore(true)?;
        let after = self.hashes_now(&reader, &start)?;
        let mut n = 0;
        for (i, (a, b)) in before.iter().zip(&after).enumerate() {
            let pieces = a.iter().zip(b).filter(|(a, b)| a != b).count();
            if pieces > 0 {
                n += 1;
                let p = &reader.parts[i];
                let r = &self.frame.resources[self.objects[&p.id].index];
                say(format!(
                    "not put back, but changed: {} {:?} idle {} late {}: {pieces} pieces of {}",
                    self.called(p.id),
                    r.place,
                    r.idle,
                    r.late,
                    a.len()
                ));
            }
        }
        say(format!(
            "{} parts of {} resources looked at, {n} changed",
            reader.parts.len(),
            others.len()
        ));
        Ok(())
    }

    pub(super) fn reader(&self, ids: &[u32]) -> Result<Reader> {
        const BLOCK: u64 = 512 << 20;
        let mut parts = Vec::new();
        let mut sizes: Vec<u64> = vec![0];
        for id in ids {
            let o = &self.objects[id];
            let r = &self.frame.resources[o.index];
            let back = matches!(&r.place, Place::Committed { props, .. } if gpu::seen(props) == Seen::ReadBack);
            if r.desc.samples[0] > 1 || back {
                continue;
            }
            let is_buffer = r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0;
            let desc = gpu::to_desc(&r.desc);
            for sub in 0..o.subs {
                let (sub, len) = if is_buffer {
                    (None, r.desc.width)
                } else {
                    let (f, total) = gpu::footprints(&self.device, &desc, sub, 1);
                    (Some((sub, f[0])), total)
                };
                let last = sizes.len() - 1;
                let at = sizes[last].next_multiple_of(512);
                let (block, at) = if at + len > BLOCK && at > 0 {
                    sizes.push(0);
                    (last + 1, 0)
                } else {
                    (last, at)
                };
                sizes[block] = at + len;
                parts.push(Part {
                    id: *id,
                    sub,
                    block,
                    at,
                    len,
                });
            }
        }
        let mut blocks = Vec::new();
        for size in sizes {
            let (b, data) = gpu::buffer(
                &self.device,
                D3D12_HEAP_TYPE_READBACK,
                size,
                D3D12_RESOURCE_FLAG_NONE,
            )?;
            blocks.push((b, data as usize));
        }
        Ok(Reader { blocks, parts })
    }

    /// Runs the unit once; resources are as at its start.
    fn go(&mut self, unit: &Unit, wait: bool) -> Result<()> {
        let mut lists = Vec::new();
        if unit.delta
            && let Some(parts) = self.delta_parts.get(unit.submit).filter(|p| !p.is_empty())
            && let Some(from) = &self.deltas
        {
            let l = self.plain_list()?;
            for (d, at) in parts {
                if let Some(o) = self.objects.get(&d.res) {
                    // SAFETY: an open list; live buffers.
                    unsafe { l.1.CopyBufferRegion(&o.res, d.at, from, *at, d.len) };
                }
            }
            // SAFETY: as above.
            unsafe { l.1.Close()? };
            lists.push(l);
        }
        let l = self.plain_list()?;
        // What the commands before have set, without what they do.
        let before: Vec<Cmd> = unit.cmds[..unit.from]
            .iter()
            .filter(|c| {
                !c.works()
                    && !matches!(
                        c,
                        Cmd::Barriers { .. } | Cmd::BeginQuery { .. } | Cmd::EndQuery { .. }
                    )
            })
            .cloned()
            .collect();
        self.translate(&l.1, &before, unit.submit, None, false);
        self.translate(
            &l.1,
            &unit.cmds[unit.from..unit.to],
            unit.submit,
            None,
            wait,
        );
        // SAFETY: an open list.
        unsafe { l.1.Close()? };
        lists.push(l);
        for l in &lists {
            // SAFETY: a closed list; waited for before it goes.
            unsafe { self.worker.queue.ExecuteCommandLists(&[Some(l.1.cast()?)]) };
            self.worker.idle().context("the card was lost")?;
        }
        Ok(())
    }

    /// Hashes of all parts' pieces, as memory is now (in `states`).
    pub(super) fn hashes_now(&mut self, reader: &Reader, states: &States) -> Result<Vec<Vec<u64>>> {
        let objects = &self.objects;
        for block in 0..reader.blocks.len() {
            let parts: Vec<&Part> = reader.parts.iter().filter(|p| p.block == block).collect();
            let (mut before, mut after) = (Vec::new(), Vec::new());
            let mut seen = HashSet::new();
            for p in parts.iter().filter(|p| p.sub.is_some()) {
                if seen.insert(p.id) {
                    let state = states.get(p.id).unwrap_or(vec![COMMON]);
                    let to: Vec<u32> = state
                        .iter()
                        .map(|&s| if s & COPY_SOURCE != 0 { s } else { COPY_SOURCE })
                        .collect();
                    gpu::moves(&objects[&p.id].res, &state, &to, &mut before);
                    gpu::moves(&objects[&p.id].res, &to, &state, &mut after);
                }
            }
            let to = &reader.blocks[block].0;
            self.worker.run(|list| {
                // SAFETY: an open list; live resources.
                unsafe {
                    barriers(list, &before);
                    for p in &parts {
                        let res = &objects[&p.id].res;
                        match p.sub {
                            None => list.CopyBufferRegion(to, p.at, res, 0, p.len),
                            Some((sub, mut f)) => {
                                f.Offset = p.at;
                                list.CopyTextureRegion(
                                    &gpu::placed_location(to, f),
                                    0,
                                    0,
                                    0,
                                    &gpu::sub_location(res, sub),
                                    None,
                                );
                            }
                        }
                    }
                    barriers(list, &after);
                }
            })?;
        }
        let next = AtomicUsize::new(0);
        let out: std::sync::Mutex<Vec<Vec<u64>>> =
            std::sync::Mutex::new(vec![Vec::new(); reader.parts.len()]);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    while let Some((i, p)) = {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        reader.parts.get(i).map(|p| (i, p))
                    } {
                        let all: Vec<u64> = reader
                            .bytes(p)
                            .chunks(PIECE)
                            .map(|c| {
                                let h = blake3::hash(c);
                                u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap_or_default())
                            })
                            .collect();
                        out.lock().unwrap_or_else(|e| e.into_inner())[i] = all;
                    }
                });
            }
        });
        Ok(out.into_inner().unwrap_or_else(|e| e.into_inner()))
    }

    /// Runs the unit `runs` times (memory is as at its start, and is put
    /// back between runs) and compares. Memory is as the last run left it.
    fn trials(
        &mut self,
        reader: &Reader,
        unit: &Unit,
        states: &States,
        runs: u32,
        wait: bool,
    ) -> Result<Pieces> {
        let after = self.after(unit, states);
        let mut first: Option<Vec<Vec<u64>>> = None;
        let mut pieces = std::collections::BTreeSet::new();
        for run in 0..runs {
            if run > 0 {
                self.put_back_from(&after, states)?;
            }
            self.go(unit, wait)?;
            if runs == 1 {
                break;
            }
            let now = self.hashes_now(reader, &after)?;
            match &first {
                None => first = Some(now),
                Some(first) => {
                    for (p, (a, b)) in first.iter().zip(&now).enumerate() {
                        pieces.extend(
                            a.iter()
                                .zip(b)
                                .enumerate()
                                .filter(|(_, (a, b))| a != b)
                                .map(|(i, _)| (p, i)),
                        );
                    }
                }
            }
        }
        self.at = after;
        Ok(pieces.into_iter().collect())
    }

    /// Puts memory back to the start of a unit in `states`, from where the
    /// last run left it.
    fn put_back(&mut self, states: &States) -> Result<()> {
        let from = self.at.clone();
        self.put_back_from(&from, states)
    }

    fn put_back_from(&mut self, from: &States, to: &States) -> Result<()> {
        self.copy_masters(from, to, false)?;
        self.at = to.clone();
        Ok(())
    }

    /// What memory holds after the last run is the start of what follows.
    fn commit(&mut self, unit: &Unit, states: &mut States) -> Result<()> {
        *states = self.after(unit, states);
        self.copy_masters(&states.clone(), states, true)?;
        Ok(())
    }

    /// Copies the masters over the resources (which are in `from` and go to
    /// `to`), or the resources into the masters.
    fn copy_masters(&mut self, from: &States, to: &States, save: bool) -> Result<()> {
        let known: HashMap<u32, &ID3D12Resource> =
            self.masters.iter().map(|(id, m)| (*id, m)).collect();
        let (mut before, mut after, mut plain) = (Vec::new(), Vec::new(), Vec::new());
        let here = if save { COPY_SOURCE } else { COPY_DEST };
        for id in self.start.keys() {
            let Some(o) = self.objects.get(id) else {
                continue;
            };
            let (a, b) = (
                from.get(*id).unwrap_or(vec![COMMON]),
                to.get(*id).unwrap_or(vec![COMMON]),
            );
            let Some(m) = known.get(id) else {
                gpu::moves(&o.res, &a, &b, &mut plain);
                continue;
            };
            gpu::moves(&o.res, &a, &[here], &mut before);
            gpu::moves(&o.res, &[here], &b, &mut after);
            if save {
                before.push(gpu::transition(m, ALL, COPY_SOURCE, COPY_DEST));
                after.push(gpu::transition(m, ALL, COPY_DEST, COPY_SOURCE));
            }
        }
        let objects = &self.objects;
        self.worker.run(|list| {
            // SAFETY: an open list; live resources of one description.
            unsafe {
                barriers(list, &plain);
                barriers(list, &before);
                for (id, m) in &known {
                    if save {
                        list.CopyResource(*m, &objects[id].res);
                    } else {
                        list.CopyResource(&objects[id].res, *m);
                    }
                }
                barriers(list, &after);
            }
        })?;
        Ok(())
    }

    /// The masters hold another state than the frame's start now.
    fn restore_all(&mut self) -> Result<()> {
        self.note("the player's copies are no longer of the frame's start: load it again".into());
        Ok(())
    }

    /// A command in words: its pipeline, what its list bound for it, and the
    /// barriers since the last command that worked.
    fn describe(&self, cmds: &[Cmd], at: usize) -> String {
        let mut pso = NONE;
        let mut binds: BTreeMap<(bool, u32), String> = BTreeMap::new();
        let mut since: Vec<String> = Vec::new();
        let mut targets = String::new();
        for c in &cmds[..at] {
            match c {
                Cmd::Pipeline { pso: p } => pso = *p,
                Cmd::RootSignature { graphics, .. } => binds.retain(|k, _| k.0 != *graphics),
                Cmd::RootView {
                    graphics,
                    kind,
                    param,
                    va,
                } => {
                    let kind = ["", "cbv", "srv", "uav"]
                        .get(*kind as usize)
                        .copied()
                        .unwrap_or("");
                    let text = match va {
                        Va::At { res, offset } => {
                            format!("{kind} {} +{offset:#x}", self.called(*res))
                        }
                        _ => format!("{kind} null"),
                    };
                    binds.insert((*graphics, *param), text);
                }
                Cmd::Table {
                    graphics,
                    param,
                    heap,
                    slot,
                } => {
                    binds.insert(
                        (*graphics, *param),
                        format!("table heap {heap} slot {slot}"),
                    );
                }
                Cmd::Constants {
                    graphics,
                    param,
                    offset,
                    values,
                } => {
                    let text = format!("constants +{offset} {values:x?}");
                    binds
                        .entry((*graphics, *param))
                        .and_modify(|t| *t = format!("{t}, {text}"))
                        .or_insert(text);
                }
                Cmd::Targets { colors, depth } => {
                    let all: Vec<String> = colors
                        .iter()
                        .chain(depth)
                        .map(|s| self.called(s.res))
                        .collect();
                    targets = all.join(", ");
                }
                Cmd::Barriers { all } => since.extend(all.iter().map(|b| match b {
                    Barrier::Transition {
                        res, before, after, ..
                    } => format!("{} {before:#x}->{after:#x}", self.called(*res)),
                    Barrier::Uav { res: 0, .. } => "all uav".into(),
                    Barrier::Uav { res, .. } => format!("uav {}", self.called(*res)),
                    Barrier::Alias { .. } => "alias".into(),
                })),
                _ => {}
            }
            if c.works() {
                since.clear();
            }
        }
        let graphics = !matches!(cmds[at], Cmd::Dispatch { .. });
        let binds: Vec<String> = binds
            .iter()
            .filter(|(k, _)| k.0 == graphics)
            .map(|(k, t)| format!("[{}] {t}", k.1))
            .collect();
        let mut text = format!("{:?}", cmds[at]);
        if cmds[at].runs_shaders() {
            text += &format!(" of {}", self.pipeline_name(pso));
            text += &format!("\n    bound: {}", binds.join("; "));
            if graphics && !targets.is_empty() {
                text += &format!("\n    targets: {targets}");
            }
        }
        text + &format!(
            "\n    barriers since the command before: {}",
            since.join("; ")
        )
    }

    /// Says which lists (of the list in `s.lists`: which commands) change
    /// words of a buffer, and to what.
    pub fn watch(
        &mut self,
        s: &Search,
        parts: &[(u32, u64, u64)],
        say: &mut dyn FnMut(String),
    ) -> Result<()> {
        let mut report = Report::default();
        let submits = self.edited(&s.options, &mut report, &[]);
        self.restore(true)?;
        self.fresh = false;
        let words = |p: &mut Player| -> Result<Vec<Vec<u32>>> {
            parts
                .iter()
                .map(|&(id, at, len)| {
                    let bytes = p.slice(id, at, len)?;
                    Ok(bytes
                        .chunks_exact(4)
                        .map(|c| u32::from_le_bytes(c.try_into().unwrap_or_default()))
                        .collect())
                })
                .collect()
        };
        let mut was = words(self)?;
        say(format!("at the start: {was:x?}"));
        let mut global = 0u32;
        for (i, lists) in submits.iter().enumerate() {
            for (j, cmds) in lists.iter().enumerate() {
                let here = global;
                global += 1;
                let inside = s.lists.is_some_and(|(a, b)| (a..=b).contains(&here));
                let mut cuts: Vec<usize> = vec![0];
                if inside {
                    cuts.extend((0..cmds.len()).filter(|&c| cmds[c].works()).map(|c| c + 1));
                }
                if *cuts.last().unwrap_or(&0) != cmds.len() {
                    cuts.push(cmds.len());
                }
                for k in 0..cuts.len() - 1 {
                    let unit = Unit {
                        submit: i,
                        cmds,
                        from: cuts[k],
                        to: cuts[k + 1],
                        delta: j == 0 && k == 0,
                    };
                    self.go(&unit, s.wait_all)?;
                    let now = words(self)?;
                    if now != was {
                        if inside {
                            let last = (unit.from..unit.to)
                                .rev()
                                .find(|&c| cmds[c].works())
                                .unwrap_or(unit.to - 1);
                            say(format!(
                                "list {here} (submission {i}, its list {j}) command {last}: {}",
                                self.describe(cmds, last)
                            ));
                        } else {
                            say(format!("list {here} (submission {i}, its list {j})"));
                        }
                        for (n, (a, b)) in was.iter().zip(&now).enumerate() {
                            let changed: Vec<String> = a
                                .iter()
                                .zip(b)
                                .enumerate()
                                .filter(|(_, (a, b))| a != b)
                                .take(40)
                                .map(|(w, (a, b))| format!("+{:#x}: {a:x} -> {b:x}", w * 4))
                                .collect();
                            if !changed.is_empty() {
                                say(format!("    part {n}: {}", changed.join(", ")));
                            }
                        }
                        was = now;
                    }
                }
                if s.options.only.is_some_and(|(_, b)| here >= b) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Each draw of one list whose pipeline's name has `text` in it, run
    /// alone (with what prepares its arguments) from the memory at the list's
    /// start, `runs` times: how many different contents its targets get.
    pub fn diverge_draws(
        &mut self,
        s: &Search,
        text: &str,
        say: &mut dyn FnMut(String),
    ) -> Result<()> {
        let mut report = Report::default();
        let submits = self.edited(&s.options, &mut report, &[]);
        let flat: Vec<(usize, usize, &Vec<Cmd>)> = submits
            .iter()
            .enumerate()
            .flat_map(|(i, lists)| lists.iter().enumerate().map(move |(j, c)| (i, j, c)))
            .collect();
        let wanted = s.lists.map_or(0, |l| l.0 as usize);
        self.restore(true)?;
        self.fresh = false;
        let mut states = self.start_states();
        for (here, &(i, j, cmds)) in flat.iter().enumerate() {
            if here < wanted {
                let unit = Unit {
                    submit: i,
                    cmds,
                    from: 0,
                    to: cmds.len(),
                    delta: j == 0,
                };
                self.go(&unit, s.wait_all)?;
                self.commit(&unit, &mut states)?;
                if flat.get(here + 1).is_none_or(|n| n.0 != i) {
                    states.decay();
                }
                self.at = states.clone();
                continue;
            }
            let mut last = 0;
            let mut pso = NONE;
            let mut targets: Vec<u32> = Vec::new();
            for (c, cmd) in cmds.iter().enumerate() {
                match cmd {
                    Cmd::Pipeline { pso: p } => pso = *p,
                    Cmd::Targets { colors, depth } => {
                        targets = colors
                            .iter()
                            .chain(depth)
                            .map(|s| s.res)
                            .filter(|r| *r != NONE)
                            .collect()
                    }
                    _ => {}
                }
                if !matches!(
                    cmd,
                    Cmd::Indirect { .. }
                        | Cmd::Draw { .. }
                        | Cmd::DrawIndexed { .. }
                        | Cmd::DispatchMesh { .. }
                ) {
                    continue;
                }
                let from = std::mem::replace(&mut last, c + 1);
                if !self.pipeline_name(pso).contains(text) {
                    continue;
                }
                targets.sort_unstable();
                targets.dedup();
                let reader = self.reader(&targets)?;
                let unit = Unit {
                    submit: i,
                    cmds,
                    from,
                    to: c + 1,
                    delta: false,
                };
                // Barriers before it in the list are made up for: textures
                // are in the states the list has them in here.
                let mut before = states.clone();
                let mut all = Vec::new();
                cmds[..from]
                    .iter()
                    .for_each(|c| states::changes(c, &mut all));
                all.iter().for_each(|c| before.change(c));
                let after = self.after(&unit, &before);
                let mut kinds: HashMap<Vec<Vec<u64>>, u32> = HashMap::new();
                for _ in 0..s.runs {
                    let at = self.at.clone();
                    self.copy_masters(&at, &before, false)?;
                    self.at = before.clone();
                    self.go(&unit, s.wait_all)?;
                    self.at = after.clone();
                    *kinds.entry(self.hashes_now(&reader, &after)?).or_default() += 1;
                }
                let at = self.at.clone();
                self.copy_masters(&at, &before, false)?;
                self.at = before.clone();
                if kinds.len() > 1 || std::env::var_os("VICTORY_REPLAY_PEEK").is_some() {
                    self.peek(cmds, c, i, say)?;
                }
                let mut counts: Vec<u32> = kinds.values().copied().collect();
                counts.sort_unstable();
                say(format!(
                    "command {c} (from {from}) {:?} of {}: {} outcomes {counts:?}",
                    cmd,
                    self.pipeline_name(pso),
                    counts.len()
                ));
            }
            break;
        }
        Ok(())
    }

    /// `len` bytes of a buffer from `at`.
    fn slice(&mut self, id: u32, at: u64, len: u64) -> Result<Vec<u8>> {
        let Some(o) = self.objects.get(&id) else {
            return Ok(Vec::new());
        };
        let len = len.min(self.frame.resources[o.index].desc.width.saturating_sub(at));
        if len == 0 {
            return Ok(Vec::new());
        }
        let res = o.res.clone();
        let (back, data) = gpu::buffer(
            &self.device,
            D3D12_HEAP_TYPE_READBACK,
            len,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        self.worker.run(|list| {
            // SAFETY: an open list; live buffers.
            unsafe { list.CopyBufferRegion(&back, 0, &res, at, len) };
        })?;
        // SAFETY: the mapping; the copy into it has run.
        Ok(unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec())
    }

    /// The words of a buffer view (raw or typed, of words).
    fn view_words(&mut self, s: &Slot, first: u64, n: u64) -> Result<Vec<u32>> {
        let at = u64::from_le_bytes([
            s.desc[16], s.desc[17], s.desc[18], s.desc[19], s.desc[20], s.desc[21], s.desc[22],
            s.desc[23],
        ]);
        let count = u64::from(u32::from_le_bytes([
            s.desc[24], s.desc[25], s.desc[26], s.desc[27],
        ]));
        let n = n.min(count.saturating_sub(first));
        let bytes = self.slice(s.res, (at + first) * 4, n * 4)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap_or_default()))
            .collect())
    }

    /// What a mesh draw of the title's (with its usual constants) reads.
    fn peek(
        &mut self,
        cmds: &[Cmd],
        at: usize,
        submit: usize,
        say: &mut dyn FnMut(String),
    ) -> Result<()> {
        let mut v = [0u32; 16];
        let mut heap = NONE;
        for c in &cmds[..at] {
            match c {
                Cmd::Constants {
                    graphics: true,
                    param: 0,
                    offset,
                    values,
                } => {
                    for (k, x) in values.iter().enumerate() {
                        if let Some(at) = v.get_mut(*offset as usize + k) {
                            *at = *x;
                        }
                    }
                }
                Cmd::Heaps { heaps } => heap = heaps.first().copied().unwrap_or(NONE),
                _ => {}
            }
        }
        say(format!("    constants {v:x?}"));
        let slot = |p: &Player, byte: u32| p.slot_at(heap, byte >> 5, submit);
        for (name, byte, n) in [
            ("y+0", v[9], 64u64),
            ("y+32 (matrices)", v[9] + 32, 64),
            ("y+64", v[9] + 64, 32),
            ("y+96 (instances)", v[9] + 96, 64),
            ("y+128 (meshlets)", v[9] + 128, 64),
            ("y+192 (visible)", v[9] + 192, 64),
            ("x+1600", v[8] + 1600, 0),
            ("x+2752", v[8] + 2752, 0),
            ("x+2848", v[8] + 2848, 0),
        ] {
            let s = slot(self, byte);
            let words = if s.kind == slot::SRV {
                self.view_words(&s, 0, n)?
            } else {
                Vec::new()
            };
            say(format!(
                "    {name}: slot {} {}: {words:x?}",
                byte >> 5,
                self.slot_text(&s)
            ));
        }
        // The list of visible meshlets, as the mesh shader finds it.
        let lists = slot(self, v[8] + 2752);
        let word = self.view_words(&lists, u64::from((v[13] & 0x07ff_ffff) & 0x07ff_ffff), 4)?;
        let typed = slot(self, v[8] + 1600);
        let index = self.view_words(&typed, 510, 1)?;
        say(format!(
            "    list words {word:x?} (count {}, first {}), typed view slot {index:x?}",
            word.first().map_or(0, |w| w >> 15),
            word.first().map_or(0, |w| w & 0x7fff)
        ));
        if let (Some(w), Some(i)) = (word.first(), index.first()) {
            let s = self.slot_at(heap, i & 0x07ff_ffff, submit);
            say(format!("    typed view: {}", self.slot_text(&s)));
            if s.kind == slot::SRV && s.res != NONE {
                let list =
                    self.view_words(&s, u64::from(w & 0x7fff) * 16, u64::from(w >> 15) + 16)?;
                say(format!("    visible meshlets: {list:x?}"));
            }
        }
        Ok(())
    }

    /// What the heap slots hold that a draw's constants name, as the title's
    /// shaders find them: byte offsets in the view heap among its first
    /// constants, a descriptor of 32 bytes each.
    pub fn slots(&self, list: u32, command: usize, say: &mut dyn FnMut(String)) -> Result<()> {
        let mut here = 0;
        for (i, s) in self.frame.submits.iter().enumerate() {
            for &l in s.lists.iter().filter(|&&l| l != MISSING) {
                if here == list {
                    let cmds = &self.commands[l as usize];
                    say(self.describe(cmds, command));
                    let mut values = [[0u32; 16]; 3];
                    let mut heap = NONE;
                    for c in &cmds[..command] {
                        match c {
                            Cmd::Constants {
                                param,
                                offset,
                                values: v,
                                ..
                            } => {
                                for (k, x) in v.iter().enumerate() {
                                    if let Some(all) = values.get_mut(*param as usize)
                                        && let Some(at) = all.get_mut(*offset as usize + k)
                                    {
                                        *at = *x;
                                    }
                                }
                            }
                            Cmd::Heaps { heaps } => heap = heaps.first().copied().unwrap_or(NONE),
                            _ => {}
                        }
                    }
                    say(format!("heap {heap}, constants {values:x?}"));
                    let mut bases: Vec<u32> = values
                        .iter()
                        .flatten()
                        .map(|v| v >> 5)
                        .filter(|v| *v > 64)
                        .collect();
                    bases.dedup();
                    for base in bases {
                        for k in 0..12 {
                            let s = self.slot_at(heap, base + k, i);
                            if s.kind != slot::EMPTY {
                                say(format!("  slot {base}+{k}: {}", self.slot_text(&s)));
                            } else {
                                say(format!("  slot {base}+{k}: never written"));
                            }
                        }
                    }
                }
                here += 1;
            }
        }
        Ok(())
    }

    /// A heap slot as it was for a submission.
    fn slot_at(&self, heap: u32, at: u32, submit: usize) -> Slot {
        let late = self.frame.submits[..=submit].iter().rev().find_map(|s| {
            s.views
                .iter()
                .rev()
                .find(|(h, i, _)| *h == heap && *i == at)
                .map(|v| v.2)
        });
        late.unwrap_or_else(|| {
            let runs = self
                .frame
                .descriptor_heaps
                .iter()
                .find(|h| h.id == heap)
                .map(|h| &h.runs);
            runs.and_then(|runs| {
                let k = runs.partition_point(|r| r.0 <= at).checked_sub(1)?;
                let (first, n, s) = &runs[k];
                (at - first < *n).then_some(*s)
            })
            .unwrap_or(Slot::EMPTY)
        })
    }

    fn slot_text(&self, s: &Slot) -> String {
        let word = |i: usize| {
            u32::from_le_bytes([
                s.desc[i * 4],
                s.desc[i * 4 + 1],
                s.desc[i * 4 + 2],
                s.desc[i * 4 + 3],
            ])
        };
        let kind = ["empty", "cbv", "srv", "uav", "sampler", "rtv", "dsv"]
            .get(s.kind as usize)
            .copied()
            .unwrap_or("?");
        let res = if s.res == NONE {
            "null".into()
        } else {
            self.called(s.res)
        };
        match s.kind {
            slot::CBV => format!("cbv {res} +{:#x} size {:#x}", s.address(), word(2)),
            _ => format!(
                "{kind} format {} dimension {} first {} count {} stride {} flags {}: {res}",
                word(0),
                word(1),
                word(if s.kind == slot::SRV { 4 } else { 2 }),
                word(if s.kind == slot::SRV { 6 } else { 4 }),
                word(if s.kind == slot::SRV { 7 } else { 5 }),
                word(8),
            ),
        }
    }

    pub(super) fn called(&self, id: u32) -> String {
        let Some(o) = self.objects.get(&id) else {
            return format!("resource {id} (not there)");
        };
        let r = &self.frame.resources[o.index];
        let d = &r.desc;
        if d.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
            let kind = match &r.place {
                Place::Committed { props, .. } => format!("{:?}", gpu::seen(props)),
                _ => "placed".into(),
            };
            format!("#{id} '{}' buffer {:#x} {kind}", r.name, d.width)
        } else {
            format!(
                "#{id} '{}' {}x{}x{} m{} f{}",
                r.name, d.width, d.height, d.depth, d.mips, d.format
            )
        }
    }

    /// Runs the unit until two runs differ, and says how.
    fn compare(
        &mut self,
        reader: &Reader,
        unit: &Unit,
        states: &States,
        pieces: &Pieces,
        wait: bool,
    ) -> Result<Vec<String>> {
        let after = self.after(unit, states);
        let parts: std::collections::BTreeSet<usize> = pieces.iter().map(|p| p.0).collect();
        let mut seen: Vec<(Vec<Vec<u64>>, Vec<Vec<u8>>)> = Vec::new();
        for run in 0..8 {
            if run > 0 {
                self.put_back_from(&after, states)?;
            }
            self.go(unit, wait)?;
            let hashes = self.hashes_now(reader, &after)?;
            if seen.iter().any(|(h, _)| *h == hashes) {
                continue;
            }
            let data = parts
                .iter()
                .map(|&p| reader.bytes(&reader.parts[p]).to_vec())
                .collect();
            seen.push((hashes, data));
            if seen.len() == 2 {
                break;
            }
        }
        let mut out = Vec::new();
        // The arguments it ran with.
        if let Some(Cmd::Indirect { args, at, .. }) = unit.cmds.get(unit.to - 1)
            && let Some((p, part)) = reader.parts.iter().enumerate().find(|(_, p)| p.id == *args)
        {
            let bytes = &reader.bytes(part)[*at as usize..];
            let words: Vec<u32> = bytes
                .chunks_exact(4)
                .take(8)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap_or_default()))
                .collect();
            let each: HashSet<&Vec<u64>> = seen.iter().map(|s| &s.0[p]).collect();
            out.push(format!(
                "arguments {words:x?} (the same in all runs: {})",
                each.len() == 1
            ));
        }
        self.at = after;
        let [(_, a), (_, b)] = &seen[..] else {
            out.push("it did not differ when looked at again".into());
            return Ok(out);
        };
        // And without the command.
        let before = Unit {
            to: unit.to - 1,
            ..*unit
        };
        self.put_back(states)?;
        self.go(&before, wait)?;
        self.at = self.after(&before, states);
        let now = self.at.clone();
        self.hashes_now(reader, &now)?;
        let without: Vec<Vec<u8>> = parts
            .iter()
            .map(|&p| reader.bytes(&reader.parts[p]).to_vec())
            .collect();
        let to = self.folder.join("diverge");
        let _ = std::fs::create_dir_all(&to);
        for (k, &p) in parts.iter().enumerate().take(12) {
            let part = &reader.parts[p];
            // Both contents, to look at.
            let sub = part.sub.map_or(0, |s| s.0);
            let _ = std::fs::write(to.join(format!("{}-{sub}-0.bin", part.id)), &without[k]);
            let changed = |v: &[u8]| {
                v.chunks_exact(4)
                    .zip(without[k].chunks_exact(4))
                    .filter(|(x, y)| x != y)
                    .count()
            };
            out.push(format!(
                "it changes {} dwords in one run, {} in another",
                changed(&a[k]),
                changed(&b[k])
            ));
            let _ = std::fs::write(to.join(format!("{}-{sub}-a.bin", part.id)), &a[k]);
            let _ = std::fs::write(to.join(format!("{}-{sub}-b.bin", part.id)), &b[k]);
            let words = |v: &[u8]| -> Vec<u32> {
                v.chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().unwrap_or_default()))
                    .collect()
            };
            let (x, y) = (words(&a[k]), words(&b[k]));
            let differ: Vec<usize> = (0..x.len().min(y.len()))
                .filter(|&i| x[i] != y[i])
                .collect();
            if differ.is_empty() {
                continue;
            }
            // The same words in another order: an order, not a value.
            let sorted = |v: &[u32]| {
                let mut s: Vec<u32> = differ.iter().map(|&i| v[i]).collect();
                s.sort_unstable();
                s
            };
            let order = if sorted(&x) == sorted(&y) {
                " (the same values in another order)"
            } else {
                ""
            };
            let sub = part.sub.map_or(String::new(), |(s, f)| {
                format!(" subresource {s} pitch {}", f.Footprint.RowPitch)
            });
            let sample: Vec<String> = differ
                .iter()
                .take(8)
                .map(|&i| format!("+{:#x}: {:08x} {:08x}", i * 4, x[i], y[i]))
                .collect();
            out.push(format!(
                "writes {}{sub}: {} of {} dwords differ, {:#x}..{:#x}{order}: {}",
                self.called(part.id),
                differ.len(),
                x.len(),
                differ[0] * 4,
                differ[differ.len() - 1] * 4 + 4,
                sample.join(", ")
            ));
        }
        if parts.len() > 12 {
            out.push(format!("and {} more parts", parts.len() - 12));
        }
        Ok(out)
    }
}

impl Reader {
    fn bytes(&self, p: &Part) -> &[u8] {
        // SAFETY: a part of a mapped block; copies into it have run.
        unsafe {
            std::slice::from_raw_parts(
                (self.blocks[p.block].1 + p.at as usize) as *const u8,
                p.len as usize,
            )
        }
    }
}
