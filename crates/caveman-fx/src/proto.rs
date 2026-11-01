//! The trainer's line protocol over the named pipe: one JSON request per line, one JSON answer per line.
//!
//! ```text
//! {"cmd":"status"}                          -> {"ok":true, "phase":"active", "inputs":[...], "fx":[...], ...}
//! {"cmd":"fire","fx":"fx.test","ms":3000}   -> {"ok":true, "fx":"fx.test", "ms":3000}
//! {"cmd":"stop","fx":"fx.test"}             (no fx: every override)
//! {"cmd":"rules","rules":{"rules":[...]}}   -> {"ok":true, "rules":13, "inputs":[...]} or {"ok":false,"error":"rule 2 (fx.grind): ..."}
//! {"cmd":"rules_get"} {"cmd":"rescan"} {"cmd":"ping"} {"cmd":"unload"}
//! {"cmd":"draw","test":true}                -> {"ok":true, "render":{...}}   (also "queue":"auto|swapchain|scan|busiest", "reset":true, "streamline":true)
//! {"cmd":"bars","set":{"enabled":true,"gain":2}} -> {"ok":true, "bars":{...settings}, "render":{...}, "audio":{...}}   ("land":true: a landing spike)
//! {"cmd":"depth","on":false}                -> {"ok":true, "render":{...}}   (also "near":0.1: the near plane by hand, 0: the game's own)
//! {"cmd":"board"}                           -> {"ok":true, "board":{...}, "fx":[...]}   The Caveman's state (board.rs)
//! {"cmd":"board","art":{...},"keep":true}   a new graphic (see BoardArt); also "state":"srv"|"auto", "find":true
//! {"cmd":"board","fx":"aura","on":true}     one of its effects on (false: off); "ms":150 instead: on for that long
//! {"cmd":"rim","path":{...}}                -> {"ok":true, "rim":{...}}   the deck's rim for the bars (rim.rs); "clear":true forgets it
//! ```

use std::io::{BufRead, BufReader, Write};

use serde::{Deserialize, Serialize};

use crate::rules::RulesFile;

/// The pipe in Skate.exe. Anywhere else (tests, a host program) the DLL adds the process id, so a test can never talk
/// to the game's DLL.
pub const PIPE: &str = r"\\.\pipe\caveman-fx";

pub fn pipe_for(game: bool, pid: u32) -> String {
    if game {
        PIPE.to_string()
    } else {
        format!("{PIPE}-{pid}")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Ping,
    Status,
    /// Forces an fx state on for `ms` (default 2000).
    Fire {
        fx: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u64>,
    },
    /// Ends an override (all of them without `fx`).
    Stop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fx: Option<String>,
    },
    Rules {
        rules: RulesFile,
    },
    RulesGet,
    Rescan,
    Unload,
    /// The overlay: the screen-space test rectangle on or off, the queue strategy (tests), re-arming after failures.
    Draw {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        test: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reset: Option<bool>,
        /// Also hook Streamline's swap chain wrapper (drawn under ReSkate's menus; see overlay::STREAMLINE).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        streamline: Option<bool>,
    },
    /// The overlay's depth test (hiding the bars behind the game's scene): on or off, and the near plane by hand
    /// (metres; 0: calibrate on the board). Without either: the state.
    Depth {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        near: Option<f32>,
        /// `picture` (every G-buffer pass end copied, the default) or `window` (only the first of each present window).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        copies: Option<String>,
    },
    /// The depth test's diagnostics (diag.rs): `dump` (true, or a file path) writes the record of the last presents
    /// as JSON; `key` names the key that saves the moments around it ("F8", a hex code, "off"); `auto` turns the
    /// saving of the moments around each blink found on or off. Without any: the state.
    Diag {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dump: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auto: Option<bool>,
    },
    /// The bars: settings to change (fields of `bars::BarSettings`), a landing spike by hand. Without either: the
    /// current settings.
    Bars {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        land: Option<bool>,
    },
    /// The Caveman (`board.rs`). Every field is optional; the answer is always the board's state (`board`: found,
    /// equipped, the texture's state, the art, the last upload; `fx`: the fx.cave.* states).
    /// - `art`: a mip chain to put on its graphic, kept and put back after the texture is made again unless `keep` is
    ///   false;
    /// - `state`: the texture's resource state by hand (`common`, `srv`, `0x80`, ...; `auto` to go back);
    /// - `find`: look for The Caveman again now;
    /// - `fx` with `on` (true: on until turned off; false: off) or `ms` (on for that long, e.g. one burst): one of its
    ///   effects (`aura` or `fx.cave.aura`).
    Board {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        art: Option<BoardArt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keep: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        find: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fx: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u64>,
        /// Read a block of the texture back now and compare it with the art written (`board verify`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verify: Option<bool>,
        /// More chains of the same art in other formats (`rgba8`, `bgra8`, up to 12 levels): the DLL picks, per
        /// target, the chain whose format family fits the texture's own (the composite render target is RGBA8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arts: Option<Vec<BoardArt>>,
        /// Also write the siblings (the same-shaped textures next to the main one in a material's table; off by
        /// default since 2026-10-21 night: they turned out to be other decks' graphics).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        siblings: Option<bool>,
        /// What `state` applies to: a resource (`0x...`) or a kind (`kind:composite`, `kind:main`, `kind:sibling`);
        /// without it, every target.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_for: Option<String>,
        /// Write a composite that never shows a barrier (probably kept in COMMON, used through implicit promotion)
        /// from COMMON when its state is automatic (off by default).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composite_common: Option<bool>,
        /// A target added by hand: a runtime texture object's address (`0x...`), any 2D shape of a format the art has
        /// a chain for; `clear` removes every one. A `state` sent with it is for that target.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// Ask the render thread to describe the resources of the uploads ring (`board uploads`: every write the game
        /// ended lately, with each resource's shape).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uploads: Option<bool>,
        /// `board finals on|off`: also write the finals (the run-time-made copies of The Caveman's shape with the
        /// known name hashes, newest first, at most 2 per hash); default on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finals: Option<bool>,
        /// `board copies`: ask the render thread to describe the copy tracer's recent resources.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        copies: Option<bool>,
        /// `board peek`: an N x N block at a texel position of a target's mip read back (the status's `peek`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peek: Option<PeekReq>,
        /// With `target: resource:0x...`: accept it with no live owner when the deck-shaped uploads subset holds it
        /// (the game wrote it lately) and GetDesc passes (`board target --resource 0x... --force`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
        /// `board uploads --owners`: run the owner scan for the uploads table's entries (a few seconds).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owners: Option<bool>,
        /// The slices (the tile arrays the world samples): `match` reads back the corners of the slices written since
        /// the main was last made and finds our art's tiles among them; `write` puts our tiles into the matched
        /// slices (and keeps them there); `clear` forgets them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slices: Option<String>,
        /// The tile arrays' state by hand (`auto`: the uploads table's), for the slices.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slices_state: Option<String>,
        /// `board hunt RRGGBB`: every 2D texture the DLL knows read back at three blocks and searched for a colour.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hunt: Option<HuntReq>,
        /// With `owners`: whose owners (`uploads`, the default, or `hunt`: the hunt's hits).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owners_of: Option<String>,
        /// `board worldcopy on|off`: the BC1 texture the game re-fills at every equip is written too (default on).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worldcopy: Option<bool>,
        /// `board worldcopy hashes 0x...`: the world copies' name hashes (empty: the default one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worldcopy_hashes: Option<Vec<String>>,
        /// `board hunt --abort`: the hunt running is dropped.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hunt_abort: Option<bool>,
    },
    /// The deck's rim for the bars' `rim` layout (`rim.rs`): `path` to use it (the trainer reads it from the player's
    /// install: `bars rim load`), `clear` to forget it (the layout then draws the ring). Without either: its state.
    /// The answer: `{"rim": {...}}`.
    Rim {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<crate::rim::RimPath>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clear: Option<bool>,
    },
    /// The ridden board's live material parameters (material.rs): alone, every parameter of its Deck_Mat instances;
    /// with `key` and `values` (floats), `int` or `raw` (hex bytes), that parameter written and the materials
    /// published (unless `publish` is false), and the parameter read back.
    /// The board cam's meshes (pip.rs): `mesh` sets them (kept for the next load in the game), `clear` forgets them;
    /// neither: the status. The cam itself is switched and shaped through the bars' settings (`pip`, `pip_size`, ...).
    /// `art`: a test host's own texture for the deck's underside (a raw `ID3D12Resource` pointer in hex, "0" for
    /// none), refused in the game, where the cam shows the ridden board's art.
    Pip {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mesh: Option<crate::pip::PipMesh>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clear: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        art: Option<String>,
    },
    /// The allocation trace (alloctrace.rs): `trace` starts one of that many seconds (allocations of `min_kb` and
    /// more), `stop` ends it, `report` writes the file and answers the summary; none of them: the status.
    /// `ring` switches the staging ring (stagering.rs) with `ring_mb` its size; `ring_any` (a test host only) takes
    /// every big upload buffer, not only the game's call site.
    Mem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_kb: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ring: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ring_mb: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ring_any: Option<bool>,
    },
    /// Video on the ridden board (video.rs): `on` opens the producer's ring and uploads its frames every present,
    /// `off` stops (the kept art returns); without `on`, the status.
    Video {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<bool>,
    },
    Material {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Vec<f32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        int: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        publish: Option<bool>,
    },
}

impl Request {
    /// A board request with nothing in it: the board's state.
    pub fn board() -> Request {
        Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        }
    }
}

/// Art for The Caveman: its graphic's whole mip chain, BC7 (sRGB), the levels one after another, each level tightly
/// `board hunt`: the colour looked for (RGB bytes, as given: sRGB), the tolerance per channel (none: 24 for BC7 and
/// 4-byte texels, 32 for BC1 and BC3), and whether every texture the DLL ever described is read (default: those
/// seen since the main was last made or in the last 30 s).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuntReq {
    pub rgb: [u8; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<u8>,
    #[serde(default)]
    pub everything: bool,
    /// The state to read a texture of unknown state from (`srv`, `common`, `0xNN`; none: those are not read), never
    /// a render or depth target's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assume: Option<String>,
    /// The census: read the resources of every runtime texture object in the game's memory (a scan for the runtime
    /// texture vtable, seconds), not the describe cache: surfaces the hooks never saw a barrier on are read too,
    /// each one's owner checked again right before it is read.
    #[serde(default)]
    pub census: bool,
    /// Also read render targets under the assumed state (never a back buffer, never anything screen-sized).
    #[serde(default)]
    pub targets: bool,
}

/// `board peek`: the target's resource (`0x...`), the texel position, the mip and the block's side (1 to 16).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeekReq {
    pub resource: String,
    pub x: u32,
    pub y: u32,
    #[serde(default)]
    pub mip: u32,
    #[serde(default = "four")]
    pub n: u32,
}

fn four() -> u32 {
    4
}

/// packed rows of 4 x 4 blocks (caveman-core's `fxmod::board_art` makes it; 1,398,160 bytes for 512 x 2048, 12 levels),
/// base64 in `mips`, with its SHA-256 (hex) to check it arrived whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardArt {
    #[serde(default)]
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    /// "bc7".
    pub format: String,
    pub sha256: String,
    pub mips: String,
    /// The deck slot the chain is for: "" or "art" (the graphic), "osk" (the mask), "ny" (the normal map), "rgb"
    /// (the holo map). Chains of one slot make one art.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot: String,
}

impl BoardArt {
    pub fn new(label: &str, chain: &[u8]) -> BoardArt {
        let mut h = crate::sha256::Sha256::new();
        h.update(chain);
        BoardArt {
            label: label.to_string(),
            width: crate::board::WIDTH,
            height: crate::board::HEIGHT,
            levels: crate::board::LEVELS as u32,
            format: "bc7".into(),
            sha256: crate::sha256::hex(&h.finish()),
            mips: base64_encode(chain),
            slot: String::new(),
        }
    }

    /// The same chain for another slot of the deck (`osk`, `ny`, `rgb`).
    pub fn for_slot(mut self, slot: &str) -> BoardArt {
        self.slot = slot.to_string();
        self
    }

    /// A chain in another format (`rgba8` or `bgra8`: 4 bytes a texel, tightly packed rows; `bc1`: 8 bytes a 4 x 4
    /// block) or level count.
    pub fn with_format(label: &str, format: &str, levels: u32, chain: &[u8]) -> BoardArt {
        BoardArt::with_shape(label, format, crate::board::WIDTH, crate::board::HEIGHT, levels, chain)
    }

    /// A chain of any 2D shape (for a target added by hand).
    pub fn with_shape(label: &str, format: &str, width: u32, height: u32, levels: u32, chain: &[u8]) -> BoardArt {
        let mut h = crate::sha256::Sha256::new();
        h.update(chain);
        BoardArt {
            label: label.to_string(),
            width,
            height,
            levels,
            format: format.to_string(),
            sha256: crate::sha256::hex(&h.finish()),
            mips: base64_encode(chain),
            slot: String::new(),
        }
    }

    /// The chain's format, when it is one the board writes.
    pub fn chain_format(&self) -> Result<crate::board::ChainFormat, String> {
        crate::board::ChainFormat::from_name(&self.format).ok_or_else(|| format!("art format {:?}: bc7, rgba8 or bgra8", self.format))
    }

    /// The chain, checked: a 2D size, a known format, 1 to 12 levels, the size the format gives, the digest.
    pub fn chain(&self) -> Result<Vec<u8>, String> {
        let format = self.chain_format()?;
        if !(1..=16384).contains(&self.width) || !(1..=16384).contains(&self.height) || !(1..=crate::board::LEVELS as u32).contains(&self.levels) {
            return Err(format!("art {}x{} with {} levels of {}: a 2D size with 1 to 12 levels", self.width, self.height, self.levels, self.format));
        }
        let bytes = base64_decode(&self.mips)?;
        let want = crate::board::chain_len_for(format, self.width, self.height, self.levels as usize);
        if bytes.len() != want {
            return Err(format!("art of {} bytes: a {} chain of {} levels is {} bytes", bytes.len(), self.format, self.levels, want));
        }
        let mut h = crate::sha256::Sha256::new();
        h.update(&bytes);
        let digest = crate::sha256::hex(&h.finish());
        if !digest.eq_ignore_ascii_case(&self.sha256) {
            return Err(format!("art damaged on the way (sha256 {}..., expected {}...)", &digest[..12], &self.sha256[..12.min(self.sha256.len())]));
        }
        Ok(bytes)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Standard base64 (padding optional; whitespace refused).
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let t = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(t.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &ch in t {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("base64: unexpected {:?}", ch as char)),
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

pub const FIRE_MS: u64 = 2000;

/// Sends one request to a pipe and returns the answer (the pipe is opened for this request only).
pub fn request(pipe: &str, req: &Request, wait_ms: u32) -> Result<serde_json::Value, String> {
    let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    request_line(pipe, &line, wait_ms)
}

/// Sends a raw request line.
pub fn request_line(pipe: &str, line: &str, wait_ms: u32) -> Result<serde_json::Value, String> {
    let name = crate::win::wide(pipe);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms as u64);
    let file = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(pipe) {
            Ok(f) => break f,
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    return Err(match e.raw_os_error() {
                        Some(2) => format!("{pipe} is not there (is caveman_fx.dll loaded in the game?)"),
                        _ => format!("{pipe}: {e}"),
                    });
                }
                // busy (another client) or not up yet: wait a little
                unsafe { crate::win::WaitNamedPipeW(name.as_ptr(), 200) };
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    };
    let mut writer = &file;
    writer.write_all(line.trim_end().as_bytes()).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    BufReader::new(&file).read_line(&mut answer).map_err(|e| e.to_string())?;
    if answer.trim().is_empty() {
        return Err("no answer (the DLL closed the pipe)".into());
    }
    serde_json::from_str(answer.trim()).map_err(|e| format!("bad answer {answer:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let r: Request = serde_json::from_str(r#"{"cmd":"fire","fx":"fx.test","ms":3000}"#).unwrap();
        assert_eq!(r, Request::Fire { fx: "fx.test".into(), ms: Some(3000) });
        assert_eq!(serde_json::to_string(&Request::Status).unwrap(), r#"{"cmd":"status"}"#);
        let r: Request = serde_json::from_str(r#"{"cmd":"rules","rules":{"rules":[{"fx":"a","when":"grind"}]}}"#).unwrap();
        assert!(matches!(r, Request::Rules { .. }));
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"explode"}"#).is_err());
        assert_eq!(pipe_for(false, 12), r"\\.\pipe\caveman-fx-12");
        let r: Request = serde_json::from_str(r#"{"cmd":"bars","set":{"gain":2.5}}"#).unwrap();
        assert!(matches!(r, Request::Bars { set: Some(_), land: None }));
        assert_eq!(
            serde_json::to_string(&Request::Draw { test: Some(true), queue: None, reset: None, streamline: None }).unwrap(),
            r#"{"cmd":"draw","test":true}"#
        );
        assert_eq!(serde_json::to_string(&Request::Depth { on: Some(false), near: None, copies: None }).unwrap(), r#"{"cmd":"depth","on":false}"#);
        assert_eq!(serde_json::from_str::<Request>(r#"{"cmd":"depth","near":0.1}"#).unwrap(), Request::Depth { on: None, near: Some(0.1), copies: None });
        assert_eq!(serde_json::to_string(&Request::board()).unwrap(), r#"{"cmd":"board"}"#);
        let r: Request = serde_json::from_str(r#"{"cmd":"board","fx":"aura","on":true}"#).unwrap();
        assert_eq!(
            r,
            Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: Some("aura".into()),
                on: Some(true),
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None
            }
        );
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"board","colour":"ff0000"}"#).is_err());
    }

    #[test]
    fn base64_and_art() {
        for n in 0..10 {
            let data: Vec<u8> = (0..n as u8).map(|k| k.wrapping_mul(37)).collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_decode("TWE").unwrap(), b"Ma");
        assert!(base64_decode("TW E=").is_err());
        // a whole chain survives the trip, and damage is caught
        let chain: Vec<u8> = (0..crate::board::chain_len()).map(|k| (k * 7 + k / 255) as u8).collect();
        let art = BoardArt::new("test", &chain);
        let text = serde_json::to_string(&Request::Board {
            art: Some(art.clone()),
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        })
        .unwrap();
        assert!(text.len() < 4 << 20, "under the pipe's longest line: {} bytes", text.len());
        let back: Request = serde_json::from_str(&text).unwrap();
        let Request::Board { art: Some(a), .. } = back else { panic!() };
        assert_eq!(a.chain().unwrap(), chain);
        let mut bad = art.clone();
        bad.mips.replace_range(100..104, "AAAA");
        assert!(bad.chain().unwrap_err().contains("damaged"));
        let mut short = art;
        short.mips = base64_encode(&chain[..1000]);
        assert!(short.chain().is_err());
    }
}
