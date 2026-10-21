//! CAS blocks and Oodle against the Python toolkit (fb.decode_cas, fbw.encode_cas, fbw.encode_cas_raw,
//! fbw.block_offset).
mod io_common;

use std::path::Path;

use caveman_core::bundle::FileLoc;
use caveman_core::cas;
use io_common::*;

#[test]
fn raw_blocks_match_python() {
    assert_eq!(hex(&cas::encode_raw(b"hello")), "000000050070000568656c6c6f");
    assert_eq!(hex(&cas::encode_raw(b"")), "0000000000700000");
    let big: Vec<u8> = (0..(cas::BLOCK * 2 + 5)).map(|i| (i * 7 % 251) as u8).collect();
    let enc = cas::encode_raw(&big);
    assert_eq!(cas::block_starts(&enc), vec![0, 8 + cas::BLOCK, 16 + 2 * cas::BLOCK]);
    // stored blocks decode without Oodle
    assert_eq!(cas::decode_bytes(&enc, Path::new("nowhere")).unwrap(), big);
    assert_eq!(cas::block_offset(&enc, cas::BLOCK).unwrap(), 8 + cas::BLOCK);
    assert!(cas::block_offset(&enc, 5).is_err());
    assert_eq!(cas::block_containing(&enc, 5).unwrap(), 0);
    assert_eq!(cas::block_containing(&enc, 2 * cas::BLOCK + 4).unwrap(), 16 + 2 * cas::BLOCK);
}

#[test]
fn truncated_data_is_an_error() {
    let enc = cas::encode_raw(b"hello world");
    assert!(cas::decode_bytes(&enc[..enc.len() - 1], Path::new("nowhere")).is_err());
    assert!(cas::decode_bytes(&enc[..5], Path::new("nowhere")).is_err());
}

/// Real blocks of the game decode to what fb.decode_cas gives (both stored and Kraken), and encoding with the game's
/// Oodle writes the same bytes as fbw.encode_cas.
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and the Python fixtures"]
fn game_blocks_match_python() {
    let game = game();
    let decode_rows = rows("cas.tsv");
    let mut kraken = 0;
    for r in &decode_rows {
        let loc = FileLoc {
            patch: false,
            install: r[0].parse().unwrap(),
            archive: r[1].parse().unwrap(),
            offset: r[2].parse().unwrap(),
            size: r[3].parse().unwrap(),
        };
        let (dec, codes) = cas::decode(&game.raw(&loc).unwrap(), game.root()).unwrap();
        assert_eq!(sha(&dec), r[4]);
        assert_eq!(dec.len().to_string(), r[5]);
        let codes_text = codes.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",");
        assert_eq!(codes_text, r[6]);
        kraken += codes.iter().filter(|&&c| c == cas::KRAKEN).count();
    }
    eprintln!("decoded {} game files as Python does ({kraken} Kraken blocks)", decode_rows.len());

    let enc_rows = rows("encode.tsv");
    for r in &enc_rows {
        let loc = FileLoc {
            patch: false,
            install: r[0].parse().unwrap(),
            archive: r[1].parse().unwrap(),
            offset: r[2].parse().unwrap(),
            size: r[3].parse().unwrap(),
        };
        let dec = game.decode(&loc).unwrap();
        assert_eq!(sha(&dec), r[4]);
        let enc = cas::encode(&dec, game.root()).unwrap();
        assert_eq!(sha(&enc), r[5], "Kraken encode differs from fbw.encode_cas");
        assert_eq!(enc.len().to_string(), r[6]);
        assert_eq!(sha(&cas::encode_raw(&dec)), r[7]);
        assert_eq!(cas::decode_bytes(&enc, game.root()).unwrap(), dec);
    }
    eprintln!("encoded {} game payloads exactly as fbw.encode_cas and encode_cas_raw", enc_rows.len());
}

/// Encode then decode round trips with the game's Oodle, for sizes around block boundaries and data that does not
/// compress (stored fallback).
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate (its oo2core DLL)"]
fn kraken_round_trips() {
    let root = Path::new(GAME_ROOT);
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut noise = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    };
    let mut cases = 0;
    for n in [0usize, 1, 100, cas::BLOCK - 1, cas::BLOCK, cas::BLOCK + 1, 3 * cas::BLOCK + 17] {
        let text: Vec<u8> = (0..n).map(|i| b"caveman deck "[i % 13]).collect();
        let random = noise(n);
        for data in [text, random] {
            let enc = cas::encode(&data, root).unwrap();
            assert_eq!(cas::decode_bytes(&enc, root).unwrap(), data, "size {n}");
            let blocks = cas::blocks(&enc);
            assert_eq!(blocks.len(), n.div_ceil(cas::BLOCK).max(1));
            for b in &blocks {
                assert!(b.compression == cas::KRAKEN || b.compression == cas::STORED);
                if b.compression == cas::KRAKEN {
                    assert!(b.encoded < b.decoded);
                }
            }
            cases += 1;
        }
    }
    eprintln!("Kraken encode/decode round trips: {cases}");
}
