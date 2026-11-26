//! P.4 harness: device method 0x218's pieces, as dumped by
//! `VICTORY_NATIVE_PSO_DUMP`, put back together as an `FSPX` program (as
//! `victory_gpu::device::native_compute` does) and translated. Prints the
//! errors grouped, most first.
//!
//! cargo run --release -p victory-shader --example p4_fspx -- DUMP_DIR [-v]

use std::collections::BTreeMap;

fn piece(dir: &std::path::Path, word: usize) -> Option<Vec<u8>> {
    let prefix = format!("w{word}_");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let name = e.file_name().into_string().ok()?;
        name.starts_with(&prefix).then(|| std::fs::read(e.path()).ok()).flatten()
    })
}

pub fn assemble(packet: &[u8], hash: &[u8], code: &[u8], meta: &[u8]) -> Vec<u8> {
    let pieces: [(u32, &[u8]); 4] = [(0, packet), (1, hash), (6, code), (7, meta)];
    let mut fspx = vec![0u8; 0x10 + 16 * pieces.len()];
    let mut table = Vec::new();
    for (kind, bytes) in pieces {
        while fspx.len() % 16 != 0 {
            fspx.push(0);
        }
        table.push((kind, fspx.len() as u32, bytes.len() as u32));
        fspx.extend_from_slice(bytes);
    }
    let size = fspx.len() as u32;
    fspx[0..4].copy_from_slice(b"FSPX");
    fspx[4..8].copy_from_slice(&1u32.to_le_bytes());
    fspx[8..12].copy_from_slice(&(pieces.len() as u32).to_le_bytes());
    fspx[12..16].copy_from_slice(&size.to_le_bytes());
    for (i, (kind, offset, len)) in table.into_iter().enumerate() {
        let at = 0x10 + 16 * i;
        fspx[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        fspx[at + 4..at + 8].copy_from_slice(&offset.to_le_bytes());
        fspx[at + 8..at + 12].copy_from_slice(&len.to_le_bytes());
    }
    let total = 0x24 + 8 + fspx.len();
    let mut c = Vec::with_capacity(total);
    c.extend_from_slice(b"DXBC");
    c.extend_from_slice(&[0x11; 16]);
    c.extend_from_slice(&1u16.to_le_bytes());
    c.extend_from_slice(&0u16.to_le_bytes());
    c.extend_from_slice(&(total as u32).to_le_bytes());
    c.extend_from_slice(&1u32.to_le_bytes());
    c.extend_from_slice(&0x24u32.to_le_bytes());
    c.extend_from_slice(b"ACCC");
    c.extend_from_slice(&(fspx.len() as u32).to_le_bytes());
    c.extend_from_slice(&fspx);
    c
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).expect("DUMP_DIR"));
    let verbose = args.iter().any(|a| a == "-v");
    // `--ok FILE`: the code addresses (directory names after `218_`) of the
    // programs that translate, one a line.
    let ok_file = args.iter().position(|a| a == "--ok").and_then(|i| args.get(i + 1)).cloned();
    let mut ok_list = Vec::new();
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut ok, mut total) = (0, 0);
    let mut dirs: Vec<_> = std::fs::read_dir(&dir).expect("dump dir").flatten().map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        let name = d.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("218_") {
            continue;
        }
        total += 1;
        let (Some(packet), Some(hash), Some(code), Some(meta)) = (piece(&d, 1), piece(&d, 3), piece(&d, 5), piece(&d, 7)) else {
            errors.entry("a piece missing from the dump".into()).or_default().push(name);
            continue;
        };
        let c = assemble(&packet, &hash, &code, &meta);
        let parsed = victory_shader::Container::parse(&c)
            .map_err(|e| e.to_string())
            .and_then(|c| victory_shader::native_programs(c.part(victory_shader::FourCC::ACCC).unwrap().data).map_err(|e| format!("fspx: {e}")));
        let result = parsed.and_then(|_| victory_shader::emit::translate(&c).map(|_| ()).map_err(|e| format!("translate: {e}")));
        match result {
            Ok(()) => {
                ok += 1;
                ok_list.push(name.trim_start_matches("218_").to_owned());
            }
            Err(e) => {
                // One group per kind of error: without the code offset.
                let e = match (e.find(" code at "), e.rfind(": ")) {
                    (Some(_), Some(i)) => format!("translate: {}", &e[i + 2..]),
                    _ => e,
                };
                errors.entry(e).or_default().push(name)
            }
        }
    }
    println!("{ok} of {total} translate");
    if let Some(f) = ok_file {
        std::fs::write(f, ok_list.join("\n") + "\n").expect("write --ok file");
    }
    let mut by: Vec<_> = errors.into_iter().collect();
    by.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (e, v) in by {
        println!("{:5}  {e}   e.g. {}", v.len(), v[0]);
        if verbose {
            for n in v.iter().take(5) {
                println!("         {n}");
            }
        }
    }
}
