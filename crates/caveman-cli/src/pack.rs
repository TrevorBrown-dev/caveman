//! `caveman pack`: chosen decks and wheels built into a package anyone can install from Thunderstore (or drop into
//! the launcher) with nothing of Caveman: a mod folder `<Owner>-<Name>` and the zip `<Owner>-<Name>-<version>.zip`
//! with `manifest.json`, `icon.png`, `README.md` and `CHANGELOG.md` at its top and the mod's files beside them, as
//! docs.reskate.dev/mods/thunderstore describes. The chosen files are staged into a scratch folder laid out like a
//! drop folder (their subfolders kept, so names come out as the library shows them), the ordinary build runs on it
//! under the pack's mod name, and the package files are written over the build's own.
//!
//! Decks that use Caveman's shader effects (colour cycle, glow, the rainbow trail) need the Caveman-Shaders mod,
//! which a package cannot carry (one mod per package): they are built as plain decks and named in the README and in
//! the result's `shader_decks`.

use std::io::Write;
use std::path::{Path, PathBuf};

use caveman_core::image::Image;
use caveman_core::settings::{self, Settings};
use serde_json::json;

use crate::names::{self, Kind};
use crate::report::Reporter;
use crate::{Options, Outcome, Result, FINGERPRINT};

pub const PACK_USAGE: &str = "usage: caveman pack --owner OWNER --name NAME [--version 1.0.0] [--description TEXT] [--website URL] [--icon FILE.png]
                    [--out DIR] [--game DIR] [--decks DIR] [--wheels DIR] [--deck FILE]... [--wheel FILE]... [--json]

Builds decks and wheels into a Thunderstore-ready package: <out>\\OWNER-NAME\\ (the mod folder) and
<out>\\OWNER-NAME-VERSION.zip (default <out> = <game>\\Caveman Packs). With --deck and --wheel the named files (their
place under the drop folders decides their names); without any, every deck and wheel of the drop folders that is on.
OWNER and NAME: letters, digits and _ (Thunderstore's rule; _ shows as a space); the version is three numbers; the
description at most 250 characters. --icon is made 256 x 256 (else the first deck's thumbnail). Nothing in the game
folder changes.";

/// One file to pack.
#[derive(Clone, Debug)]
pub struct PackItem {
    pub kind: Kind,
    pub path: PathBuf,
    /// Its place under the staged drop folder (`Sub/Cool Deck.png`): the subfolders become name prefixes as in the
    /// library. Empty: the file's own name at the top.
    pub rel: String,
}

#[derive(Clone, Debug)]
pub struct PackOptions {
    pub game: PathBuf,
    /// Where the folder and the zip go (default `<game>\Caveman Packs`).
    pub out: Option<PathBuf>,
    pub owner: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub website: String,
    pub icon: Option<PathBuf>,
    pub items: Vec<PackItem>,
    /// The item cache (default `%LOCALAPPDATA%\Caveman\cache`); packs share it with the ordinary build.
    pub cache: Option<PathBuf>,
    /// The scratch folder (default `%TEMP%\caveman-pack\<Owner-Name>`); emptied first.
    pub stage: Option<PathBuf>,
    pub json: bool,
}

#[derive(Clone, Debug)]
pub struct PackResult {
    pub folder: PathBuf,
    pub zip: PathBuf,
    pub zip_bytes: u64,
    pub decks: usize,
    pub wheels: usize,
    /// Decks whose shader effects a package cannot carry (built plain).
    pub shader_decks: Vec<String>,
}

impl PackOptions {
    pub fn folder_name(&self) -> String {
        format!("{}-{}", self.owner, self.name)
    }

    pub fn out_dir(&self) -> PathBuf {
        self.out.clone().unwrap_or_else(|| crate::folders::packs(&self.game))
    }

    fn stage_dir(&self) -> PathBuf {
        self.stage.clone().unwrap_or_else(|| std::env::temp_dir().join("caveman-pack").join(self.folder_name()))
    }

    /// Thunderstore's and ReSkate's rules on the names, the version and the description.
    pub fn validate(&self) -> std::result::Result<(), String> {
        let word = |what: &str, s: &str| -> std::result::Result<(), String> {
            if s.is_empty() {
                return Err(format!("the {what} is empty"));
            }
            if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!("the {what} \"{s}\" can only have letters, digits and _ (Thunderstore shows _ as a space)"));
            }
            Ok(())
        };
        word("owner", &self.owner)?;
        word("name", &self.name)?;
        if self.folder_name().len() > 64 {
            return Err(format!("\"{}\" is longer than 64 characters, the longest mod folder name", self.folder_name()));
        }
        let parts: Vec<&str> = self.version.split('.').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
            return Err(format!("the version \"{}\" must be three numbers, such as 1.0.0", self.version));
        }
        if self.description.chars().count() > 250 {
            return Err(format!("the description is {} characters; Thunderstore allows 250", self.description.chars().count()));
        }
        if !self.website.is_empty() && !self.website.starts_with("https://") && !self.website.starts_with("http://") {
            return Err("the website must be a link (https://...) or empty".into());
        }
        if self.items.is_empty() {
            return Err("nothing to pack: choose at least one deck or wheel".into());
        }
        Ok(())
    }
}

/// Reads the command line. `Ok(None)` for `--help`.
pub fn parse_pack_args(args: &[String]) -> std::result::Result<Option<PackOptions>, String> {
    let mut o = PackOptions {
        game: PathBuf::from(crate::DEFAULT_GAME),
        out: None,
        owner: String::new(),
        name: String::new(),
        version: "1.0.0".into(),
        description: String::new(),
        website: String::new(),
        icon: None,
        items: Vec::new(),
        cache: None,
        stage: None,
        json: false,
    };
    let (mut decks_dir, mut wheels_dir) = (None::<PathBuf>, None::<PathBuf>);
    let (mut deck_files, mut wheel_files) = (Vec::new(), Vec::new());
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
            "--owner" => o.owner = value()?,
            "--name" => o.name = value()?,
            "--version" => o.version = value()?,
            "--description" => o.description = value()?,
            "--website" => o.website = value()?,
            "--icon" => o.icon = Some(PathBuf::from(value()?)),
            "--decks" => decks_dir = Some(PathBuf::from(value()?)),
            "--wheels" => wheels_dir = Some(PathBuf::from(value()?)),
            "--deck" => deck_files.push(PathBuf::from(value()?)),
            "--wheel" => wheel_files.push(PathBuf::from(value()?)),
            "--cache" => o.cache = Some(PathBuf::from(value()?)),
            "--stage" => o.stage = Some(PathBuf::from(value()?)),
            "--json" => o.json = true,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    let decks_dir = decks_dir.unwrap_or_else(|| crate::folders::decks(&o.game));
    let wheels_dir = wheels_dir.unwrap_or_else(|| crate::folders::wheels(&o.game));
    if deck_files.is_empty() && wheel_files.is_empty() {
        // everything that is on
        let mut quiet = Reporter::to(Box::new(std::io::sink()), true);
        let mut found = Vec::new();
        if decks_dir.is_dir() {
            names::scan_decks(&decks_dir, "", true, &mut quiet, &mut found);
        }
        let n = found.len();
        if wheels_dir.is_dir() {
            names::scan_wheels(&wheels_dir, "", true, &mut quiet, &mut found);
        }
        let (off_decks, _) = crate::selection::OffList::load(&decks_dir);
        let (off_wheels, _) = crate::selection::OffList::load(&wheels_dir);
        for (k, f) in found.into_iter().enumerate() {
            let (root, off) = if k < n { (&decks_dir, &off_decks) } else { (&wheels_dir, &off_wheels) };
            if off.is_off(root, &f.path) {
                continue;
            }
            o.items.push(PackItem { kind: f.kind.clone(), rel: rel_in(root, &f.path), path: f.path });
        }
    } else {
        for p in deck_files {
            o.items.push(PackItem { kind: Kind::Deck, rel: rel_in(&decks_dir, &p), path: p });
        }
        for p in wheel_files {
            o.items.push(PackItem { kind: Kind::Wheel, rel: rel_in(&wheels_dir, &p), path: p });
        }
    }
    Ok(Some(o))
}

/// A file's place under a drop folder, or its own name when it is elsewhere.
pub fn rel_in(folder: &Path, file: &Path) -> String {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    match abs(file).strip_prefix(abs(folder)) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

fn clean_rel(rel: &str, path: &Path) -> std::result::Result<PathBuf, String> {
    let rel = rel.trim_matches('/');
    let text = if rel.is_empty() { path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { rel.to_string() };
    let mut out = PathBuf::new();
    for part in text.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(format!("\"{text}\": not a place under the drop folder"));
        }
        out.push(part);
    }
    Ok(out)
}

/// Copies an item into the staged drop folder: the file, its settings file, and the layer textures the settings name.
fn stage_item(item: &PackItem, root: &Path) -> std::result::Result<(), String> {
    let dest = root.join(clean_rel(&item.rel, &item.path)?);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::copy(&item.path, &dest).map_err(|e| format!("{}: {e}", item.path.display()))?;
    if item.kind == Kind::Wheel {
        return Ok(());
    }
    let sidecar = settings::sidecar_path(&item.path);
    if sidecar.is_file() {
        std::fs::copy(&sidecar, settings::sidecar_path(&dest)).map_err(|e| format!("{}: {e}", sidecar.display()))?;
        if let Ok(Some(s)) = Settings::read_sidecar(&item.path) {
            let from_dir = item.path.parent().unwrap_or(Path::new(""));
            let to_dir = dest.parent().unwrap_or(Path::new(""));
            for layer in s.layer_paths(&item.path) {
                let rel = layer.strip_prefix(from_dir).map_err(|_| format!("{}: its layer {} is not beside it", item.path.display(), layer.display()))?;
                let to = to_dir.join(rel);
                if let Some(dir) = to.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                std::fs::copy(&layer, &to).map_err(|e| format!("{}: {e}", layer.display()))?;
            }
        }
    }
    Ok(())
}

/// The mod folders Caveman built in the game's Mods (their reskate-build.json names Caveman as the tool): a pack's
/// items may already be in them under the same names, which is no clash.
fn caveman_built_mods(mods_dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(mods_dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            std::fs::read_to_string(e.path().join("reskate-build.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .is_some_and(|v| v["tool"] == "Caveman")
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Builds the package.
pub fn run_pack(o: &PackOptions, report: &mut Reporter) -> Result<PackResult> {
    o.validate()?;
    let folder_name = o.folder_name();
    let stage = o.stage_dir();
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    let (decks_dir, wheels_dir, mods_dir) = (stage.join("decks"), stage.join("wheels"), stage.join("mods"));
    for d in [&decks_dir, &wheels_dir, &mods_dir] {
        std::fs::create_dir_all(d)?;
    }
    let mut staged = (0usize, 0usize);
    for item in &o.items {
        let root = if item.kind == Kind::Deck { &decks_dir } else { &wheels_dir };
        stage_item(item, root)?;
        if item.kind == Kind::Deck {
            staged.0 += 1;
        } else {
            staged.1 += 1;
        }
    }
    report.say(
        &format!("Packing {} deck(s) and {} wheel(s) as {folder_name} {}", staged.0, staged.1, o.version),
        json!({"event": "pack_start", "folder": folder_name, "version": o.version, "decks": staged.0, "wheels": staged.1, "stage": stage.display().to_string()}),
    );

    // the decks that need the shader mod (named in the README; the package cannot carry it)
    let mut shader_decks = Vec::new();
    {
        let mut quiet = Reporter::to(Box::new(std::io::sink()), true);
        let mut found = Vec::new();
        names::scan_decks(&decks_dir, "", true, &mut quiet, &mut found);
        for f in found {
            let nyan = matches!(f.effects.board, Some(caveman_core::effects::BoardEffect::Nyan { .. }));
            if f.effects.cycle || f.effects.glow || nyan || f.clip.is_some() {
                shader_decks.push(f.display);
            }
        }
    }

    let mut options = Options::new(&o.game);
    options.decks = Some(decks_dir.clone());
    options.wheels = Some(wheels_dir.clone());
    options.out = Some(mods_dir.clone());
    options.mod_name = folder_name.clone();
    options.json = o.json;
    options.cache = o.cache.clone();
    options.author = Some(o.owner.clone());
    options.skip_mods = caveman_built_mods(&o.game.join("Mods"));
    let outcome = crate::run(&options, report)?;
    let (decks, wheels) = match outcome {
        Outcome::Built { decks, wheels, .. } => (decks, wheels),
        _ => return Err("nothing was built: every chosen file was left out (see the lines above)".into()),
    };
    if decks + wheels == 0 {
        return Err("nothing was built: every chosen file was left out (see the lines above)".into());
    }
    let built = mods_dir.join(&folder_name);
    if !built.join("layout.toc").is_file() {
        return Err(format!("the build left no mod at {}", built.display()).into());
    }

    // the package's own files over the build's
    let _ = std::fs::remove_file(built.join(FINGERPRINT));
    let names: Vec<String> = {
        // the README the build wrote lists what it built, one "- name" line each
        std::fs::read_to_string(built.join("README.md")).unwrap_or_default().lines().filter_map(|l| l.strip_prefix("- ")).map(str::to_string).collect()
    };
    let manifest = json!({
        "name": o.name,
        "author": o.owner,
        "version_number": o.version,
        "website_url": o.website,
        "description": if o.description.is_empty() { default_description(decks, wheels) } else { o.description.clone() },
        "dependencies": [],
    });
    std::fs::write(built.join("manifest.json"), format!("{}\n", serde_json::to_string_pretty(&manifest)?))?;
    let mut readme = format!("# {}\n\n", o.name.replace('_', " "));
    if !o.description.is_empty() {
        readme.push_str(&format!("{}\n\n", o.description));
    }
    readme.push_str(&format!("{}:\n\n", what(decks, wheels)));
    for n in &names {
        readme.push_str(&format!("- {n}\n"));
    }
    if !shader_decks.is_empty() {
        readme.push_str(&format!(
            "\nNote: {} deck(s) here were made with Caveman's shader effects (colour cycle, glow, the rainbow trail, animation), which need the Caveman-Shaders mod; in this package they are plain decks: {}.\n",
            shader_decks.len(),
            shader_decks.join(", ")
        ));
    }
    readme.push_str(&format!("\n---\n\nGenerated with Caveman {}.\n", env!("CARGO_PKG_VERSION")));
    std::fs::write(built.join("README.md"), readme)?;
    std::fs::write(
        built.join("CHANGELOG.md"),
        format!("# {}\n\n## {}\n\n- Built with Caveman {}.\n", o.name.replace('_', " "), o.version, env!("CARGO_PKG_VERSION")),
    )?;
    if let Some(icon) = &o.icon {
        let img = Image::read_png(icon).map_err(|e| format!("the icon {}: {e}", icon.display()))?;
        let img = if img.width == 256 && img.height == 256 { img.to_rgba() } else { img.resize_lanczos(256, 256).to_rgba() };
        img.write_png(built.join("icon.png"))?;
    }

    // the folder and the zip in the output folder
    let out_dir = o.out_dir();
    std::fs::create_dir_all(&out_dir)?;
    let folder = out_dir.join(&folder_name);
    if folder.exists() {
        std::fs::remove_dir_all(&folder)?;
    }
    copy_tree(&built, &folder)?;
    let zip = out_dir.join(format!("{folder_name}-{}.zip", o.version));
    let zip_bytes = write_zip(&folder, &zip, &format!("Generated with Caveman {}", env!("CARGO_PKG_VERSION")))?;
    report.say(
        &format!("Packed {folder_name} {}: {} and {} ({} KB)", o.version, folder.display(), zip.display(), zip_bytes / 1024),
        json!({"event": "packed", "folder": folder.display().to_string(), "zip": zip.display().to_string(), "zip_bytes": zip_bytes, "decks": decks, "wheels": wheels, "shader_decks": shader_decks}),
    );
    Ok(PackResult { folder, zip, zip_bytes, decks, wheels, shader_decks })
}

fn what(decks: usize, wheels: usize) -> &'static str {
    match (decks > 0, wheels > 0) {
        (true, false) => "Decks",
        (false, true) => "Wheels",
        _ => "Decks and wheels",
    }
}

fn default_description(decks: usize, wheels: usize) -> String {
    match (decks, wheels) {
        (d, 0) => format!("{d} deck(s)."),
        (0, w) => format!("{w} wheel(s)."),
        (d, w) => format!("{d} deck(s) and {w} wheel(s)."),
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        let dest = to.join(e.file_name());
        if p.is_dir() {
            copy_tree(&p, &dest)?;
        } else {
            std::fs::copy(&p, &dest)?;
        }
    }
    Ok(())
}

/// Every file under a folder, as (zip name, path), sorted; `manifest.json`, `icon.png`, `README.md` and
/// `CHANGELOG.md` first (the files Thunderstore reads at the top).
fn files_of(root: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(root, &p, out)?;
            } else {
                let name = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push((name, p));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    let rank = |n: &str| match n {
        "manifest.json" => 0,
        "icon.png" => 1,
        "README.md" => 2,
        "CHANGELOG.md" => 3,
        _ => 4,
    };
    out.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

/// CRC-32 (IEEE 802.3, as zip uses it).
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// The current local time as zip's DOS date and time words (seconds halved; before 1980 clamps to 1980).
fn dos_time() -> (u16, u16) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = (yoe + era * 400 + (m <= 2) as i64).max(1980);
    let date = (((y - 1980) as u16) << 9) | ((m as u16) << 5) | d as u16;
    let time = (((rem / 3600) as u16) << 11) | (((rem % 3600 / 60) as u16) << 5) | ((rem % 60) / 2) as u16;
    (date, time)
}

/// Writes a zip of a folder's files, stored (the mod's archives are already compressed), with an archive comment.
/// Returns the zip's size.
pub fn write_zip(folder: &Path, zip: &Path, comment: &str) -> Result<u64> {
    let files = files_of(folder)?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(zip)?);
    let (date, time) = dos_time();
    let mut central: Vec<u8> = Vec::new();
    let mut offset: u64 = 0;
    let put16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    for (name, path) in &files {
        let data = std::fs::read(path)?;
        if data.len() as u64 > u32::MAX as u64 || offset > u32::MAX as u64 {
            return Err(format!("{}: too large for a plain zip (4 GB)", path.display()).into());
        }
        let crc = crc32(&data);
        let name_bytes = name.as_bytes();
        let mut local = Vec::with_capacity(30 + name_bytes.len());
        put32(&mut local, 0x0403_4b50);
        put16(&mut local, 20); // version needed
        put16(&mut local, 0x0800); // UTF-8 names
        put16(&mut local, 0); // stored
        put16(&mut local, time);
        put16(&mut local, date);
        put32(&mut local, crc);
        put32(&mut local, data.len() as u32);
        put32(&mut local, data.len() as u32);
        put16(&mut local, name_bytes.len() as u16);
        put16(&mut local, 0);
        local.extend_from_slice(name_bytes);
        out.write_all(&local)?;
        out.write_all(&data)?;
        put32(&mut central, 0x0201_4b50);
        put16(&mut central, 20); // made by
        put16(&mut central, 20);
        put16(&mut central, 0x0800);
        put16(&mut central, 0);
        put16(&mut central, time);
        put16(&mut central, date);
        put32(&mut central, crc);
        put32(&mut central, data.len() as u32);
        put32(&mut central, data.len() as u32);
        put16(&mut central, name_bytes.len() as u16);
        put16(&mut central, 0); // extra
        put16(&mut central, 0); // file comment
        put16(&mut central, 0); // disk
        put16(&mut central, 0); // internal attributes
        put32(&mut central, 0); // external attributes
        put32(&mut central, offset as u32);
        central.extend_from_slice(name_bytes);
        offset += (local.len() + data.len()) as u64;
    }
    if central.len() as u64 > u32::MAX as u64 || files.len() > u16::MAX as usize {
        return Err("too many files for a plain zip".into());
    }
    out.write_all(&central)?;
    let comment_bytes = comment.as_bytes();
    let mut end = Vec::with_capacity(22 + comment_bytes.len());
    put32(&mut end, 0x0605_4b50);
    put16(&mut end, 0);
    put16(&mut end, 0);
    put16(&mut end, files.len() as u16);
    put16(&mut end, files.len() as u16);
    put32(&mut end, central.len() as u32);
    put32(&mut end, offset as u32);
    put16(&mut end, comment_bytes.len() as u16);
    end.extend_from_slice(comment_bytes);
    out.write_all(&end)?;
    out.flush()?;
    Ok(offset + central.len() as u64 + end.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        let base = |items: Vec<PackItem>| PackOptions {
            game: PathBuf::from("G"),
            out: None,
            owner: "Trevor".into(),
            name: "Cool_Decks".into(),
            version: "1.0.0".into(),
            description: String::new(),
            website: String::new(),
            icon: None,
            items,
            cache: None,
            stage: None,
            json: false,
        };
        let item = PackItem { kind: Kind::Deck, path: PathBuf::from("x.png"), rel: "x.png".into() };
        assert!(base(vec![]).validate().unwrap_err().contains("nothing to pack"));
        let mut o = base(vec![item.clone()]);
        assert!(o.validate().is_ok());
        assert_eq!(o.folder_name(), "Trevor-Cool_Decks");
        assert_eq!(o.out_dir(), PathBuf::from("G").join("Caveman").join("Packs"));
        o.name = "Cool Decks".into();
        assert!(o.validate().unwrap_err().contains("letters, digits and _"));
        o.name = "Cool_Decks".into();
        o.version = "1.0".into();
        assert!(o.validate().unwrap_err().contains("three numbers"));
        o.version = "1.0.0".into();
        o.description = "x".repeat(251);
        assert!(o.validate().unwrap_err().contains("250"));
        o.description.clear();
        o.website = "example.com".into();
        assert!(o.validate().is_err());
        assert_eq!(rel_in(Path::new(r"C:\d"), Path::new(r"C:\d\Sub\a.png")), "Sub/a.png");
        assert_eq!(rel_in(Path::new(r"C:\d"), Path::new(r"C:\e\a.png")), "a.png");
        assert_eq!(clean_rel("", Path::new(r"C:\e\a.png")).unwrap(), PathBuf::from("a.png"));
        assert!(clean_rel("../a.png", Path::new("a.png")).is_err());
    }

    #[test]
    fn crc_and_zip() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let dir = std::env::temp_dir().join(format!("caveman-zip-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("in").join("Win32")).unwrap();
        std::fs::write(dir.join("in").join("manifest.json"), "{}").unwrap();
        std::fs::write(dir.join("in").join("Win32").join("items.toc"), b"toc bytes").unwrap();
        let zip = dir.join("out.zip");
        let size = write_zip(&dir.join("in"), &zip, "Generated with Caveman").unwrap();
        let bytes = std::fs::read(&zip).unwrap();
        assert_eq!(bytes.len() as u64, size);
        assert_eq!(&bytes[..4], &[0x50, 0x4b, 0x03, 0x04]);
        assert!(bytes.ends_with(b"Generated with Caveman"));
        // the end record counts two files and the names are there in order
        let end = bytes.len() - 22 - "Generated with Caveman".len();
        assert_eq!(&bytes[end..end + 4], &[0x50, 0x4b, 0x05, 0x06]);
        assert_eq!(u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]), 2);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.find("manifest.json").unwrap() < text.find("Win32/items.toc").unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
