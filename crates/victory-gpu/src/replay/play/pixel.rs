//! What draws a pixel: `--pixel`. After every command that can change a
//! texture, the texel under the pixel is copied out of it (of a draw: of its
//! targets, where the pixel lies in its viewport, and of the textures shaders
//! may write; of a dispatch: of those; of a copy, clear or resolve: of where
//! it goes), and commands that changed one are named with the values before
//! and after.

use super::edit::{PROBE, SCRATCH};
use super::*;

/// What to look at.
#[derive(Clone, Debug, Default)]
pub struct Pixels {
    /// Points of the presented picture.
    pub points: Vec<(u32, u32)>,
    /// Texels of textures: resource, subresource, place.
    pub texels: Vec<(u32, u32, u32, u32)>,
    /// Only these lists are looked into.
    pub lists: Option<(u32, u32)>,
    /// Depth targets too (a copy of the whole target for each draw).
    pub depth: bool,
    /// Textures shaders may write too.
    pub written: bool,
    /// Indirect draws that are run one by one: list, command.
    pub split: Vec<(u32, u32)>,
}

/// Bytes a sample has.
const ROOM: u64 = 512;
/// The most samples of one run.
const MOST: usize = 1 << 20;

#[derive(Clone, Debug)]
struct Sample {
    list: u32,
    command: usize,
    /// Of a split draw: which one.
    part: u32,
    before: bool,
    spot: Spot,
    pso: u32,
    what: &'static str,
}

/// A texture's part that a sample can be taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Spot {
    res: u32,
    sub: u32,
    x: u32,
    y: u32,
    z: u32,
    /// The point of the picture it stands for (or [`OWN`]).
    point: usize,
    format: i32,
    depth: bool,
}

/// In [`Spot::point`]: a texel asked for by itself.
const OWN: usize = usize::MAX;

/// The texel of a target of `size` that shows the point `(x, y)` of a picture
/// of `picture`, drawn through `view` (x, y, width, height): none if the
/// target has another shape than the picture.
fn texel(
    (x, y): (u32, u32),
    picture: (u32, u32),
    view: [f32; 4],
    size: (u32, u32),
) -> Option<(u32, u32)> {
    let (w, h) = (picture.0 as f32, picture.1 as f32);
    if !same_shape((view[2], view[3]), (w, h)) || view[2] < 16.0 {
        return None;
    }
    let a = (view[0] + (x as f32 + 0.5) / w * view[2]).floor();
    let b = (view[1] + (y as f32 + 0.5) / h * view[3]).floor();
    (a >= 0.0 && b >= 0.0 && a < size.0 as f32 && b < size.1 as f32).then_some((a as u32, b as u32))
}

fn same_shape(a: (f32, f32), b: (f32, f32)) -> bool {
    (0.97..1.03).contains(&(a.0 * b.1 / (a.1 * b.0).max(1e-6)))
}

/// A float from a small one's bits (5 bits of exponent, `mantissa` of
/// mantissa, no sign).
fn small(bits: u32, mantissa: u32) -> f32 {
    let (e, m) = (
        bits >> mantissa & 31,
        (bits & ((1 << mantissa) - 1)) as f32,
    );
    let unit = (1u32 << mantissa) as f32;
    match e {
        0 => m / unit * 2f32.powi(-14),
        31 if m == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + m / unit) * 2f32.powi(e as i32 - 15),
    }
}

/// Bytes of a texel of a format; 0 for formats not read.
fn texel_bytes(format: i32) -> usize {
    match format {
        1..=4 => 16,
        5..=8 => 12,
        9..=22 => 8,
        23..=47 => 4,
        48..=59 => 2,
        60..=65 => 1,
        67 | 85..=93 => 4,
        _ => 0,
    }
}

/// A texel in words.
fn texel_text(format: i32, b: &[u8]) -> String {
    let n = texel_bytes(format);
    if n == 0 || b.len() < n {
        return format!("{b:02x?}");
    }
    let word = |i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    let float = |i: usize| f32::from_bits(word(i));
    let short = |i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
    let half = |i: usize| {
        let v = small(u32::from(short(i) & 0x7FFF), 10);
        if short(i) >> 15 != 0 { -v } else { v }
    };
    let unorm16 = |i: usize| f32::from(short(i)) / 65535.0;
    let snorm16 = |i: usize| f32::from(short(i) as i16) / 32767.0;
    let unorm8 = |i: usize| f32::from(b[i]) / 255.0;
    let snorm8 = |i: usize| f32::from(b[i] as i8) / 127.0;
    let all: Vec<f32> = match format {
        1 | 2 => (0..4).map(float).collect(),
        5 | 6 => (0..3).map(float).collect(),
        9 | 10 => (0..4).map(half).collect(),
        11 => (0..4).map(unorm16).collect(),
        13 => (0..4).map(snorm16).collect(),
        15 | 16 => (0..2).map(float).collect(),
        23 | 24 => vec![
            (word(0) & 1023) as f32 / 1023.0,
            (word(0) >> 10 & 1023) as f32 / 1023.0,
            (word(0) >> 20 & 1023) as f32 / 1023.0,
            (word(0) >> 30) as f32 / 3.0,
        ],
        26 => vec![
            small(word(0) & 2047, 6),
            small(word(0) >> 11 & 2047, 6),
            small(word(0) >> 22, 5),
        ],
        27..=29 | 87 | 90 | 91 => (0..4).map(unorm8).collect(),
        31 => (0..4).map(snorm8).collect(),
        33 | 34 => (0..2).map(half).collect(),
        35 => (0..2).map(unorm16).collect(),
        37 => (0..2).map(snorm16).collect(),
        39..=41 => vec![float(0)],
        48 | 49 => (0..2).map(unorm8).collect(),
        51 => (0..2).map(snorm8).collect(),
        53 | 54 => vec![half(0)],
        55 | 56 => vec![unorm16(0)],
        58 => vec![snorm16(0)],
        60 | 61 => vec![unorm8(0)],
        63 => vec![snorm8(0)],
        _ => Vec::new(),
    };
    let all: Vec<String> = all.iter().map(|v| format!("{v:.5}")).collect();
    format!("{} {:02x?}", all.join(" "), &b[..n]).trim().to_owned()
}

/// One draw of an indirect call that has many.
fn part_of(c: &Cmd, i: u32, stride: u32) -> Cmd {
    match c {
        Cmd::Indirect {
            signature,
            args,
            at,
            ..
        } => Cmd::Indirect {
            signature: *signature,
            most: 1,
            args: *args,
            at: at + u64::from(i) * u64::from(stride),
            count: NONE,
            count_at: 0,
        },
        c => c.clone(),
    }
}

fn transition(res: u32, sub: u32, before: u32, after: u32) -> Barrier {
    Barrier::Transition {
        flags: 0,
        res,
        sub,
        before: before as i32,
        after: after as i32,
    }
}

/// What is known of the frame's textures.
struct Textures {
    /// Formats copies have.
    copied_as: HashMap<u32, i32>,
    /// Textures shaders may write and that have the picture's shape.
    shaped: Vec<u32>,
    /// The viewport targets of a size were last drawn through.
    drawn: HashMap<(u32, u32), [f32; 4]>,
}

impl Player {
    fn texture(&self, id: u32) -> Option<&wire::Resource> {
        let o = self.objects.get(&id)?;
        let r = &self.frame.resources[o.index];
        (r.desc.dimension != D3D12_RESOURCE_DIMENSION_BUFFER.0 && r.desc.samples[0] <= 1)
            .then_some(r)
    }

    /// What a view of a target is of: subresource, depth slice, size, format.
    fn seen_by(&self, s: &Slot) -> Option<(u32, u32, (u32, u32), i32)> {
        let r = self.texture(s.res)?;
        let word = |i: usize| {
            u32::from_le_bytes([
                s.desc[i * 4],
                s.desc[i * 4 + 1],
                s.desc[i * 4 + 2],
                s.desc[i * 4 + 3],
            ])
        };
        let (mut mip, mut slice, mut z, mut format) = (0, 0, 0, 0);
        if s.described != 0 {
            format = word(0) as i32;
            match (s.kind, word(1) as i32) {
                // 2D, 2D array and 3D views.
                (slot::RTV, 4) => mip = word(2),
                (slot::RTV, 5) => (mip, slice) = (word(2), word(3)),
                (slot::RTV, 8) => (mip, z) = (word(2), word(3)),
                (slot::DSV, 3) => mip = word(3),
                (slot::DSV, 4) => (mip, slice) = (word(3), word(4)),
                _ => return None,
            }
        }
        if format == 0 {
            format = r.desc.format;
        }
        let mips = u32::from(r.desc.mips.max(1));
        let mip = mip.min(mips - 1);
        let size = (
            (r.desc.width as u32 >> mip).max(1),
            (r.desc.height >> mip).max(1),
        );
        Some((mip + slice * mips, z, size, format))
    }

    /// Width and height of the presented picture.
    pub fn picture_size(&self) -> Option<(u32, u32)> {
        self.frame.presents.last().map(|p| (p.width, p.height))
    }

    /// Says which commands change the texels under points of the picture.
    pub fn pixel(&mut self, o: &Options, p: &Pixels, say: &mut dyn FnMut(String)) -> Result<()> {
        let picture = self.picture_size().context("no picture was presented")?;
        self.set_merging(o.merge)?;
        let mut report = Report::default();
        let submits = self.edited(o, &mut report, &[]);
        let counts = self.counts_of(o, p, &submits)?;
        let mut samples: Vec<Sample> = Vec::new();
        let made = self.sampled(p, &submits, picture, &counts, &mut samples)?;
        let bytes = self.probed(o, &made, samples.len() as u64 * ROOM)?;
        say(format!(
            "{} texels taken; picture {}x{}; points {:?}",
            samples.len(),
            picture.0,
            picture.1,
            p.points
        ));
        report.notes.drain(..).for_each(&mut *say);
        let mut last: HashMap<(u32, u32, (u32, u32), usize), Vec<u8>> = HashMap::new();
        let mut said = None;
        for (i, s) in samples.iter().enumerate() {
            let t = &s.spot;
            let n = texel_bytes(t.format).max(1);
            let now = bytes[i * ROOM as usize..][..n].to_vec();
            let was = last.insert((t.res, t.sub, (t.x, t.y), t.point), now.clone());
            if was.as_ref() == Some(&now) {
                continue;
            }
            let point = match t.point {
                OWN => String::new(),
                n => format!("point {n} "),
            };
            let spot = format!(
                "    {point}{} sub {} at {},{}:",
                self.called(t.res),
                t.sub,
                t.x,
                t.y
            );
            let now = texel_text(t.format, &now);
            if s.before {
                // Seen for the first time, or changed by what is not looked
                // into.
                if was.is_some() {
                    say(format!("{spot} changed unseen to {now}"));
                }
                continue;
            }
            if said != Some((s.list, s.command, s.part)) {
                said = Some((s.list, s.command, s.part));
                let part = match p.split.contains(&(s.list, s.command as u32)) {
                    true => format!(" draw {}", s.part),
                    false => String::new(),
                };
                let of = match s.pso {
                    NONE => String::new(),
                    pso => format!(" of {pso} {}", self.pipeline_name(pso)),
                };
                say(format!("{}:{}{part} {}{of}", s.list, s.command, s.what));
            }
            let was = was.map_or("?".to_owned(), |w| texel_text(t.format, &w));
            say(format!("{spot} {was} -> {now}"));
        }
        Ok(())
    }

    /// Runs the frame with commands that copy to [`PROBE`], which is of
    /// `len` bytes; its bytes after that.
    fn probed(&mut self, o: &Options, submits: &[Vec<Vec<Cmd>>], len: u64) -> Result<Vec<u8>> {
        let (back, data) = gpu::buffer(
            &self.device,
            D3D12_HEAP_TYPE_READBACK,
            len.max(4),
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        self.extra.insert(PROBE, back);
        self.restore(o.restore)?;
        let built = self.build(submits, o)?;
        self.fresh = false;
        self.send(&built, o)?;
        // SAFETY: the mapping; the copies into it have run.
        let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec();
        self.extra.remove(&PROBE);
        Ok(bytes)
    }

    /// How many draws the calls to be split have.
    fn counts_of(
        &mut self,
        o: &Options,
        p: &Pixels,
        submits: &[Vec<Vec<Cmd>>],
    ) -> Result<HashMap<(u32, u32), u32>> {
        let mut counts = HashMap::new();
        if p.split.is_empty() {
            return Ok(counts);
        }
        const ARGS: u32 = 0x200;
        let mut made = submits.to_vec();
        let mut asked: Vec<(u32, u32)> = Vec::new();
        for (here, cmds) in made.iter_mut().flatten().enumerate() {
            let mut out = Vec::new();
            for (n, c) in std::mem::take(cmds).into_iter().enumerate() {
                let key = (here as u32, n as u32);
                if let Cmd::Indirect {
                    most,
                    count,
                    count_at,
                    ..
                } = c
                    && p.split.contains(&key)
                {
                    counts.insert(key, most);
                    if count != NONE {
                        out.push(Cmd::Barriers {
                            all: vec![transition(count, ALL, ARGS, COPY_SOURCE)],
                        });
                        out.push(Cmd::CopyBuffer {
                            dst: PROBE,
                            dst_at: asked.len() as u64 * 4,
                            src: count,
                            src_at: count_at,
                            len: 4,
                        });
                        out.push(Cmd::Barriers {
                            all: vec![transition(count, ALL, COPY_SOURCE, ARGS)],
                        });
                        asked.push(key);
                    }
                }
                out.push(c);
            }
            *cmds = out;
        }
        let bytes = self.probed(o, &made, asked.len() as u64 * 4)?;
        for (key, n) in asked.iter().zip(bytes.chunks_exact(4)) {
            if let Some(most) = counts.get_mut(key) {
                *most = (*most).min(u32::from_le_bytes([n[0], n[1], n[2], n[3]]));
            }
        }
        Ok(counts)
    }

    fn textures(&self, picture: (u32, u32)) -> Textures {
        let textures = || {
            self.frame
                .resources
                .iter()
                .filter(|r| self.texture(r.id).is_some())
        };
        let shaped = textures()
            .filter(|r| {
                let d = &r.desc;
                d.dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D.0
                    && d.flags & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0
                    && d.width as u32 * 4 >= picture.0
                    && same_shape(
                        (d.width as f32, d.height as f32),
                        (picture.0 as f32, picture.1 as f32),
                    )
            })
            .map(|r| r.id)
            .collect();
        let copied_as = textures()
            .map(|r| {
                let f = gpu::footprints(&self.device, &gpu::to_desc(&r.desc), 0, 1).0;
                (r.id, f[0].Footprint.Format.0)
            })
            .collect();
        Textures {
            copied_as,
            shaped,
            drawn: HashMap::new(),
        }
    }

    /// The spots of a texture (of its subresource `sub`) that is written not
    /// as a target.
    fn spots_of(
        &self,
        p: &Pixels,
        t: &Textures,
        picture: (u32, u32),
        id: u32,
        sub: u32,
        out: &mut Vec<Spot>,
    ) {
        let Some(r) = self.texture(id) else { return };
        let depth = r.desc.flags & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 != 0;
        for &(res, at, x, y) in &p.texels {
            if res == id && (sub == at || sub == ALL) {
                out.push(Spot {
                    res,
                    sub: at,
                    x,
                    y,
                    z: 0,
                    point: OWN,
                    format: r.desc.format,
                    depth,
                });
            }
        }
        if r.desc.dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D.0 || !(sub == 0 || sub == ALL) {
            return;
        }
        let size = (r.desc.width as u32, r.desc.height);
        let whole = [0.0, 0.0, size.0 as f32, size.1 as f32];
        let view = t.drawn.get(&size).copied().unwrap_or(whole);
        for (point, at) in p.points.iter().enumerate() {
            if let Some((x, y)) = texel(*at, picture, view, size) {
                out.push(Spot {
                    res: id,
                    sub: 0,
                    x,
                    y,
                    z: 0,
                    point,
                    format: r.desc.format,
                    depth,
                });
            }
        }
    }

    /// Whether a texture is the one in use of those that share its memory.
    fn in_use(&self, states: &States, id: u32) -> bool {
        self.objects.get(&id).is_some_and(|o| {
            !self.frame.resources[o.index].idle || states.used.contains_key(&id)
        })
    }

    /// The commands that copy the texels of `spots` to [`PROBE`] from `first`
    /// on; `copied`: a copy to them went before.
    #[allow(clippy::too_many_arguments)]
    fn take(
        &self,
        spots: &[Spot],
        first: u64,
        states: &States,
        t: &Textures,
        copied: bool,
        scratch: &mut u64,
        out: &mut Vec<Cmd>,
    ) {
        let (mut to, mut from, mut copies) = (Vec::new(), Vec::new(), Vec::new());
        let mut moved = HashSet::new();
        for (i, s) in spots.iter().enumerate() {
            let Some(r) = self.texture(s.res) else {
                continue;
            };
            let all = states.get(s.res).unwrap_or(vec![COMMON]);
            let mut state = all
                .get(s.sub as usize)
                .or(all.first())
                .copied()
                .unwrap_or(COMMON);
            if copied && state == COMMON {
                state = COPY_DEST;
            }
            if state & COPY_SOURCE == 0 && moved.insert((s.res, s.sub)) {
                to.push(transition(s.res, s.sub, state, COPY_SOURCE));
                from.push(transition(s.res, s.sub, COPY_SOURCE, state));
            }
            let offset = (first + i as u64) * ROOM;
            if !s.depth {
                copies.push(Cmd::CopyTexture {
                    dst: Loc::Placed {
                        res: PROBE,
                        offset,
                        format: t.copied_as.get(&s.res).copied().unwrap_or(r.desc.format),
                        size: [1, 1, 1],
                        pitch: 256,
                    },
                    at: [0; 3],
                    src: Loc::Sub {
                        res: s.res,
                        sub: s.sub,
                    },
                    part: Some([s.x, s.y, s.z, s.x + 1, s.y + 1, s.z + 1]),
                });
                continue;
            }
            // Depth is only copied whole.
            let mip = s.sub % u32::from(r.desc.mips.max(1));
            let w = (r.desc.width as u32 >> mip).max(1);
            let h = (r.desc.height >> mip).max(1);
            let bytes = texel_bytes(s.format) as u64;
            let pitch = (w * bytes as u32).next_multiple_of(256);
            *scratch = (*scratch).max(u64::from(pitch) * u64::from(h));
            copies.push(Cmd::CopyTexture {
                dst: Loc::Placed {
                    res: SCRATCH,
                    offset: 0,
                    format: t.copied_as.get(&s.res).copied().unwrap_or(r.desc.format),
                    size: [w, h, 1],
                    pitch,
                },
                at: [0; 3],
                src: Loc::Sub {
                    res: s.res,
                    sub: s.sub,
                },
                part: None,
            });
            copies.push(Cmd::Barriers {
                all: vec![transition(SCRATCH, ALL, COPY_DEST, COPY_SOURCE)],
            });
            copies.push(Cmd::CopyBuffer {
                dst: PROBE,
                dst_at: offset,
                src: SCRATCH,
                src_at: u64::from(pitch) * u64::from(s.y) + u64::from(s.x) * bytes,
                len: bytes,
            });
            copies.push(Cmd::Barriers {
                all: vec![transition(SCRATCH, ALL, COPY_SOURCE, COPY_DEST)],
            });
        }
        if !to.is_empty() {
            out.push(Cmd::Barriers { all: to });
        }
        out.extend(copies);
        if !from.is_empty() {
            out.push(Cmd::Barriers { all: from });
        }
    }

    /// The frame's lists with copies of texels after commands.
    fn sampled(
        &mut self,
        p: &Pixels,
        submits: &[Vec<Vec<Cmd>>],
        picture: (u32, u32),
        counts: &HashMap<(u32, u32), u32>,
        samples: &mut Vec<Sample>,
    ) -> Result<Vec<Vec<Vec<Cmd>>>> {
        let strides: HashMap<u32, u32> = self
            .frame
            .signatures
            .iter()
            .map(|s| (s.id, s.stride))
            .collect();
        let dispatching: HashSet<u32> = self
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
        let mut t = self.textures(picture);
        let mut states = self.start_states();
        let mut known: HashSet<Spot> = HashSet::new();
        let mut made = Vec::new();
        let mut here = 0;
        let mut scratch = 0u64;
        for lists in submits {
            let mut out_lists = Vec::new();
            for cmds in lists {
                let wanted = p.lists.is_none_or(|(a, b)| (a..=b).contains(&here));
                let mut out = Vec::with_capacity(cmds.len());
                let mut pso = NONE;
                let mut targets: Vec<Slot> = Vec::new();
                let mut view: Option<[f32; 4]> = None;
                for (n, c) in cmds.iter().enumerate() {
                    match c {
                        Cmd::Pipeline { pso: id } => pso = *id,
                        Cmd::Targets { colors, depth } => {
                            targets = colors.iter().chain(depth).copied().collect();
                        }
                        Cmd::Viewports { all } => {
                            view = all.first().map(|v| [v[0], v[1], v[2], v[3]]);
                        }
                        _ => {}
                    }
                    let dispatches = matches!(c, Cmd::Dispatch { .. })
                        || matches!(c, Cmd::Indirect { signature, .. } if dispatching.contains(signature));
                    let draws = c.runs_shaders() && !dispatches;
                    let mut spots: Vec<Spot> = Vec::new();
                    let looked = wanted && c.works() && samples.len() < MOST;
                    for s in targets.iter().filter(|_| looked && draws) {
                        let Some((sub, z, size, format)) = self.seen_by(s) else {
                            continue;
                        };
                        let depth = s.kind == slot::DSV;
                        let spot = |x, y, point| Spot {
                            res: s.res,
                            sub,
                            x,
                            y,
                            z,
                            point,
                            format,
                            depth,
                        };
                        let view = view.unwrap_or([0.0, 0.0, size.0 as f32, size.1 as f32]);
                        for (point, at) in p.points.iter().enumerate() {
                            if let Some((x, y)) = texel(*at, picture, view, size) {
                                t.drawn.insert(size, view);
                                spots.push(spot(x, y, point));
                            }
                        }
                        for &(res, at, x, y) in &p.texels {
                            if (res, at) == (s.res, sub) {
                                spots.push(spot(x, y, OWN));
                            }
                        }
                    }
                    if looked && c.runs_shaders() {
                        let all = p.texels.iter().map(|t| t.0);
                        let some = p.written && (dispatches || !targets.is_empty());
                        let all: HashSet<u32> =
                            all.chain(t.shaped.iter().copied().filter(|_| some)).collect();
                        for id in all {
                            let writable = states
                                .get(id)
                                .is_some_and(|s| s.iter().any(|s| s & UNORDERED_ACCESS != 0));
                            if writable && self.in_use(&states, id) {
                                self.spots_of(p, &t, picture, id, ALL, &mut spots);
                            }
                        }
                    }
                    let mut copied = false;
                    if looked {
                        let mut to = None;
                        match c {
                            Cmd::CopyTexture {
                                dst: Loc::Sub { res, sub },
                                ..
                            } => to = Some((*res, *sub, true)),
                            Cmd::CopyResource { dst, .. } => to = Some((*dst, ALL, true)),
                            Cmd::Resolve { dst, dst_sub, .. }
                            | Cmd::ResolveRegion { dst, dst_sub, .. } => {
                                to = Some((*dst, *dst_sub, false));
                            }
                            Cmd::ClearTarget { view, .. } | Cmd::ClearDepth { view, .. } => {
                                to = self.seen_by(view).map(|s| (view.res, s.0, false));
                            }
                            _ => {}
                        }
                        if let Some((res, sub, is_copy)) = to {
                            copied = is_copy;
                            self.spots_of(p, &t, picture, res, sub, &mut spots);
                        }
                    }
                    let mut seen = HashSet::new();
                    spots.retain(|s| {
                        (p.depth || !s.depth) && texel_bytes(s.format) > 0 && seen.insert(*s)
                    });
                    let what = match c {
                        Cmd::Draw { .. } => "Draw",
                        Cmd::DrawIndexed { .. } => "DrawIndexed",
                        Cmd::DispatchMesh { .. } => "DispatchMesh",
                        Cmd::Dispatch { .. } => "Dispatch",
                        Cmd::Indirect { .. } if dispatches => "indirect Dispatch",
                        Cmd::Indirect { .. } => "indirect draw",
                        Cmd::ClearTarget { .. } | Cmd::ClearDepth { .. } => "clear",
                        Cmd::Resolve { .. } | Cmd::ResolveRegion { .. } => "resolve",
                        _ => "copy",
                    };
                    let runs = if c.runs_shaders() { pso } else { NONE };
                    let mut take = |before: bool, part: u32, spots: &[Spot], out: &mut Vec<Cmd>| {
                        let first = samples.len() as u64;
                        self.take(spots, first, &states, &t, copied && !before, &mut scratch, out);
                        samples.extend(spots.iter().map(|spot| Sample {
                            list: here,
                            command: n,
                            part,
                            before,
                            spot: *spot,
                            pso: runs,
                            what,
                        }));
                    };
                    // What was not seen before this command, as it is now.
                    let new: Vec<Spot> =
                        spots.iter().filter(|s| known.insert(**s)).copied().collect();
                    if !new.is_empty() {
                        take(true, 0, &new, &mut out);
                    }
                    match (counts.get(&(here, n as u32)), c) {
                        (Some(&count), Cmd::Indirect { signature, .. }) => {
                            let stride = strides.get(signature).copied().unwrap_or(0);
                            for i in 0..count {
                                out.push(part_of(c, i, stride));
                                take(false, i, &spots, &mut out);
                            }
                        }
                        _ => {
                            out.push(c.clone());
                            if !spots.is_empty() {
                                take(false, 0, &spots, &mut out);
                            }
                        }
                    }
                    let mut all = Vec::new();
                    states::changes(c, &mut all);
                    all.iter().for_each(|c| states.change(c));
                }
                out_lists.push(out);
                here += 1;
            }
            states.decay();
            made.push(out_lists);
        }
        if scratch > 0 {
            let buffer = gpu::buffer(
                &self.device,
                D3D12_HEAP_TYPE_DEFAULT,
                scratch,
                D3D12_RESOURCE_FLAG_NONE,
            )?;
            self.extra.insert(SCRATCH, buffer.0);
        }
        Ok(made)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_lie_in_viewports_of_the_pictures_shape() {
        let whole = |w: u32, h: u32| ([0.0, 0.0, w as f32, h as f32], (w, h));
        let picture = (3840, 2160);
        let (view, size) = whole(1920, 1080);
        assert_eq!(texel((3839, 2159), picture, view, size), Some((1919, 1079)));
        assert_eq!(texel((0, 0), picture, view, size), Some((0, 0)));
        let (view, size) = whole(2048, 2048);
        assert_eq!(texel((0, 0), picture, view, size), None);
        // A part of a larger target.
        let part = [0.0, 0.0, 1920.0, 1080.0];
        assert_eq!(
            texel((1920, 1080), picture, part, (3840, 2160)),
            Some((960, 540))
        );
        let beside = [1920.0, 0.0, 1920.0, 1080.0];
        assert_eq!(texel((1920, 1080), picture, beside, (1920, 1080)), None);
    }

    #[test]
    fn texels_are_read() {
        assert_eq!(
            texel_text(41, &1.5f32.to_le_bytes()),
            "1.50000 [00, 00, c0, 3f]"
        );
        assert_eq!(
            texel_text(28, &[255, 0, 0, 51]),
            "1.00000 0.00000 0.00000 0.20000 [ff, 00, 00, 33]"
        );
        assert!(
            texel_text(10, &[0, 0x3C, 0, 0xBC, 0, 0, 0, 0x38])
                .starts_with("1.00000 -1.00000 0.00000 0.50000 ")
        );
        let v = 0x3C0u32 | 0x380 << 11 | 0x200 << 22;
        assert!(texel_text(26, &v.to_le_bytes()).starts_with("1.00000 0.50000 2.00000 "));
        assert_eq!(texel_text(42, &[1, 2, 3, 4]), "[01, 02, 03, 04]");
        // Too few bytes, or a format that is not read.
        assert_eq!(texel_text(41, &[1]), "[01]");
        assert_eq!(texel_text(98, &[1, 2]), "[01, 02]");
    }

    #[test]
    fn an_indirect_draw_is_taken_apart() {
        let all = Cmd::Indirect {
            signature: 1,
            most: 8,
            args: 2,
            at: 100,
            count: 3,
            count_at: 4,
        };
        let one = Cmd::Indirect {
            signature: 1,
            most: 1,
            args: 2,
            at: 124,
            count: NONE,
            count_at: 0,
        };
        assert_eq!(part_of(&all, 2, 12), one);
    }
}
