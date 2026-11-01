//! Finding a C++ class's vtable in a loaded module by its MSVC RTTI name, without an object of the class: the type
//! descriptor holds the decorated name (`.?AUDXGISwapChain@interposer@sl@@`); a complete object locator (x64:
//! signature 1, then RVAs) points at the descriptor and at itself; the vtable is the word after a pointer to the
//! locator. Used for NVIDIA Streamline's swap chain wrapper (`sl.interposer.dll`, which the game presents through),
//! so the overlay can hook the wrapper's `Present` the way it hooks DXGI's.

use crate::win;

/// One section of a mapped image: (RVA, size).
fn sections(base: usize) -> Option<Vec<(usize, usize)>> {
    let pe = win::read_u32(base + 0x3C)? as usize;
    if win::read_u32(base + pe)? != 0x0000_4550 {
        return None;
    }
    let count = win::read_u16(base + pe + 6)? as usize;
    let opt_size = win::read_u16(base + pe + 20)? as usize;
    let table = base + pe + 24 + opt_size;
    let mut out = Vec::new();
    for i in 0..count.min(96) {
        let s = table + i * 40;
        let vsize = win::read_u32(s + 8)? as usize;
        let rva = win::read_u32(s + 12)? as usize;
        let raw = win::read_u32(s + 16)? as usize;
        out.push((rva, vsize.max(raw)));
    }
    Some(out)
}

/// The image's readable bytes, section by section: (RVA, bytes).
fn read_image(base: usize) -> Vec<(usize, Vec<u8>)> {
    let mut out = Vec::new();
    for (rva, size) in sections(base).unwrap_or_default() {
        if size == 0 || size > 64 << 20 {
            continue;
        }
        let mut buf = vec![0u8; size];
        if win::read_into(base + rva, &mut buf) {
            out.push((rva, buf));
        } else {
            // a section whose tail is not mapped: take what is, page by page
            let mut got = Vec::new();
            for page in (0..size).step_by(0x1000) {
                let mut p = vec![0u8; 0x1000.min(size - page)];
                if !win::read_into(base + rva + page, &mut p) {
                    break;
                }
                got.extend_from_slice(&p);
            }
            if !got.is_empty() {
                out.push((rva, got));
            }
        }
    }
    out
}

/// The primary vtable (offset 0 in the object) of the class named `decorated` in the image mapped at `base`, whose
/// absolute pointers are relative to `pointer_base` (= `base` for a module loaded normally; the preferred image base
/// for a file mapped as an image resource, which is not relocated). Returns the vtable's address in the mapping.
pub fn find_vtable(base: usize, pointer_base: usize, decorated: &str) -> Option<usize> {
    let image = read_image(base);
    let name = {
        let mut n = decorated.as_bytes().to_vec();
        n.push(0);
        n
    };
    // the type descriptor: {vftable ptr, spare, name}
    let mut td_rva = None;
    'find: for (rva, bytes) in &image {
        let mut from = 0;
        while let Some(i) = find(&bytes[from..], &name) {
            let at = from + i;
            if at >= 16 && (rva + at - 16) % 8 == 0 {
                td_rva = Some(rva + at - 16);
                break 'find;
            }
            from = at + 1;
        }
    }
    let td_rva = td_rva? as u32;
    // the complete object locators naming it, offset 0
    let mut cols = Vec::new();
    for (rva, bytes) in &image {
        let mut i = 0;
        while i + 24 <= bytes.len() {
            let w = |k: usize| u32::from_le_bytes(bytes[i + k..i + k + 4].try_into().unwrap());
            if w(0) == 1 && w(12) == td_rva && w(20) as usize == rva + i && w(4) == 0 {
                cols.push(rva + i);
            }
            i += 4;
        }
    }
    // the vtable follows a pointer to the locator
    for col in cols {
        let want = (pointer_base + col) as u64;
        for (rva, bytes) in &image {
            let mut i = 0;
            while i + 16 <= bytes.len() {
                if u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap()) == want {
                    return Some(base + rva + i + 8);
                }
                i += 8;
            }
        }
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let p = hay[i..hay.len() - needle.len() + 1].iter().position(|&b| b == first)?;
        i += p;
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The decorated names of Streamline's interposer classes.
pub const SL_SWAPCHAIN: &str = ".?AUDXGISwapChain@interposer@sl@@";
pub const SL_QUEUE: &str = ".?AUD3D12CommandQueue@interposer@sl@@";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bytes() {
        assert_eq!(find(b"abcabd", b"abd"), Some(3));
        assert_eq!(find(b"abc", b"abcd"), None);
        assert_eq!(find(b"xxab", b"ab"), Some(2));
    }
}
