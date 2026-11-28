//! What is recorded, and when. Every event goes into one buffer made ahead:
//! a write takes a place in it (one atomic addition) and nothing else, so it
//! can be done anywhere, the handler of write faults included.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// What an event is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    /// A span of ours.
    Span,
    /// A run of list calls: `a` calls, `b` ticks inside them.
    Calls,
    /// A wait on the object `a` (an address, or a handle's value); `b`: the
    /// value of the host's fence it stands for (0: none, [`COPY_QUEUE`]).
    Wait,
    /// The object `a` is signalled or released.
    Wake,
    /// A call of the kernel's that took long; `a` its bytes.
    Kernel,
    /// Lists went to the card: `a` the value signalled behind them, `b` the
    /// place of their timestamps plus one.
    Sent,
    /// A present came back: a frame ends.
    Frame,
    /// The title is told something: `b` is stored at `a` (a fence, or what
    /// a list writes when it has run).
    Told,
    /// The title asked a fence (`a`) for its value and got `b`, again and
    /// again from the beginning to the end.
    Poll,
    /// Where a thread of the title's was: `a` the instruction, the stack
    /// what on its stack may be return addresses.
    Sample,
}

/// The `b` of a wait for the card's copy queue.
pub(super) const COPY_QUEUE: u64 = u64::MAX;

#[derive(Clone, Copy, Debug)]
pub(super) struct Event {
    pub(super) t0: u64,
    pub(super) t1: u64,
    pub(super) a: u64,
    pub(super) b: u64,
    pub(super) name: &'static str,
    pub(super) thread: u32,
    /// Its place among the stacks, plus one (0: none).
    pub(super) stack: u32,
    pub(super) cpu: u16,
    pub(super) kind: Kind,
}

struct Slot {
    event: std::cell::UnsafeCell<std::mem::MaybeUninit<Event>>,
    written: AtomicBool,
}

/// Events kept, and stacks.
const EVENTS: usize = 1 << 20;
const STACKS: usize = 1 << 16;
/// Return addresses of a stack.
pub(super) const DEPTH: usize = 14;

static ON: AtomicBool = AtomicBool::new(false);
static SLOTS: AtomicPtr<Slot> = AtomicPtr::new(std::ptr::null_mut());
static NEXT: AtomicUsize = AtomicUsize::new(0);
static STACK: AtomicPtr<[usize; DEPTH]> = AtomicPtr::new(std::ptr::null_mut());
static NEXT_STACK: AtomicUsize = AtomicUsize::new(0);

/// [`ticks`] per second.
pub(super) fn frequency() -> u64 {
    static TICKS: OnceLock<u64> = OnceLock::new();
    *TICKS.get_or_init(|| {
        let mut ticks = 0;
        // SAFETY: a valid out pointer.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut ticks) };
        ticks.max(1) as u64
    })
}

/// The performance counter: the clock the card's is calibrated against.
pub(crate) fn ticks() -> u64 {
    let mut ticks = 0;
    // SAFETY: a valid out pointer.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut ticks) };
    ticks as u64
}

pub(super) fn micros(ticks: u64) -> u64 {
    ticks.saturating_mul(1_000_000) / frequency()
}

/// Whether events are recorded now.
#[inline(always)]
pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub(super) fn thread() -> u32 {
    thread_local!(static ID: Cell<u32> = const { Cell::new(0) });
    ID.with(|id| {
        if id.get() == 0 {
            // SAFETY: a plain query.
            id.set(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() });
        }
        id.get()
    })
}

/// Records an event of this thread.
pub(super) fn put(kind: Kind, name: &'static str, t: (u64, u64), a: u64, b: u64, stack: u32) {
    put_of(thread(), kind, name, t, a, b, stack);
}

/// Records an event of `thread`.
fn put_of(thread: u32, kind: Kind, name: &'static str, (t0, t1): (u64, u64), a: u64, b: u64, stack: u32) {
    let slots = SLOTS.load(Ordering::Acquire);
    if slots.is_null() {
        return;
    }
    let at = NEXT.fetch_add(1, Ordering::Relaxed);
    if at >= EVENTS {
        return;
    }
    // SAFETY: a plain query.
    let cpu = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessorNumber() } as u16;
    let event = Event { t0, t1, a, b, name, thread, stack, cpu, kind };
    // SAFETY: within the buffer, and this place was given to this call alone.
    unsafe {
        let slot = &*slots.add(at);
        (*slot.event.get()).write(event);
        slot.written.store(true, Ordering::Release);
    }
}

/// The caller's stack: its place, plus one.
fn stack() -> u32 {
    let stacks = STACK.load(Ordering::Acquire);
    if stacks.is_null() {
        return 0;
    }
    let at = NEXT_STACK.fetch_add(1, Ordering::Relaxed);
    if at >= STACKS {
        return 0;
    }
    let mut frames = [0usize; DEPTH];
    // SAFETY: room for `DEPTH` return addresses; the place was given to this
    // call alone.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace(
            1,
            DEPTH as u32,
            frames.as_mut_ptr().cast(),
            std::ptr::null_mut(),
        );
        stacks.add(at).write(frames);
    }
    at as u32 + 1
}

/// Shorter spans of ours are not kept (microseconds).
const SHORTEST: u64 = 5;
/// The stack of a wait or a list call this long is kept (microseconds).
const DEEP: u64 = 50;

/// A span of ours: from where it is made until it is dropped.
pub struct Span {
    name: &'static str,
    /// When it began (0: nothing is recorded).
    began: u64,
}

/// A span named `name`.
#[inline(always)]
pub fn span(name: &'static str) -> Span {
    Span {
        name,
        began: if on() { ticks() } else { 0 },
    }
}

impl Span {
    /// The span ends here and another begins.
    #[inline(always)]
    pub fn next(&mut self, name: &'static str) {
        self.close();
        (self.name, self.began) = (name, if on() { ticks() } else { 0 });
    }

    /// The span ends here.
    #[inline(always)]
    pub fn end(self) {}

    fn close(&mut self) {
        let began = std::mem::take(&mut self.began);
        if began != 0 {
            let now = ticks();
            if micros(now - began) >= SHORTEST {
                put(Kind::Span, self.name, (began, now), 0, 0, 0);
            }
        }
    }
}

impl Drop for Span {
    #[inline(always)]
    fn drop(&mut self) {
        if self.began != 0 {
            self.close();
        }
    }
}

/// List calls closer than this (microseconds) are one event.
const RUN_GAP: u64 = 10;

/// A list call of the title's that took `took` has returned.
pub(crate) fn called(took: std::time::Duration) {
    if !on() {
        return;
    }
    let inside = (took.as_nanos() as u64).saturating_mul(frequency()) / 1_000_000_000;
    let t1 = ticks();
    let t0 = t1 - inside;
    thread_local! {
        /// The run under way: from, to, calls, ticks inside them.
        static RUN: Cell<(u64, u64, u64, u64)> = const { Cell::new((0, 0, 0, 0)) };
    }
    RUN.with(|run| {
        let (from, to, calls, sum) = run.get();
        if calls > 0 && micros(t0.saturating_sub(to)) < RUN_GAP {
            run.set((from, t1, calls + 1, sum + inside));
            return;
        }
        if calls > 0 {
            put(Kind::Calls, "list calls", (from, to), calls, sum, 0);
        }
        run.set((t0, t1, 1, inside));
    });
    if micros(inside) >= DEEP {
        put(Kind::Span, "a long list call", (t0, t1), 0, 0, stack());
    }
}

thread_local! {
    /// What the wait this thread makes now stands for ([`card_wait`]).
    static CARD: Cell<u64> = const { Cell::new(0) };
}

/// While it lives, this thread's waits are for the card.
pub(crate) struct CardWait(());

/// This thread's waits are for the card to reach `fence` (`None`: for its
/// copy queue) until what comes back is dropped.
pub(crate) fn card_wait(fence: Option<u64>) -> CardWait {
    CARD.with(|c| c.set(fence.unwrap_or(COPY_QUEUE)));
    CardWait(())
}

impl Drop for CardWait {
    fn drop(&mut self) {
        CARD.with(|c| c.set(0));
    }
}

/// This thread waited on `object` with `name` since `t0`.
pub(super) fn waited(name: &'static str, object: u64, t0: u64) {
    let t1 = ticks();
    let us = micros(t1 - t0);
    if us >= 2 {
        let stack = if us >= DEEP { stack() } else { 0 };
        put(Kind::Wait, name, (t0, t1), object, CARD.with(Cell::get), stack);
    }
}

/// `object` is about to be signalled.
pub(super) fn woken(name: &'static str, object: u64) {
    let t = ticks();
    put(Kind::Wake, name, (t, t), object, 0, 0);
}

/// A call of the kernel's on `bytes` began at `t0`.
pub(super) fn kernel(name: &'static str, bytes: u64, t0: u64) {
    let t1 = ticks();
    if micros(t1 - t0) >= 20 {
        put(Kind::Kernel, name, (t0, t1), bytes, 0, 0);
    }
}

/// Lists went to the card between `t0` ([`ticks`]) and now, the value
/// `fence` signalled behind them, with the timestamps at `stamp`.
pub(crate) fn sent(t0: u64, fence: u64, stamp: Option<usize>) {
    if on() {
        put(Kind::Sent, "ExecuteCommandLists", (t0, ticks()), fence, stamp.map_or(0, |s| s as u64 + 1), 0);
    }
}

/// `value` is being stored at `at`, where the title looks for it.
#[inline(always)]
pub(crate) fn told(name: &'static str, at: usize, value: u64) {
    if on() {
        let t = ticks();
        put(Kind::Told, name, (t, t), at as u64, value, 0);
    }
}

/// The title asked the fence at `at` for its value, which is `value`.
#[inline(always)]
pub(crate) fn polled(at: usize, value: u64) {
    if !on() {
        return;
    }
    thread_local! {
        /// The same answer, again and again: fence, value, from, to.
        static SAME: Cell<(usize, u64, u64, u64)> = const { Cell::new((0, 0, 0, 0)) };
    }
    let t = ticks();
    SAME.with(|same| {
        let (fence, was, from, to) = same.get();
        if (fence, was) == (at, value) && micros(t.saturating_sub(to)) < 1000 {
            same.set((at, value, from, t));
            return;
        }
        if fence != 0 && micros(to.saturating_sub(from)) >= 100 {
            put(Kind::Poll, "asks a fence", (from, to), fence as u64, was, 0);
        }
        same.set((at, value, t, t));
    });
}

/// Threads of the title's that submit or present.
static TITLES: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

/// This thread is one of the title's that submit: what it does is sampled.
#[inline(always)]
pub(crate) fn title_thread() {
    if !on() {
        return;
    }
    let id = thread();
    for place in &TITLES {
        match place.load(Ordering::Relaxed) {
            there if there == id => return,
            0 if place.compare_exchange(0, id, Ordering::Relaxed, Ordering::Relaxed).is_ok() => return,
            _ => {}
        }
    }
}

/// Looks every quarter of a millisecond where the title's submitting threads
/// are, while the recording goes on: what they do outside our calls (a loop
/// on a value in memory has no event of its own).
fn sample() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_CONTROL_AMD64, GetThreadContext};
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_SUSPEND_RESUME,
    };
    /// `GetThreadContext` wants a 16-byte aligned context.
    #[repr(C, align(16))]
    struct Aligned(CONTEXT);
    let mut open: Vec<(u32, *mut std::ffi::c_void)> = Vec::new();
    while on() {
        for place in &TITLES {
            let id = place.load(Ordering::Relaxed);
            if id == 0 {
                continue;
            }
            let thread = match open.iter().find(|o| o.0 == id) {
                Some(o) => o.1,
                None => {
                    // SAFETY: a thread of our own process.
                    let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, 0, id) };
                    open.push((id, handle));
                    handle
                }
            };
            if thread.is_null() {
                continue;
            }
            let mut words = [0usize; 64];
            // SAFETY: zeroed is a valid CONTEXT; the thread is another one of
            // this process and runs on before anything here allocates or
            // locks; its stack is read from its pointer to the end of that
            // page alone.
            let at = unsafe {
                let mut context = Aligned(std::mem::zeroed());
                context.0.ContextFlags = CONTEXT_CONTROL_AMD64;
                if SuspendThread(thread) == u32::MAX {
                    continue;
                }
                let known = GetThreadContext(thread, &mut context.0) != 0;
                if known {
                    let sp = context.0.Rsp as usize & !7;
                    let n = ((sp | 0xfff) + 1 - sp) / 8;
                    for (i, word) in words.iter_mut().enumerate().take(n) {
                        *word = (sp as *const usize).add(i).read_volatile();
                    }
                }
                ResumeThread(thread);
                if known { context.0.Rip as usize } else { 0 }
            };
            let t = ticks();
            // What lies near the instruction may be a return address; where
            // the instruction is the system's, all of it may tell something.
            let mut frames = [0usize; DEPTH];
            let near = words.iter().copied().filter(|w| *w != 0 && (at >> 40 == 0x7f || w.abs_diff(at) < 0x1000_0000));
            for (frame, word) in frames.iter_mut().zip(near) {
                *frame = word;
            }
            put_of(id, Kind::Sample, "a sample", (t, t), at as u64, 0, keep(frames));
        }
        let until = ticks() + frequency() / 4000;
        while ticks() < until {
            std::hint::spin_loop();
        }
    }
    for (_, handle) in open {
        // SAFETY: opened above.
        unsafe { CloseHandle(handle) };
    }
}

/// `frames` among the stacks: its place, plus one.
fn keep(frames: [usize; DEPTH]) -> u32 {
    let stacks = STACK.load(Ordering::Acquire);
    let at = NEXT_STACK.fetch_add(1, Ordering::Relaxed);
    if stacks.is_null() || at >= STACKS {
        return 0;
    }
    // SAFETY: the place was given to this call alone.
    unsafe { stacks.add(at).write(frames) };
    at as u32 + 1
}

/// What was asked for.
struct Asked {
    file: std::path::PathBuf,
    /// Seconds after which a recording begins, for each.
    after: Vec<f64>,
    frames: u64,
    started: crate::clock::Instant,
}

/// `<from s>[+<from s>..],<frames>,<file>`, or a file alone.
fn parse(text: &str) -> (Option<(Vec<f64>, u64)>, &str) {
    let mut parts = text.splitn(3, ',');
    let three = (parts.next(), parts.next(), parts.next());
    if let (Some(from), Some(frames), Some(file)) = three
        && let Ok(from) = from.split('+').map(|f| f.trim().parse::<f64>()).collect::<Result<Vec<f64>, _>>()
        && let Ok(frames) = frames.trim().parse::<u64>()
    {
        return (Some((from, frames)), file);
    }
    (None, text)
}

/// The variables, read once (before the title runs: its imports are bound
/// by them).
fn asked() -> Option<&'static Asked> {
    static ASKED: OnceLock<Option<Asked>> = OnceLock::new();
    ASKED
        .get_or_init(|| {
            let file = std::env::var_os("VICTORY_TIMELINE").filter(|v| !v.is_empty())?;
            let number = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<f64>().ok());
            let text = file.to_string_lossy();
            let (when, file) = parse(&text);
            let (after, frames) = when.unwrap_or_else(|| {
                (
                    vec![number("VICTORY_TIMELINE_AT").unwrap_or(60.0)],
                    number("VICTORY_TIMELINE_FRAMES").map_or(3, |n| n as u64),
                )
            });
            Some(Asked {
                file: file.into(),
                after,
                frames: frames.clamp(1, 60),
                started: crate::clock::Instant::now(),
            })
        })
        .as_ref()
}

/// Whether a timeline was asked for.
pub(super) fn wanted() -> bool {
    asked().is_some()
}

/// Where the recording is.
struct Stage {
    ready: bool,
    began: bool,
    frames: u64,
    /// Recordings made, and whether one is being written.
    made: usize,
    writing: bool,
    before: Option<super::report::Threads>,
}

/// The title's present came back.
pub(crate) fn presented() {
    let Some(asked) = asked() else {
        return;
    };
    static STAGE: Mutex<Stage> = Mutex::new(Stage {
        ready: false,
        began: false,
        frames: 0,
        made: 0,
        writing: false,
        before: None,
    });
    let mut stage = STAGE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(&after) = asked.after.get(stage.made) else {
        return;
    };
    if stage.writing || asked.started.elapsed().as_secs_f64() < after {
        return;
    }
    if !stage.ready {
        // The buffers, the card's lists and the threads' clocks take a
        // while: the recording begins two frames later.
        stage.ready = true;
        make_room();
        crate::host::timeline_ready();
        return;
    }
    if !stage.began {
        stage.began = true;
        stage.before = Some(super::report::Threads::now());
        tracing::info!(file = %asked.file.display(), frames = asked.frames, "the timeline begins (VICTORY_TIMELINE)");
        begin();
        if std::thread::Builder::new().name("victory-sample".into()).spawn(sample).is_err() {
            tracing::warn!("the timeline is without samples of the title's threads");
        }
        frame();
        return;
    }
    frame();
    stage.frames += 1;
    if stage.frames < asked.frames {
        return;
    }
    stage.writing = true;
    let before = stage.before.take().unwrap_or_default();
    // Several recordings: a file each.
    let file = match asked.after.len() {
        1 => asked.file.clone(),
        _ => {
            let end = asked.file.extension().map_or("json".into(), |e| e.to_string_lossy().into_owned());
            asked.file.with_extension(format!("{after:.0}.{end}"))
        }
    };
    drop(stage);
    let write = move || {
        let after = super::report::Threads::now();
        // The spans under way at the last present end first.
        std::thread::sleep(std::time::Duration::from_millis(250));
        let data = end();
        super::report::write(&file, data, &before, &after, crate::host::timeline_stamps());
        crate::host::timeline_restart();
        let mut stage = STAGE.lock().unwrap_or_else(|e| e.into_inner());
        (stage.began, stage.frames, stage.writing) = (false, 0, false);
        stage.made += 1;
    };
    if std::thread::Builder::new().name("victory-timeline".into()).spawn(write).is_err() {
        tracing::warn!("no thread to write the timeline");
    }
}

/// A frame ends now.
pub(super) fn frame() {
    let t = ticks();
    put(Kind::Frame, "present", (t, t), 0, 0, 0);
}

/// Makes the buffers (kept for good: a thread may be about to write).
pub(super) fn make_room() {
    if !SLOTS.load(Ordering::Acquire).is_null() {
        return;
    }
    fn zeroed<T>(n: usize) -> *mut T {
        let layout = std::alloc::Layout::array::<T>(n).expect("a small number");
        // SAFETY: not of no size.
        unsafe { std::alloc::alloc_zeroed(layout) }.cast()
    }
    STACK.store(zeroed(STACKS), Ordering::Release);
    SLOTS.store(zeroed(EVENTS), Ordering::Release);
}

/// Recording starts.
pub(super) fn begin() {
    ON.store(true, Ordering::SeqCst);
}

/// What was recorded, in no order.
pub(super) struct Data {
    pub(super) events: Vec<Event>,
    pub(super) stacks: Vec<[usize; DEPTH]>,
    /// Events there was no room for.
    pub(super) lost: usize,
}

/// Recording ends.
pub(super) fn end() -> Data {
    ON.store(false, Ordering::SeqCst);
    // Those about to write do.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let taken = NEXT.load(Ordering::SeqCst);
    TITLES.iter().for_each(|t| t.store(0, Ordering::SeqCst));
    let slots = SLOTS.load(Ordering::Acquire);
    let mut events = Vec::with_capacity(taken.min(EVENTS));
    for at in 0..taken.min(EVENTS) {
        // SAFETY: within the buffer; a written place is never written again.
        unsafe {
            let slot = &*slots.add(at);
            if slot.written.load(Ordering::Acquire) {
                events.push((*slot.event.get()).assume_init());
                slot.written.store(false, Ordering::Release);
            }
        }
    }
    NEXT.store(0, Ordering::SeqCst);
    let count = NEXT_STACK.swap(0, Ordering::SeqCst).min(STACKS);
    // SAFETY: that many were given out; each was written before the event
    // that names it.
    let stacks = unsafe { std::slice::from_raw_parts(STACK.load(Ordering::Acquire), count) }.to_vec();
    Data {
        events,
        stacks,
        lost: taken.saturating_sub(EVENTS),
    }
}

/// Objects threads stand at now, by a hash of theirs: releases of a lock
/// nobody waits for are not recorded.
static STOOD_AT: [AtomicU32; 4096] = [const { AtomicU32::new(0) }; 4096];

fn place(object: u64) -> &'static AtomicU32 {
    &STOOD_AT[(object.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 52) as usize]
}

/// This thread is about to wait for `object`, until what comes back is
/// dropped.
pub(super) fn standing(object: u64) -> Standing {
    place(object).fetch_add(1, Ordering::SeqCst);
    Standing(object)
}

/// See [`standing`].
pub(super) struct Standing(u64);

impl Drop for Standing {
    fn drop(&mut self) {
        place(self.0).fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether a thread may wait for `object`.
pub(super) fn stood_at(object: u64) -> bool {
    place(object).load(Ordering::SeqCst) != 0
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn a_timeline_is_asked_for_by_seconds_frames_and_a_file() {
        assert_eq!(parse("120,20,D:/a.json"), (Some((vec![120.0], 20)), "D:/a.json"));
        assert_eq!(parse("120+130.5,20,D:/a,b.json"), (Some((vec![120.0, 130.5], 20)), "D:/a,b.json"));
        assert_eq!(parse("D:/a,1,2.json"), (None, "D:/a,1,2.json"));
        assert_eq!(parse("D:/a.json"), (None, "D:/a.json"));
    }
}
