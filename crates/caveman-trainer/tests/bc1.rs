//! The BC1 encoder (caveman-core) against the BC1 decoder (caveman-fx): round trips within the format's tolerance,
//! and the chain the world copy takes.

use caveman_core::{bc1, build, image::Image};
use caveman_fx::bc7;
use caveman_fx::board::{chain_len_for, ChainFormat};

#[test]
fn bc1_round_trips_within_tolerance() {
    // a solid colour: within the RGB565 quantisation
    let img = Image::filled(8, 8, &[60, 240, 138]);
    let enc = bc1::encode(&img);
    assert_eq!(enc.len(), 4 * 8);
    for block in enc.as_chunks::<8>().0 {
        let dec = bc7::decode_bc1(block);
        assert!(
            dec.iter().all(|p| (p[0] as i32 - 60).abs() <= 8 && (p[1] as i32 - 240).abs() <= 4 && (p[2] as i32 - 138).abs() <= 8 && p[3] == 255),
            "{dec:?}"
        );
    }
    // a gentle gradient block (a span of 60 a channel: the four palette colours are 20 apart): every texel within
    // 24 per channel of its source after the round trip
    let mut g = Image::new(4, 4, 3);
    for i in 0..16 {
        g.data[i * 3..i * 3 + 3].copy_from_slice(&[(100 + i * 4) as u8, 128, (200 - i * 4) as u8]);
    }
    let enc = bc1::encode(&g);
    let dec = bc7::decode_bc1(enc[..8].try_into().unwrap());
    for i in 0..16 {
        let want = [(100 + i * 4) as u8, 128u8, (200 - i * 4) as u8];
        assert!((0..3).all(|c| (dec[i][c] as i32 - want[c] as i32).abs() <= 24), "texel {i}: {:?} for {want:?}", dec[i]);
    }
    // a steep gradient (a span of 240): the four colours are 80 apart, so within 48
    let mut g = Image::new(4, 4, 3);
    for i in 0..16 {
        g.data[i * 3..i * 3 + 3].copy_from_slice(&[(i * 16) as u8, 128, (255 - i * 16) as u8]);
    }
    let dec = bc7::decode_bc1(bc1::encode(&g)[..8].try_into().unwrap());
    for i in 0..16 {
        let want = [(i * 16) as u8, 128u8, (255 - i * 16) as u8];
        assert!((0..3).all(|c| (dec[i][c] as i32 - want[c] as i32).abs() <= 48), "texel {i}: {:?} for {want:?}", dec[i]);
    }
    // an odd size is padded by the edge; RGBA input drops its alpha
    let odd = Image::filled(5, 3, &[10, 200, 30, 7]);
    assert_eq!(bc1::encode(&odd).len(), 2 * 8);
    // the world copy's chain: 12 levels of 512 x 2048
    let art = Image::filled(512, 2048, &[1, 2, 3]);
    let chain = build::mip_chain(&art, 12, false, bc1::encode);
    assert_eq!(chain.len(), chain_len_for(ChainFormat::Bc1, 512, 2048, 12));
}
