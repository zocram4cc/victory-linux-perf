/// Resolve both spellings once, before dispatching either translate mode.
pub(crate) fn select(
    wave: Option<&str>,
    alias: Option<&str>,
    warn: impl FnOnce(String),
) -> Option<u32> {
    let wave = wave.and_then(|value| value.parse::<u32>().ok());
    let alias = alias.and_then(|value| value.parse::<u32>().ok());
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
