//! A stand-in for `gcc -E` (the C preprocessor), installed as `gcc.exe` beside MinGW's `windres.exe` in Caveman's
//! build toolchain (app/toolchain/README.md). On the GNU Rust toolchain, Tauri compiles its Windows resource script
//! (version info, icon, manifest) with windres, which runs `gcc -E` first; Rust's own MinGW kit has no C compiler.
//! Tauri's script needs no preprocessing (its one directive, `#pragma code_page`, is read by windres itself), so this
//! prints the input file unchanged. Anything but `-E` fails, so it can never be mistaken for a compiler.

use std::io::{Read, Write};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "-E") {
        eprintln!("gcc (Caveman's preprocessor stand-in): only `-E` is supported, got {args:?}");
        std::process::exit(1);
    }
    let input = args.iter().rev().find(|a| !a.starts_with('-') && Path::new(a.as_str()).is_file());
    let data = match input {
        Some(path) => std::fs::read(path),
        None => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf).map(|_| buf)
        }
    };
    match data {
        Ok(bytes) => {
            let mut out = std::io::stdout();
            if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("gcc (Caveman's preprocessor stand-in): {e}");
            std::process::exit(1);
        }
    }
}
