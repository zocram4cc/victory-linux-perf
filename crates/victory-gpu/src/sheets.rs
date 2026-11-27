//! The frame recorder's watch for stretched geometry ("sheets": character
//! meshlets drawn with vertices in the wrong place, as huge flat triangles
//! across the screen for a frame or a few). `VICTORY_RECORD_SHEETS=1`: the
//! title's pre-skinned mesh programs (`PreSkin` in their container's name)
//! are translated with [`victory_shader::emit::with_sheet_watch`], and note
//! host waves of vertices that span much of the screen in a small buffer in
//! the PC's memory ([`victory_shader::emit::SLOT_SHEETS`]). The recorder
//! reads and clears it at each present ([`take`]): no copy, no wait.
//! Everything else is as without it; what is drawn does not change.

use std::sync::OnceLock;

use victory_shader::emit::{SHEET_BUCKETS, SHEET_SLOTS};
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

/// Dwords of the buffer.
const DWORDS: u32 = SHEET_BUCKETS * SHEET_SLOTS;

/// Whether the watch is on (`VICTORY_RECORD_SHEETS=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RECORD_SHEETS").is_some_and(|v| v == "1"))
}

/// Whether a program is watched: what its container is named. Only with
/// the buffer made: a watched program's root view must always be bound.
pub(crate) fn watched(container: &[u8]) -> bool {
    on() && is_watched_name(crate::pipeline::named::name_in(container).as_deref()) && notes().is_some()
}

/// Whether a container is of a pre-skinned program (the replay's
/// `--retranslate` asks, with the watch on).
pub fn pre_skinned(container: &[u8]) -> bool {
    is_watched_name(crate::pipeline::named::name_in(container).as_deref())
}

/// The pre-skinned programs: those that draw the characters' meshlets.
fn is_watched_name(name: Option<&str>) -> bool {
    name.is_some_and(|n| n.contains("PreSkin"))
}

struct Notes {
    _buffer: ID3D12Resource,
    data: *mut u32,
    gpu: u64,
}

// SAFETY: the mapping lives as long as the buffer, which lives as long as
// the process; its dwords are read and written as single volatile words.
unsafe impl Send for Notes {}
// SAFETY: as above.
unsafe impl Sync for Notes {}

fn notes() -> Option<&'static Notes> {
    static NOTES: OnceLock<Option<Notes>> = OnceLock::new();
    NOTES
        .get_or_init(|| {
            let host = crate::host::host()?;
            let (buffer, data) = host.shared_buffer(u64::from(DWORDS) * 4)?;
            crate::host::name(&buffer, || "frame recorder: sheets".into());
            // SAFETY: a fresh mapping of that many bytes.
            unsafe { std::ptr::write_bytes(data, 0, DWORDS as usize * 4) };
            // SAFETY: a live buffer.
            let gpu = unsafe { buffer.GetGPUVirtualAddress() };
            tracing::info!("frame recorder: the pre-skinned mesh programs note stretched geometry (VICTORY_RECORD_SHEETS)");
            Some(Notes {
                _buffer: buffer,
                data: data.cast::<u32>(),
                gpu,
            })
        })
        .as_ref()
}

/// Where watched programs note what they saw (their root UAV), if the
/// watch is on.
pub(crate) fn address() -> Option<u64> {
    if !on() {
        return None;
    }
    notes().map(|n| n.gpu)
}

/// Per row (span), the slots noted since the last call, which clears them.
/// None while the watch is off or nothing was ever bound.
pub fn take() -> Option<[u32; SHEET_BUCKETS as usize]> {
    if !on() {
        return None;
    }
    let n = notes()?;
    let mut rows = [0; SHEET_BUCKETS as usize];
    for (row, count) in rows.iter_mut().enumerate() {
        for slot in 0..SHEET_SLOTS as usize {
            // SAFETY: inside the mapping; the card may be writing, and a
            // note missed now is written again by the next wave that sees it.
            let at = unsafe { n.data.add(row * SHEET_SLOTS as usize + slot) };
            // SAFETY: as above.
            if unsafe { at.read_volatile() } != 0 {
                *count += 1;
                // SAFETY: as above.
                unsafe { at.write_volatile(0) };
            }
        }
    }
    Some(rows)
}

/// Rows of the buffer (for the replay, which keeps one of its own).
pub fn counts(dwords: &[u32]) -> [u32; SHEET_BUCKETS as usize] {
    let mut rows = [0; SHEET_BUCKETS as usize];
    for (row, count) in rows.iter_mut().enumerate() {
        let from = row * SHEET_SLOTS as usize;
        *count = dwords
            .get(from..from + SHEET_SLOTS as usize)
            .map_or(0, |s| s.iter().filter(|&&d| d != 0).count() as u32);
    }
    rows
}

/// Bytes of the buffer.
pub const BYTES: u64 = DWORDS as u64 * 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pre_skinned_programs_are_watched() {
        assert!(is_watched_name(Some("MS_VS_PedTransformPreSkinD_Packed")));
        assert!(!is_watched_name(Some("MS_VS_PedTransform")));
        assert!(!is_watched_name(None));
    }

    #[test]
    fn rows_count_the_slots_noted() {
        let mut d = vec![0u32; DWORDS as usize];
        d[3] = 1;
        d[SHEET_SLOTS as usize + 7] = 1;
        d[SHEET_SLOTS as usize + 8] = 5;
        d[DWORDS as usize - 1] = 1;
        assert_eq!(counts(&d), [1, 2, 0, 1]);
        assert_eq!(counts(&[]), [0; 4]);
    }
}
