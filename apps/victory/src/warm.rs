//! `victory warm`: host pipelines made ahead of a run (`known_pipelines`), in
//! processes of their own: a compiler that crashes or hangs costs one pipeline.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use victory_hle::known_pipelines as known;

/// Pipelines a process makes before another takes its place: the compiler's
/// and the translations' memory goes with it.
const AT_ONCE: usize = 40;
/// A pipeline that takes longer is taken for one the compiler hangs on.
const LONGEST: Duration = Duration::from_secs(180);

/// As the runs count one that takes long.
const SLOW_MS: f64 = 1000.0;

fn folder() -> Result<PathBuf> {
    let cfg = victory_core::Config::load()?;
    Ok(cfg.cache_subdir("shaders")?.with_file_name("pipelines"))
}

pub fn run(jobs: Option<usize>, limit: Option<usize>, again: bool, used: bool, these: Option<String>) -> Result<()> {
    let dir = folder()?;
    if let Some(these) = these {
        return make(&dir, &these);
    }
    let began = Instant::now();
    victory_hle::known_pipelines::use_translations(dir.with_file_name("shaders"));
    if known::prepared() && let Some(game) = victory_core::Config::load()?.game_dir {
        seed_compute(&dir,&game)?;
        known::seed_passes(&dir)?;
    }
    let fingerprint = known::fingerprint();
    let native = known::prepared().then(|| known::artifacts(&dir));
    let done_file = dir.join("warm-done.txt");
    let bad_file = dir.join("warm-bad.txt");
    let (keys, revised) = known::warm_catalog(&dir);
    if known::prepared() && limit.is_none() && !keys.is_empty() { known::repair_shaders(&dir)?; }
    let revision = format!("{fingerprint}/{}", known::SHADOW_REVISION);
    let stamp = |n: usize| if revised.contains(&n) { &revision } else { &fingerprint };
    let listed = std::fs::read_to_string(&done_file).unwrap_or_default();
    let mut done: HashSet<(&str, &str)> = HashSet::new();
    for line in listed.lines() {
        if let Some((of, key)) = line.split_once(' ')
            && (of == fingerprint || of == revision)
        {
            done.insert((of, key));
        }
    }
    let bad = std::fs::read_to_string(&bad_file).unwrap_or_default();
    let bad: HashSet<&str> = bad.lines().filter_map(|l| l.split(' ').next()).collect();
    let early = std::fs::read_to_string(dir.join("early-pipelines.txt")).unwrap_or_default();
    let early: HashSet<&str> = early.lines().collect();
    // Those earlier runs drew with first, the others in no order (by key).
    let mut all: Vec<usize> = (0..keys.len())
        .filter(|&n| {
            let key = keys[n].as_str();
            if let Some(native) = &native { !native.contains(key) } else { (again || !done.contains(&(stamp(n).as_str(), key))) && !bad.contains(key) && (!used || early.contains(key)) }
        })
        .collect();
    all.sort_by_key(|&n| (!early.contains(keys[n].as_str()), &keys[n]));
    all.truncate(limit.unwrap_or(usize::MAX));
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let jobs = jobs.unwrap_or(cores * 2 / 3).max(1);
    println!(
        "{} pipelines known, {} made before ({fingerprint}), {} bad; {} to make, {jobs} processes",
        keys.len(),
        native.as_ref().map_or_else(|| keys.iter().enumerate().filter(|(n, k)| done.contains(&(stamp(*n).as_str(), k.as_str()))).count(), |n| keys.iter().filter(|k| n.contains(k.as_str())).count()),
        if native.is_some() { 0 } else { bad.len() },
        all.len()
    );
    let progress = AtomicUsize::new(0);
    let state = Mutex::new((Vec::<f64>::new(), 0usize, 0usize));
    let done_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&done_file)?);
    // (Read back by the runs: these are made beside the others, not before.)
    let slow_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(dir.join("slow-pipelines.txt"))?);
    let bad_out = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&bad_file)?);
    run_batches(
        &all,
        jobs,
        start_worker,
        |n, ms, there| {
            if ms >= SLOW_MS {
                let _ = writeln!(lock(&slow_out), "\n{} {ms:.0}", keys[n]);
            }
            let mut state = lock(&state);
            state.0.push(ms);
            state.1 += usize::from(!there);
            let count = state.0.len();
            let elapsed = began.elapsed();
            let timed = native.is_some() && {
                let period = elapsed.as_secs() as usize / 3;
                period > progress.fetch_max(period, Ordering::Relaxed)
            };
            if count.is_multiple_of(1000) || timed || native.is_some() && count == 1 {
                println!("{count} of {}, {:.0} s", all.len(), elapsed.as_secs_f64());
            }
            drop(state);
            if native.is_none() { let _ = writeln!(lock(&done_out), "{} {}", stamp(n), keys[n]); }
        },
        |n, why| {
            println!("pipeline {n} {}: {why}", keys[n]);
            if native.is_none() { let _ = writeln!(lock(&bad_out), "{} {why}", keys[n]); }
            lock(&state).2 += 1;
        },
    )?;
    let (mut times, refused, crashed) = state.into_inner().unwrap_or_else(|e| e.into_inner());
    times.sort_by(f64::total_cmp);
    let at = |q: f64| times.get((times.len() as f64 * q) as usize).copied().unwrap_or(0.0);
    println!(
        "{} made in {:.0} s ({refused} have no host pipeline, {crashed} ended their process); ms each: median {:.1}, p90 {:.1}, p99 {:.1}, most {:.0}, sum {:.0} s",
        times.len(),
        began.elapsed().as_secs_f64(),
        at(0.5),
        at(0.9),
        at(0.99),
        times.last().copied().unwrap_or(0.0),
        times.iter().sum::<f64>() / 1e3
    );
    if native.is_some() {
        let durable = if all.is_empty() { native.unwrap() } else { known::artifacts(&dir) };
        let missing = keys.iter().filter(|k| !durable.contains(k.as_str())).count();
        anyhow::ensure!(missing == 0 || limit.is_some(), "{missing} native pipelines still missing; rerun play to resume");
        println!("{} of {} native pipelines on disk", keys.len()-missing, keys.len());
    }
    Ok(())
}

fn seed_compute(dir: &Path, game: &Path) -> Result<()> {
    let files = [game.join("shaders.rpf"), game.join("XboxBvhBuilder.dll"), game.join("game_scarlett_master.exe")];
    let present: Vec<_> = files.iter().filter(|p| p.is_file()).collect();
    if present.is_empty() { return Ok(()); }
    let mut source = blake3::Hasher::new();
    source.update(b"all supported shader stages 1");
    for path in &present {
        let meta = std::fs::metadata(path)?;
        source.update(path.as_os_str().as_encoded_bytes());
        source.update(&meta.len().to_le_bytes());
        source.update(&meta.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos().to_le_bytes());
    }
    let signature = source.finalize().to_hex().to_string();
    let passes = known::streamed_passes();
    let manifest = dir.join(if passes { "material-sources-2.txt" } else { "compute-sources.txt" });
    let previous = std::fs::read_to_string(&manifest).unwrap_or_default();
    let mut lines = previous.lines();
    if lines.next() == Some(signature.as_str())
        && let Some(length) = lines.next().and_then(|v|v.parse::<u64>().ok())
        && std::fs::metadata(dir.join("known-shaders.bin")).is_ok_and(|m|m.len() >= length && length != 0)
    {
        let known: HashSet<_> = known::keys(dir).into_iter().collect();
        if lines.clone().next().is_some() && lines.all(|k| known.contains(k)) { return Ok(()); }
    }
    println!("Discovering pipeline recipes and shader programs in the game files");
    let mut found = HashSet::new();
    for path in present {
        let data = std::fs::read(path)?;
        let mut effects: std::collections::HashMap<(bool,String),[Vec<Vec<u8>>;2]> = Default::default();
        for source in crate::shader::sources(&data) {
            let Some(bytes) = source.bytes() else { continue; };
            if let Some(key) = known::seed_compute(&bytes) { found.insert(key); }
            if passes && let crate::shader::Source::Packed(record) = &source
                && let Some(name) = record.name.and_then(|s| s.strip_suffix("_Wrapped"))
                && let Some((effect,tag)) = name.rsplit_once('_')
                && tag.len() == 8 && tag.bytes().all(|c| c.is_ascii_hexdigit())
            {
                let (water,stage) = match effect {
                    "VS_WaterClip" => (true,0), "PS_WaterClipOcean" => (true,1),
                    "VS_TAA2_UpscaleEntityMask" | "VS_TAA2_SpatialUpscale" => (false,0),
                    "PS_TAA2_UpscaleEntityMask" | "PS_TAA2_SpatialUpscale" | "PS_TAA2_Blit" | "PS_TAA2_SpatialUpscaleWithDithering" => (false,1),
                    _ => continue,
                };
                effects.entry((water,tag.to_owned())).or_default()[stage].push(bytes.into_owned());
            }
        }
        for ((water,_),[vertex,pixel]) in effects {
            for vs in &vertex { for ps in &pixel { found.extend(known::seed_screen(vs,ps,water)); } }
        }
    }
    known::flush();
    let persisted: HashSet<_> = known::keys(dir).into_iter().collect();
    anyhow::ensure!(found.is_subset(&persisted),"could not save compute pipeline catalogue");
    let mut entries: Vec<_> = found.into_iter().collect();
    entries.sort_unstable();
    let tmp = manifest.with_extension(format!("{}.tmp",std::process::id()));
    let length = std::fs::metadata(dir.join("known-shaders.bin"))?.len();
    std::fs::write(&tmp, format!("{signature}\n{length}\n{}\n",entries.join("\n")))?;
    std::fs::rename(tmp,&manifest)?;
    println!("{} pipelines discovered in the game files before starting the game",entries.len());
    Ok(())
}

fn run_batches(
    all: &[usize],
    jobs: usize,
    start: impl Fn(&[usize]) -> Result<Child> + Sync,
    each: impl Fn(usize, f64, bool) + Sync,
    bad: impl Fn(usize, String) + Sync,
) -> Result<()> {
    let chunks: Vec<&[usize]> = all.chunks(if known::prepared() { 256 } else { AT_ONCE }).collect();
    let next = AtomicUsize::new(0);
    // Stop launching workers after a setup failure. In-flight workers can still report.
    let failure = OnceLock::new();
    std::thread::scope(|s| {
        for _ in 0..jobs.min(chunks.len()) {
            s.spawn(|| {
                while let Some(&chunk) = chunks.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let mut rest = chunk;
                    while !rest.is_empty() {
                        if failure.get().is_some() {
                            return;
                        }
                        let outcome = start(rest)
                            .context("could not start warm worker")
                            .and_then(|process| child(process, &each));
                        match outcome {
                            Ok(Some((made, why))) => {
                                let remaining = &rest[made.min(rest.len())..];
                                let Some((&n, after)) = remaining.split_first() else {
                                    break;
                                };
                                bad(n, why);
                                rest = after;
                            }
                            Ok(None) => break,
                            Err(error) => {
                                let _ = failure.set(error);
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    failure.into_inner().map_or(Ok(()), Err)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn start_worker(these: &[usize]) -> Result<Child> {
    use std::os::windows::process::CommandExt;
    const IDLE_PRIORITY_CLASS: u32 = 0x40;
    let exe = std::env::current_exe().context("could not locate the warm worker executable")?;
    let list: Vec<String> = these.iter().map(usize::to_string).collect();
    Command::new(&exe)
        .args(["warm", "--these", &list.join(",")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(IDLE_PRIORITY_CLASS)
        .spawn()
        .with_context(|| format!("could not spawn {}", exe.display()))
}

/// Under Wine the postmortem debugger may inherit the worker's stdout. EOF
/// then says nothing about the worker being alive. Conversely, closing stdout
/// need not mean the process exited. Poll the pipe and the process separately;
/// the no-progress deadline covers *both* and does not wait for a debugger.
fn checkpoint_worker(mut worker: Child, mut each: impl FnMut(usize, f64, bool)) -> Result<Option<(usize, String)>> {
    use std::ffi::c_void;
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn PeekNamedPipe(pipe: *mut c_void, data: *mut c_void, capacity: u32, read: *mut u32, available: *mut u32, left: *mut u32) -> i32;
    }
    let mut out = worker.stdout.take().context("warm worker stdout unavailable")?;
    let mut bytes = [0;8192];
    let mut line = Vec::new();
    let mut made = 0;
    let mut last = Instant::now();
    let mut status = None;
    loop {
        let mut available = 0u32;
        // SAFETY: stdout stays live, the only requested output is one DWORD.
        let open = unsafe { PeekNamedPipe(out.as_raw_handle(), std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut available, std::ptr::null_mut()) } != 0;
        if open && available > 0 {
            let len = bytes.len().min(available as usize);
            let n = out.read(&mut bytes[..len])?;
            for &c in &bytes[..n] {
                if c != b'\n' {
                    line.push(c);
                    continue;
                }
                if let Ok(record) = std::str::from_utf8(&line) {
                    let mut words = record.split_ascii_whitespace();
                    match words.next() {
                        Some("at") => { last = Instant::now(); }
                        Some("made") => if let (Some(index),Some(ms),Some(valid)) = (words.next().and_then(|v|v.parse().ok()), words.next().and_then(|v|v.parse().ok()), words.next()) {
                            each(index, ms, valid == "1");
                            made += 1;
                            last = Instant::now();
                        },
                        _ => (),
                    }
                }
                line.clear();
            }
        } else if let Some(exit) = status {
            // Checked for buffered reports again *after* observing exit.
            let exit: std::process::ExitStatus = exit;
            return Ok((!exit.success()).then(|| (made,exit.to_string())));
        } else if let Some(exit) = worker.try_wait()? {
            status = Some(exit);
            continue;
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
        if last.elapsed() >= LONGEST {
            // No blocking wait after TerminateProcess: a debugger can hold a
            // faulting thread suspended even during its last instructions.
            if worker.try_wait()?.is_none() { worker.kill().context("stopping unresponsive warm worker")?; }
            return Ok(Some((made,format!("unresponsive for {} s",LONGEST.as_secs()))));
        }
    }
}

/// On abnormal exit, return the number of reports before the failure and the exit reason.
fn child(mut process: Child, mut each: impl FnMut(usize, f64, bool)) -> Result<Option<(usize, String)>> {
    if known::prepared() { return checkpoint_worker(process, each); }
    let Some(out) = process.stdout.take() else {
        let _ = process.kill();
        process.wait().context("warm worker stdout unavailable, failed to reap worker")?;
        anyhow::bail!("warm worker stdout unavailable");
    };
    let process = Mutex::new(process);
    let (heard, over) = (Mutex::new(Instant::now()), AtomicBool::new(false));
    let hung = AtomicBool::new(false);
    let mut made = 0;
    std::thread::scope(|s| {
        s.spawn(|| {
            while !over.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                if lock(&heard).elapsed() > LONGEST {
                    hung.store(true, Ordering::Relaxed);
                    let _ = lock(&process).kill();
                    return;
                }
            }
        });
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            *lock(&heard) = Instant::now();
            let mut words = line.split(' ');
            if words.next() == Some("made")
                && let (Some(n), Some(ms), Some(there)) = (words.next(), words.next(), words.next())
                && let (Ok(n), Ok(ms)) = (n.parse(), ms.parse())
            {
                made += 1;
                each(n, ms, there == "1");
            }
        }
        over.store(true, Ordering::Relaxed);
    });
    let status = lock(&process).wait();
    let well = status.as_ref().is_ok_and(|s| s.success());
    if well {
        return Ok(None);
    }
    let why = if hung.load(Ordering::Relaxed) {
        format!("hung for {} s", LONGEST.as_secs())
    } else {
        status.map_or_else(|e| e.to_string(), |s| format!("{s}"))
    };
    Ok(Some((made, why)))
}

/// What such a process does.
fn make(dir: &Path, these: &str) -> Result<()> {
    let these: Vec<usize> = these.split(',').map(|n| n.parse().context("--these")).collect::<Result<_>>()?;
    let cfg = victory_core::Config::load()?;
    victory_hle::known_pipelines::use_translations(cfg.cache_subdir("shaders")?);
    let mut out = std::io::stdout();
    known::make(dir, &these, |n, came| {
        let told = match came {
            None => writeln!(out, "at {n}"),
            Some((ms, there)) => writeln!(out, "made {n} {ms:.2} {}", u8::from(there)),
        };
        if told.and_then(|()| out.flush()).is_err() {
            // Whoever asked is gone.
            std::process::exit(1);
        }
    });
    // Without what a run's end does (nothing to save, the host's threads).
    let _ = out.flush();
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;

    fn fake_child(script: &str, stdout: Stdio) -> Result<Child> {
        let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\cmd.exe");
        Ok(Command::new(cmd)
            .args(["/d", "/c"])
            .raw_arg(script)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::null())
            .spawn()?)
    }

    #[test]
    fn missing_executable_leaves_the_batch_pending() {
        let missing = std::env::temp_dir().join(format!("victory-missing-worker-{}.exe", std::process::id()));
        assert!(!missing.try_exists().unwrap());
        let starts = AtomicUsize::new(0);
        let done = Mutex::new(Vec::new());
        let bad = Mutex::new(Vec::new());
        let result = run_batches(
            &[7, 8, 9],
            1,
            |_| {
                starts.fetch_add(1, Ordering::Relaxed);
                Ok(Command::new(&missing).spawn()?)
            },
            |n, _, _| lock(&done).push(n),
            |n, _| lock(&bad).push(n),
        );
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        assert!(lock(&done).is_empty());
        assert!(lock(&bad).is_empty());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("start warm worker"));
        assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn missing_stdout_is_a_startup_failure() {
        let error = run_batches(
            &[1, 2],
            1,
            |_| fake_child("exit /b 0", Stdio::null()),
            |_, _, _| panic!("unexpected completion"),
            |_, _| panic!("unexpected bad pipeline"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("stdout"));
    }

    #[test]
    fn worker_exit_records_the_failed_pipeline_and_resumes() {
        let done = Mutex::new(Vec::new());
        let bad = Mutex::new(Vec::new());
        run_batches(
            &[7, 8, 9],
            1,
            |rest| match rest {
                [7, 8, 9] => fake_child("echo made 7 3.5 1&exit /b 7", Stdio::piped()),
                [9] => fake_child("echo made 9 2.0 0", Stdio::piped()),
                _ => panic!("unexpected batch {rest:?}"),
            },
            |n, ms, there| lock(&done).push((n, ms, there)),
            |n, why| lock(&bad).push((n, why)),
        )
        .unwrap();
        assert_eq!(*lock(&done), [(7, 3.5, true), (9, 2.0, false)]);
        let bad = lock(&bad);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].0, 8);
        assert!(!bad[0].1.is_empty());
    }

    #[test]
    fn startup_failure_preserves_another_workers_completion() {
        // Hold the failed start until its peer has reported the only key in its chunk.
        let keys: Vec<_> = (0..=AT_ONCE).collect();
        let (confirmed, received) = std::sync::mpsc::channel();
        let received = Mutex::new(received);
        let error = run_batches(
            &keys,
            2,
            |chunk| {
                if chunk[0] == 0 {
                    assert_eq!(lock(&received).recv_timeout(Duration::from_secs(5)).unwrap(), AT_ONCE);
                    anyhow::bail!("worker unavailable")
                }
                fake_child(&format!("echo made {AT_ONCE} 1 1"), Stdio::piped())
            },
            |n, _, _| confirmed.send(n).unwrap(),
            |_, _| panic!("unexpected bad pipeline"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("worker unavailable"));
    }
}
