//! Caveman: the app. The library of decks and wheels in the drop folders, an editor for each deck's effects (its
//! settings file), a 3D preview, and the build (caveman-cli's, the one Caveman.exe runs) with ReSkate's launch.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use caveman_app::{art, bake, config, library};

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

#[tauri::command]
fn load_config(app: AppHandle, state: State<'_, AppState>) -> Result<Config, String> {
    current(&app, &state)
}

#[tauri::command]
fn save_config(app: AppHandle, state: State<'_, AppState>, config: Config) -> Result<Config, String> {
    config.save(&config_file(&app)?)?;
    *state.config.lock().unwrap() = Some(config.clone());
    Ok(config)
}

#[tauri::command]
fn library(app: AppHandle, state: State<'_, AppState>) -> Result<library::Library, String> {
    Ok(library::scan(&current(&app, &state)?))
}

#[tauri::command]
fn save_settings(source: String, settings: Value) -> Result<String, String> {
    library::save_settings(Path::new(&source), settings)
}

#[tauri::command]
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

/// A PNG's bytes, for the preview to read its pixels (an image loaded through the asset protocol is cross-origin, so
/// its pixels could not be read). PNG files only.
#[tauri::command]
fn read_png(path: String) -> Result<tauri::ipc::Response, String> {
    if !path.to_lowercase().ends_with(".png") {
        return Err(format!("not a PNG: {path}"));
    }
    std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| format!("{path}: {e}"))
}

#[tauri::command]
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

#[tauri::command]
fn launch(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    bake::launch(&current(&app, &state)?.game_dir())
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    bake::reveal(Path::new(&path))
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            load_config,
            save_config,
            library,
            save_settings,
            clear_settings,
            fitted_art,
            read_png,
            game_running,
            build,
            launch,
            reveal
        ])
        .run(tauri::generate_context!())
        .expect("error while running Caveman");
}
