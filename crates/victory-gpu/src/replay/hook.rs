//! Methods of single COM objects replaced: an object's method table pointer
//! is set to a copy of its table ([`Shim`]) with some entries changed. Other
//! objects of the class keep theirs, so what is not asked for is untouched.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Methods in a copied table: more than any D3D12 interface has (an
/// interface's private methods may follow its public ones).
pub(super) const SLOTS: usize = 512;
/// Entries copied from before the table (type information).
const PRE: usize = 4;

/// A copied method table. Objects point at `table`; what we keep for them
/// lies before it. Never freed: an object may still use it while it dies.
#[repr(C)]
struct Shim {
    state: *const c_void,
    orig: *const usize,
    pre: [usize; PRE],
    table: [usize; SLOTS],
}

const TO_ORIG: usize = std::mem::offset_of!(Shim, table) - std::mem::offset_of!(Shim, orig);

/// The shims made, by the address objects point at.
static MADE: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());

unsafe fn shim(this: *mut c_void) -> *const Shim {
    // SAFETY: the caller's object points into a shim.
    unsafe {
        let table = *this.cast::<*const u8>();
        table.sub(std::mem::offset_of!(Shim, table)).cast()
    }
}

/// The object's own method table, as `T` (an interface's `_Vtbl`).
///
/// # Safety
/// `this` is an object [`install`] was called for, of an interface `T` is
/// the table of.
pub(super) unsafe fn orig<T>(this: *mut c_void) -> &'static T {
    // SAFETY: the caller's.
    unsafe { &*(*shim(this)).orig.cast::<T>() }
}

/// What [`install`] was given for the object.
///
/// # Safety
/// As [`orig`]; `T` is what `state` pointed at.
pub(super) unsafe fn state<T>(this: *mut c_void) -> &'static T {
    // SAFETY: the caller's.
    unsafe { &*(*shim(this)).state.cast::<T>() }
}

/// Bytes readable from `at` on (up to a limit): what is copied of a table
/// must not run past its module's data.
fn readable(at: usize, most: usize) -> usize {
    use windows_sys::Win32::System::Memory::*;
    let mut end = at;
    while end < at + most {
        // SAFETY: a plain query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        let n = unsafe {
            VirtualQuery(
                end as *const c_void,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        const READ: u32 = PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY;
        if n == 0
            || info.State != MEM_COMMIT
            || info.Protect & READ == 0
            || info.Protect & PAGE_GUARD != 0
        {
            break;
        }
        end = info.BaseAddress as usize + info.RegionSize;
    }
    end.min(at + most) - at
}

/// Gives the object a table of its own, changed by `patch`. `state` is what
/// [`state`] answers; it must live for good. False if the object has one
/// already (then nothing changes).
///
/// # Safety
/// `this` is a live COM object no other thread uses yet; what `patch` puts
/// into the table are functions of the methods' types.
pub(super) unsafe fn install(
    this: *mut c_void,
    state: *const c_void,
    patch: impl FnOnce(&mut [usize; SLOTS]),
) -> bool {
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: a live COM object starts with its table's address.
    let at = unsafe { *this.cast::<usize>() };
    if made.contains(&at) {
        return false;
    }
    let mut new = Box::new(Shim {
        state,
        orig: at as *const usize,
        pre: [0; PRE],
        table: [0; SLOTS],
    });
    let have = readable(at, SLOTS * 8) / 8;
    // SAFETY: `have` entries are readable; the object's table is at least
    // as long as its interface, and what lies behind it is only copied.
    unsafe { std::ptr::copy_nonoverlapping(at as *const usize, new.table.as_mut_ptr(), have) };
    if readable(at - PRE * 8, PRE * 8) == PRE * 8 {
        // SAFETY: as above.
        unsafe {
            std::ptr::copy_nonoverlapping((at as *const usize).sub(PRE), new.pre.as_mut_ptr(), PRE)
        };
    }
    patch(&mut new.table);
    let new: &'static Shim = Box::leak(new);
    made.insert(new.table.as_ptr() as usize);
    // SAFETY: the object's first word, its table's address.
    unsafe { this.cast::<*const usize>().write(new.table.as_ptr()) };
    true
}

/// The table the object points at.
///
/// # Safety
/// `this` is a live COM object.
pub(super) unsafe fn table(this: *mut c_void) -> usize {
    // SAFETY: the caller's.
    unsafe { *this.cast::<usize>() }
}

/// Whether the object points at a shim of ours.
///
/// # Safety
/// `this` is a live COM object.
pub(super) unsafe fn ours(this: *mut c_void) -> bool {
    // SAFETY: the caller's.
    let at = unsafe { table(this) };
    MADE.lock().unwrap_or_else(|e| e.into_inner()).contains(&at)
}

/// After a method that may have set the object's table pointer anew: it
/// points at its shim (`before`, where it pointed) again. A table it did
/// not have before gets a shim of its own.
///
/// # Safety
/// As [`install`]; `before` is a shim's table.
pub(super) unsafe fn again(
    this: *mut c_void,
    before: usize,
    patch: impl FnOnce(&mut [usize; SLOTS]),
) {
    // SAFETY: the caller's.
    unsafe {
        let now = table(this);
        if now == before {
            return;
        }
        let shim = (before as *const u8)
            .sub(std::mem::offset_of!(Shim, table))
            .cast::<Shim>();
        if (*shim).orig as usize == now {
            this.cast::<usize>().write(before);
        } else {
            install(this, (*shim).state, patch);
        }
    }
}

/// Gives the object its own table back, if it points at a shim of ours
/// (tests).
///
/// # Safety
/// `this` is a live object, used by no other thread now.
pub(super) unsafe fn remove(this: *mut c_void) {
    let made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: as in `install`.
    unsafe {
        let at = *this.cast::<usize>();
        if made.contains(&at) {
            let own = (*shim(this)).orig;
            this.cast::<*const usize>().write(own);
        }
    }
}

/// Classes of [`uncounted`] calls.
pub(super) const LIST: usize = 0;
pub(super) const DEVICE: usize = 1;
pub(super) const QUEUE: usize = 2;
const CLASSES: usize = 3;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);
static COUNTS: [AtomicU64; CLASSES * SLOTS] = [ZERO; CLASSES * SLOTS];

/// How often a method the recorder does not record was called.
pub(super) fn count(class: usize, slot: usize) -> u64 {
    COUNTS[class * SLOTS + slot].load(Ordering::Relaxed)
}

/// A method the recorder does not know how to record: counts the call in
/// [`COUNTS`] and goes on to the object's own. Any arguments pass as they
/// are: only the scratch register `rax` is used.
#[unsafe(naked)]
unsafe extern "system" fn noted<const AT: usize>() {
    core::arch::naked_asm!(
        "lock inc qword ptr [rip + {counts} + {at} * 8]",
        "mov rax, [rcx]",
        "mov rax, [rax - {orig}]",
        "jmp qword ptr [rax + ({at} % {slots}) * 8]",
        counts = sym COUNTS,
        at = const AT,
        orig = const TO_ORIG,
        slots = const SLOTS,
    )
}

/// [`noted`] for method `SLOT` of a class, `AT` being `class * SLOTS + slot`.
pub(super) fn noted_entry<const AT: usize>() -> usize {
    noted::<AT> as *const () as usize
}

/// The slot of a method in its table.
macro_rules! method {
    ($vtbl:ident . $name:ident) => {
        std::mem::offset_of!(windows::Win32::Graphics::Direct3D12::$vtbl, $name) / 8
    };
}
pub(super) use method;

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct Vtbl {
        methods: [usize; 8],
    }

    #[repr(C)]
    struct Object {
        vtbl: *const Vtbl,
        value: u64,
    }

    unsafe extern "system" fn add(
        this: *mut c_void,
        a: u64,
        b: u64,
        c: u64,
        d: u64,
        e: f64,
    ) -> u64 {
        // SAFETY: one of the test's objects.
        unsafe { (*this.cast::<Object>()).value + a + b + c + d + e as u64 }
    }

    type Add = unsafe extern "system" fn(*mut c_void, u64, u64, u64, u64, f64) -> u64;

    unsafe extern "system" fn double(
        this: *mut c_void,
        a: u64,
        b: u64,
        c: u64,
        d: u64,
        e: f64,
    ) -> u64 {
        // SAFETY: an object with a shim whose first method is `add`.
        unsafe {
            let first = orig::<[Add; 1]>(this)[0];
            first(this, a, b, c, d, e) * 2 + *state::<u64>(this)
        }
    }

    unsafe fn call(object: &mut Object, slot: usize) -> u64 {
        // SAFETY: an entry of `Add`'s type.
        unsafe {
            let f: Add = std::mem::transmute((*object.vtbl).methods[slot]);
            f((&raw mut *object).cast(), 1, 2, 3, 4, 5.0)
        }
    }

    #[test]
    fn one_object_is_changed_and_changed_back() {
        static STATE: u64 = 1000;
        let vtbl = Vtbl {
            methods: [add as *const () as usize; 8],
        };
        let mut a = Object {
            vtbl: &vtbl,
            value: 100,
        };
        let mut b = Object {
            vtbl: &vtbl,
            value: 200,
        };
        // SAFETY: the test's objects; entries of `Add`'s type.
        unsafe {
            let this: *mut c_void = (&raw mut a).cast();
            let patch = |t: &mut [usize; SLOTS]| {
                t[0] = double as *const () as usize;
                t[2] = noted_entry::<{ QUEUE * SLOTS + 2 }>();
            };
            assert!(install(this, (&raw const STATE).cast(), patch));
            assert!(
                !install(this, (&raw const STATE).cast(), patch),
                "only once"
            );
            assert_eq!(call(&mut a, 0), 230 + 1000);
            assert_eq!(call(&mut a, 1), 115);
            assert_eq!(call(&mut b, 0), 215, "other objects are untouched");
            // Counted, and passed on with all its arguments.
            let before = count(QUEUE, 2);
            assert_eq!(call(&mut a, 2), 115);
            assert_eq!(call(&mut a, 2), 115);
            assert_eq!(count(QUEUE, 2), before + 2);
            // A method that set the table pointer anew.
            let shim = table(this);
            assert!(ours(this));
            a.vtbl = &vtbl;
            assert!(!ours(this));
            again(this, shim, patch);
            assert_eq!(table(this), shim);
            // Or to another table.
            let other = Vtbl {
                methods: [add as *const () as usize; 8],
            };
            a.vtbl = &other;
            again(this, shim, patch);
            assert!(ours(this) && table(this) != shim);
            assert_eq!(call(&mut a, 0), 230 + 1000);
            remove(this);
            assert!(std::ptr::eq(a.vtbl, &other));
            assert_eq!(call(&mut a, 0), 115);
        }
    }
}
