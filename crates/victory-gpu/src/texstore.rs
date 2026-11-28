//! A store of the larger levels of textures the title streams
//! (`VICTORY_TEXTURE_STORE=<folder>`; off).
//!
//! The title streams a texture's levels smallest first, and views it from
//! the first level it holds: a new texture is blurred until its larger
//! levels have been read (its streaming, and ours of its files) and shown,
//! seconds later, and again each time it is streamed out and back in.
//! Measured (run light-1, `crate::texlate`): a slot written while its
//! memory was not there waited 8.7 s on average to be shown.
//!
//! Here, once a texture has been uploaded whole (all levels, from level 0),
//! its levels as the host lays them out are kept, in memory and in the
//! folder, under keys made of its shape and of the bytes of its smaller
//! levels: for each level `k` from which on its levels fill at least
//! `MIN_PREFIX` bytes, the digest of the memory those levels take (the
//! title lays a texture's smaller levels out first, so that is the start of
//! its memory). When a texture of that shape is later first viewed from
//! level `k` with those very bytes there, in this run or a later one, its
//! host copy is made whole at once: levels from `k` on from title memory as
//! always, the larger ones from the store, and its views show them all.
//!
//! What it assumes: two textures of one shape whose smaller levels hold the
//! same bytes (at least `MIN_PREFIX` of them) have the same larger levels.
//! A texture shown wrong would be one of two such. Files are never trimmed.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

/// Least bytes of the smaller levels a key is made of.
pub(crate) const MIN_PREFIX: u64 = 32 << 10;
/// Bytes of levels kept in memory at most (the rest is read from the folder
/// when asked).
const KEPT: u64 = 4 << 30;

/// A texture's shape: width, height, format, levels.
pub(crate) type Shape = [u32; 4];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Key {
    shape: Shape,
    level: u32,
    digest: u128,
}

/// Levels `0..levels` of a texture as the host lays them out, by the
/// digest of the whole texture they came from.
#[derive(Clone, Copy)]
struct Entry {
    blob: u128,
    levels: u32,
}

struct Store {
    folder: PathBuf,
    index: Mutex<HashMap<Key, Entry>>,
    kept: Mutex<HashMap<u128, Arc<Vec<u8>>>>,
    kept_bytes: AtomicU64,
    jobs: Mutex<mpsc::Sender<Job>>,
    /// Contents given to it this run (digests of whole textures).
    seen: Mutex<std::collections::HashSet<u128>>,
}

/// Since the start: entries found at startup, textures kept, keys asked,
/// found, blobs read from the folder, bytes written.
pub(crate) static STATS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

fn line_of(key: &Key, entry: &Entry) -> String {
    format!(
        "{} {} {} {} {} {:032x} {:032x} {}",
        key.shape[0], key.shape[1], key.shape[2], key.shape[3], key.level, key.digest, entry.blob, entry.levels
    )
}

fn parse_line(line: &str) -> Option<(Key, Entry)> {
    let mut p = line.split_whitespace();
    let mut n = || p.next();
    let shape = [n()?.parse().ok()?, n()?.parse().ok()?, n()?.parse().ok()?, n()?.parse().ok()?];
    let level = n()?.parse().ok()?;
    let digest = u128::from_str_radix(n()?, 16).ok()?;
    let blob = u128::from_str_radix(n()?, 16).ok()?;
    let levels = n()?.parse().ok()?;
    Some((Key { shape, level, digest }, Entry { blob, levels }))
}

fn store() -> Option<&'static Store> {
    static STORE: OnceLock<Option<Store>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let folder = PathBuf::from(std::env::var_os("VICTORY_TEXTURE_STORE").filter(|v| !v.is_empty() && v != "0")?);
            std::fs::create_dir_all(&folder).ok()?;
            let mut index = HashMap::new();
            if let Ok(text) = std::fs::read_to_string(folder.join("index.txt")) {
                for (key, entry) in text.lines().filter_map(parse_line) {
                    // (A blob whose file is gone is no entry.)
                    index.insert(key, entry);
                }
            }
            STATS[0].store(index.len() as u64, Relaxed);
            tracing::info!(
                folder = %folder.display(),
                keys = index.len(),
                "texture store: larger levels kept from earlier runs (VICTORY_TEXTURE_STORE)"
            );
            let (send, receive) = mpsc::channel::<Job>();
            let to = folder.clone();
            std::thread::Builder::new()
                .name("texture store".into())
                .spawn(move || {
                    let Ok(mut index) =
                        std::fs::OpenOptions::new().create(true).append(true).open(to.join("index.txt"))
                    else {
                        return;
                    };
                    for job in receive {
                        // (The store is made before this thread's first job.)
                        let Some(store) = store() else {
                            continue;
                        };
                        let Some((blob, bytes, keys)) = kept(store, job) else {
                            continue;
                        };
                        let path = to.join(format!("{blob:032x}.lv"));
                        if !path.exists() {
                            let part = to.join(format!("{blob:032x}.part"));
                            if std::fs::write(&part, &bytes[..]).is_err() || std::fs::rename(&part, &path).is_err() {
                                continue;
                            }
                            STATS[5].fetch_add(bytes.len() as u64, Relaxed);
                        }
                        let text: String = keys.iter().map(|(k, e)| line_of(k, e) + "\n").collect();
                        let _ = index.write_all(text.as_bytes());
                        let _ = index.flush();
                    }
                })
                .ok()?;
            Some(Store {
                folder,
                index: Mutex::new(index),
                kept: Mutex::new(HashMap::new()),
                kept_bytes: AtomicU64::new(0),
                jobs: Mutex::new(send),
                seen: Mutex::new(std::collections::HashSet::new()),
            })
        })
        .as_ref()
}

/// Whether the store is on.
pub(crate) fn on() -> bool {
    store().is_some()
}

/// The levels a key can be made from: those from which on the levels fill
/// at least [`MIN_PREFIX`] bytes and are the start of the memory, with
/// those bytes (`level`, bytes). Not level 0: that is the whole texture.
pub(crate) fn key_levels(layout: &victory_formats::texture::Layout, mips: u32) -> Vec<(u32, u64)> {
    if layout.slices != 1 {
        return Vec::new();
    }
    (1..mips)
        .filter(|&k| crate::resource::read_from(layout, k) == k)
        .map(|k| (k, layout.bytes_from(k)))
        .filter(|&(_, bytes)| bytes >= MIN_PREFIX && bytes < layout.size)
        .collect()
}

/// A texture of `shape` was read whole: `tiled` is its memory (all levels),
/// `parts` where the host lays out each of its levels (offset, row pitch,
/// rows). Detiled and kept on the store's thread, unless that content is
/// known.
pub(crate) fn keep(shape: Shape, layout: &victory_formats::texture::Layout, tiled: &[u8], parts: Vec<(u64, u32, u32)>) {
    let Some(store) = store() else {
        return;
    };
    let blob = crate::texture::digest(tiled).0;
    if !store.seen.lock().unwrap_or_else(|e| e.into_inner()).insert(blob) {
        return;
    }
    let job = Job { shape, layout: layout.clone(), tiled: tiled.to_vec(), parts, blob };
    let _ = store.jobs.lock().unwrap_or_else(|e| e.into_inner()).send(job);
}

/// A texture to keep ([`keep`]).
struct Job {
    shape: Shape,
    layout: victory_formats::texture::Layout,
    tiled: Vec<u8>,
    parts: Vec<(u64, u32, u32)>,
    blob: u128,
}

/// [`keep`]'s work, on the store's thread: the levels detiled as the host
/// lays them out, the keys, memory, and what goes to the folder.
fn kept(store: &Store, job: Job) -> Option<(u128, Arc<Vec<u8>>, Vec<(Key, Entry)>)> {
    let Job { shape, layout, tiled, parts, blob } = job;
    let keys = key_levels(&layout, shape[3]);
    let &(top, _) = keys.iter().max_by_key(|(k, _)| *k)?;
    let end = parts.get(top as usize)?.0 as usize;
    let made: Vec<Key> = keys
        .iter()
        .filter(|&&(_, bytes)| bytes as usize <= tiled.len())
        .map(|&(level, bytes)| Key { shape, level, digest: crate::texture::digest(&tiled[..bytes as usize]).0 })
        .collect();
    {
        let index = store.index.lock().unwrap_or_else(|e| e.into_inner());
        if made.iter().all(|k| index.contains_key(k)) {
            return None;
        }
    }
    // Levels `0..top`, the ones a key can stand for.
    let mut levels = vec![0u8; end];
    for (i, &(at, pitch, rows)) in parts.iter().take(top as usize).enumerate() {
        let (at, pitch, rows) = (at as usize, pitch as usize, rows as usize);
        let len = (pitch * rows).min(end.saturating_sub(at));
        layout.detile(&tiled, i as u32, 0, &mut levels[at..at + len], pitch, len).ok()?;
    }
    let bytes = Arc::new(levels);
    let entries: Vec<(Key, Entry)> = made.into_iter().map(|k| (k, Entry { blob, levels: k.level })).collect();
    {
        let mut index = store.index.lock().unwrap_or_else(|e| e.into_inner());
        for (k, e) in &entries {
            index.insert(*k, *e);
        }
    }
    if store.kept_bytes.load(Relaxed) + bytes.len() as u64 <= KEPT {
        store.kept_bytes.fetch_add(bytes.len() as u64, Relaxed);
        store.kept.lock().unwrap_or_else(|e| e.into_inner()).insert(blob, bytes.clone());
    }
    STATS[1].fetch_add(1, Relaxed);
    Some((blob, bytes, entries))
}

/// The larger levels of a texture of `shape` first viewed from `level`,
/// whose memory from that level on holds `smaller`: levels `0..level` in
/// the host's layout, if the store has them.
pub(crate) fn find(shape: Shape, level: u32, smaller: &[u8]) -> Option<Arc<Vec<u8>>> {
    let store = store()?;
    STATS[2].fetch_add(1, Relaxed);
    let key = Key { shape, level, digest: crate::texture::digest(smaller).0 };
    let entry = *store.index.lock().unwrap_or_else(|e| e.into_inner()).get(&key)?;
    if entry.levels != level {
        return None;
    }
    let kept = store.kept.lock().unwrap_or_else(|e| e.into_inner()).get(&entry.blob).cloned();
    let bytes = match kept {
        Some(bytes) => bytes,
        None => {
            let bytes = Arc::new(std::fs::read(store.folder.join(format!("{:032x}.lv", entry.blob))).ok()?);
            STATS[4].fetch_add(1, Relaxed);
            if store.kept_bytes.load(Relaxed) + bytes.len() as u64 <= KEPT {
                store.kept_bytes.fetch_add(bytes.len() as u64, Relaxed);
                store.kept.lock().unwrap_or_else(|e| e.into_inner()).insert(entry.blob, bytes.clone());
            }
            bytes
        }
    };
    STATS[3].fetch_add(1, Relaxed);
    Some(bytes)
}

/// The line for the memory lines.
pub(crate) fn line() -> Option<String> {
    let store = store()?;
    let [at_start, kept, asked, found, read, written] = [0, 1, 2, 3, 4, 5].map(|i| STATS[i].load(Relaxed));
    Some(format!(
        "texture store (VICTORY_TEXTURE_STORE), since the start: {at_start} keys from earlier runs, {kept} textures kept \
         ({} MB written, {} MB held in memory), {asked} first views from a smaller level asked, {found} found ({read} read \
         from the folder), {} host copies made whole from it",
        written >> 20,
        store.kept_bytes.load(Relaxed) >> 20,
        PREFILLED.load(Relaxed)
    ))
}

/// Host copies whose larger levels a list copied from the store.
pub(crate) static PREFILLED: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_lines_read_back() {
        let key = Key { shape: [1024, 512, 71, 11], level: 3, digest: 0x0123_4567_89ab_cdef_0011_2233_4455_6677 };
        let entry = Entry { blob: 0xffee_ddcc_bbaa_9988_7766_5544_3322_1100, levels: 3 };
        let (k, e) = parse_line(&line_of(&key, &entry)).expect("a line");
        assert_eq!(k, key);
        assert_eq!((e.blob, e.levels), (entry.blob, entry.levels));
        assert!(parse_line("1 2 3").is_none());
    }
}
