//! Caveman: the app, a launcher for ReSkate in the shape of ReSkate's own (app/README.md). The Studio: the library of
//! decks and wheels in the drop folders with a switch per item, an editor for each deck's effects (its settings file),
//! a 3D preview, the build (caveman-cli's, the one Caveman.exe runs) and the pack generator; the Plugins: the OBS
//! link, the board cam window and the soundbars through the FX DLL and caveman-trainer; the Console; and Play, which
//! builds and starts ReSkate's launcher, then puts the FX DLL in.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use caveman_app::{art, bake, config, library, live, loadout, ring, widgets};

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use config::Config;

#[derive(Default)]
struct AppState {
    config: Mutex<Option<Config>>,
    building: Mutex<bool>,
    art: art::ArtState,
    /// Skate.exe's SHA-256, by its size and time (the front page's status; 144 MB read once).
    exe_hash: Mutex<Option<((u64, Option<std::time::SystemTime>), String)>>,
    /// The equipped board parts, from ReSkate's profile (read again when the profile changes).
    loadout: loadout::LoadoutCache,
    /// The picture on the board as the board cam window last asked (`board_art`).
    board_art: Mutex<BoardArtCache>,
}

/// The picture on the board as the board cam window last asked: the DLL's label, the fitted PNG it resolved to
/// (once per label), and when the label was last read the slow way; and for a deck other than The Caveman, its key
/// and the fitted PNG of its art (once per deck).
#[derive(Default)]
struct BoardArtCache {
    label: Option<String>,
    path: Option<String>,
    resolved: bool,
    full_at: Option<std::time::Instant>,
    deck_key: Option<String>,
    deck_path: Option<String>,
}

fn config_file(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|d| d.join("config.json")).map_err(|e| e.to_string())
}

fn current(app: &AppHandle, state: &AppState) -> Result<Config, String> {
    let mut slot = state.config.lock().unwrap();
    if slot.is_none() {
        *slot = Some(Config::load(&config_file(app)?));
    }
    Ok(slot.clone().unwrap())
}

#[tauri::command(async)]
fn load_config(app: AppHandle, state: State<'_, AppState>) -> Result<Config, String> {
    let config = current(&app, &state)?;
    adopt_old_folders(&config);
    Ok(config)
}

#[tauri::command(async)]
fn save_config(app: AppHandle, state: State<'_, AppState>, mut config: Config) -> Result<Config, String> {
    // a widget window's place is this side's to keep (it is saved as the window moves): what the front end sends
    // without one keeps the one on file
    if let Ok(stored) = current(&app, &state) {
        widgets::keep_geometry(&mut config.plugins, &stored.plugins);
    }
    config.save(&config_file(&app)?)?;
    *state.config.lock().unwrap() = Some(config.clone());
    adopt_old_folders(&config);
    Ok(config)
}

/// The one Caveman folder beside the game: the drop folders from before 2026-10-30 move into it (caveman_cli::folders),
/// at the app's start and when the game folder is set anew; the lines go to the log.
fn adopt_old_folders(config: &Config) {
    for line in caveman_cli::folders::adopt_old_folders(&config.game_dir()) {
        eprintln!("folders: {line}");
    }
}

#[tauri::command(async)]
fn library(app: AppHandle, state: State<'_, AppState>) -> Result<library::Library, String> {
    Ok(library::scan(&current(&app, &state)?))
}

#[tauri::command(async)]
fn save_settings(source: String, settings: Value) -> Result<String, String> {
    library::save_settings(Path::new(&source), settings)
}

#[tauri::command(async)]
fn clear_settings(source: String) -> Result<bool, String> {
    library::clear_settings(Path::new(&source))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Fitted {
    path: String,
    notes: Vec<String>,
}

#[tauri::command]
async fn fitted_art(app: AppHandle, state: State<'_, AppState>, source: String) -> Result<Fitted, String> {
    let config = current(&app, &state)?;
    let cache = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("art");
    let (path, notes) = art::fitted(&state.art, &config.game_dir(), Path::new(&source), &cache)?;
    Ok(Fitted { path: path.display().to_string(), notes })
}

/// A picture staged for the board earlier (`live_stage_png`, `live_read_picture`), by the name the DLL labels the
/// board with: its path, or null when there is no such file. For the board cam window to show the picture that is on
/// the board when it is not one of the library's decks.
#[tauri::command(async)]
fn live_staged_path(app: AppHandle, name: String) -> Result<Option<String>, String> {
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("live");
    Ok(staged_path(&dir, &name).map(|p| p.display().to_string()))
}

/// A staged picture in `dir` by the DLL's label for it (the file name as staged, cleaned, or its stem with `.png`).
fn staged_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || " ._-()".contains(c) { c } else { '_' }).collect();
    let stem = clean.trim().trim_end_matches(".png").trim_matches('.').trim().to_string();
    [dir.join(name), dir.join(format!("{stem}.png")), dir.join(&clean)].into_iter().find(|c| c.is_file())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BoardArt {
    /// The DLL answered (the game runs with the DLL in); else the label and the path are nothing.
    injected: bool,
    /// The DLL's label for the picture on the board (a file name), or null for the plain deck.
    label: Option<String>,
    /// The fitted PNG for it, or null (no picture, or one this app cannot find).
    path: Option<String>,
    /// Where the label came from ("status", "board status", "cached"), or why there is none.
    how: String,
    /// What decides the deck's graphic: "dll" (The Caveman is equipped: the DLL's picture, or the OBS link's video,
    /// is what is on it), "deck" (another of the library's decks is equipped: `path` is its art) or "model" (a game
    /// deck, or one whose art this app cannot read: the board model's own graphic).
    source: &'static str,
    /// The equipped deck's item key, as the profile has it (null when the profile cannot be read).
    deck: Option<String>,
}

/// The Caveman's item key: the deck the FX DLL draws on (`board::TEXTURE_NAME` is its texture with the default
/// prefix), "Own_000_-_The_Caveman".
fn the_caveman_key() -> String {
    caveman_core::fxmod::deck_key(&caveman_core::fxmod::cave_display("000 - "))
}

/// What is on the board in the game, for the board cam window's graphic layer. The equipped deck decides (ReSkate's
/// profile, as the grip, trucks and wheels): another deck than The Caveman shows its own art (a deck of the library,
/// fitted as the build fits it) or the model's graphic (a game deck); The Caveman shows what the DLL put on it: its
/// label for the picture, resolved to the fitted PNG through the library (a deck by file name or display name) or the
/// pictures the Live panel staged. The window asks every few seconds and at once when the app says the picture
/// changed (`force`): pulled, never pushed, so a window opened late cannot miss it. The label is in the light
/// `status` (DLLs from 2026-10-30 on); from an older DLL the full `board status` is read, at most every 12 s unless
/// forced. A profile that cannot be read counts as The Caveman (the board cam's first use).
#[tauri::command]
async fn board_art(app: AppHandle, state: State<'_, AppState>, force: bool) -> Result<BoardArt, String> {
    let config = current(&app, &state)?;
    let trainer = live::trainer_exe(&config)?;
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<BoardArt, String> {
        let state = handle.state::<AppState>();
        let deck = state.loadout.current().ok().map(|l| l.deck).filter(|d| !d.is_empty());
        if let Some(key) = deck.as_deref().filter(|k| !k.eq_ignore_ascii_case(&the_caveman_key())) {
            let known = {
                let c = state.board_art.lock().map_err(|e| e.to_string())?;
                (!force && c.deck_key.as_deref() == Some(key)).then(|| c.deck_path.clone())
            };
            let path = match known {
                Some(path) => path,
                None => {
                    let path = resolve_deck_art(&state, &config, key, &cache_dir);
                    let mut c = state.board_art.lock().map_err(|e| e.to_string())?;
                    c.deck_key = Some(key.to_string());
                    c.deck_path = path.clone();
                    path
                }
            };
            let source = if path.is_some() { "deck" } else { "model" };
            return Ok(BoardArt { injected: false, label: None, path, how: "the equipped deck".into(), source, deck });
        }
        let none = |how: String| BoardArt { injected: false, label: None, path: None, how, source: "dll", deck: deck.clone() };
        if !bake::game_running() {
            return Ok(none("the game is not running".into()));
        }
        let (label, how) = match live::board_label(&trainer) {
            Err(e) => return Ok(none(e)),
            Ok(Some(label)) => (label, "status"),
            Ok(None) => {
                let mut c = state.board_art.lock().map_err(|e| e.to_string())?;
                if force || c.full_at.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(12)) {
                    c.full_at = Some(std::time::Instant::now());
                    match live::board_label_full(&trainer) {
                        Ok(label) => (label, "board status"),
                        Err(e) => return Ok(none(e)),
                    }
                } else {
                    (c.label.clone(), "cached")
                }
            }
        };
        let known = {
            let c = state.board_art.lock().map_err(|e| e.to_string())?;
            (c.resolved && c.label == label).then(|| c.path.clone())
        };
        let path = match known {
            Some(path) => path,
            None => {
                let path = resolve_board_art(&state, &config, label.as_deref(), &cache_dir);
                let mut c = state.board_art.lock().map_err(|e| e.to_string())?;
                c.label = label.clone();
                c.path = path.clone();
                c.resolved = true;
                path
            }
        };
        Ok(BoardArt { injected: true, label, path, how: how.into(), source: "dll", deck })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The fitted PNG of an equipped deck's art, when it is one of the library's decks (by its item key) drawn from a
/// PNG; None for a game deck (the model has its graphic), an animated deck or one this app cannot fit.
fn resolve_deck_art(state: &AppState, config: &Config, key: &str, cache_dir: &Path) -> Option<String> {
    let lib = library::scan(config);
    let source = lib.decks.iter().find(|d| d.key.eq_ignore_ascii_case(key)).map(|d| PathBuf::from(&d.source))?;
    if !source.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        return None;
    }
    match art::fitted(&state.art, &config.game_dir(), &source, &cache_dir.join("art")) {
        Ok((path, _)) => Some(path.display().to_string()),
        Err(e) => {
            eprintln!("board art: {}: {e}", source.display());
            None
        }
    }
}

/// The fitted PNG for the DLL's label: a deck of the library by file name or display name, else a staged picture.
fn resolve_board_art(state: &AppState, config: &Config, label: Option<&str>, cache_dir: &Path) -> Option<String> {
    let label = label?;
    let lib = library::scan(config);
    let source = lib
        .decks
        .iter()
        .find(|d| Path::new(&d.source).file_name().and_then(|n| n.to_str()) == Some(label) || d.display == label)
        .map(|d| PathBuf::from(&d.source))
        .or_else(|| staged_path(&cache_dir.join("live"), label))?;
    if !source.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        return None;
    }
    match art::fitted(&state.art, &config.game_dir(), &source, &cache_dir.join("art")) {
        Ok((path, _)) => Some(path.display().to_string()),
        Err(e) => {
            eprintln!("board art: {}: {e}", source.display());
            None
        }
    }
}

/// The newest frame of the OBS link's video on the board (ring.rs), at level `level` of the ring's chain, when it is
/// not frame `after`: a 24-byte header (the sequence as u64, then the width, height and level as u32 and 4 spare
/// bytes, little endian) and the RGBA8 rows, top row first; empty when there is none (no video, nothing new yet, a
/// torn copy).
#[tauri::command]
fn board_frame(ring: State<'_, ring::RingState>, after: u64, level: u32) -> tauri::ipc::Response {
    let Some(f) = ring::newest(&ring, after, level) else { return tauri::ipc::Response::new(Vec::new()) };
    let mut out = Vec::with_capacity(24 + f.rgba.len());
    out.extend_from_slice(&f.seq.to_le_bytes());
    out.extend_from_slice(&f.width.to_le_bytes());
    out.extend_from_slice(&f.height.to_le_bytes());
    out.extend_from_slice(&f.level.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&f.rgba);
    tauri::ipc::Response::new(out)
}

/// A file's bytes for the windows: a PNG (the preview reads its pixels; an image loaded through the asset protocol
/// is cross-origin, so they could not be read) or the board model (.glb). Those kinds only.
#[tauri::command(async)]
fn read_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    let lower = path.to_lowercase();
    if !lower.ends_with(".png") && !lower.ends_with(".glb") {
        return Err(format!("not a PNG or a model: {path}"));
    }
    std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| format!("{path}: {e}"))
}

/// The game's skateboard as the player rides it (the deck, grip, trucks and wheels the profile says are equipped, on
/// the board rig, with their textures) as a glTF binary for the board cam window, built from the player's install
/// into the app's cache once per game build, Caveman version and loadout (`caveman_core::boardmodel`). The file's
/// path; the window asks again now and then and reloads when it changes. A profile that cannot be read gives the
/// stock board.
#[tauri::command]
async fn board_model(app: AppHandle, state: State<'_, AppState>, force: bool) -> Result<String, String> {
    let config = current(&app, &state)?;
    let game_dir = config.game_dir();
    let exe = std::fs::metadata(game_dir.join("Skate.exe")).map_err(|e| format!("Skate.exe: {e}"))?;
    let modified = exe.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    // `CAVEMAN_LOADOUT` in the app's environment forces a loadout (JSON with deck, grip, cutout, trucks, wheels item
    // keys), for looking at parts that are not equipped
    let forced: Option<caveman_core::boardmodel::Loadout> =
        std::env::var("CAVEMAN_LOADOUT").ok().and_then(|j| serde_json::from_str::<Value>(&j).ok()).map(|v| {
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
            caveman_core::boardmodel::Loadout { deck: s("deck"), grip: s("grip"), cutout: s("cutout"), trucks: s("trucks"), wheels: s("wheels") }
        });
    let mut loadout = match (forced, state.loadout.current()) {
        (Some(l), _) => l,
        (None, Ok(l)) => l,
        (None, Err(why)) => {
            eprintln!("board model: the profile's loadout: {why}; the stock board");
            caveman_core::boardmodel::Loadout::stock()
        }
    };
    // a deck that is not one of the game's own items (the game's are `Own_DeckGraphic_...` and `Own_Deck_...`; the
    // library's decks and The Caveman are not) has no graphic in the game's files: the model is the same for all of
    // them, with the window laying the deck's art on it (`board_art`), so they share one instead of a build each
    let deck = loadout.deck.to_lowercase();
    if !(deck.starts_with("own_deckgraphic_") || deck.starts_with("own_deck_")) {
        loadout.deck.clear();
    }
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("board");
    let out = dir.join(format!("skateboard-{}-{}-{modified}-{}.glb", env!("CARGO_PKG_VERSION"), exe.len(), loadout.key()));
    if !force && out.is_file() {
        return Ok(out.display().to_string());
    }
    let target = out.clone();
    let facts = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let game = caveman_core::game::Game::open(&game_dir).map_err(|e| format!("the game at {}: {e}", game_dir.display()))?;
        caveman_core::boardmodel::build_for(&game, &loadout, &target).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    eprintln!("board model: {facts}");
    // older models go (the newest six stay: the loadouts the player switches between)
    prune_models(&dir, 6);
    Ok(out.display().to_string())
}

/// Removes all but the newest `keep` models of a cache folder.
fn prune_models(dir: &Path, keep: usize) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .filter(|(_, p)| p.extension().is_some_and(|x| x == "glb"))
            .collect();
        files.sort_by_key(|x| std::cmp::Reverse(x.0));
        for (_, p) in files.into_iter().skip(keep) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The board for the Studio's 3D preview: the stock board as the game draws it (the deck mesh, black grip, the
/// default trucks, natural wheels), or with one of the player's own wheels on it (`wheel`, its PNG, in `colour`,
/// "#rrggbb", as the build would make it), built from the player's install like the board cam's (`board_model`) and
/// cached beside it: one file per game build, Caveman version and wheel (its path, size, time and colour). Builds
/// take turns, and a model is written under another name and renamed, so a preview never reads half a file.
#[tauri::command]
async fn studio_board_model(app: AppHandle, state: State<'_, AppState>, wheel: Option<String>, colour: Option<String>) -> Result<String, String> {
    use caveman_core::boardmodel::{self, Loadout, OwnWheel};
    let config = current(&app, &state)?;
    let game_dir = config.game_dir();
    let exe = std::fs::metadata(game_dir.join("Skate.exe")).map_err(|e| format!("Skate.exe: {e}"))?;
    let seconds = |m: &std::fs::Metadata| m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let rgb = colour.as_deref().and_then(|c| {
        let h = c.trim_start_matches('#');
        (h.len() == 6).then(|| [0, 2, 4].map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(255)))
    });
    // the wheel's part of the file name: a hash of what makes its look
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let wheel_key = match &wheel {
        None => "stock".to_string(),
        Some(path) => {
            let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
            let text = format!("{}|{}|{}|{rgb:?}", path.to_lowercase(), meta.len(), seconds(&meta));
            for b in text.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            format!("{h:016x}")
        }
    };
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("studio-board");
    let out = dir.join(format!("studio-{}-{}-{}-{wheel_key}.glb", env!("CARGO_PKG_VERSION"), exe.len(), seconds(&exe)));
    if out.is_file() {
        return Ok(out.display().to_string());
    }
    let target = out.clone();
    let built = tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        static BUILDING: Mutex<()> = Mutex::new(());
        let _one_at_a_time = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
        if target.is_file() {
            return Ok(None);
        }
        let own = match &wheel {
            Some(path) => {
                Some(OwnWheel { face: caveman_core::image::Image::read_png(path).map_err(|e| format!("{path}: {e}"))?, colour: rgb.unwrap_or([255, 255, 255]) })
            }
            None => None,
        };
        let game = caveman_core::game::Game::open(&game_dir).map_err(|e| format!("the game at {}: {e}", game_dir.display()))?;
        let part = target.with_extension("glb.part");
        let facts = boardmodel::build_with(&game, &Loadout::stock(), own.as_ref(), &part).map_err(|e| e.to_string())?;
        std::fs::rename(&part, &target).map_err(|e| format!("{}: {e}", target.display()))?;
        Ok(Some(facts))
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some(facts) = built {
        eprintln!("studio board model: {facts}");
        // the stock board and the last few wheels looked at
        prune_models(&dir, 6);
    }
    Ok(out.display().to_string())
}

// A plain command runs on the main thread, where every event to a webview is dispatched too: `game_running`
// there (tasklist, 100 ms and more every 4 s) held the pose events back, which froze the board cam on that beat
// (2026-11-01). Commands marked `async` run on Tauri's thread pool; the ones left plain return in microseconds.
#[tauri::command(async)]
fn game_running() -> bool {
    bake::game_running()
}

#[tauri::command]
async fn build(app: AppHandle, state: State<'_, AppState>, force: bool) -> Result<bake::BuildResult, String> {
    let config = current(&app, &state)?;
    {
        let mut busy = state.building.lock().unwrap();
        if *busy {
            return Err("A build is already running.".into());
        }
        *busy = true;
    }
    let emitter = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        bake::build(&config, force, move |event| {
            let _ = emitter.emit("build-event", event);
        })
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    *state.building.lock().unwrap() = false;
    result
}

/// Starts ReSkate's launcher (the front end builds first); then, when the settings say so, the FX DLL and the staging
/// ring go in once the game is up, as Caveman.exe does (nothing without a trainer). Returns the attach command line.
#[tauri::command(async)]
fn launch(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    let config = current(&app, &state)?;
    bake::launch(&config.game_dir())?;
    if !config.attach {
        return Ok(None);
    }
    Ok(live::trainer_exe(&config).ok().and_then(|t| live::attach_now(&t, &config.game_dir(), config.ring_mb, 600)))
}

/// Puts the FX DLL into the game now (it waits up to a minute for the game if it is still starting). The command line.
#[tauri::command(async)]
fn attach_dll(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let config = current(&app, &state)?;
    let trainer = live::trainer_exe(&config)?;
    live::attach_now(&trainer, &config.game_dir(), config.ring_mb, 60).ok_or_else(|| "the attach helper could not be started".to_string())
}

/// Switches decks or wheels off or on for the build (`.caveman-off.json` in their drop folder): one, a folder's or all
/// of them. Returns how many changed.
#[tauri::command(async)]
fn set_items_off(app: AppHandle, state: State<'_, AppState>, kind: String, sources: Vec<String>, off: bool) -> Result<usize, String> {
    library::set_off(&current(&app, &state)?, &kind, &sources, off)
}

/// The front page's status: the game folder, the game's build, ReSkate's version, whether the mods on disk are what a
/// build would write, what is installed of the FX DLL, and how many decks and wheels are on.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LauncherStatus {
    game: String,
    game_ok: bool,
    /// "match", "other", "missing".
    skate_build: &'static str,
    reskate_version: Option<String>,
    dll_installed: bool,
    trainer: Option<String>,
    /// "up_to_date", "stale", "none".
    mods_built: &'static str,
    decks: usize,
    decks_on: usize,
    wheels: usize,
    wheels_on: usize,
}

/// ReSkate's supported Skate.exe (Steam build 25414733), as caveman_fx::game::EXE_SHA256 has it.
const EXE_SHA256: &str = "fbce74d5e28ef525dbba2cb4adbebc13405bdbd88f31bc940bca45e4ae88b8f9";

#[tauri::command]
async fn launcher_status(app: AppHandle, state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    let config = current(&app, &state)?;
    let cached = state.exe_hash.lock().unwrap().clone();
    let status = tauri::async_runtime::spawn_blocking(move || {
        let game = config.game_dir();
        let exe = game.join("Skate.exe");
        let game_ok = exe.is_file() && game.join("ReSkateLauncher.exe").is_file();
        // the exe's hash is 144 MB of reading: once per size and time
        let stamp = std::fs::metadata(&exe).ok().map(|m| (m.len(), m.modified().ok()));
        let (hash, stamp) = match (&cached, &stamp) {
            (Some((s, h)), Some(now)) if s == now => (Some(h.clone()), stamp),
            (_, Some(_)) => (std::fs::read(&exe).ok().map(|b| hex(&caveman_core::build::sha256(&b))), stamp),
            _ => (None, None),
        };
        let skate_build = match &hash {
            Some(h) if h == EXE_SHA256 => "match",
            Some(_) => "other",
            None => "missing",
        };
        let lib = library::scan(&config);
        LauncherStatus {
            game: game.display().to_string(),
            game_ok,
            skate_build,
            reskate_version: file_version(&game.join("ReSkateLauncher.exe")),
            dll_installed: game.join("Caveman").join("caveman_fx.dll").is_file() && game.join("Caveman").join("caveman-trainer.exe").is_file(),
            trainer: live::trainer_exe(&config).ok().map(|p| p.display().to_string()),
            mods_built: bake::mods_state(&config),
            decks: lib.decks.len(),
            decks_on: lib.decks.iter().filter(|d| d.enabled).count(),
            wheels: lib.wheels.len(),
            wheels_on: lib.wheels.iter().filter(|w| w.enabled).count(),
        }
        .pipe(|s| (s, stamp.zip(hash)))
    })
    .await
    .map_err(|e| e.to_string())?;
    let (status, hashed) = status;
    if let Some((stamp, hash)) = hashed {
        *state.exe_hash.lock().unwrap() = Some((stamp, hash));
    }
    Ok(status)
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A Windows executable's file version ("2.1.3"), from its version resource. version.dll is loaded at run time: the
/// GNU toolchain used here has no import library for it.
fn file_version(exe: &Path) -> Option<String> {
    use std::ffi::c_void;
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }
    type SizeFn = unsafe extern "system" fn(*const u16, *mut u32) -> u32;
    type InfoFn = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void) -> i32;
    type QueryFn = unsafe extern "system" fn(*const c_void, *const u16, *mut *mut c_void, *mut u32) -> i32;
    if !exe.is_file() {
        return None;
    }
    let wide: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let root: Vec<u16> = "\\".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let dll_name: Vec<u16> = "version.dll".encode_utf16().chain(std::iter::once(0)).collect();
        let dll = LoadLibraryW(dll_name.as_ptr());
        if dll.is_null() {
            return None;
        }
        let size_fn = GetProcAddress(dll, c"GetFileVersionInfoSizeW".as_ptr().cast());
        let info_fn = GetProcAddress(dll, c"GetFileVersionInfoW".as_ptr().cast());
        let query_fn = GetProcAddress(dll, c"VerQueryValueW".as_ptr().cast());
        if size_fn.is_null() || info_fn.is_null() || query_fn.is_null() {
            return None;
        }
        let size_of: SizeFn = std::mem::transmute(size_fn);
        let info: InfoFn = std::mem::transmute(info_fn);
        let query: QueryFn = std::mem::transmute(query_fn);
        let mut handle = 0u32;
        let size = size_of(wide.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if info(wide.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
            return None;
        }
        let mut out: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if query(data.as_ptr() as *const c_void, root.as_ptr(), &mut out, &mut len) == 0 || len < 20 || out.is_null() {
            return None;
        }
        // VS_FIXEDFILEINFO: dwSignature, dwStrucVersion, dwFileVersionMS, dwFileVersionLS
        let words = std::slice::from_raw_parts(out as *const u32, 5);
        if words[0] != 0xFEEF_04BD {
            return None;
        }
        let (ms, ls) = (words[2], words[3]);
        let parts = [ms >> 16, ms & 0xFFFF, ls >> 16, ls & 0xFFFF];
        Some(if parts[3] == 0 { format!("{}.{}.{}", parts[0], parts[1], parts[2]) } else { format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]) })
    }
}

/// The deck and wheel templates (Caveman's own drawings in assets/) written to `<game>\Caveman\Templates`, for
/// artists to start from. Returns the folder.
#[tauri::command(async)]
fn templates_save(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    const TEMPLATES: [(&str, &[u8]); 3] = [
        ("Caveman Deck Template 1024x4096.png", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/Caveman Deck Template 1024x4096.png"))),
        ("Deck Outline 512x2048.png", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/deck_template_512x2048.png"))),
        ("Wheel Template 1024.png", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/wheel_template_1024.png"))),
    ];
    let config = current(&app, &state)?;
    let dir = caveman_cli::folders::templates(&config.game_dir());
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, bytes) in TEMPLATES {
        let path = dir.join(name);
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let readme = "# Caveman templates\n\nDeck art is 512 x 2048 (or any size with the same 1:4 shape; the template is drawn at twice that). The red line is the deck's edge, the cyan line the safe zone, the orange marks the trucks and bolts. Paint past the red line to the canvas edge. Save as PNG and drop it in the Decks folder beside this one (Caveman\\Decks).\n\nWheel art is square (1024 x 1024 here): the outer face is the top half as an ellipse, the inner face the bottom half. Name the file Name.ff0000.png for a coloured wheel.\n";
    std::fs::write(dir.join("README.md"), readme).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// A pack's specification from the front end (Studio, Packs).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackSpec {
    owner: String,
    name: String,
    version: String,
    description: String,
    website: String,
    icon: Option<String>,
    out: Option<String>,
    items: Vec<PackSpecItem>,
}

#[derive(serde::Deserialize)]
struct PackSpecItem {
    kind: String,
    source: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PackOut {
    folder: String,
    zip: String,
    zip_bytes: u64,
    decks: usize,
    wheels: usize,
    shader_decks: Vec<String>,
}

/// Builds a Thunderstore-ready package (`caveman pack`) from chosen decks and wheels; its events go out as
/// `pack-event`. Nothing in the game folder changes.
#[tauri::command]
async fn pack_build(app: AppHandle, state: State<'_, AppState>, spec: PackSpec) -> Result<PackOut, String> {
    use caveman_cli::names::Kind;
    use caveman_cli::pack::{self, PackItem, PackOptions};
    let config = current(&app, &state)?;
    {
        let mut busy = state.building.lock().unwrap();
        if *busy {
            return Err("A build is already running.".into());
        }
        *busy = true;
    }
    let (decks_dir, wheels_dir) = (config.decks_dir(), config.wheels_dir());
    let items = spec
        .items
        .iter()
        .map(|i| {
            let kind = if i.kind == "wheel" { Kind::Wheel } else { Kind::Deck };
            let root = if kind == Kind::Wheel { &wheels_dir } else { &decks_dir };
            PackItem { kind, rel: pack::rel_in(root, Path::new(&i.source)), path: PathBuf::from(&i.source) }
        })
        .collect();
    let options = PackOptions {
        game: config.game_dir(),
        out: spec.out.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from),
        owner: spec.owner.trim().to_string(),
        name: spec.name.trim().to_string(),
        version: spec.version.trim().to_string(),
        description: spec.description.trim().to_string(),
        website: spec.website.trim().to_string(),
        icon: spec.icon.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from),
        items,
        cache: Some(config.cache.trim()).filter(|c| !c.is_empty()).map(PathBuf::from),
        stage: None,
        json: true,
    };
    let emitter = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut report = bake::json_reporter(move |event| {
            let _ = emitter.emit("pack-event", event);
        });
        pack::run_pack(&options, &mut report).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    *state.building.lock().unwrap() = false;
    let r = result?;
    Ok(PackOut {
        folder: r.folder.display().to_string(),
        zip: r.zip.display().to_string(),
        zip_bytes: r.zip_bytes,
        decks: r.decks,
        wheels: r.wheels,
        shader_decks: r.shader_decks,
    })
}

/// A widget window's pose stream (`caveman-trainer pose`), relayed as `board-pose` events to every window: one trainer
/// process for all of them, stopped with the last window that asked (the asking window is the user). The command line.
#[tauri::command(async)]
fn pose_start(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    live_state: State<'_, live::LiveState>,
    rate: Option<u32>,
) -> Result<String, String> {
    let trainer = live::trainer_exe(&current(&app, &state)?)?;
    live::pose_start(&app, &trainer, &live_state, rate.unwrap_or(60), window.label())
}

#[tauri::command]
fn pose_stop(window: tauri::Window, live_state: State<'_, live::LiveState>) {
    live::pose_release(&live_state, window.label());
}

/// A widget's window (widgets.rs): see-through, without a frame, above everything, for OBS to capture by its title.
/// One per kind, labelled `widget-<kind>`; an open one is closed and made again, so new options apply. It opens where
/// it was last (the settings keep its place and size), else near the top right corner. Async: closing the old window
/// needs the event loop, which a blocking command would hold.
#[tauri::command]
async fn widget_open(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: String,
    title: String,
    query: String,
    width: u32,
    height: u32,
    index: usize,
) -> Result<(), String> {
    let label = widgets::label(&kind);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
        for _ in 0..40 {
            if app.get_webview_window(&label).is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let config = current(&app, &state)?;
    let g = widgets::place(&app, widgets::saved(&config.plugins, &kind), width.clamp(80, 4000), height.clamp(40, 4000), index);
    let mut builder = tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App(format!("index.html?view=widget&kind={kind}&{query}").into()));
    // every webview of the app must be made with the same browser arguments (WebView2 refuses a second one made with
    // other options for the same user data folder: 0x8007139F), so the DevTools port goes on this one too
    if let Some(args) = browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    builder
        .title(&title)
        .inner_size(g.w as f64, g.h as f64)
        .position(g.x as f64, g.y as f64)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(true)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = app.emit("widget-state", serde_json::json!({"kind": kind, "open": true}));
    Ok(())
}

/// Closes a widget's window; its reading of the pose stream and the `widget-state` event follow from the window's end.
#[tauri::command]
fn widget_close(app: AppHandle, kind: String) {
    if let Some(w) = app.get_webview_window(&widgets::label(&kind)) {
        let _ = w.close();
    }
}

/// The kinds whose windows are open.
#[tauri::command]
fn widget_list(app: AppHandle) -> Vec<String> {
    app.webview_windows().keys().filter_map(|l| widgets::kind_of(l).map(str::to_string)).collect()
}

/// A widget window's place and size into the settings (`plugins.widgets.<kind>.window`), for its next opening.
fn save_widget_geometry(app: &AppHandle, label: &str, g: widgets::Geometry) {
    let Some(kind) = widgets::kind_of(label) else { return };
    let state = app.state::<AppState>();
    let Ok(mut config) = current(app, &state) else { return };
    if widgets::saved(&config.plugins, kind) == Some(g) {
        return;
    }
    widgets::store(&mut config.plugins, kind, g);
    if let Ok(file) = config_file(app) {
        let _ = config.save(&file);
    }
    *state.config.lock().unwrap() = Some(config);
}

#[tauri::command(async)]
fn reveal(path: String) -> Result<(), String> {
    bake::reveal(Path::new(&path))
}

// ---- the Live panel (live.rs): The Caveman in the running game through caveman-trainer.exe

/// Runs one trainer command (`--game` added where it reads the install and the app's game folder is not the default).
#[tauri::command]
async fn live(app: AppHandle, state: State<'_, AppState>, args: Vec<String>) -> Result<live::LiveOut, String> {
    let config = current(&app, &state)?;
    let trainer = live::trainer_exe(&config)?;
    let args = live::with_game(&args, &config.game_dir());
    tauri::async_runtime::spawn_blocking(move || live::run(&trainer, &args)).await.map_err(|e| e.to_string())?
}

/// The panel's poll: the DLL's state in a few fields, The Caveman's too when `board` is true.
#[tauri::command]
async fn live_status(app: AppHandle, state: State<'_, AppState>, board: bool) -> Result<Value, String> {
    let trainer = live::trainer_exe(&current(&app, &state)?);
    tauri::async_runtime::spawn_blocking(move || match trainer {
        Ok(t) => live::status(&t, board),
        Err(e) => serde_json::json!({ "trainer": null, "trainer_error": e, "game": bake::game_running(), "injected": false, "error": e, "dry": live::dry() }),
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn cave_effects() -> Vec<live::CaveFxInfo> {
    live::cave_effects()
}

/// Starts the video producer; returns its command line.
#[tauri::command(async)]
fn live_video_start(app: AppHandle, state: State<'_, AppState>, video: State<'_, live::LiveState>, args: Vec<String>) -> Result<String, String> {
    let trainer = live::trainer_exe(&current(&app, &state)?)?;
    live::video_start(&app, &trainer, &args, &video)
}

#[tauri::command]
async fn live_video_stop(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let trainer = live::trainer_exe(&current(&app, &state)?)?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || live::video_stop(&trainer, &handle.state::<live::LiveState>())).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn live_video_running(video: State<'_, live::LiveState>) -> bool {
    live::video_running(&video)
}

/// A picture's bytes (PNG, JPG, WebP, BMP, GIF), for the webview to make a PNG of it.
#[tauri::command(async)]
fn live_read_picture(path: String) -> Result<tauri::ipc::Response, String> {
    live::read_picture(Path::new(&path)).map(tauri::ipc::Response::new)
}

/// A PNG made in the webview (the request's body; its name in the `x-name` header) written to the app's cache
/// (`%LOCALAPPDATA%\dev.caveman.app\live`); returns the path, for `board art`.
#[tauri::command(async)]
fn live_stage_png(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected the PNG's bytes".into());
    };
    let name = request.headers().get("x-name").and_then(|v| v.to_str().ok()).unwrap_or("picture").to_string();
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("live");
    live::stage_png(&dir, &name, bytes).map(|p| p.display().to_string())
}

/// On the way out: a video still playing is stopped (else the board would keep its last frame).
fn stop_video_on_exit(app: &AppHandle) {
    if !live::video_running(&app.state::<live::LiveState>()) {
        return;
    }
    let state = app.state::<AppState>();
    if let Ok(trainer) = current(app, &state).and_then(|c| live::trainer_exe(&c)) {
        let _ = live::video_stop(&trainer, &app.state::<live::LiveState>());
    }
}

/// The main window, made here rather than by Tauri from the config (`create: false` there) so that
/// `CAVEMAN_DEVTOOLS_PORT=N` can open WebView2's DevTools port, for driving the app from a script
/// (`tools/screenshot.mjs` with `ATTACH=N`). WebView2's own `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` does not reach it:
/// wry sets the browser arguments itself.
fn main_window(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let config = app.config().app.windows.iter().find(|w| w.label == "main").cloned().ok_or("no main window in tauri.conf.json")?;
    let mut builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?;
    if let Some(args) = browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    builder.build()?;
    Ok(())
}

/// WebView2's browser arguments when `CAVEMAN_DEVTOOLS_PORT=N` is set (wry's own, then the port); None otherwise.
/// The same for every window of the app.
fn browser_args() -> Option<String> {
    let port = std::env::var("CAVEMAN_DEVTOOLS_PORT").ok().filter(|p| p.trim().parse::<u16>().is_ok())?;
    Some(format!("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={}", port.trim()))
}

fn main() {
    tauri::Builder::default()
        .setup(main_window)
        .manage(AppState::default())
        .manage(live::LiveState::default())
        .manage(ring::RingState::default())
        .manage(widgets::WidgetState::default())
        .on_window_event(|window, event| {
            let app = window.app_handle().clone();
            let label = window.label().to_string();
            match event {
                tauri::WindowEvent::Destroyed => {
                    // a window's reading of the pose stream ends with it (the stream itself with the last reader)
                    live::pose_release(&app.state::<live::LiveState>(), &label);
                    if let Some(kind) = widgets::kind_of(&label) {
                        let _ = app.emit("widget-state", serde_json::json!({"kind": kind, "open": false}));
                    }
                    // the widget windows go with the main one (on their own they would keep the app alive, drawn
                    // over whatever is on the screen)
                    if label == "main" {
                        for (l, w) in app.webview_windows() {
                            if widgets::kind_of(&l).is_some() {
                                let _ = w.close();
                            }
                        }
                    }
                }
                // a widget window's place and size are kept, once it has held still for a moment
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) if widgets::kind_of(&label).is_some() => {
                    if let (Ok(pos), Ok(size), Ok(scale)) = (window.outer_position(), window.inner_size(), window.scale_factor()) {
                        let g = widgets::Geometry {
                            x: (pos.x as f64 / scale).round() as i32,
                            y: (pos.y as f64 / scale).round() as i32,
                            w: (size.width as f64 / scale).round() as u32,
                            h: (size.height as f64 / scale).round() as u32,
                        };
                        if g.w < 80 || g.h < 40 {
                            return;
                        }
                        let at = app.state::<widgets::WidgetState>().note(&label, g);
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(600));
                            if let Some(g) = app.state::<widgets::WidgetState>().settled(&label, at) {
                                save_widget_geometry(&app, &label, g);
                            }
                        });
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_config,
            save_config,
            library,
            save_settings,
            clear_settings,
            fitted_art,
            read_bytes,
            board_model,
            board_art,
            board_frame,
            live_staged_path,
            game_running,
            build,
            launch,
            attach_dll,
            set_items_off,
            studio_board_model,
            launcher_status,
            templates_save,
            pack_build,
            reveal,
            live,
            live_status,
            cave_effects,
            live_video_start,
            live_video_stop,
            live_video_running,
            live_read_picture,
            live_stage_png,
            pose_start,
            pose_stop,
            widget_open,
            widget_close,
            widget_list
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Caveman")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                stop_video_on_exit(app);
                live::pose_stop(&app.state::<live::LiveState>());
            }
        });
}
