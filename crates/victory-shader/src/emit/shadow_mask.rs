pub const REVISION: &str = "shadow-bounds-1";

pub(super) const LIGHT_LATCH: usize = 0x1410;
pub(super) const LIGHT_EXIT: usize = 0x8ad0;

pub(super) fn bounded_loop(offset: usize, limit: u32) -> bool {
    const ACTIVE_BITS: usize = 0x1460;
    const LIGHTS_BY_WAVE: usize = 0x7580;
    match offset {
        ACTIVE_BITS => limit >= u32::BITS,
        LIGHTS_BY_WAVE => limit >= 256 / 32,
        _ => false,
    }
}

pub fn applies_to_hash(hash: &[u8]) -> bool {
    const SHADOW_MASK: [u8; 32] = [
        0x5c, 0xf0, 0x0d, 0xc3, 0x90, 0x42, 0xed, 0xaf, 0x0b, 0xce, 0x07, 0x9f, 0x28, 0xf9, 0x1b,
        0x11, 0xa4, 0x47, 0xb2, 0x0d, 0x41, 0x8c, 0x51, 0x8b, 0x57, 0x55, 0x69, 0x63, 0xbf, 0x92,
        0xc5, 0x00,
    ];
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    hash == SHADOW_MASK
        && *ENABLED.get_or_init(|| std::env::var_os("VICTORY_LIMIT_EXITS").is_none_or(|v| v != "0"))
}

pub fn applies_to(container: &[u8]) -> bool {
    container.len() == 23140 && applies_to_hash(blake3::hash(container).as_bytes())
}
