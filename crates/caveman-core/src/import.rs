//! Deck import: the decks an installed mod adds, as PNGs in Caveman's drop folder (docs/backlog.md "Import decks from
//! installed mods"). New with the Rust engine; read-only on the mod.
//!
//! For every deck item the mod adds (`items/board_bottomart/*`, a `DelMarBaseItemAsset`): its display name from its
//! `Key`, its appearance preset (`AssetPaths[0].AssetName`, found among the mod's bundles), and the preset's slots:
//! the art (`param_graphic_c`), and the deck's own mask (`param_graphic_osk`), normal map (`param_graphic_ny`) and holo
//! map (`param_graphic_rgb`) with its foil mode (`param_foilmode`). Each slot's texture is found by the preset's
//! import (a boxed reference), its resource header gives the chunk, the chunk (the mod's TOC chunk, a whole bundle
//! chunk, or the game's) is decoded at its first mip.
//!
//! Written per pack, into `<to>\<pack>\`: `<Deck>.png` (the art), `.layers\<Deck>.mask.png` / `.normal.png` /
//! `.holo.png` for the slots the deck fills with textures of its own (the game's generic defaults are left out), and
//! `<Deck>.caveman.json` naming the deck and its layers (settings.rs `layers`), so the build uses them as they are.
//! A mask whose preset does not switch metal on (`param_graphic_metalmaskvalue`) is written with its metal channel
//! cleared: Caveman builds own-mask decks on the gold deck's preset, which would switch it on.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bundle::{FileLoc, Manifest};
use crate::ebx::{self, Data, Document};
use crate::effects::FoilMode;
use crate::game::{self, Game, GameError, Layer, ITEMS_TOC};
use crate::image::Image;
use crate::settings::{HoloMode, LayerSettings, Settings};
use crate::{bcdec, texture, toc};

/// The `ParamKey` GUIDs of the slots read (build::SLOT_*), the foil mode and the metal switch.
const KEY_ART: &str = "2259d419-387f-b1ba-30fc-c5140100b012";
const KEY_OSK: &str = "c1b71b8a-395f-e8ce-30fc-c5140100a381";
const KEY_NY: &str = "830b93ff-7cad-e8ee-30fc-c5140100c668";
const KEY_RGB: &str = "698a46f9-c8de-d48e-30fc-c5140100a381";
const KEY_FOILMODE: &str = "3a957fd1-02b6-b993-7e81-4a8e0100a9aa";
const KEY_METAL: &str = "a2189044-0676-ba57-2d5c-b62b0100a211";
/// Where a mod's deck items are.
const DECK_ITEMS: &str = "items/board_bottomart/";
/// Where the layer textures go, beside the deck PNGs (a dot-folder: the drop-folder scan skips it).
pub const LAYERS_DIR: &str = ".layers";

/// An import that cannot go on.
#[derive(Debug)]
pub struct ImportError(pub String);

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImportError {}

macro_rules! from_error {
    ($($t:ty),*) => {$(
        impl From<$t> for ImportError {
            fn from(e: $t) -> ImportError {
                ImportError(e.to_string())
            }
        }
    )*};
}
from_error!(GameError, ebx::Error, texture::TextureError, crate::image::ImageError, std::io::Error, toc::TocError);

type Result<T> = std::result::Result<T, ImportError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(ImportError(message.into()))
}

/// What happened to one deck.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeckStatus {
    Imported,
    /// Its PNG was there already (an earlier import); left as it is.
    AlreadyThere,
    Skipped(String),
}

/// One deck of a pack.
#[derive(Clone, Debug)]
pub struct ImportedDeck {
    /// The item (`items/board_bottomart/own_...`).
    pub item: String,
    /// Its name in game (the item key without `Own_`, underscores as spaces).
    pub display: String,
    /// The deck's name in Caveman (cleaned; a duplicate within the pack gets " 2", " 3").
    pub name: String,
    pub png: PathBuf,
    pub status: DeckStatus,
    /// What came with the art: "mask", "normal map", "holo map (frost)", "metal".
    pub carried: Vec<String>,
    /// What did not carry over, or was changed.
    pub notes: Vec<String>,
    /// The art's size in the mod.
    pub size: (usize, usize),
}

/// One mod's import.
#[derive(Clone, Debug)]
pub struct PackReport {
    pub mod_dir: PathBuf,
    /// The pack's folder name (the mod's manifest name, else its folder name, cleaned).
    pub pack: String,
    pub folder: PathBuf,
    pub decks: Vec<ImportedDeck>,
}

impl PackReport {
    pub fn count(&self, f: impl Fn(&DeckStatus) -> bool) -> usize {
        self.decks.iter().filter(|d| f(&d.status)).count()
    }
}

/// How to import.
#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// The folder packs go into (one subfolder per pack).
    pub to: PathBuf,
    /// Overwrite decks imported before (their PNG, layers and settings file).
    pub force: bool,
    /// The game's default deck mask and normal map (build::default_layers, RGB): a deck's own mask or normal map that
    /// is the same (many packs ship a copy) is left out, so the deck builds on the plain host as it looked.
    pub defaults: Option<Arc<(Image, Image)>>,
}

/// Mean absolute difference per channel value, or None when the sizes differ.
fn mean_difference(a: &Image, b: &Image) -> Option<f64> {
    if (a.width, a.height, a.channels) != (b.width, b.height, b.channels) {
        return None;
    }
    let sum: u64 = a.data.iter().zip(&b.data).map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs() as u64).sum();
    Some(sum as f64 / a.data.len().max(1) as f64)
}

/// Below this mean difference a mask or normal map counts as the game's default (BC7 noise is about 1).
const SAME_AS_DEFAULT: f64 = 2.0;

/// One of the mod's bundles, read.
struct ModBundle {
    name: String,
    manifest: Manifest,
    /// The bundle's files after the manifest (ebx, res, chunks: `Manifest::*_file(i) - 1`).
    files: Vec<FileLoc>,
    /// file GUID -> ebx index, filled on first use.
    guids: Option<HashMap<ebx::Guid, usize>>,
}

impl ModBundle {
    fn ebx_loc(&self, i: usize) -> FileLoc {
        self.files[self.manifest.ebx_file(i) - 1]
    }

    fn res_loc(&self, i: usize) -> FileLoc {
        self.files[self.manifest.res_file(i) - 1]
    }

    fn chunk_loc(&self, i: usize) -> FileLoc {
        self.files[self.manifest.chunk_file(i) - 1]
    }
}

/// A mod opened for import.
struct Mod {
    layer: Layer,
    bundles: Vec<ModBundle>,
    /// The mod's own TOC chunks.
    chunks: HashMap<toc::Guid, toc::TocChunk>,
    /// ebx leaf name (lower case) -> (bundle, ebx index), last wins.
    leaves: HashMap<String, (usize, usize)>,
}

impl Mod {
    fn open(game: &Arc<Game>, root: &Path) -> Result<Mod> {
        let layer = Layer::open(root, game.clone());
        let t = layer.toc(ITEMS_TOC)?;
        let mut bundles = Vec::new();
        for b in &t.bundles {
            let Some(region) = &b.region else { continue };
            let (manifest, files) = layer.bundle(region)?;
            bundles.push(ModBundle { name: b.name.clone(), manifest, files, guids: None });
        }
        let chunks = t.chunks.iter().filter(|c| !c.removed).map(|c| (c.guid, *c)).collect();
        let mut leaves = HashMap::new();
        for (bi, b) in bundles.iter().enumerate() {
            for (i, e) in b.manifest.ebx.iter().enumerate() {
                leaves.insert(e.name.rsplit('/').next().unwrap_or(&e.name).to_lowercase(), (bi, i));
            }
        }
        Ok(Mod { layer, bundles, chunks, leaves })
    }

    fn ebx(&self, bi: usize, i: usize) -> Result<Vec<u8>> {
        Ok(self.layer.decode(&self.bundles[bi].ebx_loc(i))?)
    }

    fn doc(&self, bi: usize, i: usize) -> Result<Document> {
        Ok(ebx::read_document(&self.ebx(bi, i)?)?)
    }

    /// The (bundle, ebx index) of the document with this file GUID: in bundle `first`, else in any of the mod's.
    fn find_file(&mut self, first: usize, guid: ebx::Guid) -> Option<(usize, usize)> {
        let order: Vec<usize> = std::iter::once(first).chain((0..self.bundles.len()).filter(|&b| b != first)).collect();
        for bi in order {
            if self.bundles[bi].guids.is_none() {
                let mut map = HashMap::new();
                for i in 0..self.bundles[bi].manifest.ebx.len() {
                    if let Ok(bytes) = self.ebx(bi, i) {
                        if let Ok(g) = ebx::read_file_guid(&bytes) {
                            map.insert(g, i);
                        }
                    }
                }
                self.bundles[bi].guids = Some(map);
            }
            if let Some(&i) = self.bundles[bi].guids.as_ref().and_then(|m| m.get(&guid)) {
                return Some((bi, i));
            }
        }
        None
    }

    /// A texture's first mip as an image (RGBA), with whether the texture is the mod's own (else the game's) and its
    /// name.
    fn texture(&self, bi: usize, ebx_index: usize, game: &Game) -> Result<(Image, bool, String)> {
        let b = &self.bundles[bi];
        let name = b.manifest.ebx[ebx_index].name.clone();
        let own = b.ebx_loc(ebx_index).patch;
        let ri = b.manifest.res_index(&name).ok_or_else(|| ImportError(format!("{name}: no texture resource beside it")))?;
        let payload = self.layer.decode(&b.res_loc(ri))?;
        let h = texture::TextureHeader::parse(&payload, &b.manifest.res[ri].meta)?;
        let format = bcdec::BcFormat::from_frostbite(h.format).ok_or_else(|| ImportError(format!("{name}: texture format {} not supported", h.format)))?;
        let guid = toc::Guid::from_le_bytes(h.chunk);
        let data = if let Some(c) = self.chunks.get(&guid) {
            self.layer.decode(&game::chunk_loc(c))?
        } else if let Some(ci) = b.manifest.chunks.iter().position(|c| c.guid == guid && c.logical_offset == 0) {
            self.layer.decode(&b.chunk_loc(ci))?
        } else {
            game.chunk_data(&guid, ITEMS_TOC)?
        };
        let (w, ht) = (h.width as usize, h.height as usize);
        let mip0 = w.div_ceil(4).max(1) * ht.div_ceil(4).max(1) * format.block_bytes();
        if data.len() < mip0 {
            return fail(format!("{name}: its chunk holds no first mip ({} of {mip0} bytes)", data.len()));
        }
        Ok((bcdec::decode(format, &data[..mip0], w, ht), own, name))
    }
}

/// The pack's name: the mod's manifest.json `name`, else its folder name (Thunderstore's `Owner-Name`: the name).
pub fn pack_name(mod_dir: &Path) -> String {
    let from_manifest = std::fs::read_to_string(mod_dir.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).ok())
        .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
        .filter(|s| !s.trim().is_empty());
    from_manifest.unwrap_or_else(|| {
        let folder = mod_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        folder.split_once('-').map(|(_, rest)| rest.to_string()).unwrap_or(folder)
    })
}

/// The deck's name in game from its item key: `Own_Cool_Deck` -> "Cool Deck".
pub fn display_from_key(key: &str) -> String {
    let rest = if key.len() >= 4 && key[..4].eq_ignore_ascii_case("own_") { &key[4..] } else { key };
    rest.replace('_', " ")
}

/// The import of every deck one mod adds. `clean` turns a name into a deck/file name (the CLI's clean_name).
pub fn import_mod(game: &Arc<Game>, mod_dir: &Path, opts: &ImportOptions, clean: &dyn Fn(&str) -> String) -> Result<PackReport> {
    let pack = clean(&pack_name(mod_dir));
    let pack = if pack.is_empty() { clean(&mod_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) } else { pack };
    let folder = opts.to.join(&pack);
    let mut report = PackReport { mod_dir: mod_dir.to_path_buf(), pack, folder: folder.clone(), decks: Vec::new() };
    if !mod_dir.join("Win32").join(ITEMS_TOC).is_file() {
        return Ok(report);
    }
    let mut m = Mod::open(game, mod_dir)?;
    let mut items = Vec::new();
    for (bi, b) in m.bundles.iter().enumerate() {
        if !b.name.ends_with("cas_main_sharedbundle") {
            continue;
        }
        for (i, e) in b.manifest.ebx.iter().enumerate() {
            if b.ebx_loc(i).patch && e.name.starts_with(DECK_ITEMS) {
                items.push((bi, i, e.name.clone()));
            }
        }
    }
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    for (bi, i, item) in items {
        let mut deck = ImportedDeck {
            item: item.clone(),
            display: String::new(),
            name: String::new(),
            png: PathBuf::new(),
            status: DeckStatus::Imported,
            carried: Vec::new(),
            notes: Vec::new(),
            size: (0, 0),
        };
        match import_deck(game, &mut m, bi, i, &folder, opts, clean, &mut names, &mut deck) {
            Ok(()) => {}
            Err(e) => deck.status = DeckStatus::Skipped(e.0),
        }
        report.decks.push(deck);
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn import_deck(
    game: &Arc<Game>,
    m: &mut Mod,
    bi: usize,
    i: usize,
    folder: &Path,
    opts: &ImportOptions,
    clean: &dyn Fn(&str) -> String,
    names: &mut BTreeMap<String, usize>,
    deck: &mut ImportedDeck,
) -> Result<()> {
    let doc = m.doc(bi, i)?;
    if doc.root_type != "DelMarBaseItemAsset" {
        return fail(format!("not an item ({})", doc.root_type));
    }
    let key = doc.get("Key", 0)?.as_str().unwrap_or_default();
    deck.display = display_from_key(&key);
    let base = clean(&deck.display);
    if base.is_empty() {
        return fail(format!("no usable letters or digits in its name {:?}", deck.display));
    }
    // a unique name within the pack
    let n = names.entry(base.to_lowercase()).or_insert(0);
    *n += 1;
    deck.name = if *n == 1 { base.clone() } else { format!("{base} {n}") };
    if *n > 1 {
        deck.notes.push(format!("another deck of the pack is called \"{base}\": named \"{}\"", deck.name));
    }
    deck.png = folder.join(format!("{}.png", deck.name));
    let data = *doc.instances_of("DelMarCustomizationItemData").first().ok_or_else(|| ImportError("no item data".into()))?;
    let asset = doc.get("AssetPaths[0].AssetName", data)?.as_str().unwrap_or_default().to_lowercase();
    if asset.starts_with("reskateboard1_") {
        deck.notes.push("a custom board mesh (ReSkate Studio): only the art came over, on the standard deck".into());
    }
    if asset.starts_with("lenticular_") {
        deck.notes.push("a deck-tool effect deck (lenticular/animated): only its texture came over, as plain art".into());
    }
    let (pbi, pi) = *m.leaves.get(&asset).ok_or_else(|| ImportError(format!("its preset {asset} is not in the mod")))?;
    let preset = m.doc(pbi, pi)?;

    // the preset's slots
    let mut slots: HashMap<&'static str, ebx::Guid> = HashMap::new();
    let mut foilmode = None;
    let mut metal = false;
    let infos = preset.get("ParamInfo", 0)?.as_array().cloned().unwrap_or_default();
    let import_offsets: std::collections::HashSet<usize> = preset.import_offsets.iter().copied().collect();
    for v in &infos {
        let Some(info) = v.data.as_object() else { continue };
        let Ok(key) = info.get("ParamKey.Data") else { continue };
        let Some(key) = key.as_guid() else { continue };
        let key = key.to_string();
        let Ok(value) = info.get("Value.Value") else { continue };
        let slot = match key.as_str() {
            KEY_ART => "art",
            KEY_OSK => "osk",
            KEY_NY => "ny",
            KEY_RGB => "rgb",
            KEY_FOILMODE => {
                foilmode = foil_mode(&preset);
                continue;
            }
            KEY_METAL => {
                metal = boxed_f32(&preset, value).map(|x| x > 0.0).unwrap_or(false);
                continue;
            }
            _ => continue,
        };
        if let Some(idx) = boxed_import(&preset, value, &import_offsets) {
            if let Some(&(file, _)) = preset.imports.get(idx) {
                slots.insert(slot, file);
            }
        }
    }
    let art_file = *slots.get("art").ok_or_else(|| ImportError("its preset has no art (param_graphic_c)".into()))?;
    let (abi, ai) = m.find_file(pbi, art_file).ok_or_else(|| ImportError(format!("the art texture {art_file} is not in the mod")))?;
    let (art, _, _) = m.texture(abi, ai, game)?;
    deck.size = (art.width, art.height);
    if (art.width, art.height) != (512, 2048) {
        deck.notes.push(format!("art {} x {} (decks are 512 x 2048: Caveman fits it)", art.width, art.height));
    }

    // the other slots, when the deck has textures of its own there
    let mut layers = LayerSettings::default();
    let mut images: Vec<(&'static str, String, Image)> = Vec::new();
    for (slot, suffix, what) in [("osk", "mask", "mask"), ("ny", "normal", "normal map"), ("rgb", "holo", "holo map")] {
        let Some(&file) = slots.get(slot) else { continue };
        let Some((tbi, ti)) = m.find_file(pbi, file) else {
            deck.notes.push(format!("its {what} is not in the mod (a game texture elsewhere): left out"));
            continue;
        };
        let (img, own, name) = match m.texture(tbi, ti, game) {
            Ok(x) => x,
            Err(e) => {
                deck.notes.push(format!("its {what}: {e}: left out"));
                continue;
            }
        };
        if !own && (name.contains("/defaults/") || name.contains("/generic/popsicle/")) {
            continue; // the game's generic default: Caveman's build has the same
        }
        let mut img = img.to_rgb();
        if let Some(d) = &opts.defaults {
            let default = match slot {
                "osk" => Some(&d.0),
                "ny" => Some(&d.1),
                _ => None,
            };
            if default.and_then(|def| mean_difference(&img, def)).is_some_and(|x| x < SAME_AS_DEFAULT) {
                continue; // a copy of the game's default
            }
        }
        if slot == "osk" && !metal && img.data.chunks_exact(3).any(|p| p[2] != 0) {
            let strong = img.data.chunks_exact(3).any(|p| p[2] > 16);
            for p in img.data.chunks_exact_mut(3) {
                p[2] = 0;
            }
            if strong {
                deck.notes.push("its mask's metal channel cleared (its preset left metal off, so it never showed)".into());
            }
        }
        let rel = format!("{LAYERS_DIR}/{}.{suffix}.png", deck.name);
        match slot {
            "osk" => layers.osk = Some(rel.clone()),
            "ny" => layers.ny = Some(rel.clone()),
            _ => layers.rgb = Some(rel.clone()),
        }
        deck.carried.push(if slot == "rgb" { format!("holo map ({})", holo_name(foilmode.unwrap_or(FoilMode::Pearl))) } else { what.to_string() });
        images.push((slot, rel, img));
    }
    if layers.rgb.is_some() {
        layers.foilmode = Some(holo_mode(foilmode.unwrap_or(FoilMode::Pearl)));
    } else if foilmode.is_some() {
        deck.notes.push("a foil mode without a holo map of its own (left out)".into());
    }
    if metal && layers.osk.is_some() {
        deck.carried.push("metal".into());
    }

    // write: the art, the layers, the settings
    if deck.png.exists() && !opts.force {
        deck.status = DeckStatus::AlreadyThere;
        return Ok(());
    }
    std::fs::create_dir_all(folder)?;
    art.to_rgb().write_png(&deck.png)?;
    if !images.is_empty() {
        std::fs::create_dir_all(folder.join(LAYERS_DIR))?;
    }
    for (_, rel, img) in &images {
        img.write_png(folder.join(rel))?;
    }
    let settings = Settings { name: Some(deck.name.clone()), layers: if images.is_empty() { None } else { Some(layers) }, ..Settings::default() };
    std::fs::write(crate::settings::sidecar_path(&deck.png), settings.to_json())?;
    Ok(())
}

/// A boxed reference's import index (ebx.py / dump_ebx `boxed`: the record's first u32 >> 1 when the record is an
/// import slot).
fn boxed_import(doc: &Document, value: &Data, import_offsets: &std::collections::HashSet<usize>) -> Option<usize> {
    let Data::Boxed(r) = value else { return None };
    let rec = doc.boxed_values.iter().find(|b| b.offset as i64 == r.data_offset)?;
    if !import_offsets.contains(&rec.offset) {
        return None;
    }
    let raw: [u8; 4] = rec.raw_bytes.get(..4)?.try_into().ok()?;
    Some((u32::from_le_bytes(raw) >> 1) as usize)
}

/// A boxed float32 value.
fn boxed_f32(doc: &Document, value: &Data) -> Option<f32> {
    let Data::Boxed(r) = value else { return None };
    let rec = doc.boxed_values.iter().find(|b| b.offset as i64 == r.data_offset)?;
    let raw: [u8; 4] = rec.raw_bytes.get(..4)?.try_into().ok()?;
    Some(f32::from_le_bytes(raw))
}

/// The foil mode a holo preset imports (one of the foilmode enum's enumerators).
fn foil_mode(doc: &Document) -> Option<FoilMode> {
    [FoilMode::Pearl, FoilMode::Cellular, FoilMode::Frost, FoilMode::Vertical, FoilMode::Glitter]
        .into_iter()
        .find(|m| doc.imports.iter().any(|(_, inst)| *inst == m.enumerator()))
}

fn holo_mode(m: FoilMode) -> HoloMode {
    match m {
        FoilMode::Pearl => HoloMode::Pearl,
        FoilMode::Cellular => HoloMode::Cellular,
        FoilMode::Frost => HoloMode::Frost,
        FoilMode::Vertical => HoloMode::Vertical,
        FoilMode::Glitter => HoloMode::Glitter,
    }
}

fn holo_name(m: FoilMode) -> &'static str {
    match m {
        FoilMode::Pearl => "pearl",
        FoilMode::Cellular => "cellular",
        FoilMode::Frost => "frost",
        FoilMode::Vertical => "vertical",
        FoilMode::Glitter => "glitter",
    }
}

/// The installed mods in `mods_dir` that add decks: every folder with an items.toc, except dot-folders and Caveman's
/// own (`skip`).
pub fn mods_with_items(mods_dir: &Path, skip: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(mods_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                !name.starts_with('.') && !skip.iter().any(|s| s.eq_ignore_ascii_case(&name)) && p.join("Win32").join(ITEMS_TOC).is_file()
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(display_from_key("Own_Supreme_Spaghetti_Deck"), "Supreme Spaghetti Deck");
        assert_eq!(display_from_key("own_x"), "x");
        assert_eq!(display_from_key("Deck"), "Deck");
    }
}
