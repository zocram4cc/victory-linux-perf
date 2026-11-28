//! Media Foundation is the host's (`mfplat.dll` passes through), except for
//! the source resolver: the host's would fetch `http`, `rtsp` and other
//! URLs over the network, past `modules::net`. Titles get a proxy that
//! opens local files only, with paths on our virtual drives rewritten to
//! their host folders, and forwards everything else to the host's resolver.
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use super::files;
use super::xgameruntime::{HRESULT, S_OK};
use crate::{HleModule, hle_export};

const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;
const E_POINTER: HRESULT = 0x8000_4003_u32 as i32;
/// What Media Foundation answers for a URL scheme it has no handler for.
const UNSUPPORTED_SCHEME: HRESULT = 0xC00D_36C3_u32 as i32;

const IID_IUNKNOWN: [u8; 16] = guid(0x0000_0000, 0x0000, 0x0000, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_RESOLVER: [u8; 16] = guid(
    0xFBE5_A32D,
    0xA497,
    0x4B61,
    [0xBB, 0x85, 0x97, 0xB1, 0xA8, 0x48, 0xA6, 0xE3],
);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

// The URL rule.

/// Where a resolver may read `url` from: `Ok` with the NUL-terminated
/// string to hand to the host, or `Err((scheme, host))` for anything that
/// is not a local file.
fn local(url: &[u16]) -> Result<Vec<u16>, (String, String)> {
    let text = String::from_utf16_lossy(url);
    let drive = |s: &str| {
        let b = s.as_bytes();
        b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    };
    let host_of = |rest: &str| rest.split(['/', '\\']).next().unwrap_or("").to_string();
    // Two leading separators, of either kind: to the host, a share.
    let share = |s: &str| {
        let b = s.as_bytes();
        b.len() >= 2 && b[..2].iter().all(|c| b"/\\".contains(c))
    };
    let path = if let Some(rest) = text.strip_prefix(r"\\?\") {
        if !drive(rest) {
            return Err(("unc".into(), host_of(rest.trim_start_matches("UNC\\"))));
        }
        text.clone()
    } else if let Some(rest) = text.strip_prefix(r"\??\") {
        // The same prefix as the object manager spells it; the host's file
        // calls take it too.
        if !drive(rest) {
            return Err(("unc".into(), host_of(rest.trim_start_matches("UNC\\"))));
        }
        return Ok(with_nul(url));
    } else if share(&text) {
        return Err(("unc".into(), host_of(&text[2..])));
    } else if drive(&text) {
        text.clone()
    } else if let Some((scheme, rest)) = text.split_once(':').filter(|(s, _)| {
        s.len() > 1
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c))
    }) {
        let scheme = scheme.to_ascii_lowercase();
        let authority = rest.strip_prefix("//");
        if scheme != "file" {
            return Err((scheme, authority.map(host_of).unwrap_or_default()));
        }
        let (path, this_machine) = match authority {
            Some(rest) => {
                let host = host_of(rest);
                if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                    return Err((scheme, host));
                }
                (&rest[host.len()..], !host.is_empty())
            }
            None => (rest, false),
        };
        let whole = percent_decode(path);
        let path = percent_decode(path.trim_start_matches(['/', '\\'])).replace('/', "\\");
        if !drive(&path) {
            // With no host named, a path that starts with two separators
            // names one: `file:////server/share` is `\\server\share`.
            if !this_machine && share(&whole) {
                return Err((scheme, host_of(whole.trim_start_matches(['/', '\\']))));
            }
            // A local file without a drive: the host resolves it as is.
            return Ok(with_nul(url));
        }
        path
    } else {
        // A relative path.
        return Ok(with_nul(url));
    };
    let wide: Vec<u16> = path.encode_utf16().collect();
    Ok(files::translate(&wide).unwrap_or_else(|| with_nul(&wide)))
}

fn with_nul(s: &[u16]) -> Vec<u16> {
    s.iter().copied().chain([0]).collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // `from_str_radix` accepts a leading sign, so `%+1` would decode as
        // a control byte. A percent escape is exactly two hex digits.
        let hex = b
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(|c| c.is_ascii_hexdigit()))
            .and_then(|h| std::str::from_utf8(h).ok());
        match (b[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The URL to give the host, or the refusal (logged once per scheme and
/// host).
fn checked(url: *const u16) -> Result<Vec<u16>, HRESULT> {
    if url.is_null() {
        return Err(E_POINTER);
    }
    // SAFETY: a NUL-terminated wide string from the title.
    let len = (0..)
        .take_while(|&i| unsafe { url.add(i).read() } != 0)
        .count();
    // SAFETY: as above, `len` characters before the NUL.
    let wide = unsafe { std::slice::from_raw_parts(url, len) };
    local(wide).map_err(|(scheme, host)| {
        static REFUSED: Mutex<Option<HashSet<(String, String)>>> = Mutex::new(None);
        let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
        if refused
            .get_or_insert_with(HashSet::new)
            .insert((scheme.clone(), host.clone()))
        {
            tracing::warn!(
                scheme,
                host,
                "media: refused a URL that is not a local file"
            );
        }
        UNSUPPORTED_SCHEME
    })
}

// The proxy resolver.

/// The host's resolver, called through its method table.
#[repr(C)]
struct Host {
    vtable: *const usize,
}

impl Host {
    /// Method `n` of the host object, typed by the caller.
    unsafe fn method<F: Copy>(this: *mut Host, n: usize) -> F {
        // SAFETY: the caller names the slot's real signature; `this` is the
        // host's live resolver.
        unsafe { std::mem::transmute_copy(&*(*this).vtable.add(n)) }
    }
}

#[repr(C)]
pub struct Resolver {
    vtable: &'static Methods,
    host: *mut Host,
    refs: AtomicU32,
}

type Unknown = *mut c_void;

#[repr(C)]
struct Methods {
    query_interface:
        extern "system" fn(*mut Resolver, *const [u8; 16], *mut *mut c_void) -> HRESULT,
    add_ref: extern "system" fn(*mut Resolver) -> u32,
    release: extern "system" fn(*mut Resolver) -> u32,
    create_from_url: extern "system" fn(
        *mut Resolver,
        *const u16,
        u32,
        Unknown,
        *mut u32,
        *mut Unknown,
    ) -> HRESULT,
    create_from_stream: extern "system" fn(
        *mut Resolver,
        Unknown,
        *const u16,
        u32,
        Unknown,
        *mut u32,
        *mut Unknown,
    ) -> HRESULT,
    begin_from_url: extern "system" fn(
        *mut Resolver,
        *const u16,
        u32,
        Unknown,
        *mut Unknown,
        Unknown,
        Unknown,
    ) -> HRESULT,
    end_from_url: extern "system" fn(*mut Resolver, Unknown, *mut u32, *mut Unknown) -> HRESULT,
    begin_from_stream: extern "system" fn(
        *mut Resolver,
        Unknown,
        *const u16,
        u32,
        Unknown,
        *mut Unknown,
        Unknown,
        Unknown,
    ) -> HRESULT,
    end_from_stream: extern "system" fn(*mut Resolver, Unknown, *mut u32, *mut Unknown) -> HRESULT,
    cancel: extern "system" fn(*mut Resolver, Unknown) -> HRESULT,
}

static METHODS: Methods = Methods {
    query_interface,
    add_ref,
    release,
    create_from_url,
    create_from_stream,
    begin_from_url,
    end_from_url,
    begin_from_stream,
    end_from_stream,
    cancel,
};

fn host(this: *mut Resolver) -> *mut Host {
    // SAFETY: title code calls methods on a proxy we made, alive until its
    // last release.
    unsafe { (*this).host }
}

/// Only the resolver's own interfaces: anything else could hand out the
/// host object unguarded.
extern "system" fn query_interface(
    this: *mut Resolver,
    iid: *const [u8; 16],
    out: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null.
    let iid = unsafe { *iid };
    let found = iid == IID_IUNKNOWN || iid == IID_RESOLVER;
    // SAFETY: checked non-null.
    unsafe {
        out.write(if found {
            this.cast()
        } else {
            std::ptr::null_mut()
        })
    };
    if !found {
        return E_NOINTERFACE;
    }
    add_ref(this);
    S_OK
}

extern "system" fn add_ref(this: *mut Resolver) -> u32 {
    // SAFETY: as in `host`.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn release(this: *mut Resolver) -> u32 {
    // SAFETY: as in `host`.
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: made by `MFCreateSourceResolver`; this was the last
        // reference, to the proxy and to the host object it holds.
        let proxy = unsafe { Box::from_raw(this) };
        // SAFETY: slot 2 is `Release`.
        let release: extern "system" fn(*mut Host) -> u32 = unsafe { Host::method(proxy.host, 2) };
        release(proxy.host);
    }
    left
}

extern "system" fn create_from_url(
    this: *mut Resolver,
    url: *const u16,
    flags: u32,
    props: Unknown,
    kind: *mut u32,
    object: *mut Unknown,
) -> HRESULT {
    let url = match checked(url) {
        Ok(url) => url,
        Err(error) => {
            if !object.is_null() {
                // SAFETY: the title's out parameter.
                unsafe { object.write(std::ptr::null_mut()) };
            }
            return error;
        }
    };
    let host = host(this);
    // SAFETY: slot 3 is `CreateObjectFromURL`.
    let f: extern "system" fn(
        *mut Host,
        *const u16,
        u32,
        Unknown,
        *mut u32,
        *mut Unknown,
    ) -> HRESULT = unsafe { Host::method(host, 3) };
    f(host, url.as_ptr(), flags, props, kind, object)
}

extern "system" fn begin_from_url(
    this: *mut Resolver,
    url: *const u16,
    flags: u32,
    props: Unknown,
    cookie: *mut Unknown,
    callback: Unknown,
    state: Unknown,
) -> HRESULT {
    let url = match checked(url) {
        Ok(url) => url,
        Err(error) => return error,
    };
    let host = host(this);
    // SAFETY: slot 5 is `BeginCreateObjectFromURL`.
    let f: extern "system" fn(
        *mut Host,
        *const u16,
        u32,
        Unknown,
        *mut Unknown,
        Unknown,
        Unknown,
    ) -> HRESULT = unsafe { Host::method(host, 5) };
    f(host, url.as_ptr(), flags, props, cookie, callback, state)
}

extern "system" fn create_from_stream(
    this: *mut Resolver,
    stream: Unknown,
    url: *const u16,
    flags: u32,
    props: Unknown,
    kind: *mut u32,
    object: *mut Unknown,
) -> HRESULT {
    let host = host(this);
    // SAFETY: slot 4 is `CreateObjectFromByteStream`; the URL is only a
    // hint for picking a handler.
    let f: extern "system" fn(
        *mut Host,
        Unknown,
        *const u16,
        u32,
        Unknown,
        *mut u32,
        *mut Unknown,
    ) -> HRESULT = unsafe { Host::method(host, 4) };
    f(host, stream, url, flags, props, kind, object)
}

extern "system" fn end_from_url(
    this: *mut Resolver,
    result: Unknown,
    kind: *mut u32,
    object: *mut Unknown,
) -> HRESULT {
    let host = host(this);
    // SAFETY: slot 6 is `EndCreateObjectFromURL`.
    let f: extern "system" fn(*mut Host, Unknown, *mut u32, *mut Unknown) -> HRESULT =
        unsafe { Host::method(host, 6) };
    f(host, result, kind, object)
}

#[allow(clippy::too_many_arguments)]
extern "system" fn begin_from_stream(
    this: *mut Resolver,
    stream: Unknown,
    url: *const u16,
    flags: u32,
    props: Unknown,
    cookie: *mut Unknown,
    callback: Unknown,
    state: Unknown,
) -> HRESULT {
    let host = host(this);
    // SAFETY: slot 7 is `BeginCreateObjectFromByteStream`.
    let f: extern "system" fn(
        *mut Host,
        Unknown,
        *const u16,
        u32,
        Unknown,
        *mut Unknown,
        Unknown,
        Unknown,
    ) -> HRESULT = unsafe { Host::method(host, 7) };
    f(host, stream, url, flags, props, cookie, callback, state)
}

extern "system" fn end_from_stream(
    this: *mut Resolver,
    result: Unknown,
    kind: *mut u32,
    object: *mut Unknown,
) -> HRESULT {
    let host = host(this);
    // SAFETY: slot 8 is `EndCreateObjectFromByteStream`.
    let f: extern "system" fn(*mut Host, Unknown, *mut u32, *mut Unknown) -> HRESULT =
        unsafe { Host::method(host, 8) };
    f(host, result, kind, object)
}

extern "system" fn cancel(this: *mut Resolver, cookie: Unknown) -> HRESULT {
    let host = host(this);
    // SAFETY: slot 9 is `CancelObjectCreation`.
    let f: extern "system" fn(*mut Host, Unknown) -> HRESULT = unsafe { Host::method(host, 9) };
    f(host, cookie)
}

/// The host's resolver behind our proxy.
pub extern "system" fn MFCreateSourceResolver(out: *mut *mut Resolver) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    static CREATE: OnceLock<Option<usize>> = OnceLock::new();
    let Some(create) =
        *CREATE.get_or_init(|| crate::host::function("mfplat.dll", "MFCreateSourceResolver"))
    else {
        return UNSUPPORTED_SCHEME;
    };
    // SAFETY: the host's `MFCreateSourceResolver(IMFSourceResolver**)`.
    let create: extern "system" fn(*mut *mut Host) -> HRESULT =
        unsafe { std::mem::transmute(create) };
    let mut host = std::ptr::null_mut();
    let hr = create(&mut host);
    if hr < 0 || host.is_null() {
        return hr;
    }
    let proxy = Box::new(Resolver {
        vtable: &METHODS,
        host,
        refs: AtomicU32::new(1),
    });
    // SAFETY: checked non-null.
    unsafe { out.write(Box::into_raw(proxy)) };
    S_OK
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "media",
        dlls: &["mfplat.dll"],
        exports: vec![hle_export!(MFCreateSourceResolver)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn text(v: &[u16]) -> String {
        String::from_utf16_lossy(v.strip_suffix(&[0]).unwrap_or(v))
    }

    #[test]
    fn percent_decode_requires_two_hex_digits() {
        assert_eq!(percent_decode("%20"), " ");
        assert_eq!(percent_decode("%+1"), "%+1");
        assert_eq!(percent_decode("%2"), "%2");
        assert_eq!(percent_decode("%%20"), "% ");
        assert_eq!(percent_decode("a%2Gb"), "a%2Gb");
    }

    #[test]
    fn only_local_files_reach_the_host() {
        // Network schemes and shares are refused, naming scheme and host.
        let refused = |url: &str| local(&wide(url)).unwrap_err();
        assert_eq!(
            refused("https://cdn.example.com/a.mp4"),
            ("https".into(), "cdn.example.com".into())
        );
        assert_eq!(refused("RTSP://10.0.0.1:554/x").0, "rtsp");
        assert_eq!(refused("mms://h/x").0, "mms");
        assert_eq!(
            refused(r"\\server\share\a.mp4"),
            ("unc".into(), "server".into())
        );
        assert_eq!(
            refused("file://server/share/a.mp4"),
            ("file".into(), "server".into())
        );

        // A path on a virtual drive goes to its host folder, also as a
        // `file:` URL; other local files pass as they are.
        // Checked by opening the files: `files` may spell the host root
        // differently (absolute, `\\?\` for long paths) from the temp dir.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("movies")).unwrap();
        std::fs::write(dir.path().join("movies").join("intro.mp4"), b"1").unwrap();
        std::fs::write(dir.path().join("movies").join("intro a.mp4"), b"2").unwrap();
        let letter = files::mount(dir.path()).expect("a free drive letter");
        let host = text(&local(&wide(&format!(r"{letter}:\movies\intro.mp4"))).unwrap());
        assert_eq!(
            std::fs::read(&host).ok().as_deref(),
            Some(&b"1"[..]),
            "{host}"
        );
        let from_url =
            text(&local(&wide(&format!("file:///{letter}:/movies/intro%20a.mp4"))).unwrap());
        assert_eq!(
            std::fs::read(&from_url).ok().as_deref(),
            Some(&b"2"[..]),
            "{from_url}"
        );
        assert_eq!(
            text(&local(&wide("file:///C:/Windows/a.wav")).unwrap()),
            r"C:\Windows\a.wav"
        );
        assert_eq!(text(&local(&wide("clip.mp4")).unwrap()), "clip.mp4");
    }

    /// Other spellings of `\\server\share\a.mp4`, as paths and as `file:`
    /// URLs without a host.
    const SHARE_PATHS: [&str; 3] = [
        r"/\server\share\a.mp4",
        r"\/server/share/a.mp4",
        r"\??\UNC\server\share\a.mp4",
    ];
    const SHARE_URLS: [&str; 5] = [
        "file:////server/share/a.mp4",
        "file://///server/share/a.mp4",
        r"file:\\server\share\a.mp4",
        "file:/%5Cserver/share/a.mp4",
        "file:///%5C%5Cserver/share/a.mp4",
    ];

    #[test]
    fn shares_are_refused_however_they_are_spelled() {
        let asked = |url: &str| local(&wide(url)).map(|v| text(&v));
        for path in SHARE_PATHS {
            assert_eq!(asked(path), Err(("unc".into(), "server".into())), "{path}");
        }
        for url in SHARE_URLS {
            assert_eq!(asked(url), Err(("file".into(), "server".into())), "{url}");
        }
        // Local files that look alike still pass: one leading separator, a
        // drive after the slashes, a drive after the object prefix, and the
        // path of a URL that names this machine.
        for local_file in [
            r"\movies\a.mp4",
            "file:///movies/a.mp4",
            r"\??\C:\Windows\a.wav",
            "file://localhost//server/share/a.mp4",
        ] {
            assert_eq!(asked(local_file), Ok(local_file.into()), "{local_file}");
        }
        assert_eq!(
            asked("file:////C:/Windows/a.wav"),
            Ok(r"C:\Windows\a.wav".into())
        );
    }

    /// What the host makes of those spellings: the share, for each.
    #[cfg(windows)]
    #[test]
    fn the_host_reads_those_spellings_as_shares() {
        for path in &SHARE_PATHS[..2] {
            assert_eq!(
                std::path::absolute(path).unwrap().to_str(),
                Some(r"\\server\share\a.mp4"),
                "{path}"
            );
        }
        // The object prefix reaches the host's file calls as it is.
        let windows = std::env::var("SystemRoot").unwrap();
        assert!(std::fs::metadata(format!(r"\??\{windows}")).is_ok());

        let from_url = files::host!("shlwapi.dll", "PathCreateFromUrlW":
            extern "system" fn(*const u16, *mut u16, *mut u32, u32) -> HRESULT);
        let host_path = |url: &str| {
            let url: Vec<u16> = url.encode_utf16().chain([0]).collect();
            let (mut path, mut n) = ([0u16; 260], 260);
            assert_eq!(from_url(url.as_ptr(), path.as_mut_ptr(), &mut n, 0), S_OK);
            String::from_utf16_lossy(&path[..n as usize])
        };
        for url in SHARE_URLS {
            let path = host_path(url);
            assert!(
                path.starts_with(r"\\") && path.ends_with(r"server\share\a.mp4"),
                "{url}: {path}"
            );
        }
        // And of the ones that pass: local paths.
        assert_eq!(
            host_path("file://localhost//server/share/a.mp4"),
            r"\server\share\a.mp4"
        );
        assert_eq!(host_path("file:////C:/Windows/a.wav"), r"C:\Windows\a.wav");
        assert_eq!(host_path("file:///movies/a.mp4"), r"\movies\a.mp4");
    }

    #[test]
    fn interface_ids_are_the_resolvers() {
        assert_eq!(&IID_IUNKNOWN[8..], &[0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
        assert_eq!(&IID_RESOLVER[..4], &[0x2D, 0xA3, 0xE5, 0xFB]);
    }
}
