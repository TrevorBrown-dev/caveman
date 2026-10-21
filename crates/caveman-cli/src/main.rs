//! `caveman`: builds the PNGs in Caveman Decks and Caveman Wheels into ReSkate mods (caveman.py, ported; see lib.rs);
//! `caveman import` copies installed mods' decks into the drop folder (importer.rs).

use caveman_cli::importer::{parse_import_args, run_import, IMPORT_USAGE};
use caveman_cli::report::Reporter;
use caveman_cli::{parse_args, run, run_testclip, USAGE};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "testclip").unwrap_or(false) {
        if let Err(e) = run_testclip(&args[1..]) {
            eprintln!("caveman testclip: {e}");
            std::process::exit(2);
        }
        return;
    }
    if args.first().map(|a| a == "fx").unwrap_or(false) {
        use caveman_cli::fx::{parse_fx_args, run_fx, FX_USAGE};
        let o = match parse_fx_args(&args[1..]) {
            Ok(Some(o)) => o,
            Ok(None) => {
                println!("{FX_USAGE}");
                return;
            }
            Err(e) => {
                eprintln!("caveman fx: {e}\n{FX_USAGE}");
                std::process::exit(2);
            }
        };
        let mut report = Reporter::text();
        if let Err(e) = run_fx(&o, &mut report) {
            eprintln!("caveman fx: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.first().map(|a| a == "import").unwrap_or(false) {
        let a = match parse_import_args(&args[1..]) {
            Ok(Some(a)) => a,
            Ok(None) => {
                println!("{IMPORT_USAGE}");
                return;
            }
            Err(e) => {
                eprintln!("caveman import: {e}\n{IMPORT_USAGE}");
                std::process::exit(2);
            }
        };
        let mut report = if a.json { Reporter::json() } else { Reporter::text() };
        if let Err(e) = run_import(&a, &mut report) {
            report.say(&format!("caveman import: {e}"), serde_json::json!({"event": "error", "message": e.to_string()}));
            std::process::exit(1);
        }
        return;
    }
    let options = match parse_args(&args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{USAGE}");
            return;
        }
        Err(e) => {
            eprintln!("caveman: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let mut report = if options.json { Reporter::json() } else { Reporter::text() };
    if let Err(e) = run(&options, &mut report) {
        report.say(&format!("caveman: {e}"), serde_json::json!({"event": "error", "message": e.to_string()}));
        std::process::exit(1);
    }
}
