//! Control-flow structurisation (decision 0003): the program as one lane
//! runs it, without EXEC.
//!
//! Lane masks (EXEC, compare and carry results, the SGPRs they are saved
//! in) become per-lane booleans ([`Bools`]). A branch on a wave-wide test of
//! a mask (`s_cbranch_execz`, `vccz`, SCC of a mask operation) becomes a
//! branch on the lane's own bit: compiled code only skips work that lanes
//! outside the mask would not change, or tests a value that is the same in
//! every lane, so either way each lane can decide alone. A path-sensitive
//! pass then proves, for each vector instruction, that EXEC holds in every
//! lane that reaches it; what it cannot prove stays as the instruction's
//! guard, a per-lane `if` around it.
//!
//! The control-flow graph keeps its shape and must be reducible (DXIL's
//! only requirement). Helper lanes of a pixel shader count as live: on PC
//! their side effects are dropped too, and what they compute is only read
//! through derivatives.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use crate::ShaderError;
use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{EXEC, Exit, Inst, Program, Reg, VCC, Value};

/// A per-lane boolean: a node of [`Bools`]. Equal functions are equal nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bool(u32);

impl Bool {
    pub const FALSE: Bool = Bool(0);
    pub const TRUE: Bool = Bool(1);
}

/// What a variable of [`Bools`] stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Atom {
    /// The lane's bit of the compare or carry result in this VALU-written
    /// value, before EXEC masking.
    Lane(Value),
    /// An SCC value, the same in every lane.
    Uniform(Value),
    /// A lane mask joined at a block entry, by its low dword's phi.
    Phi(Value),
    /// `s_wqm` result: some lane of the quad is in the source mask.
    Quad(Value),
    /// A mask the pass cannot see into (loaded, built from scalars, an
    /// EXEC write it does not follow): the lane's bit of the register pair
    /// `(low, high)` (both equal in wave32).
    Opaque(Value, Value),
    /// A constant mask other than all lanes or none.
    Constant(u64),
}

/// Reduced ordered binary decision diagrams over [`Atom`]s.
#[derive(Debug, Clone)]
pub struct Bools {
    /// (variable, low, high); nodes 0 and 1 are the constants.
    nodes: Vec<(u32, Bool, Bool)>,
    unique: Map<(u32, Bool, Bool), Bool>,
    memo: Map<(Bool, Bool, Bool), Bool>,
    atoms: Vec<Atom>,
    /// Reverse-postorder index of the block that defines each atom.
    born: Vec<usize>,
    ids: Map<Atom, u32>,
    gc_at: usize,
    overflow: bool,
}

/// The tables below have small integer keys; SipHash dominated run time.
type Map<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

/// FxHash's multiply-rotate step.
#[derive(Default)]
struct Fx(u64);

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

const TERMINAL: u32 = u32::MAX;
const MAX_NODES: usize = 1 << 21;
/// Case splits [`Bools::meet`] makes before it gives up.
const MAX_MEET: usize = 1 << 16;
/// Conditions [`Meets`] keeps; past that, they are searched anew.
const MAX_MEETS: usize = 1 << 18;

/// What [`Bools::meet`] found out about one [`Bools`], for its later calls:
/// the conditions no lane satisfies, and the case splits it took to find
/// that out of each.
#[derive(Debug, Default)]
pub struct Meets {
    none: Map<Box<[u32]>, usize>,
}

impl Bools {
    fn new() -> Self {
        Bools {
            nodes: vec![
                (TERMINAL, Bool::FALSE, Bool::FALSE),
                (TERMINAL, Bool::TRUE, Bool::TRUE),
            ],
            unique: Map::default(),
            memo: Map::default(),
            atoms: Vec::new(),
            born: Vec::new(),
            ids: Map::default(),
            gc_at: MAX_NODES / 2,
            overflow: false,
        }
    }

    fn should_gc(&self) -> bool {
        !self.overflow && self.nodes.len() >= self.gc_at
    }

    /// Rebuilds the node table keeping only nodes reachable from `live`,
    /// preserving all atom ids and returning the old-to-new [`Bool`] map.
    fn gc_remap(&mut self, live: &[Bool]) -> Vec<Bool> {
        let len = self.nodes.len();
        let mut keep = vec![false; len];
        keep[0] = true;
        keep[1] = true;
        let mut stack = Vec::with_capacity(live.len());
        for &b in live {
            let idx = b.0 as usize;
            if idx < len && !keep[idx] {
                keep[idx] = true;
                stack.push(idx);
            }
        }
        while let Some(idx) = stack.pop() {
            let (v, lo, hi) = self.nodes[idx];
            if v == TERMINAL {
                continue;
            }
            for child in [lo.0 as usize, hi.0 as usize] {
                if !keep[child] {
                    keep[child] = true;
                    stack.push(child);
                }
            }
        }
        let mut remap = vec![Bool::FALSE; len];
        remap[0] = Bool::FALSE;
        remap[1] = Bool::TRUE;
        let mut new_nodes = Vec::with_capacity(len.min(self.gc_at));
        new_nodes.push(self.nodes[0]);
        new_nodes.push(self.nodes[1]);
        let mut new_unique = Map::default();
        for i in 2..len {
            if !keep[i] {
                continue;
            }
            let (v, lo, hi) = self.nodes[i];
            let lo = remap[lo.0 as usize];
            let hi = remap[hi.0 as usize];
            let nb = Bool(new_nodes.len() as u32);
            new_nodes.push((v, lo, hi));
            new_unique.insert((v, lo, hi), nb);
            remap[i] = nb;
        }
        self.nodes = new_nodes;
        self.unique = new_unique;
        self.memo.clear();
        self.gc_at = if self.nodes.len() >= MAX_NODES * 7 / 8 {
            usize::MAX
        } else {
            (self.nodes.len() * 2)
                .max(MAX_NODES / 4)
                .min(MAX_NODES - 65_536)
        };
        remap
    }

    /// The atom tested at the top of `b` and the results when it is false
    /// and true; `None` for the constants.
    pub fn node(&self, b: Bool) -> Option<(Atom, Bool, Bool)> {
        let (v, lo, hi) = self.nodes[b.0 as usize];
        (v != TERMINAL).then(|| (self.atoms[v as usize], lo, hi))
    }

    /// The atoms `b` depends on, in variable order.
    pub fn support(&self, b: Bool) -> Vec<Atom> {
        let mut vars = self.vars(b);
        vars.sort_unstable();
        vars.into_iter().map(|v| self.atoms[v as usize]).collect()
    }

    fn vars(&self, b: Bool) -> Vec<u32> {
        let mut seen: Map<Bool, ()> = Map::default();
        let mut has: Map<u32, ()> = Map::default();
        let mut vars = Vec::new();
        let mut stack = vec![b];
        while let Some(b) = stack.pop() {
            let (v, lo, hi) = self.nodes[b.0 as usize];
            if v == TERMINAL || seen.insert(b, ()).is_some() {
                continue;
            }
            if has.insert(v, ()).is_none() {
                vars.push(v);
            }
            stack.extend([lo, hi]);
        }
        vars
    }

    fn atom(&mut self, atom: Atom, born: usize) -> Bool {
        let v = *self.ids.entry(atom).or_insert_with(|| {
            self.atoms.push(atom);
            self.born.push(born);
            (self.atoms.len() - 1) as u32
        });
        self.mk(v, Bool::FALSE, Bool::TRUE)
    }

    fn mk(&mut self, v: u32, lo: Bool, hi: Bool) -> Bool {
        if lo == hi {
            return lo;
        }
        if let Some(&b) = self.unique.get(&(v, lo, hi)) {
            return b;
        }
        if self.nodes.len() >= MAX_NODES {
            // The caller fails once it sees `overflow`.
            self.overflow = true;
            return Bool::FALSE;
        }
        let b = Bool(self.nodes.len() as u32);
        self.nodes.push((v, lo, hi));
        self.unique.insert((v, lo, hi), b);
        b
    }

    /// Cofactors of `b` for `v`, which is `b`'s top variable or above it.
    fn split(&self, b: Bool, v: u32) -> (Bool, Bool) {
        match self.nodes[b.0 as usize] {
            (bv, lo, hi) if bv == v => (lo, hi),
            _ => (b, b),
        }
    }

    fn ite(&mut self, f: Bool, g: Bool, h: Bool) -> Bool {
        if self.overflow {
            return Bool::FALSE;
        }
        if f == Bool::TRUE || g == h {
            return g;
        }
        if f == Bool::FALSE {
            return h;
        }
        if (g, h) == (Bool::TRUE, Bool::FALSE) {
            return f;
        }
        if let Some(&r) = self.memo.get(&(f, g, h)) {
            return r;
        }
        let top = |b: Bool| self.nodes[b.0 as usize].0;
        let v = top(f).min(top(g)).min(top(h));
        let ((f0, f1), (g0, g1), (h0, h1)) = (self.split(f, v), self.split(g, v), self.split(h, v));
        let lo = self.ite(f0, g0, h0);
        let hi = self.ite(f1, g1, h1);
        let r = self.mk(v, lo, hi);
        if !self.overflow {
            self.memo.insert((f, g, h), r);
        }
        r
    }

    fn and(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(a, b, Bool::FALSE)
    }

    fn or(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(a, Bool::TRUE, b)
    }

    fn not(&mut self, a: Bool) -> Bool {
        self.ite(a, Bool::FALSE, Bool::TRUE)
    }

    fn andn2(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(b, Bool::FALSE, a)
    }

    fn andn1(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(a, Bool::FALSE, b)
    }

    fn orn2(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(b, a, Bool::TRUE)
    }

    fn orn1(&mut self, a: Bool, b: Bool) -> Bool {
        self.ite(a, b, Bool::TRUE)
    }

    fn nand(&mut self, a: Bool, b: Bool) -> Bool {
        let nb = self.not(b);
        self.ite(a, nb, Bool::TRUE)
    }

    fn nor(&mut self, a: Bool, b: Bool) -> Bool {
        let nb = self.not(b);
        self.ite(a, Bool::FALSE, nb)
    }

    fn xor(&mut self, a: Bool, b: Bool) -> Bool {
        let nb = self.not(b);
        self.ite(a, nb, b)
    }

    fn xnor(&mut self, a: Bool, b: Bool) -> Bool {
        let nb = self.not(b);
        self.ite(a, b, nb)
    }

    /// Whether some lane can satisfy every `(b, holds)` of `all` at once:
    /// `b` where `holds`, its negation where not. Yes when the search grows
    /// too large to finish.
    pub fn meet(&self, all: &[(Bool, bool)]) -> bool {
        self.meet_seen(all, &mut Meets::default())
    }

    /// [`Self::meet`], which looks up in `seen` what earlier calls on these
    /// `Bools` found out and adds what it finds. The answer is the one
    /// `meet` gives.
    pub fn meet_seen(&self, all: &[(Bool, bool)], seen: &mut Meets) -> bool {
        self.meet_within(all, MAX_MEET, seen)
    }

    /// [`Self::meet_seen`] with `budget` case splits.
    fn meet_within(&self, all: &[(Bool, bool)], mut budget: usize, seen: &mut Meets) -> bool {
        let mut work = all
            .iter()
            .map(|&(b, holds)| b.0 << 1 | u32::from(holds))
            .collect();
        self.meet_in(0, &mut work, &mut budget, seen)
    }

    /// The search of [`Self::meet`] for the conditions `work[at..]` (a node
    /// doubled, plus one where it must hold), which it takes off `work`.
    ///
    /// The search is a case split on the top variable, and the cofactors of
    /// a few large functions come down to the same conditions over and over
    /// (a block's reach under a mask and under its negation share most of
    /// their nodes), so splitting each anew is exponential where the
    /// distinct conditions are few. `seen` keeps the conditions that no lane
    /// satisfies with the case splits it took to find that out: meeting
    /// them again takes those splits off the budget at once. A search
    /// therefore gives up exactly where one that split everything anew
    /// would (nothing below such conditions ends it early, so it either
    /// makes all those splits or runs out among them), and gives the same
    /// answer.
    fn meet_in(
        &self,
        at: usize,
        work: &mut Vec<u32>,
        budget: &mut usize,
        seen: &mut Meets,
    ) -> bool {
        // Constants either fail or say nothing.
        let mut kept = at;
        for i in at..work.len() {
            let (b, holds) = (work[i] >> 1, work[i] & 1 != 0);
            if b > Bool::TRUE.0 {
                work[kept] = work[i];
                kept += 1;
            } else if (b == Bool::TRUE.0) != holds {
                work.truncate(at);
                return false;
            }
        }
        work.truncate(kept);
        if kept == at || *budget == 0 {
            work.truncate(at);
            return true;
        }
        // As a set: neither order nor repeats change the search.
        work[at..].sort_unstable();
        let mut end = at + 1;
        for i in at + 1..kept {
            if work[i] != work[end - 1] {
                work[end] = work[i];
                end += 1;
            }
        }
        work.truncate(end);
        if let Some(&splits) = seen.none.get(&work[at..]) {
            let within = splits <= *budget;
            *budget = budget.saturating_sub(splits);
            work.truncate(at);
            return !within;
        }
        let before = *budget;
        *budget -= 1;
        let top = |code: u32| self.nodes[(code >> 1) as usize].0;
        let v = work[at..].iter().map(|&c| top(c)).min().expect("not empty");
        let mut found = false;
        for value in [false, true] {
            for i in at..end {
                let (lo, hi) = self.split(Bool(work[i] >> 1), v);
                work.push((if value { hi } else { lo }).0 << 1 | work[i] & 1);
            }
            if self.meet_in(end, work, budget, seen) {
                found = true;
                break;
            }
        }
        if !found && seen.none.len() < MAX_MEETS {
            seen.none.insert(work[at..end].into(), before - *budget);
        }
        work.truncate(at);
        found
    }

    /// Whether `a => b` for every variable assignment, allocating no nodes.
    fn implies(&self, a: Bool, b: Bool) -> bool {
        self.implies_in(a, b, &mut Map::default())
    }

    fn implies_in(&self, a: Bool, b: Bool, memo: &mut Map<(Bool, Bool), bool>) -> bool {
        if a == Bool::FALSE || b == Bool::TRUE || a == b {
            return true;
        }
        if a == Bool::TRUE || b == Bool::FALSE {
            return false;
        }
        if let Some(&r) = memo.get(&(a, b)) {
            return r;
        }
        let v = self.nodes[a.0 as usize].0.min(self.nodes[b.0 as usize].0);
        let (a0, a1) = self.split(a, v);
        let (b0, b1) = self.split(b, v);
        let r = self.implies_in(a0, b0, memo) && self.implies_in(a1, b1, memo);
        memo.insert((a, b), r);
        r
    }

    /// Whether `a & b == FALSE` (equivalently `a => !b`), allocating no nodes.
    fn disjoint(&self, a: Bool, b: Bool) -> bool {
        self.disjoint_in(a, b, &mut Map::default())
    }

    fn disjoint_in(&self, a: Bool, b: Bool, memo: &mut Map<(Bool, Bool), bool>) -> bool {
        if a == Bool::FALSE || b == Bool::FALSE {
            return true;
        }
        if a == Bool::TRUE || b == Bool::TRUE || a == b {
            return false;
        }
        let key = if a.0 <= b.0 { (a, b) } else { (b, a) };
        if let Some(&r) = memo.get(&key) {
            return r;
        }
        let v = self.nodes[a.0 as usize].0.min(self.nodes[b.0 as usize].0);
        let (a0, a1) = self.split(a, v);
        let (b0, b1) = self.split(b, v);
        let r = self.disjoint_in(a0, b0, memo) && self.disjoint_in(a1, b1, memo);
        memo.insert(key, r);
        r
    }

    /// Whether some satisfying assignment of `known` sets variable `v` to `val`.
    fn can_be(&self, known: Bool, v: u32, val: bool) -> bool {
        self.can_be_in(known, v, val, &mut Map::default())
    }

    fn can_be_in(&self, node: Bool, v: u32, val: bool, seen: &mut Map<Bool, ()>) -> bool {
        if node == Bool::FALSE {
            return false;
        }
        if node == Bool::TRUE {
            return true;
        }
        let (nv, lo, hi) = self.nodes[node.0 as usize];
        if nv > v {
            return true;
        }
        if nv == v {
            return (if val { hi } else { lo }) != Bool::FALSE;
        }
        if seen.insert(node, ()).is_some() {
            return false;
        }
        self.can_be_in(lo, v, val, seen) || self.can_be_in(hi, v, val, seen)
    }

    /// `b` with every atom born at reverse-postorder index `from` or later
    /// existentially quantified: what still holds once those are redefined.
    fn forget(&mut self, b: Bool, from: usize, memo: &mut Map<Bool, Bool>) -> Bool {
        if self.overflow {
            return Bool::FALSE;
        }
        let (v, lo, hi) = self.nodes[b.0 as usize];
        if v == TERMINAL {
            return b;
        }
        if let Some(&r) = memo.get(&b) {
            return r;
        }
        let lo = self.forget(lo, from, memo);
        let hi = self.forget(hi, from, memo);
        let r = if self.born[v as usize] >= from {
            self.or(lo, hi)
        } else {
            self.mk(v, lo, hi)
        };
        if !self.overflow {
            memo.insert(b, r);
        }
        r
    }

    fn restrict_many(
        &mut self,
        b: Bool,
        assign: &Map<u32, bool>,
        max_var: u32,
        memo: &mut Map<Bool, Bool>,
    ) -> Bool {
        if self.overflow {
            return Bool::FALSE;
        }
        let (bv, lo, hi) = self.nodes[b.0 as usize];
        if bv == TERMINAL || bv > max_var {
            return b;
        }
        if let Some(&r) = memo.get(&b) {
            return r;
        }
        let r = if let Some(&value) = assign.get(&bv) {
            let branch = if value { hi } else { lo };
            self.restrict_many(branch, assign, max_var, memo)
        } else {
            let l = self.restrict_many(lo, assign, max_var, memo);
            let h = self.restrict_many(hi, assign, max_var, memo);
            self.mk(bv, l, h)
        };
        if !self.overflow {
            memo.insert(b, r);
        }
        r
    }

    /// `b` simplified under `known`: returns constants immediately when
    /// `known => b` or `known => !b`, otherwise substitutes every atom
    /// forced by `known` in one pass.
    fn given(&mut self, b: Bool, known: Bool) -> Bool {
        if b == Bool::FALSE || b == Bool::TRUE || known == Bool::TRUE {
            return b;
        }
        if self.implies(known, b) {
            return Bool::TRUE;
        }
        if self.disjoint(known, b) {
            return Bool::FALSE;
        }
        let mut assign = Map::default();
        let mut max_var = 0;
        for v in self.vars(b) {
            if !self.can_be(known, v, false) {
                assign.insert(v, true);
                max_var = max_var.max(v);
            } else if !self.can_be(known, v, true) {
                assign.insert(v, false);
                max_var = max_var.max(v);
            }
        }
        if assign.is_empty() {
            return b;
        }
        self.restrict_many(b, &assign, max_var, &mut Map::default())
    }
}

/// How a lane leaves a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneExit {
    Jump(usize),
    /// To `taken` in lanes where `when` holds. A condition over
    /// [`Atom::Uniform`]s only is a uniform branch.
    Branch {
        when: Bool,
        taken: usize,
        not_taken: usize,
    },
    End,
    Indirect,
}

/// A [`Program`] as one lane runs it: same blocks, per-lane branches.
#[derive(Debug, Clone)]
pub struct Structured {
    pub bools: Bools,
    /// Per block.
    pub exits: Vec<LaneExit>,
    /// Per block and instruction: the condition, beyond reaching it, under
    /// which a lane runs a vector instruction; `None` when reaching it is
    /// enough (lanes outside EXEC then never get there, so a VGPR write
    /// needs no old value). Always `None` for scalar instructions.
    pub guards: Vec<Vec<Option<Bool>>>,
    /// Per block: what holds in every lane that enters it (over forward
    /// edges; a loop header forgets what its loop redefines).
    pub reach: Vec<Bool>,
    /// The lane's bit of each lane-mask value, by its low dword's value.
    pub masks: HashMap<Value, Bool>,
    /// Exact Wave64 pairs for checking masks at their uses.
    paired_masks: Map<Key, Bool>,
    /// Wave64 masks: low dword's value by the high dword's.
    pub highs: HashMap<Value, Value>,
    /// Masks (low dwords) set only in lanes whose EXEC bit is set where
    /// they are written: every such lane reaches the writing block, so a
    /// ballot there gives the mask as data. No split low dword, as in
    /// `masks`.
    pub active: HashSet<Value>,
    /// Incoming lane boolean per predecessor edge (in block predecessor order)
    /// for each [`Atom::Phi`] id.
    pub phi_masks: HashMap<Value, Vec<Bool>>,
    /// Blocks whose closing branch the whole wave takes together ([`structure_wide`]): its
    /// `when` is still the lane's own bit; the wave goes by all of them.
    pub wide: HashSet<usize>,
    /// Per block closed by a branch on a lane mask: whether the console takes it when some
    /// lane's `when` holds (`true`) or when every lane's does (`false`).
    pub wave_any: Vec<Option<bool>>,
    /// SCC values a mask operation sets when some lane's bit of its result is set, with that
    /// lane bit: as data such an SCC is "some lane of the wave has it".
    pub scc_some: HashMap<Value, Bool>,
    /// Natural loop bodies containing a divergent branch whose region reaches
    /// the loop header (empty if the CFG has an indirect exit).
    pub(crate) div_loops: Vec<Vec<bool>>,
}

impl Structured {
    pub(crate) fn mask_bits(&self, low: Value, high: Option<Value>) -> Option<Bool> {
        match high {
            Some(high) => self.paired_masks.get(&(low, high)),
            None => self.masks.get(&low),
        }
        .copied()
    }
}

/// Builds the per-lane form of `program`.
pub fn structure(program: &Program) -> Result<Structured, ShaderError> {
    structure_wide(program, &HashSet::new())
}

/// [`structure`], with the branches that close the blocks of `wide` kept as the console has
/// them: one decision for the whole wave (`s_cbranch_execz` and `vccz`: taken when no lane's
/// bit is set; `execnz` and `vccnz`: when some lane's is; SCC of a mask operation likewise,
/// [`Structured::wave_any`]), which every lane follows. A lane
/// then learns nothing about its own bit from the way it went, so past such a branch EXEC
/// stays a guard on every vector instruction, as it is on the console. For a console wave
/// run as two host waves, which exchange that decision and can then meet (a group barrier)
/// inside the branch.
pub fn structure_wide(program: &Program, wide: &HashSet<usize>) -> Result<Structured, ShaderError> {
    let mut l = Lanes::new(program)?;
    l.wide = wide.clone();
    loop {
        let mut execs = l.masks_in_order();
        let mut cond = l.paths(&mut execs)?;
        let (wrong, new_derived) = l.check_joins(&cond);
        if l.b.overflow {
            return Err(TOO_LARGE);
        }
        if wrong.is_empty() && new_derived.is_empty() {
            if l.b.should_gc() {
                l.gc(&mut execs, &mut cond);
            }
            return l.finish(&execs, &cond);
        }
        l.unresolved.extend(wrong);
        l.derived_phis.extend(new_derived);
        l.reset();
    }
}

/// A lane mask by the values of its low and high dword (both the low one in
/// wave32).
type Key = (Value, Value);

struct Lanes<'a> {
    p: &'a Program,
    order: Vec<usize>,
    /// Reverse-postorder index per block.
    index: Vec<usize>,
    /// Immediate dominator per reachable block (`idom[entry] == Some(entry)`).
    idom: Vec<Option<usize>>,
    /// Per block, in predecessor order: (block, over its taken edge).
    incoming: Vec<Vec<(usize, bool)>>,
    /// Block and instruction index writing each value.
    defs: Map<Value, (usize, usize)>,
    /// Block and index of each phi value.
    phis: Map<Value, (usize, usize)>,
    /// Mask joins found wrong when resolved to a constant: atoms instead.
    unresolved: HashSet<Value>,
    /// Phi ids whose joins depend on EXEC or VALU results (including along
    /// back edges), carried across resets so their joins stay in `masks`.
    derived_phis: HashSet<Value>,
    b: Bools,
    /// Masks derived from EXEC and VALU results.
    masks: Map<Key, Bool>,
    /// Other values read as masks: constants, uniform selects, opaque ones.
    other: Map<Key, Bool>,
    /// Pairs read as masks that `mask` does not see: carry-ins, and pairs
    /// joins take over back edges.
    seen: Vec<Key>,
    /// Keys currently being expanded in `mask` (guards split-half recursion).
    visiting: Vec<Key>,
    /// For branches: SCC of a mask operation ("some lane is set") as the
    /// lane's own bit.
    scc_lane: Map<Value, Bool>,
    /// Joins resolved to a constant, to check once all edges are known.
    joins: Vec<Join>,
    /// [`Atom::Phi`] joins created in the current pass, resolved in
    /// `check_joins` and exported in `finish`.
    phi_joins: Vec<PhiJoin>,
    phi_join_ids: HashSet<Value>,
    /// Partner `Key` associated with each `Atom::Phi(id)` so two pairs sharing
    /// one phi dword with different partners never alias the same atom.
    phi_join_keys: Map<Value, Key>,
    /// Per block: what a lane knows on entry, over forward edges only.
    first: Vec<Bool>,
    /// Per block ending in a branch: when a lane takes it.
    when: Vec<Option<Bool>>,
    /// See [`Structured::wide`].
    wide: HashSet<usize>,
    /// SCC values of [`Self::scc_lane`] that are set when EVERY lane's bit is (`s_cmp_eq` of
    /// two masks: "no lane differs"); the others are set when some lane's is.
    scc_every: HashSet<Value>,
    /// See [`Structured::wave_any`].
    wave_any: Vec<Option<bool>>,
}

/// A mask join taken as `value` in every lane that reaches `block`: each
/// incoming mask held (or failed) on its edge.
struct Join {
    block: usize,
    /// The phi value naming the join.
    id: Value,
    value: bool,
    derived: bool,
    args: Vec<Key>,
}

/// A mask join represented as [`Atom::Phi`], whose incoming edge masks are
/// resolved once every block (including loop back edges) has been visited.
struct PhiJoin {
    block: usize,
    id: Value,
    derived: bool,
    args: Vec<Key>,
}

struct SopMasks {
    dst: Option<(Key, Bool)>,
    exec: Option<(Key, Bool)>,
    /// Some operand derives from EXEC or a VALU result.
    derived: bool,
    /// For branches: SCC ("result is not zero") per lane.
    scc: Option<Bool>,
}

const MAX_ROUNDS: usize = 256;

impl<'a> Lanes<'a> {
    fn new(p: &'a Program) -> Result<Self, ShaderError> {
        let blocks = &p.blocks;
        let order = crate::ir::reverse_postorder(blocks);
        let mut index = vec![usize::MAX; blocks.len()];
        for (i, &b) in order.iter().enumerate() {
            index[b] = i;
        }
        check_reducible(p, &order, &index)?;
        let mut incoming = vec![Vec::new(); blocks.len()];
        let (mut defs, mut phis) = (Map::default(), Map::default());
        for (b, block) in blocks.iter().enumerate() {
            for (k, s) in block.exit.successors().into_iter().enumerate() {
                incoming[s].push((b, k == 0));
            }
            for (i, phi) in block.phis.iter().enumerate() {
                phis.insert(phi.value, (b, i));
            }
            for (i, inst) in block.insts.iter().enumerate() {
                for &(_, v) in &inst.writes {
                    defs.insert(v, (b, i));
                }
            }
        }
        let idom = compute_idoms(&order, &index, &incoming);
        Ok(Lanes {
            p,
            order,
            index,
            idom,
            incoming,
            defs,
            phis,
            unresolved: HashSet::new(),
            derived_phis: HashSet::new(),
            b: Bools::new(),
            masks: Map::default(),
            other: Map::default(),
            seen: Vec::new(),
            visiting: Vec::new(),
            scc_lane: Map::default(),
            joins: Vec::new(),
            phi_joins: Vec::new(),
            phi_join_ids: HashSet::new(),
            phi_join_keys: Map::default(),
            first: vec![Bool::FALSE; blocks.len()],
            when: vec![None; blocks.len()],
            wide: HashSet::new(),
            scc_every: HashSet::new(),
            wave_any: vec![None; blocks.len()],
        })
    }

    fn reset(&mut self) {
        self.b = Bools::new();
        self.masks.clear();
        self.other.clear();
        self.seen.clear();
        self.visiting.clear();
        self.scc_lane.clear();
        self.scc_every.clear();
        self.wave_any.fill(None);
        self.joins.clear();
        self.phi_joins.clear();
        self.phi_join_ids.clear();
        self.phi_join_keys.clear();
        self.first.fill(Bool::FALSE);
        self.when.fill(None);
    }

    fn dominates(&self, a: usize, mut b: usize) -> bool {
        loop {
            if b == a {
                return true;
            }
            let Some(p) = self.idom[b] else {
                return false;
            };
            if p == b {
                return false;
            }
            b = p;
        }
    }

    fn lca_idom(&self, mut a: usize, mut b: usize) -> usize {
        while a != b {
            while self.index[a] > self.index[b] {
                a = self.idom[a].expect("reachable dominator");
            }
            while self.index[b] > self.index[a] {
                b = self.idom[b].expect("reachable dominator");
            }
        }
        a
    }

    fn gc(&mut self, execs: &mut [Vec<Option<Bool>>], cond: &mut [Bool]) {
        let mut live = Vec::with_capacity(
            self.masks.len()
                + self.other.len()
                + self.scc_lane.len()
                + self.first.len()
                + self.when.len()
                + cond.len(),
        );
        live.extend(self.masks.values().copied());
        live.extend(self.other.values().copied());
        live.extend(self.scc_lane.values().copied());
        live.extend(self.first.iter().copied());
        live.extend(self.when.iter().flatten().copied());
        live.extend(execs.iter().flatten().flatten().copied());
        live.extend(cond.iter().copied());
        let remap = self.b.gc_remap(&live);
        let map = |b: &mut Bool| *b = remap[b.0 as usize];
        for v in self.masks.values_mut() {
            map(v);
        }
        for v in self.other.values_mut() {
            map(v);
        }
        for v in self.scc_lane.values_mut() {
            map(v);
        }
        for v in &mut self.first {
            map(v);
        }
        for v in self.when.iter_mut().flatten() {
            map(v);
        }
        for v in execs.iter_mut().flatten().flatten() {
            map(v);
        }
        for v in cond.iter_mut() {
            map(v);
        }
    }

    /// Lane masks, branch conditions and forward-edge knowledge, block by
    /// block in reverse postorder; returns EXEC at each instruction reading it.
    fn masks_in_order(&mut self) -> Vec<Vec<Option<Bool>>> {
        let p = self.p;
        let input = |reg| p.inputs.iter().find(|i| i.0 == reg).map(|i| i.1);
        if let Some(lo) = input(Reg::S(EXEC)) {
            let hi = if p.wave64 {
                input(Reg::S(EXEC + 1)).unwrap_or(lo)
            } else {
                lo
            };
            self.masks.insert((lo, hi), Bool::TRUE);
        }
        let mut execs: Vec<Vec<Option<Bool>>> =
            p.blocks.iter().map(|b| vec![None; b.insts.len()]).collect();
        for n in 0..self.order.len() {
            let b = self.order[n];
            let mut c = if n == 0 { Bool::TRUE } else { Bool::FALSE };
            for k in 0..self.incoming[b].len() {
                let (q, taken) = self.incoming[b][k];
                if self.index[q] < n {
                    let e = self.edge(self.first[q], q, taken);
                    c = self.b.or(c, e);
                }
            }
            self.first[b] = c;
            // EXEC where known in this block (read or written so far).
            let mut now = None;
            for (i, inst) in p.blocks[b].insts.iter().enumerate() {
                if inst.read(Reg::S(EXEC)).is_some() {
                    let e = self.exec(inst);
                    execs[b][i] = Some(e);
                    now = Some(e);
                }
                self.step(inst, now);
                if let Some(k) = self.key(|r| inst.write(r), EXEC) {
                    now = self.masks.get(&k).or(self.other.get(&k)).copied();
                }
            }
            if let Exit::Branch { .. } = p.blocks[b].exit
                && let Some(last) = p.blocks[b].insts.last()
            {
                self.when[b] = Some(self.branch(last));
                self.wave_any[b] = self.wave_kind(last);
            }
            if self.b.should_gc() {
                self.gc(&mut execs, &mut []);
            }
        }
        execs
    }

    /// `c` narrowed by the branch at the end of `q` towards one successor.
    fn edge(&mut self, c: Bool, q: usize, taken: bool) -> Bool {
        if self.wide.contains(&q) {
            // the wave's decision: it says nothing about this lane's own bit
            return c;
        }
        match self.when[q] {
            Some(w) if taken => self.b.and(c, w),
            Some(w) => self.b.andn2(c, w),
            None => c,
        }
    }

    /// Joins whose constant fails on some edge, and joins that need another
    /// pass because a later edge turned out to derive from EXEC or VALU.
    fn check_joins(&mut self, cond: &[Bool]) -> (Vec<Value>, Vec<Value>) {
        let mut wrong = Vec::new();
        let mut new_derived = Vec::new();
        let mut seen_derived = HashSet::new();
        let mut j = 0;
        let mut p = 0;
        // Checking an argument can resolve another join; it is checked too.
        while j < self.joins.len() || p < self.phi_joins.len() {
            while j < self.joins.len() {
                let (block, id, value, was_derived) = (
                    self.joins[j].block,
                    self.joins[j].id,
                    self.joins[j].value,
                    self.joins[j].derived,
                );
                let mut failed = false;
                for (n, a) in self.joins[j].args.clone().into_iter().enumerate() {
                    let (q, taken) = self.incoming[block][n];
                    let e = self.edge(cond[q], q, taken);
                    let (m, d) = self.mask(a);
                    if !was_derived
                        && d
                        && !self.derived_phis.contains(&id)
                        && seen_derived.insert(id)
                    {
                        new_derived.push(id);
                    }
                    let holds = if value {
                        self.b.implies(e, m)
                    } else {
                        self.b.disjoint(e, m)
                    };
                    if !failed && !holds {
                        wrong.push(id);
                        failed = true;
                    }
                }
                j += 1;
            }
            while p < self.phi_joins.len() {
                let (id, was_derived) = (self.phi_joins[p].id, self.phi_joins[p].derived);
                for a in self.phi_joins[p].args.clone() {
                    let (_, d) = self.mask(a);
                    if !was_derived
                        && d
                        && !self.derived_phis.contains(&id)
                        && seen_derived.insert(id)
                    {
                        new_derived.push(id);
                    }
                }
                p += 1;
            }
        }
        (wrong, new_derived)
    }

    fn finish(
        mut self,
        execs: &[Vec<Option<Bool>>],
        cond: &[Bool],
    ) -> Result<Structured, ShaderError> {
        let phi_joins = std::mem::take(&mut self.phi_joins);
        let mut phi_masks = HashMap::with_capacity(phi_joins.len());
        for pj in phi_joins {
            let incoming: Vec<Bool> = pj
                .args
                .into_iter()
                .enumerate()
                .map(|(n, a)| {
                    let (q, taken) = self.incoming[pj.block][n];
                    let e = self.edge(cond[q], q, taken);
                    let m = self.mask(a).0;
                    self.b.given(m, e)
                })
                .collect();
            phi_masks.insert(pj.id, incoming);
        }
        let blocks = &self.p.blocks;
        let b = &mut self.b;
        let guards = blocks
            .iter()
            .enumerate()
            .map(|(n, block)| {
                // Reachability is fixed within the block. Vector instructions
                // with the same EXEC therefore need the same guard.
                let mut last_guard = None;
                block
                    .insts
                    .iter()
                    .zip(&execs[n])
                    .map(|(inst, exec)| match (exec, &inst.fields) {
                        (None, _)
                        | (_, Fields::Sop { .. } | Fields::Sopp { .. } | Fields::Smem { .. }) => {
                            None
                        }
                        (&Some(e), _) => {
                            if let Some((prev, guard)) = last_guard
                                && prev == e
                            {
                                return guard;
                            }
                            let guard = (!b.implies(cond[n], e)).then(|| b.given(e, cond[n]));
                            last_guard = Some((e, guard));
                            guard
                        }
                    })
                    .collect()
            })
            .collect();
        let exits: Vec<LaneExit> = blocks
            .iter()
            .enumerate()
            .map(|(n, block)| match block.exit {
                Exit::Jump(t) => LaneExit::Jump(t),
                Exit::End => LaneExit::End,
                Exit::Indirect => LaneExit::Indirect,
                Exit::Branch { taken, not_taken } => LaneExit::Branch {
                    when: b.given(self.when[n].unwrap_or(Bool::FALSE), cond[n]),
                    taken,
                    not_taken,
                },
            })
            .collect();
        if self.b.overflow {
            return Err(TOO_LARGE);
        }
        let (mut active, div_loops) = self.compute_active(&exits);
        // Exports go by low dword. A low dword read with two high dwords
        // (one rewritten alone) names no one mask: it is left out of `masks`,
        // so single-dword lowering refuses what reads it instead of taking the
        // other pair's lanes 32-63. `paired_masks` keeps exact `(lo, hi)` pairs
        // so pair-aware readers (`mask_bits`) still get each pair's exact bits.
        let (mut high, mut split) = (Map::default(), HashSet::new());
        let keys = self.masks.keys().chain(self.other.keys());
        for &(lo, hi) in keys.chain(&self.seen) {
            if *high.entry(lo).or_insert(hi) != hi {
                split.insert(lo);
            }
        }
        let paired_masks = if self.p.wave64 {
            self.masks.clone()
        } else {
            Map::default()
        };
        self.masks.retain(|k, _| !split.contains(&k.0));
        active.retain(|lo| !split.contains(lo));
        Ok(Structured {
            masks: self.masks.iter().map(|(&(lo, _), &m)| (lo, m)).collect(),
            highs: self
                .masks
                .keys()
                .filter(|(lo, hi)| lo != hi)
                .map(|&(lo, hi)| (hi, lo))
                .collect(),
            paired_masks,
            active,
            wide: self.wide,
            wave_any: self.wave_any,
            scc_some: self
                .scc_lane
                .iter()
                .filter(|(scc, _)| !self.scc_every.contains(scc))
                .map(|(&scc, &m)| (scc, m))
                .collect(),
            div_loops,
            phi_masks,
            bools: self.b,
            exits,
            guards,
            reach: cond.to_vec(),
        })
    }

    fn key(&self, value: impl Fn(Reg) -> Option<Value>, code: u8) -> Option<Key> {
        let lo = value(Reg::S(code))?;
        let hi = if self.p.wave64 {
            value(Reg::S(code + 1))?
        } else {
            lo
        };
        Some((lo, hi))
    }

    fn born(&self, v: Value) -> usize {
        match (self.defs.get(&v), self.phis.get(&v)) {
            (Some(&(b, _)), _) | (_, Some(&(b, _))) => self.index[b],
            _ => 0,
        }
    }

    fn exec(&mut self, inst: &Inst) -> Bool {
        match self.key(|r| inst.read(r), EXEC) {
            Some(k) => self.mask(k).0,
            None => Bool::TRUE,
        }
    }

    /// The mask a value pair holds, and whether it derives from EXEC or a
    /// VALU result.
    fn mask(&mut self, k: Key) -> (Bool, bool) {
        if let Some(&m) = self.masks.get(&k) {
            return (m, true);
        }
        if let Some(&m) = self.other.get(&k) {
            return (m, false);
        }
        if self.visiting.contains(&k) {
            let born = self.born(k.0).max(self.born(k.1));
            return (self.b.atom(Atom::Opaque(k.0, k.1), born), false);
        }
        self.visiting.push(k);
        let (m, derived) = if let Some(join) = self.join(k) {
            join
        } else if let Some(same) = self.same_inst_mask(k) {
            same
        } else if let Some(split) = self.split_mask(k) {
            split
        } else {
            let p = self.p;
            let (lo, hi) = (self.defs.get(&k.0).copied(), self.defs.get(&k.1).copied());
            let def = |d: Option<(usize, usize)>| d.map(|(b, i)| &p.blocks[b].insts[i]);
            // What an EXEC write the pass does not follow leaves there.
            let exec_write = |d: Option<(usize, usize)>, v: Value| {
                def(d).is_some_and(|i| {
                    i.write(Reg::S(EXEC)) == Some(v) || i.write(Reg::S(EXEC + 1)) == Some(v)
                })
            };
            let derived = exec_write(lo, k.0) || exec_write(hi, k.1);
            let born = self.born(k.0).max(self.born(k.1));
            (self.b.atom(Atom::Opaque(k.0, k.1), born), derived)
        };
        self.visiting.pop();
        let cache = if derived {
            &mut self.masks
        } else {
            &mut self.other
        };
        cache.insert(k, m);
        (m, derived)
    }

    fn same_inst_mask(&mut self, k: Key) -> Option<(Bool, bool)> {
        let (Some(lo), Some(hi)) = (self.defs.get(&k.0).copied(), self.defs.get(&k.1).copied())
        else {
            return None;
        };
        if lo != hi {
            return None;
        }
        let inst = &self.p.blocks[lo.0].insts[lo.1];
        let r = self.sop(inst)?;
        if let Some((dk, m)) = r.dst
            && dk == k
        {
            return Some((m, r.derived));
        }
        if let Some((ek, m)) = r.exec
            && ek == k
        {
            return Some((m, r.derived));
        }
        None
    }

    /// Follows scalar copy chains (`s_mov_b32`, `s_mov_b64`) back to the
    /// underlying value of a single dword.
    fn unmov32(&self, mut v: Value) -> Value {
        for _ in 0..32 {
            let Some(&(b, i)) = self.defs.get(&v) else {
                break;
            };
            let inst = &self.p.blocks[b].insts[i];
            let Fields::Sop { dst, src, .. } = &inst.fields else {
                break;
            };
            let Some(Operand::Scalar { code: src_code, .. }) = src.first().copied() else {
                break;
            };
            if src_code == NULL {
                break;
            }
            let next = match inst.name() {
                "s_mov_b32" => inst.read(Reg::S(src_code)),
                "s_mov_b64" => match dst {
                    Some(Operand::Scalar { code: dst_code, .. }) if *dst_code != NULL => {
                        if inst.write(Reg::S(*dst_code)) == Some(v) {
                            inst.read(Reg::S(src_code))
                        } else if inst.write(Reg::S(*dst_code + 1)) == Some(v) {
                            inst.read(Reg::S(src_code + 1))
                        } else {
                            None
                        }
                    }
                    _ => None,
                },
                _ => None,
            };
            match next {
                Some(n) if n != v => v = n,
                _ => break,
            }
        }
        v
    }

    /// Constant 32-bit value of a scalar dword if built from moves/immediates.
    fn const_u32(&self, v: Value) -> Option<u32> {
        let v = self.unmov32(v);
        let &(b, i) = self.defs.get(&v)?;
        let inst = &self.p.blocks[b].insts[i];
        let Fields::Sop { dst, src, imm, .. } = &inst.fields else {
            return None;
        };
        let inline_i = |c: u8| -> Option<u32> {
            match c {
                128..=192 => Some(u32::from(c) - 128),
                193..=208 => Some((192 - i32::from(c)) as u32),
                _ => None,
            }
        };
        match inst.name() {
            "s_movk_i32" => Some(i32::from((*imm)? as i16) as u32),
            "s_mov_b32" => match *src.first()? {
                Operand::Scalar { code: NULL, .. } => Some(0),
                Operand::Literal(x) => Some(x),
                Operand::Const(c) => inline_i(c),
                _ => None,
            },
            "s_mov_b64" => {
                let high = match dst {
                    Some(Operand::Scalar { code, .. }) if *code != NULL => {
                        if inst.write(Reg::S(*code)) == Some(v) {
                            false
                        } else if inst.write(Reg::S(*code + 1)) == Some(v) {
                            true
                        } else {
                            return None;
                        }
                    }
                    _ => return None,
                };
                match *src.first()? {
                    Operand::Scalar { code: NULL, .. } => Some(0),
                    // Below 2^31 the high dword is 0 under both zero and sign extension.
                    Operand::Literal(x) if x >> 31 == 0 => Some(if high { 0 } else { x }),
                    Operand::Const(c @ 128..=208) => {
                        let lo = inline_i(c)?;
                        Some(if high { ((lo as i32) >> 31) as u32 } else { lo })
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn constant_bool(&mut self, val: u64) -> Bool {
        let v = if self.p.wave64 { val } else { val & 0xFFFF_FFFF };
        match v {
            0 => Bool::FALSE,
            _ if v == u64::MAX >> (32 * u8::from(!self.p.wave64)) => Bool::TRUE,
            _ => self.b.atom(Atom::Constant(v), 0),
        }
    }

    /// Look up the full 64-bit mask key that uniquely produced `v` as its
    /// low (`!high`) or high (`high`) dword.
    fn known_half_key(&self, v: Value, high: bool) -> Option<Key> {
        let p = self.p;
        if let Some(&(b, i)) = self.defs.get(&v) {
            let inst = &p.blocks[b].insts[i];
            for &(reg, w) in &inst.writes {
                if w != v {
                    continue;
                }
                let Reg::S(code) = reg else {
                    continue;
                };
                let k = if !high {
                    let h = inst.write(Reg::S(code + 1))?;
                    (v, h)
                } else if code > 0 {
                    let l = inst.write(Reg::S(code - 1))?;
                    (l, v)
                } else {
                    continue;
                };
                if self.masks.contains_key(&k) {
                    return Some(k);
                }
            }
            return None;
        }
        // Entry EXEC input (wave64 may omit EXEC+1 from `p.inputs` if overwritten before read).
        if let Some(&(_, lo)) = p.inputs.iter().find(|i| i.0 == Reg::S(EXEC)) {
            let hi = p
                .inputs
                .iter()
                .find(|i| i.0 == Reg::S(EXEC + 1))
                .map_or(lo, |i| i.1);
            if ((!high && v == lo) || (high && v == hi && hi != lo))
                && self.masks.contains_key(&(lo, hi))
            {
                return Some((lo, hi));
            }
        }
        // Unique tracked phi mask sharing this half.
        if self.phis.contains_key(&v) {
            let mut found = None;
            for &k in self.masks.keys() {
                if (!high && k.0 == v) || (high && k.1 == v) {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(k);
                }
            }
            return found;
        }
        None
    }

    /// Read-only check for `step()`: whether a wave64 pair's halves trace via
    /// moves/32-bit bitwise ops to a mask already in `self.masks`.
    fn has_split_mask(&self, k: Key) -> bool {
        if !self.p.wave64 {
            return false;
        }
        let (r0, r1) = (self.unmov32(k.0), self.unmov32(k.1));
        if self.masks.contains_key(&(r0, r1)) {
            return true;
        }
        self.half_has_mask(r0, false, 0) || self.half_has_mask(r1, true, 0)
    }

    fn half_has_mask(&self, v: Value, high: bool, depth: usize) -> bool {
        if depth > 8 {
            return false;
        }
        let v = self.unmov32(v);
        if self.known_half_key(v, high).is_some() {
            return true;
        }
        let Some(&(b, i)) = self.defs.get(&v) else {
            return false;
        };
        let inst = &self.p.blocks[b].insts[i];
        let Fields::Sop { dst, src, .. } = &inst.fields else {
            return false;
        };
        let Some(op) = inst.name().strip_prefix("s_").and_then(|s| s.strip_suffix("_b32")) else {
            return false;
        };
        if !matches!(
            op,
            "not" | "and" | "or" | "xor" | "andn2" | "orn2" | "andn1" | "orn1" | "nand" | "nor"
                | "xnor" | "cselect" | "cmov"
        ) {
            return false;
        }
        let check_op = |op: &Operand| match *op {
            Operand::Scalar { code, .. } if code != NULL => inst
                .read(Reg::S(code))
                .is_some_and(|u| self.half_has_mask(u, high, depth + 1)),
            _ => false,
        };
        src.iter().any(check_op) || (op == "cmov" && dst.as_ref().is_some_and(check_op))
    }

    /// Resolves one 32-bit half of a wave64 mask (lanes 0..31 when `!high`,
    /// lanes 32..63 when `high`), exact on those 32 lanes.
    fn half_mask(&mut self, v: Value, high: bool, depth: usize) -> Option<(Bool, bool)> {
        if depth > 12 {
            return None;
        }
        let v = self.unmov32(v);
        if let Some(c) = self.const_u32(v) {
            let b = match c {
                0 => Bool::FALSE,
                u32::MAX => Bool::TRUE,
                _ => {
                    let bits = if high {
                        u64::from(c) << 32
                    } else {
                        u64::from(c)
                    };
                    self.b.atom(Atom::Constant(bits), 0)
                }
            };
            return Some((b, false));
        }
        if let Some(k) = self.known_half_key(v, high) {
            return Some((self.masks[&k], true));
        }
        let &(b, i) = self.defs.get(&v)?;
        let inst = &self.p.blocks[b].insts[i];
        let Fields::Sop { dst, src, .. } = &inst.fields else {
            return None;
        };
        let op = inst.name().strip_prefix("s_")?.strip_suffix("_b32")?;
        let half_arg = |op: &Operand, l: &mut Self| -> Option<(Bool, bool)> {
            match *op {
                Operand::Scalar { code: NULL, .. } => Some((Bool::FALSE, false)),
                Operand::Scalar { code, .. } => {
                    let u = inst.read(Reg::S(code))?;
                    l.half_mask(u, high, depth + 1)
                }
                Operand::Const(c @ 128..=208) => {
                    let val = if c <= 192 {
                        u32::from(c) - 128
                    } else {
                        (192 - i32::from(c)) as u32
                    };
                    let b = match val {
                        0 => Bool::FALSE,
                        u32::MAX => Bool::TRUE,
                        _ => {
                            let bits = if high {
                                u64::from(val) << 32
                            } else {
                                u64::from(val)
                            };
                            l.b.atom(Atom::Constant(bits), 0)
                        }
                    };
                    Some((b, false))
                }
                Operand::Literal(val) => {
                    let b = match val {
                        0 => Bool::FALSE,
                        u32::MAX => Bool::TRUE,
                        _ => {
                            let bits = if high {
                                u64::from(val) << 32
                            } else {
                                u64::from(val)
                            };
                            l.b.atom(Atom::Constant(bits), 0)
                        }
                    };
                    Some((b, false))
                }
                _ => None,
            }
        };
        match op {
            "not" => {
                let (a, d) = half_arg(src.first()?, self)?;
                Some((self.b.not(a), d))
            }
            "cselect" => {
                let (a, da) = half_arg(src.first()?, self)?;
                let (c, dc) = half_arg(src.get(1)?, self)?;
                let scc = inst.read(Reg::Scc)?;
                let r = if self.clear_after_loop(scc) {
                    c
                } else {
                    let born = self.born(scc);
                    let u = self.b.atom(Atom::Uniform(scc), born);
                    self.b.ite(u, a, c)
                };
                Some((r, da | dc))
            }
            "cmov" => {
                let (a, da) = half_arg(src.first()?, self)?;
                let (c, dc) = half_arg(dst.as_ref()?, self)?;
                let scc = inst.read(Reg::Scc)?;
                let r = if self.clear_after_loop(scc) {
                    c
                } else {
                    let born = self.born(scc);
                    let u = self.b.atom(Atom::Uniform(scc), born);
                    self.b.ite(u, a, c)
                };
                Some((r, da | dc))
            }
            _ => {
                let (a, da) = half_arg(src.first()?, self)?;
                let (c, dc) = half_arg(src.get(1)?, self)?;
                let r = self.combine(op, a, c)?;
                Some((r, da | dc))
            }
        }
    }

    /// Wave64 pairs whose halves were written by separate 32-bit instructions
    /// (`s_mov_b32` unspills, `s_mov_b32 vcc_hi, 0`, 32-bit bitwise halves).
    fn split_mask(&mut self, k: Key) -> Option<(Bool, bool)> {
        if !self.p.wave64 {
            // Wave32 constant `s_mov_b32` / `s_movk_i32` (not already caught by `same_inst_mask`).
            if let Some(c) = self.const_u32(k.0) {
                return Some((self.constant_bool(u64::from(c)), false));
            }
            let r0 = self.unmov32(k.0);
            if r0 != k.0 {
                return Some(self.mask((r0, r0)));
            }
            return None;
        }
        // Step 1: unwrap `s_mov_b32` / `s_mov_b64` copies on both halves.
        let (r0, r1) = (self.unmov32(k.0), self.unmov32(k.1));
        if (r0, r1) != k {
            return Some(self.mask((r0, r1)));
        }
        // Step 2: both halves are 32-bit constants.
        if let (Some(c0), Some(c1)) = (self.const_u32(k.0), self.const_u32(k.1)) {
            let val = u64::from(c0) | (u64::from(c1) << 32);
            return Some((self.constant_bool(val), false));
        }
        // Step 3: resolve each 32-bit half independently and join at lane 32.
        let (lo_b, d0) = self.half_mask(k.0, false, 0)?;
        let (hi_b, d1) = self.half_mask(k.1, true, 0)?;
        let low32 = self.b.atom(Atom::Constant(0x0000_0000_FFFF_FFFF), 0);
        let m = self.b.ite(low32, lo_b, hi_b);
        Some((m, d0 | d1))
    }

    fn phi_parts(&self, k: Key) -> Option<(usize, Value, Vec<Key>)> {
        let p = self.p;
        let (lo, hi) = (self.phis.get(&k.0).copied(), self.phis.get(&k.1).copied());
        let def_block = |v: Value| {
            self.defs
                .get(&v)
                .map(|&(b, _)| b)
                .or_else(|| self.phis.get(&v).map(|&(b, _)| b))
        };
        let (block, use_lo, use_hi) = match (lo, hi) {
            (Some((a, _)), Some((b, _))) if a == b => (a, lo, hi),
            (Some((a, _)), Some((b, _))) if self.dominates(a, b) => (b, None, hi),
            (Some((a, _)), Some((b, _))) if self.dominates(b, a) => (a, lo, None),
            (Some(_), Some(_)) => return None,
            (Some((a, _)), None) => {
                if def_block(k.1).is_some_and(|db| db == a || !self.dominates(db, a)) {
                    return None;
                }
                (a, lo, None)
            }
            (None, Some((b, _))) => {
                if def_block(k.0).is_some_and(|db| db == b || !self.dominates(db, b)) {
                    return None;
                }
                (b, None, hi)
            }
            (None, None) => return None,
        };
        let target = &p.blocks[block];
        let arg = |phi: Option<(usize, usize)>, own: Value, n: usize| {
            phi.map_or(own, |(_, i)| target.phis[i].args[n])
        };
        let args: Vec<Key> = (0..target.preds.len())
            .map(|n| (arg(use_lo, k.0, n), arg(use_hi, k.1, n)))
            .collect();
        let id = use_lo.map_or(k.1, |_| k.0);
        Some((block, id, args))
    }

    /// A mask joined by phis at a block entry, and whether a mask coming in
    /// on a forward edge derives from EXEC or a VALU result. Where every
    /// lane arriving so far has the incoming mask set (or clear), the join
    /// is that constant, for a lane can only read it after arriving.
    fn join(&mut self, k: Key) -> Option<(Bool, bool)> {
        let (block, id, args) = self.phi_parts(k)?;
        if let Some(&prev) = self.phi_join_keys.get(&id)
            && prev != k
        {
            return None;
        }
        let mut derived = self.derived_phis.contains(&id);
        // (edge condition, mask) per forward edge; back edges come later.
        let mut forward = Vec::new();
        for (n, &a) in args.iter().enumerate() {
            let (q, taken) = self.incoming[block][n];
            if self.index[q] < self.index[block] {
                let (m, d) = self.mask(a);
                derived |= d;
                let e = self.edge(self.first[q], q, taken);
                forward.push((e, m));
            } else {
                self.seen.push(a);
            }
        }
        if forward.len() == args.len()
            && let Some(&(_, m)) = forward.first()
            && forward.iter().all(|f| f.1 == m)
        {
            return Some((m, derived));
        }
        if !self.unresolved.contains(&id) {
            for value in [true, false] {
                let holds = forward.iter().all(|&(e, m)| {
                    if value {
                        self.b.implies(e, m)
                    } else {
                        self.b.disjoint(e, m)
                    }
                });
                if holds {
                    self.phi_join_keys.insert(id, k);
                    self.joins.push(Join {
                        block,
                        id,
                        value,
                        derived,
                        args,
                    });
                    let m = if value { Bool::TRUE } else { Bool::FALSE };
                    return Some((m, derived));
                }
            }
        }
        self.phi_join_keys.insert(id, k);
        if self.phi_join_ids.insert(id) {
            self.phi_joins.push(PhiJoin {
                block,
                id,
                derived,
                args,
            });
        }
        let born = self.index[block];
        Some((self.b.atom(Atom::Phi(id), born), derived))
    }

    fn operand(&mut self, inst: &Inst, op: &Operand) -> Option<(Bool, bool)> {
        let wave64 = self.p.wave64;
        let constant = |b: &mut Bools, v: u64| {
            let v = if wave64 { v } else { v & 0xFFFF_FFFF };
            match v {
                0 => Bool::FALSE,
                _ if v == u64::MAX >> (32 * u8::from(!wave64)) => Bool::TRUE,
                _ => b.atom(Atom::Constant(v), 0),
            }
        };
        Some(match *op {
            Operand::Scalar { code, .. } if code != NULL => {
                let k = self.key(|r| inst.read(r), code)?;
                self.mask(k)
            }
            Operand::Const(c @ 128..=208) => {
                let v = if c <= 192 {
                    i64::from(c) - 128
                } else {
                    192 - i64::from(c)
                };
                (constant(&mut self.b, v as u64), false)
            }
            Operand::Literal(x) => (constant(&mut self.b, u64::from(x)), false),
            _ => return None,
        })
    }

    /// Lane-mask meaning of a lane-width scalar operation.
    fn sop(&mut self, inst: &Inst) -> Option<SopMasks> {
        let Fields::Sop { dst, src, .. } = &inst.fields else {
            return None;
        };
        let width = if self.p.wave64 { "_b64" } else { "_b32" };
        let op = inst.name().strip_prefix("s_")?.strip_suffix(width)?;
        let dst_op = *dst;
        let dst = match dst_op {
            Some(Operand::Scalar { code, .. }) if code != NULL => {
                self.key(|r| inst.write(r), code)
            }
            _ => None,
        };
        let exec_out = self.key(|r| inst.write(r), EXEC);
        let (save, f) = match (op.strip_suffix("_saveexec"), op.strip_suffix("_wrexec")) {
            (Some(f), _) => (Some(true), f),
            (_, Some(f)) => (Some(false), f),
            _ => (None, op),
        };
        let mut derived = exec_out.is_some() || save.is_some();
        let mut arg = |n: usize, l: &mut Self| {
            let (m, d) = l.operand(inst, src.get(n)?)?;
            derived |= d;
            Some(m)
        };
        if let Some(saveexec) = save {
            let s = arg(0, self)?;
            let exec = self.exec(inst);
            let new = self.combine(f, s, exec)?;
            let saved = if saveexec { exec } else { new };
            return Some(SopMasks {
                dst: dst.map(|d| (d, saved)),
                exec: exec_out.map(|e| (e, new)),
                derived,
                scc: Some(new),
            });
        }
        let (result, scc) = match op {
            "mov" => (arg(0, self)?, None),
            "not" => {
                let a = arg(0, self)?;
                let r = self.b.not(a);
                (r, Some(r))
            }
            "wqm" => {
                let a = arg(0, self)?;
                let lo = dst.or(exec_out)?.0;
                let born = self.born(lo);
                let q = self.b.atom(Atom::Quad(lo), born);
                let r = self.b.or(a, q);
                (r, Some(r))
            }
            "cselect" => {
                let (a, c) = (arg(0, self)?, arg(1, self)?);
                let scc = inst.read(Reg::Scc)?;
                if self.clear_after_loop(scc) {
                    (c, None)
                } else {
                    let born = self.born(scc);
                    let u = self.b.atom(Atom::Uniform(scc), born);
                    (self.b.ite(u, a, c), None)
                }
            }
            "cmov" => {
                let a = arg(0, self)?;
                let Some(Operand::Scalar { code, .. }) = dst_op else {
                    return None;
                };
                if code == NULL {
                    return None;
                }
                let old_k = self.key(|r| inst.read(r), code)?;
                let (c, d) = self.mask(old_k);
                derived |= d;
                let scc = inst.read(Reg::Scc)?;
                if self.clear_after_loop(scc) {
                    (c, None)
                } else {
                    let born = self.born(scc);
                    let u = self.b.atom(Atom::Uniform(scc), born);
                    (self.b.ite(u, a, c), None)
                }
            }
            _ => {
                let (a, c) = (arg(0, self)?, arg(1, self)?);
                let r = self.combine(op, a, c)?;
                (r, Some(r))
            }
        };
        Some(SopMasks {
            dst: dst.map(|d| (d, result)),
            exec: exec_out.map(|e| (e, result)),
            derived,
            scc,
        })
    }

    /// Whether SCC value `scc` is clear wherever it is read: the instruction before an
    /// `s_cbranch_execnz` that closes a loop wrote it as "the EXEC it leaves is not
    /// empty" (`s_andn2_wrexec`, say), and the wave leaves `do { .. } while (EXEC)`
    /// with EXEC empty. A read inside the loop or behind another way out of it sees a
    /// join of this value, not the value itself.
    fn clear_after_loop(&self, scc: Value) -> bool {
        let Some(&(b, i)) = self.defs.get(&scc) else {
            return false;
        };
        let block = &self.p.blocks[b];
        let (Some(writer), Some(last)) = (block.insts.get(i), block.insts.last()) else {
            return false;
        };
        let exec = writer.write(Reg::S(EXEC));
        let back = match block.exit {
            Exit::Branch { taken, .. } => self.index[taken] <= self.index[b],
            _ => false,
        };
        back && i + 2 == block.insts.len()
            && exec.is_some()
            && last.name() == "s_cbranch_execnz"
            && last.read(Reg::S(EXEC)) == exec
            && (!self.p.wave64 || last.read(Reg::S(EXEC + 1)) == writer.write(Reg::S(EXEC + 1)))
    }

    fn combine(&mut self, f: &str, a: Bool, c: Bool) -> Option<Bool> {
        let b = &mut self.b;
        Some(match f {
            "and" => b.and(a, c),
            "or" => b.or(a, c),
            "xor" => b.xor(a, c),
            "andn2" => b.andn2(a, c),
            "orn2" => b.orn2(a, c),
            "andn1" => b.andn1(a, c),
            "orn1" => b.orn1(a, c),
            "nand" => b.nand(a, c),
            "nor" => b.nor(a, c),
            "xnor" => b.xnor(a, c),
            _ => return None,
        })
    }

    /// Records the masks `inst` defines; `exec` is EXEC there, if known.
    fn step(&mut self, inst: &Inst, exec: Option<Bool>) {
        let name = inst.name();
        match &inst.fields {
            Fields::Sop { dst, src, .. } => {
                let mut takes_mask = false;
                let extra_dst = (matches!(name, "s_cmov_b32" | "s_cmov_b64"))
                    .then_some(dst.as_ref())
                    .flatten();
                for s in src.iter().chain(extra_dst) {
                    if let Operand::Scalar { code, .. } = *s
                        && code != NULL
                        && let Some(k) = self.key(|r| inst.read(r), code)
                    {
                        let phi = self.phis.contains_key(&k.0) || self.phis.contains_key(&k.1);
                        takes_mask |= self.masks.contains_key(&k)
                            || ((phi || self.has_split_mask(k)) && self.mask(k).1);
                    }
                }
                let touches_exec =
                    inst.write(Reg::S(EXEC)).is_some() || inst.read(Reg::S(EXEC)).is_some();
                if (takes_mask || touches_exec)
                    && let Some(r) = self.sop(inst).filter(|r| r.derived)
                {
                    for (k, m) in r.dst.into_iter().chain(r.exec) {
                        self.masks.insert(k, m);
                    }
                    if let (Some(scc), Some(m)) = (inst.write(Reg::Scc), r.scc) {
                        self.scc_lane.insert(scc, m);
                    }
                }
                // `s_cmp_eq/lg` of a mask: "no lane differs" per lane.
                let width = if self.p.wave64 { "64" } else { "32" };
                if takes_mask
                    && let (Some(rest), Some(scc)) =
                        (name.strip_prefix("s_cmp_"), inst.write(Reg::Scc))
                    && rest
                        .strip_suffix(width)
                        .is_some_and(|r| r == "eq_u" || r == "lg_u")
                    && let [a, c] = &src[..]
                    && let (Some((a, _)), Some((c, _))) =
                        (self.operand(inst, a), self.operand(inst, c))
                {
                    let ne = self.b.xor(a, c);
                    let m = if rest.starts_with("eq") {
                        self.b.not(ne)
                    } else {
                        ne
                    };
                    self.scc_lane.insert(scc, m);
                    if rest.starts_with("eq") {
                        self.scc_every.insert(scc);
                    }
                }
                // `s_bcnt1` of a mask every running lane sees alike (EXEC under a scalar
                // condition, or EXEC itself): its SCC, "some lane is set", is each lane's own
                // bit. A branch on it then reads no ballot.
                if takes_mask
                    && name.starts_with("s_bcnt1_i32_b")
                    && let Some(scc) = inst.write(Reg::Scc)
                    && let [a] = &src[..]
                    && let Some((m, true)) = self.operand(inst, a)
                {
                    let e = exec.unwrap_or(Bool::TRUE);
                    let own = self.b.given(m, e);
                    if self.b.implies(m, e)
                        && self.b.support(own).iter().all(|a| matches!(a, Atom::Uniform(_)))
                    {
                        self.scc_lane.insert(scc, own);
                    }
                }
            }
            Fields::Valu(v) => {
                let exec = self.exec(inst);
                let lane = |l: &mut Self, k: Key| {
                    let born = l.born(k.0);
                    let a = l.b.atom(Atom::Lane(k.0), born);
                    let m = l.b.and(exec, a);
                    l.masks.insert(k, m);
                };
                // A select reads a mask nothing else may: a join of compare
                // results is resolved here, as for EXEC. A carry-in does not
                // resolve phi joins (that would turn a refusal after a
                // divergent branch into a per-lane phi), but non-phi split-half
                // copies/writes (e.g. `s_mov_b32` unspills or `vcc_hi = 0`)
                // are resolved safely.
                if (name == "v_cndmask_b32" || name.contains("_co_ci_"))
                    && let Some(&Operand::Scalar { code, .. }) = v.src.get(2)
                    && code != NULL
                    && let Some(k) = self.key(|r| inst.read(r), code)
                {
                    if name == "v_cndmask_b32" {
                        self.mask(k);
                    } else {
                        let (r0, r1) = (self.unmov32(k.0), self.unmov32(k.1));
                        if !self.phis.contains_key(&r0)
                            && !self.phis.contains_key(&r1)
                            && self.has_split_mask(k)
                        {
                            self.mask(k);
                        } else {
                            self.seen.push(k);
                        }
                    }
                }
                if matches!(name, "v_div_fmas_f32" | "v_div_fmas_f64")
                    && let Some(k) = self.key(|r| inst.read(r), VCC)
                {
                    self.mask(k);
                }
                if name.starts_with("v_cmpx") {
                    if let Some(k) = self.key(|r| inst.write(r), EXEC) {
                        lane(self, k);
                    }
                } else {
                    let compare = v.dst.filter(|_| name.starts_with("v_cmp"));
                    for op in compare.iter().chain(&v.carry) {
                        if let Operand::Scalar { code, .. } = *op
                            && let Some(k) = self.key(|r| inst.write(r), code)
                        {
                            lane(self, k);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The per-lane condition under which a block's closing branch is taken.
    /// How the console decides the branch `inst` on a lane mask, in terms of the lanes' own
    /// `when` ([`Self::branch`]): taken when some lane's holds (`true`) or when every lane's
    /// does (`false`). `execnz`/`vccnz` ask for some set bit, `execz`/`vccz` for none; SCC
    /// of a mask operation is "some lane's bit is set", SCC of `s_cmp_eq` of two masks is
    /// "every lane's is" ([`Self::scc_every`]), and `scc0` branches on the opposite. None
    /// for a branch on a scalar condition.
    fn wave_kind(&self, inst: &Inst) -> Option<bool> {
        match inst.name() {
            "s_cbranch_execnz" | "s_cbranch_vccnz" => Some(true),
            "s_cbranch_execz" | "s_cbranch_vccz" => Some(false),
            n @ ("s_cbranch_scc0" | "s_cbranch_scc1") => {
                let scc = inst.read(Reg::Scc)?;
                self.scc_lane.get(&scc)?;
                // set: SCC is 1 when some lane's bit is set (every lane's, for a compare)
                let every = self.scc_every.contains(&scc);
                Some((n == "s_cbranch_scc1") != every)
            }
            _ => None,
        }
    }

    fn branch(&mut self, inst: &Inst) -> Bool {
        let exec = self.exec(inst);
        match inst.name() {
            "s_cbranch_execz" => self.b.not(exec),
            "s_cbranch_execnz" => exec,
            n @ ("s_cbranch_vccz" | "s_cbranch_vccnz") => {
                let vcc = match self.key(|r| inst.read(r), VCC) {
                    Some(k) => self.mask(k).0,
                    None => Bool::FALSE,
                };
                let any = self.b.and(vcc, exec);
                if n.ends_with("nz") {
                    any
                } else {
                    self.b.not(any)
                }
            }
            n => {
                let Some(scc) = inst.read(Reg::Scc) else {
                    return Bool::FALSE;
                };
                let set = match self.scc_lane.get(&scc) {
                    Some(&m) => m,
                    None => {
                        let born = self.born(scc);
                        self.b.atom(Atom::Uniform(scc), born)
                    }
                };
                if n == "s_cbranch_scc1" {
                    set
                } else {
                    self.b.not(set)
                }
            }
        }
    }

    /// What a lane knows on entering each block, to a fixpoint. A loop
    /// header forgets what the back edge says about atoms the loop redefines.
    fn paths(&mut self, execs: &mut [Vec<Option<Bool>>]) -> Result<Vec<Bool>, ShaderError> {
        let n = self.p.blocks.len();
        let mut cond = vec![Bool::FALSE; n];
        let mut dirty = vec![false; n];
        if let Some(&entry) = self.order.first() {
            cond[entry] = Bool::TRUE;
            for s in self.p.blocks[entry].exit.successors() {
                dirty[s] = true;
            }
        }
        for _ in 0..MAX_ROUNDS {
            let mut changed = false;
            for i in 1..self.order.len() {
                let s = self.order[i];
                if !dirty[s] {
                    continue;
                }
                dirty[s] = false;
                let mut c = Bool::FALSE;
                for k in 0..self.incoming[s].len() {
                    let (q, taken) = self.incoming[s][k];
                    if cond[q] == Bool::FALSE {
                        continue;
                    }
                    let mut e = self.edge(cond[q], q, taken);
                    if i <= self.index[q] {
                        e = self.b.forget(e, i, &mut Map::default());
                    }
                    c = self.b.or(c, e);
                }
                if c != cond[s] {
                    cond[s] = c;
                    changed = true;
                    for succ in self.p.blocks[s].exit.successors() {
                        dirty[succ] = true;
                    }
                }
                if self.b.should_gc() {
                    self.gc(execs, &mut cond);
                }
            }
            if self.b.overflow {
                return Err(TOO_LARGE);
            }
            if !changed {
                return Ok(cond);
            }
        }
        Err(ShaderError::Isa {
            offset: 0,
            reason: "lane conditions do not settle",
        })
    }

    /// Computes [`Structured::active`] soundly from final structured `exits`:
    /// a mask written in block `b` is active iff every lane whose bit can be 1
    /// is proven to execute block `b` in the same dynamic wave instance.
    /// Also returns the divergent natural loop bodies (`div_loops`).
    fn compute_active(&self, exits: &[LaneExit]) -> (HashSet<Value>, Vec<Vec<bool>>) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Owner {
            Empty,
            Block(usize),
            Top,
        }

        let n = exits.len();
        let Some(&entry) = self.order.first() else {
            return (HashSet::new(), Vec::new());
        };
        // Build CFG & strict post-dominators on the final structured exits.
        let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut has_pdom = true;
        for (b, e) in exits.iter().enumerate() {
            match *e {
                LaneExit::Jump(t) => {
                    succ[b].push(t);
                    preds[t].push(b);
                }
                LaneExit::Branch {
                    taken, not_taken, ..
                } => {
                    succ[b].push(taken);
                    preds[taken].push(b);
                    if not_taken != taken {
                        succ[b].push(not_taken);
                        preds[not_taken].push(b);
                    }
                }
                LaneExit::End => {}
                LaneExit::Indirect => has_pdom = false,
            }
        }
        let words = n.div_ceil(64);
        let mut pdom: Vec<Vec<u64>> = Vec::new();
        let mut divergent = vec![false; n];
        let mut div_loops: Vec<Vec<bool>> = Vec::new();
        if has_pdom {
            let full = vec![u64::MAX; words];
            pdom = (0..n)
                .map(|b| {
                    if succ[b].is_empty() {
                        let mut v = vec![0; words];
                        v[b / 64] |= 1 << (b % 64);
                        v
                    } else {
                        full.clone()
                    }
                })
                .collect();
            let mut changed = true;
            while changed {
                changed = false;
                for b in (0..n).rev() {
                    if succ[b].is_empty() {
                        continue;
                    }
                    let mut v = full.clone();
                    for &t in &succ[b] {
                        for w in 0..words {
                            v[w] &= pdom[t][w];
                        }
                    }
                    v[b / 64] |= 1 << (b % 64);
                    if v != pdom[b] {
                        pdom[b] = v;
                        changed = true;
                    }
                }
            }
            let mut regions: Vec<Option<Vec<bool>>> = vec![None; n];
            for (d, e) in exits.iter().enumerate() {
                let LaneExit::Branch { when, .. } = *e else {
                    continue;
                };
                if self.wide.contains(&d)
                    || self
                        .b
                        .support(when)
                        .iter()
                        .all(|a| matches!(a, Atom::Uniform(_)))
                {
                    continue;
                }
                divergent[d] = true;
                // Strict post-dominators: `d` itself does not stop the walk, so
                // divergent loops include their header and latch.
                let mut seen = vec![false; n];
                let mut stack = succ[d].clone();
                while let Some(b) = stack.pop() {
                    if seen[b] || (b != d && (pdom[d][b / 64] >> (b % 64)) & 1 == 1) {
                        continue;
                    }
                    seen[b] = true;
                    stack.extend(succ[b].iter().copied());
                }
                regions[d] = Some(seen);
            }
            // Natural loops containing a divergent branch whose region reaches the header.
            for &h in &self.order {
                let back_sources: Vec<usize> = self.incoming[h]
                    .iter()
                    .map(|&(q, _)| q)
                    .filter(|&q| self.index[q] != usize::MAX && self.index[h] <= self.index[q])
                    .collect();
                if back_sources.is_empty() {
                    continue;
                }
                let mut body = vec![false; n];
                body[h] = true;
                let mut stack = Vec::new();
                for q in back_sources {
                    if !body[q] {
                        body[q] = true;
                        stack.push(q);
                    }
                }
                while let Some(x) = stack.pop() {
                    for &(q, _) in &self.incoming[x] {
                        if self.index[q] != usize::MAX && !body[q] {
                            body[q] = true;
                            stack.push(q);
                        }
                    }
                }
                let has_div = (0..n).any(|d| {
                    body[d] && regions[d].as_ref().is_some_and(|r| r[h])
                });
                if has_div {
                    div_loops.push(body);
                }
            }
        }

        let mut arrive_cache: Map<(usize, usize), bool> = Map::default();
        let mut lanes_arrive = |u: usize, b: usize| -> bool {
            if u == b {
                return true;
            }
            if !has_pdom {
                return false;
            }
            if let Some(&ok) = arrive_cache.get(&(u, b)) {
                return ok;
            }
            let ok = 'check: {
                // 1. `u` and `b` must lie in the exact same set of divergent loops
                //    (preventing both shrunk iterations inside a loop and mixed exit
                //    iterations outside a loop).
                if div_loops.iter().any(|body| body[u] != body[b]) {
                    break 'check false;
                }
                // Dominators of `b` (including `b`) in the same divergent-loop nest as `b`.
                // A divergent branch `d` reconverges before (or at) `b` if one of these
                // dominators `m` strictly post-dominates `d` after `d`.
                let mut b_doms = Vec::new();
                let mut cur = b;
                loop {
                    if div_loops.iter().all(|body| body[cur] == body[b]) {
                        b_doms.push(cur);
                    }
                    match self.idom[cur] {
                        Some(p) if p != cur => cur = p,
                        _ => break,
                    }
                }
                // 2. Either `u` dominates `b`, or some dominator `m` of `b` (after `u`)
                //    strictly post-dominates `u`.
                let reaches_dom = self.dominates(u, b)
                    || b_doms.iter().any(|&m| {
                        m != u
                            && self.index[u] < self.index[m]
                            && !self.dominates(m, u)
                            && (pdom[u][m / 64] >> (m % 64)) & 1 == 1
                    });
                if !reaches_dom {
                    break 'check false;
                }
                // 3. Every divergent branch `d` on a path from `u` to `b` (stopping at `b`)
                //    must reconverge at some dominator `m` of `b` strictly post-dominating
                //    `d` after `d`.
                let mut reach_u = vec![false; n];
                reach_u[u] = true;
                let mut stack = vec![u];
                while let Some(x) = stack.pop() {
                    for &t in &succ[x] {
                        if t != b && !reach_u[t] {
                            reach_u[t] = true;
                            stack.push(t);
                        }
                    }
                }
                let mut can_b = vec![false; n];
                let mut rstack = Vec::new();
                for &q in &preds[b] {
                    if reach_u[q] && !can_b[q] {
                        can_b[q] = true;
                        if q != u {
                            rstack.push(q);
                        }
                    }
                }
                while let Some(x) = rstack.pop() {
                    for &q in &preds[x] {
                        if reach_u[q] && !can_b[q] {
                            can_b[q] = true;
                            if q != u {
                                rstack.push(q);
                            }
                        }
                    }
                }
                for d in 0..n {
                    if can_b[d] && divergent[d] {
                        let reconverged = b_doms.iter().any(|&m| {
                            m != d
                                && self.index[d] < self.index[m]
                                && !self.dominates(m, d)
                                && (pdom[d][m / 64] >> (m % 64)) & 1 == 1
                        });
                        if !reconverged {
                            break 'check false;
                        }
                    }
                }
                true
            };
            arrive_cache.insert((u, b), ok);
            ok
        };

        let join_owner = |a: Owner, b: Owner| -> Owner {
            match (a, b) {
                (Owner::Empty, x) | (x, Owner::Empty) => x,
                (Owner::Top, _) | (_, Owner::Top) => Owner::Top,
                (Owner::Block(u), Owner::Block(v)) => Owner::Block(self.lca_idom(u, v)),
            }
        };
        let and_owner = |a: Owner, b: Owner| -> Owner {
            match (a, b) {
                (Owner::Empty, _) | (_, Owner::Empty) => Owner::Empty,
                (Owner::Top, x) | (x, Owner::Top) => x,
                (Owner::Block(u), Owner::Block(v)) => {
                    if self.dominates(u, v) {
                        Owner::Block(v)
                    } else if self.dominates(v, u) {
                        Owner::Block(u)
                    } else {
                        Owner::Block(u)
                    }
                }
            }
        };

        // Exact EXEC-branch narrowing edges: if block `d` (not wave-wide) ends in
        // `s_cbranch_execz` / `s_cbranch_execnz` reading EXEC key `k`, and the
        // live-EXEC successor `t` is dominated by `d` along a forward edge inside
        // the exact same set of divergent loops, then whenever every 1-lane of `k`
        // arrives at `d`, every 1-lane of `k` must take `d -> t` and execute `t`.
        let p = self.p;
        let mut exec_narrow: Map<Key, Vec<(usize, usize)>> = Map::default();
        if has_pdom {
            for &d in &self.order {
                if self.wide.contains(&d) {
                    continue;
                }
                let Some(last) = p.blocks[d].insts.last() else {
                    continue;
                };
                let t = match (last.name(), p.blocks[d].exit) {
                    ("s_cbranch_execz", Exit::Branch { not_taken, .. }) => not_taken,
                    ("s_cbranch_execnz", Exit::Branch { taken, .. }) => taken,
                    _ => continue,
                };
                if self.index[t] == usize::MAX
                    || self.index[d] >= self.index[t]
                    || !self.dominates(d, t)
                    || div_loops.iter().any(|body| body[d] != body[t])
                {
                    continue;
                }
                if let Some(k) = self.key(|r| last.read(r), EXEC) {
                    exec_narrow.entry(k).or_default().push((d, t));
                }
            }
            for edges in exec_narrow.values_mut() {
                edges.sort_by_key(|&(_, t)| self.index[t]);
            }
        }

        // Partition keys in `self.masks` into disjoint origins so external inputs
        // and unhandled instructions never default to `Owner::Empty`.
        let mut owner: Map<Key, Owner> = Map::default();
        let entry_exec = p.inputs.iter().find(|i| i.0 == Reg::S(EXEC)).map(|&(_, lo)| {
            let hi = if p.wave64 {
                p.inputs
                    .iter()
                    .find(|i| i.0 == Reg::S(EXEC + 1))
                    .map_or(lo, |i| i.1)
            } else {
                lo
            };
            (lo, hi)
        });
        if let Some(ek) = entry_exec {
            owner.insert(ek, Owner::Block(entry));
        }
        for &k in self.other.keys() {
            if self.phi_parts(k).is_none()
                && self.const_u32(k.0) == Some(0)
                && (!p.wave64 || self.const_u32(k.1) == Some(0))
            {
                owner.insert(k, Owner::Empty);
            }
        }
        let mut phi_keys: Vec<(Key, usize, Vec<Key>)> = Vec::new();
        let mut inst_keys: Map<(usize, usize), Vec<Key>> = Map::default();
        let mut split_keys: Vec<(Key, usize)> = Vec::new();
        for &k in self.masks.keys() {
            if Some(k) == entry_exec {
                continue;
            }
            if let Some((block, id, args)) = self.phi_parts(k) {
                let is_exec_phi = self
                    .phis
                    .get(&id)
                    .is_some_and(|&(b, i)| p.blocks[b].phis[i].reg == Reg::S(EXEC));
                if is_exec_phi {
                    owner.insert(k, Owner::Block(block));
                } else {
                    owner.insert(k, Owner::Empty);
                    phi_keys.push((k, block, args));
                }
                continue;
            }
            match (self.defs.get(&k.0).copied(), self.defs.get(&k.1).copied()) {
                (Some(a), Some(b)) if a == b => {
                    owner.insert(k, Owner::Empty);
                    inst_keys.entry(a).or_default().push(k);
                }
                (None, None)
                    if !self.phis.contains_key(&k.0) && !self.phis.contains_key(&k.1) =>
                {
                    // External input other than entry EXEC: arbitrary bits.
                    owner.insert(k, Owner::Top);
                }
                _ if p.wave64 => {
                    let b0 = self
                        .defs
                        .get(&k.0)
                        .map(|&(b, _)| b)
                        .or_else(|| self.phis.get(&k.0).map(|&(b, _)| b));
                    let b1 = self
                        .defs
                        .get(&k.1)
                        .map(|&(b, _)| b)
                        .or_else(|| self.phis.get(&k.1).map(|&(b, _)| b));
                    let def_b = match (b0, b1) {
                        (Some(a), Some(b)) => self.lca_idom(a, b),
                        (Some(a), None) | (None, Some(a)) => a,
                        (None, None) => entry,
                    };
                    owner.insert(k, Owner::Empty);
                    split_keys.push((k, def_b));
                }
                _ => {
                    owner.insert(k, Owner::Top);
                }
            }
        }

        let effective_owner = |b: usize,
                               k: Key,
                               owner: &Map<Key, Owner>,
                               lanes_arrive: &mut dyn FnMut(usize, usize) -> bool|
         -> Owner {
            let o = owner.get(&k).copied().unwrap_or(Owner::Top);
            let Owner::Block(mut u) = o else {
                return o;
            };
            if let Some(edges) = exec_narrow.get(&k) {
                for &(d, t) in edges {
                    if self.dominates(t, b) && lanes_arrive(u, d) {
                        u = t;
                    }
                }
            }
            Owner::Block(u)
        };
        let op_owner = |b: usize,
                        inst: &Inst,
                        op: &Operand,
                        owner: &Map<Key, Owner>,
                        lanes_arrive: &mut dyn FnMut(usize, usize) -> bool|
         -> Owner {
            match *op {
                Operand::Scalar { code: NULL, .. } | Operand::Const(128) | Operand::Literal(0) => {
                    Owner::Empty
                }
                Operand::Scalar { code, .. } => match self.key(|r| inst.read(r), code) {
                    Some(k) => effective_owner(b, k, owner, lanes_arrive),
                    None => Owner::Top,
                },
                _ => Owner::Top,
            }
        };
        let combine_owner = |f: &str, a: Owner, c: Owner| -> Owner {
            match f {
                "and" => and_owner(a, c),
                "andn2" => a,
                "andn1" => c,
                "or" | "xor" => join_owner(a, c),
                _ => Owner::Top,
            }
        };

        let half_owner = |b: usize,
                          v: Value,
                          high: bool,
                          owner: &Map<Key, Owner>,
                          lanes_arrive: &mut dyn FnMut(usize, usize) -> bool|
         -> Owner {
            let v = self.unmov32(v);
            if let Some(c) = self.const_u32(v) {
                return if c == 0 { Owner::Empty } else { Owner::Top };
            }
            if let Some(k) = self.known_half_key(v, high) {
                return effective_owner(b, k, owner, lanes_arrive);
            }
            Owner::Top
        };

        let max_iters = n + 4;
        for iter in 0..max_iters {
            let mut changed = false;
            // Recompute non-EXEC phis.
            for &(k, block, ref args) in &phi_keys {
                let mut acc = Owner::Empty;
                for (idx, &a) in args.iter().enumerate() {
                    let (q, _) = self.incoming[block][idx];
                    let mut o = effective_owner(q, a, &owner, &mut lanes_arrive);
                    if self.index[q] != usize::MAX && self.index[block] <= self.index[q] {
                        o = match o {
                            Owner::Empty => Owner::Empty,
                            Owner::Top => Owner::Top,
                            Owner::Block(u) => {
                                if block == entry {
                                    Owner::Top
                                } else {
                                    Owner::Block(self.lca_idom(u, self.idom[block].unwrap()))
                                }
                            }
                        };
                    }
                    acc = join_owner(acc, o);
                }
                if owner.get(&k).copied() != Some(acc) {
                    owner.insert(k, acc);
                    changed = true;
                }
            }
            // Walk instructions in RPO.
            for &b in &self.order {
                for (i, inst) in p.blocks[b].insts.iter().enumerate() {
                    let Some(tracked) = inst_keys.get(&(b, i)) else {
                        continue;
                    };
                    let name = inst.name();
                    let mut updates: [(Option<Key>, Owner); 4] = [(None, Owner::Top); 4];
                    let mut n_updates = 0usize;
                    let mut push_update = |k: Key, o: Owner| {
                        if n_updates < updates.len() {
                            updates[n_updates] = (Some(k), o);
                            n_updates += 1;
                        }
                    };
                    match &inst.fields {
                        Fields::Valu(v) => {
                            if name.starts_with("v_cmpx") {
                                if let Some(k) = self.key(|r| inst.write(r), EXEC) {
                                    push_update(k, Owner::Block(b));
                                }
                            } else {
                                let compare = v.dst.filter(|_| name.starts_with("v_cmp"));
                                for op in compare.iter().chain(&v.carry) {
                                    if let Operand::Scalar { code, .. } = *op
                                        && let Some(k) = self.key(|r| inst.write(r), code)
                                    {
                                        push_update(k, Owner::Block(b));
                                    }
                                }
                            }
                        }
                        Fields::Sop { dst, src, .. } => {
                            let width = if p.wave64 { "_b64" } else { "_b32" };
                            if let Some(op) =
                                name.strip_prefix("s_").and_then(|s| s.strip_suffix(width))
                            {
                                let dst_k = match dst {
                                    Some(Operand::Scalar { code, .. }) if *code != NULL => {
                                        self.key(|r| inst.write(r), *code)
                                    }
                                    _ => None,
                                };
                                let exec_out = self.key(|r| inst.write(r), EXEC);
                                if let Some(f) = op.strip_suffix("_saveexec") {
                                    let old_exec = self
                                        .key(|r| inst.read(r), EXEC)
                                        .map_or(Owner::Top, |ek| {
                                            effective_owner(b, ek, &owner, &mut lanes_arrive)
                                        });
                                    let s0 = src.first().map_or(Owner::Top, |s| {
                                        op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                    });
                                    let new_exec = combine_owner(f, s0, old_exec);
                                    if let Some(k) = dst_k {
                                        push_update(k, old_exec);
                                    }
                                    if let Some(k) = exec_out {
                                        push_update(k, new_exec);
                                    }
                                } else if let Some(f) = op.strip_suffix("_wrexec") {
                                    let old_exec = self
                                        .key(|r| inst.read(r), EXEC)
                                        .map_or(Owner::Top, |ek| {
                                            effective_owner(b, ek, &owner, &mut lanes_arrive)
                                        });
                                    let s0 = src.first().map_or(Owner::Top, |s| {
                                        op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                    });
                                    let new_exec = combine_owner(f, s0, old_exec);
                                    if let Some(k) = dst_k {
                                        push_update(k, new_exec);
                                    }
                                    if let Some(k) = exec_out {
                                        push_update(k, new_exec);
                                    }
                                } else {
                                    let res = match op {
                                        "mov" => src.first().map_or(Owner::Top, |s| {
                                            op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                        }),
                                        // `s_wqm` activates helper lanes of quads already running
                                        // block `b`, which execute block `b` alongside live lanes.
                                        "wqm" => src.first().map_or(Owner::Top, |s| {
                                            op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                        }),
                                        "cselect" => {
                                            let a = src.first().map_or(Owner::Top, |s| {
                                                op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                            });
                                            let c = src.get(1).map_or(Owner::Top, |s| {
                                                op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                            });
                                            join_owner(a, c)
                                        }
                                        "cmov" => {
                                            let a = src.first().map_or(Owner::Top, |s| {
                                                op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                            });
                                            let c = dst.as_ref().map_or(Owner::Top, |s| {
                                                op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                            });
                                            join_owner(a, c)
                                        }
                                        _ => {
                                            let a = src.first().map_or(Owner::Top, |s| {
                                                op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                            });
                                            let c = src.get(1).map_or(Owner::Top, |s| {
                                                op_owner(b, inst, s, &owner, &mut lanes_arrive)
                                            });
                                            combine_owner(op, a, c)
                                        }
                                    };
                                    if let Some(k) = dst_k {
                                        push_update(k, res);
                                    }
                                    if let Some(k) = exec_out {
                                        push_update(k, res);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    for &(k_opt, o) in &updates[..n_updates] {
                        if let Some(k) = k_opt
                            && owner.get(&k).copied() != Some(o)
                        {
                            owner.insert(k, o);
                            changed = true;
                        }
                    }
                    // Any tracked key written by an unhandled instruction (e.g. `s_load`,
                    // `s_movk_i32`, `s_bfm`, `v_readfirstlane`) defaults to Top unless
                    // it is a known constant 0.
                    for &k in tracked {
                        if !updates[..n_updates].iter().any(|&(u, _)| u == Some(k)) {
                            let o = if self.const_u32(k.0) == Some(0)
                                && (!p.wave64 || self.const_u32(k.1) == Some(0))
                            {
                                Owner::Empty
                            } else {
                                Owner::Top
                            };
                            if owner.get(&k).copied() != Some(o) {
                                owner.insert(k, o);
                                changed = true;
                            }
                        }
                    }
                }
            }
            // Split-half keys in `self.masks` (wave64 only).
            for &(k, def_b) in &split_keys {
                let (r0, r1) = (self.unmov32(k.0), self.unmov32(k.1));
                let o = if (r0, r1) != k {
                    effective_owner(def_b, (r0, r1), &owner, &mut lanes_arrive)
                } else {
                    let o0 = half_owner(def_b, k.0, false, &owner, &mut lanes_arrive);
                    let o1 = half_owner(def_b, k.1, true, &owner, &mut lanes_arrive);
                    join_owner(o0, o1)
                };
                if owner.get(&k).copied() != Some(o) {
                    owner.insert(k, o);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
            if iter + 1 == max_iters {
                // Sound fallback if unconverged: mark remaining phis Top.
                for &(k, _, _) in &phi_keys {
                    owner.insert(k, Owner::Top);
                }
            }
        }

        let mut active = HashSet::new();
        for &k in self.masks.keys() {
            let def_b = self
                .defs
                .get(&k.0)
                .map(|&(b, _)| b)
                .or_else(|| self.phis.get(&k.0).map(|&(b, _)| b))
                .unwrap_or(entry);
            let o = effective_owner(def_b, k, &owner, &mut lanes_arrive);
            let ok = match o {
                Owner::Empty => true,
                Owner::Block(u) => lanes_arrive(u, def_b),
                Owner::Top => false,
            };
            if ok {
                active.insert(k.0);
            }
        }
        (active, div_loops)
    }
}

fn compute_idoms(
    order: &[usize],
    index: &[usize],
    incoming: &[Vec<(usize, bool)>],
) -> Vec<Option<usize>> {
    let mut idom = vec![None; index.len()];
    let Some(&entry) = order.first() else {
        return idom;
    };
    idom[entry] = Some(entry);
    let intersect = |mut a: usize, mut b: usize, idom: &[Option<usize>]| {
        while a != b {
            while index[a] > index[b] {
                a = idom[a].unwrap();
            }
            while index[b] > index[a] {
                b = idom[b].unwrap();
            }
        }
        a
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order[1..] {
            let mut new_idom = None;
            for &(p, _) in &incoming[b] {
                if idom[p].is_some() {
                    new_idom = Some(match new_idom {
                        None => p,
                        Some(d) => intersect(p, d, &idom),
                    });
                }
            }
            if new_idom != idom[b] {
                idom[b] = new_idom;
                changed = true;
            }
        }
    }
    idom
}

const TOO_LARGE: ShaderError = ShaderError::Isa {
    offset: 0,
    reason: "lane conditions too large",
};

/// `ir::build` splits irreducible loops; checked again because everything
/// here assumes a reducible graph.
fn check_reducible(p: &Program, order: &[usize], index: &[usize]) -> Result<(), ShaderError> {
    match crate::ir::irreducible_edge(&p.blocks, order, index) {
        Some((from, _)) => Err(ShaderError::Unsupported {
            offset: p.blocks[from].offset,
            what: "irreducible control flow",
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn build(words: &[u32], wave64: bool) -> Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        ir::build(&code, wave64, 256).unwrap()
    }

    fn guarded(s: &Structured) -> usize {
        s.guards.iter().flatten().filter(|g| g.is_some()).count()
    }

    /// The search of [`Bools::meet`] with every case split made anew, as it
    /// was before [`Meets`].
    fn meet_anew(b: &Bools, mut all: Vec<(Bool, bool)>, budget: &mut usize) -> bool {
        let fails = |&(b, holds): &(Bool, bool)| b == if holds { Bool::FALSE } else { Bool::TRUE };
        if all.iter().any(fails) {
            return false;
        }
        all.retain(|&(b, _)| b != Bool::FALSE && b != Bool::TRUE);
        if all.is_empty() || *budget == 0 {
            return true;
        }
        *budget -= 1;
        let top = |x: Bool| b.nodes[x.0 as usize].0;
        let v = all.iter().map(|&(x, _)| top(x)).min().expect("not empty");
        [false, true].into_iter().any(|value| {
            let next = all
                .iter()
                .map(|&(x, holds)| {
                    let (lo, hi) = b.split(x, v);
                    (if value { hi } else { lo }, holds)
                })
                .collect();
            meet_anew(b, next, budget)
        })
    }

    #[test]
    fn a_search_that_remembers_answers_as_one_made_anew() {
        // Functions of 12 variables put together at random, and conditions
        // of two to five of them (repeats and constants among them), under
        // budgets that run out early, late and never: the answer, and where
        // no lane is found the budget left, are those of the search that
        // remembers nothing; with what one `Meets` gathered over all of
        // them, and with an empty one.
        let mut b = Bools::new();
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut random = move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        let mut f: Vec<Bool> = (0..12).map(|i| b.atom(Atom::Constant(i + 2), 0)).collect();
        f.extend([Bool::FALSE, Bool::TRUE]);
        for _ in 0..120 {
            let (x, y) = (f[random(f.len())], f[random(f.len())]);
            let made = match random(4) {
                0 => b.and(x, y),
                1 => b.or(x, y),
                2 => b.xor(x, y),
                _ => b.not(x),
            };
            f.push(made);
        }
        let mut seen = Meets::default();
        let (mut none, mut some, mut gave_up) = (0, 0, 0);
        for _ in 0..1500 {
            let all: Vec<(Bool, bool)> = (0..2 + random(4))
                .map(|_| (f[random(f.len())], random(2) == 0))
                .collect();
            for budget in [0, 1, 2, 3, 5, 9, 20, 60, 250, 1000, MAX_MEET] {
                let mut left = budget;
                let anew = meet_anew(&b, all.clone(), &mut left);
                for fresh in [false, true] {
                    let mut empty = Meets::default();
                    let seen = if fresh { &mut empty } else { &mut seen };
                    let mut work = all
                        .iter()
                        .map(|&(x, holds)| x.0 << 1 | u32::from(holds))
                        .collect();
                    let mut mine = budget;
                    let found = b.meet_in(0, &mut work, &mut mine, seen);
                    assert_eq!(found, anew, "{all:?} within {budget}");
                    assert!(work.is_empty());
                    if !found {
                        assert_eq!(mine, left, "{all:?} within {budget}");
                    }
                }
                assert_eq!(b.meet_within(&all, budget, &mut seen), anew);
                if !anew {
                    none += 1;
                } else if meet_anew(&b, all.clone(), &mut usize::MAX.clone()) {
                    some += 1;
                } else {
                    gave_up += 1;
                }
            }
        }
        // All three outcomes came up, and searches were remembered.
        assert!(
            none > 100 && some > 100 && gave_up > 100,
            "{none} {some} {gave_up}"
        );
        assert!(seen.none.len() > 100);
    }

    #[test]
    fn divergent_if_becomes_a_lane_branch() {
        let p = build(
            &[
                0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                0xBE80_3C6A, // s_and_saveexec_b32 s0, vcc_lo
                0xBF88_0001, // s_cbranch_execz -> 16
                0x7E02_0280, // v_mov_b32 v1, 0
                0x887E_007E, // 16: s_or_b32 exec_lo, exec_lo, s0
                0x7E04_0280, // v_mov_b32 v2, 0
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let LaneExit::Branch {
            when,
            taken: 2,
            not_taken: 1,
        } = s.exits[0]
        else {
            panic!("{:?}", s.exits)
        };
        // Skipped exactly where the compare fails.
        let cmp = p.blocks[0].insts[0].write(Reg::S(VCC)).unwrap();
        assert_eq!(
            s.bools.node(when),
            Some((Atom::Lane(cmp), Bool::TRUE, Bool::FALSE))
        );
        assert_eq!(guarded(&s), 0, "{:?}", s.guards);
    }

    #[test]
    fn exec_narrowed_without_a_branch_leaves_a_guard() {
        let p = build(
            &[
                0x7DA4_0080, // v_cmpx_eq_u32 exec, 0, v0
                0x7E02_0280, // v_mov_b32 v1, 0
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        assert_eq!(s.guards[0][0], None, "the compare runs in every lane");
        let g = s.guards[0][1].expect("guard");
        assert!(matches!(s.bools.support(g)[..], [Atom::Lane(_)]));
    }

    #[test]
    fn guards_follow_exec_and_block_reachability() {
        let p = build(
            &[
                0x7DA4_0080, // v_cmpx_eq_u32 exec, 0, v0
                0x7E02_0280, // v_mov_b32 v1, 0
                0x7E04_0280, // v_mov_b32 v2, 0
                0xBF88_0005, // s_cbranch_execz -> end
                0x7E06_0280, // v_mov_b32 v3, 0
                0x7DA4_0880, // v_cmpx_eq_u32 exec, 0, v4
                0x7E06_0280, // v_mov_b32 v3, 0
                0xBE82_0480, // s_mov_b64 s[2:3], 0
                0x7E0A_0280, // v_mov_b32 v5, 0
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let lane = |block: usize, i: usize| {
            Atom::Lane(p.blocks[block].insts[i].write(Reg::S(EXEC)).unwrap())
        };
        let a = s.guards[0][1].unwrap();
        let b = s.guards[1][2].unwrap();
        assert_eq!(s.bools.node(a), Some((lane(0, 0), Bool::FALSE, Bool::TRUE)));
        assert_eq!(s.guards[0], [None, Some(a), Some(a), None]);
        // The fallthrough already implies the first compare. Only the second
        // compare needs testing, and the scalar move never needs a guard.
        assert_eq!(s.bools.node(b), Some((lane(1, 1), Bool::FALSE, Bool::TRUE)));
        assert_eq!(s.guards[1], [None, None, Some(b), None, Some(b)]);
    }

    #[test]
    fn divergent_loop_exits_per_lane() {
        let p = build(
            &[
                0xBE82_0480, // s_mov_b64 s[2:3], 0
                0x4A00_0081, // 4: v_add_nc_u32 v0, 1, v0
                0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                0x8882_026A, // s_or_b64 s[2:3], vcc, s[2:3]
                0x8AFE_027E, // s_andn2_b64 exec, exec, s[2:3]
                0xBF89_FFFB, // s_cbranch_execnz -> 4
                0x88FE_027E, // s_or_b64 exec, exec, s[2:3]
                0x7E02_0280, // v_mov_b32 v1, 0
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        assert_eq!(guarded(&s), 0, "{:?}", s.guards);
        let LaneExit::Branch {
            when,
            taken: 1,
            not_taken: 2,
        } = s.exits[1]
        else {
            panic!("{:?}", s.exits)
        };
        // Back while the lane's compare fails.
        assert!(matches!(
            s.bools.node(when),
            Some((Atom::Lane(_), Bool::TRUE, Bool::FALSE))
        ));
    }

    /// `s_cselect` on the SCC a lane loop ended with (the blend-shape skinning
    /// programs): the loop is left when no lane remains, so the select takes
    /// its second operand; it was a uniform atom whose value no lane has.
    #[test]
    fn a_select_on_the_condition_a_lane_loop_ended_with_takes_its_second_operand() {
        let cnd = 256 | (256 << 9) | (3 << 18);
        let p = build(
            &[
                0xBE83_037E, // 0: s_mov_b32 s3, exec_lo
                0xBE9C_037E, // s_mov_b32 s28, exec_lo
                0x7E3A_0507, // 8: v_readfirstlane_b32 s29, v7
                0x7DA4_0E1D, // v_cmpx_eq_u32 exec_lo, s29, v7
                0xBE9C_471C, // s_andn2_wrexec_b32 s28, s28
                0xBF89_FFFC, // s_cbranch_execnz -> 8
                0xBEFE_0303, // s_mov_b32 exec_lo, s3
                0x8503_807E, // s_cselect_b32 s3, exec_lo, 0
                0xD501_0001,
                cnd, // v_cndmask_b32 v1, v0, v0, s3
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let select = p
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .find(|i| i.name() == "s_cselect_b32")
            .expect("the select");
        let mask = select.write(Reg::S(3)).unwrap();
        assert_eq!(s.masks.get(&mask), Some(&Bool::FALSE));
    }

    #[test]
    fn scalar_branches_stay_uniform() {
        let p = build(
            &[
                0xBF06_0100, // s_cmp_eq_u32 s0, s1
                0xBF85_0001, // s_cbranch_scc1 -> 12
                0x7E02_0280, // v_mov_b32 v1, 0
                S_ENDPGM,    // 12
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let LaneExit::Branch { when, .. } = s.exits[0] else {
            panic!()
        };
        assert!(matches!(s.bools.support(when)[..], [Atom::Uniform(_)]));
        assert_eq!(guarded(&s), 0);
    }

    #[test]
    fn a_mask_whose_high_dword_is_rewritten_is_not_the_compare() {
        for reader in [
            0x0202_0480, // v_cndmask_b32 v1, 0, v2, vcc
            0x5002_0480, // v_add_co_ci_u32 v1, vcc, 0, v2, vcc
        ] {
            let p = build(
                &[
                    0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                    0xBEEB_0380, // s_mov_b32 vcc_hi, 0
                    reader,
                    S_ENDPGM,
                ],
                true,
            );
            let s = structure(&p).unwrap();
            let lo = p.blocks[0].insts[2].read(Reg::S(VCC)).unwrap();
            assert_eq!(p.blocks[0].insts[0].write(Reg::S(VCC)), Some(lo));
            // The reader sees (compare low dword, 0): lanes 32-63 are clear.
            // The mask exported for that low dword must not be the compare.
            assert_ne!(
                s.masks.get(&lo).and_then(|&m| s.bools.node(m)),
                Some((Atom::Lane(lo), Bool::FALSE, Bool::TRUE)),
                "{reader:#x}: vcc_hi was zeroed after the compare"
            );
        }
    }

    #[test]
    fn a_mask_joined_over_a_back_edge_with_a_rewritten_high_dword_is_not_the_compare() {
        let p = build(
            &[
                0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
                0x0202_0480, // 4: v_cndmask_b32 v1, 0, v2, vcc
                0x7D84_0280, // v_cmp_eq_u32 vcc, 0, v1
                0xBEEB_0380, // s_mov_b32 vcc_hi, 0
                0xBF85_FFFC, // s_cbranch_scc1 -> 4
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        // The select reads the loop header's join; over the back edge it
        // takes (compare low dword, 0), which only the join sees.
        let lo = p.blocks[1].insts[1].write(Reg::S(VCC)).unwrap();
        assert_ne!(
            s.masks.get(&lo).and_then(|&m| s.bools.node(m)),
            Some((Atom::Lane(lo), Bool::FALSE, Bool::TRUE))
        );
    }

    #[test]
    fn irreducible_loops_are_split() {
        let p = build(
            &[
                0xBF06_0100, // s_cmp_eq_u32 s0, s1
                0xBF85_0001, // s_cbranch_scc1 -> 12, into the loop's middle
                0x8002_8102, // 8: s_add_u32 s2, s2, 1
                0xBF84_FFFE, // 12: s_cbranch_scc0 -> 8
                S_ENDPGM,
            ],
            false,
        );
        // The loop keeps 12 as its header; the entry into 8 gets a copy of it.
        let copy = &p.blocks[4];
        assert_eq!((copy.offset, copy.exit), (8, ir::Exit::Jump(2)));
        assert_eq!(p.blocks[0].exit.successors(), [2, 4]);
        assert_eq!(p.blocks[1].preds, [2]);
        assert!(structure(&p).is_ok());
    }

    #[test]
    fn mask_phi_joining_constant_and_compare_records_edge_bools() {
        let cnd = 258 | (259 << 9) | (2 << 18);
        let p = build(
            &[
                0xBF06_8000, // 0: s_cmp_eq_u32 s0, 0
                0xBF85_0002, // 4: s_cbranch_scc1 -> 16
                0xBE82_0380, // 8: s_mov_b32 s2, 0
                0xBF82_0002, // 12: s_branch -> 24
                0xD4C2_0002,
                0x0002_0080, // 16: v_cmp_eq_u32 s2, 0, v0
                0xD501_0001,
                cnd, // 24: v_cndmask_b32 v1, v2, v3, s2
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let phi = p
            .blocks
            .iter()
            .flat_map(|b| &b.phis)
            .find(|phi| phi.reg == Reg::S(2))
            .expect("mask phi for s2");
        assert_eq!(
            s.masks.get(&phi.value).and_then(|&m| s.bools.node(m)),
            Some((Atom::Phi(phi.value), Bool::FALSE, Bool::TRUE))
        );
        let edges = &s.phi_masks[&phi.value];
        assert_eq!(edges.len(), 2);
        assert!(edges.contains(&Bool::FALSE), "{edges:?}");
        assert!(
            edges
                .iter()
                .any(|&e| matches!(s.bools.support(e)[..], [Atom::Lane(_)])),
            "{edges:?}"
        );
    }

    #[test]
    fn loop_mask_accumulator_from_zero_tracks_phi_and_edges() {
        let cnd = 258 | (259 << 9) | (2 << 18);
        let p = build(
            &[
                0xBE82_0480, // 0: s_mov_b64 s[2:3], 0
                0xBE84_0380, // 4: s_mov_b32 s4, 0
                0xD501_0001,
                cnd,         // 8: v_cndmask_b32 v1, v2, v3, s[2:3]
                0x7D84_0080, // 16: v_cmp_eq_u32 vcc, 0, v0
                0x8882_026A, // 20: s_or_b64 s[2:3], vcc, s[2:3]
                0x8004_8104, // 24: s_add_u32 s4, s4, 1
                0xBF0A_8404, // 28: s_cmp_lt_u32 s4, 4
                0xBF85_FFF9, // 32: s_cbranch_scc1 -> 8
                0xD501_0004,
                cnd, // 36: v_cndmask_b32 v4, v2, v3, s[2:3]
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let phi = p
            .blocks
            .iter()
            .flat_map(|b| &b.phis)
            .find(|phi| phi.reg == Reg::S(2))
            .expect("loop mask phi for s[2:3]");
        assert!(s.masks.contains_key(&phi.value));
        let edges = &s.phi_masks[&phi.value];
        assert_eq!(edges.len(), 2);
        assert!(edges.contains(&Bool::FALSE), "{edges:?}");
        assert!(edges.iter().any(|&e| {
            let sup = s.bools.support(e);
            sup.contains(&Atom::Phi(phi.value)) && sup.iter().any(|a| matches!(a, Atom::Lane(_)))
        }));
    }

    #[test]
    fn loop_mask_phi_reset_to_constant_on_back_edge() {
        let cnd = 258 | (259 << 9) | (2 << 18);
        let p = build(
            &[
                0xD4C2_0002,
                0x0002_0080, // 0: v_cmp_eq_u32 s2, 0, v0
                0xBE84_0380, // 8: s_mov_b32 s4, 0
                0xD501_0001,
                cnd,         // 12: v_cndmask_b32 v1, v2, v3, s2
                0xBE82_0380, // 20: s_mov_b32 s2, 0
                0x8004_8104, // 24: s_add_u32 s4, s4, 1
                0xBF0A_8404, // 28: s_cmp_lt_u32 s4, 4
                0xBF85_FFFA, // 32: s_cbranch_scc1 -> 12
                S_ENDPGM,
            ],
            false,
        );
        let s = structure(&p).unwrap();
        let phi = p
            .blocks
            .iter()
            .flat_map(|b| &b.phis)
            .find(|phi| phi.reg == Reg::S(2))
            .expect("loop mask phi for s2");
        assert!(s.masks.contains_key(&phi.value));
        let edges = &s.phi_masks[&phi.value];
        assert_eq!(edges.len(), 2);
        assert!(edges.contains(&Bool::FALSE), "{edges:?}");
        assert!(
            edges
                .iter()
                .any(|&e| matches!(s.bools.support(e)[..], [Atom::Lane(_)])),
            "{edges:?}"
        );
    }

    #[test]
    fn bools_implies_disjoint_given_and_gc_preserve_truth_tables() {
        let mut b = Bools::new();
        let mut state = 0x243F_6A88_85A3_08D3_u64;
        let mut random = move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        let vars: Vec<Bool> = (0..10).map(|i| b.atom(Atom::Constant(i + 2), 0)).collect();
        let mut f = vars.clone();
        f.extend([Bool::FALSE, Bool::TRUE]);
        for _ in 0..80 {
            let (x, y) = (f[random(f.len())], f[random(f.len())]);
            let made = match random(6) {
                0 => b.and(x, y),
                1 => b.or(x, y),
                2 => b.xor(x, y),
                3 => b.andn2(x, y),
                4 => b.orn2(x, y),
                _ => b.not(x),
            };
            f.push(made);
        }
        let eval = |bools: &Bools, mut node: Bool, mask: u32| -> bool {
            loop {
                let (v, lo, hi) = bools.nodes[node.0 as usize];
                if v == TERMINAL {
                    return node == Bool::TRUE;
                }
                node = if (mask >> v) & 1 != 0 { hi } else { lo };
            }
        };
        for _ in 0..400 {
            let (x, y) = (f[random(f.len())], f[random(f.len())]);
            let before = b.nodes.len();
            let imp = b.implies(x, y);
            let disj = b.disjoint(x, y);
            assert_eq!(b.nodes.len(), before, "implies/disjoint must allocate 0 nodes");
            let mut truth_imp = true;
            let mut truth_disj = true;
            for m in 0..(1u32 << 10) {
                let vx = eval(&b, x, m);
                let vy = eval(&b, y, m);
                if vx && !vy {
                    truth_imp = false;
                }
                if vx && vy {
                    truth_disj = false;
                }
            }
            assert_eq!(imp, truth_imp);
            assert_eq!(disj, truth_disj);
            let g = b.given(x, y);
            for m in 0..(1u32 << 10) {
                if eval(&b, y, m) {
                    assert_eq!(eval(&b, g, m), eval(&b, x, m));
                }
            }
        }
        // Dead temporary BDDs are reclaimed by `gc_remap` while preserving live roots.
        let live: Vec<Bool> = f[..20].to_vec();
        let expected: Vec<Vec<bool>> = live
            .iter()
            .map(|&r| (0..(1u32 << 10)).map(|m| eval(&b, r, m)).collect())
            .collect();
        for _ in 0..400 {
            let (x, y) = (f[random(f.len())], f[random(f.len())]);
            let _ = b.xor(x, y);
        }
        let before_gc = b.nodes.len();
        let remap = b.gc_remap(&live);
        assert!(b.nodes.len() < before_gc);
        for (idx, &old) in live.iter().enumerate() {
            let new = remap[old.0 as usize];
            for m in 0..(1u32 << 10) {
                assert_eq!(eval(&b, new, m), expected[idx][m as usize]);
            }
        }
    }

    #[test]
    fn wave64_split_half_masks_and_cmov_and_multiblock_phis_are_tracked() {
        // 1. Two `s_mov_b32` halves copying a wave64 compare (as `ir::unspill` emits).
        let p = build(
            &[
                0xD4C20002, 0x02020080, // v_cmp_eq_u32_e64 s[2:3], 0, v0
                0xBE840302,             // s_mov_b32 s4, s2
                0xBE850303,             // s_mov_b32 s5, s3
                0xD5010001, 0x00120702, // v_cndmask_b32_e64 v1, v2, v3, s[4:5]
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let cmp = p.blocks[0].insts[0].write(Reg::S(2)).unwrap();
        let lo = p.blocks[0].insts[3].read(Reg::S(4)).unwrap();
        let hi = p.blocks[0].insts[3].read(Reg::S(5)).unwrap();
        let m = s.mask_bits(lo, Some(hi)).expect("tracked split s_mov_b32 pair");
        assert_eq!(s.bools.node(m), Some((Atom::Lane(cmp), Bool::FALSE, Bool::TRUE)));

        // 2. Two `s_mov_b32` zeroing a 64-bit mask -> Bool::FALSE (no Atom::Opaque).
        let p = build(&[0xBE840380, 0xBE850380, 0x87FE047E, 0x7E020287, S_ENDPGM], true);
        let s = structure(&p).unwrap();
        assert_eq!(s.guards[0][3], Some(Bool::FALSE));

        // 3. Half-wise `s_and_b32` combining two wave64 compares -> exact AND of both lanes.
        let p = build(
            &[
                0xD4C20002, 0x02020080, // v_cmp_eq_u32_e64 s[2:3], 0, v0
                0xD4C20004, 0x02020281, // v_cmp_eq_u32_e64 s[4:5], 1, v1
                0x87060402,             // s_and_b32 s6, s2, s4
                0x87070503,             // s_and_b32 s7, s3, s5
                0xD5010002, 0x001A0300, // v_cndmask_b32_e64 v2, v0, v1, s[6:7]
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let lo = p.blocks[0].insts[4].read(Reg::S(6)).unwrap();
        let hi = p.blocks[0].insts[4].read(Reg::S(7)).unwrap();
        let m = s.mask_bits(lo, Some(hi)).expect("tracked half-wise s_and_b32");
        let sup = s.bools.support(m);
        assert!(sup.iter().all(|a| matches!(a, Atom::Lane(_))), "{sup:?}");
        assert_eq!(sup.len(), 2);

        // 4. `s_cmov_b64` conditionally overwriting a compare mask with 0.
        let p = build(
            &[
                0xD4C20002, 0x02020080, // v_cmp_eq_u32_e64 s[2:3], 0, v0
                0xBF068000,             // s_cmp_eq_u32 s0, 0
                0xBE820680,             // s_cmov_b64 s[2:3], 0
                0xD5010001, 0x000A0702, // v_cndmask_b32_e64 v1, v2, v3, s[2:3]
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let lo = p.blocks[0].insts[3].read(Reg::S(2)).unwrap();
        let hi = p.blocks[0].insts[3].read(Reg::S(3)).unwrap();
        let m = s.mask_bits(lo, Some(hi)).expect("tracked s_cmov_b64");
        let sup = s.bools.support(m);
        assert!(sup.iter().any(|a| matches!(a, Atom::Lane(_))) && sup.iter().any(|a| matches!(a, Atom::Uniform(_))), "{sup:?}");
        assert!(!sup.iter().any(|a| matches!(a, Atom::Opaque(_, _))));

        // 5. Multi-block phi join: low dword phi in outer merge, high dword phi in inner merge.
        let p = build(
            &[
                0xBF068000, 0xBF850003, 0xD4C20002, 0x02020080, 0xBF820002, 0xD4C20002,
                0x02020081, 0xBF068001, 0xBF850001, 0xBE830380, 0xD5010001, 0x000A0702,
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let cnd = p.blocks.iter().flat_map(|b| &b.insts).find(|i| i.name() == "v_cndmask_b32").unwrap();
        let lo = cnd.read(Reg::S(2)).unwrap();
        let hi = cnd.read(Reg::S(3)).unwrap();
        let m = s.mask_bits(lo, Some(hi)).expect("multi-block phi mask");
        let sup = s.bools.support(m);
        assert!(sup.iter().any(|a| matches!(a, Atom::Phi(_))), "{sup:?}");
        assert!(!sup.iter().any(|a| matches!(a, Atom::Opaque(_, _))), "{sup:?}");
    }

    #[test]
    fn active_soundness_in_divergent_loops_and_completeness_at_block_entries() {
        // 1. Divergent loop with `s_not_b64` and loop-carried accumulator `s[2:3]`:
        // neither `s_not_b64` nor the accumulator inside the loop may be in `active`,
        // while `vcc` inside the loop and restored `exec` after the loop ARE active.
        let p = build(
            &[
                0xBE820480, // s_mov_b64 s[2:3], 0
                0x4A000081, // L_loop: v_add_nc_u32 v0, 1, v0
                0x7D840080, // v_cmp_eq_u32 vcc, 0, v0
                0xBE84086A, // s_not_b64 s[4:5], vcc
                0x88826A02, // s_or_b64 s[2:3], s[2:3], vcc
                0x8AFE6A7E, // s_andn2_b64 exec, exec, vcc
                0xBF89FFFA, // s_cbranch_execnz L_loop
                0x88FE027E, // s_or_b64 exec, exec, s[2:3]
                0x7E020280, // v_mov_b32 v1, 0
                S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let loop_blk = &p.blocks[1];
        let vcc = loop_blk.insts[1].write(Reg::S(VCC)).unwrap();
        let not_vcc = loop_blk.insts[2].write(Reg::S(4)).unwrap();
        let acc_step = loop_blk.insts[3].write(Reg::S(2)).unwrap();
        let exit_exec = p.blocks[2].insts[0].write(Reg::S(EXEC)).unwrap();
        assert!(s.active.contains(&vcc), "vcc in loop is active");
        assert!(!s.active.contains(&not_vcc), "s_not_b64 in divergent loop must NOT be active");
        assert!(!s.active.contains(&acc_step), "loop-carried accumulator inside divergent loop must NOT be active");
        assert!(s.active.contains(&exit_exec), "restored exec after divergent loop reconvergence IS active");

        // 2. `s_and_b64 s[6:7], s[8:9], exec` at the start of a block before any EXEC read
        // (previously missed because `now` started as `None` at each block).
        let p = build(&[0xBF068000, 0xBF850002, 0x87867E08, 0x7E020281, S_ENDPGM], true);
        let s = structure(&p).unwrap();
        let and_mask = p.blocks[1].insts[0].write(Reg::S(6)).unwrap();
        assert!(s.active.contains(&and_mask), "s_and_b64 with exec at block entry is active");

        // 3. Acyclic divergent `if` merge phi joining 0 and a compare in the `then` branch.
        let p = build(
            &[
                0xBE840480, 0x7D840080, 0xBE82246A, 0xBF880002, 0xD4C20004, 0x02020281,
                0x88FE027E, 0xBE861004, S_ENDPGM,
            ],
            true,
        );
        let s = structure(&p).unwrap();
        let phi = p.blocks[2].phis.iter().find(|ph| ph.reg == Reg::S(4)).unwrap();
        assert!(s.active.contains(&phi.value), "reconverged phi(0, vcc_in_then) is active");
    }
}
