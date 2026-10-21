//! `caveman import`: the decks installed mods add, as PNGs (and their own masks, normal maps and holo maps, with a
//! settings file each) in the drop folder, one folder per pack (caveman_core::import). Read-only on the mods: they stay
//! installed until their owner removes them.

use std::path::PathBuf;
use std::sync::Arc;

use caveman_core::game::Game;
use caveman_core::import::{self, DeckStatus, ImportOptions, PackReport};
use serde_json::json;

use crate::names::clean_name;
use crate::report::Reporter;
use crate::{Result, DEFAULT_GAME};

/// The folder imports go into, under the decks folder.
pub const IMPORTED: &str = "Imported";

/// What to import, from where, to where.
#[derive(Clone, Debug)]
pub struct ImportArgs {
    pub game: PathBuf,
    /// The decks folder (default `<game>\Caveman Decks`); packs go into its `Imported` subfolder unless `to` is given.
    pub decks: Option<PathBuf>,
    /// Mod folders (a path, or a folder name in `<game>\Mods`).
    pub mods: Vec<String>,
    /// Every installed mod (except Caveman's own).
    pub all: bool,
    pub to: Option<PathBuf>,
    /// Overwrite decks imported before.
    pub force: bool,
    pub json: bool,
}

pub const IMPORT_USAGE: &str = "usage: caveman import (--mod FOLDER ... | --all) [--to DIR] [--force] [--game DIR] [--decks DIR] [--json]

Copies the decks of installed mods into the drop folder as PNGs (default <decks>\\Imported\\<pack>\\<deck>.png), with
the deck's own mask, normal map and holo map in .layers\\ and a settings file naming them, so Caveman builds them as
they were. --mod takes a folder in <game>\\Mods (or a path) and can be given more than once; --all imports every mod
that adds decks. The mods are only read. Decks imported before are left as they are unless --force.";

/// Reads `caveman import`'s arguments (after the word `import`). `Ok(None)` for `--help`.
pub fn parse_import_args(args: &[String]) -> std::result::Result<Option<ImportArgs>, String> {
    let mut o = ImportArgs { game: PathBuf::from(DEFAULT_GAME), decks: None, mods: Vec::new(), all: false, to: None, force: false, json: false };
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
            "--decks" => o.decks = Some(PathBuf::from(value()?)),
            "--mod" => o.mods.push(value()?),
            "--all" => o.all = true,
            "--to" => o.to = Some(PathBuf::from(value()?)),
            "--force" => o.force = true,
            "--json" => o.json = true,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    if o.mods.is_empty() && !o.all {
        return Err("give --mod FOLDER or --all".into());
    }
    Ok(Some(o))
}

/// Whether a mod folder was built by Caveman (its reskate-build.json names Caveman): never imported from.
fn caveman_built(dir: &std::path::Path) -> bool {
    std::fs::read_to_string(dir.join("reskate-build.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("tool").and_then(|t| t.as_str()).map(|t| t == "Caveman"))
        .unwrap_or(false)
}

/// Runs an import; returns the packs' reports.
pub fn run_import(a: &ImportArgs, report: &mut Reporter) -> Result<Vec<PackReport>> {
    let mods_dir = a.game.join("Mods");
    let to = a.to.clone().unwrap_or_else(|| a.decks.clone().unwrap_or_else(|| a.game.join("Caveman Decks")).join(IMPORTED));
    let mut dirs: Vec<PathBuf> = Vec::new();
    if a.all {
        dirs.extend(import::mods_with_items(&mods_dir, &[]).into_iter().filter(|d| !caveman_built(d)));
    }
    for m in &a.mods {
        let p = PathBuf::from(m);
        let dir = if p.is_dir() { p } else { mods_dir.join(m) };
        if !dir.is_dir() {
            return Err(format!("no mod folder {}", dir.display()).into());
        }
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    let game = Arc::new(Game::open(&a.game)?);
    let (mask, normal) = caveman_core::build::default_layers(&game)?;
    let opts = ImportOptions { to: to.clone(), force: a.force, defaults: Some(Arc::new((mask.to_rgb(), normal.to_rgb()))) };
    let clean = |s: &str| clean_name(s);
    let mut packs = Vec::new();
    let (mut total, mut imported, mut there, mut skipped) = (0, 0, 0, 0);
    for dir in dirs {
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let pack = match import::import_mod(&game, &dir, &opts, &clean) {
            Ok(p) => p,
            Err(e) => {
                let text = format!("{folder}: could not be read ({e})");
                report.say(&text, json!({"event": "pack_failed", "mod": folder, "message": e.to_string()}));
                continue;
            }
        };
        if pack.decks.is_empty() {
            continue;
        }
        let (n_imp, n_there) = (pack.count(|s| *s == DeckStatus::Imported), pack.count(|s| *s == DeckStatus::AlreadyThere));
        let n_skip = pack.decks.len() - n_imp - n_there;
        let with = |what: &str| pack.decks.iter().filter(|d| d.status == DeckStatus::Imported && d.carried.iter().any(|c| c.starts_with(what))).count();
        let (masks, normals, holos) = (with("mask"), with("normal"), with("holo"));
        let mut text = format!("{} ({folder}): {} deck(s): {n_imp} imported", pack.pack, pack.decks.len());
        if n_there > 0 {
            text.push_str(&format!(", {n_there} already there"));
        }
        if n_skip > 0 {
            text.push_str(&format!(", {n_skip} skipped"));
        }
        let extras: Vec<String> = [(masks, "own mask"), (normals, "normal map"), (holos, "holo")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, w)| format!("{n} with {w}"))
            .collect();
        if !extras.is_empty() {
            text.push_str(&format!(" ({})", extras.join(", ")));
        }
        report.say(
            &text,
            json!({"event": "pack", "pack": pack.pack, "mod": folder, "folder": pack.folder.display().to_string(),
                   "decks": pack.decks.len(), "imported": n_imp, "already_there": n_there, "skipped": n_skip,
                   "masks": masks, "normal_maps": normals, "holo": holos}),
        );
        for d in &pack.decks {
            let status = match &d.status {
                DeckStatus::Imported => "imported".to_string(),
                DeckStatus::AlreadyThere => "already there".to_string(),
                DeckStatus::Skipped(why) => format!("skipped: {why}"),
            };
            let line_needed = !d.notes.is_empty() || !matches!(d.status, DeckStatus::Imported);
            let text = if d.notes.is_empty() { format!("  {}: {status}", d.name) } else { format!("  {}: {status} ({})", d.name, d.notes.join("; ")) };
            let event = json!({"event": "deck", "pack": pack.pack, "item": d.item, "display": d.display, "name": d.name,
                               "png": d.png.display().to_string(), "status": status, "carried": d.carried, "notes": d.notes,
                               "width": d.size.0, "height": d.size.1});
            if line_needed || report.is_json() {
                report.say(&text, event);
            }
        }
        total += pack.decks.len();
        imported += n_imp;
        there += n_there;
        skipped += n_skip;
        packs.push(pack);
    }
    let text = format!("Imported {imported} of {total} deck(s) from {} pack(s) into {} ({there} already there, {skipped} skipped)", packs.len(), to.display());
    report.say(&text, json!({"event": "imported", "to": to.display().to_string(), "packs": packs.len(), "decks": total, "imported": imported, "already_there": there, "skipped": skipped}));
    Ok(packs)
}
