//! Caveman's command line and the build it runs (caveman.py, ported): the PNGs in `Caveman Decks` and `Caveman Wheels`
//! become decks and wheels in one mod (`Caveman-Decks`), and when any deck uses a shader effect (cycle, glow) or the
//! rainbow trail, the game's deck and ribbon programs are patched into a second mod (`Caveman-Shaders`). Caveman.exe
//! (`launcher/`) runs the same build in process before it starts ReSkate.

pub mod folders;
pub mod fx;
pub mod importer;
pub mod items;
pub mod names;
pub mod pack;
pub mod report;
pub mod selection;

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::UNIX_EPOCH;

use caveman_core::anim;
use caveman_core::build::{self, BuildOptions, CutoutSpec, DeckLayers, DeckSource, DeckSpec, GripSpec, WheelSource, WheelSpec};
use caveman_core::effects::{self, BoardEffect, DeckEffects, Finish, Layers};
use caveman_core::game::Game;
use caveman_core::image::Image;
use caveman_core::settings::{self, Settings};
use caveman_core::shader::{build_shader_mod, ShaderModOptions};
use serde_json::json;

use names::{Kind, PREVIEWS};
use report::Reporter;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The mod Caveman builds decks and wheels into, by default.
pub const MOD: &str = "Caveman-Decks";
/// The mod Caveman patches the game's deck and ribbon programs into, when a deck needs them.
pub const SHADER_MOD: &str = "Caveman-Shaders";
/// What a build stamps into its mod folder: the inputs it was built from (`--if-changed`).
pub const FINGERPRINT: &str = "caveman-fingerprint.txt";
/// The game folder when none is given.
pub const DEFAULT_GAME: &str = r"E:\Games\ReSkate";

/// The deck outline as a preview mask (255 on the deck, 90 off it), drawn by Pillow from assets/deck_outline.json
/// exactly as caveman.preview does (assets/make_preview_mask.py).
const PREVIEW_MASK: &[u8] = include_bytes!("../assets/deck_preview_mask.png");

/// What to build, from where, to where.
#[derive(Clone, Debug)]
pub struct Options {
    pub game: PathBuf,
    /// The deck PNGs (default `<game>\Caveman\Decks`; folders.rs).
    pub decks: Option<PathBuf>,
    /// The wheel PNGs (default `<game>\Caveman\Wheels`).
    pub wheels: Option<PathBuf>,
    /// The grips (default `<game>\Caveman\Grips`): pictures printed on the grip and plain colours (names::scan_grips).
    pub grips: Option<PathBuf>,
    /// The grip patterns (default `<game>\Caveman\Patterns`): cutout masks (names::scan_patterns).
    pub patterns: Option<PathBuf>,
    pub mod_name: String,
    /// Where the mods are written (default `<game>\Mods`).
    pub out: Option<PathBuf>,
    /// Text put before every name ("000 - " lists them first while testing).
    pub prefix: String,
    /// Do nothing when the inputs and Caveman itself are as at the last build.
    pub if_changed: bool,
    pub json: bool,
    /// Where the item cache and thumbnail lighting live (default `%LOCALAPPDATA%\Caveman\cache`).
    pub cache: Option<PathBuf>,
    /// Build every deck and wheel again instead of reusing unchanged ones from the item cache.
    pub no_cache: bool,
    /// The mod's author in its manifest.json (default "Caveman").
    pub author: Option<String>,
    /// Installed mods whose items do not count as taken (besides the mod being built): a pack's items may already be
    /// in the personal Caveman-Decks under the same names.
    pub skip_mods: Vec<String>,
}

impl Options {
    pub fn new(game: impl Into<PathBuf>) -> Options {
        Options {
            game: game.into(),
            decks: None,
            wheels: None,
            grips: None,
            patterns: None,
            mod_name: MOD.into(),
            out: None,
            prefix: String::new(),
            if_changed: false,
            json: false,
            cache: None,
            no_cache: false,
            author: None,
            skip_mods: Vec::new(),
        }
    }

    pub fn decks_dir(&self) -> PathBuf {
        self.decks.as_deref().map(|d| folders::modern(&self.game, d)).unwrap_or_else(|| folders::decks(&self.game))
    }

    pub fn wheels_dir(&self) -> PathBuf {
        self.wheels.as_deref().map(|d| folders::modern(&self.game, d)).unwrap_or_else(|| folders::wheels(&self.game))
    }

    pub fn grips_dir(&self) -> PathBuf {
        self.grips.clone().unwrap_or_else(|| folders::grips(&self.game))
    }

    pub fn patterns_dir(&self) -> PathBuf {
        self.patterns.clone().unwrap_or_else(|| folders::patterns(&self.game))
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.out.clone().unwrap_or_else(|| self.game.join("Mods"))
    }
}

pub const USAGE: &str = "usage: caveman [--game DIR] [--decks DIR] [--wheels DIR] [--grips DIR] [--patterns DIR] [--mod NAME] [--out DIR] [--prefix TEXT] [--if-changed] [--json]
               [--cache DIR] [--no-cache] [--author NAME]

Builds the PNGs in <game>\\Caveman\\Decks (decks; effect suffixes such as Cool Deck.holo-frost.spot.png, or a settings
file Cool Deck.caveman.json beside the PNG; Cool Deck.ff0000.png shows red where the art is transparent) and <game>\\Caveman\\Wheels (wheels; Name.ff0000.png for a coloured wheel)
into the mod <game>\\Mods\\Caveman-Decks, and Caveman-Shaders when a deck uses cycle, glow or nyan. A GIF (or, with
ffmpeg on PATH, a video) in the decks folder becomes an animated deck. (The folders from before 2026-10-30, Caveman Decks
and Caveman Wheels in the game folder itself, move into the Caveman folder on the first run.) Decks and wheels that did not change since the
last build come from the item cache (--cache, default %LOCALAPPDATA%\\Caveman\\cache); --no-cache builds them all
again.

Grips (the game's grip colours) come from <game>\\Caveman\\Grips: a PNG is printed on the grip tape (transparent parts
show the grit; Name.ff69b4.png puts the grit in that colour under it), and a file Name.ff69b4.grip (any contents) is
plain grip in that colour. Grip patterns (the game's cutouts) come from <game>\\Caveman\\Patterns: a PNG whose clear or
dark parts cut the grip away so the wood shows; a 1:4 picture covers the deck, any other is repeated as a tile.

A drop folder's .caveman-off.json (the Caveman app's switch per item) lists files that are not built. --author NAME
goes into the mod's manifest.json (default Caveman).

caveman testclip <out.gif> [--frames N] [--fps F] [--size WxH] writes a looping test animation to drop into the folder.
caveman fx [--out DIR] builds Caveman FX's own mod (Caveman-FX: the fx states and The Caveman; caveman fx --help).
caveman pack --owner OWNER --name NAME ... builds chosen decks and wheels into a Thunderstore-ready package (caveman pack --help).";

/// `caveman testclip`: a test animation as a GIF (anim::test_clip).
pub fn run_testclip(args: &[String]) -> Result<()> {
    let (mut out, mut frames, mut fps, mut size) = (None::<PathBuf>, 8usize, 2u32, (256usize, 1024usize));
    let mut i = 0;
    while i < args.len() {
        let value = |i: &mut usize| -> Result<String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", args[*i - 1]).into())
        };
        match args[i].as_str() {
            "--frames" => frames = value(&mut i)?.parse()?,
            "--fps" => fps = value(&mut i)?.parse()?,
            "--size" => {
                let v = value(&mut i)?;
                let (w, h) = v.split_once('x').ok_or("--size takes WxH")?;
                size = (w.parse()?, h.parse()?);
            }
            other if out.is_none() && !other.starts_with("--") => out = Some(PathBuf::from(other)),
            other => return Err(format!("unknown argument {other}").into()),
        }
        i += 1;
    }
    let out = out.ok_or("testclip needs an output file")?;
    if !(1..=anim::MAX_FRAMES).contains(&frames) || !(1..=60).contains(&fps) || size.0 == 0 || size.1 == 0 {
        return Err("frames 1 to 254, fps 1 to 60, a non-empty size".into());
    }
    let clip = anim::test_clip(frames, fps, size.0, size.1);
    anim::write_gif(&clip, &out)?;
    println!("Wrote {} ({frames} frames at {fps} a second, {} x {})", out.display(), size.0, size.1);
    Ok(())
}

/// An animated deck from a clip file: the art (its first frame on the deck), its layers (the frame atlas in the holo
/// map slot with the shader marker; the other layers as the effects make them) and notes on what was done.
fn anim_deck(
    path: &Path,
    settings: &Settings,
    effects: &DeckEffects,
    wood: &Image,
    defaults: &(Image, Image),
) -> std::result::Result<(Image, Layers, Vec<String>), String> {
    let policy = settings.policy();
    let clip = anim::load_clip(path, &policy).map_err(|e| e.to_string())?;
    let plan = anim::plan(&clip, &policy).map_err(|e| e.to_string())?;
    let atlas = anim::atlas(&clip, &plan, wood).map_err(|e| e.to_string())?;
    let mut notes = plan.notes.clone();
    let texels = plan.width() * plan.height();
    notes.push(format!(
        "{} frames at {} a second ({:.1} s loop), {} x {} frames in a {} x {} atlas ({:.1} MB with mips)",
        plan.frames(),
        plan.fps,
        plan.loop_seconds(),
        plan.frame.0,
        plan.frame.1,
        plan.width(),
        plan.height(),
        texels as f64 * 4.0 / 3.0 / (1 << 20) as f64
    ));
    if clip.width > clip.height {
        notes.push("turned upright (the clip is wider than tall; its left edge is now the top)".to_string());
    }
    let mut e = effects.clone();
    e.cycle = false;
    if effects.cycle {
        notes.push("cycle has no effect on an animated deck (left out)".to_string());
    }
    if matches!(e.finish, Some(Finish::Holo(_))) {
        notes.push("holo finish left out: an animated deck's holo map holds its frames".to_string());
        e.finish = None;
    }
    let mut layers = effects::layers(&e, &atlas.art, &defaults.0, &defaults.1);
    if layers.osk.is_none() {
        layers.osk = Some(defaults.0.to_rgb()); // the holo host's own mask is its graphic's
    }
    let glow = settings.marker_bytes().map_or(0, |(_, g)| g);
    layers.rgb = Some(anim::mark_atlas(&atlas.image, plan.frames(), plan.fps, plan.cols, plan.rows, glow).map_err(|e| e.to_string())?);
    layers.foilmode = None;
    Ok((atlas.art, layers, notes))
}

/// Reads the command line (without the program name). `Ok(None)` for `--help`.
pub fn parse_args(args: &[String]) -> std::result::Result<Option<Options>, String> {
    let mut o = Options::new(DEFAULT_GAME);
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
            "--wheels" => o.wheels = Some(PathBuf::from(value()?)),
            "--grips" => o.grips = Some(PathBuf::from(value()?)),
            "--patterns" => o.patterns = Some(PathBuf::from(value()?)),
            "--mod" => o.mod_name = value()?,
            "--out" => o.out = Some(PathBuf::from(value()?)),
            "--prefix" => o.prefix = value()?,
            "--if-changed" => o.if_changed = true,
            "--json" => o.json = true,
            "--cache" => o.cache = Some(PathBuf::from(value()?)),
            "--no-cache" => o.no_cache = true,
            "--author" => o.author = Some(value()?),
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok(Some(o))
}

/// What happened to the shader mod.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shaders {
    /// No deck needs it and there was none of Caveman's.
    None,
    Built,
    Removed,
}

/// What a run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// `--if-changed` and nothing changed.
    UpToDate,
    /// Nothing to build (Caveman's mods removed if there were any).
    Nothing,
    Built {
        decks: usize,
        wheels: usize,
        grips: usize,
        cutouts: usize,
        shaders: Shaders,
    },
}

/// Runs a build: scan, check names, preview, build, stamp; the shader mod when needed.
pub fn run(o: &Options, report: &mut Reporter) -> Result<Outcome> {
    // the one Caveman folder: drop folders from before 2026-10-30 move into it first (folders.rs)
    for line in folders::adopt_old_folders(&o.game) {
        report.say(&line, json!({"event": "moved_folder", "text": line}));
    }
    let (decks_dir, wheels_dir, mods_dir) = (o.decks_dir(), o.wheels_dir(), o.mods_dir());
    let (grips_dir, patterns_dir) = (o.grips_dir(), o.patterns_dir());
    let mod_dir = mods_dir.join(&o.mod_name);
    let shader_dir = mods_dir.join(SHADER_MOD);
    let mut made = false;
    for (dir, what) in [(&decks_dir, "deck"), (&wheels_dir, "wheel"), (&grips_dir, "grip"), (&patterns_dir, "grip pattern")] {
        if !dir.is_dir() {
            std::fs::create_dir_all(dir)?;
            let text = format!("Made {}: put {what} PNGs in it and run Caveman again.", dir.display());
            report.say(&text, json!({"event": "made_folder", "kind": what, "path": dir.display().to_string()}));
            made = true;
        }
    }
    if made && !has_png(&decks_dir) && !has_png(&wheels_dir) && !has_png(&grips_dir) && !has_png(&patterns_dir) {
        return Ok(Outcome::Nothing);
    }
    let stamp = fingerprint(o, &decks_dir, &wheels_dir, &grips_dir, &patterns_dir)?;
    let stamp_file = mod_dir.join(FINGERPRINT);
    if o.if_changed && std::fs::read_to_string(&stamp_file).ok().as_deref() == Some(stamp.as_str()) {
        report.say("Decks are up to date.", json!({"event": "up_to_date", "mod_dir": mod_dir.display().to_string()}));
        return Ok(Outcome::UpToDate);
    }
    let game = Arc::new(Game::open(&o.game)?);
    let mut found = Vec::new();
    names::scan_decks(&decks_dir, &o.prefix, true, report, &mut found);
    let n_decks = found.len();
    report.say(
        &format!("{n_decks} deck image(s) in {}", decks_dir.display()),
        json!({"event": "scanned", "kind": "deck", "count": n_decks, "folder": decks_dir.display().to_string()}),
    );
    names::scan_wheels(&wheels_dir, &o.prefix, true, report, &mut found);
    let n_wheels = found.len() - n_decks;
    if n_wheels > 0 {
        report.say(
            &format!("{n_wheels} wheel image(s) in {}", wheels_dir.display()),
            json!({"event": "scanned", "kind": "wheel", "count": n_wheels, "folder": wheels_dir.display().to_string()}),
        );
    }
    let n_before = found.len();
    names::scan_grips(&grips_dir, &o.prefix, true, report, &mut found);
    let n_grips = found.len() - n_before;
    if n_grips > 0 {
        report.say(
            &format!("{n_grips} grip(s) in {}", grips_dir.display()),
            json!({"event": "scanned", "kind": "grip", "count": n_grips, "folder": grips_dir.display().to_string()}),
        );
    }
    let n_before = found.len();
    names::scan_patterns(&patterns_dir, &o.prefix, true, report, &mut found);
    let n_patterns = found.len() - n_before;
    if n_patterns > 0 {
        report.say(
            &format!("{n_patterns} grip pattern(s) in {}", patterns_dir.display()),
            json!({"event": "scanned", "kind": "pattern", "count": n_patterns, "folder": patterns_dir.display().to_string()}),
        );
    }
    // what the Caveman app switched off (.caveman-off.json in each folder) is not built
    let (off_decks, off_problem) = selection::OffList::load(&decks_dir);
    let (off_wheels, off_problem_wheels) = selection::OffList::load(&wheels_dir);
    let (off_grips, off_problem_grips) = selection::OffList::load(&grips_dir);
    let (off_patterns, off_problem_patterns) = selection::OffList::load(&patterns_dir);
    for problem in [off_problem, off_problem_wheels, off_problem_grips, off_problem_patterns].into_iter().flatten() {
        let text = format!("note: {problem} (nothing switched off)");
        report.say(&format!("  {text}"), json!({"event": "log", "message": text}));
    }
    let mut kept = Vec::with_capacity(found.len());
    let mut off_count = 0usize;
    for f in found {
        let (root, list) = match f.kind {
            Kind::Deck => (&decks_dir, &off_decks),
            Kind::Wheel => (&wheels_dir, &off_wheels),
            Kind::Grip => (&grips_dir, &off_grips),
            Kind::Pattern => (&patterns_dir, &off_patterns),
        };
        if list.is_off(root, &f.path) {
            off_count += 1;
            let kind = report::kind_name(&f.kind);
            report.say(&format!("  off {}", f.display), json!({"event": "off", "kind": kind, "display": f.display, "source": f.path.display().to_string()}));
            continue;
        }
        kept.push(f);
    }
    let found = kept;
    if off_count > 0 {
        report.say(&format!("{off_count} switched off in Caveman (not built)"), json!({"event": "off_count", "count": off_count}));
    }
    if found.is_empty() {
        let shaders = clear(&mod_dir, &shader_dir, report)?;
        return Ok(if shaders == Shaders::Removed { Outcome::Built { decks: 0, wheels: 0, grips: 0, cutouts: 0, shaders } } else { Outcome::Nothing });
    }

    let wood = Arc::new(build::natural_wood(&game)?);
    let skip: Vec<&str> = std::iter::once(o.mod_name.as_str()).chain(o.skip_mods.iter().map(String::as_str)).collect();
    let taken = items::existing_items_except(&game, &o.game.join("Mods"), &skip, report)?;
    let previews = decks_dir.join(PREVIEWS);
    std::fs::create_dir_all(&previews)?;
    let mask = Image::decode_png(PREVIEW_MASK)?;
    let bopts = BuildOptions {
        cache_dir: o.cache.clone(),
        item_cache: !o.no_cache,
        author: o.author.clone(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        ..BuildOptions::default()
    };
    // the game's default mask and normal map, under every deck with its own layers: loaded once, when first needed
    let defaults: Arc<OnceLock<std::result::Result<(Image, Image), String>>> = Arc::new(OnceLock::new());
    // a deck's art and layers, loaded when the build gets to it (not held for every deck of a big collection until then)
    let deck_loader = |path: &Path, settings: &Option<Settings>, effects: &caveman_core::effects::DeckEffects, paint: Option<[u8; 3]>| -> build::DeckLoader {
        let (path, wood, settings, effects, game, defaults) =
            (path.to_path_buf(), wood.clone(), settings.clone(), effects.clone(), game.clone(), defaults.clone());
        Arc::new(move || {
            let img = Image::read_png(&path).map_err(|e| e.to_string())?;
            let (art, _) = build::fit_art_on(&img, &wood, paint).map_err(|e| e.to_string())?;
            let layers = match &settings {
                Some(s) if s.needs_layers() => {
                    let (m, n) = defaults.get_or_init(|| build::default_layers(&game).map_err(|e| e.to_string())).as_ref().map_err(|e| e.clone())?;
                    DeckLayers::Layers(s.custom_layers(&path, &art, m, n).map_err(|e| e.to_string())?)
                }
                _ => DeckLayers::Effects(effects.clone()),
            };
            Ok((art, layers))
        })
    };
    // an animated deck's clip, decoded and laid out when the build gets to it (the item-cache path)
    let anim_loader = |path: &Path, settings: &Settings, effects: &DeckEffects, paint: Option<[u8; 3]>| -> build::DeckLoader {
        let (path, wood, settings, effects, game, defaults) =
            (path.to_path_buf(), wood.clone(), settings.clone(), effects.clone(), game.clone(), defaults.clone());
        Arc::new(move || {
            let d = defaults.get_or_init(|| build::default_layers(&game).map_err(|e| e.to_string())).as_ref().map_err(|e| e.clone())?;
            let (art, layers, _) = anim_deck(&path, &settings, &effects, &build::deck_backdrop(&wood, paint), d)?;
            Ok((art, DeckLayers::Layers(layers)))
        })
    };
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    let (mut decks, mut wheels) = (Vec::new(), Vec::new());
    let (mut grips, mut cutouts): (Vec<GripSpec>, Vec<CutoutSpec>) = (Vec::new(), Vec::new());
    let mut needs_shaders = false;
    let mut animated = false;
    for mut f in found {
        let key = f.key.to_lowercase();
        if let Some(other) = seen.get(&key) {
            let message = format!("same name as {}; ReSkate would load only one, skipped", names::file_name(other));
            report.warning(f.kind.clone(), &f.display, &f.path, &message);
            continue;
        }
        if let Some(who) = taken.get(&key) {
            let what = if f.kind == Kind::Deck { "a deck" } else { "an item" };
            let message = format!("\"{}\" is already {what} in {who}; skipped (rename the PNG)", f.display);
            report.warning(f.kind.clone(), &f.display, &f.path, &message);
            continue;
        }
        // The file's bytes: what the item cache knows an unchanged deck or wheel by. Its art is only decoded (and a deck's
        // fitted and previewed) when it is not in the cache, or a deck's preview is missing.
        let bytes = match std::fs::read(&f.path) {
            Ok(b) => b,
            Err(e) => {
                report.skipped(f.kind.clone(), &f.path, &e.to_string());
                continue;
            }
        };
        if f.kind == Kind::Grip {
            // a grip: a picture printed on the grit (`.png`), plain grip in a colour (`.grip`), or both
            let colour_bytes: Vec<u8> = f.paint.map(|c| c.to_vec()).unwrap_or_default();
            let plain = !f.path.to_string_lossy().to_lowercase().ends_with(".png");
            let input = build::item_input(&[b"grip", &bytes, &colour_bytes, &[plain as u8]]);
            let print = if plain {
                None
            } else {
                match Image::decode_png(&bytes) {
                    Ok(img) => Some(img),
                    Err(e) => {
                        report.skipped(Kind::Grip, &f.path, &e.to_string());
                        continue;
                    }
                }
            };
            seen.insert(key, f.path.clone());
            let colour = f.paint.map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
            report.item(Kind::Grip, &f.display, &f.key, &f.path, &f.notes, json!({"colour": colour, "print": !plain}));
            grips.push(GripSpec { display: f.display, key: f.key, colour: f.paint, print, input: Some(input), notes: Vec::new() });
            continue;
        }
        if f.kind == Kind::Pattern {
            let input = build::item_input(&[b"pattern", &bytes]);
            let img = match Image::decode_png(&bytes) {
                Ok(img) => img,
                Err(e) => {
                    report.skipped(Kind::Pattern, &f.path, &e.to_string());
                    continue;
                }
            };
            let (mask, notes) = build::fit_mask(&img);
            seen.insert(key, f.path.clone());
            f.notes.extend(notes.iter().cloned());
            report.item(Kind::Pattern, &f.display, &f.key, &f.path, &f.notes, json!({}));
            cutouts.push(CutoutSpec { display: f.display, key: f.key, mask, input: Some(input), notes });
            continue;
        }
        if f.kind == Kind::Wheel {
            let input = build::item_input(&[b"wheel", &bytes, &f.colour]);
            let cached = build::cached_notes(&game, &bopts, &mods_dir, &o.mod_name, "wheel", &f.key, &f.display, input);
            let (source, art_notes) = match cached {
                Some(notes) => {
                    let path = f.path.clone();
                    let load: build::WheelLoader = Arc::new(move || Image::read_png(&path).map_err(|e| e.to_string()));
                    (WheelSource::Lazy(load), notes)
                }
                None => {
                    let img = match Image::decode_png(&bytes) {
                        Ok(img) => img,
                        Err(e) => {
                            report.skipped(f.kind.clone(), &f.path, &e.to_string());
                            continue;
                        }
                    };
                    let mut notes = Vec::new();
                    if img.width != img.height {
                        notes.push(format!("cropped to a square ({} x {})", img.width, img.height));
                    }
                    (WheelSource::Ready(img), notes)
                }
            };
            f.notes.extend(art_notes.iter().cloned());
            seen.insert(key, f.path.clone());
            let colour = format!("#{:02x}{:02x}{:02x}", f.colour[0], f.colour[1], f.colour[2]);
            report.item(Kind::Wheel, &f.display, &f.key, &f.path, &f.notes, json!({"colour": colour}));
            wheels.push(WheelSpec { display: f.display, key: f.key, colour: f.colour, source, input: Some(input), notes: art_notes });
            continue;
        }
        if let Some(kind) = f.clip {
            // an animated deck (anim.rs): the clip's frames in an atlas, the first frame as the art
            if kind == "video" && anim::ffmpeg_tools().is_none() {
                report.skipped(Kind::Deck, &f.path, "a video deck needs ffmpeg and ffprobe on PATH (ffmpeg.org); a GIF needs nothing");
                continue;
            }
            let settings = f.settings.clone().unwrap_or_else(|| Settings::from_effects(&f.effects));
            let sidecar = match &f.settings {
                Some(s) => serde_json::to_string(s)?,
                None => String::new(),
            };
            let effects_text = format!("{:?}", f.effects);
            let policy_text = format!("{:?}", settings.policy());
            // a deck's colour counts only when it has one, so the inputs of decks without one stay as they were
            let paint_text = f.paint.map(|c| format!("paint {c:?}")).unwrap_or_default();
            let mut parts: Vec<&[u8]> = vec![b"anim", &bytes, effects_text.as_bytes(), sidecar.as_bytes(), policy_text.as_bytes(), anim::SCHEME.as_bytes()];
            if f.paint.is_some() {
                parts.push(paint_text.as_bytes());
            }
            let input = build::item_input(&parts);
            let preview_path = previews.join(format!("{}.png", f.display));
            let cached =
                if preview_path.is_file() { build::cached_notes(&game, &bopts, &mods_dir, &o.mod_name, "deck", &f.key, &f.display, input) } else { None };
            let (source, art_notes) = match cached {
                Some(notes) => (DeckSource::Lazy(anim_loader(&f.path, &settings, &f.effects, f.paint)), notes),
                None => {
                    let d = defaults.get_or_init(|| build::default_layers(&game).map_err(|e| e.to_string())).as_ref().map_err(|e| e.clone())?;
                    match anim_deck(&f.path, &settings, &f.effects, &build::deck_backdrop(&wood, f.paint), d) {
                        Ok((art, layers, notes)) => {
                            preview(&art, &mask).write_png(&preview_path)?;
                            // decoded once: the build takes the atlas as it is (animated decks are few and large)
                            (DeckSource::Ready { art, layers: DeckLayers::Layers(layers) }, notes)
                        }
                        Err(e) => {
                            report.skipped(Kind::Deck, &f.path, &e);
                            continue;
                        }
                    }
                }
            };
            f.notes.extend(art_notes.iter().cloned());
            seen.insert(key, f.path.clone());
            needs_shaders = true;
            animated = true;
            let extra = json!({"preview": preview_path.display().to_string(), "settings": serde_json::to_value(&settings)?, "sidecar": f.settings.is_some(), "animated": kind});
            report.item(Kind::Deck, &f.display, &f.key, &f.path, &f.notes, extra);
            decks.push(DeckSpec { display: f.display, key: f.key, source, input: Some(input), notes: art_notes });
            continue;
        }
        let custom = matches!(&f.settings, Some(s) if s.needs_layers());
        let sidecar = match &f.settings {
            Some(s) => serde_json::to_string(s)?,
            None => String::new(),
        };
        // a deck's own layer files (settings `layers`) count with its art: a changed file rebuilds the deck
        let mut layer_bytes = Vec::new();
        if let Some(s) = &f.settings {
            for p in s.layer_paths(&f.path) {
                match std::fs::read(&p) {
                    Ok(b) => layer_bytes.push(b),
                    Err(e) => layer_bytes.push(format!("missing {}: {e}", p.display()).into_bytes()),
                }
            }
        }
        let effects_text = format!("{:?}", f.effects);
        let paint_text = f.paint.map(|c| format!("paint {c:?}")).unwrap_or_default();
        let mut parts: Vec<&[u8]> = vec![b"deck", &bytes, effects_text.as_bytes(), sidecar.as_bytes()];
        parts.extend(layer_bytes.iter().map(|b| b.as_slice()));
        if f.paint.is_some() {
            parts.push(paint_text.as_bytes());
        }
        let input = build::item_input(&parts);
        let preview_path = previews.join(format!("{}.png", f.display));
        let cached = if preview_path.is_file() { build::cached_notes(&game, &bopts, &mods_dir, &o.mod_name, "deck", &f.key, &f.display, input) } else { None };
        let (source, art_notes) = match cached {
            Some(notes) => (DeckSource::Lazy(deck_loader(&f.path, &f.settings, &f.effects, f.paint)), notes),
            None => {
                let img = match Image::decode_png(&bytes) {
                    Ok(img) => img,
                    Err(e) => {
                        report.skipped(f.kind.clone(), &f.path, &e.to_string());
                        continue;
                    }
                };
                let (art, fit_notes) = match build::fit_art_on(&img, &wood, f.paint) {
                    Ok(x) => x,
                    Err(e) => {
                        report.skipped(Kind::Deck, &f.path, &e.to_string());
                        continue;
                    }
                };
                preview(&art, &mask).write_png(&preview_path)?;
                if let Some(s) = f.settings.as_ref().filter(|_| custom) {
                    let (m, n) = defaults.get_or_init(|| build::default_layers(&game).map_err(|e| e.to_string())).as_ref().map_err(|e| e.clone())?;
                    if let Err(e) = s.custom_layers(&f.path, &art, m, n) {
                        report.skipped(Kind::Deck, &f.path, &e.to_string());
                        continue;
                    }
                }
                // checked and previewed: the build loads the art and layers again when it gets to this deck
                (DeckSource::Lazy(deck_loader(&f.path, &f.settings, &f.effects, f.paint)), fit_notes)
            }
        };
        f.notes.extend(art_notes.iter().cloned());
        seen.insert(key, f.path.clone());
        needs_shaders |= f.effects.cycle || f.effects.glow || matches!(f.effects.board, Some(BoardEffect::Nyan { .. }));
        let settings = f.settings.clone().unwrap_or_else(|| Settings::from_effects(&f.effects));
        let extra = json!({"preview": preview_path.display().to_string(), "settings": serde_json::to_value(&settings)?, "sidecar": f.settings.is_some()});
        report.item(Kind::Deck, &f.display, &f.key, &f.path, &f.notes, extra);
        decks.push(DeckSpec { display: f.display, key: f.key, source, input: Some(input), notes: art_notes });
    }
    let ready = format!("{} deck(s) ready; previews in {}", decks.len(), previews.display());
    report.say(&ready, json!({"event": "ready", "decks": decks.len(), "wheels": wheels.len(), "previews": previews.display().to_string()}));
    if decks.is_empty() && wheels.is_empty() && grips.is_empty() && cutouts.is_empty() {
        let shaders = clear(&mod_dir, &shader_dir, report)?;
        return Ok(if shaders == Shaders::Removed { Outcome::Built { decks: 0, wheels: 0, grips: 0, cutouts: 0, shaders } } else { Outcome::Nothing });
    }

    // the build's progress for a program reading the JSON (the app's progress bar): a `progress` event per item placed
    // (`stage` "items", `done` of `total`, its `display`), and one as the mod's files are written (`stage` "writing",
    // `mb`); the shader effects' follows below
    let total = decks.len() + wheels.len() + grips.len() + cutouts.len();
    let built = {
        let mut done = 0usize;
        let mut log = |line: &str| {
            report.log(line);
            if let Some(rest) = line.strip_prefix(build::BUILT_LINE) {
                done += 1;
                let display = rest.rsplit_once(" (").map_or(rest, |(d, _)| d);
                report.event(json!({"event": "progress", "stage": "items", "done": done, "total": total, "display": display}));
            } else if let Some(rest) = line.strip_prefix(build::WRITING_LINE) {
                let mb = rest.split(' ').next().and_then(|n| n.parse::<u64>().ok());
                report.event(json!({"event": "progress", "stage": "writing", "mb": mb}));
            }
        };
        build::build(game.clone(), &decks, &wheels, &grips, &cutouts, &mods_dir, &o.mod_name, &bopts, &mut log)?
    };
    // the menu thumbnails of wheels, grips and patterns are their previews (decks render their own)
    for (items, folder) in [(&built.wheels, &wheels_dir), (&built.grips, &grips_dir), (&built.cutouts, &patterns_dir)] {
        if items.is_empty() {
            continue;
        }
        let dir = folder.join(PREVIEWS);
        std::fs::create_dir_all(&dir)?;
        for w in items {
            if let Some(thumbnail) = &w.thumbnail {
                thumbnail.write_png(dir.join(format!("{}.png", w.display)))?;
            }
        }
    }
    let shaders = if needs_shaders {
        report.event(json!({"event": "progress", "stage": "shaders"}));
        let stats = build_shader_mod(&game, &shader_dir, &ShaderModOptions { title: "Caveman Shaders".into(), built: None, animated })?;
        let text =
            format!("Built the shader effects into {} ({} KB{})", shader_dir.display(), stats.cas / 1024, if animated { ", with animated decks" } else { "" });
        report.say(&text, json!({"event": "shaders_built", "mod_dir": shader_dir.display().to_string(), "cas_bytes": stats.cas, "animated": animated}));
        Shaders::Built
    } else if remove_generated(&shader_dir)? {
        let text = format!("No deck uses a shader effect: removed {}", shader_dir.display());
        report.say(&text, json!({"event": "removed", "mod_dir": shader_dir.display().to_string()}));
        Shaders::Removed
    } else {
        Shaders::None
    };
    std::fs::write(&stamp_file, &stamp)?;
    let counts: Vec<String> =
        [(built.decks.len(), "deck(s)"), (built.wheels.len(), "wheel(s)"), (built.grips.len(), "grip(s)"), (built.cutouts.len(), "grip pattern(s)")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect();
    let what = match counts.len() {
        0 => "nothing".to_string(),
        1 => counts[0].clone(),
        n => format!("{} and {}", counts[..n - 1].join(", "), counts[n - 1]),
    };
    let text = format!("Built {what} into {} ({} KB)", built.mod_dir.display(), built.cas_bytes / 1024);
    report.say(
        &text,
        json!({"event": "built", "mod_dir": built.mod_dir.display().to_string(), "decks": built.decks.len(), "wheels": built.wheels.len(), "grips": built.grips.len(), "patterns": built.cutouts.len(), "cas_bytes": built.cas_bytes, "shaders": format!("{shaders:?}").to_lowercase()}),
    );
    Ok(Outcome::Built { decks: built.decks.len(), wheels: built.wheels.len(), grips: built.grips.len(), cutouts: built.cutouts.len(), shaders })
}

/// No decks or wheels left to build: removes Caveman's mods (only Caveman-built ones).
fn clear(mod_dir: &Path, shader_dir: &Path, report: &mut Reporter) -> Result<Shaders> {
    if remove_generated(mod_dir)? {
        let text = format!("No decks to build: removed {}", mod_dir.display());
        report.say(&text, json!({"event": "removed", "mod_dir": mod_dir.display().to_string()}));
    }
    if remove_generated(shader_dir)? {
        let text = format!("No deck uses a shader effect: removed {}", shader_dir.display());
        report.say(&text, json!({"event": "removed", "mod_dir": shader_dir.display().to_string()}));
        return Ok(Shaders::Removed);
    }
    Ok(Shaders::None)
}

/// Removes a mod folder Caveman built (its reskate-build.json names Caveman as its tool); anything else is left
/// alone (caveman.remove_generated). Returns whether it removed one.
pub fn remove_generated(mod_dir: &Path) -> Result<bool> {
    let Ok(text) = std::fs::read_to_string(mod_dir.join("reskate-build.json")) else { return Ok(false) };
    let Ok(stamp) = serde_json::from_str::<serde_json::Value>(&text) else { return Ok(false) };
    if stamp.get("tool").and_then(|t| t.as_str()) != Some("Caveman") {
        return Ok(false);
    }
    std::fs::remove_dir_all(mod_dir)?;
    Ok(true)
}

/// The art with everything off the board darkened: what makes it onto the deck (caveman.preview: Pillow's
/// `Image.composite(art, dark, mask)`, a paste through an L mask).
pub fn preview(art: &Image, mask: &Image) -> Image {
    let art = art.to_rgb();
    let mut out = Image::new(art.width, art.height, 3);
    for ((px, src), &m) in out.data.as_chunks_mut::<3>().0.iter_mut().zip(art.data.chunks_exact(3)).zip(&mask.data) {
        let m = m as u32;
        for (o, &a) in px.iter_mut().zip(src) {
            let t = 20 * (255 - m) + a as u32 * m + 128; // Pillow's BLEND: DIV255(dark (255 - m) + art m)
            *o = (((t >> 8) + t) >> 8) as u8;
        }
    }
    out
}

fn has_png(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let lower = name.to_lowercase();
                if lower.ends_with(".png") || lower.ends_with(".grip") || anim::clip_kind(&name).is_some() {
                    return true;
                }
            }
        }
    }
    false
}

/// What a build depends on (caveman.fingerprint): every PNG and settings file under the two folders (path, size,
/// time), the options, and Caveman itself (this executable's size and time, and the engine's version). Equal
/// fingerprints mean the mod on disk is already what a build would write.
pub fn fingerprint(o: &Options, decks_dir: &Path, wheels_dir: &Path, grips_dir: &Path, patterns_dir: &Path) -> Result<String> {
    let mut text = format!(
        "caveman {}|game {}|mod {}|prefix {:?}|decks {}|wheels {}|grips {}|patterns {}|author {:?}\n",
        env!("CARGO_PKG_VERSION"),
        absolute(&o.game).display(),
        o.mod_name,
        o.prefix,
        absolute(decks_dir).display(),
        absolute(wheels_dir).display(),
        absolute(grips_dir).display(),
        absolute(patterns_dir).display(),
        o.author
    );
    for (label, dir) in [("deck", decks_dir), ("wheel", wheels_dir), ("grip", grips_dir), ("pattern", patterns_dir)] {
        walk(dir, dir, label, &mut text);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(md) = std::fs::metadata(&exe) {
            text.push_str(&format!("exe {}|{}\n", md.len(), mtime_ns(&md)));
        }
    }
    Ok(build::sha256(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect())
}

/// os.walk order: a folder's files (sorted), then its subfolders (sorted, without dot-folders and the top previews).
fn walk(root: &Path, dir: &Path, label: &str, text: &mut String) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<(String, PathBuf)> = rd.filter_map(|e| e.ok()).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect();
    entries.sort();
    let mut dirs = Vec::new();
    for (name, path) in entries {
        if path.is_dir() {
            // dot-folders are skipped, except the layer textures decks' settings name (import::LAYERS_DIR)
            let layers = name == caveman_core::import::LAYERS_DIR;
            if (layers || !name.starts_with('.')) && (dir != root || name.to_lowercase() != PREVIEWS) {
                dirs.push(path);
            }
            continue;
        }
        let lower = name.to_lowercase();
        if lower.ends_with(".png")
            || lower.ends_with(".grip")
            || settings::is_sidecar(&lower)
            || anim::clip_kind(&lower).is_some()
            || (dir == root && lower == selection::OFF_FILE)
        {
            if let Ok(md) = std::fs::metadata(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
                text.push_str(&format!("{label} {rel}|{}|{}\n", md.len(), mtime_ns(&md)));
            }
        }
    }
    for d in dirs {
        walk(root, &d, label, text);
    }
}

fn mtime_ns(md: &std::fs::Metadata) -> u128 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0)
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        let a: Vec<String> = ["--game", "G", "--decks=D", "--prefix", "000 - ", "--if-changed", "--json"].iter().map(|s| s.to_string()).collect();
        let o = parse_args(&a).unwrap().unwrap();
        assert_eq!(o.game, PathBuf::from("G"));
        assert_eq!(o.decks_dir(), PathBuf::from("D"));
        assert_eq!(o.wheels_dir(), PathBuf::from("G").join("Caveman").join("Wheels"));
        assert_eq!(o.prefix, "000 - ");
        assert!(o.if_changed && o.json);
        assert!(parse_args(&["--nope".to_string()]).is_err());
        assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    }

    #[test]
    fn preview_mask() {
        let mask = Image::decode_png(PREVIEW_MASK).unwrap();
        assert_eq!((mask.width, mask.height, mask.channels), (512, 2048, 1));
        let art = Image::filled(512, 2048, &[200, 100, 0]);
        let p = preview(&art, &mask);
        assert_eq!(p.pixel(256, 1000), &[200, 100, 0]); // on the deck: the art
        assert_ne!(p.pixel(0, 0), &[200, 100, 0]); // off it: darkened
    }
}
