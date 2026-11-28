//! File I/O preparation and CRT failure reporting for deferred GPU pages.
use super::*;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Streams whose last `fread` failed destination preparation before calling
/// the host CRT, so `ferror` reports a stream error instead of silent zero
/// until `clearerr`, `rewind`, `freopen`, or `fclose`.
static STREAM_ERRORS: LazyLock<Mutex<HashSet<usize>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[cfg(test)]
std::thread_local! {
    /// Address-scoped test hook causing `prepare_kernel_write` to return `false`
    /// for nonzero writes targeting the matching buffer on the current thread.
    static FAIL_KERNEL_WRITE_AT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

pub(super) fn prepare(at: usize, len: usize) -> Option<impl Drop> {
    #[cfg(test)]
    if len != 0 && FAIL_KERNEL_WRITE_AT.with(|cell| cell.get() == Some(at)) {
        return None;
    }
    victory_gpu::try_kernel_writing(at, len)
}

pub(super) fn metadata(read: *mut u32, overlapped: *mut c_void) -> Option<impl Drop> {
    let mut pins = Vec::new();
    if victory_gpu::lazy_readback_enabled() {
        if !read.is_null() {
            pins.push(prepare(read as usize, size_of::<u32>())?);
        }
        if !overlapped.is_null() {
            pins.push(prepare(
                overlapped as usize,
                size_of::<windows_sys::Win32::System::IO::OVERLAPPED>(),
            )?);
        }
    }
    Some(pins)
}

pub(super) fn mark_stream_error(stream: File) {
    const EACCES: i32 = 13;
    if !stream.is_null() {
        STREAM_ERRORS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(stream as usize);
    }
    let _ = host!("ucrtbase.dll", "_set_errno": extern "C" fn(i32) -> i32)(EACCES);
}

fn clear_stream_error(stream: File) {
    if !stream.is_null() {
        STREAM_ERRORS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(stream as usize));
    }
}

fn has_stream_error(stream: File) -> bool {
    !stream.is_null()
        && STREAM_ERRORS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(stream as usize))
}

pub extern "C" fn ferror(stream: File) -> i32 {
    if has_stream_error(stream) {
        return 1;
    }
    host!("ucrtbase.dll", "ferror": extern "C" fn(File) -> i32)(stream)
}

pub extern "C" fn clearerr(stream: File) {
    clear_stream_error(stream);
    host!("ucrtbase.dll", "clearerr": extern "C" fn(File))(stream);
}

pub extern "C" fn clearerr_s(stream: File) -> i32 {
    clear_stream_error(stream);
    host!("ucrtbase.dll", "clearerr_s": extern "C" fn(File) -> i32)(stream)
}

pub extern "C" fn rewind(stream: File) {
    clear_stream_error(stream);
    host!("ucrtbase.dll", "rewind": extern "C" fn(File))(stream);
}

pub extern "C" fn fclose(stream: File) -> i32 {
    clear_stream_error(stream);
    host!("ucrtbase.dll", "fclose": extern "C" fn(File) -> i32)(stream)
}

pub extern "C" fn freopen(path: *const c_char, mode: *const c_char, stream: File) -> File {
    clear_stream_error(stream);
    match narrow(path) {
        Some(p) => {
            host!("ucrtbase.dll", "_wfreopen": extern "C" fn(*const u16, *const u16, File) -> File)(
                p.as_ptr(),
                widen(mode).as_ptr(),
                stream,
            )
        }
        None => {
            host!("ucrtbase.dll", "freopen": extern "C" fn(*const c_char, *const c_char, File) -> File)(
                path, mode, stream,
            )
        }
    }
}

pub extern "C" fn _wfreopen(path: *const u16, mode: *const u16, stream: File) -> File {
    clear_stream_error(stream);
    let p = wide(path);
    host!("ucrtbase.dll", "_wfreopen": extern "C" fn(*const u16, *const u16, File) -> File)(
        p.ptr(),
        mode,
        stream,
    )
}

pub(super) fn extend(module: &mut HleModule) {
    module.exports.extend([
        hle_export!(ferror),
        hle_export!(clearerr),
        hle_export!(clearerr_s),
        hle_export!(rewind),
        hle_export!(fclose),
        hle_export!(freopen),
        hle_export!(_wfreopen),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FailPrepareGuard(Option<usize>);
    impl FailPrepareGuard {
        fn at(ptr: *const u8) -> Self {
            let prev = FAIL_KERNEL_WRITE_AT.with(|c| c.replace(Some(ptr as usize)));
            Self(prev)
        }
    }
    impl Drop for FailPrepareGuard {
        fn drop(&mut self) {
            FAIL_KERNEL_WRITE_AT.with(|c| c.set(self.0));
        }
    }

    #[test]
    fn read_file_clears_sync_byte_count_on_prepare_failure_and_handles_overlapped_null() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{ERROR_NOACCESS, GetLastError, SetLastError};
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, VirtualAlloc, VirtualFree,
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rf.bin");
        std::fs::write(&path, b"0123456789").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let handle = file.as_raw_handle().cast();

        // 1. Synchronous call: stale `n = 10` must be cleared to `0`, last error
        //    set to `ERROR_NOACCESS`, buffer untouched, and file cursor unchanged.
        let mut out = [0xAAu8; 10];
        let mut n = 10u32;
        let _fail = FailPrepareGuard::at(out.as_ptr());
        // SAFETY: clearing thread-local last error before call.
        unsafe { SetLastError(0) };
        assert_eq!(
            ReadFile(handle, out.as_mut_ptr(), 10, &mut n, std::ptr::null_mut()),
            0
        );
        assert_eq!(n, 0, "synchronous ReadFile failure must clear *read to 0");
        // SAFETY: reads thread-local Win32 last error.
        assert_eq!(unsafe { GetLastError() }, ERROR_NOACCESS);
        assert_eq!(out, [0xAA; 10]);

        // 2. Synchronous call with an unwritable `read` pointer must not fault.
        // SAFETY: allocating a read-only page to exercise checked output write.
        let ro_page = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READONLY,
            )
        };
        assert!(!ro_page.is_null());
        // SAFETY: clearing thread-local last error before call.
        unsafe { SetLastError(0) };
        assert_eq!(
            ReadFile(
                handle,
                out.as_mut_ptr(),
                10,
                ro_page.cast::<u32>(),
                std::ptr::null_mut(),
            ),
            0
        );
        // SAFETY: reads thread-local Win32 last error and releases test page.
        unsafe {
            assert_eq!(GetLastError(), ERROR_NOACCESS);
            VirtualFree(ro_page, 0, MEM_RELEASE);
        }

        // 3. Overlapped call with null `read` pointer must not dereference null.
        let mut overlapped = [0u8; 32];
        // SAFETY: clearing thread-local last error before call.
        unsafe { SetLastError(0) };
        assert_eq!(
            ReadFile(
                handle,
                out.as_mut_ptr(),
                10,
                std::ptr::null_mut(),
                overlapped.as_mut_ptr().cast(),
            ),
            0
        );
        // SAFETY: reads thread-local Win32 last error.
        assert_eq!(unsafe { GetLastError() }, ERROR_NOACCESS);

        // 3b. Synchronous handle with non-null `overlapped` (offset) and non-null
        //     `read` must still clear `*read` to `0` on prepare failure.
        let mut n_ov = 10u32;
        // SAFETY: clearing thread-local last error before call.
        unsafe { SetLastError(0) };
        assert_eq!(
            ReadFile(
                handle,
                out.as_mut_ptr(),
                10,
                &mut n_ov,
                overlapped.as_mut_ptr().cast(),
            ),
            0
        );
        assert_eq!(
            n_ov, 0,
            "ReadFile failure with non-null overlapped and non-null read must clear *read to 0"
        );
        // SAFETY: reads thread-local Win32 last error.
        assert_eq!(unsafe { GetLastError() }, ERROR_NOACCESS);
        drop(_fail);

        // 4. Subsequent normal read still reads from offset 0.
        assert_ne!(
            ReadFile(handle, out.as_mut_ptr(), 10, &mut n, std::ptr::null_mut()),
            0
        );
        assert_eq!((&out, n), (b"0123456789", 10));
    }

    #[test]
    fn fread_reports_stream_error_on_prepare_failure_and_preserves_zero_and_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fr.bin");
        std::fs::write(&path, b"0123456789").unwrap();
        let host_path = format!("{}\0", path.display());
        let f = fopen(host_path.as_ptr().cast(), c"rb".as_ptr());
        assert!(!f.is_null());

        let feof = host!("ucrtbase.dll", "feof": extern "C" fn(File) -> i32);
        let ftell = host!("ucrtbase.dll", "ftell": extern "C" fn(File) -> i32);
        let get_errno = host!("ucrtbase.dll", "_get_errno": extern "C" fn(*mut i32) -> i32);
        const EACCES: i32 = 13;

        let mut out = [0x55u8; 10];
        {
            let _fail = FailPrepareGuard::at(out.as_ptr());
            // Zero-size requests keep returning 0 without setting stream error.
            assert_eq!(fread(out.as_mut_ptr(), 0, 10, f), 0);
            assert_eq!(ferror(f), 0);
            assert_eq!(fread(out.as_mut_ptr(), 10, 0, f), 0);
            assert_eq!(ferror(f), 0);

            // Nonzero request whose destination preparation fails must return 0,
            // set `ferror(f)`, leave `feof(f) == 0`, not advance file position,
            // and not touch the destination buffer.
            assert_eq!(fread(out.as_mut_ptr(), 1, 10, f), 0);
            assert_ne!(
                ferror(f),
                0,
                "failed nonzero fread preparation must set stream error indicator"
            );
            assert_eq!(feof(f), 0, "failed fread preparation must not set feof");
            assert_eq!(
                ftell(f),
                0,
                "failed fread preparation must not advance stream"
            );
            assert_eq!(out, [0x55; 10]);
            let mut err = 0i32;
            assert_eq!(get_errno(&mut err), 0);
            assert_eq!(err, EACCES);
        }

        // `clearerr` clears the stream error indicator; `rewind` does too.
        clearerr(f);
        assert_eq!(ferror(f), 0);
        {
            let _fail = FailPrepareGuard::at(out.as_ptr());
            assert_eq!(fread(out.as_mut_ptr(), 1, 10, f), 0);
            assert_ne!(ferror(f), 0);
        }
        rewind(f);
        assert_eq!(ferror(f), 0);

        // Normal read succeeds and leaves `ferror` clear.
        assert_eq!(fread(out.as_mut_ptr(), 1, 10, f), 10);
        assert_eq!(&out, b"0123456789");
        assert_eq!(ferror(f), 0);

        // `fclose` cleans up any pending error tracking for the closed stream pointer.
        {
            let _fail = FailPrepareGuard::at(out.as_ptr());
            assert_eq!(fread(out.as_mut_ptr(), 1, 10, f), 0);
            assert_ne!(ferror(f), 0);
        }
        assert_eq!(fclose(f), 0);
        assert!(!has_stream_error(f));
    }
}
