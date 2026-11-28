//! Which imports do titles need, and which do we cover?

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use victory_hle::{HleResolver, HostPolicy};
use victory_loader::Provenance;
use victory_pe::{ImageInfo, ImportSymbol};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum FunctionKey {
    Name(String),
    Ordinal(u16),
}

impl From<&ImportSymbol> for FunctionKey {
    fn from(symbol: &ImportSymbol) -> Self {
        match symbol {
            ImportSymbol::Name { name, .. } => Self::Name(name.clone()),
            ImportSymbol::Ordinal(ordinal) => Self::Ordinal(*ordinal),
        }
    }
}

impl fmt::Display for FunctionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Double a literal leading '#' to distinguish names from ordinals.
            Self::Name(name) if name.starts_with('#') => write!(f, "#{name}"),
            Self::Name(name) => f.write_str(name),
            Self::Ordinal(ordinal) => write!(f, "#{ordinal}"),
        }
    }
}

type MissingImports = BTreeMap<(String, FunctionKey), BTreeSet<PathBuf>>;

#[derive(Default)]
struct DllStats {
    functions: BTreeSet<FunctionKey>,
    hle: usize,
    host: usize,
    missing: usize,
    images: BTreeSet<PathBuf>,
}

pub fn run(paths: Vec<PathBuf>, strict: bool, list_missing: bool) -> Result<()> {
    let paths = crate::default_paths(paths)?;
    let resolver = HleResolver::new(if strict {
        HostPolicy::none()
    } else {
        HostPolicy::default()
    });

    let mut images = 0usize;
    let mut seen = BTreeSet::new();
    let mut failed = Vec::new();
    let mut dlls: BTreeMap<String, DllStats> = BTreeMap::new();
    let mut missing = MissingImports::new();

    for root in &paths {
        for entry in walkdir::WalkDir::new(root) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.depth() == 0 => {
                    return Err(error).context("cannot scan census input");
                }
                Err(error) => {
                    failed.push(error.to_string());
                    continue;
                }
            };
            let path = entry.path();
            if !entry.file_type().is_file() || !looks_like_pe(path) {
                continue;
            }
            // Use the resolved path for deduplication and keep the original for output.
            let identity = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if !seen.insert(identity) {
                continue;
            }
            let info = match std::fs::read(path)
                .map_err(anyhow::Error::from)
                .and_then(|d| ImageInfo::parse(&d).map_err(anyhow::Error::from))
            {
                Ok(info) => info,
                Err(e) => {
                    failed.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            images += 1;
            tally_image(path, &info, &resolver, &mut dlls, &mut missing);
        }
    }

    println!("{images} images scanned, {} unreadable", failed.len());
    for error in &failed {
        println!("  skipped {error}");
    }
    println!(
        "\n{:<56} {:>6} {:>6} {:>6} {:>8} {:>7}",
        "dll", "funcs", "hle", "host", "missing", "images"
    );
    let (mut f, mut h, mut o, mut m) = (0, 0, 0, 0);
    for (dll, s) in &dlls {
        println!(
            "{dll:<56} {:>6} {:>6} {:>6} {:>8} {:>7}",
            s.functions.len(),
            s.hle,
            s.host,
            s.missing,
            s.images.len()
        );
        f += s.functions.len();
        h += s.hle;
        o += s.host;
        m += s.missing;
    }
    println!("{:<56} {f:>6} {h:>6} {o:>6} {m:>8}", "TOTAL");

    if list_missing {
        let mut rows: Vec<_> = missing
            .into_iter()
            .map(|((dll, sym), users)| ((dll, sym.to_string()), users.len()))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        println!("\nmissing (by number of images using it)");
        for ((dll, sym), count) in rows {
            println!("  {count:>4}  {dll}!{sym}");
        }
    }
    Ok(())
}

/// Cheap pre-filter: an `MZ` header, regardless of extension (console
/// packages do not always use .exe/.dll).
fn looks_like_pe(path: &Path) -> bool {
    let mut magic = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"MZ"
}

fn tally_image(
    path: &Path,
    info: &ImageInfo,
    resolver: &HleResolver,
    dlls: &mut BTreeMap<String, DllStats>,
    missing: &mut MissingImports,
) {
    for module in info.imports.iter().chain(&info.delay_imports) {
        let key = module.dll.to_ascii_lowercase();
        let stats = dlls.entry(key.clone()).or_default();
        stats.images.insert(path.to_path_buf());
        for e in &module.entries {
            let class = resolver.classify(&module.dll, &e.symbol);
            let function = FunctionKey::from(&e.symbol);
            if class.is_none() {
                missing
                    .entry((key.clone(), function.clone()))
                    .or_default()
                    .insert(path.to_path_buf());
            }
            // Count each dll!function once across all images.
            if stats.functions.insert(function) {
                match class {
                    Some(Provenance::Hle) => stats.hle += 1,
                    Some(Provenance::Host) => stats.host += 1,
                    Some(Provenance::Title) => unreachable!("census maps no title DLLs"),
                    None => stats.missing += 1,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_pe::{ImportEntry, ImportModule};

    fn entry(symbol: ImportSymbol) -> ImportEntry {
        ImportEntry {
            symbol,
            iat_slot_rva: 0,
        }
    }

    fn named(hint: u16, name: &str) -> ImportEntry {
        entry(ImportSymbol::Name {
            hint,
            name: name.into(),
        })
    }

    fn ordinal(ord: u16) -> ImportEntry {
        entry(ImportSymbol::Ordinal(ord))
    }

    fn synthetic_image(imports: Vec<ImportModule>, delay_imports: Vec<ImportModule>) -> ImageInfo {
        ImageInfo {
            machine: 0,
            characteristics: 0,
            timestamp: 0,
            subsystem: 0,
            dll_characteristics: 0,
            image_base: 0,
            size_of_image: 0,
            entry_rva: 0,
            stack_reserve: 0,
            stack_commit: 0,
            sections: Vec::new(),
            imports,
            delay_imports,
            exports: Vec::new(),
            tls: None,
            has_relocations: false,
            has_exception_table: false,
            has_load_config: false,
            codeview: None,
        }
    }

    #[test]
    fn deduplicates_imports_ignoring_hint_across_images_and_delay_imports() {
        let resolver = HleResolver::new(HostPolicy::none());
        let mut dlls: BTreeMap<String, DllStats> = BTreeMap::new();
        let mut missing = MissingImports::new();

        let image_a = synthetic_image(
            vec![ImportModule {
                dll: "XGameRuntime.dll".into(),
                entries: vec![
                    named(1, "XGameRuntimeInitialize"),
                    named(10, "SyntheticMissingApi"),
                    ordinal(42),
                ],
            }],
            vec![ImportModule {
                dll: "XGameRuntime.dll".into(),
                entries: vec![
                    named(2, "XGameRuntimeInitialize"),
                    named(20, "SyntheticMissingApi"),
                ],
            }],
        );
        let image_b = synthetic_image(
            vec![ImportModule {
                dll: "xgameruntime.dll".into(),
                entries: vec![
                    named(99, "XGameRuntimeInitialize"),
                    named(200, "SyntheticMissingApi"),
                    ordinal(42),
                ],
            }],
            vec![ImportModule {
                dll: "XGameRuntime.dll".into(),
                entries: vec![named(300, "SyntheticMissingApi"), ordinal(42)],
            }],
        );

        tally_image(
            Path::new("image_a.exe"),
            &image_a,
            &resolver,
            &mut dlls,
            &mut missing,
        );
        tally_image(
            Path::new("image_b.dll"),
            &image_b,
            &resolver,
            &mut dlls,
            &mut missing,
        );

        assert_eq!(dlls.len(), 1);
        let stats = &dlls["xgameruntime.dll"];
        assert_eq!(stats.functions.len(), 3);
        assert_eq!(stats.hle, 1);
        assert_eq!(stats.host, 0);
        assert_eq!(stats.missing, 2);
        assert_eq!(stats.images.len(), 2);
        assert_eq!(
            dlls.values().map(|s| s.missing).sum::<usize>(),
            missing.len()
        );
        let user_count = |symbol| missing[&("xgameruntime.dll".into(), symbol)].len();
        assert_eq!(user_count(FunctionKey::Name("SyntheticMissingApi".into())), 2);
        assert_eq!(user_count(FunctionKey::Ordinal(42)), 2);
    }

    #[test]
    fn distinguishes_missing_named_and_ordinal_imports() {
        let resolver = HleResolver::new(HostPolicy::none());
        let mut dlls = BTreeMap::new();
        let mut missing = BTreeMap::new();
        let dll = "census-fixture.dll";
        let module = |entry| ImportModule {
            dll: dll.into(),
            entries: vec![entry],
        };
        let image_a = synthetic_image(vec![module(named(1, "#42"))], vec![module(named(2, "#42"))]);
        let image_b = synthetic_image(vec![module(ordinal(42))], vec![]);
        for (path, image) in [("a.exe", &image_a), ("b.exe", &image_b)] {
            tally_image(Path::new(path), image, &resolver, &mut dlls, &mut missing);
        }

        assert_eq!(dlls[dll].missing, 2);
        let mut rows: Vec<_> = missing
            .into_iter()
            .map(|((_, symbol), users)| (symbol.to_string(), users))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            [
                ("##42".into(), BTreeSet::from([PathBuf::from("a.exe")])),
                ("#42".into(), BTreeSet::from([PathBuf::from("b.exe")])),
            ]
        );
    }

    #[test]
    fn counts_distinct_symbols_and_dlls_separately() {
        let resolver = HleResolver::new(HostPolicy::none());
        let mut dlls: BTreeMap<String, DllStats> = BTreeMap::new();
        let mut missing = MissingImports::new();

        let image = synthetic_image(
            vec![
                ImportModule {
                    dll: "XGameRuntime.dll".into(),
                    entries: vec![
                        named(0, "XGameRuntimeInitialize"),
                        named(0, "SyntheticMissingA"),
                        named(0, "SyntheticMissingB"),
                        ordinal(7),
                        ordinal(8),
                    ],
                },
                ImportModule {
                    dll: "SyntheticExtra.dll".into(),
                    entries: vec![named(0, "SyntheticMissingA"), ordinal(7)],
                },
            ],
            Vec::new(),
        );

        tally_image(
            Path::new("single.exe"),
            &image,
            &resolver,
            &mut dlls,
            &mut missing,
        );

        assert_eq!(dlls.len(), 2);
        let xgr = &dlls["xgameruntime.dll"];
        assert_eq!(xgr.functions.len(), 5);
        assert_eq!(xgr.hle, 1);
        assert_eq!(xgr.missing, 4);
        assert_eq!(xgr.images.len(), 1);

        let extra = &dlls["syntheticextra.dll"];
        assert_eq!(extra.functions.len(), 2);
        assert_eq!(extra.hle, 0);
        assert_eq!(extra.missing, 2);
        assert_eq!(extra.images.len(), 1);

        assert_eq!(
            dlls.values().map(|s| s.missing).sum::<usize>(),
            missing.len()
        );
    }
}
