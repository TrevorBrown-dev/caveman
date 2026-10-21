//! The pixel port against the Python toolkit and Pillow: fixtures written by the session's
//! `scratchpad/pixels/make_fixtures.py` (set CAVEMAN_PIXEL_FIXTURES to their folder). Ignored by default:
//! `cargo test -p caveman-core --test pixels_fixtures -- --ignored --nocapture`.

use std::path::PathBuf;

use caveman_core::bc1;
use caveman_core::bc7;
use caveman_core::bcdec::{self, BcFormat};
use caveman_core::image::Image;
use caveman_core::thumbs::{self, Field, Lighting, Mapping};

fn dir() -> PathBuf {
    std::env::var("CAVEMAN_PIXEL_FIXTURES").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\pixels\fixtures")
    })
}

fn png(name: &str) -> Image {
    Image::read_png(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// A .npy array: (shape, values as f64).
fn npy(name: &str) -> (Vec<usize>, Vec<f64>) {
    let b = std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(&b[..6], b"\x93NUMPY");
    let (hlen, start) = if b[6] == 1 { (u16::from_le_bytes([b[8], b[9]]) as usize, 10) } else { (u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, 12) };
    let header = std::str::from_utf8(&b[start..start + hlen]).unwrap();
    let descr = header.split("'descr':").nth(1).unwrap().split('\'').nth(1).unwrap();
    let shape_txt = header.split("'shape':").nth(1).unwrap();
    let shape: Vec<usize> = shape_txt[shape_txt.find('(').unwrap() + 1..shape_txt.find(')').unwrap()]
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    let data = &b[start + hlen..];
    let values = match descr {
        "<f8" => data.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect(),
        "|u1" => data.iter().map(|&v| v as f64).collect(),
        other => panic!("{name}: dtype {other}"),
    };
    (shape, values)
}

fn npy_image(name: &str) -> Image {
    let (shape, v) = npy(name);
    let c = if shape.len() == 3 { shape[2] } else { 1 };
    Image::from_raw(shape[1], shape[0], c, v.iter().map(|&x| x as u8).collect()).unwrap()
}

fn npy_field(name: &str) -> Field {
    let (shape, v) = npy(name);
    let c = if shape.len() == 3 { shape[2] } else { 1 };
    Field { width: shape[1], height: shape[0], channels: c, data: v }
}

/// (differing bytes, max difference)
fn diff_bytes(a: &[u8], b: &[u8]) -> (usize, i32) {
    assert_eq!(a.len(), b.len());
    let mut n = 0;
    let mut m = 0;
    for (x, y) in a.iter().zip(b) {
        let d = (*x as i32 - *y as i32).abs();
        if d > 0 {
            n += 1;
            m = m.max(d);
        }
    }
    (n, m)
}

/// (max abs difference, mean abs difference)
fn diff_f64(a: &[f64], b: &[f64]) -> (f64, f64) {
    assert_eq!(a.len(), b.len());
    let mut m = 0f64;
    let mut s = 0f64;
    for (x, y) in a.iter().zip(b) {
        let d = (x - y).abs();
        m = m.max(d);
        s += d;
    }
    (m, s / a.len() as f64)
}

fn convert(img: &Image, mode: &str) -> Image {
    match mode {
        "RGBA" => img.to_rgba(),
        "RGB" => img.to_rgb(),
        _ => img.to_l(),
    }
}

#[test]
#[ignore]
fn resize_matches_pillow() {
    let mut cases: Vec<(&str, &str, (usize, usize))> = vec![
        ("dan.png", "RGBA", (512, 2048)),
        ("dan.png", "RGBA", (300, 700)),
        ("dan.png", "RGB", (512, 2048)),
        ("dan.png", "L", (200, 801)),
    ];
    for m in 1..12 {
        cases.push(("art10.png", "RGB", ((512usize >> m).max(1), (2048usize >> m).max(1))));
    }
    cases.push(("art10.png", "RGB", (194, 2048)));
    cases.push(("small.png", "RGBA", (512, 2048)));
    let mut bad = 0;
    for (k, (src, mode, (w, h))) in cases.iter().enumerate() {
        let ours = convert(&png(src), mode).resize_lanczos(*w, *h);
        let theirs = convert(&png(&format!("resize_{k}.png")), mode);
        let (n, m) = diff_bytes(&ours.data, &theirs.data);
        println!("resize {k}: {src} {mode} -> {w}x{h}: {n} bytes differ (max {m})");
        bad += n;
    }
    assert_eq!(bad, 0, "resize differs from Pillow");
}

#[test]
#[ignore]
fn composite_rotate_filters_match_pillow() {
    let wood = png("wood.png").to_rgba();
    let over = png("resize_0.png").to_rgba();
    let (n, m) = diff_bytes(&wood.alpha_composite(&over).data, &png("composite.png").to_rgba().data);
    println!("alpha_composite: {n} bytes differ (max {m})");
    let mut bad = n;
    let (n, m) = diff_bytes(&png("small.png").to_rgba().rotate_cw().data, &png("rotated.png").to_rgba().data);
    println!("rotate: {n} bytes differ (max {m})");
    bad += n;
    let inside = png("inside.png").to_l();
    for size in [13usize, 5] {
        let (n, m) = diff_bytes(&inside.min_filter(size).data, &png(&format!("min_{size}.png")).to_l().data);
        println!("min filter {size}: {n} bytes differ (max {m})");
        bad += n;
    }
    for (r, name) in [(2.0f32, "blur_2.png"), (1.5, "blur_1.5.png"), (6.0, "blur_6.png")] {
        let (n, m) = diff_bytes(&inside.gaussian_blur(r).data, &png(name).to_l().data);
        println!("gaussian blur {r}: {n} bytes differ (max {m})");
        bad += n;
    }
    assert_eq!(bad, 0, "composite, rotate or filters differ from Pillow");
}

fn check_encoder(kind: &str, names: &[&str]) -> usize {
    let unit = if kind == "bc7" { 16 } else { 8 };
    let mut bad_blocks = 0;
    for name in names {
        let input = npy_image(&format!("{name}.npy"));
        let want = std::fs::read(dir().join(format!("{name}.{kind}"))).unwrap();
        let t = std::time::Instant::now();
        let got = if kind == "bc7" { bc7::encode(&input) } else { bc1::encode(&input) };
        let ms = t.elapsed().as_millis();
        assert_eq!(got.len(), want.len(), "{name}: size");
        let bad = got.chunks(unit).zip(want.chunks(unit)).filter(|(a, b)| a != b).count();
        println!("{kind} {name}: {}x{}x{}: {} of {} blocks differ ({ms} ms)", input.width, input.height, input.channels, bad, want.len() / unit);
        bad_blocks += bad;
    }
    bad_blocks
}

#[test]
#[ignore]
fn bc7_matches_python() {
    let names = ["bc7_art00010", "bc7_art00010_m3", "bc7_art00041", "bc7_art00041_m3", "bc7_art00088", "bc7_art00088_m3", "bc7_osk88", "bc7_ny88", "bc7_thumb", "bc7_odd"];
    assert_eq!(check_encoder("bc7", &names), 0, "BC7 blocks differ from Python");
}

#[test]
#[ignore]
fn bc1_matches_python() {
    let names = ["bc1_wheelmask", "bc1_art", "bc1_dan", "bc1_odd"];
    assert_eq!(check_encoder("bc1", &names), 0, "BC1 blocks differ from Python");
}

#[test]
#[ignore]
fn decoders_match_pillow() {
    let cases: [(&str, BcFormat, usize, usize); 8] = [
        ("dec_bc7_art88", BcFormat::Bc7, 512, 2048),
        ("dec_bc7_osk88", BcFormat::Bc7, 512, 2048),
        ("dec_bc1_msk", BcFormat::Bc1, 0, 0),
        ("dec_bc1_wheelosk", BcFormat::Bc1, 512, 512),
        ("dec_bc7_thumb", BcFormat::Bc7, 768, 768),
        ("rand_bc7", BcFormat::Bc7, 256, 4096),
        ("rand_bc1", BcFormat::Bc1, 256, 1024),
        ("rand_bc3", BcFormat::Bc3, 256, 1024),
    ];
    let mut bad = 0;
    for (tag, fmt, w, h) in cases {
        let want = png(&format!("{tag}.png")).to_rgba();
        let (w, h) = if w == 0 { (want.width, want.height) } else { (w, h) };
        let raw = std::fs::read(dir().join(format!("{tag}.raw"))).unwrap();
        let got = bcdec::decode(fmt, &raw, w, h);
        let (n, m) = diff_bytes(&got.data, &want.data);
        println!("decode {tag} ({fmt:?} {w}x{h}): {n} bytes differ (max {m})");
        bad += n;
    }
    assert_eq!(bad, 0, "decoders differ from Pillow");
}

fn report(name: &str, ours: &[f64], theirs: &[f64]) -> f64 {
    let (m, mean) = diff_f64(ours, theirs);
    println!("{name}: max |diff| {m:.3e}, mean {mean:.3e}");
    m
}

fn report_png(name: &str, ours: &Image, file: &str) -> i32 {
    let theirs = png(file).to_rgba();
    let (n, m) = diff_bytes(&ours.data, &theirs.data);
    println!("{name}: {n} of {} bytes differ (max {m})", theirs.data.len());
    m
}

#[test]
#[ignore]
fn thumbnails_match_python() {
    let map = Mapping::default();
    let ns = ["00010", "00020", "00030", "00050", "00060", "00070"];
    let dan = png("render_art.png").to_rgb(); // the exact art the Python renders used
    let mut worst = 0f64;
    let mut worst_px = 0;
    for size in [256usize, 768] {
        let pairs: Vec<(Image, Image)> =
            ns.iter().map(|n| (png(&format!("tart_{n}.png")), png(&format!("tthumb_{n}_{size}.png")))).collect();
        worst = worst.max(report(&format!("map_art {size}"), &thumbs::map_art(&pairs[0].0, size, &map).data, &npy_field(&format!("map_art_{size}.npy")).data));
        let t = std::time::Instant::now();
        let lit = thumbs::fit_light(&pairs, size, &map);
        println!("fit_light {size}: {} ms", t.elapsed().as_millis());
        worst = worst.max(report(&format!("light {size}"), &lit.light.data, &npy_field(&format!("light_{size}.npy")).data));
        worst = worst.max(report(&format!("sheen {size}"), &lit.sheen.data, &npy_field(&format!("sheen_{size}.npy")).data));
        worst = worst.max(report(&format!("alpha {size}"), &lit.alpha, &npy_field(&format!("alpha_{size}.npy")).data));
        worst_px = worst_px.max(report_png(&format!("render {size}"), &thumbs::render(&dan, &lit, None, &map), &format!("render_{size}.png")));
    }
    let metal_decks = ["gen_00041", "gen_00042", "chocolate_00001", "nhs_00013"];
    let mpairs: Vec<(Image, Image)> = metal_decks.iter().map(|d| (png(&format!("mart_{d}.png")), png(&format!("mthumb_{d}.png")))).collect();
    let ml = thumbs::fit_metal(&mpairs, 256, &map);
    worst = worst.max(report("metal light 256", &ml.data, &npy_field("metal_256.npy").data));
    let pairs: Vec<(Image, Image)> = ns.iter().map(|n| (png(&format!("tart_{n}.png")), png(&format!("tthumb_{n}_256.png")))).collect();
    let lit = thumbs::fit_light(&pairs, 256, &map);
    let mask = png("metalmask.png").to_l();
    worst_px = worst_px.max(report_png("render metal 256", &thumbs::render(&dan, &lit, Some((&mask, &ml)), &map), "render_metal_256.png"));
    // wheels
    let wpairs: Vec<(Field, Image)> = (0..6).map(|k| (npy_field(&format!("wtex_{k}.npy")), png(&format!("wthumb_{k}.png")))).collect();
    worst = worst.max(report("map_wheel 256", &thumbs::map_wheel(&wpairs[0].0, 256).data, &npy_field("map_wheel_256.npy").data));
    let wl: Lighting = thumbs::fit_wheel_light(&wpairs, 256);
    worst = worst.max(report("wheel light 256", &wl.light.data, &npy_field("wlight_256.npy").data));
    worst = worst.max(report("wheel sheen 256", &wl.sheen.data, &npy_field("wsheen_256.npy").data));
    worst = worst.max(report("wheel alpha 256", &wl.alpha, &npy_field("walpha_256.npy").data));
    worst_px = worst_px.max(report_png("render wheel 256", &thumbs::render_wheel(&wpairs[0].0, &wl), "render_wheel_256.png"));
    println!("worst float difference {worst:.3e}, worst rendered pixel difference {worst_px}");
    assert!(worst < 1e-6, "thumbnail arrays differ from Python by {worst}");
    assert_eq!(worst_px, 0, "rendered thumbnails differ from Python by {worst_px}");
}

#[test]
#[ignore]
fn texture_headers_match_python() {
    use caveman_core::texture::{new_texture_header, owner_hash, TextureHeader};
    for tag in ["deck", "wheelmask"] {
        let read = |k: &str| std::fs::read(dir().join(format!("hdr_{tag}.{k}"))).unwrap();
        let (tmpl, meta, enc, want) = (read("tmpl"), read("meta"), read("enc"), read("want"));
        let txt = String::from_utf8(read("txt")).unwrap();
        let mut lines = txt.lines();
        let name = lines.next().unwrap();
        let f: Vec<u64> = lines.next().unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
        let h = TextureHeader::parse(&tmpl, &meta).unwrap();
        let mips = f[3] as usize;
        assert_eq!(
            (h.format as u64, h.width as u64, h.height as u64, h.mips as u64, h.first_mip as u64, h.chunk_size as u64),
            (f[0], f[1], f[2], f[3], f[4], f[5]),
            "{tag}: parsed fields"
        );
        assert_eq!(h.mip_sizes.iter().map(|&v| v as u64).collect::<Vec<_>>(), f[6..6 + mips].to_vec(), "{tag}: mip sizes");
        assert_eq!(
            (h.version as u64, h.kind as u64, h.flags as u64, h.depth as u64, h.slices as u64),
            (f[6 + mips], f[7 + mips], f[8 + mips], f[9 + mips], f[10 + mips]),
            "{tag}: version, type, flags, depth, slices"
        );
        let chunk: [u8; 16] = want[40..56].try_into().unwrap();
        let got = new_texture_header(&tmpl, &meta, &enc, chunk, name).unwrap();
        println!("header {tag}: {} bytes, identical {}", got.len(), got == want);
        assert_eq!(got, want, "{tag}: new header differs from Python");
        assert_eq!(&want[120..128], &owner_hash(name).to_le_bytes(), "{tag}: name hash");
    }
}
