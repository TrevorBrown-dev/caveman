//! The deck art as the build puts it on the deck (caveman_core::build::fit_art: cover-cropped to 1:4, turned upright
//! when drawn sideways, flattened onto the deck's colour or the Natural Blank wood), for the 3D preview. Written once per PNG version into
//! the app's cache folder; the game is opened once (its wood texture is the backdrop).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use caveman_core::build;
use caveman_core::game::Game;
use caveman_core::image::Image;

#[derive(Default)]
pub struct ArtState {
    /// (game folder, the game, its Natural Blank wood)
    game: Mutex<Option<(PathBuf, Arc<Game>, Arc<Image>)>>,
}

impl ArtState {
    fn wood(&self, game_dir: &Path) -> Result<Arc<Image>, String> {
        let mut slot = self.game.lock().unwrap();
        if let Some((dir, _, wood)) = slot.as_ref() {
            if dir == game_dir {
                return Ok(wood.clone());
            }
        }
        let game = Arc::new(Game::open(game_dir).map_err(|e| format!("the game at {}: {e}", game_dir.display()))?);
        let wood = Arc::new(build::natural_wood(&game).map_err(|e| e.to_string())?);
        *slot = Some((game_dir.to_path_buf(), game, wood.clone()));
        Ok(wood)
    }
}

/// The fitted art of a deck PNG as a cached PNG file; returns its path and the fit's notes.
pub fn fitted(state: &ArtState, game_dir: &Path, source: &Path, cache_dir: &Path) -> Result<(PathBuf, Vec<String>), String> {
    let meta = std::fs::metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    game_dir.hash(&mut h);
    let paint = caveman_cli::names::deck_paint(source);
    if paint.is_some() {
        paint.hash(&mut h);
    }
    let out = cache_dir.join(format!("{:016x}.png", h.finish()));
    let notes_file = out.with_extension("txt");
    if out.is_file() {
        let notes = std::fs::read_to_string(&notes_file).unwrap_or_default();
        return Ok((out, notes.lines().filter(|l| !l.is_empty()).map(String::from).collect()));
    }
    let wood = state.wood(game_dir)?;
    let img = Image::read_png(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let (art, notes) = build::fit_art_on(&img, &wood, paint).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    art.write_png(&out).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&notes_file, notes.join("\n"));
    Ok((out, notes))
}
