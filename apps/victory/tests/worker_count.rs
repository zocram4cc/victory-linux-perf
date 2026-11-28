use std::process::Command;

#[test]
fn worker_count_is_validated_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    // Accepted arguments reach the config error without reading game files or probing the GPU.
    let missing_config = dir.path().join("missing.toml");
    let modes: &[&[&str]] = &[
        &["translate"],
        &["translate", "--warm", "all"],
        &["shaders"],
        #[cfg(windows)]
        &["warm"],
    ];

    for args in modes {
        for jobs in [Some("0"), Some("1"), Some("3"), None] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_victory"));
            command
                .args(*args)
                .env("VICTORY_CONFIG", &missing_config)
                .env_remove("VICTORY_AUDIO_CAPTURE")
                .env_remove("VICTORY_EMIT_DUMP");
            if let Some(jobs) = jobs {
                command.args(["--jobs", jobs]);
            }
            let output = command.output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let (code, message) = if jobs == Some("0") {
                (2, "invalid value '0' for '--jobs")
            } else {
                (1, "VICTORY_CONFIG points at")
            };
            assert_eq!(
                output.status.code(),
                Some(code),
                "{args:?} {jobs:?}: {stderr}"
            );
            assert!(stderr.contains(message), "{args:?} {jobs:?}: {stderr}");
        }
    }
}
