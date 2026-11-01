//! The Windows APIs Caveman FX calls, declared by hand (as `caveman-core`'s oodle.rs and dxc.rs do), and the safe
//! memory reads everything else is built on: every read of game memory goes through `ReadProcessMemory` on our own
//! process, which fails cleanly on an unmapped or protected page instead of faulting.

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub type HANDLE = *mut c_void;
pub type BOOL = i32;
pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_RELEASE: u32 = 0x8000;
pub const MEM_FREE: u32 = 0x10000;
pub const FILE_MAP_ALL_ACCESS: u32 = 0xF001F;
pub const EVENT_MODIFY_STATE: u32 = 0x0002;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const MEM_PRIVATE: u32 = 0x20000;
pub const MEM_MAPPED: u32 = 0x40000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_WRITECOPY: u32 = 0x08;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const PAGE_GUARD: u32 = 0x100;
pub const PAGE_NOCACHE: u32 = 0x200;
pub const PAGE_WRITECOMBINE: u32 = 0x400;

pub const TH32CS_SNAPPROCESS: u32 = 0x2;
pub const TH32CS_SNAPTHREAD: u32 = 0x4;
pub const TH32CS_SNAPMODULE: u32 = 0x8;
pub const TH32CS_SNAPMODULE32: u32 = 0x10;
pub const THREAD_QUERY_INFORMATION: u32 = 0x40;
pub const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x800;
pub const PROCESS_CREATE_THREAD: u32 = 0x2;
pub const PROCESS_VM_OPERATION: u32 = 0x8;
pub const PROCESS_VM_READ: u32 = 0x10;
pub const PROCESS_VM_WRITE: u32 = 0x20;
pub const PROCESS_QUERY_INFORMATION: u32 = 0x400;
pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
pub const SYNCHRONIZE: u32 = 0x0010_0000;

pub const PIPE_ACCESS_DUPLEX: u32 = 0x3;
pub const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
pub const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
pub const PIPE_TYPE_BYTE: u32 = 0;
pub const PIPE_READMODE_BYTE: u32 = 0;
pub const PIPE_WAIT: u32 = 0;
pub const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8;
pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const OPEN_EXISTING: u32 = 3;

pub const ERROR_IO_PENDING: u32 = 997;
pub const ERROR_PIPE_CONNECTED: u32 = 535;
pub const ERROR_PIPE_BUSY: u32 = 231;
pub const ERROR_MORE_DATA: u32 = 234;
pub const WAIT_OBJECT_0: u32 = 0;
pub const WAIT_TIMEOUT: u32 = 258;
pub const INFINITE: u32 = 0xFFFF_FFFF;

pub const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x2;
pub const TIMER_ALL_ACCESS: u32 = 0x1F_0003;
pub const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MEMORY_BASIC_INFORMATION {
    pub BaseAddress: *mut c_void,
    pub AllocationBase: *mut c_void,
    pub AllocationProtect: u32,
    pub PartitionId: u16,
    pub RegionSize: usize,
    pub State: u32,
    pub Protect: u32,
    pub Type: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PSAPI_WORKING_SET_EX_INFORMATION {
    pub VirtualAddress: *mut c_void,
    /// Bit 0: the page is resident (valid) in the working set.
    pub VirtualAttributes: usize,
}

#[repr(C)]
pub struct THREADENTRY32 {
    pub dwSize: u32,
    pub cntUsage: u32,
    pub th32ThreadID: u32,
    pub th32OwnerProcessID: u32,
    pub tpBasePri: i32,
    pub tpDeltaPri: i32,
    pub dwFlags: u32,
}

#[repr(C)]
pub struct PROCESSENTRY32W {
    pub dwSize: u32,
    pub cntUsage: u32,
    pub th32ProcessID: u32,
    pub th32DefaultHeapID: usize,
    pub th32ModuleID: u32,
    pub cntThreads: u32,
    pub th32ParentProcessID: u32,
    pub pcPriClassBase: i32,
    pub dwFlags: u32,
    pub szExeFile: [u16; 260],
}

#[repr(C)]
pub struct MODULEENTRY32W {
    pub dwSize: u32,
    pub th32ModuleID: u32,
    pub th32ProcessID: u32,
    pub GlblcntUsage: u32,
    pub ProccntUsage: u32,
    pub modBaseAddr: *mut u8,
    pub modBaseSize: u32,
    pub hModule: HANDLE,
    pub szModule: [u16; 256],
    pub szExePath: [u16; 260],
}

#[repr(C)]
pub struct THREAD_BASIC_INFORMATION {
    pub ExitStatus: i32,
    pub TebBaseAddress: *mut c_void,
    pub UniqueProcess: HANDLE,
    pub UniqueThread: HANDLE,
    pub AffinityMask: usize,
    pub Priority: i32,
    pub BasePriority: i32,
}

#[repr(C)]
pub struct OVERLAPPED {
    pub Internal: usize,
    pub InternalHigh: usize,
    pub Offset: u32,
    pub OffsetHigh: u32,
    pub hEvent: HANDLE,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SYSTEMTIME {
    pub wYear: u16,
    pub wMonth: u16,
    pub wDayOfWeek: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
    pub wMilliseconds: u16,
}

/// LoadLibraryExW: map a file as an image, nothing run (offline checks of the exe).
pub const LOAD_LIBRARY_AS_IMAGE_RESOURCE: u32 = 0x20;

pub type ThreadProc = unsafe extern "system" fn(*mut c_void) -> u32;

#[link(name = "kernel32")]
extern "system" {
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetCurrentThread() -> HANDLE;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetLastError() -> u32;
    pub fn CloseHandle(h: HANDLE) -> BOOL;
    pub fn Sleep(ms: u32);
    pub fn ReadProcessMemory(process: HANDLE, base: *const c_void, buffer: *mut c_void, size: usize, read: *mut usize) -> BOOL;
    pub fn WriteProcessMemory(process: HANDLE, base: *mut c_void, buffer: *const c_void, size: usize, written: *mut usize) -> BOOL;
    pub fn VirtualQuery(address: *const c_void, info: *mut MEMORY_BASIC_INFORMATION, length: usize) -> usize;
    pub fn VirtualQueryEx(process: HANDLE, address: *const c_void, info: *mut MEMORY_BASIC_INFORMATION, length: usize) -> usize;
    pub fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> BOOL;
    pub fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    pub fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> BOOL;
    pub fn VirtualAllocEx(process: HANDLE, address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    pub fn VirtualFreeEx(process: HANDLE, address: *mut c_void, size: usize, kind: u32) -> BOOL;
    pub fn FlushInstructionCache(process: HANDLE, base: *const c_void, size: usize) -> BOOL;
    pub fn CreateFileMappingW(file: HANDLE, attrs: *mut c_void, protect: u32, size_hi: u32, size_lo: u32, name: *const u16) -> HANDLE;
    pub fn OpenFileMappingW(access: u32, inherit: BOOL, name: *const u16) -> HANDLE;
    pub fn MapViewOfFile(mapping: HANDLE, access: u32, offset_hi: u32, offset_lo: u32, bytes: usize) -> *mut c_void;
    pub fn UnmapViewOfFile(view: *const c_void) -> BOOL;
    pub fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> BOOL>, add: BOOL) -> BOOL;
    pub fn OpenEventW(access: u32, inherit: BOOL, name: *const u16) -> HANDLE;
    pub fn SetEvent(event: HANDLE) -> BOOL;
    pub fn K32QueryWorkingSetEx(process: HANDLE, info: *mut c_void, size: u32) -> BOOL;
    pub fn CreateThread(attrs: *mut c_void, stack: usize, start: ThreadProc, param: *mut c_void, flags: u32, id: *mut u32) -> HANDLE;
    pub fn CreateRemoteThread(process: HANDLE, attrs: *mut c_void, stack: usize, start: *mut c_void, param: *mut c_void, flags: u32, id: *mut u32) -> HANDLE;
    pub fn GetExitCodeThread(thread: HANDLE, code: *mut u32) -> BOOL;
    pub fn SetThreadPriority(thread: HANDLE, priority: i32) -> BOOL;
    pub fn WaitForSingleObject(h: HANDLE, ms: u32) -> u32;
    pub fn DisableThreadLibraryCalls(module: HANDLE) -> BOOL;
    pub fn FreeLibraryAndExitThread(module: HANDLE, code: u32) -> !;
    pub fn GetModuleHandleW(name: *const u16) -> HANDLE;
    pub fn GetModuleFileNameW(module: HANDLE, name: *mut u16, size: u32) -> u32;
    pub fn GetProcAddress(module: HANDLE, name: *const u8) -> *mut c_void;
    pub fn LoadLibraryW(name: *const u16) -> HANDLE;
    pub fn LoadLibraryExW(name: *const u16, file: HANDLE, flags: u32) -> HANDLE;
    pub fn GetLocalTime(time: *mut SYSTEMTIME);
    pub fn FreeLibrary(module: HANDLE) -> BOOL;
    pub fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> HANDLE;
    pub fn Thread32First(snapshot: HANDLE, entry: *mut THREADENTRY32) -> BOOL;
    pub fn Thread32Next(snapshot: HANDLE, entry: *mut THREADENTRY32) -> BOOL;
    pub fn Process32FirstW(snapshot: HANDLE, entry: *mut PROCESSENTRY32W) -> BOOL;
    pub fn Process32NextW(snapshot: HANDLE, entry: *mut PROCESSENTRY32W) -> BOOL;
    pub fn Module32FirstW(snapshot: HANDLE, entry: *mut MODULEENTRY32W) -> BOOL;
    pub fn Module32NextW(snapshot: HANDLE, entry: *mut MODULEENTRY32W) -> BOOL;
    pub fn OpenThread(access: u32, inherit: BOOL, id: u32) -> HANDLE;
    pub fn OpenProcess(access: u32, inherit: BOOL, id: u32) -> HANDLE;
    pub fn QueryFullProcessImageNameW(process: HANDLE, flags: u32, name: *mut u16, size: *mut u32) -> BOOL;
    pub fn CreateNamedPipeW(name: *const u16, open: u32, mode: u32, instances: u32, out: u32, inp: u32, timeout: u32, attrs: *mut c_void) -> HANDLE;
    pub fn ConnectNamedPipe(pipe: HANDLE, overlapped: *mut OVERLAPPED) -> BOOL;
    pub fn DisconnectNamedPipe(pipe: HANDLE) -> BOOL;
    pub fn WaitNamedPipeW(name: *const u16, ms: u32) -> BOOL;
    pub fn CreateFileW(name: *const u16, access: u32, share: u32, attrs: *mut c_void, disposition: u32, flags: u32, template: HANDLE) -> HANDLE;
    pub fn ReadFile(file: HANDLE, buffer: *mut c_void, size: u32, read: *mut u32, overlapped: *mut OVERLAPPED) -> BOOL;
    pub fn WriteFile(file: HANDLE, buffer: *const c_void, size: u32, written: *mut u32, overlapped: *mut OVERLAPPED) -> BOOL;
    pub fn FlushFileBuffers(file: HANDLE) -> BOOL;
    pub fn GetOverlappedResult(file: HANDLE, overlapped: *mut OVERLAPPED, transferred: *mut u32, wait: BOOL) -> BOOL;
    pub fn CancelIo(file: HANDLE) -> BOOL;
    pub fn CreateEventW(attrs: *mut c_void, manual: BOOL, initial: BOOL, name: *const u16) -> HANDLE;
    pub fn ResetEvent(event: HANDLE) -> BOOL;
    pub fn CreateWaitableTimerExW(attrs: *mut c_void, name: *const u16, flags: u32, access: u32) -> HANDLE;
    pub fn SetWaitableTimer(timer: HANDLE, due: *const i64, period: i32, routine: *mut c_void, arg: *mut c_void, resume: BOOL) -> BOOL;
}

#[link(name = "ntdll")]
extern "system" {
    pub fn NtQueryInformationThread(thread: HANDLE, class: u32, info: *mut c_void, length: u32, returned: *mut u32) -> i32;
}

/// A NUL-terminated UTF-16 string.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A UTF-16 buffer up to its first NUL, as a String.
pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Closes a handle when dropped.
pub struct Handle(pub HANDLE);

impl Handle {
    pub fn valid(&self) -> bool {
        !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.valid() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

// ---------------------------------------------------------------- safe reads

/// The lowest and highest user-mode addresses a read may touch.
pub const LOWEST: usize = 0x10000;
pub const HIGHEST: usize = 0x7FFF_FFFF_FFFF;

static TARGET: AtomicUsize = AtomicUsize::new(0);
static TARGET_PID: AtomicU32 = AtomicU32::new(0);

/// Points every read (and the scanner's queries and the thread walk) at another process (the trainer's read-only
/// probe); the DLL never calls this and reads its own process.
pub fn set_target(process: HANDLE, pid: u32) {
    TARGET.store(process as usize, Ordering::Release);
    TARGET_PID.store(pid, Ordering::Release);
}

/// The process read: the target, or this one.
pub fn target() -> HANDLE {
    match TARGET.load(Ordering::Acquire) {
        0 => unsafe { GetCurrentProcess() },
        h => h as HANDLE,
    }
}

pub fn target_pid() -> u32 {
    match TARGET_PID.load(Ordering::Acquire) {
        0 => unsafe { GetCurrentProcessId() },
        p => p,
    }
}

/// VirtualQuery in the process read.
pub fn query(addr: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let n = unsafe { VirtualQueryEx(target(), addr as *const c_void, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
    (n != 0).then_some(mbi)
}

/// Copies `buf.len()` bytes from `addr` in the process read (this one, in the DLL), or returns false (unmapped,
/// protected, out of range). Never faults: the kernel does the copy.
pub fn read_into(addr: usize, buf: &mut [u8]) -> bool {
    if buf.is_empty() {
        return true;
    }
    if addr < LOWEST || addr.checked_add(buf.len()).is_none_or(|end| end > HIGHEST) {
        return false;
    }
    let mut got = 0usize;
    let ok = unsafe { ReadProcessMemory(target(), addr as *const c_void, buf.as_mut_ptr().cast(), buf.len(), &mut got) };
    ok != 0 && got == buf.len()
}

pub fn read_u8(addr: usize) -> Option<u8> {
    let mut b = [0u8; 1];
    read_into(addr, &mut b).then_some(b[0])
}

pub fn read_u16(addr: usize) -> Option<u16> {
    let mut b = [0u8; 2];
    read_into(addr, &mut b).then(|| u16::from_le_bytes(b))
}

pub fn read_u32(addr: usize) -> Option<u32> {
    let mut b = [0u8; 4];
    read_into(addr, &mut b).then(|| u32::from_le_bytes(b))
}

pub fn read_u64(addr: usize) -> Option<u64> {
    let mut b = [0u8; 8];
    read_into(addr, &mut b).then(|| u64::from_le_bytes(b))
}

pub fn read_ptr(addr: usize) -> Option<usize> {
    read_u64(addr).map(|v| v as usize)
}

/// Copies `bytes` to `addr` in this process through the kernel (as [`read_into`] reads): false when the range is
/// unmapped, read-only or out of range, never a fault. For data the game keeps writable (its heaps), not code.
pub fn write_into(addr: usize, bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    if addr < LOWEST || addr.checked_add(bytes.len()).is_none_or(|end| end > HIGHEST) {
        return false;
    }
    let mut put = 0usize;
    let ok = unsafe { WriteProcessMemory(target(), addr as *mut c_void, bytes.as_ptr().cast(), bytes.len(), &mut put) };
    ok != 0 && put == bytes.len()
}

/// A NUL-terminated byte string of at most `max` bytes at `addr` (printable ASCII only), or None. A string near the
/// end of a mapped page is read up to the page end.
pub fn read_cstr(addr: usize, max: usize) -> Option<String> {
    if addr < LOWEST {
        return None;
    }
    let mut buf = vec![0u8; max];
    if !read_into(addr, &mut buf) {
        let to_page_end = 0x1000 - (addr & 0xFFF);
        if to_page_end >= max {
            return None;
        }
        buf.truncate(to_page_end);
        if !read_into(addr, &mut buf) {
            return None;
        }
    }
    let n = buf.iter().position(|&c| c == 0)?;
    if n == 0 || !buf[..n].iter().all(|&c| (32..127).contains(&c)) {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// The full path of a module of this process (None: the exe).
#[allow(clippy::not_unsafe_ptr_arg_deref, reason = "an opaque module handle, checked by the call itself, not a pointer this reads")]
pub fn module_path(module: HANDLE) -> Option<String> {
    let mut buf = vec![0u16; 32768];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| from_wide(&buf[..n]))
}

/// The address range of a module in this process (from its PE header in memory).
pub fn module_range(module: HANDLE) -> Option<(usize, usize)> {
    let base = module as usize;
    let pe = read_u32(base + 0x3C)? as usize;
    if read_u32(base + pe)? != 0x0000_4550 {
        return None;
    }
    let size = read_u32(base + pe + 24 + 56)? as usize;
    Some((base, base + size))
}

/// Milliseconds on a monotonic clock.
pub fn now_ms() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}
