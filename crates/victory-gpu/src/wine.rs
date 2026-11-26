//! Whether we run under Wine, for switches whose default is on there only
//! (a change not yet tried on a Windows host).

use std::ffi::OsStr;

/// Whether we run under Wine (its ntdll exports `wine_get_version`); the
/// same probe as `victory-hle`'s `xmem::on_wine`.
pub(crate) fn on_wine() -> bool {
    use std::sync::LazyLock;
    static WINE: LazyLock<bool> = LazyLock::new(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        // SAFETY: plain lookups in ntdll, always loaded.
        unsafe {
            let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            !ntdll.is_null() && GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()).is_some()
        }
    });
    *WINE
}

/// Whether a switch is on: `0` off and `1` on, on any host; unset, or any
/// other value (empty, a typo), only under Wine. Only an exact `1` turns a
/// change on where it has not been tried.
fn value_is_on(value: Option<&OsStr>, wine: bool) -> bool {
    match value {
        Some(v) if v == "0" => false,
        Some(v) if v == "1" => true,
        _ => wine,
    }
}

/// Whether the switch `name` is on: `0` never; `1` on every host; unset or
/// anything else, only under Wine.
pub(crate) fn on_under_wine(name: &str) -> bool {
    value_is_on(std::env::var_os(name).as_deref(), on_wine())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off on Windows unless asked for, on under Wine unless turned off.
    #[test]
    fn switches_follow_the_host_unless_set() {
        assert!(!value_is_on(None, false));
        assert!(value_is_on(None, true));
        assert!(!value_is_on(Some(OsStr::new("0")), true));
        assert!(value_is_on(Some(OsStr::new("1")), false));
        // Anything but an exact 1 does not turn it on off Wine.
        assert!(!value_is_on(Some(OsStr::new("")), false));
        assert!(!value_is_on(Some(OsStr::new("false")), false));
        assert!(value_is_on(Some(OsStr::new("true")), true));
    }
}
