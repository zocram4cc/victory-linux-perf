//! Under Wine: the title's physical pages shown by the host kernel's own
//! `mmap`, not as views (`VICTORY_WINE_MAP=1`; unset: views in placeholders,
//! as on Windows). Read by `xmem`.
//!
//! The title shows its pages one 64 KiB view at a time, 3,000 to 5,500
//! mapping calls a second in the world, with 50,000 and more views there.
//! Wine's server keeps a process's views in a list and walks all of it for
//! every view mapped (`is_valid_view_addr`) and most of it for every view
//! unmapped (`find_mapped_view`), while the calling thread holds the one lock
//! of the address space that every fault, protection change, query and
//! write-watch asking of the process takes too (ntdll's `virtual_mutex`). A
//! map and unmap of one view took 7 microseconds with no other view there,
//! 253 with 60,000 and 498 with 120,000 (`tests/fixtures/wine-bench`); in the
//! title's first scene the calls took 800 to 1,000 ms of every second, the
//! server ran at 80 % of a processor and the whole process at under two
//! (runs 9 and 10, `docs/linux-wine.md`).
//!
//! Here the server is not asked. Where the host would have a placeholder the
//! reservation is a plain one ([`Spaces`]); a page is shown by mapping the
//! section's file there with `mmap` (the descriptor is the server's own, from
//! `wine_server_handle_to_fd`: the same pages as any view of the section)
//! and then committing the range, which under Wine changes its books and the
//! protection and nothing else; it is taken away by decommitting, which maps
//! fresh inaccessible memory there. Protection changes, queries and faults
//! go through Wine as before; a query says such a page is private memory,
//! not a view, so the GPU side is told where these reservations are
//! (`victory_gpu::title_view_space`).
//!
//! `mmap` is the C library's, found in this process: Wine lets system calls
//! through only from there (syscall user dispatch; one made from our own code
//! would be taken for a Windows one).
//!
//! Ported from avatarmanz/VIctory-7900XTX#440 (94530f03), with one change:
//! upstream turns this on under Wine and `VICTORY_WINE_MAP=0` turns it off;
//! here it is off unless `VICTORY_WINE_MAP=1`. Unset, [`on`] is false, no
//! reservation is ever made in [`Spaces`], and every `holds` and `in_view_space`
//! answer is false: `xmem` takes the paths it took before.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{LazyLock, Mutex};

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_WRITE, MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE,
    MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_NOACCESS,
    PAGE_READONLY, PAGE_READWRITE, SEC_COMMIT, UnmapViewOfFile, VirtualAlloc, VirtualFree,
    VirtualProtect, VirtualQuery,
};

type Mmap = unsafe extern "sysv64" fn(*mut c_void, usize, i32, i32, i32, i64) -> *mut c_void;
type Close = unsafe extern "sysv64" fn(i32) -> i32;
type Madvise = unsafe extern "sysv64" fn(*mut c_void, usize, i32) -> i32;
type HandleToFd = unsafe extern "C" fn(HANDLE, u32, *mut i32, *mut u32) -> i32;
type MemfdCreate = unsafe extern "sysv64" fn(*const u8, u32) -> i32;
type Ftruncate = unsafe extern "sysv64" fn(i32, i64) -> i32;
// (`fcntl`, `open` and `ioctl` take more arguments in C, after these fixed
// ones, in the registers the convention gives fixed ones: only these are passed.)
type Fcntl = unsafe extern "sysv64" fn(i32, i32, i32) -> i32;
type Open = unsafe extern "sysv64" fn(*const u8, i32) -> i32;
type Ioctl = unsafe extern "sysv64" fn(i32, u64, *mut UdmabufCreate) -> i32;
type FdToHandle = unsafe extern "C" fn(i32, u32, u32, *mut HANDLE) -> i32;

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;
const MAP_PRIVATE: i32 = 2;
const MAP_FIXED: i32 = 0x10;
const MAP_ANONYMOUS: i32 = 0x20;
const MAP_NORESERVE: i32 = 0x4000;
const MADV_NOHUGEPAGE: i32 = 15;
const MFD_CLOEXEC: u32 = 1;
const MFD_ALLOW_SEALING: u32 = 2;
const F_ADD_SEALS: i32 = 1033;
const F_SEAL_SHRINK: i32 = 2;
const O_RDWR: i32 = 2;
const O_CLOEXEC: i32 = 0o200_0000;
/// `_IOW('u', 0x42, struct udmabuf_create)`.
const UDMABUF_CREATE: u64 = 0x4018_7542;
const UDMABUF_FLAGS_CLOEXEC: u32 = 1;

struct Direct {
    mmap: Mmap,
    close: Close,
    /// The C library's `madvise`, if found ([`Direct::no_huge`] only).
    madvise: Option<Madvise>,
    handle_to_fd: HandleToFd,
    /// With `VICTORY_HOME_BO=1` ([`home_bo`]), if all of it is found.
    bo: Option<Bo>,
}

/// `struct udmabuf_create` (`linux/udmabuf.h`).
#[repr(C)]
struct UdmabufCreate {
    memfd: u32,
    flags: u32,
    offset: u64,
    size: u64,
}

/// What makes memory at home a memfd the card opens as a heap by handle
/// ([`home_bo`]): the C library's calls and Wine's way from a descriptor to a
/// handle.
struct Bo {
    memfd_create: MemfdCreate,
    ftruncate: Ftruncate,
    fcntl: Fcntl,
    open: Open,
    ioctl: Ioctl,
    fd_to_handle: FdToHandle,
}

/// What is needed, if all of it is there and a page shown this way reads and
/// writes the section's bytes ([`probe`]).
static DIRECT: LazyLock<Option<Direct>> = LazyLock::new(|| {
    if !super::xmem::on_wine() {
        return None;
    }
    // Read here: only `VICTORY_WINE_MAP=1` shows pages this way, beside Wine's
    // server; anything else shows them as views through it.
    if !std::env::var_os("VICTORY_WINE_MAP").is_some_and(|v| v == "1") {
        return None;
    }
    match find().and_then(|direct| victory_core::mapping::changing(|| probe(&direct)).map(|()| direct)) {
        Ok(direct) => {
            tracing::info!(
                "title pages are shown with the host's mmap, beside Wine's server (VICTORY_WINE_MAP=1)"
            );
            Some(direct)
        }
        Err(why) => {
            tracing::warn!(
                why,
                "title pages are shown as views in placeholders: the host's mmap cannot be used (VICTORY_WINE_MAP=1)"
            );
            None
        }
    }
});

/// Whether pages are shown this way.
pub(crate) fn on() -> bool {
    DIRECT.is_some()
}

/// A file of the host's, by its path there.
fn host_file(path: &str) -> std::io::Result<Vec<u8>> {
    // (Wine's name for a host path whatever the prefix's drives; then the usual drive.)
    std::fs::read(format!(r"\\?\unix{}", path.replace('/', "\\")))
        .or_else(|_| std::fs::read(format!("Z:{}", path.replace('/', "\\"))))
}

/// A folder of the host's, by its path there: its entries' names.
fn host_folder(path: &str) -> Vec<String> {
    let names = |folder: String| -> Option<Vec<String>> {
        Some(
            std::fs::read_dir(folder)
                .ok()?
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect(),
        )
    };
    names(format!(r"\\?\unix{}", path.replace('/', "\\")))
        .or_else(|| names(format!("Z:{}", path.replace('/', "\\"))))
        .unwrap_or_default()
}

/// A file of this process's in the host's `/proc/<pid>` (`maps`, `smaps`).
///
/// Wine's server opens the files, so `/proc/self` is the server; and nothing
/// tells a Windows program its host process. A file of our own is mapped,
/// and the process that has it in its map is this one.
fn own_proc(what: &str) -> Result<String, &'static str> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Memory::FILE_MAP_READ;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.subsec_nanos());
    let name = format!("victory-wine-map-{}-{nanos:08x}.tmp", std::process::id());
    let path = std::env::temp_dir().join(&name);
    std::fs::write(&path, [0u8; 4096]).map_err(|_| "the marker file was not written")?;
    let found = (|| {
        let file = std::fs::File::open(&path).map_err(|_| "the marker file was not opened")?;
        // SAFETY: a read-only mapping of a file of ours, and a view of it, both let go below.
        let (section, view) = unsafe {
            let section = CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            );
            (
                section,
                if section.is_null() {
                    std::ptr::null_mut()
                } else {
                    MapViewOfFile(section, FILE_MAP_READ, 0, 0, 0).Value
                },
            )
        };
        let own = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let processes: Vec<String> = host_folder("/proc")
            .into_iter()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            .collect();
        let maps_of = |pid: &String| {
            host_file(&format!("/proc/{pid}/maps"))
                .ok()
                .map(|m| String::from_utf8_lossy(&m).into_owned())
        };
        let ours = |pid: &String| {
            maps_of(pid)
                .filter(|maps| maps.contains(&name))
                .map(|maps| (pid.clone(), maps))
        };
        // Those that run our program first; then any.
        let named = |pid: &&String| {
            host_file(&format!("/proc/{pid}/cmdline"))
                .is_ok_and(|c| String::from_utf8_lossy(&c).contains(&own))
        };
        let found = if view.is_null() {
            Err("the marker file was not mapped")
        } else {
            processes
                .iter()
                .filter(named)
                .find_map(ours)
                .or_else(|| processes.iter().find_map(ours))
                .ok_or("this process was not found in the host's /proc")
        };
        // SAFETY: ours, from above.
        unsafe {
            if !view.is_null() {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view });
            }
            if !section.is_null() {
                CloseHandle(section);
            }
        }
        found
    })();
    let _ = std::fs::remove_file(&path);
    let (pid, maps) = found?;
    if what == "maps" {
        return Ok(maps);
    }
    host_file(&format!("/proc/{pid}/{what}"))
        .map(|f| String::from_utf8_lossy(&f).into_owned())
        .map_err(|_| "this process's file in the host's /proc was not read")
}

/// This process's memory map as the host has it (`/proc/<pid>/maps`).
fn own_maps() -> Result<String, &'static str> {
    victory_core::mapping::changing(|| own_proc("maps"))
}

/// One line of `/proc/<pid>/maps`: start, end, executable, file offset, path.
fn mapping(line: &str) -> Option<(usize, usize, bool, usize, &str)> {
    let mut parts = line.splitn(6, ' ');
    let (range, perms, offset) = (parts.next()?, parts.next()?, parts.next()?);
    let path = parts.nth(2)?.trim();
    let (start, end) = range.split_once('-')?;
    Some((
        usize::from_str_radix(start, 16).ok()?,
        usize::from_str_radix(end, 16).ok()?,
        perms.contains('x'),
        usize::from_str_radix(offset, 16).ok()?,
        path,
    ))
}

/// For tests: whether one mapping of the process's own, with no file behind
/// it, holds all of `[at, at + len)`, by the host's list of mappings (`None`:
/// the list could not be read).
#[cfg(test)]
pub(crate) fn one_own_mapping(at: usize, len: usize) -> Option<bool> {
    let maps = own_maps().ok()?;
    Some(
        maps.lines()
            .filter_map(mapping)
            .any(|(start, end, _, _, path)| start <= at && at + len <= end && path.is_empty()),
    )
}

/// For tests: whether the host maps `at` from a memfd [`own`] made
/// (`VICTORY_HOME_BO=1`), by its list of this process's mappings (`None`: the
/// list could not be read).
#[cfg(test)]
pub(crate) fn memfd_at(at: usize) -> Option<bool> {
    let maps = own_maps().ok()?;
    Some(
        maps.lines()
            .filter_map(mapping)
            .any(|(start, end, _, _, path)| start <= at && at < end && path.starts_with("/memfd:victory-home")),
    )
}

/// For tests: whether the host's device opens a heap by a handle to a dma-buf
/// of a memfd, [`own`]'s question with `VICTORY_HOME_BO=1`, asked on one of
/// the test's own (`None`: pages are not shown by mmap, or the switch is off).
#[cfg(test)]
pub(crate) fn opens_by_handle() -> Option<bool> {
    let direct = DIRECT.as_ref()?;
    Some(direct.bo.as_ref()?.made(direct, 64 << 10).map(|(memfd, handle)| direct.let_go(memfd, handle)).is_ok())
}

/// The values of the functions `names` in the dynamic symbols of `elf` (a
/// 64-bit little-endian shared object), in order.
fn symbols(elf: &[u8], names: &[&str]) -> Option<Vec<usize>> {
    let u16_at =
        |at: usize| Some(u16::from_le_bytes(elf.get(at..at + 2)?.try_into().ok()?) as usize);
    let u32_at =
        |at: usize| Some(u32::from_le_bytes(elf.get(at..at + 4)?.try_into().ok()?) as usize);
    let u64_at =
        |at: usize| Some(u64::from_le_bytes(elf.get(at..at + 8)?.try_into().ok()?) as usize);
    if elf.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let (sections, each, count) = (u64_at(0x28)?, u16_at(0x3a)?, u16_at(0x3c)?);
    const DYNSYM: usize = 11;
    let table = (0..count)
        .map(|i| sections + i * each)
        .find(|&s| u32_at(s + 4) == Some(DYNSYM))?;
    let (at, bytes, link, entry) = (
        u64_at(table + 0x18)?,
        u64_at(table + 0x20)?,
        u32_at(table + 0x28)?,
        u64_at(table + 0x38)?,
    );
    let strings = u64_at(sections + link * each + 0x18)?;
    let mut found = vec![0usize; names.len()];
    for symbol in (at..at + bytes).step_by(entry.max(24)) {
        let (name, info, section, value) = (
            u32_at(symbol)?,
            *elf.get(symbol + 4)?,
            u16_at(symbol + 6)?,
            u64_at(symbol + 8)?,
        );
        // A function defined here, global or weak.
        if section == 0 || info & 0xf != 2 || !matches!(info >> 4, 1 | 2) {
            continue;
        }
        let text = elf.get(strings + name..)?;
        let text = &text[..text.iter().position(|&b| b == 0)?];
        if let Some(i) = names.iter().position(|n| n.as_bytes() == text)
            && found[i] == 0
        {
            found[i] = value;
        }
    }
    found.iter().all(|&v| v != 0).then_some(found)
}

/// The C library's `mmap`, `close` and `madvise` in this process, and Wine's
/// way to a handle's file descriptor.
fn find() -> Result<Direct, &'static str> {
    // SAFETY: plain lookups in ntdll, always loaded.
    let handle_to_fd = unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        GetProcAddress(ntdll, c"wine_server_handle_to_fd".as_ptr().cast())
    }
    .ok_or("ntdll has no wine_server_handle_to_fd")?;
    let maps = own_maps()?;
    let of_libc = |path: &str| {
        path.rsplit('/')
            .next()
            .is_some_and(|name| name == "libc.so.6")
    };
    let all: Vec<_> = maps
        .lines()
        .filter_map(mapping)
        .filter(|m| of_libc(m.4))
        .collect();
    let &(base, _, _, _, path) = all
        .iter()
        .find(|m| m.3 == 0)
        .ok_or("no libc.so.6 is mapped in this process")?;
    if all.iter().any(|m| m.4 != path) {
        return Err("more than one libc.so.6 is mapped in this process");
    }
    let file = host_file(path).map_err(|_| "the C library's file cannot be read")?;
    let values =
        symbols(&file, &["mmap", "close"]).ok_or("the C library's symbols were not found")?;
    let at = |value: usize| {
        let at = base + value;
        all.iter()
            .any(|m| m.2 && (m.0..m.1).contains(&at))
            .then_some(at)
    };
    let (mmap, close) = at(values[0])
        .zip(at(values[1]))
        .ok_or("the C library's functions are not where its file says")?;
    // (Looked up alone: without it pages are shown all the same.)
    let madvise = symbols(&file, &["madvise"]).and_then(|values| at(values[0]));
    // (Looked up only with `VICTORY_HOME_BO=1`.)
    let bo = home_bo()
        .then(|| {
            // SAFETY: a plain lookup in ntdll, always loaded.
            let fd_to_handle = unsafe {
                let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
                GetProcAddress(ntdll, c"wine_server_fd_to_handle".as_ptr().cast())
            }?;
            let values = symbols(&file, &["memfd_create", "ftruncate", "fcntl", "open", "ioctl"])?;
            let calls = values.into_iter().map(&at).collect::<Option<Vec<usize>>>()?;
            // SAFETY: as below: the C library's functions in its executable mappings, called
            // as the host's calling convention has them, and ntdll's export, of this signature.
            Some(unsafe {
                Bo {
                    memfd_create: std::mem::transmute::<usize, MemfdCreate>(calls[0]),
                    ftruncate: std::mem::transmute::<usize, Ftruncate>(calls[1]),
                    fcntl: std::mem::transmute::<usize, Fcntl>(calls[2]),
                    open: std::mem::transmute::<usize, Open>(calls[3]),
                    ioctl: std::mem::transmute::<usize, Ioctl>(calls[4]),
                    fd_to_handle: std::mem::transmute::<unsafe extern "system" fn() -> isize, FdToHandle>(fd_to_handle),
                }
            })
        })
        .flatten();
    // SAFETY: the addresses of the C library's `mmap`, `close` and `madvise`, in its executable
    // mappings, called as the host's calling convention has them; and ntdll's export, of this
    // signature.
    unsafe {
        Ok(Direct {
            mmap: std::mem::transmute::<usize, Mmap>(mmap),
            close: std::mem::transmute::<usize, Close>(close),
            madvise: madvise.map(|at| std::mem::transmute::<usize, Madvise>(at)),
            handle_to_fd: std::mem::transmute::<unsafe extern "system" fn() -> isize, HandleToFd>(
                handle_to_fd,
            ),
            bo,
        })
    }
}

/// A page of a section of our own shown this way, at the start: read and
/// written through it and through a view, made read-only, taken away.
fn probe(direct: &Direct) -> Result<(), &'static str> {
    const K64: usize = 64 << 10;
    // SAFETY: a section, a view and a reservation of our own, used as the title's are.
    unsafe {
        let section = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE | SEC_COMMIT,
            0,
            (2 * K64) as u32,
            std::ptr::null(),
        );
        if section.is_null() {
            return Err("the probe's section was not made");
        }
        let view = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, 2 * K64).Value as *mut u8;
        let space = VirtualAlloc(std::ptr::null(), 2 * K64, MEM_RESERVE, PAGE_NOACCESS) as *mut u8;
        let fd = direct.fd_of(section);
        let result = (|| {
            if view.is_null() || space.is_null() {
                return Err("the probe's view or reservation was not made");
            }
            let fd = fd.ok_or("the section's file descriptor was not given")?;
            view.add(K64 + 5).write(0x5a);
            let at = space.add(K64);
            if !direct.show(at as usize, K64, fd, K64 as u64) {
                return Err("the probe's page was not shown");
            }
            if at.add(5).read() != 0x5a {
                return Err("the probe's page does not show the section's bytes");
            }
            at.add(6).write(0xa5);
            if view.add(K64 + 6).read() != 0xa5 {
                return Err("a write through the probe's page did not reach the section");
            }
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let mut old = 0;
            if VirtualQuery(at.cast(), &mut info, size_of_val(&info)) == 0
                || info.State != MEM_COMMIT
                || info.Protect != PAGE_READWRITE
                || info.RegionSize != K64
                || VirtualProtect(at.cast(), K64, PAGE_READONLY, &mut old) == 0
                || old != PAGE_READWRITE
                || VirtualProtect(at.cast(), K64, PAGE_READWRITE, &mut old) == 0
            {
                return Err(
                    "the probe's page is not committed read-write memory to the host, or its protection cannot be changed",
                );
            }
            if !hide(at as usize, K64)
                || VirtualQuery(at.cast(), &mut info, size_of_val(&info)) == 0
                || info.State != MEM_RESERVE
            {
                return Err("the probe's page was not taken away");
            }
            Ok(())
        })();
        if let Some(fd) = fd {
            (direct.close)(fd);
        }
        if !space.is_null() {
            VirtualFree(space.cast(), 0, MEM_RELEASE);
        }
        if !view.is_null() {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view.cast() });
        }
        CloseHandle(section);
        result
    }
}

impl Direct {
    fn fd_of(&self, section: HANDLE) -> Option<i32> {
        let (mut fd, mut options) = (-1, 0);
        // SAFETY: Wine's own call; the descriptor it gives is ours to close.
        let status = unsafe { (self.handle_to_fd)(section, 0, &mut fd, &mut options) };
        (status >= 0 && fd >= 0).then_some(fd)
    }

    fn show(&self, base: usize, len: usize, fd: i32, offset: u64) -> bool {
        // SAFETY: over a range of a reservation of the title's that shows nothing (the caller
        // unmapped it): the file's pages first, then the host's books; nothing but the address
        // the title asked for is touched.
        unsafe {
            let at = (self.mmap)(
                base as *mut c_void,
                len,
                PROT_READ | PROT_WRITE,
                MAP_SHARED | MAP_FIXED,
                fd,
                offset as i64,
            );
            if at as usize != base {
                return false;
            }
            if VirtualAlloc(base as *const c_void, len, MEM_COMMIT, PAGE_READWRITE).is_null() {
                VirtualFree(base as *mut c_void, len, MEM_DECOMMIT);
                return false;
            }
        }
        true
    }

    /// Keeps `[base, base + len)` off huge pages (`madvise`,
    /// `MADV_NOHUGEPAGE`): the kernel neither faults it in as one nor
    /// gathers its pages into one later, which moves them.
    fn no_huge(&self, base: usize, len: usize) -> bool {
        // SAFETY: the C library's `madvise` on a range of the caller's that is mapped: advice,
        // no byte changes.
        self.madvise.is_some_and(|madvise| {
            unsafe { madvise(base as *mut c_void, len, MADV_NOHUGEPAGE) == 0 }
        })
    }

    /// Fresh memory of the process's own over `[base, base + len)`, in one
    /// mapping of the host's, then the host's books ([`own`]).
    fn fresh(&self, base: usize, len: usize) -> bool {
        // SAFETY: over a range of a reservation of the title's that shows nothing
        // (the caller looked): fresh memory first, then the host's books; nothing
        // but that range is touched.
        unsafe {
            let at = (self.mmap)(
                base as *mut c_void,
                len,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED | MAP_NORESERVE,
                -1,
                0,
            );
            if at as usize != base {
                return false;
            }
            if VirtualAlloc(base as *const c_void, len, MEM_COMMIT, PAGE_READWRITE).is_null() {
                VirtualFree(base as *mut c_void, len, MEM_DECOMMIT);
                return false;
            }
        }
        true
    }

    /// [`own`] with `VICTORY_HOME_BO=1` made `[base, base + len)` anew
    /// (`shown`), from the memfd and handle in `shared` if it is theirs: what
    /// was made there before goes ([`SHARED`]), and `victory_gpu` is told the
    /// handle the heap there is opened by, if any.
    fn made_home(&self, base: usize, len: usize, shared: Option<(i32, usize)>, shown: bool) {
        let mut made = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        made.retain(|s| {
            let apart = s.0 + s.1 <= base || base + len <= s.0;
            if !apart {
                self.let_go(s.2, s.3);
            }
            apart
        });
        match shared {
            Some((memfd, handle)) if shown => {
                made.push((base, len, memfd, handle));
                tracing::info!(
                    at = format_args!("{base:#x}"),
                    len,
                    "memory at home is a memfd the card opens as a heap by handle (VICTORY_HOME_BO=1)"
                );
            }
            Some((memfd, handle)) => self.let_go(memfd, handle),
            None => {}
        }
        victory_gpu::title_memory_home_handle(base, len, shared.filter(|_| shown).map(|s| s.1));
    }

    /// Lets go of a memfd and a handle [`Bo::memfd`] gave (a mapping made with
    /// the memfd keeps it on its own, and a heap opened by the handle its memory).
    fn let_go(&self, memfd: i32, handle: usize) {
        // SAFETY: a descriptor and a handle of ours that no call uses any more.
        unsafe {
            (self.close)(memfd);
            CloseHandle(handle as HANDLE);
        }
    }
}

/// Makes `[base, base + len)`, a stretch of a reservation of [`Spaces`] that
/// shows nothing, committed memory of the process's own in one mapping of the
/// host's: one `mmap` of fresh memory over all of it, then the host's books.
/// (Committed piece by piece, with pages written in between, the host kernel
/// keeps a mapping for each piece: memory of the process's own all the same,
/// but not what its graphics driver takes by address, `xmem`'s `show_home`.)
///
/// With `VICTORY_HOME_BO=1` ([`home_bo`]) the memory is a sealed memfd shown
/// there ([`Direct::show`]) if the host's device opens a heap by a handle to a
/// dma-buf of its pages (asked first), and `victory_gpu` opens the heap by that
/// handle; else, or if a step fails, fresh memory as without the switch.
pub(crate) fn own(base: usize, len: usize) -> bool {
    let Some(direct) = DIRECT.as_ref() else {
        return false;
    };
    let shared = direct.bo.as_ref().and_then(|bo| {
        bo.made(direct, len)
            .inspect_err(|why| tracing::warn!(why = %why, "memory at home is the process's own, as without VICTORY_HOME_BO=1"))
            .ok()
    });
    let shown = match shared {
        Some((memfd, _)) => victory_core::mapping::changing(|| direct.show(base, len, memfd, 0)),
        None => victory_core::mapping::changing(|| direct.fresh(base, len)),
    };
    if direct.bo.is_some() {
        direct.made_home(base, len, shared, shown);
    }
    if !shown {
        return false;
    }
    if home_no_huge() {
        let kept = direct.no_huge(base, len);
        tracing::info!(
            at = format_args!("{base:#x}"),
            len,
            kept,
            "memory of the process's own for pages at home, kept off huge pages (VICTORY_HOME_NOHUGE=1)"
        );
    }
    true
}

/// `VICTORY_HOME_NOHUGE=1` (off unless set; read here): memory [`own`] makes is
/// kept off huge pages. The card's driver holds it by address (a heap opened
/// there, read in place), and with transparent huge pages on (`always` on the
/// rig) the kernel may move its pages to gather them into a huge one; the
/// driver then waits for the card's work on them and takes them anew.
fn home_no_huge() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("VICTORY_HOME_NOHUGE").is_some_and(|v| v == "1"));
    *ON
}

/// `VICTORY_HOME_BO=1` (off unless set; read here; it takes effect where
/// renderer port 1's pages are at home, `VICTORY_PORT_HOME=1`): memory [`own`]
/// makes is a sealed memfd, and the card's heap on it is opened by a handle to
/// a dma-buf of its pages (`/dev/udmabuf`), not by its address. By address
/// (`VK_EXT_external_memory_host`) amdgpu holds a user pointer and walks its
/// pages at every submission (~90 a frame, 64-512 us each for the 185 MB
/// home); a dma-buf's pages are pinned once. Needs a vkd3d-proton that opens a
/// heap by such a handle (`OpenSharedHandle(ID3D12Heap)`) and a Wine whose
/// win32u imports a handle that is no D3DKMT object; without them the device
/// refuses the handle and the memory is the process's own, as without the
/// switch (opened by address: a user pointer takes no memfd, its pages have a
/// file behind them).
fn home_bo() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("VICTORY_HOME_BO").is_some_and(|v| v == "1"));
    *ON
}

/// The stretches [`own`] made a memfd ([`home_bo`]): start, bytes, the memfd
/// (kept, to show its pages there again, [`shared`]) and the handle the card's
/// heap there is opened by.
static SHARED: Mutex<Vec<(usize, usize, i32, usize)>> = Mutex::new(Vec::new());

/// The memfd [`own`] made the stretch at home that starts at `first`, if it did.
pub(crate) fn shared(first: usize) -> Option<i32> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|s| s.0 == first).map(|s| s.2)
}

/// The reservations [`zero_copy`] made a memfd: start, bytes, the memfd and
/// the handle the card's heap there is opened by.
static ZERO_COPY: Mutex<Vec<(usize, usize, i32, usize)>> = Mutex::new(Vec::new());

/// Zero copy (`VICTORY_ZERO_COPY`, `xmem`): `[base, base + len)`, a fresh
/// reservation of the title's that shows nothing, made a sealed memfd shown
/// there and committed, as [`own`] does at home with `VICTORY_HOME_BO=1`
/// (which this needs); `victory_gpu` places the title's buffers and windows
/// in it by a handle to a dma-buf of its pages. Not asked first whether the
/// device opens such a heap (the title asks for memory before there is a
/// device): where it does not, `victory_gpu` copies whole what lies there.
/// `false`: nothing changed, the caller commits as before.
pub(crate) fn zero_copy(base: usize, len: usize) -> bool {
    let Some(direct) = DIRECT.as_ref() else {
        return false;
    };
    let Some(bo) = direct.bo.as_ref() else {
        return false;
    };
    let (memfd, handle) = match bo.memfd(direct.close, len) {
        Ok(made) => made,
        Err(why) => {
            tracing::warn!(why = %why, at = format_args!("{base:#x}"), len, "zero copy: no memfd, title memory as before (VICTORY_ZERO_COPY)");
            return false;
        }
    };
    if !victory_core::mapping::changing(|| direct.show(base, len, memfd, 0)) {
        direct.let_go(memfd, handle);
        return false;
    }
    let n = {
        let mut made = ZERO_COPY.lock().unwrap_or_else(|e| e.into_inner());
        made.push((base, len, memfd, handle));
        made.len()
    };
    if n <= 32 || n.is_power_of_two() {
        tracing::info!(at = format_args!("{base:#x}"), len, n, "zero copy: title memory is a memfd the card reads in place (VICTORY_ZERO_COPY)");
    }
    victory_gpu::title_memory_zero_copy(base, len, Some(handle));
    true
}

/// Before the title frees `[at, at + len)` (`len` 0: the allocation at
/// `at`): a reservation [`zero_copy`] made there goes, what is in place in
/// it first. The bytes of the one released, if any.
pub(crate) fn zero_copy_freeing(at: usize, len: usize) -> Option<usize> {
    let mut made = ZERO_COPY.lock().unwrap_or_else(|e| e.into_inner());
    let end = if len == 0 { at + 1 } else { at.saturating_add(len) };
    let i = made.iter().position(|z| z.0 < end && at < z.0 + z.1)?;
    let (base, bytes, memfd, handle) = made.swap_remove(i);
    if (at, len) != (base, 0) && (at, len) != (base, bytes) {
        // shortcut: part of it freed; the whole goes from the card's side,
        // and copies of the rest would miss writes (the host watches none
        // there). Not seen in the title: this line says if it happens.
        tracing::warn!(at = format_args!("{at:#x}"), len, base = format_args!("{base:#x}"), bytes, "zero copy: part of a memfd reservation freed");
    }
    victory_gpu::title_memory_zero_copy(base, bytes, None);
    if let Some(direct) = DIRECT.as_ref() {
        direct.let_go(memfd, handle);
    }
    Some(bytes)
}

impl Bo {
    /// A sealed memfd of `len` bytes, and a handle to a dma-buf of its pages
    /// that the host's device opens a heap by: [`own`]'s memory with the
    /// switch. `Err` says why not; what was made on the way is let go.
    fn made(&self, direct: &Direct, len: usize) -> Result<(i32, usize), String> {
        let (memfd, handle) = self.memfd(direct.close, len)?;
        victory_gpu::opens_heap_by_handle(handle).inspect_err(|_| direct.let_go(memfd, handle))?;
        Ok((memfd, handle))
    }

    /// A sealed memfd of `len` bytes and a handle (Wine's) to a dma-buf of its
    /// pages (`/dev/udmabuf`, which pins them).
    fn memfd(&self, close: Close, len: usize) -> Result<(i32, usize), &'static str> {
        // SAFETY: the C library's calls and Wine's on descriptors made here, with
        // the arguments they take; each is kept or closed.
        unsafe {
            let memfd = (self.memfd_create)(c"victory-home".as_ptr().cast(), MFD_CLOEXEC | MFD_ALLOW_SEALING);
            if memfd < 0 {
                return Err("no memfd was made");
            }
            let handle = (|| {
                if (self.ftruncate)(memfd, len as i64) != 0 || (self.fcntl)(memfd, F_ADD_SEALS, F_SEAL_SHRINK) != 0 {
                    return Err("the memfd was not sized and sealed");
                }
                let device = (self.open)(c"/dev/udmabuf".as_ptr().cast(), O_RDWR | O_CLOEXEC);
                if device < 0 {
                    return Err("/dev/udmabuf was not opened");
                }
                let mut create = UdmabufCreate { memfd: memfd as u32, flags: UDMABUF_FLAGS_CLOEXEC, offset: 0, size: len as u64 };
                let dmabuf = (self.ioctl)(device, UDMABUF_CREATE, &mut create);
                close(device);
                if dmabuf < 0 {
                    return Err("/dev/udmabuf made no dma-buf of the memfd");
                }
                let mut handle: HANDLE = std::ptr::null_mut();
                // (Wine's server keeps a descriptor of its own.)
                let status = (self.fd_to_handle)(dmabuf, GENERIC_ALL, 0, &mut handle);
                close(dmabuf);
                if status < 0 || handle.is_null() {
                    return Err("Wine gave no handle to the dma-buf");
                }
                Ok(handle as usize)
            })();
            if handle.is_err() {
                close(memfd);
            }
            handle.map(|handle| (memfd, handle))
        }
    }
}

/// The file descriptor behind `section`, to show its pages with; `None` if
/// pages are shown as views ([`on`]) or the host gives none.
pub(crate) fn fd_of(section: HANDLE) -> Option<i32> {
    DIRECT.as_ref()?.fd_of(section)
}

/// Closes a descriptor [`fd_of`] gave.
pub(crate) fn close(fd: i32) {
    if let Some(direct) = DIRECT.as_ref() {
        // SAFETY: a descriptor of ours, used by no mapping call any more (those that were made
        // with it keep the file on their own).
        unsafe { (direct.close)(fd) };
    }
}

/// Shows `len` bytes of the file `fd` from `offset` on at `base`, read-write,
/// in a reservation of [`Spaces`] that shows nothing there.
pub(crate) fn show(base: usize, len: usize, fd: i32, offset: u64) -> bool {
    DIRECT
        .as_ref()
        .is_some_and(|direct| victory_core::mapping::changing(|| direct.show(base, len, fd, offset)))
}

/// Takes away what [`show`] put at `[base, base + len)`: reserved, as before.
pub(crate) fn hide(base: usize, len: usize) -> bool {
    // SAFETY: a range of a reservation of the title's; under Wine a decommit maps fresh
    // inaccessible memory over it, whatever was there.
    victory_core::mapping::changing(|| unsafe { VirtualFree(base as *mut c_void, len, MEM_DECOMMIT) != 0 })
}

/// The reservations that stand for placeholders, in the pieces the title cut
/// them into (it splits, joins and releases placeholders as the host has
/// them): start and bytes.
#[derive(Default)]
pub(crate) struct Spaces(BTreeMap<usize, usize>);

static SPACES: LazyLock<Mutex<Spaces>> = LazyLock::new(Default::default);
/// Whether there is any, for those who ask at every call.
static ANY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn spaces() -> std::sync::MutexGuard<'static, Spaces> {
    SPACES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `at` lies in a reservation that stands for a placeholder.
pub(crate) fn holds(at: usize) -> bool {
    ANY.load(std::sync::atomic::Ordering::Relaxed) && spaces().piece(at).is_some()
}

impl Spaces {
    /// A reservation made in a placeholder's stead.
    pub(crate) fn made(&mut self, at: usize, len: usize) {
        self.0.insert(at, len);
        ANY.store(true, std::sync::atomic::Ordering::Relaxed);
        victory_gpu::title_view_space(at, len, true);
    }

    /// The piece that holds `at`: its start and bytes.
    pub(crate) fn piece(&self, at: usize) -> Option<(usize, usize)> {
        self.0
            .range(..=at)
            .next_back()
            .map(|(&start, &len)| (start, len))
            .filter(|&(start, len)| at < start + len)
    }

    /// `[at, at + len)` becomes a piece of its own, if it lies in one.
    pub(crate) fn split(&mut self, at: usize, len: usize) -> bool {
        let Some((start, bytes)) = self
            .piece(at)
            .filter(|&(start, bytes)| len != 0 && at + len <= start + bytes)
        else {
            return false;
        };
        if start < at {
            self.0.insert(start, at - start);
        }
        self.0.insert(at, len);
        if at + len < start + bytes {
            self.0.insert(at + len, start + bytes - at - len);
        }
        true
    }

    /// The pieces that make up `[at, at + len)` exactly become one.
    pub(crate) fn join(&mut self, at: usize, len: usize) -> bool {
        let inside: Vec<(usize, usize)> = self
            .0
            .range(at..at + len)
            .map(|(&start, &bytes)| (start, bytes))
            .collect();
        let whole = inside.first().is_some_and(|p| p.0 == at)
            && inside.windows(2).all(|w| w[0].0 + w[0].1 == w[1].0)
            && inside.last().is_some_and(|p| p.0 + p.1 == at + len);
        if whole {
            for (start, _) in inside {
                self.0.remove(&start);
            }
            self.0.insert(at, len);
        }
        whole
    }

    /// The piece that starts at `at` is the title's no more: its bytes.
    pub(crate) fn released(&mut self, at: usize) -> Option<usize> {
        let len = self.0.remove(&at)?;
        victory_gpu::title_view_space(at, len, false);
        Some(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapping_line_is_read() {
        let line = "7f08d7a00000-7f08d7b70000 r-xp 00024000 103:02 1234567                    /usr/lib/libc.so.6";
        assert_eq!(
            mapping(line),
            Some((
                0x7f08_d7a0_0000,
                0x7f08_d7b7_0000,
                true,
                0x24000,
                "/usr/lib/libc.so.6"
            ))
        );
        assert_eq!(
            mapping("7f08d7a00000-7f08d7b70000 rw-p 00000000 00:00 0"),
            None
        );
    }

    #[test]
    fn pieces_are_cut_joined_and_released() {
        let mut spaces = Spaces::default();
        spaces.0.insert(0x10_0000, 0x10_0000);
        assert!(spaces.split(0x14_0000, 0x2_0000));
        assert_eq!(spaces.piece(0x13_ffff), Some((0x10_0000, 0x4_0000)));
        assert_eq!(spaces.piece(0x15_0000), Some((0x14_0000, 0x2_0000)));
        assert_eq!(spaces.piece(0x1f_ffff), Some((0x16_0000, 0xa_0000)));
        assert_eq!(spaces.piece(0x20_0000), None);
        // Not across pieces, and not what is no piece.
        assert!(!spaces.split(0x13_0000, 0x2_0000));
        assert!(!spaces.join(0x10_0000, 0x5_0000));
        assert!(spaces.join(0x10_0000, 0x6_0000));
        assert_eq!(spaces.piece(0x15_0000), Some((0x10_0000, 0x6_0000)));
    }

    #[test]
    fn functions_are_found_in_dynamic_symbols() {
        // Two sections: the dynamic symbols (two of them) and their strings.
        let mut elf = vec![0u8; 0x40];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        let strings = b"\0mmap\0close\0";
        let (symbols_at, strings_at) = (0x40usize, 0x40 + 3 * 24);
        let sections_at = strings_at + strings.len();
        let mut symbol = |name: u32, info: u8, section: u16, value: u64| {
            elf.extend(name.to_le_bytes());
            elf.extend([info, 0]);
            elf.extend(section.to_le_bytes());
            elf.extend(value.to_le_bytes());
            elf.extend(0u64.to_le_bytes());
        };
        symbol(0, 0, 0, 0);
        symbol(6, 0x12, 1, 0x2000);
        symbol(1, 0x22, 1, 0x1000);
        elf.extend(strings);
        let mut section = |kind: u32, at: usize, bytes: usize, link: u32, entry: u64| {
            let mut s = [0u8; 0x40];
            s[4..8].copy_from_slice(&kind.to_le_bytes());
            s[0x18..0x20].copy_from_slice(&(at as u64).to_le_bytes());
            s[0x20..0x28].copy_from_slice(&(bytes as u64).to_le_bytes());
            s[0x28..0x2c].copy_from_slice(&link.to_le_bytes());
            s[0x38..0x40].copy_from_slice(&entry.to_le_bytes());
            elf.extend(s);
        };
        section(11, symbols_at, 3 * 24, 1, 24);
        section(3, strings_at, strings.len(), 0, 0);
        elf[0x28..0x30].copy_from_slice(&(sections_at as u64).to_le_bytes());
        elf[0x3a..0x3c].copy_from_slice(&0x40u16.to_le_bytes());
        elf[0x3c..0x3e].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            super::symbols(&elf, &["mmap", "close"]),
            Some(vec![0x1000, 0x2000])
        );
        assert_eq!(super::symbols(&elf, &["mmap", "munmap"]), None);
    }

    /// The flags the host keeps for the mapping that holds `at` (`VmFlags`
    /// of this process's `smaps`); `None`: not read.
    fn own_flags(at: usize) -> Option<String> {
        let smaps = own_proc("smaps").ok()?;
        let mut inside = false;
        for line in smaps.lines() {
            let range = line.split_once(' ').and_then(|(range, _)| range.split_once('-'));
            let bounds = range.and_then(|(from, to)| {
                Some((usize::from_str_radix(from, 16).ok()?, usize::from_str_radix(to, 16).ok()?))
            });
            if let Some((from, to)) = bounds {
                inside = from <= at && at < to;
            } else if inside && let Some(flags) = line.strip_prefix("VmFlags:") {
                return Some(flags.trim().to_owned());
            }
        }
        None
    }

    /// `VICTORY_HOME_NOHUGE=1`'s call: memory made the process's own by
    /// [`own`] is kept off huge pages (`nh` among the host's flags of its
    /// mapping), so the kernel never moves its pages to make one under a
    /// heap the card holds there. Needs pages shown by the host's mmap
    /// (`VICTORY_WINE_MAP=1`, under Wine).
    #[test]
    fn memory_of_its_own_is_kept_off_huge_pages() {
        let Some(direct) = DIRECT.as_ref() else {
            eprintln!("not tried: not under Wine with pages shown by mmap (VICTORY_WINE_MAP=1)");
            return;
        };
        const LEN: usize = 4 << 20;
        // SAFETY: a reservation of the test's own, released below.
        let base =
            unsafe { VirtualAlloc(std::ptr::null(), LEN, MEM_RESERVE, PAGE_NOACCESS) } as usize;
        assert_ne!(base, 0, "no reservation for the test");
        assert!(own(base, LEN), "made memory of the process's own");
        assert!(direct.no_huge(base, LEN), "the host took the advice");
        let flags = own_flags(base).expect("the host's flags of the mapping");
        assert!(flags.split_whitespace().any(|f| f == "nh"), "kept off huge pages: {flags}");
        // SAFETY: ours, from above.
        unsafe { VirtualFree(base as *mut c_void, 0, MEM_RELEASE) };
    }

    /// `VICTORY_HOME_BO=1`'s memory at home (a memfd whose pages a dma-buf
    /// pins) against [`own`]'s fresh memory, each over a write-watched
    /// reservation, and against memory Wine commits there itself: Wine's write
    /// watch answers alike for the memfd and [`own`]'s memory after a write, a
    /// reset and another write (and finds the second write in Wine's own,
    /// which shows that the asking works), and a write to a page made
    /// read-only faults into a handler for all three (the guards' way, for
    /// pages at home given up). Needs pages shown by the host's mmap and the
    /// switch (`VICTORY_WINE_MAP=1 VICTORY_HOME_BO=1`, under Wine).
    #[test]
    fn a_memfd_at_home_is_watched_and_guarded_as_memory_of_its_own() {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
        use windows_sys::Win32::System::Diagnostics::Debug::{
            AddVectoredExceptionHandler, EXCEPTION_POINTERS, RemoveVectoredExceptionHandler,
        };
        use windows_sys::Win32::System::Memory::GetWriteWatch;
        const MEM_WRITE_WATCH: u32 = 0x20_0000;
        const WRITE_WATCH_FLAG_RESET: u32 = 1;
        const PAGE: usize = 4 << 10;
        const LEN: usize = 64 << 10;
        static GUARDED: AtomicUsize = AtomicUsize::new(0);
        static FAULTS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "system" fn guard(info: *mut EXCEPTION_POINTERS) -> i32 {
            const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
            const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
            // SAFETY: the record of the exception being dispatched.
            let record = unsafe { &*(*info).ExceptionRecord };
            let page = GUARDED.load(Relaxed);
            if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION
                || page == 0
                || !(page..page + PAGE).contains(&record.ExceptionInformation[1])
            {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            let mut old = 0;
            // SAFETY: the test's page, read-write again.
            unsafe { VirtualProtect(page as *const c_void, PAGE, PAGE_READWRITE, &mut old) };
            FAULTS.fetch_add(1, Relaxed);
            EXCEPTION_CONTINUE_EXECUTION
        }
        let Some(direct) = DIRECT.as_ref() else {
            eprintln!("not tried: not under Wine with pages shown by mmap (VICTORY_WINE_MAP=1)");
            return;
        };
        let Some(bo) = direct.bo.as_ref() else {
            eprintln!("not tried: without VICTORY_HOME_BO=1");
            return;
        };
        let mut answers = Vec::new();
        for kind in ["Wine's own", "memory of its own", "a memfd"] {
            // SAFETY: a reservation of the test's own, released below.
            let base = unsafe { VirtualAlloc(std::ptr::null(), LEN, MEM_RESERVE | MEM_WRITE_WATCH, PAGE_NOACCESS) } as usize;
            assert_ne!(base, 0, "no reservation for the test");
            let made = (kind == "a memfd").then(|| bo.memfd(direct.close, LEN).expect("a memfd and a handle to a dma-buf of it"));
            let shown = match made {
                Some((memfd, _)) => direct.show(base, LEN, memfd, 0),
                // SAFETY: the reservation above, committed by Wine.
                None if kind == "Wine's own" => !unsafe { VirtualAlloc(base as *const c_void, LEN, MEM_COMMIT, PAGE_READWRITE) }.is_null(),
                None => direct.fresh(base, LEN),
            };
            assert!(shown, "memory over the reservation");
            let page = |k: usize| (base + k * PAGE) as *mut u8;
            let (mut found, mut granularity) = ([0usize; 16], 0u32);
            // SAFETY: committed read-write pages of the test's; room for `count` answers.
            let written: Vec<usize> = unsafe {
                page(0).write_volatile(1);
                let mut count = found.len();
                GetWriteWatch(WRITE_WATCH_FLAG_RESET, base as *const c_void, LEN, found.as_mut_ptr().cast(), &mut count, &mut granularity);
                page(2).write_volatile(1);
                let mut count = found.len();
                let status = GetWriteWatch(0, base as *const c_void, LEN, found.as_mut_ptr().cast(), &mut count, &mut granularity);
                assert_eq!(status, 0, "write-watched");
                found[..count].iter().map(|&at| (at - base) / PAGE).collect()
            };
            GUARDED.store(page(1) as usize, Relaxed);
            FAULTS.store(0, Relaxed);
            // SAFETY: the handler above, taken away below; the test's page.
            let faulted = unsafe {
                let handler = AddVectoredExceptionHandler(1, Some(guard));
                let mut old = 0;
                assert_ne!(VirtualProtect(page(1).cast(), PAGE, PAGE_READONLY, &mut old), 0);
                page(1).write_volatile(5);
                RemoveVectoredExceptionHandler(handler);
                (FAULTS.load(Relaxed), page(1).read_volatile())
            };
            GUARDED.store(0, Relaxed);
            eprintln!("{kind}: pages written after the reset {written:?}; a write to a read-only page (faults, byte) {faulted:?}");
            answers.push((written, faulted));
            if let Some((memfd, handle)) = made {
                direct.let_go(memfd, handle);
            }
            // SAFETY: ours, from above.
            unsafe { VirtualFree(base as *mut c_void, 0, MEM_RELEASE) };
        }
        assert_eq!(answers[0].0, [2], "Wine's own write watch finds the page written after the reset");
        assert_eq!(answers[1], answers[2], "a memfd is watched and guarded as memory of its own");
        assert_eq!(answers.iter().map(|a| a.1).collect::<Vec<_>>(), [(1, 5); 3], "the write faulted once, then landed");
    }
}
