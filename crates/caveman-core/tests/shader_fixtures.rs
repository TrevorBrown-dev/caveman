//! The shader layer against the Python toolkit's own outputs (fixtures made by `scratchpad/shaderport/make_fixtures.py`):
//! per program, the disassembly, the patched IR, the signed container and the wrapped payload; then the whole shader
//! mod, byte for byte. Needs the game (E:\Games\ReSkate or CAVEMAN_GAME) and the fixtures (CAVEMAN_SHADER_FIXTURES).
//! Run with `cargo test --release -p caveman-core --test shader_fixtures -- --include-ignored --nocapture`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use caveman_core::game::Game;
use caveman_core::shader::{self, Dxc, Patch};

fn game_root() -> PathBuf {
    std::env::var_os("CAVEMAN_GAME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"E:\Games\ReSkate"))
}

fn fixtures() -> PathBuf {
    std::env::var_os("CAVEMAN_SHADER_FIXTURES").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(
            r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\shaderport\fixtures",
        )
    })
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y).or(if a.len() != b.len() { Some(a.len().min(b.len())) } else { None })
}

#[test]
#[ignore]
fn programs_match_python() {
    let dxc = Dxc::open(&game_root()).expect("the game's DXC");
    let started = Instant::now();
    let mut checked = 0;
    for (guid, which) in shader::programs() {
        let dir = fixtures().join(guid);
        let original = read(&dir.join("original.bin"));
        let res = shader::read_bytecode_resource(&original).expect("a DXBC");
        let ir_text = dxc.disassemble(res.dxbc).expect("disassembly");
        let py_ir = String::from_utf8(read(&dir.join("ir.ll"))).unwrap();
        assert!(ir_text == py_ir, "{guid}: disassembly differs at {:?}", first_difference(ir_text.as_bytes(), py_ir.as_bytes()));
        let (patched, notes) = shader::patch_ir(which, &py_ir).expect("patch");
        let py_patched = String::from_utf8(read(&dir.join("patched.ll"))).unwrap();
        assert!(
            patched == py_patched,
            "{guid}: patched IR differs at {:?}",
            first_difference(patched.as_bytes(), py_patched.as_bytes())
        );
        assert_eq!(notes, String::from_utf8(read(&dir.join("notes.txt"))).unwrap(), "{guid}: notes");
        let out = dxc.assemble(&patched, true).expect("assemble");
        assert!(out.ok, "{guid}: {}", out.messages);
        let py_signed = read(&dir.join("signed.dxbc"));
        assert!(out.bytes == py_signed, "{guid}: container differs at {:?}", first_difference(&out.bytes, &py_signed));
        let (payload, meta) = shader::wrap(&original, &out.bytes).unwrap();
        assert_eq!(payload, read(&dir.join("payload.bin")), "{guid}: payload");
        assert_eq!(meta.to_vec(), read(&dir.join("meta.bin")), "{guid}: meta");
        if which == Patch::Ribbon {
            assert_eq!(notes, "nyan ribbon");
        }
        checked += 1;
    }
    assert_eq!(checked, 8);
    println!("8 programs: disassembly, patched IR, signed container, payload and meta identical to Python ({:.3} s)", started.elapsed().as_secs_f64());
}

/// The animated-deck patch (deck::patch_anim) on the six deck programs' disassembly: it assembles, validates and
/// signs with the game's DXC, the stock art sample's channels are replaced, the holo mask is switched off, and the
/// non-animated patch is untouched. Writes each patched disassembly to CAVEMAN_ANIM_OUT when set.
#[test]
#[ignore]
fn anim_programs_validate() {
    let dxc = Dxc::open(&game_root()).expect("the game's DXC");
    let out_dir = std::env::var_os("CAVEMAN_ANIM_OUT").map(PathBuf::from);
    let mut checked = 0;
    for (guid, which) in shader::programs_with(true) {
        if which != Patch::DeckAnim {
            continue;
        }
        let dir = fixtures().join(guid);
        let ir_text = String::from_utf8(read(&dir.join("ir.ll"))).unwrap();
        let (plain, _) = shader::patch_ir(Patch::Deck, &ir_text).expect("plain patch");
        assert_eq!(plain, String::from_utf8(read(&dir.join("patched.ll"))).unwrap(), "{guid}: the plain patch changed");
        let (patched, notes) = shader::patch_ir(Patch::DeckAnim, &ir_text).unwrap_or_else(|e| panic!("{guid}: {e}"));
        if let Some(d) = &out_dir {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(format!("{guid}-anim.ll")), &patched).unwrap();
        }
        assert!(notes.contains("anim: art t") && notes.contains("holo mask %") && notes.contains(" off"), "{guid}: {notes}");
        assert!(patched.contains("@dx.op.textureLoad.f32(i32 66,"), "{guid}: marker loads");
        assert_eq!(patched.matches("%cvm_fs = call %dx.types.ResRet.f32 @dx.op.sampleBias.f32").count(), 1, "{guid}: one frame sample");
        assert!(!patched.contains("@dx.op.sampleLevel.f32(i32 62, %dx.types.Handle"), "{guid}: no sampled marker reads");
        let out = dxc.assemble(&patched, true).unwrap_or_else(|e| panic!("{guid}: {e}"));
        assert!(out.ok, "{guid}: {}", out.messages);
        let original = read(&dir.join("original.bin"));
        let (payload, _) = shader::wrap(&original, &out.bytes).unwrap();
        assert!(payload.len() > 0x20);
        println!("{guid}: {notes}; {} bytes signed", out.bytes.len());
        checked += 1;
    }
    assert_eq!(checked, 6);
}

/// Every file under `root`, relative, sorted.
fn tree(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

#[test]
#[ignore]
fn shader_mod_matches_python() {
    let py = fixtures().join("pymod");
    // the build time is the one thing that differs between runs: take Python's
    let stamp = String::from_utf8(read(&py.join("reskate-build.json"))).unwrap();
    let built = stamp.split("\"built\": \"").nth(1).and_then(|r| r.split('"').next()).expect("built").to_string();
    let out = std::env::temp_dir().join(format!("caveman-shader-mod-test-{}", std::process::id()));
    let game = Game::open(&game_root()).expect("the game");
    let started = Instant::now();
    let stats = shader::build_shader_mod(
        &game,
        &out,
        &shader::ShaderModOptions { title: "Caveman Shaders (test)".into(), built: Some(built), animated: false },
    )
    .expect("build");
    let seconds = started.elapsed().as_secs_f64();
    let (ours, theirs) = (tree(&out), tree(&py));
    assert_eq!(ours, theirs, "the same files");
    for f in &ours {
        let (a, b) = (read(&out.join(f)), read(&py.join(f)));
        assert!(a == b, "{}: differs at {:?} ({} vs {} bytes)", f.display(), first_difference(&a, &b), a.len(), b.len());
    }
    println!(
        "shader mod: {} files byte-identical to Python's (cas {} bytes, globals.toc {} bytes, items.toc {} bytes) in {seconds:.3} s",
        ours.len(),
        stats.cas,
        stats.toc,
        stats.items_toc
    );
    let _ = std::fs::remove_dir_all(&out);
}
