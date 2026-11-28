use std::ffi::OsStr;

/// Resolve both spellings once, before dispatching either translate mode.
pub(crate) fn select(
    wave: Option<&OsStr>,
    alias: Option<&OsStr>,
    mut warn: impl FnMut(String),
) -> Option<u32> {
    let wave = parse("VICTORY_HOST_WAVE", wave, &mut warn);
    let alias = parse("VICTORY_HOST_MAX_WAVE", alias, &mut warn);
    if let Some(alias) = alias {
        match wave {
            Some(wave) if wave != alias => warn(format!(
                "using VICTORY_HOST_WAVE={wave} over VICTORY_HOST_MAX_WAVE={alias}"
            )),
            None => warn(format!(
                "using VICTORY_HOST_MAX_WAVE={alias} (alias for VICTORY_HOST_WAVE)"
            )),
            _ => {}
        }
    }
    wave.or(alias)
}

fn parse(name: &str, value: Option<&OsStr>, warn: &mut impl FnMut(String)) -> Option<u32> {
    let value = value?;
    let wave = value.to_str().and_then(|text| text.trim().parse().ok());
    if wave.is_none() {
        warn(format!(
            "ignoring {name}={value:?}. Use an unsigned 32-bit integer (0 means unknown width)."
        ));
    }
    wave
}

/// Both translate modes start their workers here. Set the explicit width
/// before the worker can open the host, fingerprint a shader, or translate.
pub(crate) fn spawn<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    wave: Option<u32>,
    work: impl FnOnce() + Send + 'scope,
) {
    scope.spawn(move || {
        victory_shader::emit::set_thread_max_wave(wave);
        work();
    });
}
