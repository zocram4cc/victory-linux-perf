//! Whether we run under Wine, for fixes that work around what Wine,
//! vkd3d-proton or a Linux driver do and must change nothing for a Windows
//! host.

use std::ffi::OsStr;

/// Whether we run under Wine (its ntdll exports `wine_get_version`).
pub fn on_wine() -> bool {
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

/// Whether a switch for a fix needed under Wine is on: as its value says when
/// it is set (`0` off, anything else on), else only under Wine.
fn value_is_on(value: Option<&OsStr>, wine: bool) -> bool {
    match value {
        Some(v) => v != "0",
        None => wine,
    }
}

/// Whether the switch `name` of a fix needed under Wine is on: unset, only
/// under Wine; `0` never; any other value on every host.
pub fn on_under_wine(name: &str) -> bool {
    value_is_on(std::env::var_os(name).as_deref(), on_wine())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Wine fix is off on Windows unless asked for, on under Wine unless
    /// turned off.
    #[test]
    fn wine_fixes_follow_the_host_unless_set() {
        assert!(!value_is_on(None, false));
        assert!(value_is_on(None, true));
        assert!(!value_is_on(Some(OsStr::new("0")), true));
        assert!(value_is_on(Some(OsStr::new("1")), false));
    }

    /// The probe answers, and the same each time.
    #[test]
    fn the_wine_probe_is_stable() {
        assert_eq!(on_wine(), on_wine());
    }
}
