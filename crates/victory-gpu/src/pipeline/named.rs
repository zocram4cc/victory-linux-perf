//! Compute programs named by their container's name, for experiments with
//! a command line (breakwater 4f65819 and 874af5e). All off unless set:
//!
//! - `VICTORY_SHADER_NAMES=1`: every shader's name (the first `CS_`, `PS_`,
//!   `VS_`, `MS_` or `AS_` word in its container) logged with its hash when
//!   the title first creates it.
//! - `VICTORY_VIEWS_OF=<part>,...`: for a compute program whose container
//!   holds one of the parts, the views it reads and writes through its
//!   tables, once a program: kind, root parameter, dwords into the table,
//!   register, and the resource there (dimension, format, size).
//! - `VICTORY_CLEAR_AFTER=<part>:<root parameter>:<dwords>[:<w>x<h>];...`:
//!   after each dispatch of such a program, the unordered view at that
//!   place of its tables is cleared to zeros (with a size, only that corner
//!   at the top left). `<part>:*` clears every unordered view it has in its
//!   tables (the first of each range).
//! - `VICTORY_CLEAR_BEFORE=...`: the same, before each dispatch: what the
//!   program reads of its outputs before it writes them is zeros.
//!
//! The part is matched anywhere in the container, as upstream does: a part
//! that is the start of a longer name matches that one too
//! (`CS_RefractionRayEstimation` also matches `...EstimationBlur`), where a
//! place it does not have is left out with one line. The clears are the
//! host's `ClearUnorderedAccessView*` through [`crate::draw::State::clear_uav`],
//! as the title's own clears (`VICTORY_UAV_CLEARS`), with an unordered-access
//! barrier on each side; buffers, placed views and depth textures are left
//! out as there, with one line each.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use victory_shader::{ResourceType, RootType, UsedResource};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_TYPE_UAV, ID3D12GraphicsCommandList};
use windows::Win32::Graphics::Dxgi::Common::*;

use super::Shader;

/// One place to clear: (root parameter, dwords into its table), or every
/// unordered view of the tables; and the top-left corner to clear, if not all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) at: Option<(u8, u32)>,
    pub(crate) corner: Option<(u32, u32)>,
}

/// What is asked for one program.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub(crate) views: bool,
    pub(crate) before: Vec<Place>,
    pub(crate) after: Vec<Place>,
}

/// `VICTORY_CLEAR_*`: `<part>:<slot>:<dwords>[:<w>x<h>]` or `<part>:*`,
/// separated by `;`. What does not parse is left out.
fn parse_clears(list: &str) -> Vec<(Vec<u8>, Place)> {
    list.split(';')
        .filter_map(|entry| {
            let mut parts = entry.trim().split(':');
            let name = parts.next()?.trim().as_bytes().to_vec();
            if name.is_empty() {
                return None;
            }
            let first = parts.next()?.trim();
            if first == "*" {
                return Some((name, Place { at: None, corner: None }));
            }
            let slot = first.parse().ok()?;
            let dwords = parts.next()?.trim().parse().ok()?;
            let corner = match parts.next() {
                None => None,
                Some(size) => {
                    let (w, h) = size.trim().split_once('x')?;
                    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
                    (w > 0 && h > 0).then_some((w, h))
                }
            };
            Some((name, Place { at: Some((slot, dwords)), corner }))
        })
        .collect()
}

/// `VICTORY_VIEWS_OF`: parts of names, separated by commas.
fn parse_parts(list: &str) -> Vec<Vec<u8>> {
    list.split(',').map(|p| p.trim().as_bytes().to_vec()).filter(|p| !p.is_empty()).collect()
}

struct Asked {
    views: Vec<Vec<u8>>,
    before: Vec<(Vec<u8>, Place)>,
    after: Vec<(Vec<u8>, Place)>,
}

fn asked() -> Option<&'static Asked> {
    static ASKED: OnceLock<Option<Asked>> = OnceLock::new();
    ASKED
        .get_or_init(|| {
            let var = |name| std::env::var(name).unwrap_or_default();
            let asked = Asked {
                views: parse_parts(&var("VICTORY_VIEWS_OF")),
                before: parse_clears(&var("VICTORY_CLEAR_BEFORE")),
                after: parse_clears(&var("VICTORY_CLEAR_AFTER")),
            };
            if asked.views.is_empty() && asked.before.is_empty() && asked.after.is_empty() {
                return None;
            }
            tracing::info!(
                views_of = asked.views.len(),
                clear_before = asked.before.len(),
                clear_after = asked.after.len(),
                "named compute programs (VICTORY_VIEWS_OF, VICTORY_CLEAR_BEFORE, VICTORY_CLEAR_AFTER)"
            );
            Some(asked)
        })
        .as_ref()
}

fn holds(container: &[u8], part: &[u8]) -> bool {
    container.windows(part.len()).any(|w| w == part)
}

/// What `asked` wants of a compute program with this container.
fn wanted_of(asked: &Asked, container: &[u8]) -> Wanted {
    let places = |list: &[(Vec<u8>, Place)]| -> Vec<Place> {
        let mut places: Vec<Place> = Vec::new();
        for (part, place) in list {
            if holds(container, part) && !places.contains(place) {
                places.push(*place);
            }
        }
        places
    };
    Wanted {
        views: asked.views.iter().any(|part| holds(container, part)),
        before: places(&asked.before),
        after: places(&asked.after),
    }
}

/// What is asked for the program of `shader`, if anything: `None` at once
/// while none of the switches is set.
pub(crate) fn of(shader: &Shader) -> Option<Arc<Wanted>> {
    let asked = asked()?;
    if shader.stage != "compute" {
        return None;
    }
    static OF: Mutex<Option<HashMap<String, Option<Arc<Wanted>>>>> = Mutex::new(None);
    let mut of = OF.lock().unwrap_or_else(|e| e.into_inner());
    of.get_or_insert_with(HashMap::new)
        .entry(shader.hash.clone())
        .or_insert_with(|| {
            let wanted = wanted_of(asked, &shader.container);
            (wanted != Wanted::default()).then(|| Arc::new(wanted))
        })
        .clone()
}

/// The program's name in `container`, if it has one.
pub(crate) fn name_in(container: &[u8]) -> Option<String> {
    let at = container.windows(3).position(|w| matches!(w, b"CS_" | b"PS_" | b"VS_" | b"MS_" | b"AS_"))?;
    let end = container[at..]
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
        .map_or(container.len(), |n| at + n);
    Some(String::from_utf8_lossy(&container[at..end]).into_owned())
}

/// A shader is first created (`VICTORY_SHADER_NAMES=1`: its name is logged).
pub(crate) fn created(stage: &str, hash: &str, container: &[u8]) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("VICTORY_SHADER_NAMES").is_some_and(|v| v == "1")) {
        tracing::info!(
            stage,
            hash,
            size = container.len(),
            name = name_in(container).unwrap_or_default(),
            "shader name (VICTORY_SHADER_NAMES)"
        );
    }
}

/// Whether `key` is new to `said` (for lines said once).
fn first(said: &Mutex<Vec<String>>, key: String) -> bool {
    let mut said = said.lock().unwrap_or_else(|e| e.into_inner());
    if said.contains(&key) {
        return false;
    }
    said.push(key);
    true
}

/// Whether the views of `shader`'s program are to be said now: once.
pub(crate) fn first_views(shader: &Shader) -> bool {
    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
    first(&SAID, shader.hash.clone())
}

/// `VICTORY_VIEWS_OF`: one view of a program's tables, and the resource
/// there (dimension, format, width, height, depth or slices, mips).
pub(crate) fn say_view(shader: &Shader, used: &UsedResource, resource: Option<[u64; 6]>) {
    tracing::info!(
        program = name_in(&shader.container).unwrap_or_default(),
        hash = %shader.hash,
        kind = ?used.kind,
        slot = used.slot,
        dwords = used.table_offset_dwords,
        register = used.lower_bound,
        resource = ?resource,
        "a view of a named program (VICTORY_VIEWS_OF; resource: dimension, format, width, height, depth or slices, mips)"
    );
}

/// Where `places` (before or after a dispatch) are in a program that reads
/// through `used`: every unordered view of its tables for `*`, else the
/// places that are such views; one that is not is left out, said once.
pub(crate) fn places(shader: &Shader, places: &[Place], used: &[UsedResource], after: bool) -> Vec<((u8, u32), Option<(u32, u32)>)> {
    let unordered = |u: &&UsedResource| u.kind == ResourceType::Uav && u.root == RootType::Table;
    let mut out: Vec<((u8, u32), Option<(u32, u32)>)> = Vec::new();
    for place in places {
        match place.at {
            None => {
                for u in used.iter().filter(unordered) {
                    let at = (u.slot, u.table_offset_dwords);
                    if !out.iter().any(|(a, _)| *a == at) {
                        out.push((at, place.corner));
                    }
                }
            }
            Some(at) if used.iter().filter(unordered).any(|u| (u.slot, u.table_offset_dwords) == at) => {
                if !out.iter().any(|(a, _)| *a == at) {
                    out.push((at, place.corner));
                }
            }
            Some(at) => cleared(shader, at, after, Err("not an unordered view of the program's tables")),
        }
    }
    out
}

/// What a clear of `at` came to: said once a program, place and outcome.
pub(crate) fn cleared(shader: &Shader, at: (u8, u32), after: bool, result: Result<(), &'static str>) {
    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let key = format!("{} {at:?} {after} {result:?}", shader.hash);
    if !first(&SAID, key) {
        return;
    }
    let switch = if after { "VICTORY_CLEAR_AFTER" } else { "VICTORY_CLEAR_BEFORE" };
    let program = name_in(&shader.container).unwrap_or_default();
    match result {
        Ok(()) => tracing::info!(program, hash = %shader.hash, slot = at.0, dwords = at.1, switch, "an output of a named program is cleared to zeros"),
        Err(why) => {
            tracing::info!(program, hash = %shader.hash, slot = at.0, dwords = at.1, switch, why, "an output of a named program is not cleared")
        }
    }
}

/// Whether views in `format` take integer clears.
fn integer(format: DXGI_FORMAT) -> bool {
    [
        DXGI_FORMAT_R32G32B32A32_UINT,
        DXGI_FORMAT_R32G32B32A32_SINT,
        DXGI_FORMAT_R32G32B32_UINT,
        DXGI_FORMAT_R32G32B32_SINT,
        DXGI_FORMAT_R16G16B16A16_UINT,
        DXGI_FORMAT_R16G16B16A16_SINT,
        DXGI_FORMAT_R32G32_UINT,
        DXGI_FORMAT_R32G32_SINT,
        DXGI_FORMAT_R10G10B10A2_UINT,
        DXGI_FORMAT_R8G8B8A8_UINT,
        DXGI_FORMAT_R8G8B8A8_SINT,
        DXGI_FORMAT_R16G16_UINT,
        DXGI_FORMAT_R16G16_SINT,
        DXGI_FORMAT_R32_UINT,
        DXGI_FORMAT_R32_SINT,
        DXGI_FORMAT_R8G8_UINT,
        DXGI_FORMAT_R8G8_SINT,
        DXGI_FORMAT_R16_UINT,
        DXGI_FORMAT_R16_SINT,
        DXGI_FORMAT_R8_UINT,
        DXGI_FORMAT_R8_SINT,
    ]
    .contains(&format)
}

/// Zeros for a view of `format` (the view's, else its resource's): an
/// integer clear for integer formats, a float clear of 0.0 for the others.
pub(crate) fn zeros(format: u32) -> crate::list::UavValues {
    if integer(DXGI_FORMAT(format as i32)) {
        crate::list::UavValues::Uint([0; 4])
    } else {
        crate::list::UavValues::Float([0.0; 4])
    }
}

/// An unordered-access barrier over everything, around the clears.
pub(crate) fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(slot: u8, dwords: u32) -> Place {
        Place { at: Some((slot, dwords)), corner: None }
    }

    /// Upstream's setting for the refraction passes parses as it reads.
    #[test]
    fn clears_parse_as_upstream_writes_them() {
        let set = "CS_RayPrePass_Alpha:1:112;CS_RayPrePass_Alpha:1:120;CS_RayPrePass_Alpha:1:128;\
                   CS_RayPrePass_Alpha:1:136;CS_RefractionRayEstimation:1:40;CS_RefractionRayEstimationBlur:1:48";
        let clears = parse_clears(set);
        assert_eq!(clears.len(), 6);
        assert_eq!(clears[0], (b"CS_RayPrePass_Alpha".to_vec(), place(1, 112)));
        assert_eq!(clears[5], (b"CS_RefractionRayEstimationBlur".to_vec(), place(1, 48)));
        let more = parse_clears("A:2:8:80x45; B:* ;C:1;:1:2;D:1:2:0x5;E:x:1");
        assert_eq!(
            more,
            vec![
                (b"A".to_vec(), Place { at: Some((2, 8)), corner: Some((80, 45)) }),
                (b"B".to_vec(), Place { at: None, corner: None }),
                // (A size of 0: all of it, as upstream.)
                (b"D".to_vec(), place(1, 2)),
            ]
        );
        assert_eq!(parse_parts(" CS_A, ,CS_B"), vec![b"CS_A".to_vec(), b"CS_B".to_vec()]);
    }

    /// A part matches anywhere in the container: a longer name too.
    #[test]
    fn programs_are_matched_by_a_part_of_their_name() {
        let asked = Asked {
            views: parse_parts("RayPrePass"),
            before: Vec::new(),
            after: parse_clears("CS_RefractionRayEstimation:1:40;CS_RefractionRayEstimationBlur:1:48"),
        };
        let blur = wanted_of(&asked, b"..DXIL..CS_RefractionRayEstimationBlur\0..");
        assert_eq!(blur.after, vec![place(1, 40), place(1, 48)]);
        assert!(!blur.views);
        let estimate = wanted_of(&asked, b"CS_RefractionRayEstimation\0");
        assert_eq!(estimate.after, vec![place(1, 40)]);
        let pre = wanted_of(&asked, b"CS_RayPrePass_Alpha\0");
        assert!(pre.views && pre.after.is_empty());
        assert_eq!(wanted_of(&asked, b"CS_Other\0"), Wanted::default());
    }

    #[test]
    fn names_are_read_from_containers() {
        assert_eq!(name_in(b"\x01\x02CS_RayPrePass_Alpha\0rest").as_deref(), Some("CS_RayPrePass_Alpha"));
        assert_eq!(name_in(b"xxPS_Hair").as_deref(), Some("PS_Hair"));
        assert_eq!(name_in(b"nothing here"), None);
    }

    /// Only unordered views of the tables are cleared; `*` is all of them.
    #[test]
    fn places_are_unordered_views_of_the_tables() {
        let used = |kind, root, slot, dwords| UsedResource {
            kind,
            root,
            table_offset_dwords: dwords,
            lower_bound: 0,
            stride: 0,
            dwords: 8,
            user_data: 0,
            slot,
        };
        let used = [
            used(ResourceType::Srv, RootType::Table, 1, 0),
            used(ResourceType::Uav, RootType::Table, 1, 112),
            used(ResourceType::Uav, RootType::Table, 1, 120),
            used(ResourceType::Uav, RootType::Table, 2, 0),
        ];
        let shader = Shader {
            stage: "compute",
            hash: "0123456789abcdef".to_owned(),
            container: b"CS_Test".to_vec(),
            ambient: None,
            pre_skinned: false,
            skinning: false,
            translated: OnceLock::new(),
        };
        let corner = Some((80, 45));
        let asked = [place(1, 112), place(1, 0), Place { at: Some((1, 120)), corner }, place(1, 112)];
        assert_eq!(places(&shader, &asked, &used, true), vec![((1, 112), None), ((1, 120), corner)]);
        let all = [Place { at: None, corner: None }];
        assert_eq!(places(&shader, &all, &used, false), vec![((1, 112), None), ((1, 120), None), ((2, 0), None)]);
    }

    #[test]
    fn integer_views_get_integer_zeros() {
        assert!(matches!(zeros(DXGI_FORMAT_R32_UINT.0 as u32), crate::list::UavValues::Uint([0, 0, 0, 0])));
        assert!(matches!(zeros(DXGI_FORMAT_R16_UINT.0 as u32), crate::list::UavValues::Uint(_)));
        assert!(matches!(zeros(DXGI_FORMAT_R32_FLOAT.0 as u32), crate::list::UavValues::Float(v) if v == [0.0; 4]));
        assert!(matches!(zeros(DXGI_FORMAT_R16G16_FLOAT.0 as u32), crate::list::UavValues::Float(_)));
        assert!(matches!(zeros(DXGI_FORMAT_R11G11B10_FLOAT.0 as u32), crate::list::UavValues::Float(_)));
    }
}
