//! Frostbite MeshSet resources: a level of detail's positions and triangles, read from the player's install (ported
//! from ReSkate's GPL `Engine/Resource/mesh_set.cpp`, `read_mesh_positions` and `check_mesh_lod`; the frozen Python
//! prototype `prototypes/tees/prototype/meshlib.py` read the same way). Only what geometry needs: no skinning, no
//! texture coordinates. The stock deck mesh ([`DECK_MESH`]) is found and read by [`deck_mesh`].
//!
//! The resource: three layout words (208, 188, 384 for a skinned mesh; 192, 176, 384 for a static one), the levels of
//! detail as a table of pointers at 48 (a pointer counts from byte 16) and their count (u16 at 180). A level of detail:
//! its kind (1), its sections (count at 8, table at 12), the index format at 84 (33: 16-bit indices), the index and
//! vertex buffer sizes (88, 92) and the geometry chunk's GUID at 116 (the vertex buffer, then the index buffer). A
//! section: primitive type at 31 (3: a triangle list), triangles, first index, where its vertices start in the vertex
//! buffer and how many (32, 36, 40, 44), and its vertex declaration at 112 (16 elements of {usage, format, offset,
//! stream}, a {stride, classification} per stream at +64, the element and stream counts at +96 and +97). A section's
//! streams follow one another from where its vertices start; positions are usage 1, format 3 (three floats) or 8 (four
//! half floats: the deck mesh's).

use std::fmt;

use crate::game::{chunk_loc, Game, GameError, ITEMS_TOC};
use crate::toc::Guid;

/// The MeshSet resource type.
pub const MESHSET_TYPE: u32 = 0x49b1_56d4;

/// The one deck mesh every deck item draws on (`Deck_Mat`; knowledge/shaders.md).
pub const DECK_MESH: &str = "characters/skateboard/unlicensed/deck/generic/popsicle/2022/deck_gen_popsicle_mesh";
/// Its bundle in items.toc (the first one whose name ends so).
pub const DECK_BUNDLE_SUFFIX: &str = "/deck_gen_popsicle_cas_main_bundlereftable";

#[derive(Debug)]
pub struct MeshError(pub String);

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshSet: {}", self.0)
    }
}

impl std::error::Error for MeshError {}

impl From<GameError> for MeshError {
    fn from(e: GameError) -> Self {
        MeshError(e.to_string())
    }
}

fn err<T>(text: impl Into<String>) -> Result<T, MeshError> {
    Err(MeshError(text.into()))
}

const POINTER_BASE: usize = 16;
const LOD_TABLE: usize = 48;
const LOD_COUNT: usize = 180;
const SECTION_COUNT: usize = 8;
const SECTION_TABLE: usize = 12;
const INDEX_FORMAT: usize = 84;
const INDEX_BYTES: usize = 88;
const VERTEX_BYTES: usize = 92;
const GEOMETRY_CHUNK: usize = 116;
const KNOWN_LOD_KIND: u32 = 1;
const SIXTEEN_BIT: u32 = 33;
const PRIMITIVE: usize = 31;
const TRIANGLE_COUNT: usize = 32;
const FIRST_INDEX: usize = 36;
const VERTEX_START: usize = 40;
const VERTEX_COUNT: usize = 44;
const DECLARATION: usize = 112;
const STREAM_TABLE: usize = 64;
const ELEMENT_TOTAL: usize = 96;
const STREAM_TOTAL: usize = 97;
const TRIANGLE_LIST: u8 = 3;
const USAGE_POSITION: u8 = 1;
const FLOAT3: u8 = 3;
/// Four half floats (x, y, z and a w the position does not use): the deck mesh's.
const HALF4: u8 = 8;

/// An IEEE half float.
pub fn half(h: u16) -> f32 {
    let (negative, exp, frac) = (h >> 15 == 1, ((h >> 10) & 0x1f) as i32, (h & 0x3ff) as f32);
    let v = match exp {
        0 => frac * 2.0f32.powi(-24),
        31 if frac == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + frac / 1024.0) * 2.0f32.powi(exp - 15),
    };
    if negative {
        -v
    } else {
        v
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes<const N: usize>(&self, o: usize) -> Result<[u8; N], MeshError> {
        match self.0.get(o..o.saturating_add(N)) {
            Some(b) => Ok(b.try_into().unwrap()),
            None => err("it is truncated"),
        }
    }
    fn u8(&self, o: usize) -> Result<u8, MeshError> {
        Ok(self.bytes::<1>(o)?[0])
    }
    fn u16(&self, o: usize) -> Result<u16, MeshError> {
        Ok(u16::from_le_bytes(self.bytes(o)?))
    }
    fn u32(&self, o: usize) -> Result<u32, MeshError> {
        Ok(u32::from_le_bytes(self.bytes(o)?))
    }
    fn f32(&self, o: usize) -> Result<f32, MeshError> {
        Ok(f32::from_le_bytes(self.bytes(o)?))
    }
    fn pointer(&self, o: usize) -> Result<usize, MeshError> {
        let v = u64::from_le_bytes(self.bytes(o)?);
        if v == 0 || v > (self.0.len() - POINTER_BASE.min(self.0.len())) as u64 {
            return err("a pointer leads outside it");
        }
        Ok(v as usize + POINTER_BASE)
    }
}

/// Which of the two known layouts: skinned (208, 188, 384) or static (192, 176, 384).
fn skinned(r: &Reader) -> Result<bool, MeshError> {
    match (r.u32(0)?, r.u32(4)?, r.u32(8)?) {
        (208, 188, 384) => Ok(true),
        (192, 176, 384) => Ok(false),
        (a, b, c) => err(format!("its layout ({a}, {b}, {c}) is not one this reader knows")),
    }
}

/// A level of detail: its geometry chunk, buffer sizes, index format (33: 16-bit) and section count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lod {
    pub chunk: Guid,
    pub vertex_bytes: u32,
    pub index_bytes: u32,
    pub index_format: u32,
    pub sections: u32,
    pub skinned: bool,
}

/// How many levels of detail a MeshSet has.
pub fn lod_count(res: &[u8]) -> Result<usize, MeshError> {
    let r = Reader(res);
    skinned(&r)?;
    Ok(r.u16(LOD_COUNT)? as usize)
}

/// The `lod`-th level of detail (0 the finest).
pub fn lod(res: &[u8], lod: usize) -> Result<Lod, MeshError> {
    let r = Reader(res);
    let skinned = skinned(&r)?;
    if lod >= r.u16(LOD_COUNT)? as usize {
        return err(format!("it has no level of detail {lod}"));
    }
    let at = r.pointer(LOD_TABLE + lod * 8)?;
    if r.u32(at)? != KNOWN_LOD_KIND {
        return err("the level of detail is of another kind");
    }
    let l = Lod {
        chunk: Guid::from_le_bytes(r.bytes(at + GEOMETRY_CHUNK)?),
        vertex_bytes: r.u32(at + VERTEX_BYTES)?,
        index_bytes: r.u32(at + INDEX_BYTES)?,
        index_format: r.u32(at + INDEX_FORMAT)?,
        sections: r.u32(at + SECTION_COUNT)?,
        skinned,
    };
    if l.chunk == Guid([0; 16]) || l.vertex_bytes == 0 {
        return err("the level of detail is empty");
    }
    if l.sections > 4096 {
        return err("it has too many sections");
    }
    Ok(l)
}

/// One section of a level of detail as read: where its vertices and triangles are in [`Mesh`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub first_vertex: u32,
    pub vertices: u32,
    pub first_triangle: u32,
    pub triangles: u32,
}

/// A level of detail's sections as one mesh: model-space positions, triangles (indices into `positions`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub sections: Vec<Section>,
}

impl Mesh {
    /// The positions' bounding box (min, max).
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }
}

/// A level of detail's positions and triangles from its geometry chunk (the vertex buffer, then the index buffer).
/// Sections with no triangles are skipped; every triangle must name a vertex of its own section.
pub fn read(res: &[u8], lod_index: usize, geometry: &[u8]) -> Result<Mesh, MeshError> {
    let l = lod(res, lod_index)?;
    let r = Reader(res);
    let g = Reader(geometry);
    let (vb, ib) = (l.vertex_bytes as usize, l.index_bytes as usize);
    if geometry.len() < vb + ib {
        return err(format!("the geometry chunk holds {} bytes of the {} its buffers need", geometry.len(), vb + ib));
    }
    let isz = if l.index_format == SIXTEEN_BIT { 2 } else { 4 };
    let at = r.pointer(LOD_TABLE + lod_index * 8)?;
    let mut mesh = Mesh::default();
    if l.sections == 0 {
        return Ok(mesh);
    }
    let table = r.pointer(at + SECTION_TABLE)?;
    // a static mesh's sections are 384 or 416 bytes apart (check_mesh_lod): take the one under which every section reads
    let strides: &[usize] = if l.skinned { &[384] } else { &[384, 416] };
    let stride = strides
        .iter()
        .copied()
        .find(|&st| (0..l.sections as usize).all(|i| section_reads(&r, table + i * st, isz, vb, ib).unwrap_or(false)))
        .ok_or_else(|| MeshError("its sections read neither 384 nor 416 bytes apart".into()))?;
    for i in 0..l.sections as usize {
        let s = table + i * stride;
        let triangles = r.u32(s + TRIANGLE_COUNT)? as usize;
        let count = r.u32(s + VERTEX_COUNT)? as usize;
        if triangles == 0 || count == 0 {
            continue;
        }
        let d = s + DECLARATION;
        let (elements, streams) = (r.u8(d + ELEMENT_TOTAL)? as usize, r.u8(d + STREAM_TOTAL)? as usize);
        if elements > 16 || streams == 0 || streams > 16 {
            return err(format!("section {i}'s vertex declaration is malformed"));
        }
        let mut bases = [0usize; 16];
        let mut stream_strides = [0usize; 16];
        let mut end = r.u32(s + VERTEX_START)? as usize;
        for k in 0..streams {
            stream_strides[k] = r.u8(d + STREAM_TABLE + k * 2)? as usize;
            if stream_strides[k] == 0 {
                return err(format!("section {i} has a vertex stream of no size"));
            }
            bases[k] = end;
            end += stream_strides[k] * count;
        }
        if end > vb {
            return err(format!("section {i}'s vertices run past the vertex buffer ({end} of {vb} bytes)"));
        }
        let position = (0..elements).map(|e| d + e * 4).find(|&e| r.u8(e).ok() == Some(USAGE_POSITION));
        let Some(e) = position else { return err(format!("section {i} has no positions")) };
        let (format, offset, stream) = (r.u8(e + 1)?, r.u8(e + 2)? as usize, r.u8(e + 3)? as usize);
        let size = match format {
            FLOAT3 => 12,
            HALF4 => 8,
            _ => return err(format!("section {i}'s positions have format {format} (this reader knows 3, three floats, and 8, four halves)")),
        };
        if stream >= streams || offset + size > stream_strides[stream] {
            return err(format!("section {i}'s positions run past their stream"));
        }
        let first_vertex = mesh.positions.len() as u32;
        for v in 0..count {
            let o = bases[stream] + v * stream_strides[stream] + offset;
            let p = if format == FLOAT3 { [g.f32(o)?, g.f32(o + 4)?, g.f32(o + 8)?] } else { [half(g.u16(o)?), half(g.u16(o + 2)?), half(g.u16(o + 4)?)] };
            if !p.iter().all(|c| c.is_finite()) {
                return err(format!("section {i}'s vertex {v} is not finite"));
            }
            mesh.positions.push(p);
        }
        let first_triangle = mesh.triangles.len() as u32;
        let start = vb + r.u32(s + FIRST_INDEX)? as usize * isz;
        for t in 0..triangles {
            let mut tri = [0u32; 3];
            for (c, slot) in tri.iter_mut().enumerate() {
                let o = start + (t * 3 + c) * isz;
                let index = if isz == 2 { g.u16(o)? as u32 } else { g.u32(o)? };
                if index as usize >= count {
                    return err(format!("section {i}'s triangle {t} names vertex {index} of its {count}"));
                }
                *slot = first_vertex + index;
            }
            mesh.triangles.push(tri);
        }
        mesh.sections.push(Section { first_vertex, vertices: count as u32, first_triangle, triangles: triangles as u32 });
    }
    Ok(mesh)
}

/// Whether the section at `s` reads as one: a triangle list whose indices are in the index buffer and whose vertices
/// start in the vertex buffer (or one with no triangles).
fn section_reads(r: &Reader, s: usize, isz: usize, vb: usize, ib: usize) -> Result<bool, MeshError> {
    let triangles = r.u32(s + TRIANGLE_COUNT)? as usize;
    if triangles == 0 {
        return Ok(true);
    }
    Ok(r.u8(s + PRIMITIVE)? == TRIANGLE_LIST && (r.u32(s + VERTEX_START)? as usize) < vb && r.u32(s + FIRST_INDEX)? as usize * isz + triangles * 3 * isz <= ib)
}

/// The level root's TOC and bundle, where the prototype skateboard's truck and wheel meshes live (the ones every
/// truck and wheel item draws on), and those meshes.
pub const LEVEL_TOC: &str = "levels/game/bam_levelroot/bam_levelroot.toc";
pub const LEVEL_BUNDLE: &str = "win32/levels/game/bam_levelroot/bam_levelroot";
pub const TRUCKS_MESH: &str = "characters/_prototype/skateboard/trucks/skateboard_trucks_default_mesh";
pub const WHEELS_MESH: &str = "characters/_prototype/skateboard/wheels/skateboard_wheels_default_mesh";

/// A MeshSet resource of a bundle of items.toc, by name, and its geometry chunk's bytes (from the TOC, else from the
/// bundle's own chunks), for a level of detail.
pub fn bundle_mesh(game: &Game, bundle: &str, name: &str, lod_index: usize) -> Result<Mesh, MeshError> {
    bundle_mesh_in(game, ITEMS_TOC, bundle, name, lod_index)
}

/// The stock trucks' finest level of detail, from the player's install (the level root's TOC).
pub fn trucks_mesh(game: &Game) -> Result<Mesh, MeshError> {
    bundle_mesh_in(game, LEVEL_TOC, LEVEL_BUNDLE, TRUCKS_MESH, 0)
}

/// The stock wheels' finest level of detail, from the player's install (the level root's TOC).
pub fn wheels_mesh(game: &Game) -> Result<Mesh, MeshError> {
    bundle_mesh_in(game, LEVEL_TOC, LEVEL_BUNDLE, WHEELS_MESH, 0)
}

/// [`bundle_mesh`] for a bundle of any TOC.
pub fn bundle_mesh_in(game: &Game, toc_name: &str, bundle: &str, name: &str, lod_index: usize) -> Result<Mesh, MeshError> {
    let b = game.bundle(bundle, toc_name)?;
    let i = b.manifest.res.iter().position(|r| r.rtype == MESHSET_TYPE && r.name == name).ok_or_else(|| MeshError(format!("no MeshSet {name} in {bundle}")))?;
    let loc = b.res_loc(i).ok_or_else(|| MeshError(format!("{name} has no file in {bundle}")))?;
    let res = game.decode(&loc)?;
    let l = lod(&res, lod_index)?;
    let geometry = match game.chunk(&l.chunk, toc_name)? {
        Some(c) if !c.removed => game.decode(&chunk_loc(&c))?,
        _ => {
            let ci = b
                .manifest
                .chunk_index(&l.chunk)
                .ok_or_else(|| MeshError(format!("{name}'s geometry chunk {} is in neither items.toc nor {bundle}", l.chunk)))?;
            game.decode(&b.chunk_loc(ci).ok_or_else(|| MeshError(format!("{name}'s geometry chunk has no file in {bundle}")))?)?
        }
    };
    read(&res, lod_index, &geometry)
}

/// The stock deck mesh's finest level of detail ([`DECK_MESH`]), from the player's install.
pub fn deck_mesh(game: &Game) -> Result<Mesh, MeshError> {
    let bundle = match game.find_bundle(DECK_BUNDLE_SUFFIX, ITEMS_TOC)? {
        Some(b) => b,
        None => return err(format!("no bundle ending {DECK_BUNDLE_SUFFIX} in items.toc")),
    };
    bundle_mesh(game, &bundle, DECK_MESH, 0)
}

// ---------------------------------------------------------------- the full vertex: UVs, bone, normal (2026-10-30)

const USAGE_BONE_INDICES: u8 = 2;
const USAGE_BONE_WEIGHTS: u8 = 4;
const USAGE_UV0: u8 = 33;
const USAGE_UV1: u8 = 34;
/// The tangent space, four bytes a vertex (the deck programs call it TEXCOORD4): see [`tangent_space_normal`].
const USAGE_TANGENT_SPACE: u8 = 52;
const FLOAT2: u8 = 2;
const HALF2: u8 = 6;
/// Four unsigned bytes (weights, the tangent space), four unsigned shorts (bone indices).
const UBYTE4: u8 = 13;
const USHORT4: u8 = 23;

/// One vertex with everything the skateboard's drawing needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FullVertex {
    pub position: [f32; 3],
    /// Model-space normal (unit), from the tangent-space element; the geometric normal when a section has none.
    pub normal: [f32; 3],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// The skeleton bone with the largest weight (the skateboard's parts are each rigid on one bone).
    pub bone: u16,
}

/// One section of a level of detail with its material's name and full vertices (indices into its own vertices).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FullSection {
    pub material: String,
    pub vertices: Vec<FullVertex>,
    pub triangles: Vec<[u32; 3]>,
    pub has_uv1: bool,
    pub has_normals: bool,
}

/// The normal a vertex's tangent-space element holds, as the deck vertex program decodes it
/// (`shaders/bytecode/28c6a292-47db-b522-0acb-5835520bd809`, DXIL): the four bytes are unorm; two components come
/// from bytes 0 and 1 scaled by 1/sqrt(2) with their signs in flag bits 4 and 5 of byte 3, the third is rebuilt as
/// sqrt(1 - a^2 - b^2) with its sign in bit 0, and bits 1 and 2 say where each goes; byte 2 and bits 6 and 7 are the
/// tangent's angle about the normal (not decoded here). Checked against the geometric normals of the grip (100% of
/// vertices within 25 degrees), the wheels (100%) and the trucks (95%).
pub fn tangent_space_normal(b: [u8; 4]) -> [f32; 3] {
    let u = |k: usize| b[k] as f32 / 255.0;
    let flags = (u(3) * 255.0 + 0.25) as u32;
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let a = u(0) * s - s + if flags & 16 != 0 { s } else { 0.0 };
    let bb = u(1) * s - s + if flags & 32 != 0 { s } else { 0.0 };
    let w0 = (1.0 - (a * a + bb * bb)).abs().sqrt();
    let w = if flags & 1 != 0 { -w0 } else { w0 };
    let (c0, c1) = if flags & 4 != 0 { (w, a) } else { (a, w) };
    let n = if flags & 2 != 0 { [bb, c1, c0] } else { [c0, bb, c1] };
    // the program's components come out in (z, y, x) order against the mesh's axes
    let v = [n[2], n[1], n[0]];
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// A level of detail's sections with full vertices: positions, UV sets, the bone, the normal (from the tangent-space
/// element, else from the geometry) and the material's name (the C string the section points at).
pub fn read_full(res: &[u8], lod_index: usize, geometry: &[u8]) -> Result<Vec<FullSection>, MeshError> {
    let l = lod(res, lod_index)?;
    let r = Reader(res);
    let g = Reader(geometry);
    let (vb, ib) = (l.vertex_bytes as usize, l.index_bytes as usize);
    if geometry.len() < vb + ib {
        return err(format!("the geometry chunk holds {} bytes of the {} its buffers need", geometry.len(), vb + ib));
    }
    let isz = if l.index_format == SIXTEEN_BIT { 2 } else { 4 };
    let at = r.pointer(LOD_TABLE + lod_index * 8)?;
    let mut out = Vec::new();
    if l.sections == 0 {
        return Ok(out);
    }
    let table = r.pointer(at + SECTION_TABLE)?;
    let strides: &[usize] = if l.skinned { &[384] } else { &[384, 416] };
    let stride = strides
        .iter()
        .copied()
        .find(|&st| (0..l.sections as usize).all(|i| section_reads(&r, table + i * st, isz, vb, ib).unwrap_or(false)))
        .ok_or_else(|| MeshError("its sections read neither 384 nor 416 bytes apart".into()))?;
    for i in 0..l.sections as usize {
        let s = table + i * stride;
        let triangles = r.u32(s + TRIANGLE_COUNT)? as usize;
        let count = r.u32(s + VERTEX_COUNT)? as usize;
        if triangles == 0 || count == 0 {
            continue;
        }
        // the material's name: the C string the section's second pointer leads to
        let material = match r.pointer(s + 8) {
            Ok(p) => res[p..].iter().take(64).take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>(),
            Err(_) => String::new(),
        };
        let d = s + DECLARATION;
        let (elements, streams) = (r.u8(d + ELEMENT_TOTAL)? as usize, r.u8(d + STREAM_TOTAL)? as usize);
        if elements > 16 || streams == 0 || streams > 16 {
            return err(format!("section {i}'s vertex declaration is malformed"));
        }
        let mut bases = [0usize; 16];
        let mut stream_strides = [0usize; 16];
        let mut end = r.u32(s + VERTEX_START)? as usize;
        for k in 0..streams {
            stream_strides[k] = r.u8(d + STREAM_TABLE + k * 2)? as usize;
            if stream_strides[k] == 0 {
                return err(format!("section {i} has a vertex stream of no size"));
            }
            bases[k] = end;
            end += stream_strides[k] * count;
        }
        if end > vb {
            return err(format!("section {i}'s vertices run past the vertex buffer ({end} of {vb} bytes)"));
        }
        // (format, offset, stream) per usage
        let element = |usage: u8| -> Option<(u8, usize, usize)> {
            (0..elements).map(|e| d + e * 4).find(|&e| res[e] == usage).map(|e| (res[e + 1], res[e + 2] as usize, res[e + 3] as usize))
        };
        let (pf, po, ps) = element(USAGE_POSITION).ok_or_else(|| MeshError(format!("section {i} has no positions")))?;
        let uv0 = element(USAGE_UV0);
        let uv1 = element(USAGE_UV1);
        let bones = element(USAGE_BONE_INDICES);
        let weights = element(USAGE_BONE_WEIGHTS);
        let tangent = element(USAGE_TANGENT_SPACE);
        let read_uv = |e: Option<(u8, usize, usize)>, v: usize| -> Result<[f32; 2], MeshError> {
            match e {
                Some((HALF2, o, st)) => {
                    let a = bases[st] + v * stream_strides[st] + o;
                    Ok([half(g.u16(a)?), half(g.u16(a + 2)?)])
                }
                Some((FLOAT2, o, st)) => {
                    let a = bases[st] + v * stream_strides[st] + o;
                    Ok([g.f32(a)?, g.f32(a + 4)?])
                }
                Some((f, ..)) => err(format!("section {i}: a UV set of format {f} (this reader knows 6, two halves, and 2, two floats)")),
                None => Ok([0.0, 0.0]),
            }
        };
        let mut vertices = Vec::with_capacity(count);
        for v in 0..count {
            let o = bases[ps] + v * stream_strides[ps] + po;
            let position = match pf {
                FLOAT3 => [g.f32(o)?, g.f32(o + 4)?, g.f32(o + 8)?],
                HALF4 => [half(g.u16(o)?), half(g.u16(o + 2)?), half(g.u16(o + 4)?)],
                f => return err(format!("section {i}'s positions have format {f}")),
            };
            let bone = match (bones, weights) {
                (Some((USHORT4, bo, bs)), Some((UBYTE4, wo, ws))) => {
                    let (ba, wa) = (bases[bs] + v * stream_strides[bs] + bo, bases[ws] + v * stream_strides[ws] + wo);
                    let mut best = (0u8, 0u16);
                    for k in 0..4 {
                        let w = g.u8(wa + k)?;
                        if w > best.0 {
                            best = (w, g.u16(ba + k * 2)?);
                        }
                    }
                    best.1
                }
                (Some((USHORT4, bo, bs)), None) => g.u16(bases[bs] + v * stream_strides[bs] + bo)?,
                _ => 0,
            };
            let normal = match tangent {
                Some((UBYTE4, to, ts)) => {
                    let a = bases[ts] + v * stream_strides[ts] + to;
                    tangent_space_normal(g.bytes::<4>(a)?)
                }
                _ => [0.0; 3],
            };
            vertices.push(FullVertex { position, normal, uv0: read_uv(uv0, v)?, uv1: read_uv(uv1, v)?, bone });
        }
        let start = vb + r.u32(s + FIRST_INDEX)? as usize * isz;
        let mut tris = Vec::with_capacity(triangles);
        for t in 0..triangles {
            let mut tri = [0u32; 3];
            for (c, slot) in tri.iter_mut().enumerate() {
                let o = start + (t * 3 + c) * isz;
                let index = if isz == 2 { g.u16(o)? as u32 } else { g.u32(o)? };
                if index as usize >= count {
                    return err(format!("section {i}'s triangle {t} names vertex {index} of its {count}"));
                }
                *slot = index;
            }
            tris.push(tri);
        }
        let has_normals = tangent.is_some();
        if !has_normals {
            // the geometry's own, area weighted
            let mut acc = vec![[0f32; 3]; count];
            for t in &tris {
                let [a, b, c] = [vertices[t[0] as usize].position, vertices[t[1] as usize].position, vertices[t[2] as usize].position];
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let f = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
                for &k in t {
                    for j in 0..3 {
                        acc[k as usize][j] += f[j];
                    }
                }
            }
            for (vtx, n) in vertices.iter_mut().zip(acc) {
                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                vtx.normal = if l > 1e-12 { [n[0] / l, n[1] / l, n[2] / l] } else { [0.0, 1.0, 0.0] };
            }
        }
        out.push(FullSection { material, vertices, triangles: tris, has_uv1: uv1.is_some(), has_normals });
    }
    Ok(out)
}

/// A bundle's MeshSet by name with full vertices, for a level of detail (as [`bundle_mesh_in`] reads it).
pub fn bundle_mesh_full(game: &Game, toc_name: &str, bundle: &str, name: &str, lod_index: usize) -> Result<Vec<FullSection>, MeshError> {
    let b = game.bundle(bundle, toc_name)?;
    let i = b.manifest.res.iter().position(|r| r.rtype == MESHSET_TYPE && r.name == name).ok_or_else(|| MeshError(format!("no MeshSet {name} in {bundle}")))?;
    let loc = b.res_loc(i).ok_or_else(|| MeshError(format!("{name} has no file in {bundle}")))?;
    let res = game.decode(&loc)?;
    let l = lod(&res, lod_index)?;
    let geometry = match game.chunk(&l.chunk, toc_name)? {
        Some(c) if !c.removed => game.decode(&chunk_loc(&c))?,
        _ => {
            let ci =
                b.manifest.chunk_index(&l.chunk).ok_or_else(|| MeshError(format!("{name}'s geometry chunk {} is in neither the TOC nor {bundle}", l.chunk)))?;
            game.decode(&b.chunk_loc(ci).ok_or_else(|| MeshError(format!("{name}'s geometry chunk has no file in {bundle}")))?)?
        }
    };
    read_full(&res, lod_index, &geometry)
}
