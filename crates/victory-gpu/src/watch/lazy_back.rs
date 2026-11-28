//! Demand readback of a synchronous submission or the completed tail of the relay.
//! Next submit saves any still-live source before the GPU can reuse its storage.
//! Only a full, singly mapped section page with an identical CPU baseline can
//! take the whole GPU page without a three-way merge. All other cases stay eager.
use super::{BLOCK, Bits, GUARDS, mapping_hold_now, open};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_MAPPED, MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, PAGE_READWRITE,
    VirtualProtect, VirtualQuery,
};

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("VICTORY_LAZY_BACK").is_some_and(|v| v == "1"))
}

thread_local! {
    static LANDING: std::cell::RefCell<Option<std::collections::HashMap<usize, Option<u64>>>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) struct Landing;
pub(crate) fn completed_landing(
    ranges: impl Iterator<Item = (u64, usize, usize)>,
) -> Option<Landing> {
    if !enabled() {
        return None;
    }
    let mut owners = std::collections::HashMap::new();
    for (shadow, start, end) in ranges.filter(|&(_, start, end)| start < end) {
        for block in start / BLOCK..end.div_ceil(BLOCK) {
            let page = block * BLOCK;
            let owner = owners.entry(page).or_insert(Some(shadow));
            if *owner != Some(shadow) || start > page || end - page < BLOCK {
                *owner = None;
            }
        }
    }
    LANDING.replace(Some(owners));
    Some(Landing)
}
impl Drop for Landing {
    fn drop(&mut self) {
        LANDING.replace(None);
    }
}
pub(crate) fn eligible(id: u64, at: usize) -> bool {
    LANDING.with_borrow(|pages| pages.as_ref().and_then(|p| p.get(&at)) == Some(&Some(id)))
}
pub(crate) fn can_defer() -> bool {
    LANDING.with_borrow(Option::is_some)
}

// Fetches touch only host-owned backing and the permanently mapped side view.
// Never take a submission, mapping or watch lock inside a fetch.
type Fetch = Box<dyn Fn(*mut u8) -> Result<(), ()> + Send>;
enum Source {
    Live(Fetch),
    // Next submit can reuse the GPU source. Keep its completed version here
    // until a consumer actually touches title memory.
    Saved(Vec<u8>),
}
struct Entry {
    own: usize,
    source: Source,
    copied: bool,
}

#[derive(Default)]
struct Pages {
    entries: BTreeMap<usize, Entry>,
    physical_writers: usize,
    // The title can receive overlapped completion without passing through HLE.
    // Keep these destinations eager after the kernel has accepted a pending read.
    pending_io: Vec<(usize, usize)>,
    pending_sections: std::collections::HashSet<usize>,
}

static PAGES: LazyLock<Mutex<Pages>> = LazyLock::new(Default::default);
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn restore(at: usize, protection: u32) {
    let mut old = 0;
    // SAFETY: mapping changes drain the table before touching any title view.
    if unsafe { VirtualProtect(at as _, BLOCK, protection, &mut old) } == 0 {
        tracing::error!(at, "cannot restore a deferred readback page");
        std::process::abort();
    }
}

static TOUCHED: Bits = Bits::new();

struct Protection(usize, u32);

impl Drop for Protection {
    fn drop(&mut self) {
        restore(self.0, self.1);
    }
}

impl Pages {
    fn finish(&mut self, at: usize) -> Result<(), ()> {
        let Some(entry) = self.entries.get_mut(&at) else {
            return Ok(());
        };
        if !entry.copied {
            match &entry.source {
                Source::Live(fetch) => fetch(entry.own as *mut u8)?,
                Source::Saved(bytes) => {
                    // SAFETY: our stable alias spans the guarded title page.
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), entry.own as *mut u8, BLOCK)
                    };
                }
            }
            entry.copied = true;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(at as _, BLOCK, PAGE_READWRITE, &mut old) } == 0 {
            tracing::error!(at, "cannot reopen a materialized GPU page");
            return Err(());
        }
        self.entries.remove(&at);
        ACTIVE.store(!self.entries.is_empty(), Ordering::Release);
        Ok(())
    }

    fn drain(&mut self) -> Result<(), ()> {
        while let Some((&at, _)) = self.entries.first_key_value() {
            self.finish(at)?;
        }
        Ok(())
    }
}

fn required<T>(result: Result<T, ()>) -> T {
    result.unwrap_or_else(|()| {
        crate::host::fail(format_args!("cannot materialize a deferred GPU page"))
    })
}

pub(crate) fn drain() {
    if ACTIVE.load(Ordering::Acquire) {
        required(PAGES.lock().unwrap_or_else(|e| e.into_inner()).drain());
    }
}

pub(crate) fn retire_sources() {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let mut pages = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    for entry in pages.entries.values_mut() {
        if !entry.copied
            && let Source::Live(fetch) = &entry.source
        {
            let mut bytes = vec![0; BLOCK];
            required(fetch(bytes.as_mut_ptr()));
            entry.source = Source::Saved(bytes);
        }
    }
}

pub(crate) fn try_materialize(at: usize, len: usize) -> Result<bool, ()> {
    if len == 0 || !ACTIVE.load(Ordering::Acquire) {
        return Ok(false);
    }
    let first = at / BLOCK * BLOCK;
    let end = at.saturating_add(len);
    // Do not take the pager lock for unrelated faults on this thread.
    if !TOUCHED.any(
        first / BLOCK,
        end.div_ceil(BLOCK).min(Bits::PIECES * Bits::WORDS * 64),
    ) {
        return Ok(false);
    }
    let mut pages = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let mut found = false;
    while let Some((&page, _)) = pages.entries.range(first..end).next() {
        pages.finish(page)?;
        found = true;
    }
    Ok(found)
}

pub(crate) fn materialize(at: usize, len: usize) -> bool {
    required(try_materialize(at, len))
}

pub(crate) fn fault(at: usize, write: bool) -> Result<bool, ()> {
    let block = at / BLOCK;
    if !TOUCHED.any(block, block + 1) {
        return Ok(false);
    }
    // Another fault can restore the page before this handler starts.
    Ok(try_materialize(at, 1)?
        || if write {
            super::writable(at)
        } else {
            super::readable_view(at)
        })
}

pub struct PhysicalWrite(());

/// `Err`: a page could not be materialized; the caller fails its I/O.
#[allow(clippy::result_unit_err)]
pub fn physical_write(
    ranges: impl FnOnce() -> Vec<(usize, usize)>,
) -> Result<Option<PhysicalWrite>, ()> {
    if !enabled() {
        return Ok(None);
    }
    let mut pages = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    pages.physical_writers += 1;
    drop(pages);
    let guard = PhysicalWrite(());
    if ACTIVE.load(Ordering::Acquire) {
        for (at, len) in ranges() {
            try_materialize(at, len)?;
        }
    }
    Ok(Some(guard))
}

impl Drop for PhysicalWrite {
    fn drop(&mut self) {
        PAGES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .physical_writers -= 1;
    }
}

pub fn pending_io(at: usize, len: usize) {
    if enabled() && len != 0 {
        let sections = crate::inplace::sections_in(at, len);
        let mut pages = PAGES.lock().unwrap_or_else(|e| e.into_inner());
        pages.pending_sections.extend(sections);
        let (mut lo, mut hi) = (at, at.saturating_add(len));
        pages.pending_io.retain(|&(a, b)| {
            if a > hi || b < lo {
                return true;
            }
            lo = lo.min(a);
            hi = hi.max(b);
            false
        });
        pages.pending_io.push((lo, hi));
    }
}

pub(crate) fn arm(
    at: usize,
    baseline: &[u8],
    fetch: impl Fn(*mut u8) -> Result<(), ()> + Send + 'static,
) -> bool {
    if !at.is_multiple_of(BLOCK) || baseline.len() != BLOCK || !super::ensure_handler() {
        return false;
    }
    // An outstanding read through a private alias may still be using it.
    let Some(_mapping) = mapping_hold_now(at, BLOCK) else {
        return false;
    };
    let crate::inplace::Cover::Run(section, _) = crate::inplace::cover(at, BLOCK) else {
        return false;
    };
    if crate::inplace::pages_of(at) != [at] {
        return false;
    }
    let Some(own) = crate::inplace::beside(at, BLOCK) else {
        return false;
    };
    let Ok(guards) = GUARDS.try_write() else {
        return false;
    };
    if guards.left_alone(at / BLOCK) {
        return false;
    }
    open(&guards, at, at + BLOCK);
    let mut pages = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    const MAX_PENDING_BYTES: usize = 64 << 20;
    if pages.entries.len() >= MAX_PENDING_BYTES / BLOCK
        || pages.pending_sections.contains(&section)
        || pages.physical_writers != 0
        || pages
            .pending_io
            .iter()
            .any(|&(a, b)| a < at + BLOCK && at < b)
    {
        return false;
    }
    // Overlapping shadows are landed in submission order.
    required(pages.finish(at));
    // SAFETY: query the entire page before changing its protection. A mixed
    // page cannot be restored using VirtualProtect's single old protection.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { VirtualQuery(at as _, &mut info, size_of_val(&info)) } == 0
        || info.State != MEM_COMMIT
        || info.Type != MEM_MAPPED
        || info.Protect != PAGE_READWRITE
        || (info.BaseAddress as usize).saturating_add(info.RegionSize) < at + BLOCK
    {
        return false;
    }
    let mut old = 0;
    TOUCHED.set(at / BLOCK);
    ACTIVE.store(true, Ordering::Release);
    // SAFETY: the entire view is stable under MAPPING and the watch lock.
    if unsafe { VirtualProtect(at as _, BLOCK, PAGE_NOACCESS, &mut old) } == 0 {
        ACTIVE.store(!pages.entries.is_empty(), Ordering::Release);
        return false;
    }
    let protection = Protection(at, old);
    // SAFETY: all title access is stopped, the permanent alias is committed.
    if old != PAGE_READWRITE
        || unsafe { std::slice::from_raw_parts(own as *const u8, BLOCK) } != baseline
    {
        drop(protection);
        ACTIVE.store(!pages.entries.is_empty(), Ordering::Release);
        return false;
    }
    pages.entries.insert(
        at,
        Entry {
            own,
            source: Source::Live(Box::new(fetch)),
            copied: false,
        },
    );
    std::mem::forget(protection);
    true
}
