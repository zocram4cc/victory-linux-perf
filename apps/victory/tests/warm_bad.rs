#![cfg(windows)]

use std::fs;
use std::path::Path;
use std::process::Command;

fn run_warm(root: &Path, cache: &Path, extra_env: &[(&str, &str)], args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_victory"));
    command
        .current_dir(root)
        .env_remove("VICTORY_CONFIG")
        .env_remove("VICTORY_WARM_BAD")
        .env_remove("VICTORY_WAVE_WIDE")
        .env("VICTORY_CACHE_DIR", cache)
        .env("VICTORY_DATA_DIR", root.join("data"))
        // Keep fingerprint computation deterministic without probing a host GPU.
        .env("VICTORY_D3D12_ADAPTER", u32::MAX.to_string())
        .env("VICTORY_D3D12_WARP", "0")
        .args(["warm", "--jobs", "1"])
        .args(args);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn parse_fingerprint(stdout: &str) -> String {
    let (_, after) = stdout
        .split_once("made before (")
        .unwrap_or_else(|| panic!("missing fingerprint prefix in:\n{stdout}"));
    let (fp, _) = after
        .split_once(')')
        .unwrap_or_else(|| panic!("missing fingerprint suffix in:\n{stdout}"));
    assert_eq!(fp.len(), 16, "unexpected fingerprint {fp:?} in:\n{stdout}");
    assert!(
        fp.bytes().all(|b| b.is_ascii_hexdigit()),
        "non-hex fingerprint {fp:?}"
    );
    fp.to_owned()
}

#[test]
fn warm_bad_fingerprint_override_again_and_help() {
    let help = Command::new(env!("CARGO_BIN_EXE_victory"))
        .args(["warm", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success(), "{help:?}");
    let help_stdout = String::from_utf8_lossy(&help.stdout);
    assert!(help_stdout.contains("VICTORY_WARM_BAD=0"), "{help_stdout}");
    assert!(help_stdout.contains("warm-bad.txt"), "{help_stdout}");

    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    let pipelines = cache.join("pipelines");
    fs::create_dir_all(&pipelines).unwrap();

    let ids = [0x11u8, 0x22, 0x33, 0x44];
    let [k_fp_bad, k_legacy_bad, k_fresh, k_other] = ids.map(|id| format!("{id:02x}").repeat(16));
    let mut book = Vec::new();
    for id in ids {
        // Minimal 16-byte record so known::keys reads the key while workers safely skip compilation.
        book.extend(16u32.to_le_bytes());
        book.extend([id; 16]);
    }
    fs::write(pipelines.join("known-pipelines.bin"), book).unwrap();
    fs::write(pipelines.join("known-shaders.bin"), b"").unwrap();
    fs::write(pipelines.join("early-pipelines.txt"), format!("{k_fp_bad}\n{k_fresh}\n")).unwrap();

    let probe = run_warm(root.path(), &cache, &[], &["--limit", "0"]);
    let fp_base = parse_fingerprint(&probe);

    let fp_entry = format!("{k_fp_bad} fp={fp_base} exit code: 0xc0000005");
    assert_eq!(fp_entry.split(' ').next(), Some(k_fp_bad.as_str()));
    fs::write(
        pipelines.join("warm-bad.txt"),
        format!("\n# comment line\n   # indented comment\n{fp_entry}\n\t{k_legacy_bad}\tlegacy crash\n"),
    )
    .unwrap();

    let out_default = run_warm(root.path(), &cache, &[], &[]);
    assert!(
        out_default.contains(&format!("4 pipelines known, 0 made before ({fp_base}), 2 bad; 2 to make")),
        "{out_default}"
    );

    let out_repeat = run_warm(root.path(), &cache, &[], &[]);
    assert!(
        out_repeat.contains(&format!("4 pipelines known, 2 made before ({fp_base}), 2 bad; 0 to make")),
        "{out_repeat}"
    );

    let out_override = run_warm(root.path(), &cache, &[("VICTORY_WARM_BAD", "0")], &[]);
    assert!(
        out_override.contains(&format!("4 pipelines known, 2 made before ({fp_base}), 0 bad; 2 to make")),
        "{out_override}"
    );

    let out_again = run_warm(root.path(), &cache, &[], &["--again", "--used"]);
    assert!(
        out_again.contains(&format!("4 pipelines known, 4 made before ({fp_base}), 2 bad; 2 to make")),
        "{out_again}"
    );

    // Changing a translator switch rotates the fingerprint, expiring fp-tagged entries automatically.
    let out_new_fp = run_warm(root.path(), &cache, &[("VICTORY_WAVE_WIDE", "0")], &[]);
    let fp_new = parse_fingerprint(&out_new_fp);
    assert_ne!(fp_new, fp_base);
    assert!(
        out_new_fp.contains(&format!("4 pipelines known, 0 made before ({fp_new}), 1 bad; 3 to make")),
        "{out_new_fp}"
    );

    let done = fs::read_to_string(pipelines.join("warm-done.txt")).unwrap();
    let new_fp_keys: Vec<_> = done
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(fp, _)| *fp == fp_new)
        .map(|(_, key)| key)
        .collect();
    assert_eq!(
        new_fp_keys,
        [k_fp_bad.as_str(), k_fresh.as_str(), k_other.as_str()]
    );
}
