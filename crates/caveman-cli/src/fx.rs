//! `caveman fx`: builds Caveman FX's mod (`Caveman-FX`: the fx states and the test decks, caveman_core::fxmod) for
//! Trevor's stream branch. It never touches Caveman-Decks or Caveman-Shaders. The decks only come alive with
//! `caveman_fx.dll` in the game (`caveman-trainer inject --yes`); without it they show nothing extra.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use caveman_core::fxmod::{self, FxLook, CAVE, CAVEMAN, COMBOS, DECKS, LIBRARY, STATES};
use caveman_core::game::Game;

use crate::items;
use crate::report::Reporter;
use crate::{Result, DEFAULT_GAME};

pub const FX_USAGE: &str = "usage: caveman fx [--game DIR] [--out DIR] [--mod NAME] [--prefix TEXT] [--test-decks] [--no-sprites] [--force]

Builds Caveman FX's mod (default <game>\\Mods\\Caveman-FX): the fx states Animation/Caveman/FX.<Name> and \"The Caveman\"
(a plain white deck whose graphic and effects caveman-trainer's `board` commands change live: the OBS link and the
live board). Caveman-Decks and Caveman-Shaders are not touched. The deck reacts only with caveman_fx.dll in the game
(caveman-trainer). --test-decks adds the particle effect test decks (parked since 2026-10-30): one deck per fx state,
the combo deck and the FX Library decks (\"FXL ...\", all on fx.test); --no-sprites then leaves out the sprite
prototype (FXL Star Burst, the only part that patches globals.toc). Refuses to write into the game folder while
Skate.exe runs (--force overrides); --prefix defaults to \"000 - \" (the decks list first).";

#[derive(Clone, Debug)]
pub struct FxOptions {
    pub game: PathBuf,
    pub out: Option<PathBuf>,
    pub mod_name: String,
    pub prefix: String,
    pub force: bool,
    pub sprites: bool,
    pub test_decks: bool,
}

pub fn parse_fx_args(args: &[String]) -> std::result::Result<Option<FxOptions>, String> {
    let mut o = FxOptions {
        game: PathBuf::from(DEFAULT_GAME),
        out: None,
        mod_name: fxmod::MOD.into(),
        prefix: "000 - ".into(),
        force: false,
        sprites: true,
        test_decks: false,
    };
    let mut i = 0;
    while i < args.len() {
        let (flag, inline) = match args[i].split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (args[i].clone(), None),
        };
        let mut value = || -> std::result::Result<String, String> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(None),
            "--game" => o.game = PathBuf::from(value()?),
            "--out" => o.out = Some(PathBuf::from(value()?)),
            "--mod" => o.mod_name = value()?,
            "--prefix" => o.prefix = value()?,
            "--force" => o.force = true,
            "--no-sprites" => o.sprites = false,
            "--test-decks" => o.test_decks = true,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok(Some(o))
}

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
    fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
}

/// Whether a process with this exe name runs (any case).
pub fn running(exe: &str) -> bool {
    let snap = unsafe { CreateToolhelp32Snapshot(0x2, 0) };
    if snap.is_null() || snap as isize == -1 {
        return false;
    }
    let mut e: ProcessEntry = unsafe { std::mem::zeroed() };
    e.size = std::mem::size_of::<ProcessEntry>() as u32;
    let mut found = false;
    if unsafe { Process32FirstW(snap, &mut e) } != 0 {
        loop {
            let n = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
            if String::from_utf16_lossy(&e.exe[..n]).eq_ignore_ascii_case(exe) {
                found = true;
                break;
            }
            if unsafe { Process32NextW(snap, &mut e) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snap) };
    found
}

fn inside(path: &Path, dir: &Path) -> bool {
    let (p, d) = (std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()), std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()));
    p.to_string_lossy().to_lowercase().starts_with(&d.to_string_lossy().to_lowercase())
}

/// Builds the mod; returns its folder.
pub fn run_fx(o: &FxOptions, report: &mut Reporter) -> Result<PathBuf> {
    let mods_dir = o.out.clone().unwrap_or_else(|| o.game.join("Mods"));
    if inside(&mods_dir, &o.game) && !o.force && running("Skate.exe") {
        return Err(format!(
            "Skate.exe is running: ReSkate reads mods at launch; build into {} with the game closed (or --out elsewhere, or --force)",
            mods_dir.display()
        )
        .into());
    }
    let game = Arc::new(Game::open(&o.game)?);
    let taken = items::existing_items(&game, &o.game.join("Mods"), &o.mod_name, report)?;
    let test_names: Vec<&str> = if o.test_decks {
        DECKS.iter().map(|d| d.name).chain(COMBOS.iter().map(|c| c.name)).chain(LIBRARY.iter().map(|d| d.name)).collect()
    } else {
        Vec::new()
    };
    for name in test_names.into_iter().chain([CAVEMAN]) {
        let key = fxmod::deck_key(&format!("{}{name}", o.prefix)).to_lowercase();
        if let Some(who) = taken.get(&key) {
            return Err(format!("\"{}{name}\" is already an item in {who}: use another --prefix", o.prefix).into());
        }
    }
    let built = {
        let mut log = |line: &str| report.log(line);
        fxmod::build_fx_mod_with(game, &mods_dir, &o.mod_name, &o.prefix, &fxmod::FxModOptions { sprites: o.sprites, test_decks: o.test_decks }, &mut log)?
    };
    let r = &built.report;
    report.say(
        &format!("Built {} FX deck(s) and {} fx states into {} ({} KB)", r.decks.len(), STATES.len() + CAVE.len(), r.mod_dir.display(), r.cas_bytes / 1024),
        serde_json::json!({"event": "fx_built", "mod_dir": r.mod_dir.display().to_string(), "decks": r.decks.len(), "states": STATES.len() + CAVE.len(), "cas_bytes": r.cas_bytes}),
    );
    let (test_decks, combos, library): (&[fxmod::FxDeck], &[fxmod::FxCombo], &[fxmod::FxDeck]) =
        if o.test_decks { (&DECKS, &COMBOS, &LIBRARY) } else { (&[], &[], &[]) };
    for d in test_decks {
        report.log(&format!(
            "  {}{}: fires on fx.{} ({:?}{})",
            o.prefix,
            d.name,
            d.state.to_lowercase(),
            d.usage,
            if d.cooldown > 0.0 { format!(", cooldown {} s", d.cooldown) } else { String::new() }
        ));
    }
    for c in combos {
        let parts: Vec<String> = c.parts.iter().map(|p| format!("{:?} on fx.{}", p.look, p.state.to_lowercase())).collect();
        report.log(&format!("  {}{}: {}", o.prefix, c.name, parts.join(", ")));
    }
    for d in library {
        let what = match d.look {
            FxLook::Stock(bp, at) => format!("{} at the {}", bp.rsplit('/').next().unwrap_or(bp), at.name()),
            FxLook::Own(short, at) => format!("{}ebp_{short} (the mod's own) at the {}", fxmod::OWN_PREFIX, at.name()),
            other => format!("{other:?}"),
        };
        report.log(&format!(
            "  {}{}: {what}, fires on fx.{} ({:?}{})",
            o.prefix,
            d.name,
            d.state.to_lowercase(),
            d.usage,
            if d.cooldown > 0.0 { format!(", cooldown {} s", d.cooldown) } else { String::new() }
        ));
    }
    for c in CAVE {
        report.log(&format!(
            "  {}: fx.cave.{} = {} ({:?}{})",
            fxmod::cave_display(&o.prefix),
            c.name,
            c.what,
            c.usage,
            if c.cooldown > 0.0 { format!(", cooldown {} s", c.cooldown) } else { String::new() }
        ));
    }
    match built.sprite_key {
        Some(k) => report.log(&format!("  sprite material key {k} (globals.toc rows and a depot of the mod's own)")),
        None if o.test_decks => report.log("  built without the sprite prototype (no globals.toc)"),
        None => report.log("  the particle effect test decks are parked (--test-decks brings them back)"),
    }
    Ok(r.mod_dir.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        let a: Vec<String> = ["--game", "G", "--out=O", "--prefix", "", "--force"].iter().map(|s| s.to_string()).collect();
        let o = parse_fx_args(&a).unwrap().unwrap();
        assert_eq!((o.game.clone(), o.out.clone(), o.prefix.as_str(), o.force), (PathBuf::from("G"), Some(PathBuf::from("O")), "", true));
        assert_eq!(o.mod_name, "Caveman-FX");
        assert!(o.sprites, "the sprite prototype is in by default (with the test decks)");
        assert!(!o.test_decks, "the test decks are parked by default");
        assert!(parse_fx_args(&["--test-decks".into()]).unwrap().unwrap().test_decks);
        assert!(!parse_fx_args(&["--no-sprites".into()]).unwrap().unwrap().sprites);
        assert!(parse_fx_args(&["--nope".into()]).is_err());
        assert!(inside(Path::new(r"E:\Games\ReSkate\Mods"), Path::new(r"E:\Games\ReSkate")));
        assert!(!inside(Path::new(r"C:\tmp\Mods"), Path::new(r"E:\Games\ReSkate")));
        assert!(!running("surely-not-a-process-name.exe"));
    }
}
