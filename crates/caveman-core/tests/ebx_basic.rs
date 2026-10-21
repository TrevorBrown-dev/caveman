//! EBX pieces that need no game data (the corpus tests in ebx_corpus.rs and ebx_edits.rs do).

use caveman_core::ebx::{align_up, Guid};

#[test]
fn guid_text_round_trips_in_the_native_layout() {
    // ebx.py: Guid('2259d419-387f-b1ba-30fc-c5140100b012') is uuid.UUID(...).bytes_le
    let g = Guid::parse("2259d419-387f-b1ba-30fc-c5140100b012").unwrap();
    assert_eq!(g.0, [0x19, 0xd4, 0x59, 0x22, 0x7f, 0x38, 0xba, 0xb1, 0x30, 0xfc, 0xc5, 0x14, 0x01, 0x00, 0xb0, 0x12]);
    assert_eq!(g.to_string(), "2259d419-387f-b1ba-30fc-c5140100b012");
    assert!(Guid::parse("not a guid").is_err());
}

#[test]
fn random_guids_are_version_4() {
    let a = Guid::new_random();
    let b = Guid::new_random();
    assert_ne!(a, b);
    let be = a.to_be();
    assert_eq!(be[6] >> 4, 4);
    assert_eq!(be[8] & 0xC0, 0x80);
}

#[test]
fn align_up_matches_python() {
    assert_eq!(align_up(13, 16), 16);
    assert_eq!(align_up(16, 16), 16);
    assert_eq!(align_up(5, 1), 5);
    assert_eq!(align_up(5, 0), 5);
}
