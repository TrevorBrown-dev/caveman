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
