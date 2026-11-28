use std::path::Path;
use std::process::Command;

#[test]
fn census_fails_when_a_requested_path_is_missing() {
    let valid = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let missing = valid.with_extension("missing-census-input");
    assert!(!missing.exists());

    let output = Command::new(env!("CARGO_BIN_EXE_victory"))
        .arg("census")
        .args([&valid, &missing])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "census succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(stderr.contains("Cargo.missing-census-input"), "{stderr}");
}

#[test]
fn census_accepts_a_valid_path_with_no_pe_images() {
    let valid = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_victory"))
        .arg("census")
        .arg(valid)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("0 images scanned, 0 unreadable"));
}
