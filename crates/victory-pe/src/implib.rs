//! COFF import libraries (`.lib`).
//!
//! An umbrella import library tells us which DLL provides each API that a
//! console title may link against. That is the map the runtime needs to
//! decide which of our implementations answers which import.

use object::read::archive::ArchiveFile;
use object::read::coff::{ImportFile, ImportName, ImportType};

use crate::{ImportSymbol, PeError};

/// One `dll!export` pair described by an import library.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LibImport {
    pub dll: String,
    pub export: ImportSymbol,
    pub kind: LibImportKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LibImportKind {
    Code,
    Data,
    Const,
}

/// Result of reading an import library.
#[derive(Debug, Default)]
pub struct ImportLibrary {
    pub imports: Vec<LibImport>,
    /// Members that are not short-format import objects (regular objects,
    /// long-format import descriptors). Reported so nothing is silently lost.
    pub other_members: usize,
}

impl ImportLibrary {
    pub fn parse(data: &[u8]) -> Result<Self, PeError> {
        let archive = ArchiveFile::parse(data)?;
        let mut lib = ImportLibrary::default();
        for member in archive.members() {
            let member = member?;
            let bytes = member.data(data)?;
            match ImportFile::parse(bytes) {
                Ok(import) => lib.imports.push(LibImport {
                    dll: String::from_utf8_lossy(import.dll()).into_owned(),
                    export: match import.import() {
                        ImportName::Name(name) => ImportSymbol::Name {
                            hint: 0,
                            name: String::from_utf8_lossy(name).into_owned(),
                        },
                        ImportName::Ordinal(o) => ImportSymbol::Ordinal(o),
                    },
                    kind: match import.import_type() {
                        ImportType::Code => LibImportKind::Code,
                        ImportType::Data => LibImportKind::Data,
                        ImportType::Const => LibImportKind::Const,
                    },
                }),
                Err(_) => lib.other_members += 1,
            }
        }
        lib.imports.sort();
        lib.imports.dedup();
        Ok(lib)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MACHINE_AMD64;

    /// `Sig1`, which must be `IMAGE_FILE_MACHINE_UNKNOWN` (not a machine).
    const SIG1: u16 = 0;
    /// `Sig2` of a short-format import object (`IMPORT_OBJECT_HDR_SIG2`).
    const SIG2: u16 = 0xFFFF;

    /// `Type` field of the header, a 2-bit value at bit 0.
    mod import_type {
        pub const CODE: u16 = 0;
        pub const DATA: u16 = 1;
        pub const CONST: u16 = 2;
    }

    /// `NameType` field of the header, a 3-bit value at bit 2.
    mod name_type {
        pub const ORDINAL: u16 = 0;
        pub const NAME: u16 = 1;
        pub const NAME_NO_PREFIX: u16 = 2;
        pub const NAME_UNDECORATE: u16 = 3;
        pub const NAME_EXPORTAS: u16 = 4;
    }

    /// One short-format import object: the 20-byte header followed by the
    /// NUL-terminated symbol, DLL and (for `NAME_EXPORTAS`) export names.
    fn import_object(
        import_type: u16,
        name_type: u16,
        ordinal_or_hint: u16,
        symbol: &str,
        dll: &str,
        export_as: Option<&str>,
    ) -> Vec<u8> {
        let mut strings = Vec::new();
        for s in [Some(symbol), Some(dll), export_as].into_iter().flatten() {
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&SIG1.to_le_bytes());
        out.extend_from_slice(&SIG2.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // Version
        out.extend_from_slice(&MACHINE_AMD64.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
        out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        out.extend_from_slice(&ordinal_or_hint.to_le_bytes());
        out.extend_from_slice(&(import_type | name_type << 2).to_le_bytes());
        out.extend_from_slice(&strings);
        out
    }

    /// One archive member: the 60-byte header, the data, and the pad byte the
    /// format adds so the next header starts even.
    fn archive_member(name: &str, data: &[u8]) -> Vec<u8> {
        assert!(name.len() <= 16, "an archive member name is 16 bytes");
        let mut out = vec![b' '; 60];
        out[..name.len()].copy_from_slice(name.as_bytes());
        let size = data.len().to_string();
        out[48..48 + size.len()].copy_from_slice(size.as_bytes());
        out[58] = b'`';
        out[59] = b'\n';
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
        out
    }

    fn archive(members: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut out = b"!<arch>\n".to_vec();
        for (name, data) in members {
            out.extend_from_slice(&archive_member(name, &data));
        }
        out
    }

    #[test]
    fn parses_named_code_imports() {
        // Deliberately out of order, to show that the result is sorted.
        let lib = archive(vec![
            (
                "version.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME,
                    0,
                    "GetFileVersionInfoSizeW",
                    "version.dll",
                    None,
                ),
            ),
            (
                "kernel32.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME,
                    7,
                    "ExitProcess",
                    "kernel32.dll",
                    None,
                ),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.other_members, 0);
        assert_eq!(
            parsed.imports,
            [
                LibImport {
                    dll: "kernel32.dll".into(),
                    // `object` does not expose the hint of a short-format
                    // import, so it is always 0 here.
                    export: ImportSymbol::Name {
                        hint: 0,
                        name: "ExitProcess".into()
                    },
                    kind: LibImportKind::Code,
                },
                LibImport {
                    dll: "version.dll".into(),
                    export: ImportSymbol::Name {
                        hint: 0,
                        name: "GetFileVersionInfoSizeW".into()
                    },
                    kind: LibImportKind::Code,
                },
            ]
        );
    }

    #[test]
    fn parses_imports_by_ordinal() {
        let lib = archive(vec![(
            "d3d12.dll/",
            import_object(
                import_type::CODE,
                name_type::ORDINAL,
                42,
                "Alpha",
                "d3d12.dll",
                None,
            ),
        )]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.other_members, 0);
        assert_eq!(parsed.imports.len(), 1);
        assert_eq!(parsed.imports[0].dll, "d3d12.dll");
        assert_eq!(parsed.imports[0].export, ImportSymbol::Ordinal(42));
        assert_eq!(parsed.imports[0].kind, LibImportKind::Code);
    }

    #[test]
    fn maps_the_import_kinds() {
        let lib = archive(vec![
            (
                "a.dll/",
                import_object(
                    import_type::DATA,
                    name_type::NAME,
                    0,
                    "tls_index",
                    "a.dll",
                    None,
                ),
            ),
            (
                "b.dll/",
                import_object(
                    import_type::CONST,
                    name_type::NAME,
                    0,
                    "limit",
                    "b.dll",
                    None,
                ),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        let kinds: Vec<_> = parsed.imports.iter().map(|i| i.kind).collect();
        assert_eq!(parsed.other_members, 0);
        assert_eq!(parsed.imports.len(), 2);
        assert!(kinds.contains(&LibImportKind::Data));
        assert!(kinds.contains(&LibImportKind::Const));
        assert!(!kinds.contains(&LibImportKind::Code));
    }

    #[test]
    fn a_name_exported_under_another_name_uses_the_export() {
        let lib = archive(vec![(
            "a.dll/",
            import_object(
                import_type::CODE,
                name_type::NAME_EXPORTAS,
                0,
                "__imp_local",
                "a.dll",
                Some("Remote"),
            ),
        )]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.other_members, 0);
        assert_eq!(
            parsed.imports[0].export,
            ImportSymbol::Name {
                hint: 0,
                name: "Remote".into()
            }
        );
    }

    #[test]
    fn prefixed_and_decorated_symbols_are_cleaned_up() {
        // `NAME_NO_PREFIX` drops a leading `?`, `@` or `_`; `NAME_UNDECORATE`
        // also stops at the first `@`. Both are what a linker writes for C
        // symbols, so the map holds the plain export name.
        let lib = archive(vec![
            (
                "a.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME_NO_PREFIX,
                    0,
                    "_Remap",
                    "a.dll",
                    None,
                ),
            ),
            (
                "b.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME_UNDECORATE,
                    0,
                    "?Thing@@YAHXZ",
                    "b.dll",
                    None,
                ),
            ),
            (
                "c.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME_NO_PREFIX,
                    0,
                    "plain",
                    "c.dll",
                    None,
                ),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        let names: Vec<_> = parsed
            .imports
            .iter()
            .map(|i| i.export.to_string())
            .collect();
        assert_eq!(parsed.other_members, 0);
        assert_eq!(names, ["Remap", "Thing", "plain"]);
    }

    #[test]
    fn the_same_import_twice_is_listed_once() {
        let exit_process = || {
            import_object(
                import_type::CODE,
                name_type::NAME,
                0,
                "ExitProcess",
                "kernel32.dll",
                None,
            )
        };
        let lib = archive(vec![
            ("kernel32.dll/", exit_process()),
            ("kernel32.dll/", exit_process()),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.imports.len(), 1);
        assert_eq!(parsed.other_members, 0);
    }

    #[test]
    fn the_same_symbol_of_a_different_kind_is_kept() {
        // A data import (`__imp_`) and a code import of one name differ.
        let lib = archive(vec![
            (
                "a.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME,
                    0,
                    "table",
                    "a.dll",
                    None,
                ),
            ),
            (
                "a.dll/",
                import_object(
                    import_type::DATA,
                    name_type::NAME,
                    0,
                    "table",
                    "a.dll",
                    None,
                ),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.imports.len(), 2);
        assert_ne!(parsed.imports[0].kind, parsed.imports[1].kind);
    }

    #[test]
    fn sorted_ordinal_and_named_exports_order_by_dll_then_name() {
        let lib = archive(vec![
            (
                "b.dll/",
                import_object(
                    import_type::CODE,
                    name_type::ORDINAL,
                    1,
                    "second",
                    "b.dll",
                    None,
                ),
            ),
            (
                "a.dll/",
                import_object(import_type::CODE, name_type::NAME, 0, "zeta", "a.dll", None),
            ),
            (
                "a.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME,
                    0,
                    "alpha",
                    "a.dll",
                    None,
                ),
            ),
            (
                "a.dll/",
                import_object(
                    import_type::CODE,
                    name_type::ORDINAL,
                    9,
                    "first",
                    "a.dll",
                    None,
                ),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        let pairs: Vec<_> = parsed
            .imports
            .iter()
            .map(|i| (i.dll.as_str(), i.export.clone()))
            .collect();
        assert_eq!(
            pairs,
            [
                (
                    "a.dll",
                    ImportSymbol::Name {
                        hint: 0,
                        name: "alpha".into()
                    }
                ),
                (
                    "a.dll",
                    ImportSymbol::Name {
                        hint: 0,
                        name: "zeta".into()
                    }
                ),
                ("a.dll", ImportSymbol::Ordinal(9)),
                ("b.dll", ImportSymbol::Ordinal(1)),
            ]
        );
    }

    #[test]
    fn an_empty_archive_has_no_imports() {
        let parsed = ImportLibrary::parse(b"!<arch>\n").unwrap();
        assert!(parsed.imports.is_empty());
        assert_eq!(parsed.other_members, 0);
    }

    #[test]
    fn linker_and_names_members_are_skipped_not_counted() {
        // What a COFF archive looks like: two linker members, an optional
        // names table, then the import objects.
        let lib = archive(vec![
            ("/", vec![0; 8]),
            ("/", vec![0; 8]),
            ("//", b"a_long_member_name/\n".to_vec()),
            (
                "kernel32.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME,
                    0,
                    "ExitProcess",
                    "kernel32.dll",
                    None,
                ),
            ),
            (
                "user32.dll/",
                import_object(
                    import_type::CODE,
                    name_type::NAME,
                    0,
                    "MessageBoxW",
                    "user32.dll",
                    None,
                ),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.imports.len(), 2);
        assert_eq!(parsed.other_members, 0);
    }

    #[test]
    fn members_that_are_not_import_objects_are_counted() {
        let lib = archive(vec![
            // A regular COFF object starts with a machine, not a zero `Sig1`.
            ("object.obj/", {
                let mut object = Vec::new();
                object.extend_from_slice(&MACHINE_AMD64.to_le_bytes());
                object.extend_from_slice(&1u16.to_le_bytes()); // NumberOfSections
                object.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
                object.resize(60, 0);
                object
            }),
            // A long-format descriptor: no short-format `Sig2`.
            ("long.lib/", {
                let mut long = import_object(
                    import_type::CODE,
                    name_type::NAME,
                    0,
                    "Old",
                    "old.dll",
                    None,
                );
                long[2..4].copy_from_slice(&0u16.to_le_bytes());
                long
            }),
            // An import header whose string table is cut short.
            ("short.lib/", {
                let mut short = import_object(
                    import_type::CODE,
                    name_type::NAME,
                    0,
                    "Cut",
                    "cut.dll",
                    None,
                );
                short.truncate(24);
                short
            }),
            (
                "a.dll/",
                import_object(import_type::CODE, name_type::NAME, 0, "Kept", "a.dll", None),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.other_members, 3);
        assert_eq!(parsed.imports.len(), 1);
        assert_eq!(parsed.imports[0].dll, "a.dll");
    }

    #[test]
    fn odd_sized_members_are_padded_aligned() {
        // 20-byte header plus 9 bytes of strings: the archive pads one byte,
        // and the next member must still be found.
        let odd = import_object(import_type::CODE, name_type::NAME, 0, "f", "ab", None);
        assert_eq!(odd.len() % 2, 1);
        let lib = archive(vec![
            ("a.dll/", odd),
            (
                "b.dll/",
                import_object(import_type::CODE, name_type::NAME, 0, "Next", "b.dll", None),
            ),
        ]);
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(parsed.imports.len(), 2);
        assert_eq!(parsed.other_members, 0);
    }

    #[test]
    fn names_that_are_not_utf8_are_reported_lossily() {
        // The symbol name "Café" is stored as UTF-8; replacing its lead byte
        // leaves a stray continuation byte, so the member holds a name that
        // is not valid UTF-8.
        let mut lib = archive(vec![(
            "x.dll/",
            import_object(
                import_type::CODE,
                name_type::NAME,
                0,
                "Caf\u{e9}",
                "x.dll",
                None,
            ),
        )]);
        let at = lib
            .windows(2)
            .position(|w| w == [0xC3, 0xA9])
            .expect("the name is in the member");
        lib[at] = 0xFF;
        let parsed = ImportLibrary::parse(&lib).unwrap();
        assert_eq!(
            parsed.imports[0].export,
            // One replacement character for the invalid byte, one for the
            // continuation byte that is then unexpected.
            ImportSymbol::Name {
                hint: 0,
                name: "Caf\u{fffd}\u{fffd}".into()
            }
        );
    }

    #[test]
    fn rejects_input_that_is_not_an_archive() {
        let cases: [&[u8]; 3] = [
            b"",
            b"MZ a PE image, not a library",
            b"!<arch>", // truncated magic
        ];
        for data in cases {
            assert!(
                matches!(ImportLibrary::parse(data), Err(PeError::Malformed(_))),
                "{data:?} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_a_truncated_member_header() {
        let mut data = b"!<arch>\n".to_vec();
        data.extend_from_slice(&[b'x'; 20]);
        assert!(matches!(
            ImportLibrary::parse(&data),
            Err(PeError::Malformed(_))
        ));
    }

    #[test]
    fn rejects_a_member_whose_data_is_cut_off() {
        // The header promises 0x100 bytes, the archive holds 4.
        let mut data = b"!<arch>\n".to_vec();
        let mut member = archive_member("a.dll/", &[0u8; 0x100]);
        member.truncate(60 + 4);
        data.extend_from_slice(&member);
        assert!(matches!(
            ImportLibrary::parse(&data),
            Err(PeError::Malformed(_))
        ));
    }
}
