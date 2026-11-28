use std::sync::OnceLock;

pub const NAME: &str = "f22a76fcb967ee59";
pub const WORDS: u32 = 8;
pub const REVISION: &str = "screen-probe-intersections-1";

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    cfg!(feature = "diagnostics")
        && *ON.get_or_init(|| {
            matches!(
                std::env::var("VICTORY_CAPTURE_RAY_INTERSECTIONS").as_deref(),
                Ok("1") | Ok(NAME)
            )
        })
}

pub fn applies_to(bytes: &[u8]) -> bool {
    const HASH: [u8; 32] = [
        0xf2, 0x2a, 0x76, 0xfc, 0xb9, 0x67, 0xee, 0x59, 0x91, 0x30, 0x34, 0xa6, 0x50, 0xe8, 0xe9,
        0x04, 0x3c, 0x88, 0x19, 0xbb, 0xf6, 0x34, 0x52, 0x10, 0xa5, 0x7e, 0x8a, 0x9b, 0x14, 0x25,
        0x0a, 0xd5,
    ];
    bytes.len() == 14712 && enabled() && blake3::hash(bytes).as_bytes() == &HASH
}
