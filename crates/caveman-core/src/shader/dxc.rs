//! DXC through the game's own `dxcompiler.dll` and `dxil.dll` by raw COM vtable calls (`dxc.py`): disassemble,
//! assemble LLVM IR text into a container, validate and sign, compile HLSL. The DLLs are loaded at run time from the game
//! folder (read-only), `dxil.dll` first, so the compiler's own validation finds the game's validator.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::null_mut;

use super::ShaderError;

type Hresult = i32;
type Unknown = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// The `DxcCreateInstance` export of both DLLs.
type CreateInstance = unsafe extern "system" fn(*const Iid, *const Iid, *mut Unknown) -> Hresult;

/// A COM CLSID or IID, in memory (little-endian) order.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Iid([u8; 16]);

impl Iid {
    /// From text such as `73e22d93-e6ce-47f3-b5bf-f0664f39c1b0` (`uuid.UUID(...).bytes_le`).
    const fn parse(s: &str) -> Iid {
        let b = s.as_bytes();
        let mut hex = [0u8; 16];
        let mut n = 0;
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'-' {
                let hi = nibble(b[i]);
                let lo = nibble(b[i + 1]);
                hex[n] = hi << 4 | lo;
                n += 1;
                i += 1;
            }
            i += 1;
        }
        // data1 (4 bytes), data2 and data3 (2 each) are little-endian; data4 (8) as written
        Iid([hex[3], hex[2], hex[1], hex[0], hex[5], hex[4], hex[7], hex[6], hex[8], hex[9], hex[10], hex[11], hex[12], hex[13], hex[14], hex[15]])
    }
}

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

const CLSID_DXC_COMPILER: Iid = Iid::parse("73e22d93-e6ce-47f3-b5bf-f0664f39c1b0");
const CLSID_DXC_OPTIMIZER: Iid = Iid::parse("ae2cd79f-cc22-453f-9b6b-b124e7a5204c");
const IID_IDXC_OPTIMIZER: Iid = Iid::parse("25740e2e-9cba-401b-9119-4fb42f39f270");
const CLSID_DXC_ASSEMBLER: Iid = Iid::parse("d728db68-f903-4f80-94cd-dccf76ec7151");
const IID_IDXC_ASSEMBLER: Iid = Iid::parse("091f7a26-1c1f-4948-904b-e6e3a8a771d5");
const CLSID_DXC_VALIDATOR: Iid = Iid::parse("8ca3e215-f728-4cf3-8cdd-88af917587a1");
const IID_IDXC_VALIDATOR: Iid = Iid::parse("a6e82bd2-1fd7-4826-9811-2857e797f49a");
const CLSID_DXC_UTILS: Iid = Iid::parse("6245d6af-66e0-48fd-80b4-4d271796748c");
const IID_IDXC_COMPILER3: Iid = Iid::parse("228b4687-5a6a-4730-900c-9702b2203f54");
const IID_IDXC_UTILS: Iid = Iid::parse("4605c4cb-2019-492a-ada4-65f20bb7d67f");
const IID_IDXC_RESULT: Iid = Iid::parse("58346cda-dde7-4497-9461-6f87af5e0659");
const IID_IDXC_BLOB_UTF8: Iid = Iid::parse("3da636c9-ba71-4024-a301-30cbf125305b");
/// `DXC_OUT_ERRORS` (`IDxcResult::GetOutput`).
const DXC_OUT_ERRORS: u32 = 2;
/// `DxcValidatorFlags_InPlaceEdit`: the validator writes its hash into the container it was given (signing).
const VALIDATE_IN_PLACE: u32 = 1;
/// The UTF-8 code page (`DxcBuffer.Encoding` for HLSL source).
const CP_UTF8: u32 = 65001;

/// `DxcBuffer`.
#[repr(C)]
struct DxcBuffer {
    ptr: *const c_void,
    size: usize,
    encoding: u32,
}

/// A COM interface pointer, released on drop.
struct Com(Unknown);

impl Com {
    fn null() -> Com {
        Com(null_mut())
    }

    fn is_null(&self) -> bool {
        self.0.is_null()
    }

    /// The function at `index` of the object's vtable.
    unsafe fn method(&self, index: usize) -> *const c_void {
        let vtable = *(self.0 as *const *const *const c_void);
        *vtable.add(index)
    }

    /// `IDxcBlob::GetBufferPointer` / `GetBufferSize`: a copy of the blob's bytes (`dxc._blob_bytes`).
    unsafe fn blob_bytes(&self) -> Vec<u8> {
        let pointer: unsafe extern "system" fn(Unknown) -> *const u8 = std::mem::transmute(self.method(3));
        let size: unsafe extern "system" fn(Unknown) -> usize = std::mem::transmute(self.method(4));
        let n = size(self.0);
        if n == 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(pointer(self.0), n).to_vec()
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let release: unsafe extern "system" fn(Unknown) -> u32 = std::mem::transmute(self.method(2));
                release(self.0);
            }
        }
    }
}

fn check(hr: Hresult, what: &str) -> Result<(), ShaderError> {
    if hr < 0 {
        Err(ShaderError::Dxc(format!("{what} failed: HRESULT {:#010x}", hr as u32)))
    } else {
        Ok(())
    }
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn load(path: &Path) -> Result<CreateInstance, ShaderError> {
    let module = unsafe { LoadLibraryW(wide(path).as_ptr()) };
    if module.is_null() {
        return Err(ShaderError::Dxc(format!("could not load {}: {}", path.display(), std::io::Error::last_os_error())));
    }
    let f = unsafe { GetProcAddress(module, c"DxcCreateInstance".as_ptr().cast()) };
    if f.is_null() {
        return Err(ShaderError::Dxc(format!("{} has no DxcCreateInstance", path.display())));
    }
    Ok(unsafe { std::mem::transmute::<*mut c_void, CreateInstance>(f) })
}

fn create(f: CreateInstance, clsid: &Iid, iid: &Iid, what: &str) -> Result<Com, ShaderError> {
    let mut out = Com::null();
    check(unsafe { f(clsid, iid, &mut out.0) }, what)?;
    Ok(out)
}

/// What an operation gave back: its output bytes, the messages (errors and warnings) and whether it succeeded.
#[derive(Debug, Clone)]
pub struct DxcOutput {
    pub bytes: Vec<u8>,
    pub messages: String,
    pub ok: bool,
}

/// The game's DXC (`dxc.Dxc`). The DLLs stay loaded for the life of the process, as in Python.
pub struct Dxc {
    compiler: Com,
    utils: Com,
    optimizer: Com,
    assembler: Com,
    validator: Com,
}

impl Dxc {
    /// Loads `dxil.dll` and then `dxcompiler.dll` from the game folder and creates the compiler, utils, optimizer,
    /// assembler and validator objects (`dxc.Dxc.__init__`).
    pub fn open(game_root: &Path) -> Result<Dxc, ShaderError> {
        let dxil = load(&game_root.join("dxil.dll"))?;
        let lib = load(&game_root.join("dxcompiler.dll"))?;
        Ok(Dxc {
            compiler: create(lib, &CLSID_DXC_COMPILER, &IID_IDXC_COMPILER3, "DxcCreateInstance(compiler)")?,
            utils: create(lib, &CLSID_DXC_UTILS, &IID_IDXC_UTILS, "DxcCreateInstance(utils)")?,
            optimizer: create(lib, &CLSID_DXC_OPTIMIZER, &IID_IDXC_OPTIMIZER, "DxcCreateInstance(optimizer)")?,
            assembler: create(lib, &CLSID_DXC_ASSEMBLER, &IID_IDXC_ASSEMBLER, "DxcCreateInstance(assembler)")?,
            validator: create(dxil, &CLSID_DXC_VALIDATOR, &IID_IDXC_VALIDATOR, "dxil DxcCreateInstance(validator)")?,
        })
    }

    /// An `IDxcBlobEncoding` holding a copy of `data` (`IDxcUtils::CreateBlob`, code page 0; `dxc.Dxc._blob`).
    fn blob(&self, data: &[u8]) -> Result<Com, ShaderError> {
        let len = u32::try_from(data.len()).map_err(|_| ShaderError::Dxc("blob over 4 GiB".into()))?;
        let mut out = Com::null();
        unsafe {
            let f: unsafe extern "system" fn(Unknown, *const c_void, u32, u32, *mut Unknown) -> Hresult = std::mem::transmute(self.utils.method(6));
            check(f(self.utils.0, data.as_ptr().cast(), len, 0, &mut out.0), "CreateBlob")?;
        }
        Ok(out)
    }

    /// The output bytes, error text and status of an `IDxcOperationResult` (`dxc.Dxc._operation`).
    fn operation(result: &Com) -> Result<(Vec<u8>, String, Hresult), ShaderError> {
        unsafe {
            let mut status: Hresult = 0;
            let get_status: unsafe extern "system" fn(Unknown, *mut Hresult) -> Hresult = std::mem::transmute(result.method(3));
            check(get_status(result.0, &mut status), "GetStatus")?;
            let get_result: unsafe extern "system" fn(Unknown, *mut Unknown) -> Hresult = std::mem::transmute(result.method(4));
            let mut blob = Com::null();
            let out = if get_result(result.0, &mut blob.0) >= 0 && !blob.is_null() { blob.blob_bytes() } else { Vec::new() };
            let get_errors: unsafe extern "system" fn(Unknown, *mut Unknown) -> Hresult = std::mem::transmute(result.method(5));
            let mut err = Com::null();
            let errors = if get_errors(result.0, &mut err.0) >= 0 && !err.is_null() { err.blob_bytes() } else { Vec::new() };
            Ok((out, text(&errors), status))
        }
    }

    /// The object bytes, error text and status of an `IDxcResult` (`dxc.Dxc._result`): the errors through
    /// `GetOutput(DXC_OUT_ERRORS)` as UTF-8, the object through `GetResult` when the status is a success.
    fn result(result: &Com) -> Result<(Vec<u8>, String, Hresult), ShaderError> {
        unsafe {
            let mut status: Hresult = 0;
            let get_status: unsafe extern "system" fn(Unknown, *mut Hresult) -> Hresult = std::mem::transmute(result.method(3));
            check(get_status(result.0, &mut status), "GetStatus")?;
            let get_output: unsafe extern "system" fn(Unknown, u32, *const Iid, *mut Unknown, *mut Unknown) -> Hresult = std::mem::transmute(result.method(7));
            let mut err = Com::null();
            let mut name = Com::null();
            let errors = if get_output(result.0, DXC_OUT_ERRORS, &IID_IDXC_BLOB_UTF8, &mut err.0, &mut name.0) >= 0 && !err.is_null() {
                err.blob_bytes()
            } else {
                Vec::new()
            };
            let mut obj = Vec::new();
            if status >= 0 {
                let get_result: unsafe extern "system" fn(Unknown, *mut Unknown) -> Hresult = std::mem::transmute(result.method(4));
                let mut out = Com::null();
                check(get_result(result.0, &mut out.0), "GetResult")?;
                if !out.is_null() {
                    obj = out.blob_bytes();
                }
            }
            Ok((obj, text(&errors), status))
        }
    }

    /// A DXBC container's DXIL as LLVM assembly text (`IDxcCompiler3::Disassemble`; `dxc.Dxc.disassemble`).
    pub fn disassemble(&self, dxbc: &[u8]) -> Result<String, ShaderError> {
        let buffer = DxcBuffer { ptr: dxbc.as_ptr().cast(), size: dxbc.len(), encoding: 0 };
        let mut result = Com::null();
        unsafe {
            let f: unsafe extern "system" fn(Unknown, *const DxcBuffer, *const Iid, *mut Unknown) -> Hresult = std::mem::transmute(self.compiler.method(4));
            check(f(self.compiler.0, &buffer, &IID_IDXC_RESULT, &mut result.0), "Disassemble")?;
        }
        let (bytes, errors, status) = Dxc::result(&result)?;
        if status < 0 {
            return Err(ShaderError::Dxc(format!("disassembly failed: {errors}")));
        }
        Ok(text(&bytes))
    }

    /// DXIL LLVM assembly text (as the disassembler prints it) to a DXBC container (`dxc.Dxc.assemble`): parsed by
    /// `IDxcOptimizer::RunOptimizer` with no passes, packed by `IDxcAssembler::AssembleToContainer`, then (with
    /// `sign`) validated and signed in place by the game's `dxil.dll`.
    pub fn assemble(&self, ir_text: &str, sign: bool) -> Result<DxcOutput, ShaderError> {
        let src = self.blob(ir_text.as_bytes())?;
        let mut module = Com::null();
        let mut log = Com::null();
        let hr = unsafe {
            let f: unsafe extern "system" fn(Unknown, Unknown, *const *const u16, u32, *mut Unknown, *mut Unknown) -> Hresult =
                std::mem::transmute(self.optimizer.method(5));
            f(self.optimizer.0, src.0, std::ptr::null(), 0, &mut module.0, &mut log.0)
        };
        drop(src);
        let log = if log.is_null() { String::new() } else { unsafe { text(&log.blob_bytes()) }.trim().to_string() };
        if hr < 0 || module.is_null() {
            return Ok(DxcOutput { bytes: Vec::new(), messages: format!("IR parse failed: HRESULT {:#010x} {log}", hr as u32), ok: false });
        }
        let mut result = Com::null();
        unsafe {
            let f: unsafe extern "system" fn(Unknown, Unknown, *mut Unknown) -> Hresult = std::mem::transmute(self.assembler.method(3));
            check(f(self.assembler.0, module.0, &mut result.0), "AssembleToContainer")?;
        }
        drop(module);
        let (dxbc, errors, status) = Dxc::operation(&result)?;
        drop(result);
        if status < 0 || dxbc.is_empty() {
            return Ok(DxcOutput { bytes: Vec::new(), messages: errors, ok: false });
        }
        if sign {
            return self.validate(&dxbc, true);
        }
        Ok(DxcOutput { bytes: dxbc, messages: errors, ok: true })
    }

    /// Validates a container with the game's `dxil.dll`; with `sign` the validator writes its hash into it
    /// (`dxc.Dxc.validate`). Returns the (signed) container.
    pub fn validate(&self, dxbc: &[u8], sign: bool) -> Result<DxcOutput, ShaderError> {
        let blob = self.blob(dxbc)?;
        let mut result = Com::null();
        unsafe {
            let f: unsafe extern "system" fn(Unknown, Unknown, u32, *mut Unknown) -> Hresult = std::mem::transmute(self.validator.method(3));
            check(f(self.validator.0, blob.0, if sign { VALIDATE_IN_PLACE } else { 0 }, &mut result.0), "Validate")?;
        }
        let (_, errors, status) = Dxc::operation(&result)?;
        drop(result);
        let signed = unsafe { blob.blob_bytes() };
        Ok(DxcOutput { bytes: signed, messages: errors, ok: status >= 0 })
    }

    /// Compiles HLSL with dxc command-line arguments (`["-E", "main", "-T", "ps_6_0"]`; `dxc.Dxc.compile`).
    pub fn compile(&self, source: &str, args: &[&str]) -> Result<DxcOutput, ShaderError> {
        let buffer = DxcBuffer { ptr: source.as_ptr().cast(), size: source.len(), encoding: CP_UTF8 };
        let wide_args: Vec<Vec<u16>> = args.iter().map(|a| a.encode_utf16().chain(std::iter::once(0)).collect()).collect();
        let argv: Vec<*const u16> = wide_args.iter().map(|a| a.as_ptr()).collect();
        let argc = u32::try_from(argv.len()).map_err(|_| ShaderError::Dxc("too many arguments".into()))?;
        let mut result = Com::null();
        unsafe {
            type Compile = unsafe extern "system" fn(Unknown, *const DxcBuffer, *const *const u16, u32, Unknown, *const Iid, *mut Unknown) -> Hresult;
            let f: Compile = std::mem::transmute(self.compiler.method(3));
            check(f(self.compiler.0, &buffer, argv.as_ptr(), argc, null_mut(), &IID_IDXC_RESULT, &mut result.0), "Compile")?;
        }
        let (bytes, messages, status) = Dxc::result(&result)?;
        Ok(DxcOutput { bytes, messages, ok: status >= 0 })
    }
}

/// UTF-8 with replacement and trailing NULs removed (`.decode('utf-8', 'replace').rstrip('\0')`).
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string()
}

/// The parts of a DXBC container: (four-character code, offset of the part's data, its length)
/// (`dxc.container_parts`).
pub fn container_parts(dxbc: &[u8]) -> Result<Vec<(String, usize, usize)>, ShaderError> {
    if dxbc.get(..4) != Some(b"DXBC") {
        return Err(ShaderError::Patch("not a DXBC container".into()));
    }
    let u32_at = |o: usize| -> Result<u32, ShaderError> {
        dxbc.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| ShaderError::Patch("truncated DXBC container".into()))
    };
    let count = u32_at(28)? as usize;
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let off = u32_at(32 + 4 * k)? as usize;
        let fourcc = dxbc.get(off..off + 4).ok_or_else(|| ShaderError::Patch("truncated DXBC part".into()))?;
        out.push((fourcc.iter().map(|&c| c as char).collect(), off + 8, u32_at(off + 4)? as usize));
    }
    Ok(out)
}
