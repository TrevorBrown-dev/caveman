//! The skateboard as the game draws it, for the board cam window: the stock deck, truck and wheel meshes (full
//! vertices: positions, normals, UVs, the bone of each), the board rig's bind pose from the skeleton asset, and the
//! parts' textures from the player's install, written as a glTF binary (`.glb`) that three.js loads straight away.
//! Nothing of it is shipped: it is built on the player's PC from their own game.
//!
//! The rig at run time is 16 bones in ReSkate's order (`LANES`; the parents `LANE_PARENTS` were read out of the
//! running game's skeleton resource on 2026-10-30), not the skeleton asset's order: the asset's bones are mapped by
//! name. Each mesh part is rigid on one bone (the deck and the truck baseplates on `Deck`, each hanger on its truck,
//! each wheel on its wheel bone), so a part's vertices go into the glTF in its bone's space (the asset's inverse
//! model pose applied), as a mesh under that bone's node; the window then sets each node's transform from the live
//! pose (`caveman-trainer pose --bones`: scale, rotation and position in the parent's frame) and the parts follow.
//! The normals are the ones the game's vertex program decodes (`mesh::tangent_space_normal`).
//!
//! The deck is two layers, as the game's deck shader has them (knowledge/cosmetics.md, shaders.md): the **wood**
//! (`param_base_c/_ny/_msk`, 1024 x 2048: the top face in the left half, the bottom face in the right, the ply edge
//! in a strip at the top) on the first UV set, and the **graphic** (`param_graphic_c/_osk/_ny`, 512 x 2048) on the
//! second, which is laid out in UDIM tiles: tile 1 (u in 1..2) is the top under the grip, tile 2 (u in 2..3) the
//! bottom, where the graphic goes (`param_udim_graphica`); the rim sits at (0, 1) and shows the wood. So the body
//! mesh is split by tile: the bottom becomes a part of its own with the graphic material (the window puts the deck
//! art there), the rest keeps the wood. The grip (`DeckTop_mat`) samples its own textures with the second set, tile
//! 1. The second set is rebased to 0..1 per part.
//!
//! Masks: the `_msk` textures (wood, trucks, wheels) hold **R metal, G smoothness, B occlusion** (sampled on the
//! meshes: the trucks' bushings and the wheels' urethane have R 0 and their metal parts 255; the wood's top face,
//! under the grip, has B 0 but for the bolt holes); the graphic's `_osk` holds **R occlusion, G smoothness, B metal**
//! (the live calibration of 2026-10-22), and the board under the skater samples a copy with B lifted a tenth of the
//! way to full (`effects::ridden_mask`), which this copies. The grip's `_osk` has R and B at 255 everywhere (R is
//! where the grip is, as the wheel graphics' masks; B is not metal: grip is grit) and G its smoothness. All go into
//! the glTF as one occlusion-roughness-metal image (roughness = 255 - smoothness).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde_json::{json, Value};

use crate::bcdec::{self, BcFormat};
use crate::build::{self, SHARED};
use crate::ebx::{self, Data, Document};
use crate::effects;
use crate::game::{self, Bundle, Game, ITEMS_TOC};
use crate::image::Image;
use crate::mesh::{self, FullSection, FullVertex, DECK_BUNDLE_SUFFIX, DECK_MESH, LEVEL_BUNDLE, LEVEL_TOC, TRUCKS_MESH, WHEELS_MESH};
use crate::texture::TextureHeader;
use crate::toc::Guid;

/// sRGB 0..1 to linear 0..1.
fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear 0..1 to sRGB 0..1.
fn linear_to_srgb(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// The board rig's skeleton asset (the level root bundle).
pub const SKELETON_ASSET: &str = "characters/_prototype/skateboard/skateboard_default_skeleton";
/// The texture resource type.
pub const TEXTURE_TYPE: u32 = 0x6bde_20ba;
/// The deck whose graphic is the plain wood (Caveman's Natural Blank), in items.toc.
pub const BLANK_DECK: &str = "deck_gen_graphic_00001";
/// The graphic layer's stock mask and normal map (shared by the decks without their own).
pub const GRAPHIC_DEFAULT_OSK: &str = "characters/skateboard/defaults/deck/common/generic/graphic/deck_gen_graphic_default_osk";
pub const GRAPHIC_DEFAULT_NY: &str = "characters/skateboard/defaults/deck/common/generic/graphic/deck_gen_graphic_default_ny";
/// The UDIM tile of the second UV set that carries the deck graphic (the bottom), and the grip's.
pub const GRAPHIC_TILE: u32 = 2;
pub const GRIP_TILE: u32 = 1;
/// The wheel colour: a preset's `param_region_color_0` (three linear floats and a u32 1; knowledge/wheels.md).
pub const WHEEL_COLOUR_KEY: &str = "4b18a9e2-cd35-8c46-59ff-5e880100548c";
/// The grip tint: the grip presets' one 16-byte colour (`param_graphic_tintcolor` by the bundle's parameter assets;
/// black grip is (0.048, 0.044, 0.054) linear).
pub const GRIP_TINT_KEY: &str = "36e50d59-48af-873f-59ff-5e88010042d4";
/// The asset types an item record names (`DelMarCustomizationItemData.AssetPaths[].AssetTypeId`).
pub const ASSET_PRESET: i64 = 3;
pub const ASSET_TEXTURE: i64 = 6;

/// What the player rides, by the items' keys as ReSkate's profile holds them (`Own_...`); empty means the stock
/// part. Read from the profile by the app (`cosmetic_loadouts`, the board recipe); `stock()` is the plain board.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Loadout {
    pub deck: String,
    pub grip: String,
    pub cutout: String,
    pub trucks: String,
    pub wheels: String,
}

impl Loadout {
    /// The plain board: the Natural Blank graphic, black grip, no cutout, the first default trucks, natural wheels.
    pub fn stock() -> Loadout {
        Loadout {
            deck: String::new(),
            grip: "Own_DeckGripColor_Gen_Popsicle_00001".into(),
            cutout: "Own_DeckGripCutout_Gen_Popsicle_00001".into(),
            trucks: "Own_Truck_Gen_Default_00001".into(),
            wheels: "Own_WheelColor_Gen_Classic_00001".into(),
        }
    }

    /// A short name for a cache file: a hash of the five keys.
    pub fn key(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for part in [&self.deck, &self.grip, &self.cutout, &self.trucks, &self.wheels] {
            for b in part.to_lowercase().bytes().chain(std::iter::once(b'|')) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        format!("{h:016x}")
    }
}

/// The rig's bones at run time, in the pose buffer's order (ReSkate's `reskate.board` numbering), named as the
/// skeleton asset names them. Lanes 12 to 15 are the trajectory helpers (identity while riding; the breakable deck
/// pack's stretches in its own model).
pub const LANES: [&str; 16] = [
    "Reference",
    "AITrajectory",
    "Skateboard_Root",
    "Deck",
    "Truck_Back",
    "eff_truckBack",
    "Right_WheelBack",
    "Left_WheelBack",
    "Truck_Front",
    "eff_truckFront",
    "Right_WheelFront",
    "Left_WheelFront",
    "Trajectory",
    "Connect",
    "Connect2",
    "Ground",
];
/// Each lane's parent lane (read from the game's skeleton resource).
pub const LANE_PARENTS: [i32; 16] = [-1, 0, 1, 2, 3, 4, 4, 4, 3, 8, 8, 8, 1, 1, 1, 1];
/// The lane of the deck bone: the board's own frame (its model space: x across, y up, z towards the nose).
pub const DECK_LANE: usize = 3;

/// The material slots of the model, in order.
pub const ROLE_WOOD: usize = 0;
pub const ROLE_GRIP: usize = 1;
pub const ROLE_TRUCKS: usize = 2;
pub const ROLE_WHEELS: usize = 3;
pub const ROLE_GRAPHIC: usize = 4;

#[derive(Debug)]
pub struct BoardModelError(pub String);

impl fmt::Display for BoardModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "board model: {}", self.0)
    }
}

impl std::error::Error for BoardModelError {}

type Result<T> = std::result::Result<T, BoardModelError>;

fn fail<T>(text: impl Into<String>) -> Result<T> {
    Err(BoardModelError(text.into()))
}

impl From<game::GameError> for BoardModelError {
    fn from(e: game::GameError) -> Self {
        BoardModelError(e.to_string())
    }
}
impl From<mesh::MeshError> for BoardModelError {
    fn from(e: mesh::MeshError) -> Self {
        BoardModelError(e.to_string())
    }
}
impl From<ebx::Error> for BoardModelError {
    fn from(e: ebx::Error) -> Self {
        BoardModelError(e.to_string())
    }
}
impl From<std::io::Error> for BoardModelError {
    fn from(e: std::io::Error) -> Self {
        BoardModelError(e.to_string())
    }
}

/// A Frostbite `LinearTransform`: the basis vectors (the images of x, y and z) and the translation. A point `p` in
/// the transform's own space maps out as `p.x * right + p.y * up + p.z * forward + trans`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub trans: [f32; 3],
}

impl Transform {
    pub const IDENTITY: Transform = Transform { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], forward: [0.0, 0.0, 1.0], trans: [0.0; 3] };

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.trans;
        for k in 0..3 {
            out[k] += p[0] * self.right[k] + p[1] * self.up[k] + p[2] * self.forward[k];
        }
        out
    }

    pub fn direction(&self, d: [f32; 3]) -> [f32; 3] {
        let mut out = [0f32; 3];
        for k in 0..3 {
            out[k] = d[0] * self.right[k] + d[1] * self.up[k] + d[2] * self.forward[k];
        }
        out
    }

    /// The rotation as a quaternion (x, y, z, w), from the basis vectors as the columns of the rotation matrix.
    pub fn quaternion(&self) -> [f32; 4] {
        let (r, u, f) = (self.right, self.up, self.forward);
        let m = [[r[0], u[0], f[0]], [r[1], u[1], f[1]], [r[2], u[2], f[2]]];
        let trace = m[0][0] + m[1][1] + m[2][2];
        let (x, y, z, w);
        if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            w = 0.25 * s;
            x = (m[2][1] - m[1][2]) / s;
            y = (m[0][2] - m[2][0]) / s;
            z = (m[1][0] - m[0][1]) / s;
        } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
            let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
            w = (m[2][1] - m[1][2]) / s;
            x = 0.25 * s;
            y = (m[0][1] + m[1][0]) / s;
            z = (m[0][2] + m[2][0]) / s;
        } else if m[1][1] > m[2][2] {
            let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
            w = (m[0][2] - m[2][0]) / s;
            x = (m[0][1] + m[1][0]) / s;
            y = 0.25 * s;
            z = (m[1][2] + m[2][1]) / s;
        } else {
            let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
            w = (m[1][0] - m[0][1]) / s;
            x = (m[0][2] + m[2][0]) / s;
            y = (m[1][2] + m[2][1]) / s;
            z = 0.25 * s;
        }
        let l = (x * x + y * y + z * z + w * w).sqrt().max(1e-9);
        [x / l, y / l, z / l, w / l]
    }
}

/// The skeleton asset: bone names, each bone's parent, and the bind pose three ways.
#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub names: Vec<String>,
    pub parents: Vec<i32>,
    pub local: Vec<Transform>,
    pub model: Vec<Transform>,
    pub inverse_model: Vec<Transform>,
}

impl Skeleton {
    pub fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n.eq_ignore_ascii_case(name))
    }
}

fn vec3_of(d: &Data) -> Result<[f32; 3]> {
    let o = d.as_object().ok_or_else(|| BoardModelError("a Vec3 is not an object".into()))?;
    let f = |n: &str| -> Result<f32> {
        match o.get(n)? {
            Data::F32(v) => Ok(*v),
            other => fail(format!("Vec3 {n}: {other:?}")),
        }
    };
    Ok([f("x")?, f("y")?, f("z")?])
}

fn transform_of(d: &Data) -> Result<Transform> {
    let o = d.as_object().ok_or_else(|| BoardModelError("a LinearTransform is not an object".into()))?;
    Ok(Transform { right: vec3_of(o.get("right")?)?, up: vec3_of(o.get("up")?)?, forward: vec3_of(o.get("forward")?)?, trans: vec3_of(o.get("trans")?)? })
}

/// The board rig's skeleton asset from the player's install.
pub fn read_skeleton(game: &Game) -> Result<Skeleton> {
    let b = game.bundle(LEVEL_BUNDLE, LEVEL_TOC)?;
    let i = b.manifest.ebx_index(SKELETON_ASSET).ok_or_else(|| BoardModelError(format!("no {SKELETON_ASSET} in the level root bundle")))?;
    let loc = b.ebx_loc(i).ok_or_else(|| BoardModelError("the skeleton asset has no file".into()))?;
    let doc = ebx::read_document(&game.decode(&loc)?)?;
    let strings = |field: &str| -> Result<Vec<String>> {
        Ok(doc.get(field, 0)?.as_array().ok_or_else(|| BoardModelError(format!("{field} is not an array")))?.iter().filter_map(|v| v.data.as_str()).collect())
    };
    let transforms = |field: &str| -> Result<Vec<Transform>> {
        doc.get(field, 0)?.as_array().ok_or_else(|| BoardModelError(format!("{field} is not an array")))?.iter().map(|v| transform_of(&v.data)).collect()
    };
    let names = strings("BoneNames")?;
    let parents: Vec<i32> = doc
        .get("Hierarchy", 0)?
        .as_array()
        .ok_or_else(|| BoardModelError("Hierarchy is not an array".into()))?
        .iter()
        .filter_map(|v| v.data.as_i64())
        .map(|v| v as i32)
        .collect();
    let (local, model, inverse_model) = (transforms("LocalPose")?, transforms("ModelPose")?, transforms("InverseModelPose")?);
    let n = names.len();
    if n == 0 || parents.len() != n || local.len() != n || model.len() != n || inverse_model.len() != n {
        return fail(format!(
            "the skeleton asset's arrays disagree ({n} names, {} parents, {} local, {} model, {} inverse)",
            parents.len(),
            local.len(),
            model.len(),
            inverse_model.len()
        ));
    }
    Ok(Skeleton { names, parents, local, model, inverse_model })
}

/// One rigid part of the board: the triangles of one mesh section (or of one UDIM tile of it) on one bone, in the
/// bone's space.
#[derive(Clone, Debug, Default)]
pub struct Part {
    pub name: String,
    /// The lane (run-time bone) the part hangs on.
    pub lane: usize,
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    /// The second UV set, rebased to its tile's 0..1.
    pub uv1: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// A part's look: the textures as the game has them (decoded first mips, RGBA).
#[derive(Clone, Debug, Default)]
pub struct Material {
    pub name: String,
    /// "wood", "grip", "trucks", "wheels" or "graphic".
    pub role: &'static str,
    pub base: Option<Image>,
    /// A tangent-space normal map.
    pub normal: Option<Image>,
    /// R occlusion, G roughness, B metal, made from the game's mask (see the module notes).
    pub orm: Option<Image>,
    /// Whether R of `orm` is an occlusion worth applying.
    pub occlusion: bool,
    /// The UV set the textures are sampled with (1 for the grip and the graphic).
    pub tex_coord: u32,
    /// What the textures were read from, for the record.
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct BoardModel {
    pub skeleton: Skeleton,
    pub parts: Vec<Part>,
    pub materials: Vec<Material>,
    /// Triangles of the deck body whose corners sat in different UDIM tiles (none expected).
    pub mixed_triangles: usize,
    /// What was resolved how, and what fell back to the stock part.
    pub notes: Vec<String>,
}

/// A texture resource's first mip as an RGBA image, from a bundle (the chunk in the TOC, else in the bundle).
pub fn texture_image(game: &Game, toc: &str, bundle: &Bundle, name: &str) -> Result<Image> {
    let ri = bundle
        .manifest
        .res
        .iter()
        .position(|r| r.rtype == TEXTURE_TYPE && r.name == name)
        .ok_or_else(|| BoardModelError(format!("no texture {name} in {}", bundle.name)))?;
    let payload = game.decode(&bundle.res_loc(ri).ok_or_else(|| BoardModelError(format!("{name}: no file")))?)?;
    let h = TextureHeader::parse(&payload, &bundle.manifest.res[ri].meta).map_err(|e| BoardModelError(e.to_string()))?;
    let format =
        BcFormat::from_frostbite(h.format).ok_or_else(|| BoardModelError(format!("{name}: texture format {} is not one the decoder knows", h.format)))?;
    let guid = Guid::from_le_bytes(h.chunk);
    let data = match game.chunk(&guid, toc)? {
        Some(c) if !c.removed => game.decode(&game::chunk_loc(&c))?,
        _ => {
            let ci = bundle
                .manifest
                .chunks
                .iter()
                .position(|c| c.guid == guid && c.logical_offset == 0)
                .ok_or_else(|| BoardModelError(format!("{name}: its chunk {guid} is in neither {toc} nor the bundle")))?;
            game.decode(&bundle.chunk_loc(ci).ok_or_else(|| BoardModelError(format!("{name}: its chunk has no file")))?)?
        }
    };
    let (w, ht) = (h.width as usize, h.height as usize);
    let mip0 = w.div_ceil(4).max(1) * ht.div_ceil(4).max(1) * format.block_bytes();
    if data.len() < mip0 {
        return fail(format!("{name}: its chunk holds {} of the {mip0} bytes of its first mip", data.len()));
    }
    Ok(bcdec::decode(format, &data[..mip0], w, ht).to_rgba())
}

/// Which channels a game mask holds, by its kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaskKind {
    /// `_msk`: R metal, G smoothness, B where the region colour applies (not an occlusion: the wheels' bearings and
    /// the wood's top under the grip have 0).
    Msk,
    /// The graphic's `_osk`: R occlusion, G smoothness, B metal; the ridden copy's lift of B applied.
    GraphicOsk,
    /// The grip's `_osk`: G smoothness; R where the grip is, B not metal.
    GripOsk,
}

/// A game mask as glTF wants it: R occlusion, G roughness, B metal.
pub fn orm_of(mask: &Image, kind: MaskKind) -> Image {
    let src = match kind {
        MaskKind::GraphicOsk => effects::ridden_mask(&mask.to_rgba()),
        _ => mask.to_rgba(),
    };
    let mut out = Image::new(src.width, src.height, 4);
    for (o, p) in out.data.chunks_exact_mut(4).zip(src.data.chunks_exact(4)) {
        let (occlusion, smoothness, metal) = match kind {
            MaskKind::Msk => (255, p[1], p[0]),
            MaskKind::GraphicOsk => (p[0], p[1], p[2]),
            MaskKind::GripOsk => (255, p[1], 0),
        };
        o[0] = occlusion;
        o[1] = 255 - smoothness;
        o[2] = metal;
        o[3] = 255;
    }
    out
}

/// The first items.toc bundle whose name starts with `prefix` (sorted), with its stem (the item's name).
fn first_item_bundle(game: &Game, prefix: &str) -> Result<(String, String)> {
    let toc = game.toc(ITEMS_TOC)?;
    let mut names: Vec<&str> =
        toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.starts_with(prefix) && n.ends_with("_ap_cas_main_bundlereftable")).collect();
    names.sort();
    let name = names.first().ok_or_else(|| BoardModelError(format!("no bundle under {prefix}")))?.to_string();
    let leaf = name.rsplit('/').next().unwrap_or(&name);
    let stem = leaf.trim_end_matches("_ap_cas_main_bundlereftable").to_string();
    Ok((name, stem))
}

/// The items.toc bundle of an item, by its name's end.
fn item_bundle(game: &Game, item: &str) -> Result<String> {
    let suffix = format!("/{item}_ap_cas_main_bundlereftable");
    game.find_bundle(&suffix, ITEMS_TOC)?.ok_or_else(|| BoardModelError(format!("no bundle ending {suffix} in items.toc")))
}

/// A texture of a bundle whose name contains `contains` and ends with `suffix` (the first found).
fn any_texture(game: &Game, toc: &str, bundle: &Bundle, contains: &str, suffix: &str) -> Option<(Image, String)> {
    let name = bundle.manifest.res.iter().find(|r| r.rtype == TEXTURE_TYPE && r.name.contains(contains) && r.name.ends_with(suffix)).map(|r| r.name.clone())?;
    texture_image(game, toc, bundle, &name).ok().map(|img| (img, name))
}

/// A material from a bundle's textures named `<stem>_c`, `<stem>_ny` (or `_n`) and `<stem>_msk` or `_osk`; missing
/// ones are left out.
fn material_from(game: &Game, toc: &str, bundle: &Bundle, stem: &str, name: &str, role: &'static str, kind: MaskKind, tex_coord: u32) -> Material {
    let mut m = Material { name: name.into(), role, occlusion: true, tex_coord, ..Material::default() };
    if let Some((img, src)) = any_texture(game, toc, bundle, stem, "_c") {
        m.base = Some(img);
        m.sources.push(src);
    }
    if let Some((img, src)) = any_texture(game, toc, bundle, stem, "_ny").or_else(|| any_texture(game, toc, bundle, stem, "_n")) {
        m.normal = Some(img);
        m.sources.push(src);
    }
    if let Some((img, src)) = any_texture(game, toc, bundle, stem, "_msk").or_else(|| any_texture(game, toc, bundle, stem, "_osk")) {
        m.orm = Some(orm_of(&img, kind));
        m.sources.push(src);
    }
    m
}

/// The UDIM tile of a vertex's second UV set.
fn tile_of(v: &FullVertex) -> u32 {
    v.uv1[0].floor().max(0.0) as u32
}

/// A section's triangles grouped by the UDIM tile of their corners (a triangle with corners in different tiles,
/// which should not happen, goes with its first corner and is counted).
fn split_by_tile(s: &FullSection) -> (Vec<(u32, Vec<[u32; 3]>)>, usize) {
    let mut by_tile: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    let mut mixed = 0;
    for t in &s.triangles {
        let tiles = [tile_of(&s.vertices[t[0] as usize]), tile_of(&s.vertices[t[1] as usize]), tile_of(&s.vertices[t[2] as usize])];
        if tiles[0] != tiles[1] || tiles[1] != tiles[2] {
            mixed += 1;
        }
        by_tile.entry(tiles[0]).or_default().push(*t);
    }
    (by_tile.into_iter().collect(), mixed)
}

/// Parts from a section's triangles: one per bone its corners sit on (the first corner's bone decides; the board's
/// parts are rigid, so the three agree), each in its bone's space, with only the vertices its triangles use and the
/// second UV set rebased by `tile`.
fn parts_of(
    s: &FullSection,
    triangles: &[[u32; 3]],
    skeleton: &Skeleton,
    lane_of_bone: &dyn Fn(u16) -> Option<usize>,
    material: usize,
    name: &str,
    tile: u32,
    what: &str,
) -> Result<Vec<Part>> {
    let mut by_bone: BTreeMap<u16, Vec<[u32; 3]>> = BTreeMap::new();
    for t in triangles {
        by_bone.entry(s.vertices[t[0] as usize].bone).or_default().push(*t);
    }
    let several = by_bone.len() > 1;
    let mut parts = Vec::new();
    for (bone, tris) in by_bone {
        let lane = lane_of_bone(bone).ok_or_else(|| BoardModelError(format!("{what}: bone {bone} of {} has no lane", s.material)))?;
        let inv = skeleton.inverse_model.get(bone as usize).copied().ok_or_else(|| BoardModelError(format!("{what}: bone {bone} is not in the skeleton")))?;
        let mut map = vec![u32::MAX; s.vertices.len()];
        let mut part =
            Part { name: if several { format!("{name}:{}", skeleton.names[bone as usize]) } else { name.to_string() }, lane, material, ..Part::default() };
        for t in &tris {
            let mut tri = [0u32; 3];
            for (k, &i) in t.iter().enumerate() {
                if map[i as usize] == u32::MAX {
                    let v = &s.vertices[i as usize];
                    map[i as usize] = part.positions.len() as u32;
                    part.positions.push(inv.point(v.position));
                    let n = inv.direction(v.normal);
                    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                    part.normals.push([n[0] / l, n[1] / l, n[2] / l]);
                    part.uv0.push(v.uv0);
                    part.uv1.push([v.uv1[0] - tile as f32, v.uv1[1]]);
                }
                tri[k] = map[i as usize];
            }
            part.indices.extend_from_slice(&tri);
        }
        parts.push(part);
    }
    Ok(parts)
}

// ---------------------------------------------------------------- the equipped parts

/// An item record's asset paths, (AssetTypeId, AssetName), from the shared bundle (`items/<category>/<key, lower
/// case>`, its `DelMarCustomizationItemData`).
fn item_assets(game: &Game, shared: &Bundle, key: &str) -> Result<Vec<(i64, String)>> {
    // the records' names are the keys in lower case, but a few drop an underscore (`own_wheelgraphic_gen_classic00006`
    // for `Own_WheelGraphic_Gen_Classic_00006`): compare with the underscores out
    let squeeze = |s: &str| -> String { s.chars().filter(|c| *c != '_').collect::<String>().to_lowercase() };
    let want = squeeze(key);
    let name = shared
        .manifest
        .ebx
        .iter()
        .find(|e| e.name.starts_with("items/") && e.name.rsplit('/').next().is_some_and(|leaf| squeeze(leaf) == want))
        .map(|e| e.name.clone())
        .ok_or_else(|| BoardModelError(format!("no item record for {key}")))?;
    let i = shared.manifest.ebx_index(&name).ok_or_else(|| BoardModelError(format!("{name}: no index")))?;
    let doc = ebx::read_document(&game.decode(&shared.ebx_loc(i).ok_or_else(|| BoardModelError(format!("{name}: no file")))?)?)?;
    let mut out = Vec::new();
    for inst in 0..4 {
        let Ok(arr) = doc.get("AssetPaths", inst) else { continue };
        let Some(values) = arr.as_array() else { continue };
        for v in values {
            if let Some(o) = v.data.as_object() {
                let t = o.get("AssetTypeId").ok().and_then(|d| d.as_i64()).unwrap_or(-1);
                let n = o.get("AssetName").ok().and_then(|d| d.as_str()).unwrap_or_default();
                out.push((t, n));
            }
        }
        break;
    }
    if out.is_empty() {
        return fail(format!("{name} names no assets"));
    }
    Ok(out)
}

/// The bundle an asset (a preset `..._AP`, a texture `..._OSK`) lives in: `<asset, lower case>_cas_main_bundlereftable`.
fn asset_bundle(game: &Game, asset: &str) -> Result<Bundle> {
    let suffix = format!("/{}_cas_main_bundlereftable", asset.to_lowercase());
    let name = game.find_bundle(&suffix, ITEMS_TOC)?.ok_or_else(|| BoardModelError(format!("no bundle ending {suffix}")))?;
    Ok(game.bundle(&name, ITEMS_TOC)?)
}

/// A bundle's EBX document named `asset` (by its leaf, lower case).
fn bundle_doc(game: &Game, bundle: &Bundle, asset: &str) -> Result<Document> {
    let leaf = format!("/{}", asset.to_lowercase());
    let name = bundle
        .manifest
        .ebx
        .iter()
        .find(|e| e.name.to_lowercase().ends_with(&leaf))
        .map(|e| e.name.clone())
        .ok_or_else(|| BoardModelError(format!("{} has no {asset}", bundle.name)))?;
    Ok(ebx::read_document(&build::bundle_ebx(game, bundle, &name).map_err(|e| BoardModelError(e.to_string()))?)?)
}

/// A preset's boxed colour parameter (three linear floats), by its ParamKey.
fn preset_colour(doc: &Document, key: &str) -> Option<[f32; 3]> {
    let i = *build::param_records(doc).get(key)?;
    let raw = &doc.boxed_values.get(i)?.raw_bytes;
    let f = |k: usize| raw.get(k * 4..k * 4 + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap()));
    Some([f(0)?, f(1)?, f(2)?])
}

/// The item's preset: its bundle, its document and its name (lower case, without `_ap`: the stem its own textures
/// are named by).
fn item_preset(game: &Game, shared: &Bundle, key: &str) -> Result<(Bundle, Document, String)> {
    let preset = item_assets(game, shared, key)?
        .into_iter()
        .find(|(t, _)| *t == ASSET_PRESET)
        .map(|(_, n)| n)
        .ok_or_else(|| BoardModelError(format!("{key} names no appearance preset")))?;
    let bundle = asset_bundle(game, &preset)?;
    let doc = bundle_doc(game, &bundle, &preset)?;
    let stem = preset.to_lowercase().trim_end_matches("_ap").to_string();
    Ok((bundle, doc, stem))
}

/// A bundle's texture named for the stem with the suffix, else any with the suffix whose name has `word`.
fn pick_texture(game: &Game, bundle: &Bundle, stem: &str, suffix: &str, word: &str) -> Option<(Image, String)> {
    any_texture(game, ITEMS_TOC, bundle, &format!("/{stem}"), suffix).or_else(|| any_texture(game, ITEMS_TOC, bundle, word, suffix))
}

/// A colour texture as the game's region colouring shows it: the colour where the texture is at its typical value
/// and the texture's variation around it. (The menu thumbnails of the wheel colours and of black grip show the
/// preset's colour itself at about the renders' light level, not the colour times the grey texture.) `colour` is
/// linear rgb; the typical value is the median linear luminance of the texels that are not nearly black (the
/// bearing seal, cut-outs).
pub fn coloured(tex: &Image, colour: [f32; 3]) -> Image {
    let src = tex.to_rgba();
    let lum = |p: &[u8]| -> f64 {
        0.2126 * srgb_to_linear(p[0] as f64 / 255.0) + 0.7152 * srgb_to_linear(p[1] as f64 / 255.0) + 0.0722 * srgb_to_linear(p[2] as f64 / 255.0)
    };
    let mut lums: Vec<f64> = src.data.chunks_exact(4).map(lum).filter(|l| *l > 0.02).collect();
    if lums.is_empty() {
        return src;
    }
    lums.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = lums[lums.len() / 2].max(1e-3);
    let mut out = src.clone();
    for px in out.data.chunks_exact_mut(4) {
        for c in 0..3 {
            let lin = srgb_to_linear(px[c] as f64 / 255.0) / median * colour[c] as f64;
            px[c] = (linear_to_srgb(lin.clamp(0.0, 1.0)) * 255.0).round() as u8;
        }
    }
    out
}

/// A colour texture with a region colour laid over it where the mask's B says (the regions shader component: the
/// wheels' urethane takes the preset's colour flat, their bearings keep the texture's black seal and metal).
/// `colour` is linear rgb; `mask` is the part's `_msk`.
pub fn region_coloured(tex: &Image, mask: &Image, colour: [f32; 3]) -> Image {
    let mut out = tex.to_rgba();
    let mask = mask.to_rgba();
    let (w, h) = (out.width, out.height);
    for y in 0..h {
        for x in 0..w {
            let b = sample(&mask, [(x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64])[2] / 255.0;
            if b <= 0.0 {
                continue;
            }
            let i = (y * w + x) * 4;
            for c in 0..3 {
                let t = srgb_to_linear(out.data[i + c] as f64 / 255.0);
                let lin = t * (1.0 - b as f64) + colour[c] as f64 * b as f64;
                out.data[i + c] = (linear_to_srgb(lin.clamp(0.0, 1.0)) * 255.0).round() as u8;
            }
        }
    }
    out
}

/// An affine map between two UV sets, `to = a * from + b` per coordinate, fitted by least squares; with the root
/// mean square residual.
#[derive(Clone, Copy, Debug)]
pub struct Affine {
    pub rows: [[f64; 3]; 2],
    pub rms: f64,
}

impl Affine {
    pub fn fit(from: &[[f32; 2]], to: &[[f32; 2]]) -> Option<Affine> {
        if from.len() < 3 || from.len() != to.len() {
            return None;
        }
        // normal equations: M (3x3) = sum x x^T with x = (u, v, 1); r_k = sum x * to_k
        let mut m = [[0f64; 3]; 3];
        let mut r = [[0f64; 3]; 2];
        for (f, t) in from.iter().zip(to) {
            let x = [f[0] as f64, f[1] as f64, 1.0];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += x[i] * x[j];
                }
                r[0][i] += x[i] * t[0] as f64;
                r[1][i] += x[i] * t[1] as f64;
            }
        }
        let det3 = |a: &[[f64; 3]; 3]| {
            a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
        };
        let d = det3(&m);
        if d.abs() < 1e-12 {
            return None;
        }
        let solve = |rhs: &[f64; 3]| -> [f64; 3] {
            let mut out = [0f64; 3];
            for k in 0..3 {
                let mut mk = m;
                for i in 0..3 {
                    mk[i][k] = rhs[i];
                }
                out[k] = det3(&mk) / d;
            }
            out
        };
        let rows = [solve(&r[0]), solve(&r[1])];
        let mut sq = 0.0;
        for (f, t) in from.iter().zip(to) {
            let p = Affine { rows, rms: 0.0 }.map(*f);
            sq += (p[0] - t[0] as f64).powi(2) + (p[1] - t[1] as f64).powi(2);
        }
        Some(Affine { rows, rms: (sq / from.len() as f64).sqrt() })
    }

    pub fn map(&self, uv: [f32; 2]) -> [f64; 2] {
        let (u, v) = (uv[0] as f64, uv[1] as f64);
        [self.rows[0][0] * u + self.rows[0][1] * v + self.rows[0][2], self.rows[1][0] * u + self.rows[1][1] * v + self.rows[1][2]]
    }
}

/// Bilinear sample of an RGBA image at a UV (wrapping).
fn sample(img: &Image, uv: [f64; 2]) -> [f32; 4] {
    let (w, h) = (img.width as f64, img.height as f64);
    let x = (uv[0].rem_euclid(1.0) * w - 0.5).max(0.0).min(w - 1.0);
    let y = (uv[1].rem_euclid(1.0) * h - 0.5).max(0.0).min(h - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let p = |xx: usize, yy: usize| -> [f32; 4] {
        let i = (yy * img.width + xx) * img.channels;
        let mut out = [0f32, 0.0, 0.0, 255.0];
        for c in 0..img.channels.min(4) {
            out[c] = img.data[i + c] as f32;
        }
        out
    };
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    let mut out = [0f32; 4];
    for k in 0..4 {
        out[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
    }
    out
}

/// A bundle's MeshSet resources, by name.
fn bundle_meshes(bundle: &Bundle) -> Vec<String> {
    bundle.manifest.res.iter().filter(|r| r.rtype == mesh::MESHSET_TYPE).map(|r| r.name.clone()).collect()
}

/// How much of the deck shows through the grip at each texel of a `size` grip texture, 0..255: the cutout's holes
/// (its R at 0) and the bolt heads (the wood mask's B on the top face, reached through the two UV sets' map).
fn base_showing(size: (usize, usize), cutout: Option<&Image>, wood_mask: &Image, map: &Affine) -> Vec<u8> {
    let (w, h) = size;
    let wood_mask = wood_mask.to_rgba();
    let cutout = cutout.map(|c| c.to_rgba());
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let uv1 = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
            let hole = cutout.as_ref().map(|c| 1.0 - sample(c, [uv1[0] as f64, uv1[1] as f64])[0] / 255.0).unwrap_or(0.0);
            let bolt = sample(&wood_mask, map.map(uv1))[2] / 255.0;
            out[y * w + x] = (hole.max(bolt).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

/// `over` (in the grip's UV space) with `under` (in the wood's, through `map`) showing where `show` says.
fn mix_under(over: &Image, under: &Image, show: &[u8], map: &Affine) -> Image {
    let mut out = over.to_rgba();
    let under = under.to_rgba();
    let (w, h) = (out.width, out.height);
    for y in 0..h {
        for x in 0..w {
            let s = show.get(y * w + x).copied().unwrap_or(0) as f32 / 255.0;
            if s <= 0.0 {
                continue;
            }
            let uv1 = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
            let u = sample(&under, map.map(uv1));
            let i = (y * w + x) * 4;
            for c in 0..3 {
                out.data[i + c] = (out.data[i + c] as f32 * (1.0 - s) + u[c] * s).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// A texture laid out in a part's second UV set carried into its first: the part's triangles rasterized in the
/// first set's space at `size`, each texel's second-set coordinates interpolated, `src` sampled there. Returns the
/// image and a coverage (255 where a triangle was drawn).
fn transfer_uv1_to_uv0(part: &Part, src: &Image, size: (usize, usize)) -> (Image, Vec<u8>) {
    let (w, h) = size;
    let mut out = Image::new(w, h, 4);
    let mut cover = vec![0u8; w * h];
    let src = src.to_rgba();
    for t in part.indices.chunks_exact(3) {
        let p: Vec<[f32; 2]> = t.iter().map(|&i| [part.uv0[i as usize][0] * w as f32, part.uv0[i as usize][1] * h as f32]).collect();
        let q: Vec<[f32; 2]> = t.iter().map(|&i| part.uv1[i as usize]).collect();
        let x0 = p.iter().map(|v| v[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let x1 = (p.iter().map(|v| v[0]).fold(f32::NEG_INFINITY, f32::max).ceil().max(0.0) as usize).min(w);
        let y0 = p.iter().map(|v| v[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let y1 = (p.iter().map(|v| v[1]).fold(f32::NEG_INFINITY, f32::max).ceil().max(0.0) as usize).min(h);
        let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
        if area.abs() < 1e-6 {
            continue;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((p[1][0] - px) * (p[2][1] - py) - (p[2][0] - px) * (p[1][1] - py)) / area;
                let w1 = ((p[2][0] - px) * (p[0][1] - py) - (p[0][0] - px) * (p[2][1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.003 || w1 < -0.003 || w2 < -0.003 {
                    continue;
                }
                let uv1 = [q[0][0] * w0 + q[1][0] * w1 + q[2][0] * w2, q[0][1] * w0 + q[1][1] * w1 + q[2][1] * w2];
                let s = sample(&src, [uv1[0] as f64, uv1[1] as f64]);
                let i = (y * w + x) * 4;
                for c in 0..4 {
                    out.data[i + c] = s[c].round().clamp(0.0, 255.0) as u8;
                }
                cover[y * w + x] = 255;
            }
        }
    }
    (out, cover)
}

/// The whole board from the player's install: the three meshes on the rig, with the stock parts' textures.
pub fn read_board_model(game: &Game) -> Result<BoardModel> {
    read_board_model_for(game, &Loadout::stock())
}

/// The graphic wheel Caveman's own wheels are built on (build.rs's `WHEEL_HOST_ITEM`), by its profile key.
pub const OWN_WHEEL_HOST: &str = "Own_WheelGraphic_Gen_Classic_00006";

/// One of the player's own wheels (a Caveman wheel, for the Studio's preview): its face graphic (any image, centre-
/// cropped to a square) and its sRGB colour. It goes on the board as the game will show it once built: the host
/// graphic wheel's mesh, the face and colour laid out by the build's own `build::wheel_textures` over the host's
/// mask, the colour as the urethane's region colour.
#[derive(Clone, Debug)]
pub struct OwnWheel {
    pub face: Image,
    pub colour: [u8; 3],
}

/// The whole board as the player rides it: the deck mesh and the equipped trucks' and wheels' own meshes on the
/// rig, with the equipped parts' textures (each falling back to the stock part's when it cannot be resolved;
/// `notes` says what happened).
pub fn read_board_model_for(game: &Game, loadout: &Loadout) -> Result<BoardModel> {
    read_board_model_with(game, loadout, None)
}

/// The board as `read_board_model_for` reads it, with one of the player's own wheels in place of the loadout's.
pub fn read_board_model_with(game: &Game, loadout: &Loadout, own_wheel: Option<&OwnWheel>) -> Result<BoardModel> {
    let wheels_key: &str = if own_wheel.is_some() { OWN_WHEEL_HOST } else { &loadout.wheels };
    let skeleton = read_skeleton(game)?;
    // the asset's bone -> the run-time lane, by name
    let lane_by_bone: Vec<Option<usize>> = skeleton.names.iter().map(|n| LANES.iter().position(|l| l.eq_ignore_ascii_case(n))).collect();
    let lane_of_bone = |b: u16| lane_by_bone.get(b as usize).copied().flatten();
    let mut notes = Vec::new();
    let shared = game.bundle(SHARED, ITEMS_TOC)?;

    // the deck's bundle: the mesh and the wood
    let deck_bundle_name = game.find_bundle(DECK_BUNDLE_SUFFIX, ITEMS_TOC)?.ok_or_else(|| BoardModelError(format!("no bundle ending {DECK_BUNDLE_SUFFIX}")))?;
    let deck_bundle = game.bundle(&deck_bundle_name, ITEMS_TOC)?;
    let wood = material_from(game, ITEMS_TOC, &deck_bundle, "/deck_gen_popsicle", "Deck_Mat", "wood", MaskKind::Msk, 0);
    let wood_mask = any_texture(game, ITEMS_TOC, &deck_bundle, "/deck_gen_popsicle", "_msk").map(|(i, _)| i);
    let blank_bundle = game.bundle(&item_bundle(game, BLANK_DECK)?, ITEMS_TOC)?;

    // the equipped items' presets, resolved once
    let truck_item = item_preset(game, &shared, &loadout.trucks);
    let wheel_item = item_preset(game, &shared, wheels_key);
    // the trucks' own mesh: the truck item's model asset (type 2); the wheels': the bundle of their base texture
    let truck_model: Option<String> = item_assets(game, &shared, &loadout.trucks).ok().and_then(|a| a.into_iter().find(|(t, _)| *t == 2).map(|(_, n)| n));
    let mut truck_sections: Option<Vec<FullSection>> = None;
    if let Some(model) = &truck_model {
        match asset_bundle(game, model) {
            Ok(b) => match bundle_meshes(&b).first() {
                Some(m) => match mesh::bundle_mesh_full(game, ITEMS_TOC, &b.name, m, 0) {
                    Ok(s) => {
                        notes.push(format!("trucks: mesh {m}"));
                        truck_sections = Some(s);
                    }
                    Err(e) => notes.push(format!("trucks: mesh {m}: {e}; the prototype mesh")),
                },
                None => notes.push(format!("trucks: {} has no mesh; the prototype mesh", b.name)),
            },
            Err(e) => notes.push(format!("trucks: model {model}: {e}; the prototype mesh")),
        }
    }
    let wheel_base_c: Option<String> = wheel_item.as_ref().ok().and_then(|(b, _, _)| {
        b.manifest
            .res
            .iter()
            .find(|r| r.rtype == TEXTURE_TYPE && r.name.contains("wheel") && r.name.ends_with("_c") && !r.name.contains("graphic"))
            .map(|r| r.name.clone())
    });
    let mut wheel_sections: Option<Vec<FullSection>> = None;
    if let Some(stem) = wheel_base_c.as_deref().and_then(|n| n.rsplit('/').next()).map(|leaf| leaf.trim_end_matches("_c").to_string()) {
        match asset_bundle(game, &stem) {
            Ok(b) => match bundle_meshes(&b).first() {
                Some(m) => match mesh::bundle_mesh_full(game, ITEMS_TOC, &b.name, m, 0) {
                    Ok(s) => {
                        notes.push(format!("wheels: mesh {m}"));
                        wheel_sections = Some(s);
                    }
                    Err(e) => notes.push(format!("wheels: mesh {m}: {e}; the prototype mesh")),
                },
                None => notes.push(format!("wheels: {} has no mesh; the prototype mesh", b.name)),
            },
            Err(e) => notes.push(format!("wheels: model {stem}: {e}; the prototype mesh")),
        }
    }

    // the graphic layer: the equipped deck's graphic, else the Natural Blank's wood graphic; the stock mask and
    // normal map where the deck has none of its own
    let mut graphic = Material { name: "Deck_Mat:graphic".into(), role: "graphic", occlusion: true, tex_coord: 1, ..Material::default() };
    let (graphic_bundle, graphic_stem) = match (!loadout.deck.is_empty()).then(|| item_preset(game, &shared, &loadout.deck)) {
        Some(Ok((b, _, stem))) => {
            notes.push(format!("deck {}: preset {stem}", loadout.deck));
            (b, stem)
        }
        Some(Err(e)) => {
            notes.push(format!("deck {}: {e}; the Natural Blank's graphic (the window puts the art on)", loadout.deck));
            (blank_bundle.clone(), BLANK_DECK.to_string())
        }
        None => (blank_bundle.clone(), BLANK_DECK.to_string()),
    };
    if let Some((img, src)) = pick_texture(game, &graphic_bundle, &graphic_stem, "_c", "graphic") {
        graphic.base = Some(img);
        graphic.sources.push(src);
    }
    match any_texture(game, ITEMS_TOC, &graphic_bundle, &format!("/{graphic_stem}"), "_osk")
        .or_else(|| texture_image(game, ITEMS_TOC, &graphic_bundle, GRAPHIC_DEFAULT_OSK).ok().map(|i| (i, GRAPHIC_DEFAULT_OSK.to_string())))
        .or_else(|| texture_image(game, ITEMS_TOC, &blank_bundle, GRAPHIC_DEFAULT_OSK).ok().map(|i| (i, GRAPHIC_DEFAULT_OSK.to_string())))
    {
        Some((img, src)) => {
            graphic.orm = Some(orm_of(&img, MaskKind::GraphicOsk));
            graphic.sources.push(src);
        }
        None => notes.push("the graphic's mask was not found".into()),
    }
    if let Some((img, src)) = any_texture(game, ITEMS_TOC, &graphic_bundle, &format!("/{graphic_stem}"), "_ny")
        .or_else(|| texture_image(game, ITEMS_TOC, &graphic_bundle, GRAPHIC_DEFAULT_NY).ok().map(|i| (i, GRAPHIC_DEFAULT_NY.to_string())))
        .or_else(|| texture_image(game, ITEMS_TOC, &blank_bundle, GRAPHIC_DEFAULT_NY).ok().map(|i| (i, GRAPHIC_DEFAULT_NY.to_string())))
    {
        graphic.normal = Some(img);
        graphic.sources.push(src);
    }

    // the meshes, split per bone (and the deck body per UDIM tile), into their bones' spaces
    let deck_sections = mesh::bundle_mesh_full(game, ITEMS_TOC, &deck_bundle_name, DECK_MESH, 0)?;
    let mut parts = Vec::new();
    let mut mixed_triangles = 0;
    for s in &deck_sections {
        if s.material.ends_with("_Shadow") {
            continue;
        }
        if s.material.starts_with("DeckTop") {
            parts.extend(parts_of(s, &s.triangles, &skeleton, &lane_of_bone, ROLE_GRIP, &s.material, GRIP_TILE, "the grip")?);
            continue;
        }
        let (tiles, mixed) = split_by_tile(s);
        mixed_triangles += mixed;
        for (tile, tris) in tiles {
            let (material, label) = if tile == GRAPHIC_TILE { (ROLE_GRAPHIC, "graphic") } else { (ROLE_WOOD, "wood") };
            parts.extend(parts_of(s, &tris, &skeleton, &lane_of_bone, material, &format!("{}:{label}", s.material), tile, "the deck")?);
        }
    }
    let trucks_sections = match truck_sections {
        Some(s) => s,
        None => mesh::bundle_mesh_full(game, LEVEL_TOC, LEVEL_BUNDLE, TRUCKS_MESH, 0)?,
    };
    for s in trucks_sections.iter().filter(|s| !s.material.ends_with("_Shadow")) {
        parts.extend(parts_of(s, &s.triangles, &skeleton, &lane_of_bone, ROLE_TRUCKS, &s.material, 0, "the trucks")?);
    }
    let wheels_sections = match wheel_sections {
        Some(s) => s,
        None => mesh::bundle_mesh_full(game, LEVEL_TOC, LEVEL_BUNDLE, WHEELS_MESH, 0)?,
    };
    for s in wheels_sections.iter().filter(|s| !s.material.ends_with("_Shadow")) {
        parts.extend(parts_of(s, &s.triangles, &skeleton, &lane_of_bone, ROLE_WHEELS, &s.material, 0, "the wheels")?);
    }
    if parts.is_empty() {
        return fail("no parts came out of the meshes");
    }

    // the grip: the equipped grip colour preset's textures, tinted by its colour, with the deck showing through
    // where the cutout has holes and at the bolt heads (the wood's top face, by the first UV set)
    let grip = {
        let stock = || material_from(game, ITEMS_TOC, &deck_bundle, "gripcolor", "DeckTop_mat", "grip", MaskKind::GripOsk, 1);
        let mut m = match item_preset(game, &shared, &loadout.grip) {
            Ok((b, doc, stem)) => {
                let mut m = Material { name: "DeckTop_mat".into(), role: "grip", occlusion: true, tex_coord: 1, ..Material::default() };
                if let Some((img, src)) = pick_texture(game, &b, &stem, "_c", "gripcolor") {
                    m.base = Some(match preset_colour(&doc, GRIP_TINT_KEY) {
                        Some(tint) => {
                            notes.push(format!("grip {}: preset {stem}, tint ({:.3}, {:.3}, {:.3}) linear", loadout.grip, tint[0], tint[1], tint[2]));
                            coloured(&img, tint)
                        }
                        None => {
                            notes.push(format!("grip {}: preset {stem}, no tint", loadout.grip));
                            img
                        }
                    });
                    m.sources.push(src);
                }
                if let Some((img, src)) = pick_texture(game, &b, &stem, "_ny", "gripcolor") {
                    m.normal = Some(img);
                    m.sources.push(src);
                }
                if let Some((img, src)) = pick_texture(game, &b, &stem, "_osk", "gripcolor") {
                    m.orm = Some(orm_of(&img, MaskKind::GripOsk));
                    m.sources.push(src);
                }
                if m.base.is_none() {
                    notes.push(format!("grip {}: its preset {stem} has no colour texture; the stock grip", loadout.grip));
                    stock()
                } else {
                    m
                }
            }
            Err(e) => {
                notes.push(format!("grip {}: {e}; the stock grip", loadout.grip));
                stock()
            }
        };
        // the cutout's mask
        let mut cutout: Option<(Image, String)> = None;
        if !loadout.cutout.is_empty() {
            let cut = item_assets(game, &shared, &loadout.cutout).and_then(|assets| {
                let tex = assets
                    .into_iter()
                    .find(|(t, _)| *t == ASSET_TEXTURE)
                    .map(|(_, n)| n)
                    .ok_or_else(|| BoardModelError(format!("{} names no texture", loadout.cutout)))?;
                let b = asset_bundle(game, &tex)?;
                let name = b
                    .manifest
                    .res
                    .iter()
                    .find(|r| r.rtype == TEXTURE_TYPE && r.name.to_lowercase().ends_with(&tex.to_lowercase()))
                    .map(|r| r.name.clone())
                    .ok_or_else(|| BoardModelError(format!("{} has no texture {tex}", b.name)))?;
                Ok((texture_image(game, ITEMS_TOC, &b, &name)?, name))
            });
            match cut {
                Ok((mask, src)) => {
                    let holes = mask.to_rgba().data.chunks_exact(4).filter(|p| p[0] < 250).count();
                    notes.push(format!("cutout {}: {src}, {holes} texel(s) open", loadout.cutout));
                    cutout = Some((mask, src));
                }
                Err(e) => notes.push(format!("cutout {}: {e}; no holes", loadout.cutout)),
            }
        }
        // the deck through the grip: holes and bolt heads, colour and finish
        let mut baked: Option<(Image, Image)> = None;
        if let (Some(grip_part), Some(base), Some(wood_c), Some(wood_msk)) =
            (parts.iter().find(|p| p.material == ROLE_GRIP), m.base.as_ref(), wood.base.as_ref(), wood_mask.as_ref())
        {
            match Affine::fit(&grip_part.uv1, &grip_part.uv0) {
                Some(map) => {
                    let show = base_showing((base.width, base.height), cutout.as_ref().map(|(c, _)| c), wood_msk, &map);
                    let open = show.iter().filter(|&&s| s > 0).count();
                    notes.push(format!("grip: the deck shows through {open} texel(s) (holes and bolt heads); the top's UV map fitted to {:.4} rms", map.rms));
                    let wood_orm = orm_of(wood_msk, MaskKind::Msk);
                    let new_orm = match m.orm.as_ref() {
                        Some(orm) => mix_under(orm, &wood_orm, &show, &map),
                        None => mix_under(&Image::filled(base.width, base.height, &[255, 150, 0, 255]), &wood_orm, &show, &map),
                    };
                    baked = Some((mix_under(base, wood_c, &show, &map), new_orm));
                }
                None => notes.push("grip: the grip's UV sets do not fit an affine map; no holes, no bolt heads".into()),
            }
        }
        if let Some((new_base, new_orm)) = baked {
            m.base = Some(new_base);
            m.orm = Some(new_orm);
            if let Some((_, src)) = cutout {
                m.sources.push(src);
            }
        }
        m
    };

    // the trucks: the equipped truck item's preset textures (the normal map may be another truck's, shared)
    let trucks = {
        let stock = || -> Result<Material> {
            let (name, stem) = first_item_bundle(game, "win32/characters/skateboard/unlicensed/truck/generic/default/")?;
            let b = game.bundle(&name, ITEMS_TOC)?;
            Ok(material_from(game, ITEMS_TOC, &b, &format!("/{stem}"), "mat_skateboard_trucks", "trucks", MaskKind::Msk, 0))
        };
        match &truck_item {
            Ok((b, _, stem)) => {
                let mut m = Material { name: "Truck_Mat".into(), role: "trucks", occlusion: true, tex_coord: 0, ..Material::default() };
                for (suffix, slot) in [("_c", 0), ("_ny", 1), ("_msk", 2)] {
                    if let Some((img, src)) = pick_texture(game, b, stem, suffix, "truck") {
                        match slot {
                            0 => m.base = Some(img),
                            1 => m.normal = Some(img),
                            _ => m.orm = Some(orm_of(&img, MaskKind::Msk)),
                        }
                        m.sources.push(src);
                    }
                }
                if m.base.is_none() {
                    notes.push(format!("trucks {}: preset {stem} has no colour texture; the stock trucks", loadout.trucks));
                    stock()?
                } else {
                    notes.push(format!("trucks {}: preset {stem}", loadout.trucks));
                    m
                }
            }
            Err(e) => {
                notes.push(format!("trucks {}: {e}; the stock trucks", loadout.trucks));
                stock()?
            }
        }
    };

    // the wheels: the base textures coloured by the equipped preset's colour where their mask says; a graphic
    // wheel's face (laid out in the second UV set) carried into the first over that, where its mask says
    let wheels = {
        let stock_bundle = || -> Result<Bundle> {
            let (name, _) = first_item_bundle(game, "win32/characters/skateboard/unlicensed/wheel/generic/classic/wheelcolor/presets/")?;
            Ok(game.bundle(&name, ITEMS_TOC)?)
        };
        let (b, colour, graphic_face, stem) = match &wheel_item {
            Ok((b, doc, stem)) => match own_wheel {
                // the player's own wheel on the host: the face and colour as the build writes them over the host's
                // mask, the colour as the preset's
                Some(own) => {
                    let colour = own.colour.map(|c| srgb_to_linear(c as f64 / 255.0) as f32);
                    let face = any_texture(game, ITEMS_TOC, b, &format!("/{stem}"), "_osk").map(|(host_mask, msrc)| {
                        let (art, mask) = build::wheel_textures(&own.face, own.colour, &host_mask);
                        (art, mask, "the wheel's own face".to_string(), msrc)
                    });
                    (b.clone(), Some(colour), face, stem.clone())
                }
                None => {
                    let colour = preset_colour(doc, WHEEL_COLOUR_KEY);
                    let face = any_texture(game, ITEMS_TOC, b, &format!("/{stem}"), "_c")
                        .and_then(|(c, src)| any_texture(game, ITEMS_TOC, b, &format!("/{stem}"), "_osk").map(|(m, msrc)| (c, m, src, msrc)));
                    (b.clone(), colour, face, stem.clone())
                }
            },
            Err(e) => {
                notes.push(format!("wheels {}: {e}; the stock wheels", wheels_key));
                (stock_bundle()?, None, None, String::new())
            }
        };
        let mut m = material_from(game, ITEMS_TOC, &b, "/wheel_gen_classic", "Wheel_Mat", "wheels", MaskKind::Msk, 0);
        if m.base.is_none() {
            // another base than the classic one: whatever `_c` the bundle holds that is not the graphic
            if let Some(name) = wheel_base_c.as_deref().and_then(|n| n.rsplit('/').next()).map(|l| l.trim_end_matches("_c").to_string()) {
                m = material_from(game, ITEMS_TOC, &b, &format!("/{name}"), "Wheel_Mat", "wheels", MaskKind::Msk, 0);
            }
        }
        if m.base.is_none() {
            let sb = stock_bundle()?;
            m = material_from(game, ITEMS_TOC, &sb, "/wheel_gen_classic", "Wheel_Mat", "wheels", MaskKind::Msk, 0);
        }
        let region_mask = wheel_base_c
            .as_deref()
            .and_then(|n| any_texture(game, ITEMS_TOC, &b, &n[..n.len() - 2], "_msk"))
            .or_else(|| stock_bundle().ok().and_then(|sb| any_texture(game, ITEMS_TOC, &sb, "/wheel_gen_classic", "_msk")))
            .map(|(i, _)| i);
        // a graphic wheel's face, per wheel: each wheel's triangles carry their own second-set coordinates, and the
        // left and right wheels' outer faces fall on different discs of the shared texture, so each wheel gets a
        // texture of its own with the face carried into its first set where the mask says
        let mut per_wheel: Vec<(usize, Image)> = Vec::new();
        if let (Some(base), Some(c)) = (m.base.as_ref(), colour) {
            notes.push(format!("wheels {}: preset {stem}, colour ({:.3}, {:.3}, {:.3}) linear", wheels_key, c[0], c[1], c[2]));
            let tinted = match &region_mask {
                Some(mask) => region_coloured(base, mask, c),
                None => coloured(base, c),
            };
            if let Some((face, mask, src, msrc)) = graphic_face {
                let size = (tinted.width * 2, tinted.height * 2);
                let tinted2 = tinted.resize_lanczos(size.0, size.1);
                let mut shown_total = 0usize;
                for (index, part) in parts.iter().enumerate().filter(|(_, p)| p.material == ROLE_WHEELS) {
                    if !part.uv1.iter().any(|uv| uv[0] != 0.0 || uv[1] != 0.0) {
                        continue;
                    }
                    let mut own = tinted2.clone();
                    let (face0, cover) = transfer_uv1_to_uv0(part, &face, size);
                    let (mask0, _) = transfer_uv1_to_uv0(part, &mask, size);
                    let mut shown = 0usize;
                    for i in 0..size.0 * size.1 {
                        if cover[i] == 0 {
                            continue;
                        }
                        let r = mask0.data[i * 4] as f32 / 255.0;
                        if r <= 0.001 {
                            continue;
                        }
                        shown += 1;
                        for ch in 0..3 {
                            own.data[i * 4 + ch] =
                                (own.data[i * 4 + ch] as f32 * (1.0 - r) + face0.data[i * 4 + ch] as f32 * r).round().clamp(0.0, 255.0) as u8;
                        }
                    }
                    shown_total += shown;
                    per_wheel.push((index, own));
                }
                if per_wheel.is_empty() {
                    notes.push(format!("wheels {}: graphic {src}, but the wheel mesh has no second UV set to place it; the colour alone", wheels_key));
                } else {
                    notes.push(format!("wheels {}: graphic {src} by {msrc}, on {shown_total} texel(s) over {} wheel(s)", wheels_key, per_wheel.len()));
                }
                m.sources.push(src);
                m.sources.push(msrc);
            }
            m.base = Some(tinted);
        } else if colour.is_none() && !stem.is_empty() {
            notes.push(format!("wheels {}: preset {stem} has no colour; the texture as it is", wheels_key));
        }
        (m, per_wheel)
    };
    let (wheels, per_wheel) = wheels;
    let mut materials = vec![wood, grip, trucks, wheels, graphic];
    for (index, own) in per_wheel {
        let mut mat = materials[ROLE_WHEELS].clone();
        mat.name = format!("Wheel_Mat:{}", LANES[parts[index].lane]);
        mat.base = Some(own);
        materials.push(mat);
        parts[index].material = materials.len() - 1;
    }
    Ok(BoardModel { skeleton, parts, materials, mixed_triangles, notes })
}

/// Pads to 4 bytes.
fn pad4(v: &mut Vec<u8>, fill: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(fill);
    }
}

/// Writes the model as a glTF binary: the 16 lane nodes (bind transforms from the asset), the parts as meshes under
/// their lanes, PBR materials with the textures as PNGs. The root node's `extras.caveman` carries the lane names and
/// parents and each material's role, for the window.
pub fn write_glb(model: &BoardModel, out: &Path) -> Result<()> {
    let mut bin: Vec<u8> = Vec::new();
    let mut buffer_views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut images: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    let mut materials: Vec<Value> = Vec::new();
    let mut meshes: Vec<Value> = Vec::new();
    let mut nodes: Vec<Value> = Vec::new();

    let mut view = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| -> usize {
        pad4(bin, 0);
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        let mut v = json!({ "buffer": 0, "byteOffset": offset, "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        buffer_views.push(v);
        buffer_views.len() - 1
    };

    // images and textures
    let mut image_index = |bin: &mut Vec<u8>, img: &Image, name: &str| -> Result<usize> {
        let png = img.encode_png().map_err(|e| BoardModelError(format!("{name}: {e}")))?;
        let bv = view(bin, &png, None);
        images.push(json!({ "bufferView": bv, "mimeType": "image/png", "name": name }));
        textures.push(json!({ "sampler": 0, "source": images.len() - 1 }));
        Ok(textures.len() - 1)
    };
    for m in &model.materials {
        let mut pbr = json!({ "metallicFactor": 1.0, "roughnessFactor": 1.0 });
        let mut mat = json!({ "name": m.name, "doubleSided": false, "extras": { "role": m.role, "sources": m.sources } });
        let tc = m.tex_coord;
        if let Some(img) = &m.base {
            let t = image_index(&mut bin, img, &format!("{}_c", m.name))?;
            pbr["baseColorTexture"] = json!({ "index": t, "texCoord": tc });
        } else {
            pbr["baseColorFactor"] = json!([0.85, 0.85, 0.85, 1.0]);
        }
        if let Some(img) = &m.orm {
            let t = image_index(&mut bin, img, &format!("{}_orm", m.name))?;
            pbr["metallicRoughnessTexture"] = json!({ "index": t, "texCoord": tc });
            if m.occlusion {
                mat["occlusionTexture"] = json!({ "index": t, "texCoord": tc });
            }
        } else {
            pbr["metallicFactor"] = json!(0.0);
            pbr["roughnessFactor"] = json!(0.6);
        }
        if let Some(img) = &m.normal {
            let t = image_index(&mut bin, img, &format!("{}_n", m.name))?;
            mat["normalTexture"] = json!({ "index": t, "texCoord": tc });
        }
        mat["pbrMetallicRoughness"] = pbr;
        materials.push(mat);
    }

    // the lane nodes, with the asset's bind pose as their transforms
    for (lane, name) in LANES.iter().enumerate() {
        let (t, q) = match model.skeleton.index(name) {
            Some(b) => (model.skeleton.local[b].trans, model.skeleton.local[b].quaternion()),
            None => ([0.0; 3], [0.0, 0.0, 0.0, 1.0]),
        };
        let mut node = json!({ "name": name, "translation": t, "rotation": q, "children": [] });
        if lane == 0 {
            node["extras"] = json!({ "caveman": { "lanes": LANES, "laneParents": LANE_PARENTS, "deckLane": DECK_LANE, "roles": model.materials.iter().map(|m| m.role).collect::<Vec<_>>() } });
        }
        nodes.push(node);
    }
    for (lane, parent) in LANE_PARENTS.iter().enumerate() {
        if *parent >= 0 {
            nodes[*parent as usize]["children"].as_array_mut().unwrap().push(json!(lane));
        }
    }

    // the parts
    for part in &model.parts {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in &part.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let floats = |v: &[[f32; 3]]| -> Vec<u8> { v.iter().flat_map(|p| p.iter().flat_map(|x| x.to_le_bytes())).collect() };
        let floats2 = |v: &[[f32; 2]]| -> Vec<u8> { v.iter().flat_map(|p| p.iter().flat_map(|x| x.to_le_bytes())).collect() };
        let pos_bv = view(&mut bin, &floats(&part.positions), Some(34962));
        accessors.push(json!({ "bufferView": pos_bv, "componentType": 5126, "count": part.positions.len(), "type": "VEC3", "min": lo, "max": hi }));
        let pos_acc = accessors.len() - 1;
        let nrm_bv = view(&mut bin, &floats(&part.normals), Some(34962));
        accessors.push(json!({ "bufferView": nrm_bv, "componentType": 5126, "count": part.normals.len(), "type": "VEC3" }));
        let nrm_acc = accessors.len() - 1;
        let uv0_bv = view(&mut bin, &floats2(&part.uv0), Some(34962));
        accessors.push(json!({ "bufferView": uv0_bv, "componentType": 5126, "count": part.uv0.len(), "type": "VEC2" }));
        let uv0_acc = accessors.len() - 1;
        let uv1_bv = view(&mut bin, &floats2(&part.uv1), Some(34962));
        accessors.push(json!({ "bufferView": uv1_bv, "componentType": 5126, "count": part.uv1.len(), "type": "VEC2" }));
        let uv1_acc = accessors.len() - 1;
        let idx_bytes: Vec<u8> = part.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let idx_bv = view(&mut bin, &idx_bytes, Some(34963));
        accessors.push(json!({ "bufferView": idx_bv, "componentType": 5125, "count": part.indices.len(), "type": "SCALAR" }));
        let idx_acc = accessors.len() - 1;
        meshes.push(json!({
            "name": part.name,
            "primitives": [{
                "attributes": { "POSITION": pos_acc, "NORMAL": nrm_acc, "TEXCOORD_0": uv0_acc, "TEXCOORD_1": uv1_acc },
                "indices": idx_acc,
                "material": part.material,
                "mode": 4
            }]
        }));
        nodes.push(json!({ "name": part.name, "mesh": meshes.len() - 1, "extras": { "lane": part.lane, "role": model.materials[part.material].role } }));
        let node_index = nodes.len() - 1;
        nodes[part.lane]["children"].as_array_mut().unwrap().push(json!(node_index));
    }
    pad4(&mut bin, 0);

    let gltf = json!({
        "asset": { "version": "2.0", "generator": format!("Caveman {}", env!("CARGO_PKG_VERSION")) },
        "scene": 0,
        "scenes": [{ "nodes": [0], "name": "skateboard" }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "textures": textures,
        "images": images,
        "samplers": [{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }],
        "accessors": accessors,
        "bufferViews": buffer_views,
        "buffers": [{ "byteLength": bin.len() }],
    });
    let mut json_bytes = serde_json::to_vec(&gltf).map_err(|e| BoardModelError(e.to_string()))?;
    pad4(&mut json_bytes, b' ');
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut file = Vec::with_capacity(total);
    file.extend_from_slice(b"glTF");
    file.extend_from_slice(&2u32.to_le_bytes());
    file.extend_from_slice(&(total as u32).to_le_bytes());
    file.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    file.extend_from_slice(b"JSON");
    file.extend_from_slice(&json_bytes);
    file.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    file.extend_from_slice(b"BIN\0");
    file.extend_from_slice(&bin);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, file)?;
    Ok(())
}

/// Reads the stock board from the install and writes the glTF; returns a few facts for the log.
pub fn build(game: &Game, out: &Path) -> Result<String> {
    build_for(game, &Loadout::stock(), out)
}

/// Reads the board as the loadout has it and writes the glTF; returns a few facts for the log.
pub fn build_for(game: &Game, loadout: &Loadout, out: &Path) -> Result<String> {
    build_with(game, loadout, None, out)
}

/// `build_for` with one of the player's own wheels in place of the loadout's (the Studio's preview).
pub fn build_with(game: &Game, loadout: &Loadout, own_wheel: Option<&OwnWheel>, out: &Path) -> Result<String> {
    let model = read_board_model_with(game, loadout, own_wheel)?;
    write_glb(&model, out)?;
    let vertices: usize = model.parts.iter().map(|p| p.positions.len()).sum();
    let triangles: usize = model.parts.iter().map(|p| p.indices.len() / 3).sum();
    let textures: Vec<String> = model.materials.iter().flat_map(|m| m.sources.iter().cloned()).collect();
    Ok(format!(
        "{} part(s), {vertices} vertices, {triangles} triangles ({} across tiles), {} texture(s): {}; {}",
        model.parts.len(),
        model.mixed_triangles,
        textures.len(),
        textures.join(", "),
        model.notes.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quaternions_of_the_basis() {
        assert_eq!(Transform::IDENTITY.quaternion(), [0.0, 0.0, 0.0, 1.0]);
        // 90 degrees about y: x -> -z, z -> x
        let t = Transform { right: [0.0, 0.0, -1.0], up: [0.0, 1.0, 0.0], forward: [1.0, 0.0, 0.0], trans: [0.0; 3] };
        let q = t.quaternion();
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!((q[1] - s).abs() < 1e-5 && (q[3] - s).abs() < 1e-5 && q[0].abs() < 1e-5 && q[2].abs() < 1e-5, "{q:?}");
        assert_eq!(t.point([1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]);
    }

    #[test]
    fn lanes_and_parents_agree() {
        assert_eq!(LANES.len(), LANE_PARENTS.len());
        for (i, p) in LANE_PARENTS.iter().enumerate() {
            assert!(*p < i as i32, "lane {i}'s parent {p} comes after it");
        }
        assert_eq!(LANES[DECK_LANE], "Deck");
    }

    #[test]
    fn affine_fit_recovers_a_map() {
        let from: Vec<[f32; 2]> = (0..20).map(|i| [(i % 5) as f32 * 0.2, (i / 5) as f32 * 0.25]).collect();
        let to: Vec<[f32; 2]> = from.iter().map(|p| [0.5 * p[0] + 0.01, 0.936 * p[1] + 0.064]).collect();
        let a = Affine::fit(&from, &to).unwrap();
        assert!(a.rms < 1e-6, "{a:?}");
        let p = a.map([0.5, 0.5]);
        assert!((p[0] - 0.26).abs() < 1e-5 && (p[1] - 0.532).abs() < 1e-5, "{p:?}");
    }

    #[test]
    fn colouring_keeps_the_texture_around_the_colour() {
        // a flat mid grey takes the colour exactly; a brighter texel comes out brighter
        let tex = Image { width: 2, height: 1, channels: 4, data: vec![154, 154, 154, 255, 200, 200, 200, 255] };
        let out = coloured(&tex, [0.5, 0.25, 0.1]);
        let expect = |v: f64| (linear_to_srgb(v) * 255.0).round() as u8;
        // the median of two values is the upper one here (index 1), so the brighter texel is the colour itself
        assert_eq!(&out.data[4..7], &[expect(0.5), expect(0.25), expect(0.1)]);
        assert!(out.data[0] < out.data[4] && out.data[1] < out.data[5]);
        let loadout = Loadout::stock();
        assert_eq!(loadout.key().len(), 16);
        assert_ne!(loadout.key(), Loadout { deck: "Own_X".into(), ..Loadout::stock() }.key());
    }

    #[test]
    fn region_colour_goes_where_the_mask_says() {
        let tex = Image { width: 2, height: 1, channels: 4, data: vec![40, 40, 40, 255, 200, 200, 200, 255] };
        let mask = Image { width: 2, height: 1, channels: 4, data: vec![0, 0, 0, 255, 0, 0, 255, 255] };
        let out = region_coloured(&tex, &mask, [1.0, 0.0, 0.0]);
        assert_eq!(&out.data[0..3], &[40, 40, 40], "no region, the texture stays");
        assert_eq!(&out.data[4..7], &[255, 0, 0], "the region takes the colour");
    }

    #[test]
    fn masks_become_orm() {
        let msk = Image { width: 1, height: 1, channels: 4, data: vec![255, 200, 10, 255] };
        assert_eq!(orm_of(&msk, MaskKind::Msk).data, vec![255, 55, 255, 255]);
        let osk = Image { width: 1, height: 1, channels: 4, data: vec![250, 211, 0, 255] };
        let g = orm_of(&osk, MaskKind::GraphicOsk).data;
        assert_eq!((g[0], g[1], g[3]), (250, 44, 255));
        assert!((25..=26).contains(&g[2]), "the ridden lift of B: {}", g[2]);
        assert_eq!(orm_of(&osk, MaskKind::GripOsk).data, vec![255, 44, 0, 255]);
    }
}
