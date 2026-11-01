//! The build: decks and wheels into one ReSkate mod (`newdeck.py`, `newwheel.py`, `patch.py`, `caveman.fit`), every
//! template cloned from the player's own install.
//!
//! Per deck, in its host deck's bundle (plain, metal or holo: [`Host`]):
//!
//! - `characters/customization/reskate/board_bottomart/<base>_ap`: the appearance preset (the host's, re-pointed);
//! - `.../<base>_role3_<slot>`: the art, and the deck's own mask, normal map and holo map when its effects make them:
//!   texture asset, texture resource (header: mip 1/2 block offsets, first mip 0, chunk GUID, name hash) and a whole,
//!   Kraken-compressed chunk with its chunk-metadata record (firstMip 0).
//!
//! Per wheel, the same in the donut wheel's bundle, the preset's `param_region_color_0` set to the wheel colour. In
//! `cas_main_sharedbundle`: each item, its two thumbnails (TOC-only chunks) and two CDN records, a board effect's
//! blueprint and effect asset; once per mod the master item list and the bundle reference table.
//!
//! Incremental: each item is built into a recorded list of encoded pieces (`item_cache::Product`) that is kept in the
//! item cache; a later build replays unchanged items from it and encodes only new or changed ones. Every item's GUIDs
//! and resource ids come from its key, so the assembled mod is the same as a clean build's.

// Index loops mirror the Python's array indexing (and numpy's evaluation order) on purpose.
#![allow(clippy::needless_range_loop)]

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bundle::{self, FileLoc, Manifest, ManifestChunk, ManifestEbx, ManifestRes};
use crate::ebx::{self, Data, Document, Guid, ResourceRef, Value};
use crate::effects::{self, DeckEffects, Layers, ResolvedBoard};
use crate::game::{self, Game, GameError, ITEMS_TOC};
use crate::image::Image;
use crate::item_cache::{self, ItemCache, Op, Product};
use crate::shader::{CONFIG_BUNDLE, GLOBALS_TOC};
use crate::thumbs::{self, Field, Lighting, Mapping};
use crate::{bc1, bc7, bcdec, brt, cas, ndb, texture, toc};

pub const SHARED: &str = "win32/characters/customization/configs/cas_main_sharedbundle";
pub const BRT: &str = "characters/customization/configs/cas_main_bundlereftable";
pub const MASTER: &str = "items/itemmastercollection";
/// The shader parameter slots (`ParamKey` GUIDs) a deck fills: the art, mask, normal map and holo map.
pub const SLOT_ART: &str = "2259d419-387f-b1ba-30fc-c5140100b012";
pub const SLOT_OSK: &str = "c1b71b8a-395f-e8ce-30fc-c5140100a381";
pub const SLOT_NY: &str = "830b93ff-7cad-e8ee-30fc-c5140100c668";
pub const SLOT_RGB: &str = "698a46f9-c8de-d48e-30fc-c5140100a381";
/// Float parameters of the appearance presets (ParamKey GUIDs, from the preset dump of 2026-10-22,
/// `tests/preset_params.rs`): the holo iridescence amount (0.25 on the holo host, 1.0 on the Carpet Co and Deathwish
/// holo decks: the shader scales it by the holo map's red), and the metal mask switch (1.0 on the metal and holo
/// hosts: without it the mask's metal channel is ignored).
pub const PARAM_IRIDESCENCE_AMOUNT: &str = "6f007e99-9dc5-d442-2d5c-b62b01007daa";
pub const PARAM_METAL_MASK: &str = "a2189044-0676-ba57-2d5c-b62b0100a211";
const DEFAULTS: &str = "characters/skateboard/defaults/deck/common/generic/graphic/deck_gen_graphic_default";
pub const PREFIX: &str = "characters/customization/reskate/board_bottomart/";
/// The deck item and thumbnail templates: deck_gen_graphic_00003's.
const HOST_NUM: &str = "00003";
/// Natural Blank: what shows through transparent parts of deck art.
const WOOD: &str = "deck_gen_graphic_00001";
const LIGHT_DECKS: usize = 40;
const SKIP_LIGHT: [&str; 2] = ["00041", "00042"];
/// The game's all-metal decks (mask B 255 everywhere): their thumbnails give a metal deck's light.
const METAL_DECKS: [(&str, &str); 4] = [("gen", "00041"), ("gen", "00042"), ("chocolate", "00001"), ("nhs", "00013")];
/// The mip levels of a deck texture (512 x 2048, down to 1 x 1).
pub const DECK_LEVELS: usize = 12;
const ART_SIZE: (usize, usize) = (512, 2048);
const MIN_WIDTH: usize = 256;
const MAX_SIDE: usize = 16384;

const WHEEL_HOST: &str = "characters/skateboard/unlicensed/wheel/common/classic/wheelgraphic/wheel_gen_graphic_00006/2024/wheel_gen_graphic_00006";
const WHEEL_HOST_ITEM: &str = "items/boardwheelcolors0/own_wheelgraphic_gen_classic00006";
const WHEEL_HOST_THUMB: &str = "own_wheelgraphic_gen_classic00006";
pub const WHEEL_PREFIX: &str = "characters/customization/reskate/board_wheelcolor/";
pub const WHEEL_ITEMS: &str = "items/board_wheelcolor/";
/// `param_region_color_0`: three linear floats and a u32 1 (the wheel colour).
const REGION_COLOUR: &str = "4b18a9e2-cd35-8c46-59ff-5e880100548c";
const WHEEL_LEVELS: usize = 10;

// ---- grips (knowledge/cosmetics.md, "Grip colours and patterns"): a grip colour is an appearance preset of the grip
// material (the game's grey grit textures with a tint, or a printed colour map of its own); a grip pattern is a
// cutout texture the item names directly (R 255 where the grip is, the wood shows through the rest)
/// The host grip colour preset: hot pink, with the template grit textures and a tint.
const GRIP_HOST: &str = "characters/skateboard/unlicensed/deck/common/generic/gripcolor/presets/deck_gen_popsiclegripcolor_00006";
const GRIP_HOST_ITEM: &str = "items/boardtapecolors0/own_deckgripcolor_gen_popsicle_00006";
const GRIP_HOST_THUMB: &str = "own_deckgripcolor_gen_popsicle_00006";
/// The template grit colour map every tinted grip uses (512 x 2048 BC7 sRGB, 12 mips).
const GRIP_TEMPLATE_C: &str = "characters/skateboard/unlicensed/deck/common/generic/gripcolor/presets/_template/2023/deck_gen_gripcolor_c";
/// The wood's colour map and mask in the host's bundle (for the thumbnails: the wood through a pattern, the bolt heads).
const GRIP_WOOD_C: &str = "characters/skateboard/unlicensed/deck/generic/popsicle/2022/deck_gen_popsicle_c";
const GRIP_WOOD_MSK: &str = "characters/skateboard/unlicensed/deck/generic/popsicle/2022/deck_gen_popsicle_msk";
/// `param_graphic_tintcolor`: three linear floats and a u32 1.
pub const GRIP_TINT_KEY: &str = "36e50d59-48af-873f-59ff-5e88010042d4";
/// Black grip's tint (preset 00001): what a pattern's thumbnail shows the pattern in.
const GRIP_BLACK: [f32; 3] = [0.0481, 0.0440, 0.0537];
pub const GRIP_PREFIX: &str = "characters/customization/reskate/board_gripcolor/";
pub const GRIP_ITEMS: &str = "items/board_gripcolor/";
/// The host cutout: hearts (its bundle holds the one texture, 512 x 2048 BC1, 12 mips).
const CUTOUT_HOST: &str = "characters/skateboard/unlicensed/deck/common/generic/griptape/deck_gen_gripcutout_00018/2024/deck_gen_gripcutout_00018_osk";
const CUTOUT_HOST_ITEM: &str = "items/boardtapetextures/own_deckgripcutout_gen_popsicle_00018";
const CUTOUT_HOST_THUMB: &str = "own_deckgripcutout_gen_popsicle_00018";
pub const CUTOUT_PREFIX: &str = "characters/customization/reskate/board_griptape/";
pub const CUTOUT_ITEMS: &str = "items/board_griptape/";
/// A cutout texture's G (the grit's smoothness) and B, as the game's.
const CUTOUT_GB: [u8; 2] = [99, 255];
const GRIP_LEVELS: usize = 12;

/// A failed build.
#[derive(Debug)]
pub struct BuildError(pub String);

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BuildError {}

macro_rules! from_error {
    ($($t:ty),*) => {$(
        impl From<$t> for BuildError {
            fn from(e: $t) -> BuildError {
                BuildError(e.to_string())
            }
        }
    )*};
}
from_error!(
    GameError,
    ebx::Error,
    brt::BrtError,
    cas::CasError,
    texture::TextureError,
    crate::image::ImageError,
    ndb::NdbError,
    bundle::BundleError,
    toc::TocError,
    std::io::Error
);

type Result<T> = std::result::Result<T, BuildError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(BuildError(message.into()))
}

/// The hosts a deck can be built on (newdeck.HOSTS): plain (deck_gen_graphic_00003), metal (the gold deck, whose
/// preset switches the mask's metal channel on) or holo (the Primitive holo deck: holo parameters and the holo map).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Plain,
    Metal,
    Holo,
}

impl Host {
    pub fn stock(self) -> &'static str {
        match self {
            Host::Plain => "characters/skateboard/unlicensed/deck/common/generic/graphic/deck_gen_graphic_00003/2023/deck_gen_graphic_00003",
            Host::Metal => "characters/skateboard/unlicensed/deck/common/generic/graphic/deck_gen_graphic_00041/deck_gen_graphic_00041",
            Host::Holo => "characters/skateboard/licensed/deck/common/primitive/graphic/deck_primitive_graphic_00001/2026/deck_primitive_graphic_00001",
        }
    }

    /// The host's bundle in items.toc.
    pub fn bundle(self) -> String {
        format!("win32/{}_ap_cas_main_bundlereftable", self.stock())
    }

    /// The host's texture in each slot: the templates of a new deck's textures (newdeck.host_textures).
    fn textures(self) -> Vec<(&'static str, String)> {
        let s = self.stock();
        match self {
            Host::Plain => vec![("art", format!("{s}_c")), ("osk", format!("{DEFAULTS}_osk")), ("ny", format!("{DEFAULTS}_ny"))],
            Host::Metal => vec![("art", format!("{s}_c")), ("osk", format!("{s}_osk")), ("ny", format!("{DEFAULTS}_ny"))],
            Host::Holo => vec![("art", format!("{s}_c")), ("osk", format!("{s}_osk")), ("ny", format!("{s}_ny")), ("rgb", format!("{s}_rgb"))],
        }
    }

    /// Which host a deck with these layers needs (newdeck.host_kind).
    pub fn for_layers(layers: &Layers) -> Host {
        if layers.rgb.is_some() {
            Host::Holo
        } else if layers.osk.is_some() {
            Host::Metal
        } else {
            Host::Plain
        }
    }
}

// ---------------------------------------------------------------- inputs and outputs

/// Where a deck's layers come from: its effects (computed from the art), or layers made elsewhere (calibration
/// decks compose them by hand).
#[derive(Clone, Debug)]
pub enum DeckLayers {
    Effects(DeckEffects),
    Layers(Layers),
}

/// Loads a deck's art (512 x 2048 RGB, see [`fit_art`]) and layers: called only when the build needs them (the deck
/// is not in the item cache).
pub type DeckLoader = Arc<dyn Fn() -> std::result::Result<(Image, DeckLayers), String> + Send + Sync>;
/// Loads a wheel's face graphic, likewise.
pub type WheelLoader = Arc<dyn Fn() -> std::result::Result<Image, String> + Send + Sync>;

/// Where a deck's art and layers come from: ready (the caller decoded and fitted the art), or a loader.
#[derive(Clone)]
pub enum DeckSource {
    Ready { art: Image, layers: DeckLayers },
    Lazy(DeckLoader),
}

impl fmt::Debug for DeckSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DeckSource::Ready { art, layers } => f.debug_struct("Ready").field("art", &(art.width, art.height)).field("layers", layers).finish(),
            DeckSource::Lazy(_) => f.write_str("Lazy"),
        }
    }
}

/// A deck to build: its display name, item key (`Own_...`) and where its art and effects come from.
///
/// `input` is a hash of everything the deck is built from that the build cannot see in the spec itself (the art
/// file's bytes and its effects: [`item_input`]); with it, the build takes the deck from the item cache when it was
/// built before from the same inputs, and keeps it there for the next build. `None`: always built, never cached.
/// `notes` (what fitting the art did) are kept with a cached deck, for reporting without the art.
#[derive(Clone, Debug)]
pub struct DeckSpec {
    pub display: String,
    pub key: String,
    pub source: DeckSource,
    pub input: Option<u64>,
    pub notes: Vec<String>,
}

impl DeckSpec {
    /// A deck with its art ready, not cached.
    pub fn new(display: impl Into<String>, key: impl Into<String>, art: Image, layers: DeckLayers) -> DeckSpec {
        DeckSpec { display: display.into(), key: key.into(), source: DeckSource::Ready { art, layers }, input: None, notes: Vec::new() }
    }
}

/// Where a wheel's face graphic comes from: ready, or a loader.
#[derive(Clone)]
pub enum WheelSource {
    Ready(Image),
    Lazy(WheelLoader),
}

impl fmt::Debug for WheelSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WheelSource::Ready(face) => f.debug_tuple("Ready").field(&(face.width, face.height)).finish(),
            WheelSource::Lazy(_) => f.write_str("Lazy"),
        }
    }
}

/// A wheel to build: display name, item key, face graphic (any image; centre-cropped to a square) and sRGB colour;
/// `input` and `notes` as for [`DeckSpec`].
#[derive(Clone, Debug)]
pub struct WheelSpec {
    pub display: String,
    pub key: String,
    pub colour: [u8; 3],
    pub source: WheelSource,
    pub input: Option<u64>,
    pub notes: Vec<String>,
}

impl WheelSpec {
    /// A wheel with its face ready, not cached.
    pub fn new(display: impl Into<String>, key: impl Into<String>, face: Image, colour: [u8; 3]) -> WheelSpec {
        WheelSpec { display: display.into(), key: key.into(), colour, source: WheelSource::Ready(face), input: None, notes: Vec::new() }
    }
}

/// A grip to build (the game's "grip colour"): the game's grit in a colour (`colour`, sRGB; the tint of the preset),
/// a picture printed on the grit (`print`, any image, fitted as deck art is; its transparent parts show the grit, in
/// the colour when one is given), or both. `input` and `notes` as a deck's.
#[derive(Clone, Debug)]
pub struct GripSpec {
    pub display: String,
    pub key: String,
    pub colour: Option<[u8; 3]>,
    pub print: Option<Image>,
    pub input: Option<u64>,
    pub notes: Vec<String>,
}

/// A grip pattern to build (the game's cutout): `mask` is 512 x 2048, one channel, 255 where the grip is and 0 where
/// it is cut away (the wood shows), laid out as deck art (the top face fills it, the nose at the top).
#[derive(Clone, Debug)]
pub struct CutoutSpec {
    pub display: String,
    pub key: String,
    pub mask: Image,
    pub input: Option<u64>,
    pub notes: Vec<String>,
}

/// The size of a grip's textures and of a cutout.
pub const GRIP_SIZE: (usize, usize) = (512, 2048);

/// A pattern mask from any image: its alpha where it has one that is not all opaque, else its lightness; an image of
/// the deck's 1:4 shape (within 3 per cent) is fitted whole, any other is repeated at its own size across the
/// 512 x 2048 (a tile). Returns the one-channel mask and notes.
pub fn fit_mask(img: &Image) -> (Image, Vec<String>) {
    let mut notes = Vec::new();
    let rgba = img.to_rgba();
    let has_alpha = rgba.data.chunks_exact(4).any(|p| p[3] < 255);
    let source = if has_alpha {
        notes.push("the pattern is the picture's transparency (clear parts cut the grip away)".to_string());
        rgba.channel(3)
    } else {
        notes.push("the pattern is the picture's lightness (dark parts cut the grip away)".to_string());
        rgba.to_l()
    };
    let target = GRIP_SIZE.0 as f64 / GRIP_SIZE.1 as f64;
    let aspect = source.width as f64 / source.height as f64;
    if (aspect - target).abs() / target <= 0.03 {
        return (source.resize_lanczos(GRIP_SIZE.0, GRIP_SIZE.1), notes);
    }
    notes.push(format!("{} x {} is not the deck's 1:4 shape: repeated as a tile", source.width, source.height));
    let mut out = Image::new(GRIP_SIZE.0, GRIP_SIZE.1, 1);
    for y in 0..GRIP_SIZE.1 {
        for x in 0..GRIP_SIZE.0 {
            out.pixel_mut(x, y)[0] = source.pixel(x % source.width, y % source.height)[0];
        }
    }
    (out, notes)
}

/// Build settings. `seed` salts every GUID and resource id (each item's ids come from its key and the mod's name, so
/// they are the same from build to build); `cache_dir` holds the fitted thumbnail lighting and the item cache
/// (default `%LOCALAPPDATA%\Caveman\cache`); `item_cache` off builds every item.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub tool_version: String,
    pub seed: Option<u64>,
    pub cache_dir: Option<PathBuf>,
    pub item_cache: bool,
    /// Assets that belong to no item, as (bundle, name, EBX bytes), added after the items in this order (Caveman FX's
    /// state assets; empty for every other build).
    pub extra_ebx: Vec<(String, String, Vec<u8>)>,
    /// More mod-level pieces in items.toc bundles (EBX, resources, chunks; Caveman FX's sprite texture and material
    /// depot), added after `extra_ebx` in this order. Empty for every other build.
    pub extra_ops: Vec<Op>,
    /// Resources of the game configuration bundle (`globals.toc`) replaced whole, as (name, payload, meta): Caveman FX's
    /// material rows in the global shader database and texture lookup. When any are given the mod also carries a
    /// `globals.toc` patch; empty (every other build), it carries none.
    pub config_res: Vec<(String, Vec<u8>, [u8; 16])>,
    /// The mod's author in its manifest.json (None: "Caveman").
    pub author: Option<String>,
}

impl Default for BuildOptions {
    fn default() -> BuildOptions {
        BuildOptions {
            tool_version: "0.0.1".to_string(),
            seed: None,
            cache_dir: None,
            item_cache: true,
            extra_ebx: Vec::new(),
            extra_ops: Vec::new(),
            config_res: Vec::new(),
            author: None,
        }
    }
}

/// The input hash of an item for the cache: the art file's bytes, its effects, anything else its pieces depend on
/// that the spec does not carry (FNV-1a over the parts).
pub fn item_input(parts: &[&[u8]]) -> u64 {
    item_cache::hash64(parts)
}

/// The notes of an item in the item cache (built before from the same inputs), or None: lets a caller skip decoding
/// and fitting an unchanged item's art. `kind` is "deck" or "wheel".
#[allow(clippy::too_many_arguments)]
pub fn cached_notes(
    game: &Game,
    opts: &BuildOptions,
    mods_dir: &Path,
    mod_name: &str,
    kind: &str,
    key: &str,
    display: &str,
    input: u64,
) -> Option<Vec<String>> {
    if !opts.item_cache {
        return None;
    }
    let cache = ItemCache::open(Some(&cache_dir(opts, game)), mods_dir, mod_name);
    let game_id = exe_key(game).ok()?;
    cache.peek_notes(&ItemCache::entry(kind, mod_name, key, display, input, 0, opts.seed.unwrap_or(0), game_id))
}

/// One built item: display name, base name (the item key's lower-case tail) and, for a wheel, its 768 px menu
/// thumbnail (the wheel previews; a deck's preview is rendered before the build, and holding 2.4 MB per deck for a
/// whole collection is gigabytes).
#[derive(Clone, Debug)]
pub struct BuiltItem {
    pub display: String,
    pub base: String,
    pub thumbnail: Option<Image>,
}

/// What a build wrote. `reused`: items taken from the item cache; `encoded`: items built this time.
#[derive(Clone, Debug)]
pub struct BuildReport {
    pub mod_dir: PathBuf,
    pub decks: Vec<BuiltItem>,
    pub wheels: Vec<BuiltItem>,
    pub grips: Vec<BuiltItem>,
    pub cutouts: Vec<BuiltItem>,
    pub cas_bytes: usize,
    pub toc_bytes: usize,
    pub bundles: usize,
    pub chunks: usize,
    pub reused: usize,
    pub encoded: usize,
}

// ---------------------------------------------------------------- hashes and ids

/// HashedAssetKey (of Key) and NameHash (of Name): djb2 with xor, 32-bit, case as written (newdeck.djb32).
pub fn djb32(text: &str) -> u32 {
    let mut h: u32 = 5381;
    for c in text.bytes() {
        h = h.wrapping_mul(33) ^ c as u32;
    }
    h
}

/// Names the scheme that derives an item's ids from its key; bump if the derivation changes.
const ID_SCHEME: &str = "caveman-ids-1";

/// Where new GUIDs, chunk GUIDs and resource ids come from: the system's randomness, or a seeded stream.
struct Ids {
    state: Option<u64>,
    rids: std::collections::HashSet<u64>,
}

impl Ids {
    /// The ids of one item: a stream seeded by the mod's name, the item's kind and key, a salt (raised only if an id
    /// clashed with one already taken) and the build's seed. The same item gets the same GUIDs and resource ids in
    /// every build, so items reused from the item cache fit a mod exactly as freshly built ones would.
    fn for_item(mod_name: &str, kind: &str, key: &str, salt: u32, seed: u64) -> Ids {
        let state = item_cache::hash64(&[
            ID_SCHEME.as_bytes(),
            mod_name.to_lowercase().as_bytes(),
            kind.as_bytes(),
            key.to_lowercase().as_bytes(),
            &salt.to_le_bytes(),
            &seed.to_le_bytes(),
        ]);
        Ids { state: Some(state), rids: Default::default() }
    }

    fn bytes16(&mut self) -> [u8; 16] {
        match &mut self.state {
            None => crate::ebx::random_bytes::<16>(),
            Some(s) => {
                let mut out = [0u8; 16];
                for half in out.chunks_mut(8) {
                    // splitmix64
                    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
                    let mut z = *s;
                    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                    half.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
                }
                out
            }
        }
    }

    /// A random version-4 GUID in the little-endian layout (uuid.uuid4().bytes_le).
    fn guid(&mut self) -> Guid {
        let mut be = self.bytes16();
        be[6] = (be[6] & 0x0F) | 0x40;
        be[8] = (be[8] & 0x3F) | 0x80;
        Guid::from_be(be)
    }

    /// A chunk GUID with the low bit of its last (little-endian) byte set: compressed blocks (newdeck.new_chunk_guid).
    fn chunk_guid(&mut self) -> toc::Guid {
        let mut le = self.guid().0;
        le[15] |= 1;
        toc::Guid::from_le_bytes(le)
    }

    /// A new resource id: random and odd, like every one the game ships (newdeck.DeckPatch.new_rid). An even id was
    /// taken for a tagged pointer and crashed the game at boot.
    fn rid(&mut self) -> u64 {
        loop {
            let b = self.bytes16();
            let rid = u64::from_le_bytes(b[..8].try_into().unwrap()) | 1;
            if self.rids.insert(rid) {
                return rid;
            }
        }
    }

    fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.bytes16()[..8].try_into().unwrap())
    }
}

// ---------------------------------------------------------------- the patch (patch.Patch + newdeck.DeckPatch)

/// A bundle as the mod will ship it: the game's, assets added or replaced. `meta_root`: its chunk metadata, parsed
/// once when the first record is added and written back with the manifest.
struct PatchedBundle {
    name: String,
    files: Vec<FileLoc>,
    manifest: Manifest,
    meta_root: Option<ndb::Node>,
}

/// A mod being built (patch.Patch, newdeck.DeckPatch): one cas archive, edited copies of game bundles, TOC chunks.
struct ModPatch {
    game: Arc<Game>,
    cas: Vec<u8>,
    bundles: Vec<PatchedBundle>,
    chunks: Vec<toc::PatchChunk>,
    /// The game configuration bundle's region when the mod replaces resources there (`globals.toc`), else None.
    config_region: Option<Vec<u8>>,
}

impl ModPatch {
    fn new(game: Arc<Game>) -> ModPatch {
        ModPatch { game, cas: Vec::new(), bundles: Vec::new(), chunks: Vec::new(), config_region: None }
    }

    /// A copy of the game configuration bundle (`globals.toc`) with `replaced` resources (name, payload, meta) put in
    /// the archive, the manifest's SHA-1s, sizes and metas edited in place (as the shader mod does), the manifest
    /// appended last; its region is kept for [`ModPatch::write`].
    fn patch_config(&mut self, replaced: &[(String, Vec<u8>, [u8; 16])]) -> Result<()> {
        let b = self.game.bundle(CONFIG_BUNDLE, GLOBALS_TOC)?;
        let m = &b.manifest;
        if m.endian != bundle::Endian::Little {
            return fail("the configuration bundle's manifest is not little-endian");
        }
        let (ne, nr, nc) = (m.ebx.len(), m.res.len(), m.chunks.len());
        let total = ne + nr + nc;
        let mut files = b.files.clone();
        let mut raw = b.manifest_raw.clone();
        let put = |raw: &mut Vec<u8>, at: usize, bytes: &[u8]| -> Result<()> {
            raw.get_mut(at..at + bytes.len()).ok_or_else(|| BuildError("the configuration manifest is shorter than its tables".into()))?.copy_from_slice(bytes);
            Ok(())
        };
        for (name, payload, meta) in replaced {
            let i = m.res.iter().rposition(|r| &r.name == name).ok_or_else(|| BuildError(format!("{CONFIG_BUNDLE} has no {name}")))?;
            let enc = cas::encode(payload, self.game.root())?;
            let at = self.append(&enc)?;
            files[1 + ne + i] = FileLoc::in_mod(at, enc.len() as u32);
            put(&mut raw, 36 + 20 * (ne + i), &bundle::sha1(&enc))?;
            let names = 36 + 20 * total + 8 * ne;
            put(&mut raw, names + 8 * i + 4, &(payload.len() as u32).to_le_bytes())?;
            let metas = names + 8 * nr + 4 * nr;
            put(&mut raw, metas + 16 * i, meta)?;
        }
        let check = bundle::read_manifest(&raw)?;
        for (name, payload, meta) in replaced {
            let r = check.res.iter().rfind(|r| &r.name == name).ok_or_else(|| BuildError(format!("{name}: lost from the edited manifest")))?;
            if r.size as usize != payload.len() || &r.meta != meta {
                return fail(format!("{name}: the edited configuration manifest does not read back"));
            }
        }
        let at = self.append(&raw)?;
        files[0] = FileLoc::in_mod(at, raw.len() as u32);
        self.config_region = Some(bundle::write_region(&files));
        Ok(())
    }

    /// Appends to the archive: where the data starts. A location is 32 bits, so one archive holds at most 4 GiB
    /// (about 3,000 decks).
    fn append(&mut self, data: &[u8]) -> Result<u32> {
        let at = self.cas.len();
        if at + data.len() > u32::MAX as usize {
            return fail("the mod would be over 4 GiB, more than one archive holds: build fewer decks into it (move some folders out of the drop folder)");
        }
        self.cas.extend_from_slice(data);
        Ok(at as u32)
    }

    fn bundle(&mut self, name: &str) -> Result<&mut PatchedBundle> {
        if let Some(i) = self.bundles.iter().position(|b| b.name == name) {
            return Ok(&mut self.bundles[i]);
        }
        let b = self.game.bundle(name, ITEMS_TOC)?;
        self.bundles.push(PatchedBundle { name: name.to_string(), files: b.files, manifest: b.manifest, meta_root: None });
        Ok(self.bundles.last_mut().unwrap())
    }

    /// Encodes `data` (Kraken) into the archive: (encoded bytes, its location).
    fn put(&mut self, data: &[u8]) -> Result<(Vec<u8>, FileLoc)> {
        let enc = cas::encode(data, self.game.root())?;
        let at = self.append(&enc)?;
        Ok((enc.clone(), FileLoc::in_mod(at, enc.len() as u32)))
    }

    /// Replays one recorded piece of an item (item_cache::Op) into the mod, in the order the item recorded it.
    fn apply(&mut self, op: Op) -> Result<()> {
        match op {
            Op::Ebx { bundle: in_bundle, name, size, enc } => {
                let at = self.append(&enc)?;
                let b = self.bundle(&in_bundle)?;
                let i = 1 + b.manifest.ebx.len();
                b.files.insert(i, FileLoc::in_mod(at, enc.len() as u32));
                b.manifest.ebx.push(ManifestEbx { name, size, sha1: bundle::sha1(&enc) });
            }
            Op::Res { bundle: in_bundle, name, rtype, meta, rid, size, enc } => {
                let at = self.append(&enc)?;
                let b = self.bundle(&in_bundle)?;
                let i = 1 + b.manifest.ebx.len() + b.manifest.res.len();
                b.files.insert(i, FileLoc::in_mod(at, enc.len() as u32));
                b.manifest.res.push(ManifestRes { name, size, sha1: bundle::sha1(&enc), rtype, meta, rid });
            }
            Op::Chunk { bundle: in_bundle, guid, enc, record } => {
                let record = match record {
                    Some(r) => Some(ndb::read(&r, 0)?.0),
                    None => None,
                };
                self.add_chunk(in_bundle.as_deref(), guid, &enc, record)?;
            }
        }
        Ok(())
    }

    /// A chunk already encoded: in the TOC always, and in `bundle` (whole) when given, with its chunk-metadata record.
    fn add_chunk(&mut self, bundle: Option<&str>, guid: toc::Guid, enc: &[u8], record: Option<ndb::Node>) -> Result<()> {
        let at = self.append(enc)?;
        self.chunks.push(toc::PatchChunk {
            guid,
            patch: true,
            install: bundle::MOD_INSTALL_CHUNK,
            archive: bundle::MOD_ARCHIVE,
            offset: at,
            size: enc.len() as u32,
        });
        let Some(name) = bundle else { return Ok(()) };
        let decoded: usize = cas::blocks(enc).iter().map(|h| h.decoded).sum();
        let b = self.bundle(name)?;
        b.files.push(FileLoc::in_mod(at, enc.len() as u32));
        b.manifest.chunks.push(ManifestChunk { guid, logical_offset: 0, logical_size: decoded as u32, sha1: bundle::sha1(enc) });
        if let Some(record) = record {
            if b.meta_root.is_none() {
                let (root, end) = ndb::read(&b.manifest.chunk_meta, 0)?;
                if end != b.manifest.chunk_meta.len() {
                    return fail(format!("{name}: chunk metadata has trailing bytes"));
                }
                b.meta_root = Some(root);
            }
            let index = b.manifest.chunks.len() - 1;
            b.meta_root.as_mut().unwrap().children.insert(index, record);
        }
        Ok(())
    }

    fn replace_ebx(&mut self, bundle: &str, name: &str, data: &[u8]) -> Result<()> {
        let (enc, loc) = self.put(data)?;
        let b = self.bundle(bundle)?;
        let i = b.manifest.ebx_index(name).ok_or_else(|| BuildError(format!("{bundle} has no {name}")))?;
        b.files[1 + i] = loc;
        b.manifest.ebx[i].size = data.len() as u32;
        b.manifest.ebx[i].sha1 = bundle::sha1(&enc);
        Ok(())
    }

    fn replace_res(&mut self, bundle: &str, name: &str, data: &[u8], meta: [u8; 16]) -> Result<()> {
        let (enc, loc) = self.put(data)?;
        let b = self.bundle(bundle)?;
        let i = b.manifest.res_index(name).ok_or_else(|| BuildError(format!("{bundle} has no {name}")))?;
        let ne = b.manifest.ebx.len();
        b.files[1 + ne + i] = loc;
        let r = &mut b.manifest.res[i];
        r.size = data.len() as u32;
        r.sha1 = bundle::sha1(&enc);
        r.meta = meta;
        Ok(())
    }

    /// Writes the mod folder (patch.Patch.write): the archive, the patch TOC, layout.toc and initfs from the game,
    /// the build stamp, reskate-build.json, manifest.json, README.md and the icon.
    fn write(
        &mut self,
        mod_dir: &Path,
        title: &str,
        description: &str,
        readme: &str,
        icon: Option<&Image>,
        tool_version: &str,
        author: &str,
    ) -> Result<(usize, usize)> {
        let mut regions = Vec::new();
        let bundles = std::mem::take(&mut self.bundles);
        for mut b in bundles {
            if let Some(root) = b.meta_root.take() {
                b.manifest.chunk_meta = ndb::write(&root);
            }
            let raw = bundle::write_manifest(&b.manifest)?;
            let at = self.append(&raw)?;
            b.files[0] = FileLoc::in_mod(at, raw.len() as u32);
            regions.push(toc::PatchBundle { name: b.name.clone(), region: bundle::write_region(&b.files), load: 1 });
        }
        let toc_bytes = toc::write_patch_toc(&regions, &self.chunks, 3)?;
        let globals_toc = match self.config_region.take() {
            Some(region) => Some(toc::write_patch_toc(&[toc::PatchBundle { name: CONFIG_BUNDLE.to_string(), region, load: 1 }], &[], 3)?),
            None => None,
        };
        if mod_dir.is_dir() {
            std::fs::remove_dir_all(mod_dir)?;
        }
        let pkg = game::PKG.split('/').fold(mod_dir.join("Win32"), |p, part| p.join(part));
        std::fs::create_dir_all(&pkg)?;
        std::fs::write(pkg.join("cas_01.cas"), &self.cas)?;
        std::fs::write(mod_dir.join("Win32").join("items.toc"), &toc_bytes)?;
        if let Some(g) = &globals_toc {
            std::fs::write(mod_dir.join("Win32").join(GLOBALS_TOC), g)?;
        }
        let data = self.game.data();
        let layout = std::fs::read(data.join("layout.toc"))?;
        let mut out = layout[..8].to_vec();
        out.resize(toc::ENVELOPE, 0);
        out.extend_from_slice(&layout[toc::ENVELOPE..]);
        std::fs::write(mod_dir.join("layout.toc"), out)?;
        std::fs::copy(data.join("initfs_Win32"), mod_dir.join("initfs_win32"))?;
        let skate_sha = hex(&sha256(&std::fs::read(self.game.root().join("Skate.exe"))?));
        std::fs::write(mod_dir.join(".reskate-studio-patch"), format!("ReSkate Studio native Patch v1\nskate_sha256={skate_sha}\n"))?;
        let now = utc_now();
        std::fs::write(
            mod_dir.join("reskate-build.json"),
            format!("{{\n  \"schema\": 1,\n  \"tool\": \"Caveman\",\n  \"version\": {},\n  \"built\": {}\n}}", json_str(tool_version), json_str(&now)),
        )?;
        std::fs::write(
            mod_dir.join("manifest.json"),
            format!(
                "{{\n    \"name\": {},\n    \"author\": {},\n    \"version_number\": {},\n    \"website_url\": \"\",\n    \"description\": {},\n    \"dependencies\": []\n}}",
                json_str(title),
                json_str(author),
                json_str(tool_version),
                json_str(description)
            ),
        )?;
        std::fs::write(mod_dir.join("README.md"), readme)?;
        if let Some(icon) = icon {
            icon.write_png(mod_dir.join("icon.png"))?;
        }
        Ok((self.cas.len(), toc_bytes.len()))
    }
}

/// A JSON string as Python's json.dump writes it (ensure_ascii).
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7E => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 (FIPS 180-4): the build stamp names the Skate.exe a mod was built for.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

// ---------------------------------------------------------------- reading the game

/// The decoded EBX `name` of a bundle (newdeck._ebx).
pub(crate) fn bundle_ebx(game: &Game, b: &game::Bundle, name: &str) -> Result<Vec<u8>> {
    let i = b.manifest.ebx_index(name).ok_or_else(|| BuildError(format!("{} has no EBX {name}", b.name)))?;
    Ok(game.decode(&b.files[b.manifest.ebx_file(i)])?)
}

/// The decoded resource `name` of a bundle and its manifest entry (newdeck._res).
fn bundle_res(game: &Game, b: &game::Bundle, name: &str) -> Result<(Vec<u8>, ManifestRes)> {
    let i = b.manifest.res_index(name).ok_or_else(|| BuildError(format!("{} has no resource {name}", b.name)))?;
    Ok((game.decode(&b.files[b.manifest.res_file(i)])?, b.manifest.res[i].clone()))
}

/// A texture of a bundle as an RGBA image: its first mip, decoded (gamedata.Game.texture_image).
pub fn texture_image(game: &Game, b: &game::Bundle, name: &str) -> Result<Image> {
    let (payload, res) = bundle_res(game, b, name)?;
    let h = texture::TextureHeader::parse(&payload, &res.meta)?;
    let format = bcdec::BcFormat::from_frostbite(h.format).ok_or_else(|| BuildError(format!("{name}: texture format {} not decoded", h.format)))?;
    let pixels = game.chunk_data(&toc::Guid::from_le_bytes(h.chunk), ITEMS_TOC)?;
    let (w, ht) = (h.width as usize, h.height as usize);
    let mip0 = w.div_ceil(4).max(1) * ht.div_ceil(4).max(1) * format.block_bytes();
    if pixels.len() < mip0 {
        return fail(format!("{name}: its chunk is shorter than its first mip"));
    }
    Ok(bcdec::decode(format, &pixels[..mip0], w, ht))
}

fn bundle_names(game: &Game) -> Result<Vec<String>> {
    let mut names: Vec<String> = game.toc(ITEMS_TOC)?.toc.bundles.iter().map(|b| b.name.clone()).collect();
    names.sort();
    Ok(names)
}

fn find_bundle(game: &Game, suffix: &str) -> Result<String> {
    game.find_bundle(suffix, ITEMS_TOC)?.ok_or_else(|| BuildError(format!("no bundle ends with {suffix}")))
}

/// What a new texture is cloned from (newdeck._template): the EBX bytes and document, resource header and entry, and
/// the host bundle's chunk-metadata record for it.
struct TextureTemplate {
    ebx: Vec<u8>,
    doc: Document,
    header: Vec<u8>,
    res: ManifestRes,
    record: ndb::Node,
}

fn texture_template(game: &Game, b: &game::Bundle, records: &[ndb::Node], name: &str) -> Result<TextureTemplate> {
    let data = bundle_ebx(game, b, name)?;
    let (header, res) = bundle_res(game, b, name)?;
    let want = texture::owner_hash(name);
    let record = records
        .iter()
        .find(|r| r.field("h64").and_then(|f| f.payload.get(..8)).map(|p| u64::from_le_bytes(p.try_into().unwrap())) == Some(want))
        .cloned()
        .ok_or_else(|| BuildError(format!("{}: no chunk-metadata record for {name}", b.name)))?;
    Ok(TextureTemplate { doc: ebx::read_document(&data)?, ebx: data, header, res, record })
}

fn chunk_records(b: &game::Bundle) -> Result<Vec<ndb::Node>> {
    Ok(ndb::read(&b.manifest.chunk_meta, 0)?.0.children)
}

/// The game's default mask and normal map for decks (RGBA): what the effects start from (newdeck.default_layers).
pub fn default_layers(game: &Game) -> Result<(Image, Image)> {
    let host = game.bundle(&Host::Plain.bundle(), ITEMS_TOC)?;
    Ok((texture_image(game, &host, &format!("{DEFAULTS}_osk"))?, texture_image(game, &host, &format!("{DEFAULTS}_ny"))?))
}

/// Natural Blank's art: what shows through transparent parts of deck art (caveman.WOOD).
pub fn natural_wood(game: &Game) -> Result<Image> {
    let b = game.bundle(&find_bundle(game, &format!("/{WOOD}_ap_cas_main_bundlereftable"))?, ITEMS_TOC)?;
    let name =
        b.manifest.res.iter().find(|r| r.name.ends_with(&format!("/{WOOD}_c"))).map(|r| r.name.clone()).ok_or_else(|| BuildError(format!("no {WOOD} art")))?;
    texture_image(game, &b, &name)
}

/// Any image as 512 x 2048 deck art (caveman.fit): turned upright when drawn sideways (3:1 to 5:1, a quarter turn
/// clockwise), cover-cropped to 1:4 and centred, resized (Lanczos), flattened onto `wood` where transparent. Returns
/// the art and notes on what was done.
pub fn fit_art(img: &Image, wood: &Image) -> Result<(Image, Vec<String>)> {
    fit_art_on(img, wood, None)
}

/// What shows through transparent parts of a deck's art: the deck's colour (`Name.ff0000.png`, settings `colour`),
/// else natural wood. The game's own plain-colour decks are solid-colour art too: deck materials take no colour.
pub fn deck_backdrop(wood: &Image, paint: Option<[u8; 3]>) -> Image {
    match paint {
        Some(c) => Image::filled(ART_SIZE.0, ART_SIZE.1, &c),
        None => wood.clone(),
    }
}

/// `fit_art` over the deck's colour (`deck_backdrop`) where the art is transparent.
pub fn fit_art_on(img: &Image, wood: &Image, paint: Option<[u8; 3]>) -> Result<(Image, Vec<String>)> {
    let (img, mut notes) = fit_frame(img)?;
    if img.data.chunks_exact(4).any(|p| p[3] < 255) {
        notes.push(match paint {
            Some(c) => format!("transparent parts show #{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
            None => "transparent parts show natural wood".to_string(),
        });
    }
    Ok((deck_backdrop(wood, paint).to_rgba().alpha_composite(&img).to_rgb(), notes))
}

/// Any image in the deck art's frame, its transparency kept: turned upright when drawn sideways, cover-cropped to
/// 1:4 and centred, resized to 512 x 2048 RGBA (the fit of `fit_art_on` without the backdrop; printed grips use it).
pub fn fit_frame(img: &Image) -> Result<(Image, Vec<String>)> {
    let mut notes = Vec::new();
    if img.width.max(img.height) > MAX_SIDE {
        return fail(format!("{} x {} is too large (at most {MAX_SIDE} px a side)", img.width, img.height));
    }
    let mut img = img.clone();
    let (mut w, mut h) = (img.width, img.height);
    if w > h && (3.0..=5.0).contains(&(w as f64 / h as f64)) {
        img = img.rotate_cw();
        notes.push("turned upright (it was drawn sideways; its left edge is now the top)".to_string());
        (w, h) = (img.width, img.height);
    }
    let target = ART_SIZE.0 as f64 / ART_SIZE.1 as f64;
    if (w as f64 / h as f64 - target).abs() / target > 0.03 {
        let scale = (ART_SIZE.0 as f64 / w as f64).max(ART_SIZE.1 as f64 / h as f64);
        let (cw, ch) = ((ART_SIZE.0 as f64 / scale).round_ties_even() as usize, (ART_SIZE.1 as f64 / scale).round_ties_even() as usize);
        let (left, top) = ((w - cw) / 2, (h - ch) / 2);
        img = img.crop(left, top, left + cw, top + ch);
        notes.push(format!("cropped to fill the deck ({w} x {h} is not 1:4)"));
    }
    if img.width < MIN_WIDTH {
        notes.push(format!("only {} px wide: it will look soft (1024 x 4096 is ideal)", img.width));
    }
    Ok((img.to_rgba().resize_lanczos(ART_SIZE.0, ART_SIZE.1), notes))
}

// ---------------------------------------------------------------- textures

/// The full mip chain of an image (each level resized from the full image, Lanczos), encoded (newdeck.mips_bc7,
/// newwheel.mips). A normal map's smaller mips are made unit length again.
pub fn mip_chain(img: &Image, levels: usize, normal: bool, encode: fn(&Image) -> Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for m in 0..levels {
        let mut level = if m == 0 { img.clone() } else { img.resize_lanczos((img.width >> m).max(1), (img.height >> m).max(1)) };
        if normal && m > 0 {
            for px in level.data.chunks_exact_mut(level.channels) {
                let n = [px[0] as f64 / 127.5 - 1.0, px[1] as f64 / 127.5 - 1.0, px[2] as f64 / 127.5 - 1.0];
                let norm = ((n[0] * n[0] + n[1] * n[1]) + n[2] * n[2]).sqrt().max(1e-6);
                for c in 0..3 {
                    px[c] = ((n[c] / norm + 1.0) * 127.5).round_ties_even().clamp(0.0, 255.0) as u8;
                }
            }
        }
        out.extend_from_slice(&encode(&level));
    }
    out
}

/// What one item adds to a mod, recorded as it is built (the pieces encoded, in order) so it can go into the item
/// cache and be replayed into this build and later ones (item_cache::Op).
struct Recorder<'a> {
    root: &'a Path,
    ops: Vec<Op>,
}

impl<'a> Recorder<'a> {
    fn new(root: &'a Path) -> Recorder<'a> {
        Recorder { root, ops: Vec::new() }
    }

    fn ebx(&mut self, bundle: &str, name: &str, data: &[u8]) -> Result<()> {
        let enc = cas::encode(data, self.root)?;
        self.ops.push(Op::Ebx { bundle: bundle.to_string(), name: name.to_string(), size: data.len() as u32, enc });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn res(&mut self, bundle: &str, name: &str, rtype: u32, meta: [u8; 16], rid: u64, data: &[u8]) -> Result<()> {
        let enc = cas::encode(data, self.root)?;
        self.ops.push(Op::Res { bundle: bundle.to_string(), name: name.to_string(), rtype, meta, rid, size: data.len() as u32, enc });
        Ok(())
    }

    fn chunk(&mut self, bundle: Option<&str>, guid: toc::Guid, enc: Vec<u8>, record: Option<&ndb::Node>) {
        self.ops.push(Op::Chunk { bundle: bundle.map(str::to_string), guid, enc, record: record.map(ndb::write) });
    }
}

/// A new texture in a host bundle (newdeck._add_texture, newwheel.add_texture): whole (first mip 0), Kraken, the
/// header's mip offsets and name hash, a chunk-metadata record with firstMip 0. Returns its document.
fn add_texture(rec: &mut Recorder, ids: &mut Ids, host: &str, t: &TextureTemplate, name: &str, pixels: &[u8]) -> Result<Document> {
    add_texture_sized(rec, ids, host, t, name, pixels, None)
}

/// [`add_texture`], with the texture at another size and mip count than the template's when `size` is given
/// (`(width, height, levels)`: an animated deck's atlas in the holo map slot). The header is resized
/// (texture::resize_header) and the texture asset's CropInfo and authored size follow.
fn add_texture_sized(
    rec: &mut Recorder,
    ids: &mut Ids,
    host: &str,
    t: &TextureTemplate,
    name: &str,
    pixels: &[u8],
    size: Option<(usize, usize, usize)>,
) -> Result<Document> {
    let template = match size {
        Some((w, h, levels)) => texture::resize_header(&t.header, &t.res.meta, w, h, levels)?,
        None => t.header.clone(),
    };
    let h = texture::TextureHeader::parse(&template, &t.res.meta)?;
    if pixels.len() != h.chunk_size as usize {
        return fail(format!("{name}: {} bytes of pixels, the texture's chunk holds {}", pixels.len(), h.chunk_size));
    }
    let enc = cas::encode(pixels, rec.root)?;
    let chunk = ids.chunk_guid();
    let header = texture::new_texture_header(&template, &t.res.meta, &enc, chunk.le_bytes(), name)?;
    let rid = ids.rid();
    let mut tex = clone_doc(&t.ebx, name, ids)?;
    tex.set("Resource", ResourceRef::new(rid), 0)?;
    if let Some((w, h, _)) = size {
        for (field, v) in [("CropInfo.Z", w), ("CropInfo.W", h), ("AuthoredWidth", w), ("AuthoredHeight", h)] {
            tex.set(field, Data::Int(v as i64), 0).map_err(|e| BuildError(format!("{name}: {field}: {e}")))?;
        }
    }
    rec.ebx(host, name, &write(&tex)?)?;
    rec.res(host, name, t.res.rtype, t.res.meta, rid, &header)?;
    let mut record = t.record.clone();
    record.field_mut("h64").ok_or_else(|| BuildError("record without h64".into()))?.payload = texture::owner_hash(name).to_le_bytes().to_vec();
    record.field_mut("meta").and_then(|m| m.field_mut("firstMip")).ok_or_else(|| BuildError("record without meta.firstMip".into()))?.payload =
        0i32.to_le_bytes().to_vec();
    rec.chunk(Some(host), chunk, enc, Some(&record));
    Ok(tex)
}

/// A texture that belongs to no item (Caveman FX's sprite sheet), built exactly as a deck's are: cloned from
/// `template` (a BC7 texture of `bundle` in items.toc, same size as `img`), the whole mip chain BC7-encoded, the chunk
/// Kraken-compressed, the header's mip offsets, first mip 0, chunk GUID and name hash set, a chunk-metadata record
/// with firstMip 0, a new odd resource id. Its ids come from `mod_name` and `name`, so every build makes the same.
pub fn mod_texture(game: &Game, bundle: &str, template: &str, name: &str, img: &Image, mod_name: &str) -> Result<ModTexture> {
    let b = game.bundle(bundle, ITEMS_TOC)?;
    let records = chunk_records(&b)?;
    let t = texture_template(game, &b, &records, template)?;
    let h = texture::TextureHeader::parse(&t.header, &t.res.meta)?;
    if !matches!(h.format, 66 | 67) {
        return fail(format!("{template}: format {} is not BC7", h.format));
    }
    if (h.width as usize, h.height as usize) != (img.width, img.height) {
        return fail(format!("{name}: {} x {}, the template is {} x {}", img.width, img.height, h.width, h.height));
    }
    let pixels = mip_chain(img, h.mip_sizes.len(), false, bc7::encode);
    let mut ids = Ids::for_item(mod_name, "texture", name, 0, 0);
    let mut rec = Recorder::new(game.root());
    let doc = add_texture(&mut rec, &mut ids, bundle, &t, name, &pixels)?;
    let (rid, chunk) = rec
        .ops
        .iter()
        .find_map(|op| match op {
            Op::Res { rid, .. } => Some(*rid),
            _ => None,
        })
        .zip(rec.ops.iter().find_map(|op| match op {
            Op::Chunk { guid, .. } => Some(*guid),
            _ => None,
        }))
        .ok_or_else(|| BuildError(format!("{name}: the texture has no resource or chunk")))?;
    Ok(ModTexture { ops: rec.ops, reference: doc.reference(), rid, chunk })
}

/// A texture that belongs to no item ([`mod_texture`]): its pieces (for [`BuildOptions::extra_ops`]), its (file,
/// instance) GUIDs, its resource id and chunk GUID.
pub struct ModTexture {
    pub ops: Vec<Op>,
    pub reference: (Guid, Guid),
    pub rid: u64,
    pub chunk: toc::Guid,
}

/// A template EBX as a new document: new file and instance GUIDs, renamed (newdeck._clone).
fn clone_doc(data: &[u8], name: &str, ids: &mut Ids) -> Result<Document> {
    let mut doc = ebx::read_document(data)?;
    doc.set_file_guid(ids.guid());
    for i in 0..doc.instances.len() {
        let g = ids.guid();
        doc.set_instance_guid(i, g);
    }
    doc.rename(name)?;
    Ok(doc)
}

/// newdeck._write: the compact layout.
fn write(doc: &Document) -> Result<Vec<u8>> {
    Ok(ebx::write_document_with(doc, false, false)?)
}

fn remap_template(remap: &mut HashMap<Guid, Guid>, template: &Document, new: &Document) {
    remap.insert(template.file_guid, new.file_guid);
    remap.insert(template.instances[0].instance_guid, new.instances[0].instance_guid);
}

// ---------------------------------------------------------------- thumbnail lighting (cached)

fn cache_dir(opts: &BuildOptions, game: &Game) -> PathBuf {
    opts.cache_dir
        .clone()
        .unwrap_or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| game.root().to_path_buf()).join("Caveman").join("cache"))
}

/// (size, mtime) of Skate.exe: the cache key (the fit only changes with the game).
fn exe_key(game: &Game) -> Result<(u64, i64)> {
    let st = std::fs::metadata(game.root().join("Skate.exe"))?;
    let mtime = st.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    Ok((st.len(), mtime))
}

/// A cache file of f64 arrays: magic, count, then per array its length and values (little-endian).
fn save_arrays(path: &Path, arrays: &[&[f64]]) -> std::io::Result<()> {
    let mut out = b"CAVEMAN-LIGHT-1\0".to_vec();
    out.extend_from_slice(&(arrays.len() as u32).to_le_bytes());
    for a in arrays {
        out.extend_from_slice(&(a.len() as u64).to_le_bytes());
        for v in *a {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, out)
}

fn load_arrays(path: &Path) -> Option<Vec<Vec<f64>>> {
    let data = std::fs::read(path).ok()?;
    if data.get(..16)? != b"CAVEMAN-LIGHT-1\0" {
        return None;
    }
    let n = u32::from_le_bytes(data.get(16..20)?.try_into().ok()?) as usize;
    let mut at = 20;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let len = u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?) as usize;
        at += 8;
        let bytes = data.get(at..at + len * 8)?;
        out.push(bytes.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect());
        at += len * 8;
    }
    (at == data.len()).then_some(out)
}

/// The deck thumbnail lighting at 768 and 256 px: light, sheen, alpha and the metal light (newdeck.thumbnail_lighting),
/// cached by the Skate.exe it was fitted from.
struct DeckLighting {
    by_size: HashMap<usize, (Lighting, Field)>,
}

fn deck_lighting(game: &Game, opts: &BuildOptions, log: &mut dyn FnMut(&str)) -> Result<DeckLighting> {
    let (size, mtime) = exe_key(game)?;
    let path = cache_dir(opts, game).join(format!("thumbnail-lighting-{size}-{mtime}-{LIGHT_DECKS}-metal{}grey.rs.bin", METAL_DECKS.len()));
    if let Some(arrays) = load_arrays(&path) {
        if arrays.len() == 8 {
            let mut by_size = HashMap::new();
            for (k, s) in [768usize, 256].iter().enumerate() {
                let a = &arrays[k * 4..k * 4 + 4];
                if a[0].len() != s * s * 3 || a[1].len() != s * s * 3 || a[2].len() != s * s || a[3].len() != s * s {
                    by_size.clear();
                    break;
                }
                let lighting = Lighting {
                    light: Field { width: *s, height: *s, channels: 3, data: a[0].clone() },
                    sheen: Field { width: *s, height: *s, channels: 3, data: a[1].clone() },
                    alpha: a[2].clone(),
                };
                by_size.insert(*s, (lighting, Field { width: *s, height: *s, channels: 1, data: a[3].clone() }));
            }
            if by_size.len() == 2 {
                log("  thumbnail lighting from the cache");
                return Ok(DeckLighting { by_size });
            }
        }
    }
    let fitted = fit_deck_lighting(game, log)?;
    let mut arrays: Vec<&[f64]> = Vec::new();
    for s in [768usize, 256] {
        let (l, m) = &fitted.by_size[&s];
        arrays.extend([l.light.data.as_slice(), l.sheen.data.as_slice(), l.alpha.as_slice(), m.data.as_slice()]);
    }
    if let Err(e) = save_arrays(&path, &arrays) {
        log(&format!("  note: could not cache the thumbnail lighting ({e})"));
    }
    Ok(fitted)
}

fn fit_deck_lighting(game: &Game, log: &mut dyn FnMut(&str)) -> Result<DeckLighting> {
    let shared = game.bundle(SHARED, ITEMS_TOC)?;
    let names: std::collections::HashSet<&str> = shared.manifest.res.iter().map(|r| r.name.as_str()).collect();
    let mut candidates = Vec::new();
    for b in bundle_names(game)? {
        if b.contains("/deck/common/generic/graphic/deck_gen_graphic_") && b.ends_with("_ap_cas_main_bundlereftable") {
            let n = b.rsplit("deck_gen_graphic_").next().unwrap().split('_').next().unwrap().to_string();
            let thumbs_exist = ["", "_lrg"].iter().all(|s| names.contains(format!("thumbnail/tool/own_deckgraphic_gen_popsicle_{n}{s}").as_str()));
            if !SKIP_LIGHT.contains(&n.as_str()) && thumbs_exist {
                candidates.push(n);
            }
        }
    }
    let step = (candidates.len() / LIGHT_DECKS).max(1);
    let training: Vec<String> = candidates.iter().step_by(step).take(LIGHT_DECKS).cloned().collect();
    let art_of = |brand: &str, n: &str| -> Result<Image> {
        let b = game.bundle(&find_bundle(game, &format!("/deck_{brand}_graphic_{n}_ap_cas_main_bundlereftable"))?, ITEMS_TOC)?;
        let suffix = format!("/deck_{brand}_graphic_{n}_c");
        let name = b.manifest.res.iter().find(|r| r.name.ends_with(&suffix)).map(|r| r.name.clone()).ok_or_else(|| BuildError(format!("no art {suffix}")))?;
        texture_image(game, &b, &name)
    };
    let arts: Vec<Image> = training.iter().map(|n| art_of("gen", n)).collect::<Result<_>>()?;
    let metal_arts: Vec<Image> = METAL_DECKS.iter().map(|(b, n)| art_of(b, n)).collect::<Result<_>>()?;
    let m = Mapping::default();
    let mut by_size = HashMap::new();
    for (suffix, size) in [("_lrg", 768usize), ("", 256)] {
        let pairs: Vec<(Image, Image)> = arts
            .iter()
            .zip(&training)
            .map(|(a, n)| Ok((a.clone(), texture_image(game, &shared, &format!("thumbnail/tool/own_deckgraphic_gen_popsicle_{n}{suffix}"))?)))
            .collect::<Result<_>>()?;
        let metal_pairs: Vec<(Image, Image)> = metal_arts
            .iter()
            .zip(METAL_DECKS)
            .map(|(a, (b, n))| Ok((a.clone(), texture_image(game, &shared, &format!("thumbnail/tool/own_deckgraphic_{b}_popsicle_{n}{suffix}"))?)))
            .collect::<Result<_>>()?;
        by_size.insert(size, (thumbs::fit_light(&pairs, size, &m), thumbs::fit_metal(&metal_pairs, size, &m)));
    }
    log(&format!("  thumbnail lighting fitted from {} stock decks and {} metal ones", training.len(), METAL_DECKS.len()));
    Ok(DeckLighting { by_size })
}

/// The wheel thumbnail lighting at 768 and 256 px (newwheel.thumbnail_lighting), cached likewise.
fn wheel_lighting(game: &Game, opts: &BuildOptions, log: &mut dyn FnMut(&str)) -> Result<HashMap<usize, Lighting>> {
    let (size, mtime) = exe_key(game)?;
    let path = cache_dir(opts, game).join(format!("wheel-thumbnail-lighting-{size}-{mtime}-{}.rs.bin", thumbs::WHEEL_FACE_SCALE));
    if let Some(arrays) = load_arrays(&path) {
        if arrays.len() == 6 {
            let mut out = HashMap::new();
            for (k, s) in [768usize, 256].iter().enumerate() {
                let a = &arrays[k * 3..k * 3 + 3];
                if a[0].len() == s * s * 3 && a[1].len() == s * s * 3 && a[2].len() == s * s {
                    out.insert(
                        *s,
                        Lighting {
                            light: Field { width: *s, height: *s, channels: 3, data: a[0].clone() },
                            sheen: Field { width: *s, height: *s, channels: 3, data: a[1].clone() },
                            alpha: a[2].clone(),
                        },
                    );
                }
            }
            if out.len() == 2 {
                log("  wheel thumbnail lighting from the cache");
                return Ok(out);
            }
        }
    }
    let shared = game.bundle(SHARED, ITEMS_TOC)?;
    let names: std::collections::HashSet<&str> = shared.manifest.res.iter().map(|r| r.name.as_str()).collect();
    let mut pairs: Vec<(Field, Image, Image)> = Vec::new();
    for n in bundle_names(game)? {
        if !n.contains("/wheelgraphic/wheel_gen_graphic_") || !n.ends_with("_ap_cas_main_bundlereftable") {
            continue;
        }
        let num: String = n.split("wheel_gen_graphic_").nth(1).unwrap_or("").chars().take(5).collect();
        let thumb = [format!("thumbnail/tool/own_wheelgraphic_gen_classic_{num}"), format!("thumbnail/tool/own_wheelgraphic_gen_classic{num}")]
            .into_iter()
            .find(|t| names.contains(t.as_str()) && names.contains(format!("{t}_lrg").as_str()));
        let Some(thumb) = thumb else { continue };
        let b = game.bundle(&n, ITEMS_TOC)?;
        let res = |suffix: &str| b.manifest.res.iter().find(|r| r.name.ends_with(&format!("wheel_gen_graphic_{num}{suffix}"))).map(|r| r.name.clone());
        let ap = b.manifest.ebx.iter().find(|e| e.name.ends_with(&format!("wheel_gen_graphic_{num}_ap"))).map(|e| e.name.clone());
        let (Some(c), Some(o), Some(ap)) = (res("_c"), res("_osk"), ap) else { continue };
        let (art, mask) = (texture_image(game, &b, &c)?, texture_image(game, &b, &o)?);
        if (art.width, art.height) != (512, 512) || (mask.width, mask.height) != (512, 512) {
            continue;
        }
        let Some(colour) = region_colour(&ebx::read_document(&bundle_ebx(game, &b, &ap)?)?) else { continue };
        let seen = thumbs::wheel_seen_texture(&art, &mask, colour.map(|c| c as f64));
        pairs.push((seen, texture_image(game, &shared, &format!("{thumb}_lrg"))?, texture_image(game, &shared, &thumb)?));
    }
    log(&format!("  wheel thumbnail lighting: {} stock wheels", pairs.len()));
    let mut out = HashMap::new();
    for (k, size) in [768usize, 256].iter().enumerate() {
        let p: Vec<(Field, Image)> = pairs.iter().map(|(t, a, b)| (t.clone(), if k == 0 { a.clone() } else { b.clone() })).collect();
        out.insert(*size, thumbs::fit_wheel_light(&p, *size));
    }
    let mut arrays: Vec<&[f64]> = Vec::new();
    for s in [768usize, 256] {
        let l = &out[&s];
        arrays.extend([l.light.data.as_slice(), l.sheen.data.as_slice(), l.alpha.as_slice()]);
    }
    if let Err(e) = save_arrays(&path, &arrays) {
        log(&format!("  note: could not cache the wheel thumbnail lighting ({e})"));
    }
    Ok(out)
}

// ---------------------------------------------------------------- appearance preset parameters

/// The index into `doc.boxed_values` of each parameter's boxed value, by ParamKey GUID text (newwheel._param_records).
pub(crate) fn param_records(doc: &Document) -> HashMap<String, usize> {
    let by_offset: HashMap<usize, usize> = doc.boxed_values.iter().enumerate().map(|(i, r)| (r.offset, i)).collect();
    let mut out = HashMap::new();
    let Ok(root) = doc.object(0) else { return out };
    let Some(infos) = root.find("ParamInfo").and_then(|f| f.value.data.as_array()) else { return out };
    for v in infos {
        let Some(info) = v.data.as_object() else { continue };
        let key = info.get("ParamKey.Data").ok().and_then(|d| d.as_guid());
        let boxed = info.get("Value.Value").ok();
        if let (Some(key), Some(Data::Boxed(b))) = (key, boxed) {
            if b.data_offset >= 0 {
                if let Some(&i) = by_offset.get(&(b.data_offset as usize)) {
                    out.insert(key.to_string(), i);
                }
            }
        }
    }
    out
}

/// A wheel preset's colour (linear rgb), or None (newwheel.region_colour).
fn region_colour(doc: &Document) -> Option<[f32; 3]> {
    let i = *param_records(doc).get(REGION_COLOUR)?;
    let raw = &doc.boxed_values[i].raw_bytes;
    let f = |k: usize| raw.get(k * 4..k * 4 + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap()));
    Some([f(0)?, f(1)?, f(2)?])
}

fn set_region_colour(doc: &mut Document, linear: [f64; 3]) -> Result<()> {
    let i = *param_records(doc).get(REGION_COLOUR).ok_or_else(|| BuildError("the wheel preset has no param_region_color_0".into()))?;
    let raw = &mut doc.boxed_values[i].raw_bytes;
    if raw.len() < 12 {
        return fail("param_region_color_0 is too short");
    }
    for (k, v) in linear.iter().enumerate() {
        raw[k * 4..k * 4 + 4].copy_from_slice(&(*v as f32).to_le_bytes());
    }
    Ok(())
}

/// The boxed parameters of a host deck's appearance preset: (ParamKey GUID, the value's raw bytes), for finding a
/// parameter's key (a float is 4 bytes, a colour 12 or 16; the texture slots are references, not boxed here).
pub fn preset_params(game: &Game, host: Host) -> Result<Vec<(String, Vec<u8>)>> {
    let h = load_host(game, host)?;
    let doc = ebx::read_document(&h.ap)?;
    let mut out: Vec<(String, Vec<u8>)> = param_records(&doc).into_iter().map(|(k, i)| (k, doc.boxed_values[i].raw_bytes.clone())).collect();
    out.sort();
    Ok(out)
}

/// The same for any deck's appearance preset: `stock` is the deck's path as [`Host::stock`] gives a host's (its
/// bundle is `win32/<stock>_ap_cas_main_bundlereftable`, its preset `<stock>_ap`).
pub fn preset_params_of(game: &Game, stock: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let b = game.bundle(&format!("win32/{stock}_ap_cas_main_bundlereftable"), ITEMS_TOC)?;
    let doc = ebx::read_document(&bundle_ebx(game, &b, &format!("{stock}_ap"))?)?;
    let mut out: Vec<(String, Vec<u8>)> = param_records(&doc).into_iter().map(|(k, i)| (k, doc.boxed_values[i].raw_bytes.clone())).collect();
    out.sort();
    Ok(out)
}

/// Sets a boxed float parameter of an appearance preset by its ParamKey GUID (Err when the preset has no such
/// parameter: a preset only carries the parameters its host's does).
pub fn set_float_param(doc: &mut Document, key: &str, value: f32) -> Result<()> {
    let i = *param_records(doc).get(key).ok_or_else(|| BuildError(format!("the preset has no parameter {key}")))?;
    let raw = &mut doc.boxed_values[i].raw_bytes;
    if raw.len() < 4 {
        return fail(format!("parameter {key} is too short for a float"));
    }
    raw[..4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Sets a 16-byte colour parameter (three linear floats; the fourth word, a 1, is kept), such as a grip's tint.
pub fn set_vec3_param(doc: &mut Document, key: &str, value: [f32; 3]) -> Result<()> {
    let i = *param_records(doc).get(key).ok_or_else(|| BuildError(format!("the preset has no parameter {key}")))?;
    let raw = &mut doc.boxed_values[i].raw_bytes;
    if raw.len() < 12 {
        return fail(format!("parameter {key} is too short for a colour"));
    }
    for (k, v) in value.iter().enumerate() {
        raw[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    Ok(())
}

// ---------------------------------------------------------------- the build

/// The item and thumbnail templates of one kind of item, and how to name its thumbnails (newdeck.build, newwheel.Templates).
struct ItemTemplates {
    item: Vec<u8>,
    thumb: [Vec<u8>; 2],
    thumb_res: [(Vec<u8>, ManifestRes); 2],
    cdn: [Vec<u8>; 2],
    cdn_refs: [(Guid, Guid); 2],
    thumb_refs: [(Guid, Guid); 2],
}

impl ItemTemplates {
    fn load(game: &Game, shared: &game::Bundle, item: &str, thumb: &str) -> Result<ItemTemplates> {
        let t = |s: &str| bundle_ebx(game, shared, &format!("thumbnail/tool/{thumb}{s}"));
        let c = |s: &str| bundle_ebx(game, shared, &format!("thumbnail/cdn/img_{thumb}{s}"));
        let r = |s: &str| bundle_res(game, shared, &format!("thumbnail/tool/{thumb}{s}"));
        let thumb_docs = [t("")?, t("_lrg")?];
        let cdn_docs = [c("")?, c("_lrg")?];
        Ok(ItemTemplates {
            item: bundle_ebx(game, shared, item)?,
            thumb_refs: [ebx::read_document(&thumb_docs[0])?.reference(), ebx::read_document(&thumb_docs[1])?.reference()],
            cdn_refs: [ebx::read_document(&cdn_docs[0])?.reference(), ebx::read_document(&cdn_docs[1])?.reference()],
            thumb: thumb_docs,
            cdn: cdn_docs,
            thumb_res: [r("")?, r("_lrg")?],
        })
    }
}

/// Adds an item's thumbnails (TOC-only chunks) and CDN records (newdeck.build): `images` are the 256 and 768 px
/// renders. Returns the CDN records' references.
fn add_thumbnails(rec: &mut Recorder, ids: &mut Ids, t: &ItemTemplates, base: &str, images: [&Image; 2]) -> Result<[(Guid, Guid); 2]> {
    let mut refs = [(Guid::default(), Guid::default()); 2];
    for (k, suffix) in ["", "_lrg"].iter().enumerate() {
        let th_name = format!("thumbnail/tool/own_{base}{suffix}");
        let tenc = cas::encode(&bc7::encode(images[k]), rec.root)?;
        let tchunk = ids.chunk_guid();
        let (th_hdr, th_res) = &t.thumb_res[k];
        let mut h2 = th_hdr.clone();
        h2[40..56].copy_from_slice(&tchunk.le_bytes());
        h2[120..128].copy_from_slice(&texture::owner_hash(&th_name).to_le_bytes());
        let trid = ids.rid();
        let mut th = clone_doc(&t.thumb[k], &th_name, ids)?;
        th.set("Resource", ResourceRef::new(trid), 0)?;
        rec.ebx(SHARED, &th_name, &write(&th)?)?;
        rec.res(SHARED, &th_name, th_res.rtype, th_res.meta, trid, &h2)?;
        rec.chunk(None, tchunk, tenc, None);
        let cdn_name = format!("thumbnail/cdn/img_own_{base}{suffix}");
        let mut cdn = clone_doc(&t.cdn[k], &cdn_name, ids)?;
        cdn.remap_imports(&HashMap::from([(t.thumb_refs[k].0, th.file_guid), (t.thumb_refs[k].1, th.instances[0].instance_guid)]));
        cdn.set("NameHash", Data::UInt(djb32(&cdn_name) as u64), 0)?;
        rec.ebx(SHARED, &cdn_name, &write(&cdn)?)?;
        refs[k] = cdn.reference();
    }
    Ok(refs)
}

/// A new item cloned from `t.item`: Key, HashedAssetKey, its CDN records, AssetName. Returns it and the index of its
/// DelMarCustomizationItemData.
fn new_item(t: &ItemTemplates, ids: &mut Ids, item_name: &str, key: &str, base: &str, cdn_refs: [(Guid, Guid); 2]) -> Result<(Document, usize)> {
    new_item_for(t, ids, item_name, key, &format!("{base}_ap"), cdn_refs)
}

/// `new_item` with the asset the item names given whole (`<base>_ap` for a preset, `<base>_osk` for a cutout texture).
fn new_item_for(t: &ItemTemplates, ids: &mut Ids, item_name: &str, key: &str, asset: &str, cdn_refs: [(Guid, Guid); 2]) -> Result<(Document, usize)> {
    let mut item = clone_doc(&t.item, item_name, ids)?;
    item.set("Key", key, 0)?;
    item.set("HashedAssetKey", Data::UInt(djb32(key) as u64), 0)?;
    let mut mapping = HashMap::new();
    for k in 0..2 {
        mapping.insert(t.cdn_refs[k].0, cdn_refs[k].0);
        mapping.insert(t.cdn_refs[k].1, cdn_refs[k].1);
    }
    item.remap_imports(&mapping);
    let di = *item.instances_of("DelMarCustomizationItemData").first().ok_or_else(|| BuildError("no DelMarCustomizationItemData".into()))?;
    item.set("AssetPaths[0].AssetName", asset.to_string(), di)?;
    Ok((item, di))
}

/// The base name of an item key: `Own_000_-_Cool_Deck` -> `000_-_cool_deck`.
pub fn base_name(key: &str) -> String {
    key.strip_prefix("Own_").unwrap_or(key).to_lowercase()
}

struct LoadedHost {
    name: String,
    ap: Vec<u8>,
    textures: HashMap<&'static str, TextureTemplate>,
}

fn load_host(game: &Game, host: Host) -> Result<LoadedHost> {
    let name = host.bundle();
    let b = game.bundle(&name, ITEMS_TOC)?;
    let records = chunk_records(&b)?;
    let mut textures = HashMap::new();
    for (slot, tex) in host.textures() {
        textures.insert(slot, texture_template(game, &b, &records, &tex)?);
    }
    Ok(LoadedHost { ap: bundle_ebx(game, &b, &format!("{}_ap", host.stock()))?, name, textures })
}

/// What building decks needs from the game, loaded once per build (only when some deck is not in the item cache).
struct DeckCtx {
    t: ItemTemplates,
    lighting: DeckLighting,
    defaults: (Image, Image),
    hosts: HashMap<Host, LoadedHost>,
    vfx: (Vec<u8>, Vec<u8>),
    mapping: Mapping,
}

fn deck_ctx(game: &Game, shared: &game::Bundle, opts: &BuildOptions, log: &mut dyn FnMut(&str)) -> Result<DeckCtx> {
    let t = ItemTemplates::load(
        game,
        shared,
        &format!("items/cust_board/own_deck_gen_popsiclegraphic_{HOST_NUM}"),
        &format!("own_deckgraphic_gen_popsicle_{HOST_NUM}"),
    )?;
    let lighting = deck_lighting(game, opts, log)?;
    let mut hosts = HashMap::new();
    for host in [Host::Plain, Host::Metal, Host::Holo] {
        hosts.insert(host, load_host(game, host)?);
    }
    Ok(DeckCtx {
        t,
        lighting,
        defaults: default_layers(game)?,
        hosts,
        vfx: (bundle_ebx(game, shared, effects::BLUEPRINT)?, bundle_ebx(game, shared, effects::ASSET)?),
        mapping: Mapping::default(),
    })
}

/// What building wheels needs from the game, likewise.
struct WheelCtx {
    t: ItemTemplates,
    lighting: HashMap<usize, Lighting>,
    host_name: String,
    art_t: TextureTemplate,
    osk_t: TextureTemplate,
    host_mask: Image,
    ap_t: Vec<u8>,
}

fn wheel_ctx(game: &Game, shared: &game::Bundle, opts: &BuildOptions, log: &mut dyn FnMut(&str)) -> Result<WheelCtx> {
    let t = ItemTemplates::load(game, shared, WHEEL_HOST_ITEM, WHEEL_HOST_THUMB)?;
    let lighting = wheel_lighting(game, opts, log)?;
    let host_name = format!("win32/{WHEEL_HOST}_ap_cas_main_bundlereftable");
    let hb = game.bundle(&host_name, ITEMS_TOC)?;
    let records = chunk_records(&hb)?;
    Ok(WheelCtx {
        t,
        lighting,
        art_t: texture_template(game, &hb, &records, &format!("{WHEEL_HOST}_c"))?,
        osk_t: texture_template(game, &hb, &records, &format!("{WHEEL_HOST}_osk"))?,
        host_mask: texture_image(game, &hb, &format!("{WHEEL_HOST}_osk"))?,
        ap_t: bundle_ebx(game, &hb, &format!("{WHEEL_HOST}_ap"))?,
        host_name,
    })
}

/// Everything one deck adds to the mod (newdeck.build's per-deck part), recorded.
fn deck_product(ctx: &DeckCtx, root: &Path, spec: &DeckSpec, ids: &mut Ids) -> Result<Product> {
    let loaded;
    let (art, layers) = match &spec.source {
        DeckSource::Ready { art, layers } => (art, layers),
        DeckSource::Lazy(load) => {
            loaded = load().map_err(|e| BuildError(format!("{}: {e}", spec.display)))?;
            (&loaded.0, &loaded.1)
        }
    };
    let layers = match layers {
        DeckLayers::Layers(l) => l.clone(),
        DeckLayers::Effects(e) => effects::layers(e, art, &ctx.defaults.0, &ctx.defaults.1),
    };
    let slot_param = |slot: &str| match slot {
        "osk" => SLOT_OSK,
        "ny" => SLOT_NY,
        _ => SLOT_RGB,
    };
    let host = Host::for_layers(&layers);
    let h = &ctx.hosts[&host];
    let host_name = h.name.clone();
    let base = base_name(&spec.key);
    let ap_name = format!("{PREFIX}{base}_ap");
    let item_name = format!("items/board_bottomart/own_{base}");
    let mut rec = Recorder::new(root);
    // the art, and the deck's own mask, normal map and holo map when it has them
    let art = art.to_rgb();
    let mut remap = HashMap::new();
    let tex = add_texture(
        &mut rec,
        ids,
        &host_name,
        &h.textures["art"],
        &format!("{PREFIX}{base}_role3_{SLOT_ART}"),
        &mip_chain(&art, DECK_LEVELS, false, bc7::encode),
    )?;
    remap_template(&mut remap, &h.textures["art"].doc, &tex);
    for (slot, img) in [("osk", &layers.osk), ("ny", &layers.ny), ("rgb", &layers.rgb)] {
        let Some(img) = img else { continue };
        let template = h.textures.get(slot).ok_or_else(|| BuildError(format!("the {host:?} host has no {slot} slot")))?;
        let img = img.to_rgb();
        let name = format!("{PREFIX}{base}_role3_{}", slot_param(slot));
        let tex = if (img.width, img.height) == ART_SIZE {
            let pixels = mip_chain(&img, DECK_LEVELS, slot == "ny", bc7::encode);
            add_texture(&mut rec, ids, &host_name, template, &name, &pixels)?
        } else if slot == "rgb" {
            // an animated deck's frame atlas (anim.rs): the holo map at the atlas's size, a full mip chain
            if !img.width.is_power_of_two() || !img.height.is_power_of_two() || img.width.max(img.height) > MAX_SIDE {
                return fail(format!("{}: a {} x {} atlas (power-of-two sides up to {MAX_SIDE})", spec.display, img.width, img.height));
            }
            let levels = texture::mip_levels(img.width, img.height);
            let pixels = mip_chain(&img, levels, false, bc7::encode);
            add_texture_sized(&mut rec, ids, &host_name, template, &name, &pixels, Some((img.width, img.height, levels)))?
        } else {
            return fail(format!("{}: the {slot} layer is {} x {}, not {} x {}", spec.display, img.width, img.height, ART_SIZE.0, ART_SIZE.1));
        };
        remap_template(&mut remap, &template.doc, &tex);
    }
    if let Some(mode) = layers.foilmode {
        remap.extend(effects::foil_mode_remap(mode));
    }
    // the appearance preset: the host's, its slots pointed at the new textures
    let mut ap = clone_doc(&h.ap, &ap_name, ids)?;
    ap.remap_imports(&remap);
    for (key, value) in &layers.preset_floats {
        set_float_param(&mut ap, key, *value)?;
    }
    rec.ebx(&host_name, &ap_name, &write(&ap)?)?;
    // thumbnails: rendered on the board (metal decks with the metal light)
    let metal_mask = layers.osk.as_ref().map(|m| m.channel(2));
    let render = |size: usize| {
        let (l, metal_light) = &ctx.lighting.by_size[&size];
        thumbs::render(&art, l, metal_mask.as_ref().map(|m| (m, metal_light)), &ctx.mapping)
    };
    let (small, large) = (render(256), render(768));
    let icon = thumbs::render(&art, &ctx.lighting.by_size[&256].0, None, &ctx.mapping);
    let cdn_refs = add_thumbnails(&mut rec, ids, &ctx.t, &base, [&small, &large])?;
    // the item, and its board effect
    let (mut item, di) = new_item(&ctx.t, ids, &item_name, &spec.key, &base, cdn_refs)?;
    if let Some(board) = layers.board {
        let (tb, ta) = &ctx.vfx;
        let style = match board {
            ResolvedBoard::Aura(_) | ResolvedBoard::AuraTuned { .. } => "aura",
            ResolvedBoard::Nyan { .. } => "nyan",
            ResolvedBoard::Burst(_) => "burst",
            ResolvedBoard::Stock { .. } => "fx",
        };
        let (bp_name, va_name) = (format!("{}ebp_{base}_{style}", effects::VFX_PREFIX), format!("{}{base}_{style}", effects::VFX_PREFIX));
        let (blueprint, points): ((ebx::Guid, ebx::Guid), &[[f64; 3]]) = match board {
            // the game's own blueprint (or one the mod ships on its own), imported as it is
            ResolvedBoard::Burst(stock) => (stock, &effects::NYAN_POINTS),
            ResolvedBoard::Stock { blueprint, at } => (blueprint, at.points()),
            _ => {
                let mut bp = clone_doc(tb, &bp_name, ids)?;
                let points: &[[f64; 3]] = match board {
                    ResolvedBoard::Aura(colour) => {
                        effects::aura_blueprint(&mut bp, colour)?;
                        &effects::POINTS
                    }
                    ResolvedBoard::AuraTuned { colour, tuning } => {
                        effects::aura_blueprint_tuned(&mut bp, colour, &tuning)?;
                        &effects::POINTS
                    }
                    ResolvedBoard::Nyan { steps, trigger } => {
                        effects::nyan_blueprint(&mut bp, steps, trigger)?;
                        &effects::NYAN_POINTS
                    }
                    ResolvedBoard::Burst(_) | ResolvedBoard::Stock { .. } => unreachable!(),
                };
                rec.ebx(SHARED, &bp_name, &write(&bp)?)?;
                (bp.reference(), points)
            }
        };
        let mut va = clone_doc(ta, &va_name, ids)?;
        let identity = ids.i64();
        // more points than the template's two attachments copy the first (only then are more ids drawn)
        effects::effect_asset_at(&mut va, blueprint, points, identity, &mut || ids.guid())?;
        if let Some(t) = &layers.trigger {
            effects::trigger(&mut va, t)?;
        }
        rec.ebx(SHARED, &va_name, &write(&va)?)?;
        effects::attach(&mut item, di, va.reference())?;
    } else if !layers.combo.is_empty() {
        // several state-fired board effects on one deck (Caveman FX's combo decks): a blueprint per aura or rainbow,
        // the game's own for a burst, and one asset with an effect item per part
        let (tb, ta) = &ctx.vfx;
        let mut parts = Vec::new();
        for (k, (board, trigger)) in layers.combo.iter().enumerate() {
            let (blueprint, points): ((ebx::Guid, ebx::Guid), &[[f64; 3]]) = match *board {
                ResolvedBoard::Burst(stock) => (stock, &effects::NYAN_POINTS),
                ResolvedBoard::Stock { blueprint, at } => (blueprint, at.points()),
                _ => {
                    let style = if matches!(board, ResolvedBoard::Nyan { .. }) { "nyan" } else { "aura" };
                    let bp_name = format!("{}ebp_{base}_combo{k}_{style}", effects::VFX_PREFIX);
                    let mut bp = clone_doc(tb, &bp_name, ids)?;
                    let points: &[[f64; 3]] = match *board {
                        ResolvedBoard::Aura(colour) => {
                            effects::aura_blueprint(&mut bp, colour)?;
                            &effects::POINTS
                        }
                        ResolvedBoard::AuraTuned { colour, tuning } => {
                            effects::aura_blueprint_tuned(&mut bp, colour, &tuning)?;
                            &effects::POINTS
                        }
                        ResolvedBoard::Nyan { steps, trigger } => {
                            effects::nyan_blueprint(&mut bp, steps, trigger)?;
                            &effects::NYAN_POINTS
                        }
                        ResolvedBoard::Burst(_) | ResolvedBoard::Stock { .. } => unreachable!(),
                    };
                    rec.ebx(SHARED, &bp_name, &write(&bp)?)?;
                    (bp.reference(), points)
                }
            };
            parts.push(effects::ComboPart { blueprint, points: points.to_vec(), trigger: *trigger });
        }
        let va_name = format!("{}{base}_combo", effects::VFX_PREFIX);
        let mut va = clone_doc(ta, &va_name, ids)?;
        effects::combo_asset(&mut va, &parts, &mut || ids.guid())?;
        rec.ebx(SHARED, &va_name, &write(&va)?)?;
        effects::attach(&mut item, di, va.reference())?;
    }
    rec.ebx(SHARED, &item_name, &write(&item)?)?;
    Ok(Product {
        display: spec.display.clone(),
        base,
        notes: spec.notes.clone(),
        ops: rec.ops,
        preset: (ap_name.to_lowercase(), host_name),
        item_ref: item.reference(),
        thumbnail: large,
        icon: Some(icon),
    })
}

/// Everything one wheel adds to the mod (newwheel.add_wheel), recorded.
fn wheel_product(ctx: &WheelCtx, root: &Path, spec: &WheelSpec, ids: &mut Ids) -> Result<Product> {
    let loaded;
    let face = match &spec.source {
        WheelSource::Ready(face) => face,
        WheelSource::Lazy(load) => {
            loaded = load().map_err(|e| BuildError(format!("{}: {e}", spec.display)))?;
            &loaded
        }
    };
    let base = base_name(&spec.key);
    let ap_name = format!("{WHEEL_PREFIX}{base}_ap");
    let item_name = format!("{WHEEL_ITEMS}own_{base}");
    let linear = spec.colour.map(|c| thumbs::srgb_to_linear(c as f64));
    let (art, mask) = wheel_textures(face, spec.colour, &ctx.host_mask);
    let mut rec = Recorder::new(root);
    let host_name = &ctx.host_name;
    let tex_art = add_texture(
        &mut rec,
        ids,
        host_name,
        &ctx.art_t,
        &format!("{WHEEL_PREFIX}{base}_role3_{SLOT_ART}"),
        &mip_chain(&art, WHEEL_LEVELS, false, bc7::encode),
    )?;
    let tex_osk = add_texture(
        &mut rec,
        ids,
        host_name,
        &ctx.osk_t,
        &format!("{WHEEL_PREFIX}{base}_role3_{SLOT_OSK}"),
        &mip_chain(&mask, WHEEL_LEVELS, false, bc1::encode),
    )?;
    let mut remap = HashMap::new();
    remap_template(&mut remap, &ctx.art_t.doc, &tex_art);
    remap_template(&mut remap, &ctx.osk_t.doc, &tex_osk);
    let mut ap = clone_doc(&ctx.ap_t, &ap_name, ids)?;
    ap.remap_imports(&remap);
    set_region_colour(&mut ap, linear)?;
    rec.ebx(host_name, &ap_name, &write(&ap)?)?;
    let field = Field::from_image(&art);
    let (small, large) = (thumbs::render_wheel(&field, &ctx.lighting[&256]), thumbs::render_wheel(&field, &ctx.lighting[&768]));
    let cdn_refs = add_thumbnails(&mut rec, ids, &ctx.t, &base, [&small, &large])?;
    let (mut item, di) = new_item(&ctx.t, ids, &item_name, &spec.key, &base, cdn_refs)?;
    for (axis, v) in ["x", "y", "z"].iter().zip(linear) {
        item.set(&format!("BaseColor.{axis}"), Data::F64(v), di)?;
    }
    rec.ebx(SHARED, &item_name, &write(&item)?)?;
    Ok(Product {
        display: spec.display.clone(),
        base,
        notes: spec.notes.clone(),
        ops: rec.ops,
        preset: (ap_name.to_lowercase(), host_name.clone()),
        item_ref: item.reference(),
        thumbnail: large,
        icon: None,
    })
}

/// What a thumbnail of the deck's top needs (thumbs::render_top): the wood's colour map and mask (the top face in
/// their left half, the bolt heads where the mask's B is set) and the host item's thumbnails, whose alpha is the
/// deck's shape from above at 256 and 768 px.
struct TopView {
    wood: Image,
    wood_mask: Image,
    shapes: [Image; 2],
}

fn top_view(game: &Game, shared: &game::Bundle, grip_bundle: &game::Bundle, thumb: &str) -> Result<TopView> {
    Ok(TopView {
        wood: texture_image(game, grip_bundle, GRIP_WOOD_C)?,
        wood_mask: texture_image(game, grip_bundle, GRIP_WOOD_MSK)?,
        shapes: [texture_image(game, shared, &format!("thumbnail/tool/{thumb}"))?, texture_image(game, shared, &format!("thumbnail/tool/{thumb}_lrg"))?],
    })
}

/// What building grips needs from the game, loaded once per build: the host grip colour's item and thumbnail
/// templates, its preset and bundle, the template grit's colour map (the texture a printed grip replaces, and the
/// image a print is laid over) and what the thumbnails need.
struct GripCtx {
    t: ItemTemplates,
    host_name: String,
    ap_t: Vec<u8>,
    c_t: TextureTemplate,
    grit: Image,
    top: TopView,
}

fn grip_ctx(game: &Game, shared: &game::Bundle) -> Result<GripCtx> {
    let t = ItemTemplates::load(game, shared, GRIP_HOST_ITEM, GRIP_HOST_THUMB)?;
    let host_name = format!("win32/{GRIP_HOST}_ap_cas_main_bundlereftable");
    let hb = game.bundle(&host_name, ITEMS_TOC)?;
    let records = chunk_records(&hb)?;
    Ok(GripCtx {
        t,
        ap_t: bundle_ebx(game, &hb, &format!("{GRIP_HOST}_ap"))?,
        c_t: texture_template(game, &hb, &records, GRIP_TEMPLATE_C)?,
        grit: texture_image(game, &hb, GRIP_TEMPLATE_C)?,
        top: top_view(game, shared, &hb, GRIP_HOST_THUMB)?,
        host_name,
    })
}

/// What building grip patterns needs: the host cutout's item and thumbnail templates, its bundle and texture (the
/// template of a new cutout), the grit (black, for the thumbnails) and what the thumbnails need.
struct CutoutCtx {
    t: ItemTemplates,
    host_name: String,
    osk_t: TextureTemplate,
    grit: Image,
    top: TopView,
}

fn cutout_ctx(game: &Game, shared: &game::Bundle) -> Result<CutoutCtx> {
    let t = ItemTemplates::load(game, shared, CUTOUT_HOST_ITEM, CUTOUT_HOST_THUMB)?;
    let host_name = format!("win32/{CUTOUT_HOST}_cas_main_bundlereftable");
    let hb = game.bundle(&host_name, ITEMS_TOC)?;
    let records = chunk_records(&hb)?;
    let grip_bundle = game.bundle(&format!("win32/{GRIP_HOST}_ap_cas_main_bundlereftable"), ITEMS_TOC)?;
    Ok(CutoutCtx {
        t,
        osk_t: texture_template(game, &hb, &records, CUTOUT_HOST)?,
        grit: texture_image(game, &grip_bundle, GRIP_TEMPLATE_C)?,
        top: top_view(game, shared, &grip_bundle, CUTOUT_HOST_THUMB)?,
        host_name,
    })
}

/// The grit in a colour, as the game's tint shows it (boardmodel::coloured, which matches the menu). `colour` sRGB.
fn tinted_grit(grit: &Image, colour: [u8; 3]) -> Image {
    crate::boardmodel::coloured(grit, colour.map(|v| thumbs::srgb_to_linear(v as f64) as f32)).to_rgb()
}

/// Everything one grip adds to the mod, recorded: a plain colour is the host preset with the tint set (the template
/// textures stay, shared with the game's own grips); a print gets a colour map of its own (the picture over the
/// grit, in the colour when one is given) and a white tint.
fn grip_product(ctx: &GripCtx, root: &Path, spec: &GripSpec, ids: &mut Ids) -> Result<Product> {
    let base = base_name(&spec.key);
    let ap_name = format!("{GRIP_PREFIX}{base}_ap");
    let item_name = format!("{GRIP_ITEMS}own_{base}");
    let mut rec = Recorder::new(root);
    let mut remap = HashMap::new();
    let mut notes = spec.notes.clone();
    let under = match spec.colour {
        Some(c) => tinted_grit(&ctx.grit, c),
        None => ctx.grit.to_rgb(),
    };
    let top = match &spec.print {
        Some(print) => {
            let (fitted, fit_notes) = fit_frame(print)?;
            notes.extend(fit_notes);
            let c = under.to_rgba().alpha_composite(&fitted).to_rgb();
            let tex = add_texture(
                &mut rec,
                ids,
                &ctx.host_name,
                &ctx.c_t,
                &format!("{GRIP_PREFIX}{base}_role3_{SLOT_ART}"),
                &mip_chain(&c, GRIP_LEVELS, false, bc7::encode),
            )?;
            remap_template(&mut remap, &ctx.c_t.doc, &tex);
            c
        }
        None => under,
    };
    let mut ap = clone_doc(&ctx.ap_t, &ap_name, ids)?;
    ap.remap_imports(&remap);
    let tint = match (&spec.print, spec.colour) {
        (None, Some(c)) => c.map(|v| thumbs::srgb_to_linear(v as f64) as f32),
        _ => [1.0, 1.0, 1.0],
    };
    set_vec3_param(&mut ap, GRIP_TINT_KEY, tint)?;
    rec.ebx(&ctx.host_name, &ap_name, &write(&ap)?)?;
    let small = thumbs::render_top(&top, None, &ctx.top.wood, &ctx.top.wood_mask, &ctx.top.shapes[0]);
    let large = thumbs::render_top(&top, None, &ctx.top.wood, &ctx.top.wood_mask, &ctx.top.shapes[1]);
    let cdn_refs = add_thumbnails(&mut rec, ids, &ctx.t, &base, [&small, &large])?;
    let (mut item, di) = new_item_for(&ctx.t, ids, &item_name, &spec.key, &format!("{base}_ap"), cdn_refs)?;
    item.set("CustomizationItemId", "", di)?;
    if let Some(c) = spec.colour {
        for (axis, v) in ["x", "y", "z"].iter().zip(c) {
            item.set(&format!("BaseColor.{axis}"), Data::F64(thumbs::srgb_to_linear(v as f64)), di)?;
        }
    }
    rec.ebx(SHARED, &item_name, &write(&item)?)?;
    Ok(Product {
        display: spec.display.clone(),
        base,
        notes,
        ops: rec.ops,
        preset: (ap_name.to_lowercase(), ctx.host_name.clone()),
        item_ref: item.reference(),
        thumbnail: large,
        icon: None,
    })
}

/// Everything one grip pattern adds to the mod, recorded: a cutout texture (R the mask, G and B as the game's) in the
/// host cutout's bundle, named by the item as the game names its own, and thumbnails of black grip with the wood
/// through the holes.
fn cutout_product(ctx: &CutoutCtx, root: &Path, spec: &CutoutSpec, ids: &mut Ids) -> Result<Product> {
    if (spec.mask.width, spec.mask.height) != GRIP_SIZE {
        return fail(format!("{}: the mask is {} x {}, not {} x {}", spec.display, spec.mask.width, spec.mask.height, GRIP_SIZE.0, GRIP_SIZE.1));
    }
    let base = base_name(&spec.key);
    let tex_name = format!("{CUTOUT_PREFIX}{base}_osk");
    let item_name = format!("{CUTOUT_ITEMS}own_{base}");
    let mut rec = Recorder::new(root);
    let mut osk = Image::new(GRIP_SIZE.0, GRIP_SIZE.1, 3);
    for (dst, src) in osk.data.chunks_exact_mut(3).zip(spec.mask.data.chunks_exact(spec.mask.channels)) {
        dst.copy_from_slice(&[src[0], CUTOUT_GB[0], CUTOUT_GB[1]]);
    }
    add_texture(&mut rec, ids, &ctx.host_name, &ctx.osk_t, &tex_name, &mip_chain(&osk, GRIP_LEVELS, false, bc1::encode))?;
    let black = crate::boardmodel::coloured(&ctx.grit, GRIP_BLACK).to_rgb();
    let small = thumbs::render_top(&black, Some(&spec.mask), &ctx.top.wood, &ctx.top.wood_mask, &ctx.top.shapes[0]);
    let large = thumbs::render_top(&black, Some(&spec.mask), &ctx.top.wood, &ctx.top.wood_mask, &ctx.top.shapes[1]);
    let cdn_refs = add_thumbnails(&mut rec, ids, &ctx.t, &base, [&small, &large])?;
    let (item, _) = new_item_for(&ctx.t, ids, &item_name, &spec.key, &format!("{base}_osk"), cdn_refs)?;
    rec.ebx(SHARED, &item_name, &write(&item)?)?;
    Ok(Product {
        display: spec.display.clone(),
        base,
        notes: spec.notes.clone(),
        ops: rec.ops,
        preset: (tex_name.to_lowercase(), ctx.host_name.clone()),
        item_ref: item.reference(),
        thumbnail: large,
        icon: None,
    })
}

/// The resource ids and chunk GUIDs already in use: the game's, in the bundles a build patches and items.toc's
/// chunks, then every item the build has placed. A new item's ids must not clash with any of them.
struct Taken {
    rids: std::collections::HashSet<u64>,
    chunks: std::collections::HashSet<[u8; 16]>,
}

impl Taken {
    fn new(game: &Game, shared: &game::Bundle, decks: bool, wheels: bool, grips: bool, cutouts: bool) -> Result<Taken> {
        let mut t = Taken { rids: Default::default(), chunks: Default::default() };
        for c in &game.toc(ITEMS_TOC)?.toc.chunks {
            t.chunks.insert(c.guid.le_bytes());
        }
        let mut add = |m: &Manifest| {
            t.rids.extend(m.res.iter().map(|r| r.rid));
            t.chunks.extend(m.chunks.iter().map(|c| c.guid.le_bytes()));
        };
        add(&shared.manifest);
        let mut hosts: Vec<String> = Vec::new();
        if decks {
            hosts.extend([Host::Plain, Host::Metal, Host::Holo].map(Host::bundle));
        }
        if wheels {
            hosts.push(format!("win32/{WHEEL_HOST}_ap_cas_main_bundlereftable"));
        }
        if grips {
            hosts.push(format!("win32/{GRIP_HOST}_ap_cas_main_bundlereftable"));
        }
        if cutouts {
            hosts.push(format!("win32/{CUTOUT_HOST}_cas_main_bundlereftable"));
        }
        for h in hosts {
            add(&game.bundle(&h, ITEMS_TOC)?.manifest);
        }
        Ok(t)
    }

    fn clashes(&self, p: &Product) -> bool {
        let mut rids = std::collections::HashSet::new();
        let mut chunks = std::collections::HashSet::new();
        p.rids().any(|r| self.rids.contains(&r) || !rids.insert(r))
            || p.chunk_guids().any(|g| self.chunks.contains(&g.le_bytes()) || !chunks.insert(g.le_bytes()))
    }

    fn add(&mut self, p: &Product) {
        self.rids.extend(p.rids());
        self.chunks.extend(p.chunk_guids().map(|g| g.le_bytes()));
    }
}

/// Runs `f` for each of `items` on a few threads (the encoders inside already use every core for the big textures;
/// the rest of an item's work is single-threaded). Returns (item, result) in no particular order, or the first error.
fn in_parallel<T: Send>(items: &[usize], f: impl Fn(usize) -> Result<T> + Sync) -> Result<Vec<(usize, T)>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let workers = (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) / 2).clamp(1, 6).min(items.len().max(1));
    let next = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::with_capacity(items.len()));
    let error: std::sync::Mutex<Option<BuildError>> = std::sync::Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                if k >= items.len() || error.lock().unwrap().is_some() {
                    break;
                }
                match f(items[k]) {
                    Ok(v) => results.lock().unwrap().push((items[k], v)),
                    Err(e) => {
                        error.lock().unwrap().get_or_insert(e);
                        break;
                    }
                }
            });
        }
    });
    if let Some(e) = error.into_inner().unwrap() {
        return Err(e);
    }
    Ok(results.into_inner().unwrap())
}

/// What the cache needs to know about a deck or wheel spec.
trait ItemSpec: Sync {
    fn key(&self) -> &str;
    fn display(&self) -> &str;
    fn input(&self) -> Option<u64>;
}

impl ItemSpec for DeckSpec {
    fn key(&self) -> &str {
        &self.key
    }
    fn display(&self) -> &str {
        &self.display
    }
    fn input(&self) -> Option<u64> {
        self.input
    }
}

impl ItemSpec for WheelSpec {
    fn key(&self) -> &str {
        &self.key
    }
    fn display(&self) -> &str {
        &self.display
    }
    fn input(&self) -> Option<u64> {
        self.input
    }
}

/// Items taken from the cache or built, then placed, this many at a time: only one batch of products is held at once
/// (a whole imported collection is over a thousand decks, about 3.6 MB each with the thumbnails).
const BATCH: usize = 64;

/// The items of one kind, each from the item cache (salt 0) when it is there, the rest built in parallel. Entries
/// written for new items; notes on cache writes that failed go to `notes`.
impl ItemSpec for GripSpec {
    fn key(&self) -> &str {
        &self.key
    }
    fn display(&self) -> &str {
        &self.display
    }
    fn input(&self) -> Option<u64> {
        self.input
    }
}

impl ItemSpec for CutoutSpec {
    fn key(&self) -> &str {
        &self.key
    }
    fn display(&self) -> &str {
        &self.display
    }
    fn input(&self) -> Option<u64> {
        self.input
    }
}

fn products<S: ItemSpec, C: Sync>(
    specs: &[S],
    cache: &ItemCache,
    entry: &(dyn Fn(&str, &str, u64, u32) -> String + Sync),
    ctx: &mut Option<C>,
    load_ctx: &mut dyn FnMut() -> Result<C>,
    make: &(dyn Fn(&C, &S, u32) -> Result<Product> + Sync),
    notes: &mut Vec<String>,
) -> Result<(Vec<Option<Product>>, usize)> {
    let mut out: Vec<Option<Product>> =
        specs.iter().map(|s| s.input().filter(|_| cache.enabled()).and_then(|i| cache.load(&entry(s.key(), s.display(), i, 0)))).collect();
    let reused = out.iter().filter(|p| p.is_some()).count();
    let misses: Vec<usize> = (0..out.len()).filter(|&i| out[i].is_none()).collect();
    if !misses.is_empty() {
        if ctx.is_none() {
            *ctx = Some(load_ctx()?);
        }
        let c = ctx.as_ref().unwrap();
        let store_notes = std::sync::Mutex::new(Vec::new());
        let built = in_parallel(&misses, |i| {
            let s = &specs[i];
            let product = make(c, s, 0)?;
            if let Some(input) = s.input().filter(|_| cache.enabled()) {
                if let Err(e) = cache.store(&entry(s.key(), s.display(), input, 0), &product) {
                    store_notes.lock().unwrap().push(format!("  note: could not cache {} ({e})", s.display()));
                }
            }
            Ok(product)
        })?;
        for (i, p) in built {
            out[i] = Some(p);
        }
        notes.extend(store_notes.into_inner().unwrap());
    }
    Ok((out, reused))
}

/// Builds decks and wheels into `mods_dir/mod_name` (newdeck.build + newwheel.add_wheel + patch.Patch.write).
///
/// Incremental: an item whose spec carries an `input` hash is taken from the item cache when it was built before
/// from the same inputs (and the same engine and game), and kept there for the next build; only new or changed items
/// are encoded (in parallel). The mod is then assembled from the items in order, so it is byte-identical to a clean
/// build of the same inputs.
#[allow(clippy::too_many_arguments)]
pub fn build(
    game: Arc<Game>,
    decks: &[DeckSpec],
    wheels: &[WheelSpec],
    grips: &[GripSpec],
    cutouts: &[CutoutSpec],
    mods_dir: &Path,
    mod_name: &str,
    opts: &BuildOptions,
    log: &mut dyn FnMut(&str),
) -> Result<BuildReport> {
    let root = game.root().to_path_buf();
    let seed = opts.seed.unwrap_or(0);
    let game_id = exe_key(&game)?;
    let cache = ItemCache::open(opts.item_cache.then(|| cache_dir(opts, &game)).as_deref(), mods_dir, mod_name);
    let shared = game.bundle(SHARED, ITEMS_TOC)?;
    let mut taken = Taken::new(&game, &shared, !decks.is_empty(), !wheels.is_empty(), !grips.is_empty(), !cutouts.is_empty())?;
    let mut p = ModPatch::new(game.clone());
    let mut master = ebx::read_document(&bundle_ebx(&game, &shared, MASTER)?)?;
    let mut presets: Vec<(String, String)> = Vec::new();
    let mut built_decks = Vec::new();
    let mut built_wheels = Vec::new();
    let mut built_grips = Vec::new();
    let mut built_cutouts = Vec::new();
    let mut icon: Option<Image> = None;
    let mut notes: Vec<String> = Vec::new();
    let (mut reused, mut encoded) = (0usize, 0usize);

    // Places one item: re-derives its ids with a salt if any clash with ids already taken (vanishingly rare; the same
    // in a clean build, which places items in the same order), then replays it into the mod.
    #[allow(clippy::too_many_arguments)]
    fn place(
        product: Product,
        mut rebuild: impl FnMut(u32) -> Result<Product>,
        display: &str,
        keep_thumbnail: bool,
        taken: &mut Taken,
        p: &mut ModPatch,
        master: &mut Document,
        presets: &mut Vec<(String, String)>,
        log: &mut dyn FnMut(&str),
    ) -> Result<(BuiltItem, Option<Image>)> {
        let mut product = product;
        let mut salt = 0u32;
        while taken.clashes(&product) {
            salt += 1;
            if salt > 16 {
                return fail(format!("{display}: could not find ids that are not taken"));
            }
            log(&format!("  note: {display} got new ids (its first ones were already taken)"));
            product = rebuild(salt)?;
        }
        taken.add(&product);
        let Product { base, ops, preset, item_ref, thumbnail, icon, .. } = product;
        presets.push(preset);
        append_to_master(master, item_ref)?;
        for op in ops {
            p.apply(op)?;
        }
        Ok((BuiltItem { display: display.to_string(), base, thumbnail: keep_thumbnail.then_some(thumbnail) }, icon))
    }

    if !decks.is_empty() {
        let mut ctx: Option<DeckCtx> = None;
        let entry = |key: &str, display: &str, input: u64, salt: u32| ItemCache::entry("deck", mod_name, key, display, input, salt, seed, game_id);
        let make = |c: &DeckCtx, s: &DeckSpec, salt: u32| deck_product(c, &root, s, &mut Ids::for_item(mod_name, "deck", &s.key, salt, seed));
        for batch in decks.chunks(BATCH) {
            let (found, hits) = {
                let mut load = || deck_ctx(&game, &shared, opts, log);
                products(batch, &cache, &entry, &mut ctx, &mut load, &make, &mut notes)?
            };
            reused += hits;
            encoded += batch.len() - hits;
            for (spec, product) in batch.iter().zip(found) {
                let product = product.unwrap();
                if ctx.is_none() && taken.clashes(&product) {
                    ctx = Some(deck_ctx(&game, &shared, opts, log)?);
                }
                let rebuild = |salt: u32| -> Result<Product> {
                    if let Some(cached) = spec.input.filter(|_| cache.enabled()).and_then(|i| cache.load(&entry(&spec.key, &spec.display, i, salt))) {
                        return Ok(cached);
                    }
                    let c = match &ctx {
                        Some(c) => c,
                        None => return fail("internal: the deck templates were not loaded"),
                    };
                    let made = make(c, spec, salt)?;
                    if let Some(i) = spec.input.filter(|_| cache.enabled()) {
                        let _ = cache.store(&entry(&spec.key, &spec.display, i, salt), &made);
                    }
                    Ok(made)
                };
                let (item, deck_icon) = place(product, rebuild, &spec.display, false, &mut taken, &mut p, &mut master, &mut presets, log)?;
                if icon.is_none() {
                    icon = deck_icon;
                }
                built_decks.push(item);
                log(&format!("  built {} ({})", spec.display, spec.key));
            }
        }
    }

    if !wheels.is_empty() {
        let mut ctx: Option<WheelCtx> = None;
        let entry = |key: &str, display: &str, input: u64, salt: u32| ItemCache::entry("wheel", mod_name, key, display, input, salt, seed, game_id);
        let make = |c: &WheelCtx, s: &WheelSpec, salt: u32| wheel_product(c, &root, s, &mut Ids::for_item(mod_name, "wheel", &s.key, salt, seed));
        for batch in wheels.chunks(BATCH) {
            let (found, hits) = {
                let mut load = || wheel_ctx(&game, &shared, opts, log);
                products(batch, &cache, &entry, &mut ctx, &mut load, &make, &mut notes)?
            };
            reused += hits;
            encoded += batch.len() - hits;
            for (spec, product) in batch.iter().zip(found) {
                let product = product.unwrap();
                if ctx.is_none() && taken.clashes(&product) {
                    ctx = Some(wheel_ctx(&game, &shared, opts, log)?);
                }
                let rebuild = |salt: u32| -> Result<Product> {
                    if let Some(cached) = spec.input.filter(|_| cache.enabled()).and_then(|i| cache.load(&entry(&spec.key, &spec.display, i, salt))) {
                        return Ok(cached);
                    }
                    let c = match &ctx {
                        Some(c) => c,
                        None => return fail("internal: the wheel templates were not loaded"),
                    };
                    let made = make(c, spec, salt)?;
                    if let Some(i) = spec.input.filter(|_| cache.enabled()) {
                        let _ = cache.store(&entry(&spec.key, &spec.display, i, salt), &made);
                    }
                    Ok(made)
                };
                let (item, _) = place(product, rebuild, &spec.display, true, &mut taken, &mut p, &mut master, &mut presets, log)?;
                if icon.is_none() {
                    icon = item.thumbnail.as_ref().map(|t| t.resize_lanczos(256, 256));
                }
                built_wheels.push(item);
                log(&format!("  built {} ({})", spec.display, spec.key));
            }
        }
    }
    if !grips.is_empty() {
        let mut ctx: Option<GripCtx> = None;
        let entry = |key: &str, display: &str, input: u64, salt: u32| ItemCache::entry("grip", mod_name, key, display, input, salt, seed, game_id);
        let make = |c: &GripCtx, s: &GripSpec, salt: u32| grip_product(c, &root, s, &mut Ids::for_item(mod_name, "grip", &s.key, salt, seed));
        for batch in grips.chunks(BATCH) {
            let (found, hits) = {
                let mut load = || grip_ctx(&game, &shared);
                products(batch, &cache, &entry, &mut ctx, &mut load, &make, &mut notes)?
            };
            reused += hits;
            encoded += batch.len() - hits;
            for (spec, product) in batch.iter().zip(found) {
                let product = product.unwrap();
                if ctx.is_none() && taken.clashes(&product) {
                    ctx = Some(grip_ctx(&game, &shared)?);
                }
                let rebuild = |salt: u32| -> Result<Product> {
                    if let Some(cached) = spec.input.filter(|_| cache.enabled()).and_then(|i| cache.load(&entry(&spec.key, &spec.display, i, salt))) {
                        return Ok(cached);
                    }
                    let c = match &ctx {
                        Some(c) => c,
                        None => return fail("internal: the grip templates were not loaded"),
                    };
                    let made = make(c, spec, salt)?;
                    if let Some(i) = spec.input.filter(|_| cache.enabled()) {
                        let _ = cache.store(&entry(&spec.key, &spec.display, i, salt), &made);
                    }
                    Ok(made)
                };
                let (item, _) = place(product, rebuild, &spec.display, true, &mut taken, &mut p, &mut master, &mut presets, log)?;
                if icon.is_none() {
                    icon = item.thumbnail.as_ref().map(|t| t.resize_lanczos(256, 256));
                }
                built_grips.push(item);
                log(&format!("  built {} ({})", spec.display, spec.key));
            }
        }
    }

    if !cutouts.is_empty() {
        let mut ctx: Option<CutoutCtx> = None;
        let entry = |key: &str, display: &str, input: u64, salt: u32| ItemCache::entry("cutout", mod_name, key, display, input, salt, seed, game_id);
        let make = |c: &CutoutCtx, s: &CutoutSpec, salt: u32| cutout_product(c, &root, s, &mut Ids::for_item(mod_name, "cutout", &s.key, salt, seed));
        for batch in cutouts.chunks(BATCH) {
            let (found, hits) = {
                let mut load = || cutout_ctx(&game, &shared);
                products(batch, &cache, &entry, &mut ctx, &mut load, &make, &mut notes)?
            };
            reused += hits;
            encoded += batch.len() - hits;
            for (spec, product) in batch.iter().zip(found) {
                let product = product.unwrap();
                if ctx.is_none() && taken.clashes(&product) {
                    ctx = Some(cutout_ctx(&game, &shared)?);
                }
                let rebuild = |salt: u32| -> Result<Product> {
                    if let Some(cached) = spec.input.filter(|_| cache.enabled()).and_then(|i| cache.load(&entry(&spec.key, &spec.display, i, salt))) {
                        return Ok(cached);
                    }
                    let c = match &ctx {
                        Some(c) => c,
                        None => return fail("internal: the cutout templates were not loaded"),
                    };
                    let made = make(c, spec, salt)?;
                    if let Some(i) = spec.input.filter(|_| cache.enabled()) {
                        let _ = cache.store(&entry(&spec.key, &spec.display, i, salt), &made);
                    }
                    Ok(made)
                };
                let (item, _) = place(product, rebuild, &spec.display, true, &mut taken, &mut p, &mut master, &mut presets, log)?;
                if icon.is_none() {
                    icon = item.thumbnail.as_ref().map(|t| t.resize_lanczos(256, 256));
                }
                built_cutouts.push(item);
                log(&format!("  built {} ({})", spec.display, spec.key));
            }
        }
    }
    for n in &notes {
        log(n);
    }
    if cache.enabled() && reused > 0 {
        log(&format!("  {reused} of {} item(s) reused from the item cache", decks.len() + wheels.len() + grips.len() + cutouts.len()));
    }

    for (bundle, name, data) in &opts.extra_ebx {
        let enc = cas::encode(data, &root)?;
        p.apply(Op::Ebx { bundle: bundle.clone(), name: name.clone(), size: data.len() as u32, enc })?;
    }
    for op in &opts.extra_ops {
        p.apply(op.clone())?;
    }

    // once per mod: the master item list and the bundle reference table
    p.replace_ebx(SHARED, MASTER, &write(&master)?)?;
    let (data, res) = bundle_res(&game, &shared, BRT)?;
    let table = brt::Table::parse(&data, &res.meta)?;
    let (resource, meta) = brt::add_presets(&table, &presets)?;
    p.replace_res(SHARED, BRT, &resource, meta)?;
    if !opts.config_res.is_empty() {
        p.patch_config(&opts.config_res)?;
    }

    let names: Vec<&str> = built_decks.iter().chain(&built_wheels).chain(&built_grips).chain(&built_cutouts).map(|b| b.display.as_str()).collect();
    let kinds: Vec<&str> = [(&built_decks, "decks"), (&built_wheels, "wheels"), (&built_grips, "grips"), (&built_cutouts, "grip patterns")]
        .iter()
        .filter(|(b, _)| !b.is_empty())
        .map(|(_, k)| *k)
        .collect();
    let what = match kinds.len() {
        0 => "Nothing".to_string(),
        1 => kinds[0].to_string(),
        n => format!("{} and {}", kinds[..n - 1].join(", "), kinds[n - 1]),
    };
    let what = format!("{}{}", what[..1].to_uppercase(), &what[1..]);
    let description = format!("{what} built by Caveman from PNGs: {}.", names.join(", "));
    let mut readme = format!("# {mod_name}\n\n{what} built by Caveman from PNGs:\n\n");
    for b in built_decks.iter().chain(&built_wheels).chain(&built_grips).chain(&built_cutouts) {
        readme.push_str(&format!("- {}\n", b.display));
    }
    readme.push_str(&format!("\n---\n\nGenerated with Caveman {}.\n", opts.tool_version));
    let mod_dir = mods_dir.join(mod_name);
    let bundles = p.bundles.len();
    let chunks = p.chunks.len();
    let icon = icon.map(|i| i.to_rgba());
    let (cas_bytes, toc_bytes) =
        p.write(&mod_dir, mod_name, &description, &readme, icon.as_ref(), &opts.tool_version, opts.author.as_deref().unwrap_or("Caveman"))?;
    if cache.enabled() {
        let removed = cache.prune();
        if removed > 0 {
            log(&format!("  removed {removed} item(s) no longer built from the item cache"));
        }
    }
    Ok(BuildReport {
        mod_dir,
        decks: built_decks,
        wheels: built_wheels,
        grips: built_grips,
        cutouts: built_cutouts,
        cas_bytes,
        toc_bytes,
        bundles,
        chunks,
        reused,
        encoded,
    })
}

/// One more entry in the master item list: an import of the item and a pointer to it.
fn append_to_master(master: &mut Document, item: (Guid, Guid)) -> Result<()> {
    let ptr = master.add_import(item);
    master
        .object_mut(0)?
        .find_mut("Items")
        .and_then(|f| f.value.data.as_array_mut())
        .ok_or_else(|| BuildError("the master list has no Items".into()))?
        .push(Value::new(ptr));
    Ok(())
}

/// A wheel's art and mask, 512 x 512 RGB (newwheel.wheel_textures): the face graphic (centre-cropped to a square)
/// squashed into the top half over the wheel colour; mask R = the face's alpha in the top half, 0 in the bottom; G
/// and B the host mask's.
pub fn wheel_textures(face: &Image, colour: [u8; 3], host_mask: &Image) -> (Image, Image) {
    let face = face.to_rgba();
    let side = face.width.min(face.height);
    let (left, top) = ((face.width - side) / 2, (face.height - side) / 2);
    let squashed = face.crop(left, top, left + side, top + side).resize_lanczos(512, 256);
    let mut art = Image::new(512, 512, 3);
    let mut mask = host_mask.to_rgb();
    for y in 0..512 {
        for x in 0..512 {
            let i = y * 512 + x;
            if y < 256 {
                let px = squashed.pixel(x, y);
                let a = px[3] as f64 / 255.0;
                for c in 0..3 {
                    art.data[i * 3 + c] = (px[c] as f64 * a + colour[c] as f64 * (1.0 - a)).round_ties_even().clamp(0.0, 255.0) as u8;
                }
                mask.data[i * 3] = px[3];
            } else {
                art.data[i * 3..i * 3 + 3].copy_from_slice(&colour);
                mask.data[i * 3] = 0;
            }
        }
    }
    (art, mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_colour_behind_transparent_art() {
        let wood = Image::filled(ART_SIZE.0, ART_SIZE.1, &[180, 140, 90]);
        // half transparent, half opaque blue: the transparent half shows the colour, else the wood
        let mut img = Image::filled(ART_SIZE.0, ART_SIZE.1, &[0, 0, 0, 0]);
        let half = img.data.len() / 2;
        for px in img.data[..half].chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 0, 255, 255]);
        }
        let (red, notes) = fit_art_on(&img, &wood, Some([255, 0, 0])).unwrap();
        assert_eq!(notes, ["transparent parts show #ff0000"]);
        assert_eq!(&red.data[..3], &[0, 0, 255]);
        assert_eq!(&red.data[red.data.len() - 3..], &[255, 0, 0]);
        let (plain, notes) = fit_art_on(&img, &wood, None).unwrap();
        assert_eq!(notes, ["transparent parts show natural wood"]);
        assert_eq!(&plain.data[plain.data.len() - 3..], &[180, 140, 90]);
        assert_eq!(plain.data, fit_art(&img, &wood).unwrap().0.data);
        // a fully transparent image is a plain deck in the colour, like the game's own plain decks
        let (solid, _) = fit_art_on(&Image::filled(64, 256, &[0, 0, 0, 0]), &wood, Some([12, 34, 56])).unwrap();
        assert!(solid.data.chunks_exact(3).all(|p| p == [12, 34, 56]));
    }

    #[test]
    fn sha256_known() {
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn djb32_and_base() {
        assert_eq!(base_name("Own_000_-_Dan_Da_Dan"), "000_-_dan_da_dan");
        assert_eq!(djb32(""), 5381);
    }

    #[test]
    fn json_strings() {
        assert_eq!(json_str("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json_str("é"), "\"\\u00e9\"");
    }
}
