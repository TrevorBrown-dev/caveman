//! The Caveman: one board, customised live (docs/fx-hook-plan.md, "The Caveman"). The Caveman-FX mod ships a plain
//! white deck, "000 - The Caveman", whose graphic is a texture only it uses; this module rewrites that texture on the
//! GPU with art the trainer sends (a BC7 mip chain encoded by caveman-core exactly as a deck build would), and keeps it
//! there: a re-made texture (the deck loaded again) gets the art again.
//!
//! **Finding it** (the worker thread, [`tick`]): every `TextureAsset` object starts with the class's vtable (exe
//! +0x64ec390) and holds its TypeInfo at +8; a scan of the game's private memory for that vtable word, then each
//! candidate's Name (+0x18) compared with [`TEXTURE_NAME`] exactly. From the asset, the chain to the GPU (read live,
//! 2026-10-21, with skwatch, from the game's own deck textures):
//!
//! ```text
//! TextureAsset       +0x00 vtable  +0x08 TypeInfo (-> descriptor, name hash 0xee74fdcd)  +0x18 Name  +0x20 Resource
//! runtime texture    +0x00 vtable exe+0x6506c10  +0x44 u32 format (67: BC7 sRGB)  +0x50 u32 width  +0x54 u32 height
//!                    +0x58 u32 depth  +0x5c u32 array size  +0x64 u32 mips  +0x68 u64 name hash (owner_hash of the
//!                    texture's name)  +0xd0 ID3D12Resource*  +0x110 RenderCore2's resource object
//! RenderCore2 object +0x00 vtable exe+0x658be10  +0x1c u32 bind flags (8)  +0xf8 u32 (8)  +0x118 the same ID3D12Resource*
//! ```
//!
//! Everything is checked before anything is written: the exact name, the vtables, the format, size, depth, array size
//! and mip count, the name hash, and at the GPU the resource's own `GetDesc` (2D, 512 x 2048, 12 mips, one sample, a
//! BC7 format), its device (the one presenting), and its COM vtable (a D3D12Core resource's). Only the texture found
//! this way is ever written; no address comes from the pipe.
//!
//! **Its state.** The game's renderer (Frostbite's RenderCore2, `Engine.Render.Core2.PlatformPcDx12.dll`) uses legacy
//! resource states and tracks them itself; a texture read by shaders is, on the direct queue, in
//! PIXEL_SHADER_RESOURCE | NON_PIXEL_SHADER_RESOURCE (0xc0: RenderCore2's own copy helper moves a "shader resource" from
//! 0xc0 on the direct queue, 0x40 on compute, COMMON on copy). The upload uses, in this order: a state set by hand
//! (`board state ...`); the state the game's own barriers last gave this resource (watched through the depth module's
//! `ResourceBarrier` hooks, [`observe`]); else 0xc0 when RenderCore2's object says shader resource (bind flags 8 and
//! 8 at +0xf8, inferred: not proven); else nothing is written (`state unknown`). Each subresource goes from that state
//! to COPY_DEST and back to exactly it, so the game finds it as it left it.
//!
//! **Writing it** (the render thread, [`on_present`], called from the overlay's Present hook before anything is
//! drawn): an upload buffer of our own on the game's device, filled row by row from the chain (the footprints from
//! `GetCopyableFootprints`), then on our own command list: the transitions, `CopyTextureRegion` for each of the 12
//! mips, the transitions back; executed on the queue the game presents with (so after the frame that sampled it, before
//! the next), our fence signalled. About 1.4 MB, well inside a frame's 2 ms budget; a frame where the lock or the GPU is
//! busy is skipped and the upload goes on the next.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};
use windows::core::Interface;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D::ID3DDestructionNotifier;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::game::rva;
use crate::scan::WordScan;
use crate::win;

/// The Caveman's graphic texture, as the game names it (`fxmod::cave_texture_name("000 - ")` in caveman-core).
pub const TEXTURE_NAME: &str = "characters/customization/reskate/board_bottomart/000_-_the_caveman_role3_2259d419-387f-b1ba-30fc-c5140100b012";
pub const WIDTH: u32 = 512;
pub const HEIGHT: u32 = 2048;
pub const LEVELS: usize = 12;
/// Frostbite's texture format number for BC7 sRGB (the texture header's, and the runtime texture's +0x44).
pub const FB_BC7_SRGB: u32 = 67;
/// BC7 linear: the mask, normal map and holo map textures.
pub const FB_BC7_LINEAR: u32 = 66;
/// The deck shader's parameter slots a deck fills, as ParamKey GUIDs (Studio names the textures `_role3_<guid>`): the
/// art (`param_graphic_c`), the mask (`param_graphic_osk`: R occlusion, G smoothness, B metal), the normal map
/// (`param_graphic_ny`) and the holo map (`param_graphic_rgb`: red where the deck is holo). Since 2026-10-22 The
/// Caveman owns a texture in each (built on the holo host), so a finish is a write into the other three.
pub const SLOT_GUIDS: [&str; 4] = [
    "2259d419-387f-b1ba-30fc-c5140100b012",
    "c1b71b8a-395f-e8ce-30fc-c5140100a381",
    "830b93ff-7cad-e8ee-30fc-c5140100c668",
    "698a46f9-c8de-d48e-30fc-c5140100a381",
];

/// A deck slot: which of The Caveman's four textures an art or a target is about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Slot {
    #[default]
    Art,
    Osk,
    Ny,
    Rgb,
}

impl Slot {
    pub const ALL: [Slot; 4] = [Slot::Art, Slot::Osk, Slot::Ny, Slot::Rgb];

    pub fn index(self) -> usize {
        match self {
            Slot::Art => 0,
            Slot::Osk => 1,
            Slot::Ny => 2,
            Slot::Rgb => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Slot::Art => "art",
            Slot::Osk => "osk",
            Slot::Ny => "ny",
            Slot::Rgb => "rgb",
        }
    }

    /// The slot a request names ("" and "art": the graphic; "osk"/"mask", "ny"/"normal", "rgb"/"holo").
    pub fn parse(s: &str) -> Option<Slot> {
        Some(match s {
            "" | "art" => Slot::Art,
            "osk" | "mask" => Slot::Osk,
            "ny" | "normal" => Slot::Ny,
            "rgb" | "holo" => Slot::Rgb,
            _ => return None,
        })
    }

    /// The Caveman's texture in this slot as the game names it (the art's name with the slot's GUID).
    pub fn texture_name(self) -> String {
        TEXTURE_NAME.replace(SLOT_GUIDS[0], SLOT_GUIDS[self.index()])
    }

    /// The slot a texture name is for, when it is one of The Caveman's.
    pub fn of_name(name: &str) -> Option<Slot> {
        Slot::ALL.into_iter().find(|s| s.texture_name() == name)
    }

    /// The Frostbite format of the slot's texture: BC7 sRGB for the art, BC7 linear for the layers.
    pub fn fb_format(self) -> u32 {
        if self == Slot::Art {
            FB_BC7_SRGB
        } else {
            FB_BC7_LINEAR
        }
    }
}
/// The name hash of `TextureAsset` in the game's type system (its descriptor's first u32; the EBX reflection's).
pub const TEXTURE_ASSET_HASH: u32 = 0xee74_fdcd;
/// `mov rax, [rcx+8]; ret`: getType's code.
const GET_TYPE_CODE: [u8; 5] = [0x48, 0x8b, 0x41, 0x08, 0xc3];
/// RenderCore2's "shader resource" bind state (inferred: see the module's notes).
const RC2_SHADER_RESOURCE: u32 = 8;
/// D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE | NON_PIXEL_SHADER_RESOURCE.
pub const SHADER_RESOURCE: u32 = 0xc0;
const COPY_DEST: u32 = 0x400;
const COPY_SOURCE: u32 = 0x800;
/// D3D12_RESOURCE_STATE_GENERIC_READ: every read-only state's bit.
const READ_BITS: u32 = 0xac3;
const ALL_SUBRESOURCES: u32 = 0xffff_ffff;

/// Offsets inside the game's objects (see the module's notes).
pub mod layout {
    pub const ASSET_TYPEINFO: usize = 0x08;
    pub const ASSET_NAME: usize = 0x18;
    pub const ASSET_RESOURCE: usize = 0x20;
    /// The runtime texture's id (consecutive as they are made: a higher id is a newer texture).
    pub const TEX_ID: usize = 0x18;
    pub const TEX_FORMAT: usize = 0x44;
    pub const TEX_WIDTH: usize = 0x50;
    pub const TEX_HEIGHT: usize = 0x54;
    pub const TEX_DEPTH: usize = 0x58;
    pub const TEX_ARRAY: usize = 0x5c;
    pub const TEX_MIPS: usize = 0x64;
    pub const TEX_NAME_HASH: usize = 0x68;
    pub const TEX_D3D12: usize = 0xd0;
    pub const TEX_RC2: usize = 0x110;
    pub const RC2_BIND: usize = 0x1c;
    pub const RC2_STATE: usize = 0xf8;
    pub const RC2_D3D12: usize = 0x118;
}

/// The format of a chain of texture data the board writes: BC7 (16 bytes a 4 x 4 block) or 4 bytes a texel in RGBA or
/// BGRA order (tightly packed rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainFormat {
    Bc7,
    Rgba8,
    Bgra8,
    /// BC1 (8 bytes a 4 x 4 block): the world copy's format (DXGI 70..72).
    Bc1,
}

impl ChainFormat {
    pub fn from_name(name: &str) -> Option<ChainFormat> {
        match name.trim().to_ascii_lowercase().as_str() {
            "bc7" => Some(ChainFormat::Bc7),
            "rgba8" => Some(ChainFormat::Rgba8),
            "bgra8" => Some(ChainFormat::Bgra8),
            "bc1" => Some(ChainFormat::Bc1),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ChainFormat::Bc7 => "bc7",
            ChainFormat::Rgba8 => "rgba8",
            ChainFormat::Bgra8 => "bgra8",
            ChainFormat::Bc1 => "bc1",
        }
    }

    /// The bytes of one level's rows: (rows, bytes a row).
    pub fn level_rows(self, width: usize, height: usize) -> (usize, usize) {
        match self {
            ChainFormat::Bc7 => (height.div_ceil(4), width.div_ceil(4) * 16),
            ChainFormat::Bc1 => (height.div_ceil(4), width.div_ceil(4) * 8),
            ChainFormat::Rgba8 | ChainFormat::Bgra8 => (height, width * 4),
        }
    }

    /// The family of a DXGI format the chain can be written into (the other 4-byte order is swizzled on the way):
    /// BC7 (typeless, unorm, sRGB), RGBA8 (R8G8B8A8 typeless, unorm, sRGB), BGRA8 (B8G8R8A8 and B8G8R8X8 typeless,
    /// unorm, sRGB).
    pub fn family_of(dxgi: i32) -> Option<ChainFormat> {
        match dxgi {
            97..=99 => Some(ChainFormat::Bc7),
            70..=72 => Some(ChainFormat::Bc1),
            27..=29 => Some(ChainFormat::Rgba8),
            87 | 88 | 90..=93 => Some(ChainFormat::Bgra8),
            _ => None,
        }
    }
}

/// The byte size of each level of a chain of `levels` levels of `format`, from `width` x `height` down.
pub fn chain_sizes(format: ChainFormat, width: u32, height: u32, levels: usize) -> Vec<usize> {
    (0..levels)
        .map(|m| {
            let (w, h) = ((width as usize >> m).max(1), (height as usize >> m).max(1));
            let (rows, row) = format.level_rows(w, h);
            rows * row
        })
        .collect()
}

/// The whole chain's size.
pub fn chain_len_for(format: ChainFormat, width: u32, height: u32, levels: usize) -> usize {
    chain_sizes(format, width, height, levels).iter().sum()
}

/// The byte size of each level of the chain: BC7, 16 bytes per 4 x 4 block, 512 x 2048 down to 1 x 1.
pub fn mip_sizes() -> [usize; LEVELS] {
    std::array::from_fn(|m| {
        let (w, h) = ((WIDTH as usize >> m).max(1), (HEIGHT as usize >> m).max(1));
        w.div_ceil(4) * h.div_ceil(4) * 16
    })
}

/// One chain of the art: its format, its size (512 x 2048 for The Caveman's; a target added by hand has its own),
/// how many levels it holds, the levels one after another.
#[derive(Clone, Debug)]
pub struct Chain {
    pub format: ChainFormat,
    pub width: u32,
    pub height: u32,
    pub levels: usize,
    pub bytes: Vec<u8>,
}

impl Chain {
    /// Where level `m` starts.
    pub fn level_offset(&self, m: usize) -> usize {
        chain_sizes(self.format, self.width, self.height, m.min(self.levels)).iter().sum()
    }
}

/// The whole chain's size (1,398,160 bytes).
pub fn chain_len() -> usize {
    mip_sizes().iter().sum()
}

/// A texture's owner hash (caveman-core `texture::owner_hash`): 64-bit djb2 with xor over the lower-case name; the
/// runtime texture keeps it at +0x68.
pub fn owner_hash(name: &str) -> u64 {
    let mut h: u64 = 5381;
    for c in name.to_ascii_lowercase().bytes() {
        h = h.wrapping_mul(33) ^ c as u64;
    }
    h
}

// ---------------------------------------------------------------- what identifies the game's objects

/// The values the chain is checked against: the build's (from its RVAs), or a test host's fakes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ids {
    pub asset_vtable: usize,
    pub type_info: usize,
    pub descriptor: usize,
    pub texture_vtable: usize,
    pub rc2_vtable: usize,
}

impl Ids {
    /// The game's, checked in the running image: getType and the class's function in the asset vtable, the descriptor's
    /// hash, the TypeInfo naming the descriptor (filled at start-up), and the texture and RenderCore2 vtables' slot 0.
    pub fn for_game(base: usize) -> Result<Ids, String> {
        let ids = Ids {
            asset_vtable: base + rva::TEXTURE_ASSET_VTABLE,
            type_info: base + rva::TEXTURE_ASSET_TYPEINFO,
            descriptor: base + rva::TEXTURE_ASSET_DESCRIPTOR,
            texture_vtable: base + rva::RUNTIME_TEXTURE_VTABLE,
            rc2_vtable: base + rva::RC2_RESOURCE_VTABLE,
        };
        let slot = |vt: usize, k: usize| win::read_ptr(vt + 8 * k);
        if slot(ids.asset_vtable, 0) != Some(base + rva::GET_TYPE) || slot(ids.asset_vtable, 1) != Some(base + rva::TEXTURE_ASSET_SLOT1) {
            return Err(format!("the TextureAsset vtable at exe+{:#x} is not as in build {}", rva::TEXTURE_ASSET_VTABLE, crate::game::BUILD));
        }
        let mut code = [0u8; 5];
        if !win::read_into(base + rva::GET_TYPE, &mut code) || code != GET_TYPE_CODE {
            return Err("getType's code is not as in this build".into());
        }
        if win::read_u32(ids.descriptor) != Some(TEXTURE_ASSET_HASH) {
            return Err(format!("the TextureAsset descriptor at exe+{:#x} has another hash", rva::TEXTURE_ASSET_DESCRIPTOR));
        }
        if win::read_ptr(ids.type_info) != Some(ids.descriptor) {
            return Err("TextureAsset's TypeInfo does not name its descriptor (not filled yet?)".into());
        }
        if slot(ids.texture_vtable, 0) != Some(base + rva::RUNTIME_TEXTURE_SLOT0) {
            return Err("the runtime texture's vtable is not as in this build".into());
        }
        if slot(ids.rc2_vtable, 0) != Some(base + rva::RC2_RESOURCE_SLOT0) {
            return Err("RenderCore2's resource vtable is not as in this build".into());
        }
        Ok(ids)
    }
}

/// The chain from The Caveman's TextureAsset to its GPU resource; or a sibling of it (see [`read_sibling`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Target {
    /// 0 for a sibling (found through the texture's pointer, not an asset).
    pub asset: usize,
    pub texture: usize,
    pub resource: usize,
    /// RenderCore2's object for it (0: not found as expected), and its bind flags and the u32 at +0xf8.
    pub rc2: usize,
    pub rc2_bind: u32,
    pub rc2_state: u32,
    /// An equip-time copy of the texture (a sibling), not the asset's own.
    pub sibling: bool,
    /// The composite: the nameless render target the game draws the deck's graphic into at equip, which the ridden
    /// board samples (see [`read_composite`]).
    pub composite: bool,
    /// The runtime texture's id (+0x18): a higher id is a newer texture.
    pub id: u32,
    /// Added by hand (`board target 0x...`): any 2D shape, for the identification test.
    pub manual: bool,
    /// A final (see [`read_final`]): one of the run-time-made copies of The Caveman's exact shape whose name hash is
    /// one of the fixed set the game gives them.
    pub is_final: bool,
    /// The runtime texture's name hash (+0x68), read for a final.
    pub hash: u64,
    /// A raw target's live owner (`board target --resource`): the RenderCore2 object (resource at +0x118) or the
    /// runtime texture (+0xd0) that names the resource, found by a scan for the pointer value; checked again at every
    /// upload and validation.
    pub owner: usize,
    pub owner_texture: bool,
    /// The world copy (see [`read_worldcopy`]): the BC1 texture the game re-fills at every deck equip.
    pub is_worldcopy: bool,
    /// The ridden copy (see [`read_ridden`]): the nameless sRGB RGBA8 deck-shaped texture the in-world board samples.
    pub is_ridden: bool,
    /// The deck slot the texture is in: the art (every kind above), or one of The Caveman's own layers (the mask,
    /// the normal map, the holo map: an asset-based target like the main, found by its name).
    pub slot: Slot,
}

impl Target {
    /// The main: The Caveman's graphic texture itself (the asset's), not a copy of it nor a layer.
    pub fn is_main(&self) -> bool {
        !self.sibling && !self.composite && !self.manual && !self.is_final && !self.is_worldcopy && !self.is_ridden && self.slot == Slot::Art
    }

    /// A layer source: The Caveman's own mask, normal map or holo map texture (the asset's).
    pub fn is_layer(&self) -> bool {
        self.slot != Slot::Art && self.asset != 0
    }
}

/// The world copy: found live with r5i (2026-10-22, pid 113492): equipping a deck made the game upload (COPY_DEST ->
/// 0xc0 on a direct list) a 2D 512 x 2048 12-mip BC1_UNORM (DXGI 71) texture owned by a runtime texture of Frostbite
/// format code 54 whose name hash (+0x68) is fixed across sessions (0x0cb4cea5efea33dd: in the first census at 20:1x
/// with the same hash and format), session-persistent, re-filled at every deck equip: the profile of the ridden
/// board's copy (one reused slot, a cheaper format than the BC7 asset). A runtime texture of the main's size, format
/// code 54 and a name hash in `hashes`; its RenderCore2 object (8 and 8: an ordinary shader resource, so the main's
/// state rule applies).
pub const WORLDCOPY_FORMAT: u32 = 54;
pub const DEFAULT_WORLDCOPY_HASHES: [u64; 1] = [0x0cb4_cea5_efea_33dd];
pub const WORLDCOPIES_MAX: usize = 2;
static WORLDCOPY_ON: AtomicBool = AtomicBool::new(true);

pub fn read_worldcopy(texture: usize, ids: &Ids, hashes: &[u64]) -> Result<Target, String> {
    let p = |a: usize| win::read_ptr(a).ok_or_else(|| format!("{a:#x} is not readable"));
    let u = |a: usize| win::read_u32(a).ok_or_else(|| format!("{a:#x} is not readable"));
    if texture == 0 || !texture.is_multiple_of(8) || texture < win::LOWEST {
        return Err("not a pointer".into());
    }
    if p(texture)? != ids.texture_vtable {
        return Err("not a runtime texture (vtable)".into());
    }
    for (what, off, want) in [
        ("format", layout::TEX_FORMAT, WORLDCOPY_FORMAT),
        ("width", layout::TEX_WIDTH, WIDTH),
        ("height", layout::TEX_HEIGHT, HEIGHT),
        ("depth", layout::TEX_DEPTH, 1),
        ("array size", layout::TEX_ARRAY, 1),
    ] {
        let got = u(texture + off)?;
        if got != want {
            return Err(format!("its {what} is {got}, not {want}"));
        }
    }
    let mips = u(texture + layout::TEX_MIPS)?;
    if !(1..=LEVELS as u32).contains(&mips) {
        return Err(format!("its mip count is {mips}"));
    }
    let hash = win::read_u64(texture + layout::TEX_NAME_HASH).ok_or("its name hash is not readable")?;
    if !hashes.contains(&hash) {
        return Err(format!("its name hash {hash:#x} is not a world copy's"));
    }
    let resource = p(texture + layout::TEX_D3D12)?;
    if !(win::LOWEST..=win::HIGHEST).contains(&resource) || resource % 8 != 0 || win::read_ptr(resource).is_none() {
        return Err(format!("its D3D12 resource {resource:#x} is not readable"));
    }
    let (rc2, rc2_bind, rc2_state) = read_rc2(texture, resource, ids);
    Ok(Target {
        asset: 0,
        texture,
        resource,
        rc2,
        rc2_bind,
        rc2_state,
        sibling: false,
        composite: false,
        id: win::read_u32(texture + layout::TEX_ID).unwrap_or(0),
        manual: false,
        is_final: false,
        hash,
        owner: 0,
        owner_texture: false,
        is_worldcopy: true,
        is_ridden: false,
        slot: Slot::Art,
    })
}

/// The world copies among the runtime texture objects, the main's resource left out, newest first, at most `cap`.
pub fn worldcopies_among(hits: &[usize], ids: &Ids, main: &Target, hashes: &[u64], cap: usize) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    for &h in hits {
        if h == main.texture {
            continue;
        }
        if let Ok(t) = read_worldcopy(h, ids, hashes) {
            if t.resource != main.resource && !out.iter().any(|x| x.resource == t.resource) {
                out.push(t);
            }
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id).then(a.texture.cmp(&b.texture)));
    out.truncate(cap);
    out
}

/// The ridden copy: THE texture the in-world board samples, found 2026-10-22 09:25 by the census hunt (the colour the
/// ridden board showed after a re-equip, written into nothing else, was held by exactly one texture, and writing it
/// changed the board at once, no re-equip). A NAMELESS (hash 0) 2D texture of Frostbite format 20
/// (R8G8B8A8_UNORM_SRGB, DXGI 29) in the deck's shape at full size or demoted by the streamer (512 x 2048, 256 x
/// 1024, 128 x 512: that session's was 256 x 1024 with 9 mips), an ordinary shader resource (RenderCore2 bind flags
/// 8 and +0xf8 8: the main's state rule applies, 0xc0), made at equip (the delivered sRGB result of the equip-time
/// build, then the streamer's demoted copies of it) and released at the next. The newest by id is the one in use.
/// The art is written into it fitted to its size: a demoted size takes the tail of the full RGBA8 chain
/// (`Art::pick_chain` with a level offset).
pub const RIDDEN_FORMAT: u32 = 20;
pub const RIDDENS_MAX: usize = 1;
static RIDDEN_ON: AtomicBool = AtomicBool::new(true);

/// Whether a size is the deck's or a demotion of it (the streamer halves both sides).
pub fn ridden_size(width: u32, height: u32) -> bool {
    matches!((width, height), (512, 2048) | (256, 1024) | (128, 512))
}

pub fn read_ridden(texture: usize, ids: &Ids) -> Result<Target, String> {
    read_ridden_fmt(texture, ids, RIDDEN_FORMAT, Slot::Art)
}

/// The companions of the ridden copy (found 2026-10-22 10:3x by the needle method on the layers): the game makes the
/// composited mask and normal map the in-world board samples as two more nameless textures of the same shape, format
/// 18 (R8G8B8A8_UNORM), bind flags 8 and 8, created right after the graphic's copy: ids +1 (the mask: a magenta
/// needle in the mask source came out in it) and +2 (the normal map). The glitter written into the mask and normal
/// map SOURCES showed in every preview but not on the in-world board, as the graphic did before its copy was found.
pub const RIDDEN_COMPANION_FORMAT: u32 = 18;

/// The format a ridden-copy target of a slot has: sRGB for the graphic's copy, UNORM for the mask's and normal map's.
pub fn ridden_format(slot: Slot) -> u32 {
    if slot == Slot::Art {
        RIDDEN_FORMAT
    } else {
        RIDDEN_COMPANION_FORMAT
    }
}

/// A ridden copy of one slot: a nameless texture of `format` in the deck's shape or a demotion of it, an ordinary
/// shader resource (RenderCore2 8 and 8).
pub fn read_ridden_fmt(texture: usize, ids: &Ids, want_format: u32, slot: Slot) -> Result<Target, String> {
    let p = |a: usize| win::read_ptr(a).ok_or_else(|| format!("{a:#x} is not readable"));
    let u = |a: usize| win::read_u32(a).ok_or_else(|| format!("{a:#x} is not readable"));
    if texture == 0 || !texture.is_multiple_of(8) || texture < win::LOWEST {
        return Err("not a pointer".into());
    }
    if p(texture)? != ids.texture_vtable {
        return Err("not a runtime texture (vtable)".into());
    }
    let format = u(texture + layout::TEX_FORMAT)?;
    if format != want_format {
        return Err(format!("its format code is {format}, not {want_format}"));
    }
    let (w, h) = (u(texture + layout::TEX_WIDTH)?, u(texture + layout::TEX_HEIGHT)?);
    if !ridden_size(w, h) {
        return Err(format!("its size is {w}x{h}, not the deck's or a demotion of it"));
    }
    for (what, off) in [("depth", layout::TEX_DEPTH), ("array size", layout::TEX_ARRAY)] {
        let got = u(texture + off)?;
        if got != 1 {
            return Err(format!("its {what} is {got}, not 1"));
        }
    }
    let mips = u(texture + layout::TEX_MIPS)?;
    if !(1..=LEVELS as u32).contains(&mips) {
        return Err(format!("its mip count is {mips}"));
    }
    if win::read_u64(texture + layout::TEX_NAME_HASH).unwrap_or(1) != 0 {
        return Err("it has a name hash (an asset's texture)".into());
    }
    let resource = p(texture + layout::TEX_D3D12)?;
    if !(win::LOWEST..=win::HIGHEST).contains(&resource) || resource % 8 != 0 || win::read_ptr(resource).is_none() {
        return Err(format!("its D3D12 resource {resource:#x} is not readable"));
    }
    let (rc2, rc2_bind, rc2_state) = read_rc2(texture, resource, ids);
    if rc2 == 0 {
        return Err("no RenderCore2 object of the known vtable names its resource".into());
    }
    if rc2_bind != 8 || rc2_state != 8 {
        return Err(format!("its RenderCore2 object says bind flags {rc2_bind} and +0xf8 {rc2_state}, not 8 and 8 (a working surface, not the sampled copy)"));
    }
    Ok(Target {
        asset: 0,
        texture,
        resource,
        rc2,
        rc2_bind,
        rc2_state,
        sibling: false,
        composite: false,
        id: win::read_u32(texture + layout::TEX_ID).unwrap_or(0),
        manual: false,
        is_final: false,
        hash: 0,
        owner: 0,
        owner_texture: false,
        is_worldcopy: false,
        is_ridden: true,
        slot,
    })
}

/// The ridden copies among the runtime texture objects: the graphic's (the newest, at most `cap`), and for the
/// newest its companions, the mask's (id + 1) and the normal map's (id + 2) of the same size; the main's resource
/// left out.
pub fn riddens_among(hits: &[usize], ids: &Ids, main: &Target, cap: usize) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    for &h in hits {
        if h == main.texture {
            continue;
        }
        if let Ok(t) = read_ridden(h, ids) {
            if t.resource != main.resource && !out.iter().any(|x| x.resource == t.resource) {
                out.push(t);
            }
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id).then(a.texture.cmp(&b.texture)));
    out.truncate(cap);
    if let Some(c) = out.first().copied() {
        let size = |t: usize| (win::read_u32(t + layout::TEX_WIDTH), win::read_u32(t + layout::TEX_HEIGHT));
        let c_size = size(c.texture);
        for &h in hits {
            if h == main.texture || h == c.texture {
                continue;
            }
            let Some(id) = win::read_u32(h + layout::TEX_ID) else { continue };
            let slot = if id == c.id.wrapping_add(1) {
                Slot::Osk
            } else if id == c.id.wrapping_add(2) {
                Slot::Ny
            } else {
                continue;
            };
            if size(h) != c_size {
                continue;
            }
            if let Ok(t) = read_ridden_fmt(h, ids, RIDDEN_COMPANION_FORMAT, slot) {
                if !out.iter().any(|x| x.resource == t.resource) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Whether a raw target's owner still names its resource (a texture made again or released: it does not). A forced
/// raw target (`board target --resource 0x... --force`: no owner; the deck-shaped uploads subset vouched for it, and
/// GetDesc is checked at every upload) has none to check.
pub fn owner_alive(t: &Target, ids: &Ids) -> bool {
    if !t.is_raw() {
        return true;
    }
    if t.owner == 0 {
        return true;
    }
    let (vt, off) = if t.owner_texture { (ids.texture_vtable, layout::TEX_D3D12) } else { (ids.rc2_vtable, layout::RC2_D3D12) };
    vt != 0 && win::read_ptr(t.owner) == Some(vt) && win::read_ptr(t.owner + off) == Some(t.resource)
}

/// The same as a result, with the reason.
pub fn raw_still_owned(t: &Target, ids: &Ids) -> Result<(), String> {
    if owner_alive(t, ids) {
        Ok(())
    } else {
        Err(format!(
            "no live owner ({} {:#x} no longer names it): the game may have released it",
            if t.owner_texture { "runtime texture" } else { "RenderCore2 object" },
            t.owner
        ))
    }
}

/// The name hashes of the finals: the two run-time-made BC7 512 x 2048 12-mip textures the game creates together at
/// every equip (consecutive ids), with name hashes that are the same from one session to the next (seen 2026-10-21
/// twice, with skwatch). They sit next to The Caveman's texture in the material's texture table, and their RenderCore2
/// objects say bind flags 8 and +0xf8 8 like the main's: ordinary shader-resource textures, written as the main is.
pub const FINAL_HASHES: [u64; 2] = [0xb01d_f88f_6992_cc49, 0x0d3c_3d39_3677_0c86];
/// At most this many finals per hash are written (a re-equip makes new ones while the old linger): newest first.
pub const FINALS_PER_HASH: usize = 2;
/// `board finals on|off`: the finals are looked for and written (default on).
static FINALS_ON: AtomicBool = AtomicBool::new(true);

/// A final: a runtime texture of The Caveman's exact shape and format (as a sibling) whose name hash is one of
/// `hashes`. Err says what did not match.
pub fn read_final(texture: usize, ids: &Ids, hashes: &[u64]) -> Result<Target, String> {
    let t = read_sibling(texture, ids)?;
    let hash = win::read_u64(texture + layout::TEX_NAME_HASH).ok_or("its name hash is not readable")?;
    if !hashes.contains(&hash) {
        return Err(format!("its name hash {hash:#x} is not a final's"));
    }
    Ok(Target { sibling: false, is_final: true, hash, ..t })
}

/// The finals among the runtime texture objects (`hits`: the words holding the runtime texture vtable, one per
/// object), the main's resource left out, newest first (the id at +0x18 grows with each texture made), at most
/// `per_hash` of each hash.
pub fn finals_among(hits: &[usize], ids: &Ids, main: &Target, hashes: &[u64], per_hash: usize) -> Vec<Target> {
    let mut all: Vec<Target> = Vec::new();
    for &h in hits {
        if h == main.texture {
            continue;
        }
        if let Ok(t) = read_final(h, ids, hashes) {
            if t.resource != main.resource && !all.iter().any(|x| x.resource == t.resource) {
                all.push(t);
            }
        }
    }
    all.sort_by(|a, b| b.id.cmp(&a.id).then(a.texture.cmp(&b.texture)));
    let mut out: Vec<Target> = Vec::new();
    for t in all {
        if out.iter().filter(|x| x.hash == t.hash).count() < per_hash {
            out.push(t);
        }
    }
    out
}

/// A target added by hand from a runtime texture object's address (`board target 0x...`): the vtable, a readable
/// resource (its real shape from GetDesc on the render thread), the RenderCore2 object if there is one; never
/// inferred states (a state by hand, or the game's barriers seen on it).
pub fn read_manual(texture: usize, ids: &Ids) -> Result<Target, String> {
    let p = |a: usize| win::read_ptr(a).ok_or_else(|| format!("{a:#x} is not readable"));
    if texture == 0 || !texture.is_multiple_of(8) || texture < win::LOWEST {
        return Err("not a pointer".into());
    }
    if p(texture)? != ids.texture_vtable {
        return Err("not a runtime texture (vtable)".into());
    }
    let resource = p(texture + layout::TEX_D3D12)?;
    if !(win::LOWEST..=win::HIGHEST).contains(&resource) || resource % 8 != 0 || win::read_ptr(resource).is_none() {
        return Err(format!("its D3D12 resource {resource:#x} is not readable"));
    }
    let (rc2, rc2_bind, rc2_state) = read_rc2(texture, resource, ids);
    Ok(Target {
        asset: 0,
        texture,
        resource,
        rc2,
        rc2_bind,
        rc2_state,
        sibling: false,
        composite: false,
        id: win::read_u32(texture + layout::TEX_ID).unwrap_or(0),
        manual: true,
        is_final: false,
        hash: 0,
        owner: 0,
        owner_texture: false,
        is_worldcopy: false,
        is_ridden: false,
        slot: Slot::Art,
    })
}

/// A target from a raw `ID3D12Resource` pointer (`board target --resource 0x...`: a texture no runtime texture object
/// names, like the equip-time results the game fills by a copy, 2026-10-21 night): a readable COM object whose
/// vtable is a D3D12 resource's (checked here when our GPU objects exist, else at the upload), its shape from
/// GetDesc on the render thread, its device checked there too. Kind `manual`, no texture, no RenderCore2 object; its
/// state is what the uploads table last saw the game leave it in, else one given by hand.
pub fn read_raw(resource: usize) -> Result<Target, String> {
    if resource == 0 || !resource.is_multiple_of(8) || !(win::LOWEST..=win::HIGHEST).contains(&resource) {
        return Err("not a pointer".into());
    }
    if dead(resource) {
        return Err(format!("resource {resource:#x}: released by the game (its last Release seen)"));
    }
    let vt = win::read_ptr(resource).ok_or_else(|| format!("{resource:#x} is not readable"))?;
    if let Ok(g) = GPU.try_lock() {
        if let Some(gpu) = g.as_ref() {
            if !gpu.resource_vtables.contains(&vt) {
                return Err(format!("resource {resource:#x}: its vtable {vt:#x} is not a D3D12 resource's ({:#x?})", gpu.resource_vtables));
            }
        }
    }
    Ok(Target {
        asset: 0,
        texture: 0,
        resource,
        rc2: 0,
        rc2_bind: 0,
        rc2_state: 0,
        sibling: false,
        composite: false,
        id: 0,
        manual: true,
        is_final: false,
        hash: 0,
        owner: 0,
        owner_texture: false,
        is_worldcopy: false,
        is_ridden: false,
        slot: Slot::Art,
    })
}

impl Target {
    /// A target from a raw resource pointer (no runtime texture object).
    pub fn is_raw(&self) -> bool {
        self.manual && self.texture == 0
    }

    pub fn kind(&self) -> &'static str {
        if self.manual {
            "manual"
        } else if self.slot != Slot::Art {
            match (self.is_ridden, self.slot) {
                (true, Slot::Osk) => "osk copy",
                (true, Slot::Ny) => "ny copy",
                (true, _) => "rgb copy",
                (false, s) => s.name(),
            }
        } else if self.is_ridden {
            "ridden"
        } else if self.is_worldcopy {
            "worldcopy"
        } else if self.is_final {
            "final"
        } else if self.composite {
            "composite"
        } else if self.sibling {
            "sibling"
        } else {
            "main"
        }
    }
}

/// The composite: found live (2026-10-21, skwatch, a census of every 512 x 2048 runtime texture): one of them is no
/// asset's (name hash 0), format 18, 10 mips, RenderCore2 bind flags 128 and +0xf8 256, at the same address before
/// and after a re-equip: a persistent render target the game draws the deck's graphic into (composited, mipped) at
/// equip, and the one the ridden board samples. Read from a runtime texture object: the vtable, the main's size,
/// depth and array size 1, no name hash, a readable D3D12 resource, a RenderCore2 object of the known vtable naming
/// that resource (its flags are reported, not required). Its real format and mip count come from `GetDesc` on the
/// render thread. Err says what did not match.
pub fn read_composite(texture: usize, ids: &Ids) -> Result<Target, String> {
    let p = |a: usize| win::read_ptr(a).ok_or_else(|| format!("{a:#x} is not readable"));
    let u = |a: usize| win::read_u32(a).ok_or_else(|| format!("{a:#x} is not readable"));
    if texture == 0 || !texture.is_multiple_of(8) || texture < win::LOWEST {
        return Err("not a pointer".into());
    }
    if p(texture)? != ids.texture_vtable {
        return Err("not a runtime texture (vtable)".into());
    }
    for (what, off, want) in
        [("width", layout::TEX_WIDTH, WIDTH), ("height", layout::TEX_HEIGHT, HEIGHT), ("depth", layout::TEX_DEPTH, 1), ("array size", layout::TEX_ARRAY, 1)]
    {
        let got = u(texture + off)?;
        if got != want {
            return Err(format!("its {what} is {got}, not {want}"));
        }
    }
    if win::read_u64(texture + layout::TEX_NAME_HASH).unwrap_or(1) != 0 {
        return Err("it has a name hash (an asset's texture)".into());
    }
    let resource = p(texture + layout::TEX_D3D12)?;
    if !(win::LOWEST..=win::HIGHEST).contains(&resource) || resource % 8 != 0 || win::read_ptr(resource).is_none() {
        return Err(format!("its D3D12 resource {resource:#x} is not readable"));
    }
    let (rc2, rc2_bind, rc2_state) = read_rc2(texture, resource, ids);
    if rc2 == 0 {
        return Err("no RenderCore2 object of the known vtable names its resource".into());
    }
    Ok(Target {
        asset: 0,
        texture,
        resource,
        rc2,
        rc2_bind,
        rc2_state,
        sibling: false,
        composite: true,
        id: win::read_u32(texture + layout::TEX_ID).unwrap_or(0),
        manual: false,
        is_final: false,
        hash: 0,
        owner: 0,
        owner_texture: false,
        is_worldcopy: false,
        is_ridden: false,
        slot: Slot::Art,
    })
}

/// The composites among the runtime texture objects (`hits`: the words holding the runtime texture vtable, one per
/// object), the main's resource left out, newest first (the runtime texture's id at +0x18 grows with each one made:
/// a re-equip makes a new composite while the old one lingers), at most `cap`.
pub fn composites_among(hits: &[usize], ids: &Ids, main: &Target, cap: usize) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    for &h in hits {
        if h == main.texture {
            continue;
        }
        if let Ok(t) = read_composite(h, ids) {
            if t.resource != main.resource && !out.iter().any(|x| x.resource == t.resource) {
                out.push(t);
            }
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id).then(a.texture.cmp(&b.texture)));
    out.truncate(cap);
    out
}

/// At most this many composites are written (a re-equip adds one while the old ones linger).
pub const MAX_COMPOSITES: usize = 4;

/// DXGI formats a composite may have (the family RGBA8 or BGRA8, up to 12 mips, 2D, The Caveman's size).
pub fn composite_format_ok(dxgi: i32) -> bool {
    matches!(ChainFormat::family_of(dxgi), Some(ChainFormat::Rgba8 | ChainFormat::Bgra8))
}

/// RenderCore2's object for a runtime texture, when it is as expected (its vtable, and it names the resource).
fn read_rc2(texture: usize, resource: usize, ids: &Ids) -> (usize, u32, u32) {
    if let Some(rc2) = win::read_ptr(texture + layout::TEX_RC2).filter(|&r| r >= win::LOWEST && r % 8 == 0) {
        if ids.rc2_vtable != 0 && win::read_ptr(rc2) == Some(ids.rc2_vtable) && win::read_ptr(rc2 + layout::RC2_D3D12) == Some(resource) {
            return (rc2, win::read_u32(rc2 + layout::RC2_BIND).unwrap_or(0), win::read_u32(rc2 + layout::RC2_STATE).unwrap_or(0));
        }
    }
    (0, 0, 0)
}

/// A sibling of The Caveman's texture: a runtime texture of the same shape (format, size, depth, array size, mips)
/// with a readable D3D12 resource, whatever its name hash (the game makes copies of the texture when the item is
/// equipped, and the ridden board samples one of those, not the asset's own: seen 2026-10-21 with skwatch, the
/// copies' pointers next to the texture's in a material's texture table, 0x30 bytes apart). Err says what did not
/// match.
pub fn read_sibling(texture: usize, ids: &Ids) -> Result<Target, String> {
    let p = |a: usize| win::read_ptr(a).ok_or_else(|| format!("{a:#x} is not readable"));
    let u = |a: usize| win::read_u32(a).ok_or_else(|| format!("{a:#x} is not readable"));
    if texture == 0 || !texture.is_multiple_of(8) || texture < win::LOWEST {
        return Err("not a pointer".into());
    }
    if p(texture)? != ids.texture_vtable {
        return Err("not a runtime texture (vtable)".into());
    }
    let shape = [
        ("format", layout::TEX_FORMAT, FB_BC7_SRGB),
        ("width", layout::TEX_WIDTH, WIDTH),
        ("height", layout::TEX_HEIGHT, HEIGHT),
        ("depth", layout::TEX_DEPTH, 1),
        ("array size", layout::TEX_ARRAY, 1),
        ("mip count", layout::TEX_MIPS, LEVELS as u32),
    ];
    for (what, off, want) in shape {
        let got = u(texture + off)?;
        if got != want {
            return Err(format!("its {what} is {got}, not {want}"));
        }
    }
    let resource = p(texture + layout::TEX_D3D12)?;
    if !(win::LOWEST..=win::HIGHEST).contains(&resource) || resource % 8 != 0 || win::read_ptr(resource).is_none() {
        return Err(format!("its D3D12 resource {resource:#x} is not readable"));
    }
    let (rc2, rc2_bind, rc2_state) = read_rc2(texture, resource, ids);
    Ok(Target {
        asset: 0,
        texture,
        resource,
        rc2,
        rc2_bind,
        rc2_state,
        sibling: true,
        composite: false,
        id: win::read_u32(texture + layout::TEX_ID).unwrap_or(0),
        manual: false,
        is_final: false,
        hash: 0,
        owner: 0,
        owner_texture: false,
        is_worldcopy: false,
        is_ridden: false,
        slot: Slot::Art,
    })
}

/// The siblings around the words that hold the main texture's pointer (`hits`: a material's texture table holds
/// the copies' pointers 0x30 bytes apart from it): the neighbours at -0x90, -0x60, -0x30, +0x30 and +0x60 that read
/// as siblings, the main texture itself and duplicates left out, at most `cap`.
pub fn siblings_around(hits: &[usize], ids: &Ids, main: &Target, cap: usize) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    for &h in hits {
        for d in [-0x90i64, -0x60, -0x30, 0x30, 0x60] {
            let at = h.wrapping_add(d as usize);
            let Some(candidate) = win::read_ptr(at) else { continue };
            if candidate == main.texture || candidate == 0 || out.iter().any(|t| t.texture == candidate) {
                continue;
            }
            if let Ok(t) = read_sibling(candidate, ids) {
                if t.resource != main.resource && !out.iter().any(|x| x.resource == t.resource) {
                    out.push(t);
                    if out.len() >= cap {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// At most this many siblings are written.
pub const MAX_SIBLINGS: usize = 4;

/// Reads and checks the chain from a candidate asset (see the module's notes): The Caveman's graphic. Err says what
/// did not match.
pub fn read_chain(asset: usize, ids: &Ids) -> Result<Target, String> {
    read_chain_slot(asset, ids, Slot::Art)
}

/// The same for whichever of The Caveman's four textures the asset is (by its name): the graphic or a layer.
pub fn read_chain_any(asset: usize, ids: &Ids) -> Result<Target, String> {
    let p = |a: usize| win::read_ptr(a).ok_or_else(|| format!("{a:#x} is not readable"));
    if ids.asset_vtable != 0 && p(asset)? != ids.asset_vtable {
        return Err("not a TextureAsset (vtable)".into());
    }
    let name = win::read_ptr(asset + layout::ASSET_NAME).and_then(|n| win::read_cstr(n, TEXTURE_NAME.len() + 1));
    let slot = name.as_deref().and_then(Slot::of_name).ok_or_else(|| "another texture".to_string())?;
    read_chain_slot(asset, ids, slot)
}

/// Reads and checks the chain from a candidate asset for one slot: the asset's name is the slot's, its runtime
/// texture of the slot's format (BC7 sRGB for the art, BC7 linear for a layer) and The Caveman's shape.
pub fn read_chain_slot(asset: usize, ids: &Ids, slot: Slot) -> Result<Target, String> {
    let p = |a: usize| win::read_ptr(a).ok_or_else(|| format!("{a:#x} is not readable"));
    let u = |a: usize| win::read_u32(a).ok_or_else(|| format!("{a:#x} is not readable"));
    if ids.asset_vtable != 0 && p(asset)? != ids.asset_vtable {
        return Err("not a TextureAsset (vtable)".into());
    }
    if p(asset + layout::ASSET_TYPEINFO)? != ids.type_info || p(ids.type_info)? != ids.descriptor || u(ids.descriptor)? != TEXTURE_ASSET_HASH {
        return Err("not a TextureAsset (type)".into());
    }
    let wanted = slot.texture_name();
    let name = win::read_ptr(asset + layout::ASSET_NAME).and_then(|n| win::read_cstr(n, wanted.len() + 1));
    if name.as_deref() != Some(wanted.as_str()) {
        return Err("another texture".into());
    }
    let texture = p(asset + layout::ASSET_RESOURCE)?;
    if texture == 0 || texture % 8 != 0 {
        return Err("its Resource is not loaded".into());
    }
    if p(texture)? != ids.texture_vtable {
        return Err(format!("its Resource {texture:#x} is not a runtime texture (vtable)"));
    }
    let shape = [
        ("format", layout::TEX_FORMAT, slot.fb_format()),
        ("width", layout::TEX_WIDTH, WIDTH),
        ("height", layout::TEX_HEIGHT, HEIGHT),
        ("depth", layout::TEX_DEPTH, 1),
        ("array size", layout::TEX_ARRAY, 1),
        ("mip count", layout::TEX_MIPS, LEVELS as u32),
    ];
    for (what, off, want) in shape {
        let got = u(texture + off)?;
        if got != want {
            return Err(format!("its runtime texture's {what} is {got}, not {want}"));
        }
    }
    if win::read_u64(texture + layout::TEX_NAME_HASH) != Some(owner_hash(&wanted)) {
        return Err("its runtime texture's name hash is not The Caveman's".into());
    }
    let resource = p(texture + layout::TEX_D3D12)?;
    if !(win::LOWEST..=win::HIGHEST).contains(&resource) || resource % 8 != 0 || win::read_ptr(resource).is_none() {
        return Err(format!("its D3D12 resource {resource:#x} is not readable"));
    }
    let (rc2, rc2_bind, rc2_state) = read_rc2(texture, resource, ids);
    Ok(Target {
        asset,
        texture,
        resource,
        rc2,
        rc2_bind,
        rc2_state,
        sibling: false,
        composite: false,
        id: win::read_u32(texture + layout::TEX_ID).unwrap_or(0),
        manual: false,
        is_final: false,
        hash: 0,
        owner: 0,
        owner_texture: false,
        is_worldcopy: false,
        is_ridden: false,
        slot,
    })
}

// ---------------------------------------------------------------- the texture's state

/// What the game's own barriers did to a watched resource: the state each subresource was last given (legacy
/// barriers), and whether enhanced barriers touched it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Observed {
    pub states: [Option<u32>; LEVELS],
    pub transitions: u64,
    /// The last enhanced barrier's layout after (None: none seen).
    pub layout: Option<i32>,
}

/// Where the state used for an upload came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Manual,
    Observed,
    Inferred,
    /// A composite that never showed a barrier, written from COMMON (`board composite-common on`).
    AssumedCommon,
    /// A target from a raw resource pointer: the state the uploads table last saw the game leave it in.
    LastWrite,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Manual => "set by hand",
            Source::Observed => "the game's own barriers on it",
            Source::Inferred => "inferred from RenderCore2 (shader resource on the direct queue)",
            Source::AssumedCommon => "assumed COMMON (no barriers ever; composite-common on)",
            Source::LastWrite => "the last write the game ended on it (board uploads)",
        }
    }
}

/// What a state set by hand applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualFor {
    Resource(usize),
    Kind(String),
}

impl ManualFor {
    /// `0x...` (a resource) or `kind:composite|main|sibling`.
    pub fn parse(text: &str) -> Result<ManualFor, String> {
        let t = text.trim();
        if let Some(k) = t.strip_prefix("kind:") {
            return match k {
                "composite" | "main" | "sibling" => Ok(ManualFor::Kind(k.to_string())),
                other => Err(format!("kind {other:?}: composite, main or sibling")),
            };
        }
        let hex = t.strip_prefix("0x").ok_or_else(|| format!("{t:?}: a resource (0x...) or kind:composite|main|sibling"))?;
        usize::from_str_radix(hex, 16).map(ManualFor::Resource).map_err(|_| format!("{t:?}: a resource (0x...) or kind:composite|main|sibling"))
    }

    fn matches(&self, t: &Target) -> bool {
        match self {
            ManualFor::Resource(r) => *r == t.resource,
            ManualFor::Kind(k) => k == t.kind(),
        }
    }
}

/// Whether a legacy state is one an upload may start from and must end in: COMMON or read-only.
pub fn usable_state(s: u32) -> bool {
    s & !READ_BITS == 0
}

/// A state as typed: `common`, `srv` (0xc0), `ps` (0x80), `nps` (0x40), `generic` (0xac3), a hex or decimal number;
/// `auto`: None.
pub fn parse_state(text: &str) -> Result<Option<u32>, String> {
    let t = text.trim().to_ascii_lowercase();
    let v = match t.as_str() {
        "auto" => return Ok(None),
        "common" => 0,
        "srv" | "shader" | "shader_resource" => SHADER_RESOURCE,
        "ps" | "pixel" => 0x80,
        "nps" | "non_pixel" => 0x40,
        "generic" | "generic_read" => READ_BITS,
        _ => match t.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16).map_err(|_| format!("state {text:?}: not a number"))?,
            None => t.parse::<u32>().map_err(|_| format!("state {text:?}: auto, common, srv, ps, nps, generic or a number"))?,
        },
    };
    if !usable_state(v) {
        return Err(format!("state {v:#x}: an upload starts from and returns to COMMON or a read-only state only"));
    }
    Ok(Some(v))
}

/// The state of each subresource for an upload (see the module's notes): Err when it is not known (nothing written).
pub fn decide(manual: Option<u32>, seen: &Observed, rc2: Option<(u32, u32)>) -> Result<([u32; LEVELS], Source), String> {
    if let Some(s) = manual {
        return Ok(([s; LEVELS], Source::Manual));
    }
    if let Some(l) = seen.layout {
        return Err(format!("the game moved this texture with an enhanced barrier (layout {l}): not supported; set the state by hand (board state)"));
    }
    let inferred = match rc2 {
        Some((RC2_SHADER_RESOURCE, RC2_SHADER_RESOURCE)) => Some(SHADER_RESOURCE),
        _ => None,
    };
    if seen.states.iter().any(Option::is_some) {
        let mut out = [0u32; LEVELS];
        for (m, s) in seen.states.iter().enumerate() {
            out[m] = match s.or(inferred) {
                Some(v) => v,
                None => return Err(format!("the game's barriers set some mips but not mip {m}, and RenderCore2 says nothing: state unknown")),
            };
            if !usable_state(out[m]) {
                return Err(format!("the game last put mip {m} in state {:#x} (it is writing it?): not touched", out[m]));
            }
        }
        return Ok((out, Source::Observed));
    }
    match (inferred, rc2) {
        (Some(s), _) => Ok(([s; LEVELS], Source::Inferred)),
        (None, Some((bind, state))) => Err(format!("RenderCore2's object holds bind flags {bind} and {state} (not the shader resource 8 and 8 seen before) and no barrier was seen: state unknown; set it by hand (board state)")),
        (None, None) => Err("no RenderCore2 object and no barrier seen: state unknown; set it by hand (board state)".into()),
    }
}

// The barrier watch: the resources being watched, what was seen on them, and upload ends of any resource lately (a
// texture made again shows its first transition before the scan finds it). What runs in the game's barrier hooks
// ([`observe`], [`observe_enhanced`]) only loads and stores atomics: no lock, no allocation, nothing that can wait;
// and it returns at once until a board request made the board live.
const WATCHED: usize = 6;
#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO64: AtomicU64 = AtomicU64::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const STATES0: [AtomicU64; LEVELS] = [ZERO64; LEVELS];
/// The watched resources (0: a free slot).
static WATCH: [AtomicUsize; WATCHED] = [ZERO; WATCHED];
/// Per slot and mip: the state a barrier last gave it, `KNOWN | state` (0: none seen).
static SEEN_STATES: [[AtomicU64; LEVELS]; WATCHED] = [STATES0; WATCHED];
static SEEN_COUNT: [AtomicU64; WATCHED] = [ZERO64; WATCHED];
/// Per slot: an enhanced barrier's layout after, `KNOWN | layout` (0: none).
static SEEN_LAYOUT: [AtomicU64; WATCHED] = [ZERO64; WATCHED];
const KNOWN: u64 = 1 << 32;
const ENDS: usize = 64;
/// The present each ring entry was recorded at: an entry older than `END_MERGE_PRESENTS` is not merged into a new
/// watch (the resource's address may have been reused by a new texture since).
static END_PRESENT: [AtomicU64; ENDS] = [ZERO64; ENDS];
const END_MERGE_PRESENTS: u64 = 300;
/// The states a write leaves: COPY_DEST, RENDER_TARGET, UNORDERED_ACCESS.
const LEAVES: u32 = COPY_DEST | 0x4 | 0x8;

/// `board uploads`: every write the game ended lately (a barrier out of COPY_DEST, RENDER_TARGET or
/// UNORDERED_ACCESS, on any resource), one slot per resource: how many, the first and the last present it happened
/// at, the last transition. Lock-free, since the game records barriers from its job threads: a slot is claimed with
/// a compare-exchange, and the one written longest ago is reused when all are taken, so a resource written every
/// frame keeps its slot (and its count tells) while a one-off stays until it is the oldest.
const UP_SLOTS: usize = 256;
static UP_RES: [AtomicUsize; UP_SLOTS] = [ZERO; UP_SLOTS];
static UP_FIRST: [AtomicU64; UP_SLOTS] = [ZERO64; UP_SLOTS];
static UP_LAST: [AtomicU64; UP_SLOTS] = [ZERO64; UP_SLOTS];
static UP_COUNT: [AtomicU64; UP_SLOTS] = [ZERO64; UP_SLOTS];
/// `before << 32 | after` of the last transition.
static UP_STATE: [AtomicU64; UP_SLOTS] = [ZERO64; UP_SLOTS];
static UP_SUB: [AtomicU64; UP_SLOTS] = [ZERO64; UP_SLOTS];

/// The deck-shaped subset of the uploads table: entries whose resource is 2D of the main's size (the set the render
/// thread fills from the describe cache) or not described yet (a resource just made: its first write is here, and
/// the render thread drops it once described as something else), in a table of their own that only they evict
/// (the one written longest ago first), so the last equip's writes are still there minutes later (`board uploads
/// --deck`), while the main table turns over within seconds.
const UPD_SLOTS: usize = 64;
static UPD_RES: [AtomicUsize; UPD_SLOTS] = [ZERO; UPD_SLOTS];
static UPD_FIRST: [AtomicU64; UPD_SLOTS] = [ZERO64; UPD_SLOTS];
static UPD_LAST: [AtomicU64; UPD_SLOTS] = [ZERO64; UPD_SLOTS];
static UPD_COUNT: [AtomicU64; UPD_SLOTS] = [ZERO64; UPD_SLOTS];
static UPD_STATE: [AtomicU64; UPD_SLOTS] = [ZERO64; UPD_SLOTS];
static UPD_SUB: [AtomicU64; UPD_SLOTS] = [ZERO64; UPD_SLOTS];

/// The render thread: `res` was described as not deck-shaped: out of the deck subset.
fn uploads_deck_drop(res: usize) {
    for (k, slot) in UPD_RES.iter().enumerate() {
        if slot.load(Ordering::Relaxed) == res {
            let _ = slot.compare_exchange(res, 0, Ordering::AcqRel, Ordering::Relaxed);
            UPD_COUNT[k].store(0, Ordering::Relaxed);
        }
    }
}

/// A write ended on `res` (its slot updated, or a slot taken: an empty one, else the one written longest ago), in
/// the main table and, when deck-shaped or not described yet, in the deck subset.
fn note_write_ended(res: usize, sub: u32, before: u32, after: u32) {
    // its shape, described at the next present (again when the last description is old: the address may be another
    // resource's by now)
    want_desc(res);
    let present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    let packed = (before as u64) << 32 | after as u64;
    if crate::copies::in_deck(res) || !crate::copies::described(res) {
        note_in(&UPD_RES, &UPD_FIRST, &UPD_LAST, &UPD_COUNT, &UPD_STATE, &UPD_SUB, res, sub, present, packed);
    }
    note_in(&UP_RES, &UP_FIRST, &UP_LAST, &UP_COUNT, &UP_STATE, &UP_SUB, res, sub, present, packed);
}

#[allow(clippy::too_many_arguments)]
fn note_in(
    up_res: &[AtomicUsize],
    up_first: &[AtomicU64],
    up_last: &[AtomicU64],
    up_count: &[AtomicU64],
    up_state: &[AtomicU64],
    up_sub: &[AtomicU64],
    res: usize,
    sub: u32,
    present: u64,
    packed: u64,
) {
    let (mut slot, mut empty, mut oldest) = (None, None, (u64::MAX, 0usize));
    for k in 0..up_res.len() {
        let r = up_res[k].load(Ordering::Acquire);
        if r == res {
            slot = Some(k);
            break;
        }
        if r == 0 {
            if empty.is_none() {
                empty = Some(k);
            }
            continue;
        }
        let last = up_last[k].load(Ordering::Relaxed);
        if last < oldest.0 {
            oldest = (last, k);
        }
    }
    match slot {
        Some(k) => {
            up_count[k].fetch_add(1, Ordering::Relaxed);
            up_last[k].store(present, Ordering::Relaxed);
            up_state[k].store(packed, Ordering::Relaxed);
            up_sub[k].store(sub as u64, Ordering::Relaxed);
        }
        None => {
            let k = empty.unwrap_or(oldest.1);
            let was = up_res[k].load(Ordering::Acquire);
            // (another thread took the slot first: this write goes uncounted)
            if up_res[k].compare_exchange(was, res, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                up_first[k].store(present, Ordering::Relaxed);
                up_last[k].store(present, Ordering::Relaxed);
                up_count[k].store(1, Ordering::Relaxed);
                up_state[k].store(packed, Ordering::Relaxed);
                up_sub[k].store(sub as u64, Ordering::Relaxed);
            }
        }
    }
}

/// The present the uploads table last saw a write of the game end on `resource` (the main table or the deck subset).
fn uploads_last_present(resource: usize) -> Option<u64> {
    if resource == 0 {
        return None;
    }
    let a = (0..UPD_SLOTS).find(|&k| UPD_RES[k].load(Ordering::Acquire) == resource).map(|k| UPD_LAST[k].load(Ordering::Relaxed));
    let b = (0..UP_SLOTS).find(|&k| UP_RES[k].load(Ordering::Acquire) == resource).map(|k| UP_LAST[k].load(Ordering::Relaxed));
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

/// The state the uploads table last saw the game leave `resource` in (the `after` of its last write ended): the
/// deck-shaped subset first (it keeps an equip's writes for minutes), then the main table.
fn last_write_after(resource: usize) -> Option<u32> {
    if resource == 0 {
        return None;
    }
    (0..UPD_SLOTS)
        .find(|&k| UPD_RES[k].load(Ordering::Acquire) == resource)
        .map(|k| UPD_STATE[k].load(Ordering::Relaxed) as u32)
        .or_else(|| (0..UP_SLOTS).find(|&k| UP_RES[k].load(Ordering::Acquire) == resource).map(|k| UP_STATE[k].load(Ordering::Relaxed) as u32))
}

/// An entry of the uploads table.
#[derive(Clone, Copy, Debug)]
struct Upload {
    resource: usize,
    first: u64,
    last: u64,
    count: u64,
    sub: u32,
    before: u32,
    after: u32,
}

impl Upload {
    /// Written at least every fourth present since it was first seen (a frame regular: a back buffer, which a
    /// swap chain of three writes every third present; a G-buffer target).
    fn regular(&self) -> bool {
        let span = self.last.saturating_sub(self.first) + 1;
        span >= 4 && self.count * 4 >= span
    }
}

/// The table's entries, the last written first (and among those of one present, the rarely written first).
fn upload_entries() -> Vec<Upload> {
    entries_of(&UP_RES, &UP_FIRST, &UP_LAST, &UP_COUNT, &UP_STATE, &UP_SUB)
}

/// The deck subset's entries, the same order.
fn upload_deck_entries() -> Vec<Upload> {
    entries_of(&UPD_RES, &UPD_FIRST, &UPD_LAST, &UPD_COUNT, &UPD_STATE, &UPD_SUB)
}

fn entries_of(
    up_res: &[AtomicUsize],
    up_first: &[AtomicU64],
    up_last: &[AtomicU64],
    up_count: &[AtomicU64],
    up_state: &[AtomicU64],
    up_sub: &[AtomicU64],
) -> Vec<Upload> {
    let mut out = Vec::new();
    for k in 0..up_res.len() {
        let resource = up_res[k].load(Ordering::Acquire);
        if resource == 0 {
            continue;
        }
        let s = up_state[k].load(Ordering::Relaxed);
        out.push(Upload {
            resource,
            first: up_first[k].load(Ordering::Relaxed),
            last: up_last[k].load(Ordering::Relaxed),
            count: up_count[k].load(Ordering::Relaxed),
            sub: up_sub[k].load(Ordering::Relaxed) as u32,
            before: (s >> 32) as u32,
            after: s as u32,
        });
    }
    out.sort_by(|a, b| b.last.cmp(&a.last).then(a.count.cmp(&b.count)).then(a.resource.cmp(&b.resource)));
    out
}

/// What GetDesc said of a resource the hooks saw (the uploads table, the copy tracer), described on the render
/// thread once per resource, at the present after it was first seen (a resource the game releases right after an
/// equip is still whole then).
#[derive(Clone, Copy, Debug)]
struct RingDesc {
    /// The present it was described at.
    at: u64,
    dimension: i32,
    width: u64,
    height: u32,
    mips: u16,
    array: u16,
    format: i32,
    flags: i32,
}

static RING_DESCS: Mutex<Option<HashMap<usize, RingDesc>>> = Mutex::new(None);
/// The render thread is asked to describe the table's and the ring's recent resources (`board uploads`, `board
/// copies`), whether or not the queue below reached them.
static DESCRIBE_RING: AtomicBool = AtomicBool::new(false);
/// An entry last written more than this many presents ago is not described (the resource may be gone).
const RING_DESCRIBE_PRESENTS: u64 = 1800;
/// Resources the hooks saw (a new slot of the uploads table, a copy's source and destination), for the render thread
/// to describe at the next present: a ring the hooks push to and the render thread drains. Each entry holds a
/// reference of ours (AddRef in the hook, where the game is using the object, so it is alive): a resource the game
/// releases right after (an upload buffer, an equip-time result) is still whole when GetDesc runs, and Release
/// follows. An entry overwritten before the render thread took it leaks its reference (counted: `desc_leaks`).
const NEW_RES_N: usize = 8192;
static NEW_RES: [AtomicUsize; NEW_RES_N] = [ZERO; NEW_RES_N];
static NEW_NEXT: AtomicUsize = AtomicUsize::new(0);
static NEW_TAKEN: AtomicUsize = AtomicUsize::new(0);
static DESC_LEAKS: AtomicU64 = AtomicU64::new(0);
/// A description older than this many presents is made again when the resource is seen again (a released
/// resource's address is reused by a new one).
const REDESCRIBE_PRESENTS: u64 = 30;

/// The two resource vtables our GPU objects showed (committed, placed), for the hooks' check (0: not known yet).
static RES_VTABLES: [AtomicUsize; 2] = [ZERO; 2];

// ---------------------------------------------------------------- released resources
//
// A copy from a resource the game released reads freed GPU memory and removes the device (the host test lost its
// device to the hunt reading a texture it had dropped; the game crashed at 21:49 on a forced raw target). A hook on
// the resource classes' `Release` slot (a Release returning 0 marking the pointer dead) was tried for r5h and taken
// out again: with any body beyond a plain call-through it broke the host test (the host's own back buffer writes
// were refused by the debug layer, or an access violation), unexplained in the time there was.
//
// Since r5k the runtime itself tells (no hook): every resource the DLL describes, while a reference of ours holds it
// alive, gets a destruction callback registered through `ID3DDestructionNotifier` (a D3D12 object's documented way
// to say it is being destroyed; every resource class of the runtime carries it, D3D12Core's `CDestructionNotifier`
// base). The runtime calls it on whichever thread does the final release, before the memory goes, and the callback
// marks the pointer dead. The hunt skips dead resources and, while it reads one, holds a reference of its own until
// the GPU is done with the batch (the hunt that crashed the game at 08:5x read textures freed seconds earlier). A
// pointer marked dead and then seen alive again is a new object at the same address: registered again. Every
// registration is undone at unload: a callback into an unloaded DLL would crash the game.

struct Liveness {
    /// The resources with a registration (the callback's id), not dead.
    registered: HashMap<usize, u32>,
    dead: HashSet<usize>,
}
static LIVENESS: Mutex<Option<Liveness>> = Mutex::new(None);
/// Destruction callbacks received; registrations that failed (no notifier interface, or an error).
pub static DEAD_SEEN: AtomicU64 = AtomicU64::new(0);
pub static LIVENESS_FAILED: AtomicU64 = AtomicU64::new(0);

/// The runtime's call when a resource is destroyed (any thread, inside the final release: only the sets touched).
unsafe extern "system" fn dead_callback(pdata: *mut c_void) {
    let r = pdata as usize;
    if let Ok(mut g) = LIVENESS.lock() {
        let l = g.get_or_insert_with(|| Liveness { registered: HashMap::new(), dead: HashSet::new() });
        l.registered.remove(&r);
        l.dead.insert(r);
    }
    DEAD_SEEN.fetch_add(1, Ordering::Relaxed);
}

/// Whether the game is known to have released `res` (its destruction callback came, and nothing alive at that
/// address has been registered since).
pub fn dead(res: usize) -> bool {
    LIVENESS.lock().ok().and_then(|g| g.as_ref().map(|l| l.dead.contains(&res))).unwrap_or(false)
}

/// How many resources are watched, and how many are known dead.
pub fn liveness_counts() -> (usize, usize) {
    LIVENESS.lock().ok().and_then(|g| g.as_ref().map(|l| (l.registered.len(), l.dead.len()))).unwrap_or((0, 0))
}

/// Registers the destruction callback on `res` (alive now: a reference of ours holds it, or its owner was checked
/// this instant), once per live object; an address marked dead earlier is a new object now, alive again.
fn watch_liveness(res: &ID3D12Resource, r: usize) {
    if LIVENESS.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|l| l.registered.contains_key(&r))) {
        return;
    }
    let Ok(n) = res.cast::<ID3DDestructionNotifier>() else {
        LIVENESS_FAILED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    // SAFETY: a live resource (see above); the callback only touches our sets
    match unsafe { n.RegisterDestructionCallback(Some(dead_callback), r as *const c_void) } {
        Ok(id) => {
            if let Ok(mut g) = LIVENESS.lock() {
                let l = g.get_or_insert_with(|| Liveness { registered: HashMap::new(), dead: HashSet::new() });
                l.registered.insert(r, id);
                l.dead.remove(&r);
            }
        }
        Err(_) => {
            LIVENESS_FAILED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// At unload: every registration undone (on the resources still alive by our bookkeeping, whose vtable is still a
/// resource's), so the runtime never calls into the unloaded DLL.
fn unwatch_all() {
    let regs: Vec<(usize, u32)> = LIVENESS.lock().ok().and_then(|mut g| g.as_mut().map(|l| l.registered.drain().collect())).unwrap_or_default();
    let mut undone = 0usize;
    for (r, id) in regs {
        if dead(r) {
            continue;
        }
        let vt = win::read_ptr(r).unwrap_or(0);
        if vt == 0 || (vt != RES_VTABLES[0].load(Ordering::Relaxed) && vt != RES_VTABLES[1].load(Ordering::Relaxed)) {
            continue;
        }
        let raw = r as *mut c_void;
        // SAFETY: a resource alive by its own callback's say-so, of a vtable we know
        if let Some(res) = unsafe { ID3D12Resource::from_raw_borrowed(&raw) } {
            if let Ok(n) = res.cast::<ID3DDestructionNotifier>() {
                if unsafe { n.UnregisterDestructionCallback(id) }.is_ok() {
                    undone += 1;
                }
            }
        }
    }
    if undone > 0 {
        crate::worker::log(&format!("board: {undone} destruction callback(s) unregistered at unload"));
    }
}
/// Resources queued lately (by a hash of the pointer): one queue entry per resource per `REDESCRIBE_PRESENTS`.
const RECENT_N: usize = 1024;
static RECENT_RES: [AtomicUsize; RECENT_N] = [ZERO; RECENT_N];
static RECENT_AT: [AtomicU64; RECENT_N] = [ZERO64; RECENT_N];

/// A resource a hook saw (the game is using it: alive): described at the next present, while the board is live.
/// Only an object whose vtable is one of the two resource vtables we know (read through ReadProcessMemory: never a
/// fault), once per `REDESCRIBE_PRESENTS` per resource.
pub fn want_desc(res: usize) {
    if res == 0 || !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let now = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    let h = (res >> 4) % RECENT_N;
    if RECENT_RES[h].load(Ordering::Relaxed) == res && now.saturating_sub(RECENT_AT[h].load(Ordering::Relaxed)) <= REDESCRIBE_PRESENTS {
        return;
    }
    let vt = win::read_ptr(res).unwrap_or(0);
    if vt == 0 || (vt != RES_VTABLES[0].load(Ordering::Relaxed) && vt != RES_VTABLES[1].load(Ordering::Relaxed)) {
        return;
    }
    RECENT_RES[h].store(res, Ordering::Relaxed);
    RECENT_AT[h].store(now, Ordering::Relaxed);
    let raw = res as *mut c_void;
    // SAFETY: a resource the game just passed to a D3D12 call we are in (a live COM object), of a vtable we know
    let Some(r) = (unsafe { ID3D12Resource::from_raw_borrowed(&raw) }) else { return };
    let held = r.clone();
    let at = NEW_NEXT.fetch_add(1, Ordering::AcqRel) % NEW_RES_N;
    if NEW_RES[at].swap(held.into_raw() as usize, Ordering::AcqRel) != 0 {
        DESC_LEAKS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Takes the queued references (the render thread, or the unload): GetDesc into `descs` when given, Release always.
fn drain_wanted(mut descs: Option<&mut HashMap<usize, RingDesc>>, now: u64) {
    let mut changed = false;
    let next = NEW_NEXT.load(Ordering::Acquire);
    let mut taken = NEW_TAKEN.load(Ordering::Relaxed);
    if next.saturating_sub(taken) > NEW_RES_N {
        taken = next - NEW_RES_N;
    }
    while taken < next {
        let r = NEW_RES[taken % NEW_RES_N].swap(0, Ordering::AcqRel);
        taken += 1;
        if r == 0 {
            continue;
        }
        // SAFETY: the reference the hook took (`want_desc`): ours to describe and release
        let owned = unsafe { ID3D12Resource::from_raw(r as *mut c_void) };
        if let Some(descs) = descs.as_deref_mut() {
            // alive (our reference): its destruction callback registered, once per object
            watch_liveness(&owned, r);
            // (described again when the last description is old: the address may be another resource's by now)
            if descs.get(&r).is_none_or(|d| now.saturating_sub(d.at) > REDESCRIBE_PRESENTS) {
                let d = unsafe { owned.GetDesc() };
                let rd = RingDesc {
                    at: now,
                    dimension: d.Dimension.0,
                    width: d.Width,
                    height: d.Height,
                    mips: d.MipLevels,
                    array: d.DepthOrArraySize,
                    format: d.Format.0,
                    flags: d.Flags.0,
                };
                described_now(r, &rd);
                descs.insert(r, rd);
                changed = true;
            }
        }
        drop(owned);
    }
    NEW_TAKEN.store(taken, Ordering::Relaxed);
    if changed {
        if let Some(descs) = descs.as_deref() {
            refresh_deck_set(descs);
        }
    }
}

/// The shape of a resource, for the status (null: not described).
fn desc_json(descs: Option<&HashMap<usize, RingDesc>>, r: usize, now_present: u64) -> Value {
    match descs.and_then(|m| m.get(&r)) {
        Some(d) => {
            json!({"dimension": d.dimension, "width": d.width, "height": d.height, "mips": d.mips, "array": d.array, "dxgi_format": d.format, "flags": d.flags, "described_presents_ago": now_present.saturating_sub(d.at)})
        }
        None => Value::Null,
    }
}

/// The owner a scan found for a resource (`board uploads`, `board target --resource`): the runtime texture or
/// RenderCore2 object naming it, "none" when the scan found no such object, null when not looked up.
fn owner_json(r: usize) -> Value {
    match OWNERS.lock().ok().and_then(|m| m.iter().find(|e| e.0 == r).map(|e| e.1)) {
        Some(Some((a, true))) => json!({"kind": "runtime texture", "address": format!("{a:#x}")}),
        Some(Some((a, false))) => json!({"kind": "RenderCore2", "address": format!("{a:#x}")}),
        Some(None) => json!("none"),
        None => Value::Null,
    }
}

/// The owners found by the scans, by resource (None: scanned, nothing names it).
static OWNERS: Mutex<Vec<(usize, Option<(usize, bool)>)>> = Mutex::new(Vec::new());

/// `board uploads`: the table (`deck`: its deck-shaped subset) with each resource's shape (GetDesc, when it was
/// described), its owner (when looked up) and whether it is a target.
fn uploads_status(b: &Board, now_present: u64, deck: bool) -> Value {
    let guard = RING_DESCS.lock().ok();
    let descs = guard.as_ref().and_then(|g| g.as_ref());
    if deck { upload_deck_entries() } else { upload_entries() }
        .into_iter()
        .map(|u| {
            json!({
                "resource": format!("{:#x}", u.resource),
                "presents_ago": now_present.saturating_sub(u.last),
                "first_presents_ago": now_present.saturating_sub(u.first),
                "first_present": u.first,
                "last_present": u.last,
                "count": u.count,
                "regular": u.regular(),
                "subresource": if u.sub == ALL_SUBRESOURCES { "all".to_string() } else { u.sub.to_string() },
                "before": format!("{:#x}", u.before),
                "after": format!("{:#x}", u.after),
                "target": b.targets.iter().find(|f| f.t.resource == u.resource).map(|f| f.t.kind()),
                "released": dead(u.resource),
                "owner": owner_json(u.resource),
                "desc": desc_json(descs, u.resource, now_present),
            })
        })
        .collect()
}

/// `board copies`: the copy tracer's generic ring (`deck`: the deck ring) with the shapes.
fn copies_status(now_present: u64, deck: bool) -> Value {
    let guard = RING_DESCS.lock().ok();
    let descs = guard.as_ref().and_then(|g| g.as_ref());
    if deck {
        crate::copies::deck_status(now_present, |r| desc_json(descs, r, now_present))
    } else {
        crate::copies::status(now_present, |r| desc_json(descs, r, now_present))
    }
}

/// The deck shape (the main's width and height as described, else The Caveman's) for the deck-shaped set.
static DECK_W: AtomicU64 = AtomicU64::new(WIDTH as u64);
static DECK_H: AtomicU64 = AtomicU64::new(HEIGHT as u64);

/// A resource described: the deck-shaped set and the described set told (the copy tracer's and the uploads deck
/// subset's tests in the hooks), and a resource that is not deck-shaped leaves the uploads deck subset.
fn described_now(r: usize, d: &RingDesc) {
    crate::copies::note_described(r);
    if !deck_shaped(d) {
        uploads_deck_drop(r);
    }
}

fn deck_shaped(d: &RingDesc) -> bool {
    d.dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D.0
        && d.array == 1
        && d.width == DECK_W.load(Ordering::Relaxed)
        && d.height == DECK_H.load(Ordering::Relaxed) as u32
}

/// A tile array: a 256 x 256 texture array of a BC format (BC1 to BC5 70..84, BC6H and BC7 94..99).
fn tile_array(d: &RingDesc) -> bool {
    d.dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D.0
        && d.width == 256
        && d.height == 256
        && d.array > 1
        && ((70..=84).contains(&d.format) || (94..=99).contains(&d.format))
}

/// The deck-shaped set and the tile arrays made again from the cache (the last described first, at most
/// `copies::DECK_SET` each).
fn refresh_deck_set(descs: &HashMap<usize, RingDesc>) {
    let mut v: Vec<(usize, u64)> = descs.iter().filter(|(_, d)| deck_shaped(d)).map(|(r, d)| (*r, d.at)).collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v.truncate(crate::copies::DECK_SET);
    crate::copies::set_deck(&v.iter().map(|x| x.0).collect::<Vec<_>>());
    let mut t: Vec<(usize, u64)> = descs.iter().filter(|(_, d)| tile_array(d)).map(|(r, d)| (*r, d.at)).collect();
    t.sort_by_key(|x| std::cmp::Reverse(x.1));
    t.truncate(crate::copies::DECK_SET);
    crate::copies::set_tiles(&t.iter().map(|x| x.0).collect::<Vec<_>>());
}

// ---------------------------------------------------------------- the slices: the tile arrays the world samples
//
// Live (2026-10-21 night, the copy ring's statistics): hundreds of times a frame the game copies 64 x 64 textures of
// DXGI 3 (R32G32B32A32_UINT: 16 bytes a texel, one BC7 block) into slice N of 256 x 256 texture arrays (256 slices)
// of DXGI 99, 98 (BC7) and 83 (BC5): a GPU compressor writing 256 x 256 tiles into pages. The menu previews sample
// the asset's texture directly (every write showed there); the world samples tiles built from the source at equip,
// which is why only a re-equip propagates a source write. So: the slices written since the main was last made
// (`board slices`), which of them hold our art (`board slices match`: their corner blocks read back, decoded and
// compared with our art's blocks at every tile position of mips 0 to 3), and our art's tiles written into those
// slices (`board tiles`), written again whenever the game writes one of them again.

/// One slice of a tile array to read back (a match) or write (a tile).
#[derive(Clone, Debug)]
struct SliceItem {
    resource: usize,
    slice: u32,
    format: i32,
    /// The array's resting state (the uploads table's `after`, or one by hand); 0 with a note: not touched.
    state: u32,
    note: String,
}

/// A slice found to hold one of our art's tiles: at which mip and tile position, how well.
#[derive(Clone, Debug)]
struct SliceMatch {
    resource: usize,
    slice: u32,
    format: i32,
    mip: usize,
    tx: u32,
    ty: u32,
    error: f64,
    corners: usize,
    /// The last write's read-back verdict.
    verdict: String,
}

/// A job on the render thread: the corners read back (`write` false) or the tiles written and the corners read back
/// (`write` true); stage 0 to record, 1 waiting for the fence, 2 done.
#[derive(Clone, Debug)]
struct SliceJob {
    write: bool,
    stage: u8,
    items: Vec<SliceItem>,
    /// For a write: the tile bytes sent for each item (rows of `cols` blocks), with the region's size in texels.
    tiles: Vec<(Vec<u8>, usize, u32, u32)>,
    value: u64,
    started: u64,
}

static SLICES: AtomicBool = AtomicBool::new(false);

/// The tile positions of a 512 x 2048 art at mips 0 to 3: (mip, tx, ty, the region's width and height in texels).
fn tile_positions() -> Vec<(usize, u32, u32, u32, u32)> {
    let mut out = Vec::new();
    for mip in 0..4usize {
        let (w, h) = ((WIDTH >> mip).max(1), (HEIGHT >> mip).max(1));
        for ty in 0..h.div_ceil(256) {
            for tx in 0..w.div_ceil(256) {
                out.push((mip, tx, ty, (w - tx * 256).min(256), (h - ty * 256).min(256)));
            }
        }
    }
    out
}

/// Our art's block at texel (x, y) of mip `mip` (16 bytes), from the BC7 chain.
fn art_block(chain: &Chain, mip: usize, x: u32, y: u32) -> Option<[u8; 16]> {
    let (w, h) = ((chain.width >> mip).max(1), (chain.height >> mip).max(1));
    if x >= w || y >= h || mip >= chain.levels {
        return None;
    }
    let cols = w.div_ceil(4) as usize;
    let at = chain.level_offset(mip) + ((y as usize / 4) * cols + x as usize / 4) * 16;
    chain.bytes.get(at..at + 16).map(|b| b.try_into().unwrap())
}

/// The tile of our art at (mip, tx, ty): the region's blocks row by row (`cols` blocks a row), and its size.
fn art_tile(chain: &Chain, mip: usize, tx: u32, ty: u32) -> (Vec<u8>, usize, u32, u32) {
    let (w, h) = ((chain.width >> mip).max(1), (chain.height >> mip).max(1));
    let (rw, rh) = ((w - tx * 256).min(256), (h - ty * 256).min(256));
    let cols = (rw as usize).div_ceil(4);
    let rows = (rh as usize).div_ceil(4);
    let full = (w as usize).div_ceil(4);
    let mut out = Vec::with_capacity(cols * rows * 16);
    for r in 0..rows {
        let at = chain.level_offset(mip) + ((ty as usize * 64 + r) * full + tx as usize * 64) * 16;
        out.extend_from_slice(&chain.bytes[at..at + cols * 16]);
    }
    (out, cols, rw, rh)
}

/// The slices of tile arrays written since the main was last made (`board slices`): by destination array, each
/// slice with its writes and last present.
fn slices_report(b: &Board, now_present: u64) -> Value {
    let guard = RING_DESCS.lock().ok();
    let descs = guard.as_ref().and_then(|g| g.as_ref());
    let since = b.main_made_at;
    let mut arrays: Vec<(usize, RingDesc, BTreeMap<u32, (u64, u64)>)> = Vec::new();
    for c in crate::copies::deck_entries() {
        if c.present < since {
            continue;
        }
        let Some(d) = descs.and_then(|m| m.get(&c.dst)) else { continue };
        if !tile_array(d) {
            continue;
        }
        let i = match arrays.iter().position(|a| a.0 == c.dst) {
            Some(i) => i,
            None => {
                arrays.push((c.dst, *d, BTreeMap::new()));
                arrays.len() - 1
            }
        };
        let e = arrays[i].2.entry(c.dst_sub).or_insert((0, 0));
        e.0 += 1;
        e.1 = e.1.max(c.present);
    }
    json!({
        "since_present": since,
        "present": now_present,
        "arrays": arrays.iter().map(|(r, d, slices)| json!({
            "resource": format!("{r:#x}"),
            "dxgi_format": d.format,
            "array": d.array,
            "mips": d.mips,
            "state": last_write_after(*r).map(|s| format!("{s:#x}")),
            "slices": slices.iter().map(|(k, v)| json!({"slice": if *k == crate::copies::SUB_ALL { json!("all") } else { json!(k) }, "writes": v.0, "last_present": v.1, "presents_ago": now_present.saturating_sub(v.1)})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// The slices to read back for a match: BC7 slices of tile arrays written since the main was last made, each with
/// the array's resting state (by hand, else the uploads table's).
fn slice_candidates(b: &Board) -> Vec<SliceItem> {
    let guard = RING_DESCS.lock().ok();
    let descs = guard.as_ref().and_then(|g| g.as_ref());
    let mut out: Vec<SliceItem> = Vec::new();
    for c in crate::copies::deck_entries() {
        if c.present < b.main_made_at || c.dst_sub == crate::copies::SUB_ALL {
            continue;
        }
        let Some(d) = descs.and_then(|m| m.get(&c.dst)) else { continue };
        if !tile_array(d) || !(97..=99).contains(&d.format) || out.iter().any(|i| i.resource == c.dst && i.slice == c.dst_sub) {
            continue;
        }
        let (state, note) = match b.slices_state.or_else(|| last_write_after(c.dst)) {
            Some(s) if usable_state(s) => (s, String::new()),
            Some(s) => (0, format!("the array's last state {s:#x} is a write state: not touched")),
            None => (0, "the array's state is unknown (no write of it in the uploads table; board tiles --state srv by hand)".into()),
        };
        out.push(SliceItem { resource: c.dst, slice: c.dst_sub, format: d.format, state, note });
        if out.len() >= SLICE_ITEMS {
            break;
        }
    }
    out
}

/// Starts the write of our art's tiles into the matched slices (`board tiles`, and again when the game writes one).
fn start_tiles(b: &mut Board, now: u64) -> Result<(), String> {
    if b.slice_job.is_some() {
        return Err("a slice job is running: wait for it".into());
    }
    let art = b.art.clone().ok_or("no art: board deck ... (or board art, board fill) first")?;
    let chain =
        art.chains.first().filter(|c| c.format == ChainFormat::Bc7 && c.width == WIDTH && c.height == HEIGHT).ok_or("the art has no 512 x 2048 BC7 chain")?;
    if b.slice_matches.is_empty() {
        return Err("no matched slices: board slices match first".into());
    }
    let mut items = Vec::new();
    let mut tiles = Vec::new();
    for m in &b.slice_matches {
        let (state, note) = match b.slices_state.or_else(|| last_write_after(m.resource)) {
            Some(s) if usable_state(s) => (s, String::new()),
            Some(s) => (0, format!("the array's last state {s:#x} is a write state: not touched")),
            None => (0, "the array's state is unknown: board tiles --state srv".into()),
        };
        items.push(SliceItem { resource: m.resource, slice: m.slice, format: m.format, state, note });
        tiles.push(art_tile(chain, m.mip, m.tx, m.ty));
    }
    b.slice_job = Some(SliceJob { write: true, stage: 0, items, tiles, value: 0, started: now });
    SLICES.store(true, Ordering::Release);
    Ok(())
}

fn slice_fail(b: &mut Board, why: String) {
    b.slices_result = json!({"error": why});
    b.slice_job = None;
    let line = format!("slices: {why}");
    b.say_later(line);
    SLICES.store(false, Ordering::Release);
}

/// The slice jobs on the render thread (see above).
unsafe fn slices_present(device: &ID3D12Device, queue: &ID3D12CommandQueue) {
    let Ok(mut b) = cell().try_lock() else { return };
    let Ok(mut g) = GPU.try_lock() else { return };
    if g.is_none() {
        match Gpu::new(device) {
            Ok(x) => *g = Some(x),
            Err(e) => return slice_fail(&mut b, format!("cannot make our GPU objects: {e}")),
        }
    }
    let gpu = g.as_mut().unwrap();
    if gpu.device.as_raw() != device.as_raw() {
        SLICES.store(false, Ordering::Release);
        return;
    }
    let Some((stage, write)) = b.slice_job.as_ref().map(|j| (j.stage, j.write)) else {
        SLICES.store(false, Ordering::Release);
        return;
    };
    let present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    match stage {
        0 => {
            if gpu.busy() {
                return;
            }
            let job = b.slice_job.clone().unwrap();
            // the upload of the tiles, laid out 512-aligned with 256-aligned rows
            let mut layout: Vec<(u64, u32)> = Vec::new();
            let mut total = 0u64;
            if write {
                for (bytes, cols, _, rh) in &job.tiles {
                    let pitch = ((*cols * 16) as u32).div_ceil(256) * 256;
                    let rows = (*rh as u64).div_ceil(4);
                    layout.push((total, pitch));
                    let _ = bytes;
                    total += (rows * pitch as u64).div_ceil(512) * 512;
                }
                let map = match gpu.upload(total.max(512)) {
                    Ok(m) => m,
                    Err(e) => return slice_fail(&mut b, e),
                };
                for (k, (bytes, cols, _, _)) in job.tiles.iter().enumerate() {
                    let (off, pitch) = layout[k];
                    for (r, row) in bytes.chunks(cols * 16).enumerate() {
                        std::ptr::copy_nonoverlapping(row.as_ptr(), map.add(off as usize + r * pitch as usize), row.len());
                    }
                }
            }
            if gpu.allocator.Reset().is_err() || gpu.list.Reset(&gpu.allocator, None::<&ID3D12PipelineState>).is_err() {
                return slice_fail(&mut b, "our command list did not reset".into());
            }
            let upload_res = gpu.upload.as_ref().map(|u| u.0.clone());
            let mut any = false;
            let mut notes: Vec<(usize, String)> = Vec::new();
            RECORDING.with(|r| r.set(true));
            for (i, it) in job.items.iter().enumerate() {
                if it.state == 0 {
                    continue;
                }
                if dead(it.resource) {
                    notes.push((i, format!("resource {:#x}: released by the game since: not touched", it.resource)));
                    continue;
                }
                let vt = win::read_ptr(it.resource).unwrap_or(0);
                if vt == 0 || !gpu.resource_vtables.contains(&vt) {
                    notes.push((i, format!("resource {:#x}: its vtable {vt:#x} is not a D3D12 resource's: not touched", it.resource)));
                    continue;
                }
                let raw = it.resource as *mut c_void;
                let Some(res) = ID3D12Resource::from_raw_borrowed(&raw) else { continue };
                let d = res.GetDesc();
                if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
                    || d.Width != 256
                    || d.Height != 256
                    || d.DepthOrArraySize as u32 <= it.slice
                    || d.Format.0 != it.format
                {
                    notes.push((
                        i,
                        format!(
                            "resource {:#x}: GetDesc says {}x{}x{} format {}: not the tile array it was (released and reused?): not touched",
                            it.resource, d.Width, d.Height, d.DepthOrArraySize, d.Format.0
                        ),
                    ));
                    continue;
                }
                if write {
                    let (ref bytes, cols, rw, rh) = job.tiles[i];
                    let _ = (bytes, cols);
                    let (off, pitch) = layout[i];
                    let Some(up) = upload_res.as_ref() else { continue };
                    barriers(&gpu.list, &[transition(res, it.slice, it.state, COPY_DEST)]);
                    let dst = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(res),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: it.slice },
                    };
                    let from = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(up),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                                Offset: off,
                                Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: d.Format, Width: rw, Height: rh, Depth: 1, RowPitch: pitch },
                            },
                        },
                    };
                    gpu.list.CopyTextureRegion(&dst, 0, 0, 0, &from, None);
                    barriers(&gpu.list, &[transition(res, it.slice, COPY_DEST, COPY_SOURCE)]);
                } else {
                    barriers(&gpu.list, &[transition(res, it.slice, it.state, COPY_SOURCE)]);
                }
                for (k, (cx, cy)) in CORNERS.iter().enumerate() {
                    let to = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&gpu.slice_readback),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                                Offset: ((i * 4 + k) * 512) as u64,
                                Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: d.Format, Width: 4, Height: 4, Depth: 1, RowPitch: 256 },
                            },
                        },
                    };
                    let from = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(res),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: it.slice },
                    };
                    let bx = D3D12_BOX { left: *cx, top: *cy, front: 0, right: cx + 4, bottom: cy + 4, back: 1 };
                    gpu.list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&bx));
                }
                barriers(&gpu.list, &[transition(res, it.slice, COPY_SOURCE, it.state)]);
                any = true;
            }
            RECORDING.with(|r| r.set(false));
            if gpu.list.Close().is_err() {
                return slice_fail(&mut b, "our command list did not close".into());
            }
            if let Some(j) = b.slice_job.as_mut() {
                for (i, n) in notes {
                    j.items[i].note = n;
                }
            }
            if !any {
                return slice_fail(&mut b, "no slice could be touched (every one without a usable state or not a resource)".into());
            }
            let Ok(as_list) = gpu.list.cast::<ID3D12CommandList>() else { return slice_fail(&mut b, "our command list".into()) };
            queue.ExecuteCommandLists(&[Some(as_list)]);
            gpu.value += 1;
            if queue.Signal(&gpu.fence, gpu.value).is_err() {
                gpu.value -= 1;
                return slice_fail(&mut b, "the fence did not signal".into());
            }
            gpu.slice_value = gpu.value;
            if let Some(j) = b.slice_job.as_mut() {
                j.value = gpu.value;
                j.stage = 1;
            }
        }
        1 => {
            if gpu.fence.GetCompletedValue() < gpu.slice_value {
                return;
            }
            let job = b.slice_job.clone().unwrap();
            let read = |i: usize, k: usize| -> [u8; 16] {
                let mut blk = [0u8; 16];
                std::ptr::copy_nonoverlapping(gpu.slice_map.add((i * 4 + k) * 512), blk.as_mut_ptr(), 16);
                blk
            };
            if write {
                let mut lines = Vec::new();
                let mut results = Vec::new();
                let mut ok = 0;
                for (i, it) in job.items.iter().enumerate() {
                    let verdict = if it.state == 0 || !it.note.is_empty() {
                        format!("not written: {}", it.note)
                    } else {
                        let (ref bytes, cols, rw, rh) = job.tiles[i];
                        let mut bad = None;
                        for (k, (cx, cy)) in CORNERS.iter().enumerate() {
                            if *cx >= rw || *cy >= rh {
                                continue;
                            }
                            let at = ((*cy as usize / 4) * cols + *cx as usize / 4) * 16;
                            if read(i, k) != bytes[at..at + 16] {
                                bad = Some(k);
                                break;
                            }
                        }
                        match bad {
                            None => {
                                ok += 1;
                                "match".to_string()
                            }
                            Some(k) => format!("mismatch at corner {k} ({}, {})", CORNERS[k].0, CORNERS[k].1),
                        }
                    };
                    if let Some(m) = b.slice_matches.iter_mut().find(|m| m.resource == it.resource && m.slice == it.slice) {
                        m.verdict = verdict.clone();
                    }
                    results
                        .push(json!({"resource": format!("{:#x}", it.resource), "slice": it.slice, "state": format!("{:#x}", it.state), "verdict": verdict}));
                }
                b.tiles_written_at = present;
                b.tiles_keep = true;
                b.slices_result = json!({"kind": "write", "present": present, "written": ok, "items": results});
                lines.push(format!("tiles: {ok} of {} slice(s) written and read back as match (present {present})", job.items.len()));
                for l in lines {
                    b.say_later(l);
                }
            } else {
                let chain =
                    b.art.as_ref().and_then(|a| a.chains.first().cloned()).filter(|c| c.format == ChainFormat::Bc7 && c.width == WIDTH && c.height == HEIGHT);
                let Some(chain) = chain else { return slice_fail(&mut b, "no art with a 512 x 2048 BC7 chain to compare with (board deck ... first)".into()) };
                let positions = tile_positions();
                let mut matches = Vec::new();
                let mut results = Vec::new();
                for (i, it) in job.items.iter().enumerate() {
                    if it.state == 0 || !it.note.is_empty() {
                        results.push(json!({"resource": format!("{:#x}", it.resource), "slice": it.slice, "note": it.note}));
                        continue;
                    }
                    let got: Vec<[[u8; 4]; 16]> = (0..4).map(|k| crate::bc7::decode_block(&read(i, k))).collect();
                    let mut best: Option<(usize, u32, u32, f64, usize)> = None;
                    for &(mip, tx, ty, _, _) in &positions {
                        let mut sum = 0.0;
                        let mut n = 0;
                        for (k, (cx, cy)) in CORNERS.iter().enumerate() {
                            let Some(ours) = art_block(&chain, mip, tx * 256 + cx, ty * 256 + cy) else { continue };
                            sum += crate::bc7::block_error(&got[k], &crate::bc7::decode_block(&ours));
                            n += 1;
                        }
                        if n < 2 {
                            continue;
                        }
                        let err = sum / n as f64;
                        if best.is_none_or(|b| err < b.3) {
                            best = Some((mip, tx, ty, err, n));
                        }
                    }
                    let matched = best.is_some_and(|b| b.3 <= SLICE_MAX_ERROR);
                    if let Some((mip, tx, ty, err, n)) = best.filter(|_| matched) {
                        matches.push(SliceMatch {
                            resource: it.resource,
                            slice: it.slice,
                            format: it.format,
                            mip,
                            tx,
                            ty,
                            error: err,
                            corners: n,
                            verdict: String::new(),
                        });
                    }
                    results.push(json!({
                        "resource": format!("{:#x}", it.resource),
                        "slice": it.slice,
                        "matched": matched,
                        "best": best.map(|(mip, tx, ty, err, n)| json!({"mip": mip, "tx": tx, "ty": ty, "error": (err * 100.0).round() / 100.0, "corners": n})),
                        "corner_blocks": (0..4).map(|k| read(i, k).iter().map(|x| format!("{x:02x}")).collect::<String>()).collect::<Vec<_>>(),
                    }));
                }
                let n = matches.len();
                b.slice_matches = matches;
                b.slices_result = json!({"kind": "match", "present": present, "matched": n, "max_error": SLICE_MAX_ERROR, "items": results});
                let line = format!(
                    "slices: {n} of {} slice(s) read back hold our art's tiles ({})",
                    job.items.len(),
                    b.slice_matches
                        .iter()
                        .map(|m| format!("{:#x}[{}] = mip {} tile ({}, {}) err {:.1}", m.resource, m.slice, m.mip, m.tx, m.ty, m.error))
                        .collect::<Vec<_>>()
                        .join("; ")
                );
                b.say_later(line);
            }
            b.slice_job = None;
            SLICES.store(false, Ordering::Release);
        }
        _ => {
            b.slice_job = None;
            SLICES.store(false, Ordering::Release);
        }
    }
}

// ---------------------------------------------------------------- the colour hunt
//
// Trevor's idea (2026-10-21 night): find the texture the ridden board samples by CONTENT. The source holds a solid
// unique colour (`board fill 3CF08A`), a real re-equip puts it on the ridden board and so into whatever texture the
// board samples; `board hunt 3CF08A` reads three blocks of mip 0 (the centre, a quarter and three quarters along the
// longer axis) of every 2D texture the DLL knows (the describe cache: every resource a hook saw, with its shape),
// decodes them (BC7, BC1, BC3, RGBA8, BGRA8) and reports every texture where a texel is within the tolerance of the
// colour. Read back on the render thread in batches of `HUNT_BATCH` items a present, each from the state the
// uploads table or the target rule knows (else listed as unknown).

/// One texture (or one slice of an array) the hunt reads.
#[derive(Clone, Debug)]
struct HuntItem {
    resource: usize,
    slice: u16,
    desc: RingDesc,
    /// The state to read it from (none: unknown, not read unless assumed).
    state: Option<u32>,
    /// Read under the assumed state (`--assume`): its real state unknown.
    assumed: bool,
    /// The three blocks' texel positions.
    at: [(u32, u32); 3],
    /// Decoded: which blocks hit, which encodings of the colour hit (sRGB as given, linearised, and both
    /// premultiplied by the texel's alpha), the mean colour over the 48 texels, a note.
    hits: [bool; 3],
    /// Blocks where only a premultiplied encoding hit (a weaker sign: a texel with alpha 128 or more whose colour
    /// times its alpha is near the colour; random data hits it now and then).
    maybe: [bool; 3],
    encodings: [bool; 4],
    mean: [u8; 3],
    note: String,
    read: bool,
    /// A census item: the runtime texture object owning the resource (checked again right before the read), its
    /// name hash and its RenderCore2 bind flags (0: an item of the describe cache).
    owner: usize,
    hash: u64,
    bind: u32,
}

const ENCODINGS: [&str; 4] = ["srgb", "linear", "srgb_premultiplied", "linear_premultiplied"];

/// An sRGB byte linearised (the value a UNORM texture holds when the game stores linear light).
pub fn srgb_to_linear(c: u8) -> u8 {
    let x = c as f64 / 255.0;
    let l = if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
    (l * 255.0).round() as u8
}

/// The tolerance per channel for a format when none was given: 24 for BC7 and 4-byte texels, 32 for BC1 and BC3.
fn hunt_tolerance(given: Option<u8>, format: i32) -> i32 {
    given.map(|t| t as i32).unwrap_or(if (70..=78).contains(&format) { 32 } else { 24 })
}

/// The hunt: the colour and tolerance, the items, how far the batches got, the result.
#[derive(Clone, Debug)]
struct Hunt {
    rgb: [u8; 3],
    tolerance: Option<u8>,
    assume: Option<u32>,
    items: Vec<HuntItem>,
    /// The first item of the batch recorded (stage 1) or to record (stage 0); done when past the end.
    next: usize,
    batch: usize,
    stage: u8,
    started: u64,
    done_ms: u64,
    unknown: usize,
    /// When the job last moved (ms), the fence value of the batch in flight, and why it stopped early.
    progress_ms: u64,
    fence_value: u64,
    stuck: Option<String>,
    /// The items came from the census (every runtime texture object), not the describe cache.
    census: bool,
    /// References of ours on the resources of the batch in flight (taken as each is recorded, released once the
    /// fence passed: the game cannot free one under our copy).
    held: Vec<ID3D12Resource>,
}

static HUNTING: AtomicBool = AtomicBool::new(false);
/// Presents at which the hunt could not take the board's lock (the worker held it).
static HUNT_LOCK_MISSES: AtomicU64 = AtomicU64::new(0);
/// A batch that does not move for this long ends the hunt with what was read.
const HUNT_STUCK_MS: u64 = 10_000;

/// Whether a format is one the hunt decodes: BC7, BC1, BC3, RGBA8, BGRA8.
fn hunt_format(f: i32) -> bool {
    matches!(f, 97..=99 | 70..=72 | 76..=78 | 27..=29 | 87 | 88 | 91)
}

/// The items of a hunt: every described 2D texture of a format the hunt decodes, at least 64 x 64, seen since the
/// main was last made (`everything`: every one described, at the risk of one the game released since), each slice
/// (up to 64) of an array; the state from the uploads table, else the target rule.
fn hunt_items(b: &Board, everything: bool, assume: Option<u32>, _now_present: u64) -> Vec<HuntItem> {
    let guard = RING_DESCS.lock().ok();
    let Some(descs) = guard.as_ref().and_then(|g| g.as_ref()) else { return Vec::new() };
    let mut out = Vec::new();
    for (&r, d) in descs.iter() {
        if dead(r) || d.dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D.0 || d.width < 64 || d.height < 64 || !hunt_format(d.format) {
            continue;
        }
        if !everything && d.at < b.main_made_at {
            continue;
        }
        let target = b.targets.iter().find(|f| f.t.resource == r);
        // (a resource resting in COMMON/PRESENT is a swap chain's back buffer or the like: reading one between the
        // host's frames lost the host test its device; never read)
        let (mut state, mut note) = match target {
            Some(f) => (decide_target(b, f, f.dxgi_format, f.described).map(|(s, _)| s[0]).ok(), String::new()),
            None => match last_write_after(r) {
                Some(0) => (None, "last seen in COMMON/PRESENT (a back buffer?): not read".to_string()),
                Some(st) if usable_state(st) => (Some(st), String::new()),
                Some(st) => (None, format!("last seen in a write state {st:#x}: not read")),
                None => (None, String::new()),
            },
        };
        // `--assume`: a texture of unknown state is read from the assumed one, unless it is a render or depth target
        // (a wrong assumed state on a read gives garbage bytes, not a device loss; a back buffer read between the
        // host's frames did lose the device)
        let mut assumed = false;
        if state.is_none() && note.is_empty() {
            match assume {
                Some(_) if d.flags & 0x3 != 0 => note = "a render or depth target (its flags): not read under an assumption".into(),
                Some(a) => {
                    state = Some(a);
                    assumed = true;
                }
                None => {}
            }
        }
        let (w, h) = (d.width as u32, d.height);
        let at = if h >= w { [(w / 2, h / 2), (w / 2, h / 4), (w / 2, h * 3 / 4)] } else { [(w / 2, h / 2), (w / 4, h / 2), (w * 3 / 4, h / 2)] };
        let at = at.map(|(x, y)| ((x & !3).min(w - 4), (y & !3).min(h - 4)));
        for slice in 0..d.array.min(HUNT_SLICES) {
            out.push(HuntItem {
                resource: r,
                slice,
                desc: *d,
                state,
                assumed,
                at,
                hits: [false; 3],
                maybe: [false; 3],
                encodings: [false; 4],
                mean: [0; 3],
                note: note.clone(),
                read: false,
                owner: 0,
                hash: 0,
                bind: 0,
            });
        }
    }
    out.sort_by(|a, b| b.desc.at.cmp(&a.desc.at).then(a.resource.cmp(&b.resource)).then(a.slice.cmp(&b.slice)));
    out
}

/// The DXGI format of a Frostbite format code when the hunt decodes it (RGBA8, BGRA8, BC1, BC3, BC7; GetDesc has
/// the last word when the item is read).
fn frostbite_dxgi(f: u32) -> Option<i32> {
    Some(match f {
        18 => 28,
        20 => 29,
        23 => 87,
        24 => 91,
        54 => 71,
        55 => 72,
        60 => 77,
        61 => 78,
        66 => 98,
        67 => 99,
        _ => return None,
    })
}

/// A runtime texture object read for the census (the fields skwatch reads too): its shape as a description, its
/// resource, name hash and RenderCore2 bind flags; None when it is not a plain 2D texture of a decoded format with a
/// resource.
fn read_census(texture: usize, ids: &Ids, now: u64) -> Option<(RingDesc, usize, u64, u32)> {
    if texture == 0 || !texture.is_multiple_of(8) || texture < win::LOWEST || win::read_ptr(texture)? != ids.texture_vtable {
        return None;
    }
    let u = |o: usize| win::read_u32(texture + o);
    let (fmt, w, h, depth, array, mips) =
        (u(layout::TEX_FORMAT)?, u(layout::TEX_WIDTH)?, u(layout::TEX_HEIGHT)?, u(layout::TEX_DEPTH)?, u(layout::TEX_ARRAY)?, u(layout::TEX_MIPS)?);
    let hash = win::read_u64(texture + layout::TEX_NAME_HASH)?;
    let res = win::read_ptr(texture + layout::TEX_D3D12)?;
    if res == 0
        || res % 8 != 0
        || res < win::LOWEST
        || depth != 1
        || !(64..=16384).contains(&w)
        || !(64..=16384).contains(&h)
        || mips == 0
        || mips > 16
        || array == 0
        || array > 4096
    {
        return None;
    }
    let dxgi = frostbite_dxgi(fmt)?;
    let (_, bind, _) = read_rc2(texture, res, ids);
    let flags = if bind & 0x80 != 0 { 1 } else { 0 };
    Some((
        RingDesc {
            at: now,
            dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D.0,
            width: w as u64,
            height: h,
            mips: mips as u16,
            array: array as u16,
            format: dxgi,
            flags,
        },
        res,
        hash,
        bind,
    ))
}

/// The items of a census hunt: every runtime texture object the scan found whose resource is a plain 2D texture of
/// a decoded format (never screen-sized: a back buffer or the G-buffer), each slice of an array up to the cap; the
/// state from the uploads table or the target rule, else the assumed one (`targets`: for render targets too).
fn census_items(b: &Board, hits: &[usize], ids: &Ids, assume: Option<u32>, targets: bool, now: u64) -> Vec<HuntItem> {
    let mut out = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for &texture in hits {
        let Some((d, r, hash, bind)) = read_census(texture, ids, now) else { continue };
        if !seen.insert(r) {
            continue;
        }
        if d.width >= 1920 && d.height >= 1080 {
            continue;
        }
        let target = b.targets.iter().find(|f| f.t.resource == r);
        let (mut state, mut note) = match target {
            Some(f) => (decide_target(b, f, f.dxgi_format, f.described).map(|(s, _)| s[0]).ok(), String::new()),
            None => match last_write_after(r) {
                Some(0) => (None, "last seen in COMMON/PRESENT (a back buffer?): not read".to_string()),
                Some(st) if usable_state(st) => (Some(st), String::new()),
                Some(st) => (None, format!("last seen in a write state {st:#x}: not read")),
                None => (None, String::new()),
            },
        };
        let mut assumed = false;
        if state.is_none() && note.is_empty() {
            match assume {
                Some(_) if d.flags & 0x3 != 0 && !targets => {
                    note = "a render target (its bind flags): not read under an assumption (--include-targets reads it)".into()
                }
                Some(a) => {
                    state = Some(a);
                    assumed = true;
                }
                None => {}
            }
        }
        let (w, h) = (d.width as u32, d.height);
        let at = if h >= w { [(w / 2, h / 2), (w / 2, h / 4), (w / 2, h * 3 / 4)] } else { [(w / 2, h / 2), (w / 4, h / 2), (w * 3 / 4, h / 2)] };
        let at = at.map(|(x, y)| ((x & !3).min(w - 4), (y & !3).min(h - 4)));
        for slice in 0..d.array.min(HUNT_SLICES) {
            out.push(HuntItem {
                resource: r,
                slice,
                desc: d,
                state,
                assumed,
                at,
                hits: [false; 3],
                maybe: [false; 3],
                encodings: [false; 4],
                mean: [0; 3],
                note: note.clone(),
                read: false,
                owner: texture,
                hash,
                bind,
            });
        }
    }
    // the deck-shaped ones first (the likely needles), then by address
    out.sort_by(|a, b| deck_shaped(&b.desc).cmp(&deck_shaped(&a.desc)).then(a.resource.cmp(&b.resource)).then(a.slice.cmp(&b.slice)));
    out
}

/// The census scan: stepped in the ticks; when it is done, the hunt is made from what it found.
fn census_tick(b: &mut Board, now: u64, ids: &Ids) {
    let Some((req, assume)) = b.census_req.clone() else { return };
    let done = match b.census_scan.as_mut() {
        Some(s) => s.step(SCAN_BUDGET, SCAN_STEP_MS),
        None => {
            b.census_req = None;
            return;
        }
    };
    if !done {
        return;
    }
    let s = b.census_scan.take().unwrap();
    b.census_req = None;
    let present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    let items = census_items(b, &s.found, ids, assume, req.targets, present);
    let deckish = items.iter().filter(|i| deck_shaped(&i.desc)).count();
    b.say(format!(
        "census: {} MB in {} ms, {} runtime texture object(s), {} texture slice(s) of a decoded 2D shape to read ({deckish} deck-shaped)",
        s.bytes >> 20,
        now.saturating_sub(b.census_started),
        s.found.len(),
        items.len()
    ));
    if items.is_empty() {
        b.say("hunt: nothing to read from the census".into());
        return;
    }
    let line = format!(
        "hunt: #{:02x}{:02x}{:02x} (linear {:?}) within {} over {} texture slice(s) (the census{})",
        req.rgb[0],
        req.rgb[1],
        req.rgb[2],
        req.rgb.map(srgb_to_linear),
        req.tolerance.map(|t| t.to_string()).unwrap_or_else(|| "24 (32 for BC1/BC3)".into()),
        items.len(),
        if req.targets { ", render targets included" } else { "" }
    );
    b.say(line);
    b.hunt = Some(Hunt {
        rgb: req.rgb,
        tolerance: req.tolerance,
        assume,
        items,
        next: 0,
        batch: 0,
        stage: 0,
        started: now,
        done_ms: 0,
        unknown: 0,
        progress_ms: win::now_ms(),
        fence_value: 0,
        stuck: None,
        census: true,
        held: Vec::new(),
    });
    HUNTING.store(true, Ordering::Release);
}

/// A block's bytes decoded by the format: 16 texels RGBA (BGRA swapped to RGBA).
fn decode_hunt_block(format: i32, bytes: &[u8]) -> [[u8; 4]; 16] {
    match format {
        97..=99 => crate::bc7::decode_block(bytes[..16].try_into().unwrap()),
        70..=72 => crate::bc7::decode_bc1(bytes[..8].try_into().unwrap()),
        76..=78 => crate::bc7::decode_bc3(bytes[..16].try_into().unwrap()),
        87 | 88 | 91 => std::array::from_fn(|t| [bytes[t * 4 + 2], bytes[t * 4 + 1], bytes[t * 4], bytes[t * 4 + 3]]),
        _ => std::array::from_fn(|t| [bytes[t * 4], bytes[t * 4 + 1], bytes[t * 4 + 2], bytes[t * 4 + 3]]),
    }
}

/// The hunt on the render thread: a batch recorded (stage 0), read once the fence passed (stage 1), until the items
/// are done.
unsafe fn hunt_present(device: &ID3D12Device, queue: &ID3D12CommandQueue) {
    let Ok(mut b) = cell().try_lock() else {
        HUNT_LOCK_MISSES.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let Ok(mut g) = GPU.try_lock() else {
        HUNT_LOCK_MISSES.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if g.is_none() {
        match Gpu::new(device) {
            Ok(x) => *g = Some(x),
            Err(e) => {
                b.say_later(format!("hunt: cannot make our GPU objects: {e}"));
                b.hunt = None;
                HUNTING.store(false, Ordering::Release);
                return;
            }
        }
    }
    let gpu = g.as_mut().unwrap();
    if gpu.device.as_raw() != device.as_raw() {
        HUNTING.store(false, Ordering::Release);
        return;
    }
    let Some((stage, next, n)) = b.hunt.as_ref().map(|h| (h.stage, h.next, h.items.len())) else {
        HUNTING.store(false, Ordering::Release);
        return;
    };
    let now_ms = win::now_ms();
    // a batch that does not move for 10 s (the GPU objects busy for good, a fence never completing) ends the hunt
    // with what was read, and says why
    let stuck_since = b.hunt.as_ref().map_or(0, |h| now_ms.saturating_sub(h.progress_ms));
    if stage < 2 && stuck_since > HUNT_STUCK_MS {
        let (completed, value, busy) = (gpu.fence.GetCompletedValue(), gpu.value, gpu.busy());
        let h = b.hunt.as_mut().unwrap();
        let why = format!("stuck for {stuck_since} ms at stage {stage}, item {next} of {n} (fence completed {completed} of {value}, batch fence {}, busy {busy}): ended with what was read", h.fence_value);
        h.stuck = Some(why.clone());
        h.stage = 2;
        h.done_ms = now_ms;
        // (the batch may still be on the GPU: the references we hold on its resources are never released)
        for r in h.held.drain(..) {
            std::mem::forget(r);
        }
        HUNTING.store(false, Ordering::Release);
        b.say_later(format!("hunt: {why}"));
        return;
    }
    if stage == 0 {
        if next >= n {
            let h = b.hunt.as_mut().unwrap();
            h.stage = 2;
            h.done_ms = win::now_ms();
            let hits = h.items.iter().filter(|i| i.hits.iter().any(|&x| x)).count();
            let read = h.items.iter().filter(|i| i.read).count();
            let assumed = h.items.iter().filter(|i| i.read && i.assumed).count();
            let line = format!("hunt: #{:02x}{:02x}{:02x} within {} found in {hits} of {read} texture slice(s) read ({assumed} under the assumed state, {} unknown state not read, {} items, {} ms)", h.rgb[0], h.rgb[1], h.rgb[2], h.tolerance.map(|t| t.to_string()).unwrap_or_else(|| "24 (32 for BC1/BC3)".into()), h.unknown, n, win::now_ms().saturating_sub(h.started));
            b.say_later(line);
            HUNTING.store(false, Ordering::Release);
            return;
        }
        if gpu.busy() || gpu.allocator.Reset().is_err() || gpu.list.Reset(&gpu.allocator, None::<&ID3D12PipelineState>).is_err() {
            return;
        }
        if let Some(h) = b.hunt.as_mut() {
            h.progress_ms = now_ms;
        }
        let end = (next + HUNT_BATCH).min(n);
        let mut any = false;
        let texture_vtable = b.ids.as_ref().map_or(0, |i| i.texture_vtable);
        RECORDING.with(|r| r.set(true));
        let h = b.hunt.as_mut().unwrap();
        for (k, it) in h.items[next..end].iter_mut().enumerate() {
            let Some(state) = it.state else {
                if it.note.is_empty() {
                    it.note = "state unknown: not read".into();
                }
                h.unknown += 1;
                continue;
            };
            // (a census item's owner, checked right below, is the authority: a dead mark on its address may be an
            // earlier resource's that the new one reuses)
            if it.owner == 0 && dead(it.resource) {
                it.note = "released by the game since: not read".into();
                continue;
            }
            // a census item: its owner (the runtime texture object) must still be there and still name the resource
            if it.owner != 0
                && (texture_vtable == 0 || win::read_ptr(it.owner) != Some(texture_vtable) || win::read_ptr(it.owner + layout::TEX_D3D12) != Some(it.resource))
            {
                it.note = "its runtime texture no longer names it (released?): not read".into();
                continue;
            }
            let vt = win::read_ptr(it.resource).unwrap_or(0);
            if vt == 0 || !gpu.resource_vtables.contains(&vt) {
                it.note = "not a D3D12 resource now (released?)".into();
                continue;
            }
            let raw = it.resource as *mut c_void;
            let Some(res) = ID3D12Resource::from_raw_borrowed(&raw) else { continue };
            let d = res.GetDesc();
            if it.owner != 0 {
                // the census guessed the DXGI format from the Frostbite code: GetDesc has the last word
                if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
                    || d.Width != it.desc.width
                    || d.Height != it.desc.height
                    || d.DepthOrArraySize != it.desc.array
                {
                    it.note =
                        format!("GetDesc differs from the runtime texture ({}x{}x{} format {}): not read", d.Width, d.Height, d.DepthOrArraySize, d.Format.0);
                    continue;
                }
                if !hunt_format(d.Format.0) {
                    it.note = format!("DXGI format {} (GetDesc) is not one the hunt decodes: not read", d.Format.0);
                    continue;
                }
                it.desc.format = d.Format.0;
                it.desc.mips = d.MipLevels;
                it.desc.flags = d.Flags.0;
            } else if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
                || d.Width != it.desc.width
                || d.Height != it.desc.height
                || d.Format.0 != it.desc.format
                || d.DepthOrArraySize != it.desc.array
            {
                it.note = format!(
                    "GetDesc changed since it was described ({}x{}x{} format {}): released and reused? not read",
                    d.Width, d.Height, d.DepthOrArraySize, d.Format.0
                );
                continue;
            }
            // alive now: watched from here on, and held by us until the GPU is done with this batch
            watch_liveness(res, it.resource);
            h.held.push(res.clone());
            let sub = it.slice as u32 * d.MipLevels as u32;
            // (a texture resting in COPY_SOURCE needs no barrier; one from COPY_SOURCE to COPY_SOURCE is invalid and
            // the list then refuses to close: the hunt stalled at its first batch on the transcode scratch)
            let barrier = state != COPY_SOURCE;
            if barrier {
                barriers(&gpu.list, &[transition(res, sub, state, COPY_SOURCE)]);
            }
            for (j, (x, y)) in it.at.iter().enumerate() {
                let to = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: std::mem::transmute_copy(&gpu.hunt_readback),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                            Offset: ((k * 3 + j) * HUNT_STRIDE) as u64,
                            Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: d.Format, Width: 4, Height: 4, Depth: 1, RowPitch: 256 },
                        },
                    },
                };
                let from = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: std::mem::transmute_copy(res),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: sub },
                };
                let bx = D3D12_BOX { left: *x, top: *y, front: 0, right: x + 4, bottom: y + 4, back: 1 };
                gpu.list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&bx));
            }
            if barrier {
                barriers(&gpu.list, &[transition(res, sub, COPY_SOURCE, state)]);
            }
            it.read = true;
            any = true;
        }
        RECORDING.with(|r| r.set(false));
        if gpu.list.Close().is_err() {
            // an invalid command in the batch: nothing was submitted; the batch is skipped, not the hunt
            let n_batch = h.items[next..end].iter().filter(|i| i.read).count();
            for it in h.items[next..end].iter_mut() {
                if it.read {
                    it.read = false;
                    it.note = "the batch's command list did not close (an invalid command in it): not read".into();
                }
            }
            h.held.clear();
            h.batch = end;
            h.next = end;
            h.progress_ms = now_ms;
            b.say_later(format!("hunt: the list of batch {next}..{end} did not close ({n_batch} item(s) skipped)"));
            return;
        }
        h.batch = end;
        if !any {
            h.next = end;
            return;
        }
        let Ok(as_list) = gpu.list.cast::<ID3D12CommandList>() else { return };
        queue.ExecuteCommandLists(&[Some(as_list)]);
        gpu.value += 1;
        if queue.Signal(&gpu.fence, gpu.value).is_err() {
            gpu.value -= 1;
            return;
        }
        gpu.hunt_value = gpu.value;
        h.fence_value = gpu.value;
        h.stage = 1;
    } else if stage == 1 {
        if gpu.fence.GetCompletedValue() < gpu.hunt_value {
            return;
        }
        let h = b.hunt.as_mut().unwrap();
        h.progress_ms = now_ms;
        let rgb = h.rgb;
        let lin = rgb.map(srgb_to_linear);
        let end = h.batch;
        for (k, it) in h.items[next..end].iter_mut().enumerate() {
            if !it.read {
                continue;
            }
            let tol = hunt_tolerance(h.tolerance, it.desc.format);
            let mut sum = [0u32; 3];
            for j in 0..3 {
                let mut bytes = [0u8; 64];
                std::ptr::copy_nonoverlapping(gpu.hunt_map.add((k * 3 + j) * HUNT_STRIDE), bytes.as_mut_ptr(), 16);
                // a 4-byte-texel block is 4 rows of 16 bytes, 256 apart
                if !(70..=99).contains(&it.desc.format) {
                    for row in 0..4 {
                        std::ptr::copy_nonoverlapping(gpu.hunt_map.add((k * 3 + j) * HUNT_STRIDE + row * 256), bytes[row * 16..].as_mut_ptr(), 16);
                    }
                }
                let texels = decode_hunt_block(it.desc.format, &bytes);
                for px in &texels {
                    for ch in 0..3 {
                        sum[ch] += px[ch] as u32;
                    }
                }
                // the colour as given (sRGB bytes), linearised, and both premultiplied by the texel's alpha
                for px in &texels {
                    let a = px[3] as i32;
                    let near = |want: [u8; 3], premul: bool| {
                        (0..3).all(|ch| {
                            let w = if premul { (want[ch] as i32 * a + 127) / 255 } else { want[ch] as i32 };
                            (px[ch] as i32 - w).abs() <= tol
                        })
                    };
                    let hit = [near(rgb, false), near(lin, false), (128..255).contains(&a) && near(rgb, true), (128..255).contains(&a) && near(lin, true)];
                    for e in 0..4 {
                        if hit[e] {
                            it.encodings[e] = true;
                            if e < 2 {
                                it.hits[j] = true;
                            } else {
                                it.maybe[j] = true;
                            }
                        }
                    }
                }
            }
            it.mean = [(sum[0] / 48) as u8, (sum[1] / 48) as u8, (sum[2] / 48) as u8];
        }
        // the GPU is done with the batch: our references go
        h.held.clear();
        h.next = end;
        h.stage = 0;
    }
}

/// The hunt for the status: the hits (and the deck-shaped misses), the progress.
fn hunt_status(b: &Board, now_present: u64) -> Value {
    let Some(h) = b.hunt.as_ref() else { return Value::Null };
    let item = |it: &HuntItem| {
        json!({
            "resource": format!("{:#x}", it.resource),
            "slice": it.slice,
            "width": it.desc.width,
            "height": it.desc.height,
            "dxgi_format": it.desc.format,
            "mips": it.desc.mips,
            "array": it.desc.array,
            "flags": it.desc.flags,
            "described_presents_ago": now_present.saturating_sub(it.desc.at),
            "state": it.state.map(|st| format!("{st:#x}")),
            "assumed": it.assumed,
            "blocks_at": it.at.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>(),
            "hits": it.hits,
            "maybe": it.maybe,
            "encodings": ENCODINGS.iter().zip(it.encodings).filter(|(_, hit)| *hit).map(|(n, _)| *n).collect::<Vec<_>>(),
            "tolerance": hunt_tolerance(h.tolerance, it.desc.format),
            "mean": it.mean,
            "note": it.note,
            "target": b.targets.iter().find(|f| f.t.resource == it.resource).map(|f| f.t.kind()),
            "released": dead(it.resource),
            "deck_shaped": deck_shaped(&it.desc),
            "owner": if it.owner != 0 { json!({"kind": "runtime texture", "address": format!("{:#x}", it.owner), "name_hash": format!("{:#x}", it.hash), "bind": it.bind}) } else { owner_json(it.resource) },
        })
    };
    let (watched, dead_n) = liveness_counts();
    json!({
        "rgb": h.rgb,
        "linear": h.rgb.map(srgb_to_linear),
        "tolerance": h.tolerance,
        "assume": h.assume.map(|a| format!("{a:#x}")),
        "census": h.census,
        "liveness": {"watched": watched, "dead": dead_n, "callbacks": DEAD_SEEN.load(Ordering::Relaxed), "failed": LIVENESS_FAILED.load(Ordering::Relaxed)},
        "done": h.stage == 2,
        "stage": h.stage,
        "next": h.next,
        "batch_end": h.batch,
        "fence_value": h.fence_value,
        "since_progress_ms": if h.stage == 2 { 0 } else { win::now_ms().saturating_sub(h.progress_ms) },
        "stuck": h.stuck,
        "lock_misses": HUNT_LOCK_MISSES.load(Ordering::Relaxed),
        "items": h.items.len(),
        "read": h.items.iter().filter(|i| i.read).count(),
        "assumed_read": h.items.iter().filter(|i| i.read && i.assumed).count(),
        "progress": h.next,
        "unknown_state": h.unknown,
        "ms": if h.stage == 2 { h.done_ms.saturating_sub(h.started) } else { win::now_ms().saturating_sub(h.started) },
        "hits": h.items.iter().filter(|i| i.hits.iter().any(|&x| x)).map(item).collect::<Vec<_>>(),
        "maybe": h.items.iter().filter(|i| !i.hits.iter().any(|&x| x) && i.maybe.iter().any(|&x| x)).map(item).collect::<Vec<_>>(),
        "deck_misses": h.items.iter().filter(|i| deck_shaped(&i.desc) && !i.hits.iter().any(|&x| x) && !i.maybe.iter().any(|&x| x)).map(item).collect::<Vec<_>>(),
    })
}

/// The tiles kept: the game wrote one of our slices again (the tracer saw it after our last write): written again.
fn tiles_keep_tick(b: &mut Board, now: u64) {
    if !b.tiles_keep || b.slice_job.is_some() || b.slice_matches.is_empty() {
        return;
    }
    let mut again: Vec<(usize, u32)> = Vec::new();
    for c in crate::copies::deck_entries() {
        if c.present <= b.tiles_written_at {
            break;
        }
        if b.slice_matches.iter().any(|m| m.resource == c.dst && (c.dst_sub == m.slice || c.dst_sub == crate::copies::SUB_ALL))
            && !again.contains(&(c.dst, c.dst_sub))
        {
            again.push((c.dst, c.dst_sub));
        }
    }
    if again.is_empty() {
        return;
    }
    b.tiles_rewrites += 1;
    b.say(format!(
        "tiles: the game wrote {} of our slice(s) again ({}): written again",
        again.len(),
        again.iter().map(|(r, s)| format!("{r:#x}[{s}]")).collect::<Vec<_>>().join(", ")
    ));
    if let Err(e) = start_tiles(b, now) {
        b.say(format!("tiles: not written again: {e}"));
    }
}

/// GetDesc of the resources the hooks saw since the last present (the queue), and, when asked (`board uploads`,
/// `board copies`), of the table's and the ring's recent resources not yet described: once per resource, only an
/// object whose vtable is a D3D12 resource's.
unsafe fn describe_ring(device: &ID3D12Device) {
    let Ok(mut g) = GPU.try_lock() else { return };
    if g.is_none() {
        match Gpu::new(device) {
            Ok(x) => *g = Some(x),
            Err(_) => {
                DESCRIBE_RING.store(false, Ordering::Release);
                return;
            }
        }
    }
    let gpu = g.as_ref().unwrap();
    let now = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    let Ok(mut guard) = RING_DESCS.lock() else { return };
    let descs = guard.get_or_insert_with(HashMap::new);
    drain_wanted(Some(descs), now);
    if DESCRIBE_RING.swap(false, Ordering::AcqRel) {
        let wanted: Vec<usize> = upload_entries()
            .iter()
            .chain(upload_deck_entries().iter())
            .filter(|u| now.saturating_sub(u.last) <= RING_DESCRIBE_PRESENTS)
            .map(|u| u.resource)
            .chain(crate::copies::recent_resources(now, RING_DESCRIBE_PRESENTS))
            .collect();
        let mut any = false;
        for r in wanted {
            if !descs.contains_key(&r) {
                any |= describe_one(gpu, descs, r, now);
            }
        }
        if any {
            refresh_deck_set(descs);
        }
    }
    if descs.len() > 4096 {
        let mut by_age: Vec<(usize, u64)> = descs.iter().map(|(k, d)| (*k, d.at)).collect();
        by_age.sort_by_key(|x| x.1);
        for (k, _) in by_age.iter().take(2048) {
            descs.remove(k);
        }
    }
}

/// GetDesc of one resource into the cache, with no reference of ours on it (asked by hand, for an entry older than
/// the queue reached): only an object whose vtable is a D3D12 resource's (false otherwise).
unsafe fn describe_one(gpu: &Gpu, descs: &mut HashMap<usize, RingDesc>, r: usize, now: u64) -> bool {
    if dead(r) {
        return false;
    }
    // (read through ReadProcessMemory: never a fault)
    let vt = win::read_ptr(r).unwrap_or(0);
    if vt == 0 || !gpu.resource_vtables.contains(&vt) {
        return false;
    }
    let raw = r as *mut c_void;
    let Some(res) = ID3D12Resource::from_raw_borrowed(&raw) else { return false };
    let d = res.GetDesc();
    watch_liveness(res, r);
    let rd = RingDesc {
        at: now,
        dimension: d.Dimension.0,
        width: d.Width,
        height: d.Height,
        mips: d.MipLevels,
        array: d.DepthOrArraySize,
        format: d.Format.0,
        flags: d.Flags.0,
    };
    described_now(r, &rd);
    descs.insert(r, rd);
    true
}
/// Transitions out of COPY_DEST lately, of any resource: (resource, subresource << 32 | state after), a ring.
static END_RES: [AtomicUsize; ENDS] = [ZERO; ENDS];
static END_STATE: [AtomicU64; ENDS] = [ZERO64; ENDS];
static END_NEXT: AtomicUsize = AtomicUsize::new(0);
static BARRIERS_SEEN: AtomicU64 = AtomicU64::new(0);

// Recording our own upload (this thread): our own barriers are not the game's.
thread_local! {
    static RECORDING: Cell<bool> = const { Cell::new(false) };
}

pub fn recording() -> bool {
    RECORDING.try_with(|r| r.get()).unwrap_or(true)
}

fn note(states: &mut [Option<u32>; LEVELS], sub: u32, after: u32) {
    if sub == ALL_SUBRESOURCES {
        *states = [Some(after); LEVELS];
    } else if (sub as usize) < LEVELS {
        states[sub as usize] = Some(after);
    }
}

fn note_slot(k: usize, sub: u32, after: u32) {
    let v = KNOWN | after as u64;
    if sub == ALL_SUBRESOURCES {
        for s in &SEEN_STATES[k] {
            s.store(v, Ordering::Release);
        }
    } else if (sub as usize) < LEVELS {
        SEEN_STATES[k][sub as usize].store(v, Ordering::Release);
    }
    SEEN_COUNT[k].fetch_add(1, Ordering::AcqRel);
}

/// The game's legacy barriers, from the depth module's `ResourceBarrier` hook (after the game's own call):
/// transitions on a watched resource, and upload ends of any. One atomic load when the board is not live; otherwise
/// a few compares per barrier and, on a match, a few atomic stores.
pub fn observe(barriers: &[D3D12_RESOURCE_BARRIER]) {
    if !ACTIVE.load(Ordering::Relaxed) || recording() {
        return;
    }
    let w: [usize; WATCHED] = std::array::from_fn(|k| WATCH[k].load(Ordering::Acquire));
    for b in barriers {
        if b.Type != D3D12_RESOURCE_BARRIER_TYPE_TRANSITION || (b.Flags & D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY).0 != 0 {
            continue;
        }
        // SAFETY: a transition barrier's union member, as its Type says.
        let t = unsafe { &b.Anonymous.Transition };
        let res: usize = unsafe { std::mem::transmute_copy(&t.pResource) };
        let (before, after) = (t.StateBefore.0 as u32, t.StateAfter.0 as u32);
        if res == 0 {
            continue;
        }
        let sub_text = || if t.Subresource == ALL_SUBRESOURCES { "all".to_string() } else { t.Subresource.to_string() };
        // the identity test (see IDENTITY_UNTIL_MS): a write ending on any 512 x 2048 RGBA8 texture
        const WRITES: u32 = COPY_DEST | 0x4 | 0x8;
        if before & WRITES != 0 && after & WRITES == 0 && win::now_ms() < IDENTITY_UNTIL_MS.load(Ordering::Relaxed) && nameless_shape(res) {
            IDENTITY_LINES.fetch_add(1, Ordering::Relaxed);
            crate::overlay::log_later(format!(
                "board: equip-time write on nameless 512x2048 RGBA8 resource {res:#x}: subresource {} {before:#x} -> {after:#x}",
                sub_text()
            ));
        }
        if before & LEAVES != 0 && after & LEAVES == 0 {
            // a write ended on any resource (out of COPY_DEST, RENDER_TARGET or UNORDERED_ACCESS): `board uploads`
            note_write_ended(res, t.Subresource, before, after);
        }
        if let Some(k) = w.iter().position(|&x| x == res) {
            BARRIERS_SEEN.fetch_add(1, Ordering::Relaxed);
            note_slot(k, t.Subresource, after);
            if let Some(v) = VERBOSE_RES.iter().position(|v| v.load(Ordering::Relaxed) == res) {
                LAST_SEEN_STATES[v].store((before as u64) << 32 | after as u64, Ordering::Release);
                LAST_SEEN_MS[v].store(win::now_ms().max(1), Ordering::Release);
                crate::overlay::log_later(format!("board: the game's barrier on the composite {res:#x}: subresource {} {before:#x} -> {after:#x}", sub_text()));
            }
        } else if before & COPY_DEST != 0 && after & COPY_DEST == 0 {
            let at = END_NEXT.fetch_add(1, Ordering::AcqRel) % ENDS;
            END_RES[at].store(0, Ordering::Release);
            END_STATE[at].store((t.Subresource as u64) << 32 | after as u64, Ordering::Release);
            END_PRESENT[at].store(crate::depth::PRESENT_NO.load(Ordering::Relaxed), Ordering::Release);
            END_RES[at].store(res, Ordering::Release);
        }
    }
}

/// The game's enhanced barriers, from the depth module's `Barrier` hook: a texture barrier on a watched resource is
/// noted (uploads then refuse: not supported). Same rules as [`observe`].
pub fn observe_enhanced(groups: &[D3D12_BARRIER_GROUP]) {
    if !ACTIVE.load(Ordering::Relaxed) || recording() {
        return;
    }
    let w: [usize; WATCHED] = std::array::from_fn(|k| WATCH[k].load(Ordering::Acquire));
    if w.iter().all(|&x| x == 0) {
        return;
    }
    for g in groups {
        // SAFETY: the texture member of the union, as the group's Type says; the game's own arrays.
        if g.Type != D3D12_BARRIER_TYPE_TEXTURE || g.NumBarriers == 0 || unsafe { g.Anonymous.pTextureBarriers.is_null() } {
            continue;
        }
        for t in unsafe { std::slice::from_raw_parts(g.Anonymous.pTextureBarriers, g.NumBarriers as usize) } {
            let res: usize = unsafe { std::mem::transmute_copy(&t.pResource) };
            if let Some(k) = w.iter().position(|&x| x == res && x != 0) {
                SEEN_LAYOUT[k].store(KNOWN | t.LayoutAfter.0 as u32 as u64, Ordering::Release);
                SEEN_COUNT[k].fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

/// What a slot holds now.
fn slot_seen(k: usize) -> Observed {
    let mut seen = Observed::default();
    for (m, s) in SEEN_STATES[k].iter().enumerate() {
        let v = s.load(Ordering::Acquire);
        if v & KNOWN != 0 {
            seen.states[m] = Some(v as u32);
        }
    }
    seen.transitions = SEEN_COUNT[k].load(Ordering::Acquire);
    let l = SEEN_LAYOUT[k].load(Ordering::Acquire);
    if l & KNOWN != 0 {
        seen.layout = Some(l as u32 as i32);
    }
    seen
}

/// Starts watching a set of resources (the targets): a resource still watched keeps its slot and what was seen; a new
/// one gets a free slot, cleared, with the upload ends seen lately on it. Only the worker calls this.
fn watch(resources: &[usize]) {
    let resources = &resources[..resources.len().min(WATCHED)];
    let old: [usize; WATCHED] = std::array::from_fn(|k| WATCH[k].load(Ordering::Acquire));
    // free the slots of resources no longer watched, remembering what was seen on them (keyed by the resource, until
    // it is gone or a `board find` by hand)
    for (k, &r) in old.iter().enumerate() {
        if r != 0 && !resources.contains(&r) {
            remember(r, slot_seen(k));
            WATCH[k].store(0, Ordering::Release);
        }
    }
    for &r in resources {
        if old.contains(&r) {
            continue;
        }
        let Some(k) = (0..WATCHED).find(|&k| WATCH[k].load(Ordering::Acquire) == 0) else { break };
        // what was seen on it while it was watched before (the target list rebuilt without it for a moment: the
        // composites go with the main when the main is made again) comes back; else a clear slot
        let remembered = REMEMBERED.lock().ok().and_then(|m| m.iter().find(|e| e.0 == r).map(|e| e.1));
        for (m, s) in SEEN_STATES[k].iter().enumerate() {
            s.store(remembered.and_then(|o| o.states[m]).map_or(0, |v| KNOWN | v as u64), Ordering::Release);
        }
        SEEN_COUNT[k].store(remembered.map_or(0, |o| o.transitions), Ordering::Release);
        SEEN_LAYOUT[k].store(remembered.and_then(|o| o.layout).map_or(0, |l| KNOWN | l as u32 as u64), Ordering::Release);
        SEEN_LAYOUT[k].store(0, Ordering::Release);
        // the upload ends seen on it lately, oldest first (an old one may be another texture's, at a reused address)
        let next = END_NEXT.load(Ordering::Acquire);
        let present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
        let mut states = [None; LEVELS];
        let mut n = 0;
        for i in 0..ENDS {
            let at = (next + i) % ENDS;
            if END_RES[at].load(Ordering::Acquire) == r && present.saturating_sub(END_PRESENT[at].load(Ordering::Acquire)) <= END_MERGE_PRESENTS {
                let v = END_STATE[at].load(Ordering::Acquire);
                note(&mut states, (v >> 32) as u32, v as u32);
                n += 1;
            }
        }
        for (m, s) in states.iter().enumerate() {
            if let Some(v) = s {
                SEEN_STATES[k][m].store(KNOWN | *v as u64, Ordering::Release);
            }
        }
        SEEN_COUNT[k].store(n + remembered.map_or(0, |o| o.transitions), Ordering::Release);
        WATCH[k].store(r, Ordering::Release);
    }
}

/// What was seen on a watched resource (or remembered of one watched before).
fn observed(resource: usize) -> Observed {
    match (0..WATCHED).find(|&k| WATCH[k].load(Ordering::Acquire) == resource && resource != 0) {
        Some(k) => slot_seen(k),
        None => REMEMBERED.lock().ok().and_then(|m| m.iter().find(|e| e.0 == resource).map(|e| e.1)).unwrap_or_default(),
    }
}

/// What was seen on resources watched before (keyed by the resource pointer): kept across rebuilds of the target
/// list, cleared for a resource that is gone ([`forget_observed`]) or for all by a `board find` by hand.
static REMEMBERED: Mutex<Vec<(usize, Observed)>> = Mutex::new(Vec::new());

fn remember(resource: usize, seen: Observed) {
    if seen.transitions == 0 && seen.layout.is_none() {
        return;
    }
    if let Ok(mut m) = REMEMBERED.lock() {
        m.retain(|e| e.0 != resource);
        if m.len() >= 16 {
            m.remove(0);
        }
        m.push((resource, seen));
    }
}

/// Forgets what was seen on a resource (it is gone: not a runtime texture's any more).
fn forget_observed(resource: usize) {
    if let Ok(mut m) = REMEMBERED.lock() {
        m.retain(|e| e.0 != resource);
    }
}

/// Forgets everything seen on resources no longer watched (`board find` by hand).
fn forget_all_observed() {
    if let Ok(mut m) = REMEMBERED.lock() {
        m.clear();
    }
}

// The identity test: for 60 s after a `board find` by hand, every transition the game makes out of COPY_DEST,
// RENDER_TARGET or UNORDERED_ACCESS on any resource GetDesc describes as a 2D 512 x 2048 texture of the RGBA8 or
// BGRA8 family is logged, watched or not: a composite born at equip is caught before the scan finds it. GetDesc is
// asked once per resource (a small cache the hook reads and fills with atomics).
static IDENTITY_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
const IDENTITY_MS: u64 = 60_000;
const SHAPE_CACHE: usize = 256;
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_U8: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static SHAPE_RES: [AtomicUsize; SHAPE_CACHE] = [ZERO; SHAPE_CACHE];
/// 1: a 512 x 2048 RGBA8/BGRA8 2D texture; 2: not.
static SHAPE_KIND: [std::sync::atomic::AtomicU8; SHAPE_CACHE] = [ZERO_U8; SHAPE_CACHE];
static IDENTITY_LINES: AtomicU64 = AtomicU64::new(0);

/// Whether `res` is a 2D 512 x 2048 texture of the RGBA8 or BGRA8 family (GetDesc, cached by address; a resource the
/// game freed and made again at the same address with another shape is the rare miss).
fn nameless_shape(res: usize) -> bool {
    let h = (res >> 4) % SHAPE_CACHE;
    for i in 0..4 {
        let k = (h + i) % SHAPE_CACHE;
        if SHAPE_RES[k].load(Ordering::Acquire) == res {
            return SHAPE_KIND[k].load(Ordering::Acquire) == 1;
        }
    }
    let raw = res as *mut c_void;
    // SAFETY: the resource is referenced by the barrier being recorded (alive), as the depth module's describe
    let kind = match unsafe { ID3D12Resource::from_raw_borrowed(&raw) } {
        Some(r) => {
            let d = unsafe { r.GetDesc() };
            let yes = d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
                && d.Width == WIDTH as u64
                && d.Height == HEIGHT
                && d.DepthOrArraySize == 1
                && composite_format_ok(d.Format.0);
            if yes {
                1
            } else {
                2
            }
        }
        None => 2,
    };
    for i in 0..4 {
        let k = (h + i) % SHAPE_CACHE;
        if SHAPE_RES[k].compare_exchange(0, res, Ordering::AcqRel, Ordering::Acquire).is_ok() || SHAPE_RES[k].load(Ordering::Acquire) == res {
            SHAPE_KIND[k].store(kind, Ordering::Release);
            break;
        }
    }
    kind == 1
}

// ---------------------------------------------------------------- the board's state (worker and pipe side)

/// A board request made the board live: the overlay installs (its Present hook does the uploads), barriers are watched.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// An upload is waiting for the render thread.
static PENDING: AtomicBool = AtomicBool::new(false);
/// Test support: the ids to use outside the game (a host's fakes).
static TEST_IDS: Mutex<Option<Ids>> = Mutex::new(None);

/// Whether the overlay is wanted for the board (any board request since load).
pub fn wanted() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Whether an upload, a step of the read-back check, or a GetDesc of the targets waits (the overlay calls
/// [`on_present`] only then).
pub fn pending() -> bool {
    PENDING.load(Ordering::Acquire)
        || VERIFY.load(Ordering::Acquire)
        || PEEK.load(Ordering::Acquire)
        || SLICES.load(Ordering::Acquire)
        || HUNTING.load(Ordering::Acquire)
        || DESCRIBE.load(Ordering::Acquire)
        || DESCRIBE_RING.load(Ordering::Acquire)
        || NEW_NEXT.load(Ordering::Acquire) != NEW_TAKEN.load(Ordering::Relaxed)
}

/// A step of the read-back check wants a present.
static VERIFY: AtomicBool = AtomicBool::new(false);

/// Test support: a host's fake objects stand in for the game's (the caller refuses in Skate.exe).
pub fn set_test_ids(ids: Ids) {
    *TEST_IDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(ids);
}

/// Art for the board: its chains (the BC7 one first, whose digest the status reports; others of the same picture in
/// other formats), its label and digest.
pub struct Art {
    pub chains: Vec<Chain>,
    pub label: String,
    pub sha256: String,
    /// The deck slot the chains are for (the graphic, or a layer: the mask, the normal map, the holo map).
    pub slot: Slot,
}

impl Art {
    /// The chain for a `width` x `height` texture of DXGI format `dxgi` with `mips` levels: the one of its family and
    /// size with enough levels (the other 4-byte order, swizzled on the way: `swizzle`); failing that, a chain of the
    /// same family whose size is the texture's doubled once, twice or three times with enough levels past the
    /// first ones: its tail (the level offset returned) is exactly the demoted texture's chain, as the streamer's
    /// demotion drops the top levels. Err says what is missing.
    pub fn pick_chain(&self, dxgi: i32, mips: u16, width: u32, height: u32) -> Result<(&Chain, bool, usize), String> {
        let family = ChainFormat::family_of(dxgi).ok_or_else(|| format!("DXGI format {dxgi} is none the board writes (BC7, BC1, RGBA8, BGRA8)"))?;
        let other = match family {
            ChainFormat::Rgba8 => Some(ChainFormat::Bgra8),
            ChainFormat::Bgra8 => Some(ChainFormat::Rgba8),
            ChainFormat::Bc7 | ChainFormat::Bc1 => None,
        };
        for skip in 0..4usize {
            let fits = |c: &&Chain| c.levels >= mips as usize + skip && c.width == width << skip && c.height == height << skip;
            if let Some(c) = self.chains.iter().filter(fits).find(|c| c.format == family) {
                return Ok((c, false, skip));
            }
            if let Some(c) = other.and_then(|o| self.chains.iter().filter(fits).find(|c| c.format == o)) {
                return Ok((c, true, skip));
            }
        }
        Err(format!(
            "no {} chain of {width}x{height} with {mips} or more levels among the art's ({})",
            family.name(),
            self.chains.iter().map(|c| format!("{} {}x{} x{}", c.format.name(), c.width, c.height, c.levels)).collect::<Vec<_>>().join(", ")
        ))
    }
}

/// Whether to write the siblings (off: they turned out to be other decks' graphics, 2026-10-21 night).
static SIBLINGS_ON: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Found {
    t: Target,
    found_ms: u64,
    /// The art (its digest) last written into this resource, and when (ms).
    applied: Option<(String, u64)>,
    /// What GetDesc says on the render thread (`described`): its DXGI format, mip count and size.
    dxgi_format: i32,
    mips: u16,
    width: u32,
    height: u32,
    described: bool,
    /// A target by hand: whether its device was checked against the presenting one, and found to be another.
    device_checked: bool,
    wrong_device: bool,
    /// The present we last wrote it at; for the world copy, the present of the game's last re-fill noticed and when
    /// (ms) it was noticed (0: none pending): it is written again right after.
    written_present: u64,
    refill_present: u64,
    refill_noticed_ms: u64,
    /// Why the render thread last refused to write it (empty: it did not).
    refused: String,
    /// A composite that never showed a barrier while another did ("no barriers ever: probably COMMON with implicit
    /// promotion"), once noted.
    note: String,
}

/// Write a composite that never shows a barrier from COMMON when its state is automatic (`board composite-common on`).
static COMPOSITE_COMMON: AtomicBool = AtomicBool::new(false);
/// A composite found this long ago with no barrier seen on it, while another composite showed some, gets the note.
const NO_BARRIERS_MS: u64 = 10_000;

/// Lists that carried a barrier on a watched resource (set by the barrier hook, taken by the queue's
/// `ExecuteCommandLists` hook): a composite copied into on a COPY queue would show so.
const TOUCHED: usize = 16;
static TOUCHED_LISTS: [AtomicUsize; TOUCHED] = [ZERO; TOUCHED];
static COPY_QUEUE_LISTS: AtomicU64 = AtomicU64::new(0);

/// The game's legacy barriers, with the list they are recorded on (see [`observe`]).
pub fn observe_on(list: usize, barriers: &[D3D12_RESOURCE_BARRIER]) {
    if !ACTIVE.load(Ordering::Relaxed) || recording() {
        return;
    }
    observe(barriers);
    // did one of them touch a watched resource? remember the list
    let w: [usize; WATCHED] = std::array::from_fn(|k| WATCH[k].load(Ordering::Acquire));
    let touched = barriers.iter().any(|b| {
        if b.Type != D3D12_RESOURCE_BARRIER_TYPE_TRANSITION {
            return false;
        }
        // SAFETY: a transition barrier's union member, as its Type says.
        let res: usize = unsafe { std::mem::transmute_copy(&b.Anonymous.Transition.pResource) };
        res != 0 && w.contains(&res)
    });
    if touched && !TOUCHED_LISTS.iter().any(|l| l.load(Ordering::Relaxed) == list) {
        for l in &TOUCHED_LISTS {
            if l.compare_exchange(0, list, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                break;
            }
        }
    }
}

/// From the overlay's `ExecuteCommandLists` hook: lists that carried a barrier on a watched resource, submitted on a
/// COPY queue, are logged (and forgotten either way).
///
/// # Safety
/// `queue` is the queue being submitted on.
pub unsafe fn on_execute(queue: *mut c_void, lists: &[*mut c_void]) {
    let mut carried = Vec::new();
    for l in &TOUCHED_LISTS {
        let v = l.load(Ordering::Acquire);
        if v != 0 && lists.iter().any(|&x| x as usize == v) {
            l.store(0, Ordering::Release);
            carried.push(v);
        }
    }
    if carried.is_empty() {
        return;
    }
    let kind = ID3D12CommandQueue::from_raw_borrowed(&queue).map(|q| q.GetDesc().Type);
    if kind == Some(D3D12_COMMAND_LIST_TYPE_COPY) {
        COPY_QUEUE_LISTS.fetch_add(carried.len() as u64, Ordering::Relaxed);
        crate::overlay::log_later(format!(
            "board: a COPY queue ({:#x}) executed {} list(s) with a barrier on a watched texture: {}",
            queue as usize,
            carried.len(),
            carried.iter().map(|l| format!("{l:#x}")).collect::<Vec<_>>().join(", ")
        ));
    }
}

/// The render thread is asked to GetDesc the targets (they changed).
static DESCRIBE: AtomicBool = AtomicBool::new(false);
/// Resources whose every transition seen is logged (the composites: what the game does to them at equip), and the
/// last transition seen on each (before << 32 | after, and when in ms; 0: none).
/// Room for the resources whose barriers are logged: the composites, the finals, the targets added by hand.
const VERBOSE_SLOTS: usize = 12;
static VERBOSE_RES: [AtomicUsize; VERBOSE_SLOTS] = [ZERO; VERBOSE_SLOTS];
static LAST_SEEN_STATES: [AtomicU64; VERBOSE_SLOTS] = [ZERO64; VERBOSE_SLOTS];
static LAST_SEEN_MS: [AtomicU64; VERBOSE_SLOTS] = [ZERO64; VERBOSE_SLOTS];

/// The last transition seen on a composite, for the status: "N ms ago 0xBEFORE -> 0xAFTER" (empty: none).
fn last_seen(resource: usize, now: u64) -> String {
    match VERBOSE_RES.iter().position(|v| v.load(Ordering::Relaxed) == resource && resource != 0) {
        Some(k) => {
            let at = LAST_SEEN_MS[k].load(Ordering::Acquire);
            if at == 0 {
                return String::new();
            }
            let s = LAST_SEEN_STATES[k].load(Ordering::Acquire);
            format!("{} ms ago {:#x} -> {:#x}", now.saturating_sub(at), s >> 32, s as u32)
        }
        None => String::new(),
    }
}

#[derive(Default)]
struct Board {
    ids: Option<Ids>,
    ids_error: String,
    ids_tried: Option<u64>,
    targets: Vec<Found>,
    scan: Option<WordScan>,
    scan_started: u64,
    scans: u32,
    last_scan: String,
    /// The search for the texture's siblings (the equip-time copies): a scan for the texture's pointer, run when the
    /// main target changes and, while fewer siblings are found than were last seen (a re-equip makes new ones), every
    /// `AUTO_FIND_MS`.
    sibling_scan: Option<WordScan>,
    sibling_scan_started: u64,
    sibling_scans: u32,
    sibling_find_wanted: bool,
    siblings_max: usize,
    next_sibling_find: u64,
    last_sibling_scan: String,
    /// The search for the composite (the render target the ridden board samples): a scan for the runtime texture
    /// vtable's word, the same rules as the siblings'.
    composite_scan: Option<WordScan>,
    composite_scan_started: u64,
    composite_scans: u32,
    composite_find_wanted: bool,
    composites_max: usize,
    /// How many finals were last found (fewer now: a re-equip made new ones, look again).
    finals_max: usize,
    /// The world copies: the name hashes looked for (empty: the default), how many were last found.
    worldcopy_hashes: Vec<u64>,
    worldcopies_max: usize,
    /// How many ridden copies were last found (fewer now: a re-equip made a new one, look again).
    riddens_max: usize,
    /// Whether The Caveman was the equipped deck at the last tick (a rising edge requeues the kept arts for the
    /// ridden copies).
    was_equipped: bool,
    /// A target by a raw resource pointer waiting for its owner (`board target --resource`): a scan of the process's
    /// memory for the pointer value finds the RenderCore2 object or runtime texture naming it (run in `tick`, never
    /// while another scan runs); what the last one came to.
    raw_pending: Option<RawPending>,
    raw_last: String,
    /// The owner scan running (for the raw target, or for every entry of the uploads table: `board uploads`), and
    /// whether the uploads' owners are wanted; when they were last looked up.
    owner_scan: Option<OwnerScan>,
    owners_wanted: bool,
    owners_at: u64,
    /// The peek asked for, then its result (`board peek`).
    peek: Option<Peek>,
    /// The present the main texture was last found or made again at (the equip's writes come after it).
    main_made_at: u64,
    /// The slice job running, the slices found to hold our art, the last job's result, whether the tiles are kept
    /// (written again when the game writes one), when they were last written and how often again, a state by hand
    /// for the arrays.
    slice_job: Option<SliceJob>,
    slice_matches: Vec<SliceMatch>,
    slices_result: Value,
    tiles_keep: bool,
    tiles_written_at: u64,
    tiles_rewrites: u64,
    slices_state: Option<u32>,
    /// The colour hunt running or done (`board hunt`), and the owner scan's values when they are not the uploads
    /// table's (the hunt's hits).
    hunt: Option<Hunt>,
    owner_values: Option<Vec<u64>>,
    /// The census hunt waiting for its scan (`board hunt --census`): the scan for the runtime texture vtable, the
    /// request and its parsed assumed state, when it started (ms).
    census_scan: Option<WordScan>,
    census_req: Option<(crate::proto::HuntReq, Option<u32>)>,
    census_started: u64,
    next_composite_find: u64,
    last_composite_scan: String,
    find_now: bool,
    next_auto_find: u64,
    /// When the main was lost (ms; 0: not lost): the scan runs at once and fast for the next 30 s, and the time
    /// from the loss to the kept art written is logged.
    lost_at: u64,
    found_again_at: u64,
    lost_to_written_ms: u64,
    validated: u64,
    /// Why candidates were refused at the last find.
    refused: String,
    manual: Option<u32>,
    /// States set by hand for a resource or a kind (over `manual`).
    manual_per: Vec<(ManualFor, u32)>,
    art: Option<Arc<Art>>,
    /// The arts kept for the layers (the mask, the normal map, the holo map: `Slot::index() - 1`), written into The
    /// Caveman's own layer textures as the art is into the graphic's.
    layer_arts: [Option<Arc<Art>>; 3],
    /// Put the art back on a texture made again (default on).
    drop_art: bool,
    job: Option<Arc<Art>>,
    job_since: u64,
    uploads: u64,
    last_upload: Value,
    last_error: String,
    /// The game's reads of the fx.cave.* states (summed) and when they last changed: the deck's effects read them
    /// every frame while it is equipped (or hovered in the menu).
    reads: u64,
    reads_changed: Option<u64>,
    log: Vec<String>,
    /// Lines from the render thread for the worker to log.
    unlogged: Vec<String>,
    /// The read-back check of the last write (`board verify`): a 4 x 4 block of mips 0 and 2 of every written target
    /// read back right after the upload (in its own list) and again 2 s later, against the art sent. What the blocks
    /// should hold, the targets (the texture and the states it was written in) with each one's result now and later,
    /// the stage (0 idle; 1 the read after the upload recorded; 2 waiting for `verify_due`; 3 the later read
    /// recorded), when each read landed, and the art it is about.
    verify_targets: Vec<VerifyTarget>,
    verify_stage: u8,
    verify_due: u64,
    verify_now_at: u64,
    verify_later_at: u64,
    verify_label: String,
}

/// `board peek`: an N x N block at a texel position of a mip of a target, read back through the check's path (the
/// mip to COPY_SOURCE from the target's state and back), the texels printed as the bytes they are (RGBA8/BGRA8: 4 a
/// texel; BC7: the 16-byte blocks, the position and size rounded to blocks).
#[derive(Clone, Debug)]
struct Peek {
    resource: usize,
    x: u32,
    y: u32,
    mip: u32,
    n: u32,
    /// 0: to record; 1: recorded, waiting for the fence; 2: done (`result`).
    stage: u8,
    /// What the copy took: the box's left, top, width, height (texels), the rows and bytes a row in the read-back,
    /// the format and family.
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    rows: usize,
    row_bytes: usize,
    format: i32,
    family: Option<ChainFormat>,
    states: String,
    source: String,
    result: Value,
}

static PEEK: AtomicBool = AtomicBool::new(false);

/// A raw resource waiting for its owner: the pointer, a state given with it, since when.
#[derive(Clone, Copy, Debug)]
struct RawPending {
    resource: usize,
    state: Option<u32>,
    since: u64,
}

/// The owner scan: the words holding one of `values` (the raw resource, or the uploads table's resources).
struct OwnerScan {
    scan: WordScan,
    values: Vec<usize>,
    raw: Option<RawPending>,
    started: u64,
}

/// A texture written, for the read-back check: the resource (raw), the states it was written in, its mip count and
/// the format its blocks are copied as, the rows a block takes (1 for BC7, 4 for 4-byte texels), what the blocks of
/// mips 0 and 2 should hold, and the results.
#[derive(Clone, Debug, Default)]
struct VerifyTarget {
    resource: usize,
    states: [u32; LEVELS],
    mips: usize,
    format: i32,
    rows: usize,
    expect: [Vec<u8>; 2],
    now: String,
    later: String,
}

/// At most this many targets are read back (the read-back buffer's room: 2 KB a target).
const VERIFY_TARGETS: usize = 8;

impl Board {
    fn keep(&self) -> bool {
        !self.drop_art
    }

    /// The art kept for a slot: the graphic's for the art slot (every kind of copy of it too), a layer's for its own.
    fn art_for(&self, slot: Slot) -> Option<Arc<Art>> {
        match slot {
            Slot::Art => self.art.clone(),
            s => self.layer_arts[s.index() - 1].clone(),
        }
    }

    /// Whether any art is kept (the graphic's or a layer's).
    fn any_art(&self) -> bool {
        self.art.is_some() || self.layer_arts.iter().any(Option::is_some)
    }

    /// The world copies' name hashes: the list set by hand, else the default.
    fn worldcopy_hashes_list(&self) -> Vec<u64> {
        if self.worldcopy_hashes.is_empty() {
            DEFAULT_WORLDCOPY_HASHES.to_vec()
        } else {
            self.worldcopy_hashes.clone()
        }
    }

    /// The state set by hand for a target: for its resource, else its kind, else for every target.
    fn manual_for(&self, t: &Target) -> Option<u32> {
        self.manual_per
            .iter()
            .find(|(f, _)| matches!(f, ManualFor::Resource(_)) && f.matches(t))
            .or_else(|| self.manual_per.iter().find(|(f, _)| f.matches(t)))
            .map(|(_, s)| *s)
            .or(self.manual)
    }

    /// Whether a composite may be written from COMMON (no barrier ever seen while another composite showed some,
    /// and `board composite-common on`).
    fn assume_common(&self, f: &Found) -> bool {
        f.t.composite && COMPOSITE_COMMON.load(Ordering::Relaxed) && !f.note.is_empty()
    }

    fn say(&mut self, line: String) {
        crate::worker::log(&format!("board: {line}"));
        self.remember(line);
    }

    /// From the render thread: kept for the status, written to the DLL's log by the worker's next tick (the log takes
    /// the worker's lock and writes a file: never on a frame).
    fn say_later(&mut self, line: String) {
        self.unlogged.push(line.clone());
        self.remember(line);
    }

    fn remember(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 12 {
            self.log.remove(0);
        }
    }

    /// The kept art goes on any target that does not have it yet.
    /// Whether The Caveman is the equipped deck now: the game reads its fx.cave.* states every frame while it is (or
    /// while it is hovered in the menu).
    fn equipped(&self, now: u64) -> bool {
        self.reads_changed.is_some_and(|t| now.saturating_sub(t) < 1500)
    }

    fn requeue(&mut self, now: u64) {
        if self.job.is_some() || !self.keep() {
            return;
        }
        // any target whose slot has a kept art it does not hold yet (the ridden copies only while The Caveman is
        // the equipped deck: they are the game's slots for whatever deck is ridden, refilled at every equip, and
        // writing them while another deck is equipped puts The Caveman's look on that deck)
        let equipped = self.equipped(now);
        let due = self.targets.iter().find_map(|f| {
            if f.t.is_ridden && !equipped {
                return None;
            }
            let art = self.art_for(f.t.slot)?;
            (f.applied.as_ref().map(|a| a.0.as_str()) != Some(art.sha256.as_str())).then_some(art)
        });
        if let Some(art) = due {
            self.job = Some(art);
            self.job_since = now;
            PENDING.store(true, Ordering::Release);
        }
    }
}

fn cell() -> &'static Mutex<Board> {
    static B: OnceLock<Mutex<Board>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(Board::default()))
}

fn board() -> MutexGuard<'static, Board> {
    cell().lock().unwrap_or_else(|e| e.into_inner())
}

/// Bytes the finder reads per tick, and at most this long.
const SCAN_BUDGET: usize = 32 << 20;
const SCAN_STEP_MS: u64 = 6;
/// The look for a main lost while an art is kept: a bigger step, again every 300 ms, for 30 s.
const FAST_SCAN_BUDGET: usize = 128 << 20;
const FAST_SCAN_STEP_MS: u64 = 12;
const FAST_FIND_AGAIN_MS: u64 = 300;
const FAST_FIND_MS: u64 = 30_000;
const VALIDATE_MS: u64 = 250;
/// How long after a failed find (art waiting, or kept) the next one starts by itself.
const AUTO_FIND_MS: u64 = 5_000;

/// Every worker tick: the ids (once), the targets checked again, a find in steps when one is wanted, a texture made
/// again given the kept art. `base`: Skate.exe's (None outside the game); `cave_reads`: the game's reads of the
/// fx.cave.* states so far.
pub fn tick(now: u64, base: Option<usize>, cave_reads: u64) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let mut b = board();
    for line in std::mem::take(&mut b.unlogged) {
        crate::worker::log(&format!("board: {line}"));
    }
    if cave_reads != b.reads {
        b.reads = cave_reads;
        b.reads_changed = Some(now);
    }
    if b.ids.is_none() && b.ids_tried.is_none_or(|t| now >= t + 10_000) {
        b.ids_tried = Some(now);
        let ids = match base {
            Some(base) => Ids::for_game(base),
            None => TEST_IDS.lock().ok().and_then(|g| *g).ok_or_else(|| "not in the game (no test ids)".to_string()),
        };
        match ids {
            Ok(ids) => {
                b.ids = Some(ids);
                b.ids_error.clear();
            }
            Err(e) => {
                if b.ids_error != e {
                    b.say(format!("not usable: {e}"));
                }
                b.ids_error = e;
            }
        }
    }
    // the later read of the check, when due
    if b.verify_stage == 2 && now >= b.verify_due && !b.targets.is_empty() {
        VERIFY.store(true, Ordering::Release);
    }
    let Some(ids) = b.ids else { return };
    // the targets, again: a texture made again (another resource behind the same asset) is a new target
    let wc_hashes = b.worldcopy_hashes_list();
    if now >= b.validated + VALIDATE_MS && !b.targets.is_empty() {
        b.validated = now;
        let mut changed = false;
        let mut main_changed = false;
        let mut kept = Vec::new();
        for f in std::mem::take(&mut b.targets) {
            let check = if f.t.is_raw() {
                read_raw(f.t.resource).and_then(|t| {
                    raw_still_owned(&f.t, &ids).map(|_| Target {
                        owner: f.t.owner,
                        owner_texture: f.t.owner_texture,
                        rc2: f.t.rc2,
                        rc2_bind: f.t.rc2_bind,
                        rc2_state: f.t.rc2_state,
                        id: f.t.id,
                        ..t
                    })
                })
            } else if f.t.is_worldcopy {
                read_worldcopy(f.t.texture, &ids, &wc_hashes)
            } else if f.t.is_ridden {
                read_ridden_fmt(f.t.texture, &ids, ridden_format(f.t.slot), f.t.slot)
            } else if f.t.manual {
                read_manual(f.t.texture, &ids)
            } else if f.t.is_final {
                read_final(f.t.texture, &ids, &FINAL_HASHES)
            } else if f.t.composite {
                read_composite(f.t.texture, &ids)
            } else if f.t.sibling {
                read_sibling(f.t.texture, &ids)
            } else {
                read_chain_slot(f.t.asset, &ids, f.t.slot)
            };
            match check {
                Ok(t) if t.resource == f.t.resource && t.texture == f.t.texture => kept.push(Found { t, ..f }),
                Ok(t) => {
                    changed = true;
                    main_changed |= f.t.is_main();
                    if f.t.is_main() {
                        b.main_made_at = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
                    }
                    b.say(format!(
                        "{} texture was made again (resource {:#x} -> {:#x})",
                        if f.t.sibling { "a sibling" } else { "its" },
                        f.t.resource,
                        t.resource
                    ));
                    kept.push(Found { t, found_ms: now, applied: None, ..Default::default() });
                }
                Err(e) => {
                    changed = true;
                    main_changed |= f.t.is_main();
                    forget_observed(f.t.resource);
                    b.say(format!(
                        "lost the {} texture at {:#x}: {e}",
                        f.t.kind(),
                        if f.t.sibling || f.t.composite || f.t.is_ridden || f.t.is_final || f.t.is_worldcopy { f.t.texture } else { f.t.asset }
                    ));
                }
            }
        }
        // siblings and composites go with the main texture (targets added by hand stay)
        if !kept.iter().any(|f| f.t.is_main()) && kept.iter().any(|f| f.t.sibling || f.t.composite || f.t.is_final || f.t.is_ridden) {
            kept.retain(|f| !f.t.sibling && !f.t.composite && !f.t.is_final && !f.t.is_ridden);
            changed = true;
        }
        b.targets = kept;
        if changed {
            targets_changed(&b);
            if b.targets.is_empty() {
                // the main lost (a re-equip made it again): looked for at once, fast
                b.next_auto_find = now;
                b.lost_at = now;
                b.found_again_at = 0;
            }
            b.requeue(now);
        }
        if main_changed {
            b.sibling_find_wanted = true;
            b.composite_find_wanted = true;
            // a pointer is never trusted across an equip: the raw targets go
            let raw: Vec<usize> = b.targets.iter().filter(|f| f.t.is_raw()).map(|f| f.t.resource).collect();
            if !raw.is_empty() {
                b.targets.retain(|f| !f.t.is_raw());
                b.manual_per.retain(|(f, _)| !matches!(f, ManualFor::Resource(r) if raw.contains(r)));
                b.say(format!("the main texture was made again: {} raw target(s) dropped (a pointer is never trusted across an equip)", raw.len()));
                targets_changed(&b);
            }
        }
    }
    // a composite found a while ago with no barrier ever seen on it, while another composite showed some: noted (it
    // is probably kept in COMMON and used through implicit promotion: no barrier would ever show)
    let any_seen = b.targets.iter().any(|f| f.t.composite && observed(f.t.resource).transitions > 0);
    if any_seen {
        let mut lines = Vec::new();
        for f in b.targets.iter_mut() {
            if f.t.composite && f.note.is_empty() && now >= f.found_ms + NO_BARRIERS_MS && observed(f.t.resource).transitions == 0 {
                f.note = "no barriers ever: probably COMMON with implicit promotion".into();
                lines.push(format!(
                    "composite {:#x} (id {:#x}): {} (board composite-common on writes it from COMMON; board state common --resource {:#x} by hand)",
                    f.t.resource, f.t.id, f.note, f.t.resource
                ));
            }
        }
        for l in lines {
            b.say(l);
        }
    }
    // a composite whose state became known (the game's barrier on it seen, at a re-equip) gets the kept art by
    // itself; one the render thread refused for its own reasons (GetDesc, no chain) waits for a new art
    if b.job.is_none() && b.keep() {
        if let Some(art) = b.art.clone() {
            let due = b.targets.iter().any(|f| {
                f.t.composite
                    && f.applied.as_ref().map(|a| a.0.as_str()) != Some(art.sha256.as_str())
                    && (f.refused.is_empty() || f.refused.contains("state unknown"))
                    && decide_composite(
                        b.manual_for(&f.t),
                        &observed(f.t.resource),
                        (f.t.rc2 != 0).then_some((f.t.rc2_bind, f.t.rc2_state)),
                        true,
                        b.assume_common(f),
                    )
                    .is_ok()
            });
            if due {
                b.requeue(now);
            }
        }
    }
    // finding (fast for 30 s after the main was lost while an art is kept: a bigger step, and again every 300 ms
    // while nothing is found, so a texture made again is found in about 2 s rather than after the next 5 s look)
    let wants_art = b.job.is_some() || (b.keep() && b.any_art());
    let fast = b.lost_at != 0 && now.saturating_sub(b.lost_at) < FAST_FIND_MS && wants_art;
    let want_find = b.find_now || (b.targets.is_empty() && wants_art && now >= b.next_auto_find);
    if b.scan.is_none() && want_find {
        b.find_now = false;
        match WordScan::new(&[ids.asset_vtable as u64], 1 << 20) {
            Ok(s) => {
                b.scan = Some(s);
                b.scan_started = now;
            }
            Err(e) => {
                b.say(format!("cannot scan: {e}"));
                b.next_auto_find = now + AUTO_FIND_MS;
            }
        }
    }
    let done = match b.scan.as_mut() {
        Some(s) if fast => s.step(FAST_SCAN_BUDGET, FAST_SCAN_STEP_MS),
        Some(s) => s.step(SCAN_BUDGET, SCAN_STEP_MS),
        None => false,
    };
    if done {
        let s = b.scan.take().unwrap();
        b.scans += 1;
        let mut found: Vec<Target> = Vec::new();
        let mut refused = Vec::new();
        for &a in &s.found {
            // The Caveman's graphic, and since 2026-10-22 its own mask, normal map and holo map (the holo host): all
            // four are targets (kinds main, osk, ny, rgb), each written with its slot's art
            match read_chain_any(a, &ids) {
                Ok(t) => {
                    if !found.iter().any(|x| x.resource == t.resource) {
                        found.push(t);
                    }
                }
                Err(e) if e.starts_with("not a TextureAsset") || e == "another texture" || e.ends_with("is not readable") && !e.contains("D3D12") => {}
                Err(e) => refused.push(format!("{a:#x}: {e}")),
            }
        }
        found.truncate(WATCHED);
        b.last_scan = format!("{} MB in {} ms, {} TextureAsset words, {} of The Caveman's", s.bytes >> 20, now - b.scan_started, s.found.len(), found.len());
        b.refused = refused.join("; ");
        let line = match found.len() {
            0 if refused.is_empty() => format!("not found ({}): is the rebuilt Caveman-FX installed, and The Caveman loaded (equip it once)?", b.last_scan),
            0 => format!("found but refused ({}): {}", b.last_scan, b.refused),
            _ => format!(
                "found: {} ({})",
                found.iter().map(|t| format!("asset {:#x}, resource {:#x}", t.asset, t.resource)).collect::<Vec<_>>().join("; "),
                b.last_scan
            ),
        };
        b.say(line);
        let old: Vec<(usize, Option<(String, u64)>)> = b.targets.iter().map(|f| (f.t.resource, f.applied.clone())).collect();
        b.targets = found
            .into_iter()
            .map(|t| Found { t, found_ms: now, applied: old.iter().find(|o| o.0 == t.resource).and_then(|o| o.1.clone()), ..Default::default() })
            .collect();
        if b.targets.iter().any(|f| f.t.is_main()) {
            b.main_made_at = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
        }
        targets_changed(&b);
        if b.targets.is_empty() {
            b.next_auto_find = now + if fast { FAST_FIND_AGAIN_MS } else { AUTO_FIND_MS };
        } else if b.lost_at != 0 && b.found_again_at == 0 {
            b.found_again_at = now;
            let line = format!("the main found again {} ms after it was lost (scan {} of the fast look)", now.saturating_sub(b.lost_at), b.scans);
            b.say(line);
        }
        b.requeue(now);
        b.sibling_find_wanted = !b.targets.is_empty();
        b.composite_find_wanted = !b.targets.is_empty();
    }
    // the world copy re-filled by the game (its COPY_DEST -> 0xc0 seen by the uploads table after our last write):
    // written again at once, the art on it forgotten so the requeue takes it
    let mut refilled = Vec::new();
    for f in b.targets.iter_mut() {
        if !f.t.is_worldcopy || f.applied.is_none() {
            continue;
        }
        let Some(last) = uploads_last_present(f.t.resource) else { continue };
        if last > f.written_present && last != f.refill_present {
            f.refill_present = last;
            f.refill_noticed_ms = now;
            f.applied = None;
            refilled.push(format!("{:#x} at present {last} (ours at {})", f.t.resource, f.written_present));
        }
    }
    if !refilled.is_empty() {
        b.say(format!("the world copy re-filled by the game: {}: written again", refilled.join(", ")));
        b.requeue(now);
    }
    if b.lost_at != 0 && b.job.is_none() {
        let written =
            b.art.as_ref().is_some_and(|a| b.targets.iter().any(|f| f.t.is_main() && f.applied.as_ref().map(|x| x.0.as_str()) == Some(a.sha256.as_str())));
        if written {
            let line = format!(
                "the kept art written {} ms after the main was lost (found again after {} ms)",
                now.saturating_sub(b.lost_at),
                b.found_again_at.saturating_sub(b.lost_at)
            );
            b.say(line);
            b.lost_to_written_ms = now.saturating_sub(b.lost_at);
            b.lost_at = 0;
        } else if now.saturating_sub(b.lost_at) > 120_000 {
            b.lost_at = 0;
        }
    }
    // the owner scan (`board target --resource`: a raw resource needs a live owner; `board uploads`: the owner of
    // every entry), never while another scan runs
    // The Caveman equipped again (its fx states read): its ridden copies, the game's slots for the ridden deck, take
    // the kept arts now (parked while another deck was equipped)
    let equipped = b.equipped(now);
    if equipped && !b.was_equipped {
        b.requeue(now);
    }
    b.was_equipped = equipped;
    owner_scan_tick(&mut b, now, &ids);
    census_tick(&mut b, now, &ids);
    tiles_keep_tick(&mut b, now);
    // the composite (the render target the game draws the deck's graphic into at equip, which the ridden board
    // samples): every runtime texture object (the words holding the runtime texture vtable), those of the main's size
    // with no name hash and a RenderCore2 object. Looked for as the siblings are (below)
    let main = b.targets.iter().find(|f| f.t.is_main()).map(|f| f.t);
    let composites = b.targets.iter().filter(|f| f.t.composite).count();
    let finals = b.targets.iter().filter(|f| f.t.is_final).count();
    let finals_on = FINALS_ON.load(Ordering::Relaxed);
    let worldcopies = b.targets.iter().filter(|f| f.t.is_worldcopy).count();
    let worldcopy_on = WORLDCOPY_ON.load(Ordering::Relaxed);
    let riddens = b.targets.iter().filter(|f| f.t.is_ridden).count();
    let ridden_on = RIDDEN_ON.load(Ordering::Relaxed);
    let main_scanning = b.scan.is_some();
    if b.composite_scan.is_none() && !main_scanning && b.sibling_scan.is_none() {
        if main.is_some() {
            let equipped = b.reads_changed.is_some_and(|t| now.saturating_sub(t) < 1500);
            // (the same census finds the finals: the run-time-made copies with the known name hashes; and the ridden
            // copy: the nameless sRGB texture the in-world board samples)
            let missing = composites < b.composites_max
                || (composites == 0 && equipped)
                || (finals_on && (finals < b.finals_max || (finals == 0 && equipped)))
                || (worldcopy_on && (worldcopies < b.worldcopies_max || (worldcopies == 0 && equipped)))
                || (ridden_on && (riddens < b.riddens_max || (riddens == 0 && equipped)));
            if b.composite_find_wanted || (missing && now >= b.next_composite_find) {
                b.composite_find_wanted = false;
                match WordScan::new(&[ids.texture_vtable as u64], 1 << 20) {
                    Ok(s) => {
                        b.composite_scan = Some(s);
                        b.composite_scan_started = now;
                    }
                    Err(e) => {
                        b.say(format!("cannot scan for the composite: {e}"));
                        b.next_composite_find = now + AUTO_FIND_MS;
                    }
                }
            }
        } else {
            b.composite_find_wanted = false;
        }
    }
    let done = match b.composite_scan.as_mut() {
        Some(s) if !main_scanning => s.step(SCAN_BUDGET, SCAN_STEP_MS),
        _ => false,
    };
    if done {
        let s = b.composite_scan.take().unwrap();
        b.composite_scans += 1;
        b.next_composite_find = now + AUTO_FIND_MS;
        let Some(m) = main else { return };
        let found = composites_among(&s.found, &ids, &m, MAX_COMPOSITES);
        let finals_found = if finals_on { finals_among(&s.found, &ids, &m, &FINAL_HASHES, FINALS_PER_HASH) } else { Vec::new() };
        let wc_found = if worldcopy_on { worldcopies_among(&s.found, &ids, &m, &wc_hashes, WORLDCOPIES_MAX) } else { Vec::new() };
        let ridden_found = if ridden_on { riddens_among(&s.found, &ids, &m, RIDDENS_MAX) } else { Vec::new() };
        b.last_composite_scan = format!(
            "{} MB in {} ms, {} runtime textures, {} composite(s), {} final(s), {} world cop{}, {} ridden cop{}",
            s.bytes >> 20,
            now - b.composite_scan_started,
            s.found.len(),
            found.len(),
            finals_found.len(),
            wc_found.len(),
            if wc_found.len() == 1 { "y" } else { "ies" },
            ridden_found.len(),
            if ridden_found.len() == 1 { "y" } else { "ies" }
        );
        // the ridden copy: replaced when it changed (a re-equip makes a new one; the old one is released)
        let before: Vec<usize> = b.targets.iter().filter(|f| f.t.is_ridden).map(|f| f.t.resource).collect();
        let after: Vec<usize> = ridden_found.iter().map(|t| t.resource).collect();
        if before != after {
            let old: Vec<(usize, Option<(String, u64)>)> = b.targets.iter().filter(|f| f.t.is_ridden).map(|f| (f.t.resource, f.applied.clone())).collect();
            b.targets.retain(|f| !f.t.is_ridden);
            for t in &ridden_found {
                b.targets.push(Found { t: *t, found_ms: now, applied: old.iter().find(|o| o.0 == t.resource).and_then(|o| o.1.clone()), ..Default::default() });
            }
            b.say(format!(
                "ridden copy: {}{}",
                ridden_found.len(),
                if ridden_found.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        ridden_found
                            .iter()
                            .map(|t| format!(
                                "texture {:#x}, resource {:#x}, id {:#x}, RenderCore2 {:#x} (bind flags {}, +0xf8 {})",
                                t.texture, t.resource, t.id, t.rc2, t.rc2_bind, t.rc2_state
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                }
            ));
            targets_changed(&b);
            b.requeue(now);
        }
        b.riddens_max = b.riddens_max.max(ridden_found.len());
        // the world copies: replaced as a set when the set changed (what was applied to one that stays is kept)
        let before: Vec<usize> = b.targets.iter().filter(|f| f.t.is_worldcopy).map(|f| f.t.resource).collect();
        let after: Vec<usize> = wc_found.iter().map(|t| t.resource).collect();
        if before != after {
            let old: Vec<(usize, Option<(String, u64)>)> = b.targets.iter().filter(|f| f.t.is_worldcopy).map(|f| (f.t.resource, f.applied.clone())).collect();
            b.targets.retain(|f| !f.t.is_worldcopy);
            for t in &wc_found {
                b.targets.push(Found { t: *t, found_ms: now, applied: old.iter().find(|o| o.0 == t.resource).and_then(|o| o.1.clone()), ..Default::default() });
            }
            b.say(format!(
                "world copies: {}{}",
                wc_found.len(),
                if wc_found.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        wc_found
                            .iter()
                            .map(|t| format!(
                                "texture {:#x}, resource {:#x}, id {:#x}, hash {:#x}, RenderCore2 {:#x} (bind flags {}, +0xf8 {})",
                                t.texture, t.resource, t.id, t.hash, t.rc2, t.rc2_bind, t.rc2_state
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                }
            ));
            targets_changed(&b);
            b.requeue(now);
        }
        b.worldcopies_max = b.worldcopies_max.max(wc_found.len());
        // the finals: replaced as a set when the set changed (what was applied to one that stays is kept)
        let before: Vec<usize> = b.targets.iter().filter(|f| f.t.is_final).map(|f| f.t.resource).collect();
        let after: Vec<usize> = finals_found.iter().map(|t| t.resource).collect();
        if before != after {
            let old: Vec<(usize, Option<(String, u64)>)> = b.targets.iter().filter(|f| f.t.is_final).map(|f| (f.t.resource, f.applied.clone())).collect();
            b.targets.retain(|f| !f.t.is_final);
            for t in &finals_found {
                b.targets.push(Found { t: *t, found_ms: now, applied: old.iter().find(|o| o.0 == t.resource).and_then(|o| o.1.clone()), ..Default::default() });
            }
            b.say(format!(
                "finals: {}{}",
                finals_found.len(),
                if finals_found.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        finals_found
                            .iter()
                            .map(|t| format!(
                                "texture {:#x}, resource {:#x}, id {:#x}, hash {:#x}, RenderCore2 {:#x} (bind flags {}, +0xf8 {})",
                                t.texture, t.resource, t.id, t.hash, t.rc2, t.rc2_bind, t.rc2_state
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                }
            ));
            targets_changed(&b);
            b.requeue(now);
        }
        b.finals_max = b.finals_max.max(finals_found.len());
        let before: Vec<usize> = b.targets.iter().filter(|f| f.t.composite).map(|f| f.t.resource).collect();
        let after: Vec<usize> = found.iter().map(|t| t.resource).collect();
        if before != after {
            let old: Vec<(usize, Option<(String, u64)>)> = b.targets.iter().filter(|f| f.t.composite).map(|f| (f.t.resource, f.applied.clone())).collect();
            b.targets.retain(|f| !f.t.composite);
            for t in &found {
                b.targets.push(Found { t: *t, found_ms: now, applied: old.iter().find(|o| o.0 == t.resource).and_then(|o| o.1.clone()), ..Default::default() });
            }
            let line = format!(
                "composite: {} ({}){}",
                found.len(),
                b.last_composite_scan,
                if found.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        found
                            .iter()
                            .map(|t| format!(
                                "texture {:#x}, resource {:#x}, RenderCore2 {:#x} (bind flags {}, +0xf8 {})",
                                t.texture, t.resource, t.rc2, t.rc2_bind, t.rc2_state
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                }
            );
            b.say(line);
            targets_changed(&b);
            b.requeue(now);
        }
        b.composites_max = b.composites_max.max(found.len());
    }
    // the siblings (the equip-time copies of the texture, which the ridden board samples): the words holding the
    // main texture's pointer (a material's texture table among them), their neighbours read as runtime textures of
    // the same shape. Looked for when the main texture is found or made again, and while fewer are found than were
    // last seen (a re-equip makes new ones), every AUTO_FIND_MS; never while the main scan runs
    let main = b.targets.iter().find(|f| f.t.is_main()).map(|f| f.t);
    let siblings = b.targets.iter().filter(|f| f.t.sibling).count();
    let main_scanning = b.scan.is_some();
    if !SIBLINGS_ON.load(Ordering::Relaxed) {
        b.sibling_find_wanted = false;
    } else if b.sibling_scan.is_none() && !main_scanning && b.composite_scan.is_none() {
        if let Some(m) = main {
            // again every AUTO_FIND_MS while fewer are found than were last seen, or none ever while the deck is
            // equipped (the game reads its fx.cave.* states then: the copies exist only once it is equipped)
            let equipped = b.reads_changed.is_some_and(|t| now.saturating_sub(t) < 1500);
            let missing = siblings < b.siblings_max || (siblings == 0 && equipped);
            if b.sibling_find_wanted || (missing && now >= b.next_sibling_find) {
                b.sibling_find_wanted = false;
                match WordScan::new(&[m.texture as u64], 1 << 16) {
                    Ok(s) => {
                        b.sibling_scan = Some(s);
                        b.sibling_scan_started = now;
                    }
                    Err(e) => {
                        b.say(format!("cannot scan for siblings: {e}"));
                        b.next_sibling_find = now + AUTO_FIND_MS;
                    }
                }
            }
        } else {
            b.sibling_find_wanted = false;
        }
    }
    let done = match b.sibling_scan.as_mut() {
        Some(s) if !main_scanning => s.step(SCAN_BUDGET, SCAN_STEP_MS),
        _ => false,
    };
    if done {
        let s = b.sibling_scan.take().unwrap();
        b.sibling_scans += 1;
        b.next_sibling_find = now + AUTO_FIND_MS;
        let Some(m) = main else { return };
        let found = siblings_around(&s.found, &ids, &m, MAX_SIBLINGS);
        b.last_sibling_scan = format!(
            "{} MB in {} ms, {} words hold the texture's pointer, {} sibling(s)",
            s.bytes >> 20,
            now - b.sibling_scan_started,
            s.found.len(),
            found.len()
        );
        let before: Vec<usize> = b.targets.iter().filter(|f| f.t.sibling).map(|f| f.t.resource).collect();
        let after: Vec<usize> = found.iter().map(|t| t.resource).collect();
        if before != after {
            let old: Vec<(usize, Option<(String, u64)>)> = b.targets.iter().filter(|f| f.t.sibling).map(|f| (f.t.resource, f.applied.clone())).collect();
            b.targets.retain(|f| !f.t.sibling);
            for t in &found {
                b.targets.push(Found { t: *t, found_ms: now, applied: old.iter().find(|o| o.0 == t.resource).and_then(|o| o.1.clone()), ..Default::default() });
            }
            let line = format!(
                "siblings: {} ({}){}",
                found.len(),
                b.last_sibling_scan,
                if found.is_empty() {
                    String::new()
                } else {
                    format!(": {}", found.iter().map(|t| format!("texture {:#x}, resource {:#x}", t.texture, t.resource)).collect::<Vec<_>>().join("; "))
                }
            );
            b.say(line);
            targets_changed(&b);
            b.requeue(now);
        }
        b.siblings_max = b.siblings_max.max(found.len());
    }
}

/// The owner scan: started for a raw target waiting (one value) or for the uploads table's entries (many), stepped
/// within the tick's budget, and read out: the object around each word found that reads as a runtime texture (the
/// resource at +0xd0) or a RenderCore2 object (+0x118), the texture preferred. A raw target with an owner becomes a
/// target (its state by hand, if given, set for it); with none it is refused: the game may have released it.
fn owner_scan_tick(b: &mut Board, now: u64, ids: &Ids) {
    let busy = b.scan.is_some() || b.composite_scan.is_some() || b.sibling_scan.is_some();
    if b.owner_scan.is_none() && !busy {
        if let Some(raw) = b.raw_pending.take() {
            match WordScan::new(&[raw.resource as u64], 4096) {
                Ok(scan) => b.owner_scan = Some(OwnerScan { scan, values: vec![raw.resource], raw: Some(raw), started: now }),
                Err(e) => {
                    b.raw_last = format!("resource {:#x}: cannot scan for an owner: {e}", raw.resource);
                    let line = b.raw_last.clone();
                    b.say(line);
                }
            }
        } else if b.owners_wanted {
            b.owners_wanted = false;
            let mut values: Vec<u64> = match b.owner_values.take() {
                Some(v) => v,
                None => upload_entries().iter().chain(upload_deck_entries().iter()).map(|u| u.resource as u64).collect(),
            };
            values.sort_unstable();
            values.dedup();
            if !values.is_empty() {
                match WordScan::many(&values, 1 << 16) {
                    Ok(scan) => b.owner_scan = Some(OwnerScan { scan, values: values.iter().map(|&v| v as usize).collect(), raw: None, started: now }),
                    Err(e) => b.say(format!("cannot scan for the uploads' owners: {e}")),
                }
            }
        }
    }
    let done = match b.owner_scan.as_mut() {
        Some(o) if !busy => o.scan.step(SCAN_BUDGET, SCAN_STEP_MS),
        _ => false,
    };
    if !done {
        return;
    }
    let o = b.owner_scan.take().unwrap();
    let mut found: Vec<(usize, Option<(usize, bool)>)> = o.values.iter().map(|&r| (r, None)).collect();
    for &a in &o.scan.found {
        let Some(v) = win::read_ptr(a) else { continue };
        let Some(e) = found.iter_mut().find(|e| e.0 == v) else { continue };
        if let Some(obj) = a.checked_sub(layout::TEX_D3D12) {
            if win::read_ptr(obj) == Some(ids.texture_vtable) {
                e.1 = Some((obj, true));
                continue;
            }
        }
        if let Some(obj) = a.checked_sub(layout::RC2_D3D12) {
            if ids.rc2_vtable != 0 && win::read_ptr(obj) == Some(ids.rc2_vtable) && e.1.is_none_or(|x| !x.1) {
                e.1 = Some((obj, false));
            }
        }
    }
    let ms = now.saturating_sub(o.started);
    let mb = o.scan.bytes >> 20;
    if let Ok(mut m) = OWNERS.lock() {
        for (r, owner) in &found {
            m.retain(|e| e.0 != *r);
            if m.len() >= 512 {
                m.remove(0);
            }
            m.push((*r, *owner));
        }
    }
    match o.raw {
        Some(raw) => match found.first().and_then(|e| e.1) {
            Some((owner, texture)) => {
                let (rc2, rc2_bind, rc2_state) = if texture {
                    read_rc2(owner, raw.resource, ids)
                } else {
                    (owner, win::read_u32(owner + layout::RC2_BIND).unwrap_or(0), win::read_u32(owner + layout::RC2_STATE).unwrap_or(0))
                };
                let t = Target {
                    asset: 0,
                    texture: 0,
                    resource: raw.resource,
                    rc2,
                    rc2_bind,
                    rc2_state,
                    sibling: false,
                    composite: false,
                    id: if texture { win::read_u32(owner + layout::TEX_ID).unwrap_or(0) } else { 0 },
                    manual: true,
                    is_final: false,
                    hash: 0,
                    owner,
                    owner_texture: texture,
                    is_worldcopy: false,
                    is_ridden: false,
                    slot: Slot::Art,
                };
                if let Some(st) = raw.state {
                    b.manual_per.retain(|(f, _)| *f != ManualFor::Resource(raw.resource));
                    b.manual_per.push((ManualFor::Resource(raw.resource), st));
                }
                b.raw_last = format!(
                    "resource {:#x}: live owner: {} {:#x}{} ({} word(s) hold its value; {mb} MB in {ms} ms): added as a target{}",
                    raw.resource,
                    if texture { "runtime texture" } else { "RenderCore2 object" },
                    owner,
                    if rc2 != 0 { format!(" (RenderCore2 {rc2:#x}, bind flags {rc2_bind}, +0xf8 {rc2_state})") } else { String::new() },
                    o.scan.found.len(),
                    raw.state.map(|st| format!(", state {st:#x} set for it")).unwrap_or_default()
                );
                let line = b.raw_last.clone();
                b.say(line);
                b.targets.push(Found { t, found_ms: now, ..Default::default() });
                targets_changed(b);
                b.requeue(now);
            }
            None => {
                b.raw_last = format!("resource {:#x}: no live owner: the game may have released it ({} word(s) hold its value, none inside a runtime texture or a RenderCore2 object; {mb} MB in {ms} ms): not added", raw.resource, o.scan.found.len());
                let line = b.raw_last.clone();
                b.say(line);
            }
        },
        None => {
            b.owners_at = now;
            let (tex, rc2, none) = found.iter().fold((0, 0, 0), |(t, r, n), e| match e.1 {
                Some((_, true)) => (t + 1, r, n),
                Some((_, false)) => (t, r + 1, n),
                None => (t, r, n + 1),
            });
            b.say(format!("uploads: owners looked up for {} resource(s): {tex} named by a runtime texture, {rc2} by a RenderCore2 object only, {none} by none ({mb} MB in {ms} ms)", found.len()));
        }
    }
}

/// The targets changed: the barrier watch follows them, the composites' transitions get logged, and the render thread
/// is asked to describe them (GetDesc).
fn targets_changed(b: &Board) {
    watch(&b.targets.iter().map(|f| f.t.resource).collect::<Vec<_>>());
    let verbose: Vec<usize> = b.targets.iter().filter(|f| f.t.composite || f.t.is_final || f.t.manual).map(|f| f.t.resource).collect();
    for (k, v) in VERBOSE_RES.iter().enumerate() {
        let r = verbose.get(k).copied().unwrap_or(0);
        if v.swap(r, Ordering::AcqRel) != r {
            LAST_SEEN_STATES[k].store(0, Ordering::Release);
            LAST_SEEN_MS[k].store(0, Ordering::Release);
        }
    }
    DESCRIBE.store(true, Ordering::Release);
}

/// The trainer's `board` request, its board part: `art` (the chain, its size and digest checked by the caller) goes up
/// at the next frames; `keep` puts it back after the texture is made again (default true); `state` by hand (`auto`
/// to go back); `find` looks again now. The first request makes the board live.
#[allow(clippy::too_many_arguments)]
pub fn request(
    now: u64,
    arts_in: Vec<Art>,
    keep: Option<bool>,
    state: Option<&str>,
    state_for: Option<&str>,
    find: bool,
    verify: bool,
    siblings: Option<bool>,
    composite_common: Option<bool>,
    target: Option<&str>,
    uploads: bool,
    finals: Option<bool>,
    copies: bool,
    peek: Option<&crate::proto::PeekReq>,
    force: bool,
    owners: bool,
    slices: Option<&str>,
    slices_state: Option<&str>,
    hunt: Option<&crate::proto::HuntReq>,
    owners_of: Option<&str>,
    worldcopy: Option<bool>,
    worldcopy_hashes: Option<&[String]>,
    hunt_abort: bool,
) -> Result<(), String> {
    let wc_hashes_given: Option<Vec<u64>> = match worldcopy_hashes {
        Some(list) => Some(
            list.iter()
                .map(|h| u64::from_str_radix(h.trim().trim_start_matches("0x"), 16).map_err(|_| format!("worldcopy hash {h:?}: hex")))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    let slices_state = match slices_state {
        Some(s) => Some(parse_state(s)?),
        None => None,
    };
    let peek = match peek {
        Some(p) => {
            let hex = p.resource.trim().strip_prefix("0x").ok_or_else(|| format!("peek resource {:?}: 0x...", p.resource))?;
            let resource = usize::from_str_radix(hex, 16).map_err(|_| format!("peek resource {:?}: 0x...", p.resource))?;
            Some(Peek {
                resource,
                x: p.x,
                y: p.y,
                mip: p.mip,
                n: p.n.clamp(1, PEEK_MAX),
                stage: 0,
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                rows: 0,
                row_bytes: 0,
                format: 0,
                family: None,
                states: String::new(),
                source: String::new(),
                result: Value::Null,
            })
        }
        None => None,
    };
    let mut manual = match state {
        Some(s) => Some(parse_state(s)?),
        None => None,
    };
    let mut selector = match state_for {
        Some(s) => Some(ManualFor::parse(s)?),
        None => None,
    };
    // a target added by hand (`board target 0x...`), or every one removed (`clear`); a state given with it is for
    // that target
    let mut added: Option<Target> = None;
    let mut forced_entry: Option<Upload> = None;
    let mut raw_request: Option<RawPending> = None;
    let mut clear_manual = false;
    // `ridden on|off`: whether the ridden copy is looked for and written (default on)
    let ridden_toggle = target.map(str::trim).and_then(|t| match t {
        "ridden on" => Some(true),
        "ridden off" => Some(false),
        _ => None,
    });
    // (applied once the board is locked, below)
    let target = if ridden_toggle.is_some() { None } else { target };
    match target.map(str::trim) {
        Some("clear") => clear_manual = true,
        Some(t) => {
            // `0x...`: a runtime texture object; `resource:0x...`: a raw ID3D12Resource (no runtime texture names it)
            let (raw, hex) = match t.strip_prefix("resource:") {
                Some(h) => (true, h.trim()),
                None => (false, t),
            };
            let bad = || format!("target {t:?}: a runtime texture object's address (0x...), resource:0x... (a raw ID3D12Resource) or clear");
            let address = usize::from_str_radix(hex.strip_prefix("0x").ok_or_else(bad)?, 16).map_err(|_| bad())?;
            if raw && force {
                // no owner: the deck-shaped uploads subset must hold it (the game wrote it lately, within this equip);
                // GetDesc is checked at every upload
                let t = read_raw(address)?;
                let entry = upload_deck_entries().into_iter().find(|u| u.resource == address).ok_or_else(|| {
                    format!("resource {address:#x}: not in the deck-shaped uploads subset (board uploads --deck): the game did not write it lately, not forced")
                })?;
                if manual.is_some() && selector.is_none() {
                    selector = Some(ManualFor::Resource(t.resource));
                }
                forced_entry = Some(entry);
                added = Some(t);
            } else if raw {
                // checked now for what can be read; its owner is looked for by a scan in the ticks (the state given
                // goes with it, set once it is a target)
                read_raw(address)?;
                raw_request = Some(RawPending { resource: address, state: manual.take().flatten(), since: now });
            } else {
                let ids = board().ids.ok_or("the board is not usable yet (no ids)")?;
                let t = read_manual(address, &ids)?;
                if manual.is_some() && selector.is_none() {
                    selector = Some(ManualFor::Resource(t.resource));
                }
                added = Some(t);
            }
        }
        None => {}
    }
    for a in &arts_in {
        // every slot's first chain is the full BC7 chain of The Caveman's shape (the graphic's sRGB, a layer's linear:
        // the same bytes), the digest the status reports; the other chains any shape and format the board writes
        let bc7 = a.chains.first().filter(|c| c.format == ChainFormat::Bc7 && c.levels == LEVELS && c.width == WIDTH && c.height == HEIGHT);
        if bc7.is_none_or(|c| c.bytes.len() != chain_len()) {
            return Err(format!("the {} art's first chain must be The Caveman's BC7 chain of {} bytes (512 x 2048, 12 levels)", a.slot.name(), chain_len()));
        }
        for c in &a.chains {
            if c.bytes.len() != chain_len_for(c.format, c.width, c.height, c.levels) {
                return Err(format!(
                    "a {} chain of {}x{} with {} levels is {} bytes, not {}",
                    c.format.name(),
                    c.width,
                    c.height,
                    c.levels,
                    chain_len_for(c.format, c.width, c.height, c.levels),
                    c.bytes.len()
                ));
            }
        }
    }
    let mut b = board();
    ACTIVE.store(true, Ordering::Release);
    if let Some(on) = ridden_toggle {
        let was = RIDDEN_ON.swap(on, Ordering::AcqRel);
        if !on {
            let n = b.targets.iter().filter(|f| f.t.is_ridden).count();
            b.targets.retain(|f| !f.t.is_ridden);
            if n > 0 {
                targets_changed(&b);
            }
        } else if !was {
            b.composite_find_wanted = true;
        }
        b.say(format!(
            "ridden copy: {}",
            if on { "looked for and written (the nameless sRGB RGBA8 deck-shaped texture the in-world board samples)" } else { "not written" }
        ));
    }
    if uploads {
        DESCRIBE_RING.store(true, Ordering::Release);
    }
    if owners {
        b.owners_wanted = true;
        b.owner_values = match owners_of {
            Some("hunt") => {
                let mut v: Vec<u64> =
                    b.hunt.as_ref().map(|h| h.items.iter().filter(|i| i.hits.iter().any(|&x| x)).map(|i| i.resource as u64).collect()).unwrap_or_default();
                v.sort_unstable();
                v.dedup();
                if v.is_empty() {
                    return Err("no hunt hits to look up the owners of (board hunt first)".into());
                }
                Some(v)
            }
            _ => None,
        };
    }
    if let Some(hq) = hunt {
        if b.hunt.as_ref().is_some_and(|h| h.stage != 2) {
            return Err("a hunt is running: wait for it".into());
        }
        let present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
        let assume = match hq.assume.as_deref() {
            Some(a) => parse_state(a)?.filter(|st| usable_state(*st) || *st == 0).ok_or_else(|| format!("--assume {a}: srv, common or a usable state"))?.into(),
            None => None,
        };
        if hq.census {
            // the census: a scan for every runtime texture object first (seconds, in the ticks), the hunt when it is
            // done (census_tick); the old hunt goes so the trainer does not read its result as this one's
            if b.census_req.is_some() {
                return Err("a census is running: wait for it".into());
            }
            let ids = b.ids.ok_or("the game's texture vtable is not known yet (the board must be live)")?;
            let scan = WordScan::new(&[ids.texture_vtable as u64], 1 << 20)?;
            b.hunt = None;
            b.census_scan = Some(scan);
            b.census_req = Some((hq.clone(), assume));
            b.census_started = now;
            b.say(format!(
                "census: scanning the game's memory for every runtime texture object (vtable {:#x}), for a hunt for #{:02x}{:02x}{:02x}",
                ids.texture_vtable, hq.rgb[0], hq.rgb[1], hq.rgb[2]
            ));
            return Ok(());
        }
        let items = hunt_items(&b, hq.everything, assume, present);
        if items.is_empty() {
            return Err(format!("no texture to read: none described since the main was last made (present {}; the board must be live during the equip; --everything reads every texture described, --census every runtime texture object)", b.main_made_at));
        }
        let line = format!(
            "hunt: #{:02x}{:02x}{:02x} (linear {:?}) within {} over {} texture slice(s){}",
            hq.rgb[0],
            hq.rgb[1],
            hq.rgb[2],
            hq.rgb.map(srgb_to_linear),
            hq.tolerance.map(|t| t.to_string()).unwrap_or_else(|| "24 (32 for BC1/BC3)".into()),
            items.len(),
            if hq.everything { " (everything described)" } else { "" }
        );
        b.say(line);
        b.hunt = Some(Hunt {
            rgb: hq.rgb,
            tolerance: hq.tolerance,
            assume,
            items,
            next: 0,
            batch: 0,
            stage: 0,
            started: now,
            done_ms: 0,
            unknown: 0,
            progress_ms: win::now_ms(),
            fence_value: 0,
            stuck: None,
            census: false,
            held: Vec::new(),
        });
        HUNTING.store(true, Ordering::Release);
    }
    if hunt_abort {
        let census = b.census_req.take().is_some();
        b.census_scan = None;
        let was = b.hunt.take();
        HUNTING.store(false, Ordering::Release);
        if let Some(mut h) = was {
            // a batch on the GPU: wait for its fence (at most 2 s) before our references on its resources go; if it
            // never comes, they are never released
            if h.stage == 1 && !h.held.is_empty() {
                let mut done = false;
                if let Ok(g) = GPU.lock() {
                    if let Some(gpu) = g.as_ref() {
                        let t0 = win::now_ms();
                        // SAFETY: our own fence
                        while unsafe { gpu.fence.GetCompletedValue() } < h.fence_value && win::now_ms().saturating_sub(t0) < 2000 {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        done = unsafe { gpu.fence.GetCompletedValue() } >= h.fence_value;
                    }
                }
                if !done {
                    for r in h.held.drain(..) {
                        std::mem::forget(r);
                    }
                }
            }
            b.say(format!("hunt aborted at stage {}, item {} of {} ({} read)", h.stage, h.next, h.items.len(), h.items.iter().filter(|i| i.read).count()));
        } else if census {
            b.say("census aborted".into());
        } else {
            b.say("no hunt to abort".into());
        }
    }
    if copies {
        DESCRIBE_RING.store(true, Ordering::Release);
    }
    if let Some(st) = slices_state {
        b.slices_state = st;
        b.say(match st {
            Some(v) => format!("slices: the tile arrays' state by hand: {v:#x}"),
            None => "slices: the tile arrays' state from the uploads table again".into(),
        });
    }
    match slices.map(str::trim) {
        Some("match") => {
            if b.slice_job.is_some() {
                return Err("a slice job is running: wait for it".into());
            }
            if !b.art.as_ref().is_some_and(|a| a.chains.first().is_some_and(|c| c.format == ChainFormat::Bc7)) {
                return Err("no art to compare with: board deck ... (or board art, board fill) first".into());
            }
            let items = slice_candidates(&b);
            if items.is_empty() {
                return Err(format!(
                    "no BC7 tile array slice written since the main was last made (present {}): re-equip The Caveman, then board slices",
                    b.main_made_at
                ));
            }
            let line = format!("slices: reading back the corners of {} slice(s) written since present {}", items.len(), b.main_made_at);
            b.say(line);
            b.slice_job = Some(SliceJob { write: false, stage: 0, items, tiles: Vec::new(), value: 0, started: now });
            SLICES.store(true, Ordering::Release);
        }
        Some("write") => {
            start_tiles(&mut b, now)?;
            let line = format!("tiles: writing our art's tiles into {} matched slice(s)", b.slice_matches.len());
            b.say(line);
        }
        Some("clear") => {
            b.slice_matches.clear();
            b.tiles_keep = false;
            b.slices_result = Value::Null;
            b.say("tiles: the matched slices forgotten, nothing written again".into());
        }
        Some(other) => return Err(format!("slices {other:?}: match, write or clear")),
        None => {}
    }
    if let Some(p) = peek {
        if PEEK.load(Ordering::Acquire) {
            return Err("a peek is being read already: wait for it".into());
        }
        if !b.targets.iter().any(|f| f.t.resource == p.resource) {
            return Err(format!("resource {:#x} is not a target (board target first)", p.resource));
        }
        b.peek = Some(p);
        PEEK.store(true, Ordering::Release);
    }
    if let Some(raw) = raw_request {
        if let Some(f) = b.targets.iter().find(|f| f.t.resource == raw.resource) {
            return Err(format!("resource {:#x} is already a target ({})", raw.resource, f.t.kind()));
        }
        if b.raw_pending.is_some() || b.owner_scan.as_ref().is_some_and(|o| o.raw.is_some()) {
            return Err("a raw target is being checked already: wait for it (board status: raw_pending)".into());
        }
        b.raw_last = format!("resource {:#x}: looking for a live owner (a scan of the process's memory for the pointer value)", raw.resource);
        let line = b.raw_last.clone();
        b.say(line);
        b.raw_pending = Some(raw);
    }
    if clear_manual {
        let gone: Vec<usize> = b.targets.iter().filter(|f| f.t.manual).map(|f| f.t.resource).collect();
        if !gone.is_empty() {
            b.targets.retain(|f| !f.t.manual);
            b.manual_per.retain(|(f, _)| !matches!(f, ManualFor::Resource(r) if gone.contains(r)));
            targets_changed(&b);
            b.say(format!("{} target(s) added by hand removed", gone.len()));
        }
    }
    if let Some(t) = added {
        if let Some(f) = b.targets.iter().find(|f| f.t.resource == t.resource) {
            return Err(format!("resource {:#x} is already a target ({})", t.resource, f.t.kind()));
        }
        match &forced_entry {
            Some(u) => b.say(format!(
                "target FORCED by hand: raw resource {:#x} with no owner (the game ended a write on it {} present(s) ago, {:#x} -> {:#x}; a re-equip drops it)",
                t.resource,
                crate::depth::PRESENT_NO.load(Ordering::Relaxed).saturating_sub(u.last),
                u.before,
                u.after
            )),
            None => b.say(format!(
                "target added by hand: texture {:#x}, resource {:#x}, id {:#x} (RenderCore2 {:#x}, bind flags {}, +0xf8 {})",
                t.texture, t.resource, t.id, t.rc2, t.rc2_bind, t.rc2_state
            )),
        }
        b.targets.push(Found { t, found_ms: now, ..Default::default() });
        targets_changed(&b);
    }
    if let Some(on) = siblings {
        let was = SIBLINGS_ON.swap(on, Ordering::AcqRel);
        if was != on {
            b.say(format!("siblings {}", if on { "on: the same-shaped textures next to the main one are written too" } else { "off" }));
            if !on && b.targets.iter().any(|f| f.t.sibling) {
                b.targets.retain(|f| !f.t.sibling);
                targets_changed(&b);
            }
            b.sibling_find_wanted = on;
        }
    }
    match (manual, selector) {
        (Some(v), Some(sel)) => {
            // for one resource or one kind: set (auto: cleared), the others untouched
            b.manual_per.retain(|(f, _)| *f != sel);
            let what = match &sel {
                ManualFor::Resource(r) => format!("resource {r:#x}"),
                ManualFor::Kind(k) => format!("every {k}"),
            };
            match v {
                Some(v) => {
                    b.manual_per.push((sel, v));
                    b.say(format!("state set by hand for {what}: {v:#x}"));
                }
                None => b.say(format!("state for {what}: automatic again")),
            }
        }
        (Some(v), None) => {
            b.manual = v;
            if v.is_none() {
                b.manual_per.clear();
            }
            b.say(match v {
                Some(v) => format!("state set by hand: {v:#x}"),
                None => "state: automatic again (for every target)".into(),
            });
        }
        (None, Some(_)) => return Err("state_for needs a state".into()),
        (None, None) => {}
    }
    if let Some(on) = composite_common {
        let was = COMPOSITE_COMMON.swap(on, Ordering::AcqRel);
        if was != on {
            b.say(format!("composite-common {}", if on { "on: a composite that never shows a barrier is written from COMMON" } else { "off" }));
        }
    }
    if let Some(list) = wc_hashes_given {
        b.worldcopy_hashes = list;
        let shown = b.worldcopy_hashes_list();
        b.say(format!("world copy: the name hashes looked for: {}", shown.iter().map(|h| format!("{h:#x}")).collect::<Vec<_>>().join(", ")));
        if b.targets.iter().any(|f| f.t.is_worldcopy) {
            b.targets.retain(|f| !f.t.is_worldcopy);
            targets_changed(&b);
        }
        b.worldcopies_max = 0;
        b.composite_find_wanted = !b.targets.is_empty();
    }
    if let Some(on) = worldcopy {
        let was = WORLDCOPY_ON.swap(on, Ordering::AcqRel);
        if was != on {
            b.say(format!(
                "worldcopy {}",
                if on { "on: the BC1 texture the game re-fills at every equip is written too (with the art's BC1 chain)" } else { "off" }
            ));
            if !on && b.targets.iter().any(|f| f.t.is_worldcopy) {
                b.targets.retain(|f| !f.t.is_worldcopy);
                targets_changed(&b);
            }
            b.worldcopies_max = 0;
            b.composite_find_wanted = on && !b.targets.is_empty();
        }
    }
    if let Some(on) = finals {
        let was = FINALS_ON.swap(on, Ordering::AcqRel);
        if was != on {
            b.say(format!("finals {}", if on { "on: the run-time-made copies with the known name hashes are written too" } else { "off" }));
            if !on && b.targets.iter().any(|f| f.t.is_final) {
                b.targets.retain(|f| !f.t.is_final);
                targets_changed(&b);
            }
            b.finals_max = 0;
            b.composite_find_wanted = on && !b.targets.is_empty();
        }
    }
    if find {
        // by hand: what was remembered of resources no longer watched is dropped, and the identity test runs for a
        // minute (every write the game ends on any 512 x 2048 RGBA8 texture is logged)
        forget_all_observed();
        IDENTITY_UNTIL_MS.store(now + IDENTITY_MS, Ordering::Release);
    }
    if find || (b.targets.is_empty() && b.scan.is_none()) {
        b.find_now = true;
    }
    if let Some(k) = keep {
        b.drop_art = !k;
    }
    for art in arts_in {
        let art = Arc::new(art);
        let line = format!(
            "art {:?} ({}) for the {} queued{}",
            art.label,
            &art.sha256[..12.min(art.sha256.len())],
            art.slot.name(),
            if b.keep() { ", kept for a texture made again" } else { "" }
        );
        b.say(line);
        match art.slot {
            Slot::Art => b.art = Some(art.clone()),
            s => b.layer_arts[s.index() - 1] = Some(art.clone()),
        }
        b.job = Some(art);
        b.job_since = now;
        b.last_error.clear();
        PENDING.store(true, Ordering::Release);
    }
    if verify {
        if b.verify_targets.is_empty() {
            return Err("nothing written yet to read back (board art or board deck first)".into());
        }
        if b.verify_stage == 1 || b.verify_stage == 3 {
            return Err("a read-back is on its way: ask again in a moment".into());
        }
        b.verify_stage = 2;
        b.verify_due = now;
        for t in b.verify_targets.iter_mut() {
            t.later = "not yet".into();
        }
        VERIFY.store(true, Ordering::Release);
    }
    Ok(())
}

/// What a target's blocks read back say against the art sent: "match", or where they first differ, told apart:
/// right after the write, all zeros means the copy did not take; later, a difference after a match means the game
/// rendered it again.
fn verify_verdict(got: &[Vec<u8>; 2], expect: &[Vec<u8>; 2], mips: usize, later: bool, matched_now: bool) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    for (k, &m) in VERIFY_MIPS.iter().enumerate() {
        if m >= mips {
            continue;
        }
        let n = expect[k].len().min(got[k].len());
        if let Some(i) = (0..n).find(|&i| got[k][i] != expect[k][i]) {
            let what = if !later && got[k][..n].iter().all(|&x| x == 0) {
                "ZEROS right after the write (the copy did not take)"
            } else if later && matched_now {
                "changed later (the game rendered it again)"
            } else {
                "mismatch"
            };
            return format!("{what} at mip {m} byte {i}: read {} expected {}", hex(&got[k][..n]), hex(&expect[k][..n]));
        }
    }
    "match".into()
}

/// The state a target is written in: for the asset's texture and the siblings as [`decide`]; for the composite never
/// inferred: the game's own barriers on it (seen through the watch) or a state set by hand, else unknown (or, with
/// `assume_common`, COMMON: a composite that never showed a barrier while another did, when `board composite-common
/// on` is in force).
pub fn decide_for(manual: Option<u32>, seen: &Observed, rc2: Option<(u32, u32)>, composite: bool) -> Result<([u32; LEVELS], Source), String> {
    decide_composite(manual, seen, rc2, composite, false)
}

/// Whether a target's state is never inferred from RenderCore2: the composites, and a target added by hand whose
/// DXGI format is a composite's (RGBA8 or BGRA8 family). A final, a sibling, the main and a BC7 target added by hand
/// (a shader-resource texture whose RenderCore2 object says bind flags 8 and +0xf8 8) take the main's rule.
pub fn never_inferred(t: &Target, dxgi: i32) -> bool {
    t.composite || (t.manual && composite_format_ok(dxgi))
}

/// The state of a target from a raw resource pointer: the game's barriers seen on it since it became a target, else
/// the state the uploads table last saw the game leave it in (the `after` of its last write ended: COPY_DEST ->
/// 0xc0 for the equip-time results), else one given by hand, else unknown.
pub fn decide_raw(manual: Option<u32>, seen: &Observed, last_write: Option<u32>) -> Result<([u32; LEVELS], Source), String> {
    if seen.states.iter().any(Option::is_some) {
        let mut out = [0u32; LEVELS];
        for (m, s) in seen.states.iter().enumerate() {
            out[m] = s.ok_or_else(|| format!("the game's barriers set some mips but not mip {m}: state unknown"))?;
            if !usable_state(out[m]) {
                return Err(format!("the game last put mip {m} in state {:#x} (it is writing it?): not touched", out[m]));
            }
        }
        return Ok((out, Source::Observed));
    }
    if let Some(a) = last_write {
        if !usable_state(a) {
            return Err(format!("the last write the game ended on it left it in state {a:#x} (it is writing it?): not touched"));
        }
        return Ok(([a; LEVELS], Source::LastWrite));
    }
    if let Some(s) = manual {
        return Ok(([s; LEVELS], Source::Manual));
    }
    Err("state unknown: no write the game ended on it is in the uploads table (board uploads), and none was given (board target --resource 0x... --state srv)"
        .into())
}

/// The state to write a target in, whatever its kind (`dxgi`: its format when described).
fn decide_target(b: &Board, f: &Found, dxgi: i32, described: bool) -> Result<([u32; LEVELS], Source), String> {
    let t = &f.t;
    if t.is_raw() {
        return decide_raw(b.manual_for(t), &observed(t.resource), last_write_after(t.resource));
    }
    let rc2 = (t.rc2 != 0).then_some((t.rc2_bind, t.rc2_state));
    decide_composite(b.manual_for(t), &observed(t.resource), rc2, t.composite || (described && never_inferred(t, dxgi)), b.assume_common(f))
}

pub fn decide_composite(
    manual: Option<u32>,
    seen: &Observed,
    rc2: Option<(u32, u32)>,
    composite: bool,
    assume_common: bool,
) -> Result<([u32; LEVELS], Source), String> {
    if !composite {
        return decide(manual, seen, rc2);
    }
    if let Some(s) = manual {
        return Ok(([s; LEVELS], Source::Manual));
    }
    if assume_common && !seen.states.iter().any(Option::is_some) {
        return Ok(([0; LEVELS], Source::AssumedCommon));
    }
    if seen.states.iter().any(Option::is_some) {
        let mut out = [0u32; LEVELS];
        for (m, s) in seen.states.iter().enumerate() {
            // (a level never named by a barrier takes the state of the whole resource's last transition, which the
            // watch notes on every level; none at all: unknown)
            out[m] = s.ok_or_else(|| format!("the game's barriers set some mips but not mip {m}: state unknown"))?;
            if !usable_state(out[m]) {
                return Err(format!("the game last put mip {m} in state {:#x} (it is writing it?): not touched", out[m]));
            }
        }
        return Ok((out, Source::Observed));
    }
    Err(format!(
        "state unknown: re-equip the deck once so the game's barriers on it are seen (RenderCore2 says bind flags {} and {}; never inferred for the composite)",
        rc2.map_or(0, |r| r.0),
        rc2.map_or(0, |r| r.1)
    ))
}

/// The finals for the status: how many, how many were last found, the hashes looked for.
fn finals_status(b: &Board) -> Value {
    json!({
        "found": b.targets.iter().filter(|f| f.t.is_final).count(),
        "max": b.finals_max,
        "hashes": FINAL_HASHES.iter().map(|h| format!("{h:#x}")).collect::<Vec<_>>(),
        "per_hash": FINALS_PER_HASH,
    })
}

/// The check's state for the status.
fn verify_status(b: &Board) -> Value {
    if b.verify_targets.is_empty() {
        return Value::Null;
    }
    json!({
        "art": b.verify_label,
        "stage": match b.verify_stage { 1 => "reading after the upload", 2 => "waiting for the later read", 3 => "reading later", _ => "done" },
        "now_at_ms": b.verify_now_at,
        "later_at_ms": b.verify_later_at,
        "targets": b.verify_targets.iter().map(|t| json!({"resource": format!("{:#x}", t.resource), "verified_now": t.now, "verified_later": t.later})).collect::<Vec<_>>(),
    })
}

/// A step of the read-back check, at a present (see `Board::verify_stage`): reads the blocks once their copy ran and
/// compares them; records the later read when it is due.
unsafe fn verify_present(device: &ID3D12Device, queue: &ID3D12CommandQueue) {
    let Ok(mut b) = cell().try_lock() else { return };
    let Ok(mut g) = GPU.try_lock() else { return };
    let Some(gpu) = g.as_mut() else {
        VERIFY.store(false, Ordering::Release);
        return;
    };
    if gpu.device.as_raw() != device.as_raw() {
        VERIFY.store(false, Ordering::Release);
        return;
    }
    let now = win::now_ms();
    match b.verify_stage {
        1 | 3 => {
            if gpu.fence.GetCompletedValue() < gpu.verify_value {
                return; // the copy has not run yet
            }
            let later = b.verify_stage == 3;
            for (i, t) in b.verify_targets.iter_mut().enumerate() {
                let matched_now = t.now == "match";
                let pending = if later { &mut t.later } else { &mut t.now };
                if *pending != "not yet" {
                    continue; // a target gone before the later read keeps its note
                }
                let got = gpu.read_verify(i, t.rows);
                *pending = verify_verdict(&got, &t.expect, t.mips, later, matched_now);
            }
            if !later {
                b.verify_now_at = now;
                b.verify_stage = 2;
                b.verify_due = now + 2000;
                VERIFY.store(false, Ordering::Release);
            } else {
                b.verify_later_at = now;
                b.verify_stage = 0;
                VERIFY.store(false, Ordering::Release);
                let line = format!(
                    "read-back of {:?}: right after the write {}; later {}",
                    b.verify_label,
                    b.verify_targets.iter().map(|t| t.now.clone()).collect::<Vec<_>>().join(", "),
                    b.verify_targets.iter().map(|t| t.later.clone()).collect::<Vec<_>>().join(", ")
                );
                b.say_later(line);
            }
        }
        2 => {
            if now < b.verify_due || gpu.busy() {
                return;
            }
            if b.verify_targets.is_empty() {
                b.verify_stage = 0;
                VERIFY.store(false, Ordering::Release);
                return;
            }
            if gpu.allocator.Reset().is_err() || gpu.list.Reset(&gpu.allocator, None::<&ID3D12PipelineState>).is_err() {
                return;
            }
            // each texture must still be a target (the same resource behind The Caveman's asset, or the composite)
            let present: Vec<bool> = b.verify_targets.iter().map(|t| b.targets.iter().any(|f| f.t.resource == t.resource)).collect();
            let mut any = false;
            RECORDING.with(|r| r.set(true));
            for (i, t) in b.verify_targets.iter_mut().enumerate() {
                if !present[i] {
                    t.later = "the texture is gone (made again?)".into();
                    continue;
                }
                let ptr = t.resource as *mut c_void;
                let Some(res) = ID3D12Resource::from_raw_borrowed(&ptr) else { continue };
                let mut to = t.states;
                for &m in VERIFY_MIPS.iter().filter(|&&m| m < t.mips) {
                    to[m] = COPY_SOURCE;
                }
                barriers(&gpu.list, &transitions_n(res, &t.states, &to, t.mips));
                gpu.record_verify_copies(res, DXGI_FORMAT(t.format), i, t.rows, t.mips);
                barriers(&gpu.list, &transitions_n(res, &to, &t.states, t.mips));
                any = true;
            }
            RECORDING.with(|r| r.set(false));
            if gpu.list.Close().is_err() {
                return;
            }
            if !any {
                b.verify_stage = 0;
                VERIFY.store(false, Ordering::Release);
                return;
            }
            let Ok(as_list) = gpu.list.cast::<ID3D12CommandList>() else { return };
            queue.ExecuteCommandLists(&[Some(as_list)]);
            gpu.value += 1;
            if queue.Signal(&gpu.fence, gpu.value).is_err() {
                gpu.value -= 1;
                return;
            }
            gpu.verify_value = gpu.value;
            b.verify_stage = 3;
        }
        _ => VERIFY.store(false, Ordering::Release),
    }
}

/// `board peek` on the render thread: records the box copy (stage 0), then reads it back once the fence passed
/// (stage 1), the result left for the status.
unsafe fn peek_present(device: &ID3D12Device, queue: &ID3D12CommandQueue) {
    let Ok(mut b) = cell().try_lock() else { return };
    let Ok(mut g) = GPU.try_lock() else { return };
    if g.is_none() {
        match Gpu::new(device) {
            Ok(x) => *g = Some(x),
            Err(e) => {
                if let Some(p) = b.peek.as_mut() {
                    p.result = json!({"error": format!("cannot make our GPU objects: {e}")});
                    p.stage = 2;
                }
                PEEK.store(false, Ordering::Release);
                return;
            }
        }
    }
    let gpu = g.as_mut().unwrap();
    if gpu.device.as_raw() != device.as_raw() {
        PEEK.store(false, Ordering::Release);
        return;
    }
    let ids = b.ids;
    let Some(stage) = b.peek.as_ref().map(|p| p.stage) else {
        PEEK.store(false, Ordering::Release);
        return;
    };
    let fail = |b: &mut Board, why: String| {
        if let Some(p) = b.peek.as_mut() {
            p.result = json!({"error": why});
            p.stage = 2;
        }
        PEEK.store(false, Ordering::Release);
    };
    match stage {
        0 => {
            if gpu.busy() {
                return;
            }
            let resource = b.peek.as_ref().map_or(0, |p| p.resource);
            let Some(f) = b.targets.iter().find(|f| f.t.resource == resource) else {
                return fail(&mut b, format!("resource {resource:#x} is not a target (board target first)"));
            };
            if f.t.is_raw() && !ids.is_some_and(|ids| owner_alive(&f.t, &ids)) {
                return fail(&mut b, format!("resource {resource:#x}: no live owner: the game may have released it: not read"));
            }
            // (a dead mark refuses only a target nothing owns, as the upload does: an owned target's resource was
            // validated this tick, and a new resource at a dead one's address is alive again)
            if dead(resource) && f.t.is_raw() && f.t.owner == 0 {
                return fail(&mut b, format!("resource {resource:#x}: released by the game (its last Release seen): not read"));
            }
            let (states, source) = match decide_target(&b, f, f.dxgi_format, f.described) {
                Ok(x) => x,
                Err(e) => return fail(&mut b, format!("resource {resource:#x}: state unknown: {e}")),
            };
            let vt = win::read_ptr(resource).unwrap_or(0);
            if vt == 0 || !gpu.resource_vtables.contains(&vt) {
                return fail(&mut b, format!("resource {resource:#x}: its vtable {vt:#x} is not a D3D12 resource's"));
            }
            let raw = resource as *mut c_void;
            let Some(res) = ID3D12Resource::from_raw_borrowed(&raw) else { return fail(&mut b, "not a resource".into()) };
            let res = res.clone();
            let d = res.GetDesc();
            if f.described && (d.Format.0 != f.dxgi_format || d.MipLevels != f.mips) {
                return fail(&mut b, format!("resource {resource:#x}: GetDesc changed since it was described: released and reused? not read"));
            }
            let family = ChainFormat::family_of(d.Format.0);
            let bc3 = (76..=78).contains(&d.Format.0);
            let p = b.peek.as_mut().unwrap();
            if p.mip as u16 >= d.MipLevels {
                let m = p.mip;
                return fail(&mut b, format!("mip {m}: the texture has {} mips", d.MipLevels));
            }
            let (mw, mh) = (((d.Width as u32) >> p.mip).max(1), (d.Height >> p.mip).max(1));
            let n = p.n.clamp(1, PEEK_MAX);
            let (mut left, mut top, mut n_w, mut n_h) = (p.x.min(mw - 1), p.y.min(mh - 1), n, n);
            if matches!(family, Some(ChainFormat::Bc7) | Some(ChainFormat::Bc1)) || bc3 {
                // whole 4 x 4 blocks (or the mip's edge)
                left &= !3;
                top &= !3;
                n_w = n_w.div_ceil(4) * 4;
                n_h = n_h.div_ceil(4) * 4;
            }
            let right = (left + n_w).min(mw);
            let bottom = (top + n_h).min(mh);
            let (rows, row_bytes) = match family {
                Some(ChainFormat::Bc7) => (((bottom - top) as usize).div_ceil(4), ((right - left) as usize).div_ceil(4) * 16),
                Some(ChainFormat::Bc1) => (((bottom - top) as usize).div_ceil(4), ((right - left) as usize).div_ceil(4) * 8),
                _ if bc3 => (((bottom - top) as usize).div_ceil(4), ((right - left) as usize).div_ceil(4) * 16),
                _ => ((bottom - top) as usize, (right - left) as usize * 4),
            };
            if (family.is_none() && !bc3) || row_bytes > 256 || rows > 16 {
                return fail(
                    &mut b,
                    format!("resource {resource:#x}: format {} is none the board reads (BC7, BC1, BC3, RGBA8, BGRA8), or the block is too big", d.Format.0),
                );
            }
            if gpu.allocator.Reset().is_err() || gpu.list.Reset(&gpu.allocator, None::<&ID3D12PipelineState>).is_err() {
                return fail(&mut b, "our command list did not reset".into());
            }
            let mip = p.mip as usize;
            let mut to = states;
            to[mip] = COPY_SOURCE;
            let mips = d.MipLevels as usize;
            RECORDING.with(|r| r.set(true));
            barriers(&gpu.list, &transitions_n(&res, &states, &to, mips));
            let dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&gpu.peek_readback),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: d.Format, Width: right - left, Height: bottom - top, Depth: 1, RowPitch: 256 },
                    },
                },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&res),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: p.mip },
            };
            let bx = D3D12_BOX { left, top, front: 0, right, bottom, back: 1 };
            gpu.list.CopyTextureRegion(&dst, 0, 0, 0, &from, Some(&bx));
            barriers(&gpu.list, &transitions_n(&res, &to, &states, mips));
            RECORDING.with(|r| r.set(false));
            if gpu.list.Close().is_err() {
                return fail(&mut b, "our command list did not close".into());
            }
            let Ok(as_list) = gpu.list.cast::<ID3D12CommandList>() else { return fail(&mut b, "our command list".into()) };
            queue.ExecuteCommandLists(&[Some(as_list)]);
            gpu.value += 1;
            if queue.Signal(&gpu.fence, gpu.value).is_err() {
                gpu.value -= 1;
                return fail(&mut b, "the fence did not signal".into());
            }
            gpu.peek_value = gpu.value;
            let p = b.peek.as_mut().unwrap();
            p.left = left;
            p.top = top;
            p.width = right - left;
            p.height = bottom - top;
            p.rows = rows;
            p.row_bytes = row_bytes;
            p.format = d.Format.0;
            p.family = family;
            p.states = states_text(&states);
            p.source = source.name().to_string();
            p.stage = 1;
        }
        1 => {
            if gpu.fence.GetCompletedValue() < gpu.peek_value {
                return;
            }
            let p = b.peek.as_mut().unwrap();
            let mut bytes = vec![0u8; p.rows * p.row_bytes];
            for y in 0..p.rows {
                std::ptr::copy_nonoverlapping(gpu.peek_map.add(y * 256), bytes[y * p.row_bytes..].as_mut_ptr(), p.row_bytes);
            }
            let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
            let bc3 = (76..=78).contains(&p.format);
            let block = match p.family {
                Some(ChainFormat::Bc7) => 16,
                Some(ChainFormat::Bc1) => 8,
                _ if bc3 => 16,
                _ => 0,
            };
            let texels: Vec<Vec<String>> = if block > 0 {
                bytes.chunks(p.row_bytes).map(|row| row.chunks(block).map(hex).collect()).collect()
            } else {
                bytes.chunks(p.row_bytes).map(|row| row.chunks(4).map(hex).collect()).collect()
            };
            // the blocks decoded to texels (4 rows a block row), for BC formats
            let decoded: Vec<Vec<String>> = if block > 0 {
                let mut rows_out: Vec<Vec<String>> = vec![Vec::new(); p.rows * 4];
                for (br, row) in bytes.chunks(p.row_bytes).enumerate() {
                    for blk in row.chunks(block) {
                        let px = match p.family {
                            Some(ChainFormat::Bc7) => crate::bc7::decode_block(blk.try_into().unwrap()),
                            Some(ChainFormat::Bc1) => crate::bc7::decode_bc1(blk.try_into().unwrap()),
                            _ => crate::bc7::decode_bc3(blk.try_into().unwrap()),
                        };
                        for y in 0..4 {
                            for x in 0..4 {
                                rows_out[br * 4 + y].push(hex(&px[y * 4 + x]));
                            }
                        }
                    }
                }
                rows_out
            } else {
                Vec::new()
            };
            p.result = json!({
                "resource": format!("{:#x}", p.resource),
                "mip": p.mip,
                "left": p.left,
                "top": p.top,
                "width": p.width,
                "height": p.height,
                "dxgi_format": p.format,
                "family": p.family.map(|f| f.name()).or(if bc3 { Some("bc3") } else { None }),
                "bytes_per_texel_or_block": block.max(4),
                "decoded": decoded,
                "state": p.states,
                "source": p.source,
                "texels": texels,
            });
            p.stage = 2;
            PEEK.store(false, Ordering::Release);
            let line = format!("peek of {:#x} mip {} at ({}, {}) {}x{}: {} row(s) read back", p.resource, p.mip, p.left, p.top, p.width, p.height, p.rows);
            b.say_later(line);
        }
        _ => PEEK.store(false, Ordering::Release),
    }
}

/// The board's state for the trainer (and the app).
pub fn status(now: u64) -> Value {
    let b = board();
    let targets: Vec<Value> = b
        .targets
        .iter()
        .map(|f| {
            let seen = observed(f.t.resource);
            let state = match decide_target(&b, f, f.dxgi_format, f.described) {
                Ok((s, src)) => json!({"ok": true, "states": states_text(&s), "source": src.name()}),
                Err(e) => json!({"ok": false, "error": e}),
            };
            json!({
                "kind": f.t.kind(),
                "id": format!("{:#x}", f.t.id),
                "hash": if f.t.is_final || f.t.is_worldcopy { json!(format!("{:#x}", f.t.hash)) } else { Value::Null },
                "written_present": if f.written_present == 0 { Value::Null } else { json!(f.written_present) },
                "refill_present": if f.refill_present == 0 { Value::Null } else { json!(f.refill_present) },
                "last_seen": last_seen(f.t.resource, now),
                "dxgi_format": if f.described { json!(f.dxgi_format) } else { Value::Null },
                "mips": if f.described { json!(f.mips) } else { Value::Null },
                "width": if f.described { json!(f.width) } else { Value::Null },
                "height": if f.described { json!(f.height) } else { Value::Null },
                "refused": f.refused,
                "note": f.note,
                "manual_state": b.manual_for(&f.t).map(|v| format!("{v:#x}")),
                "asset": if f.t.is_raw() { Value::Null } else { json!(format!("{:#x}", f.t.asset)) },
                "texture": if f.t.is_raw() { Value::Null } else { json!(format!("{:#x}", f.t.texture)) },
                "resource": format!("{:#x}", f.t.resource),
                "rc2": if f.t.is_raw() { Value::Null } else { json!(format!("{:#x}", f.t.rc2)) },
                "raw": f.t.is_raw(),
                "owner": if f.t.is_raw() && f.t.owner != 0 { json!({"kind": if f.t.owner_texture { "runtime texture" } else { "RenderCore2" }, "address": format!("{:#x}", f.t.owner), "alive": b.ids.is_some_and(|ids| owner_alive(&f.t, &ids))}) } else { Value::Null },
                "forced": f.t.is_raw() && f.t.owner == 0,
                "rc2_bind": f.t.rc2_bind,
                "rc2_state": f.t.rc2_state,
                "found_ms_ago": now.saturating_sub(f.found_ms),
                "state": state,
                "seen": {
                    "transitions": seen.transitions,
                    "states": seen.states.iter().map(|s| s.map(|v| format!("{v:#x}"))).collect::<Vec<_>>(),
                    "enhanced_layout": seen.layout,
                },
                "art": f.applied.as_ref().map(|(sha, at)| json!({"sha256": sha, "ms_ago": now.saturating_sub(*at)})),
            })
        })
        .collect();
    let equipped = match b.reads_changed {
        Some(t) if now.saturating_sub(t) < 1500 => "yes: the game reads its fx.cave.* states (its effects are live)",
        Some(_) => "not now: the game stopped reading its fx.cave.* states (another deck?)",
        None => "unknown: the game has not read its fx.cave.* states since the board started (not equipped, or the fx states not found yet)",
    };
    let present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    let uploads_ring = uploads_status(&b, present, false);
    let uploads_deck = uploads_status(&b, present, true);
    let copies_ring = copies_status(present, false);
    let copies_deck = copies_status(present, true);
    let peek = b
        .peek
        .as_ref()
        .map(|p| json!({"resource": format!("{:#x}", p.resource), "x": p.x, "y": p.y, "mip": p.mip, "n": p.n, "done": p.stage == 2, "result": p.result}));
    let slices = slices_report(&b, present);
    let hunt = hunt_status(&b, present);
    let slice_job = b
        .slice_job
        .as_ref()
        .map(|j| json!({"kind": if j.write { "write" } else { "match" }, "stage": j.stage, "items": j.items.len(), "ms": now.saturating_sub(j.started)}));
    let slice_matches: Vec<Value> = b.slice_matches.iter().map(|m| json!({"resource": format!("{:#x}", m.resource), "slice": m.slice, "dxgi_format": m.format, "mip": m.mip, "tx": m.tx, "ty": m.ty, "error": (m.error * 100.0).round() / 100.0, "corners": m.corners, "verdict": m.verdict})).collect();
    let raw_pending = b.raw_pending.or(b.owner_scan.as_ref().and_then(|o| o.raw)).map(|r| json!({"resource": format!("{:#x}", r.resource), "since_ms": now.saturating_sub(r.since), "scanning": b.owner_scan.as_ref().is_some_and(|o| o.raw.is_some())}));
    let owners_pending = b.owners_wanted || b.owner_scan.as_ref().is_some_and(|o| o.raw.is_none());
    json!({
        "active": ACTIVE.load(Ordering::Relaxed),
        "texture_name": TEXTURE_NAME,
        "usable": b.ids.is_some(),
        "not_usable": b.ids_error,
        "found": !b.targets.is_empty(),
        "targets": targets,
        "equipped": equipped,
        "finding": b.scan.is_some() || b.find_now,
        "scans": b.scans,
        "last_scan": b.last_scan,
        "siblings": b.targets.iter().filter(|f| f.t.sibling).count(),
        "siblings_on": SIBLINGS_ON.load(Ordering::Relaxed),
        "siblings_max": b.siblings_max,
        "sibling_scans": b.sibling_scans,
        "finding_siblings": b.sibling_scan.is_some(),
        "last_sibling_scan": b.last_sibling_scan,
        "composites": b.targets.iter().filter(|f| f.t.composite).count(),
        "composites_max": b.composites_max,
        "identity_test_ms_left": IDENTITY_UNTIL_MS.load(Ordering::Relaxed).saturating_sub(now),
        "manual_targets": b.targets.iter().filter(|f| f.t.manual).count(),
        "uploads_ring": uploads_ring,
        "uploads_deck": uploads_deck,
        "copies_deck": copies_deck,
        "deck_seen": crate::copies::DECK_SEEN.load(Ordering::Relaxed),
        "deck_shape": [DECK_W.load(Ordering::Relaxed), DECK_H.load(Ordering::Relaxed)],
        "present": present,
        "main_made_at": if b.main_made_at == 0 { Value::Null } else { json!(b.main_made_at) },
        "lost_ms_ago": if b.lost_at == 0 { Value::Null } else { json!(now.saturating_sub(b.lost_at)) },
        "lost_to_written_ms": if b.lost_to_written_ms == 0 { Value::Null } else { json!(b.lost_to_written_ms) },
        "slices": slices,
        "slice_job": slice_job,
        "slice_matches": slice_matches,
        "slices_result": b.slices_result,
        "tiles_keep": b.tiles_keep,
        "tiles_written_at": b.tiles_written_at,
        "tiles_rewrites": b.tiles_rewrites,
        "slices_state": b.slices_state.map(|v| format!("{v:#x}")),
        "hunt": hunt,
        "peek": peek,
        "owners_pending": owners_pending,
        "owners_at_ms_ago": if b.owners_at == 0 { Value::Null } else { json!(now.saturating_sub(b.owners_at)) },
        "raw_pending": raw_pending,
        "raw_last": b.raw_last,
        "copies_ring": copies_ring,
        "copies_seen": crate::copies::COPIES_SEEN.load(Ordering::Relaxed),
        "desc_leaks": DESC_LEAKS.load(Ordering::Relaxed),

        "identity_lines": IDENTITY_LINES.load(Ordering::Relaxed),
        "composite_scans": b.composite_scans,
        "finding_composite": b.composite_scan.is_some(),
        "last_composite_scan": b.last_composite_scan,
        "refused": b.refused,
        "manual_state": b.manual.map(|v| format!("{v:#x}")),
        "manual_per": b.manual_per.iter().map(|(f, s)| json!({"for": match f { ManualFor::Resource(r) => format!("{r:#x}"), ManualFor::Kind(k) => format!("kind:{k}") }, "state": format!("{s:#x}")})).collect::<Vec<_>>(),
        "composite_common": COMPOSITE_COMMON.load(Ordering::Relaxed),
        "finals_on": FINALS_ON.load(Ordering::Relaxed),
        "finals": finals_status(&b),
        "ridden_on": RIDDEN_ON.load(Ordering::Relaxed),
        "ridden": json!({"found": b.targets.iter().filter(|f| f.t.is_ridden).count(), "max": b.riddens_max}),
        "worldcopy_on": WORLDCOPY_ON.load(Ordering::Relaxed),
        "worldcopies": json!({"found": b.targets.iter().filter(|f| f.t.is_worldcopy).count(), "max": b.worldcopies_max, "hashes": b.worldcopy_hashes_list().iter().map(|h| format!("{h:#x}")).collect::<Vec<_>>(), "format_code": WORLDCOPY_FORMAT}),
        "copy_queue_lists": COPY_QUEUE_LISTS.load(Ordering::Relaxed),
        "art": b.art.as_ref().map(|a| json!({"label": a.label, "sha256": a.sha256, "keep": b.keep()})),
        "layer_arts": Slot::ALL[1..].iter().map(|s| (s.name().to_string(), b.layer_arts[s.index() - 1].as_ref().map(|a| json!({"label": a.label, "sha256": a.sha256})).unwrap_or(Value::Null))).collect::<serde_json::Map<String, Value>>(),
        "pending": b.job.as_ref().map(|a| json!({"label": a.label, "waiting_ms": now.saturating_sub(b.job_since)})),
        "uploads": b.uploads,
        "last_upload": b.last_upload,
        "last_error": b.last_error,
        "barriers_seen": BARRIERS_SEEN.load(Ordering::Relaxed),
        "watching": crate::depth::ENABLED.load(Ordering::Relaxed) && crate::depth::hooked(),
        "verify": verify_status(&b),
        "log": b.log,
    })
}

/// A short line for the DLL's status.
pub fn brief() -> Value {
    if !ACTIVE.load(Ordering::Relaxed) {
        return Value::Null;
    }
    let Ok(b) = cell().try_lock() else { return json!("busy") };
    // `art`: the label of the picture on the board (null: the plain deck), for the board cam window, which reads it
    // from the light status instead of the whole board state (2026-10-30)
    json!({"found": b.targets.len(), "uploads": b.uploads, "pending": b.job.is_some(), "error": b.last_error, "art": b.art.as_ref().map(|a| a.label.clone())})
}

fn states_text(s: &[u32; LEVELS]) -> String {
    if s.iter().all(|&v| v == s[0]) {
        format!("{:#x} (every mip)", s[0])
    } else {
        s.iter().map(|v| format!("{v:#x}")).collect::<Vec<_>>().join(" ")
    }
}

// ---------------------------------------------------------------- the upload (render thread)

/// Our GPU objects on the game's device.
struct Gpu {
    device: ID3D12Device,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    value: u64,
    event: win::Handle,
    upload: Option<(ID3D12Resource, *mut u8, u64)>,
    /// Video (video.rs): staging buffers with the fence value of the last upload from each, the allocators they
    /// record with, one list, and which slot is next.
    video_staging: [Option<(ID3D12Resource, *mut u8, u64)>; VIDEO_SLOTS],
    video_value: [u64; VIDEO_SLOTS],
    video_alloc: Option<Vec<ID3D12CommandAllocator>>,
    video_list: Option<ID3D12GraphicsCommandList>,
    video_next: usize,
    /// The vtables of a committed and of a placed resource of ours: a game resource must have one of them.
    resource_vtables: [usize; 2],
    /// The read-back of the check (two 4 x 4 blocks, 512 bytes apart), mapped for good, and the fence value of the
    /// last read-back recorded.
    readback: ID3D12Resource,
    readback_map: *const u8,
    verify_value: u64,
    /// The read-back of `board peek` (an N x N block at a texel position of a target), mapped for good, and the
    /// fence value of the last peek recorded.
    peek_readback: ID3D12Resource,
    peek_map: *const u8,
    peek_value: u64,
    /// The read-back of `board slices match` and `board tiles` (4 corner blocks of up to `SLICE_ITEMS` slices, 512
    /// bytes a block), mapped for good, and the fence value of the last one recorded.
    slice_readback: ID3D12Resource,
    slice_map: *const u8,
    slice_value: u64,
    /// The read-back of `board hunt` (3 blocks of up to `HUNT_BATCH` textures a present), mapped for good, and the
    /// fence value of the last batch.
    hunt_readback: ID3D12Resource,
    hunt_map: *const u8,
    hunt_value: u64,
}

/// At most this many (texture, slice) items are read back in one present by the hunt.
pub const HUNT_BATCH: usize = 64;
/// A block's place in the read-back: a 4-byte-texel block is 4 rows 256 bytes apart (1 KB), a BC block one row.
const HUNT_STRIDE: usize = 1024;
const HUNT_BYTES: u64 = (HUNT_BATCH * 3 * HUNT_STRIDE) as u64;
/// At most this many slices of one array are read.
const HUNT_SLICES: u16 = 64;

/// At most this many slices are read back or written in one job.
pub const SLICE_ITEMS: usize = 128;
const SLICE_BYTES: u64 = (SLICE_ITEMS * 4 * 512) as u64;
/// The corner blocks of a 256 x 256 slice read back: their texel positions.
const CORNERS: [(u32, u32); 4] = [(0, 0), (252, 0), (0, 252), (252, 252)];
/// A slice is ours when its corners decode within this mean absolute error per channel of our art's blocks (the
/// game's encoder is not ours: the same source gives different blocks).
pub const SLICE_MAX_ERROR: f64 = 12.0;

/// The peek's read-back: up to 16 rows of 256 bytes.
const PEEK_BYTES: u64 = 16 * 256;
/// At most this many texels a side.
pub const PEEK_MAX: u32 = 16;

/// The mips read back, and where a target's two blocks land in the read-back (512-byte placement alignment; a block
/// is up to 4 rows of 256 bytes, a target takes 2048 bytes).
const VERIFY_MIPS: [usize; 2] = [0, 2];
fn verify_offset(target: usize, k: usize) -> u64 {
    (target * 2048 + k * 1024) as u64
}

/// Video staging buffers in flight at most (the GPU runs a few frames behind the CPU with frame generation on: with
/// two, a 60 fps feed dropped one frame in six as busy).
const VIDEO_SLOTS: usize = 4;

// Used by one thread at a time (behind a mutex); the COM objects are free-threaded.
unsafe impl Send for Gpu {}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some((u, _, _)) = &self.upload {
            unsafe { u.Unmap(0, None) };
        }
        for s in self.video_staging.iter().flatten() {
            unsafe { s.0.Unmap(0, None) };
        }
    }
}

static GPU: Mutex<Option<Gpu>> = Mutex::new(None);

unsafe fn buffer(device: &ID3D12Device, kind: D3D12_HEAP_TYPE, bytes: u64, state: D3D12_RESOURCE_STATES) -> windows::core::Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
    let mut b: Option<ID3D12Resource> = None;
    device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer_desc(bytes), state, None, &mut b)?;
    b.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

fn buffer_desc(bytes: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    }
}

impl Gpu {
    unsafe fn new(device: &ID3D12Device) -> Result<Gpu, String> {
        let e = |what: &'static str| move |err: windows::core::Error| format!("{what}: {err}");
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).map_err(e("command allocator"))?;
        let list: ID3D12GraphicsCommandList =
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).map_err(e("command list"))?;
        list.Close().map_err(e("command list"))?;
        let _ = list.SetName(windows::core::w!("Caveman FX board"));
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(e("fence"))?;
        let event = win::Handle(win::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()));
        if !event.valid() {
            return Err("no event".into());
        }
        // what a game resource's COM vtable may be: a committed one's, or a placed one's (the game places its textures
        // in heaps of its own)
        let committed = buffer(device, D3D12_HEAP_TYPE_DEFAULT, 65536, D3D12_RESOURCE_STATE_COMMON).map_err(e("probe buffer"))?;
        let heap_desc = D3D12_HEAP_DESC {
            SizeInBytes: 65536,
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
            Alignment: 0,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS,
        };
        let mut heap: Option<ID3D12Heap> = None;
        device.CreateHeap(&heap_desc, &mut heap).map_err(e("probe heap"))?;
        let mut placed: Option<ID3D12Resource> = None;
        device
            .CreatePlacedResource(heap.as_ref().ok_or("no probe heap")?, 0, &buffer_desc(65536), D3D12_RESOURCE_STATE_COMMON, None, &mut placed)
            .map_err(e("probe placed buffer"))?;
        let vt = |r: &ID3D12Resource| win::read_ptr(r.as_raw() as usize).unwrap_or(0);
        let resource_vtables = [vt(&committed), placed.as_ref().map_or(0, vt)];
        for (k, v) in resource_vtables.iter().enumerate() {
            RES_VTABLES[k].store(*v, Ordering::Release);
        }
        // the read-back of the check (made once, here, as every object of ours): 1 KB a target
        let readback =
            buffer(device, D3D12_HEAP_TYPE_READBACK, (VERIFY_TARGETS * 2048) as u64, D3D12_RESOURCE_STATE_COPY_DEST).map_err(e("read-back buffer"))?;
        let mut map: *mut c_void = std::ptr::null_mut();
        readback.Map(0, None, Some(&mut map)).map_err(e("read-back map"))?;
        if map.is_null() {
            return Err("the read-back buffer did not map".into());
        }
        let peek_readback = buffer(device, D3D12_HEAP_TYPE_READBACK, PEEK_BYTES, D3D12_RESOURCE_STATE_COPY_DEST).map_err(e("peek read-back buffer"))?;
        let mut peek_map: *mut c_void = std::ptr::null_mut();
        peek_readback.Map(0, None, Some(&mut peek_map)).map_err(e("peek read-back map"))?;
        if peek_map.is_null() {
            return Err("the peek read-back buffer did not map".into());
        }
        let slice_readback = buffer(device, D3D12_HEAP_TYPE_READBACK, SLICE_BYTES, D3D12_RESOURCE_STATE_COPY_DEST).map_err(e("slice read-back buffer"))?;
        let mut slice_map: *mut c_void = std::ptr::null_mut();
        slice_readback.Map(0, None, Some(&mut slice_map)).map_err(e("slice read-back map"))?;
        if slice_map.is_null() {
            return Err("the slice read-back buffer did not map".into());
        }
        let hunt_readback = buffer(device, D3D12_HEAP_TYPE_READBACK, HUNT_BYTES, D3D12_RESOURCE_STATE_COPY_DEST).map_err(e("hunt read-back buffer"))?;
        let mut hunt_map: *mut c_void = std::ptr::null_mut();
        hunt_readback.Map(0, None, Some(&mut hunt_map)).map_err(e("hunt read-back map"))?;
        if hunt_map.is_null() {
            return Err("the hunt read-back buffer did not map".into());
        }
        crate::memwatch::set_adapter(device.GetAdapterLuid());
        Ok(Gpu {
            device: device.clone(),
            allocator,
            list,
            fence,
            value: 0,
            event,
            upload: None,
            video_staging: [None, None, None, None],
            video_value: [0; VIDEO_SLOTS],
            video_alloc: None,
            video_list: None,
            video_next: 0,
            resource_vtables,
            readback,
            readback_map: map as *const u8,
            verify_value: 0,
            peek_readback,
            peek_map: peek_map as *const u8,
            peek_value: 0,
            slice_readback,
            slice_map: slice_map as *const u8,
            slice_value: 0,
            hunt_readback,
            hunt_map: hunt_map as *const u8,
            hunt_value: 0,
        })
    }

    /// Records the read-back of the check's blocks (a 4 x 4 texel block of mips 0 and 2, those below `mips`) of
    /// `res` (in COPY_SOURCE for those mips) for target `target`; `rows` rows of 16 bytes each land in the read-back.
    unsafe fn record_verify_copies(&self, res: &ID3D12Resource, format: DXGI_FORMAT, target: usize, rows: usize, mips: usize) {
        let _ = rows;
        for (k, &m) in VERIFY_MIPS.iter().enumerate() {
            if m >= mips {
                continue;
            }
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&self.readback),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: verify_offset(target, k),
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: format, Width: 4, Height: 4, Depth: 1, RowPitch: 256 },
                    },
                },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(res),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: m as u32 },
            };
            let b = D3D12_BOX { left: 0, top: 0, front: 0, right: 4, bottom: 4, back: 1 };
            self.list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&b));
        }
    }

    /// A target's two blocks read back (once the fence passed): `rows` rows of 16 bytes each (the rows 256 bytes
    /// apart in the read-back).
    unsafe fn read_verify(&self, target: usize, rows: usize) -> [Vec<u8>; 2] {
        std::array::from_fn(|k| {
            let mut out = vec![0u8; rows * 16];
            for y in 0..rows {
                std::ptr::copy_nonoverlapping(self.readback_map.add(verify_offset(target, k) as usize + y * 256), out[y * 16..].as_mut_ptr(), 16);
            }
            out
        })
    }

    fn busy(&self) -> bool {
        unsafe { self.fence.GetCompletedValue() < self.value }
    }

    /// Waits (at most `ms`) for our last upload; true when it is done.
    fn wait(&self, ms: u32) -> bool {
        unsafe {
            if self.fence.GetCompletedValue() >= self.value {
                return true;
            }
            if self.fence.SetEventOnCompletion(self.value, HANDLE(self.event.0 as isize)).is_ok() {
                win::WaitForSingleObject(self.event.0, ms);
            }
            self.fence.GetCompletedValue() >= self.value
        }
    }

    /// The upload buffer, at least `bytes` (kept between uploads).
    unsafe fn upload(&mut self, bytes: u64) -> Result<*mut u8, String> {
        if let Some((_, map, size)) = &self.upload {
            if *size >= bytes {
                return Ok(*map);
            }
        }
        if let Some((u, _, _)) = self.upload.take() {
            u.Unmap(0, None);
        }
        let u = buffer(&self.device, D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ).map_err(|e| format!("upload buffer: {e}"))?;
        let _ = u.SetName(windows::core::w!("Caveman FX board upload"));
        let mut map: *mut c_void = std::ptr::null_mut();
        let none = D3D12_RANGE { Begin: 0, End: 0 };
        u.Map(0, Some(&none), Some(&mut map)).map_err(|e| format!("upload buffer map: {e}"))?;
        if map.is_null() {
            return Err("the upload buffer did not map".into());
        }
        self.upload = Some((u, map as *mut u8, bytes));
        Ok(map as *mut u8)
    }

    /// Video: staging buffer `s`, at least `bytes` (kept; made again larger when short).
    unsafe fn video_staging(&mut self, s: usize, bytes: u64) -> Result<*mut u8, String> {
        if let Some((_, map, size)) = &self.video_staging[s] {
            if *size >= bytes {
                return Ok(*map);
            }
        }
        if let Some((u, _, _)) = self.video_staging[s].take() {
            u.Unmap(0, None);
        }
        // (made once per slot, and only with room under the memory budgets: never a squeeze on the game's own uploads)
        crate::memwatch::room_for(bytes).map_err(|why| format!("video staging buffer not made: {why}"))?;
        let u = buffer(&self.device, D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ).map_err(|e| format!("video staging buffer: {e}"))?;
        let _ = u.SetName(windows::core::w!("Caveman FX video staging"));
        let mut map: *mut c_void = std::ptr::null_mut();
        let none = D3D12_RANGE { Begin: 0, End: 0 };
        u.Map(0, Some(&none), Some(&mut map)).map_err(|e| format!("video staging map: {e}"))?;
        if map.is_null() {
            return Err("the video staging buffer did not map".into());
        }
        self.video_staging[s] = Some((u, map as *mut u8, bytes));
        Ok(map as *mut u8)
    }

    /// Video: the allocator of staging slot `s` and the list they record with (made on first use).
    unsafe fn video_objects(&mut self, s: usize) -> Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList), String> {
        if self.video_alloc.is_none() {
            let mut allocs = Vec::with_capacity(VIDEO_SLOTS);
            for _ in 0..VIDEO_SLOTS {
                let a: ID3D12CommandAllocator =
                    self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).map_err(|e| format!("video allocator: {e}"))?;
                allocs.push(a);
            }
            let list: ID3D12GraphicsCommandList = self
                .device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocs[0], None::<&ID3D12PipelineState>)
                .map_err(|e| format!("video list: {e}"))?;
            let _ = list.Close();
            self.video_alloc = Some(allocs);
            self.video_list = Some(list);
        }
        Ok((self.video_alloc.as_ref().unwrap()[s].clone(), self.video_list.clone().unwrap()))
    }
}

/// A target checked once more on the render thread, with its COM object, desc and states.
struct Ready {
    resource: ID3D12Resource,
    states: [u32; LEVELS],
    source: Source,
    kind: &'static str,
    /// Its levels, its DXGI format, their footprints in the upload buffer (rows and bytes a row), the chain of the
    /// art written into it (index) and whether its 4-byte order is swapped on the way.
    mips: usize,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
    fp: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    rows: Vec<u32>,
    row_bytes: Vec<u64>,
    chain: usize,
    swizzle: bool,
    /// The chain's first level written into mip 0 (a demoted target: the tail of the full chain).
    skip: usize,
    /// The art the chain belongs to (the target's slot's).
    art: Arc<Art>,
}

fn transition(res: &ID3D12Resource, sub: u32, before: u32, after: u32) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: sub,
                StateBefore: D3D12_RESOURCE_STATES(before as i32),
                StateAfter: D3D12_RESOURCE_STATES(after as i32),
            }),
        },
    }
}

/// Barriers taking each subresource from `from[m]` to `to[m]` (one barrier when they all agree).
/// The barriers taking the first `n` levels of a texture from their states to others (none for a level already there).
fn transitions_n(res: &ID3D12Resource, from: &[u32; LEVELS], to: &[u32; LEVELS], n: usize) -> Vec<D3D12_RESOURCE_BARRIER> {
    let n = n.clamp(1, LEVELS);
    if from[..n].iter().all(|&v| v == from[0]) && to[..n].iter().all(|&v| v == to[0]) {
        return if from[0] == to[0] { Vec::new() } else { vec![transition(res, ALL_SUBRESOURCES, from[0], to[0])] };
    }
    // (a level already in its state gets no barrier: a transition to the same state is an error)
    (0..n).filter(|&m| from[m] != to[m]).map(|m| transition(res, m as u32, from[m], to[m])).collect()
}

/// `ResourceBarrier` with what there is (nothing for none).
unsafe fn barriers(list: &ID3D12GraphicsCommandList, v: &[D3D12_RESOURCE_BARRIER]) {
    if !v.is_empty() {
        list.ResourceBarrier(v);
    }
}

/// Whether a BC7 format (the deck textures are BC7_UNORM_SRGB; typeless and linear are the same blocks).
fn bc7(f: DXGI_FORMAT) -> bool {
    f == DXGI_FORMAT_BC7_UNORM_SRGB || f == DXGI_FORMAT_BC7_UNORM || f == DXGI_FORMAT_BC7_TYPELESS
}

/// From the overlay's Present hook, before anything is drawn, when an upload waits: the art goes into every target
/// on `queue` (the one the game presents with). Never blocks: a busy lock, a target not found yet or our last upload
/// still running leaves it for a later frame.
pub fn on_present(device: &ID3D12Device, queue: Option<&ID3D12CommandQueue>) {
    let Some(queue) = queue else { return };
    if PENDING.load(Ordering::Acquire) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { present_upload(device, queue) }));
    }
    if VERIFY.load(Ordering::Acquire) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { verify_present(device, queue) }));
    }
    if PEEK.load(Ordering::Acquire) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { peek_present(device, queue) }));
    }
    if SLICES.load(Ordering::Acquire) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { slices_present(device, queue) }));
    }
    if HUNTING.load(Ordering::Acquire) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { hunt_present(device, queue) }));
    }
    if DESCRIBE.load(Ordering::Acquire) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { describe_targets(device) }));
    }
    if DESCRIBE_RING.load(Ordering::Acquire) || NEW_NEXT.load(Ordering::Acquire) != NEW_TAKEN.load(Ordering::Relaxed) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { describe_ring(device) }));
    }
}

/// Video (video.rs): the producer's newest frame into the ridden art copy, once per present, through a staging buffer
/// and a list of its own (two in flight at most: a frame whose staging buffer is still in use is dropped, never
/// waited for). Only while The Caveman is the equipped deck, as every ridden write.
pub fn video_present(device: &ID3D12Device, queue: Option<&ID3D12CommandQueue>) {
    let Some(queue) = queue else { return };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { video_upload(device, queue) }));
}

/// The ridden art copy's D3D12 resource (raw), when it is found and described on the presenting device: what the
/// in-world board samples now, for the board cam's deck (overlay.rs); 0 when there is none.
pub fn ridden_art_resource() -> usize {
    let Ok(b) = cell().try_lock() else { return 0 };
    b.targets.iter().find(|f| f.t.is_ridden && f.t.slot == Slot::Art && f.described && !f.wrong_device && !dead(f.t.resource)).map_or(0, |f| f.t.resource)
}

/// `board video off`: the ridden art copy holds the last frame; its kept art is marked not applied, so the keep (the
/// worker's requeue) puts the art back.
pub fn video_stopped() {
    let mut b = board();
    for f in b.targets.iter_mut().filter(|f| f.t.is_ridden && f.t.slot == Slot::Art) {
        f.applied = None;
    }
}

unsafe fn video_upload(device: &ID3D12Device, queue: &ID3D12CommandQueue) {
    let Some(frame) = crate::video::next_frame() else { return };
    let t0 = Instant::now();
    let Ok(b) = cell().try_lock() else { return };
    if !b.equipped(win::now_ms()) {
        crate::video::note("The Caveman is not the equipped deck: frames wait");
        return;
    }
    let Some(f) = b.targets.iter().find(|f| f.t.is_ridden && f.t.slot == Slot::Art) else {
        crate::video::note("the ridden art copy is not found yet (the census finds it at an equip)");
        return;
    };
    if f.wrong_device {
        crate::video::note("the ridden art copy is on another device");
        return;
    }
    let states = match decide_target(&b, f, f.dxgi_format, f.described) {
        Ok((s, _)) => s,
        Err(e) => {
            crate::video::note(&format!("the ridden art copy's state is unknown: {e}"));
            return;
        }
    };
    let resource_addr = f.t.resource;
    drop(b);
    let Ok(mut g) = GPU.try_lock() else { return };
    if g.as_ref().is_some_and(|x| x.device.as_raw() != device.as_raw()) {
        if !g.as_ref().unwrap().wait(0) {
            return;
        }
        *g = None;
    }
    if g.is_none() {
        match Gpu::new(device) {
            Ok(x) => *g = Some(x),
            Err(e) => {
                crate::video::note(&format!("GPU objects: {e}"));
                return;
            }
        }
    }
    let gpu = g.as_mut().unwrap();
    let vt = win::read_ptr(resource_addr).unwrap_or(0);
    if vt == 0 || !gpu.resource_vtables.contains(&vt) {
        crate::video::note("the ridden art copy is not a D3D12 resource any more");
        return;
    }
    let raw = resource_addr as *mut c_void;
    let Some(r) = ID3D12Resource::from_raw_borrowed(&raw) else { return };
    let resource = r.clone();
    let d = resource.GetDesc();
    let two_d = d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D && d.DepthOrArraySize == 1 && d.SampleDesc.Count == 1;
    if !(two_d
        && ridden_size(d.Width as u32, d.Height)
        && ChainFormat::family_of(d.Format.0) == Some(ChainFormat::Rgba8)
        && (1..=LEVELS as u16).contains(&d.MipLevels))
    {
        crate::video::note("the ridden art copy is not the shape expected");
        return;
    }
    let mut dev: Option<ID3D12Device> = None;
    if resource.GetDevice(&mut dev).is_err() || dev.as_ref().map(|x| x.as_raw()) != Some(device.as_raw()) {
        crate::video::note("the ridden art copy is not on the presenting device");
        return;
    }
    // the frame's shape against the copy's: the same, or a power-of-two larger (the chain's later levels then)
    let (mut w, mut h, mut skip) = (frame.width, frame.height, 0usize);
    while (w > d.Width as u32 || h > d.Height) && skip < 4 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        skip += 1;
    }
    let mips = d.MipLevels as usize;
    if w != d.Width as u32 || h != d.Height || (frame.levels as usize) < mips + skip {
        crate::video::skip(
            frame.seq,
            &format!("a {}x{} frame of {} levels does not fit the {}x{} copy of {mips} mips", frame.width, frame.height, frame.levels, d.Width, d.Height),
        );
        return;
    }
    let mut fp = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); mips];
    let mut rows = vec![0u32; mips];
    let mut row_bytes = vec![0u64; mips];
    let mut total = 0u64;
    gpu.device.GetCopyableFootprints(&d, 0, mips as u32, 0, Some(fp.as_mut_ptr()), Some(rows.as_mut_ptr()), Some(row_bytes.as_mut_ptr()), Some(&mut total));
    let sizes = chain_sizes(ChainFormat::Rgba8, frame.width, frame.height, mips + skip);
    if (0..mips).any(|m| row_bytes[m] * rows[m] as u64 != sizes[m + skip] as u64) || sizes.iter().sum::<usize>() > frame.len {
        crate::video::skip(frame.seq, "the frame's levels do not match the copy's footprints");
        return;
    }
    let s = gpu.video_next % VIDEO_SLOTS;
    if gpu.fence.GetCompletedValue() < gpu.video_value[s] {
        crate::video::dropped(true);
        return;
    }
    let map = match gpu.video_staging(s, total) {
        Ok(m) => m,
        Err(e) => {
            crate::video::note(&e);
            return;
        }
    };
    let bytes = std::slice::from_raw_parts(frame.ptr, frame.len);
    let mut src: usize = sizes[..skip].iter().sum();
    for m in 0..mips {
        let row = row_bytes[m] as usize;
        for y in 0..rows[m] as usize {
            let to = map.add(fp[m].Offset as usize + y * fp[m].Footprint.RowPitch as usize);
            std::ptr::copy_nonoverlapping(bytes.as_ptr().add(src + y * row), to, row);
        }
        src += row * rows[m] as usize;
    }
    let fill_us = t0.elapsed().as_micros() as u64;
    if !crate::video::still_valid(frame.seq) {
        crate::video::dropped(false);
        return;
    }
    let (alloc, list) = match gpu.video_objects(s) {
        Ok(x) => x,
        Err(e) => {
            crate::video::note(&e);
            return;
        }
    };
    if alloc.Reset().is_err() || list.Reset(&alloc, None::<&ID3D12PipelineState>).is_err() {
        return;
    }
    let staging: ID3D12Resource = gpu.video_staging[s].as_ref().unwrap().0.clone();
    RECORDING.with(|r| r.set(true));
    barriers(&list, &transitions_n(&resource, &states, &[COPY_DEST; LEVELS], mips));
    for (m, f) in fp.iter().enumerate() {
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&resource),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: m as u32 },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&staging),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *f },
        };
        list.CopyTextureRegion(&dst, 0, 0, 0, &from, None);
    }
    barriers(&list, &transitions_n(&resource, &[COPY_DEST; LEVELS], &states, mips));
    RECORDING.with(|r| r.set(false));
    if list.Close().is_err() {
        return;
    }
    let Ok(as_list) = list.cast::<ID3D12CommandList>() else { return };
    queue.ExecuteCommandLists(&[Some(as_list)]);
    gpu.value += 1;
    if queue.Signal(&gpu.fence, gpu.value).is_err() {
        gpu.value -= 1;
        return;
    }
    gpu.video_value[s] = gpu.value;
    gpu.video_next += 1;
    crate::video::took(frame.seq, fill_us, t0.elapsed().as_micros() as u64);
}

unsafe fn present_upload(device: &ID3D12Device, queue: &ID3D12CommandQueue) {
    let t0 = Instant::now();
    let Ok(mut b) = cell().try_lock() else { return };
    let Some(art) = b.job.clone() else {
        PENDING.store(false, Ordering::Release);
        return;
    };
    let Some(ids) = b.ids else { return };
    if b.targets.is_empty() {
        return; // the worker is finding it (or will, when asked)
    }
    let Ok(mut g) = GPU.try_lock() else { return };
    if g.as_ref().is_some_and(|x| x.device.as_raw() != device.as_raw()) {
        if !g.as_ref().unwrap().wait(0) {
            return; // the old device's upload still runs
        }
        *g = None;
    }
    if g.is_none() {
        // made on one frame, used from the next (each frame's work stays small)
        match Gpu::new(device) {
            Ok(x) => *g = Some(x),
            Err(e) => fail_job(&mut b, format!("cannot make our GPU objects: {e}")),
        }
        return;
    }
    let gpu = g.as_mut().unwrap();
    if gpu.busy() {
        return;
    }
    // every target again, now: the chain, the COM object, its desc and device, its state, and the chain of the art
    // that fits it (BC7 for the asset's texture and the siblings; RGBA8 or BGRA8 for the composite, its real format and
    // mip count from GetDesc)
    let mut ready: Vec<Ready> = Vec::new();
    let mut problems = Vec::new();
    // targets already holding their slot's art (left alone), and the ridden copies parked while another deck is
    // the equipped one
    let mut already = 0usize;
    let mut parked = 0usize;
    let equipped_now = b.equipped(win::now_ms());
    let wc_hashes = b.worldcopy_hashes_list();
    let mut notes: Vec<(usize, String)> = Vec::new();
    let mut offset = 0u64;
    for f in &b.targets {
        let check = if f.t.is_raw() {
            read_raw(f.t.resource).and_then(|t| {
                raw_still_owned(&f.t, &ids).map(|_| Target {
                    owner: f.t.owner,
                    owner_texture: f.t.owner_texture,
                    rc2: f.t.rc2,
                    rc2_bind: f.t.rc2_bind,
                    rc2_state: f.t.rc2_state,
                    id: f.t.id,
                    ..t
                })
            })
        } else if f.t.is_worldcopy {
            read_worldcopy(f.t.texture, &ids, &wc_hashes)
        } else if f.t.is_ridden {
            read_ridden_fmt(f.t.texture, &ids, ridden_format(f.t.slot), f.t.slot)
        } else if f.t.manual {
            read_manual(f.t.texture, &ids)
        } else if f.t.is_final {
            read_final(f.t.texture, &ids, &FINAL_HASHES)
        } else if f.t.composite {
            read_composite(f.t.texture, &ids)
        } else if f.t.sibling {
            read_sibling(f.t.texture, &ids)
        } else {
            read_chain_slot(f.t.asset, &ids, f.t.slot)
        };
        let t = match check {
            Ok(t) if t.resource == f.t.resource => t,
            Ok(_) => {
                problems.push(format!("the {} texture was made again (the worker will pick it up)", f.t.kind()));
                continue;
            }
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let mut refuse = |why: String| {
            problems.push(why.clone());
            notes.push((t.resource, why));
        };
        if f.wrong_device {
            refuse(format!("resource {:#x}: its device is not the presenting one: not touched", t.resource));
            continue;
        }
        if t.is_raw() && !owner_alive(&t, &ids) {
            refuse(format!("resource {:#x}: no live owner: the game may have released it: not touched", t.resource));
            continue;
        }
        // a dead mark refuses only a target nothing owns (a forced raw one): every other kind was validated this
        // tick by its owner naming the resource, and a new resource at a dead one's address is alive again (seen at
        // the first re-equip with the ridden copy: its slot's resource was made again twice at reused addresses)
        if dead(t.resource) && t.is_raw() && t.owner == 0 {
            refuse(format!("resource {:#x}: released by the game (its last Release seen): not touched", t.resource));
            continue;
        }
        let vt = win::read_ptr(t.resource).unwrap_or(0);
        if vt == 0 || !gpu.resource_vtables.contains(&vt) {
            refuse(format!("resource {:#x}: its vtable {vt:#x} is not a D3D12 resource's ({:#x?})", t.resource, gpu.resource_vtables));
            continue;
        }
        let raw = t.resource as *mut c_void;
        let Some(r) = ID3D12Resource::from_raw_borrowed(&raw) else { continue };
        let resource = r.clone();
        let d = resource.GetDesc();
        // alive (its owner says so, and we hold it): watched from here on, a dead mark on its address cleared
        watch_liveness(&resource, t.resource);
        // what GetDesc said when the target was described must still hold (a released and reused resource: not)
        if f.described && (d.Format.0 != f.dxgi_format || d.MipLevels != f.mips || (f.width != 0 && (d.Width as u32 != f.width || d.Height != f.height))) {
            refuse(format!("resource {:#x}: GetDesc changed since it was described ({}x{}, format {}, {} mips; now {}x{}, format {}, {} mips): released and reused? not touched", t.resource, f.width, f.height, f.dxgi_format, f.mips, d.Width, d.Height, d.Format.0, d.MipLevels));
            continue;
        }
        // the shape: The Caveman's for the asset's texture and the siblings; the composite 512 x 2048 in the RGBA8 or
        // BGRA8 family; a target added by hand any 2D size of a format the art has a chain for, up to 12 mips
        let two_d = d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D && d.DepthOrArraySize == 1 && d.SampleDesc.Count == 1;
        let caveman_size = d.Width == WIDTH as u64 && d.Height == HEIGHT;
        let (shape_ok, format_ok) = if t.is_worldcopy {
            (two_d && caveman_size, ChainFormat::family_of(d.Format.0) == Some(ChainFormat::Bc1) && (1..=LEVELS as u16).contains(&d.MipLevels))
        } else if t.is_ridden {
            (
                two_d && ridden_size(d.Width as u32, d.Height),
                ChainFormat::family_of(d.Format.0) == Some(ChainFormat::Rgba8) && (1..=LEVELS as u16).contains(&d.MipLevels),
            )
        } else if t.manual {
            (
                two_d && d.Width >= 1 && d.Width <= 16384 && d.Height >= 1 && d.Height <= 16384,
                ChainFormat::family_of(d.Format.0).is_some() && (1..=LEVELS as u16).contains(&d.MipLevels),
            )
        } else if t.composite {
            (two_d && caveman_size, composite_format_ok(d.Format.0) && (1..=LEVELS as u16).contains(&d.MipLevels))
        } else {
            (two_d && caveman_size, bc7(d.Format) && d.MipLevels == LEVELS as u16)
        };
        if !shape_ok || !format_ok {
            refuse(format!(
                "resource {:#x}: GetDesc says {}x{}x{} with {} mips, format {}, dimension {}: not {}",
                t.resource,
                d.Width,
                d.Height,
                d.DepthOrArraySize,
                d.MipLevels,
                d.Format.0,
                d.Dimension.0,
                if t.is_worldcopy {
                    "a 512x2048 BC1 texture of up to 12 mips (the world copy)"
                } else if t.is_ridden {
                    "a 512x2048, 256x1024 or 128x512 RGBA8 texture of up to 12 mips (the ridden copy)"
                } else if t.manual {
                    "a 2D texture of up to 12 mips in the BC7, BC1, RGBA8 or BGRA8 family"
                } else if t.composite {
                    "a 512x2048 RGBA8 or BGRA8 texture of up to 12 mips (the composite)"
                } else {
                    "The Caveman's"
                }
            ));
            continue;
        }
        let mut dev: Option<ID3D12Device> = None;
        if resource.GetDevice(&mut dev).is_err() || dev.as_ref().map(|x| x.as_raw()) != Some(device.as_raw()) {
            refuse(format!("resource {:#x} is not on the presenting device", t.resource));
            continue;
        }
        // the art of the target's slot (a layer target takes its layer's; a target already holding it is left alone)
        let Some(slot_art) = b.art_for(t.slot) else {
            refuse(format!("resource {:#x} ({}): no art for the {} slot yet", t.resource, t.kind(), t.slot.name()));
            continue;
        };
        if f.applied.as_ref().map(|a| a.0.as_str()) == Some(slot_art.sha256.as_str()) {
            already += 1;
            continue;
        }
        // the ridden copies are the game's slots for whatever deck is ridden: written only while The Caveman is it
        if t.is_ridden && !equipped_now {
            parked += 1;
            continue;
        }
        let (chain_index, swizzle, skip) = match slot_art.pick_chain(d.Format.0, d.MipLevels, d.Width as u32, d.Height) {
            Ok((c, s, k)) => (slot_art.chains.iter().position(|x| std::ptr::eq(x, c)).unwrap_or(0), s, k),
            Err(e) => {
                refuse(format!("resource {:#x} ({}): {e}", t.resource, t.kind()));
                continue;
            }
        };
        match decide_target(&b, f, d.Format.0, true) {
            Ok((states, source)) => {
                // the footprints of its levels, laid out after the targets before it in the upload buffer
                let mips = d.MipLevels as usize;
                let mut fp = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); mips];
                let mut rows = vec![0u32; mips];
                let mut row_bytes = vec![0u64; mips];
                let mut total = 0u64;
                gpu.device.GetCopyableFootprints(
                    &d,
                    0,
                    mips as u32,
                    offset,
                    Some(fp.as_mut_ptr()),
                    Some(rows.as_mut_ptr()),
                    Some(row_bytes.as_mut_ptr()),
                    Some(&mut total),
                );
                let chain = &slot_art.chains[chain_index];
                // (a demoted target takes the chain's levels from `skip` on)
                let sizes = chain_sizes(chain.format, chain.width, chain.height, mips + skip);
                let mut fits = true;
                for m in 0..mips {
                    if row_bytes[m] * rows[m] as u64 != sizes[m + skip] as u64 || (fp[m].Footprint.RowPitch as u64) < row_bytes[m] {
                        refuse(format!(
                            "resource {:#x} mip {m}: the footprint ({} rows of {} bytes, pitch {}) does not fit the {} chain's {} bytes (its level {})",
                            t.resource,
                            rows[m],
                            row_bytes[m],
                            fp[m].Footprint.RowPitch,
                            chain.format.name(),
                            sizes[m + skip],
                            m + skip
                        ));
                        fits = false;
                        break;
                    }
                }
                if !fits {
                    continue;
                }
                offset += total.div_ceil(512) * 512;
                ready.push(Ready {
                    resource,
                    states,
                    source,
                    kind: t.kind(),
                    mips,
                    format: d.Format,
                    width: d.Width as u32,
                    height: d.Height,
                    fp,
                    rows,
                    row_bytes,
                    chain: chain_index,
                    swizzle,
                    skip,
                    art: slot_art.clone(),
                });
            }
            Err(e) => refuse(format!("resource {:#x} ({}): {e}", t.resource, t.kind())),
        }
    }
    for (res, why) in notes {
        if let Some(f) = b.targets.iter_mut().find(|f| f.t.resource == res) {
            f.refused = why;
        }
    }
    let mut refill_lines: Vec<String> = Vec::new();
    for r in &ready {
        if let Some(f) = b.targets.iter_mut().find(|f| f.t.resource == r.resource.as_raw() as usize) {
            f.refused.clear();
            f.dxgi_format = r.format.0;
            f.mips = r.mips as u16;
            f.width = r.width;
            f.height = r.height;
            f.described = true;
            f.written_present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
            if f.refill_noticed_ms != 0 {
                let line = format!(
                    "the world copy {:#x} written again {} ms after the game's re-fill was noticed ({} present(s) after the fill at present {})",
                    f.t.resource,
                    win::now_ms().saturating_sub(f.refill_noticed_ms),
                    f.written_present.saturating_sub(f.refill_present),
                    f.refill_present
                );
                refill_lines.push(line);
                f.refill_noticed_ms = 0;
            }
        }
    }
    for l in refill_lines {
        b.say_later(l);
    }
    // nothing to write and no failure: every target already holds its slot's art, or is a ridden copy parked while
    // another deck is equipped (the composites refused for an unknown state do not count: they are never the
    // reason a write is asked for)
    let only_benign = problems.iter().all(|p| p.contains("state unknown") || p.contains("no art for the"));
    if ready.is_empty() && (already > 0 || parked > 0) && only_benign {
        b.say_later(format!("art {:?}: nothing to write ({already} target(s) already hold their slot's art, {parked} ridden cop{} parked: The Caveman is not the equipped deck)", art.label, if parked == 1 { "y" } else { "ies" }));
        b.job = None;
        b.last_error.clear();
        PENDING.store(false, Ordering::Release);
        return;
    }
    if ready.is_empty() {
        let why = if problems.is_empty() { "no target".to_string() } else { problems.join("; ") };
        fail_job(&mut b, format!("nothing written: {why}"));
        return;
    }
    let total = offset;
    if gpu.upload.as_ref().is_none_or(|u| u.2 < total) {
        // never within the margin of the process's memory budget (the memory watch; unmeasured for the first second:
        // wait, up to 5 s)
        if let Err(why) = crate::memwatch::room_for(total) {
            if !crate::memwatch::measured() && win::now_ms() < b.job_since + 5000 {
                return;
            }
            fail_job(&mut b, format!("the upload buffer ({} MB) not made: {why}", total >> 20));
            return;
        }
        // the upload buffer on a frame of its own too
        if let Err(e) = gpu.upload(total) {
            fail_job(&mut b, e);
        }
        return;
    }
    let map = match gpu.upload(total) {
        Ok(m) => m,
        Err(e) => {
            fail_job(&mut b, e);
            return;
        }
    };
    let t_fill = Instant::now();
    for r in &ready {
        let chain = &r.art.chains[r.chain];
        let mut src = chain.level_offset(r.skip);
        for m in 0..r.mips {
            let row = r.row_bytes[m] as usize;
            for y in 0..r.rows[m] as usize {
                let from = &chain.bytes[src + y * row..src + (y + 1) * row];
                let to = map.add(r.fp[m].Offset as usize + y * r.fp[m].Footprint.RowPitch as usize);
                if r.swizzle {
                    // the other 4-byte order: red and blue swapped
                    for (k, px) in from.chunks_exact(4).enumerate() {
                        let out = [px[2], px[1], px[0], px[3]];
                        std::ptr::copy_nonoverlapping(out.as_ptr(), to.add(k * 4), 4);
                    }
                } else {
                    std::ptr::copy_nonoverlapping(from.as_ptr(), to, from.len());
                }
            }
            src += row * r.rows[m] as usize;
        }
    }
    let filled_us = t_fill.elapsed().as_micros() as u64;
    // record: each target to COPY_DEST, its copies, the check's two blocks read back (mips 0 and 2 to COPY_SOURCE),
    // then every level back to the state it was in
    if gpu.allocator.Reset().is_err() {
        return;
    }
    if let Err(e) = gpu.list.Reset(&gpu.allocator, None::<&ID3D12PipelineState>) {
        fail_job(&mut b, format!("list reset: {e}"));
        return;
    }
    let (upload, _, _) = gpu.upload.as_ref().unwrap();
    RECORDING.with(|r| r.set(true));
    for (i, t) in ready.iter().enumerate() {
        let n = t.mips;
        barriers(&gpu.list, &transitions_n(&t.resource, &t.states, &[COPY_DEST; LEVELS], n));
        for (m, f) in t.fp.iter().enumerate() {
            let dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&t.resource),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: m as u32 },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(upload),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *f },
            };
            gpu.list.CopyTextureRegion(&dst, 0, 0, 0, &from, None);
        }
        let mut from = [COPY_DEST; LEVELS];
        if i < VERIFY_TARGETS {
            let mut verify_to = [COPY_DEST; LEVELS];
            for &m in VERIFY_MIPS.iter().filter(|&&m| m < n) {
                verify_to[m] = COPY_SOURCE;
                from[m] = COPY_SOURCE;
            }
            barriers(&gpu.list, &transitions_n(&t.resource, &[COPY_DEST; LEVELS], &verify_to, n));
            gpu.record_verify_copies(&t.resource, t.fp[0].Footprint.Format, i, verify_rows(t.format.0), n);
        }
        barriers(&gpu.list, &transitions_n(&t.resource, &from, &t.states, n));
    }
    RECORDING.with(|r| r.set(false));
    if let Err(e) = gpu.list.Close() {
        fail_job(&mut b, format!("list close: {e}"));
        return;
    }
    let Ok(as_list) = gpu.list.cast::<ID3D12CommandList>() else { return };
    queue.ExecuteCommandLists(&[Some(as_list)]);
    gpu.value += 1;
    if let Err(e) = queue.Signal(&gpu.fence, gpu.value) {
        gpu.value -= 1;
        fail_job(&mut b, format!("signal: {e}"));
        return;
    }
    let now = win::now_ms();
    let written: Vec<usize> = ready.iter().map(|r| r.resource.as_raw() as usize).collect();
    for f in b.targets.iter_mut() {
        if let Some(r) = ready.iter().find(|r| r.resource.as_raw() as usize == f.t.resource) {
            f.applied = Some((r.art.sha256.clone(), now));
        }
    }
    b.uploads += 1;
    // the check's first read lands with this upload's fence; what each target's blocks should hold comes from the
    // chain written into it
    b.verify_targets = ready
        .iter()
        .take(VERIFY_TARGETS)
        .map(|r| {
            let chain = &r.art.chains[r.chain];
            let expect: [Vec<u8>; 2] =
                std::array::from_fn(|k| if VERIFY_MIPS[k] < r.mips { expected_block(chain, VERIFY_MIPS[k] + r.skip, r.swizzle) } else { Vec::new() });
            VerifyTarget {
                resource: r.resource.as_raw() as usize,
                states: r.states,
                mips: r.mips,
                format: r.fp[0].Footprint.Format.0,
                rows: verify_rows(r.format.0),
                expect,
                now: "not yet".into(),
                later: "not yet".into(),
            }
        })
        .collect();
    b.verify_label = art.label.clone();
    b.verify_stage = 1;
    b.verify_now_at = 0;
    b.verify_later_at = 0;
    gpu.verify_value = gpu.value;
    VERIFY.store(true, Ordering::Release);
    let record_us = t0.elapsed().as_micros() as u64;
    b.last_upload = json!({
        "label": art.label,
        "sha256": art.sha256,
        "resources": written.iter().map(|r| format!("{r:#x}")).collect::<Vec<_>>(),
        "kinds": ready.iter().map(|r| r.kind).collect::<Vec<_>>(),
        "states": ready.iter().map(|r| json!({"states": states_text(&r.states), "source": r.source.name(), "kind": r.kind, "format": r.format.0, "mips": r.mips, "chain": r.art.chains[r.chain].format.name(), "swizzled": r.swizzle})).collect::<Vec<_>>(),
        "bytes": total,
        "fill_us": filled_us,
        "frame_us": record_us,
        "fence": gpu.value,
        "waited_ms": now.saturating_sub(b.job_since),
        "skipped": problems,
    });
    let line = format!(
        "art {:?} written into {} texture(s) ({}; {} us in the frame){}",
        art.label,
        written.len(),
        ready.iter().map(|r| r.kind).collect::<Vec<_>>().join(", "),
        record_us,
        if problems.is_empty() { String::new() } else { format!("; skipped: {}", problems.join("; ")) }
    );
    b.say_later(line);
    b.job = None;
    b.last_error.clear();
    PENDING.store(false, Ordering::Release);
}

/// How many rows the check's block takes in a format: 1 for BC7 (one 16-byte block), 4 for 4-byte texels (4 rows of
/// 16 bytes).
fn verify_rows(dxgi: i32) -> usize {
    match ChainFormat::family_of(dxgi) {
        Some(ChainFormat::Bc7) | Some(ChainFormat::Bc1) => 1,
        _ => 4,
    }
}

/// What the check's block of level `m` should read back: the chain's first 16 bytes of each of the block's rows (red
/// and blue swapped when the chain was swizzled on the way).
fn expected_block(chain: &Chain, m: usize, swizzle: bool) -> Vec<u8> {
    let start = chain.level_offset(m);
    let (w, h) = ((chain.width as usize >> m).max(1), (chain.height as usize >> m).max(1));
    let (rows, row) = chain.format.level_rows(w, h);
    let bc = matches!(chain.format, ChainFormat::Bc7 | ChainFormat::Bc1);
    let block_rows = if bc { 1 } else { 4.min(rows) };
    let mut out = Vec::with_capacity(block_rows * 16);
    for y in 0..block_rows {
        let at = start + y * row;
        let take = if chain.format == ChainFormat::Bc1 { 8 } else { 16.min(row) };
        out.extend_from_slice(&chain.bytes[at..at + take]);
    }
    if swizzle && !bc {
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    out
}

/// GetDesc of every target on the render thread (its real format and mip count, for the status and the art's
/// chains), when the worker asks for it (the targets changed).
unsafe fn describe_targets(device: &ID3D12Device) {
    let Ok(mut b) = cell().try_lock() else { return };
    let Ok(mut g) = GPU.try_lock() else { return };
    if g.is_none() {
        match Gpu::new(device) {
            Ok(x) => *g = Some(x),
            Err(e) => {
                b.say_later(format!("cannot make our GPU objects: {e}"));
                DESCRIBE.store(false, Ordering::Release);
                return;
            }
        }
    }
    let gpu = g.as_ref().unwrap();
    let mut lines = Vec::new();
    for f in b.targets.iter_mut() {
        let vt = win::read_ptr(f.t.resource).unwrap_or(0);
        if vt == 0 || !gpu.resource_vtables.contains(&vt) {
            if f.t.is_raw() && f.refused.is_empty() {
                f.refused = format!("resource {:#x}: its vtable {vt:#x} is not a D3D12 resource's ({:#x?})", f.t.resource, gpu.resource_vtables);
                lines.push(f.refused.clone());
            }
            continue;
        }
        let raw = f.t.resource as *mut c_void;
        let Some(r) = ID3D12Resource::from_raw_borrowed(&raw) else { continue };
        let d = r.GetDesc();
        f.dxgi_format = d.Format.0;
        f.mips = d.MipLevels;
        f.width = d.Width as u32;
        f.height = d.Height;
        f.described = true;
        if f.t.is_main() && d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D {
            DECK_W.store(d.Width, Ordering::Relaxed);
            DECK_H.store(d.Height as u64, Ordering::Relaxed);
        }
        // a target by hand must be the presenting device's (ours are made on it)
        if f.t.manual && !f.device_checked {
            f.device_checked = true;
            let mut dev: Option<ID3D12Device> = None;
            let same = match r.GetDevice(&mut dev) {
                Ok(()) => {
                    dev.and_then(|d| d.cast::<windows::core::IUnknown>().ok()).map(|u| u.as_raw())
                        == device.cast::<windows::core::IUnknown>().ok().map(|u| u.as_raw())
                }
                Err(_) => false,
            };
            if !same {
                f.wrong_device = true;
                f.refused = format!("resource {:#x}: its device is not the presenting one: not touched", f.t.resource);
                lines.push(f.refused.clone());
            }
        }
    }
    for l in lines {
        b.say_later(l);
    }
    DESCRIBE.store(false, Ordering::Release);
}

fn fail_job(b: &mut Board, why: String) {
    b.say_later(why.clone());
    b.last_error = why;
    b.job = None;
    PENDING.store(false, Ordering::Release);
}

/// At unload, after the overlay's hooks are out: waits for our last upload (at most a second) and releases our GPU
/// objects; if the GPU is not done, they are left alive (never freed under the GPU).
pub fn shutdown() {
    ACTIVE.store(false, Ordering::Release);
    PENDING.store(false, Ordering::Release);
    for w in &WATCH {
        w.store(0, Ordering::Release);
    }
    // the references the hooks took for the describe queue go (no GetDesc now), then every destruction callback
    // (the runtime must never call into the unloaded DLL)
    drain_wanted(None, 0);
    unwatch_all();
    let g = GPU.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(g) = g {
        if g.wait(1000) {
            drop(g);
        } else {
            crate::worker::log("board: our last upload had not finished after 1 s: its objects are left alive");
            std::mem::forget(g);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_hash() {
        let s = mip_sizes();
        assert_eq!(s[0], 512 * 2048);
        assert_eq!(s[1], 256 * 1024);
        assert_eq!(&s[8..], &[32, 16, 16, 16], "the small levels: 2 x 8, 1 x 4, 1 x 2, 1 x 1 (one or two blocks)");
        assert_eq!(chain_len(), 1_398_160);
        // the runtime texture of "000 - Colour Test - Plain FF0000" held this at +0x68 (read live, 2026-10-21)
        assert_eq!(
            owner_hash("characters/customization/reskate/board_bottomart/000_-_colour_test_-_plain_ff0000_role3_2259d419-387f-b1ba-30fc-c5140100b012"),
            0xa478_d192_b4f9_2d28
        );
    }

    #[test]
    fn states() {
        assert_eq!(parse_state("auto"), Ok(None));
        assert_eq!(parse_state("common"), Ok(Some(0)));
        assert_eq!(parse_state("SRV"), Ok(Some(0xc0)));
        assert_eq!(parse_state("0x80"), Ok(Some(0x80)));
        assert_eq!(parse_state("2755"), Ok(Some(0xac3)));
        assert!(parse_state("0x400").is_err(), "COPY_DEST is a write state");
        assert!(parse_state("0x4").is_err(), "RENDER_TARGET");
        assert!(parse_state("rt").is_err());
        let rc2 = Some((8, 8));
        let none = Observed::default();
        // nothing seen: RenderCore2's shader resource is 0xc0 on the direct queue
        assert_eq!(decide(None, &none, rc2), Ok(([0xc0; LEVELS], Source::Inferred)));
        assert!(decide(None, &none, Some((8, 2))).is_err());
        assert!(decide(None, &none, None).is_err());
        // by hand wins
        assert_eq!(decide(Some(0), &none, None), Ok(([0; LEVELS], Source::Manual)));
        // seen on every mip
        let mut seen = Observed::default();
        note(&mut seen.states, ALL_SUBRESOURCES, 0);
        assert_eq!(decide(None, &seen, rc2), Ok(([0; LEVELS], Source::Observed)));
        // seen on one mip: the others inferred, or unknown without RenderCore2
        let mut one = Observed::default();
        note(&mut one.states, 3, 0x80);
        let (s, src) = decide(None, &one, rc2).unwrap();
        assert_eq!((s[3], s[0], src), (0x80, 0xc0, Source::Observed));
        assert!(decide(None, &one, None).is_err());
        // the game writing it: not touched
        let mut writing = Observed::default();
        note(&mut writing.states, ALL_SUBRESOURCES, 0x400);
        assert!(decide(None, &writing, rc2).is_err());
        // enhanced barriers: refused unless set by hand
        let enhanced = Observed { layout: Some(6), ..Observed::default() };
        assert!(decide(None, &enhanced, rc2).is_err());
        assert!(decide(Some(0xc0), &enhanced, rc2).is_ok());
    }

    /// A fake chain in this process: found by the checks, and refused for each thing that does not match.
    #[test]
    fn chain_checks() {
        let name = std::ffi::CString::new(TEXTURE_NAME).unwrap();
        let other = std::ffi::CString::new(TEXTURE_NAME.replace("the_caveman", "the_cavewoman")).unwrap();
        let marks = vec![0u64; 8];
        let base = marks.as_ptr() as usize;
        let descriptor = vec![TEXTURE_ASSET_HASH as u64, 0];
        let type_info = vec![descriptor.as_ptr() as u64, 0];
        let ids = Ids {
            asset_vtable: base,
            type_info: type_info.as_ptr() as usize,
            descriptor: descriptor.as_ptr() as usize,
            texture_vtable: base + 8,
            rc2_vtable: base + 16,
        };
        let fake_resource = vec![0x1234u64, 0];
        let mut rc2 = vec![0u64; 0x28];
        rc2[0] = ids.rc2_vtable as u64;
        rc2[layout::RC2_BIND / 8] = 8 << 32;
        rc2[layout::RC2_STATE / 8] = 8;
        rc2[layout::RC2_D3D12 / 8] = fake_resource.as_ptr() as u64;
        let mut tex = vec![0u64; 0x24];
        let put = |t: &mut Vec<u64>, off: usize, v: u32| {
            let w = &mut t[off / 8];
            let shift = (off % 8) * 8;
            *w = (*w & !(0xffff_ffffu64 << shift)) | ((v as u64) << shift);
        };
        tex[0] = ids.texture_vtable as u64;
        for (off, v) in [
            (layout::TEX_FORMAT, 67),
            (layout::TEX_WIDTH, 512),
            (layout::TEX_HEIGHT, 2048),
            (layout::TEX_DEPTH, 1),
            (layout::TEX_ARRAY, 1),
            (layout::TEX_MIPS, 12),
        ] {
            put(&mut tex, off, v);
        }
        tex[layout::TEX_NAME_HASH / 8] = owner_hash(TEXTURE_NAME);
        tex[layout::TEX_D3D12 / 8] = fake_resource.as_ptr() as u64;
        tex[layout::TEX_RC2 / 8] = rc2.as_ptr() as u64;
        let mut asset = vec![ids.asset_vtable as u64, ids.type_info as u64, 0, name.as_ptr() as u64, tex.as_ptr() as u64, 0, 0];
        let t = read_chain(asset.as_ptr() as usize, &ids).unwrap();
        assert_eq!(
            (t.texture, t.resource, t.rc2, t.rc2_bind, t.rc2_state),
            (tex.as_ptr() as usize, fake_resource.as_ptr() as usize, rc2.as_ptr() as usize, 8, 8)
        );
        let refused = |asset: &Vec<u64>| read_chain(asset.as_ptr() as usize, &ids).unwrap_err();
        // another name
        asset[3] = other.as_ptr() as u64;
        assert_eq!(refused(&asset), "another texture");
        asset[3] = name.as_ptr() as u64;
        // each field of the runtime texture
        for (off, bad, what) in [
            (layout::TEX_FORMAT, 66, "format"),
            (layout::TEX_WIDTH, 1024, "width"),
            (layout::TEX_HEIGHT, 1024, "height"),
            (layout::TEX_MIPS, 11, "mip count"),
            (layout::TEX_ARRAY, 2, "array size"),
        ] {
            let keep = tex[off / 8];
            put(&mut tex, off, bad);
            assert!(refused(&asset).contains(what), "{what}: {}", refused(&asset));
            tex[off / 8] = keep;
        }
        tex[layout::TEX_NAME_HASH / 8] ^= 1;
        assert!(refused(&asset).contains("name hash"));
        tex[layout::TEX_NAME_HASH / 8] ^= 1;
        tex[0] = 7;
        assert!(refused(&asset).contains("not a runtime texture"));
        tex[0] = ids.texture_vtable as u64;
        // not a TextureAsset by type
        asset[1] = 0;
        assert!(refused(&asset).starts_with("not a TextureAsset"));
        asset[1] = ids.type_info as u64;
        // a RenderCore2 object that names another resource is not taken
        rc2[layout::RC2_D3D12 / 8] = 0x5678;
        assert_eq!(read_chain(asset.as_ptr() as usize, &ids).unwrap().rc2, 0);
        drop((marks, descriptor, type_info, fake_resource, rc2, tex, asset));
    }

    /// A fake runtime texture of The Caveman's shape (or another width), with its own fake resource.
    fn fake_texture(ids: &Ids, width: u32, name_hash: u64) -> (Vec<u64>, Vec<u64>) {
        let resource = vec![0x1234u64, 0];
        let mut tex = vec![0u64; 0x24];
        let put = |t: &mut Vec<u64>, off: usize, v: u32| {
            let w = &mut t[off / 8];
            let shift = (off % 8) * 8;
            *w = (*w & !(0xffff_ffffu64 << shift)) | ((v as u64) << shift);
        };
        tex[0] = ids.texture_vtable as u64;
        for (off, v) in [
            (layout::TEX_FORMAT, 67),
            (layout::TEX_WIDTH, width),
            (layout::TEX_HEIGHT, 2048),
            (layout::TEX_DEPTH, 1),
            (layout::TEX_ARRAY, 1),
            (layout::TEX_MIPS, 12),
        ] {
            put(&mut tex, off, v);
        }
        tex[layout::TEX_NAME_HASH / 8] = name_hash;
        tex[layout::TEX_D3D12 / 8] = resource.as_ptr() as u64;
        (tex, resource)
    }

    #[test]
    fn siblings_are_the_same_shaped_textures_next_to_the_main_one() {
        let marks = vec![0u64; 8];
        let base = marks.as_ptr() as usize;
        let ids = Ids { asset_vtable: base, type_info: 0, descriptor: 0, texture_vtable: base + 8, rc2_vtable: base + 16 };
        let (main_tex, main_res) = fake_texture(&ids, 512, owner_hash(TEXTURE_NAME));
        let main = Target {
            asset: 0,
            texture: main_tex.as_ptr() as usize,
            resource: main_res.as_ptr() as usize,
            rc2: 0,
            rc2_bind: 0,
            rc2_state: 0,
            sibling: false,
            composite: false,
            id: 0,
            manual: false,
            is_final: false,
            hash: 0,
            owner: 0,
            owner_texture: false,
            is_worldcopy: false,
            is_ridden: false,
            slot: Slot::Art,
        };
        let (s1, s1_res) = fake_texture(&ids, 512, 0xb01d_f88f_6992_cc49);
        let (s2, s2_res) = fake_texture(&ids, 512, 0x0d3c_3d39_3677_0c86);
        let (other, other_res) = fake_texture(&ids, 1024, 0x88bf_704b_207a_0c3d);
        let junk = vec![7u64; 4];
        // a material's texture table: entries of 0x30 bytes, the texture pointer at +0x18 (word 3)
        let mut table = vec![0u64; 6 * 6];
        let entry = |table: &mut Vec<u64>, k: usize, hash: u64, tex: u64| {
            table[k * 6 + 1] = hash;
            table[k * 6 + 3] = tex;
            table[k * 6 + 4] = 1;
        };
        entry(&mut table, 0, 1, other.as_ptr() as u64);
        entry(&mut table, 1, 2, s1.as_ptr() as u64);
        entry(&mut table, 2, 3, main.texture as u64);
        entry(&mut table, 3, 4, s2.as_ptr() as u64);
        entry(&mut table, 4, 5, junk.as_ptr() as u64);
        entry(&mut table, 5, 6, main.texture as u64); // the same pointer again: no duplicate siblings
        let hits = [table.as_ptr() as usize + (2 * 6 + 3) * 8, table.as_ptr() as usize + (5 * 6 + 3) * 8];
        let found = siblings_around(&hits, &ids, &main, 4);
        let textures: Vec<usize> = found.iter().map(|t| t.texture).collect();
        assert_eq!(textures, vec![s1.as_ptr() as usize, s2.as_ptr() as usize], "{found:?}");
        assert!(found.iter().all(|t| t.sibling && t.asset == 0));
        assert_eq!(found[0].resource, s1_res.as_ptr() as usize);
        assert_eq!(found[1].resource, s2_res.as_ptr() as usize);
        // the cap
        assert_eq!(siblings_around(&hits, &ids, &main, 1).len(), 1);
        // read_sibling refuses the wrong shape and what is not a texture
        assert!(read_sibling(other.as_ptr() as usize, &ids).unwrap_err().contains("width"));
        assert!(read_sibling(junk.as_ptr() as usize, &ids).unwrap_err().contains("vtable"));
        assert!(read_sibling(0, &ids).is_err());
        drop((marks, main_tex, main_res, s1, s1_res, s2, s2_res, other, other_res, junk, table));
    }

    #[test]
    fn finals_are_the_copies_with_the_known_name_hashes_newest_first() {
        let marks = vec![0u64; 8];
        let base = marks.as_ptr() as usize;
        let ids = Ids { asset_vtable: base, type_info: 0, descriptor: 0, texture_vtable: base + 8, rc2_vtable: base + 16 };
        let (main_tex, main_res) = fake_texture(&ids, 512, owner_hash(TEXTURE_NAME));
        let main = Target {
            asset: 0,
            texture: main_tex.as_ptr() as usize,
            resource: main_res.as_ptr() as usize,
            rc2: 0,
            rc2_bind: 0,
            rc2_state: 0,
            sibling: false,
            composite: false,
            id: 0,
            manual: false,
            is_final: false,
            hash: 0,
            owner: 0,
            owner_texture: false,
            is_worldcopy: false,
            is_ridden: false,
            slot: Slot::Art,
        };
        // three of the first hash (ids 5, 7, 6: the two newest are taken), one of the second, a sibling-shaped texture
        // of another hash and a wrong-shaped one with a final's hash (neither a final)
        let mut a5 = fake_texture(&ids, 512, FINAL_HASHES[0]);
        let mut a7 = fake_texture(&ids, 512, FINAL_HASHES[0]);
        let mut a6 = fake_texture(&ids, 512, FINAL_HASHES[0]);
        let mut b8 = fake_texture(&ids, 512, FINAL_HASHES[1]);
        let other = fake_texture(&ids, 512, 0x88bf_704b_207a_0c3d);
        let wrong = fake_texture(&ids, 1024, FINAL_HASHES[1]);
        for (t, id) in [(&mut a5, 5u32), (&mut a7, 7), (&mut a6, 6), (&mut b8, 8)] {
            t.0[layout::TEX_ID / 8] = (t.0[layout::TEX_ID / 8] & !0xffff_ffffu64) | id as u64;
        }
        let hits: Vec<usize> = [&a5, &a7, &a6, &b8, &other, &wrong].iter().map(|t| t.0.as_ptr() as usize).chain([main.texture]).collect();
        let found = finals_among(&hits, &ids, &main, &FINAL_HASHES, FINALS_PER_HASH);
        let got: Vec<(u32, u64)> = found.iter().map(|t| (t.id, t.hash)).collect();
        assert_eq!(got, vec![(8, FINAL_HASHES[1]), (7, FINAL_HASHES[0]), (6, FINAL_HASHES[0])], "{found:?}");
        assert!(found.iter().all(|t| t.is_final && !t.sibling && t.kind() == "final"));
        assert!(read_final(other.0.as_ptr() as usize, &ids, &FINAL_HASHES).unwrap_err().contains("name hash"));
        assert!(read_final(wrong.0.as_ptr() as usize, &ids, &FINAL_HASHES).unwrap_err().contains("width"));
        // a final takes the main's rule (inferred from RenderCore2); a composite never; a target by hand only when its
        // format is a composite's
        let none = Observed::default();
        assert_eq!(decide_composite(None, &none, Some((8, 8)), never_inferred(&found[0], 99), false), Ok(([0xc0; LEVELS], Source::Inferred)));
        let by_hand = Target { manual: true, ..main };
        assert!(never_inferred(&by_hand, 28) && !never_inferred(&by_hand, 99) && never_inferred(&Target { composite: true, ..main }, 99));
        // a target from a raw resource pointer: the barriers seen on it, else the uploads table's last state, else
        // the one by hand, else unknown; a write state from the table is not usable
        let raw = Target { texture: 0, ..by_hand };
        assert!(raw.is_raw() && !by_hand.is_raw() && raw.kind() == "manual");
        assert_eq!(decide_raw(None, &none, Some(0xc0)), Ok(([0xc0; LEVELS], Source::LastWrite)));
        assert_eq!(decide_raw(Some(0), &none, Some(0xc0)), Ok(([0xc0; LEVELS], Source::LastWrite)), "the table before the state by hand");
        assert_eq!(decide_raw(Some(0), &none, None), Ok(([0; LEVELS], Source::Manual)));
        assert!(decide_raw(None, &none, None).unwrap_err().contains("board uploads"));
        assert!(decide_raw(None, &none, Some(0x400)).unwrap_err().contains("0x400"));
        let seen = Observed { states: [Some(0x40); LEVELS], transitions: 1, ..Default::default() };
        assert_eq!(decide_raw(None, &seen, Some(0xc0)), Ok(([0x40; LEVELS], Source::Observed)), "what was seen on it since comes first");
        assert!(read_raw(0).is_err() && read_raw(0x10).is_err() && read_raw(0x1234_5678_0000_0001).is_err());
        // the hunt's second encoding of a colour: sRGB bytes linearised (3cf08a -> about 11/222/64)
        assert_eq!([60u8, 240, 138].map(srgb_to_linear), [12, 222, 65]);
        assert_eq!((srgb_to_linear(0), srgb_to_linear(255)), (0, 255));
        assert_eq!((hunt_tolerance(None, 99), hunt_tolerance(None, 71), hunt_tolerance(Some(5), 71)), (24, 32, 5));
        // the world copy: the main's size, format code 54, a name hash of the list; newest first, at most 2
        let mut w1 = fake_texture(&ids, 512, DEFAULT_WORLDCOPY_HASHES[0]);
        let mut w2 = fake_texture(&ids, 512, DEFAULT_WORLDCOPY_HASHES[0]);
        let bc7_named = fake_texture(&ids, 512, DEFAULT_WORLDCOPY_HASHES[0]);
        for (t, id) in [(&mut w1, 3u32), (&mut w2, 9)] {
            t.0[layout::TEX_ID / 8] = (t.0[layout::TEX_ID / 8] & !0xffff_ffffu64) | id as u64;
            let w = &mut t.0[layout::TEX_FORMAT / 8];
            let shift = (layout::TEX_FORMAT % 8) * 8;
            *w = (*w & !(0xffff_ffffu64 << shift)) | ((WORLDCOPY_FORMAT as u64) << shift);
        }
        let hits: Vec<usize> = [&w1, &w2, &bc7_named].iter().map(|t| t.0.as_ptr() as usize).collect();
        let wc = worldcopies_among(&hits, &ids, &main, &DEFAULT_WORLDCOPY_HASHES, WORLDCOPIES_MAX);
        assert_eq!(wc.iter().map(|t| t.id).collect::<Vec<_>>(), vec![9, 3], "{wc:?}");
        assert!(wc.iter().all(|t| t.is_worldcopy && t.kind() == "worldcopy" && t.hash == DEFAULT_WORLDCOPY_HASHES[0]));
        assert!(read_worldcopy(bc7_named.0.as_ptr() as usize, &ids, &DEFAULT_WORLDCOPY_HASHES).unwrap_err().contains("format"));
        assert!(
            ChainFormat::family_of(71) == Some(ChainFormat::Bc1)
                && chain_len_for(ChainFormat::Bc1, 512, 2048, 12) == chain_len_for(ChainFormat::Bc7, 512, 2048, 12) / 2
        );
        drop((w1, w2, bc7_named));
        // a raw target's owner: the runtime texture naming its resource at +0xd0, alive until it names another
        let mut owned = fake_texture(&ids, 512, 7);
        let rt = Target { texture: 0, resource: owned.1.as_ptr() as usize, owner: owned.0.as_ptr() as usize, owner_texture: true, ..by_hand };
        assert!(rt.is_raw() && owner_alive(&rt, &ids));
        owned.0[layout::TEX_D3D12 / 8] = 0;
        assert!(!owner_alive(&rt, &ids));
        assert!(owner_alive(&Target { owner: 0, ..rt }, &ids), "forced: no owner to check");
        assert!(owner_alive(&by_hand, &ids), "not raw: no owner needed");
        drop(owned);
        drop((marks, main_tex, main_res, a5, a7, a6, b8, other, wrong));
    }

    #[test]
    fn chain_sizes_per_format() {
        assert_eq!(chain_sizes(ChainFormat::Bc7, 512, 2048, 12), mip_sizes().to_vec());
        assert_eq!(chain_len_for(ChainFormat::Bc7, 512, 2048, 12), 1_398_160);
        let rgba = chain_sizes(ChainFormat::Rgba8, 512, 2048, 12);
        assert_eq!(rgba[0], 512 * 2048 * 4);
        assert_eq!(rgba[1], 256 * 1024 * 4);
        assert_eq!(&rgba[8..], &[2 * 8 * 4, 4 * 4, 2 * 4, 4], "the small levels: 2 x 8, 1 x 4, 1 x 2, 1 x 1");
        assert_eq!(chain_len_for(ChainFormat::Rgba8, 512, 2048, 10), rgba[..10].iter().sum::<usize>());
        assert_eq!(chain_len_for(ChainFormat::Bgra8, 512, 2048, 12), chain_len_for(ChainFormat::Rgba8, 512, 2048, 12));
        assert_eq!(ChainFormat::Rgba8.level_rows(512, 2048), (2048, 2048));
        assert_eq!(ChainFormat::Bc7.level_rows(512, 2048), (512, 2048));
        assert_eq!(
            (ChainFormat::from_name("RGBA8"), ChainFormat::from_name("bc7"), ChainFormat::from_name("png")),
            (Some(ChainFormat::Rgba8), Some(ChainFormat::Bc7), None)
        );
        // the families: BC7 97..99, R8G8B8A8 27..29, B8G8R8A8/X8 87, 88, 90..93
        assert_eq!(ChainFormat::family_of(99), Some(ChainFormat::Bc7));
        assert_eq!(ChainFormat::family_of(28), Some(ChainFormat::Rgba8));
        assert_eq!(ChainFormat::family_of(29), Some(ChainFormat::Rgba8));
        assert_eq!(ChainFormat::family_of(91), Some(ChainFormat::Bgra8));
        assert_eq!(ChainFormat::family_of(18), None, "the runtime texture's format code 18 is Frostbite's, not DXGI's");
        assert!(composite_format_ok(28) && composite_format_ok(87) && !composite_format_ok(99));
        let c = Chain { format: ChainFormat::Rgba8, width: 512, height: 2048, levels: 12, bytes: Vec::new() };
        assert_eq!(c.level_offset(0), 0);
        assert_eq!(c.level_offset(2), rgba[0] + rgba[1]);
    }

    #[test]
    fn slots_name_the_cavemans_four_textures() {
        assert_eq!(Slot::Art.texture_name(), TEXTURE_NAME);
        assert_eq!(Slot::Osk.texture_name(), TEXTURE_NAME.replace("2259d419-387f-b1ba-30fc-c5140100b012", "c1b71b8a-395f-e8ce-30fc-c5140100a381"));
        for s in Slot::ALL {
            assert_eq!(Slot::of_name(&s.texture_name()), Some(s));
            assert_eq!(Slot::parse(s.name()), Some(s));
            assert_eq!(s.texture_name().len(), TEXTURE_NAME.len(), "every slot's name is as long as the art's (the names are read with one length)");
        }
        assert_eq!(Slot::parse(""), Some(Slot::Art));
        assert_eq!(
            (Slot::parse("mask"), Slot::parse("normal"), Slot::parse("holo"), Slot::parse("x")),
            (Some(Slot::Osk), Some(Slot::Ny), Some(Slot::Rgb), None)
        );
        assert_eq!(Slot::of_name("characters/other"), None);
        assert_eq!((Slot::Art.fb_format(), Slot::Ny.fb_format()), (FB_BC7_SRGB, FB_BC7_LINEAR));
    }

    #[test]
    fn the_chain_that_fits_a_target() {
        let art = Art {
            chains: vec![
                Chain { format: ChainFormat::Bc7, width: 512, height: 2048, levels: 12, bytes: vec![1; chain_len()] },
                Chain { format: ChainFormat::Rgba8, width: 512, height: 2048, levels: 12, bytes: vec![2; chain_len_for(ChainFormat::Rgba8, 512, 2048, 12)] },
            ],
            label: "t".into(),
            sha256: String::new(),
            slot: Slot::Art,
        };
        // BC7 targets take the BC7 chain; RGBA8 targets of any mip count up to 12 the RGBA8 one; BGRA8 the same swizzled
        assert!(matches!(art.pick_chain(99, 12, 512, 2048), Ok((c, false, 0)) if c.format == ChainFormat::Bc7));
        assert!(matches!(art.pick_chain(28, 10, 512, 2048), Ok((c, false, 0)) if c.format == ChainFormat::Rgba8));
        assert!(matches!(art.pick_chain(29, 12, 512, 2048), Ok((c, false, 0)) if c.format == ChainFormat::Rgba8));
        // the ridden copy demoted by the streamer: the tail of the full RGBA8 chain (its level 1 is 256 x 1024)
        assert!(matches!(art.pick_chain(29, 9, 256, 1024), Ok((c, false, 1)) if c.format == ChainFormat::Rgba8));
        assert!(matches!(art.pick_chain(29, 8, 128, 512), Ok((c, false, 2)) if c.format == ChainFormat::Rgba8));
        assert!(art.pick_chain(29, 12, 256, 1024).unwrap_err().contains("no rgba8 chain of 256x1024"));
        assert!(matches!(art.pick_chain(87, 10, 512, 2048), Ok((c, true, 0)) if c.format == ChainFormat::Rgba8));
        assert!(art.pick_chain(56, 12, 512, 2048).unwrap_err().contains("DXGI format 56"));
        // too few levels in the chain: no fit
        let short = Art {
            chains: vec![Chain { format: ChainFormat::Rgba8, width: 512, height: 2048, levels: 4, bytes: Vec::new() }],
            label: "s".into(),
            sha256: String::new(),
            slot: Slot::Art,
        };
        assert!(short.pick_chain(28, 10, 512, 2048).unwrap_err().contains("no rgba8 chain of 512x2048 with 10 or more levels"));
        assert!(short.pick_chain(28, 4, 512, 2048).is_ok());
        // what the check expects of a level's block: the chain's first 16 bytes of its first rows (swizzled when the
        // chain is in the other order)
        let mut bytes = vec![0u8; chain_len_for(ChainFormat::Rgba8, 512, 2048, 12)];
        for (i, b) in bytes.iter_mut().enumerate().take(4096) {
            *b = (i % 251) as u8;
        }
        let c = Chain { format: ChainFormat::Rgba8, width: 512, height: 2048, levels: 12, bytes };
        let e = expected_block(&c, 0, false);
        assert_eq!(e.len(), 64);
        assert_eq!(&e[..16], &c.bytes[..16]);
        assert_eq!(&e[16..32], &c.bytes[2048..2064], "the second row of the 512-wide level");
        let s = expected_block(&c, 0, true);
        assert_eq!((s[0], s[1], s[2], s[3]), (e[2], e[1], e[0], e[3]));
        let bc = Chain { format: ChainFormat::Bc7, width: 512, height: 2048, levels: 12, bytes: vec![9; chain_len()] };
        assert_eq!(expected_block(&bc, 2, false).len(), 16);
        assert_eq!(verify_rows(99), 1);
        assert_eq!(verify_rows(28), 4);
        // the verdicts: a match, zeros right after the write, a change later after a match, a plain mismatch
        let expect = [vec![7u8; 16], vec![7u8; 16]];
        assert_eq!(verify_verdict(&[vec![7u8; 16], vec![7u8; 16]], &expect, 12, false, false), "match");
        assert!(verify_verdict(&[vec![0u8; 16], vec![7u8; 16]], &expect, 12, false, false)
            .starts_with("ZEROS right after the write (the copy did not take) at mip 0 byte 0"));
        assert!(
            verify_verdict(&[vec![7u8; 16], vec![1u8; 16]], &expect, 12, true, true).starts_with("changed later (the game rendered it again) at mip 2 byte 0")
        );
        assert!(verify_verdict(&[vec![7u8; 16], vec![1u8; 16]], &expect, 12, true, false).starts_with("mismatch at mip 2"));
        assert_eq!(verify_verdict(&[vec![7u8; 16], vec![1u8; 16]], &expect, 2, false, false), "match", "mip 2 is not there");
        // the states set by hand: a resource over a kind over every target
        let mut b = Board::default();
        let comp = Target { composite: true, resource: 0x10, ..Default::default() };
        let main = Target { resource: 0x20, ..Default::default() };
        assert_eq!((b.manual_for(&comp), b.manual_for(&main)), (None, None));
        b.manual = Some(0xc0);
        b.manual_per.push((ManualFor::Kind("composite".into()), 0));
        assert_eq!((b.manual_for(&comp), b.manual_for(&main)), (Some(0), Some(0xc0)));
        b.manual_per.push((ManualFor::Resource(0x10), 0x80));
        assert_eq!(b.manual_for(&comp), Some(0x80));
        assert_eq!(ManualFor::parse("kind:composite"), Ok(ManualFor::Kind("composite".into())));
        assert_eq!(ManualFor::parse("0x1941123a30"), Ok(ManualFor::Resource(0x1941123a30)));
        assert!(ManualFor::parse("kind:deck").is_err() && ManualFor::parse("deck").is_err());
        // assumed COMMON only when asked and nothing was ever seen
        let none = Observed::default();
        assert_eq!(decide_composite(None, &none, None, true, true).unwrap(), ([0; LEVELS], Source::AssumedCommon));
        assert!(decide_composite(None, &none, None, true, false).is_err());
    }

    #[test]
    fn the_composite_is_a_nameless_texture_of_the_mains_size_with_a_rendercore2_object() {
        let marks = vec![0u64; 8];
        let base = marks.as_ptr() as usize;
        let ids = Ids { asset_vtable: base, type_info: 0, descriptor: 0, texture_vtable: base + 8, rc2_vtable: base + 16 };
        let (main_tex, main_res) = fake_texture(&ids, 512, owner_hash(TEXTURE_NAME));
        let main = Target { texture: main_tex.as_ptr() as usize, resource: main_res.as_ptr() as usize, ..Default::default() };
        // the composite: no name hash, its own format code and mip count, RenderCore2 flags 128 and 256
        let (mut comp, comp_res) = fake_texture(&ids, 512, 0);
        let put = |t: &mut Vec<u64>, off: usize, v: u32| {
            let w = &mut t[off / 8];
            let shift = (off % 8) * 8;
            *w = (*w & !(0xffff_ffffu64 << shift)) | ((v as u64) << shift);
        };
        put(&mut comp, layout::TEX_FORMAT, 18);
        put(&mut comp, layout::TEX_MIPS, 10);
        let mut rc2 = vec![0u64; 0x28];
        rc2[0] = ids.rc2_vtable as u64;
        rc2[layout::RC2_BIND / 8] = 128 << 32;
        rc2[layout::RC2_STATE / 8] = 256;
        rc2[layout::RC2_D3D12 / 8] = comp_res.as_ptr() as u64;
        comp[layout::TEX_RC2 / 8] = rc2.as_ptr() as u64;
        // a nameless texture without a RenderCore2 object, and a named one: neither
        let (loose, loose_res) = fake_texture(&ids, 512, 0);
        let (named, named_res) = fake_texture(&ids, 512, 0x88bf_704b_207a_0c3d);
        let hits = [main.texture, loose.as_ptr() as usize, named.as_ptr() as usize, comp.as_ptr() as usize, 7];
        let found = composites_among(&hits, &ids, &main, 2);
        assert_eq!(found.len(), 1, "{found:?}");
        let t = found[0];
        assert!(t.composite && !t.sibling && t.asset == 0 && t.kind() == "composite");
        assert_eq!((t.resource, t.rc2, t.rc2_bind, t.rc2_state), (comp_res.as_ptr() as usize, rc2.as_ptr() as usize, 128, 256));
        assert!(read_composite(loose.as_ptr() as usize, &ids).unwrap_err().contains("RenderCore2"));
        assert!(read_composite(named.as_ptr() as usize, &ids).unwrap_err().contains("name hash"));
        // its state is never inferred: the game's barriers or a hand-set state
        let none = Observed::default();
        assert!(decide_for(None, &none, Some((128, 256)), true).unwrap_err().contains("re-equip"));
        assert_eq!(decide_for(Some(0xc0), &none, Some((128, 256)), true).unwrap().1, Source::Manual);
        let mut seen = Observed { states: [Some(0xc0); LEVELS], ..Default::default() };
        assert_eq!(decide_for(None, &seen, None, true).unwrap(), ([0xc0; LEVELS], Source::Observed));
        seen.states[0] = Some(0x4);
        assert!(decide_for(None, &seen, None, true).unwrap_err().contains("writing it"));
        assert_eq!(decide_for(None, &none, Some((8, 8)), false).unwrap().1, Source::Inferred, "the others as before");
        // newest first: the higher id
        let mut comp2 = comp.clone();
        let (comp2_res, mut rc2b) = (vec![0x1234u64, 0], rc2.clone());
        rc2b[layout::RC2_D3D12 / 8] = comp2_res.as_ptr() as u64;
        comp2[layout::TEX_D3D12 / 8] = comp2_res.as_ptr() as u64;
        comp2[layout::TEX_RC2 / 8] = rc2b.as_ptr() as u64;
        put(&mut comp, layout::TEX_ID, 0x54255);
        put(&mut comp2, layout::TEX_ID, 0x54300);
        let found = composites_among(&[comp.as_ptr() as usize, comp2.as_ptr() as usize], &ids, &main, 4);
        assert_eq!(found.iter().map(|t| t.id).collect::<Vec<_>>(), vec![0x54300, 0x54255], "newest first");
        assert_eq!(composites_among(&[comp.as_ptr() as usize, comp2.as_ptr() as usize], &ids, &main, 1)[0].id, 0x54300);
        drop((marks, main_tex, main_res, comp, comp_res, rc2, loose, loose_res, named, named_res, comp2, comp2_res, rc2b));
    }

    #[test]
    fn what_was_seen_on_a_resource_survives_a_rebuild_of_the_targets() {
        // fake resources of this test's own (the watch's slots are shared statics: unique addresses)
        let a = 0xF00D_0000_1000usize;
        let gone = 0xF00D_0000_2000usize;
        let fake_barrier = |res: usize, before: u32, after: u32| D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: unsafe { std::mem::transmute_copy(&res) },
                    Subresource: ALL_SUBRESOURCES,
                    StateBefore: D3D12_RESOURCE_STATES(before as i32),
                    StateAfter: D3D12_RESOURCE_STATES(after as i32),
                }),
            },
        };
        ACTIVE.store(true, Ordering::Release);
        watch(&[a, gone]);
        observe(&[fake_barrier(a, 0x4, 0xc0), fake_barrier(gone, 0x8, 0xc0)]);
        assert_eq!(observed(a).states[0], Some(0xc0));
        assert_eq!(observed(a).transitions, 1);
        // the target list rebuilt without them, then with `a` again: what was seen on it is back
        watch(&[]);
        assert_eq!(observed(a).states[0], Some(0xc0), "remembered while not watched");
        watch(&[a]);
        assert_eq!((observed(a).states[0], observed(a).transitions), (Some(0xc0), 1));
        // a resource gone is forgotten; a find by hand forgets all
        forget_observed(gone);
        watch(&[gone]);
        assert_eq!(observed(gone).states[0], None);
        watch(&[]);
        forget_all_observed();
        watch(&[a]);
        assert_eq!(observed(a).transitions, 0);
        watch(&[]);
    }
}
