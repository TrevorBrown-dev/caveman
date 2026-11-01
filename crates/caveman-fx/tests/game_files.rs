//! Checks the DLL's build constants against the real game files, offline: the exe's SHA-256, and every pinned code
//! range and vtable in Skate.exe mapped as an image (nothing runs). Needs the game at E:\Games\ReSkate (read only);
//! run with `--ignored`.

use caveman_fx::{game, sha256, win};

const EXE: &str = r"E:\Games\ReSkate\Skate.exe";

#[test]
#[ignore]
fn exe_hash_is_the_known_build() {
    assert_eq!(sha256::file_sha256(std::path::Path::new(EXE)).unwrap(), game::EXE_SHA256);
}

#[test]
#[ignore]
fn pins_match_the_exe_image() {
    let path = win::wide(EXE);
    let h = unsafe { win::LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), win::LOAD_LIBRARY_AS_IMAGE_RESOURCE) };
    assert!(!h.is_null(), "cannot map {EXE}: {}", unsafe { win::GetLastError() });
    let base = h as usize & !3;
    game::check_image_at(base, 0x1_4000_0000).unwrap();
    // and a wrong pointer base is caught
    assert!(game::check_image_at(base, 0x1_4000_1000).is_err());
    unsafe { win::FreeLibrary(h) };
}

/// Streamline's swap chain wrapper and queue wrapper, found by their RTTI names in the game's sl.interposer.dll
/// mapped as an image (not run): the vtables' Present (8) and Present1 (22) slots point into the module's code.
#[test]
#[ignore]
fn streamline_vtables_by_rtti() {
    let file = r"E:\Games\ReSkate\sl.interposer.dll";
    let path = win::wide(file);
    let h = unsafe { win::LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), win::LOAD_LIBRARY_AS_IMAGE_RESOURCE) };
    assert!(!h.is_null(), "cannot map {file}: {}", unsafe { win::GetLastError() });
    let base = h as usize & !3;
    let pe = win::read_u32(base + 0x3C).unwrap() as usize;
    let image_base = win::read_u64(base + pe + 24 + 24).unwrap() as usize;
    let size = win::read_u32(base + pe + 24 + 56).unwrap() as usize;
    for (name, slots) in [(caveman_fx::rtti::SL_SWAPCHAIN, &[8usize, 22][..]), (caveman_fx::rtti::SL_QUEUE, &[10usize][..])] {
        let vt = caveman_fx::rtti::find_vtable(base, image_base, name).unwrap_or_else(|| panic!("{name} not found"));
        println!("{name}: vtable at rva {:#x}", vt - base);
        for &k in slots {
            let f = win::read_u64(vt + k * 8).unwrap() as usize;
            assert!(f > image_base && f < image_base + size, "{name} slot {k}: {f:#x} is not in the module");
        }
    }
    assert!(caveman_fx::rtti::find_vtable(base, image_base, ".?AUNoSuchClass@sl@@").is_none());
    unsafe { win::FreeLibrary(h) };
}

/// The render transform proxy's and the skinned render object's vtables (src/proxy.rs) are vtables in Skate.exe:
/// their first slots point into the exe's code (the image is mapped, not run; pointers are at the preferred base).
#[test]
#[ignore]
fn proxy_vtables_are_vtables() {
    let path = win::wide(EXE);
    let h = unsafe { win::LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), win::LOAD_LIBRARY_AS_IMAGE_RESOURCE) };
    assert!(!h.is_null(), "cannot map {EXE}: {}", unsafe { win::GetLastError() });
    let base = h as usize & !3;
    let pe = win::read_u32(base + 0x3C).unwrap() as usize;
    let image_base = win::read_u64(base + pe + 24 + 24).unwrap() as usize;
    let size = win::read_u32(base + pe + 24 + 56).unwrap() as usize;
    for (name, rva) in [("proxy", caveman_fx::proxy::rva::PROXY_VTABLE), ("skinned render object", caveman_fx::proxy::rva::SKINNED_VTABLE)] {
        for k in 0..4 {
            let f = win::read_u64(base + rva + k * 8).unwrap() as usize;
            assert!(f > image_base && f < image_base + size, "{name} vtable slot {k}: {f:#x} is not in the exe");
        }
    }
    unsafe { win::FreeLibrary(h) };
}

/// The Caveman's pins (src/board.rs, game.rs `rva`) against Skate.exe mapped as an image (not run; pointers at the
/// preferred base): TextureAsset's vtable (getType and the class's own function), getType's code, the descriptor's
/// name hash, the TypeInfo the static type tables give for that descriptor (an array descriptor names it as its
/// element, with the element's GUID and the last byte + 1), and the runtime texture's and RenderCore2's vtables.
#[test]
#[ignore]
fn board_pins_match_the_exe_image() {
    use caveman_fx::board::TEXTURE_ASSET_HASH;
    use caveman_fx::game::rva;
    let path = win::wide(EXE);
    let h = unsafe { win::LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), win::LOAD_LIBRARY_AS_IMAGE_RESOURCE) };
    assert!(!h.is_null(), "cannot map {EXE}: {}", unsafe { win::GetLastError() });
    let base = h as usize & !3;
    let pb = 0x1_4000_0000usize;
    let pe = win::read_u32(base + 0x3C).unwrap() as usize;
    let size = win::read_u32(base + pe + 24 + 56).unwrap() as usize;
    let slot = |vt: usize, k: usize| win::read_u64(base + vt + 8 * k).unwrap() as usize;
    assert_eq!(slot(rva::TEXTURE_ASSET_VTABLE, 0), pb + rva::GET_TYPE, "TextureAsset vtable slot 0: getType");
    assert_eq!(slot(rva::TEXTURE_ASSET_VTABLE, 1), pb + rva::TEXTURE_ASSET_SLOT1);
    let mut code = [0u8; 5];
    assert!(win::read_into(base + rva::GET_TYPE, &mut code));
    assert_eq!(code, [0x48, 0x8b, 0x41, 0x08, 0xc3], "mov rax, [rcx+8]; ret");
    assert_eq!(win::read_u32(base + rva::TEXTURE_ASSET_DESCRIPTOR), Some(TEXTURE_ASSET_HASH));
    assert_eq!(slot(rva::RUNTIME_TEXTURE_VTABLE, 0), pb + rva::RUNTIME_TEXTURE_SLOT0);
    assert_eq!(slot(rva::RC2_RESOURCE_VTABLE, 0), pb + rva::RC2_RESOURCE_SLOT0);
    // the TypeInfo: an array descriptor (kind 4) whose element is it and whose GUID is TextureAsset's, last byte + 1
    let mut image = vec![0u8; size];
    if !win::read_into(base, &mut image) {
        // a mapped image may have unreadable gaps: page by page (a gap stays zero)
        for (k, c) in image.chunks_mut(0x1000).enumerate() {
            win::read_into(base + k * 0x1000, c);
        }
    }
    let mut guid = [0u8; 16];
    guid.copy_from_slice(&image[rva::TEXTURE_ASSET_DESCRIPTOR + 8..rva::TEXTURE_ASSET_DESCRIPTOR + 24]);
    guid[15] = guid[15].wrapping_add(1);
    let ti = (pb + rva::TEXTURE_ASSET_TYPEINFO) as u64;
    let mut named = 0;
    for p in (0x30..size - 8).step_by(8) {
        if u64::from_le_bytes(image[p..p + 8].try_into().unwrap()) != ti {
            continue;
        }
        let d = p - 0x30;
        let kind = (u16::from_le_bytes([image[d + 4], image[d + 5]]) >> 5) & 0x1f;
        if kind == 4 && image[d + 8..d + 24] == guid {
            named += 1;
        }
    }
    assert_eq!(named, 1, "TextureAsset[]'s descriptor names the TypeInfo at exe+{:#x}", rva::TEXTURE_ASSET_TYPEINFO);
    unsafe { win::FreeLibrary(h) };
}
