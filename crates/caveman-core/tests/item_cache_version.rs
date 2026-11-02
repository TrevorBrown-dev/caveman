//! A tripwire for the item cache's version (`item_cache::CACHE_VERSION`). The cache names a built item by its inputs
//! and that version, not by the program that built it, so an update of Caveman keeps a player's cache (before
//! 2026-11-02 every update re-encoded a whole library). That is right only while the same inputs still make the same
//! pieces, so this test fails whenever the sources that make an item's pieces change, and the change gets looked at.

use caveman_core::item_cache::{hash64, CACHE_VERSION};

/// What makes an item's pieces, from this crate's folder: the build, the encoders and texture headers, the thumbnails
/// and their mapping, the colours, the effect and settings layers, animated decks, the EBX writer, Kraken and the
/// chunk records, and the cache's own format. (Not here, so remember them: the command line's loaders in
/// `caveman-cli/src/lib.rs`, `deck_loader`, `anim_loader` and `anim_deck`, which hand the build its pictures.)
const SOURCES: &[&str] = &[
    "src/build.rs",
    "src/item_cache.rs",
    "src/image.rs",
    "src/texture.rs",
    "src/bc7.rs",
    "src/bc1.rs",
    "src/bcdec.rs",
    "src/thumbs.rs",
    "../../assets/thumbnail_mapping.json",
    "src/colour.rs",
    "src/effects.rs",
    "src/settings.rs",
    "src/anim.rs",
    "src/ebx/mod.rs",
    "src/ebx/read.rs",
    "src/ebx/write.rs",
    "src/oodle.rs",
    "src/ndb.rs",
];

/// The cache's version and the hash of the sources above that it was last checked against.
const CHECKED: (&str, u64) = ("caveman-items-2", 0x8e54d43cf86f1738);

#[test]
fn item_cache_version_is_checked_against_the_builder() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files: Vec<(&str, Vec<u8>)> = SOURCES
        .iter()
        .map(|rel| {
            let bytes = std::fs::read(dir.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e} (a file moved: update SOURCES)"));
            // the line endings git stores, whatever the checkout made of them
            (*rel, String::from_utf8_lossy(&bytes).replace("\r\n", "\n").into_bytes())
        })
        .collect();
    let parts: Vec<&[u8]> = files.iter().flat_map(|(rel, text)| [rel.as_bytes(), text.as_slice()]).collect();
    let now = hash64(&parts);
    assert!(
        (CACHE_VERSION, now) == CHECKED,
        "\nThe item builder's sources changed since the item cache's version was last checked.\n\
         Does the build now make different pieces for an item from the same picture and settings (its textures, EBX,\n\
         chunks, ids or thumbnail, or the cache's own format)? Then bump CACHE_VERSION in src/item_cache.rs, so every\n\
         player's cache rebuilds once. If not (logging, comments, a refactor, speed), leave it: players keep their cache.\n\
         Either way, record the check in tests/item_cache_version.rs:\n\n    \
         const CHECKED: (&str, u64) = (\"{CACHE_VERSION}\", {now:#018x});\n"
    );
}
