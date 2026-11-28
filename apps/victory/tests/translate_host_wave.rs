#[path = "../src/translate/host_wave.rs"]
mod host_wave;

use std::ffi::{OsStr, OsString};
use std::process::Command;

use victory_shader::emit::{host_settings, set_host_max_wave};

#[test]
fn alias_survives_first_host_probe() {
    let wave = host_wave::select(None, Some(OsStr::new("32")), |_| {});
    std::thread::scope(|scope| {
        host_wave::spawn(scope, wave, || {
            // Windows publishes WaveLaneCountMax before the cache fingerprints
            // host_settings() and the translator reads the same effective width.
            set_host_max_wave(64);
            assert_eq!(host_settings()[0], 32);
        });
    });
    // A thread outside translate still sees the adapter.
    assert_eq!(host_settings()[0], 64);

    // This integration binary owns the mock adapter state. Exercise the
    // shared worker setup with no probe (ordinary) and a later probe (warm).
    for (wave, alias, ordinary, warm) in [
        (None, None, 0, 64),
        (Some("32"), None, 32, 32),
        (Some("64 "), None, 64, 64),
        (None, Some("32"), 32, 32),
        (Some("32"), Some("32"), 32, 32),
        (Some("32"), Some("64"), 32, 32),
        (Some("bad"), Some("\t48 "), 48, 48),
        (Some("bad"), Some("bad"), 0, 64),
        (Some(" 0 "), Some("64"), 0, 0),
        (None, Some("0"), 0, 0),
        (Some(" "), Some("-1"), 0, 64),
    ] {
        let selected = host_wave::select(wave.map(OsStr::new), alias.map(OsStr::new), |_| {});
        set_host_max_wave(0);
        std::thread::scope(|scope| {
            host_wave::spawn(scope, selected, || {
                assert_eq!(host_settings()[0], ordinary, "{wave:?} / {alias:?}");
                set_host_max_wave(64);
                assert_eq!(host_settings()[0], warm, "{wave:?} / {alias:?}");
            });
        });
    }
    set_host_max_wave(0);
}

#[test]
fn reports_alias_and_conflict_at_selection() {
    for (preferred, alias, expected) in [
        (
            None,
            Some("32 "),
            &["using VICTORY_HOST_MAX_WAVE=32 (alias for VICTORY_HOST_WAVE)"][..],
        ),
        (
            Some("32"),
            Some("64"),
            &["using VICTORY_HOST_WAVE=32 over VICTORY_HOST_MAX_WAVE=64"],
        ),
        (
            Some("wave64"),
            Some("32"),
            &[
                "ignoring VICTORY_HOST_WAVE=\"wave64\". Use an unsigned 32-bit integer (0 means unknown width).",
                "using VICTORY_HOST_MAX_WAVE=32 (alias for VICTORY_HOST_WAVE)",
            ],
        ),
        (Some("32"), Some(" 32 "), &[]),
        (None, None, &[]),
    ] {
        let mut notices = Vec::new();
        let _ = host_wave::select(
            preferred.map(OsStr::new),
            alias.map(OsStr::new),
            |message| {
                notices.push(message);
            },
        );
        assert_eq!(notices, expected);
    }
}

#[test]
fn reports_malformed_environment_in_both_translate_modes() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    std::fs::create_dir(&input).unwrap();
    let invalid_values = [
        OsString::from("64 lanes"),
        #[cfg(windows)]
        std::os::windows::ffi::OsStringExt::from_wide(&[0xd800]),
        #[cfg(unix)]
        std::os::unix::ffi::OsStringExt::from_vec(vec![0xff]),
    ];

    for args in [&[][..], &["--warm", "all"]] {
        for invalid in &invalid_values {
            let output = Command::new(env!("CARGO_BIN_EXE_victory"))
                .arg("translate")
                .args(args)
                .arg(&input)
                .current_dir(temp.path())
                .env_remove("VICTORY_CONFIG")
                .env_remove("VICTORY_EMIT_DUMP")
                .env("VICTORY_CACHE_DIR", temp.path().join("cache"))
                .env("VICTORY_DATA_DIR", temp.path().join("data"))
                .env("VICTORY_HOST_WAVE", invalid)
                .env("VICTORY_HOST_MAX_WAVE", "wave64")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr.lines().collect::<Vec<_>>(),
                [
                    format!(
                        "ignoring VICTORY_HOST_WAVE={invalid:?}. Use an unsigned 32-bit integer (0 means unknown width)."
                    ),
                    "ignoring VICTORY_HOST_MAX_WAVE=\"wave64\". Use an unsigned 32-bit integer (0 means unknown width).".to_owned(),
                ],
                "{args:?} with {invalid:?}"
            );
        }
    }
}
