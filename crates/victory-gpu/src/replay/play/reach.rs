//! `--root-reach`: how far the pre-skinned character mesh programs read
//! through their root SRV, against the view the runtime gave it.
//!
//! A root SRV has no length; the runtime binds it as a one-view table that
//! ends where the host copy holding its address ends (a title buffer's end,
//! or the 4 MiB line of a window of title memory). The mesh programs of
//! `MS_VS_PedTransformPreSkinD*` read every vertex stream through that view:
//! a header at the root address holds one word per stream, the stream's
//! byte offset from the root address in bits 9-31 (`(w >> 7) & 0x1fffffc`)
//! and its stride in bits 5-8 (`(w >> 3) & 60`); a vertex is read at
//! `offset + stride * index`, the index 16 bits from a remap stream the
//! program reads through a table (not looked at here). Which header words
//! a program reads differs between the variants (093c64546c55554e: 0x0-0x10,
//! 0x2c, 0x30, 0x38, 0x3c; 5bd95fdec765c0d6: 0x0-0x10, 0x30-0x3c, 0x48;
//! others 0x4c, 0x68, 0x78), so every word up to the dequantisation
//! constants at 0x60 is taken for a stream of up to 12 bytes a vertex: a
//! bound, not the program's exact reads.
//!
//! The header is read from the capture's contents and the CPU's writes up
//! to the draw's submission (not what shaders wrote). Read from the
//! capture, without the card.

use std::collections::HashMap;
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::path::Path;

use anyhow::Result;

use super::super::wire::{Cmd, Frame, MISSING, NONE, Slot, ZERO, slot};
use super::find;

/// Header words taken for stream words (bytes 0x0-0x5c), and the bytes one
/// vertex's read takes from a stream at most.
pub const WORDS: usize = 24;
const VERTEX_BYTES: u64 = 12;

/// The slots after the table at user-data dword 8 that hold the skinned
/// position buffers the mesh programs pick between by bits 24-25 of the range
/// word: 0 (descriptor at +0x420), anything else (+0xa80).
pub const SKINNED: [u32; 2] = [0x420 / 32, 0xa80 / 32];

/// A stream word's byte offset from the root address and its stride.
pub fn stream(word: u32) -> (u64, u64) {
    (u64::from((word >> 7) & 0x01ff_fffc), u64::from((word >> 3) & 60))
}

/// The furthest byte past the root address the streams of `header` reach:
/// with every index at most `last`, and with the vertex count the layout
/// gives (the position stream at word 0 ends where the next stream above
/// it begins; `None` if no stream lies above it).
pub fn reach(header: &[u32; WORDS], last: u64) -> (u64, Option<u64>) {
    let words: Vec<(u64, u64, u64)> = header
        .iter()
        .map(|&w| {
            let (offset, stride) = stream(w);
            (offset, stride, VERTEX_BYTES)
        })
        .collect();
    let worst = words.iter().map(|&(o, s, size)| o + s * last + size).max().unwrap_or(0);
    let (position, stride, _) = words[0];
    let next = words.iter().map(|w| w.0).filter(|&o| o > position).min();
    let laid = next.filter(|_| stride > 0).map(|next| {
        let count = (next - position) / stride;
        words.iter().map(|&(o, s, size)| o + s * count.saturating_sub(1) + size).max().unwrap_or(0)
    });
    (worst, laid)
}

/// What the CPU had put into a buffer by a submission: bytes `[at, at +
/// len)` of resource `res`, from the frame's start and the deltas of
/// submissions `..=submit`.
struct Bytes {
    contents: File,
    deltas: File,
}

impl Bytes {
    fn read(&self, frame: &Frame, res: u32, at: u64, len: u64, submit: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len as usize];
        let mut lay = |first: u64, n: u64, file: &File, from: u64| -> Result<()> {
            let (lo, hi) = (first.max(at), (first + n).min(at + len));
            if lo >= hi {
                return Ok(());
            }
            let dst = &mut out[(lo - at) as usize..(hi - at) as usize];
            if from == ZERO {
                dst.fill(0);
            } else {
                read_at(file, dst, from + (lo - first))?;
            }
            Ok(())
        };
        for c in frame.contents.iter().filter(|c| c.res == res) {
            lay(c.first, c.len, &self.contents, c.file)?;
        }
        for s in &frame.submits[..=submit] {
            for d in s.deltas.iter().filter(|d| d.res == res) {
                lay(d.at, d.len, &self.deltas, d.file)?;
            }
        }
        Ok(out)
    }
}

/// Fills `to` from `file` at `at`.
fn read_at(file: &File, to: &mut [u8], at: u64) -> Result<()> {
    let mut done = 0;
    while done < to.len() {
        let n = file.seek_read(&mut to[done..], at + done as u64)?;
        anyhow::ensure!(n > 0, "the file ends at {:#x}", at + done as u64);
        done += n;
    }
    Ok(())
}

/// The slot `at` of heap `heap` as it stood for submission `submit`.
fn slot_at(frame: &Frame, heap: u32, at: u32, submit: usize) -> Slot {
    let late = frame.submits[..=submit]
        .iter()
        .rev()
        .find_map(|s| s.views.iter().rev().find(|(h, i, _)| *h == heap && *i == at).map(|v| v.2));
    late.unwrap_or_else(|| {
        let runs = frame.descriptor_heaps.iter().find(|h| h.id == heap).map(|h| &h.runs);
        runs.and_then(|runs| {
            let k = runs.partition_point(|r| r.0 <= at).checked_sub(1)?;
            let (first, n, s) = &runs[k];
            (at - first < *n).then_some(*s)
        })
        .unwrap_or(Slot::EMPTY)
    })
}

fn word(s: &Slot, i: usize) -> u32 {
    u32::from_le_bytes([s.desc[i * 4], s.desc[i * 4 + 1], s.desc[i * 4 + 2], s.desc[i * 4 + 3]])
}

/// For each draw of lists `lists` whose pipeline's name has one of
/// `shaders` in it: the root SRV's view (graphics root parameter `param`,
/// a one-view table), its header, and how far the streams reach against
/// the bytes the view holds.
pub fn root_reach(
    folder: &Path,
    lists: (u32, u32),
    shaders: &[String],
    param: u32,
    line: &mut dyn FnMut(&str),
) -> Result<()> {
    let at = find(folder)?;
    let frame = Frame::read(&at.join("frame.bin"))?;
    let bytes = Bytes { contents: File::open(at.join("contents.bin"))?, deltas: File::open(at.join("deltas.bin"))? };
    let names: HashMap<u32, &str> = frame.pipelines.iter().map(|p| (p.id, p.name.as_str())).collect();
    let widths: HashMap<u32, u64> = frame.resources.iter().map(|r| (r.id, r.desc.width)).collect();
    let (mut draws, mut past) = (0u32, 0u32);
    let mut place = 0;
    for (i, s) in frame.submits.iter().enumerate() {
        for &l in s.lists.iter().filter(|&&l| l != MISSING) {
            let here = place;
            place += 1;
            if here < lists.0 || here > lists.1 {
                continue;
            }
            let (mut pso, mut heap, mut table) = (NONE, NONE, None);
            // The graphics root constants of parameter 0: the title's user
            // data, whose dwords 8 and 9 hold the heap places of its tables.
            let mut user = [0u32; 16];
            for (n, c) in Cmd::all(&frame.lists[l as usize].commands.0)?.iter().enumerate() {
                match c {
                    Cmd::Pipeline { pso: p } => pso = *p,
                    Cmd::Heaps { heaps } => heap = heaps.first().copied().unwrap_or(NONE),
                    Cmd::RootSignature { graphics: true, .. } => {
                        table = None;
                        user = [0; 16];
                    }
                    Cmd::Constants { graphics: true, param: 0, offset, values } => {
                        for (k, v) in values.iter().enumerate() {
                            if let Some(at) = user.get_mut(*offset as usize + k) {
                                *at = *v;
                            }
                        }
                    }
                    Cmd::Table { graphics: true, param: p, heap: h, slot } if *p == param => table = Some((*h, *slot)),
                    c if c.runs_shaders() && !matches!(c, Cmd::Dispatch { .. }) => {
                        let name = names.get(&pso).copied().unwrap_or_default();
                        if !shaders.iter().any(|s| name.contains(s.as_str())) {
                            continue;
                        }
                        draws += 1;
                        let Some((_, at)) = table else {
                            line(&format!("{here}:{n} {name}: no table at root parameter {param}"));
                            continue;
                        };
                        let v = slot_at(&frame, heap, at, i);
                        if v.kind != slot::SRV || v.res == NONE {
                            line(&format!("{here}:{n} {name}: slot {at} holds no buffer view (kind {})", v.kind));
                            continue;
                        }
                        // A raw view: elements of 4 bytes from word 4 on.
                        let first = u64::from(word(&v, 4)) | u64::from(word(&v, 5)) << 32;
                        let (start, held) = (first * 4, u64::from(word(&v, 6)) * 4);
                        let header = bytes.read(&frame, v.res, start, 4 * WORDS as u64, i)?;
                        let header: [u32; WORDS] =
                            std::array::from_fn(|k| u32::from_le_bytes(header[k * 4..k * 4 + 4].try_into().unwrap()));
                        let (worst, laid) = reach(&header, 0xffff);
                        let end = widths.get(&v.res).copied().unwrap_or(0);
                        let over = laid.is_some_and(|r| r > held);
                        past += u32::from(over);
                        line(&format!(
                            "{here}:{n} {name}: resource {} +{start:#x}, view {held} bytes (to {:#x} of {end:#x}); \
                             reach by layout {} bytes{}, at most {worst}; header {:08x?}",
                            v.res,
                            start + held,
                            laid.map_or("?".into(), |r| r.to_string()),
                            if over { " PAST THE VIEW" } else { "" },
                            &header[..],
                        ));
                        // Where the skinned positions are found (`skinned`).
                        let raw = |at: u32| {
                            let v = slot_at(&frame, heap, at, i);
                            (v.kind == slot::SRV && v.res != NONE)
                                .then(|| (v.res, (u64::from(word(&v, 4)) | u64::from(word(&v, 5)) << 32) * 4))
                        };
                        let (base, record) = (user[8] >> 5, user[9] >> 5);
                        let show = |r: Option<(u32, u64)>| r.map_or("none".to_owned(), |(res, at)| format!("{res}+{at:#x}"));
                        line(&format!(
                            "skin {here}:{n} {name}: record {} positions-0 {} positions-1 {}",
                            show(raw(record)),
                            show(raw(base + SKINNED[0])),
                            show(raw(base + SKINNED[1])),
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    line(&format!("{draws} draws looked at, {past} read past their view by the layout"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header of draw 58:835 of the corridor capture: positions at
    /// 0x80, 8 bytes apart, up to the stream at 0x3f00 (2,000 vertices).
    #[test]
    fn a_captured_header_reaches_where_its_layout_ends() {
        let header = [
            0x0000_4042, 0x008c_e026, 0x001f_8021, 0x00bb_c028, 0x00cb_6028, 0, 0x004e_6026, 0x005e_0026,
            0x006d_a025, 0x007d_4025, 0, 0x002f_2027, 0x009c_8026, 0x00ac_2026, 0x00db_0042, 0x00fa_4028,
            0, 0x0109_e084, 0, 0x003e_c027, 0, 0, 0, 0x0000_0200,
        ];
        assert_eq!(stream(header[0]), (0x80, 8));
        let (worst, laid) = reach(&header, 0xffff);
        // The word at 0x44 (0x213c0, 16 apart) reaches furthest, read or not.
        assert_eq!(laid, Some(0x213c0 + 16 * 1999 + 12));
        assert_eq!(worst, 0x213c0 + 16 * 0xffff + 12);
    }

    #[test]
    fn a_header_with_nothing_above_its_positions_has_no_layout_reach() {
        let mut header = [0u32; WORDS];
        header[0] = 0x0000_4042;
        assert_eq!(reach(&header, 0xffff).1, None);
    }
}
