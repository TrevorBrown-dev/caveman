//! Fixture-free tests of the shader layer: float literals, the text matchers (against values Python's `re` gives),
//! the program wrapper, SHA-256. Expected values were computed with the Python toolkit.

use caveman_core::shader::{ir, marker_pixels, program, ribbon, shadermod};

#[test]
fn float_literals_match_python() {
    for (x, want) in [
        (0.9, "0x3FECCCCCC0000000"),
        (0.1, "0x3FB99999A0000000"),
        (6.2831855, "0x401921FB60000000"),
        (1.0 / 3.0, "0x3FD5555560000000"),
        (2.0 / 12.0, "0x3FC5555560000000"),
        (0.0371, "0x3FA2FEC560000000"),
        (-0.5, "0xBFE0000000000000"),
        (0.0001, "0x3F1A36E2E0000000"),
        (12.0, "0x4028000000000000"),
    ] {
        assert_eq!(ir::f32_lit(x), want, "{x}");
    }
    let lin: Vec<String> = [0u8, 51, 102, 153, 255].iter().map(|&c| ir::f32_lit(ribbon::lin(c))).collect();
    assert_eq!(lin, ["0x0000000000000000", "0x3FA0F31BA0000000", "0x3FC101D440000000", "0x3FD4631200000000", "0x3FF0000000000000"]);
}

#[test]
fn line_anchored_crosses_blank_lines_like_python() {
    let target = "call void @dx.op.storeOutput.i32(i32 5, i32 3, i32 0, i8 0, i32 %9)";
    let t = format!("a\n  x = 1\n\n   {target}  ; c\n");
    // Python: re.finditer(rf'^(\s*){re.escape(target)}', t, re.M) -> start 10, group(1) '\n   '
    assert_eq!(ir::line_anchored(&t, target), vec![(10, 14)]);
    assert_eq!(&t[10..14], "\n   ");
    assert!(ir::line_anchored(&format!("ab  {target}"), target).is_empty());
}

#[test]
fn matchers_read_disassembly_lines() {
    let text = "\
; Resource Bindings:
;
;                                   texture     f32          2d      T0             t1     1
;                                   texture     f32          2d      T1             t0     1
;                                   texture     f32          2d      T2            t33     1
;                                   texture     u32          2d      T3             t2     1
  %4 = call %dx.types.Handle @dx.op.createHandle(i32 57, i8 0, i32 1, i32 0, i1 false)  ; CreateHandle
  %h.x = call %dx.types.Handle @dx.op.createHandle(i32 57, i8 2, i32 0, i32 4, i1 false)
  %15 = call float @dx.op.loadInput.f32(i32 4, i32 1, i32 0, i8 2, i32 undef)
  %20 = call %dx.types.CBufRet.f32 @dx.op.cbufferLoadLegacy.f32(i32 59, %dx.types.Handle %3, i32 459)
  %21 = extractvalue %dx.types.CBufRet.f32 %20, 3
  call void @dx.op.storeOutput.f32(i32 5, i32 1, i32 0, i8 2, float %889)  ; StoreOutput
  call void @dx.op.storeOutput.i32(i32 5, i32 3, i32 0, i8 3, i32 %710)
!dx.valver = !{!7}
!7 = !{i32 1, i32 10}
";
    assert_eq!(ir::art_texture(text).unwrap(), "t0");
    let h = ir::handles(text).unwrap();
    assert_eq!(ir::handle(&h, "t0").unwrap(), "%4");
    assert_eq!(ir::handle(&h, "cb4").unwrap(), "%h.x");
    assert_eq!(ir::inputs(text), vec![((1, 2), "%15".to_string())]);
    assert_eq!(ir::cbuf_loads(text), vec![("%20".to_string(), "459".to_string())]);
    assert_eq!(ir::cbuf_w(text), vec![("%21".to_string(), "%20".to_string())]);
    let stores = ir::deck_stores(text);
    assert_eq!(stores.len(), 2);
    assert_eq!(stores[0].0, (1, 2));
    assert_eq!(stores[0].1.operand, "%889");
    assert_eq!(stores[1].1.kind, "i32");
    assert_eq!(ir::valver(text), Some("7"));
    assert!(ir::set_valver_17(text, "7").ends_with("!7 = !{i32 1, i32 7}\n"));
    assert_eq!(ir::py_list(&["%1", "%2"]), "['%1', '%2']");
}

#[test]
fn wrap_round_trips_the_resource_layout() {
    // a fake program: 0x20 header, an 0xA8-byte RC2S block ending with the DXBC size, a 40-byte DXBC
    let mut dxbc = b"DXBC".to_vec();
    dxbc.resize(40, 7);
    dxbc[24..28].copy_from_slice(&40u32.to_le_bytes());
    let mut original = vec![1u8; 0x20];
    let mut pre = vec![2u8; 0xA8];
    pre[0xA4..].copy_from_slice(&40u32.to_le_bytes());
    original.extend_from_slice(&pre);
    original.extend_from_slice(&dxbc);
    let mut bigger = dxbc.clone();
    bigger.resize(77, 9);
    bigger[24..28].copy_from_slice(&77u32.to_le_bytes());
    let (payload, meta) = program::wrap(&original, &bigger).unwrap();
    let end: usize = 0x20 + 0xA8 + 77;
    let data_size = end.div_ceil(16) * 16;
    assert_eq!(payload.len(), data_size + 8);
    assert_eq!(&payload[data_size..], &[0x10, 0, 0, 0, 0x18, 0, 0, 0]);
    assert_eq!(u32::from_le_bytes(meta[..4].try_into().unwrap()) as usize, data_size);
    let back = program::read_bytecode_resource(&payload).unwrap();
    assert_eq!(back.dxbc, &bigger[..]);
    assert_eq!(back.blob_size as usize, 0xA8 + 77);
    assert_eq!(back.data_size as usize, data_size);
}

#[test]
fn sha256_vectors() {
    let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(hex(shadermod::sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hex(shadermod::sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(hex(shadermod::sha256(&[b'a'; 1000])), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    // the SHA-instruction path (when the CPU has it) against the portable rounds, every length across block edges
    let data: Vec<u8> = (0..5000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    for n in (0..300).chain([1023, 1024, 1025, 4999, 5000]) {
        assert_eq!(shadermod::sha256(&data[..n]), shadermod::sha256_portable(&data[..n]), "length {n}");
    }
}

#[test]
fn marker_layout() {
    let m = marker_pixels(64, 32, 128);
    assert_eq!(m[0], (0, 0, 16, 16, (255, 0, 255)));
    assert_eq!(m[2], (32, 0, 48, 16, (64, 32, 128)));
}
