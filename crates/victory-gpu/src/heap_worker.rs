//! The pass that uploads textures picked by index, on a thread of its own
//! (`VICTORY_HEAP_WORKER=1`; off by default; needs `VICTORY_LATE_WAIT`).
//!
//! [`crate::draw::State::heap_textures`] runs on whichever thread of the
//! title records the frame's first shader that picks textures by index: it
//! asks what of each due texture is mapped, makes the host texture, makes a
//! staging buffer and records the copy, up to 250 ms of every frame while a
//! backlog waits, and the threads waiting for that list wait with it. The
//! detiling of what it recorded then runs on the thread that submits. With
//! the switch the pass runs here, and the recording thread only wakes it.
//!
//! What keeps the picture as it was:
//! - the uploads are recorded into host lists of ours ([`Batch`]), which the
//!   next submission of the title's runs ahead of its own lists
//!   ([`ahead`]): texture states are settled by submission order, as
//!   between any two lists;
//! - a submission waits for the passes begun before it ([`take_ready`]), so
//!   no list of the title runs before the uploads it may need;
//! - a texture's host slot is given its view when the upload's list runs
//!   ([`Batch::publish`]), not before: until then shaders see what the slot
//!   showed, never a texture without its contents. A slot the title wrote
//!   again meanwhile is left to the write's own turn in the queue;
//! - the detiling done here is checked at submission as any texture's is:
//!   memory the title wrote in between is read again;
//! - a title texture the pass works on is held ([`crate::resource::hold`]),
//!   and so is the title's heap;
//! - the uploads before the title's decoration bake, which must be there
//!   for that draw, still run on the recording thread; the worker makes way
//!   for them ([`urgent`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::{Acquire, Relaxed, Release}};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use crate::descriptor::Heap;
use crate::draw::Sink;
use crate::host::HostList;
use crate::list::Detached;
use crate::object::This;

/// Whether the pass runs on its own thread now.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_WORKER").is_some_and(|v| v == "1")) && crate::host::late()
}

/// For the frame line, since the last one: nanoseconds the worker was at
/// work, of them detiling; lists given to submissions; textures detiled
/// ahead in them.
pub(crate) static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// For the frame line, since the last one: microseconds the submissions
/// waited for the passes begun before them ([`take_ready`]).
pub(crate) static WAIT_US: AtomicU64 = AtomicU64::new(0);

/// One upload list holds at most this much texture memory, or this many
/// milliseconds of work, so that its textures show soon.
const LIST_BYTES: u64 = 48 << 20;
const LIST_MS: u128 = 40;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Holds a reference to a title heap.
struct HeldHeap(This);

// SAFETY: a reference counted title object, of which only the count and the
// parts behind locks are touched.
unsafe impl Send for HeldHeap {}

impl HeldHeap {
    fn new(heap: &Heap) -> Self {
        let this: This = std::ptr::from_ref(heap).cast();
        crate::object::add_ref(this);
        HeldHeap(this)
    }

    fn heap(&self) -> &Heap {
        crate::descriptor::heap(self.0)
    }

    fn is(&self, heap: &Heap) -> bool {
        self.0 as usize == std::ptr::from_ref(heap) as usize
    }
}

impl Clone for HeldHeap {
    fn clone(&self) -> Self {
        crate::object::add_ref(self.0);
        HeldHeap(self.0)
    }
}

impl Drop for HeldHeap {
    fn drop(&mut self) {
        crate::object::release_boxed::<Heap>(self.0);
    }
}

/// A closed upload list, until a submission takes it.
struct Closed(HostList);

// SAFETY: D3D12 objects are free-threaded; the list is closed and touched by
// nobody until it is taken out.
unsafe impl Send for Closed {}

static READY: Mutex<Vec<Closed>> = Mutex::new(Vec::new());

/// What the pass records into: one upload list after the other, each made
/// when first needed and closed when it holds enough.
pub(crate) struct Batch {
    heap: HeldHeap,
    list: Option<HostList>,
    bytes: u64,
    began: Instant,
}

impl Batch {
    fn new(heap: &Heap) -> Self {
        Batch { heap: HeldHeap::new(heap), list: None, bytes: 0, began: Instant::now() }
    }

    /// The open upload list.
    pub(crate) fn list(&mut self) -> Option<&HostList> {
        if self.list.is_none() {
            self.list = crate::list::fresh_list();
            self.began = Instant::now();
        }
        self.list.as_ref()
    }

    /// Slot `index` shows the record at `handle` when the open list runs, if
    /// the record is still the one with `key` then.
    pub(crate) fn publish(&mut self, index: u32, handle: u64, key: u64) {
        let heap = self.heap.clone();
        if let Some(list) = self.list() {
            list.defer_host(move || {
                if let Some(host) = heap.heap().host() {
                    let _ = host.publish(index, handle, key);
                }
            });
        }
    }

    /// `bytes` more of texture memory in the open list, which is closed and
    /// handed over when it holds enough.
    pub(crate) fn uploaded(&mut self, bytes: u64) {
        self.bytes += bytes;
        if self.bytes >= LIST_BYTES || self.began.elapsed().as_millis() >= LIST_MS {
            self.flush();
        }
    }

    /// Detiles what the open list uploads, closes it and hands it to the
    /// next submission.
    fn flush(&mut self) {
        self.bytes = 0;
        let Some(list) = self.list.take() else {
            return;
        };
        let started = Instant::now();
        let textures = list.refresh_textures_ahead();
        STATS[1].fetch_add(started.elapsed().as_nanos() as u64, Relaxed);
        STATS[2].fetch_add(1, Relaxed);
        STATS[3].fetch_add(textures, Relaxed);
        list.close();
        lock(&READY).push(Closed(list));
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Passes begun and finished, by ticket: a submission waits for those begun
/// before it.
struct Passes {
    begun: AtomicU64,
    done: AtomicU64,
    wait: Mutex<()>,
    finished: Condvar,
}

static PASSES: Passes = Passes {
    begun: AtomicU64::new(0),
    done: AtomicU64::new(0),
    wait: Mutex::new(()),
    finished: Condvar::new(),
};

impl Passes {
    /// A pass begun now: its ticket.
    fn begin(&self) -> u64 {
        self.begun.fetch_add(1, Relaxed) + 1
    }

    /// The pass with `ticket` is finished.
    fn finish(&self, ticket: u64) {
        let _wait = lock(&self.wait);
        self.done.fetch_max(ticket, Release);
        self.finished.notify_all();
    }

    /// Waits until the passes begun before this call are finished.
    fn wait_for_begun(&self) {
        let begun = self.begun.load(Acquire);
        if self.done.load(Acquire) >= begun {
            return;
        }
        let mut wait = lock(&self.wait);
        while self.done.load(Acquire) < begun {
            wait = self.finished.wait(wait).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// The upload lists closed by the passes, once the passes begun before this
/// call are finished, in the order they were closed.
pub(crate) fn take_ready() -> Vec<HostList> {
    if !on() {
        return Vec::new();
    }
    let waited = Instant::now();
    PASSES.wait_for_begun();
    WAIT_US.fetch_add(waited.elapsed().as_micros() as u64, Relaxed);
    lock(&READY).drain(..).map(|Closed(list)| list).collect()
}

/// [`take_ready`] as the runs of a submission, which goes ahead of the
/// title's own lists.
pub(crate) fn ahead() -> Option<(Vec<Detached>, Vec<usize>)> {
    let lists = take_ready();
    if lists.is_empty() {
        return None;
    }
    let order = (0..lists.len()).collect();
    Some((lists.into_iter().map(Detached::ours).collect(), order))
}

/// The heap the next pass is over, with the ticket of its pass, and what
/// wakes the worker.
struct Job {
    heap: Mutex<Option<(HeldHeap, u64)>>,
    wake: Condvar,
}

static JOB: Job = Job { heap: Mutex::new(None), wake: Condvar::new() };
static STARTED: OnceLock<bool> = OnceLock::new();
/// Held by whoever runs the pass.
static PASS: Mutex<()> = Mutex::new(());
/// A recording thread waits for [`PASS`].
static URGENT: AtomicBool = AtomicBool::new(false);

/// Starts the worker's thread, once.
fn started() -> bool {
    *STARTED.get_or_init(|| match std::thread::Builder::new().name("victory-heap".into()).spawn(|| worker(&JOB)) {
        Ok(_) => {
            tracing::info!("textures picked by index are uploaded by a thread of their own (VICTORY_HEAP_WORKER)");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "no thread for textures picked by index");
            false
        }
    })
}

/// A pass over `heap` is due (once a frame), and the worker runs it. False
/// if there is no worker, or it has another heap's pass pending: the caller
/// runs its own pass. A pass pending for `heap` covers this one.
pub(crate) fn kick(heap: &Heap) -> bool {
    if !on() || !started() {
        return false;
    }
    let mut next = lock(&JOB.heap);
    if let Some((pending, _)) = next.as_ref() {
        return pending.is(heap);
    }
    let ticket = PASSES.begin();
    *next = Some((HeldHeap::new(heap), ticket));
    drop(next);
    JOB.wake.notify_one();
    true
}

/// Takes the pass from the worker for the caller, which cannot wait for a
/// later submission: the worker stops at its next look ([`make_way`]).
pub(crate) fn urgent() -> MutexGuard<'static, ()> {
    if let Ok(pass) = PASS.try_lock() {
        return pass;
    }
    URGENT.store(true, Relaxed);
    let pass = lock(&PASS);
    URGENT.store(false, Relaxed);
    pass
}

/// Whether the worker is to stop where it is ([`urgent`]).
pub(crate) fn make_way() -> bool {
    URGENT.load(Relaxed)
}

/// The pass with its ticket is finished when this is dropped, even by a
/// panic, so that a submission never waits for one that cannot end.
struct Finished(u64);

impl Drop for Finished {
    fn drop(&mut self) {
        PASSES.finish(self.0);
    }
}

fn worker(job: &'static Job) {
    loop {
        let (pending, ticket) = {
            let mut next = lock(&job.heap);
            loop {
                match next.take() {
                    Some(pending) => break pending,
                    None => next = job.wake.wait(next).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        let _finished = Finished(ticket);
        let started = Instant::now();
        {
            let _pass = lock(&PASS);
            let heap = pending.heap();
            if let Some(host) = heap.host() {
                let mut batch = Batch::new(heap);
                crate::draw::State::default().heap_textures(Sink::Ahead(&mut batch), heap, host);
            }
        }
        STATS[0].fetch_add(started.elapsed().as_nanos() as u64, Relaxed);
        // Whoever wants the pass now has it first.
        while make_way() {
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A submission waits for a pass begun before it, not only for the ones
    /// it finds running.
    #[test]
    fn a_submission_waits_for_the_pass_begun_before_it() {
        let passes = Passes {
            begun: AtomicU64::new(0),
            done: AtomicU64::new(0),
            wait: Mutex::new(()),
            finished: Condvar::new(),
        };
        let ticket = passes.begin();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(200));
                passes.finish(ticket);
            });
            passes.wait_for_begun();
            assert!(passes.done.load(Acquire) >= ticket, "returned before the pass was finished");
        });
    }
}
