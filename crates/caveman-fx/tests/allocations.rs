//! Guardrail 2 of `docs/fx-audit.md`: the DLL allocates GPU memory in a few known places only: `Gpu::new` (once per
//! device, in `overlay.rs` and `board.rs`), `ensure_copies` (once per size and format of the main depth), the video's
//! staging buffers (`video_staging`), through the small `buffer` helpers those files keep (and `board.rs`'s `upload`,
//! which grows its upload buffer once for a larger art); the memory guard's hooks (memguard.rs) only pass the game's
//! own calls through. The staging ring (stagering.rs) makes its heaps through `CreateHeap` by name too: see below.
//! This test reads the crate's source and fails if one of D3D12's resource creation calls appears in any other
//! function, so a new allocation site cannot slip in unread.

use std::path::Path;

const CALLS: [&str; 4] = ["CreateCommittedResource", "CreateHeap", "CreatePlacedResource", "CreateReservedResource"];

/// (file, function) pairs allowed to call the creation functions or the `buffer` helpers.
const ALLOWED: [(&str, &str); 14] = [
    ("overlay.rs", "new"),
    ("overlay.rs", "ensure_copies"),
    ("overlay.rs", "buffer"),
    ("board.rs", "new"),
    ("board.rs", "buffer"),
    ("board.rs", "upload"),
    ("board.rs", "buffer_desc"),
    // the video's staging buffers (video_present): made through memwatch::room_for, grown once per frame shape
    ("board.rs", "video_staging"),
    // the memory guard's hooks pass the game's own calls through (memguard.rs): every function of that file
    ("memguard.rs", "*"),
    // the overlay's dummy swap chain probe (install_inner), the board cam's depth texture and meshes
    ("overlay.rs", "install_inner"),
    ("overlay.rs", "ensure_depth"),
    ("overlay.rs", "ensure_mesh"),
    // the staging ring: its heaps (make_ring, through memwatch::room_for_system) and the game's own placed
    // allocations it answers (intercept names CreatePlacedResource to find the game's slot)
    ("stagering.rs", "make_ring"),
    ("stagering.rs", "intercept"),
];

/// The name of the function a line starts, if it starts one.
fn starts_fn(line: &str) -> Option<String> {
    let t = line.trim_start();
    let t = t.strip_prefix("pub ").unwrap_or(t);
    let t = t.strip_prefix("pub(crate) ").unwrap_or(t);
    let t = t.strip_prefix("unsafe ").unwrap_or(t);
    let t = t.strip_prefix("extern \"system\" ").unwrap_or(t);
    let rest = t.strip_prefix("fn ")?;
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

#[test]
fn gpu_memory_is_allocated_in_two_places_only() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut problems = Vec::new();
    let mut seen = 0;
    for entry in std::fs::read_dir(&src).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut current = String::from("(top level)");
        for (n, line) in text.lines().enumerate() {
            if let Some(name) = starts_fn(line) {
                current = name;
            }
            let code = line.trim_start();
            if code.starts_with("//") {
                continue; // comments may name the calls
            }
            let names_call = CALLS.iter().any(|c| code.contains(c));
            // a call of the `buffer` helper itself (not `scan_buffer(`, `fn buffer(`, `buffer_desc(`)
            let calls_helper =
                code.match_indices("buffer(").any(|(i, _)| i == 0 || !code.as_bytes()[i - 1].is_ascii_alphanumeric() && code.as_bytes()[i - 1] != b'_')
                    && !code.contains("fn buffer(");
            if names_call || calls_helper {
                seen += 1;
                if !ALLOWED.iter().any(|(f, func)| *f == file && (*func == "*" || *func == current)) {
                    problems.push(format!("{file}:{} in fn {current}: {}", n + 1, code.trim()));
                }
            }
        }
    }
    assert!(seen >= 6, "the known allocation sites were not found ({seen}): did the helpers move?");
    assert!(problems.is_empty(), "GPU memory allocated outside Gpu::new and ensure_copies:\n{}", problems.join("\n"));
}
