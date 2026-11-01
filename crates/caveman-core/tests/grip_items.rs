//! The grip colour (`boardtapecolors0`) and grip pattern (`boardtapetextures`, the cutouts) items: their records, the
//! colour presets' bundles, textures and parameters, the cutout textures' bundles and layout, their thumbnails and
//! the master collection (ignored: reads the install; writes PNGs and a report into `target/grip/`).
//! `cargo test -p caveman-core --release --test grip_items -- --ignored --nocapture`

use caveman_core::boardmodel::texture_image;
use caveman_core::build::{preset_params_of, SHARED};
use caveman_core::ebx::{self, Data, Document, Object};
use caveman_core::game::{Bundle, Game, ITEMS_TOC};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const GAME: &str = r"E:\Games\ReSkate";

fn out_dir() -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/grip");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn show(doc: &Document, obj: &Object, indent: usize, max_array: usize, out: &mut String) {
    for f in &obj.fields {
        show_value(doc, &f.name, &f.value.data, indent, max_array, out);
    }
}

fn show_value(doc: &Document, name: &str, data: &Data, indent: usize, max_array: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match data {
        Data::Object(o) => {
            let _ = writeln!(out, "{pad}{name}: {{ ({})", doc.type_name(o.descriptor));
            show(doc, o, indent + 2, max_array, out);
            let _ = writeln!(out, "{pad}}}");
        }
        Data::Array(items) => {
            let _ = writeln!(out, "{pad}{name}: [{}]", items.len());
            for (i, v) in items.iter().enumerate().take(max_array) {
                show_value(doc, &format!("[{i}]"), &v.data, indent + 2, max_array, out);
            }
            if items.len() > max_array {
                let _ = writeln!(out, "{pad}  ... {} more", items.len() - max_array);
            }
        }
        Data::Str(b) => {
            let _ = writeln!(out, "{pad}{name}: {:?}", String::from_utf8_lossy(b));
        }
        other => {
            let _ = writeln!(out, "{pad}{name}: {other:?}");
        }
    }
}

fn read_doc(game: &Game, b: &Bundle, name: &str) -> Option<Document> {
    let i = b.manifest.ebx_index(name)?;
    let bytes = game.decode(&b.ebx_loc(i).unwrap()).ok()?;
    ebx::read_document(&bytes).ok()
}

fn dump_doc(game: &Game, b: &Bundle, name: &str, max_array: usize, max_lines: usize, report: &mut String) -> Option<Document> {
    let Some(doc) = read_doc(game, b, name) else {
        let _ = writeln!(report, "\n==== {name}: not in the bundle");
        return None;
    };
    let _ = writeln!(report, "\n==== {name} ({} instances, {} imports, file {})", doc.instances.len(), doc.imports.len(), doc.file_guid);
    for (k, (f, inst)) in doc.imports.iter().enumerate() {
        let _ = writeln!(report, "  import {k}: file {f} instance {inst}");
    }
    for (k, (a, b2)) in doc.import_list().iter().enumerate() {
        let _ = writeln!(report, "  import_list {k}: {a} | {b2}");
    }
    let mut out = String::new();
    for k in 0..doc.instances.len() {
        let _ = writeln!(out, "  -- instance {k}: {} ({})", doc.instance_type(k), doc.instances[k].instance_guid);
        if let Ok(o) = doc.object(k) {
            show(&doc, o, 6, max_array, &mut out);
        }
    }
    let lines: Vec<&str> = out.lines().collect();
    for l in lines.iter().take(max_lines) {
        let _ = writeln!(report, "{l}");
    }
    if lines.len() > max_lines {
        let _ = writeln!(report, "  ... {} more lines", lines.len() - max_lines);
    }
    Some(doc)
}

fn manifest_summary(b: &Bundle, report: &mut String) {
    let _ = writeln!(report, "  manifest: {} ebx, {} res, {} chunks", b.manifest.ebx.len(), b.manifest.res.len(), b.manifest.chunks.len());
    for e in &b.manifest.ebx {
        let _ = writeln!(report, "    ebx {}", e.name);
    }
    for r in &b.manifest.res {
        let _ =
            writeln!(report, "    res {} type {:#x} rid {:#x} meta {}", r.name, r.rtype, r.rid, r.meta.iter().map(|x| format!("{x:02x}")).collect::<String>());
    }
}

fn image_stats(img: &caveman_core::image::Image) -> String {
    let c = img.channels;
    let n = (img.width * img.height) as f64;
    let mut sums = vec![0f64; c];
    let mut full_r = 0usize;
    let mut zero_r = 0usize;
    for px in img.data.chunks_exact(c) {
        for (k, v) in px.iter().enumerate() {
            sums[k] += *v as f64;
        }
        if px[0] == 255 {
            full_r += 1;
        }
        if px[0] == 0 {
            zero_r += 1;
        }
    }
    format!(
        "{}x{} x{} channels; means {}; R==255 {:.1}%, R==0 {:.1}%",
        img.width,
        img.height,
        c,
        sums.iter().map(|s| format!("{:.1}", s / n)).collect::<Vec<_>>().join(" "),
        full_r as f64 / n * 100.0,
        zero_r as f64 / n * 100.0
    )
}

#[test]
#[ignore]
fn grip_items() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    let toc = game.toc(ITEMS_TOC).unwrap();
    let out = out_dir();
    let mut report = String::new();

    // 1. the items of both categories
    for cat in ["boardtapecolors0", "boardtapetextures"] {
        let names: Vec<&str> = shared.manifest.ebx.iter().map(|e| e.name.as_str()).filter(|n| n.starts_with(&format!("items/{cat}/"))).collect();
        let _ = writeln!(report, "== {} items in {cat}:", names.len());
        for n in &names {
            let _ = writeln!(report, "   {n}");
        }
    }
    // the thumbnails and anything else named after a few of them
    for word in
        ["deckgripcolor_gen_popsicle_00006", "deckgripcolor_gen_popsicle_00023", "deckgripcutout_gen_popsicle_00018", "deckgripcutout_gen_popsicle_00010"]
    {
        let names: Vec<&str> = shared.manifest.ebx.iter().map(|e| e.name.as_str()).filter(|n| n.to_lowercase().contains(word)).collect();
        let res: Vec<&str> = shared.manifest.res.iter().map(|r| r.name.as_str()).filter(|n| n.to_lowercase().contains(word)).collect();
        let _ = writeln!(report, "== shared bundle entries with {word}: ebx {names:?}; res {res:?}");
    }

    // 2. the item records in full
    for leaf in [
        "own_deckgripcolor_gen_popsicle_00001",
        "own_deckgripcolor_gen_popsicle_00006",
        "own_deckgripcolor_gen_popsicle_00023",
        "own_deckgripcutout_gen_popsicle_00001",
        "own_deckgripcutout_gen_popsicle_00010",
        "own_deckgripcutout_gen_popsicle_00018",
    ] {
        let Some(name) =
            shared.manifest.ebx.iter().map(|e| e.name.clone()).find(|n| n.to_lowercase().ends_with(&format!("/{leaf}")) && n.starts_with("items/"))
        else {
            let _ = writeln!(report, "\n==== {leaf}: no item record");
            continue;
        };
        dump_doc(&game, &shared, &name, 8, 120, &mut report);
    }

    // 3. the grip colour presets: bundle, textures, parameters
    for leaf in ["deck_gen_popsiclegripcolor_00001", "deck_gen_popsiclegripcolor_00006", "deck_gen_popsiclegripcolor_00023"] {
        let _ = writeln!(report, "\n######## preset {leaf}");
        let Some(bundle) = toc.toc.bundles.iter().map(|b| b.name.as_str()).find(|n| n.to_lowercase().ends_with(&format!("/{leaf}_ap_cas_main_bundlereftable")))
        else {
            let _ = writeln!(
                report,
                "  no preset bundle; bundles with {leaf}: {:?}",
                toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.contains(leaf)).collect::<Vec<_>>()
            );
            continue;
        };
        let _ = writeln!(report, "  bundle {bundle}");
        let b = game.bundle(bundle, ITEMS_TOC).unwrap();
        manifest_summary(&b, &mut report);
        let ap = b.manifest.ebx.iter().map(|e| e.name.clone()).find(|n| n.to_lowercase().ends_with(&format!("/{leaf}_ap"))).unwrap_or_default();
        dump_doc(&game, &b, &ap, 6, 260, &mut report);
        let stock = ap.trim_end_matches("_ap").to_string();
        match preset_params_of(&game, &stock) {
            Ok(params) => {
                for (key, bytes) in &params {
                    let floats: Vec<String> = bytes.as_chunks::<4>().0.iter().map(|c| format!("{:.4}", f32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect();
                    let _ = writeln!(report, "  param {key}: {} bytes {:02x?} floats {}", bytes.len(), bytes, floats.join(" "));
                }
            }
            Err(e) => {
                let _ = writeln!(report, "  params: {e}");
            }
        }
        // the textures in the bundle: sizes, and a PNG of each grip texture
        for r in &b.manifest.res {
            if r.rtype != 0x6bde_20ba {
                continue;
            }
            match texture_image(&game, ITEMS_TOC, &b, &r.name) {
                Ok(img) => {
                    let _ = writeln!(report, "  texture {}: {}", r.name, image_stats(&img));
                    if r.name.contains("gripcolor") {
                        let file = out.join(format!("{}.png", r.name.rsplit('/').next().unwrap_or("tex")));
                        let _ = img.write_png(&file);
                    }
                }
                Err(e) => {
                    let _ = writeln!(report, "  texture {}: {e}", r.name);
                }
            }
        }
    }

    // 4. the cutouts: where their textures live
    let cut_bundles: Vec<&str> = toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.to_lowercase().contains("cutout")).collect();
    let _ = writeln!(
        report,
        "\n######## {} bundles with 'cutout' in items.toc; the first 12: {:?}",
        cut_bundles.len(),
        cut_bundles.iter().take(12).collect::<Vec<_>>()
    );
    for leaf in ["deck_gen_gripcutout_00001", "deck_gen_gripcutout_00010", "deck_gen_gripcutout_00018"] {
        let _ = writeln!(report, "\n######## cutout {leaf}");
        let found: Vec<&str> = toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.to_lowercase().contains(leaf)).collect();
        let _ = writeln!(report, "  bundles named after it: {found:?}");
        for bundle in found.iter().take(2) {
            let b = game.bundle(bundle, ITEMS_TOC).unwrap();
            manifest_summary(&b, &mut report);
            for e in &b.manifest.ebx {
                if e.name.to_lowercase().contains("cutout") {
                    dump_doc(&game, &b, &e.name, 6, 80, &mut report);
                }
            }
            for r in &b.manifest.res {
                if r.rtype != 0x6bde_20ba {
                    continue;
                }
                match texture_image(&game, ITEMS_TOC, &b, &r.name) {
                    Ok(img) => {
                        let _ = writeln!(report, "  texture {}: {}", r.name, image_stats(&img));
                        let file = out.join(format!("{}.png", r.name.rsplit('/').next().unwrap_or("cutout")));
                        let _ = img.write_png(&file);
                    }
                    Err(e) => {
                        let _ = writeln!(report, "  texture {}: {e}", r.name);
                    }
                }
            }
        }
    }

    // 4b. texture formats (the res headers) of the cutouts, the grip templates and a printed grip, and the stock
    // thumbnails of a grip colour and a cutout (dims, formats, PNGs)
    let header_of = |b: &Bundle, name: &str| -> Option<caveman_core::texture::TextureHeader> {
        let i = b.manifest.res.iter().position(|r| r.name == name)?;
        let bytes = game.decode(&b.res_loc(i).unwrap()).ok()?;
        caveman_core::texture::TextureHeader::parse(&bytes, &b.manifest.res[i].meta).ok()
    };
    let _ = writeln!(report, "\n######## texture headers");
    for (bundle_suffix, names) in [
        ("/deck_gen_gripcutout_00018_osk_cas_main_bundlereftable", vec!["deck_gen_gripcutout_00018_osk"]),
        (
            "/deck_gen_popsiclegripcolor_00006_ap_cas_main_bundlereftable",
            vec!["deck_gen_gripcolor_c", "deck_gen_gripcolor_ny", "deck_gen_gripcolor_osk", "deck_gen_popsicle_c"],
        ),
        (
            "/deck_gen_popsiclegripcolor_00023_ap_cas_main_bundlereftable",
            vec!["deck_gen_popsiclegripcolor_00023_c", "deck_gen_popsiclegripcolor_00023_ny", "deck_gen_popsiclegripcolor_00023_osk"],
        ),
    ] {
        let Some(bundle) = toc.toc.bundles.iter().map(|b| b.name.as_str()).find(|n| n.to_lowercase().ends_with(bundle_suffix)) else { continue };
        let b = game.bundle(bundle, ITEMS_TOC).unwrap();
        for leaf in names {
            let Some(full) = b.manifest.res.iter().map(|r| r.name.clone()).find(|n| n.ends_with(&format!("/{leaf}"))) else {
                let _ = writeln!(report, "  {leaf}: no res");
                continue;
            };
            match header_of(&b, &full) {
                Some(h) => {
                    let _ = writeln!(
                        report,
                        "  {leaf}: format {} kind {} {}x{} mips {} first {} flags {:#x} chunk {} bytes, mip sizes {:?}",
                        h.format,
                        h.kind,
                        h.width,
                        h.height,
                        h.mips,
                        h.first_mip,
                        h.flags,
                        h.chunk_size,
                        &h.mip_sizes[..h.mip_sizes.len().min(4)]
                    );
                }
                None => {
                    let _ = writeln!(report, "  {leaf}: header unreadable");
                }
            }
        }
    }
    for leaf in [
        "own_deckgripcolor_gen_popsicle_00006",
        "own_deckgripcolor_gen_popsicle_00006_lrg",
        "own_deckgripcutout_gen_popsicle_00018",
        "own_deckgripcutout_gen_popsicle_00018_lrg",
        "own_wheelgraphic_gen_classic00006",
        "own_wheelgraphic_gen_classic00006_lrg",
    ] {
        let name = format!("thumbnail/tool/{leaf}");
        match header_of(&shared, &name) {
            Some(h) => {
                let _ = writeln!(report, "  thumbnail {leaf}: format {} {}x{} mips {} chunk {} bytes", h.format, h.width, h.height, h.mips, h.chunk_size);
            }
            None => {
                let _ = writeln!(report, "  thumbnail {leaf}: no res or unreadable");
            }
        }
        if let Ok(img) = texture_image(&game, ITEMS_TOC, &shared, &name) {
            let _ = writeln!(report, "  thumbnail {leaf} image: {}", image_stats(&img));
            let _ = img.write_png(out.join(format!("thumb_{leaf}.png")));
        }
    }

    // 5. the master collection: are the grip items in it
    if let Some(master) = read_doc(&game, &shared, "items/itemmastercollection") {
        let mut guids: HashMap<String, usize> = HashMap::new();
        for (f, _) in &master.imports {
            *guids.entry(f.to_string()).or_default() += 1;
        }
        let _ = writeln!(
            report,
            "\n######## master collection: {} instances, {} imports ({} distinct files)",
            master.instances.len(),
            master.imports.len(),
            guids.len()
        );
        for leaf in ["own_deckgripcolor_gen_popsicle_00006", "own_deckgripcutout_gen_popsicle_00018", "own_wheelgraphic_gen_classic00006"] {
            if let Some(name) =
                shared.manifest.ebx.iter().map(|e| e.name.clone()).find(|n| n.to_lowercase().ends_with(&format!("/{leaf}")) && n.starts_with("items/"))
            {
                if let Some(doc) = read_doc(&game, &shared, &name) {
                    let _ = writeln!(
                        report,
                        "  {leaf}: file {} imported by the master {} time(s)",
                        doc.file_guid,
                        guids.get(&doc.file_guid.to_string()).copied().unwrap_or(0)
                    );
                }
            }
        }
    }

    std::fs::write(out.join("report.txt"), &report).unwrap();
    println!("{report}");
}
