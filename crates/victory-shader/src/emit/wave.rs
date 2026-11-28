//! Whole-wave sections (decision 0003): code between `s_or_saveexec -1`
//! (or `s_nand_saveexec 0`) and the restore of the saved mask runs in every lane of the console's
//! wave, the inactive ones included, which is how the compiler expands
//! HLSL wave reductions and prefix sums (an identity put into the inactive
//! lanes with `v_cndmask`, then DPP, `v_permlanex16` and `v_readlane`
//! steps). A host lane that is elsewhere cannot run that code, so each
//! section is simulated on the console's lanes with symbolic values, and a
//! value it leaves is lowered by what it turned out to be: the reduction or
//! prefix of one operation over the lanes' (masked) inputs, which DXIL's
//! wave operations compute over the host's active lanes. No instruction
//! sequence is matched; any step the simulation does not model leaves the
//! values it writes unknown.

use std::collections::HashMap;

use crate::gfx10::{Dpp, Fields, NULL, Operand};
use crate::ir::{EXEC, Inst, Program, Reg, Value};

/// An associative operation and its identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    AddU,
    AddF,
    /// A sum of 16-bit floats in the low halves (`v_add_f16`), each addition rounded to 16
    /// bits: not associative, so only the balanced tree over the lanes in their order is
    /// followed (see [`buddies`]), which is how the compiler sums a wave.
    AddH,
    MinU,
    MaxU,
    MinI,
    MaxI,
    MinF,
    MaxF,
    Or,
    And,
    Xor,
}

impl Op {
    fn of(name: &str) -> Option<Op> {
        Some(match name {
            // the low dword of a 64-bit sum (`v_add_co_ci_u32` carries into
            // the high one) is the 32-bit sum of the low dwords
            "v_add_nc_u32" | "v_add_nc_i32" | "v_add_co_u32" => Op::AddU,
            "v_add_f32" => Op::AddF,
            "v_add_f16" => Op::AddH,
            "v_min_u32" => Op::MinU,
            "v_max_u32" => Op::MaxU,
            "v_min_i32" => Op::MinI,
            "v_max_i32" => Op::MaxI,
            "v_min_f32" => Op::MinF,
            "v_max_f32" => Op::MaxF,
            "v_or_b32" => Op::Or,
            "v_and_b32" => Op::And,
            "v_xor_b32" => Op::Xor,
            _ => return None,
        })
    }

    fn of3(name: &str) -> Option<Op> {
        Some(match name {
            "v_add3_u32" => Op::AddU,
            "v_or3_b32" => Op::Or,
            "v_xor3_b32" => Op::Xor,
            "v_min3_u32" => Op::MinU,
            "v_max3_u32" => Op::MaxU,
            "v_min3_i32" => Op::MinI,
            "v_max3_i32" => Op::MaxI,
            "v_min3_f32" => Op::MinF,
            "v_max3_f32" => Op::MaxF,
            _ => return None,
        })
    }

    fn of_sop(name: &str) -> Option<Op> {
        Some(match name {
            "s_add_u32" | "s_add_i32" => Op::AddU,
            "s_min_u32" => Op::MinU,
            "s_max_u32" => Op::MaxU,
            "s_min_i32" => Op::MinI,
            "s_max_i32" => Op::MaxI,
            "s_or_b32" => Op::Or,
            "s_and_b32" => Op::And,
            "s_xor_b32" => Op::Xor,
            _ => return None,
        })
    }

    /// Whether `k` leaves the other operand unchanged.
    fn identity(self, k: u32) -> bool {
        match self {
            Op::AddU | Op::AddH | Op::Or | Op::Xor | Op::MaxU => k == 0,
            Op::AddF => k == 0 || k == 0x8000_0000,
            Op::MinU | Op::And => k == u32::MAX,
            Op::MinI => k == 0x7FFF_FFFF,
            Op::MaxI => k == 0x8000_0000,
            Op::MinF => k == 0x7F80_0000,
            Op::MaxF => k == 0xFF80_0000,
        }
    }

    /// Counting a lane twice changes the result.
    fn counts(self) -> bool {
        matches!(self, Op::AddU | Op::AddF | Op::AddH | Op::Xor)
    }
}

/// Whether the lane sets `a` and `b` are the two halves of one aligned run of lanes: lanes
/// 2k and 2k+1, then two such pairs side by side, and so on up to the two halves of the
/// wave. Sums joined only that way form the balanced tree over the lanes in their order,
/// whichever of the two a lane adds to the other (the addition is commutative).
fn buddies(a: u64, b: u64) -> bool {
    let n = a.count_ones();
    if n != b.count_ones() || !n.is_power_of_two() || n == 64 {
        return false;
    }
    let run = |s: u64| {
        let first = s.trailing_zeros();
        (first.is_multiple_of(n) && s >> first == (1u64 << n) - 1).then_some(first)
    };
    match (run(a), run(b)) {
        (Some(x), Some(y)) => x.min(y) % (2 * n) == 0 && x.abs_diff(y) == n,
        _ => false,
    }
}

/// A lane's value inside a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sym {
    Unknown,
    K(u32),
    /// The lane's own value of a VGPR value from before the section.
    Own(Value),
    /// A uniform value from before the section.
    Uni(Value),
    /// `op` over `lanes` of `mask ? src : ident` (`op` none: one lane).
    Red {
        op: Option<Op>,
        src: Value,
        mask: Value,
        ident: u32,
        lanes: u64,
    },
    /// Low (`!high`) or high (`high`) 32 bits of the 64-bit float sum over
    /// `lanes` of `mask ? f64(lo, hi) : 0.0`.
    Red64F {
        lo: Value,
        hi: Value,
        mask: Value,
        lanes: u64,
        high: bool,
    },
}

/// What a value a section leaves holds in every lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    K(u32),
    Value(Value),
    /// The lane's own `mask ? src : ident`.
    Masked(Value, Value, u32),
    /// `op` over every lane.
    Active(Op, Value, Value, u32),
    /// Low (`!high`) or high (`high`) 32 bits of the 64-bit float sum over every lane.
    ActiveF64(Value, Value, Value, bool),
    /// `op` over the lanes below (`inclusive`: and including) each lane.
    Prefix(Op, Value, Value, u32, bool),
}

/// The section starting at instruction `start` of a block: its restore
/// index and the shapes of the values it writes.
pub(crate) struct Section {
    pub end: usize,
    pub shapes: HashMap<Value, Shape>,
    /// What a `v_readlane_b32` of one lane takes, later in the block, from a
    /// VGPR the section leaves (by the scalar it writes): the last lanes of
    /// a reduction hold it before the first do.
    pub reads: HashMap<Value, Shape>,
}

/// Every whole-wave section of `block`, by start index.
pub(crate) fn sections(p: &Program, block: usize) -> HashMap<usize, Section> {
    let insts = &p.blocks[block].insts;
    let wave: u32 = if p.wave64 { 64 } else { 32 };
    let mut pre_consts: Option<HashMap<Value, u32>> = None;
    let mut out = HashMap::new();
    let mut i = 0;
    while i < insts.len() {
        let inst = &insts[i];
        // EXEC becomes every lane: `s_or_saveexec -1`, `s_orn2_saveexec -1`,
        // `s_nand_saveexec 0`, or `s_orn1_saveexec 0`.
        let all = match inst.name() {
            "s_or_saveexec_b64"
            | "s_or_saveexec_b32"
            | "s_orn2_saveexec_b64"
            | "s_orn2_saveexec_b32" => Some(Operand::Const(193)),
            "s_nand_saveexec_b64"
            | "s_nand_saveexec_b32"
            | "s_orn1_saveexec_b64"
            | "s_orn1_saveexec_b32" => Some(Operand::Const(128)),
            _ => None,
        };
        let start = all.is_some_and(
            |k| matches!(&inst.fields, Fields::Sop { src, .. } if src.first() == Some(&k)),
        );
        let saved = match &inst.fields {
            Fields::Sop {
                dst: Some(Operand::Scalar { code, .. }),
                ..
            } if start => inst.write(Reg::S(*code)),
            _ => None,
        };
        let Some(saved) = saved else {
            i += 1;
            continue;
        };
        let end = (i + 1..insts.len()).find(|&j| {
            insts[j].write(Reg::S(EXEC)).is_some()
                && insts[j].read(Reg::S(saved_code(inst))) == Some(saved)
        });
        let Some(end) = end else {
            break;
        };
        let consts = pre_consts.get_or_insert_with(|| collect_scalar_consts(p));
        let (vec, uni) = simulate(&insts[i + 1..end], wave, saved, consts);
        let mut reads = HashMap::new();
        for later in insts[end + 1..].iter().filter(|i| i.name() == "v_readlane_b32") {
            if let Fields::Valu(v) = &later.fields
                && let (Some(Operand::Scalar { code, .. }), [Operand::Vector { reg, .. }, lane]) =
                    (v.dst, &v.src[..])
                && let Some(left) = later.read(Reg::V(*reg)).and_then(|x| vec.get(&x))
                && let Some(w) = later.write(Reg::S(code))
                && let Some(l) = lane_number_in(later, lane, &uni, consts)
                && let Some(&sym) = left.get(l as usize)
                // (a lane's own value stays a lane read)
                && !matches!(sym, Sym::Own(_))
                && let Some(shape) = single(sym, wave)
            {
                reads.insert(w, shape);
            }
        }
        let shapes = shapes(vec, uni, wave);
        out.insert(i, Section { end, shapes, reads });
        i = end + 1;
    }
    out
}

/// Scalar 32-bit constants built before a section (e.g. hoisted `+inf`, `-inf`,
/// `INT_MAX`, `INT_MIN` for `v_cndmask_b32` or nibble tables for `v_permlane`).
fn collect_scalar_consts(p: &Program) -> HashMap<Value, u32> {
    let mut defs: HashMap<Value, &Inst> = HashMap::new();
    for block in &p.blocks {
        for inst in &block.insts {
            if matches!(inst.fields, Fields::Sop { .. })
                && matches!(
                    inst.name(),
                    "s_mov_b32" | "s_movk_i32" | "s_brev_b32" | "s_not_b32"
                )
            {
                for &(r, w) in &inst.writes {
                    if matches!(r, Reg::S(_)) {
                        defs.insert(w, inst);
                    }
                }
            }
        }
    }
    fn resolve(
        v: Value,
        defs: &HashMap<Value, &Inst>,
        cache: &mut HashMap<Value, Option<u32>>,
        depth: usize,
    ) -> Option<u32> {
        if let Some(&r) = cache.get(&v) {
            return r;
        }
        if depth > 8 {
            return None;
        }
        let inst = *defs.get(&v)?;
        let Fields::Sop { src, imm, .. } = &inst.fields else {
            return None;
        };
        let res = match inst.name() {
            "s_movk_i32" => Some(i32::from((*imm)? as i16) as u32),
            name @ ("s_mov_b32" | "s_brev_b32" | "s_not_b32") => {
                let raw = match *src.first()? {
                    Operand::Scalar { code: NULL, .. } => Some(0),
                    Operand::Scalar { code, .. } => inst
                        .read(Reg::S(code))
                        .and_then(|u| resolve(u, defs, cache, depth + 1)),
                    Operand::Const(c) => crate::resource::inline(c),
                    Operand::Literal(x) => Some(x),
                    _ => None,
                }?;
                Some(match name {
                    "s_brev_b32" => raw.reverse_bits(),
                    "s_not_b32" => !raw,
                    _ => raw,
                })
            }
            _ => None,
        };
        cache.insert(v, res);
        res
    }
    let mut cache = HashMap::new();
    let mut out = HashMap::new();
    for &v in defs.keys() {
        if let Some(k) = resolve(v, &defs, &mut cache, 0) {
            out.insert(v, k);
        }
    }
    out
}

fn saved_code(inst: &Inst) -> u8 {
    match &inst.fields {
        Fields::Sop {
            dst: Some(Operand::Scalar { code, .. }),
            ..
        } => *code,
        _ => NULL,
    }
}

/// The lane `l` reads with DPP, whether it reads 0 instead, and whether it
/// writes; `None` for patterns not modelled.
fn dpp_lane(d: Dpp, l: u32) -> Option<(u32, bool, bool)> {
    match d {
        Dpp::Lanes { sel, .. } => Some(((l & !7) | (sel >> (3 * (l & 7)) & 7), false, true)),
        Dpp::Row {
            ctrl,
            row_mask,
            bank_mask,
            bound_ctrl,
            ..
        } => {
            let (row, pos, n) = (l & !15, l & 15, u32::from(ctrl & 15));
            let enabled = row_mask >> (l >> 4 & 3) & 1 != 0 && bank_mask >> (l >> 2 & 3) & 1 != 0;
            let (src, inside) = match ctrl {
                0..=0xFF => ((l & !3) | (u32::from(ctrl) >> (2 * (l & 3)) & 3), true),
                0x101..=0x10F => (l + n, pos + n < 16),
                0x111..=0x11F => (l.wrapping_sub(n), pos >= n),
                0x121..=0x12F => (row | (pos.wrapping_sub(n) & 15), true),
                0x140 => (l ^ 15, true),
                0x141 => (l ^ 7, true),
                0x150..=0x15F => (row | n, true),
                0x160..=0x16F => (l ^ n, true),
                _ => return None,
            };
            match (inside, bound_ctrl) {
                (true, _) => Some((src, false, enabled)),
                (false, true) => Some((l, true, enabled)),
                (false, false) => Some((l, false, false)),
            }
        }
    }
}

fn combine(op: Op, a: Sym, b: Sym) -> Sym {
    match (a, b) {
        (Sym::K(k), x) | (x, Sym::K(k)) if op.identity(k) => x,
        (
            Sym::Red {
                op: oa,
                src,
                mask,
                ident,
                lanes: la,
            },
            Sym::Red {
                op: ob,
                src: sb,
                mask: mb,
                ident: ib,
                lanes: lb,
            },
        ) if (src, mask, ident) == (sb, mb, ib)
            && op.identity(ident)
            && oa.is_none_or(|o| o == op)
            && ob.is_none_or(|o| o == op)
            && (!op.counts() || la & lb == 0)
            && (op != Op::AddH || buddies(la, lb)) =>
        {
            Sym::Red {
                op: Some(op),
                src,
                mask,
                ident,
                lanes: la | lb,
            }
        }
        _ => Sym::Unknown,
    }
}

/// The lane a `v_readlane_b32` / `v_writelane_b32` names (inline, literal, or
/// constant scalar).
fn lane_number_in(
    inst: &Inst,
    op: &Operand,
    uni: &HashMap<Value, Sym>,
    pre_consts: &HashMap<Value, u32>,
) -> Option<u32> {
    match *op {
        Operand::Const(c) => crate::resource::inline(c),
        Operand::Literal(x) => Some(x),
        Operand::Scalar { code: NULL, .. } => Some(0),
        Operand::Scalar { code, .. } => {
            let v = inst.read(Reg::S(code))?;
            match uni.get(&v).copied() {
                Some(Sym::K(k)) => Some(k),
                Some(_) => None,
                None => pre_consts.get(&v).copied(),
            }
        }
        _ => None,
    }
}

/// The lanes' values of the VGPRs the section writes, and its scalars.
type Left = (HashMap<Value, Vec<Sym>>, HashMap<Value, Sym>);

fn simulate(
    insts: &[Inst],
    wave: u32,
    saved: Value,
    pre_consts: &HashMap<Value, u32>,
) -> Left {
    let all = if wave == 64 {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    let mut exec = all;
    let mut vec: HashMap<Value, Vec<Sym>> = HashMap::new();
    let mut uni: HashMap<Value, Sym> = HashMap::new();
    // Masks the section derives from the saved EXEC (`s_and` / `s_mov` of it).
    let mut masked = vec![saved];
    let scalar_sym = |inst: &Inst, op: &Operand, uni: &HashMap<Value, Sym>| -> Sym {
        match *op {
            Operand::Scalar { code: NULL, .. } => Sym::K(0),
            Operand::Scalar { code, .. } => match inst.read(Reg::S(code)) {
                Some(x) => uni
                    .get(&x)
                    .copied()
                    .or_else(|| pre_consts.get(&x).copied().map(Sym::K))
                    .unwrap_or(Sym::Uni(x)),
                None => Sym::Unknown,
            },
            Operand::Const(c) => crate::resource::inline(c).map_or(Sym::Unknown, Sym::K),
            Operand::Literal(x) => Sym::K(x),
            _ => Sym::Unknown,
        }
    };
    for inst in insts {
        let name = inst.name();
        if inst.write(Reg::S(EXEC)).is_some() {
            // Constant lane sets (prefix sums, or `s_or_b64 exec, exec, -1`);
            // anything else ends what the simulation can follow.
            let next = update_exec(inst, exec, false, all);
            let Some(new_exec) = next else {
                return (vec, uni);
            };
            exec = new_exec;
            continue;
        }
        if inst.write(Reg::S(EXEC + 1)).is_some() {
            let Some(new_exec) = update_exec(inst, exec, true, all) else {
                return (vec, uni);
            };
            exec = new_exec;
            continue;
        }
        if let Fields::Sop { dst, src, imm } = &inst.fields {
            if (name.starts_with("s_and_b") || matches!(name, "s_mov_b64" | "s_mov_b32"))
                && src.iter().any(|o| matches!(*o, Operand::Scalar { code, .. } if inst.read(Reg::S(code)).is_some_and(|v| masked.contains(&v))))
                && let Some(&(_, w)) = inst.writes.iter().find(|w| matches!(w.0, Reg::S(_)))
            {
                masked.push(w);
            }
            if let Some(Operand::Scalar { code: dc, .. }) = dst
                && *dc != NULL
                && let Some(w) = inst.write(Reg::S(*dc))
            {
                let sym = match name {
                    "s_mov_b32" => src.first().map_or(Sym::Unknown, |s0| scalar_sym(inst, s0, &uni)),
                    "s_movk_i32" => imm.map_or(Sym::Unknown, |k| Sym::K(i32::from(k as i16) as u32)),
                    "s_brev_b32" => match src.first().map(|s0| scalar_sym(inst, s0, &uni)) {
                        Some(Sym::K(k)) => Sym::K(k.reverse_bits()),
                        _ => Sym::Unknown,
                    },
                    "s_not_b32" => match src.first().map(|s0| scalar_sym(inst, s0, &uni)) {
                        Some(Sym::K(k)) => Sym::K(!k),
                        _ => Sym::Unknown,
                    },
                    _ => match (Op::of_sop(name), src.first(), src.get(1)) {
                        (Some(op), Some(s0), Some(s1)) => {
                            let a = scalar_sym(inst, s0, &uni);
                            let b = scalar_sym(inst, s1, &uni);
                            combine(op, a, b)
                        }
                        _ => Sym::Unknown,
                    },
                };
                uni.insert(w, sym);
            }
            for &(_, w) in &inst.writes {
                uni.entry(w).or_insert(Sym::Unknown);
            }
            continue;
        }
        if let Fields::Ds {
            dst: Some(Operand::Vector { reg: dreg, .. }),
            addr: Some(Operand::Vector { reg: sreg, .. }),
            offset0,
            offset1,
            ..
        } = &inst.fields
            && name == "ds_swizzle_b32"
        {
            let Some(dst) = inst.write(Reg::V(*dreg)) else {
                continue;
            };
            let src_lanes = match inst.read(Reg::V(*sreg)) {
                Some(x) => vec
                    .get(&x)
                    .cloned()
                    .unwrap_or_else(|| vec![Sym::Own(x); wave as usize]),
                None => vec![Sym::Unknown; wave as usize],
            };
            let old = match inst.read(Reg::V(*dreg)) {
                Some(x) => vec
                    .get(&x)
                    .cloned()
                    .unwrap_or_else(|| vec![Sym::Own(x); wave as usize]),
                None => vec![Sym::Unknown; wave as usize],
            };
            let off = u32::from(*offset0) | u32::from(*offset1) << 8;
            let merged = (0..wave)
                .map(|l| {
                    if exec >> l & 1 == 0 {
                        return old[l as usize];
                    }
                    let src_l = if off & 0x8000 != 0 {
                        let pick = (off >> (2 * (l & 3))) & 3;
                        (l & !3) | pick
                    } else {
                        let and = (off & 0x1F) | !0x1F;
                        let or = (off >> 5) & 0x1F;
                        let xor = (off >> 10) & 0x1F;
                        ((l & and) | or) ^ xor
                    };
                    if src_l < wave && (exec >> src_l) & 1 != 0 {
                        src_lanes[src_l as usize]
                    } else {
                        Sym::K(0)
                    }
                })
                .collect();
            vec.insert(dst, merged);
            continue;
        }
        let Fields::Valu(v) = &inst.fields else {
            for &(_, w) in &inst.writes {
                vec.insert(w, vec![Sym::Unknown; wave as usize]);
            }
            continue;
        };
        let lanes = |op: &Operand,
                     vec: &HashMap<Value, Vec<Sym>>,
                     uni: &HashMap<Value, Sym>|
         -> Vec<Sym> {
            let one = |s: Sym| vec![s; wave as usize];
            match *op {
                Operand::Vector { reg, .. } => match inst.read(Reg::V(reg)) {
                    Some(x) => vec
                        .get(&x)
                        .cloned()
                        .unwrap_or_else(|| (0..wave).map(|_| Sym::Own(x)).collect()),
                    None => one(Sym::Unknown),
                },
                _ => one(scalar_sym(inst, op, uni)),
            }
        };
        let modifiers = v.abs != 0 || v.neg != 0 || v.clamp || v.omod != 0 || v.sdwa.is_some();
        if name == "v_readlane_b32" {
            let (Some(Operand::Scalar { code, .. }), Some(src), Some(lane)) =
                (v.dst, v.src.first(), v.src.get(1))
            else {
                continue;
            };
            let l = lane_number_in(inst, lane, &uni, pre_consts);
            let x = lanes(src, &vec, &uni);
            if let Some(w) = inst.write(Reg::S(code)) {
                let s = l
                    .and_then(|l| x.get(l as usize).copied())
                    .unwrap_or(Sym::Unknown);
                uni.insert(w, s);
            }
            continue;
        }
        if name == "v_readfirstlane_b32" {
            let (Some(Operand::Scalar { code, .. }), Some(src)) = (v.dst, v.src.first()) else {
                continue;
            };
            let x = lanes(src, &vec, &uni);
            if let Some(w) = inst.write(Reg::S(code)) {
                let s = if exec != 0 {
                    x.get(exec.trailing_zeros() as usize)
                        .copied()
                        .unwrap_or(Sym::Unknown)
                } else {
                    Sym::Unknown
                };
                uni.insert(w, s);
            }
            continue;
        }
        if name == "v_writelane_b32"
            && !modifiers
            && v.dpp.is_none()
            && let Some(Operand::Vector { reg: dreg, .. }) = v.dst
            && let Some(dst) = inst.write(Reg::V(dreg))
        {
            let mut merged = lanes(
                &Operand::Vector {
                    reg: dreg,
                    dwords: 1,
                },
                &vec,
                &uni,
            );
            if let (Some(src0), Some(lane_op)) = (v.src.first(), v.src.get(1))
                && let Some(l) = lane_number_in(inst, lane_op, &uni, pre_consts)
            {
                let target = (l & (wave - 1)) as usize;
                merged[target] = scalar_sym(inst, src0, &uni);
            } else {
                merged.fill(Sym::Unknown);
            }
            vec.insert(dst, merged);
            continue;
        }
        if name == "v_add_f64"
            && !modifiers
            && v.dpp.is_none()
            && let Some(Operand::Vector { reg: dreg, .. }) = v.dst
            && let (Some(dst_lo), Some(dst_hi)) =
                (inst.write(Reg::V(dreg)), inst.write(Reg::V(dreg + 1)))
        {
            let pair_lanes = |op: Option<&Operand>| -> Option<(Vec<Sym>, Vec<Sym>)> {
                match *op? {
                    Operand::Vector { reg, .. } => Some((
                        lanes(&Operand::Vector { reg, dwords: 1 }, &vec, &uni),
                        lanes(
                            &Operand::Vector {
                                reg: reg + 1,
                                dwords: 1,
                            },
                            &vec,
                            &uni,
                        ),
                    )),
                    Operand::Scalar { code, .. } if code != NULL => Some((
                        lanes(&Operand::Scalar { code, dwords: 1 }, &vec, &uni),
                        lanes(
                            &Operand::Scalar {
                                code: code + 1,
                                dwords: 1,
                            },
                            &vec,
                            &uni,
                        ),
                    )),
                    _ => None,
                }
            };
            let old_lo = lanes(
                &Operand::Vector {
                    reg: dreg,
                    dwords: 1,
                },
                &vec,
                &uni,
            );
            let old_hi = lanes(
                &Operand::Vector {
                    reg: dreg + 1,
                    dwords: 1,
                },
                &vec,
                &uni,
            );
            let f64_term = |lo_s: Sym, hi_s: Sym| -> Option<(Value, Value, Value, u64)> {
                match (lo_s, hi_s) {
                    (
                        Sym::Red {
                            op: None,
                            src: lo,
                            mask: ml,
                            ident: 0,
                            lanes: ll,
                        },
                        Sym::Red {
                            op: None,
                            src: hi,
                            mask: mh,
                            ident: 0,
                            lanes: lh,
                        },
                    ) if ml == mh && ll == lh => Some((lo, hi, ml, ll)),
                    (
                        Sym::Red64F {
                            lo: lo1,
                            hi: hi1,
                            mask: m1,
                            lanes: l1,
                            high: false,
                        },
                        Sym::Red64F {
                            lo: lo2,
                            hi: hi2,
                            mask: m2,
                            lanes: l2,
                            high: true,
                        },
                    ) if (lo1, hi1, m1, l1) == (lo2, hi2, m2, l2) => Some((lo1, hi1, m1, l1)),
                    _ => None,
                }
            };
            let mut out_lo = vec![Sym::Unknown; wave as usize];
            let mut out_hi = vec![Sym::Unknown; wave as usize];
            if let (Some((alo, ahi)), Some((blo, bhi))) =
                (pair_lanes(v.src.first()), pair_lanes(v.src.get(1)))
            {
                for l in 0..wave as usize {
                    let (rl, rh) = match (f64_term(alo[l], ahi[l]), f64_term(blo[l], bhi[l])) {
                        (Some((lo1, hi1, m1, la)), Some((lo2, hi2, m2, lb)))
                            if (lo1, hi1, m1) == (lo2, hi2, m2) && la & lb == 0 =>
                        {
                            (
                                Sym::Red64F {
                                    lo: lo1,
                                    hi: hi1,
                                    mask: m1,
                                    lanes: la | lb,
                                    high: false,
                                },
                                Sym::Red64F {
                                    lo: lo1,
                                    hi: hi1,
                                    mask: m1,
                                    lanes: la | lb,
                                    high: true,
                                },
                            )
                        }
                        _ => (Sym::Unknown, Sym::Unknown),
                    };
                    out_lo[l] = if exec >> l & 1 != 0 { rl } else { old_lo[l] };
                    out_hi[l] = if exec >> l & 1 != 0 { rh } else { old_hi[l] };
                }
            }
            vec.insert(dst_lo, out_lo);
            vec.insert(dst_hi, out_hi);
            continue;
        }
        let Some(Operand::Vector { reg: dreg, .. }) = v.dst else {
            for &(_, w) in &inst.writes {
                uni.insert(w, Sym::Unknown);
            }
            continue;
        };
        let Some(dst) = inst.write(Reg::V(dreg)) else {
            continue;
        };
        let old = lanes(
            &Operand::Vector {
                reg: dreg,
                dwords: 1,
            },
            &vec,
            &uni,
        );
        // src0 as each lane fetches it (DPP), with the lanes that write.
        let src0 = v.src.first().map(|o| lanes(o, &vec, &uni));
        let mut writes = exec;
        let fetched: Option<Vec<Sym>> = match (v.dpp, &src0) {
            (Some(d), Some(s)) => {
                let mut f = Vec::with_capacity(wave as usize);
                let mut ok = true;
                let (fi, bound_ctrl, is_row) = match d {
                    Dpp::Row {
                        fi, bound_ctrl, ..
                    } => (fi, bound_ctrl, true),
                    Dpp::Lanes { fi, .. } => (fi, false, false),
                };
                for l in 0..wave {
                    match dpp_lane(d, l) {
                        Some((src, zero, on)) if src < wave => {
                            if zero {
                                f.push(Sym::K(0));
                            } else if (exec >> src) & 1 == 0 && !fi {
                                if is_row && bound_ctrl {
                                    f.push(Sym::K(0));
                                } else if is_row {
                                    f.push(Sym::Unknown);
                                    writes &= !(1u64 << l);
                                } else {
                                    ok = false;
                                }
                            } else {
                                f.push(s[src as usize]);
                            }
                            if !on {
                                writes &= !(1u64 << l);
                            }
                        }
                        _ => ok = false,
                    }
                }
                ok.then_some(f)
            }
            (None, Some(s)) => Some(s.clone()),
            _ => None,
        };
        let src = |k: usize| v.src.get(k).map(|o| lanes(o, &vec, &uni));
        let result: Vec<Sym> = match (name, fetched) {
            _ if modifiers => vec![Sym::Unknown; wave as usize],
            ("v_mov_b32", Some(a)) => a,
            ("v_bfrev_b32", Some(a)) => a
                .into_iter()
                .map(|s| match s {
                    Sym::K(k) => Sym::K(k.reverse_bits()),
                    _ => Sym::Unknown,
                })
                .collect(),
            ("v_cndmask_b32", Some(a)) => {
                let mask = match v.src.get(2) {
                    Some(&Operand::Scalar { code, .. }) => inst.read(Reg::S(code)),
                    _ => None,
                };
                let b = src(1).unwrap_or_default();
                (0..wave as usize)
                    .map(|l| match (a[l], b.get(l).copied(), mask) {
                        (Sym::K(ident), Some(Sym::Own(x) | Sym::Uni(x)), Some(m))
                            if masked.contains(&m) =>
                        {
                            Sym::Red {
                                op: None,
                                src: x,
                                mask: m,
                                ident,
                                lanes: 1 << l,
                            }
                        }
                        _ => Sym::Unknown,
                    })
                    .collect()
            }
            ("v_permlane16_b32" | "v_permlanex16_b32", Some(a)) => {
                // Lanes of each row read this row (permlane16) or the other row
                // of their 32-lane half (permlanex16); lane i's pick is nibble i
                // of {src2, src1}. OP_SEL[0:1] are fetch-inactive and bound-ctrl
                // (higher OP_SEL bits are unused on 32-bit permlane).
                let fetch_inactive = v.opsel & 1 != 0;
                let bound_ctrl = v.opsel & 2 != 0;
                let sel = |k: usize| {
                    v.src
                        .get(k)
                        .and_then(|op| lane_number_in(inst, op, &uni, pre_consts))
                };
                match (sel(1), sel(2)) {
                    (Some(s1), Some(s2)) => {
                        let s = u64::from(s1) | u64::from(s2) << 32;
                        let swap = if name == "v_permlanex16_b32" { 16 } else { 0 };
                        (0..wave)
                            .map(|l| {
                                let row = (l & !15) ^ swap;
                                let pick = (s >> (4 * (l & 15)) & 15) as u32;
                                let src_l = row | pick;
                                if (exec >> src_l) & 1 == 0 && !fetch_inactive {
                                    if bound_ctrl {
                                        Sym::K(0)
                                    } else {
                                        writes &= !(1u64 << l);
                                        old[l as usize]
                                    }
                                } else {
                                    a[src_l as usize]
                                }
                            })
                            .collect()
                    }
                    _ => vec![Sym::Unknown; wave as usize],
                }
            }
            ("v_sub_nc_u32" | "v_sub_nc_i32" | "v_subrev_nc_u32" | "v_subrev_nc_i32", Some(a)) => {
                // A prefix sum less the lane's own term: the exclusive one.
                let b = src(1).unwrap_or_default();
                let rev = name.contains("subrev");
                (0..wave as usize)
                    .map(|l| {
                        let pair = if rev {
                            (b.get(l).copied(), Some(a[l]))
                        } else {
                            (Some(a[l]), b.get(l).copied())
                        };
                        match pair {
                            (
                                Some(Sym::Red {
                                    op: None | Some(Op::AddU),
                                    src,
                                    mask,
                                    ident: 0,
                                    lanes,
                                }),
                                Some(Sym::Red {
                                    op: None | Some(Op::AddU),
                                    src: s2,
                                    mask: m2,
                                    ident: 0,
                                    lanes: l2,
                                }),
                            ) if (src, mask) == (s2, m2) && lanes & l2 == l2 => {
                                let rest = lanes & !l2;
                                if rest == 0 {
                                    Sym::K(0)
                                } else {
                                    Sym::Red {
                                        op: Some(Op::AddU),
                                        src,
                                        mask,
                                        ident: 0,
                                        lanes: rest,
                                    }
                                }
                            }
                            _ => Sym::Unknown,
                        }
                    })
                    .collect()
            }
            (n, Some(a)) => {
                if let (Some(op), Some(b), Some(c)) = (Op::of3(n), src(1), src(2)) {
                    (0..wave as usize)
                        .map(|l| combine(op, combine(op, a[l], b[l]), c[l]))
                        .collect()
                } else if let (Some(op), Some(b)) = (Op::of(n), src(1)) {
                    (0..wave as usize)
                        .map(|l| combine(op, a[l], b[l]))
                        .collect()
                } else {
                    vec![Sym::Unknown; wave as usize]
                }
            }
            _ => vec![Sym::Unknown; wave as usize],
        };
        let merged = (0..wave as usize)
            .map(|l| {
                if writes >> l & 1 != 0 {
                    result[l]
                } else {
                    old[l]
                }
            })
            .collect();
        vec.insert(dst, merged);
    }
    (vec, uni)
}

/// Updates constant `exec` across an EXEC-writing scalar instruction inside a
/// section (`s_mov`, `s_or`, `s_and` with constant operands).
fn update_exec(inst: &Inst, exec: u64, high_only: bool, all: u64) -> Option<u64> {
    let name = inst.name();
    if name.starts_with("s_mov_b") {
        let k = constant(inst)?;
        let next = if high_only {
            (exec & 0xFFFF_FFFF) | ((k & 0xFFFF_FFFF) << 32)
        } else if name == "s_mov_b64" {
            k
        } else {
            (exec & !0xFFFF_FFFF) | (k & 0xFFFF_FFFF)
        };
        return Some(next & all);
    }
    let Fields::Sop { src, .. } = &inst.fields else {
        return None;
    };
    let (wide, is_or) = match name {
        "s_or_b64" => (true, true),
        "s_and_b64" => (true, false),
        "s_or_b32" => (false, true),
        "s_and_b32" => (false, false),
        _ => return None,
    };
    if high_only && wide {
        return None;
    }
    let target_reg = if high_only { EXEC + 1 } else { EXEC };
    let is_exec = |op: &Operand| matches!(*op, Operand::Scalar { code, .. } if code == target_reg);
    let const_op = |op: &Operand| -> Option<u64> {
        match *op {
            Operand::Scalar { code: NULL, .. } => Some(0),
            Operand::Const(c @ 128..=208) => {
                let x = crate::resource::inline(c)?;
                Some(if wide {
                    i64::from(x as i32) as u64
                } else {
                    u64::from(x)
                })
            }
            Operand::Const(c) if !wide => crate::resource::inline(c).map(u64::from),
            Operand::Literal(x) if !wide || x >> 31 == 0 => Some(u64::from(x)),
            _ => None,
        }
    };
    let [a, b] = &src[..] else {
        return None;
    };
    let k = if is_exec(a) {
        const_op(b)?
    } else if is_exec(b) {
        const_op(a)?
    } else {
        return None;
    };
    let next = if wide {
        if is_or { exec | k } else { exec & k }
    } else if high_only {
        let hi = (exec >> 32) & 0xFFFF_FFFF;
        let nh = if is_or { hi | k } else { hi & k } & 0xFFFF_FFFF;
        (exec & 0xFFFF_FFFF) | (nh << 32)
    } else {
        let lo = exec & 0xFFFF_FFFF;
        let nl = if is_or { lo | k } else { lo & k } & 0xFFFF_FFFF;
        (exec & !0xFFFF_FFFF) | nl
    };
    Some(next & all)
}

fn constant(inst: &Inst) -> Option<u64> {
    let Fields::Sop { src, .. } = &inst.fields else {
        return None;
    };
    let wide = inst.name() == "s_mov_b64";
    match *src.first()? {
        Operand::Scalar { code: NULL, .. } => Some(0),
        Operand::Const(c @ 128..=208) => {
            let x = crate::resource::inline(c)?;
            Some(if wide {
                i64::from(x as i32) as u64
            } else {
                u64::from(x)
            })
        }
        Operand::Const(c) if !wide => crate::resource::inline(c).map(u64::from),
        Operand::Literal(x) => Some(u64::from(x)),
        _ => None,
    }
}

/// The shape of a scalar.
fn single(s: Sym, wave: u32) -> Option<Shape> {
    let all = u64::MAX >> (64 - wave);
    match s {
        Sym::K(k) => Some(Shape::K(k)),
        Sym::Own(x) | Sym::Uni(x) => Some(Shape::Value(x)),
        Sym::Red {
            op: Some(op),
            src,
            mask,
            ident,
            lanes,
        } if lanes == all => Some(Shape::Active(op, src, mask, ident)),
        Sym::Red64F {
            lo,
            hi,
            mask,
            lanes,
            high,
        } if lanes == all => Some(Shape::ActiveF64(lo, hi, mask, high)),
        _ => None,
    }
}

/// The shape of each value, where every lane agrees on one.
fn shapes(
    vec: HashMap<Value, Vec<Sym>>,
    uni: HashMap<Value, Sym>,
    wave: u32,
) -> HashMap<Value, Shape> {
    let all = u64::MAX >> (64 - wave);
    let mut out = HashMap::new();
    for (v, s) in uni {
        if let Some(sh) = single(s, wave) {
            out.insert(v, sh);
        }
    }
    for (v, lanes) in vec {
        if let Some(sh) = per_lane(&lanes, all) {
            out.insert(v, sh);
        }
    }
    out
}

fn per_lane(lanes: &[Sym], all: u64) -> Option<Shape> {
    let first = lanes[0];
    let own = |l: usize, s: Sym| match s {
        Sym::Red {
            op: None,
            src,
            mask,
            ident,
            lanes,
        } if lanes == 1 << l => Some((src, mask, ident)),
        _ => None,
    };
    if let Some(m) = own(0, first)
        && lanes.iter().enumerate().all(|(l, &s)| own(l, s) == Some(m))
    {
        return Some(Shape::Masked(m.0, m.1, m.2));
    }
    if lanes.iter().all(|&s| s == first) {
        return match first {
            Sym::K(k) => Some(Shape::K(k)),
            Sym::Uni(x) | Sym::Own(x) => Some(Shape::Value(x)),
            Sym::Red {
                op: Some(op),
                src,
                mask,
                ident,
                lanes,
            } if lanes == all => Some(Shape::Active(op, src, mask, ident)),
            Sym::Red64F {
                lo,
                hi,
                mask,
                lanes,
                high,
            } if lanes == all => Some(Shape::ActiveF64(lo, hi, mask, high)),
            _ => None,
        };
    }
    // Prefixes: lane l holds lanes 0..l (exclusive) or 0..=l (inclusive).
    let mut meta: Option<(Value, Value, u32, bool)> = None;
    let mut op_found: Option<Op> = None;
    for (l, &s) in lanes.iter().enumerate() {
        let below = (1u64 << l) - 1;
        let (op, src, mask, ident, set) = match s {
            Sym::Red {
                op,
                src,
                mask,
                ident,
                lanes,
            } => (op, src, mask, ident, lanes),
            // Lane 0 of an exclusive prefix holds the identity.
            Sym::K(_) if l == 0 => continue,
            _ => return None,
        };
        let inclusive = set == below | (1u64 << l);
        if !inclusive && set != below {
            return None;
        }
        if l == 0 && !inclusive {
            return None;
        }
        if op.is_none() && set.count_ones() != 1 {
            return None;
        }
        let lane_meta = (src, mask, ident, inclusive);
        match meta {
            None => meta = Some(lane_meta),
            Some(prev) if prev == lane_meta => {}
            _ => return None,
        }
        if let Some(o) = op {
            match op_found {
                None => op_found = Some(o),
                Some(prev) if prev == o => {}
                _ => return None,
            }
        }
    }
    let (src, mask, ident, inclusive) = meta?;
    let op = op_found?;
    if !op.identity(ident) {
        return None;
    }
    match lanes[0] {
        Sym::K(k) if !inclusive && k == ident => {}
        Sym::Red { .. } if inclusive => {}
        _ => return None,
    }
    Some(Shape::Prefix(op, src, mask, ident, inclusive))
}

/// `WaveActiveBitOr(v7)` into s7 as the console compiler expands it: every lane takes part,
/// those outside the saved EXEC (s[4:5]) with 0.
#[cfg(test)]
pub(super) const WAVE_OR_V7_TO_S7: [u32; 20] = [
    0xBE84_25C1, // s_or_saveexec_b64 s[4:5], -1
    0xD501_0001,
    0x0012_0E80, // v_cndmask_b32 v1, 0, v7, s[4:5]
    0x3802_02E9,
    0xDE54_C101, // v_or_b32 v1, v1, v1 dpp8:[1,0,3,2,5,4,7,6]
    0x3802_02E9,
    0x9360_1201, // v_or_b32 v1, v1, v1 dpp8:[2,2,0,0,6,6,4,4]
    0x3802_02E9,
    0x0009_2401, // v_or_b32 v1, v1, v1 dpp8:[4,4,4,4,0,0,0,0]
    0x3802_02FA,
    0xFF09_4001, // v_or_b32 v1, v1, v1 row_mirror bound_ctrl:1
    0xD778_0002,
    0x0201_0101, // v_permlanex16_b32 v2, v1, 0, 0
    0x3802_0302, // v_or_b32 v1, v2, v1
    0xD760_0007,
    0x0001_4101, // v_readlane_b32 s7, v1, 32
    0x3802_0207, // v_or_b32 v1, s7, v1
    0xD760_0007,
    0x0001_0101, // v_readlane_b32 s7, v1, 0
    0xBEFE_0404, // s_mov_b64 exec, s[4:5]
];

#[cfg(test)]
mod tests {
    use super::*;

    const S_ENDPGM: u32 = 0xBF81_0000;

    const fn sop1(sdst: u8, op: u8, ssrc0: u8) -> u32 {
        0xBE80_0000 | (sdst as u32) << 16 | (op as u32) << 8 | ssrc0 as u32
    }

    const fn vop1(vdst: u8, op: u8, src0: u16) -> u32 {
        (0x3F << 25) | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
    }

    const fn vop2(vdst: u8, op: u8, src0: u16, vsrc1: u8) -> u32 {
        (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
    }

    const fn dpp16(vsrc0: u8, ctrl: u16, row_mask: u8, bank_mask: u8, bound_ctrl: bool) -> u32 {
        (row_mask as u32) << 28
            | (bank_mask as u32) << 24
            | (bound_ctrl as u32) << 19
            | (ctrl as u32) << 8
            | vsrc0 as u32
    }

    const fn vop3(vdst: u8, op: u16, src0: u16, src1: u16, src2: u16) -> [u32; 2] {
        [
            0xD400_0000 | (op as u32) << 16 | vdst as u32,
            (src0 as u32 & 0x1FF) | ((src1 as u32 & 0x1FF) << 9) | ((src2 as u32 & 0x1FF) << 18),
        ]
    }

    fn build(words: &[u32], wave64: bool) -> Program {
        let code: Vec<u8> = words
            .iter()
            .copied()
            .chain(std::iter::once(S_ENDPGM))
            .flat_map(u32::to_le_bytes)
            .collect();
        crate::ir::build(&code, wave64, 256).unwrap()
    }

    #[test]
    fn dpp_lanes_follow_the_isa() {
        // row_shr:1 with bound_ctrl: lane 16 reads 0, lane 17 lane 16.
        let shr = Dpp::Row {
            ctrl: 0x111,
            row_mask: 0xF,
            bank_mask: 0xF,
            bound_ctrl: true,
            fi: false,
        };
        assert_eq!(dpp_lane(shr, 16), Some((16, true, true)));
        assert_eq!(dpp_lane(shr, 17), Some((16, false, true)));
        // row_mirror: lane 3 reads lane 12.
        let mirror = Dpp::Row {
            ctrl: 0x140,
            row_mask: 0xF,
            bank_mask: 0xF,
            bound_ctrl: true,
            fi: false,
        };
        assert_eq!(dpp_lane(mirror, 3), Some((12, false, true)));
        // dpp8 [1,0,3,2,5,4,7,6] swaps neighbours.
        let swap = Dpp::Lanes {
            sel: 1 | 3 << 6 | 2 << 9 | 5 << 12 | 4 << 15 | 7 << 18 | 6 << 21,
            fi: false,
        };
        assert_eq!(dpp_lane(swap, 8), Some((9, false, true)));
    }

    #[test]
    fn prefixes_are_recognised() {
        let (x, m) = (Value(1), Value(2));
        let red = |lanes: u64| Sym::Red {
            op: Some(Op::AddU),
            src: x,
            mask: m,
            ident: 0,
            lanes,
        };
        let inclusive: Vec<Sym> = (0..64).map(|l| red(u64::MAX >> (63 - l))).collect();
        assert_eq!(
            per_lane(&inclusive, u64::MAX),
            Some(Shape::Prefix(Op::AddU, x, m, 0, true))
        );
        let mut exclusive: Vec<Sym> = (0..64).map(|l| red((1u64 << l) - 1)).collect();
        exclusive[0] = Sym::K(0);
        assert_eq!(
            per_lane(&exclusive, u64::MAX),
            Some(Shape::Prefix(Op::AddU, x, m, 0, false))
        );
        assert_eq!(
            per_lane(&vec![red(u64::MAX); 64], u64::MAX),
            Some(Shape::Active(Op::AddU, x, m, 0))
        );
    }

    #[test]
    fn constant_decodes_width_and_inline_kinds() {
        let b64 = 0x04;
        let b32 = 0x03;
        let check = |words: &[u32], wave64: bool| -> Option<u64> {
            let p = build(words, wave64);
            constant(&p.blocks[0].insts[0])
        };

        assert_eq!(
            check(&[sop1(EXEC, b64, 194)], true),
            Some(0xFFFF_FFFF_FFFF_FFFE)
        );
        assert_eq!(
            check(&[sop1(EXEC, b64, 196)], true),
            Some(0xFFFF_FFFF_FFFF_FFFC)
        );
        assert_eq!(
            check(&[sop1(EXEC, b64, 208)], true),
            Some(0xFFFF_FFFF_FFFF_FFF0)
        );
        assert_eq!(check(&[sop1(EXEC, b64, 193)], true), Some(u64::MAX));
        assert_eq!(check(&[sop1(EXEC, b64, NULL)], true), Some(0));
        assert_eq!(check(&[sop1(EXEC, b64, 243)], true), None);
        assert_eq!(
            check(&[sop1(EXEC, b64, 255), 0x1234_5678], true),
            Some(0x1234_5678)
        );

        assert_eq!(check(&[sop1(EXEC, b32, 194)], false), Some(0xFFFF_FFFE));
        assert_eq!(check(&[sop1(EXEC, b32, 243)], false), Some(0xBF80_0000));
        assert_eq!(check(&[sop1(EXEC, b32, NULL)], false), Some(0));
        assert_eq!(
            check(&[sop1(EXEC, b32, 255), 0x1234_5678], false),
            Some(0x1234_5678)
        );
        assert_eq!(check(&[sop1(EXEC, b32, 0)], false), None);
    }

    #[test]
    fn exec_minus_two_masks_lane_zero_and_enforces_dpp_guard() {
        for wave64 in [false, true] {
            let mut w = Vec::new();
            let save_op = if wave64 { 0x25 } else { 0x3D };
            let mov_exec = if wave64 { 0x04 } else { 0x03 };
            let push_exec = |w: &mut Vec<u32>, k: u8| w.push(sop1(EXEC, mov_exec, k));
            let push_exec_lit = |w: &mut Vec<u32>, lit: u32| {
                w.push(sop1(EXEC, mov_exec, 255));
                w.push(lit);
            };

            w.push(sop1(0, save_op, 193));
            w.extend(vop3(1, 0x101, 128, 256, 0));
            for shift in [1u16, 2, 4, 8] {
                w.push(vop2(1, 0x25, 0xFA, 1));
                w.push(dpp16(1, 0x110 + shift, 0xF, 0xF, false));
            }
            w.extend(vop3(2, 0x360, 257, 128 + 15, 0));
            push_exec_lit(&mut w, 0xFFFF_0000);
            w.push(vop2(1, 0x25, 2, 1));

            if wave64 {
                w.extend(vop3(2, 0x360, 257, 128 + 31, 0));
                w.push(sop1(EXEC, 0x03, 128));
                w.push(sop1(EXEC + 1, 0x03, 255));
                w.push(0x0000_FFFF);
                w.push(vop2(1, 0x25, 2, 1));

                w.extend(vop3(2, 0x360, 257, 128 + 47, 0));
                w.push(sop1(EXEC, 0x03, 128));
                w.push(sop1(EXEC + 1, 0x03, 255));
                w.push(0xFFFF_0000);
                w.push(vop2(1, 0x25, 2, 1));
            }

            push_exec(&mut w, 193);
            w.push(vop1(3, 0x01, 257));
            w.push(vop1(3, 0x01, 0xFA));
            w.push(dpp16(1, 0x111, 0xF, 0xF, false));

            w.extend(vop3(2, 0x360, 257, 128 + 15, 0));
            push_exec_lit(&mut w, 0x0001_0000);
            w.push(vop1(3, 0x01, 2));

            if wave64 {
                w.extend(vop3(2, 0x360, 257, 128 + 31, 0));
                w.push(sop1(EXEC, 0x03, 128));
                w.push(sop1(EXEC + 1, 0x03, 129));
                w.push(vop1(3, 0x01, 2));

                w.extend(vop3(2, 0x360, 257, 128 + 47, 0));
                w.push(sop1(EXEC, 0x03, 128));
                w.push(sop1(EXEC + 1, 0x03, 255));
                w.push(0x0001_0000);
                w.push(vop1(3, 0x01, 2));
            }

            push_exec(&mut w, 193);
            w.push(vop1(2, 0x01, 128));
            w.push(vop1(5, 0x01, 135));
            push_exec(&mut w, 194);
            w.push(vop1(2, 0x01, 259));
            w.push(vop1(4, 0x01, 0xFA));
            w.push(dpp16(5, 0xE4, 0xF, 0xF, false));
            w.push(sop1(EXEC, mov_exec, 0));

            let p = build(&w, wave64);
            let insts = &p.blocks[0].insts;
            let saved = insts[0].write(Reg::S(0)).unwrap();
            let v0_in = p
                .inputs
                .iter()
                .find_map(|&(r, v)| (r == Reg::V(0)).then_some(v))
                .unwrap();
            let v2_val = insts[insts.len() - 4].write(Reg::V(2)).unwrap();
            let v4_val = insts[insts.len() - 3].write(Reg::V(4)).unwrap();
            let sec = &sections(&p, 0)[&0];
            assert_eq!(
                sec.shapes.get(&v2_val),
                Some(&Shape::Prefix(Op::AddU, v0_in, saved, 0, false))
            );
            assert_eq!(sec.shapes.get(&v4_val), None);
        }
    }

    /// A wave OR whose last step stays in the section's VGPR: the lower
    /// lanes add the first half's to themselves, so only the upper ones
    /// hold the whole, and the last lane is read after the section (an
    /// upscaler's programs).
    #[test]
    fn one_lane_of_what_a_section_leaves_is_read_after_it() {
        let words = |lane: u32| {
            let mut w = WAVE_OR_V7_TO_S7[..14].to_vec(); // .. v_or_b32 v1, v2, v1
            w.extend([
                0xD760_0007,
                0x0001_0101, // v_readlane_b32 s7, v1, 0
                0x3802_0207, // v_or_b32 v1, s7, v1
                0xBEFE_0404, // s_mov_b64 exec, s[4:5]
                0xD760_0006,
                0x0001_0101 | lane << 9, // v_readlane_b32 s6, v1, lane
                S_ENDPGM,
            ]);
            w
        };
        let read = |lane: u32| {
            let p = build(&words(lane), true);
            let insts = &p.blocks[0].insts;
            let saved = insts[0].write(Reg::S(4)).unwrap();
            let v7 = p.inputs.iter().find_map(|&(r, v)| (r == Reg::V(7)).then_some(v)).unwrap();
            let got = insts[insts.len() - 2].write(Reg::S(6)).unwrap();
            let sec = &sections(&p, 0)[&0];
            (sec.reads.get(&got).copied(), Shape::Active(Op::Or, v7, saved, 0))
        };
        let (found, whole) = read(63);
        assert_eq!(found, Some(whole));
        // Lane 0 has the first half's alone: still a lane read.
        assert_eq!(read(0).0, None);
    }

    /// `WaveActiveSum` of 16-bit floats, as compiled in
    /// `compute_graph_ops:CS_TransposeVecMatMulAlignedOperation` (0x19c on): the tree the
    /// lowering reproduces. A tree in another order is not followed.
    #[test]
    fn a_sum_of_halves_over_the_wave_is_followed_in_the_order_of_its_tree() {
        let words = |first: [u32; 2], second: [u32; 2]| {
            [
                0xBE84_25C1, // s_or_saveexec_b64 s[4:5], -1
                0xD501_0001,
                0x0012_0E80, // v_cndmask_b32 v1, 0, v7, s[4:5]
                first[0],
                first[1],
                second[0],
                second[1],
                0x6402_02E9,
                0x0009_2401, // v_add_f16 v1, v1, v1 dpp8:[4,4,4,4,0,0,0,0]
                0x6402_02FA,
                0xFF09_4001, // v_add_f16 v1, v1, v1 row_mirror bound_ctrl:1
                0xD778_0002,
                0x0201_0101, // v_permlanex16_b32 v2, v1, 0, 0
                0x6402_0302, // v_add_f16 v1, v2, v1
                0xD760_0007,
                0x0001_4101, // v_readlane_b32 s7, v1, 32
                0x6402_0207, // v_add_f16 v1, s7, v1
                0xD760_0007,
                0x0001_0101, // v_readlane_b32 s7, v1, 0
                0xBEFE_0404, // s_mov_b64 exec, s[4:5]
            ]
        };
        let pairs = [0x6402_02E9, 0xDE54_C101]; // v_add_f16 v1, v1, v1 dpp8:[1,0,3,2,5,4,7,6]
        let quads = [0x6402_02E9, 0x9360_1201]; // v_add_f16 v1, v1, v1 dpp8:[2,2,0,0,6,6,4,4]
        let shape = |w: &[u32]| {
            let p = build(w, true);
            let insts = &p.blocks[0].insts;
            let saved = insts[0].write(Reg::S(4)).unwrap();
            let v7 = p.inputs.iter().find_map(|&(r, v)| (r == Reg::V(7)).then_some(v)).unwrap();
            // ..., v_readlane_b32 s7, v1, 0; s_mov_b64 exec, s[4:5]; s_endpgm
            let total = insts[insts.len() - 3].write(Reg::S(7)).unwrap();
            (sections(&p, 0)[&0].shapes.get(&total).copied(), Shape::Active(Op::AddH, v7, saved, 0))
        };
        let (found, want) = shape(&words(pairs, quads));
        assert_eq!(found, Some(want));
        // lanes two apart first, neighbours second: every lane counted once, but
        // (x0 + x2) + (x1 + x3) rounds differently from (x0 + x1) + (x2 + x3)
        let (found, _) = shape(&words(quads, pairs));
        assert_eq!(found, None);
    }

    #[test]
    fn buddies_are_the_two_halves_of_an_aligned_run() {
        assert!(buddies(0b01, 0b10));
        assert!(buddies(0b1100, 0b0011));
        assert!(buddies(0xF0, 0x0F));
        assert!(buddies(u64::from(u32::MAX), u64::from(u32::MAX) << 32));
        assert!(!buddies(0b010, 0b100)); // lanes 1 and 2: not one pair
        assert!(!buddies(0b0011, 0b110000)); // pairs 0 and 2
        assert!(!buddies(0b0101, 0b1010)); // not runs
        assert!(!buddies(0b01, 0b1100)); // unequal
        assert!(!buddies(0b11, 0b11));
    }

    #[test]
    fn wave32_mov_b64_all_lanes_keeps_exec_masked_for_dpp() {
        let w = [
            sop1(0, 0x3D, 193),
            vop1(1, 0x01, 135),
            sop1(EXEC, 0x04, 193),
            vop1(2, 0x01, 0xFA),
            dpp16(1, 0xE4, 0xF, 0xF, false),
            sop1(EXEC + 1, 0x03, 193),
            vop1(3, 0x01, 0xFA),
            dpp16(1, 0xE4, 0xF, 0xF, false),
            sop1(EXEC, 0x03, 0),
        ];
        let p = build(&w, false);
        let insts = &p.blocks[0].insts;
        let v2_val = insts[3].write(Reg::V(2)).unwrap();
        let v3_val = insts[5].write(Reg::V(3)).unwrap();
        let sec = &sections(&p, 0)[&0];
        assert_eq!(sec.shapes.get(&v2_val), Some(&Shape::K(7)));
        assert_eq!(sec.shapes.get(&v3_val), Some(&Shape::K(7)));
    }

    #[test]
    fn per_lane_prefix_soundness() {
        let (v0, v_other, m0, m_other) = (Value(1), Value(2), Value(3), Value(4));
        let red = |op: Option<Op>, src: Value, mask: Value, ident: u32, lanes: u64| Sym::Red {
            op,
            src,
            mask,
            ident,
            lanes,
        };

        for (wave, all) in [(32usize, u64::from(u32::MAX)), (64usize, u64::MAX)] {
            let below_mask = |l: usize| if l == 0 { 0 } else { (1u64 << l) - 1 };

            let mut valid_inc: Vec<Sym> = (0..wave)
                .map(|l| red(Some(Op::AddU), v0, m0, 0, below_mask(l) | (1u64 << l)))
                .collect();
            valid_inc[0] = red(None, v0, m0, 0, 1);
            assert_eq!(
                per_lane(&valid_inc, all),
                Some(Shape::Prefix(Op::AddU, v0, m0, 0, true))
            );

            let mut valid_exc: Vec<Sym> = (0..wave)
                .map(|l| red(Some(Op::AddU), v0, m0, 0, below_mask(l)))
                .collect();
            valid_exc[0] = Sym::K(0);
            valid_exc[1] = red(None, v0, m0, 0, 1);
            assert_eq!(
                per_lane(&valid_exc, all),
                Some(Shape::Prefix(Op::AddU, v0, m0, 0, false))
            );

            let mut mixed_other_src = valid_exc.clone();
            mixed_other_src[0] = red(None, v_other, m0, 0, 1);
            assert_eq!(per_lane(&mixed_other_src, all), None);

            let mut mixed_same_src = valid_exc.clone();
            mixed_same_src[0] = red(None, v0, m0, 0, 1);
            assert_eq!(per_lane(&mixed_same_src, all), None);

            let mut exc_bad_lane1_src = valid_exc.clone();
            exc_bad_lane1_src[1] = red(None, v_other, m0, 0, 1);
            assert_eq!(per_lane(&exc_bad_lane1_src, all), None);

            let mut exc_bad_lane1_set = valid_exc.clone();
            exc_bad_lane1_set[1] = red(None, v0, m0, 0, 0b11);
            assert_eq!(per_lane(&exc_bad_lane1_set, all), None);

            for bad0 in [
                red(None, v0, m0, 1, 1),
                red(None, v_other, m0, 0, 1),
                red(None, v0, m_other, 0, 1),
                red(None, v0, m0, 0, 0),
                Sym::K(0),
            ] {
                let mut bad_inc = valid_inc.clone();
                bad_inc[0] = bad0;
                assert_eq!(per_lane(&bad_inc, all), None);
            }

            let mut bad_ident: Vec<Sym> = (0..wave)
                .map(|l| red(Some(Op::AddU), v0, m0, 1, below_mask(l) | (1u64 << l)))
                .collect();
            bad_ident[0] = red(None, v0, m0, 1, 1);
            assert_eq!(per_lane(&bad_ident, all), None);
        }
    }

    #[test]
    fn ds_swizzle_permlane16_three_op_hoisted_identity_and_scalar_tail() {
        // 1. Wave32 OR reduction via `ds_swizzle_b32` xor butterfly.
        let p = build(
            &[
                0xBE843DC1, 0xD5010001, 0x00120080, 0xD8D4041F, 0x02000001, 0x38020501,
                0xD8D4081F, 0x02000001, 0x38020501, 0xD8D4101F, 0x02000001, 0x38020501,
                0xD8D4201F, 0x02000001, 0x38020501, 0xD8D4401F, 0x02000001, 0x38020501,
                0xBEFE0304,
            ],
            false,
        );
        let insts = &p.blocks[0].insts;
        let saved = insts[0].write(Reg::S(4)).unwrap();
        let v0 = p.inputs.iter().find_map(|&(r, v)| (r == Reg::V(0)).then_some(v)).unwrap();
        let out_v1 = insts[insts.len() - 3].write(Reg::V(1)).unwrap();
        let sec = &sections(&p, 0)[&0];
        assert_eq!(sec.shapes.get(&out_v1), Some(&Shape::Active(Op::Or, v0, saved, 0)));

        // 2. Hoisted `+inf` before `s_orn1_saveexec_b64 0` + `v_readfirstlane_b32`.
        let p = build(
            &[
                0xBE8603FF, 0x7F800000, 0xBE843880, 0xD5010001, 0x00120006, 0x1E0202E9,
                0xDE54C101, 0x1E0202E9, 0x93601201, 0x1E0202E9, 0x00092401, 0x1E0202FA,
                0xFF094001, 0xD7780002, 0x02010101, 0x1E020302, 0xD7600007, 0x02014101,
                0x1E020207, 0x7E100501, 0xBEFE0404,
            ],
            true,
        );
        let insts = &p.blocks[0].insts;
        let saved = insts[1].write(Reg::S(4)).unwrap();
        let v0 = p.inputs.iter().find_map(|&(r, v)| (r == Reg::V(0)).then_some(v)).unwrap();
        let s8 = insts[insts.len() - 3].write(Reg::S(8)).unwrap();
        let sec = &sections(&p, 0)[&1];
        assert_eq!(
            sec.shapes.get(&s8),
            Some(&Shape::Active(Op::MinF, v0, saved, 0x7F80_0000))
        );

        // 3. `v_permlane16_b32` + `v_or3_b32` + scalar `s_or_b32` tail + `v_writelane_b32`.
        let p = build(
            &[
                0xBE8425C1, 0xD5010001, 0x00120080, 0x380202E9, 0xDE54C101, 0x380202E9,
                0x93601201, 0x380202E9, 0x00092401, 0x380202FA, 0xFF094001, 0xD7770002,
                0x02010101, 0xD7780003, 0x02010102, 0xD7720001, 0x040E0501, 0xD7600006,
                0x02010101, 0xD7600007, 0x02014101, 0x88080706, 0xD7610004, 0x02010008,
                0xD7600009, 0x02010104, 0xBEFE0404,
            ],
            true,
        );
        let insts = &p.blocks[0].insts;
        let saved = insts[0].write(Reg::S(4)).unwrap();
        let v0 = p.inputs.iter().find_map(|&(r, v)| (r == Reg::V(0)).then_some(v)).unwrap();
        let s8 = insts[insts.len() - 5].write(Reg::S(8)).unwrap();
        let s9 = insts[insts.len() - 3].write(Reg::S(9)).unwrap();
        let sec = &sections(&p, 0)[&0];
        assert_eq!(sec.shapes.get(&s8), Some(&Shape::Active(Op::Or, v0, saved, 0)));
        assert_eq!(sec.shapes.get(&s9), Some(&Shape::Active(Op::Or, v0, saved, 0)));

        // 4. DPP with partial constant `exec` (`exec_hi = 0`) + `s_or_b64 exec, exec, -1`.
        let p = build(
            &[
                0xBE8425C1, 0xD5010001, 0x00120080, 0xBEFF0380, 0x380202FA, 0xFF096101,
                0x88FEC17E, 0xBEFE0404,
            ],
            true,
        );
        let insts = &p.blocks[0].insts;
        let saved = insts[0].write(Reg::S(4)).unwrap();
        let v0 = p.inputs.iter().find_map(|&(r, v)| (r == Reg::V(0)).then_some(v)).unwrap();
        let consts = collect_scalar_consts(&p);
        let (vec, _) = simulate(&insts[1..insts.len() - 2], 64, saved, &consts);
        let v1_dpp = insts[3].write(Reg::V(1)).unwrap();
        assert_eq!(
            vec[&v1_dpp][0],
            Sym::Red {
                op: Some(Op::Or),
                src: v0,
                mask: saved,
                ident: 0,
                lanes: 0b11,
            }
        );
        // Upper half (inactive during DPP) retained its single-lane masked value.
        assert_eq!(
            vec[&v1_dpp][32],
            Sym::Red {
                op: None,
                src: v0,
                mask: saved,
                ident: 0,
                lanes: 1u64 << 32,
            }
        );
    }
}
