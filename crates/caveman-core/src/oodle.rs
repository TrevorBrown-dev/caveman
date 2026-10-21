//! Oodle Kraken compress/decompress through the game's oo2core_9_win64.dll, loaded at run time (fb.py / fbw.py helpers).
//!
//! The DLL is the game's own (`<game root>/oo2core_*_win64.dll`, the last one by name, as `fb.oodle` picks it); it is
//! loaded once per process, like the Python toolkit's module-level `_oodle`, and never written or copied.

use std::ffi::c_void;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Oodle's compressor id for Kraken (`fbw.encode_cas` passes 8).
pub const KRAKEN_COMPRESSOR: i32 = 8;
/// The compression level `fbw.encode_cas` uses by default.
pub const DEFAULT_LEVEL: i32 = 4;
/// Extra output space `fb.decode_cas` gives the decompressor beyond the decoded size.
const DECODE_SLACK: usize = 0x40000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OodleError {
    /// No `oo2core_*_win64.dll` in the game folder.
    NotFound(PathBuf),
    /// LoadLibraryW failed (Windows error code).
    Load(PathBuf, u32),
    /// An export is missing from the DLL.
    MissingExport(&'static str),
    /// Decompression returned a different length than the block header promised.
    Decompress { expected: usize, got: i64 },
    /// Compression failed (Oodle returned 0 or less).
    Compress(i64),
    /// A buffer too large for Oodle's signed sizes.
    TooLarge(usize),
}

impl fmt::Display for OodleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OodleError::NotFound(p) => write!(f, "no oo2core_*_win64.dll in {}", p.display()),
            OodleError::Load(p, e) => write!(f, "could not load {} (Windows error {e})", p.display()),
            OodleError::MissingExport(name) => write!(f, "oo2core has no export {name}"),
            OodleError::Decompress { expected, got } => {
                write!(f, "oodle block failed: decoded {got} bytes, expected {expected}")
            }
            OodleError::Compress(n) => write!(f, "oodle compression failed ({n})"),
            OodleError::TooLarge(n) => write!(f, "buffer of {n} bytes is too large for Oodle"),
        }
    }
}

impl std::error::Error for OodleError {}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
}

/// `OodleLZ_Decompress(compBuf, compBufSize, rawBuf, rawLen, fuzzSafe, checkCRC, verbosity, decBufBase, decBufSize,
/// fpCallback, callbackUserData, decoderMemory, decoderMemorySize, threadPhase) -> SINTa`
type DecompressFn = unsafe extern "C" fn(
    *const c_void,
    i64,
    *mut c_void,
    i64,
    i32,
    i32,
    i32,
    *mut c_void,
    i64,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    i64,
    i32,
) -> i64;
/// `OodleLZ_Compress(compressor, rawBuf, rawLen, compBuf, level, pOptions, dictionaryBase, lrm, scratchMem,
/// scratchSize) -> SINTa`
type CompressFn = unsafe extern "C" fn(
    i32,
    *const c_void,
    i64,
    *mut c_void,
    i32,
    *const c_void,
    *const c_void,
    *const c_void,
    *mut c_void,
    i64,
) -> i64;
/// `OodleLZ_GetCompressedBufferSizeNeeded(compressor, rawSize) -> SINTa`
type SizeNeededFn = unsafe extern "C" fn(i32, i64) -> i64;

/// The game's Oodle library (`fb.oodle` / `fbw._oodle_lib`).
pub struct Oodle {
    path: PathBuf,
    decompress: DecompressFn,
    compress: CompressFn,
    size_needed: SizeNeededFn,
}

// The DLL stays loaded for the life of the process (never freed), and Oodle's LZ entry points are thread-safe.
unsafe impl Send for Oodle {}
unsafe impl Sync for Oodle {}

impl fmt::Debug for Oodle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Oodle").field("path", &self.path).finish()
    }
}

static LOADED: Mutex<Option<Arc<Oodle>>> = Mutex::new(None);

/// The game's `oo2core_*_win64.dll`: the last match by file name, as `sorted(glob(...))[-1]`.
pub fn find_dll(game_root: &Path) -> Result<PathBuf, OodleError> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(game_root)
        .map_err(|_| OodleError::NotFound(game_root.to_path_buf()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("oo2core_") && n.ends_with("_win64.dll"))
                .unwrap_or(false)
        })
        .collect();
    found.sort();
    found.pop().ok_or_else(|| OodleError::NotFound(game_root.to_path_buf()))
}

/// The process-wide Oodle, loaded from `game_root` the first time (later calls reuse it, like the Python toolkit's
/// module-level cache).
pub fn get(game_root: &Path) -> Result<Arc<Oodle>, OodleError> {
    let mut slot = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(o) = slot.as_ref() {
        return Ok(o.clone());
    }
    let o = Arc::new(Oodle::load(&find_dll(game_root)?)?);
    *slot = Some(o.clone());
    Ok(o)
}

impl Oodle {
    /// Loads an Oodle DLL and resolves the three LZ entry points.
    pub fn load(dll: &Path) -> Result<Oodle, OodleError> {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = dll.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is NUL-terminated; the module handle is kept for the life of the process.
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        if module.is_null() {
            return Err(OodleError::Load(dll.to_path_buf(), unsafe { GetLastError() }));
        }
        let export = |name: &'static [u8]| -> Result<*mut c_void, OodleError> {
            // SAFETY: `name` is NUL-terminated; `module` is a loaded module.
            let p = unsafe { GetProcAddress(module, name.as_ptr()) };
            if p.is_null() {
                let shown = std::str::from_utf8(&name[..name.len() - 1]).unwrap_or("?");
                Err(OodleError::MissingExport(match shown {
                    "OodleLZ_Decompress" => "OodleLZ_Decompress",
                    "OodleLZ_Compress" => "OodleLZ_Compress",
                    _ => "OodleLZ_GetCompressedBufferSizeNeeded",
                }))
            } else {
                Ok(p)
            }
        };
        let d = export(b"OodleLZ_Decompress\0")?;
        let c = export(b"OodleLZ_Compress\0")?;
        let s = export(b"OodleLZ_GetCompressedBufferSizeNeeded\0")?;
        // SAFETY: the exports have these C signatures (Oodle 2.9's public API, as the Python ctypes declarations).
        unsafe {
            Ok(Oodle {
                path: dll.to_path_buf(),
                decompress: std::mem::transmute::<*mut c_void, DecompressFn>(d),
                compress: std::mem::transmute::<*mut c_void, CompressFn>(c),
                size_needed: std::mem::transmute::<*mut c_void, SizeNeededFn>(s),
            })
        }
    }

    /// The DLL this was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decompresses one block to exactly `decoded` bytes (`fb.decode_cas`'s Oodle branch: fuzz-safe, no CRC check,
    /// thread phase 3, an output buffer 0x40000 bytes larger than the decoded size).
    pub fn decompress(&self, src: &[u8], decoded: usize) -> Result<Vec<u8>, OodleError> {
        let src_len = i64::try_from(src.len()).map_err(|_| OodleError::TooLarge(src.len()))?;
        let raw_len = i64::try_from(decoded).map_err(|_| OodleError::TooLarge(decoded))?;
        let mut out = vec![0u8; decoded + DECODE_SLACK];
        // SAFETY: `out` holds at least `decoded` bytes; Oodle reads `src_len` bytes from `src`.
        let n = unsafe {
            (self.decompress)(
                src.as_ptr() as *const c_void,
                src_len,
                out.as_mut_ptr() as *mut c_void,
                raw_len,
                1,
                0,
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                3,
            )
        };
        if n != raw_len {
            return Err(OodleError::Decompress { expected: decoded, got: n });
        }
        out.truncate(decoded);
        Ok(out)
    }

    /// `OodleLZ_GetCompressedBufferSizeNeeded`.
    pub fn compressed_buffer_size(&self, compressor: i32, raw_size: usize) -> Result<usize, OodleError> {
        let raw = i64::try_from(raw_size).map_err(|_| OodleError::TooLarge(raw_size))?;
        // SAFETY: a pure size computation.
        let n = unsafe { (self.size_needed)(compressor, raw) };
        usize::try_from(n).map_err(|_| OodleError::Compress(n))
    }

    /// Compresses `src` (`OodleLZ_Compress` with no options, dictionary or scratch). Returns the compressed bytes;
    /// an empty input gives an empty output (as `fbw.encode_cas`, which skips the call for empty blocks).
    pub fn compress(&self, compressor: i32, src: &[u8], level: i32) -> Result<Vec<u8>, OodleError> {
        if src.is_empty() {
            return Ok(Vec::new());
        }
        let src_len = i64::try_from(src.len()).map_err(|_| OodleError::TooLarge(src.len()))?;
        let cap = self.compressed_buffer_size(compressor, src.len())?;
        let mut out = vec![0u8; cap];
        // SAFETY: `out` holds `cap` bytes, the size Oodle says it needs for `src_len` input bytes.
        let n = unsafe {
            (self.compress)(
                compressor,
                src.as_ptr() as *const c_void,
                src_len,
                out.as_mut_ptr() as *mut c_void,
                level,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        if n <= 0 || n as usize > cap {
            return Err(OodleError::Compress(n));
        }
        out.truncate(n as usize);
        Ok(out)
    }
}
