//! The shader mod (`shadermod.build_mod`): a `globals.toc` patch of the `delmargameconfiguration` bundle with the six
//! deck programs and the ribbon's pixel and vertex programs replaced by Caveman's patched ones, and an `items.toc`
//! patch of the shared customization bundle with the ribbon's spawn and simulate programs (the banner trail), built
//! from the player's own game, plus the boilerplate every mod folder carries (layout.toc, initfs, the ReSkate stamp,
//! manifest.json, README).

use std::path::Path;

use super::{dxc::Dxc, ShaderError, BYTECODE_TYPE, CONFIG_BUNDLE, GLOBALS_TOC};
use crate::bundle::{self, Endian, FileLoc};
use crate::game::{Game, ITEMS_TOC, PKG};
use crate::{cas, toc};

/// What `build_shader_mod` writes besides the programs (`shadermod.build_mod`'s keyword arguments and clock).
#[derive(Debug, Clone)]
pub struct ShaderModOptions {
    /// The mod's title (manifest.json, README).
    pub title: String,
    /// `reskate-build.json`'s `built` (UTC, `%Y-%m-%dT%H:%M:%SZ`); None for now.
    pub built: Option<String>,
    /// The deck programs with the flipbook for animated decks (deck::patch_anim). Off, the programs are exactly what
    /// they were before the feature.
    pub animated: bool,
}

impl Default for ShaderModOptions {
    fn default() -> ShaderModOptions {
        ShaderModOptions { title: "Caveman Shaders (test)".into(), built: None, animated: false }
    }
}

/// What `build_shader_mod` wrote.
#[derive(Debug, Clone)]
pub struct ShaderModStats {
    /// Bytes of the CAS archive and of the TOCs (`shadermod.build_mod`'s return value): globals.toc (the configuration
    /// bundle) and items.toc (the shared customization bundle: the banner trail's compute programs).
    pub cas: usize,
    pub toc: usize,
    pub items_toc: usize,
    /// Per program: (resource name, note).
    pub programs: Vec<(String, String)>,
}

/// One replaced program: its resource name, payload, meta and note.
pub struct BuiltProgram {
    pub name: String,
    pub payload: Vec<u8>,
    pub meta: [u8; 16],
    pub notes: String,
}

/// Builds the shader mod into `mod_dir` (`shadermod.build_mod`). Like the Python, an existing `mod_dir` is removed
/// first: give it a folder of its own. Writes only `mod_dir`; installing it is a separate step with the game closed.
pub fn build_shader_mod(game: &Game, mod_dir: &Path, options: &ShaderModOptions) -> Result<ShaderModStats, ShaderError> {
    let b = game.bundle(CONFIG_BUNDLE, GLOBALS_TOC)?;
    let dxc = Dxc::open(game.root())?;
    let programs = super::build_programs_with(game, &b, &dxc, options.animated)?;
    // the banner trail's spawn and simulate programs live in the shared customization bundle (items.toc); its vertex
    // program is the configuration bundle's ribbon vertex program, already in `programs`
    let shared_programs: Vec<BuiltProgram> = super::ribbon_banner::build_shared(game, &dxc)?
        .into_iter()
        .map(|p| BuiltProgram { name: p.name, payload: p.payload, meta: p.meta, notes: "nyan banner".into() })
        .collect();
    let shared = game.bundle(super::ribbon_banner::SHARED_BUNDLE, ITEMS_TOC)?;
    let mut cas_bytes: Vec<u8> = Vec::new();
    let mut notes = Vec::new();
    let config_region = patch_bundle(game, &b, &programs, &mut cas_bytes, &mut notes)?;
    let shared_region = patch_bundle(game, &shared, &shared_programs, &mut cas_bytes, &mut notes)?;
    let toc_bytes = toc::write_patch_toc(&[toc::PatchBundle { name: CONFIG_BUNDLE.to_string(), region: config_region, load: 1 }], &[], 3)?;
    let items_toc =
        toc::write_patch_toc(&[toc::PatchBundle { name: super::ribbon_banner::SHARED_BUNDLE.to_string(), region: shared_region, load: 1 }], &[], 3)?;

    if mod_dir.is_dir() {
        std::fs::remove_dir_all(mod_dir).map_err(|e| ShaderError::Io(mod_dir.to_path_buf(), e))?;
    }
    let mut pkg = mod_dir.join("Win32");
    for part in PKG.split('/') {
        pkg.push(part);
    }
    std::fs::create_dir_all(&pkg).map_err(|e| ShaderError::Io(pkg.clone(), e))?;
    write(&pkg.join("cas_01.cas"), &cas_bytes)?;
    write(&mod_dir.join("Win32").join(GLOBALS_TOC), &toc_bytes)?;
    write(&mod_dir.join("Win32").join(ITEMS_TOC), &items_toc)?;
    write_boilerplate(game, mod_dir, options)?;
    Ok(ShaderModStats { cas: cas_bytes.len(), toc: toc_bytes.len(), items_toc: items_toc.len(), programs: notes })
}

/// A copy of bundle `b` with `programs` replaced (`shadermod._patch_bundle`): the programs' files appended to
/// `cas_bytes`, the manifest's SHA-1s, sizes and metas edited in place, the manifest appended last. Returns the region.
fn patch_bundle(
    game: &Game,
    b: &crate::game::Bundle,
    programs: &[BuiltProgram],
    cas_bytes: &mut Vec<u8>,
    notes: &mut Vec<(String, String)>,
) -> Result<Vec<u8>, ShaderError> {
    let m = &b.manifest;
    if m.endian != Endian::Little {
        return Err(ShaderError::Patch("expected a little-endian manifest".into()));
    }
    let (ne, nr, nc) = (m.ebx.len(), m.res.len(), m.chunks.len());
    let total = ne + nr + nc;
    let mut files: Vec<FileLoc> = b.files.clone();
    let mut raw = b.manifest_raw.clone();
    let index = |name: &str| m.res.iter().rposition(|r| r.name == name).ok_or_else(|| ShaderError::Patch(format!("no {name}")));
    for p in programs {
        let i = index(&p.name)?;
        if m.res[i].rtype != BYTECODE_TYPE {
            return Err(ShaderError::Patch(format!("{} is not a shader program", p.name)));
        }
        let enc = cas::encode(&p.payload, game.root())?;
        files[1 + ne + i] = FileLoc::in_mod(cas_bytes.len() as u32, enc.len() as u32);
        cas_bytes.extend_from_slice(&enc);
        // the manifest's raw bytes edited in place, as the Python does: the SHA-1, size and meta of resource i
        let sha_at = 36 + 20 * (ne + i);
        put(&mut raw, sha_at, &bundle::sha1(&enc))?;
        let names = 36 + 20 * total + 8 * ne;
        put(&mut raw, names + 8 * i + 4, &(p.payload.len() as u32).to_le_bytes())?;
        let metas = names + 8 * nr + 4 * nr;
        put(&mut raw, metas + 16 * i, &p.meta)?;
        notes.push((p.name.clone(), p.notes.clone()));
    }
    let check = bundle::read_manifest(&raw)?;
    for p in programs {
        let r = &check.res[index(&p.name)?];
        if r.size as usize != p.payload.len() || r.meta != p.meta || r.rtype != BYTECODE_TYPE {
            return Err(ShaderError::Patch(format!("{}: the edited manifest does not read back", p.name)));
        }
    }
    files[0] = FileLoc::in_mod(cas_bytes.len() as u32, raw.len() as u32);
    cas_bytes.extend_from_slice(&raw);
    Ok(bundle::write_region(&files))
}

/// Every mod folder's boilerplate (`shadermod._write_mod`'s tail): layout.toc, initfs, the stamp, the JSON files.
fn write_boilerplate(game: &Game, mod_dir: &Path, options: &ShaderModOptions) -> Result<(), ShaderError> {
    let layout_path = game.data().join("layout.toc");
    let layout = std::fs::read(&layout_path).map_err(|e| ShaderError::Io(layout_path.clone(), e))?;
    if layout.len() < toc::ENVELOPE {
        return Err(ShaderError::Patch("layout.toc is shorter than its envelope".into()));
    }
    let mut stripped = layout[..8].to_vec();
    stripped.resize(toc::ENVELOPE, 0);
    stripped.extend_from_slice(&layout[toc::ENVELOPE..]);
    write(&mod_dir.join("layout.toc"), &stripped)?;
    let initfs = game.data().join("initfs_Win32");
    let initfs_bytes = std::fs::read(&initfs).map_err(|e| ShaderError::Io(initfs.clone(), e))?;
    write(&mod_dir.join("initfs_win32"), &initfs_bytes)?;
    let exe = game.root().join("Skate.exe");
    let exe_bytes = std::fs::read(&exe).map_err(|e| ShaderError::Io(exe.clone(), e))?;
    let stamp = format!("ReSkate Studio native Patch v1\nskate_sha256={}\n", hex(&sha256(&exe_bytes)));
    write(&mod_dir.join(".reskate-studio-patch"), stamp.as_bytes())?;
    let built = options.built.clone().unwrap_or_else(utc_now);
    let build_json = json_object(&[("schema", Json::Int(1)), ("tool", Json::Str("Caveman")), ("version", Json::Str("0.0.1")), ("built", Json::Str(&built))], 2);
    write(&mod_dir.join("reskate-build.json"), crlf(&build_json).as_bytes())?;
    let description = if options.animated {
        "Caveman's deck and trail shaders: glow, colour cycle, animated decks and the solid rainbow trail, only on decks and trails that ask for them; everything else looks as before."
    } else {
        "Caveman's deck and trail shaders: glow, colour cycle and the solid rainbow trail, only on decks and trails that ask for them; everything else looks as before."
    };
    let manifest = json_object(
        &[
            ("name", Json::Str(&options.title)),
            ("author", Json::Str("Caveman")),
            ("version_number", Json::Str("0.0.1")),
            ("website_url", Json::Str("")),
            ("description", Json::Str(description)),
            ("dependencies", Json::EmptyList),
        ],
        4,
    );
    write(&mod_dir.join("manifest.json"), crlf(&manifest).as_bytes())?;
    write(&mod_dir.join("README.md"), crlf(&format!("# {}\n\n{description}\n", options.title)).as_bytes())
}

fn put(raw: &mut [u8], at: usize, bytes: &[u8]) -> Result<(), ShaderError> {
    raw.get_mut(at..at + bytes.len()).ok_or_else(|| ShaderError::Patch("manifest too short for its tables".into()))?.copy_from_slice(bytes);
    Ok(())
}

fn write(path: &Path, data: &[u8]) -> Result<(), ShaderError> {
    std::fs::write(path, data).map_err(|e| ShaderError::Io(path.to_path_buf(), e))
}

/// Python's text-mode write on Windows: every `\n` becomes `\r\n`.
fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

/// The values the mod's JSON files hold.
enum Json<'a> {
    Int(i64),
    Str(&'a str),
    EmptyList,
}

/// `json.dumps(obj, indent=indent)` of a flat object (ensure_ascii, Python's separators).
fn json_object(fields: &[(&str, Json)], indent: usize) -> String {
    let pad = " ".repeat(indent);
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            let value = match v {
                Json::Int(n) => n.to_string(),
                Json::Str(s) => json_string(s),
                Json::EmptyList => "[]".to_string(),
            };
            format!("{pad}{}: {value}", json_string(k))
        })
        .collect();
    format!("{{\n{}\n}}", body.join(",\n"))
}

/// A JSON string as Python's `json` writes it with `ensure_ascii=True`.
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// UTC now as `%Y-%m-%dT%H:%M:%SZ`.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The SHA-256 round constants.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74,
    0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d,
    0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e,
    0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 (FIPS 180-4), for the ReSkate build stamp's `skate_sha256` (Skate.exe is about 145 MB: the SHA
/// instructions when the CPU has them, about 10x faster than the portable rounds).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let whole = data.len() / 64 * 64;
    let mut tail = data[whole..].to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_be_bytes());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sha") && is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
            // SAFETY: the features the function needs were detected just above.
            unsafe {
                compress_shani(&mut h, &data[..whole]);
                compress_shani(&mut h, &tail);
            }
            return digest(&h);
        }
    }
    compress(&mut h, &data[..whole]);
    compress(&mut h, &tail);
    digest(&h)
}

/// SHA-256 with the portable rounds only (to test the SHA-instruction path against).
#[doc(hidden)]
pub fn sha256_portable(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let whole = data.len() / 64 * 64;
    let mut tail = data[whole..].to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_be_bytes());
    compress(&mut h, &data[..whole]);
    compress(&mut h, &tail);
    digest(&h)
}

fn digest(h: &[u32; 8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// SHA-256 rounds over whole 64-byte blocks with the x86 SHA extensions (the standard SHA-NI formulation: state
/// as ABEF/CDGH, four rounds per two `sha256rnds2`, the schedule from `sha256msg1`/`sha256msg2`).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
unsafe fn compress_shani(h: &mut [u32; 8], blocks: &[u8]) {
    use std::arch::x86_64::*;
    let mask = _mm_set_epi64x(0x0c0d_0e0f_0809_0a0b, 0x0405_0607_0001_0203);
    let tmp = _mm_shuffle_epi32(_mm_loadu_si128(h.as_ptr().cast()), 0xB1); // CDAB
    let mut state1 = _mm_shuffle_epi32(_mm_loadu_si128(h.as_ptr().add(4).cast()), 0x1B); // EFGH
    let mut state0 = _mm_alignr_epi8(tmp, state1, 8); // ABEF
    state1 = _mm_blend_epi16(state1, tmp, 0xF0); // CDGH
    for block in blocks.chunks_exact(64) {
        let (abef, cdgh) = (state0, state1);
        let mut w = [_mm_setzero_si128(); 4];
        for (t, slot) in w.iter_mut().enumerate() {
            *slot = _mm_shuffle_epi8(_mm_loadu_si128(block.as_ptr().add(16 * t).cast()), mask);
        }
        for t in 0..16 {
            let msg_t = if t < 4 {
                w[t]
            } else {
                let x = _mm_add_epi32(_mm_sha256msg1_epu32(w[(t - 4) % 4], w[(t - 3) % 4]), _mm_alignr_epi8(w[(t - 1) % 4], w[(t - 2) % 4], 4));
                let x = _mm_sha256msg2_epu32(x, w[(t - 1) % 4]);
                w[t % 4] = x;
                x
            };
            let msg = _mm_add_epi32(msg_t, _mm_loadu_si128(K.as_ptr().add(4 * t).cast()));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            state0 = _mm_sha256rnds2_epu32(state0, state1, _mm_shuffle_epi32(msg, 0x0E));
        }
        state0 = _mm_add_epi32(state0, abef);
        state1 = _mm_add_epi32(state1, cdgh);
    }
    let tmp = _mm_shuffle_epi32(state0, 0x1B); // FEBA
    state1 = _mm_shuffle_epi32(state1, 0xB1); // DCHG
    state0 = _mm_blend_epi16(tmp, state1, 0xF0); // DCBA
    state1 = _mm_alignr_epi8(state1, tmp, 8); // ABEF
    _mm_storeu_si128(h.as_mut_ptr().cast(), state0);
    _mm_storeu_si128(h.as_mut_ptr().add(4).cast(), state1);
}

/// SHA-256 rounds over whole 64-byte blocks, portable.
fn compress(h: &mut [u32; 8], blocks: &[u8]) {
    let mut w = [0u32; 64];
    for chunk in blocks.chunks_exact(64) {
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
}
