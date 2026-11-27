//! The device titles get from device creation, laid out as their inlined
//! code expects: the object header, then device fields. Its table starts
//! like the PC device interface (IUnknown, the object methods, then the
//! device methods in PC order); console additions follow. Methods we have
//! not written log their table offset and fail, so a run shows what to
//! write next.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;

use crate::object::{
    self, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, HRESULT, Iid, Object, S_FALSE, S_OK, Table, This,
    guid, guid_text,
};
use crate::{
    descriptor, dxgi, fence, frame, list, pipeline, query, queue, resource, root, signature,
};

/// What D3D returns when none of the requested feature levels is supported.
const UNSUPPORTED: HRESULT = 0x887A_0004_u32 as i32;

/// The device interface id (the same as on PC). Titles also import it as
/// data.
pub static IID: Iid = guid(
    0x189819f1,
    0x1db6,
    0x4b57,
    [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7],
);

/// The levels the device answers to, as on PC: `ID3D12Device1` to
/// `ID3D12Device8`, and `ID3D12Device11`, which the title creates
/// its device as. Each level's table is the one before plus its methods.
const LEVEL_IIDS: [Iid; 9] = [
    guid(
        0x77acce80,
        0x638e,
        0x4e65,
        [0x88, 0x95, 0xc1, 0xf2, 0x33, 0x86, 0x86, 0x3e],
    ),
    guid(
        0x30baa41e,
        0xb15b,
        0x475c,
        [0xa0, 0xbb, 0x1a, 0xf5, 0xc5, 0xb6, 0x43, 0x28],
    ),
    guid(
        0x81dadc15,
        0x2bad,
        0x4392,
        [0x93, 0xc5, 0x10, 0x13, 0x45, 0xc4, 0xaa, 0x98],
    ),
    guid(
        0xe865df17,
        0xa9ee,
        0x46f9,
        [0xa4, 0x63, 0x30, 0x98, 0x31, 0x5a, 0xa2, 0xe5],
    ),
    guid(
        0x8b4f173b,
        0x2fea,
        0x4b80,
        [0x8f, 0x58, 0x43, 0x07, 0x19, 0x1a, 0xb9, 0x5d],
    ),
    guid(
        0xc70b221b,
        0x40e4,
        0x4a17,
        [0x89, 0xaf, 0x02, 0x5a, 0x07, 0x27, 0xa6, 0xdc],
    ),
    guid(
        0x5c014b53,
        0x68a1,
        0x4b9b,
        [0x8b, 0xd1, 0xdd, 0x60, 0x46, 0xb9, 0x35, 0x8b],
    ),
    guid(
        0x728f1fa2,
        0xea11,
        0x4a9f,
        [0x9b, 0x52, 0x9b, 0x42, 0xa4, 0x84, 0xd5, 0xc9],
    ),
    guid(
        0x5405c344,
        0xd457,
        0x444e,
        [0xb4, 0xdd, 0x23, 0x66, 0xe4, 0x5a, 0xee, 0x39],
    ),
];

/// Whether `iid` names the device: its first interface or a newer level.
pub(crate) fn is_device(iid: *const Iid) -> bool {
    object::is(iid, &[IID]) || object::is(iid, &LEVEL_IIDS)
}

/// The interface version these layouts were checked against (2.4). Both
/// Minecraft (March 2022) and the title (April 2026) pass it.
pub const CHECKED_VERSION: u32 = 0x0002_0004;

/// Feature level 12_2, what the console hardware supports.
const HIGHEST_FEATURE_LEVEL: u32 = 0xc200;

#[repr(C)]
pub struct Device {
    object: Object,
    /// GPU address of a null constant buffer; none yet.
    null_constant_buffer: u64,
    /// Byte 0 non-zero: calls the title would otherwise inline come to the
    /// table. The rest is reserved.
    out_of_line: [u32; 16],
}
const _: () = assert!(size_of::<Device>() == 0x58);

/// One device per process, as on the console.
pub(crate) static DEVICE: Device = Device {
    object: Object::new(&FUNCTIONS),
    null_constant_buffer: 0,
    out_of_line: [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
};

pub(crate) fn device() -> This {
    (&raw const DEVICE).cast()
}

/// Device creation: `(adapter, parameters, iid, out)`. The parameters start
/// with the interface version; the rest (ring and scratch sizes, features
/// to leave out) does not matter to us yet.
pub extern "system" fn create(
    _adapter: *const c_void,
    parameters: *const u32,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    if parameters.is_null() || iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: checked non-null; the title passes its parameters and an id.
    let (version, iid) = unsafe { (parameters.read(), &*iid) };
    tracing::info!(version = format_args!("{version:#x}"), "D3D12.X device");
    if version != CHECKED_VERSION {
        tracing::warn!(
            version = format_args!("{version:#x}"),
            "device layouts were checked against {CHECKED_VERSION:#x} only"
        );
    }
    if !is_device(iid) {
        tracing::warn!(iid = %guid_text(iid), "device created as an unknown interface");
        return E_NOINTERFACE;
    }
    if out.is_null() {
        return S_FALSE;
    }
    if *iid == LEVEL_IIDS[8] {
        NEWER.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    object::answer(object::DEVICE, iid, Some(device()), out)
}

/// Set when the title creates the device as `ID3D12Device11`. Its lists and
/// queues then have the newer layouts (two more methods each; see
/// `list` and `queue`), although the interface version stays 2.4.
/// shortcut: the creation interface is the only sign of the newer layouts
/// we have (the title, April 2026, versus Minecraft, March 2022);
/// look for a better one if a title breaks the rule.
static NEWER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn newer() -> bool {
    NEWER.load(std::sync::atomic::Ordering::Relaxed)
}

/// The device, or its DXGI side.
extern "system" fn query_interface(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    let found = if is_device(iid) {
        Some(this)
    } else if object::is(iid, &dxgi::DEVICE_IIDS) {
        Some(dxgi::device())
    } else {
        None
    };
    object::answer(object::DEVICE, iid, found, out)
}

/// What `D3D12ConfigureVirtualMemoryX` takes and fills in.
#[repr(C)]
pub struct VirtualMemory {
    flags: u32,
    /// GPU page table size in 4 MiB pages.
    page_table_pages: u32,
    /// Memory kept across title processes for the frame buffer.
    preserved_bytes: u32,
    /// Out: the range graphics addresses fall in.
    lowest: u64,
    highest: u64,
}
const _: () = assert!(size_of::<VirtualMemory>() == 0x20);

/// `d3d12_xs!D3D12ConfigureVirtualMemoryX`, called before any graphics
/// allocation. The inputs size console structures we do not have; the
/// range out is all of user space, since GPU addresses are CPU addresses.
pub extern "system" fn configure_virtual_memory(config: *mut VirtualMemory) -> HRESULT {
    // SAFETY: the title passes its configuration (null is tolerated).
    let Some(config) = (unsafe { config.as_mut() }) else {
        return E_INVALIDARG;
    };
    tracing::info!(
        flags = config.flags,
        page_table_pages = config.page_table_pages,
        preserved_bytes = config.preserved_bytes,
        "graphics virtual memory"
    );
    (config.lowest, config.highest) = user_space();
    S_OK
}

fn user_space() -> (u64, u64) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
        // SAFETY: plain data, filled in by the call.
        let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer.
        unsafe { GetSystemInfo(&mut info) };
        (
            info.lpMinimumApplicationAddress as u64,
            info.lpMaximumApplicationAddress as u64,
        )
    }
    #[cfg(not(windows))]
    (0x1_0000, 0x7fff_fffe_ffff)
}

extern "system" fn node_count(_: This) -> u32 {
    1
}

/// Feature query 2: the highest of the requested feature levels we support.
#[repr(C)]
struct FeatureLevels {
    count: u32,
    requested: *const u32,
    highest: u32,
}

/// Feature query 0, what an RDNA 2 GPU reports. The console's layout has
/// one field more than PC's (index 11).
const OPTIONS: [u32; 16] = [
    1,  // double-precision shader operations
    1,  // output-merger logic operations
    2,  // minimum precision: 16-bit
    3,  // tiled resources tier
    3,  // resource binding tier
    1,  // stencil reference from pixel shaders
    1,  // typed UAV loads of more formats
    1,  // rasterizer-ordered views
    3,  // conservative rasterization tier
    40, // GPU address bits per resource
    0,  // 64 KiB standard swizzle
    0,  // console-only field
    0,  // cross-node sharing tier
    0,  // cross-adapter row-major textures
    1,  // viewport and render target index from any stage
    2,  // resource heap tier
];

extern "system" fn check_feature_support(
    _: This,
    feature: u32,
    data: *mut c_void,
    size: u32,
) -> HRESULT {
    const OPTIONS_QUERY: u32 = 0;
    const ARCHITECTURE: u32 = 1;
    const FEATURE_LEVELS: u32 = 2;
    const FORMAT_SUPPORT: u32 = 3;
    const MULTISAMPLE: u32 = 4;
    let expected = match feature {
        OPTIONS_QUERY => size_of_val(&OPTIONS),
        ARCHITECTURE => 16,
        FEATURE_LEVELS => size_of::<FeatureLevels>(),
        FORMAT_SUPPORT => 12,
        MULTISAMPLE => 16,
        _ => {
            tracing::warn!(feature, size, "device feature query not implemented");
            return E_NOTIMPL;
        }
    };
    if data.is_null() || size as usize != expected {
        return E_INVALIDARG;
    }
    match feature {
        OPTIONS_QUERY => {
            // SAFETY: checked size and non-null.
            unsafe { data.cast::<[u32; 16]>().write_unaligned(OPTIONS) };
            S_OK
        }
        ARCHITECTURE => {
            // SAFETY: checked size and non-null.
            let a = unsafe { &mut *data.cast::<[u32; 4]>() };
            if a[0] != 0 {
                return E_INVALIDARG; // one node only
            }
            // Not tile-based; unified, cache-coherent memory.
            a[1..].copy_from_slice(&[0, 1, 1]);
            S_OK
        }
        FORMAT_SUPPORT => {
            // SAFETY: checked size and non-null.
            let q = unsafe { &mut *data.cast::<[u32; 3]>() };
            // shortcut: every use of every format; answer from the host
            // device once there is one.
            q[1] = 0x7fff_ffff;
            q[2] = 0xffff;
            S_OK
        }
        MULTISAMPLE => {
            // SAFETY: checked size and non-null.
            let q = unsafe { &mut *data.cast::<[u32; 4]>() };
            // Format, sample count, flags in; one quality level out for the
            // counts the hardware has.
            q[3] = u32::from(matches!(q[1], 1 | 2 | 4 | 8));
            S_OK
        }
        _ => feature_levels(data),
    }
}

fn feature_levels(data: *mut c_void) -> HRESULT {
    // SAFETY: the caller checked size and non-null.
    let query = unsafe { &mut *data.cast::<FeatureLevels>() };
    if query.requested.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the title passes `count` levels.
    let requested = unsafe { std::slice::from_raw_parts(query.requested, query.count as usize) };
    match requested
        .iter()
        .copied()
        .filter(|&l| l <= HIGHEST_FEATURE_LEVEL)
        .max()
    {
        Some(level) => {
            query.highest = level;
            S_OK
        }
        None => UNSUPPORTED,
    }
}

/// Device methods 0x1c8 and 0x218 (`SLOT`). The title calls both
/// (title+0x2477578 and title+0x24800d5) as `(this, description, interface
/// id, out)`, asks for `ID3D12PipelineState`, and on success keeps the
/// object `out` names and draws with it; on failure it goes without that
/// pipeline, silently. What the descriptions hold is not known: the calls
/// fail as they always did, with `out` null, and the first ones log the
/// description's words and what its first word points at, so that the two
/// methods can be written.
/// shortcut: not implemented. Answering `S_OK` here without making the
/// pipeline would hand the title whatever its stack held as an object.
extern "system" fn unknown_pipeline<const SLOT: usize>(
    _: This,
    desc: *const u8,
    _iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    use std::sync::atomic::{AtomicU32, Ordering};
    static CALLS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
    let calls = CALLS[usize::from(SLOT == 0x218)].fetch_add(1, Ordering::Relaxed) + 1;
    if !out.is_null() {
        // SAFETY: the title's out pointer (a local of its own).
        unsafe { *out = std::ptr::null() };
    }
    if calls <= 4 || calls.is_power_of_two() {
        // Sixteen 64-bit words at `at`, read through the kernel: neither
        // pointer is known to be mapped.
        #[cfg(windows)]
        let words = |at: usize| -> Option<Vec<u64>> {
            let mut bytes = [0u8; 128];
            (at != 0 && crate::host::read_into(at as *const u8, &mut bytes)).then(|| {
                bytes
                    .as_chunks::<8>().0.iter()
                    .map(|c| c.iter().rev().fold(0u64, |v, &b| v << 8 | u64::from(b)))
                    .collect()
            })
        };
        #[cfg(not(windows))]
        let words = |_: usize| None::<Vec<u64>>;
        let description = words(desc as usize);
        let first = description
            .as_ref()
            .and_then(|w| words(w[0] as usize));
        tracing::warn!(
            slot = format_args!("{:#x}", SLOT),
            calls,
            description = format_args!("{description:x?}"),
            first_points_at = format_args!("{first:x?}"),
            "a pipeline creation method that is not implemented: the title goes without this pipeline"
        );
    }
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("VICTORY_NATIVE_PSO_DUMP") {
        dump_native(SLOT, desc as usize, std::path::Path::new(&dir));
    }
    #[cfg(windows)]
    if SLOT == 0x218 && native_compute_on() && calls <= native_compute_max() {
        // `VICTORY_NATIVE_COMPUTE_LOG=a-b`: which program calls a to b are
        // (their code address: the dump's directory name, `218_<address>`).
        if let Some((a, b)) = std::env::var("VICTORY_NATIVE_COMPUTE_LOG").ok().and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
        }) && (a..=b).contains(&calls)
        {
            let mut w = [0u8; 0x30];
            if crate::host::read_into(desc as *const u8, &mut w) {
                let code = u64::from_le_bytes(w[0x28..0x30].try_into().unwrap_or([0; 8]));
                tracing::info!(call = calls, code = format_args!("{code:016x}"), "P.4: 0x218 call");
            }
        }
        if native_compute_skipped(desc as usize) {
            return E_NOTIMPL;
        }
        if let Some(container) = native_compute(desc as usize) {
            // Only a program that translates gets a pipeline: one whose
            // dispatches are then dropped makes the title believe the pass
            // ran and use what it did not write (the picture went white or
            // black with untranslatable programs made, 26 Nov). Without a
            // pipeline the title goes its own way round, as before.
            // (A translator panic, a bug of ours on a program it gets this far
            // in, is a program that does not translate: not a crash. The
            // translation is deterministic, so one that passes here does not
            // panic when the pipeline is made.)
            let translates = std::panic::catch_unwind(|| victory_shader::emit::translate(&container).is_ok());
            if !matches!(translates, Ok(true)) {
                NATIVE_COMPUTE[2].fetch_add(1, Ordering::Relaxed);
                return E_NOTIMPL;
            }
            let hr = crate::pipeline::create_compute_from(&container, _iid, out);
            NATIVE_COMPUTE[usize::from(hr < 0)].fetch_add(1, Ordering::Relaxed);
            if NATIVE_COMPUTE[0].load(Ordering::Relaxed) == 1 && hr >= 0 {
                tracing::info!("P.4: a 0x218 compute pipeline made from its pieces (VICTORY_NATIVE_COMPUTE)");
            }
            return hr;
        }
        NATIVE_COMPUTE[2].fetch_add(1, Ordering::Relaxed);
    }
    E_NOTIMPL
}

/// P.4 (`VICTORY_NATIVE_COMPUTE=1`): 0x218 pipelines made, refused by the
/// host, and left without one (not understood or not translating).
pub(crate) static NATIVE_COMPUTE: [std::sync::atomic::AtomicU32; 3] = [const { std::sync::atomic::AtomicU32::new(0) }; 3];

/// `VICTORY_NATIVE_COMPUTE_MAX=N`: only the first N calls of 0x218 are made
/// (to find, by halves, a program that breaks the picture).
fn native_compute_max() -> u32 {
    static MAX: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| std::env::var("VICTORY_NATIVE_COMPUTE_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(u32::MAX))
}

/// `VICTORY_NATIVE_COMPUTE_SKIP=<hex>,<hex>,…`: programs left out by their
/// code address (the dump's `218_<address>`): those found to break the
/// picture (`scripts/p4-hunt.sh`).
#[cfg(windows)]
fn native_compute_skipped(desc: usize) -> bool {
    static SKIP: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    let skip = SKIP.get_or_init(|| {
        std::env::var("VICTORY_NATIVE_COMPUTE_SKIP")
            .unwrap_or_default()
            .split(',')
            .filter_map(|h| u64::from_str_radix(h.trim().trim_start_matches("0x"), 16).ok())
            .collect()
    });
    // `VICTORY_NATIVE_COMPUTE_ONLY=<file>`: only the programs whose code
    // address is in that file (one hex address a line) are made
    // (`scripts/p4-hunt.sh` halves the list).
    static ONLY: std::sync::OnceLock<Option<Vec<u64>>> = std::sync::OnceLock::new();
    let only = ONLY.get_or_init(|| {
        let path = std::env::var_os("VICTORY_NATIVE_COMPUTE_ONLY")?;
        let text = std::fs::read_to_string(path).ok()?;
        Some(text.lines().filter_map(|h| u64::from_str_radix(h.trim().trim_start_matches("0x"), 16).ok()).collect())
    });
    if skip.is_empty() && only.is_none() {
        return false;
    }
    let mut w = [0u8; 0x30];
    if !crate::host::read_into(desc as *const u8, &mut w) {
        return true;
    }
    let code = u64::from_le_bytes(w[0x28..0x30].try_into().unwrap_or([0; 8]));
    skip.contains(&code) || only.as_ref().is_some_and(|only| !only.contains(&code))
}

fn native_compute_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_NATIVE_COMPUTE").is_some_and(|v| v == "1"))
}

/// P.4: device method 0x218 describes a compute program the console's
/// compiler made, as the pieces of an `FSPX` program (the format of an
/// `ACCC` part, `victory_shader::native::fspx_program`): words (1, 2) the
/// pipeline registers (0x1c bytes: thread-group size, RSRC1, RSRC2), (3, 4)
/// the pipeline metadata (0x14: a hash), (5, 6) the GFX10 code (in title
/// GPU memory), (7, 8) the program metadata (0x403 ...: the resources the
/// code reads). Put back together as an `FSPX` blob in a container with one
/// `ACCC` part, it goes the way of any compute container. `None`: a piece
/// missing, unreadable or of another shape.
#[cfg(windows)]
fn native_compute(desc: usize) -> Option<Vec<u8>> {
    let read = |at: usize, n: usize| -> Option<Vec<u8>> {
        let mut b = vec![0u8; n];
        (at != 0 && n > 0 && n <= 1 << 20 && crate::host::read_into(at as *const u8, &mut b)).then_some(b)
    };
    let d = read(desc, 0x48)?;
    let w = |i: usize| u64::from_le_bytes(d[8 * i..8 * i + 8].try_into().unwrap_or([0; 8])) as usize;
    let packet = read(w(1), w(2))?;
    let hash = read(w(3), w(4))?;
    let code = read(w(5), w(6))?;
    let meta = read(w(7), w(8))?;
    if packet.len() != 0x1c || meta.get(0..4) != Some(&0x403u32.to_le_bytes()[..]) {
        return None;
    }
    // FSPX: "FSPX", u32 1, u32 entries, u32 size; entries of 16 B (kind,
    // offset, size, 0); then the pieces, 16-byte aligned.
    let pieces: [(u32, &[u8]); 4] = [(0, &packet), (1, &hash), (6, &code), (7, &meta)];
    let mut fspx = vec![0u8; 0x10 + 16 * pieces.len()];
    let mut table = Vec::new();
    for (kind, bytes) in pieces {
        while fspx.len() % 16 != 0 {
            fspx.push(0);
        }
        table.push((kind, fspx.len() as u32, bytes.len() as u32));
        fspx.extend_from_slice(bytes);
    }
    let size = fspx.len() as u32;
    fspx[0..4].copy_from_slice(b"FSPX");
    fspx[4..8].copy_from_slice(&1u32.to_le_bytes());
    fspx[8..12].copy_from_slice(&(pieces.len() as u32).to_le_bytes());
    fspx[12..16].copy_from_slice(&size.to_le_bytes());
    for (i, (kind, offset, len)) in table.into_iter().enumerate() {
        let at = 0x10 + 16 * i;
        fspx[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        fspx[at + 4..at + 8].copy_from_slice(&offset.to_le_bytes());
        fspx[at + 8..at + 12].copy_from_slice(&len.to_le_bytes());
    }
    // DXBC: "DXBC", a 16-byte digest (here: the code's FNV-1a, twice), u16
    // 1, u16 0, u32 total size, u32 1 part, u32 its offset; the part: "ACCC",
    // u32 size, data.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in code.iter().chain(meta.iter()).chain(packet.iter()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    let total = 0x24 + 8 + fspx.len();
    let mut c = Vec::with_capacity(total);
    c.extend_from_slice(b"DXBC");
    c.extend_from_slice(&h.to_le_bytes());
    c.extend_from_slice(&(!h).to_le_bytes());
    c.extend_from_slice(&1u16.to_le_bytes());
    c.extend_from_slice(&0u16.to_le_bytes());
    c.extend_from_slice(&(total as u32).to_le_bytes());
    c.extend_from_slice(&1u32.to_le_bytes());
    c.extend_from_slice(&0x24u32.to_le_bytes());
    c.extend_from_slice(b"ACCC");
    c.extend_from_slice(&(fspx.len() as u32).to_le_bytes());
    c.extend_from_slice(&fspx);
    Some(c)
}

/// From fxceless/victory-fork (`52ac4ec`), for P.4. `VICTORY_NATIVE_PSO_DUMP=<dir>` (off unless set): the blobs
/// a 0x1c8 / 0x218 description names, once per description key (0x218: its
/// word 11, a 64-bit hash; 0x1c8: the code pointers of words 5 and 9).
/// Files: `<dir>/<slot>_<key>/desc.bin` (the first 0x80 bytes), `root.bin`
/// (0x80 bytes at word 0), `w<i>_<size>.bin` for each (pointer, size) pair
/// at words (1,2) (3,4) (5,6) (7,8) (9,10) (11,12), and `p9.bin` /
/// `p10.bin` (0x100 bytes at 0x218's two unsized pointers). Everything is
/// read through the kernel (`host::read_into`): an unmapped pointer is
/// skipped, never touched.
#[cfg(windows)]
fn dump_native(slot: usize, desc: usize, dir: &std::path::Path) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());
    let read = |at: usize, n: usize| -> Option<Vec<u8>> {
        let mut b = vec![0u8; n];
        (at != 0 && n <= 1 << 20 && crate::host::read_into(at as *const u8, &mut b)).then_some(b)
    };
    let Some(d) = read(desc, 0x100) else {
        return;
    };
    let w = |i: usize| u64::from_le_bytes(d[8 * i..8 * i + 8].try_into().unwrap_or([0; 8]));
    // 0x218: its code pointer (word 11 is not a key: gfx41); 0x1c8: the
    // code pointers of words 5 and 9.
    let key = if slot == 0x218 { w(5) } else { w(5) ^ (w(9) << 1) };
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.contains(&(slot, key)) || seen.len() >= 16384 {
            return;
        }
        seen.push((slot, key));
    }
    let sub = dir.join(format!("{slot:x}_{key:016x}"));
    let _ = std::fs::create_dir_all(&sub);
    let _ = std::fs::write(sub.join("desc.bin"), &d);
    if let Some(r) = read(w(0) as usize, 0x80) {
        let _ = std::fs::write(sub.join("root.bin"), r);
    }
    let pairs: &[usize] = if slot == 0x218 { &[1, 3, 5, 7] } else { &[1, 3, 5, 7, 9, 11, 13, 15] };
    for &i in pairs {
        let (at, size) = (w(i) as usize, w(i + 1) as usize);
        if at == 0 || size == 0 || size > 1 << 20 {
            continue;
        }
        if let Some(b) = read(at, size) {
            let _ = std::fs::write(sub.join(format!("w{i}_{size:x}.bin")), b);
        }
    }
    if slot == 0x218 {
        for i in [9usize, 10] {
            if let Some(b) = read(w(i) as usize, 0x100) {
                let _ = std::fs::write(sub.join(format!("p{i}.bin")), b);
            }
        }
    }
}


/// The host device's state: the console's device is removed when the host's is
/// (VIctory 13185f5: the default answer, E_NOTIMPL, reads as a lost device).
extern "system" fn removed_reason(_: This) -> HRESULT {
    #[cfg(windows)]
    if let Some(host) = crate::host::host() {
        // SAFETY: a plain query.
        if let Err(e) = unsafe { host.device.GetDeviceRemovedReason() } {
            return e.code().0;
        }
    }
    S_OK
}

/// IUnknown 3, object methods 4, device methods 76 (0x18-0x290), then the
/// methods `ID3D12Device1` to `ID3D12Device8` add, in order: 3, 1, 3, 6, 9,
/// 1, 2, 9 (0x298-0x3a0, the console's own among them), then those of
/// `ID3D12Device9` to `ID3D12Device11`, whose console layout is unknown.
/// Every slot exists so that a call to one we have not written logs it.
static FUNCTIONS: Table<160> = {
    let mut f = object::d3d12_object(object::missing_table::<{ object::DEVICE }, 160>());
    f[0] = query_interface as *const (); // 0x00
    f[7] = node_count as *const (); // 0x38
    f[8] = queue::create as *const (); // 0x40
    f[9] = list::create_allocator as *const (); // 0x48
    f[12] = list::create_list as *const (); // 0x60
    f[10] = pipeline::create_graphics as *const (); // 0x50
    f[11] = pipeline::create_compute as *const (); // 0x58
    f[13] = check_feature_support as *const (); // 0x68
    f[14] = descriptor::create_heap as *const (); // 0x70
    f[15] = descriptor::increment_size as *const (); // 0x78
    f[16] = root::create as *const (); // 0x80
    f[17] = descriptor::create_cbv as *const (); // 0x88
    f[18] = descriptor::create_srv as *const (); // 0x90
    f[19] = descriptor::create_uav as *const (); // 0x98
    f[20] = descriptor::create_rtv as *const (); // 0xa0
    f[21] = descriptor::create_dsv as *const (); // 0xa8
    f[22] = descriptor::create_sampler as *const (); // 0xb0
    f[23] = descriptor::copy as *const (); // 0xb8
    f[24] = descriptor::copy_simple as *const (); // 0xc0
    f[25] = resource::allocation_info as *const (); // 0xc8
    f[26] = resource::custom_heap_properties as *const (); // 0xd0
    f[27] = resource::create_committed as *const (); // 0xd8
    f[28] = resource::create_heap as *const (); // 0xe0
    f[29] = resource::create_placed as *const (); // 0xe8
    f[36] = fence::create as *const (); // 0x120
    f[37] = removed_reason as *const (); // 0x128
    f[38] = resource::copyable_footprints as *const (); // 0x130
    f[39] = query::create as *const (); // 0x138
    f[41] = signature::create as *const (); // 0x148
    f[48] = queue::create_x as *const (); // 0x180
    f[57] = unknown_pipeline::<0x1c8> as *const (); // 0x1c8
    f[61] = descriptor::create_sampler_x as *const (); // 0x1e8
    f[62] = pipeline::create_compute_x as *const (); // 0x1f0
    f[67] = unknown_pipeline::<0x218> as *const (); // 0x218
    f[69] = object::ignore as *const (); // 0x228 debug error filter
    f[70] = descriptor::create_placed_srv as *const (); // 0x230
    f[71] = descriptor::create_placed_uav as *const (); // 0x238
    f[73] = list::create_list_x as *const (); // 0x248
    f[74] = resource::register_page_pool as *const (); // 0x250
    f[75] = resource::unregister_page_pool as *const (); // 0x258
    // CreateCommittedResourceX: its description is PC's plus DCC options,
    // which we have no use for (the AMD upscaler's, on Series S).
    f[76] = resource::create_committed as *const (); // 0x260
    f[78] = frame::set_interval as *const (); // 0x270
    f[79] = frame::schedule as *const (); // 0x278
    f[80] = frame::wait as *const (); // 0x280
    f[81] = frame::statistics as *const (); // 0x288
    f[86] = pipeline::create_stream as *const (); // 0x2b0
    f[108] = resource::allocation_info2 as *const (); // 0x360
    f[127] = resource::create_committed3 as *const (); // 0x3f8
    f[133] = resource::create_at_address as *const (); // 0x428
    Table(f)
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::slot;

    fn create_as(iid: &Iid) -> (HRESULT, This) {
        let version = CHECKED_VERSION;
        let mut device = std::ptr::null();
        let hr = create(std::ptr::null(), &version, iid, &mut device);
        (hr, device)
    }

    #[test]
    fn creation_hands_out_the_device() {
        let (hr, device) = create_as(&IID);
        assert_eq!(hr, S_OK);
        assert_eq!(device, super::device());
        // The title tests byte 0x18 before inlining a device call.
        // SAFETY: the device is 0x58 bytes.
        assert_ne!(unsafe { device.cast::<u8>().add(0x18).read() }, 0);
        assert_eq!(create_as(&[0; 16]).0, E_NOINTERFACE);
        assert_eq!(
            slot::<extern "system" fn(This) -> u32>(device, 0x38)(device),
            1
        );
        let missing = slot::<extern "system" fn() -> HRESULT>(device, 0xf0);
        assert_eq!(missing(), E_NOTIMPL);
    }

    #[test]
    fn newer_device_levels_are_the_same_device() {
        // ID3D12Device11, as the title asks for it.
        let (hr, device) = create_as(&LEVEL_IIDS[8]);
        assert_eq!((hr, device), (S_OK, super::device()));
        let mut same = std::ptr::null();
        assert_eq!(query_interface(device, &LEVEL_IIDS[4], &mut same), S_OK);
        assert_eq!(same, device);
        // A Device8 method we have not written fails without crashing.
        let missing = slot::<extern "system" fn() -> HRESULT>(device, 0x3a0);
        assert_eq!(missing(), E_NOTIMPL);
    }

    /// A device child, and the DXGI device, hand out the device as any
    /// interface the device itself answers to.
    #[test]
    fn the_device_is_found_at_every_level_through_others() {
        type Get = extern "system" fn(This, *const Iid, *mut This) -> HRESULT;
        let fence_iid = &crate::fence::FENCE_IIDS[0];
        let mut fence = std::ptr::null();
        assert_eq!(
            crate::fence::create(std::ptr::null(), 0, 0, fence_iid, &mut fence),
            S_OK
        );
        let get_device = slot::<Get>(fence, 0x38);
        let dxgi = dxgi::device();
        let query = slot::<Get>(dxgi, 0);
        for iid in std::iter::once(&IID).chain(&LEVEL_IIDS) {
            let mut found = std::ptr::null();
            assert_eq!(get_device(fence, iid, &mut found), S_OK, "{iid:02x?}");
            assert_eq!(found, device());
            let mut found = std::ptr::null();
            assert_eq!(query(dxgi, iid, &mut found), S_OK, "{iid:02x?}");
            assert_eq!(found, device());
        }
        // What the device is not, it is not through them either.
        let mut found = device();
        assert_eq!(get_device(fence, fence_iid, &mut found), E_NOINTERFACE);
        assert!(found.is_null());
        slot::<extern "system" fn(This) -> u32>(fence, 0x10)(fence);
    }

    /// The host's: a fence's `GetDevice` for `ID3D12Device5`.
    #[cfg(windows)]
    #[test]
    fn the_hosts_children_hand_out_newer_device_levels() {
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_FENCE_FLAG_NONE, ID3D12Device5, ID3D12Fence,
        };
        let Some(host) = crate::host::host() else {
            return; // no host GPU here
        };
        // SAFETY: plain calls on the host device and its fence.
        unsafe {
            let fence: ID3D12Fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
            let mut device: Option<ID3D12Device5> = None;
            assert!(fence.GetDevice(&mut device).is_ok());
            assert!(device.is_some());
        }
    }

    #[test]
    fn virtual_memory_configuration_reports_user_space() {
        let mut config = VirtualMemory {
            flags: 0,
            page_table_pages: 7,
            preserved_bytes: 0,
            lowest: 0,
            highest: 0,
        };
        assert_eq!(configure_virtual_memory(&mut config), S_OK);
        assert!(0 < config.lowest && config.lowest < config.highest);
        let heap = Box::new(0u8);
        assert!((config.lowest..=config.highest).contains(&(&raw const *heap as u64)));
        assert_eq!(configure_virtual_memory(std::ptr::null_mut()), E_INVALIDARG);
    }

    #[test]
    fn feature_levels_pick_the_highest_supported() {
        type Check = extern "system" fn(This, u32, *mut c_void, u32) -> HRESULT;
        let device = device();
        let check = slot::<Check>(device, 0x68);
        let query = |levels: &[u32], size| {
            let mut query = FeatureLevels {
                count: levels.len() as u32,
                requested: levels.as_ptr(),
                highest: 0,
            };
            let hr = check(device, 2, (&raw mut query).cast(), size);
            (hr, query.highest)
        };
        let levels = [0xb000, 0xb100, 0xc000, 0xc100, 0xd000];
        assert_eq!(query(&levels, 0x18), (S_OK, 0xc100));
        assert_eq!(query(&levels, 0x10).0, E_INVALIDARG);
        assert_eq!(query(&levels[4..], 0x18).0, UNSUPPORTED);
    }
}
