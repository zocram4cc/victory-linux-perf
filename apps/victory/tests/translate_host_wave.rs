#[path = "../src/translate/host_wave.rs"]
mod host_wave;

use victory_shader::emit::{host_settings, set_host_max_wave};

#[test]
fn alias_survives_first_host_probe() {
    let wave = host_wave::select(None, Some("32"), |_| {});
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
        (None, Some("32"), 32, 32),
        (Some("32"), Some("32"), 32, 32),
        (Some("32"), Some("64"), 32, 32),
        (Some("bad"), Some("48"), 48, 48),
        (Some("bad"), Some("bad"), 0, 64),
        (Some("0"), Some("64"), 0, 0),
        (None, Some("0"), 0, 0),
        (Some(" 32"), Some("-1"), 0, 64),
    ] {
        set_host_max_wave(0);
        std::thread::scope(|scope| {
            host_wave::spawn(scope, host_wave::select(wave, alias, |_| {}), || {
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
            Some("32"),
            Some("using VICTORY_HOST_MAX_WAVE=32 (alias for VICTORY_HOST_WAVE)"),
        ),
        (
            Some("32"),
            Some("64"),
            Some("using VICTORY_HOST_WAVE=32 over VICTORY_HOST_MAX_WAVE=64"),
        ),
        (Some("32"), Some("32"), None),
        (Some("32"), Some("bad"), None),
        (None, None, None),
    ] {
        let mut notice = None;
        let _ = host_wave::select(preferred, alias, |message| notice = Some(message));
        assert_eq!(notice.as_deref(), expected);
    }
}
