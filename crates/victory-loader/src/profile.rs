//! Where the process's time goes: `VICTORY_PROFILE=<from>,<to>,<ms>,<file>`
//! samples the stacks of the busiest threads every `<ms>` milliseconds
//! between two times (seconds, as `VICTORY_STACKS`), and writes them counted
//! to `<file>` at the end (several of them, one after another, with `;`
//! between). The times are from the one start, so a window whose turn
//! comes after its `from` (an earlier profile ran into it) covers only
//! the rest of it and says so, and a window whose turn comes at or after
//! its `to` writes nothing and says so. A thread that waits is sampled like one that
//! runs: its top frame says which (`ntdll`'s system calls).
//!
//! The file: `thread <id> <start> <cpu ms> <samples> <name>`, then
//! `stack <thread> <phase> <count> <frame>;<frame>;...` (innermost first, as
//! `module+rva`), where the phase is what the caller's function says of the
//! frame at that moment.

use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetThreadTimes, OpenThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

use crate::crash::locate;
use crate::stacks::{frames_of, name_of, other_threads, start_of};

/// The threads sampled: those that ran most since they were last chosen.
const BUSIEST: usize = 28;
/// How often they are chosen anew.
const CHOOSE: Duration = Duration::from_secs(1);

/// What `VICTORY_PROFILE` says: from, to, every, the file.
fn asked(list: &str) -> Option<(Duration, Duration, Duration, String)> {
    let mut parts = list.splitn(4, ',');
    let mut seconds = || parts.next()?.trim().parse::<f64>().ok();
    let (from, to, ms) = (seconds()?, seconds()?, seconds()?);
    let of = |s: f64| Duration::try_from_secs_f64(s).ok();
    let file = parts.next()?.trim().to_string();
    Some((of(from)?, of(to)?, of(ms / 1000.0)?, file)).filter(|(a, b, ms, f)| a < b && !ms.is_zero() && !f.is_empty())
}

struct Thread {
    handle: HANDLE,
    start: String,
    name: String,
    /// Its processor time, 100 ns: at first, when last chosen, now.
    first: u64,
    last: u64,
    now: u64,
    samples: u64,
}

fn cpu(thread: HANDLE) -> u64 {
    // SAFETY: zeroed is a valid FILETIME, and the calls write them.
    let mut t: [FILETIME; 4] = unsafe { std::mem::zeroed() };
    let [a, b, c, d] = &mut t;
    // SAFETY: an open thread; the pointers are to the structures above.
    unsafe { GetThreadTimes(thread, a, b, c, d) };
    let of = |f: &FILETIME| u64::from(f.dwHighDateTime) << 32 | u64::from(f.dwLowDateTime);
    of(&t[2]) + of(&t[3])
}

/// The windows a `VICTORY_PROFILE` list asks for, and the entries that
/// could not be read as one (an empty entry, as from a trailing `;`, is
/// neither): the unreadable ones are named, not dropped quietly, so a
/// missing profile is not taken for one that ran.
fn schedule(list: &str) -> (Vec<(Duration, Duration, Duration, String)>, Vec<String>) {
    let mut all = Vec::new();
    let mut unread = Vec::new();
    for entry in list.split(';') {
        if entry.trim().is_empty() {
            continue;
        }
        match asked(entry) {
            Some(one) => all.push(one),
            None => unread.push(entry.trim().to_string()),
        }
    }
    (all, unread)
}

/// Where one window stands when its turn comes. Every window is measured
/// against the one `start`, so a window whose turn comes after its `from`
/// (an earlier profile ran into it) covers only what is left of it, and
/// one whose turn comes at or after its `to` covers nothing.
#[derive(Debug, PartialEq, Eq)]
enum Turn {
    OnTime,
    Late,
    Elapsed,
}

fn turn(from: Duration, to: Duration, now: Duration) -> Turn {
    if now >= to {
        Turn::Elapsed
    } else if now > from {
        Turn::Late
    } else {
        Turn::OnTime
    }
}

/// Reads `VICTORY_PROFILE` and, if set, starts the thread that samples.
/// `phase` is asked at each round.
pub fn profile_on_schedule(phase: fn() -> u32) {
    let Ok(list) = std::env::var("VICTORY_PROFILE") else {
        return;
    };
    let start = Instant::now();
    let (all, unread) = schedule(&list);
    for entry in unread {
        eprintln!("VIctory: a profile was not understood and is skipped: {entry}");
    }
    std::thread::spawn(move || {
        for one in all {
            profile(one, start, phase);
        }
    });
}

fn profile((from, to, every, file): (Duration, Duration, Duration, String), start: Instant, phase: fn() -> u32) {
    std::thread::sleep(from.saturating_sub(start.elapsed()));
    match turn(from, to, start.elapsed()) {
        Turn::Elapsed => {
            // Nothing is written. A file already there is an earlier
            // run's, and is left as it is — but then the message must
            // not claim the name is empty.
            if std::path::Path::new(&file).exists() {
                eprintln!(
                    "VIctory: no new profile is in {file}: its window ({:.3} to {:.3} s) had already passed when its turn came; the file there is from an earlier run and was not touched",
                    from.as_secs_f64(),
                    to.as_secs_f64()
                );
            } else {
                eprintln!(
                    "VIctory: no profile is in {file}: its window ({:.3} to {:.3} s) had already passed when its turn came",
                    from.as_secs_f64(),
                    to.as_secs_f64()
                );
            }
            return;
        }
        Turn::Late => {
            eprintln!(
                "VIctory: the profile in {file} begins late (at {:.3} s of {:.3} to {:.3} s): it covers only the rest of its window",
                start.elapsed().as_secs_f64(),
                from.as_secs_f64(),
                to.as_secs_f64()
            );
        }
        Turn::OnTime => {}
    }
    eprintln!("VIctory: the profile begins");
    let mut threads: HashMap<u32, Thread> = HashMap::new();
    let mut chosen: Vec<u32> = Vec::new();
    let mut counts: HashMap<(u32, u32, Vec<u64>), u64> = HashMap::new();
    let mut phases: HashMap<u32, u64> = HashMap::new();
    let mut choose_at = Instant::now();
    let mut rounds = 0u64;
    while start.elapsed() < to {
        let round = Instant::now();
        if round >= choose_at {
            choose_at = round + CHOOSE;
            choose(&mut threads, &mut chosen);
        }
        let phase = phase();
        *phases.entry(phase).or_default() += 1;
        for id in &chosen {
            if let Some(t) = threads.get_mut(id) {
                let (frames, _) = frames_of(t.handle);
                t.samples += 1;
                if !frames.is_empty() {
                    *counts.entry((*id, phase, frames)).or_default() += 1;
                }
            }
        }
        rounds += 1;
        std::thread::sleep(every.saturating_sub(round.elapsed()));
    }
    choose(&mut threads, &mut chosen);
    if let Err(e) = write(&file, &threads, &counts, &phases, rounds) {
        eprintln!("VIctory: the profile was not written: {e}");
    } else {
        eprintln!("VIctory: the profile is in {file}");
    }
}

/// Opens the threads that are new, and chooses the ones to sample.
fn choose(threads: &mut HashMap<u32, Thread>, chosen: &mut Vec<u32>) {
    for id in other_threads() {
        threads.entry(id).or_insert_with(|| {
            // SAFETY: opens a thread of our own process; kept open for the
            // profile's life, so the id stays its own.
            let handle =
                unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, id) };
            let first = if handle.is_null() { 0 } else { cpu(handle) };
            Thread {
                handle,
                start: if handle.is_null() { String::new() } else { locate(start_of(handle)) },
                name: if handle.is_null() { String::new() } else { name_of(handle) },
                first,
                last: first,
                now: first,
                samples: 0,
            }
        });
    }
    let mut ran: Vec<(u64, u32)> = Vec::new();
    for (&id, t) in threads.iter_mut().filter(|(_, t)| !t.handle.is_null()) {
        t.now = cpu(t.handle);
        ran.push((t.now - t.last, id));
        t.last = t.now;
    }
    ran.sort_unstable_by(|a, b| b.cmp(a));
    *chosen = ran.into_iter().take(BUSIEST).map(|(_, id)| id).collect();
}

fn write(
    file: &str,
    threads: &HashMap<u32, Thread>,
    counts: &HashMap<(u32, u32, Vec<u64>), u64>,
    phases: &HashMap<u32, u64>,
    rounds: u64,
) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(file)?);
    writeln!(out, "rounds {rounds}")?;
    for (phase, n) in phases {
        writeln!(out, "phase {phase} {n}")?;
    }
    let mut names: HashMap<u64, String> = HashMap::new();
    for (id, t) in threads {
        writeln!(out, "thread {id} {} {} {} {}", t.start, (t.now - t.first) / 10_000, t.samples, t.name)?;
        if !t.handle.is_null() {
            // SAFETY: opened by `choose`.
            unsafe { CloseHandle(t.handle) };
        }
    }
    for ((id, phase, frames), n) in counts {
        write!(out, "stack {id} {phase} {n} ")?;
        for f in frames {
            let name = names.entry(*f).or_insert_with(|| locate(*f as usize));
            write!(out, "{name};")?;
        }
        writeln!(out)?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_asked_is_read_whole_or_not_at_all() {
        assert_eq!(
            asked("150, 160.5,5,D:\\a b,c.txt"),
            Some((
                Duration::from_secs(150),
                Duration::from_millis(160_500),
                Duration::from_millis(5),
                "D:\\a b,c.txt".to_string()
            ))
        );
        assert_eq!(asked("150,160,5"), None);
        assert_eq!(asked("160,150,5,x"), None);
        assert_eq!(asked("a,160,5,x"), None);
        assert_eq!(asked("-1,160,5,x"), None);
        assert_eq!(asked("1,160,0,x"), None);
    }

    #[test]
    fn a_windows_turn_is_judged_against_the_one_start() {
        let s = Duration::from_secs;
        // A single window, reached before it opens and as it opens.
        assert_eq!(turn(s(10), s(20), s(0)), Turn::OnTime);
        assert_eq!(turn(s(10), s(20), s(10)), Turn::OnTime);
        // Sequential, non-overlapping: the second window's turn comes at
        // 60 s (the first window's `to`), exactly its own `from`.
        assert_eq!(turn(s(60), s(90), s(60)), Turn::OnTime);
        // Partially overlapping (`0,60,…;30,90,…`): the second window's
        // turn comes at 60 s, halfway through it.
        assert_eq!(turn(s(30), s(90), s(60)), Turn::Late);
        assert_eq!(turn(s(30), s(90), s(89)), Turn::Late);
        // Completely overlapping, and a deadline already elapsed: the
        // turn comes at or after `to`.
        assert_eq!(turn(s(10), s(50), s(60)), Turn::Elapsed);
        assert_eq!(turn(s(30), s(90), s(90)), Turn::Elapsed);
        assert_eq!(turn(s(30), s(90), s(120)), Turn::Elapsed);
    }

    #[test]
    fn a_schedule_reads_its_windows_and_names_the_rest() {
        let (all, unread) = schedule("0,60,5,a.txt;60,90,5,b.txt");
        assert_eq!(all.len(), 2);
        assert!(unread.is_empty());

        let (all, unread) = schedule("0,60,5,a.txt;nonsense;90,60,5,b.txt;");
        assert_eq!(all.len(), 1);
        assert_eq!(unread, ["nonsense", "90,60,5,b.txt"]);

        // Empty input and empty entries ask for nothing and hide nothing.
        assert_eq!(schedule(""), (Vec::new(), Vec::new()));
        assert_eq!(schedule(";;"), (Vec::new(), Vec::new()));
        let (all, unread) = schedule("0,60,5,a.txt;");
        assert_eq!(all.len(), 1);
        assert!(unread.is_empty());
    }

    fn temp_file(name: &str) -> String {
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        path.to_string_lossy().into_owned()
    }

    /// A window whose deadline passed before its turn came (an earlier
    /// profile ran past it) must not leave a file that looks like a
    /// profile: nothing was sampled in it.
    #[test]
    fn a_window_that_has_already_passed_writes_no_profile() {
        let file = temp_file("victory-profile-elapsed.txt");
        let start = Instant::now() - Duration::from_millis(200);
        profile(
            (
                Duration::from_millis(30),
                Duration::from_millis(90),
                Duration::from_millis(5),
                file.clone(),
            ),
            start,
            || 0,
        );
        assert!(
            !std::path::Path::new(&file).exists(),
            "an elapsed window wrote a profile file"
        );
    }

    /// An elapsed window also leaves an earlier run's file at the same
    /// name exactly as it was: not truncated, not overwritten.
    #[test]
    fn a_window_that_has_already_passed_leaves_an_earlier_file_alone() {
        let file = temp_file("victory-profile-stale.txt");
        std::fs::write(&file, "rounds 99\nfrom an earlier run\n").expect("the earlier file is written");
        let start = Instant::now() - Duration::from_millis(200);
        profile(
            (
                Duration::from_millis(30),
                Duration::from_millis(90),
                Duration::from_millis(5),
                file.clone(),
            ),
            start,
            || 0,
        );
        let text = std::fs::read_to_string(&file).expect("the earlier file is still there");
        let _ = std::fs::remove_file(&file);
        assert_eq!(text, "rounds 99\nfrom an earlier run\n");
    }

    /// A window that begins late still samples until its (absolute)
    /// deadline, and what it writes says it sampled: rounds above zero.
    #[test]
    fn a_window_that_begins_late_samples_until_its_deadline() {
        let file = temp_file("victory-profile-late.txt");
        let start = Instant::now() - Duration::from_millis(60);
        profile(
            (
                Duration::from_millis(30),
                Duration::from_millis(400),
                Duration::from_millis(5),
                file.clone(),
            ),
            start,
            || 0,
        );
        let text = std::fs::read_to_string(&file).expect("a late window still writes its profile");
        let _ = std::fs::remove_file(&file);
        assert!(
            !text.starts_with("rounds 0"),
            "a late window sampled nothing: {text}"
        );
    }
}
