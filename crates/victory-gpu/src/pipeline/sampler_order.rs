//! Static samplers made in one kept order, before any root signature of a
//! pipeline that has them (`VICTORY_SAMPLER_ORDER=0`: not, as before).
//!
//! The card's driver (NVIDIA) keeps static samplers in a table of its own,
//! gives each a place when the first root signature holding it is created,
//! and puts the places into the compiled pipeline
//! (`library::with_static_samplers`; measured with `examples/lut3d_probe.rs`:
//! root signatures with other static samplers made first move a later one's
//! place). Pipelines with static samplers are therefore never kept in our
//! library and are left to the driver's cache on disk. That cache answers
//! only where the places are what they were when it compiled:
//!
//! Measured (runs 425 to 430, compute pipelines, time from "shader
//! translated" to "host pipeline", the same translated bytes in every run):
//! run 425: 0 of 1,303 over 1 s. Run 430: 406 over 1 s (3 to 10 s each,
//! 2,259 s in all, made four at a time: the minutes of "being made" after a
//! start, 2 fps in the world meanwhile): 0 of 803 with 0 to 2 static
//! samplers, 27 of 91 with 3 to 5, 379 of 419 with 6 to 9. 187 of run 429's
//! 202 were compiled again in run 430 by the same executable.
//!
//! The places follow the order in which the threads that make pipelines
//! happen to reach their root signatures. Here every static sampler a
//! pipeline of the title has ever had is listed in a file beside the
//! pipeline library, in the order it was first met, and before the first
//! root signature with a static sampler a root signature of its own is made
//! for each listed one, in that order, and kept for the run (a place is the
//! sampler's while a root signature holds it). A sampler not listed yet is
//! made the same way, one at a time, before its pipeline's root signature,
//! and added to the end of the file. So a sampler's place is its line in the
//! file, in every run.
//!
//! The first run with this compiles what the driver's cache holds under
//! other places once more; from the second on the cache answers.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;

/// The list, in the pipeline cache's folder: a line a sampler, thirteen
/// hexadecimal words ([`Key`]).
const FILE: &str = "static-samplers.txt";

/// The most samplers listed and made early. (The card's table holds 2,032
/// distinct ones; the 2,032nd removes the device: `lut3d_probe`.)
const MOST: usize = 1024;

/// A static sampler as the host is told of it, whole: filter, the three
/// address modes, the bias' bits, the anisotropy, the comparison, the border
/// colour, the bits of both level bounds, register, space and visibility.
type Key = [u32; 13];

fn key(s: &D3D12_STATIC_SAMPLER_DESC) -> Key {
    [
        s.Filter.0 as u32,
        s.AddressU.0 as u32,
        s.AddressV.0 as u32,
        s.AddressW.0 as u32,
        s.MipLODBias.to_bits(),
        s.MaxAnisotropy,
        s.ComparisonFunc.0 as u32,
        s.BorderColor.0 as u32,
        s.MinLOD.to_bits(),
        s.MaxLOD.to_bits(),
        s.ShaderRegister,
        s.RegisterSpace,
        s.ShaderVisibility.0 as u32,
    ]
}

fn description(k: &Key) -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER(k[0] as i32),
        AddressU: D3D12_TEXTURE_ADDRESS_MODE(k[1] as i32),
        AddressV: D3D12_TEXTURE_ADDRESS_MODE(k[2] as i32),
        AddressW: D3D12_TEXTURE_ADDRESS_MODE(k[3] as i32),
        MipLODBias: f32::from_bits(k[4]),
        MaxAnisotropy: k[5],
        ComparisonFunc: D3D12_COMPARISON_FUNC(k[6] as i32),
        BorderColor: D3D12_STATIC_BORDER_COLOR(k[7] as i32),
        MinLOD: f32::from_bits(k[8]),
        MaxLOD: f32::from_bits(k[9]),
        ShaderRegister: k[10],
        RegisterSpace: k[11],
        ShaderVisibility: D3D12_SHADER_VISIBILITY(k[12] as i32),
    }
}

fn line(k: &Key) -> String {
    k.iter().map(|w| format!("{w:x}")).collect::<Vec<_>>().join(" ")
}

/// A line of the file, or nothing for one that is not thirteen words (a line
/// cut short when the process ended).
fn parse(line: &str) -> Option<Key> {
    let mut k = [0u32; 13];
    let mut words = line.split_ascii_whitespace();
    for w in &mut k {
        *w = u32::from_str_radix(words.next()?, 16).ok()?;
    }
    words.next().is_none().then_some(k)
}

/// The samplers in their kept order.
#[derive(Default)]
struct Listed {
    order: Vec<Key>,
    known: HashSet<Key>,
}

impl Listed {
    /// The lines of the file, each sampler once, at most [`MOST`].
    fn of(text: &str) -> Listed {
        let mut listed = Listed::default();
        for k in text.lines().filter_map(parse) {
            if listed.order.len() >= MOST {
                break;
            }
            listed.place(k);
        }
        listed
    }

    /// The place `k` takes at the end if it has none yet (and the list has
    /// room).
    fn place(&mut self, k: Key) -> Option<usize> {
        if self.order.len() >= MOST || !self.known.insert(k) {
            return None;
        }
        self.order.push(k);
        Some(self.order.len() - 1)
    }
}

/// A root signature kept so that its sampler keeps its place.
struct Kept(#[allow(dead_code)] ID3D12RootSignature);
// SAFETY: D3D12 objects are free-threaded; this one is only held.
unsafe impl Send for Kept {}

struct Order {
    listed: Listed,
    kept: Vec<Kept>,
    file: Option<PathBuf>,
    /// Samplers met with the list full: counted, not made early.
    over: u64,
}

static ORDER: Mutex<Option<Order>> = Mutex::new(None);

/// Samplers added to the list in this run.
pub(crate) static NEW: AtomicU64 = AtomicU64::new(0);

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SAMPLER_ORDER").is_none_or(|v| v != "0"))
}

/// A root signature that holds the one static sampler `k` and nothing else.
fn root_of(device: &ID3D12Device, k: &Key) -> windows::core::Result<ID3D12RootSignature> {
    let sampler = description(k);
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                NumStaticSamplers: 1,
                pStaticSamplers: &sampler,
                ..Default::default()
            },
        },
    };
    // SAFETY: the description points at `sampler`, alive through the call;
    // the blob is read in full before it is released.
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes)
    }
}

impl Order {
    /// The file's samplers, each made in its turn.
    fn begin(device: &ID3D12Device) -> Order {
        let began = std::time::Instant::now();
        let file = super::library::folder().map(|d| d.join(FILE));
        let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
        let mut order = Order { listed: Listed::of(&text), kept: Vec::new(), file, over: 0 };
        let mut refused = 0u32;
        for k in &order.listed.order {
            match root_of(device, k) {
                Ok(root) => order.kept.push(Kept(root)),
                Err(_) => refused += 1,
            }
        }
        tracing::info!(
            listed = order.listed.order.len(),
            refused,
            ms = began.elapsed().as_secs_f64() * 1e3,
            "static samplers made in their kept order before the first root signature with one (VICTORY_SAMPLER_ORDER=0: not)"
        );
        order
    }

    /// `k`, met for the first time: made, kept and written to the end of the
    /// file.
    fn add(&mut self, device: &ID3D12Device, k: Key) {
        let Some(place) = self.listed.place(k) else {
            if self.listed.order.len() >= MOST && !self.listed.known.contains(&k) {
                self.listed.known.insert(k);
                self.over += 1;
                if self.over == 1 {
                    tracing::warn!(most = MOST, "more static samplers than are listed: the rest take their places as they come");
                }
            }
            return;
        };
        match root_of(device, &k) {
            Ok(root) => self.kept.push(Kept(root)),
            Err(e) => tracing::warn!(error = %e, place, "a static sampler's own root signature was refused"),
        }
        if let Some(file) = &self.file {
            use std::io::Write;
            let written = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(file))
                .and_then(|mut f| writeln!(f, "{}", line(&k)));
            if let Err(e) = written {
                tracing::warn!(error = %e, "the list of static samplers was not written");
            }
        }
        NEW.fetch_add(1, Ordering::Relaxed);
        tracing::info!(place, sampler = %line(&k), "a static sampler met for the first time takes the next place");
    }
}

/// Called before a root signature with the static samplers `samplers` is
/// created: the listed ones have their places by then, and each of these
/// that is new takes the next one.
pub(crate) fn before(device: &ID3D12Device, samplers: &[D3D12_STATIC_SAMPLER_DESC]) {
    if samplers.is_empty() || !on() {
        return;
    }
    let mut order = ORDER.lock().unwrap_or_else(|e| e.into_inner());
    let order = order.get_or_insert_with(|| Order::begin(device));
    for s in samplers {
        let k = key(s);
        if !order.listed.known.contains(&k) {
            order.add(device, k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sampler(filter: i32, register: u32) -> D3D12_STATIC_SAMPLER_DESC {
        D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER(filter),
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
            MipLODBias: -0.75,
            MaxAnisotropy: 8,
            ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
            BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
            MinLOD: 0.5,
            MaxLOD: f32::MAX,
            ShaderRegister: register,
            RegisterSpace: 3,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        }
    }

    #[test]
    fn a_sampler_comes_back_from_its_line_whole() {
        let s = sampler(0x15, 4);
        let k = key(&s);
        assert_eq!(parse(&line(&k)), Some(k));
        let back = description(&k);
        assert_eq!(key(&back), k);
        assert_eq!(back.MipLODBias, -0.75);
        assert_eq!(back.MaxLOD, f32::MAX);
        assert_eq!(back.ShaderVisibility, D3D12_SHADER_VISIBILITY_PIXEL);
    }

    #[test]
    fn lines_cut_short_or_damaged_are_left_out() {
        let k = key(&sampler(0, 0));
        let whole = line(&k);
        assert_eq!(parse(&whole[..whole.len() - 3]).is_some_and(|p| p == k), false);
        assert_eq!(parse("1 2 3"), None);
        assert_eq!(parse(&format!("{whole} 7")), None);
        assert_eq!(parse("zz 0 0 0 0 0 0 0 0 0 0 0 0"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn places_are_the_order_of_first_meeting_and_never_change() {
        let (a, b, c) = (key(&sampler(0, 0)), key(&sampler(0x15, 0)), key(&sampler(0x15, 1)));
        let mut listed = Listed::default();
        assert_eq!(listed.place(a), Some(0));
        assert_eq!(listed.place(b), Some(1));
        assert_eq!(listed.place(a), None);
        // The same sampler at another register is one of its own.
        assert_eq!(listed.place(c), Some(2));
        let text: String = listed.order.iter().map(|k| line(k) + "\n").collect();
        // A later run: the file's order, a line twice and a cut line ignored.
        let again = Listed::of(&format!("{text}{}\n1 2\n", line(&a)));
        assert_eq!(again.order, [a, b, c]);
        let mut again = again;
        assert_eq!(again.place(b), None);
        assert_eq!(again.place(key(&sampler(1, 2))), Some(3));
    }

    /// On the software adapter: every sampler of the layer's kinds gets a
    /// root signature of its own (the call the driver gives places at), a
    /// comparison sampler and one visible to one stage among them.
    #[test]
    fn each_sampler_gets_a_root_signature_of_its_own() {
        let Some(device) = crate::inplace::tests::warp() else { return };
        let mut compare = sampler(0x95, 2);
        compare.ComparisonFunc = D3D12_COMPARISON_FUNC_GREATER;
        let mut all = sampler(0x55, 7);
        all.ShaderVisibility = D3D12_SHADER_VISIBILITY_ALL;
        for s in [sampler(0, 0), sampler(0x15, 1), compare, all] {
            let k = key(&s);
            assert!(root_of(&device, &k).is_ok(), "{}", line(&k));
        }
    }

    #[test]
    fn the_list_takes_no_more_than_its_most() {
        let mut listed = Listed::default();
        for n in 0..MOST as u32 + 5 {
            listed.place(key(&sampler(0, n)));
        }
        assert_eq!(listed.order.len(), MOST);
        let text: String = (0..MOST as u32 + 5).map(|n| line(&key(&sampler(0, n))) + "\n").collect();
        assert_eq!(Listed::of(&text).order.len(), MOST);
    }
}
