//! The NaN guard (`VICTORY_NAN_GUARD`, on unless `=0`; from willy's build,
//! after Neptune-VE) on the stores it changes and on those it must leave
//! alone: each program is lowered with the guard on and off
//! ([`lower::TEST_NAN_GUARD`]), validated by `dxil.dll` and read back as
//! text with `dxc -dumpbin` (both from the Windows SDK, when present).
//! Nothing runs on a device. The values stored are constants, so what the
//! guard made of them stays in the text once the clean-up passes have
//! folded it.

use super::container::{self, SigElement};
use super::{Container, FourCC, lower, validator};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;

/// `s_mov_b32 s5, 0`: the table of views is at s[4:5].
const TABLE_HIGH: u32 = 0xBE85_0380;

/// `v_mov_b32 v0, 0`: the element or texel the stores write.
const AT_ZERO: u32 = 0x7E00_0280;

/// SMEM `s_load_dwordx4` (`s_load_dwordx8` if `eight`) `s[dst..], s[4:5], offset`.
const fn s_load(dst: u8, offset: u32, eight: bool) -> [u32; 2] {
    let op = if eight { 0x03 } else { 0x02 };
    [
        0x3D << 26 | op << 18 | (dst as u32) << 6 | 4 >> 1,
        125 << 25 | offset,
    ]
}

/// VOP1 `v_mov_b32 vdst, literal`.
const fn v_mov(vdst: u8, literal: u32) -> [u32; 2] {
    [
        0x7E00_0000 | (vdst as u32) << 17 | 0x01 << 9 | 0xFF,
        literal,
    ]
}

/// MUBUF `op vdata, v0, s[srsrc:srsrc+3], 0 idxen`.
const fn mubuf_idxen(op: u32, vdata: u8, srsrc: u8) -> [u32; 2] {
    [
        0x38 << 26 | op << 18 | 1 << 13,
        0x80 << 24 | ((srsrc as u32 >> 2) & 0x1F) << 16 | (vdata as u32) << 8,
    ]
}

const STORE_FORMAT_XYZW: u32 = 0x07;
const STORE_FORMAT_D16_XYZW: u32 = 0x87;
const STORE_DWORDX4: u32 = 0x1E;

/// The four floats most stores below write, as their bits: the default
/// quiet NaN, both infinities, and a NaN of another pattern (also the
/// largest signed integer).
const SPECIAL: [u32; 4] = [0x7FC0_0000, 0x7F80_0000, 0xFF80_0000, 0x7FFF_FFFF];

/// `v_mov_b32 v[first..first + 4], SPECIAL`.
fn special(first: u8) -> Vec<u32> {
    (0..4u8)
        .flat_map(|k| v_mov(first + k, SPECIAL[usize::from(k)]))
        .collect()
}

/// Entry `n` (32 bytes each) of the table of views: a view of `kind` whose
/// descriptor has `dwords`, of elements `stride` bytes apart.
fn entry(kind: ResourceType, n: u32, dwords: u8, stride: u32) -> UsedResource {
    UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: 8 * n,
        lower_bound: n,
        stride,
        dwords,
        user_data: 4,
        slot: 1,
    }
}

/// Runs `f` with the guard on or off on this thread.
fn guarded<R>(on: bool, f: impl FnOnce() -> R) -> R {
    let was = lower::TEST_NAN_GUARD.replace(Some(on));
    let r = f();
    lower::TEST_NAN_GUARD.set(was);
    r
}

fn validate(dxil: &[u8]) {
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        v.validate(&mut dxil.to_vec())
            .expect("DXIL validation failed");
    } else {
        eprintln!("no dxil.dll; validation skipped");
    }
}

/// Lowers a wave32 compute program of one thread: root constants in s0..s3,
/// the table of views at s4 (root parameter 1), whose entries are `used`.
fn compute(body: &[u32], used: Vec<UsedResource>) -> lower::Lowered {
    let mut words = vec![TABLE_HIGH];
    words.extend_from_slice(body);
    words.push(S_ENDPGM);
    let param = |root, sgprs, user_data, slot| RootParam {
        root,
        table_offset_dwords: 0,
        sgprs,
        base_register: 0,
        user_data,
        slot,
    };
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 8,
        layout: vec![
            param(RootType::Constants, 4, 0, 0),
            param(RootType::Table, 1, 4, 1),
        ],
        used,
        io: StageIo::Cs(ComputeIo {
            rsrc2: 5 << 1,
            threads: [1, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res =
        resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, &native).expect("io::raise");
    let lowered = lower::lower(&lower::Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
    })
    .expect("lower");
    validate(&lowered.dxil);
    lowered
}

/// The text of `dxil`, or `None` (said) without `dxc.exe`.
fn text(dxil: &[u8]) -> Option<String> {
    let text = super::tests::dump_dxil(dxil);
    if text.is_none() {
        eprintln!("no dxc.exe; DXIL text checks skipped");
    }
    text
}

/// The instructions of `text`, without the comments dxc adds to them.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|l| l.split(" ;").next().unwrap_or(l).trim())
}

/// The line of `text` that defines `%name`.
fn defined<'t>(text: &'t str, name: &str) -> &'t str {
    let head = format!("{name} = ");
    lines(text)
        .find(|l| l.starts_with(&head))
        .unwrap_or_else(|| panic!("no definition of {name} in\n{text}"))
}

/// The selects of `text`: condition, then the operand where it holds and
/// the one where it does not (each with its type).
fn selects(text: &str) -> Vec<(&str, &str, &str)> {
    lines(text)
        .filter_map(|l| l.split_once(" = select i1 "))
        .filter_map(|(_, rest)| {
            let mut operands = rest.splitn(3, ", ");
            Some((operands.next()?, operands.next()?, operands.next()?))
        })
        .collect()
}

/// The condition of the one select of `text` whose operands `pick` takes.
fn select_of<'t>(text: &'t str, pick: impl Fn(&str, &str) -> bool) -> &'t str {
    let found: Vec<&str> = selects(text)
        .into_iter()
        .filter(|s| pick(s.1, s.2))
        .map(|s| s.0)
        .collect();
    assert_eq!(found.len(), 1, "one such select in\n{text}");
    found[0]
}

/// Whether `%cond` asks whether the host's table of view kinds marks the
/// view as one of an integer format (`VIEW_INTEGER`, the word's top bit).
fn asks_integer_view(text: &str, cond: &str) -> bool {
    let test = defined(text, cond);
    let Some(word) = test
        .split_once(" = icmp ne i32 ")
        .and_then(|(_, r)| r.strip_suffix(", 0"))
    else {
        return false;
    };
    defined(text, word).ends_with(&format!(", {}", super::VIEW_INTEGER as i32))
}

/// The lines of the calls to `op` in `text` (not its declaration).
fn calls<'t>(text: &'t str, op: &str) -> Vec<&'t str> {
    let name = format!("@dx.op.{op}(");
    lines(text)
        .filter(|l| l.contains(&name) && !l.starts_with("declare "))
        .collect()
}

/// A pixel program's colour: every NaN becomes 0 and an infinity the
/// largest half float of its sign; numbers stay as they are. Without the
/// switch the exports are as before.
#[test]
fn a_colour_target_gets_no_nan_or_infinity_with_the_guard() {
    let mut words: Vec<u32> = [0x7FFF_FFFF, 0xFFC0_0000, 0xFF80_0000, 1.5f32.to_bits()]
        .into_iter()
        .enumerate()
        .flat_map(|(k, bits)| v_mov(k as u8, bits))
        .collect();
    words.extend_from_slice(&[
        0xF800_180F, // exp mrt0 v0, v1, v2, v3 done vm
        0x0302_0100,
        S_ENDPGM,
    ]);
    let native = NativeProgram {
        stage: HwStage::Ps,
        wave64: false,
        vgprs: 4,
        layout: vec![],
        used: vec![],
        io: StageIo::Ps {
            inputs: vec![],
            input_addr: 0,
            db_shader_control: 0,
            patches: vec![],
        },
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let target = SigElement {
        name: "SV_Target".into(),
        index: 0,
        kind: 16,
        comp: 9,
        interp: 0,
        row: Some(0),
        col: 0,
        cols: 4,
        usage: 15,
        half: false,
    };
    let osg1 = container::signature(&[target], false);
    let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
    let c = Container::parse(&bytes).unwrap();
    let dxil = |on: bool| {
        guarded(on, || {
            let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
            let st = structure::structure(&p).unwrap();
            let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let dxil = super::emit(&c, &native, &p, &st, &io, &res).unwrap().dxil;
            validate(&dxil);
            dxil
        })
    };
    let (Some(on), Some(off)) = (text(&dxil(true)), text(&dxil(false))) else {
        return;
    };
    for (column, value) in [
        "0.000000e+00",
        "0.000000e+00",
        "-6.550400e+04",
        "1.500000e+00",
    ]
    .iter()
    .enumerate()
    {
        let export = format!("i8 {column}, float {value})");
        assert!(on.contains(&export), "with the guard: {export} in\n{on}");
    }
    for (column, value) in [
        "0x7FFFFFFFE0000000",
        "0xFFF8000000000000",
        "0xFFF0000000000000",
        "1.500000e+00",
    ]
    .iter()
    .enumerate()
    {
        let export = format!("i8 {column}, float {value})");
        assert!(
            off.contains(&export),
            "without the guard: {export} in\n{off}"
        );
    }
}

/// An image store: the texture's format is not known, so the default NaN
/// becomes 0 and an infinity the largest half float, and other NaN patterns
/// (an integer's bits as often as not) are stored as they are.
#[test]
fn an_image_store_has_the_default_nan_and_infinities_guarded() {
    let mut body = s_load(8, 0, true).to_vec();
    body.push(AT_ZERO);
    body.extend(special(1));
    // image_store v[1:4], v0, s[8:15] dmask:0xf 1D unorm
    body.extend_from_slice(&[0xF020_1F00, 0x0002_0100]);
    let used = || vec![entry(ResourceType::Uav, 0, 8, 0)];
    let (on, off) = (
        guarded(true, || compute(&body, used())),
        guarded(false, || compute(&body, used())),
    );
    let (Some(on), Some(off)) = (text(&on.dxil), text(&off.dxil)) else {
        return;
    };
    let guarded = "float 0.000000e+00, float 6.550400e+04, float -6.550400e+04, float 0x7FFFFFFFE0000000, i8 15)";
    let stores = calls(&on, "textureStore.f32");
    assert!(
        stores.len() == 1 && stores[0].ends_with(guarded),
        "with the guard:\n{on}"
    );
    let raw = "float 0x7FF8000000000000, float 0x7FF0000000000000, float 0xFFF0000000000000, float 0x7FFFFFFFE0000000, i8 15)";
    let stores = calls(&off, "textureStore.f32");
    assert!(
        stores.len() == 1 && stores[0].ends_with(raw),
        "without the guard:\n{off}"
    );
}

/// Typed buffer stores through a view of a table, of 32-bit data and of
/// halves (d16, through a view the program also stores whole floats to,
/// so not the halves path): what each stores where the host's table of
/// view kinds says the view is of an integer format (the selects' first
/// operands) is the bits as they are, the integers a d16 store makes of its
/// halves (VIctory 4243a96a) as before. Where it is not, the default NaN is
/// 0 and an infinity the largest half float. Without the switch the view
/// kind is not read for 32-bit data, and nothing is guarded.
#[test]
fn a_typed_store_is_guarded_unless_its_view_is_of_an_integer_format() {
    let mut body = s_load(8, 0, false).to_vec();
    body.push(AT_ZERO);
    body.extend(special(1));
    body.extend(mubuf_idxen(STORE_FORMAT_XYZW, 1, 8));
    // Halves: a NaN and an infinity, then the other infinity and all ones.
    body.extend(v_mov(5, 0x7C00_7E00));
    body.extend(v_mov(6, 0xFFFF_FC00));
    body.extend(mubuf_idxen(STORE_FORMAT_D16_XYZW, 5, 8));
    let used = || vec![entry(ResourceType::Uav, 0, 4, 4)];
    let (on, off) = (
        guarded(true, || compute(&body, used())),
        guarded(false, || compute(&body, used())),
    );
    assert!(
        on.half_views.is_empty() && off.half_views.is_empty(),
        "not the halves path"
    );
    assert!(on.root_srvs.contains(&super::SLOT_TABLE_STRIDES));
    let (Some(on), Some(off)) = (text(&on.dxil), text(&off.dxil)) else {
        return;
    };
    // 32-bit data: the integer view's bits or the guarded float.
    for (bits, float) in [
        ("float 0x7FF8000000000000", "float 0.000000e+00"),
        ("float 0x7FF0000000000000", "float 6.550400e+04"),
        ("float 0xFFF0000000000000", "float -6.550400e+04"),
    ] {
        let cond = select_of(&on, |t, f| t == bits && f == float);
        assert!(
            asks_integer_view(&on, cond),
            "{bits}: chosen by the view's kind in\n{on}"
        );
    }
    let (with, without) = (
        calls(&on, "bufferStore.f32"),
        calls(&off, "bufferStore.f32"),
    );
    assert!(
        with.len() == 2 && without.len() == 2,
        "two typed stores in\n{on}\n{off}"
    );
    assert!(
        with[0].ends_with("float 0x7FFFFFFFE0000000, i8 15)"),
        "another NaN pattern as it is:\n{on}"
    );
    let raw = "float 0x7FF8000000000000, float 0x7FF0000000000000, float 0xFFF0000000000000, float 0x7FFFFFFFE0000000, i8 15)";
    assert!(without[0].ends_with(raw), "without the guard:\n{off}");
    // Halves: the integers of an integer view (0x7E00 and 0x7C00; the
    // others are signed or not by the view), with the switch or without.
    for text in [&on, &off] {
        for integer in ["i32 32256", "i32 31744"] {
            let cond = select_of(text, |t, f| t == integer && f.starts_with("i32 %"));
            assert!(
                asks_integer_view(text, cond),
                "{integer}: chosen by the view's kind in\n{text}"
            );
        }
    }
    // The halves' floats are guarded: the largest half float with the sign
    // of the bits (0x477FE000) is or-ed in.
    assert!(
        on.contains(", 1199562752"),
        "the halves' floats are guarded:\n{on}"
    );
    assert!(
        !off.contains("1199562752"),
        "nothing is guarded without the switch:\n{off}"
    );
}

/// Halves through a view of a table (fxceless PR #927: the face rig's
/// tensors) and raw stores move bits, not floats: with the switch on their
/// translation is the same, to the byte, as without it.
#[test]
fn halves_and_raw_stores_are_the_same_with_the_guard() {
    // NaN and infinity patterns as halves, then the other infinity and all ones.
    let mut halves = s_load(8, 0, false).to_vec();
    halves.push(AT_ZERO);
    halves.extend(v_mov(1, 0x7C00_7E00));
    halves.extend(v_mov(2, 0xFFFF_FC00));
    halves.extend(mubuf_idxen(STORE_FORMAT_D16_XYZW, 1, 8));
    let used = || vec![entry(ResourceType::Uav, 0, 4, 4)];
    let (on, off) = (
        guarded(true, || compute(&halves, used())),
        guarded(false, || compute(&halves, used())),
    );
    assert_eq!(on.half_views, [(1, 0)], "the halves path");
    assert_eq!(on.dxil, off.dxil, "halves through a view of a table");
    // buffer_store_dwordx4 v[1:4], off, s[8:11], 0
    let mut raw = s_load(8, 0, false).to_vec();
    raw.extend(special(1));
    raw.extend_from_slice(&[
        0x38 << 26 | STORE_DWORDX4 << 18,
        0x80 << 24 | 2 << 16 | 1 << 8,
    ]);
    let used = || vec![entry(ResourceType::Uav, 0, 4, 0)];
    let (on, off) = (
        guarded(true, || compute(&raw, used())),
        guarded(false, || compute(&raw, used())),
    );
    assert_eq!(on.dxil, off.dxil, "a raw store");
}
