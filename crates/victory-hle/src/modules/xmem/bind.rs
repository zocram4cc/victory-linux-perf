use super::*;

pub(super) fn on() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("VICTORY_ALIAS_RUNS").is_some_and(|s| s == "1") && unified());
    *ON
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode { Open, Replace, Close, Empty }

pub(super) fn mode(physical: &Physical, op: &Op) -> Option<Mode> {
    if op.at % PAGE != 0 || op.slice.as_ref().is_some_and(|s| s.section.um.is_none()) { return None; }
    let old = physical.views.range(..=op.at).next_back().filter(|(b, (n, _))| op.at < **b + n);
    match old {
        Some((&at, (PAGE, s))) if at == op.at && s.section.um.is_some() => Some(if op.slice.is_some() { Mode::Replace } else { Mode::Close }),
        None => Some(if op.slice.is_some() { Mode::Open } else { Mode::Empty }),
        _ => None,
    }
}

fn compatible(first: &Op, next: &Op, n: usize) -> bool {
    first.at.checked_add(n * PAGE) == Some(next.at) && match (&first.slice, &next.slice) {
        (Some(a), Some(b)) => Arc::ptr_eq(&a.section, &b.section) && a.index.checked_add(n) == Some(b.index),
        (None, None) => true,
        _ => false,
    }
}

fn release(at: usize) -> bool {
    unsafe { VirtualFree(at as *mut _, PAGE, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) != 0 }
}

fn run(physical: &mut Physical, ops: &[Op], mode: Mode) -> Result<(), bool> {
    if mode == Mode::Empty { return Ok(()); }
    let base = ops[0].at;
    let len = ops.len() * PAGE;
    if mode == Mode::Open {
        for (i, op) in ops.iter().enumerate() {
            let own = physical.holes.remove(&op.at) == Some(PAGE) && lean();
            if !reserve_alias(op.at, PAGE, own) {
                for p in &ops[..i] { if release(p.at) { physical.holes.insert(p.at, PAGE); } }
                return Err(true);
            }
        }
    } else {
        victory_gpu::title_memory_unmapping(base, len);
    }
    if let Some(first) = &ops[0].slice {
        if !victory_gpu::um::alias(first.section.um.unwrap(), (first.index * PAGE) as u64, len, base) {
            if mode == Mode::Open { for op in ops { if release(op.at) { physical.holes.insert(op.at, PAGE); } } }
            return Err(true);
        }
        for op in ops {
            let s = op.slice.as_ref().unwrap();
            let mut old = 0;
            if unsafe { VirtualProtect(op.at as *const _, PAGE, PAGE_READWRITE, &mut old) } == 0 {
                tracing::warn!(address = op.at, "unified page permissions could not be restored");
            }
            if let Some((n, old)) = physical.views.insert(op.at, (PAGE, s.clone())) { shown_mut().forget(op.at, n, &old); }
            shown_mut().note(op.at, PAGE, s);
            physical.holes.remove(&op.at);
        }
        victory_gpu::title_memory_remapped(base, len);
        for op in ops {
            let s = op.slice.as_ref().unwrap();
            victory_gpu::title_memory_view(op.at, PAGE, s.section.handle as usize, (s.index * PAGE) as u64);
        }
    } else {
        if !victory_gpu::um::unalias(base, len) { return Err(true); }
        let mut ok = true;
        for op in ops {
            if let Some((n, old)) = physical.views.remove(&op.at) { shown_mut().forget(op.at, n, &old); }
            if release(op.at) { physical.holes.insert(op.at, PAGE); } else { ok = false; }
        }
        if !ok { return Err(false); }
    }
    static CALLS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
    let pages = CALLS[0].fetch_add(ops.len() as u64, Ordering::Relaxed) + ops.len() as u64;
    if mode == Mode::Replace { CALLS[2].fetch_add(ops.len() as u64, Ordering::Relaxed); }
    let runs = CALLS[1].fetch_add(1, Ordering::Relaxed) + 1;
    if runs.is_power_of_two() { tracing::info!(pages, runs, replacements = CALLS[2].load(Ordering::Relaxed), "unified page binding runs"); }
    Ok(())
}

pub(super) fn apply(physical: &mut Physical, ops: Vec<Op>) -> bool {
    let mut ops = ops.into_iter().peekable();
    let mut ok = true;
    while let Some(first) = ops.next() {
        let Some(kind) = mode(physical, &first) else { ok &= super::change_pages(physical, vec![first]); continue; };
        let mut group = vec![first];
        while ops.peek().is_some_and(|p| mode(physical, p) == Some(kind) && compatible(&group[0], p, group.len())) { group.push(ops.next().unwrap()); }
        if let Err(retry) = run(physical, &group, kind) { ok &= retry && super::change_pages(physical, group); }
    }
    ok
}
