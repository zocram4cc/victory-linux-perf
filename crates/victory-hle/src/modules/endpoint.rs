//! Audio outputs as a console has them.
//!
//! The title's sound engine and its video player (Bink) reach the host's
//! WASAPI themselves, through `CoCreateInstance`. They were written for the
//! console, where the output is always there and always mixes at 48 kHz. On
//! a PC the default output can be one that will not open (a monitor's HDMI
//! audio: `AUDCLNT_E_DEVICE_INVALIDATED`) or one that mixes at another rate
//! (a Bluetooth speaker at 44.1 kHz). Either way the title's audio never
//! starts, and its loading screen waits for it forever (until the frame's
//! command buffer runs out and the title stops at its own assert).
//!
//! So the device enumerator a title makes is a proxy of ours:
//! * the default output is the host's default if it opens, else another
//!   active output that opens, else a silent output of ours that plays
//!   nothing on a 48 kHz clock;
//! * an output's mix format reads 48 kHz (32-bit float, the device's own
//!   channels), and a shared stream in a format other than the device's is
//!   made with `AUTOCONVERTPCM`, so the host converts.
//!
//! `VICTORY_AUDIO_DEVICE=<text>`: the output whose id or name contains the
//! text (`silent`: ours). `VICTORY_AUDIO_CONSOLE=0`: the host's enumerator
//! as it is.
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::xgameruntime::{HRESULT, S_OK};
use crate::modules::files::host;
use crate::{HleModule, hle_export};

type Guid = [u8; 16];

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const CLSID_ENUMERATOR: Guid = guid(0xBCDE_0395, 0xE52F, 0x467C, [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E]);
const IID_ENUMERATOR: Guid = guid(0xA956_64D2, 0x9614, 0x4F35, [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6]);
const IID_DEVICE: Guid = guid(0xD666_063F, 0x1587, 0x4E43, [0x81, 0xF1, 0xB9, 0x48, 0xE8, 0x07, 0x36, 0x3F]);
const IID_CLIENT: Guid = guid(0x1CB9_AD4C, 0xDBFA, 0x4C32, [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2]);
const IID_CLIENT2: Guid = guid(0x7267_78CD, 0xF60A, 0x4EDA, [0x82, 0xDE, 0xE4, 0x76, 0x10, 0xCD, 0x78, 0xAA]);
const IID_CLIENT3: Guid = guid(0x7ED4_EE07, 0x8E67, 0x4CD4, [0x8C, 0x1A, 0x2B, 0x7A, 0x59, 0x87, 0xAD, 0x42]);
const IID_RENDER: Guid = guid(0xF294_ACFC, 0x3146, 0x4483, [0xA7, 0xBF, 0xAD, 0xDC, 0xA7, 0xC2, 0x60, 0xE2]);
const IID_CLOCK: Guid = guid(0xCD63_314F, 0x3FBA, 0x4A1B, [0x81, 0x2C, 0xEF, 0x96, 0x35, 0x87, 0x28, 0xE7]);
const IID_SPATIAL: Guid = guid(0xBBF8_E066, 0xAAAA, 0x49BE, [0x9A, 0x4D, 0xFD, 0x2A, 0x85, 0x8E, 0xA2, 0x7F]);
const IID_SPATIAL2: Guid = guid(0xCAAB_E452, 0xA66A, 0x4BEE, [0xA9, 0x3E, 0xE3, 0x20, 0x46, 0x3F, 0x6A, 0x53]);
const SUBTYPE_FLOAT: Guid = guid(0x0000_0003, 0x0000, 0x0010, [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
/// `PKEY_Device_FriendlyName`.
const NAME_KEY: (Guid, u32) = (
    guid(0xA45C_254E, 0xDF1C, 0x4EFD, [0x80, 0x20, 0x67, 0xD1, 0x46, 0xA8, 0x50, 0xE0]),
    14,
);

const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;
const E_POINTER: HRESULT = 0x8000_4003_u32 as i32;
const E_NOTIMPL: HRESULT = 0x8000_4001_u32 as i32;
const E_NOTFOUND: HRESULT = 0x8007_0490_u32 as i32;
const NOT_INITIALIZED: HRESULT = 0x8889_0001_u32 as i32;
const ALREADY_INITIALIZED: HRESULT = 0x8889_0002_u32 as i32;
const BUFFER_TOO_LARGE: HRESULT = 0x8889_0006_u32 as i32;
const OUT_OF_ORDER: HRESULT = 0x8889_0007_u32 as i32;
const INVALID_SIZE: HRESULT = 0x8889_0011_u32 as i32;
const SHARED: i32 = 0;
const RENDER: i32 = 0;
const ACTIVE: u32 = 1;
const CLSCTX_ALL: u32 = 0x17;
const EVENT_CALLBACK: u32 = 0x0004_0000;
const AUTOCONVERT: u32 = 0x8000_0000 | 0x0800_0000;
/// The console's mixing rate.
const RATE: u32 = 48_000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Logs `what` the first time only.
fn once(what: String) -> bool {
    static SEEN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
    lock(&SEEN).insert(what)
}

fn on() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("VICTORY_AUDIO_CONSOLE").map_or(true, |v| v.trim() != "0"));
    *ON
}

fn wanted() -> Option<String> {
    static WANTED: LazyLock<Option<String>> = LazyLock::new(|| {
        std::env::var("VICTORY_AUDIO_DEVICE").ok().map(|v| v.trim().to_lowercase()).filter(|v| !v.is_empty())
    });
    WANTED.clone()
}

fn show(g: &Guid) -> String {
    let d1 = u32::from_le_bytes([g[0], g[1], g[2], g[3]]);
    let d2 = u16::from_le_bytes([g[4], g[5]]);
    let d3 = u16::from_le_bytes([g[6], g[7]]);
    format!(
        "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

fn alloc(bytes: usize) -> *mut u8 {
    host!("ole32.dll", "CoTaskMemAlloc": extern "system" fn(usize) -> *mut u8)(bytes)
}

fn free(p: *mut c_void) {
    host!("ole32.dll", "CoTaskMemFree": extern "system" fn(*mut c_void))(p)
}

/// A wide string the host allocated, read and freed.
fn take_wide(p: *mut u16) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated string from the host.
    let len = (0..).take_while(|&i| unsafe { p.add(i).read() } != 0).count();
    // SAFETY: `len` characters before the NUL.
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) });
    free(p.cast());
    s
}

// Host objects.

/// A host COM object, called through its method table.
#[repr(C)]
struct Host {
    vtable: *const usize,
}

impl Host {
    /// Method `n` of a live host object, typed by the caller.
    unsafe fn method<F: Copy>(this: *mut Host, n: usize) -> F {
        // SAFETY: the caller names the slot's real signature.
        unsafe { std::mem::transmute_copy(&*(*this).vtable.add(n)) }
    }

    fn query(this: *mut Host, iid: &Guid) -> Option<*mut Host> {
        let mut out = std::ptr::null_mut();
        // SAFETY: slot 0 of every interface is `QueryInterface`.
        let f: extern "system" fn(*mut Host, *const Guid, *mut *mut Host) -> HRESULT =
            unsafe { Host::method(this, 0) };
        (f(this, iid, &mut out) >= 0 && !out.is_null()).then_some(out)
    }

    fn add_ref(this: *mut Host) {
        // SAFETY: slot 1 is `AddRef`.
        let f: extern "system" fn(*mut Host) -> u32 = unsafe { Host::method(this, 1) };
        f(this);
    }

    fn release(this: *mut Host) {
        if this.is_null() {
            return;
        }
        // SAFETY: slot 2 is `Release`.
        let f: extern "system" fn(*mut Host) -> u32 = unsafe { Host::method(this, 2) };
        f(this);
    }
}

/// A sample format, as `WAVEFORMATEX(TENSIBLE)` says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Format {
    rate: u32,
    channels: u16,
    bits: u16,
    float: bool,
    mask: u32,
}

impl Format {
    fn read(p: *const u8) -> Option<Format> {
        if p.is_null() {
            return None;
        }
        // SAFETY: a `WAVEFORMATEX` from the title or the host; the extended
        // part only when its tag and size say it is there.
        unsafe {
            let u16_at = |o: usize| p.add(o).cast::<u16>().read_unaligned();
            let u32_at = |o: usize| p.add(o).cast::<u32>().read_unaligned();
            let tag = u16_at(0);
            let (float, mask) = if tag == 0xFFFE && u16_at(16) >= 22 {
                let sub: Guid = p.add(24).cast::<Guid>().read_unaligned();
                (sub == SUBTYPE_FLOAT, u32_at(20))
            } else {
                (tag == 3, 0)
            };
            Some(Format { rate: u32_at(4), channels: u16_at(2), bits: u16_at(14), float, mask })
        }
    }

    /// The console's format: 48 kHz float, these channels.
    fn console(channels: u16, mask: u32) -> Format {
        Format { rate: RATE, channels, bits: 32, float: true, mask }
    }

    fn block(&self) -> u32 {
        u32::from(self.channels) * u32::from(self.bits) / 8
    }

    /// As a `WAVEFORMATEXTENSIBLE` in memory the caller frees with
    /// `CoTaskMemFree`.
    fn allocate(&self) -> *mut u8 {
        let mut bytes = [0u8; 40];
        bytes[0..2].copy_from_slice(&0xFFFE_u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&self.channels.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.rate.to_le_bytes());
        bytes[8..12].copy_from_slice(&(self.rate * self.block()).to_le_bytes());
        bytes[12..14].copy_from_slice(&(self.block() as u16).to_le_bytes());
        bytes[14..16].copy_from_slice(&self.bits.to_le_bytes());
        bytes[16..18].copy_from_slice(&22_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&self.bits.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.mask.to_le_bytes());
        bytes[24..40].copy_from_slice(&SUBTYPE_FLOAT);
        let p = alloc(bytes.len());
        if !p.is_null() {
            // SAFETY: just allocated with room for the bytes.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
        }
        p
    }
}

/// What `VICTORY_AUDIO_*` and the outputs decided, said once per change.
fn said(what: String) {
    if once(what.clone()) {
        tracing::info!("audio output: {what}");
    }
}

// A host device: its id, name and whether a stream opens on it.

fn device_id(device: *mut Host) -> String {
    let mut id = std::ptr::null_mut();
    // SAFETY: slot 5 of `IMMDevice` is `GetId`.
    let f: extern "system" fn(*mut Host, *mut *mut u16) -> HRESULT = unsafe { Host::method(device, 5) };
    if f(device, &mut id) < 0 {
        return String::new();
    }
    take_wide(id)
}

fn device_name(device: *mut Host) -> String {
    let mut store = std::ptr::null_mut::<Host>();
    // SAFETY: slot 4 of `IMMDevice` is `OpenPropertyStore`; STGM_READ.
    let open: extern "system" fn(*mut Host, u32, *mut *mut Host) -> HRESULT = unsafe { Host::method(device, 4) };
    if open(device, 0, &mut store) < 0 || store.is_null() {
        return String::new();
    }
    #[repr(C)]
    struct Key {
        fmtid: Guid,
        pid: u32,
    }
    let key = Key { fmtid: NAME_KEY.0, pid: NAME_KEY.1 };
    let mut value = [0u64; 3];
    // SAFETY: slot 5 of `IPropertyStore` is `GetValue` (key, PROPVARIANT).
    let get: extern "system" fn(*mut Host, *const Key, *mut [u64; 3]) -> HRESULT = unsafe { Host::method(store, 5) };
    let mut name = String::new();
    if get(store, &key, &mut value) >= 0 {
        // VT_LPWSTR (31): the string's pointer at offset 8.
        if value[0] & 0xFFFF == 31 && value[1] != 0 {
            let p = value[1] as *const u16;
            // SAFETY: the property's NUL-terminated string.
            let len = (0..).take_while(|&i| unsafe { p.add(i).read() } != 0).count();
            // SAFETY: `len` characters before the NUL.
            name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) });
        }
        host!("ole32.dll", "PropVariantClear": extern "system" fn(*mut [u64; 3]) -> HRESULT)(&mut value);
    }
    Host::release(store);
    name
}

fn activate(device: *mut Host, iid: &Guid) -> Result<*mut Host, HRESULT> {
    let mut out = std::ptr::null_mut();
    // SAFETY: slot 3 of `IMMDevice` is `Activate`.
    let f: extern "system" fn(*mut Host, *const Guid, u32, *mut c_void, *mut *mut Host) -> HRESULT =
        unsafe { Host::method(device, 3) };
    let hr = f(device, iid, CLSCTX_ALL, std::ptr::null_mut(), &mut out);
    if hr < 0 || out.is_null() { Err(hr) } else { Ok(out) }
}

/// The host client's mix format.
fn mix_format(client: *mut Host) -> Result<Format, HRESULT> {
    let mut p = std::ptr::null_mut::<u8>();
    // SAFETY: slot 8 of `IAudioClient` is `GetMixFormat`.
    let f: extern "system" fn(*mut Host, *mut *mut u8) -> HRESULT = unsafe { Host::method(client, 8) };
    let hr = f(client, &mut p);
    if hr < 0 {
        return Err(hr);
    }
    let format = Format::read(p);
    free(p.cast());
    format.ok_or(E_POINTER)
}

/// Whether a shared stream opens on the device: what the title would do
/// first. `Err` holds the host's answer.
fn opens(device: *mut Host) -> Result<Format, HRESULT> {
    let client = activate(device, &IID_CLIENT)?;
    let result = (|| {
        let mut p = std::ptr::null_mut::<u8>();
        // SAFETY: slot 8 is `GetMixFormat`.
        let mix: extern "system" fn(*mut Host, *mut *mut u8) -> HRESULT = unsafe { Host::method(client, 8) };
        let hr = mix(client, &mut p);
        if hr < 0 {
            return Err(hr);
        }
        let format = Format::read(p).ok_or(E_POINTER)?;
        // SAFETY: slot 3 is `Initialize`.
        let init: extern "system" fn(*mut Host, i32, u32, i64, i64, *const u8, *const Guid) -> HRESULT =
            unsafe { Host::method(client, 3) };
        let hr = init(client, SHARED, 0, 200_000, 0, p, std::ptr::null());
        free(p.cast());
        if hr < 0 { Err(hr) } else { Ok(format) }
    })();
    Host::release(client);
    result
}

/// The render outputs to try, best first: the host's default for `role`,
/// then the other active ones.
fn candidates(enumerator: *mut Host, role: i32) -> Vec<*mut Host> {
    let mut list = Vec::new();
    let mut default = std::ptr::null_mut::<Host>();
    // SAFETY: slot 4 of `IMMDeviceEnumerator` is `GetDefaultAudioEndpoint`.
    let get_default: extern "system" fn(*mut Host, i32, i32, *mut *mut Host) -> HRESULT =
        unsafe { Host::method(enumerator, 4) };
    let default_id = if get_default(enumerator, RENDER, role, &mut default) >= 0 && !default.is_null() {
        list.push(default);
        device_id(default)
    } else {
        String::new()
    };
    let mut collection = std::ptr::null_mut::<Host>();
    // SAFETY: slot 3 is `EnumAudioEndpoints`.
    let all: extern "system" fn(*mut Host, i32, u32, *mut *mut Host) -> HRESULT = unsafe { Host::method(enumerator, 3) };
    if all(enumerator, RENDER, ACTIVE, &mut collection) >= 0 && !collection.is_null() {
        let mut count = 0u32;
        // SAFETY: `IMMDeviceCollection` slots 3 and 4: `GetCount`, `Item`.
        let get_count: extern "system" fn(*mut Host, *mut u32) -> HRESULT = unsafe { Host::method(collection, 3) };
        // SAFETY: as above.
        let item: extern "system" fn(*mut Host, u32, *mut *mut Host) -> HRESULT = unsafe { Host::method(collection, 4) };
        get_count(collection, &mut count);
        for i in 0..count {
            let mut device = std::ptr::null_mut();
            if item(collection, i, &mut device) >= 0 && !device.is_null() {
                if device_id(device) == default_id {
                    Host::release(device);
                } else {
                    list.push(device);
                }
            }
        }
        Host::release(collection);
    }
    list
}

/// The output the title gets for `role`: a proxy over a host device that
/// opens, or our silent one.
fn choose(enumerator: *mut Host, role: i32) -> *mut Device {
    let wanted = wanted();
    if wanted.as_deref() == Some("silent") {
        said("the silent output (VICTORY_AUDIO_DEVICE=silent)".into());
        return Device::silent();
    }
    let mut chosen = None;
    let mut refused = Vec::new();
    for device in candidates(enumerator, role) {
        if chosen.is_some() {
            Host::release(device);
            continue;
        }
        let (id, name) = (device_id(device), device_name(device));
        if let Some(w) = &wanted
            && !id.to_lowercase().contains(w)
            && !name.to_lowercase().contains(w)
        {
            Host::release(device);
            continue;
        }
        match opens(device) {
            Ok(format) => chosen = Some((device, name, format)),
            Err(hr) => {
                refused.push(format!("{name} ({hr:#010x})"));
                Host::release(device);
            }
        }
    }
    let refused = if refused.is_empty() { String::new() } else { format!("; would not open: {}", refused.join(", ")) };
    match chosen {
        Some((device, name, format)) => {
            said(format!(
                "{name}, {} channels at {} Hz{} (the title sees {RATE} Hz){refused}",
                format.channels,
                format.rate,
                if format.rate == RATE { "" } else { ", converted by the host" },
            ));
            Device::wrap(device, name)
        }
        None => {
            said(format!("no output opens: the silent one{refused}"));
            Device::silent()
        }
    }
}

// Reference counting, the same for every proxy.

trait Proxy {
    fn refs(&self) -> &AtomicU32;
}

extern "system" fn add_ref<T: Proxy>(this: *mut T) -> u32 {
    // SAFETY: a proxy of ours, alive until its last release.
    unsafe { (*this).refs().fetch_add(1, Ordering::AcqRel) + 1 }
}

extern "system" fn release<T: Proxy>(this: *mut T) -> u32 {
    // SAFETY: as above.
    let left = unsafe { (*this).refs().fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: made by `Box::into_raw`; the last reference is gone.
        drop(unsafe { Box::from_raw(this) });
    }
    left
}

/// `QueryInterface` answers: ourselves for `ours`, else the host's own
/// object if it has the interface (or nothing for a proxy without one).
fn answer<T: Proxy>(this: *mut T, host: *mut Host, ours: &[Guid], iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null.
    let iid = unsafe { *iid };
    let found = if iid == IID_IUNKNOWN || ours.contains(&iid) {
        add_ref(this);
        this.cast()
    } else if !host.is_null() {
        Host::query(host, &iid).map_or(std::ptr::null_mut(), |p| p.cast())
    } else {
        std::ptr::null_mut()
    };
    // SAFETY: checked non-null.
    unsafe { out.write(found) };
    if found.is_null() { E_NOINTERFACE } else { S_OK }
}

// The enumerator.

#[repr(C)]
struct Enumerator {
    vtable: &'static EnumeratorMethods,
    host: *mut Host,
    refs: AtomicU32,
}

impl Proxy for Enumerator {
    fn refs(&self) -> &AtomicU32 {
        &self.refs
    }
}

impl Drop for Enumerator {
    fn drop(&mut self) {
        Host::release(self.host);
    }
}

#[repr(C)]
struct EnumeratorMethods {
    query_interface: extern "system" fn(*mut Enumerator, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: extern "system" fn(*mut Enumerator) -> u32,
    release: extern "system" fn(*mut Enumerator) -> u32,
    enum_endpoints: extern "system" fn(*mut Enumerator, i32, u32, *mut *mut c_void) -> HRESULT,
    default_endpoint: extern "system" fn(*mut Enumerator, i32, i32, *mut *mut c_void) -> HRESULT,
    device: extern "system" fn(*mut Enumerator, *const u16, *mut *mut c_void) -> HRESULT,
    register: extern "system" fn(*mut Enumerator, *mut c_void) -> HRESULT,
    unregister: extern "system" fn(*mut Enumerator, *mut c_void) -> HRESULT,
}

static ENUMERATOR: EnumeratorMethods = EnumeratorMethods {
    query_interface: enumerator_query,
    add_ref: add_ref::<Enumerator>,
    release: release::<Enumerator>,
    enum_endpoints,
    default_endpoint,
    device: enumerator_device,
    register,
    unregister,
};

extern "system" fn register(this: *mut Enumerator, callback: *mut c_void) -> HRESULT {
    // SAFETY: a proxy of ours.
    let host = unsafe { (*this).host };
    // SAFETY: slot 6 is `RegisterEndpointNotificationCallback`.
    let f: extern "system" fn(*mut Host, *mut c_void) -> HRESULT = unsafe { Host::method(host, 6) };
    f(host, callback)
}

extern "system" fn unregister(this: *mut Enumerator, callback: *mut c_void) -> HRESULT {
    // SAFETY: a proxy of ours.
    let host = unsafe { (*this).host };
    // SAFETY: slot 7 is `UnregisterEndpointNotificationCallback`.
    let f: extern "system" fn(*mut Host, *mut c_void) -> HRESULT = unsafe { Host::method(host, 7) };
    f(host, callback)
}

extern "system" fn enumerator_query(this: *mut Enumerator, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
    answer(this, std::ptr::null_mut(), &[IID_ENUMERATOR], iid, out)
}

/// The host's list as it is (its devices are not ours); said once.
extern "system" fn enum_endpoints(this: *mut Enumerator, flow: i32, mask: u32, out: *mut *mut c_void) -> HRESULT {
    // SAFETY: a proxy of ours; slot 3 is `EnumAudioEndpoints`.
    let host = unsafe { (*this).host };
    let f: extern "system" fn(*mut Host, i32, u32, *mut *mut c_void) -> HRESULT = unsafe { Host::method(host, 3) };
    let hr = f(host, flow, mask, out);
    if once(format!("enum {flow} {mask}")) {
        tracing::info!(flow, mask, hr = format_args!("{hr:#010x}"), "audio output: the title lists the host's devices");
    }
    hr
}

extern "system" fn default_endpoint(this: *mut Enumerator, flow: i32, role: i32, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a proxy of ours.
    let host = unsafe { (*this).host };
    if flow != RENDER {
        // SAFETY: slot 4 is `GetDefaultAudioEndpoint`.
        let f: extern "system" fn(*mut Host, i32, i32, *mut *mut c_void) -> HRESULT = unsafe { Host::method(host, 4) };
        return f(host, flow, role, out);
    }
    let device = choose(host, role);
    // SAFETY: checked non-null.
    unsafe { out.write(device.cast()) };
    S_OK
}

extern "system" fn enumerator_device(this: *mut Enumerator, id: *const u16, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a proxy of ours; slot 5 is `GetDevice`.
    let host = unsafe { (*this).host };
    let f: extern "system" fn(*mut Host, *const u16, *mut *mut Host) -> HRESULT = unsafe { Host::method(host, 5) };
    let mut device = std::ptr::null_mut();
    let hr = f(host, id, &mut device);
    if hr < 0 || device.is_null() {
        return if hr < 0 { hr } else { E_NOTFOUND };
    }
    let name = device_name(device);
    // SAFETY: checked non-null.
    unsafe { out.write(Device::wrap(device, name).cast()) };
    S_OK
}

// A device.

#[repr(C)]
struct Device {
    vtable: &'static DeviceMethods,
    /// Null: our silent output.
    host: *mut Host,
    refs: AtomicU32,
    name: String,
}

impl Proxy for Device {
    fn refs(&self) -> &AtomicU32 {
        &self.refs
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        Host::release(self.host);
    }
}

impl Device {
    fn wrap(host: *mut Host, name: String) -> *mut Device {
        Box::into_raw(Box::new(Device { vtable: &DEVICE, host, refs: AtomicU32::new(1), name }))
    }

    fn silent() -> *mut Device {
        Device::wrap(std::ptr::null_mut(), "silent".into())
    }
}

#[repr(C)]
struct DeviceMethods {
    query_interface: extern "system" fn(*mut Device, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: extern "system" fn(*mut Device) -> u32,
    release: extern "system" fn(*mut Device) -> u32,
    activate: extern "system" fn(*mut Device, *const Guid, u32, *mut c_void, *mut *mut c_void) -> HRESULT,
    open_property_store: extern "system" fn(*mut Device, u32, *mut *mut c_void) -> HRESULT,
    id: extern "system" fn(*mut Device, *mut *mut u16) -> HRESULT,
    state: extern "system" fn(*mut Device, *mut u32) -> HRESULT,
}

static DEVICE: DeviceMethods = DeviceMethods {
    query_interface: device_query,
    add_ref: add_ref::<Device>,
    release: release::<Device>,
    activate: device_activate,
    open_property_store: device_property_store,
    id: device_get_id,
    state: device_state,
};

extern "system" fn device_property_store(this: *mut Device, mode: u32, out: *mut *mut c_void) -> HRESULT {
    // SAFETY: a proxy of ours.
    let host = unsafe { (*this).host };
    if host.is_null() {
        return E_NOTIMPL;
    }
    // SAFETY: slot 4 is `OpenPropertyStore`.
    let f: extern "system" fn(*mut Host, u32, *mut *mut c_void) -> HRESULT = unsafe { Host::method(host, 4) };
    f(host, mode, out)
}

extern "system" fn device_get_id(this: *mut Device, out: *mut *mut u16) -> HRESULT {
    // SAFETY: a proxy of ours.
    let host = unsafe { (*this).host };
    if !host.is_null() {
        // SAFETY: slot 5 is `GetId`.
        let f: extern "system" fn(*mut Host, *mut *mut u16) -> HRESULT = unsafe { Host::method(host, 5) };
        return f(host, out);
    }
    if out.is_null() {
        return E_POINTER;
    }
    let id: Vec<u16> = "{0.0.0.00000000}.{victory-silent}".encode_utf16().chain([0]).collect();
    let p = alloc(id.len() * 2).cast::<u16>();
    if !p.is_null() {
        // SAFETY: just allocated with room for the id.
        unsafe { std::ptr::copy_nonoverlapping(id.as_ptr(), p, id.len()) };
    }
    // SAFETY: checked non-null.
    unsafe { out.write(p) };
    S_OK
}

extern "system" fn device_state(this: *mut Device, out: *mut u32) -> HRESULT {
    // SAFETY: a proxy of ours.
    let host = unsafe { (*this).host };
    if !host.is_null() {
        // SAFETY: slot 6 is `GetState`.
        let f: extern "system" fn(*mut Host, *mut u32) -> HRESULT = unsafe { Host::method(host, 6) };
        return f(host, out);
    }
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null.
    unsafe { out.write(ACTIVE) };
    S_OK
}

extern "system" fn device_query(this: *mut Device, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
    // SAFETY: a proxy of ours.
    let host = unsafe { (*this).host };
    answer(this, host, &[IID_DEVICE], iid, out)
}

extern "system" fn device_activate(
    this: *mut Device,
    iid: *const Guid,
    context: u32,
    params: *mut c_void,
    out: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a proxy of ours; `iid` checked non-null.
    let (host, iid) = unsafe { ((*this).host, *iid) };
    let level = match iid {
        i if i == IID_CLIENT => 1,
        i if i == IID_CLIENT2 => 2,
        i if i == IID_CLIENT3 => 3,
        _ => 0,
    };
    if level == 0 {
        if host.is_null() {
            if iid == IID_SPATIAL || iid == IID_SPATIAL2 {
                // SAFETY: checked non-null.
                unsafe { out.write(spatial::client()) };
                return S_OK;
            }
            if once(format!("silent activate {}", show(&iid))) {
                tracing::info!(iid = show(&iid), "audio output: the silent output has no such interface");
            }
            return E_NOINTERFACE;
        }
        if once(format!("activate {}", show(&iid))) {
            tracing::info!(iid = show(&iid), "audio output: the title activates another interface of the host's device");
        }
        // SAFETY: slot 3 is `Activate`.
        let f: extern "system" fn(*mut Host, *const Guid, u32, *mut c_void, *mut *mut c_void) -> HRESULT =
            unsafe { Host::method(host, 3) };
        return f(host, &iid, context, params, out);
    }
    // SAFETY: a proxy of ours.
    let name = unsafe { (*this).name.clone() };
    let client = if host.is_null() {
        Client::silent()
    } else {
        match Client::wrap(host, level, name) {
            Ok(client) => client,
            Err(hr) => return hr,
        }
    };
    // SAFETY: checked non-null.
    unsafe { out.write(client.cast()) };
    S_OK
}

// A client.

/// Our silent output's stream: consumes what is written at the format's
/// rate, as the clock says.
struct Silent {
    format: Option<Format>,
    frames: u32,
    event: usize,
    period: Duration,
    /// Frames written, and frames played by `mark`.
    written: u64,
    played: u64,
    mark: Option<Instant>,
    held: Option<u32>,
    scratch: Vec<u8>,
    ticking: Option<Arc<AtomicBool>>,
}

impl Silent {
    /// Brings `played` up to now.
    fn advance(&mut self) {
        let (Some(mark), Some(format)) = (self.mark, self.format) else {
            return;
        };
        let now = Instant::now();
        let due = (now - mark).as_secs_f64() * f64::from(format.rate);
        self.played = (self.played + due as u64).min(self.written);
        self.mark = Some(now - Duration::from_secs_f64(due.fract() / f64::from(format.rate)));
    }

    fn padding(&mut self) -> u32 {
        self.advance();
        (self.written - self.played) as u32
    }

    fn stop_ticking(&mut self) {
        if let Some(t) = self.ticking.take() {
            t.store(false, Ordering::Release);
        }
    }
}

#[repr(C)]
struct Client {
    vtable: &'static ClientMethods,
    /// Null: our silent output.
    host: *mut Host,
    refs: AtomicU32,
    level: u32,
    /// The host's mix format (ours for the silent output).
    mix: Format,
    silent: Mutex<Silent>,
}

impl Proxy for Client {
    fn refs(&self) -> &AtomicU32 {
        &self.refs
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        lock(&self.silent).stop_ticking();
        Host::release(self.host);
    }
}

impl Client {
    fn new(host: *mut Host, level: u32, mix: Format) -> *mut Client {
        Box::into_raw(Box::new(Client {
            vtable: &CLIENT,
            host,
            refs: AtomicU32::new(1),
            level,
            mix,
            silent: Mutex::new(Silent {
                format: None,
                frames: 0,
                event: 0,
                period: Duration::from_millis(10),
                written: 0,
                played: 0,
                mark: None,
                held: None,
                scratch: Vec::new(),
                ticking: None,
            }),
        }))
    }

    /// A client over the host device's, at the highest interface level it
    /// has (at least `level`).
    fn wrap(device: *mut Host, level: u32, name: String) -> Result<*mut Client, HRESULT> {
        let first = activate(device, &IID_CLIENT)?;
        let (host, has) = if let Some(c) = Host::query(first, &IID_CLIENT3) {
            (c, 3)
        } else if let Some(c) = Host::query(first, &IID_CLIENT2) {
            (c, 2)
        } else {
            Host::add_ref(first);
            (first, 1)
        };
        Host::release(first);
        if has < level {
            Host::release(host);
            return Err(E_NOINTERFACE);
        }
        let mix = match mix_format(host) {
            Ok(mix) => mix,
            Err(hr) => {
                Host::release(host);
                return Err(hr);
            }
        };
        if once(format!("client {name} {mix:?}")) {
            tracing::info!(device = name, rate = mix.rate, channels = mix.channels, bits = mix.bits, float = mix.float, "audio output: a stream on the host's device");
        }
        Ok(Client::new(host, has, mix))
    }

    fn silent() -> *mut Client {
        Client::new(std::ptr::null_mut(), 3, Format::console(2, 0x3))
    }
}

fn client<'a>(this: *mut Client) -> &'a Client {
    // SAFETY: a proxy of ours, alive while the title calls it.
    unsafe { &*this }
}

/// What the title sees as the mix format.
fn presented(c: &Client) -> Format {
    Format::console(c.mix.channels, c.mix.mask)
}

/// Whether the host converts a shared stream in `format` to the device's.
fn convertible(format: &Format) -> bool {
    (1..=8).contains(&format.channels)
        && (8_000..=384_000).contains(&format.rate)
        && if format.float { format.bits == 32 } else { [16, 24, 32].contains(&format.bits) }
}

#[repr(C)]
struct ClientMethods {
    query_interface: extern "system" fn(*mut Client, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: extern "system" fn(*mut Client) -> u32,
    release: extern "system" fn(*mut Client) -> u32,
    initialize: extern "system" fn(*mut Client, i32, u32, i64, i64, *const u8, *const Guid) -> HRESULT,
    buffer_size: extern "system" fn(*mut Client, *mut u32) -> HRESULT,
    stream_latency: extern "system" fn(*mut Client, *mut i64) -> HRESULT,
    current_padding: extern "system" fn(*mut Client, *mut u32) -> HRESULT,
    is_format_supported: extern "system" fn(*mut Client, i32, *const u8, *mut *mut u8) -> HRESULT,
    mix_format: extern "system" fn(*mut Client, *mut *mut u8) -> HRESULT,
    device_period: extern "system" fn(*mut Client, *mut i64, *mut i64) -> HRESULT,
    start: extern "system" fn(*mut Client) -> HRESULT,
    stop: extern "system" fn(*mut Client) -> HRESULT,
    reset: extern "system" fn(*mut Client) -> HRESULT,
    set_event_handle: extern "system" fn(*mut Client, usize) -> HRESULT,
    service: extern "system" fn(*mut Client, *const Guid, *mut *mut c_void) -> HRESULT,
    is_offload_capable: extern "system" fn(*mut Client, i32, *mut i32) -> HRESULT,
    set_client_properties: extern "system" fn(*mut Client, *const c_void) -> HRESULT,
    buffer_size_limits: extern "system" fn(*mut Client, *const u8, i32, *mut i64, *mut i64) -> HRESULT,
    shared_mode_engine_period: extern "system" fn(*mut Client, *const u8, *mut u32, *mut u32, *mut u32, *mut u32) -> HRESULT,
    current_shared_mode_engine_period: extern "system" fn(*mut Client, *mut *mut u8, *mut u32) -> HRESULT,
    initialize_shared_audio_stream: extern "system" fn(*mut Client, u32, u32, *const u8, *const Guid) -> HRESULT,
}

static CLIENT: ClientMethods = ClientMethods {
    query_interface: client_query,
    add_ref: add_ref::<Client>,
    release: release::<Client>,
    initialize,
    buffer_size,
    stream_latency,
    current_padding,
    is_format_supported,
    mix_format: client_mix_format,
    device_period,
    start,
    stop,
    reset,
    set_event_handle,
    service,
    is_offload_capable,
    set_client_properties,
    buffer_size_limits,
    shared_mode_engine_period,
    current_shared_mode_engine_period,
    initialize_shared_audio_stream,
};

/// Calls host method `n` of a client proxy over a host client.
macro_rules! forward {
    ($c:expr, $n:literal, fn($($t:ty),*) $(, $a:expr)*) => {{
        // SAFETY: slot `$n` of the interface level the host object has
        // (callers check the level for slots past `IAudioClient`'s).
        let f: extern "system" fn(*mut Host $(, $t)*) -> HRESULT = unsafe { Host::method($c.host, $n) };
        f($c.host $(, $a)*)
    }};
}

extern "system" fn client_query(this: *mut Client, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
    let c = client(this);
    let ours: &[Guid] = match c.level {
        3 => &[IID_CLIENT, IID_CLIENT2, IID_CLIENT3],
        2 => &[IID_CLIENT, IID_CLIENT2],
        _ => &[IID_CLIENT],
    };
    answer(this, c.host, ours, iid, out)
}

extern "system" fn initialize(
    this: *mut Client,
    mode: i32,
    flags: u32,
    duration: i64,
    period: i64,
    format: *const u8,
    session: *const Guid,
) -> HRESULT {
    let c = client(this);
    let Some(asked) = Format::read(format) else {
        return E_POINTER;
    };
    if c.host.is_null() {
        let mut s = lock(&c.silent);
        if s.format.is_some() {
            return ALREADY_INITIALIZED;
        }
        let duration = Duration::from_nanos(duration.max(200_000) as u64 * 100);
        s.frames = (duration.as_secs_f64() * f64::from(asked.rate)) as u32;
        s.scratch = vec![0; (s.frames * asked.block()) as usize];
        s.format = Some(asked);
        if once("silent init".into()) {
            tracing::info!(rate = asked.rate, channels = asked.channels, frames = s.frames, events = flags & EVENT_CALLBACK != 0, "audio output: the title's stream on the silent output");
        }
        return S_OK;
    }
    let flags = if mode == SHARED && asked != c.mix && convertible(&asked) { flags | AUTOCONVERT } else { flags };
    let hr = forward!(c, 3, fn(i32, u32, i64, i64, *const u8, *const Guid), mode, flags, duration, period, format, session);
    if once(format!("init {asked:?} {flags:#x} {hr}")) {
        tracing::info!(
            rate = asked.rate,
            channels = asked.channels,
            bits = asked.bits,
            float = asked.float,
            device_rate = c.mix.rate,
            flags = format_args!("{flags:#x}"),
            hr = format_args!("{hr:#010x}"),
            "audio output: the title's stream made"
        );
    }
    hr
}

extern "system" fn buffer_size(this: *mut Client, out: *mut u32) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 4, fn(*mut u32), out);
    }
    let s = lock(&c.silent);
    if s.format.is_none() {
        return NOT_INITIALIZED;
    }
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null.
    unsafe { out.write(s.frames) };
    S_OK
}

extern "system" fn stream_latency(this: *mut Client, out: *mut i64) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 5, fn(*mut i64), out);
    }
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null.
    unsafe { out.write(100_000) };
    S_OK
}

extern "system" fn current_padding(this: *mut Client, out: *mut u32) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 6, fn(*mut u32), out);
    }
    let mut s = lock(&c.silent);
    if s.format.is_none() {
        return NOT_INITIALIZED;
    }
    if out.is_null() {
        return E_POINTER;
    }
    let padding = s.padding();
    // SAFETY: checked non-null.
    unsafe { out.write(padding) };
    S_OK
}

extern "system" fn is_format_supported(this: *mut Client, mode: i32, format: *const u8, closest: *mut *mut u8) -> HRESULT {
    let c = client(this);
    let asked = Format::read(format);
    if mode == SHARED && asked.as_ref().is_some_and(convertible) {
        if !closest.is_null() {
            // SAFETY: checked non-null.
            unsafe { closest.write(std::ptr::null_mut()) };
        }
        return S_OK;
    }
    if c.host.is_null() {
        return 0x8889_0008_u32 as i32;
    }
    forward!(c, 7, fn(i32, *const u8, *mut *mut u8), mode, format, closest)
}

extern "system" fn client_mix_format(this: *mut Client, out: *mut *mut u8) -> HRESULT {
    let c = client(this);
    if out.is_null() {
        return E_POINTER;
    }
    let p = presented(c).allocate();
    // SAFETY: checked non-null.
    unsafe { out.write(p) };
    if p.is_null() { 0x8007_000E_u32 as i32 } else { S_OK }
}

extern "system" fn device_period(this: *mut Client, default: *mut i64, minimum: *mut i64) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 9, fn(*mut i64, *mut i64), default, minimum);
    }
    // SAFETY: written only where the title asked.
    unsafe {
        if !default.is_null() {
            default.write(100_000);
        }
        if !minimum.is_null() {
            minimum.write(30_000);
        }
    }
    S_OK
}

extern "system" fn start(this: *mut Client) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 10, fn());
    }
    let mut s = lock(&c.silent);
    if s.format.is_none() {
        return NOT_INITIALIZED;
    }
    if s.mark.is_none() {
        s.mark = Some(Instant::now());
    }
    if s.event != 0 && s.ticking.is_none() {
        let running = Arc::new(AtomicBool::new(true));
        s.ticking = Some(running.clone());
        let (event, period) = (s.event, s.period);
        std::thread::Builder::new()
            .name("victory-silent-audio".into())
            .spawn(move || {
                let set = host!("kernel32.dll", "SetEvent": extern "system" fn(usize) -> i32);
                while running.load(Ordering::Acquire) {
                    std::thread::sleep(period);
                    set(event);
                }
            })
            .ok();
    }
    S_OK
}

extern "system" fn stop(this: *mut Client) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 11, fn());
    }
    let mut s = lock(&c.silent);
    s.advance();
    s.mark = None;
    s.stop_ticking();
    S_OK
}

extern "system" fn reset(this: *mut Client) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 12, fn());
    }
    let mut s = lock(&c.silent);
    s.written = 0;
    s.played = 0;
    S_OK
}

extern "system" fn set_event_handle(this: *mut Client, event: usize) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 13, fn(usize), event);
    }
    lock(&c.silent).event = event;
    S_OK
}

extern "system" fn service(this: *mut Client, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() {
        return forward!(c, 14, fn(*const Guid, *mut *mut c_void), iid, out);
    }
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    if lock(&c.silent).format.is_none() {
        return NOT_INITIALIZED;
    }
    // SAFETY: checked non-null.
    let iid = unsafe { *iid };
    let object: *mut c_void = if iid == IID_RENDER {
        add_ref(this);
        Box::into_raw(Box::new(Service { vtable: &RENDER_METHODS as *const _ as *const c_void, client: this, refs: AtomicU32::new(1) })).cast()
    } else if iid == IID_CLOCK {
        add_ref(this);
        Box::into_raw(Box::new(Service { vtable: &CLOCK_METHODS as *const _ as *const c_void, client: this, refs: AtomicU32::new(1) })).cast()
    } else {
        if once(format!("silent service {}", show(&iid))) {
            tracing::info!(iid = show(&iid), "audio output: the silent output has no such service");
        }
        std::ptr::null_mut()
    };
    // SAFETY: checked non-null.
    unsafe { out.write(object) };
    if object.is_null() { E_NOINTERFACE } else { S_OK }
}

extern "system" fn is_offload_capable(this: *mut Client, category: i32, out: *mut i32) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() && c.level >= 2 {
        return forward!(c, 15, fn(i32, *mut i32), category, out);
    }
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null.
    unsafe { out.write(0) };
    S_OK
}

extern "system" fn set_client_properties(this: *mut Client, properties: *const c_void) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() && c.level >= 2 {
        return forward!(c, 16, fn(*const c_void), properties);
    }
    S_OK
}

extern "system" fn buffer_size_limits(this: *mut Client, format: *const u8, event: i32, min: *mut i64, max: *mut i64) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() && c.level >= 2 {
        return forward!(c, 17, fn(*const u8, i32, *mut i64, *mut i64), format, event, min, max);
    }
    E_NOTIMPL
}

/// In the title's format when it is the device's; else 10 ms at the
/// title's rate, as a shared stream we convert has.
extern "system" fn shared_mode_engine_period(
    this: *mut Client,
    format: *const u8,
    default: *mut u32,
    fundamental: *mut u32,
    min: *mut u32,
    max: *mut u32,
) -> HRESULT {
    let c = client(this);
    let asked = Format::read(format);
    if !c.host.is_null() && c.level >= 3 && asked == Some(c.mix) {
        return forward!(c, 18, fn(*const u8, *mut u32, *mut u32, *mut u32, *mut u32), format, default, fundamental, min, max);
    }
    let frames = asked.map_or(RATE, |f| f.rate) / 100;
    for p in [default, fundamental, min, max] {
        if !p.is_null() {
            // SAFETY: checked non-null.
            unsafe { p.write(frames) };
        }
    }
    S_OK
}

extern "system" fn current_shared_mode_engine_period(this: *mut Client, format: *mut *mut u8, period: *mut u32) -> HRESULT {
    let c = client(this);
    if !c.host.is_null() && c.level >= 3 && c.mix.rate == RATE {
        return forward!(c, 19, fn(*mut *mut u8, *mut u32), format, period);
    }
    // SAFETY: written only where the title asked.
    unsafe {
        if !format.is_null() {
            format.write(presented(c).allocate());
        }
        if !period.is_null() {
            period.write(RATE / 100);
        }
    }
    S_OK
}

/// A stream in the device's format is the host's own; in another format it
/// is made with `Initialize`, which converts (this call cannot).
extern "system" fn initialize_shared_audio_stream(this: *mut Client, flags: u32, period: u32, format: *const u8, session: *const Guid) -> HRESULT {
    let c = client(this);
    let Some(asked) = Format::read(format) else {
        return E_POINTER;
    };
    if !c.host.is_null() && c.level >= 3 && asked == c.mix {
        return forward!(c, 20, fn(u32, u32, *const u8, *const Guid), flags, period, format, session);
    }
    let duration = (i64::from(period.max(1)) * 3 * 10_000_000 / i64::from(asked.rate.max(1))).max(200_000);
    initialize(this, SHARED, flags, duration, 0, format, session)
}

// The silent output's render client and clock.

#[repr(C)]
struct Service {
    vtable: *const c_void,
    client: *mut Client,
    refs: AtomicU32,
}

impl Proxy for Service {
    fn refs(&self) -> &AtomicU32 {
        &self.refs
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        release(self.client);
    }
}

fn silent_of<'a>(this: *mut Service) -> MutexGuard<'a, Silent> {
    // SAFETY: a service of ours holds its client alive.
    let c = unsafe { &*(*this).client };
    lock(&c.silent)
}

#[repr(C)]
struct RenderMethods {
    query_interface: extern "system" fn(*mut Service, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: extern "system" fn(*mut Service) -> u32,
    release: extern "system" fn(*mut Service) -> u32,
    buffer: extern "system" fn(*mut Service, u32, *mut *mut u8) -> HRESULT,
    release_buffer: extern "system" fn(*mut Service, u32, u32) -> HRESULT,
}

static RENDER_METHODS: RenderMethods = RenderMethods {
    query_interface: render_query,
    add_ref: add_ref::<Service>,
    release: release::<Service>,
    buffer: render_buffer,
    release_buffer: render_release_buffer,
};

extern "system" fn render_buffer(this: *mut Service, frames: u32, out: *mut *mut u8) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let mut s = silent_of(this);
    if s.held.is_some() {
        return OUT_OF_ORDER;
    }
    if frames > s.frames - s.padding() {
        return BUFFER_TOO_LARGE;
    }
    s.held = Some(frames);
    // SAFETY: checked non-null; the scratch buffer has room for the
    // whole buffer's frames and lives as long as the client.
    unsafe { out.write(s.scratch.as_mut_ptr()) };
    S_OK
}

extern "system" fn render_release_buffer(this: *mut Service, frames: u32, _flags: u32) -> HRESULT {
    let mut s = silent_of(this);
    match s.held.take() {
        Some(held) if frames <= held => {
            s.written += u64::from(frames);
            S_OK
        }
        Some(_) => INVALID_SIZE,
        None => OUT_OF_ORDER,
    }
}

extern "system" fn render_query(this: *mut Service, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
    answer(this, std::ptr::null_mut(), &[IID_RENDER], iid, out)
}

#[repr(C)]
struct ClockMethods {
    query_interface: extern "system" fn(*mut Service, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: extern "system" fn(*mut Service) -> u32,
    release: extern "system" fn(*mut Service) -> u32,
    frequency: extern "system" fn(*mut Service, *mut u64) -> HRESULT,
    position: extern "system" fn(*mut Service, *mut u64, *mut u64) -> HRESULT,
    characteristics: extern "system" fn(*mut Service, *mut u32) -> HRESULT,
}

static CLOCK_METHODS: ClockMethods = ClockMethods {
    query_interface: clock_query,
    add_ref: add_ref::<Service>,
    release: release::<Service>,
    frequency: clock_frequency,
    position: clock_position,
    characteristics: clock_characteristics,
};

extern "system" fn clock_frequency(this: *mut Service, out: *mut u64) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let rate = silent_of(this).format.map_or(RATE, |f| f.rate);
    // SAFETY: checked non-null.
    unsafe { out.write(u64::from(rate)) };
    S_OK
}

extern "system" fn clock_position(this: *mut Service, out: *mut u64, qpc: *mut u64) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let mut s = silent_of(this);
    s.advance();
    // SAFETY: checked non-null.
    unsafe { out.write(s.played) };
    if !qpc.is_null() {
        let (mut count, mut frequency) = (0i64, 1i64);
        host!("kernel32.dll", "QueryPerformanceCounter": extern "system" fn(*mut i64) -> i32)(&mut count);
        host!("kernel32.dll", "QueryPerformanceFrequency": extern "system" fn(*mut i64) -> i32)(&mut frequency);
        // SAFETY: checked non-null.
        unsafe { qpc.write((i128::from(count) * 10_000_000 / i128::from(frequency.max(1))) as u64) };
    }
    S_OK
}

extern "system" fn clock_characteristics(_this: *mut Service, out: *mut u32) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null.
    unsafe { out.write(0) };
    S_OK
}

extern "system" fn clock_query(this: *mut Service, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
    answer(this, std::ptr::null_mut(), &[IID_CLOCK], iid, out)
}

/// The silent output's spatial audio (`ISpatialAudioClient`), which the
/// title's sound engine plays through: a stereo device without spatial
/// sound (front left and right as its own static objects, no dynamic ones),
/// that takes any objects the title asks for, signals its event every
/// 10 ms and plays nothing.
mod spatial {
    use super::*;

    const IID_FORMATS: Guid = guid(0xDCDA_A858, 0x895A, 0x4A22, [0xA5, 0xEB, 0x67, 0xBD, 0xA5, 0x06, 0x09, 0x6D]);
    const IID_STREAM_BASE: Guid = guid(0xFEAA_F403, 0xC1D8, 0x450D, [0xAA, 0x05, 0xE0, 0xCC, 0xEE, 0x75, 0x02, 0xA8]);
    const IID_STREAM: Guid = guid(0xBAB5_F473, 0xB423, 0x477B, [0x85, 0xF5, 0xB5, 0xA3, 0x32, 0xA0, 0x41, 0x53]);
    const IID_OBJECT_BASE: Guid = guid(0xCCE0_B8F2, 0x8D4D, 0x4EFB, [0xA8, 0xCF, 0x3D, 0x6E, 0xCF, 0x1C, 0x30, 0xE0]);
    const IID_OBJECT: Guid = guid(0xDDE2_8967, 0x521B, 0x46E5, [0x8F, 0x00, 0xBD, 0x6F, 0x2B, 0xC8, 0xAB, 0x1D]);
    const UNSUPPORTED_FORMAT: HRESULT = 0x8889_0008_u32 as i32;
    const E_INVALIDARG: HRESULT = 0x8007_0057_u32 as i32;
    /// `SPTLAUDCLNT_E_NO_MORE_OBJECTS`.
    const NO_MORE_OBJECTS: HRESULT = 0x8889_0203_u32 as i32;
    /// `AudioObjectType_FrontLeft | AudioObjectType_FrontRight`.
    const STEREO: u32 = 0x6;
    const DYNAMIC: u32 = 0x1;
    /// Frames of one pass: 10 ms.
    const FRAMES: u32 = RATE / 100;

    /// The objects' format: 48 kHz mono float.
    fn object_format() -> Format {
        Format::console(1, 0x4)
    }

    fn format_bytes(f: &Format) -> Box<[u8; 40]> {
        let p = f.allocate();
        let mut bytes = Box::new([0u8; 40]);
        if !p.is_null() {
            // SAFETY: `allocate` wrote 40 bytes there.
            unsafe { std::ptr::copy_nonoverlapping(p, bytes.as_mut_ptr(), 40) };
            free(p.cast());
        }
        bytes
    }

    #[repr(C)]
    struct Object<M: 'static> {
        vtable: &'static M,
        refs: AtomicU32,
    }

    impl<M: 'static> Proxy for Object<M> {
        fn refs(&self) -> &AtomicU32 {
            &self.refs
        }
    }

    // The client.

    #[repr(C)]
    struct ClientMethods {
        query_interface: extern "system" fn(*mut Object<ClientMethods>, *const Guid, *mut *mut c_void) -> HRESULT,
        add_ref: extern "system" fn(*mut Object<ClientMethods>) -> u32,
        release: extern "system" fn(*mut Object<ClientMethods>) -> u32,
        static_object_position: extern "system" fn(*mut c_void, u32, *mut f32, *mut f32, *mut f32) -> HRESULT,
        native_static_object_type_mask: extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
        max_dynamic_object_count: extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
        supported_formats: extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
        max_frame_count: extern "system" fn(*mut c_void, *const u8, *mut u32) -> HRESULT,
        is_format_supported: extern "system" fn(*mut c_void, *const u8) -> HRESULT,
        is_stream_available: extern "system" fn(*mut c_void, *const Guid, *const c_void) -> HRESULT,
        activate_stream: extern "system" fn(*mut c_void, *const [u64; 3], *const Guid, *mut *mut c_void) -> HRESULT,
        is_offload_capable: extern "system" fn(*mut c_void, u32, *mut i32) -> HRESULT,
        max_frame_count_for_category: extern "system" fn(*mut c_void, u32, i32, *const u8, *mut u32) -> HRESULT,
    }

    static CLIENT: ClientMethods = ClientMethods {
        query_interface: client_query,
        add_ref: add_ref::<Object<ClientMethods>>,
        release: release::<Object<ClientMethods>>,
        static_object_position,
        native_static_object_type_mask,
        max_dynamic_object_count,
        supported_formats,
        max_frame_count,
        is_format_supported,
        is_stream_available,
        activate_stream,
        is_offload_capable,
        max_frame_count_for_category,
    };

    /// A new client, as the `ISpatialAudioClient` pointer the title gets.
    pub(super) fn client() -> *mut c_void {
        said("the silent output's spatial audio".into());
        Box::into_raw(Box::new(Object { vtable: &CLIENT, refs: AtomicU32::new(1) })).cast()
    }

    extern "system" fn client_query(this: *mut Object<ClientMethods>, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        answer(this, std::ptr::null_mut(), &[IID_SPATIAL, IID_SPATIAL2], iid, out)
    }

    fn write<T>(p: *mut T, value: T) -> HRESULT {
        if p.is_null() {
            return E_POINTER;
        }
        // SAFETY: checked non-null; the title's out parameter.
        unsafe { p.write(value) };
        S_OK
    }

    extern "system" fn static_object_position(_: *mut c_void, kind: u32, x: *mut f32, y: *mut f32, z: *mut f32) -> HRESULT {
        // Unit vectors as Windows gives them: right, up, back are positive.
        let (px, py, pz) = match kind {
            0x2 => (-0.707, 0.0, -0.707),
            0x4 => (0.707, 0.0, -0.707),
            0x8 => (0.0, 0.0, -1.0),
            0x20 => (-1.0, 0.0, 0.0),
            0x40 => (1.0, 0.0, 0.0),
            0x80 => (-0.707, 0.0, 0.707),
            0x100 => (0.707, 0.0, 0.707),
            0x20000 => (0.0, 0.0, 1.0),
            _ => (0.0, 0.0, 0.0),
        };
        if x.is_null() || y.is_null() || z.is_null() {
            return E_POINTER;
        }
        write(x, px);
        write(y, py);
        write(z, pz)
    }

    extern "system" fn native_static_object_type_mask(_: *mut c_void, out: *mut u32) -> HRESULT {
        write(out, STEREO)
    }

    extern "system" fn max_dynamic_object_count(_: *mut c_void, out: *mut u32) -> HRESULT {
        write(out, 0)
    }

    extern "system" fn max_frame_count(_: *mut c_void, _format: *const u8, out: *mut u32) -> HRESULT {
        write(out, FRAMES)
    }

    extern "system" fn max_frame_count_for_category(_: *mut c_void, _category: u32, _offload: i32, _format: *const u8, out: *mut u32) -> HRESULT {
        write(out, FRAMES)
    }

    extern "system" fn is_offload_capable(_: *mut c_void, _category: u32, out: *mut i32) -> HRESULT {
        write(out, 0)
    }

    fn mono_float(format: *const u8) -> Option<Format> {
        Format::read(format).filter(|f| f.channels == 1 && f.float && f.bits == 32)
    }

    extern "system" fn is_format_supported(_: *mut c_void, format: *const u8) -> HRESULT {
        if mono_float(format).is_some() { S_OK } else { UNSUPPORTED_FORMAT }
    }

    extern "system" fn is_stream_available(_: *mut c_void, iid: *const Guid, _info: *const c_void) -> HRESULT {
        if iid.is_null() {
            return E_POINTER;
        }
        // SAFETY: checked non-null.
        let iid = unsafe { *iid };
        if iid == IID_STREAM { S_OK } else { E_NOINTERFACE }
    }

    // The format list: the one object format.

    #[repr(C)]
    struct FormatsMethods {
        query_interface: extern "system" fn(*mut Formats, *const Guid, *mut *mut c_void) -> HRESULT,
        add_ref: extern "system" fn(*mut Formats) -> u32,
        release: extern "system" fn(*mut Formats) -> u32,
        count: extern "system" fn(*mut Formats, *mut u32) -> HRESULT,
        format: extern "system" fn(*mut Formats, u32, *mut *const u8) -> HRESULT,
    }

    #[repr(C)]
    struct Formats {
        vtable: &'static FormatsMethods,
        refs: AtomicU32,
        format: Box<[u8; 40]>,
    }

    impl Proxy for Formats {
        fn refs(&self) -> &AtomicU32 {
            &self.refs
        }
    }

    static FORMATS: FormatsMethods = FormatsMethods {
        query_interface: formats_query,
        add_ref: add_ref::<Formats>,
        release: release::<Formats>,
        count: formats_count,
        format: formats_format,
    };

    extern "system" fn formats_query(this: *mut Formats, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        answer(this, std::ptr::null_mut(), &[IID_FORMATS], iid, out)
    }

    extern "system" fn formats_count(_: *mut Formats, out: *mut u32) -> HRESULT {
        write(out, 1)
    }

    /// The format stays the list's: the caller does not free it.
    extern "system" fn formats_format(this: *mut Formats, index: u32, out: *mut *const u8) -> HRESULT {
        if index != 0 {
            return E_INVALIDARG;
        }
        // SAFETY: a list of ours, alive while the title holds it.
        let p = unsafe { (*this).format.as_ptr() };
        write(out, p)
    }

    extern "system" fn supported_formats(_: *mut c_void, out: *mut *mut c_void) -> HRESULT {
        let list = Box::into_raw(Box::new(Formats { vtable: &FORMATS, refs: AtomicU32::new(1), format: format_bytes(&object_format()) }));
        write(out, list.cast())
    }

    // The stream.

    #[repr(C)]
    struct StreamMethods {
        query_interface: extern "system" fn(*mut Stream, *const Guid, *mut *mut c_void) -> HRESULT,
        add_ref: extern "system" fn(*mut Stream) -> u32,
        release: extern "system" fn(*mut Stream) -> u32,
        available_dynamic_object_count: extern "system" fn(*mut Stream, *mut u32) -> HRESULT,
        service: extern "system" fn(*mut Stream, *const Guid, *mut *mut c_void) -> HRESULT,
        start: extern "system" fn(*mut Stream) -> HRESULT,
        stop: extern "system" fn(*mut Stream) -> HRESULT,
        reset: extern "system" fn(*mut Stream) -> HRESULT,
        begin_updating: extern "system" fn(*mut Stream, *mut u32, *mut u32) -> HRESULT,
        end_updating: extern "system" fn(*mut Stream) -> HRESULT,
        activate_object: extern "system" fn(*mut Stream, u32, *mut *mut c_void) -> HRESULT,
    }

    #[repr(C)]
    struct Stream {
        vtable: &'static StreamMethods,
        refs: AtomicU32,
        format: Format,
        event: usize,
        max_dynamic: u32,
        /// Dynamic objects active now, shared with the objects.
        dynamic: Arc<AtomicU32>,
        ticking: Mutex<Option<Arc<AtomicBool>>>,
    }

    impl Proxy for Stream {
        fn refs(&self) -> &AtomicU32 {
            &self.refs
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            if let Some(t) = lock(&self.ticking).take() {
                t.store(false, Ordering::Release);
            }
        }
    }

    static STREAM: StreamMethods = StreamMethods {
        query_interface: stream_query,
        add_ref: add_ref::<Stream>,
        release: release::<Stream>,
        available_dynamic_object_count: stream_available,
        service: stream_service,
        start: stream_start,
        stop: stream_stop,
        reset: stream_reset,
        begin_updating: stream_begin,
        end_updating: stream_end,
        activate_object: stream_activate_object,
    };

    /// `SpatialAudioObjectRenderStreamActivationParams` from a VT_BLOB
    /// PROPVARIANT: the stream, or why not.
    extern "system" fn activate_stream(_: *mut c_void, params: *const [u64; 3], iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        if params.is_null() || iid.is_null() || out.is_null() {
            return E_POINTER;
        }
        // SAFETY: checked non-null.
        let (variant, iid) = unsafe { (*params, *iid) };
        if iid != IID_STREAM && iid != IID_STREAM_BASE {
            if once(format!("spatial stream {}", show(&iid))) {
                tracing::info!(iid = show(&iid), "audio output: the silent output has no such spatial stream");
            }
            return E_NOINTERFACE;
        }
        // VT_BLOB (65): size at 8, data at 16.
        let (vt, size, data) = (variant[0] & 0xFFFF, variant[1] as u32, variant[2] as *const u8);
        if vt != 65 || size < 40 || data.is_null() {
            return E_INVALIDARG;
        }
        // SAFETY: the activation parameters the blob holds: format pointer,
        // static mask, min and max dynamic counts, category, event, notify.
        let (format, mask, min, max, event) = unsafe {
            (
                data.cast::<*const u8>().read_unaligned(),
                data.add(8).cast::<u32>().read_unaligned(),
                data.add(12).cast::<u32>().read_unaligned(),
                data.add(16).cast::<u32>().read_unaligned(),
                data.add(24).cast::<usize>().read_unaligned(),
            )
        };
        let Some(format) = mono_float(format) else {
            return UNSUPPORTED_FORMAT;
        };
        if once("spatial stream".into()) {
            tracing::info!(rate = format.rate, static_mask = format_args!("{mask:#x}"), min_dynamic = min, max_dynamic = max, events = event != 0, "audio output: the title's spatial stream on the silent output");
        }
        let stream = Box::into_raw(Box::new(Stream {
            vtable: &STREAM,
            refs: AtomicU32::new(1),
            format,
            event,
            max_dynamic: max.max(min),
            dynamic: Arc::new(AtomicU32::new(0)),
            ticking: Mutex::new(None),
        }));
        write(out, stream.cast())
    }

    fn stream<'a>(this: *mut Stream) -> &'a Stream {
        // SAFETY: a stream of ours, alive while the title calls it.
        unsafe { &*this }
    }

    extern "system" fn stream_query(this: *mut Stream, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        answer(this, std::ptr::null_mut(), &[IID_STREAM_BASE, IID_STREAM], iid, out)
    }

    extern "system" fn stream_available(this: *mut Stream, out: *mut u32) -> HRESULT {
        let s = stream(this);
        write(out, s.max_dynamic.saturating_sub(s.dynamic.load(Ordering::Acquire)))
    }

    extern "system" fn stream_service(_: *mut Stream, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        if !iid.is_null() && once("spatial service".into()) {
            // SAFETY: checked non-null.
            tracing::info!(iid = show(unsafe { &*iid }), "audio output: the silent spatial stream has no services");
        }
        write(out, std::ptr::null_mut());
        E_NOINTERFACE
    }

    extern "system" fn stream_start(this: *mut Stream) -> HRESULT {
        let s = stream(this);
        let mut ticking = lock(&s.ticking);
        if ticking.is_none() && s.event != 0 {
            let running = Arc::new(AtomicBool::new(true));
            *ticking = Some(running.clone());
            let event = s.event;
            std::thread::Builder::new()
                .name("victory-silent-spatial".into())
                .spawn(move || {
                    let set = host!("kernel32.dll", "SetEvent": extern "system" fn(usize) -> i32);
                    let start = Instant::now();
                    let mut n = 0u32;
                    while running.load(Ordering::Acquire) {
                        n += 1;
                        let due = start + Duration::from_millis(10) * n;
                        if let Some(wait) = due.checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                        set(event);
                    }
                })
                .ok();
        }
        S_OK
    }

    extern "system" fn stream_stop(this: *mut Stream) -> HRESULT {
        if let Some(t) = lock(&stream(this).ticking).take() {
            t.store(false, Ordering::Release);
        }
        S_OK
    }

    extern "system" fn stream_reset(this: *mut Stream) -> HRESULT {
        stream_stop(this)
    }

    extern "system" fn stream_begin(this: *mut Stream, available: *mut u32, frames: *mut u32) -> HRESULT {
        let s = stream(this);
        let free = s.max_dynamic.saturating_sub(s.dynamic.load(Ordering::Acquire));
        if !available.is_null() {
            write(available, free);
        }
        write(frames, FRAMES)
    }

    extern "system" fn stream_end(_: *mut Stream) -> HRESULT {
        S_OK
    }

    extern "system" fn stream_activate_object(this: *mut Stream, kind: u32, out: *mut *mut c_void) -> HRESULT {
        let s = stream(this);
        if kind == DYNAMIC {
            let taken = s.dynamic.fetch_add(1, Ordering::AcqRel);
            if taken >= s.max_dynamic {
                s.dynamic.fetch_sub(1, Ordering::AcqRel);
                return NO_MORE_OBJECTS;
            }
        }
        let object = Box::into_raw(Box::new(AudioObject {
            vtable: &OBJECT,
            refs: AtomicU32::new(1),
            kind,
            buffer: Mutex::new(vec![0; (FRAMES * s.format.block()) as usize]),
            active: AtomicBool::new(true),
            dynamic: (kind == DYNAMIC).then(|| s.dynamic.clone()),
        }));
        write(out, object.cast())
    }

    // An audio object: a buffer for one pass, played into nothing.

    #[repr(C)]
    struct ObjectMethods {
        query_interface: extern "system" fn(*mut AudioObject, *const Guid, *mut *mut c_void) -> HRESULT,
        add_ref: extern "system" fn(*mut AudioObject) -> u32,
        release: extern "system" fn(*mut AudioObject) -> u32,
        buffer: extern "system" fn(*mut AudioObject, *mut *mut u8, *mut u32) -> HRESULT,
        end_of_stream: extern "system" fn(*mut AudioObject, u32) -> HRESULT,
        is_active: extern "system" fn(*mut AudioObject, *mut i32) -> HRESULT,
        kind: extern "system" fn(*mut AudioObject, *mut u32) -> HRESULT,
        position: extern "system" fn(*mut AudioObject, f32, f32, f32) -> HRESULT,
        volume: extern "system" fn(*mut AudioObject, f32) -> HRESULT,
    }

    #[repr(C)]
    struct AudioObject {
        vtable: &'static ObjectMethods,
        refs: AtomicU32,
        kind: u32,
        buffer: Mutex<Vec<u8>>,
        active: AtomicBool,
        dynamic: Option<Arc<AtomicU32>>,
    }

    impl AudioObject {
        fn finish(&self) {
            if self.active.swap(false, Ordering::AcqRel)
                && let Some(d) = &self.dynamic
            {
                d.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    impl Proxy for AudioObject {
        fn refs(&self) -> &AtomicU32 {
            &self.refs
        }
    }

    impl Drop for AudioObject {
        fn drop(&mut self) {
            self.finish();
        }
    }

    static OBJECT: ObjectMethods = ObjectMethods {
        query_interface: object_query,
        add_ref: add_ref::<AudioObject>,
        release: release::<AudioObject>,
        buffer: object_buffer,
        end_of_stream: object_end,
        is_active: object_is_active,
        kind: object_kind,
        position: object_position,
        volume: object_volume,
    };

    fn object<'a>(this: *mut AudioObject) -> &'a AudioObject {
        // SAFETY: an object of ours, alive while the title calls it.
        unsafe { &*this }
    }

    extern "system" fn object_query(this: *mut AudioObject, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        answer(this, std::ptr::null_mut(), &[IID_OBJECT_BASE, IID_OBJECT], iid, out)
    }

    /// The buffer stays the object's; the title fills it for this pass.
    extern "system" fn object_buffer(this: *mut AudioObject, out: *mut *mut u8, bytes: *mut u32) -> HRESULT {
        let o = object(this);
        let mut buffer = lock(&o.buffer);
        if !bytes.is_null() {
            write(bytes, buffer.len() as u32);
        }
        write(out, buffer.as_mut_ptr())
    }

    extern "system" fn object_end(this: *mut AudioObject, _frames: u32) -> HRESULT {
        object(this).finish();
        S_OK
    }

    extern "system" fn object_is_active(this: *mut AudioObject, out: *mut i32) -> HRESULT {
        write(out, i32::from(object(this).active.load(Ordering::Acquire)))
    }

    extern "system" fn object_kind(this: *mut AudioObject, out: *mut u32) -> HRESULT {
        write(out, object(this).kind)
    }

    extern "system" fn object_position(_: *mut AudioObject, _x: f32, _y: f32, _z: f32) -> HRESULT {
        S_OK
    }

    extern "system" fn object_volume(_: *mut AudioObject, _volume: f32) -> HRESULT {
        S_OK
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_stream_takes_objects_and_ticks() {
            let client = client();
            let format = format_bytes(&object_format());
            let mut blob = [0u8; 48];
            blob[0..8].copy_from_slice(&(format.as_ptr() as u64).to_le_bytes());
            blob[8..12].copy_from_slice(&0x3F_u32.to_le_bytes());
            blob[16..20].copy_from_slice(&4_u32.to_le_bytes());
            let variant = [65_u64, 48, blob.as_ptr() as u64];
            let mut out = std::ptr::null_mut();
            assert_eq!(activate_stream(client, &variant, &IID_STREAM, &mut out), S_OK);
            let stream = out.cast::<Stream>();
            let (mut available, mut frames) = (0, 0);
            assert_eq!(stream_begin(stream, &mut available, &mut frames), S_OK);
            assert_eq!((available, frames), (4, 480));
            let mut object = std::ptr::null_mut();
            assert_eq!(stream_activate_object(stream, 0x2, &mut object), S_OK);
            let object = object.cast::<AudioObject>();
            let (mut data, mut bytes) = (std::ptr::null_mut(), 0);
            assert_eq!(object_buffer(object, &mut data, &mut bytes), S_OK);
            assert_eq!(bytes, 480 * 4);
            let mut dynamic = Vec::new();
            for _ in 0..4 {
                let mut d = std::ptr::null_mut();
                assert_eq!(stream_activate_object(stream, DYNAMIC, &mut d), S_OK);
                dynamic.push(d.cast::<AudioObject>());
            }
            let mut d = std::ptr::null_mut();
            assert_eq!(stream_activate_object(stream, DYNAMIC, &mut d), NO_MORE_OBJECTS);
            object_end(dynamic[0], 0);
            assert_eq!(stream_activate_object(stream, DYNAMIC, &mut d), S_OK);
            release(d.cast::<AudioObject>());
            for d in dynamic {
                release(d);
            }
            release(object);
            release(stream);
            release(client.cast::<Object<ClientMethods>>());
        }
    }
}

// The entry point.

/// The host's `CoCreateInstance`; the audio device enumerator comes back as
/// our proxy.
pub extern "system" fn CoCreateInstance(
    clsid: *const Guid,
    outer: *mut c_void,
    context: u32,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    let create = host!("combase.dll", "CoCreateInstance": extern "system" fn(*const Guid, *mut c_void, u32, *const Guid, *mut *mut c_void) -> HRESULT);
    let hr = create(clsid, outer, context, iid, out);
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return hr;
    }
    // SAFETY: checked non-null.
    let (class, interface) = unsafe { (*clsid, *iid) };
    if once(format!("class {}", show(&class))) {
        tracing::info!(class = show(&class), interface = show(&interface), hr = format_args!("{hr:#010x}"), "COM object made for the title");
    }
    if hr < 0 || !on() || class != CLSID_ENUMERATOR || !outer.is_null() || (interface != IID_ENUMERATOR && interface != IID_IUNKNOWN) {
        return hr;
    }
    // SAFETY: the host just wrote a live object here.
    let object = unsafe { out.read() }.cast::<Host>();
    let Some(host) = Host::query(object, &IID_ENUMERATOR) else {
        return hr;
    };
    Host::release(object);
    let proxy = Box::into_raw(Box::new(Enumerator { vtable: &ENUMERATOR, host, refs: AtomicU32::new(1) }));
    // SAFETY: checked non-null.
    unsafe { out.write(proxy.cast()) };
    hr
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "audio outputs",
        dlls: &["api-ms-win-core-com-*", "combase.dll", "ole32.dll"],
        exports: vec![hle_export!(CoCreateInstance)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_format_reads_back() {
        let f = Format::console(2, 0x3);
        let p = f.allocate();
        assert!(!p.is_null());
        assert_eq!(Format::read(p), Some(f));
        free(p.cast());
    }

    #[test]
    fn silent_output_plays_at_its_rate() {
        let client = Client::silent();
        let format = Format::console(2, 0x3).allocate();
        assert_eq!(initialize(client, SHARED, 0, 1_000_000, 0, format, std::ptr::null()), S_OK);
        free(format.cast());
        let mut frames = 0;
        assert_eq!(buffer_size(client, &mut frames), S_OK);
        assert_eq!(frames, 4800);
        let mut render = std::ptr::null_mut();
        assert_eq!(service(client, &IID_RENDER, &mut render), S_OK);
        let render = render.cast::<Service>();
        let mut data = std::ptr::null_mut();
        assert_eq!((RENDER_METHODS.buffer)(render, 4800, &mut data), S_OK);
        assert_eq!((RENDER_METHODS.release_buffer)(render, 4800, 0), S_OK);
        assert_eq!((RENDER_METHODS.buffer)(render, 1, &mut data), BUFFER_TOO_LARGE);
        assert_eq!(start(client), S_OK);
        std::thread::sleep(Duration::from_millis(50));
        let mut padding = 0;
        assert_eq!(current_padding(client, &mut padding), S_OK);
        assert!(padding < 4800 - 1000, "padding {padding}");
        release(render);
        release(client);
    }
}
