//! The replayer: makes a captured frame's objects again, puts their
//! contents back, and runs the frame as often as asked, with its time on
//! the card and experiments on it (`super::edit`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_12_0, D3D_PRIMITIVE_TOPOLOGY};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

use super::edit::{self, Clamp, RESULTS};
use super::gpu::{self, Seen, Worker};
use super::snap::read_sub;
use super::states::{
    self, COMMON, COPY_DEST, COPY_SOURCE, DEPTH_WRITE, RENDER_TARGET, RESOLVE_DEST, States,
    UNORDERED_ACCESS,
};
use super::wire::{
    self, ALL, Barrier, Cmd, Content, Delta, Frame, Loc, MISSING, NONE, Place, Slot, Va, ZERO, slot,
};

pub mod diverge;
pub mod look;
mod merge;
pub mod pixel;
mod upcopies;

/// The name of the argument clamp's pipeline, by which its passes are
/// recognised.
pub const CLAMP: &str = "victory: argument clamp";
/// The name of the pipeline that compares and hashes card buffers.
pub const COMPARE: &str = "victory: card compare";

/// An upload buffer contents come back through.
const STAGING: u64 = 256 << 20;

/// Where buffers the CPU saw lie in a replay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Buffers {
    /// Where they lay.
    #[default]
    AsRecorded,
    /// All in the card's memory.
    Card,
    /// All in the PC's memory, the card's own buffers too.
    Pc,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub loops: u32,
    /// Put back, before each run, what the run before changed.
    pub restore: bool,
    /// Time every command, not only lists.
    pub detailed: bool,
    /// Shaders listed.
    pub top: usize,
    /// Where the picture goes (default: `replay.png` in the capture).
    pub image: Option<PathBuf>,
    pub buffers: Buffers,
    /// Of those, only the ones all of these match ([`chosen`]).
    pub only_buffers: Vec<String>,
    pub no_barriers: bool,
    pub merge_barriers: bool,
    pub skip_clamps: bool,
    /// The clamps in batches (`edit::batch_clamps`); 2: carefully.
    pub batch_clamps: u32,
    pub skip_hash_passes: bool,
    pub one_submit: bool,
    /// Copies of what the CPU wrote leave alone what the card wrote in this
    /// run (`merge`).
    pub merge: bool,
    /// A full wait after every command that works.
    pub wait_all: bool,
    /// Each list sent on its own, and waited for.
    pub wait_lists: bool,
    /// Milliseconds the sender waits after each submission, as a title does
    /// that prepares the next one meanwhile: the card stands idle.
    pub pace: f64,
    /// The card kept awake meanwhile (`crate::host::awake`).
    pub awake: bool,
    /// Run only these lists (by their place in the frame).
    pub only: Option<(u32, u32)>,
    /// The last command that runs (list, command); after it only barriers.
    pub until: Option<(u32, usize)>,
    /// Skip what runs a pipeline whose name or shader hash has one of
    /// these in it.
    pub skip_shaders: Vec<String>,
    /// Shaders taken from this folder where it has them: `<hash>.dxil`, by
    /// the hash the runtime names the title's shader by (ours: by their own).
    pub shaders: Option<PathBuf>,
    /// With `compare`: the folder the shaders of the second are from.
    pub shaders_b: Option<PathBuf>,
    /// Run the frame unchanged first, and compare.
    pub compare: bool,
    /// The debug layer, if it is installed.
    pub debug: bool,
    /// The system's breadcrumbs after every command, as
    /// `VICTORY_D3D12_DRED=light` has them.
    pub breadcrumbs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            loops: 5,
            restore: true,
            detailed: false,
            top: 20,
            image: None,
            buffers: Buffers::AsRecorded,
            only_buffers: Vec::new(),
            no_barriers: false,
            merge_barriers: false,
            skip_clamps: false,
            batch_clamps: 0,
            skip_hash_passes: false,
            one_submit: false,
            merge: true,
            wait_all: false,
            wait_lists: false,
            pace: 0.0,
            awake: false,
            only: None,
            until: None,
            skip_shaders: Vec::new(),
            shaders: None,
            shaders_b: None,
            compare: false,
            debug: false,
            breadcrumbs: false,
        }
    }
}

impl Options {
    fn edited(&self) -> bool {
        self.no_barriers
            || self.merge_barriers
            || self.skip_clamps
            || self.batch_clamps > 0
            || self.skip_hash_passes
            || self.one_submit
            || self.wait_all
            || self.wait_lists
            || self.only.is_some()
            || !self.skip_shaders.is_empty()
    }

    /// The same without the experiments.
    fn plain(&self) -> Options {
        Options {
            no_barriers: false,
            merge_barriers: false,
            skip_clamps: false,
            batch_clamps: 0,
            skip_hash_passes: false,
            one_submit: false,
            wait_all: false,
            wait_lists: false,
            awake: false,
            only: None,
            skip_shaders: Vec::new(),
            ..self.clone()
        }
    }
}

/// How a run went.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Per loop: from the first list's start to the last one's end, in ms.
    pub frame_ms: Vec<f64>,
    /// Per loop: how long the lists of submissions ran.
    pub busy_ms: Vec<f64>,
    /// Per loop: copies of ours that stand for what the CPU wrote.
    pub delta_ms: Vec<f64>,
    /// Of the last loop, by submission: lists, ms.
    pub submits: Vec<(usize, f64)>,
    /// With detailed timing: ms (median of the loops) and count by category.
    pub categories: Vec<(String, f64, u32)>,
    /// With detailed timing: ms, count and name of the costliest pipelines.
    pub shaders: Vec<(f64, u32, String)>,
    /// Against the presented picture: dB (infinite: the same), and whether
    /// its bytes are the same.
    pub picture: Option<(f64, bool)>,
    /// Of each loop's picture: dB against the presented one, and its hash.
    pub pictures: Vec<(f64, String)>,
    /// Things to know: from the capture and the replay.
    pub notes: Vec<String>,
}

impl Report {
    pub fn median_ms(&self) -> f64 {
        median(&self.frame_ms)
    }

    /// The report as text.
    pub fn text(&self, what: &str) -> String {
        let mut t = String::new();
        let ms = |v: &[f64]| {
            v.iter()
                .map(|x| format!("{x:.2}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let _ = writeln!(
            t,
            "{what}: {:.2} ms on the card (median of {})",
            self.median_ms(),
            self.frame_ms.len()
        );
        let _ = writeln!(t, "  loops, ms: {}", ms(&self.frame_ms));
        let (busy, delta) = (median(&self.busy_ms), median(&self.delta_ms));
        let idle = (self.median_ms() - busy - delta).max(0.0);
        let _ = writeln!(
            t,
            "  submissions busy {busy:.2} ms, copies standing for CPU writes {delta:.2} ms, between submissions {idle:.2} ms"
        );
        let _ = writeln!(t, "  by submission (lists: ms): {}", {
            self.submits
                .iter()
                .enumerate()
                .map(|(i, (n, ms))| format!("{i} ({n}): {ms:.2}"))
                .collect::<Vec<_>>()
                .join(", ")
        });
        if !self.categories.is_empty() {
            let _ = writeln!(t, "  by kind of command, ms (commands):");
            for (name, ms, n) in &self.categories {
                let _ = writeln!(t, "    {ms:8.2}  {n:6}  {name}");
            }
        }
        if !self.shaders.is_empty() {
            let _ = writeln!(t, "  costliest pipelines, ms (commands):");
            for (ms, n, name) in &self.shaders {
                let _ = writeln!(t, "    {ms:8.2}  {n:6}  {name}");
            }
        }
        match self.picture {
            Some((_, true)) => _ = writeln!(t, "  picture: the same bytes as presented"),
            Some((db, false)) => {
                _ = writeln!(
                    t,
                    "  picture: differs from the presented one, PSNR {db:.1} dB"
                )
            }
            None => {}
        }
        if self.pictures.len() > 1 {
            let all: Vec<String> = self
                .pictures
                .iter()
                .map(|(db, hash)| format!("{db:.1} dB {}", &hash[..8]))
                .collect();
            let kinds: HashSet<&String> = self.pictures.iter().map(|p| &p.1).collect();
            let _ = writeln!(
                t,
                "  pictures of the loops: {} different: {}",
                kinds.len(),
                all.join(", ")
            );
        }
        for n in &self.notes {
            let _ = writeln!(t, "  note: {n}");
        }
        t
    }
}

/// Whether `--only-buffers` means a buffer: all of `only` match it.
/// `name=<text in its name>`, `min=<bytes>`, `max=<bytes>`,
/// `seen=<how the CPU saw it>`, `id=<a>..<b>`; with `!` in front: that does not.
fn chosen(r: &wire::Resource, only: &[String]) -> bool {
    only.iter().all(|f| {
        let (no, f) = match f.strip_prefix('!') {
            Some(f) => (true, f),
            None => (false, f.as_str()),
        };
        let (key, value) = f.split_once('=').unwrap_or(("name", f));
        let number = value.parse().unwrap_or(0u64);
        let is = match key {
            "min" => r.desc.width >= number,
            "max" => r.desc.width <= number,
            "seen" => {
                matches!(&r.place, Place::Committed { props, .. } if format!("{:?}", gpu::seen(props)).eq_ignore_ascii_case(value))
            }
            "id" => value.split_once("..").is_some_and(|(a, b)| {
                (a.parse().unwrap_or(0)..b.parse().unwrap_or(u32::MAX)).contains(&r.id)
            }),
            _ => r.name.contains(value),
        };
        is != no
    })
}

/// A command's category in a list of ours (one that runs no shader of the
/// title's: the host's state preludes, its copies of what the CPU sent up,
/// the card copies' passes), for the detailed timing's sums by owner.
fn of_ours(category: &'static str) -> &'static str {
    match category {
        "barriers" => "barriers (lists of ours)",
        "copies of buffers, PC to card" => "copies of buffers, PC to card (lists of ours)",
        "copies of buffers, card to card" => "copies of buffers, card to card (lists of ours)",
        "copies of buffers, card to PC" => "copies of buffers, card to PC (lists of ours)",
        "copies of buffers, PC to PC" => "copies of buffers, PC to PC (lists of ours)",
        "copies of textures, PC to card" => "copies of textures, PC to card (lists of ours)",
        "copies of textures, card to card" => "copies of textures, card to card (lists of ours)",
        "copies of textures, to the PC" => "copies of textures, to the PC (lists of ours)",
        "copies of whole resources" => "copies of whole resources (lists of ours)",
        "clears" => "clears (lists of ours)",
        "resolves" => "resolves (lists of ours)",
        other => other,
    }
}

fn median(v: &[f64]) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

/// The folder of a capture: `folder`, or the last `capture-<n>` in it that
/// is complete.
pub fn find(folder: &Path) -> Result<PathBuf> {
    if folder.join("frame.bin").exists() {
        return Ok(folder.to_owned());
    }
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in std::fs::read_dir(folder)
        .with_context(|| format!("{}", folder.display()))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(n) = name.strip_prefix("capture-").and_then(|n| n.parse().ok()) else {
            continue;
        };
        if entry.path().join("frame.bin").exists() && best.as_ref().is_none_or(|b| b.0 < n) {
            best = Some((n, entry.path()));
        }
    }
    best.map(|b| b.1)
        .ok_or_else(|| anyhow!("no capture (frame.bin) in {}", folder.display()))
}

/// The stage and hash by which the runtime names each of the title's shaders
/// in a capture.
pub fn title_shaders(folder: &Path) -> Result<std::collections::BTreeSet<(String, String)>> {
    let frame = Frame::read(&find(folder)?.join("frame.bin"))?;
    let mut all = std::collections::BTreeSet::new();
    for p in &frame.pipelines {
        for part in &p.parts {
            if let wire::Part::Code { blob, .. } = part
                && let Some(hash) = title_hash(p, *blob)
            {
                let at = p.name.find(hash).unwrap_or(0);
                let stage = p.name[..at].trim_end().rsplit(['[', ' ']).next().unwrap_or("");
                all.insert((stage.to_owned(), hash.to_owned()));
            }
        }
    }
    Ok(all)
}

/// The hash by which the runtime names the title's shader that code blob
/// `blob` of pipeline `p` is the translation of.
fn title_hash(p: &wire::Pipeline, blob: u32) -> Option<&str> {
    let kind = p.parts.iter().find_map(|part| match part {
        wire::Part::Code { kind, blob: b } if *b == blob => Some(*kind),
        _ => None,
    })?;
    let stage = match D3D12_PIPELINE_STATE_SUBOBJECT_TYPE(kind) {
        D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_VS => "vertex ",
        D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PS => "pixel ",
        D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS => "mesh ",
        D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CS => "compute ",
        _ => return None,
    };
    let at = p.name.find(stage)? + stage.len();
    p.name.get(at..at + 16)
}

/// Writes the DXIL of the pipelines whose name has `text` in it to `shaders`
/// in the capture, and the pipelines' descriptions; what was written.
pub fn dump(folder: &Path, text: &str) -> Result<Vec<String>> {
    let folder = find(folder)?;
    let frame = Frame::read(&folder.join("frame.bin"))?;
    let file = File::open(folder.join("shaders.bin"))?;
    let to = folder.join("shaders");
    std::fs::create_dir_all(&to)?;
    let mut out = Vec::new();
    for p in frame.pipelines.iter().filter(|p| p.name.contains(text)) {
        let mut line = format!("pipeline {} {}:", p.id, p.name);
        for part in &p.parts {
            match part {
                wire::Part::Code { kind, blob } => {
                    let c = &frame.codes[*blob as usize];
                    let mut bytes = vec![0; c.len as usize];
                    read_at(&file, &mut bytes, c.at)?;
                    let name: String = c.hash[..8].iter().map(|b| format!("{b:02x}")).collect();
                    std::fs::write(to.join(format!("{name}.dxil")), bytes)?;
                    let _ = write!(line, " {kind}={name}.dxil");
                }
                wire::Part::Root { root } => {
                    if let Some((_, blob)) = frame.roots.iter().find(|r| r.0 == *root) {
                        std::fs::write(to.join(format!("root-{root}.bin")), &blob.0)?;
                    }
                    let _ = write!(line, " root-{root}.bin");
                }
                other => {
                    let _ = write!(line, " {other:?}");
                }
            }
        }
        out.push(line);
    }
    Ok(out)
}

/// Writes, one line each, the commands of lists (by their place in the
/// frame), and before them the frame's buffers: `--listing`.
pub fn listing(folder: &Path, lists: (u32, u32), line: &mut dyn FnMut(&str)) -> Result<()> {
    let frame = Frame::read(&find(folder)?.join("frame.bin"))?;
    let pipelines: HashMap<u32, &str> =
        frame.pipelines.iter().map(|p| (p.id, p.name.as_str())).collect();
    for r in &frame.resources {
        if r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
            let place = match &r.place {
                Place::Committed { props, .. } => format!("{:?}", gpu::seen(props)),
                other => format!("{other:?}"),
            };
            line(&format!("buffer {} {} bytes, {place}: {}", r.id, r.desc.width, r.name));
        } else {
            let d = &r.desc;
            line(&format!(
                "texture {} {}x{}x{} mips {} f{}: {}",
                r.id, d.width, d.height, d.depth, d.mips, d.format, r.name
            ));
        }
    }
    let mut place = 0;
    for (i, s) in frame.submits.iter().enumerate() {
        for &l in s.lists.iter().filter(|&&l| l != MISSING) {
            let here = place;
            place += 1;
            if here < lists.0 || here > lists.1 {
                continue;
            }
            line(&format!("list {here} of submission {i}"));
            for (n, c) in Cmd::all(&frame.lists[l as usize].commands.0)?.iter().enumerate() {
                match c {
                    Cmd::Pipeline { pso } => {
                        line(&format!("{n} Pipeline {pso} {}", pipelines.get(pso).copied().unwrap_or_default()));
                    }
                    c => line(&format!("{n} {c:?}")),
                }
            }
        }
    }
    Ok(())
}

/// Loads a capture and runs it: the whole of `victory replay`. The reports,
/// with what each is of.
pub fn replay(folder: &Path, options: &Options) -> Result<Vec<(String, Report)>> {
    let mut player = Player::load(folder, options)?;
    let mut out = Vec::new();
    if options.compare && (options.edited() || options.shaders_b.is_some()) {
        // Turn by turn: what else the machine does falls on both.
        let image = Some(player.folder.join("replay-unchanged.png"));
        let plain = Options {
            image,
            ..options.plain()
        };
        let mut a = player.prepare(&plain)?;
        // The lists of the first hold its pipelines by name alone: kept.
        let _pipelines = match &options.shaders_b {
            Some(folder) => {
                let kept = std::mem::take(&mut player.pipelines);
                player.make_pipelines(Some(folder))?;
                kept
            }
            None => HashMap::new(),
        };
        let mut b = player.prepare(options)?;
        let (mut stamps_a, mut stamps_b) = (Vec::new(), Vec::new());
        for _ in 0..options.loops.max(1) {
            stamps_a.push(player.once(&plain, &mut a)?);
            stamps_b.push(player.once(options, &mut b)?);
        }
        out.push(("unchanged".into(), player.finish(&plain, a, &stamps_a)?));
        out.push(("with the experiments".into(), player.finish(options, b, &stamps_b)?));
        return Ok(out);
    }
    let what = if options.edited() {
        "with the experiments"
    } else {
        "unchanged"
    };
    out.push((what.into(), player.run(options)?));
    Ok(out)
}

struct Object {
    res: ID3D12Resource,
    va: u64,
    /// In the capture's `resources`.
    index: usize,
    subs: u32,
}

/// One version of a shader-visible descriptor heap.
struct Version {
    heap: ID3D12DescriptorHeap,
    gpu: u64,
}

pub struct Player {
    pub folder: PathBuf,
    frame: Frame,
    device: ID3D12Device,
    worker: Worker,
    info: Option<ID3D12InfoQueue>,
    frequency: f64,
    objects: HashMap<u32, Object>,
    _heaps: HashMap<u32, ID3D12Heap>,
    roots: HashMap<u32, ID3D12RootSignature>,
    pipelines: HashMap<u32, ID3D12PipelineState>,
    signatures: HashMap<u32, ID3D12CommandSignature>,
    queries: HashMap<u32, ID3D12QueryHeap>,
    /// Descriptor heaps by id, in their versions; each submission's version.
    descriptors: HashMap<u32, (Vec<Version>, u64)>,
    version_of: Vec<usize>,
    /// Render target and depth views by what they are of.
    targets: HashMap<Slot, D3D12_CPU_DESCRIPTOR_HANDLE>,
    _target_heaps: Vec<ID3D12DescriptorHeap>,
    /// Copies of what the frame changes, as at its start.
    masters: Vec<(u32, ID3D12Resource)>,
    /// What the CPU wrote between submissions, and for each submission
    /// where in it.
    deltas: Option<ID3D12Resource>,
    delta_parts: Vec<Vec<(Delta, u64)>>,
    /// States at the start of the frame and after it.
    start: HashMap<u32, Vec<u32>>,
    end: HashMap<u32, Vec<u32>>,
    /// For `Options::merge`.
    merger: Option<merge::Merger>,
    merging: bool,
    /// Resources the card may write in the frame.
    card_writes: HashSet<u32>,
    /// The results of clamp passes ([`RESULTS`]).
    results: Option<ID3D12Resource>,
    /// Other buffers of ours that commands name.
    extra: HashMap<u32, ID3D12Resource>,
    commands: Vec<Vec<Cmd>>,
    /// Resources in the states of the start (not those of the end).
    fresh: bool,
    /// States as the last run left them (`diverge`).
    at: States,
    notes: Vec<String>,
}

/// What a command of a list is timed as.
type Stamp = (usize, &'static str, u32);

/// With a stamp of a copy to a texture: this bit and the texture's number
/// stand for the pipeline's.
const COPY_TO: u32 = 1 << 31;

/// A list made from commands, with what its timestamps are of.
struct Built {
    _allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// The query of its start; its end is the one before the next list's.
    first: u32,
    last: u32,
    stamps: Vec<Stamp>,
}

impl Player {
    pub fn load(folder: &Path, options: &Options) -> Result<Player> {
        let folder = find(folder)?;
        let began = Instant::now();
        let frame = Frame::read(&folder.join("frame.bin"))?;
        let (device, info) = make_device(&frame, options.debug, options.breadcrumbs)?;
        // SAFETY: plain creation and queries.
        let (queue, frequency) = unsafe {
            let queue: ID3D12CommandQueue =
                device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })?;
            let frequency = queue.GetTimestampFrequency()? as f64;
            (queue, frequency)
        };
        let commands: Vec<Vec<Cmd>> = frame
            .lists
            .iter()
            .map(|l| Cmd::all(&l.commands.0))
            .collect::<std::result::Result<_, _>>()?;
        let mut player = Player {
            folder,
            worker: Worker::new(&device, &queue)?,
            device,
            info,
            frequency,
            objects: HashMap::new(),
            _heaps: HashMap::new(),
            roots: HashMap::new(),
            pipelines: HashMap::new(),
            signatures: HashMap::new(),
            queries: HashMap::new(),
            descriptors: HashMap::new(),
            version_of: Vec::new(),
            targets: HashMap::new(),
            _target_heaps: Vec::new(),
            masters: Vec::new(),
            deltas: None,
            delta_parts: Vec::new(),
            start: HashMap::new(),
            end: HashMap::new(),
            merger: None,
            merging: false,
            card_writes: HashSet::new(),
            results: None,
            extra: HashMap::new(),
            commands,
            fresh: true,
            at: States::default(),
            notes: frame
                .notes
                .iter()
                .map(|n| format!("recorder: {n}"))
                .collect(),
            frame,
        };
        player.check()?;
        let mut steps: Vec<(&str, std::time::Duration)> = Vec::new();
        let mut step = |what: &'static str, at: &mut Instant| {
            steps.push((what, at.elapsed()));
            *at = Instant::now();
        };
        let mut at = Instant::now();
        player.make_resources(options.buffers, &options.only_buffers)?;
        step("resources", &mut at);
        player.make_pipelines(options.shaders.as_deref())?;
        step("pipelines", &mut at);
        player.make_descriptors()?;
        step("descriptors", &mut at);
        player.load_contents()?;
        step("contents", &mut at);
        player.make_masters(options.buffers, options.restore)?;
        step("copies to restore from", &mut at);
        let steps: Vec<String> = steps
            .iter()
            .map(|(w, d)| format!("{w} {:.1} s", d.as_secs_f64()))
            .collect();
        if let Some([budget, used, pc]) = player.memory() {
            let over = if used + 256 > budget {
                ": over the budget, so the system moves memory to the PC's and the times are not the card's"
            } else {
                ""
            };
            player.notes.push(format!(
                "{used} MB of the card's memory used, budget {budget} MB; {pc} MB of the PC's{over}"
            ));
        }
        player.notes.push(format!(
            "loaded in {:.1} s: {}",
            began.elapsed().as_secs_f64(),
            steps.join(", ")
        ));
        Ok(player)
    }

    fn note(&mut self, n: String) {
        tracing::warn!("{n}");
        self.notes.push(n);
    }

    /// Says what of the frame is not there.
    fn check(&mut self) -> Result<()> {
        let missing = self
            .frame
            .submits
            .iter()
            .flat_map(|s| &s.lists)
            .filter(|&&l| l == MISSING)
            .count();
        if missing > 0 {
            self.note(format!(
                "{missing} lists of the frame were not recorded: the frame is incomplete"
            ));
        }
        if self.frame.submits.is_empty() {
            bail!("the capture has no submission");
        }
        let unknown = self
            .commands
            .iter()
            .flatten()
            .filter(|c| {
                matches!(
                    c,
                    Cmd::RootView {
                        va: Va::Unknown { .. },
                        ..
                    }
                )
            })
            .count();
        if unknown > 0 {
            self.note(format!(
                "{unknown} root views name memory in no known buffer: they are null in the replay"
            ));
        }
        Ok(())
    }

    fn resource(&self, id: u32) -> Option<&ID3D12Resource> {
        if id == RESULTS {
            return self.results.as_ref();
        }
        self.objects
            .get(&id)
            .map(|o| &o.res)
            .or_else(|| self.extra.get(&id))
    }

    fn va(&self, va: &Va) -> u64 {
        match va {
            Va::At { res, offset } => self.objects.get(res).map_or(0, |o| o.va + offset),
            _ => 0,
        }
    }

    fn make_resources(&mut self, buffers: Buffers, only: &[String]) -> Result<()> {
        // A heap of memory the CPU sees becomes one the card may write too:
        // what the CPU wrote is put back by the card.
        let props = |p: &wire::HeapProps, buffer: bool| -> D3D12_HEAP_PROPERTIES {
            let pc = |page| D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_CUSTOM,
                CPUPageProperty: page,
                MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
                ..Default::default()
            };
            let card = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            };
            match (gpu::seen(p), buffers) {
                (Seen::ReadBack, _) => gpu::to_props(p),
                (_, Buffers::Card) if buffer => card,
                (Seen::No, Buffers::Pc) if buffer => pc(D3D12_CPU_PAGE_PROPERTY_WRITE_BACK),
                (Seen::Combined, _) if p.kind > D3D12_HEAP_TYPE_CUSTOM.0 => card,
                (Seen::Combined, _) => pc(D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE),
                _ => gpu::to_props(p),
            }
        };
        // Flags a replay has no use for: sharing, display, not zeroed.
        const DROP: i32 = 0x1 | 0x8 | 0x20 | 0x1000;
        let used_by_buffers_only: HashSet<u32> = {
            let mut textures = HashSet::new();
            for r in &self.frame.resources {
                if let Place::Placed { heap, .. } = r.place
                    && r.desc.dimension != D3D12_RESOURCE_DIMENSION_BUFFER.0
                {
                    textures.insert(heap);
                }
            }
            self.frame
                .heaps
                .iter()
                .map(|h| h.id)
                .filter(|id| !textures.contains(id))
                .collect()
        };
        for h in &self.frame.heaps {
            let desc = D3D12_HEAP_DESC {
                SizeInBytes: h.size,
                Properties: props(&h.props, only.is_empty() && used_by_buffers_only.contains(&h.id)),
                Alignment: h.alignment,
                Flags: D3D12_HEAP_FLAGS(h.flags & !DROP),
            };
            let mut heap: Option<ID3D12Heap> = None;
            // SAFETY: plain creation.
            unsafe { self.device.CreateHeap(&desc, &mut heap) }
                .with_context(|| format!("heap {}", h.id))?;
            self._heaps.extend(heap.map(|heap| (h.id, heap)));
        }
        let mut failed = 0;
        let mut moves: BTreeMap<String, (u32, u64)> = BTreeMap::new();
        for (index, r) in self.frame.resources.iter().enumerate() {
            let is_buffer = r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0;
            let moved = is_buffer && chosen(r, only);
            if moved && let Place::Committed { props, .. } = &r.place {
                let seen = gpu::seen(props);
                let m = moves.entry(format!("{seen:?}")).or_insert((0u32, 0u64));
                *m = (m.0 + 1, m.1 + r.desc.width);
            }
            let mut desc = gpu::to_desc(&r.desc);
            // Heaps are made without sharing across adapters (DROP), so a
            // resource cannot ask for it: buffers placed in a heap the host
            // opened from title memory (renderer port 1) have the flag, and
            // were refused ("parameter incorrect"), and the frame's work on
            // them read nothing, or hung the card.
            desc.Flags &= !D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER;
            let mut made: Option<ID3D12Resource> = None;
            let clear: Option<D3D12_CLEAR_VALUE> = r.clear.map(|c| {
                // SAFETY: 20 bytes that were a clear value.
                unsafe { std::mem::transmute::<[u32; 5], D3D12_CLEAR_VALUE>(c) }
            });
            let clear = clear.as_ref().map(|c| c as *const _);
            // SAFETY: plain creation from a description the host took.
            let result = unsafe {
                match &r.place {
                    Place::Placed { heap, offset } => match self._heaps.get(heap) {
                        Some(h) => self.device.CreatePlacedResource(
                            h,
                            *offset,
                            &desc,
                            D3D12_RESOURCE_STATE_COMMON,
                            clear,
                            &mut made,
                        ),
                        None => Err(windows::core::Error::empty()),
                    },
                    Place::Committed { props: p, flags } => {
                        let p = props(p, moved);
                        let state = if p.Type == D3D12_HEAP_TYPE_READBACK {
                            D3D12_RESOURCE_STATE_COPY_DEST
                        } else {
                            D3D12_RESOURCE_STATE_COMMON
                        };
                        self.device.CreateCommittedResource(
                            &p,
                            D3D12_HEAP_FLAGS(flags & !DROP),
                            &desc,
                            state,
                            clear,
                            &mut made,
                        )
                    }
                    Place::BackBuffer {} => {
                        let p = D3D12_HEAP_PROPERTIES {
                            Type: D3D12_HEAP_TYPE_DEFAULT,
                            ..Default::default()
                        };
                        self.device.CreateCommittedResource(
                            &p,
                            D3D12_HEAP_FLAG_NONE,
                            &desc,
                            D3D12_RESOURCE_STATE_COMMON,
                            None,
                            &mut made,
                        )
                    }
                }
            };
            match (result, made) {
                (Ok(()), Some(res)) => {
                    // SAFETY: a live resource.
                    let va = if is_buffer {
                        unsafe { res.GetGPUVirtualAddress() }
                    } else {
                        0
                    };
                    self.objects.insert(
                        r.id,
                        Object {
                            res,
                            va,
                            index,
                            subs: gpu::subresources(&r.desc),
                        },
                    );
                }
                (e, _) => {
                    if failed == 0 {
                        tracing::warn!("resource {} not made: {e:?}: {r:?}", r.id);
                    }
                    failed += 1;
                }
            }
            if !is_buffer {
                self.start.insert(r.id, r.state.clone());
            }
        }
        if failed > 0 {
            self.note(format!("{failed} resources could not be made again"));
        }
        if buffers != Buffers::AsRecorded {
            self.notes.push(format!(
                "--buffers is of, by how the CPU saw them (buffers, bytes): {moves:?}"
            ));
        }
        Ok(())
    }

    fn make_pipelines(&mut self, shaders: Option<&Path>) -> Result<()> {
        for (id, blob) in &self.frame.roots {
            if self.roots.contains_key(id) {
                // (Made for another set of shaders: the root signatures are the same.)
                continue;
            }
            // SAFETY: a blob the host took.
            let root = unsafe { self.device.CreateRootSignature(0, &blob.0) }
                .with_context(|| format!("root signature {id}"))?;
            self.roots.insert(*id, root);
        }
        for s in &self.frame.signatures {
            if self.signatures.contains_key(&s.id) {
                continue;
            }
            let desc = D3D12_COMMAND_SIGNATURE_DESC {
                ByteStride: s.stride,
                NumArgumentDescs: s.args.len() as u32,
                pArgumentDescs: s.args.as_ptr().cast(),
                NodeMask: 0,
            };
            let mut made: Option<ID3D12CommandSignature> = None;
            // SAFETY: a description the host took.
            unsafe {
                self.device
                    .CreateCommandSignature(&desc, self.roots.get(&s.root), &mut made)?
            };
            self.signatures.extend(made.map(|m| (s.id, m)));
        }
        for q in &self.frame.query_heaps {
            if self.queries.contains_key(&q.id) {
                continue;
            }
            let desc = D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE(q.kind),
                Count: q.count,
                NodeMask: 0,
            };
            let mut made: Option<ID3D12QueryHeap> = None;
            // SAFETY: plain creation.
            unsafe { self.device.CreateQueryHeap(&desc, &mut made)? };
            self.queries.extend(made.map(|m| (q.id, m)));
        }
        let file = File::open(self.folder.join("shaders.bin"))?;
        let device: ID3D12Device2 = self.device.cast()?;
        let next = AtomicUsize::new(0);
        type Made = std::result::Result<ID3D12PipelineState, String>;
        let made: std::sync::Mutex<Vec<(u32, Made)>> = std::sync::Mutex::default();
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get() / 2)
            .max(1);
        let (frame, roots) = (&self.frame, &self.roots);
        let (swapped, kept) = (AtomicUsize::new(0), AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    while let Some(p) = frame.pipelines.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let recorded = |i: u32| {
                            let c = frame.codes.get(i as usize)?;
                            let mut bytes = vec![0; c.len as usize];
                            read_at(&file, &mut bytes, c.at).ok()?;
                            Some(bytes)
                        };
                        let root = |id: u32| roots.get(&id).cloned();
                        let taken = std::cell::Cell::new(false);
                        let mut m = super::pso::make(&device, &p.parts, root, |i| {
                            let other = shaders.and_then(|dir| {
                                let own = || frame.codes.get(i as usize).map(|c| c.hash[..8].iter().map(|b| format!("{b:02x}")).collect());
                                let hash = title_hash(p, i).map(str::to_owned).or_else(own)?;
                                std::fs::read(dir.join(format!("{hash}.dxil"))).ok()
                            });
                            taken.set(taken.get() || other.is_some());
                            other.or_else(|| recorded(i))
                        });
                        if taken.get() && m.is_err() {
                            // Not for the root signature or the other stage recorded.
                            kept.fetch_add(1, Ordering::Relaxed);
                            m = super::pso::make(&device, &p.parts, root, recorded);
                        } else if taken.get() {
                            swapped.fetch_add(1, Ordering::Relaxed);
                        }
                        made.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push((p.id, m));
                    }
                });
            }
        });
        let mut refused = Vec::new();
        for (id, m) in made.into_inner().unwrap_or_else(|e| e.into_inner()) {
            match m {
                Ok(p) => drop(self.pipelines.insert(id, p)),
                Err(e) => refused.push((id, e)),
            }
        }
        if shaders.is_some() {
            let n = format!(
                "{} of {} pipelines made with shaders of the folder given, {} refused with them (as recorded)",
                swapped.into_inner(),
                self.frame.pipelines.len(),
                kept.into_inner()
            );
            self.note(n);
        }
        if let Some((id, e)) = refused.first() {
            let n = format!(
                "{} pipelines could not be made again, first {}: {e}",
                refused.len(),
                self.pipeline_name(*id)
            );
            self.note(n);
        }
        Ok(())
    }

    /// A pipeline's name for reports: what the host layer called it and the
    /// hashes of its shaders.
    fn pipeline_name(&self, id: u32) -> String {
        let Some(p) = self.frame.pipelines.iter().find(|p| p.id == id) else {
            return format!("pipeline {id}");
        };
        let code: Vec<String> = self.hashes(p);
        format!(
            "{} (DXIL {})",
            if p.name.is_empty() {
                "unnamed"
            } else {
                &p.name
            },
            code.join(" ")
        )
    }

    fn hashes(&self, p: &wire::Pipeline) -> Vec<String> {
        p.parts
            .iter()
            .filter_map(|part| match part {
                wire::Part::Code { blob, .. } => {
                    let c = self.frame.codes.get(*blob as usize)?;
                    Some(c.hash[..8].iter().map(|b| format!("{b:02x}")).collect())
                }
                _ => None,
            })
            .collect()
    }

    /// Makes a view; one of a resource that is not there is null.
    ///
    /// # Safety
    /// `at` is a slot of a heap of the view's type.
    unsafe fn view(&self, s: &Slot, at: D3D12_CPU_DESCRIPTOR_HANDLE, sampler: bool) {
        let gone = s.res != NONE && !self.objects.contains_key(&s.res);
        let res = self.objects.get(&s.res).map(|o| &o.res);
        let counter = self.objects.get(&s.counter).map(|o| &o.res);
        // The descriptions lie in the slot unaligned.
        // SAFETY: the slot's description is of its kind; the resources live.
        unsafe {
            match (s.kind, s.described != 0 && !gone) {
                (slot::CBV, described) => {
                    let mut d: D3D12_CONSTANT_BUFFER_VIEW_DESC = Default::default();
                    if described && let Some(o) = self.objects.get(&s.res) {
                        d = s
                            .desc
                            .as_ptr()
                            .cast::<D3D12_CONSTANT_BUFFER_VIEW_DESC>()
                            .read_unaligned();
                        d.BufferLocation = o.va + s.address();
                    }
                    self.device.CreateConstantBufferView(Some(&d), at);
                }
                (slot::SRV, true) => {
                    let d = s
                        .desc
                        .as_ptr()
                        .cast::<D3D12_SHADER_RESOURCE_VIEW_DESC>()
                        .read_unaligned();
                    self.device.CreateShaderResourceView(res, Some(&d), at);
                }
                (slot::SRV, false) if res.is_some() => {
                    self.device.CreateShaderResourceView(res, None, at)
                }
                (slot::UAV, true) => {
                    let d = s
                        .desc
                        .as_ptr()
                        .cast::<D3D12_UNORDERED_ACCESS_VIEW_DESC>()
                        .read_unaligned();
                    self.device
                        .CreateUnorderedAccessView(res, counter, Some(&d), at);
                }
                (slot::UAV, false) if res.is_some() => {
                    self.device
                        .CreateUnorderedAccessView(res, None::<&ID3D12Resource>, None, at)
                }
                (slot::SAMPLER, true) => {
                    let d = s
                        .desc
                        .as_ptr()
                        .cast::<D3D12_SAMPLER_DESC>()
                        .read_unaligned();
                    self.device.CreateSampler(&d, at);
                }
                _ if sampler => {
                    let d = D3D12_SAMPLER_DESC {
                        Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
                        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                        ..Default::default()
                    };
                    self.device.CreateSampler(&d, at);
                }
                (slot::UAV, _) => {
                    let d = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                        Format: DXGI_FORMAT(41),
                        ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
                        ..Default::default()
                    };
                    self.device.CreateUnorderedAccessView(
                        None::<&ID3D12Resource>,
                        None::<&ID3D12Resource>,
                        Some(&d),
                        at,
                    );
                }
                _ => {
                    let d = D3D12_SHADER_RESOURCE_VIEW_DESC {
                        Format: DXGI_FORMAT(41),
                        ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                        ..Default::default()
                    };
                    self.device
                        .CreateShaderResourceView(None::<&ID3D12Resource>, Some(&d), at);
                }
            }
        }
    }

    fn make_descriptors(&mut self) -> Result<()> {
        // A submission whose views replace ones that were in use gets a
        // version of the heaps of its own: lists are sent without waiting,
        // so a heap cannot be written between them.
        let mut now: HashMap<(u32, u32), Slot> = HashMap::new();
        let replaced: HashSet<(u32, u32)> = self
            .frame
            .submits
            .iter()
            .flat_map(|s| s.views.iter().map(|v| (v.0, v.1)))
            .collect();
        for h in &self.frame.descriptor_heaps {
            for (id, i) in replaced.iter().filter(|k| k.0 == h.id) {
                let k = h.runs.partition_point(|run| run.0 <= *i);
                if let Some((first, n, s)) = k.checked_sub(1).and_then(|k| h.runs.get(k))
                    && i - first < *n
                {
                    now.insert((*id, *i), *s);
                }
            }
        }
        // What each version changes against the one before.
        let mut versions: Vec<Vec<(u32, u32, Slot)>> = vec![Vec::new()];
        let mut touched: HashSet<(u32, u32)> = HashSet::new();
        for s in &self.frame.submits {
            let live =
                |old: Option<&Slot>| old.is_some_and(|o| o.kind == slot::SAMPLER || o.res != NONE);
            let news: Vec<&(u32, u32, Slot)> = s
                .views
                .iter()
                .filter(|(h, i, v)| now.get(&(*h, *i)) != Some(v))
                .collect();
            if news
                .iter()
                .any(|(h, i, _)| live(now.get(&(*h, *i))) || touched.contains(&(*h, *i)))
            {
                versions.push(Vec::new());
                touched.clear();
            }
            for (h, i, v) in news {
                now.insert((*h, *i), *v);
                touched.insert((*h, *i));
                if let Some(last) = versions.last_mut() {
                    last.push((*h, *i, *v));
                }
            }
            self.version_of.push(versions.len() - 1);
        }
        let mut made = HashMap::new();
        let mut full = 0;
        for h in &self.frame.descriptor_heaps {
            let sampler = h.kind == D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER.0;
            let kind = D3D12_DESCRIPTOR_HEAP_TYPE(h.kind);
            let make = |flags| -> windows::core::Result<ID3D12DescriptorHeap> {
                let d = D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: kind,
                    NumDescriptors: h.count,
                    Flags: flags,
                    NodeMask: 0,
                };
                // SAFETY: plain creation.
                unsafe { self.device.CreateDescriptorHeap(&d) }
            };
            let cpu = make(D3D12_DESCRIPTOR_HEAP_FLAG_NONE)?;
            // SAFETY: slots of the heap just made; threads write other ones.
            unsafe {
                let step = self.device.GetDescriptorHandleIncrementSize(kind) as usize;
                let start = cpu.GetCPUDescriptorHandleForHeapStart();
                let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: start.ptr + i as usize * step,
                };
                self.view(&Slot::EMPTY, start, sampler);
                let mut filled = 1;
                while filled < h.count {
                    let n = filled.min(h.count - filled).min(1 << 16);
                    // Sources and destinations do not overlap.
                    self.device
                        .CopyDescriptorsSimple(n, at(filled), start, kind);
                    filled += n;
                }
                for (first, n, s) in &h.runs {
                    self.view(s, at(*first), sampler);
                    for i in 1..*n {
                        self.device
                            .CopyDescriptorsSimple(1, at(first + i), at(*first), kind);
                    }
                }
                let mut all = Vec::new();
                for changes in &versions {
                    let mine: Vec<&(u32, u32, Slot)> =
                        changes.iter().filter(|(id, _, _)| *id == h.id).collect();
                    for (_, i, s) in &mine {
                        if *i < h.count {
                            self.view(s, at(*i), sampler);
                        }
                    }
                    if mine.is_empty()
                        && let Some(Version { heap, gpu }) = all.last()
                    {
                        let same = Version {
                            heap: ID3D12DescriptorHeap::clone(heap),
                            gpu: *gpu,
                        };
                        all.push(same);
                        continue;
                    }
                    let heap = match make(D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE) {
                        Ok(heap) => heap,
                        // Room for no more of them: the changes go into the
                        // last one, and lists before see them too.
                        Err(_) if !all.is_empty() => {
                            full += 1;
                            let Some(Version { heap, gpu }) = all.last() else {
                                break;
                            };
                            let same = Version {
                                heap: ID3D12DescriptorHeap::clone(heap),
                                gpu: *gpu,
                            };
                            self.device.CopyDescriptorsSimple(
                                h.count,
                                same.heap.GetCPUDescriptorHandleForHeapStart(),
                                start,
                                kind,
                            );
                            all.push(same);
                            continue;
                        }
                        Err(e) => return Err(e.into()),
                    };
                    self.device.CopyDescriptorsSimple(
                        h.count,
                        heap.GetCPUDescriptorHandleForHeapStart(),
                        start,
                        kind,
                    );
                    let gpu = heap.GetGPUDescriptorHandleForHeapStart().ptr;
                    all.push(Version { heap, gpu });
                }
                made.insert(h.id, (all, step as u64));
            }
        }
        self.descriptors = made;
        if versions.len() > 1 {
            self.notes.push(format!(
                "{} versions of the descriptor heaps (views replaced during the frame)",
                versions.len()
            ));
        }
        if full > 0 {
            self.note(format!(
                "{full} versions of descriptor heaps had no room: earlier lists see later views"
            ));
        }
        // Render target and depth views.
        let mut views: [HashSet<Slot>; 2] = Default::default();
        for c in self.commands.iter().flatten() {
            match c {
                Cmd::Targets { colors, depth } => {
                    views[0].extend(colors);
                    views[1].extend(depth);
                }
                Cmd::ClearTarget { view, .. } => _ = views[0].insert(*view),
                Cmd::ClearDepth { view, .. } => _ = views[1].insert(*view),
                _ => {}
            }
        }
        for (i, all) in views.iter().enumerate() {
            if all.is_empty() {
                continue;
            }
            let kind = [
                D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
            ][i];
            let d = D3D12_DESCRIPTOR_HEAP_DESC {
                Type: kind,
                NumDescriptors: all.len() as u32,
                ..Default::default()
            };
            // SAFETY: plain creation; slots of the heap just made.
            unsafe {
                let heap: ID3D12DescriptorHeap = self.device.CreateDescriptorHeap(&d)?;
                let step = self.device.GetDescriptorHandleIncrementSize(kind) as usize;
                let mut at = heap.GetCPUDescriptorHandleForHeapStart();
                for s in all {
                    let res = self.objects.get(&s.res).map(|o| &o.res);
                    if i == 0 {
                        let d = s
                            .desc
                            .as_ptr()
                            .cast::<D3D12_RENDER_TARGET_VIEW_DESC>()
                            .read_unaligned();
                        // Without either there is nothing to make a view of.
                        let null = D3D12_RENDER_TARGET_VIEW_DESC {
                            Format: DXGI_FORMAT(28),
                            ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                            ..Default::default()
                        };
                        let d = if s.described != 0 {
                            Some(&d as *const _)
                        } else if res.is_none() {
                            Some(&null as *const _)
                        } else {
                            None
                        };
                        self.device.CreateRenderTargetView(res, d, at);
                    } else {
                        let d = s
                            .desc
                            .as_ptr()
                            .cast::<D3D12_DEPTH_STENCIL_VIEW_DESC>()
                            .read_unaligned();
                        let null = D3D12_DEPTH_STENCIL_VIEW_DESC {
                            Format: DXGI_FORMAT(40),
                            ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
                            ..Default::default()
                        };
                        let d = if s.described != 0 {
                            Some(&d as *const _)
                        } else if res.is_none() {
                            Some(&null as *const _)
                        } else {
                            None
                        };
                        self.device.CreateDepthStencilView(res, d, at);
                    }
                    self.targets.insert(*s, at);
                    at.ptr += step;
                }
                self._target_heaps.push(heap);
            }
        }
        Ok(())
    }

    /// Reads parts of files into upload memory and has `copy` record what
    /// is done with each.
    fn upload<T: Sync>(
        &mut self,
        parts: &[(T, u64, u64)],
        file: &File,
        mut copy: impl FnMut(&Player, &ID3D12GraphicsCommandList, &[(&T, u64, u64)], &ID3D12Resource),
    ) -> Result<()> {
        // Batches of parts with their place in the buffer.
        let mut batches: Vec<Vec<(usize, u64)>> = vec![Vec::new()];
        let (mut at, mut most) = (0u64, 0u64);
        for (i, (_, len, _)) in parts.iter().enumerate() {
            at = at.next_multiple_of(512);
            if at + len > STAGING && at > 0 {
                batches.push(Vec::new());
                at = 0;
            }
            if let Some(b) = batches.last_mut() {
                b.push((i, at));
            }
            at += len;
            most = most.max(at);
        }
        if parts.is_empty() {
            return Ok(());
        }
        let mut stagings: Vec<(ID3D12Resource, usize, u64)> = Vec::new();
        for _ in 0..batches.len().min(2) {
            let (b, data) = gpu::buffer(
                &self.device,
                D3D12_HEAP_TYPE_UPLOAD,
                most,
                D3D12_RESOURCE_FLAG_NONE,
            )?;
            stagings.push((b, data as usize, 0));
        }
        for (n, batch) in batches.iter().enumerate() {
            let k = n % stagings.len();
            self.worker.wait(stagings[k].2)?;
            let map = stagings[k].1;
            let next = AtomicUsize::new(0);
            let failed: std::sync::Mutex<Option<std::io::Error>> = std::sync::Mutex::default();
            std::thread::scope(|scope| {
                for _ in 0..8.min(batch.len()) {
                    scope.spawn(|| {
                        while let Some(&(i, at)) = batch.get(next.fetch_add(1, Ordering::Relaxed)) {
                            let (_, len, from) = &parts[i];
                            // SAFETY: this part's own bytes of the mapping.
                            let to = unsafe {
                                std::slice::from_raw_parts_mut(
                                    (map + at as usize) as *mut u8,
                                    *len as usize,
                                )
                            };
                            if *from == ZERO {
                                to.fill(0);
                            } else if let Err(e) = read_at(file, to, *from) {
                                failed
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .get_or_insert(e);
                            }
                        }
                    });
                }
            });
            if let Some(e) = failed.into_inner().unwrap_or_else(|e| e.into_inner()) {
                return Err(e).context("the capture's contents");
            }
            let list: Vec<(&T, u64, u64)> = batch
                .iter()
                .map(|&(i, at)| (&parts[i].0, parts[i].1, at))
                .collect();
            let buffer = stagings[k].0.clone();
            // The worker is another part of the player than `copy` uses.
            let other = Worker::new(&self.device, &self.worker.queue)?;
            let mut worker = std::mem::replace(&mut self.worker, other);
            let sent = worker.send(|l| copy(self, l, &list, &buffer));
            self.worker = worker;
            stagings[k].2 = sent?;
        }
        self.worker.idle()?;
        Ok(())
    }

    fn load_contents(&mut self) -> Result<()> {
        // Idle aliases first: a change of their state may write memory the
        // one in use holds.
        let known: HashSet<u32> = self.frame.contents.iter().map(|c| c.res).collect();
        let mut early = Vec::new();
        let mut into = Vec::new();
        for r in &self.frame.resources {
            let (Some(o), Some(state)) = (self.objects.get(&r.id), self.start.get(&r.id)) else {
                continue;
            };
            let loaded = !r.idle && known.contains(&r.id);
            gpu::moves(
                &o.res,
                &[COMMON],
                if loaded { &[COPY_DEST] } else { state },
                if loaded { &mut into } else { &mut early },
            );
        }
        let textures: Vec<(Content, u64, u64)> = self
            .frame
            .contents
            .iter()
            .filter(|c| self.objects.contains_key(&c.res))
            .map(|c| (c.clone(), c.len, c.file))
            .collect();
        let missing = self.frame.contents.len() - textures.len();
        self.worker.run(|list| {
            // SAFETY: an open list; live resources.
            unsafe {
                for all in early.chunks(1000) {
                    list.ResourceBarrier(all);
                }
                list.ResourceBarrier(&[alias()]);
                for all in into.chunks(1000) {
                    list.ResourceBarrier(all);
                }
            }
        })?;
        let file = File::open(self.folder.join("contents.bin"))?;
        self.upload(&textures, &file, |player, list, parts, staging| {
            for (c, len, at) in parts {
                let Some(o) = player.objects.get(&c.res) else {
                    continue;
                };
                let r = &player.frame.resources[o.index];
                // SAFETY: an open list; live resources.
                unsafe {
                    if r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
                        list.CopyBufferRegion(&o.res, c.first, staging, *at, *len);
                        continue;
                    }
                    let (all, _) = gpu::footprints(
                        &player.device,
                        &gpu::to_desc(&r.desc),
                        c.first as u32,
                        c.count,
                    );
                    for (i, mut f) in all.into_iter().enumerate() {
                        f.Offset += at;
                        let to = gpu::sub_location(&o.res, c.first as u32 + i as u32);
                        list.CopyTextureRegion(
                            &to,
                            0,
                            0,
                            0,
                            &gpu::placed_location(staging, f),
                            None,
                        );
                    }
                }
            }
        })?;
        // To the states of the frame's start.
        let mut back = Vec::new();
        for r in &self.frame.resources {
            let (Some(o), Some(state)) = (self.objects.get(&r.id), self.start.get(&r.id)) else {
                continue;
            };
            if !r.idle && known.contains(&r.id) {
                gpu::moves(&o.res, &[COPY_DEST], state, &mut back);
            }
        }
        self.worker.run(|list| {
            for all in back.chunks(1000) {
                // SAFETY: an open list; live resources.
                unsafe { list.ResourceBarrier(all) };
            }
        })?;
        if missing > 0 {
            self.note(format!(
                "contents of {missing} parts of resources that could not be made are left out"
            ));
        }
        // What the CPU had put into the memory both see by the first
        // submission: part of the start of the frame.
        let file = File::open(self.folder.join("deltas.bin"))?;
        let named = self.named_buffers();
        let of = |s: &wire::Submit| {
            s.deltas
                .iter()
                .filter(|d| named.contains(&d.res))
                .cloned()
                .collect::<Vec<Delta>>()
        };
        // In memory the card cannot write, bytes looked at first when a
        // later submission read them can stand there from the start.
        let upload: HashSet<u32> = self
            .frame
            .resources
            .iter()
            .filter(|r| matches!(&r.place, Place::Committed { props, .. } if props.kind == D3D12_HEAP_TYPE_UPLOAD.0))
            .map(|r| r.id)
            .collect();
        let mut first = of(&self.frame.submits[0]);
        let mut seen: HashSet<(u32, u64)> = HashSet::new();
        let pages = |d: &Delta| {
            (d.at / 4096..(d.at + d.len).div_ceil(4096))
                .map(|p| (d.res, p))
                .collect::<Vec<_>>()
        };
        first.iter().for_each(|d| seen.extend(pages(d)));
        let mut rest: Vec<Vec<Delta>> = vec![Vec::new()];
        for s in self.frame.submits.iter().skip(1) {
            let (early, late): (Vec<Delta>, Vec<Delta>) = of(s).into_iter().partition(|d| {
                upload.contains(&d.res) && pages(d).iter().all(|p| !seen.contains(p))
            });
            early.iter().for_each(|d| seen.extend(pages(d)));
            first.extend(early);
            rest.push(late);
        }
        let first: Vec<(Delta, u64, u64)> = first
            .into_iter()
            .map(|d| (d.clone(), d.len, d.file))
            .collect();
        self.upload(&first, &file, |player, list, parts, staging| {
            for (d, len, at) in parts {
                if let Some(o) = player.objects.get(&d.res) {
                    // SAFETY: an open list; live buffers.
                    unsafe { list.CopyBufferRegion(&o.res, d.at, staging, *at, *len) };
                }
            }
        })?;
        // The later ones wait in a buffer of their own.
        let mut later: Vec<((u64, Delta), u64, u64)> = Vec::new();
        let mut total = 0u64;
        self.delta_parts = vec![Vec::new(); self.frame.submits.len()];
        for (i, all) in rest.into_iter().enumerate() {
            for d in all {
                later.push(((total, d.clone()), d.len, d.file));
                total += d.len;
                self.delta_parts[i].push((d, total - later.last().map_or(0, |l| l.1)));
            }
        }
        if total > 0 {
            let (buffer, _) = gpu::buffer(
                &self.device,
                D3D12_HEAP_TYPE_DEFAULT,
                total,
                D3D12_RESOURCE_FLAG_NONE,
            )?;
            self.deltas = Some(buffer.clone());
            self.upload(&later, &file, |_, list, parts, staging| {
                for ((to, _), len, at) in parts {
                    // SAFETY: an open list; live buffers.
                    unsafe { list.CopyBufferRegion(&buffer, *to, staging, *at, *len) };
                }
            })?;
        }
        Ok(())
    }

    /// The buffers the frame names, in commands or descriptors: what the CPU
    /// writes to others (buffers it has results fetched to) is of no use.
    fn named_buffers(&self) -> HashSet<u32> {
        let mut named = HashSet::new();
        let mut va = |va: &Va| {
            if let Va::At { res, .. } = va {
                named.insert(*res);
            }
        };
        for c in self.commands.iter().flatten() {
            match c {
                Cmd::RootView { va: v, .. } => va(v),
                Cmd::IndexBuffer {
                    view: Some((v, _, _)),
                } => va(v),
                Cmd::VertexBuffers {
                    views: Some(all), ..
                } => all.iter().for_each(|(v, _, _)| va(v)),
                Cmd::WriteImmediate { all } => all.iter().for_each(|(v, _, _)| va(v)),
                _ => {}
            }
        }
        for c in self.commands.iter().flatten() {
            match c {
                Cmd::CopyBuffer { dst, src, .. } | Cmd::CopyResource { dst, src } => {
                    named.extend([*dst, *src])
                }
                Cmd::CopyTexture { dst, src, .. } => {
                    for Loc::Sub { res, .. } | Loc::Placed { res, .. } in [dst, src] {
                        named.insert(*res);
                    }
                }
                Cmd::Indirect { args, count, .. } => named.extend([*args, *count]),
                Cmd::ResolveQuery { dst, .. } => _ = named.insert(*dst),
                _ => {}
            }
        }
        for h in &self.frame.descriptor_heaps {
            named.extend(h.runs.iter().flat_map(|(_, _, s)| [s.res, s.counter]));
        }
        for s in &self.frame.submits {
            named.extend(s.views.iter().flat_map(|(_, _, s)| [s.res, s.counter]));
        }
        named
    }

    /// Copies of what the frame changes; and the states it leaves.
    fn make_masters(&mut self, buffers: Buffers, wanted: bool) -> Result<()> {
        let mut states = States::default();
        for r in &self.frame.resources {
            if r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
                continue;
            }
            let subs = gpu::subresources(&r.desc);
            let decays = r.desc.flags & D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS.0 != 0;
            states.add(r.id, subs, r.state[0], decays);
            if r.state.len() > 1 {
                for (i, s) in r.state.iter().enumerate() {
                    states.change(&states::Change {
                        res: r.id,
                        sub: i as u32,
                        after: *s,
                        promoted: false,
                        alias: false,
                    });
                }
            }
        }
        // Resources in a state they are written in at some time, or written
        // some other way.
        const WRITE: u32 =
            RENDER_TARGET | UNORDERED_ACCESS | DEPTH_WRITE | COPY_DEST | RESOLVE_DEST;
        let mut written: HashSet<u32> = self
            .frame
            .resources
            .iter()
            .filter(|r| r.state.iter().any(|s| s & WRITE != 0))
            .map(|r| r.id)
            .collect();
        let mut by_cpu: HashSet<u32> = HashSet::new();
        for (i, s) in self.frame.submits.iter().enumerate() {
            for &l in s.lists.iter().filter(|&&l| l != MISSING) {
                for c in &self.commands[l as usize] {
                    let mut changes = Vec::new();
                    states::changes(c, &mut changes);
                    for ch in &changes {
                        states.change(ch);
                    }
                    match c {
                        Cmd::Barriers { all } => {
                            for b in all {
                                if let Barrier::Transition {
                                    res, before, after, ..
                                } = b
                                    && (*before | *after) as u32 & WRITE != 0
                                {
                                    written.insert(*res);
                                }
                            }
                        }
                        Cmd::CopyBuffer { dst, .. }
                        | Cmd::CopyResource { dst, .. }
                        | Cmd::ResolveQuery { dst, .. } => _ = written.insert(*dst),
                        Cmd::CopyTexture {
                            dst: Loc::Sub { res, .. } | Loc::Placed { res, .. },
                            ..
                        } => _ = written.insert(*res),
                        Cmd::Resolve { dst, .. } | Cmd::ResolveRegion { dst, .. } => {
                            _ = written.insert(*dst)
                        }
                        Cmd::RootView {
                            kind: slot::UAV,
                            va: Va::At { res, .. },
                            ..
                        } => _ = written.insert(*res),
                        Cmd::WriteImmediate { all } => {
                            written.extend(all.iter().filter_map(|(va, _, _)| {
                                if let Va::At { res, .. } = va {
                                    Some(*res)
                                } else {
                                    None
                                }
                            }));
                        }
                        _ => {}
                    }
                }
            }
            states.decay();
            if i > 0 {
                by_cpu.extend(self.delta_parts[i].iter().map(|(d, _)| d.res));
            }
        }
        // Buffers any unordered-access view names.
        let is_buffer = |id: u32| {
            self.objects.get(&id).is_some_and(|o| {
                self.frame.resources[o.index].desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0
            })
        };
        let simultaneous = |id: u32| {
            self.objects.get(&id).is_some_and(|o| {
                self.frame.resources[o.index].desc.flags
                    & D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS.0
                    != 0
            })
        };
        let views = self
            .frame
            .descriptor_heaps
            .iter()
            .flat_map(|h| h.runs.iter().map(|(_, _, s)| s));
        let late = self
            .frame
            .submits
            .iter()
            .flat_map(|s| s.views.iter().map(|(_, _, s)| s));
        for s in views.chain(late).filter(|s| s.kind == slot::UAV) {
            for id in [s.res, s.counter] {
                if is_buffer(id) || simultaneous(id) {
                    written.insert(id);
                }
            }
        }
        self.card_writes = written.clone();
        written.extend(by_cpu);
        for id in self.start.keys() {
            self.end
                .insert(*id, states.get(*id).unwrap_or(vec![COMMON]));
        }
        let mut ids: Vec<u32> = written
            .into_iter()
            .filter(|id| {
                self.objects.get(id).is_some_and(|o| {
                    let r = &self.frame.resources[o.index];
                    let back = matches!(&r.place, Place::Committed { props, .. } if gpu::seen(props) == Seen::ReadBack);
                    !r.idle && !back && !matches!(r.place, Place::BackBuffer {})
                })
            })
            .collect();
        ids.sort_unstable();
        // Without restoring they are of no use, and their room may be what
        // the frame itself lacks.
        if !wanted {
            ids.clear();
        }
        let mut total = 0u64;
        let mut in_pc = 0u64;
        for id in ids {
            let o = &self.objects[&id];
            let r = &self.frame.resources[o.index];
            let mut desc = gpu::to_desc(&r.desc);
            desc.Alignment = 0;
            // Copies are only copied to and from; a depth format needs its
            // flag.
            if r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
                desc.Flags = D3D12_RESOURCE_FLAG_NONE;
            }
            // SAFETY: a plain query.
            let size = unsafe { self.device.GetResourceAllocationInfo(0, &[desc]) }.SizeInBytes;
            let room = self.room();
            let buffer = r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0;
            // With all buffers in the card's memory there is no room for
            // their copies as well.
            let pc = buffer
                && (buffers == Buffers::Card || room.is_some_and(|room| room < size + (1 << 30)));
            let props = if pc {
                D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_CUSTOM,
                    CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE,
                    MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
                    ..Default::default()
                }
            } else {
                D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                }
            };
            let mut made: Option<ID3D12Resource> = None;
            // SAFETY: plain creation.
            let result = unsafe {
                self.device.CreateCommittedResource(
                    &props,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut made,
                )
            };
            match (result, made) {
                (Ok(()), Some(m)) => {
                    total += size;
                    in_pc += if pc { size } else { 0 };
                    self.masters.push((id, m));
                }
                (e, _) => self.note(format!(
                    "no copy of resource {id} to restore it from ({e:?})"
                )),
            }
        }
        tracing::info!(
            resources = self.masters.len(),
            mb = total >> 20,
            in_pc_memory_mb = in_pc >> 20,
            "copies of what the frame changes"
        );
        // Filled now: the resources are as at the frame's start. Masters
        // stay in the copy-source state (buffers decay).
        let (masters, objects, start) = (&self.masters, &self.objects, &self.start);
        for some in masters.chunks(256) {
            let (mut before, mut after) = (Vec::new(), Vec::new());
            for (id, m) in some {
                let Some(state) = start.get(id) else { continue };
                let o = &objects[id];
                let to: Vec<u32> = state
                    .iter()
                    .map(|&s| if s & COPY_SOURCE != 0 { s } else { COPY_SOURCE })
                    .collect();
                gpu::moves(&o.res, state, &to, &mut before);
                gpu::moves(&o.res, &to, state, &mut after);
                before.push(gpu::transition(m, ALL, COMMON, COPY_DEST));
                after.push(gpu::transition(m, ALL, COPY_DEST, COPY_SOURCE));
            }
            self.worker.run(|list| {
                // SAFETY: an open list; live resources of one size.
                unsafe {
                    barriers(list, &before);
                    for (id, m) in some {
                        list.CopyResource(m, &objects[id].res);
                    }
                    barriers(list, &after);
                }
            })?;
        }
        Ok(())
    }

    /// The card's memory budget and use, and the use of the PC's memory, in
    /// MB.
    fn memory(&self) -> Option<[u64; 3]> {
        let mut own = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        let mut pc = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: plain queries.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
            let adapter: IDXGIAdapter3 = factory
                .EnumAdapterByLuid(self.device.GetAdapterLuid())
                .ok()?;
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut own)
                .ok()?;
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &mut pc)
                .ok()?;
        }
        Some([
            own.Budget >> 20,
            own.CurrentUsage >> 20,
            pc.CurrentUsage >> 20,
        ])
    }

    /// What is free of the card's memory budget.
    fn room(&self) -> Option<u64> {
        let mut own = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: plain queries.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
            let adapter: IDXGIAdapter3 = factory
                .EnumAdapterByLuid(self.device.GetAdapterLuid())
                .ok()?;
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut own)
                .ok()?;
        }
        Some(own.Budget.saturating_sub(own.CurrentUsage))
    }

    /// Puts back what a run changed.
    fn restore(&mut self, contents: bool) -> Result<()> {
        if self.fresh {
            return Ok(());
        }
        let restored: HashMap<u32, &ID3D12Resource> = if contents {
            self.masters.iter().map(|(id, m)| (*id, m)).collect()
        } else {
            HashMap::new()
        };
        let (mut idle, mut into, mut back, mut plain) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (id, start) in &self.start {
            let Some(o) = self.objects.get(id) else {
                continue;
            };
            let end = self.end.get(id).unwrap_or(start);
            if restored.contains_key(id) {
                gpu::moves(&o.res, end, &[COPY_DEST], &mut into);
                gpu::moves(&o.res, &[COPY_DEST], start, &mut back);
            } else if self.frame.resources[o.index].idle {
                gpu::moves(&o.res, end, start, &mut idle);
            } else {
                gpu::moves(&o.res, end, start, &mut plain);
            }
        }
        let objects = &self.objects;
        self.worker.run(|list| {
            // SAFETY: an open list; live resources, each copied from one of
            // its own description.
            unsafe {
                barriers(list, &idle);
                barriers(list, &[alias()]);
                barriers(list, &plain);
                barriers(list, &into);
                for (id, m) in &restored {
                    list.CopyResource(&objects[id].res, *m);
                }
                if let Some(merger) = &self.merger {
                    for (id, base) in &merger.bases {
                        if let Some((_, m)) = self.masters.iter().find(|m| m.0 == *id) {
                            list.CopyResource(base, m);
                        }
                    }
                }
                barriers(list, &back);
            }
        })?;
        self.fresh = true;
        Ok(())
    }

    fn pipelines_named(&self, name: &str) -> HashSet<u32> {
        self.frame
            .pipelines
            .iter()
            .filter(|p| p.name == name)
            .map(|p| p.id)
            .collect()
    }

    /// The frame's lists, edited: for each submission, its lists' commands.
    fn edited(
        &mut self,
        o: &Options,
        report: &mut Report,
        clamps: &[Vec<Clamp>],
    ) -> Vec<Vec<Vec<Cmd>>> {
        let skip: HashSet<u32> = self
            .frame
            .pipelines
            .iter()
            .filter(|p| {
                let hashes = self.hashes(p);
                o.skip_shaders.iter().any(|s| {
                    !s.is_empty()
                        && (p.name.contains(s.as_str())
                            || hashes.iter().any(|h| h.contains(s.as_str())))
                })
            })
            .map(|p| p.id)
            .collect();
        if !o.skip_shaders.is_empty() {
            report
                .notes
                .push(format!("{} pipelines match --skip-shader", skip.len()));
        }
        let hash = self.pipelines_named(COMPARE);
        let clamp = self.pipelines_named(CLAMP);
        let compute: HashSet<u32> = self
            .frame
            .pipelines
            .iter()
            .filter(|p| p.parts.iter().any(|part| matches!(part, wire::Part::Code { kind, .. } if *kind == D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CS.0)))
            .map(|p| p.id)
            .collect();
        let mut batched = edit::Batched::default();
        let mut jobs = Vec::new();
        let mut scratches = Vec::new();
        let (mut skipped, mut hashed, mut uavs, mut merged) = (0, 0, 0, 0);
        let mut place = 0;
        let mut out = Vec::new();
        for s in &self.frame.submits {
            let mut lists = Vec::new();
            for &l in s.lists.iter().filter(|&&l| l != MISSING) {
                let mut cmds = self.commands[l as usize].clone();
                let here = place;
                place += 1;
                if o.only.is_some_and(|(from, to)| here < from || here > to) {
                    lists.push(edit::barriers_only(cmds));
                    continue;
                }
                if let Some((list, command)) = o.until
                    && here >= list
                {
                    let from = if here > list { 0 } else { (command + 1).min(cmds.len()) };
                    let rest = cmds.split_off(from);
                    cmds.extend(edit::barriers_only(rest));
                }
                if o.skip_clamps
                    && let Some(c) = clamps.get(here as usize)
                {
                    cmds = edit::skip_clamps(cmds, c);
                }
                if o.batch_clamps > 0 {
                    let scratch = edit::BATCH_SCRATCH - here;
                    let done;
                    (cmds, done) = edit::batch_clamps(
                        cmds,
                        &clamp,
                        &compute,
                        scratch,
                        o.batch_clamps > 1,
                        &mut jobs,
                    );
                    batched.clamps += done.clamps;
                    batched.batches += done.batches;
                    batched.jobs += done.jobs;
                    if done.scratch > 0 {
                        scratches.push((scratch, done.scratch));
                    }
                }
                let mut n;
                if !skip.is_empty() {
                    (cmds, n) = edit::skip_pipelines(cmds, &skip);
                    skipped += n;
                }
                if o.skip_hash_passes {
                    (cmds, n) = edit::skip_pipelines(cmds, &hash);
                    hashed += n;
                }
                if o.no_barriers {
                    (cmds, n) = edit::no_uav_barriers(cmds);
                    uavs += n;
                }
                if o.merge_barriers {
                    (cmds, n) = edit::merge_barriers(cmds);
                    merged += n;
                }
                lists.push(cmds);
            }
            out.push(lists);
        }
        if o.batch_clamps > 0 {
            report.notes.push(format!(
                "--batch-clamps: {} clamp passes became {}, of {} records",
                batched.clamps, batched.batches, batched.jobs
            ));
            if let Err(e) = self.make_batches(&jobs, &scratches) {
                report.notes.push(format!("--batch-clamps failed: {e:#}"));
            }
        }
        if !skip.is_empty() {
            report
                .notes
                .push(format!("--skip-shader left out {skipped} commands"));
        }
        if o.skip_hash_passes {
            report
                .notes
                .push(format!("--skip-hash-passes left out {hashed} dispatches"));
        }
        if o.no_barriers {
            report.notes.push(format!(
                "--no-barriers left out {uavs} unordered-access barriers"
            ));
        }
        if o.merge_barriers {
            report.notes.push(format!(
                "--merge-barriers joined {merged} barrier calls to ones before them"
            ));
        }
        out
    }

    /// What lists edited by [`edit::batch_clamps`] name: the pass, its
    /// tables and scratch buffers.
    fn make_batches(&mut self, jobs: &[u8], scratches: &[(u32, u64)]) -> Result<()> {
        let (root, pso) = crate::indirect::batch_for(&self.device)?;
        self.roots.insert(edit::BATCH_ROOT, root);
        self.pipelines.insert(edit::BATCH, pso);
        let (table, data) = gpu::buffer(
            &self.device,
            D3D12_HEAP_TYPE_UPLOAD,
            jobs.len().max(4) as u64,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        // SAFETY: a mapping of as many bytes.
        unsafe { std::ptr::copy_nonoverlapping(jobs.as_ptr(), data.cast(), jobs.len()) };
        let mut all = vec![(edit::BATCH_JOBS, table)];
        for &(id, bytes) in scratches {
            let (scratch, _) = gpu::buffer(
                &self.device,
                D3D12_HEAP_TYPE_DEFAULT,
                bytes.next_multiple_of(64 << 10),
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            )?;
            all.push((id, scratch));
        }
        for (id, res) in all {
            // SAFETY: a live buffer.
            let va = unsafe { res.GetGPUVirtualAddress() };
            let object = Object {
                res,
                va,
                index: 0,
                subs: 1,
            };
            self.objects.insert(id, object);
        }
        Ok(())
    }

    /// The clamp passes of each list of the frame, in order, and their
    /// results in [`RESULTS`] (by a run that keeps them).
    fn clamp_results(&mut self, report: &mut Report) -> Result<Vec<Vec<Clamp>>> {
        let psos = self.pipelines_named(CLAMP);
        let mut next = 0;
        let mut clamps = Vec::new();
        let mut submits = Vec::new();
        for s in &self.frame.submits {
            let mut lists = Vec::new();
            for &l in s.lists.iter().filter(|&&l| l != MISSING) {
                let cmds = self.commands[l as usize].clone();
                let found = edit::find_clamps(&cmds, &psos, &mut next);
                lists.push(edit::keep_clamped(cmds, &found)?);
                clamps.push(found);
            }
            submits.push(lists);
        }
        let n: usize = clamps.iter().map(Vec::len).sum();
        report.notes.push(format!(
            "--skip-clamps: {n} clamp passes, {} KB of arguments kept from a first run",
            next >> 10
        ));
        if n == 0 {
            return Ok(clamps);
        }
        self.results = Some(
            gpu::buffer(
                &self.device,
                D3D12_HEAP_TYPE_DEFAULT,
                next,
                D3D12_RESOURCE_FLAG_NONE,
            )?
            .0,
        );
        let built = self.build(&submits, &Options::default())?;
        self.restore(true)?;
        self.send(&built, &Options::default())?;
        self.fresh = false;
        Ok(clamps)
    }

    /// Makes the lists of all submissions, and before each one's the copies
    /// of what the CPU wrote.
    fn build(&self, submits: &[Vec<Vec<Cmd>>], o: &Options) -> Result<Sends> {
        // Room for the timestamps of every command that has one.
        let count: usize = submits
            .iter()
            .flatten()
            .map(|c| {
                2 + if o.detailed {
                    c.iter()
                        .filter(|c| c.works() || matches!(c, Cmd::Barriers { .. }))
                        .count()
                } else {
                    0
                }
            })
            .sum::<usize>()
            + 2 * submits.len();
        let mut heap: Option<ID3D12QueryHeap> = None;
        let desc = D3D12_QUERY_HEAP_DESC {
            Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
            Count: count.max(1) as u32,
            NodeMask: 0,
        };
        // SAFETY: plain creation.
        unsafe { self.device.CreateQueryHeap(&desc, &mut heap)? };
        let heap = heap.context("no timestamps")?;
        let (back, data) = gpu::buffer(
            &self.device,
            D3D12_HEAP_TYPE_READBACK,
            count.max(1) as u64 * 8,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        let mut next = 0u32;
        let mut steps = Vec::new();
        for (i, lists) in submits.iter().enumerate() {
            let parts = self.delta_parts.get(i).filter(|p| !p.is_empty());
            let delta = match (parts, &self.deltas) {
                (Some(parts), Some(from)) => {
                    let open = self.open(&heap, &mut next)?;
                    // SAFETY: an open list; live buffers.
                    unsafe {
                        let mut seen = HashSet::new();
                        let mut after = Vec::new();
                        let mut merges = Vec::new();
                        for (d, at) in parts {
                            let Some(ob) = self.objects.get(&d.res) else {
                                continue;
                            };
                            if self.merging && self.card_writes.contains(&d.res) {
                                merges.push((d.res, d.at, *at, d.len));
                                continue;
                            }
                            open.1.CopyBufferRegion(&ob.res, d.at, from, *at, d.len);
                            if seen.insert(d.res) {
                                // Back to the common state by itself, for
                                // lists in the same submission.
                                after.push(gpu::transition(&ob.res, ALL, COPY_DEST, COMMON));
                            }
                        }
                        barriers(&open.1, &after);
                        if !merges.is_empty() {
                            self.wait_for_all(&open.1);
                            self.merge_all(&open.1, from.GetGPUVirtualAddress(), &merges);
                            self.wait_for_all(&open.1);
                        }
                    }
                    Some(self.close(open, &heap, &mut next, Vec::new())?)
                }
                _ => None,
            };
            let mut built = Vec::new();
            for cmds in lists {
                let open = self.open(&heap, &mut next)?;
                let stamps = self.translate(
                    &open.1,
                    cmds,
                    i,
                    o.detailed.then_some((&heap, &mut next)),
                    o.wait_all,
                );
                built.push(self.close(open, &heap, &mut next, stamps)?);
            }
            steps.push((delta, built));
        }
        // Between submissions that are sent as one: everything waits. A list
        // each: one cannot stand twice in a submission.
        let mut joints = Vec::new();
        if o.one_submit {
            for _ in 0..2 * submits.len() {
                let open = self.plain_list()?;
                // SAFETY: an open list.
                unsafe {
                    self.wait_for_all(&open.1);
                    open.1.Close()?;
                }
                joints.push(open);
            }
        }
        let resolve = {
            let open = self.plain_list()?;
            // SAFETY: an open list; `next` queries were written.
            unsafe {
                open.1.ResolveQueryData(
                    &heap,
                    D3D12_QUERY_TYPE_TIMESTAMP,
                    0,
                    next.max(1),
                    &back,
                    0,
                );
                open.1.Close()?;
            }
            open
        };
        Ok(Sends {
            steps,
            joints,
            resolve,
            _heap: heap,
            _back: back,
            data: data as usize,
            count: next,
        })
    }

    /// Records a wait for everything before, with all caches written out.
    ///
    /// # Safety
    /// An open list.
    unsafe fn wait_for_all(&self, list: &ID3D12GraphicsCommandList) {
        // SAFETY: the caller's; a feature structure of its size.
        unsafe {
            let mut o = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
            let size = size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS12>() as u32;
            let known = self
                .device
                .CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS12, (&raw mut o).cast(), size)
                .is_ok();
            if known
                && o.EnhancedBarriersSupported.as_bool()
                && let Ok(list) = list.cast::<ID3D12GraphicsCommandList7>()
            {
                let all = [D3D12_GLOBAL_BARRIER {
                    SyncBefore: D3D12_BARRIER_SYNC_ALL,
                    SyncAfter: D3D12_BARRIER_SYNC_ALL,
                    AccessBefore: D3D12_BARRIER_ACCESS_COMMON,
                    AccessAfter: D3D12_BARRIER_ACCESS_COMMON,
                }];
                list.Barrier(&[D3D12_BARRIER_GROUP {
                    Type: D3D12_BARRIER_TYPE_GLOBAL,
                    NumBarriers: 1,
                    Anonymous: D3D12_BARRIER_GROUP_0 {
                        pGlobalBarriers: all.as_ptr(),
                    },
                }]);
            } else {
                list.ResourceBarrier(&[alias(), uav_barrier(None)]);
            }
        }
    }

    fn plain_list(&self) -> Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)> {
        // SAFETY: plain creation.
        unsafe {
            let allocator: ID3D12CommandAllocator = self
                .device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let list = self.device.CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &allocator,
                None,
            )?;
            Ok((allocator, list))
        }
    }

    fn open(
        &self,
        heap: &ID3D12QueryHeap,
        next: &mut u32,
    ) -> Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList, u32)> {
        let (allocator, list) = self.plain_list()?;
        // SAFETY: an open list; a query of the heap.
        unsafe { list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, *next) };
        *next += 1;
        Ok((allocator, list, *next - 1))
    }

    fn close(
        &self,
        (allocator, list, first): (ID3D12CommandAllocator, ID3D12GraphicsCommandList, u32),
        heap: &ID3D12QueryHeap,
        next: &mut u32,
        stamps: Vec<Stamp>,
    ) -> Result<Built> {
        // SAFETY: as above.
        unsafe {
            list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, *next);
            list.Close()
                .context("a list of the frame could not be recorded again")?;
        }
        *next += 1;
        Ok(Built {
            _allocator: allocator,
            list,
            first,
            last: *next - 1,
            stamps,
        })
    }

    /// Records commands into a list, in submission `submit`.
    fn translate(
        &self,
        list: &ID3D12GraphicsCommandList,
        cmds: &[Cmd],
        submit: usize,
        mut stamp: Option<(&ID3D12QueryHeap, &mut u32)>,
        wait: bool,
    ) -> Vec<Stamp> {
        let version = self.version_of.get(submit).copied().unwrap_or(0);
        let mut clamps = self.pipelines_named(CLAMP);
        clamps.insert(edit::BATCH);
        let compares = self.pipelines_named(COMPARE);
        let mesh: HashSet<u32> = self
            .frame
            .pipelines
            .iter()
            .filter(|p| p.parts.iter().any(|part| matches!(part, wire::Part::Code { kind, .. } if *kind == D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS.0)))
            .map(|p| p.id)
            .collect();
        let dispatches: HashSet<u32> = self
            .frame
            .signatures
            .iter()
            .filter(|s| {
                s.args
                    .iter()
                    .any(|a| a[0] == D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH.0 as u32)
            })
            .map(|s| s.id)
            .collect();
        let mut stamps = Vec::new();
        // A list of ours: one that runs no shader but ours (the host's state
        // preludes, copies of what the CPU sent up, the card copies' passes);
        // its commands are summed apart from the title's lists' ones.
        let ours = {
            let mut bound = NONE;
            !cmds.iter().any(|c| {
                if let Cmd::Pipeline { pso } = c {
                    bound = *pso;
                }
                c.runs_shaders() && !clamps.contains(&bound) && !compares.contains(&bound)
            })
        };
        let mut bound = NONE;
        // What the compute side has bound.
        let mut compute: Vec<Cmd> = Vec::new();
        for c in cmds {
            if let Cmd::Pipeline { pso } = c {
                bound = *pso;
            }
            let merged = self.merged_copy(c);
            if let Some((from, what)) = merged {
                // SAFETY: an open list; live buffers; what the compute side
                // of it had bound is bound again.
                unsafe {
                    self.wait_for_all(list);
                    self.merge_all(list, from, &[what]);
                    self.wait_for_all(list);
                    for c in &compute {
                        self.command(list, c, version);
                    }
                }
            } else {
                // SAFETY: an open list; the objects are the frame's, alive
                // with the player; commands the host recorded the same way.
                unsafe { self.command(list, c, version) };
            }
            match c {
                Cmd::RootSignature {
                    graphics: false, ..
                } => {
                    compute.retain(|c| matches!(c, Cmd::Pipeline { .. }));
                    compute.push(c.clone());
                }
                Cmd::Pipeline { .. } => {
                    compute.retain(|c| !matches!(c, Cmd::Pipeline { .. }));
                    compute.push(c.clone());
                }
                Cmd::Table {
                    graphics: false, ..
                }
                | Cmd::Constants {
                    graphics: false, ..
                }
                | Cmd::RootView {
                    graphics: false, ..
                } => compute.push(c.clone()),
                _ => {}
            }
            if wait && c.works() {
                // SAFETY: an open list.
                unsafe { self.wait_for_all(list) };
            }
            let timed = c.works() || matches!(c, Cmd::Barriers { .. });
            if timed && let Some((heap, next)) = &mut stamp {
                // SAFETY: as above.
                unsafe { list.EndQuery(*heap, D3D12_QUERY_TYPE_TIMESTAMP, **next) };
                let category = match c {
                    Cmd::Barriers { .. } => "barriers",
                    Cmd::Dispatch { .. } if clamps.contains(&bound) => "argument clamps (ours)",
                    Cmd::Dispatch { .. } if compares.contains(&bound) => {
                        "hash and compare passes (ours)"
                    }
                    Cmd::Dispatch { .. } => "dispatches",
                    Cmd::Indirect { signature, .. } if dispatches.contains(signature) => {
                        "indirect dispatches"
                    }
                    Cmd::Indirect { .. } if mesh.contains(&bound) => "indirect mesh draws",
                    Cmd::Indirect { .. } => "indirect draws",
                    Cmd::Draw { .. } | Cmd::DrawIndexed { .. } | Cmd::DispatchMesh { .. } => {
                        "draws"
                    }
                    Cmd::ClearDepth { .. } | Cmd::ClearTarget { .. } | Cmd::Discard { .. } => {
                        "clears"
                    }
                    Cmd::Resolve { .. } | Cmd::ResolveRegion { .. } => "resolves",
                    _ => self.copy_kind(c),
                };
                let category = if ours { of_ours(category) } else { category };
                let runs = match c {
                    c if c.runs_shaders() => bound,
                    // Copies to textures by what they go to.
                    Cmd::CopyResource { dst, .. }
                    | Cmd::CopyTexture {
                        dst: Loc::Sub { res: dst, .. },
                        ..
                    } => COPY_TO | dst,
                    _ => NONE,
                };
                stamps.push((**next as usize, category, runs));
                **next += 1;
            }
        }
        stamps
    }

    /// Which copies a copy is counted with: by the memory it crosses.
    fn copy_kind(&self, c: &Cmd) -> &'static str {
        let pc = |id: u32| {
            self.objects.get(&id).is_some_and(|o| {
                matches!(&self.frame.resources[o.index].place,
                    Place::Committed { props, .. } if gpu::seen(props) != Seen::No)
            })
        };
        let res = |l: &Loc| match l {
            Loc::Sub { res, .. } | Loc::Placed { res, .. } => *res,
        };
        match c {
            Cmd::CopyBuffer { dst, src, .. } => match (pc(*dst), pc(*src)) {
                (false, true) => "copies of buffers, PC to card",
                (true, false) => "copies of buffers, card to PC",
                (true, true) => "copies of buffers, PC to PC",
                (false, false) => "copies of buffers, card to card",
            },
            Cmd::CopyTexture { dst, src, .. } => match (pc(res(dst)), pc(res(src))) {
                (false, true) => "copies of textures, PC to card",
                (true, _) => "copies of textures, to the PC",
                (false, false) => "copies of textures, card to card",
            },
            Cmd::CopyResource { .. } => "copies of whole resources",
            _ => "copies",
        }
    }

    unsafe fn command(&self, l: &ID3D12GraphicsCommandList, c: &Cmd, version: usize) {
        let res = |id: u32| self.resource(id);
        let view = |s: &Slot| self.targets.get(s).copied().unwrap_or_default();
        // SAFETY: the caller's.
        unsafe {
            match c {
                Cmd::Draw {
                    vertices,
                    instances,
                    first,
                    first_instance,
                } => l.DrawInstanced(*vertices, *instances, *first, *first_instance),
                Cmd::DrawIndexed {
                    indices,
                    instances,
                    first,
                    base,
                    first_instance,
                } => {
                    l.DrawIndexedInstanced(*indices, *instances, *first, *base, *first_instance);
                }
                Cmd::Dispatch { groups } => l.Dispatch(groups[0], groups[1], groups[2]),
                Cmd::DispatchMesh { groups } => {
                    if let Ok(l) = l.cast::<ID3D12GraphicsCommandList6>() {
                        l.DispatchMesh(groups[0], groups[1], groups[2]);
                    }
                }
                Cmd::CopyBuffer {
                    dst,
                    dst_at,
                    src,
                    src_at,
                    len,
                } => {
                    if let (Some(dst), Some(src)) = (res(*dst), res(*src)) {
                        l.CopyBufferRegion(dst, *dst_at, src, *src_at, *len);
                    }
                }
                Cmd::CopyTexture { dst, at, src, part } => {
                    let side = |loc: &Loc| match loc {
                        Loc::Sub { res: id, sub } => Some(gpu::sub_location(res(*id)?, *sub)),
                        Loc::Placed {
                            res: id,
                            offset,
                            format,
                            size,
                            pitch,
                        } => {
                            let f = D3D12_SUBRESOURCE_FOOTPRINT {
                                Format: DXGI_FORMAT(*format),
                                Width: size[0],
                                Height: size[1],
                                Depth: size[2],
                                RowPitch: *pitch,
                            };
                            Some(gpu::placed_location(
                                res(*id)?,
                                D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                                    Offset: *offset,
                                    Footprint: f,
                                },
                            ))
                        }
                    };
                    if let (Some(dst), Some(src)) = (side(dst), side(src)) {
                        let part = part.map(|p| D3D12_BOX {
                            left: p[0],
                            top: p[1],
                            front: p[2],
                            right: p[3],
                            bottom: p[4],
                            back: p[5],
                        });
                        l.CopyTextureRegion(
                            &dst,
                            at[0],
                            at[1],
                            at[2],
                            &src,
                            part.as_ref().map(|p| p as *const _),
                        );
                    }
                }
                Cmd::CopyResource { dst, src } => {
                    if let (Some(dst), Some(src)) = (res(*dst), res(*src)) {
                        l.CopyResource(dst, src);
                    }
                }
                Cmd::Resolve {
                    dst,
                    dst_sub,
                    src,
                    src_sub,
                    format,
                } => {
                    if let (Some(dst), Some(src)) = (res(*dst), res(*src)) {
                        l.ResolveSubresource(dst, *dst_sub, src, *src_sub, DXGI_FORMAT(*format));
                    }
                }
                Cmd::ResolveRegion {
                    dst,
                    dst_sub,
                    at,
                    src,
                    src_sub,
                    part,
                    format,
                    mode,
                } => {
                    if let (Some(dst), Some(src), Ok(l)) =
                        (res(*dst), res(*src), l.cast::<ID3D12GraphicsCommandList1>())
                    {
                        let part = part.map(to_rect);
                        let part = part.as_ref().map(|r| r as *const _);
                        l.ResolveSubresourceRegion(
                            dst,
                            *dst_sub,
                            at[0],
                            at[1],
                            src,
                            *src_sub,
                            part,
                            DXGI_FORMAT(*format),
                            D3D12_RESOLVE_MODE(*mode),
                        );
                    }
                }
                Cmd::Topology { kind } => l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY(*kind)),
                Cmd::Viewports { all } => {
                    let all: Vec<D3D12_VIEWPORT> = all
                        .iter()
                        .map(|v| D3D12_VIEWPORT {
                            TopLeftX: v[0],
                            TopLeftY: v[1],
                            Width: v[2],
                            Height: v[3],
                            MinDepth: v[4],
                            MaxDepth: v[5],
                        })
                        .collect();
                    l.RSSetViewports(&all);
                }
                Cmd::Scissors { all } => {
                    l.RSSetScissorRects(&all.iter().map(|r| to_rect(*r)).collect::<Vec<_>>())
                }
                Cmd::BlendFactor { factor } => l.OMSetBlendFactor(factor.as_ref()),
                Cmd::StencilRef { value } => l.OMSetStencilRef(*value),
                Cmd::DepthBounds { min, max } => {
                    if let Ok(l) = l.cast::<ID3D12GraphicsCommandList1>() {
                        l.OMSetDepthBounds(*min, *max);
                    }
                }
                Cmd::Pipeline { pso } => {
                    if let Some(p) = self.pipelines.get(pso) {
                        l.SetPipelineState(p);
                    }
                }
                Cmd::Barriers { all } => {
                    let all: Vec<D3D12_RESOURCE_BARRIER> =
                        all.iter().filter_map(|b| self.barrier(b)).collect();
                    barriers(l, &all);
                }
                Cmd::Heaps { heaps } => {
                    let all: Vec<Option<ID3D12DescriptorHeap>> = heaps
                        .iter()
                        .filter_map(|id| {
                            Some(Some(self.descriptors.get(id)?.0.get(version)?.heap.clone()))
                        })
                        .collect();
                    l.SetDescriptorHeaps(&all);
                }
                Cmd::RootSignature { graphics, root } => {
                    let root = self.roots.get(root);
                    if *graphics {
                        l.SetGraphicsRootSignature(root)
                    } else {
                        l.SetComputeRootSignature(root)
                    }
                }
                Cmd::Table {
                    graphics,
                    param,
                    heap,
                    slot,
                } => {
                    if let Some((versions, step)) = self.descriptors.get(heap)
                        && let Some(v) = versions.get(version)
                    {
                        let at = D3D12_GPU_DESCRIPTOR_HANDLE {
                            ptr: v.gpu + u64::from(*slot) * step,
                        };
                        if *graphics {
                            l.SetGraphicsRootDescriptorTable(*param, at)
                        } else {
                            l.SetComputeRootDescriptorTable(*param, at)
                        }
                    }
                }
                Cmd::Constants {
                    graphics,
                    param,
                    offset,
                    values,
                } => {
                    let (n, data) = (values.len() as u32, values.as_ptr().cast());
                    if *graphics {
                        l.SetGraphicsRoot32BitConstants(*param, n, data, *offset);
                    } else {
                        l.SetComputeRoot32BitConstants(*param, n, data, *offset);
                    }
                }
                Cmd::RootView {
                    graphics,
                    kind,
                    param,
                    va,
                } => {
                    let va = self.va(va);
                    match (*kind, *graphics) {
                        (slot::CBV, true) => l.SetGraphicsRootConstantBufferView(*param, va),
                        (slot::CBV, false) => l.SetComputeRootConstantBufferView(*param, va),
                        (slot::SRV, true) => l.SetGraphicsRootShaderResourceView(*param, va),
                        (slot::SRV, false) => l.SetComputeRootShaderResourceView(*param, va),
                        (_, true) => l.SetGraphicsRootUnorderedAccessView(*param, va),
                        (_, false) => l.SetComputeRootUnorderedAccessView(*param, va),
                    }
                }
                Cmd::IndexBuffer { view } => {
                    let v = view
                        .as_ref()
                        .map(|(va, size, format)| D3D12_INDEX_BUFFER_VIEW {
                            BufferLocation: self.va(va),
                            SizeInBytes: *size,
                            Format: DXGI_FORMAT(*format),
                        });
                    l.IASetIndexBuffer(v.as_ref().map(|v| v as *const _));
                }
                Cmd::VertexBuffers {
                    start,
                    views,
                    count,
                } => match views {
                    Some(views) => {
                        let all: Vec<D3D12_VERTEX_BUFFER_VIEW> = views
                            .iter()
                            .map(|(va, size, stride)| D3D12_VERTEX_BUFFER_VIEW {
                                BufferLocation: self.va(va),
                                SizeInBytes: *size,
                                StrideInBytes: *stride,
                            })
                            .collect();
                        l.IASetVertexBuffers(*start, Some(&all));
                    }
                    None => l.IASetVertexBuffers(
                        *start,
                        Some(&vec![D3D12_VERTEX_BUFFER_VIEW::default(); *count as usize]),
                    ),
                },
                Cmd::Targets { colors, depth } => {
                    let colors: Vec<D3D12_CPU_DESCRIPTOR_HANDLE> =
                        colors.iter().map(view).collect();
                    let depth = depth.as_ref().map(view);
                    l.OMSetRenderTargets(
                        colors.len() as u32,
                        (!colors.is_empty()).then_some(colors.as_ptr()),
                        false,
                        depth.as_ref().map(|d| d as *const _),
                    );
                }
                Cmd::ClearDepth {
                    view: v,
                    flags,
                    depth,
                    stencil,
                    rects,
                } => {
                    let rects: Vec<RECT> = rects.iter().map(|r| to_rect(*r)).collect();
                    let rects = (!rects.is_empty()).then_some(&rects[..]);
                    if self.objects.contains_key(&v.res) {
                        l.ClearDepthStencilView(
                            view(v),
                            D3D12_CLEAR_FLAGS(*flags),
                            *depth,
                            *stencil,
                            rects,
                        );
                    }
                }
                Cmd::ClearTarget {
                    view: v,
                    color,
                    rects,
                } => {
                    let rects: Vec<RECT> = rects.iter().map(|r| to_rect(*r)).collect();
                    let rects = (!rects.is_empty()).then_some(&rects[..]);
                    if self.objects.contains_key(&v.res) {
                        l.ClearRenderTargetView(view(v), color, rects);
                    }
                }
                Cmd::Discard {
                    res: id,
                    rects,
                    subs,
                } => {
                    if let Some(r) = res(*id) {
                        let rects: Vec<RECT> = rects.iter().map(|r| to_rect(*r)).collect();
                        let region = subs.map(|s| D3D12_DISCARD_REGION {
                            NumRects: rects.len() as u32,
                            pRects: if rects.is_empty() {
                                std::ptr::null()
                            } else {
                                rects.as_ptr()
                            },
                            FirstSubresource: s[0],
                            NumSubresources: s[1],
                        });
                        l.DiscardResource(r, region.as_ref().map(|r| r as *const _));
                    }
                }
                Cmd::BeginQuery { heap, kind, index } => {
                    if let Some(h) = self.queries.get(heap) {
                        l.BeginQuery(h, D3D12_QUERY_TYPE(*kind), *index);
                    }
                }
                Cmd::EndQuery { heap, kind, index } => {
                    if let Some(h) = self.queries.get(heap) {
                        l.EndQuery(h, D3D12_QUERY_TYPE(*kind), *index);
                    }
                }
                Cmd::ResolveQuery {
                    heap,
                    kind,
                    first,
                    count,
                    dst,
                    at,
                } => {
                    if let (Some(h), Some(dst)) = (self.queries.get(heap), res(*dst)) {
                        l.ResolveQueryData(h, D3D12_QUERY_TYPE(*kind), *first, *count, dst, *at);
                    }
                }
                Cmd::Indirect {
                    signature,
                    most,
                    args,
                    at,
                    count,
                    count_at,
                } => {
                    if let (Some(s), Some(a)) = (self.signatures.get(signature), res(*args)) {
                        l.ExecuteIndirect(s, *most, a, *at, res(*count), *count_at);
                    }
                }
                Cmd::WriteImmediate { all } => {
                    if let Ok(l) = l.cast::<ID3D12GraphicsCommandList2>() {
                        let all: Vec<&(Va, u32, i32)> =
                            all.iter().filter(|(va, _, _)| self.va(va) != 0).collect();
                        let params: Vec<D3D12_WRITEBUFFERIMMEDIATE_PARAMETER> = all
                            .iter()
                            .map(|(va, v, _)| D3D12_WRITEBUFFERIMMEDIATE_PARAMETER {
                                Dest: self.va(va),
                                Value: *v,
                            })
                            .collect();
                        let modes: Vec<D3D12_WRITEBUFFERIMMEDIATE_MODE> = all
                            .iter()
                            .map(|(_, _, m)| D3D12_WRITEBUFFERIMMEDIATE_MODE(*m))
                            .collect();
                        if !params.is_empty() {
                            l.WriteBufferImmediate(
                                params.len() as u32,
                                params.as_ptr(),
                                Some(modes.as_ptr()),
                            );
                        }
                    }
                }
            }
        }
    }

    fn barrier(&self, b: &Barrier) -> Option<D3D12_RESOURCE_BARRIER> {
        Some(match b {
            Barrier::Transition {
                flags,
                res,
                sub,
                before,
                after,
            } => {
                let mut t =
                    gpu::transition(self.resource(*res)?, *sub, *before as u32, *after as u32);
                t.Flags = D3D12_RESOURCE_BARRIER_FLAGS(*flags);
                t
            }
            Barrier::Uav { flags, res } => {
                let mut u = uav_barrier(if *res == NONE {
                    None
                } else {
                    Some(self.resource(*res)?)
                });
                u.Flags = D3D12_RESOURCE_BARRIER_FLAGS(*flags);
                u
            }
            Barrier::Alias {
                flags,
                before,
                after,
            } => {
                let side = |id: &u32| -> std::mem::ManuallyDrop<Option<ID3D12Resource>> {
                    // SAFETY: a borrowed pointer (or none) never released.
                    self.resource(*id)
                        .map_or(std::mem::ManuallyDrop::new(None), |r| unsafe {
                            std::mem::transmute_copy(r)
                        })
                };
                D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
                    Flags: D3D12_RESOURCE_BARRIER_FLAGS(*flags),
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        Aliasing: std::mem::ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                            pResourceBefore: side(before),
                            pResourceAfter: side(after),
                        }),
                    },
                }
            }
        })
    }

    /// Sends the frame, waits for it, and reads the timestamps.
    fn send(&mut self, s: &Sends, o: &Options) -> Result<Vec<u64>> {
        let cast = |l: &ID3D12GraphicsCommandList| l.cast::<ID3D12CommandList>().map(Some);
        let mut submissions: Vec<Vec<Option<ID3D12CommandList>>> = Vec::new();
        for (delta, lists) in &s.steps {
            if let Some(d) = delta {
                submissions.push(vec![cast(&d.list)?]);
            }
            let mut all = Vec::new();
            for l in lists {
                all.push(cast(&l.list)?);
            }
            if !all.is_empty() {
                submissions.push(all);
            }
        }
        if o.one_submit {
            let mut all = Vec::new();
            for (i, lists) in submissions.into_iter().enumerate() {
                if i > 0
                    && let Some(joint) = s.joints.get(i)
                {
                    all.push(cast(&joint.1)?);
                }
                all.extend(lists);
            }
            submissions = vec![all];
        }
        // SAFETY: closed lists whose last run has finished.
        unsafe {
            for all in &submissions {
                if !o.wait_lists {
                    self.worker.queue.ExecuteCommandLists(all);
                    if o.pace > 0.0 {
                        let until = Instant::now() + std::time::Duration::from_secs_f64(o.pace / 1e3);
                        while Instant::now() < until {
                            std::thread::yield_now();
                        }
                    }
                    continue;
                }
                for l in all {
                    self.worker.queue.ExecuteCommandLists(std::slice::from_ref(l));
                    self.worker.idle()?;
                }
            }
            self.worker
                .queue
                .ExecuteCommandLists(&[cast(&s.resolve.1)?]);
        }
        self.worker
            .idle()
            .context("the card was lost running the frame")?;
        // SAFETY: the mapping of `count` resolved timestamps.
        Ok(unsafe { std::slice::from_raw_parts(s.data as *const u64, s.count as usize) }.to_vec())
    }

    /// Runs the frame as the options say.
    pub fn run(&mut self, o: &Options) -> Result<Report> {
        let mut made = self.prepare(o)?;
        let _awake = if o.awake { crate::host::awake::start(&self.device, || true) } else { None };
        let mut stamps = Vec::new();
        for _ in 0..o.loops.max(1) {
            stamps.push(self.once(o, &mut made)?);
        }
        self.finish(o, made, &stamps)
    }

    /// The lists for a run as the options say.
    fn prepare(&mut self, o: &Options) -> Result<(Sends, Report)> {
        let mut report = Report {
            notes: self.notes.clone(),
            ..Default::default()
        };
        self.set_merging(o.merge)?;
        let clamps = if o.skip_clamps {
            self.clamp_results(&mut report)?
        } else {
            Vec::new()
        };
        let submits = self.edited(o, &mut report, &clamps);
        Ok((self.build(&submits, o)?, report))
    }

    /// One run of the frame, noted in the report; its timestamps.
    fn once(&mut self, o: &Options, (built, report): &mut (Sends, Report)) -> Result<Vec<u64>> {
        self.restore(o.restore)?;
        self.fresh = false;
        let stamps = self.send(built, o)?;
        if let Some((db, _, hash, _)) = self.picture_now()? {
            report.pictures.push((db, hash));
        }
        let ms = |a: u32, b: u32| {
            stamps[b as usize].saturating_sub(stamps[a as usize]) as f64 * 1e3 / self.frequency
        };
        let lists: Vec<&Built> = built.steps.iter().flat_map(|(_, l)| l).collect();
        let (Some(first), Some(last)) = (lists.first(), lists.last()) else {
            bail!("no lists to run");
        };
        let delta = built.steps.iter().filter_map(|(d, _)| d.as_ref());
        report.delta_ms.push(
            delta
                .filter(|d| d.first > first.first)
                .map(|d| ms(d.first, d.last))
                .sum(),
        );
        report.frame_ms.push(ms(first.first, last.last));
        let each = |l: &[Built]| match (l.first(), l.last()) {
            (Some(a), Some(b)) => ms(a.first, b.last),
            _ => 0.0,
        };
        report.submits = built
            .steps
            .iter()
            .map(|(_, l)| (l.len(), each(l)))
            .collect();
        report
            .busy_ms
            .push(report.submits.iter().map(|s| s.1).sum());
        Ok(stamps)
    }

    /// The report after the last run.
    fn finish(&mut self, o: &Options, (built, mut report): (Sends, Report), stamps: &[Vec<u64>]) -> Result<Report> {
        if o.detailed {
            self.detail(&built, stamps, o, &mut report);
        }
        if self.merging {
            let notes = self.merge_notes(o.loops)?;
            report.notes.extend(notes);
        }
        self.picture(o, &mut report)?;
        if let Some(q) = &self.info {
            report.notes.extend(debug_messages(q));
        }
        Ok(report)
    }

    /// The time by command: the median of the loops, without the first (which
    /// is slower) where there are others.
    fn detail(&self, built: &Sends, stamps: &[Vec<u64>], o: &Options, report: &mut Report) {
        let mut kinds: BTreeMap<&str, (Vec<f64>, u32)> = BTreeMap::new();
        let mut shaders: HashMap<u32, (Vec<f64>, u32)> = HashMap::new();
        let loops = if stamps.len() > 2 { &stamps[1..] } else { stamps };
        for stamps in loops {
            let mut of_kind: BTreeMap<&str, (f64, u32)> = BTreeMap::new();
            let mut of_shader: HashMap<u32, (f64, u32)> = HashMap::new();
            for l in built.steps.iter().flat_map(|(_, l)| l) {
                let mut before = stamps[l.first as usize];
                for &(at, kind, pso) in &l.stamps {
                    let ms = stamps[at].saturating_sub(before) as f64 * 1e3 / self.frequency;
                    before = before.max(stamps[at]);
                    let k = of_kind.entry(kind).or_default();
                    *k = (k.0 + ms, k.1 + 1);
                    if pso != NONE {
                        let s = of_shader.entry(pso).or_default();
                        *s = (s.0 + ms, s.1 + 1);
                    }
                }
            }
            for (k, (ms, n)) in of_kind {
                let all = kinds.entry(k).or_default();
                all.0.push(ms);
                all.1 = n;
            }
            for (k, (ms, n)) in of_shader {
                let all = shaders.entry(k).or_default();
                all.0.push(ms);
                all.1 = n;
            }
        }
        let kinds = kinds.into_iter().map(|(k, (ms, n))| (k, (median(&ms), n)));
        let shaders = shaders.into_iter().map(|(k, (ms, n))| (k, (median(&ms), n)));
        report.categories = kinds
            .into_iter()
            .map(|(k, (ms, n))| (k.to_owned(), ms, n))
            .collect();
        report.categories.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut all: Vec<(f64, u32, u32)> = shaders
            .into_iter()
            .map(|(pso, (ms, n))| (ms, n, pso))
            .collect();
        all.sort_by(|a, b| b.0.total_cmp(&a.0));
        all.truncate(o.top);
        report.shaders = all
            .into_iter()
            .map(|(ms, n, pso)| {
                let name = match self.objects.get(&(pso & !COPY_TO)).filter(|_| pso & COPY_TO != 0) {
                    Some(o) => {
                        let r = &self.frame.resources[o.index];
                        format!("copies to #{} {}", pso & !COPY_TO, r.name)
                    }
                    None => self.pipeline_name(pso),
                };
                (ms, n, name)
            })
            .collect();
    }

    /// The presented picture as it is now: dB against the recorded one,
    /// whether its bytes are the same, its hash, and its pixels.
    fn picture_now(&mut self) -> Result<Option<(f64, bool, String, Vec<u8>)>> {
        let Some(p) = self.frame.presents.last().cloned() else {
            return Ok(None);
        };
        let Some(bytes) = self.contents(p.res)? else {
            return Ok(None);
        };
        let Some(rgb) = gpu::rgb(p.width, p.height, p.pitch, p.format, &bytes) else {
            return Ok(None);
        };
        let mut recorded = vec![0; bytes.len()];
        read_at(
            &File::open(self.folder.join("present.bin"))?,
            &mut recorded,
            p.file,
        )?;
        let was = gpu::rgb(p.width, p.height, p.pitch, p.format, &recorded).unwrap_or_default();
        let hash = blake3::hash(&rgb).to_hex().to_string();
        Ok(Some((
            gpu::psnr(&rgb, &was),
            rows_equal(&p, &bytes, &recorded),
            hash,
            rgb,
        )))
    }

    /// Saves the presented picture and compares it with the recorded one.
    fn picture(&mut self, o: &Options, report: &mut Report) -> Result<()> {
        let Some(p) = self.frame.presents.last().cloned() else {
            report.notes.push("no picture was presented".into());
            return Ok(());
        };
        let Some(bytes) = self.contents(p.res)? else {
            report
                .notes
                .push("the presented texture is not there".into());
            return Ok(());
        };
        let Some(rgb) = gpu::rgb(p.width, p.height, p.pitch, p.format, &bytes) else {
            report
                .notes
                .push("the presented format is not shown".into());
            return Ok(());
        };
        let path = o
            .image
            .clone()
            .unwrap_or_else(|| self.folder.join("replay.png"));
        std::fs::write(&path, crate::capture::png(p.width, p.height, &rgb))?;
        let mut recorded = vec![0; bytes.len()];
        read_at(
            &File::open(self.folder.join("present.bin"))?,
            &mut recorded,
            p.file,
        )?;
        let was = gpu::rgb(p.width, p.height, p.pitch, p.format, &recorded).unwrap_or_default();
        report.picture = Some((gpu::psnr(&rgb, &was), rows_equal(&p, &bytes, &recorded)));
        report.notes.push(format!(
            "picture {} (blake3 {}; recorded {})",
            path.display(),
            &blake3::hash(&rgb).to_hex()[..16],
            &blake3::hash(&was).to_hex()[..16]
        ));
        Ok(())
    }

    /// What a resource holds now (after a run): a buffer's bytes, or a
    /// texture's first subresource in its copyable layout.
    pub fn contents(&mut self, id: u32) -> Result<Option<Vec<u8>>> {
        self.contents_of(id, 0)
    }

    /// As [`Player::contents`], of a texture's subresource `sub`.
    pub fn contents_of(&mut self, id: u32, sub: u32) -> Result<Option<Vec<u8>>> {
        let Some(o) = self.objects.get(&id) else {
            return Ok(None);
        };
        let (res, r, subs) = (o.res.clone(), &self.frame.resources[o.index], o.subs);
        let desc = gpu::to_desc(&r.desc);
        let is_buffer = r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0;
        let states = if self.fresh { &self.start } else { &self.end };
        let state = states.get(&id).cloned().unwrap_or(vec![COMMON]);
        let (f, total) = if is_buffer {
            (Vec::new(), r.desc.width)
        } else {
            gpu::footprints(&self.device, &desc, sub, 1)
        };
        let (back, data) = gpu::buffer(
            &self.device,
            D3D12_HEAP_TYPE_READBACK,
            total,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        self.worker.run(|list| {
            if is_buffer {
                // SAFETY: an open list; live buffers.
                unsafe { list.CopyBufferRegion(&back, 0, &res, 0, total) };
            } else if state.len() == 1 || subs == 1 {
                read_sub(list, &res, sub, state[0], &back, f[0]);
            } else {
                let (mut before, mut after) = (Vec::new(), Vec::new());
                gpu::moves(&res, &state, &[COPY_SOURCE], &mut before);
                gpu::moves(&res, &[COPY_SOURCE], &state, &mut after);
                // SAFETY: an open list; live resources.
                unsafe { barriers(list, &before) };
                read_sub(list, &res, sub, COPY_SOURCE, &back, f[0]);
                // SAFETY: as above.
                unsafe { barriers(list, &after) };
            }
        })?;
        // SAFETY: the buffer's mapping; the copy into it has run.
        Ok(Some(
            unsafe { std::slice::from_raw_parts(data, total as usize) }.to_vec(),
        ))
    }

    /// The id of the resource of a name (tests).
    pub fn named(&self, name: &str) -> Option<u32> {
        self.frame
            .resources
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.id)
    }
}

/// Everything to send for a frame.
struct Sends {
    /// For each submission: the copies of what the CPU wrote, its lists.
    steps: Vec<(Option<Built>, Vec<Built>)>,
    joints: Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)>,
    resolve: (ID3D12CommandAllocator, ID3D12GraphicsCommandList),
    _heap: ID3D12QueryHeap,
    _back: ID3D12Resource,
    data: usize,
    count: u32,
}

fn rows_equal(p: &wire::Present, a: &[u8], b: &[u8]) -> bool {
    let texel = match p.format {
        9..=14 => 8,
        _ => 4,
    };
    (0..p.height as usize).all(|y| {
        let from = y * p.pitch as usize;
        let to = from + p.width as usize * texel;
        a.get(from..to) == b.get(from..to)
    })
}

fn to_rect(r: [i32; 4]) -> RECT {
    RECT {
        left: r[0],
        top: r[1],
        right: r[2],
        bottom: r[3],
    }
}

/// # Safety
/// An open list; barriers of live resources.
unsafe fn barriers(list: &ID3D12GraphicsCommandList, all: &[D3D12_RESOURCE_BARRIER]) {
    for some in all.chunks(1000) {
        // SAFETY: the caller's.
        unsafe { list.ResourceBarrier(some) };
    }
}

fn uav_barrier(res: Option<&ID3D12Resource>) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                // SAFETY: a borrowed pointer (or none) never released.
                pResource: res.map_or(std::mem::ManuallyDrop::new(None), |r| unsafe {
                    std::mem::transmute_copy(r)
                }),
            }),
        },
    }
}

/// Any placed resource to any other: waits for all writes.
fn alias() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Aliasing: std::mem::ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                pResourceBefore: std::mem::ManuallyDrop::new(None),
                pResourceAfter: std::mem::ManuallyDrop::new(None),
            }),
        },
    }
}

fn read_at(file: &File, mut to: &mut [u8], mut at: u64) -> std::io::Result<()> {
    while !to.is_empty() {
        let n = file.seek_read(to, at)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        to = &mut to[n..];
        at += n as u64;
    }
    Ok(())
}

fn make_device(frame: &Frame, debug: bool, breadcrumbs: bool) -> Result<(ID3D12Device, Option<ID3D12InfoQueue>)> {
    // SAFETY: plain creation and queries.
    unsafe {
        if debug {
            let mut layer: Option<ID3D12Debug> = None;
            match D3D12GetDebugInterface(&mut layer) {
                Ok(()) => layer.iter().for_each(|l| {
                    l.EnableDebugLayer();
                    // Every shader access checked on the card: very slow.
                    if std::env::var_os("VICTORY_REPLAY_GBV").is_some()
                        && let Ok(l) = l.cast::<ID3D12Debug1>()
                    {
                        l.SetEnableGPUBasedValidation(true);
                    }
                }),
                Err(e) => tracing::warn!("no debug layer: {e}"),
            }
        }
        if breadcrumbs {
            let mut dred: Option<ID3D12DeviceRemovedExtendedDataSettings> = None;
            D3D12GetDebugInterface(&mut dred)?;
            if let Some(dred) = dred {
                dred.SetAutoBreadcrumbsEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
                dred.SetPageFaultEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
            }
        }
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let mut adapter = None;
        for i in 0.. {
            let Ok(a) = factory.EnumAdapters1(i) else {
                break;
            };
            let d = a.GetDesc1()?;
            if (d.VendorId, d.DeviceId) == (frame.vendor, frame.device) {
                adapter = Some(a);
                break;
            }
        }
        if adapter.is_none() {
            tracing::warn!(
                "the adapter recorded on ({}) is not here: the default one is used",
                frame.adapter
            );
        }
        let mut device: Option<ID3D12Device> = None;
        match &adapter {
            Some(a) => D3D12CreateDevice(a, D3D_FEATURE_LEVEL_12_0, &mut device)?,
            None => D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device)?,
        }
        let device = device.context("no device")?;
        let info = device.cast::<ID3D12InfoQueue>().ok();
        Ok((device, info))
    }
}

/// The debug layer's errors: how many of each kind, and its first text.
fn debug_messages(q: &ID3D12InfoQueue) -> Vec<String> {
    let mut seen: BTreeMap<i32, (u32, String)> = BTreeMap::new();
    // SAFETY: messages read into buffers of the size asked for.
    unsafe {
        for i in 0..q.GetNumStoredMessages() {
            let mut size = 0;
            if q.GetMessage(i, None, &mut size).is_err() {
                continue;
            }
            let mut buffer = vec![0u64; size.div_ceil(8)];
            let message = buffer.as_mut_ptr().cast::<D3D12_MESSAGE>();
            if q.GetMessage(i, Some(message), &mut size).is_err() {
                continue;
            }
            let m = &*message;
            if m.Severity.0 <= D3D12_MESSAGE_SEVERITY_ERROR.0 {
                let text = std::slice::from_raw_parts(m.pDescription, m.DescriptionByteLength);
                let text = || {
                    String::from_utf8_lossy(text)
                        .trim_end_matches('\0')
                        .to_owned()
                };
                seen.entry(m.ID.0).or_insert_with(|| (0, text())).0 += 1;
            }
        }
        q.ClearStoredMessages();
    }
    seen.into_iter()
        .map(|(id, (n, text))| format!("debug layer: {n} errors {id}, first: {text}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_are_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find(dir.path()).is_err());
        for n in [1, 2, 10, 11] {
            std::fs::create_dir(dir.path().join(format!("capture-{n}"))).unwrap();
        }
        for n in [1, 2, 10] {
            std::fs::write(dir.path().join(format!("capture-{n}/frame.bin")), "").unwrap();
        }
        // The last complete one; itself when named.
        assert_eq!(find(dir.path()).unwrap(), dir.path().join("capture-10"));
        assert_eq!(
            find(&dir.path().join("capture-2")).unwrap(),
            dir.path().join("capture-2")
        );
    }

    #[test]
    fn reports() {
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        let r = Report {
            frame_ms: vec![10.0, 12.0, 11.0],
            busy_ms: vec![9.0; 3],
            delta_ms: vec![0.5; 3],
            submits: vec![(2, 9.0)],
            picture: Some((40.0, false)),
            ..Default::default()
        };
        let t = r.text("a frame");
        assert!(t.contains("a frame: 11.00 ms"), "{t}");
        assert!(t.contains("between submissions 1.50 ms"), "{t}");
        assert!(t.contains("PSNR 40.0 dB"), "{t}");
        let o = Options {
            skip_clamps: true,
            buffers: Buffers::Card,
            ..Default::default()
        };
        assert!(o.edited() && !o.plain().edited() && o.plain().buffers == Buffers::Card);
    }
}