//! Self-contained checks of the pixel modules (no game files or fixtures needed).

use caveman_core::bc1;
use caveman_core::bc7;
use caveman_core::bcdec::{self, BcFormat};
use caveman_core::image::Image;
use caveman_core::texture::{block_containing, mip_sizes, owner_hash};
use caveman_core::thumbs::{pairwise_sum, Mapping};

/// A deterministic test pattern: gradients and hard edges.
fn pattern(w: usize, h: usize, c: usize) -> Image {
    let mut img = Image::new(w, h, c);
    for y in 0..h {
        for x in 0..w {
            let px = img.pixel_mut(x, y);
            for (ch, v) in px.iter_mut().enumerate() {
                *v = match ch {
                    0 => (x * 255 / w.max(1)) as u8,
                    1 => (y * 255 / h.max(1)) as u8,
                    2 => {
                        if (x / 8 + y / 8) % 2 == 0 {
                            230
                        } else {
                            20
                        }
                    }
                    _ => ((x + y) % 256) as u8,
                };
            }
        }
    }
    img
}

fn psnr(a: &Image, b: &Image, channels: usize) -> f64 {
    let mut se = 0f64;
    let mut n = 0f64;
    for (pa, pb) in a.data.chunks_exact(a.channels).zip(b.data.chunks_exact(b.channels)) {
        for c in 0..channels {
            let d = pa[c] as f64 - pb[c] as f64;
            se += d * d;
            n += 1.0;
        }
    }
    10.0 * (255.0f64 * 255.0 / (se / n).max(1e-9)).log10()
}

#[test]
fn bc7_round_trip_is_close() {
    let img = pattern(64, 128, 3);
    let blocks = bc7::encode(&img);
    assert_eq!(blocks.len(), 16 * 16 * 32);
    let back = bcdec::decode(BcFormat::Bc7, &blocks, 64, 128);
    let q = psnr(&img, &back, 3);
    assert!(q > 35.0, "BC7 round trip PSNR {q}");
    assert!(back.data.as_chunks::<4>().0.iter().all(|p| p[3] >= 254), "RGB input encodes opaque");
    let rgba = pattern(32, 32, 4);
    let back = bcdec::decode(BcFormat::Bc7, &bc7::encode(&rgba), 32, 32);
    assert!(psnr(&rgba, &back, 4) > 30.0);
}

#[test]
fn bc1_round_trip_is_close() {
    let img = pattern(64, 64, 3);
    let back = bcdec::decode(BcFormat::Bc1, &bc1::encode(&img), 64, 64);
    let q = psnr(&img, &back, 3);
    assert!(q > 25.0, "BC1 round trip PSNR {q}");
    let flat = Image::filled(8, 8, &[10, 200, 30]);
    let back = bcdec::decode(BcFormat::Bc1, &bc1::encode(&flat), 8, 8);
    assert!(back.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255 && (p[1] as i32 - 200).abs() <= 4));
}

#[test]
fn odd_sizes_pad_by_repeating_the_edge() {
    let img = pattern(5, 3, 3);
    assert_eq!(bc7::encode(&img).len(), 2 * 16);
    assert_eq!(bc1::encode(&img).len(), 2 * 8);
}

#[test]
fn deck_mip_chain() {
    assert_eq!(mip_sizes(512, 2048, 12, 16), vec![1048576, 262144, 65536, 16384, 4096, 1024, 256, 64, 32, 16, 16, 16]);
    assert_eq!(mip_sizes(512, 512, 10, 8)[..3], [131072, 32768, 8192]);
}

#[test]
fn block_containing_walks_cas_headers() {
    // two stored blocks: 0x10 decoded bytes, then 0x20
    let mut enc = Vec::new();
    for (decoded, payload) in [(0x10u32, 0x10usize), (0x20, 0x20)] {
        enc.extend_from_slice(&decoded.to_be_bytes());
        enc.extend_from_slice(&[0x00, 0x70]);
        enc.extend_from_slice(&(payload as u16).to_be_bytes());
        enc.extend(std::iter::repeat_n(0u8, payload));
    }
    assert_eq!(block_containing(&enc, 0).unwrap(), 0);
    assert_eq!(block_containing(&enc, 0x0F).unwrap(), 0);
    assert_eq!(block_containing(&enc, 0x10).unwrap(), 0x18);
    assert_eq!(block_containing(&enc, 0x2F).unwrap(), 0x18);
    assert!(block_containing(&enc, 0x30).is_err());
}

#[test]
fn owner_hash_is_djb2_xor_of_lower_case() {
    let name = "characters/customization/reskate/board_bottomart/Test_Deck_role3_2259d419-387f-b1ba-30fc-c5140100b012";
    assert_eq!(owner_hash(name), 0x81c3a63df2d10e1c); // newdeck.owner_hash
    assert_eq!(owner_hash(name), owner_hash(&name.to_uppercase()));
}

#[test]
fn numpy_pairwise_sum() {
    let a: Vec<f64> = (0..37).map(|i| 1.0 / (i as f64 + 1.0)).collect();
    let mut r = [0f64; 8];
    r.copy_from_slice(&a[..8]);
    for i in (8..32).step_by(8) {
        for j in 0..8 {
            r[j] += a[i + j];
        }
    }
    let mut want = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
    for v in &a[32..] {
        want += v;
    }
    assert_eq!(pairwise_sum(&a), want);
    assert_eq!(pairwise_sum(&[1.0, 2.0, 3.0]), 6.0);
}

#[test]
fn built_in_thumbnail_mapping() {
    let m = Mapping::default();
    assert_eq!(m.thumb_size, 768.0);
    assert_eq!(m.art_cols, [14, 498]);
    assert_eq!(m.poly.len(), 6);
}

#[test]
fn image_basics() {
    let img = pattern(10, 7, 4);
    assert_eq!(img.resize_lanczos(10, 7), img);
    let back = Image::decode_png(&img.encode_png().unwrap()).unwrap();
    assert_eq!(back, img);
    let r = img.rotate_cw();
    assert_eq!((r.width, r.height), (7, 10));
    assert_eq!(r.pixel(0, 0), img.pixel(0, 6)); // the left edge becomes the top
    assert_eq!(img.crop(2, 1, 5, 4).pixel(0, 0), img.pixel(2, 1));
}
