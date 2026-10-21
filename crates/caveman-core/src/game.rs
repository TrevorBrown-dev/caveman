//! The installed game: TOCs, bundles, files, texture images (gamedata.py), and mod folders layered over it
//! (fb.Layer, caveman.ModLayer).
//!
//! File locations name an install chunk; `layout.toc`'s install manifest maps each chunk's `persistentIndex` (as an
//! unsigned 32-bit number: initialinstallpackage is 3162564249) to its package folder, which holds the `cas_NN.cas`
//! archives. The Python `Game` always reads `initialinstallpackage`; this resolves every chunk, falling back to that
//! package for an unknown one.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::bundle::{self, BundleError, FileLoc, Manifest, MOD_INSTALL_CHUNK};
use crate::cas::{self, CasError};
use crate::ndb::{self, NdbError};
use crate::toc::{self, Guid, Toc, TocChunk, TocError, ENVELOPE};

/// The package of the game's base data (`gamedata.PKG`), relative to `Data/Win32`.
pub const PKG: &str = "configurations/layout/initialinstallpackage";
/// The superbundle the item and deck data live in (`gamedata.Game`'s default TOC).
pub const ITEMS_TOC: &str = "items.toc";

#[derive(Debug)]
pub enum GameError {
    Io(PathBuf, std::io::Error),
    Toc(TocError),
    Bundle(BundleError),
    Cas(CasError),
    Ndb(NdbError),
    /// No bundle of that name in the TOC.
    NoBundle(String),
    /// A bundle without a region in its TOC (load flag not 1).
    NoRegion(String),
    /// A file location's archive is missing, or ambiguous in a mod folder.
    Archive(String),
}

impl fmt::Display for GameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GameError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            GameError::Toc(e) => write!(f, "{e}"),
            GameError::Bundle(e) => write!(f, "{e}"),
            GameError::Cas(e) => write!(f, "{e}"),
            GameError::Ndb(e) => write!(f, "{e}"),
            GameError::NoBundle(n) => write!(f, "no bundle {n}"),
            GameError::NoRegion(n) => write!(f, "bundle {n} has no region in its TOC"),
            GameError::Archive(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for GameError {}

impl From<TocError> for GameError {
    fn from(e: TocError) -> Self {
        GameError::Toc(e)
    }
}
impl From<BundleError> for GameError {
    fn from(e: BundleError) -> Self {
        GameError::Bundle(e)
    }
}
impl From<CasError> for GameError {
    fn from(e: CasError) -> Self {
        GameError::Cas(e)
    }
}
impl From<NdbError> for GameError {
    fn from(e: NdbError) -> Self {
        GameError::Ndb(e)
    }
}

/// A parsed TOC with lookups (`gamedata.Game.toc`'s `bundle_by_name` and `chunk_by_guid`).
#[derive(Debug)]
pub struct TocIndex {
    pub toc: Toc,
    bundle_by_name: HashMap<String, usize>,
    chunk_by_guid: HashMap<Guid, usize>,
}

impl TocIndex {
    pub fn new(toc: Toc) -> TocIndex {
        let mut bundle_by_name = HashMap::with_capacity(toc.bundles.len());
        for (i, b) in toc.bundles.iter().enumerate() {
            bundle_by_name.insert(b.name.clone(), i);
        }
        let mut chunk_by_guid = HashMap::with_capacity(toc.chunks.len());
        for (i, c) in toc.chunks.iter().enumerate() {
            chunk_by_guid.insert(c.guid, i);
        }
        TocIndex { toc, bundle_by_name, chunk_by_guid }
    }

    /// The bundle record named `name` (the last of that name, as a Python dict keeps).
    pub fn bundle(&self, name: &str) -> Option<&toc::TocBundle> {
        self.bundle_by_name.get(name).map(|&i| &self.toc.bundles[i])
    }

    /// The chunk record `guid` (the last with that GUID, as a Python dict keeps).
    pub fn chunk(&self, guid: &Guid) -> Option<&TocChunk> {
        self.chunk_by_guid.get(guid).map(|&i| &self.toc.chunks[i])
    }

    /// The first bundle, in TOC order, whose name ends with `suffix` (`gamedata.Game.find_bundle`).
    pub fn find_bundle(&self, suffix: &str) -> Option<&str> {
        self.toc.bundles.iter().map(|b| b.name.as_str()).find(|n| n.ends_with(suffix))
    }
}

/// A bundle of the game as read (`gamedata.Game.bundle`): its region, every file's location (the manifest first),
/// the manifest's bytes and the parsed manifest.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub name: String,
    pub region: Vec<u8>,
    pub files: Vec<FileLoc>,
    pub manifest_raw: Vec<u8>,
    pub manifest: Manifest,
}

impl Bundle {
    /// The location of EBX `i` (manifest order).
    pub fn ebx_loc(&self, i: usize) -> Option<FileLoc> {
        self.files.get(self.manifest.ebx_file(i)).copied()
    }

    /// The location of resource `i`.
    pub fn res_loc(&self, i: usize) -> Option<FileLoc> {
        self.files.get(self.manifest.res_file(i)).copied()
    }

    /// The location of chunk `i`.
    pub fn chunk_loc(&self, i: usize) -> Option<FileLoc> {
        self.files.get(self.manifest.chunk_file(i)).copied()
    }
}

/// An open file cache shared by readers.
#[derive(Debug, Default)]
struct Handles(Mutex<HashMap<PathBuf, Arc<File>>>);

impl Handles {
    fn read(&self, path: &Path, offset: u32, size: u32) -> Result<Vec<u8>, GameError> {
        let file = {
            let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
            match map.get(path) {
                Some(f) => f.clone(),
                None => {
                    let f = Arc::new(File::open(path).map_err(|e| GameError::Io(path.to_path_buf(), e))?);
                    map.insert(path.to_path_buf(), f.clone());
                    f
                }
            }
        };
        let mut out = vec![0u8; size as usize];
        let mut done = 0;
        while done < out.len() {
            let n = file
                .seek_read(&mut out[done..], offset as u64 + done as u64)
                .map_err(|e| GameError::Io(path.to_path_buf(), e))?;
            if n == 0 {
                return Err(GameError::Io(
                    path.to_path_buf(),
                    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read past the end of the archive"),
                ));
            }
            done += n;
        }
        Ok(out)
    }
}

/// The installed game (`gamedata.Game`).
#[derive(Debug)]
pub struct Game {
    root: PathBuf,
    data: PathBuf,
    /// Install chunk -> package folder (absolute).
    installs: HashMap<u32, PathBuf>,
    handles: Handles,
    tocs: Mutex<HashMap<String, Arc<TocIndex>>>,
}

impl Game {
    /// Opens the game at `root` (the folder with `Skate.exe`); reads `Data/layout.toc`'s install chunks.
    pub fn open(root: &Path) -> Result<Game, GameError> {
        let data = root.join("Data");
        let layout = data.join("layout.toc");
        let raw = std::fs::read(&layout).map_err(|e| GameError::Io(layout.clone(), e))?;
        let installs = install_packages(&raw, &data)?;
        Ok(Game {
            root: root.to_path_buf(),
            data,
            installs,
            handles: Handles::default(),
            tocs: Mutex::new(HashMap::new()),
        })
    }

    /// The game folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `Data` folder.
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// The package folder of an install chunk (absolute), if `layout.toc` names it.
    pub fn package(&self, install: u32) -> Option<&Path> {
        self.installs.get(&install).map(|p| p.as_path())
    }

    /// The install chunks `layout.toc` names, with their package folders.
    pub fn packages(&self) -> &HashMap<u32, PathBuf> {
        &self.installs
    }

    /// The archive a base-game file location points into.
    pub fn archive_path(&self, loc: &FileLoc) -> PathBuf {
        let dir = self.installs.get(&loc.install).cloned().unwrap_or_else(|| self.data.join("Win32").join(PKG));
        dir.join(format!("cas_{:02}.cas", loc.archive))
    }

    /// The raw (still CAS-encoded) bytes of a base-game file (`gamedata.Game.raw`).
    pub fn raw(&self, loc: &FileLoc) -> Result<Vec<u8>, GameError> {
        self.handles.read(&self.archive_path(loc), loc.offset, loc.size)
    }

    /// The decoded bytes of a base-game file (`gamedata.Game.decode`).
    pub fn decode(&self, loc: &FileLoc) -> Result<Vec<u8>, GameError> {
        Ok(cas::decode_bytes(&self.raw(loc)?, &self.root)?)
    }

    /// A TOC of the game, by its path under `Data/Win32` (`items.toc`, `levels/game/.../x.toc`), parsed once and cached
    /// (`gamedata.Game.toc`).
    pub fn toc(&self, name: &str) -> Result<Arc<TocIndex>, GameError> {
        if let Some(t) = self.tocs.lock().unwrap_or_else(|e| e.into_inner()).get(name) {
            return Ok(t.clone());
        }
        let path = self.data.join("Win32").join(name);
        let raw = std::fs::read(&path).map_err(|e| GameError::Io(path.clone(), e))?;
        let index = Arc::new(TocIndex::new(toc::read(&raw)?));
        self.tocs.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string(), index.clone());
        Ok(index)
    }

    /// A bundle of a TOC (`gamedata.Game.bundle`): its region, file locations and manifest. The manifest is read raw,
    /// or decoded first when it is CAS-encoded; an inline manifest is used as it is.
    pub fn bundle(&self, name: &str, toc_name: &str) -> Result<Bundle, GameError> {
        let index = self.toc(toc_name)?;
        let record = index.bundle(name).ok_or_else(|| GameError::NoBundle(name.to_string()))?;
        let region = record.region.clone().ok_or_else(|| GameError::NoRegion(name.to_string()))?;
        let (inline, files) = bundle::read_region(&region)?;
        let manifest_raw = match inline {
            Some(m) => m,
            None => self.raw(files.first().ok_or(GameError::Bundle(BundleError::Truncated("no files")))?)?,
        };
        let manifest = read_manifest_any(&manifest_raw, &self.root)?;
        Ok(Bundle { name: name.to_string(), region, files, manifest_raw, manifest })
    }

    /// The first bundle of a TOC whose name ends with `suffix` (`gamedata.Game.find_bundle`).
    pub fn find_bundle(&self, suffix: &str, toc_name: &str) -> Result<Option<String>, GameError> {
        Ok(self.toc(toc_name)?.find_bundle(suffix).map(str::to_string))
    }

    /// A TOC chunk record by GUID.
    pub fn chunk(&self, guid: &Guid, toc_name: &str) -> Result<Option<TocChunk>, GameError> {
        Ok(self.toc(toc_name)?.chunk(guid).copied())
    }

    /// The decoded bytes of a TOC chunk (a texture's pixels).
    pub fn chunk_data(&self, guid: &Guid, toc_name: &str) -> Result<Vec<u8>, GameError> {
        let c = self.chunk(guid, toc_name)?.ok_or_else(|| GameError::Archive(format!("no chunk {guid} in {toc_name}")))?;
        self.decode(&chunk_loc(&c))
    }
}

/// A TOC chunk record's location.
pub fn chunk_loc(c: &TocChunk) -> FileLoc {
    FileLoc { patch: c.patch, install: c.install, archive: c.archive, offset: c.offset, size: c.size }
}

/// A manifest read raw, else decoded from CAS blocks first (`fb.Layer.bundle`'s fallback).
pub fn read_manifest_any(raw: &[u8], game_root: &Path) -> Result<Manifest, GameError> {
    match bundle::read_manifest(raw) {
        Ok(m) => Ok(m),
        Err(first) => match cas::decode_bytes(raw, game_root) {
            Ok(decoded) => Ok(bundle::read_manifest(&decoded)?),
            Err(_) => Err(first.into()),
        },
    }
}

/// `layout.toc`'s install chunks: persistentIndex (as u32) -> `<Data>/<installBundle>`.
fn install_packages(layout: &[u8], data: &Path) -> Result<HashMap<u32, PathBuf>, GameError> {
    let mut out = HashMap::new();
    if layout.len() <= ENVELOPE {
        return Ok(out);
    }
    let (root, _) = ndb::read(layout, ENVELOPE)?;
    let chunks = root.field("installManifest").and_then(|m| m.field("installChunks"));
    for c in chunks.map(|c| c.children.as_slice()).unwrap_or(&[]) {
        let index = c.field("persistentIndex").and_then(|n| n.as_i32());
        let folder = c.field("installBundle").map(|n| n.text());
        if let (Some(i), Some(f)) = (index, folder) {
            let mut p = data.to_path_buf();
            for part in f.split('/') {
                p.push(part);
            }
            out.insert(i as u32, p);
        }
    }
    Ok(out)
}

/// A mod folder (or the merged `.reskate` layer) over the game (`fb.Layer`, `caveman.ModLayer`): its own files (patch
/// flag set) are in its `Win32/.../cas_NN.cas` archives, the rest in the game's.
#[derive(Debug)]
pub struct Layer {
    root: PathBuf,
    game: Arc<Game>,
    /// archive number -> [(folder relative to Win32, lower case with '/'), path]
    cas: HashMap<u16, Vec<(String, PathBuf)>>,
    handles: Handles,
}

fn find_cas(dir: &Path, win32: &Path, out: &mut HashMap<u16, Vec<(String, PathBuf)>>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            find_cas(&p, win32, out);
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
        let lower = name.to_ascii_lowercase();
        if let Some(num) = lower.strip_prefix("cas_").and_then(|s| s.strip_suffix(".cas")) {
            if let Ok(idx) = num.parse::<u16>() {
                let rel = p
                    .parent()
                    .and_then(|d| d.strip_prefix(win32).ok())
                    .map(|r| r.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
                    .unwrap_or_default();
                out.entry(idx).or_default().push((rel, p));
            }
        }
    }
}

impl Layer {
    /// Opens a mod folder (its `Win32` archives are found recursively, as `fb.Layer`).
    pub fn open(root: &Path, game: Arc<Game>) -> Layer {
        let win32 = root.join("Win32");
        let mut cas = HashMap::new();
        find_cas(&win32, &win32, &mut cas);
        Layer { root: root.to_path_buf(), game, cas, handles: Handles::default() }
    }

    /// The mod folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The game under this layer.
    pub fn game(&self) -> &Arc<Game> {
        &self.game
    }

    /// The archive a patch file location points into: the only `cas_NN.cas` of that number, or, when several
    /// packages have one, the package of the location's install chunk (`caveman.ModLayer.read_raw`).
    pub fn archive_path(&self, loc: &FileLoc) -> Result<PathBuf, GameError> {
        let cands = self.cas.get(&loc.archive).map(|v| v.as_slice()).unwrap_or(&[]);
        match cands.len() {
            0 => Err(GameError::Archive(format!("archive {} missing in {}", loc.archive, self.root.display()))),
            1 => Ok(cands[0].1.clone()),
            _ => {
                let want = self
                    .game
                    .package(loc.install)
                    .and_then(|p| p.strip_prefix(self.game.data().join("Win32")).ok())
                    .map(|r| r.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
                    .unwrap_or_else(|| {
                        if loc.install == MOD_INSTALL_CHUNK {
                            "configurations/layout/initialinstallpackage".into()
                        } else {
                            "configurations/layout/defaultinstallpackage".into()
                        }
                    });
                cands
                    .iter()
                    .find(|(rel, _)| *rel == want)
                    .map(|c| c.1.clone())
                    .ok_or_else(|| GameError::Archive(format!("archive {} ambiguous in {}", loc.archive, self.root.display())))
            }
        }
    }

    /// The raw bytes of a file: the mod's own when the location's patch flag is set, else the game's.
    pub fn raw(&self, loc: &FileLoc) -> Result<Vec<u8>, GameError> {
        if !loc.patch {
            return self.game.raw(loc);
        }
        self.handles.read(&self.archive_path(loc)?, loc.offset, loc.size)
    }

    /// The decoded bytes of a file.
    pub fn decode(&self, loc: &FileLoc) -> Result<Vec<u8>, GameError> {
        Ok(cas::decode_bytes(&self.raw(loc)?, self.game.root())?)
    }

    /// One of the mod's TOCs, by its path under `Win32`.
    pub fn toc(&self, name: &str) -> Result<Toc, GameError> {
        let path = self.root.join("Win32").join(name);
        let raw = std::fs::read(&path).map_err(|e| GameError::Io(path.clone(), e))?;
        Ok(toc::read(&raw)?)
    }

    /// A bundle region's manifest and its other files (`fb.Layer.bundle`: the files after the manifest).
    pub fn bundle(&self, region: &[u8]) -> Result<(Manifest, Vec<FileLoc>), GameError> {
        let (inline, files) = bundle::read_region(region)?;
        if let Some(m) = inline {
            return Ok((bundle::read_manifest(&m)?, files));
        }
        let first = files.first().ok_or(GameError::Bundle(BundleError::Truncated("no files")))?;
        let raw = self.raw(first)?;
        Ok((read_manifest_any(&raw, self.game.root())?, files[1..].to_vec()))
    }
}
