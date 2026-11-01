//! EBX round trips over corpora dumped from the installed game by the Python toolkit (the reference): every unique
//! game EBX (`game.pack`) and every mod-owned EBX of the installed cosmetic mods (`mods.pack`); plus Python's compact
//! rewrites (`compact.pack`: preserve_layout=False) to compare byte for byte. Pack format: repeated records of
//! u32 length + bytes (name, then the documents). The corpora live outside the repository; set
//! CAVEMAN_EBX_CORPUS to their folder (see the session scratchpad's `ebxport/`). Run with `--ignored --nocapture`.

use std::path::PathBuf;

use caveman_core::ebx;

fn corpus_dir() -> PathBuf {
    std::env::var_os("CAVEMAN_EBX_CORPUS").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(
            r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\ebxport\corpus",
        )
    })
}

/// Records of a pack file, each `fields` byte strings long.
fn read_pack(name: &str, fields: usize) -> Vec<Vec<Vec<u8>>> {
    let path = corpus_dir().join(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let mut record = Vec::with_capacity(fields);
        for _ in 0..fields {
            let n = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            record.push(data[at + 4..at + 4 + n].to_vec());
            at += 4 + n;
        }
        out.push(record);
    }
    out
}

fn first_difference(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()))
}

fn round_trip(pack: &str) -> (usize, usize, usize) {
    let records = read_pack(pack, 2);
    let (mut identical, mut different, mut failed) = (0, 0, 0);
    let mut examples = Vec::new();
    let mut listing = Vec::new();
    for r in &records {
        let name = String::from_utf8_lossy(&r[0]).into_owned();
        let data = &r[1];
        match ebx::read_document(data).and_then(|d| ebx::write_document(&d)) {
            Ok(out) if &out == data => identical += 1,
            Ok(out) => {
                different += 1;
                listing.push(name.clone());
                if examples.len() < 10 {
                    examples.push(format!(
                        "different: {name}: first difference at {:#x} (sizes {} -> {})",
                        first_difference(&out, data),
                        data.len(),
                        out.len()
                    ));
                }
            }
            Err(e) => {
                failed += 1;
                listing.push(name.clone());
                if examples.len() < 10 {
                    examples.push(format!("failed: {name}: {e}"));
                }
            }
        }
    }
    println!("{pack}: {} EBX | identical {identical}, different {different}, failed {failed}", records.len());
    for e in examples {
        println!("   {e}");
    }
    // the names that did not round trip, for the Python comparison script
    let _ = std::fs::write(corpus_dir().join(format!("{pack}.rust-not-identical.txt")), listing.join("\n"));
    (identical, different, failed)
}

/// Every document Rust writes back identically, Python does too and vice versa: the set that does not round trip
/// equals Python's (`<pack>.python-not-identical.txt`, from the scratchpad's `ebxport/python_full.py`); those match
/// Python's bytes (non_identical_*_match_python). Without Python's list, every document must round trip.
fn check_against_python(pack: &str) {
    let (_, different, failed) = round_trip(pack);
    assert_eq!(failed, 0, "{pack}: documents Rust could not read or write");
    let listing = |name: &str| -> Option<std::collections::BTreeSet<String>> {
        let text = std::fs::read_to_string(corpus_dir().join(name)).ok()?;
        Some(text.lines().map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.is_empty()).collect())
    };
    match listing(&format!("{pack}.python-not-identical.txt")) {
        Some(python) => {
            let rust = listing(&format!("{pack}.rust-not-identical.txt")).unwrap_or_default();
            assert_eq!(rust.len(), different);
            assert!(rust == python, "{pack}: Rust and Python disagree on which documents round trip");
            println!("{pack}: the {different} documents that do not round trip are exactly Python's");
        }
        None => assert_eq!(different, 0),
    }
}

#[test]
#[ignore]
fn game_ebx_round_trip() {
    check_against_python("game.pack");
}

#[test]
#[ignore]
fn mod_ebx_round_trip() {
    check_against_python("mods.pack");
}

/// The EBX that do not round trip byte for byte (Python's writer does not either: ECS prefabs, UI features, state
/// machines): Rust writes exactly what Python writes. `<pack>.python-out.pack` holds Python's outputs (name, bytes).
fn matches_python(pack: &str) -> (usize, usize) {
    let originals: std::collections::HashMap<Vec<u8>, Vec<u8>> =
        read_pack(pack, 2).into_iter().map(|mut r| (std::mem::take(&mut r[0]), std::mem::take(&mut r[1]))).collect();
    let python = read_pack(&format!("{pack}.python-out.pack"), 2);
    let (mut same, mut differ) = (0, 0);
    for r in &python {
        let name = String::from_utf8_lossy(&r[0]).into_owned();
        let out = ebx::read_document(&originals[&r[0]]).and_then(|d| ebx::write_document(&d)).unwrap_or_default();
        if out == r[1] {
            same += 1;
        } else {
            differ += 1;
            if differ <= 10 {
                println!(
                    "   different from Python: {name}: first difference at {:#x} (python {} rust {})",
                    first_difference(&out, &r[1]),
                    r[1].len(),
                    out.len()
                );
            }
        }
    }
    println!("{pack}: {} EBX that do not round trip | Rust output same as Python's {same}, different {differ}", python.len());
    (same, differ)
}

#[test]
#[ignore]
fn non_identical_game_ebx_match_python() {
    assert_eq!(matches_python("game.pack").1, 0);
}

#[test]
#[ignore]
fn non_identical_mod_ebx_match_python() {
    assert_eq!(matches_python("mods.pack").1, 0);
}

/// preserve_layout=False: Rust's compact rewrite equals Python's byte for byte.
#[test]
#[ignore]
fn compact_rewrite_matches_python() {
    let records = read_pack("compact.pack", 3);
    let (mut same, mut differ) = (0, 0);
    for r in &records {
        let name = String::from_utf8_lossy(&r[0]).into_owned();
        let doc = ebx::read_document(&r[1]).unwrap_or_else(|e| panic!("{name}: {e}"));
        let out = ebx::write_document_with(&doc, false, false).unwrap_or_else(|e| panic!("{name}: {e}"));
        if out == r[2] {
            same += 1;
        } else {
            differ += 1;
            if differ <= 10 {
                println!("   different: {name}: first difference at {:#x} (python {} rust {})", first_difference(&out, &r[2]), r[2].len(), out.len());
            }
        }
    }
    println!("compact.pack: {} EBX | same as Python {same}, different {differ}", records.len());
    assert_eq!(differ, 0);
}
