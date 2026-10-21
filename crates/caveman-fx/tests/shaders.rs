//! The overlay's shaders: `shaders/overlay.hlsl` compiled with FXC (`d3dcompiler_47.dll`, part of Windows) must match
//! the DXBC files the DLL embeds. `CAVEMAN_REGEN_SHADERS=1 cargo test -p caveman-fx --test shaders` rewrites them
//! (also done when a file is missing). The DLL itself never compiles anything.

use std::path::PathBuf;

use windows::core::PCSTR;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::ID3DBlob;

fn shader_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders")
}

fn compile(source: &str, entry: &str, target: &str) -> Vec<u8> {
    let entry = format!("{entry}\0");
    let target = format!("{target}\0");
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR(c"overlay.hlsl".as_ptr().cast()),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let text = errors
            .map(|b| unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).into_owned() })
            .unwrap_or_default();
        panic!("{entry} ({target}): {e}\n{text}");
    }
    let blob = code.expect("a compiled blob");
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()).to_vec() }
}

#[test]
fn embedded_shaders_match_the_source() {
    let dir = shader_dir();
    let source = std::fs::read_to_string(dir.join("overlay.hlsl")).unwrap();
    let regen = std::env::var_os("CAVEMAN_REGEN_SHADERS").is_some();
    for (entry, target, file) in [("vs_main", "vs_5_0", "overlay_vs.dxbc"), ("ps_main", "ps_5_0", "overlay_ps.dxbc")] {
        let fresh = compile(&source, entry, target);
        assert_eq!(&fresh[..4], b"DXBC", "{file} is not DXBC");
        let path = dir.join(file);
        if regen || !path.is_file() {
            std::fs::write(&path, &fresh).unwrap();
            println!("wrote {} ({} bytes)", path.display(), fresh.len());
            continue;
        }
        let embedded = std::fs::read(&path).unwrap();
        assert!(
            embedded == fresh,
            "{file} is stale ({} bytes embedded, {} compiled): run CAVEMAN_REGEN_SHADERS=1 cargo test -p caveman-fx --test shaders, then rebuild the DLL",
            embedded.len(),
            fresh.len()
        );
    }
}
