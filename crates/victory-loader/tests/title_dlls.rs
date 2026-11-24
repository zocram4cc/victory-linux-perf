//! Finding the title's DLLs: one that cannot be loaded is named in the
//! error, and a folder's case sensitivity does not hide one.
#![cfg(windows)]

#[test]
fn a_title_dll_that_does_not_prepare_is_named() {
    let title = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(title.path().join("broken.dll"), b"MZ not a PE").unwrap();
    let wanted = |_: &str| true;
    let loaded = victory_loader::load_title_dll("broken.dll", title.path(), cache.path(), &wanted);
    let Err(error) = loaded else {
        panic!("a DLL that is not a PE loaded");
    };
    let text = format!("{error:#}");
    assert!(text.contains("broken.dll"), "{text}");
}

/// An empty folder whose names are case-sensitive, as folders WSL creates
/// are (`fsutil file setCaseSensitiveInfo`); `None` where Windows cannot
/// make one.
fn case_sensitive_dir() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    let made = std::process::Command::new("fsutil.exe")
        .args(["file", "setCaseSensitiveInfo"])
        .arg(dir.path())
        .arg("enable")
        .output()
        .is_ok_and(|o| o.status.success());
    made.then_some(dir)
}

#[test]
fn a_title_dll_in_a_case_sensitive_folder_is_found_by_any_spelling() {
    let Some(title) = case_sensitive_dir() else {
        eprintln!("no case-sensitive folders on this machine; skipped");
        return;
    };
    // Any real DLL will do; it imports only host DLLs, which are not in the folder.
    std::fs::copy(
        r"C:\Windows\System32\version.dll",
        title.path().join("Late.dll"),
    )
    .unwrap();
    let wanted = |_: &str| true;
    // As written on disk, and as an import or `LoadLibrary` may spell it.
    for name in ["Late.dll", "LATE.DLL", "late.dll"] {
        let cache = tempfile::tempdir().unwrap();
        let dlls =
            victory_loader::load_title_dll(name, title.path(), cache.path(), &wanted).unwrap();
        let names: Vec<_> = dlls.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["late.dll"], "loading {name}");
    }
}
