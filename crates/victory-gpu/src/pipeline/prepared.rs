use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::{D3D12_CACHED_PIPELINE_STATE, ID3D12PipelineState};

thread_local! { static KEY: RefCell<Option<String>> = const { RefCell::new(None) }; }

thread_local! { static LIVE: Cell<bool> = const { Cell::new(false) }; }

static NATIVE: OnceLock<String> = OnceLock::new();
static RESTORED: AtomicU64 = AtomicU64::new(0);

pub(super) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_PREPARE_PIPELINES"))
}

pub(super) fn with<T>(key: &str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let old = KEY.with_borrow_mut(|k| k.replace(key.to_owned()));
    let result = f();
    KEY.with_borrow_mut(|k| *k = old);
    result
}

pub(super) fn restoring<T>(key: &str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let old = LIVE.replace(true);
    let result = with(key, f);
    LIVE.set(old);
    result
}

pub(super) fn active() -> bool {
    enabled() && KEY.with_borrow(Option::is_some)
}

fn root(dir: &Path) -> PathBuf {
    dir.join(NATIVE.get_or_init(|| {
        let name = |key| format!("native-{}-{key}", super::known::SHADOW_REVISION);
        let current = name(super::known::fingerprint());
        let to = dir.join(&current);
        if !to.exists() && let Some(previous) = super::known::matching_old_fingerprint() {
            let from = dir.join(name(previous));
            if from.is_dir() && std::fs::rename(&from, &to).is_ok() {
                tracing::info!(from = %from.display(), to = %to.display(), "adopted compatible native pipeline cache");
            }
        }
        current
    }))
}

fn path(dir: &Path, key: &str) -> PathBuf {
    root(dir).join(&key[..2]).join(format!("{key}.pso"))
}

pub(super) fn inventory(dir: &Path) -> HashSet<String> {
    let mut found = HashSet::new();
    for sub in std::fs::read_dir(root(dir)).into_iter().flatten().flatten() {
        for e in std::fs::read_dir(sub.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let filename = e.file_name();
            if let Some(key) = filename.to_str().and_then(|n| n.strip_suffix(".pso"))
                && key.len() == 32
                && key.bytes().all(|c| c.is_ascii_hexdigit())
                && e.metadata().is_ok_and(|m| m.is_file() && m.len() > 64)
            {
                found.insert(key.to_owned());
            }
        }
    }
    found
}

fn save(path: &Path, name: &str, blob: &[u8]) -> io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{n}.tmp", std::process::id()));
    std::fs::create_dir_all(path.parent().unwrap())?;
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(name.as_bytes())?;
        f.write_all(blake3::hash(blob).as_bytes())?;
        f.write_all(blob)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

pub(super) fn rectangle(vs: &[u8], ps: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let emit = || victory_shader::emit::rect_list_gs(vs, ps).map_err(|e| e.to_string());
    if !enabled() {
        return emit();
    }
    let Some(dir) = super::library::folder() else {
        return emit();
    };
    let mut hash = blake3::Hasher::new();
    hash.update(crate::shader_cache::SOURCE.as_bytes());
    for code in [vs, ps.unwrap_or_default()] {
        hash.update(&(code.len() as u64).to_le_bytes());
        hash.update(code);
    }
    let key = hash.finalize().to_hex();
    let name = &key[..32];
    let path = dir
        .join("rectangle-stages")
        .join(&key[..2])
        .join(format!("{key}.gs"));
    if let Ok(data) = std::fs::read(&path)
        && data.len() > 64
        && &data[..32] == name.as_bytes()
        && data[32..64] == *blake3::hash(&data[64..]).as_bytes()
    {
        return match data[64] {
            1 => Ok(data[65..].to_vec()),
            _ => Err(String::from_utf8_lossy(&data[65..]).into_owned()),
        };
    }
    let made = emit();
    let mut bytes = vec![u8::from(made.is_ok())];
    bytes.extend_from_slice(match &made {
        Ok(code) => code,
        Err(err) => err.as_bytes(),
    });
    if let Err(e) = save(&path, name, &bytes) {
        eprintln!(
            "Could not save generated geometry shader {}: {e}. Rerun play to resume.",
            path.display()
        );
        std::process::exit(2);
    }
    made
}

pub(super) fn pipeline(
    name: &str,
    create: impl Fn(D3D12_CACHED_PIPELINE_STATE) -> windows::core::Result<ID3D12PipelineState>,
) -> windows::core::Result<ID3D12PipelineState> {
    let dir = super::library::folder().ok_or_else(windows::core::Error::empty)?;
    let key = KEY.with_borrow(|k| k.as_deref().unwrap().to_owned());
    let path = path(&dir, &key);
    static OTHER: OnceLock<Vec<PathBuf>> = OnceLock::new();
    let load = |file: &[u8]| {
        if file.len() <= 64 || file[..32] != *name.as_bytes() {
            return None;
        }
        let bytes = &file[64..];
        if file[32..64] != *blake3::hash(bytes).as_bytes() {
            return None;
        }
        create(D3D12_CACHED_PIPELINE_STATE {
            pCachedBlob: bytes.as_ptr().cast(),
            CachedBlobSizeInBytes: bytes.len(),
        })
        .ok()
    };
    if let Ok(file) = std::fs::read(&path) {
        if let Some(pso) = load(&file) {
            let n = RESTORED.fetch_add(1, Ordering::Relaxed) + 1;
            super::library::LOADED.fetch_add(1, Ordering::Relaxed);
            if n.is_power_of_two() {
                tracing::info!(n, "native pipelines restored");
            }
            return Ok(pso);
        }
        tracing::warn!(name, "native pipeline rejected; rebuilding");
    }
    let others = OTHER.get_or_init(|| {
        let current = root(&dir);
        let mut old: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|s| s.starts_with("native-"))
                    && e.path() != current
                    && e.file_type().is_ok_and(|t| t.is_dir())
            })
            .collect();
        old.sort_by_key(|e| std::cmp::Reverse(e.metadata().ok().and_then(|m| m.modified().ok())));
        old.into_iter().map(|e| e.path()).collect()
    });
    for other in others {
        let old = other.join(&key[..2]).join(path.file_name().unwrap());
        let Ok(file) = std::fs::read(&old) else {
            continue;
        };
        let Some(pso) = load(&file) else {
            continue;
        };
        std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|_| {
                std::fs::hard_link(&old, &path).or_else(|_| save(&path, name, &file[64..]))
            })
            .map_err(|e| {
                tracing::error!(%e, file = %path.display(), "native pipeline not saved");
                windows::core::Error::empty()
            })?;
        super::library::LOADED.fetch_add(1, Ordering::Relaxed);
        return Ok(pso);
    }
    if LIVE.get() {
        tracing::warn!(name, file = %path.display(), "native pipeline missed at first use");
    }
    let pso = create(Default::default())?;
    let blob = unsafe { pso.GetCachedBlob()? };
    let len = unsafe { blob.GetBufferSize() };
    if len == 0 {
        return Err(windows::core::Error::empty());
    }
    let data = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), len) };
    save(&path, name, data).map_err(|err| {
        tracing::error!(%err, file = %path.display(), "native pipeline not saved");
        windows::core::Error::empty()
    })?;
    super::library::MADE.fetch_add(1, Ordering::Relaxed);
    Ok(pso)
}

type Built = Mutex<HashSet<String>>;

pub(super) fn at_creation(
    key: &str,
    shaders: &[Arc<super::Shader>],
    state: Option<&super::host::GraphicsState>,
) {
    if !enabled() || shaders.is_empty() {
        return;
    }
    static BUILT: OnceLock<Built> = OnceLock::new();
    static ACTIVE: std::sync::LazyLock<Mutex<(usize, HashSet<String>)>> =
        std::sync::LazyLock::new(|| Mutex::new((0, HashSet::new())));
    static FREE: Condvar = Condvar::new();
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let built = BUILT.get_or_init(|| {
        Mutex::new(super::library::folder().map_or_else(HashSet::new, |d| inventory(&d)))
    });
    if built
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(key)
    {
        return;
    }
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    while active.0 >= 6 || active.1.contains(key) {
        active = FREE.wait(active).unwrap_or_else(|e| e.into_inner());
    }
    if built
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(key)
    {
        return;
    }
    active.0 += 1;
    active.1.insert(key.to_owned());
    drop(active);
    let made = with(key, || super::host::HostPipeline::new(shaders, state));
    if made.is_none() {
        eprintln!("Could not prepare pipeline {key}. Rerun play to resume.");
        super::known::flush();
        std::process::exit(2);
    }
    drop(made);
    built
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.to_owned());
    let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    static PROGRESS: AtomicU64 = AtomicU64::new(0);
    let now = crate::host::now_ms();
    let last = PROGRESS.load(Ordering::Relaxed);
    if n == 1
        || now.saturating_sub(last) >= 3000
            && PROGRESS
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
    {
        println!("{n} new native pipelines prepared");
    }
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    active.0 -= 1;
    active.1.remove(key);
    FREE.notify_all();
}

pub(super) fn jobs() -> usize {
    let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
    let memory = crate::host::available_physical_ram()
        .map_or(4, |n| (n.saturating_sub(8 << 30) / (768 << 20)) as usize);
    memory.min(cpus / 2).clamp(1, 10)
}

pub(super) fn warm() -> io::Result<()> {
    let jobs = jobs();
    let status = std::process::Command::new(std::env::current_exe()?)
        .args(["warm", "--jobs", &jobs.to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "pipeline preparation failed ({status}); rerun play to resume"
        )))
    }
}
