//! Incremental builds (the item cache): a clean build of a copy of the drop folder (plus two wheels), a rebuild with
//! nothing changed (every item reused, the same mod), then one deck added, one deck's settings changed and one deck
//! removed (only the new and changed decks encoded), and a clean build (`no_cache`) of the same final inputs: the
//! incremental mod must be byte-identical to it (apart from reskate-build.json's timestamp). Then a synthetic set of
//! 200 decks, clean and with one deck added, for timings. Leaves the mods in scratchpad/incr/snap-* for Python's
//! verifiers. Needs the game (read only) and E:\Games\ReSkate\Caveman Decks; run with `--ignored`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use caveman_cli::report::Reporter;
use caveman_cli::{run, Options, Outcome};
use caveman_core::image::Image;

fn scratch() -> PathBuf {
    std::env::var_os("CAVEMAN_SCRATCH").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad")
    })
}

#[derive(Clone)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs the CLI; returns the outcome, what it said and how long it took.
fn go(o: &Options) -> (Outcome, String, f64) {
    let buf = Buf(Arc::new(Mutex::new(Vec::new())));
    let mut report = Reporter::to(Box::new(buf.clone()), false);
    let start = Instant::now();
    let outcome = run(o, &mut report).unwrap();
    let secs = start.elapsed().as_secs_f64();
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    (outcome, text, secs)
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let t = to.join(p.file_name().unwrap());
        if p.is_dir() {
            if p.file_name().unwrap().to_string_lossy().to_lowercase() != "previews" {
                copy_dir(&p, &t);
            }
        } else {
            fs::copy(&p, &t).unwrap();
        }
    }
}

/// Every file under a folder: relative path -> bytes.
fn files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.strip_prefix(dir).unwrap().display().to_string(), fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Two mod folders hold the same files with the same bytes, reskate-build.json (its build time) aside.
fn same_mod(a: &Path, b: &Path) {
    let (fa, fb) = (files(a), files(b));
    assert_eq!(fa.keys().collect::<Vec<_>>(), fb.keys().collect::<Vec<_>>(), "{} and {} hold different files", a.display(), b.display());
    for (name, bytes) in &fa {
        if name == "reskate-build.json" {
            continue;
        }
        assert!(bytes == &fb[name], "{name} differs between {} and {}", a.display(), b.display());
    }
}

fn wheel_face(hue: f64) -> Image {
    let mut img = Image::new(512, 512, 4);
    for y in 0..512 {
        for x in 0..512 {
            let (dx, dy) = (x as f64 - 255.5, y as f64 - 255.5);
            let r = (dx * dx + dy * dy).sqrt() / 256.0;
            if (0.35..0.9).contains(&r) {
                let a = dy.atan2(dx) + hue;
                img.pixel_mut(x, y).copy_from_slice(&[(128.0 + 127.0 * a.cos()) as u8, (128.0 + 127.0 * a.sin()) as u8, 200, 255]);
            }
        }
    }
    img
}

/// A simple deck art: stripes and a gradient, different for every `n`.
fn synthetic_art(n: usize) -> Image {
    let mut img = Image::new(256, 1024, 3);
    let (r, g, b) = ((n * 37) % 256, (n * 91) % 256, (n * 151) % 256);
    for y in 0..1024 {
        for x in 0..256 {
            let stripe = ((x / 16 + y / 64 + n) % 2) as u8;
            let px = img.pixel_mut(x, y);
            px[0] = (r as u8).wrapping_add(stripe * 60);
            px[1] = (g as u8).wrapping_add((y / 8) as u8);
            px[2] = (b as u8).wrapping_add((x / 2) as u8);
        }
    }
    img
}

#[test]
#[ignore]
fn incremental() {
    let root = scratch().join("incr");
    for d in ["decks", "wheels", "mods", "mods-clean", "cache", "snap-clean", "snap-incremental"] {
        let _ = fs::remove_dir_all(root.join(d));
    }
    let (decks, wheels, mods, cache) = (root.join("decks"), root.join("wheels"), root.join("mods"), root.join("cache"));
    copy_dir(Path::new(r"E:\Games\ReSkate\Caveman\Decks"), &decks);
    fs::create_dir_all(&wheels).unwrap();
    wheel_face(0.0).write_png(wheels.join("Red Donut.ff0000.png")).unwrap();
    wheel_face(1.0).write_png(wheels.join("Plain Wheel.png")).unwrap();
    let mut o = Options::new(r"E:\Games\ReSkate");
    o.decks = Some(decks.clone());
    o.wheels = Some(wheels.clone());
    o.out = Some(mods.clone());
    o.cache = Some(cache.clone());
    // the deck PNGs: not the layer files in an imported deck's `.layers` (dot-folders are not decks)
    // (GIFs and videos are animated decks; the previews the builds write are not decks)
    let n_decks = files(&decks)
        .keys()
        .filter(|k| {
            let parts: Vec<&str> = k.split(['\\', '/']).collect();
            let name = parts.last().copied().unwrap_or_default();
            (name.to_lowercase().ends_with(".png") || caveman_core::anim::clip_kind(name).is_some())
                && !parts.iter().any(|part| part.starts_with('.'))
                && !parts[0].eq_ignore_ascii_case(caveman_cli::names::PREVIEWS)
        })
        .count();

    // 1. clean: every item built
    let (outcome, text, t_clean) = go(&o);
    print!("{text}");
    assert!(matches!(outcome, Outcome::Built { .. }), "{outcome:?}");
    assert!(!text.contains("reused from the item cache"));
    copy_dir(&mods.join("Caveman-Decks"), &root.join("snap-clean").join("Caveman-Decks"));

    // 2. nothing changed (no --if-changed, so the build runs): every item from the cache, the same mod
    let (_, text, t_same) = go(&o);
    let all = n_decks + 2;
    assert!(text.contains(&format!("{all} of {all} item(s) reused from the item cache")), "{text}");
    same_mod(&root.join("snap-clean").join("Caveman-Decks"), &mods.join("Caveman-Decks"));

    // 3. one deck added, one deck's settings changed, one deck removed: only two decks encoded
    // (the decks are whatever the drop folder's 000 holds: a plain-named PNG gets the settings, another PNG goes)
    let d000 = decks.join("000");
    let mut pngs: Vec<PathBuf> =
        fs::read_dir(&d000).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png"))).collect();
    pngs.sort();
    let changed = pngs.iter().find(|p| !p.file_stem().unwrap().to_string_lossy().contains('.')).expect("a PNG deck without suffixes in 000").clone();
    let removed = pngs.iter().find(|p| **p != changed).expect("a second PNG deck in 000").clone();
    fs::copy(&changed, d000.join("Incr Added.foil.png")).unwrap();
    // (same name, different effects: a settings file naming the deck as before, with the cycle)
    let stem = changed.file_stem().unwrap().to_string_lossy().into_owned();
    fs::write(caveman_core::settings::sidecar_path(&changed), format!(r#"{{"version": 1, "name": "{stem}", "shader": {{"effect": "cycle", "speed": 1.0}}}}"#))
        .unwrap();
    fs::remove_file(&removed).unwrap();
    let (_, text, t_incr) = go(&o);
    print!("{text}");
    let kept = all - 1 - 1; // the removed deck and the deck whose settings changed are not reused
    assert!(text.contains(&format!("{kept} of {all} item(s) reused from the item cache")), "{text}");
    assert!(text.contains("removed 2 item(s) no longer built from the item cache"), "{text}");
    copy_dir(&mods.join("Caveman-Decks"), &root.join("snap-incremental").join("Caveman-Decks"));

    // 4. a clean build of the same final inputs: byte-identical to the incremental one
    let mut clean = o.clone();
    clean.out = Some(root.join("mods-clean"));
    clean.no_cache = true;
    let (_, text, t_final_clean) = go(&clean);
    assert!(!text.contains("reused from the item cache"));
    same_mod(&mods.join("Caveman-Decks"), &root.join("mods-clean").join("Caveman-Decks"));
    same_mod(&mods.join("Caveman-Shaders"), &root.join("mods-clean").join("Caveman-Shaders"));

    println!("TIMING drop folder ({n_decks} decks + 2 wheels): clean {t_clean:.1} s, unchanged rebuild {t_same:.1} s, +1/~1/-1 deck {t_incr:.1} s, clean of the final set {t_final_clean:.1} s");
}

#[test]
#[ignore]
fn synthetic_200() {
    let root = scratch().join("incr").join("synth");
    for d in ["decks", "wheels", "mods", "mods-clean", "cache"] {
        let _ = fs::remove_dir_all(root.join(d));
    }
    let (decks, wheels, mods) = (root.join("decks"), root.join("wheels"), root.join("mods"));
    fs::create_dir_all(decks.join("000")).unwrap();
    fs::create_dir_all(&wheels).unwrap();
    for n in 0..200 {
        synthetic_art(n).write_png(decks.join("000").join(format!("Synth {n:03}.png"))).unwrap();
    }
    let mut o = Options::new(r"E:\Games\ReSkate");
    o.decks = Some(decks.clone());
    o.wheels = Some(wheels.clone());
    o.out = Some(mods.clone());
    o.cache = Some(root.join("cache"));

    let (outcome, _, t_clean) = go(&o);
    assert_eq!(outcome, Outcome::Built { decks: 200, wheels: 0, grips: 0, cutouts: 0, shaders: caveman_cli::Shaders::None });
    let (_, text, t_same) = go(&o);
    assert!(text.contains("200 of 200 item(s) reused"), "{text}");
    synthetic_art(200).write_png(decks.join("000").join("Synth 200.png")).unwrap();
    let (_, text, t_add) = go(&o);
    assert!(text.contains("200 of 201 item(s) reused"), "{text}");
    let mut clean = o.clone();
    clean.out = Some(root.join("mods-clean"));
    clean.no_cache = true;
    let (_, _, t_final_clean) = go(&clean);
    same_mod(&mods.join("Caveman-Decks"), &root.join("mods-clean").join("Caveman-Decks"));
    println!(
        "TIMING synthetic: 200 decks clean {t_clean:.1} s, unchanged rebuild {t_same:.1} s, one deck added {t_add:.1} s, clean 201 decks {t_final_clean:.1} s"
    );
}
