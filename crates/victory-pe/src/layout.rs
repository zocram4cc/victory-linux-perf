//! Raw byte offsets of PE32+ header fields, for code that patches images.
//!
//! All offsets are relative to the start of the structure named in the
//! constant's prefix. They follow the published PE/COFF specification.

/// Offset of `e_lfanew` (u32) in the DOS header.
pub const DOS_E_LFANEW: usize = 0x3C;

/// Size of the `PE\0\0` signature that precedes the file header.
pub const NT_SIGNATURE_SIZE: usize = 4;
/// Size of `IMAGE_FILE_HEADER`.
pub const FILE_HEADER_SIZE: usize = 20;

/// `IMAGE_FILE_HEADER::Characteristics` (u16).
pub const FILE_HEADER_CHARACTERISTICS: usize = 18;

/// `IMAGE_OPTIONAL_HEADER64::AddressOfEntryPoint` (u32).
pub const OPT_ADDRESS_OF_ENTRY_POINT: usize = 16;
/// `IMAGE_OPTIONAL_HEADER64::CheckSum` (u32).
pub const OPT_CHECKSUM: usize = 64;
/// `IMAGE_OPTIONAL_HEADER64::DllCharacteristics` (u16).
pub const OPT_DLL_CHARACTERISTICS: usize = 70;
/// `IMAGE_OPTIONAL_HEADER64::NumberOfRvaAndSizes` (u32).
pub const OPT_NUMBER_OF_RVA_AND_SIZES: usize = 108;
/// First `IMAGE_DATA_DIRECTORY` entry (each is `{ rva: u32, size: u32 }`).
pub const OPT_DATA_DIRECTORY: usize = 112;
/// Size of one `IMAGE_DATA_DIRECTORY`.
pub const DATA_DIRECTORY_SIZE: usize = 8;

/// `IMAGE_TLS_DIRECTORY64::AddressOfCallBacks` (u64 VA).
pub const TLS64_ADDRESS_OF_CALLBACKS: usize = 24;

/// Data directory indices.
pub mod dir {
    pub const IMPORT: usize = 1;
    pub const EXCEPTION: usize = 3;
    pub const BASERELOC: usize = 5;
    pub const LOAD_CONFIG: usize = 10;
    pub const BOUND_IMPORT: usize = 11;
    pub const IAT: usize = 12;
}

/// `IMAGE_FILE_HEADER::Characteristics` flags.
pub mod file_flags {
    pub const RELOCS_STRIPPED: u16 = 0x0001;
    pub const DLL: u16 = 0x2000;
}

/// `IMAGE_OPTIONAL_HEADER64::DllCharacteristics` flags.
pub mod dll_flags {
    pub const HIGH_ENTROPY_VA: u16 = 0x0020;
    pub const DYNAMIC_BASE: u16 = 0x0040;
    pub const FORCE_INTEGRITY: u16 = 0x0080;
    pub const NX_COMPAT: u16 = 0x0100;
    pub const NO_ISOLATION: u16 = 0x0200;
    pub const NO_SEH: u16 = 0x0400;
    pub const NO_BIND: u16 = 0x0800;
    pub const APPCONTAINER: u16 = 0x1000;
    pub const WDM_DRIVER: u16 = 0x2000;
    pub const GUARD_CF: u16 = 0x4000;
    pub const TERMINAL_SERVER_AWARE: u16 = 0x8000;
}

/// `IMAGE_OPTIONAL_HEADER64::Subsystem` values.
pub mod subsystem {
    pub const WINDOWS_GUI: u16 = 2;
    pub const WINDOWS_CUI: u16 = 3;
    pub const XBOX: u16 = 14;
    pub const XBOX_CODE_CATALOG: u16 = 17;
}

/// Offset of the optional header from the start of the file.
pub fn optional_header_offset(data: &[u8]) -> Option<usize> {
    let lfanew = read_u32(data, DOS_E_LFANEW)? as usize;
    lfanew.checked_add(NT_SIGNATURE_SIZE + FILE_HEADER_SIZE)
}

/// Offset of the file header from the start of the file.
pub fn file_header_offset(data: &[u8]) -> Option<usize> {
    let lfanew = read_u32(data, DOS_E_LFANEW)? as usize;
    lfanew.checked_add(NT_SIGNATURE_SIZE)
}

/// Offset of data directory `index` from the start of the file.
pub fn data_directory_offset(data: &[u8], index: usize) -> Option<usize> {
    let opt = optional_header_offset(data)?;
    let count = read_u32(data, opt.checked_add(OPT_NUMBER_OF_RVA_AND_SIZES)?)? as usize;
    if index >= count {
        return None;
    }
    opt.checked_add(OPT_DATA_DIRECTORY)?
        .checked_add(index.checked_mul(DATA_DIRECTORY_SIZE)?)
}

pub fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    Some(u32::from_le_bytes(data.get(at..end)?.try_into().ok()?))
}

pub fn read_u64(data: &[u8], at: usize) -> Option<u64> {
    let end = at.checked_add(8)?;
    Some(u64::from_le_bytes(data.get(at..end)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Length of a synthetic PE32+ prefix whose `e_lfanew` is 0x40: it ends
    /// right after the 16th (last) data directory entry.
    const IMAGE_LEN: usize =
        0x40 + NT_SIGNATURE_SIZE + FILE_HEADER_SIZE + OPT_DATA_DIRECTORY + 16 * DATA_DIRECTORY_SIZE;

    /// A synthetic PE32+ prefix: DOS header pointing at the NT headers, the
    /// PE32+ magic, and a full data directory table of 16 entries.
    fn image() -> Vec<u8> {
        let mut data = vec![0u8; IMAGE_LEN];
        data[..2].copy_from_slice(b"MZ");
        data[DOS_E_LFANEW..DOS_E_LFANEW + 4].copy_from_slice(&0x40u32.to_le_bytes());
        let opt = optional_header_offset(&data).unwrap();
        data[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        data[opt + OPT_NUMBER_OF_RVA_AND_SIZES..opt + OPT_NUMBER_OF_RVA_AND_SIZES + 4]
            .copy_from_slice(&16u32.to_le_bytes());
        data
    }

    #[test]
    fn reads_reject_overflowing_offsets() {
        let data = [0u8; 16];
        assert_eq!(read_u32(&data, usize::MAX - 1), None);
        assert_eq!(read_u64(&data, usize::MAX - 2), None);
    }

    #[test]
    fn reads_little_endian_values_at_the_edges_of_the_buffer() {
        let mut data = [0u8; 8];
        data.copy_from_slice(&0x0123_4567_89AB_CDEFu64.to_le_bytes());
        assert_eq!(read_u32(&data, 0), Some(0x89AB_CDEF));
        assert_eq!(read_u32(&data, 4), Some(0x0123_4567));
        // One byte short of a field is not a field.
        assert_eq!(read_u32(&data, 5), None);
        assert_eq!(read_u32(&data, 8), None);
        assert_eq!(read_u64(&data, 0), Some(0x0123_4567_89AB_CDEF));
        assert_eq!(read_u64(&data, 1), None);
        assert_eq!(read_u64(&data, 8), None);
        assert_eq!(read_u32(b"", 0), None);
        assert_eq!(read_u64(b"", 0), None);
    }

    #[test]
    fn header_offsets_follow_e_lfanew() {
        let data = image();
        assert_eq!(file_header_offset(&data), Some(0x40 + NT_SIGNATURE_SIZE));
        assert_eq!(
            optional_header_offset(&data),
            Some(0x40 + NT_SIGNATURE_SIZE + FILE_HEADER_SIZE)
        );
        let fh = file_header_offset(&data).unwrap();
        let opt = optional_header_offset(&data).unwrap();
        assert_eq!(opt - fh, FILE_HEADER_SIZE);
    }

    #[test]
    fn header_offsets_need_the_dos_header() {
        assert_eq!(optional_header_offset(b""), None);
        assert_eq!(file_header_offset(b""), None);
        // `e_lfanew` is the last field the offsets read.
        assert_eq!(optional_header_offset(&[0u8; DOS_E_LFANEW]), None);
        let mut data = [0u8; DOS_E_LFANEW + 4];
        data[DOS_E_LFANEW..].copy_from_slice(&0x40u32.to_le_bytes());
        assert_eq!(file_header_offset(&data), Some(0x44));
        assert_eq!(optional_header_offset(&data), Some(0x58));
    }

    #[test]
    fn a_hostile_e_lfanew_is_passed_on_but_the_readers_refuse_it() {
        // The offsets are pure arithmetic, so an `e_lfanew` past the end of
        // the buffer is reported unchanged; every read through it is
        // bounds-checked. (A value near `u32::MAX` would also overflow the
        // addition on a 32-bit host, which is why it is a `checked_add`.)
        let mut data = [0u8; DOS_E_LFANEW + 4];
        data[DOS_E_LFANEW..].copy_from_slice(&0x10_0000u32.to_le_bytes());
        assert_eq!(file_header_offset(&data), Some(0x10_0004));
        assert_eq!(optional_header_offset(&data), Some(0x10_0018));
        assert_eq!(
            optional_header_offset(&data).and_then(|at| read_u32(&data, at)),
            None
        );
        assert_eq!(
            file_header_offset(&data).and_then(|at| read_u32(&data, at)),
            None
        );
        assert_eq!(data_directory_offset(&data, dir::IMPORT), None);
    }

    #[test]
    fn data_directory_offsets_are_indexed_from_the_optional_header() {
        let data = image();
        let opt = optional_header_offset(&data).unwrap();
        assert_eq!(
            data_directory_offset(&data, 0),
            Some(opt + OPT_DATA_DIRECTORY)
        );
        assert_eq!(
            data_directory_offset(&data, dir::IMPORT),
            Some(opt + OPT_DATA_DIRECTORY + DATA_DIRECTORY_SIZE)
        );
        assert_eq!(
            data_directory_offset(&data, 15),
            Some(opt + OPT_DATA_DIRECTORY + 15 * DATA_DIRECTORY_SIZE)
        );
        // The table holds 16 entries in this image.
        assert_eq!(data_directory_offset(&data, 16), None);
        assert_eq!(data_directory_offset(&data, usize::MAX), None);
    }

    #[test]
    fn data_directory_offset_honours_a_shorter_table() {
        let mut data = image();
        let opt = optional_header_offset(&data).unwrap();
        data[opt + OPT_NUMBER_OF_RVA_AND_SIZES..opt + OPT_NUMBER_OF_RVA_AND_SIZES + 4]
            .copy_from_slice(&3u32.to_le_bytes());
        assert!(data_directory_offset(&data, 2).is_some());
        assert_eq!(data_directory_offset(&data, 3), None);
        assert_eq!(data_directory_offset(&data, dir::IAT), None);
    }

    #[test]
    fn data_directory_offset_needs_the_count_and_the_header() {
        let full = image();
        let opt = optional_header_offset(&full).unwrap();
        // Ends exactly where `NumberOfRvaAndSizes` starts.
        let cut = &full[..opt + OPT_NUMBER_OF_RVA_AND_SIZES];
        assert_eq!(
            data_directory_offset(cut, dir::IMPORT),
            None,
            "a header without the count field has no directories"
        );
        assert_eq!(data_directory_offset(b"", dir::IMPORT), None);
        assert_eq!(data_directory_offset(b"MZ", dir::IMPORT), None);
    }

    #[test]
    fn a_data_directory_entry_reads_back_as_the_image_wrote_it() {
        let mut data = image();
        let at = data_directory_offset(&data, dir::IMPORT).unwrap();
        data[at..at + 4].copy_from_slice(&0x3000u32.to_le_bytes());
        data[at + 4..at + 8].copy_from_slice(&0x28u32.to_le_bytes());
        assert_eq!(read_u32(&data, at), Some(0x3000));
        assert_eq!(read_u32(&data, at + 4), Some(0x28));
    }

    #[test]
    fn constants_match_the_pe32_layout() {
        // Offsets of fields inside the structures they are named after.
        assert_eq!(DOS_E_LFANEW, 0x3C);
        assert_eq!(NT_SIGNATURE_SIZE, 4);
        assert_eq!(FILE_HEADER_SIZE, 20);
        assert_eq!(FILE_HEADER_CHARACTERISTICS, FILE_HEADER_SIZE - 2);
        assert_eq!(OPT_ADDRESS_OF_ENTRY_POINT, 16);
        assert_eq!(OPT_CHECKSUM, 64);
        assert_eq!(OPT_DLL_CHARACTERISTICS, 70);
        assert_eq!(OPT_NUMBER_OF_RVA_AND_SIZES, 108);
        assert_eq!(OPT_DATA_DIRECTORY, OPT_NUMBER_OF_RVA_AND_SIZES + 4);
        assert_eq!(DATA_DIRECTORY_SIZE, 8);
        assert_eq!(TLS64_ADDRESS_OF_CALLBACKS, 24);

        // Data directory indices, which `prepare` patches by number.
        assert_eq!(dir::IMPORT, 1);
        assert_eq!(dir::EXCEPTION, 3);
        assert_eq!(dir::BASERELOC, 5);
        assert_eq!(dir::LOAD_CONFIG, 10);
        assert_eq!(dir::BOUND_IMPORT, 11);
        assert_eq!(dir::IAT, 12);
    }
}
