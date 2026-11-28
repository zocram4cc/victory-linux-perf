//! Optional accounting for the existing moving scene. A changed descriptor
//! is not necessarily an upload (old host copy, a view already tabled, etc.).
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_ACTION_MEMORY").is_some_and(|v| v == "1"))
}

// A whole 1s bin of work, not a sample of the rate-limited warnings.
// changed, first stamp unknown, notified, same host copy, ahead, tabled,
// staged bytes, recording usec, refresh usec and bytes, copies, fault poll usec.
static COUNTS: [AtomicU64; 12] = [const { AtomicU64::new(0) }; 12];
pub(crate) fn add(n: usize, v: u64) {
    COUNTS[n].fetch_add(v, Relaxed);
}

pub(crate) struct Changed(crate::clock::Instant);
impl Changed {
    pub(crate) fn new(pending: bool, notified: bool) -> Self {
        add(0, 1);
        add(1, pending as u64);
        add(2, notified as u64);
        Self(crate::clock::Instant::now())
    }
}
impl Drop for Changed {
    fn drop(&mut self) {
        add(7, self.0.elapsed().as_micros() as u64);
    }
}

pub(crate) fn fault(page: u64) {
    if !on() {
        return;
    }
    static SEEN: OnceLock<Mutex<std::collections::HashSet<u64>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.len() >= 128 || !seen.insert(page) {
        return;
    }
    drop(seen);
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let at = page * 65536;
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { VirtualQuery(at as *const _, &mut m, size_of_val(&m)) } != 0;
    tracing::info!(addr=format_args!("{at:#x}"),ok,base=?m.BaseAddress,reservation=?m.AllocationBase,state=format_args!("{:#x}",m.State),prot=format_args!("{:#x}",m.Protect),span=m.RegionSize,
        shown=crate::inplace::shown_bytes(at as usize,65536),um=crate::um::holds(at as usize,65536),mirror=crate::um::covers(at,65536),buffer=?crate::shadow::in_buffer(at,1),card=crate::um::touches_card(at as usize,65536),"first unresolved shader page, backing");
}

pub(crate) fn line() {
    if !on() {
        return;
    }
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::host::now_ms();
    let old = LAST.load(Relaxed);
    if now < old + 1000 || LAST.compare_exchange(old, now, Relaxed, Relaxed).is_err() {
        return;
    }
    let counts = COUNTS.each_ref().map(|v| v.swap(0, Relaxed));
    tracing::info!(
        frame = crate::frame::current(),
        ?counts,
        "indexed rechecks and shader fault poll per interval"
    );
}


pub(crate) fn room_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_ROOM_TIMING").is_some_and(|v| v == "1"))
}

pub(crate) struct Pass {
    start: crate::clock::Instant,
    frame: u64,
    pub(crate) full: bool,
    pub(crate) refused: u64,
    pub(crate) due: usize,
    pub(crate) reused: u32,
    pub(crate) bake: bool,
    pub(crate) admitted: [u32; 3],
    #[cfg(feature="diagnostics")]
    cpu: (u64,u64),
    ns: [u64;9],
    calls: [u32;9],
}
impl Pass {
    pub(crate) fn start() -> Option<Self> {
        room_on().then(|| Self { start: crate::clock::Instant::now(), frame: crate::frame::current(), full: false, refused: 0, due: 0, reused: 0, bake: super::BAKING.get(), admitted:[0;3], ns: [0; 9], calls:[0;9], #[cfg(feature="diagnostics")] cpu:crate::copystat::thread_times() })
    }
}
/// Per-slot timings only when requested; no atomics or logging in the loop.
/// 0 queue setup, 1 fingerprint, 2 mapped bytes, 3 resolve/create view,
/// 4 compare entire source, 5 begin copy worker, 6 stage requested read,
/// 7 update slot/after, 8 check backing range after upload.
#[inline]
pub(crate) fn phase<T>(pass: &mut Option<Pass>, n: usize, f: impl FnOnce()->T) -> T {
    let Some(p)=pass.as_mut() else { return f(); };
    let start=crate::clock::Instant::now();
    let val=f();
    p.ns[n]+=start.elapsed().as_nanos() as u64;
    p.calls[n]+=1;
    val
}
impl Drop for Pass {
    fn drop(&mut self) {
        let us=self.start.elapsed().as_micros() as u64;
        #[cfg(feature="diagnostics")]
        let active_us={ let (u,k)=crate::copystat::thread_times(); (u+k).saturating_sub(self.cpu.0+self.cpu.1) };
        #[cfg(not(feature="diagnostics"))]
        let active_us=0u64;
        tracing::info!(frame=self.frame, us, active_us, full=self.full, refused=self.refused, due=self.due, reused=self.reused, bake=self.bake, admitted=?self.admitted, phase_us=?self.ns.map(|ns| ns/1000), calls=?self.calls,"index pass duration");
    }
}
