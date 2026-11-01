//! What Caveman FX knows about one build of Skate.exe (20260929, ReSkate 2.1.1), and the checks that it is that
//! build: the file's SHA-256, then the code bytes at every address used, in the running image. Any mismatch keeps the
//! DLL dormant (knowledge/state-effects-runtime.md has the code these come from).

use crate::win;

/// ReSkate's supported Skate.exe (Steam build 25414733).
pub const EXE_SHA256: &str = "fbce74d5e28ef525dbba2cb4adbebc13405bdbd88f31bc940bca45e4ae88b8f9";
pub const BUILD: &str = "20260929";

/// RVAs into Skate.exe of this build.
pub mod rva {
    /// `BoolAsset`'s bool view (at asset+0x60): the concrete vtable; slot 3 is the getter the trigger update calls.
    pub const BOOL_VIEW_VTABLE: usize = 0x61ab080;
    pub const BOOL_VIEW_GETTER_SLOT: usize = 3;
    /// The getter `(view, ctx) -> bool` (0x11dd4c0): the live slot for the context, else a fallback, else `Default`.
    pub const BOOL_GETTER: usize = 0x11dd4c0;
    /// Asset vtables (what an asset object starts with): BoolAsset, GameStateEnumerationAsset, FloatAsset.
    pub const BOOL_ASSET_VTABLE: usize = 0x61aaf88;
    pub const ENUM_ASSET_VTABLE: usize = 0x6192630;
    pub const FLOAT_ASSET_VTABLE: usize = 0x61ab4a8;
    /// Slot 1 of each asset vtable (the class's own function): identifies the class.
    pub const BOOL_ASSET_SLOT1: usize = 0x11db170;
    pub const ENUM_ASSET_SLOT1: usize = 0x10ff040;
    pub const FLOAT_ASSET_SLOT1: usize = 0x11db250;
    /// dword: the module's TLS slot index (read by `thread_world`, 0x18b6ae0).
    pub const TLS_INDEX: usize = 0x78d40e0;
    /// dword: the skater effects manager's offset inside the world (ReSkate's native_vfx.h).
    pub const MANAGER_OFFSET: usize = 0x77abaa8;
    /// qword: the ANT instance registry singleton (read by 0x10c51e0, looked up by 0x10c4c70).
    pub const REGISTRY: usize = 0x728db10;
    /// The Caveman's texture (board.rs; read live with skwatch, 2026-10-21): `TextureAsset`'s vtable (asset +0), its
    /// slot 0 the shared getType (`GET_TYPE`), slot 1 the class's own function; its TypeInfo (asset +8: a static object
    /// filled at start-up, whose +0 is the descriptor) and its descriptor (static; the first u32 is the type's name
    /// hash, `board::TEXTURE_ASSET_HASH`).
    pub const TEXTURE_ASSET_VTABLE: usize = 0x64ec390;
    pub const TEXTURE_ASSET_SLOT1: usize = 0x3ee6730;
    pub const TEXTURE_ASSET_TYPEINFO: usize = 0x76fb470;
    pub const TEXTURE_ASSET_DESCRIPTOR: usize = 0x81f8cb0;
    /// `mov rax, [rcx+8]; ret`: every typed object's getType (vtable slot 0).
    pub const GET_TYPE: usize = 0x331b20;
    /// The engine's runtime texture (`TextureAsset.Resource`, asset +0x20; 0x120 bytes) and its slot 0.
    pub const RUNTIME_TEXTURE_VTABLE: usize = 0x6506c10;
    pub const RUNTIME_TEXTURE_SLOT0: usize = 0x40a27e0;
    /// RenderCore2's resource object (runtime texture +0x110: the same ID3D12Resource at its +0x118) and its slot 0.
    pub const RC2_RESOURCE_VTABLE: usize = 0x658be10;
    pub const RC2_RESOURCE_SLOT0: usize = 0x40e4d10;
}

/// Offsets inside the game's objects.
pub mod layout {
    /// Asset: the Name C string (`Animation/Dingo/Bool.State.Is.Grind`).
    pub const ASSET_NAME: usize = 0x18;
    /// Asset: the state handle, `page` (bits 0-11) | `offset` (bits 12-22) | size above.
    pub const ASSET_HANDLE: usize = 0x28;
    /// Asset: nonzero when the state is bound to a slot (the handle's top byte).
    pub const ASSET_BOUND: usize = 0x2b;
    /// BoolAsset: the bool view sub-object (its vtable is `rva::BOOL_VIEW_VTABLE`).
    pub const BOOL_VIEW: usize = 0x60;
    /// BoolAsset: `Default`.
    pub const BOOL_DEFAULT: usize = 0x78;
    /// Context: the page pointers start here (8 bytes each).
    pub const CTX_PAGES: usize = 0x18;
    /// Page: the validity bitmap (bit index = the value's byte offset).
    pub const PAGE_BITMAP: usize = 0x71c;
    /// Thread TLS block: the world pointer and the "initialised" byte.
    pub const TLS_WORLD: usize = 0x550;
    pub const TLS_INIT: usize = 0xb19;
    /// TEB: ThreadLocalStoragePointer.
    pub const TEB_TLS: usize = 0x58;
    /// Manager: the "in use" byte, and the skater records' red-black tree anchor (root at anchor + 0x10).
    pub const MANAGER_READY: usize = 0x240;
    pub const MANAGER_TREE: usize = 0x80;
    /// Tree node: right child +0, left child +8, the record's id at +0x28.
    pub const NODE_ID: usize = 0x28;
    /// Registry: the table's begin and end (16-byte entries {object, int16 generation}).
    pub const REGISTRY_BEGIN: usize = 0x18;
    pub const REGISTRY_END: usize = 0x20;
    /// Registry entry object: the context inside it.
    pub const ENTRY_CONTEXT: usize = 0x48;
}

/// Code bytes the layout above was read from: if any differs in the running image, nothing is used.
pub struct Pin {
    pub what: &'static str,
    pub rva: usize,
    pub bytes: &'static str,
}

pub const PINS: [Pin; 8] = [
    // the getter: lea rbx,[rcx-0x60]; slot lookup; [rax+0xa0] fallback; Default at +0x78
    Pin { what: "bool getter", rva: 0x11dd4c0, bytes: "40534883ec20488d59a0488bcbe8fef8ffff4885c0750c488b03488bcbff90a00000004885c0488d4b78480f44c10fb6004883c4205bc3" },
    // bound byte +0x2b, handle +0x28, page & 0xfff, pages at ctx+0x18, offset (>>12) & 0x7ff, bitmap +0x71c
    Pin {
        what: "bool slot lookup",
        rva: 0x11dcdd0,
        bytes: "4883ec2880792b004c8bd24c8bc976638b49284533c08bc125ff0f00004c8b5cc2184d85db7430c1e90c81e1ff0700008bc18bd148c1e80583e21f448bc14d03c3418b8c831c07000033c00fa3d14c0f43c04d85c07514498b4a084885c9740b498bd1e808ffffff4c8bc0498bc04883c428c333c04883c428c3",
    },
    // gs:[0x58], the TLS index (rip -> 0x78d40e0), +0xb19, +0x550
    Pin {
        what: "thread_world",
        rva: 0x18b6ae0,
        bytes: "48895c2408574883ec2065488b042558000000488bd98b15e4d50106488b3cd0b8190b0000803c38007505e8f04dbb03b850050000488b0438488903488bc3488b5c24304883c4205fc3",
    },
    // table at +0x18/+0x20, 16-byte entries, index = id & 0xffff, generation word at +8
    Pin {
        what: "registry lookup",
        rva: 0x10c4c70,
        bytes: "48895c2408574883ec20488bd98bfa488d0d928e1c06ff156467f204488b5318488b4320482bc20fb7cf48c1f8043bc873198bd94803db0fbf44da08c1e0100bc13bf87506488b1cdaeb02",
    },
    // mov rax, [rip -> 0x728db10]
    Pin { what: "registry singleton read", rva: 0x10c51f6, bytes: "488b0513891c06" },
    // mov ecx, [rip -> 0x77abaa8]
    Pin { what: "manager offset read", rva: 0x48693dd, bytes: "8b0dc526f402" },
    // lea rdi, [rcx+0x80]; mov rsi, rcx; mov rbx, [rdi+8]: the type-6 update walks the tree at manager+0x80
    Pin { what: "type-6 update tree", rva: 0x4875e85, bytes: "488db980000000488bf1488b5f08" },
    // mov ebp, [rbx+0x28] (the id); registry calls; lea r14, [rsi+0x48] (the context)
    Pin {
        what: "type-6 update context",
        rva: 0x4875f27,
        bytes: "8b6b2833f683fdff7442488d8c24d0080000e8a2f284fc488b8c24d00800008bd5e823ed84fc488b8c24d0080000488bf04885c97416bafffffffff00fc1510883ea017f0790488b01ff5008488d4c2450e8534c96fc4c8d76484885f675054c8d742450",
    },
];

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap_or(0)).collect()
}

/// Checks every pin and the vtables against the running image at `base`. Err names the first difference.
pub fn check_image(base: usize) -> Result<(), String> {
    check_image_at(base, base)
}

/// [`check_image`] for an image mapped at `base` whose pointers are relative to `pointer_base` (the exe file mapped
/// as data, unrelocated: 0x140000000).
pub fn check_image_at(base: usize, pointer_base: usize) -> Result<(), String> {
    for pin in &PINS {
        let want = unhex(pin.bytes);
        let mut got = vec![0u8; want.len()];
        if !win::read_into(base + pin.rva, &mut got) {
            return Err(format!("{} at exe+{:#x} is unreadable", pin.what, pin.rva));
        }
        if got != want {
            // ReSkate (2.1.2 and later) detours some of these functions in place: a `jmp rel32` over their first
            // bytes, the rest untouched. The layout the pin stands for is still this build's; the DLL never calls
            // the code, so a detoured head is accepted when everything past it matches
            let detoured = got.first() == Some(&0xE9) && want.len() > 8 && got[8..] == want[8..];
            if !detoured {
                return Err(format!("{} at exe+{:#x} differs from build {BUILD}", pin.what, pin.rva));
            }
            crate::worker::log(&format!(
                "{} at exe+{:#x}: its first bytes are a jump (a hook of ReSkate's, most likely); the rest is build {BUILD}'s",
                pin.what, pin.rva
            ));
        }
    }
    let slot = base + rva::BOOL_VIEW_VTABLE + rva::BOOL_VIEW_GETTER_SLOT * 8;
    match win::read_ptr(slot) {
        Some(v) if v == pointer_base + rva::BOOL_GETTER => {}
        Some(v) => return Err(format!("the bool view's getter slot holds {v:#x}, not exe+{:#x} (hooked by something else?)", rva::BOOL_GETTER)),
        None => return Err("the bool view vtable is unreadable".into()),
    }
    for (what, vt, slot1) in [
        ("BoolAsset", rva::BOOL_ASSET_VTABLE, rva::BOOL_ASSET_SLOT1),
        ("GameStateEnumerationAsset", rva::ENUM_ASSET_VTABLE, rva::ENUM_ASSET_SLOT1),
        ("FloatAsset", rva::FLOAT_ASSET_VTABLE, rva::FLOAT_ASSET_SLOT1),
    ] {
        if win::read_ptr(base + vt + 8) != Some(pointer_base + slot1) {
            return Err(format!("the {what} vtable at exe+{vt:#x} is not as in build {BUILD}"));
        }
    }
    Ok(())
}

/// The process's exe: Ok(base) when it is Skate.exe of this build (file hash and image), Err(why) otherwise.
pub fn verify_exe() -> Result<usize, String> {
    let path = win::module_path(std::ptr::null_mut()).ok_or("cannot read the exe's path")?;
    let file = std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
    if file != "skate.exe" {
        return Err(format!("not Skate.exe ({path})"));
    }
    let digest = crate::sha256::file_sha256(std::path::Path::new(&path)).map_err(|e| format!("cannot hash {path}: {e}"))?;
    if digest != EXE_SHA256 {
        return Err(format!("unknown Skate.exe build (sha256 {}...; this DLL knows {BUILD} only)", &digest[..16]));
    }
    let base = unsafe { win::GetModuleHandleW(std::ptr::null()) } as usize;
    check_image(base)?;
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_hex() {
        for p in &PINS {
            assert_eq!(p.bytes.len() % 2, 0, "{}", p.what);
            assert!(p.bytes.chars().all(|c| c.is_ascii_hexdigit()), "{}", p.what);
        }
        // the getter's slot-lookup call and the rip-relative reads land where the constants say
        let getter = unhex(PINS[0].bytes);
        let rel = i32::from_le_bytes(getter[14..18].try_into().unwrap()) as i64;
        assert_eq!((0x11dd4c0 + 18) as i64 + rel, 0x11dcdd0);
        let tw = unhex(PINS[2].bytes);
        let rel = i32::from_le_bytes(tw[24..28].try_into().unwrap()) as i64;
        assert_eq!((0x18b6ae0 + 28) as i64 + rel, rva::TLS_INDEX as i64);
        let reg = unhex(PINS[4].bytes);
        let rel = i32::from_le_bytes(reg[3..7].try_into().unwrap()) as i64;
        assert_eq!((0x10c51f6 + 7) as i64 + rel, rva::REGISTRY as i64);
        let mgr = unhex(PINS[5].bytes);
        let rel = i32::from_le_bytes(mgr[2..6].try_into().unwrap()) as i64;
        assert_eq!((0x48693dd + 6) as i64 + rel, rva::MANAGER_OFFSET as i64);
    }
}
