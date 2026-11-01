//! The player's ReSkate profile, read only: which board parts are equipped (the deck, grip colour, grip cutout,
//! trucks and wheels), for the board cam window's model. ReSkate keeps the profile in a SQLite file
//! (`%LOCALAPPDATA%\ReSkate\profiles\<profile>\reskate.sqlite3`, WAL mode): `cosmetic_loadouts` holds the presets
//! (`default`, `Loadout02`...), each a JSON list of recipes (the skater, the board, the gestures), and
//! `profile_extensions`' `customization` section says which preset is selected. The board recipe's items are the
//! item keys (`Own_WheelColor_Gen_Classic_00005`) in slots; the slots are told apart by the keys' prefixes.
//!
//! The file is read whole, with its write-ahead log, by `crate::sqlite` (nothing locked, nothing written) while the
//! game may be writing it; a read that fails (missing, torn, odd) keeps the last good loadout, else the stock one,
//! and the caller says so.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use caveman_core::boardmodel::Loadout;
use serde_json::Value;

use crate::sqlite::Db;

/// Where ReSkate keeps its profiles.
pub fn profiles_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("ReSkate").join("profiles"))
}

/// The newest profile database (`offline` as a rule).
pub fn profile_db() -> Option<PathBuf> {
    let dir = profiles_dir()?;
    let mut found: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path().join("reskate.sqlite3");
            let m = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((m, p))
        })
        .collect();
    found.sort_by_key(|x| std::cmp::Reverse(x.0));
    found.into_iter().next().map(|(_, p)| p)
}

/// (size, modified) of the database and its write-ahead log: what changes when the profile does.
pub fn stamp(db: &Path) -> (u64, Option<SystemTime>, u64, Option<SystemTime>) {
    let m = |p: &Path| std::fs::metadata(p).map(|m| (m.len(), m.modified().ok())).unwrap_or((0, None));
    let (a, b) = m(db);
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    let (c, d) = m(&wal);
    (a, b, c, d)
}

/// The slot a board item fills, by its key's prefix.
fn slot_of(key: &str) -> Option<&'static str> {
    let k = key.to_lowercase();
    if k.starts_with("own_deckgripcolor_") {
        Some("grip")
    } else if k.starts_with("own_deckgripcutout_") {
        Some("cutout")
    } else if k.starts_with("own_truck_") {
        Some("trucks")
    } else if k.starts_with("own_wheelcolor_") || k.starts_with("own_wheelgraphic_") {
        Some("wheels")
    } else if k.starts_with("own_sticker") || k.is_empty() {
        None
    } else {
        Some("deck")
    }
}

/// The board recipe's items as a loadout; None when no recipe looks like a board.
fn loadout_of_recipes(recipes: &Value) -> Option<Loadout> {
    let list = recipes.as_array()?;
    for recipe in list {
        let items = recipe.get("items").and_then(|i| i.as_array())?;
        let keys: Vec<&str> = items.iter().filter_map(|it| it.get("asset").and_then(|a| a.as_str())).collect();
        if !keys.iter().any(|k| matches!(slot_of(k), Some("trucks") | Some("wheels") | Some("grip"))) {
            continue;
        }
        let mut out = Loadout::default();
        for k in keys {
            match slot_of(k) {
                Some("grip") => out.grip = k.to_string(),
                Some("cutout") => out.cutout = k.to_string(),
                Some("trucks") => out.trucks = k.to_string(),
                Some("wheels") => out.wheels = k.to_string(),
                Some("deck") if out.deck.is_empty() => out.deck = k.to_string(),
                _ => {}
            }
        }
        return Some(out);
    }
    None
}

/// Reads the selected preset's board loadout from the profile database.
pub fn read_loadout(db: &Path) -> Result<Loadout, String> {
    let data = Db::open(db)?;
    // profile_extensions (section, value_json): the selected preset
    let selected: usize = data
        .rows(data.table_root("profile_extensions")?)?
        .iter()
        .find(|r| r.first().and_then(|v| v.as_text()) == Some("customization"))
        .and_then(|r| r.get(1).and_then(|v| v.as_text()))
        .and_then(|j| serde_json::from_str::<Value>(j).ok())
        .and_then(|v| v.get("selected_preset_index").and_then(|i| i.as_u64()))
        .unwrap_or(0) as usize;
    // cosmetic_loadouts (preset_id, recipe_format, recipes_json, extensions_json)
    let mut rows: Vec<(String, String)> = data
        .rows(data.table_root("cosmetic_loadouts")?)?
        .into_iter()
        .filter_map(|r| Some((r.first()?.as_text()?.to_string(), r.get(2)?.as_text()?.to_string())))
        .collect();
    if rows.is_empty() {
        return Err("the profile has no loadouts".into());
    }
    // `default` first, the numbered ones after it, as the game lists them
    rows.sort_by_key(|a| (a.0 != "default", a.0.to_lowercase()));
    let (id, json) = rows.get(selected).or(rows.first()).cloned().unwrap();
    let recipes: Value = serde_json::from_str(&json).map_err(|e| format!("loadout {id}: {e}"))?;
    loadout_of_recipes(&recipes).ok_or_else(|| format!("loadout {id} has no board recipe"))
}

/// The current loadout, read again only when the profile files changed; a failed read keeps the last good one.
#[derive(Default)]
pub struct LoadoutCache {
    last: Mutex<Option<((u64, Option<SystemTime>, u64, Option<SystemTime>), PathBuf, Result<Loadout, String>)>>,
}

impl LoadoutCache {
    /// The equipped loadout, or why it could not be read (the caller falls back to the stock board).
    pub fn current(&self) -> Result<Loadout, String> {
        let db = profile_db().ok_or_else(|| "no ReSkate profile database".to_string())?;
        let now = stamp(&db);
        let mut slot = self.last.lock().unwrap();
        if let Some((st, path, result)) = slot.as_ref() {
            if *st == now && *path == db {
                return result.clone();
            }
        }
        let mut result = read_loadout(&db);
        if let (Err(why), Some((_, _, Ok(last)))) = (&result, slot.as_ref()) {
            eprintln!("loadout: {why}; keeping the last one read");
            result = Ok(last.clone());
        }
        *slot = Some((now, db, result.clone()));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_by_prefix() {
        assert_eq!(slot_of("Own_DeckGripColor_Gen_Popsicle_00001"), Some("grip"));
        assert_eq!(slot_of("Own_WheelColor_Gen_Classic_00005"), Some("wheels"));
        assert_eq!(slot_of("Own_WheelGraphic_Gen_Classic_00006"), Some("wheels"));
        assert_eq!(slot_of("Own_DeckGripCutout_Gen_Popsicle_00001"), Some("cutout"));
        assert_eq!(slot_of("Own_Truck_Gen_Default_00006"), Some("trucks"));
        assert_eq!(slot_of("Own_000_-_The_Caveman"), Some("deck"));
        assert_eq!(slot_of("Own_DeckGraphic_Real_Popsicle_00001"), Some("deck"));
        assert_eq!(slot_of(""), None);
    }

    #[test]
    fn board_recipe_is_found() {
        let recipes: Value = serde_json::json!([
            { "items": [{ "asset": "Own_CustHead_00002", "slot": 1 }], "template_key": 1 },
            { "items": [
                { "asset": "Own_DeckGripColor_Gen_Popsicle_00001", "slot": 1969053408 },
                { "asset": "Own_WheelColor_Gen_Classic_00005", "slot": 3359357262u64 },
                { "asset": "Own_000_-_The_Caveman", "slot": 1711740456 },
                { "asset": "Own_DeckGripCutout_Gen_Popsicle_00001", "slot": 2540640452u64 },
                { "asset": "Own_Truck_Gen_Default_00006", "slot": 182294718 },
                { "asset": "", "slot": 894600414 }
            ], "template_key": 1583459055 }
        ]);
        let l = loadout_of_recipes(&recipes).unwrap();
        assert_eq!(l.deck, "Own_000_-_The_Caveman");
        assert_eq!(l.grip, "Own_DeckGripColor_Gen_Popsicle_00001");
        assert_eq!(l.cutout, "Own_DeckGripCutout_Gen_Popsicle_00001");
        assert_eq!(l.trucks, "Own_Truck_Gen_Default_00006");
        assert_eq!(l.wheels, "Own_WheelColor_Gen_Classic_00005");
    }

    /// Reads this machine's profile (ignored: needs ReSkate's profile).
    #[test]
    #[ignore]
    fn reads_the_profile() {
        let db = profile_db().expect("a profile database");
        let l = read_loadout(&db).unwrap();
        println!("{}: {l:?}", db.display());
        assert!(!l.trucks.is_empty());
    }
}
