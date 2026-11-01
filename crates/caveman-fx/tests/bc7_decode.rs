//! The BC7 decoder against Pillow's (a port of DirectXTex): random blocks of every mode, decoded by both.

use caveman_fx::bc7;

#[test]
fn decodes_every_mode_as_directxtex_does() {
    let text = include_str!("fixtures/bc7_blocks.txt");
    let mut n = 0;
    let mut by_mode = [0usize; 8];
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let mut parts = line.split_whitespace();
        let hex = parts.next().unwrap();
        let mut block = [0u8; 16];
        for (k, b) in block.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[k * 2..k * 2 + 2], 16).unwrap();
        }
        let want: Vec<[u8; 4]> = parts
            .map(|t| {
                let v: Vec<u8> = t.split(',').map(|x| x.parse().unwrap()).collect();
                [v[0], v[1], v[2], v[3]]
            })
            .collect();
        let got = bc7::decode_block(&block);
        let mode = (0..8).find(|&m| (block[0] >> m) & 1 == 1).unwrap();
        by_mode[mode] += 1;
        assert_eq!(&got[..], &want[..], "block {hex} (mode {mode})");
        n += 1;
    }
    assert!(n >= 400 && by_mode.iter().all(|&c| c >= 50), "{n} blocks, by mode {by_mode:?}");
    // the error of a block against itself is none, against another is not
    let a = bc7::decode_block(&[0x40, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    let b = bc7::decode_block(&[0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(bc7::block_error(&a, &a) == 0.0 && bc7::block_error(&a, &b) > 200.0);
}

#[test]
fn bc1_and_bc3_blocks_decode() {
    // BC1: red (0xf800) and blue (0x001f), every index 0: all red; indices 1: all blue; the 3-colour mode's
    // transparent black when the first endpoint is not above the second
    let red_blue = [0x00, 0xf8, 0x1f, 0x00, 0, 0, 0, 0];
    assert!(bc7::decode_bc1(&red_blue).iter().all(|p| *p == [255, 0, 0, 255]));
    let all_one = [0x00, 0xf8, 0x1f, 0x00, 0x55, 0x55, 0x55, 0x55];
    assert!(bc7::decode_bc1(&all_one).iter().all(|p| *p == [0, 0, 255, 255]));
    let three = [0x1f, 0x00, 0x00, 0xf8, 0xff, 0xff, 0xff, 0xff];
    assert!(bc7::decode_bc1(&three).iter().all(|p| *p == [0, 0, 0, 0]));
    // BC3: alpha 200 everywhere (a0 200, a1 100, indices 0), colours green (0x07e0) and black, indices 0
    let b = [200, 100, 0, 0, 0, 0, 0, 0, 0xe0, 0x07, 0x00, 0x00, 0, 0, 0, 0];
    assert!(bc7::decode_bc3(&b).iter().all(|p| *p == [0, 255, 0, 200]));
}
