//! `caveman import` end to end on two installed packs (read only): a plain Studio pack (AYTOKO-Supreme_Deck_Pack) and
//! the holo pack MysteriousMagexX-DGK_Foil_Vault. Checks the files and settings written, the foil modes against the
//! pack's README, a second import leaving everything as it is, a build of the imported folder (the holo decks with
//! their own layers), and that a changed layer file rebuilds its deck. Leaves the build in scratchpad/import-test for
//! Python's verify_newdeck.py. Needs the game and those mods; run with `--ignored`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use caveman_cli::importer::{run_import, ImportArgs};
use caveman_cli::report::Reporter;
use caveman_cli::{run, Options, Outcome};
use caveman_core::import::DeckStatus;
use caveman_core::settings::{HoloMode, Settings};

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

fn reporter() -> (Reporter, Buf) {
    let buf = Buf(Arc::new(Mutex::new(Vec::new())));
    (Reporter::to(Box::new(buf.clone()), false), buf)
}

#[test]
#[ignore]
fn import_and_build() {
    let root = scratch().join("import-test");
    let _ = fs::remove_dir_all(&root);
    let to = root.join("decks").join("Imported");
    let args = ImportArgs {
        game: PathBuf::from(r"E:\Games\ReSkate"),
        decks: None,
        mods: vec!["AYTOKO-Supreme_Deck_Pack".into(), "MysteriousMagexX-DGK_Foil_Vault".into()],
        all: false,
        to: Some(to.clone()),
        force: false,
        json: false,
    };
    let (mut report, buf) = reporter();
    let packs = run_import(&args, &mut report).unwrap();
    print!("{}", String::from_utf8(buf.0.lock().unwrap().clone()).unwrap());
    assert_eq!(packs.len(), 2);
    let supreme = packs.iter().find(|p| p.pack == "Supreme Deck Pack").unwrap();
    assert_eq!(supreme.decks.len(), 21);
    assert!(supreme.decks.iter().all(|d| d.status == DeckStatus::Imported && d.carried.is_empty()));
    let vault = packs.iter().find(|p| p.pack == "DGK Foil Vault").unwrap();
    assert_eq!(vault.decks.len(), 12);
    assert!(vault.decks.iter().all(|d| d.status == DeckStatus::Imported && d.carried.iter().any(|c| c.starts_with("holo map"))));
    // the foil modes, as the pack's README lists them
    for (deck, mode) in [
        ("DGK - Stevie High Gloss Holo", HoloMode::Pearl),
        ("DGK - Stevie 8-Bit TKO Holo", HoloMode::Cellular),
        ("DGK - Kalis Night Squad Holo", HoloMode::Vertical),
        ("DGK - Boo Cold Pressed Holo", HoloMode::Frost),
        ("DGK - Reid Goldie Holo", HoloMode::Glitter),
    ] {
        let png = to.join("DGK Foil Vault").join(format!("{deck}.png"));
        assert!(png.is_file(), "{}", png.display());
        let s = Settings::read_sidecar(&png).unwrap().unwrap();
        assert_eq!(s.name.as_deref(), Some(deck));
        let l = s.layers.as_ref().unwrap();
        assert_eq!(l.foilmode, Some(mode), "{deck}");
        for (_, f) in l.files() {
            assert!(png.parent().unwrap().join(f).is_file(), "{f}");
        }
    }
    let plain = to.join("Supreme Deck Pack").join("Supreme Spaghetti Deck.png");
    assert!(Settings::read_sidecar(&plain).unwrap().unwrap().layers.is_none());

    // a second import leaves everything as it is
    let (mut report, _) = reporter();
    let again = run_import(&args, &mut report).unwrap();
    assert!(again.iter().all(|p| p.decks.iter().all(|d| d.status == DeckStatus::AlreadyThere)));

    // the imported folder builds: 33 decks
    let mut o = Options::new(r"E:\Games\ReSkate");
    o.decks = Some(root.join("decks"));
    o.wheels = Some(root.join("wheels"));
    o.out = Some(root.join("mods"));
    o.cache = Some(root.join("cache"));
    o.mod_name = "Caveman-Import-Test".into();
    let (mut report, buf) = reporter();
    let outcome = run(&o, &mut report).unwrap();
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(matches!(outcome, Outcome::Built { decks: 33, .. }), "{outcome:?}\n{text}");
    assert!(!text.contains("skipped") && !text.contains("WARNING"), "{text}");

    // a changed layer file rebuilds that deck (the item cache knows it by the file's bytes)
    let holo = to.join("DGK Foil Vault").join(".layers").join("DGK - Reid Goldie Holo.holo.png");
    let mut img = caveman_core::image::Image::read_png(&holo).unwrap();
    img.pixel_mut(256, 1000)[0] ^= 0xff;
    img.write_png(&holo).unwrap();
    let (mut report, buf) = reporter();
    run(&o, &mut report).unwrap();
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("32 of 33 item(s) reused from the item cache"), "only the changed deck is encoded again:\n{text}");
}
