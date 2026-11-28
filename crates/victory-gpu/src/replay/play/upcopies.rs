//! The copies of the CPU's sides of card buffers up to them, looked at: do
//! they bring words older than what the card holds? And do the frame's
//! shaders write card buffers outside what the host layer had declared as
//! written (its compare passes cover that)?

use super::*;

/// As the host layer cuts buffers.
const PIECE: u64 = 64 << 10;

/// A copy up: submission, source, destination, offset, length.
type Up = (usize, u32, u32, u64, u64);

/// A word a copy brings that the card holds another value of: its offset,
/// what the card holds, what was last put there before the card wrote, what
/// is brought.
type Word = (u64, [u32; 3]);

impl Player {
    /// The copies up in a submission's lists.
    fn ups(&self, submit: usize, lists: &[Vec<Cmd>]) -> Vec<Up> {
        let mut out = Vec::new();
        for c in lists.iter().flatten() {
            if self.merged_copy(c).is_some()
                && let Cmd::CopyBuffer {
                    dst,
                    dst_at,
                    src,
                    len,
                    ..
                } = c
            {
                out.push((submit, *src, *dst, *dst_at, *len));
            }
        }
        out
    }

    /// Where pieces begin in a card buffer, and those the host layer's
    /// compares look at in a submission.
    fn declared(&self, lists: &[Vec<Cmd>]) -> HashMap<u32, (u64, HashSet<u64>)> {
        let compares = self.pipelines_named(COMPARE);
        let mut out: HashMap<u32, (u64, HashSet<u64>)> = HashMap::new();
        for cmds in lists {
            let (mut bound, mut constants, mut of) = (NONE, Vec::new(), NONE);
            for c in cmds {
                match c {
                    Cmd::Pipeline { pso } => bound = *pso,
                    Cmd::Constants {
                        graphics: false,
                        param: 0,
                        values,
                        ..
                    } => constants = values.clone(),
                    Cmd::RootView {
                        graphics: false,
                        param: 1,
                        va: Va::At { res, .. },
                        ..
                    } => of = *res,
                    Cmd::Dispatch { .. }
                        if compares.contains(&bound)
                            && constants.len() >= 6
                            && constants[5] != 0 =>
                    {
                        let e = out
                            .entry(of)
                            .or_insert((u64::from(constants[1]), HashSet::new()));
                        e.1.extend((constants[0]..constants[0] + constants[3]).map(u64::from));
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Bytes of buffers, as they are now.
    fn slices(&mut self, all: &[(ID3D12Resource, u64, u64)]) -> Result<Vec<Vec<u8>>> {
        let total: u64 = all.iter().map(|a| a.2).sum();
        if total == 0 {
            return Ok(vec![Vec::new(); all.len()]);
        }
        let (back, data) = gpu::buffer(
            &self.device,
            D3D12_HEAP_TYPE_READBACK,
            total,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        self.worker.run(|list| {
            let mut at = 0;
            for (res, from, len) in all {
                // SAFETY: an open list; live buffers, ranges inside them.
                unsafe { list.CopyBufferRegion(&back, at, res, *from, *len) };
                at += len;
            }
        })?;
        let mut at = 0usize;
        Ok(all
            .iter()
            .map(|(_, _, len)| {
                // SAFETY: the mapping; the copies into it have run.
                let bytes =
                    unsafe { std::slice::from_raw_parts(data.add(at), *len as usize) }.to_vec();
                at += *len as usize;
                bytes
            })
            .collect())
    }

    /// The heap slots that show a buffer, a few of each kind.
    fn views_of(&self, id: u32) -> Vec<String> {
        let mut out = Vec::new();
        let mut shown = |kind: u8, at: String| {
            let what = if kind == slot::UAV { "uav" } else { "srv" };
            if out.iter().filter(|l: &&String| l.starts_with(what)).count() < 6 {
                out.push(format!("{what} {at}"));
            }
        };
        for h in &self.frame.descriptor_heaps {
            for (first, n, s) in h.runs.iter().filter(|r| r.2.res == id) {
                shown(s.kind, format!("heap {} slot {first:#x} ({n})", h.id));
            }
        }
        for (i, s) in self.frame.submits.iter().enumerate() {
            for (heap, at, s) in s.views.iter().filter(|v| v.2.res == id) {
                shown(
                    s.kind,
                    format!("heap {heap} slot {at:#x} from submission {i} on"),
                );
            }
        }
        out
    }

    /// Runs the frame twice, submission by submission, and says what the
    /// copies up would have put over what the card wrote in both runs alike,
    /// and what was written undeclared.
    pub fn upcopies(&mut self, o: &Options, say: &mut dyn FnMut(String)) -> Result<()> {
        let mut report = Report::default();
        self.set_merging(true)?;
        let submits = self.edited(o, &mut report, &[]);
        let built = self.build(&submits, o)?;
        let ids: Vec<u32> = self
            .masters
            .iter()
            .map(|m| m.0)
            .filter(|id| {
                self.card_writes.contains(id)
                    && self.frame.resources[self.objects[id].index]
                        .name
                        .ends_with("on the card")
            })
            .collect();
        let reader = self.reader(&ids)?;
        say(format!(
            "{} buffers on the card that the frame writes, {} MB",
            ids.len(),
            reader.parts.iter().map(|p| p.len).sum::<u64>() >> 20
        ));
        // What a shader that picks a view to write through by an index could
        // write.
        let mut shown: BTreeMap<u32, u64> = BTreeMap::new();
        for s in self
            .frame
            .descriptor_heaps
            .iter()
            .flat_map(|h| h.runs.iter().map(|r| &r.2))
        {
            if s.kind == slot::UAV
                && let Some(o) = self.objects.get(&s.res)
            {
                let d = &self.frame.resources[o.index].desc;
                if d.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
                    *shown.entry(s.res).or_default() = d.width;
                }
            }
        }
        say(format!(
            "unordered views of {} buffers in the heaps at the start, {} MB of them",
            shown.len(),
            shown.values().sum::<u64>() >> 20
        ));
        let states = self.start_states();
        let mut runs: Vec<Vec<(Up, Vec<Word>)>> = Vec::new();
        for run in 0..2 {
            self.restore(true)?;
            self.fresh = false;
            let mut found: Vec<(Up, Vec<Word>)> = Vec::new();
            let mut was = if run == 0 {
                self.hashes_now(&reader, &states)?
            } else {
                Vec::new()
            };
            for (i, (delta, lists)) in built.steps.iter().enumerate() {
                if let Some(d) = delta {
                    // SAFETY: a closed list, waited for.
                    unsafe {
                        self.worker
                            .queue
                            .ExecuteCommandLists(&[Some(d.list.cast()?)])
                    };
                    self.worker.idle()?;
                }
                let ups = self.ups(i, &submits[i]);
                if !ups.is_empty() {
                    let mut asked = Vec::new();
                    for &(_, src, dst, at, len) in &ups {
                        let Some(base) = self.merger.as_ref().and_then(|m| m.bases.get(&dst))
                        else {
                            bail!("a copy up without a base");
                        };
                        asked.push((self.objects[&dst].res.clone(), at, len));
                        asked.push((base.clone(), at, len));
                        asked.push((self.objects[&src].res.clone(), at, len));
                    }
                    let bytes = self.slices(&asked)?;
                    for (k, up) in ups.iter().enumerate() {
                        let (now, base, new) =
                            (&bytes[k * 3], &bytes[k * 3 + 1], &bytes[k * 3 + 2]);
                        let word = |b: &[u8], i: usize| {
                            u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
                        };
                        let mut words = Vec::new();
                        for at in (0..now.len() / 4 * 4).step_by(4) {
                            let w = [word(now, at), word(base, at), word(new, at)];
                            if w[0] != w[2] {
                                words.push((up.3 + at as u64, w));
                            }
                        }
                        found.push((*up, words));
                    }
                }
                let all: Vec<Option<ID3D12CommandList>> = lists
                    .iter()
                    .map(|l| l.list.cast::<ID3D12CommandList>().map(Some))
                    .collect::<std::result::Result<_, _>>()?;
                if !all.is_empty() {
                    // SAFETY: closed lists, waited for.
                    unsafe { self.worker.queue.ExecuteCommandLists(&all) };
                    self.worker.idle().context("the card was lost")?;
                }
                if run > 0 {
                    continue;
                }
                let now = self.hashes_now(&reader, &states)?;
                let mut declared = self.declared(&submits[i]);
                for (_, _, dst, at, len) in &ups {
                    let e = declared.entry(*dst).or_default();
                    let skew = e.0;
                    e.1.extend((*at + skew) / PIECE..(*at + len + skew).div_ceil(PIECE));
                }
                for (p, (a, b)) in was.iter().zip(&now).enumerate() {
                    let id = reader.parts[p].id;
                    let differ: Vec<usize> = (0..a.len()).filter(|&k| a[k] != b[k]).collect();
                    if differ.is_empty() {
                        continue;
                    }
                    let outside: Vec<u64> = differ
                        .iter()
                        .map(|&k| k as u64)
                        .filter(|&k| match declared.get(&id) {
                            None => true,
                            // (A copy that does not begin on a piece: the
                            // hashes' piece lies over two of the compare's.)
                            Some((skew, pieces)) => {
                                let first = (k * PIECE + skew) / PIECE;
                                let last = ((k + 1) * PIECE + skew - 1) / PIECE;
                                !(first..=last).any(|p| pieces.contains(&p))
                            }
                        })
                        .collect();
                    if !outside.is_empty() {
                        say(format!(
                            "submission {i}: {} of {} changed pieces of {} were not declared as written, the first {:#x?}",
                            outside.len(),
                            differ.len(),
                            self.called(id),
                            &outside[..outside.len().min(8)]
                        ));
                        for line in self.views_of(id) {
                            say(format!("    {line}"));
                        }
                    }
                }
                was = now;
            }
            runs.push(found);
        }
        let [first, second] = &runs[..] else {
            bail!("not two runs");
        };
        // What the card wrote in both runs alike, and what comes is another
        // value: the title's, or an old one.
        let (mut same, mut old, mut differ, mut all, mut titles) = (0u64, 0u64, 0u64, 0u64, 0u64);
        let mut by: BTreeMap<(u32, u64), (u64, u64, Vec<String>)> = BTreeMap::new();
        for ((up, a), (_, b)) in first.iter().zip(second) {
            all += up.4 / 4;
            let b: HashMap<u64, [u32; 3]> = b.iter().copied().collect();
            for (at, [now, base, new]) in a {
                if now == base {
                    titles += 1;
                    continue;
                }
                if b.get(at).is_none_or(|other| other[0] != *now) {
                    differ += 1;
                    continue;
                }
                same += 1;
                let back = new == base;
                old += u64::from(back);
                let e = by.entry((up.2, at / PIECE)).or_default();
                e.0 += 1;
                e.1 += u64::from(back);
                if e.2.len() < 6 {
                    e.2.push(format!(
                        "submission {} +{at:#x}: card {now:#x}, before {base:#x}, comes {new:#x}",
                        up.0
                    ));
                }
            }
        }
        say(format!(
            "{} copies up of {all} words: {titles} bring something new where the card had not written; where it had and another \
             value comes: {differ} words differ between the runs, {same} the card wrote alike in both ({old} of them would get \
             back what stood there before the card wrote)",
            first.len(),
        ));
        let mut by: Vec<_> = by.into_iter().collect();
        by.sort_by_key(|b| std::cmp::Reverse(b.1.0));
        for ((id, piece), (n, back, some)) in by.into_iter().take(40) {
            say(format!(
                "  {} piece {piece:#x}: {n} words, {back} back to before",
                self.called(id)
            ));
            for line in some {
                say(format!("      {line}"));
            }
        }
        Ok(())
    }
}
