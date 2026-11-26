//! What the passes of a frame read and write, and what textures hold after a
//! command: `--flow` and `--images`.

use super::*;

/// The bytes of a view's description as words.
fn word(s: &Slot, i: usize) -> u32 {
    u32::from_le_bytes([s.desc[i * 4], s.desc[i * 4 + 1], s.desc[i * 4 + 2], s.desc[i * 4 + 3]])
}

/// A heap slot, where a frame's resources are known by their numbers only.
fn slot_text(s: &Slot, called: &dyn Fn(u32) -> String) -> String {
    let kind = ["empty", "cbv", "srv", "uav", "sampler", "rtv", "dsv"]
        .get(s.kind as usize)
        .copied()
        .unwrap_or("?");
    let res = if s.res == NONE { "null".into() } else { called(s.res) };
    match s.kind {
        slot::EMPTY => "never written".into(),
        slot::CBV => format!("cbv {res} +{:#x} size {:#x}", s.address(), word(s, 2)),
        slot::SAMPLER => format!(
            "sampler filter {:#x} address {} {} {} bias {} compare {} lod {}..{}",
            word(s, 0),
            word(s, 1),
            word(s, 2),
            word(s, 3),
            f32::from_bits(word(s, 4)),
            word(s, 6),
            f32::from_bits(word(s, 11)),
            f32::from_bits(word(s, 12)),
        ),
        _ if s.described == 0 => format!("{kind} of all: {res}"),
        slot::SRV if word(s, 1) == 1 => format!(
            "srv f{} first {} count {} stride {} flags {}: {res}",
            word(s, 0),
            word(s, 4),
            word(s, 6),
            word(s, 7),
            word(s, 8)
        ),
        slot::UAV if word(s, 1) == 1 => format!(
            "uav f{} first {} count {} stride {} flags {}: {res}",
            word(s, 0),
            word(s, 2),
            word(s, 4),
            word(s, 5),
            word(s, 8)
        ),
        slot::SRV => format!(
            "srv f{} dim {} [{} {} {} {} {}]: {res}",
            word(s, 0),
            word(s, 1),
            word(s, 4),
            word(s, 5),
            word(s, 6),
            word(s, 7),
            word(s, 8)
        ),
        _ => format!(
            "{kind} f{} dim {} [{} {} {} {}]: {res}",
            word(s, 0),
            word(s, 1),
            word(s, 2),
            word(s, 3),
            word(s, 4),
            word(s, 5)
        ),
    }
}

/// What the lists' dispatches (with `draws`: draws too) have bound: pipeline,
/// root arguments, and what lies in the heap where a root constant that is a
/// place in it (32 bytes a slot) leads, `most` slots of each. Read from the
/// capture alone; no device is made.
pub fn flow(
    folder: &Path,
    lists: (u32, u32),
    draws: bool,
    most: u32,
    line: &mut dyn FnMut(&str),
) -> Result<()> {
    let frame = Frame::read(&find(folder)?.join("frame.bin"))?;
    let pipelines: HashMap<u32, &wire::Pipeline> = frame.pipelines.iter().map(|p| (p.id, p)).collect();
    let resources: HashMap<u32, &wire::Resource> = frame.resources.iter().map(|r| (r.id, r)).collect();
    let called = |id: u32| match resources.get(&id) {
        None => format!("#{id} (not there)"),
        Some(r) if r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 => {
            format!("#{id} '{}' buffer {:#x}", r.name, r.desc.width)
        }
        Some(r) => format!(
            "#{id} '{}' {}x{}x{} m{} f{}",
            r.name, r.desc.width, r.desc.height, r.desc.depth, r.desc.mips, r.desc.format
        ),
    };
    let heaps: HashMap<u32, &wire::DescriptorHeap> =
        frame.descriptor_heaps.iter().map(|h| (h.id, h)).collect();
    let mut late: HashMap<(u32, u32), Slot> = HashMap::new();
    let mut place = 0;
    for (i, s) in frame.submits.iter().enumerate() {
        for (heap, at, slot) in &s.views {
            late.insert((*heap, *at), *slot);
        }
        let slot_at = |heap: u32, at: u32| -> Slot {
            if let Some(s) = late.get(&(heap, at)) {
                return *s;
            }
            let Some(h) = heaps.get(&heap) else {
                return Slot::EMPTY;
            };
            let after = h.runs.partition_point(|r| r.0 <= at);
            match after.checked_sub(1).map(|n| &h.runs[n]) {
                Some((first, count, slot)) if at - first < *count => *slot,
                _ => Slot::EMPTY,
            }
        };
        for &l in s.lists.iter().filter(|&&l| l != MISSING) {
            let here = place;
            place += 1;
            if here < lists.0 || here > lists.1 {
                continue;
            }
            line(&format!("list {here} of submission {i}"));
            let mut pso = NONE;
            let mut compute = false;
            let mut heap = NONE;
            let mut binds: BTreeMap<(bool, u32), (String, Vec<u32>)> = BTreeMap::new();
            let mut targets = String::new();
            for (n, c) in Cmd::all(&frame.lists[l as usize].commands.0)?.iter().enumerate() {
                match c {
                    Cmd::Pipeline { pso: p } => {
                        pso = *p;
                        compute = pipelines.get(p).is_some_and(|p| {
                            p.parts.iter().any(|part| {
                                matches!(part, wire::Part::Code { kind, .. } if *kind == D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CS.0)
                            })
                        });
                    }
                    Cmd::RootSignature { graphics, .. } => binds.retain(|k, _| k.0 != *graphics),
                    Cmd::Heaps { heaps } => heap = heaps.first().copied().unwrap_or(NONE),
                    Cmd::RootView { graphics, kind, param, va } => {
                        let kind = ["?", "cbv", "srv", "uav"].get(*kind as usize).copied().unwrap_or("?");
                        let text = match va {
                            Va::At { res, offset } => format!("{kind} {} +{offset:#x}", called(*res)),
                            Va::Null {} => format!("{kind} null"),
                            Va::Unknown { address } => format!("{kind} unknown {address:#x}"),
                        };
                        binds.insert((*graphics, *param), (text, Vec::new()));
                    }
                    Cmd::Table { graphics, param, heap, slot } => {
                        binds.insert(
                            (*graphics, *param),
                            (format!("table heap {heap} slot {slot}"), Vec::new()),
                        );
                    }
                    Cmd::Constants { graphics, param, offset, values } => {
                        let (_, all) = binds
                            .entry((*graphics, *param))
                            .or_insert_with(|| ("constants".into(), Vec::new()));
                        let end = *offset as usize + values.len();
                        if all.len() < end {
                            all.resize(end, 0);
                        }
                        all[*offset as usize..end].copy_from_slice(values);
                    }
                    Cmd::Targets { colors, depth } => {
                        targets = colors
                            .iter()
                            .chain(depth)
                            .map(|s| slot_text(s, &called))
                            .collect::<Vec<_>>()
                            .join("; ");
                    }
                    Cmd::CopyTexture { .. }
                    | Cmd::CopyResource { .. }
                    | Cmd::Resolve { .. }
                    | Cmd::ClearDepth { .. }
                    | Cmd::ClearTarget { .. } => line(&format!("{here}:{n} {c:?}")),
                    c if c.runs_shaders() && (compute || draws) => {
                        let name = pipelines.get(&pso).map_or("", |p| p.name.as_str());
                        line(&format!("{here}:{n} {c:?} pso {pso} {name}"));
                        if !compute {
                            line(&format!("  targets {targets}"));
                        }
                        for ((graphics, param), (text, values)) in &binds {
                            if *graphics == compute {
                                continue;
                            }
                            if values.is_empty() {
                                line(&format!("  {param}: {text}"));
                                continue;
                            }
                            line(&format!("  {param}: {text} {values:x?}"));
                            for (k, v) in values.iter().enumerate() {
                                if v & 31 != 0
                                    || v >> 5 < 64
                                    || heaps.get(&heap).is_none_or(|h| v >> 5 >= h.count)
                                {
                                    continue;
                                }
                                for m in 0..most {
                                    let s = slot_at(heap, (v >> 5) + m);
                                    if s.kind != slot::EMPTY {
                                        let text = slot_text(&s, &called);
                                        line(&format!("    [{k}]+{m} slot {}: {text}", (v >> 5) + m));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn half(b: &[u8]) -> f32 {
    let h = u32::from(u16::from_le_bytes([b[0], b[1]]));
    small(h & 0x7fff, 10) * if h >> 15 == 1 { -1.0 } else { 1.0 }
}

/// A float of five exponent bits and `bits` of mantissa.
fn small(v: u32, bits: u32) -> f32 {
    let m = (v & ((1 << bits) - 1)) as f32 / (1u32 << bits) as f32;
    match v >> bits & 31 {
        0 => m * 2f32.powi(-14),
        31 if m > 0.0 => f32::NAN,
        31 => f32::INFINITY,
        e => (1.0 + m) * 2f32.powi(e as i32 - 15),
    }
}

/// Bytes of a texel that [`texel`] knows.
fn texel_bytes(format: i32) -> Option<usize> {
    Some(match format {
        1..=4 => 16,
        5..=8 => 12,
        9..=22 => 8,
        23..=47 | 67 | 87..=93 => 4,
        48..=59 => 2,
        60..=65 => 1,
        _ => return None,
    })
}

/// A texel as numbers (typeless formats as their usual ones; R32 as a
/// number as R9G9B9E5, which is what it mostly stands for).
fn texel(format: i32, b: &[u8]) -> [f32; 4] {
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let f32_at = |i: usize| f32::from_bits(u32_at(i));
    let u16_at = |i: usize| f32::from(u16::from_le_bytes([b[i], b[i + 1]]));
    let u8_at = |i: usize| f32::from(b[i]);
    match format {
        1 | 2 => [f32_at(0), f32_at(4), f32_at(8), f32_at(12)],
        3 | 4 => [u32_at(0) as f32, u32_at(4) as f32, u32_at(8) as f32, u32_at(12) as f32],
        5..=8 => [f32_at(0), f32_at(4), f32_at(8), 1.0],
        9 | 10 => [half(b), half(&b[2..]), half(&b[4..]), half(&b[6..])],
        11 => [0, 2, 4, 6].map(|i| u16_at(i) / 65535.0),
        12..=14 => [0, 2, 4, 6].map(u16_at),
        15 | 16 => [f32_at(0), f32_at(4), 0.0, 1.0],
        17 | 18 => [u32_at(0) as f32, u32_at(4) as f32, 0.0, 1.0],
        19..=22 => [f32_at(0), (u32_at(4) & 255) as f32, 0.0, 1.0],
        23..=25 => {
            let v = u32_at(0);
            [
                (v & 1023) as f32 / 1023.0,
                (v >> 10 & 1023) as f32 / 1023.0,
                (v >> 20 & 1023) as f32 / 1023.0,
                (v >> 30) as f32 / 3.0,
            ]
        }
        26 => {
            let v = u32_at(0);
            [small(v & 2047, 6), small(v >> 11 & 2047, 6), small(v >> 22, 5), 1.0]
        }
        27..=29 | 31 | 87..=93 => [0, 1, 2, 3].map(|i| u8_at(i) / 255.0),
        30 | 32 => [0, 1, 2, 3].map(u8_at),
        33 | 34 => [half(b), half(&b[2..]), 0.0, 1.0],
        35 => [u16_at(0) / 65535.0, u16_at(2) / 65535.0, 0.0, 1.0],
        36..=38 => [u16_at(0), u16_at(2), 0.0, 1.0],
        39..=41 => [f32_at(0); 4],
        42 | 43 | 67 => {
            let v = u32_at(0);
            let scale = 2f32.powi((v >> 27) as i32 - 24);
            [(v & 511) as f32 * scale, (v >> 9 & 511) as f32 * scale, (v >> 18 & 511) as f32 * scale, 1.0]
        }
        44..=47 => [(u32_at(0) & 0xff_ffff) as f32 / 16_777_215.0; 4],
        48 | 49 => [u8_at(0) / 255.0, u8_at(1) / 255.0, 0.0, 1.0],
        50..=52 => [u8_at(0), u8_at(1), 0.0, 1.0],
        53 | 54 => [half(b); 4],
        55 | 56 => [u16_at(0) / 65535.0; 4],
        57..=59 => [u16_at(0); 4],
        60 | 61 | 65 => [u8_at(0) / 255.0; 4],
        62..=64 => [u8_at(0); 4],
        _ => [0.0; 4],
    }
}

/// Which textures to save, and how.
#[derive(Clone, Debug, Default)]
pub struct Images {
    /// Resource and subresource; of none: all textures the frame's shaders
    /// write, from this many texels on.
    pub which: Vec<(u32, u32)>,
    pub least: u64,
    pub folder: PathBuf,
    /// The widest picture.
    pub width: u32,
    /// The values too (`.bin`: rows of texels without padding).
    pub raw: bool,
}

impl Player {
    /// Runs the frame once as the options say (`until` ends it early) and
    /// saves what textures hold, with their least, mean and greatest values.
    pub fn images(&mut self, o: &Options, images: &Images, say: &mut dyn FnMut(String)) -> Result<()> {
        let mut made = self.prepare(o)?;
        self.once(o, &mut made)?;
        drop(made);
        let mut which = images.which.clone();
        if which.is_empty() {
            let texels = |r: &wire::Resource| r.desc.width * u64::from(r.desc.height) * u64::from(r.desc.depth);
            which = self
                .frame
                .resources
                .iter()
                .filter(|r| {
                    r.desc.dimension != D3D12_RESOURCE_DIMENSION_BUFFER.0
                        && self.card_writes.contains(&r.id)
                        && texels(r) >= images.least
                })
                .map(|r| (r.id, 0))
                .collect();
        }
        std::fs::create_dir_all(&images.folder)?;
        for (id, sub) in which {
            let Some(at) = self.objects.get(&id).map(|o| o.index) else {
                say(format!("{id}: not there"));
                continue;
            };
            let r = &self.frame.resources[at];
            if r.desc.dimension == D3D12_RESOURCE_DIMENSION_BUFFER.0 {
                // A buffer: its bytes.
                if let Some(all) = self.contents_of(id, 0)? {
                    std::fs::write(images.folder.join(format!("{id:05}.bin")), &all)?;
                    say(format!("{}: {} bytes", self.called(id), all.len()));
                }
                continue;
            }
            if r.desc.samples[0] > 1 {
                say(format!("{id}: multisampled"));
                continue;
            }
            let (name, own) = (r.name.clone(), r.desc.format);
            let (f, _) = gpu::footprints(&self.device, &gpu::to_desc(&r.desc), sub, 1);
            let f = f[0].Footprint;
            let called = self.called(id);
            // The planes of depth formats come back as these.
            let format = if matches!(f.Format.0, 39 | 53 | 60) { f.Format.0 } else { own };
            let Some(bytes) = texel_bytes(format) else {
                say(format!("{called}.{sub}: format {format} is not shown"));
                continue;
            };
            let Some(all) = self.contents_of(id, sub)? else {
                continue;
            };
            let (mut least, mut most, mut sum) = ([f32::INFINITY; 4], [f32::NEG_INFINITY; 4], [0f64; 4]);
            let (mut zero, mut bad, mut n) = (0u64, 0u64, 0u64);
            let slice = f.RowPitch as usize * f.Height as usize;
            let mut raw = Vec::new();
            for y in 0..f.Height as usize * f.Depth as usize {
                let from = y * f.RowPitch as usize;
                let row = &all[from..from + f.Width as usize * bytes];
                if images.raw {
                    raw.extend_from_slice(row);
                }
                for t in row.chunks_exact(bytes) {
                    let v = texel(format, t);
                    n += 1;
                    if v.iter().any(|x| !x.is_finite()) {
                        bad += 1;
                        continue;
                    }
                    zero += u64::from(v[..3] == [0.0; 3]);
                    for c in 0..4 {
                        least[c] = least[c].min(v[c]);
                        most[c] = most[c].max(v[c]);
                        sum[c] += f64::from(v[c]);
                    }
                }
            }
            let mean = sum.map(|s| s / (n - bad).max(1) as f64);
            say(format!(
                "{called}.{sub}: {}x{}x{} least {least:.4?} mean {mean:.4?} greatest {most:.4?}, {:.1}% zero, {bad} not numbers",
                f.Width,
                f.Height,
                f.Depth,
                zero as f64 * 100.0 / n.max(1) as f64,
            ));
            let stem: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(40)
                .collect();
            let file = |end: String| images.folder.join(format!("{id:05}.{sub}_{stem}{end}"));
            if images.raw {
                std::fs::write(file(format!("_f{format}_{}x{}x{}.bin", f.Width, f.Height, f.Depth)), &raw)?;
            }
            // Values from 0 to 1 as they are, others bent so that the mean
            // is a middle grey. Slices of a 3D texture beside each other.
            let unit = most[..3].iter().all(|&m| m <= 1.0) && least[..3].iter().all(|&m| m >= 0.0);
            let grey = ((mean[0] + mean[1] + mean[2]) / 3.0).max(1e-9) as f32;
            let slices = f.Depth.clamp(1, 16);
            let step = (f.Width * slices).div_ceil(images.width.max(64)).max(1);
            let (w, h) = ((f.Width / step).max(1), (f.Height / step).max(1));
            let mut rgb = Vec::with_capacity((w * slices * h * 3) as usize);
            for y in 0..h as usize {
                for s in 0..slices as usize {
                    let z = s * f.Depth as usize / slices as usize;
                    for x in 0..w as usize {
                        let from = z * slice + y * step as usize * f.RowPitch as usize + x * step as usize * bytes;
                        let v = texel(format, &all[from..from + bytes]);
                        for c in &v[..3] {
                            let c = match c.max(0.0) {
                                _ if c.is_nan() => 1.0,
                                c if unit => c,
                                c => c / (c + grey),
                            };
                            rgb.push((c.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8);
                        }
                    }
                }
            }
            std::fs::write(file(".png".into()), crate::capture::png(w * slices, h, &rgb))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texels() {
        assert_eq!(texel(10, &[0, 0x3c, 0, 0xc0, 0, 0, 0, 0x38]), [1.0, -2.0, 0.0, 0.5]);
        assert_eq!(texel(26, &(15u32 << 6 | 16 << 17 | 14 << 27).to_le_bytes()), [1.0, 2.0, 0.5, 1.0]);
        assert_eq!(texel(67, &(16u32 << 27 | 256).to_le_bytes()), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(texel(24, &(1023u32 << 10).to_le_bytes()), [0.0, 1.0, 0.0, 0.0]);
        assert!(texel(54, &[1, 0x7c])[0].is_nan());
    }
}

/// `--bytes <resource>[:<at>:<len>]`: what the capture holds of a buffer, read
/// from the capture alone (no device is made): its contents at the start of
/// the frame and what the CPU had changed in it by each submission, each
/// with how many of its dwords are not zero, and the dwords from `at` (as
/// hex and as floats).
pub fn bytes(folder: &Path, what: &[(u32, u64, u64)], line: &mut dyn FnMut(&str)) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let folder = find(folder)?;
    let frame = Frame::read(&folder.join("frame.bin"))?;
    let mut contents = File::open(folder.join("contents.bin"))?;
    let mut deltas = File::open(folder.join("deltas.bin"))?;
    let mut piece = |file: &mut File, from: u64, len: u64, first: u64, at: u64, most: u64, zero: bool| -> Result<String> {
        let mut all = vec![0u8; len as usize];
        if !zero {
            file.seek(SeekFrom::Start(from))?;
            file.read_exact(&mut all)?;
        }
        let dwords: Vec<u32> = all.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let set = dwords.iter().filter(|&&d| d != 0).count();
        let mut text = format!("{set} of {} dwords not zero", dwords.len());
        // The window asked for, where this piece holds it.
        let (lo, hi) = (at.max(first), (at + most).min(first + len));
        if lo < hi {
            let shown = &dwords[((lo - first) / 4) as usize..((hi - first).div_ceil(4) as usize).min(dwords.len())];
            let set = shown.iter().filter(|&&d| d != 0).count();
            text += &format!("; at {lo:#x}+{:#x}: {set} of {} not zero:", hi - lo, shown.len());
            for d in shown.iter().take(48) {
                text += &format!(" {d:08x}({:.4})", f32::from_bits(*d));
            }
        }
        Ok(text)
    };
    for &(res, at, most) in what {
        match frame.resources.iter().find(|r| r.id == res) {
            Some(r) => line(&format!("#{res} '{}' {:#x} bytes, {:?}, idle {}", r.name, r.desc.width, r.place, r.idle)),
            None => line(&format!("#{res} is not in the capture")),
        }
        for c in frame.contents.iter().filter(|c| c.res == res) {
            let text = piece(&mut contents, c.file, c.len, c.first, at, most, c.file == wire::ZERO)?;
            line(&format!("  at the frame's start {:#x}+{:#x}: {text}", c.first, c.len));
        }
        for (i, s) in frame.submits.iter().enumerate() {
            for d in s.deltas.iter().filter(|d| d.res == res) {
                let text = piece(&mut deltas, d.file, d.len, d.at, at, most, false)?;
                line(&format!("  by submission {i} the CPU's side {:#x}+{:#x}: {text}", d.at, d.len));
            }
        }
    }
    Ok(())
}
