# Caveman app plan (Rust engine, Svelte/Tauri app)

Decided by Trevor, 2026-10-20: port the engine to **Rust first** (no Python for players), deck settings in **sidecar JSON** files beside each PNG (filename suffixes keep working), and a **3D board preview** from the first version. The Python toolkit (`tools/re/`) was the **reference implementation and test oracle** for the port: every Rust part was checked against it.

**Update (Trevor, 2026-10-21): Rust only, no more parity.** The port is done and verified, so new features are written once, in Rust; the Python builders are frozen (`knowledge/toolkit.md`). The Rust tests keep the recorded Python outputs as fixed fixtures. Python stays only for investigation (the independent verifiers, asset and crash-dump readers) and for one-off prototypes that already exist; a prototype that works in game is built in Rust without a Python twin.

## Architecture

A Cargo workspace in `Caveman/`:

| Crate | What it is |
|---|---|
| `crates/caveman-core` | The engine library: game data readers and writers, encoders, the build pipeline, effects, shader patching. No UI. |
| `crates/caveman-cli` | `caveman` command line (what `caveman.py` does today, plus `--json` output for tools); the launcher (`launcher/`, `Caveman.exe`) calls the core directly instead of Python. |
| `app/` | The Tauri 2 app: Svelte front end (`app/src`), Rust back end (`app/src-tauri`, depends on `caveman-core`). |

The game stays the source of every template: the engine reads the player's own install at build time, as now (never ship game data).

## Settings: sidecar JSON

`Cool Deck.png` + `Cool Deck.caveman.json` (optional). The app writes the JSON; hand-written suffixes (`Cool Deck.holo-frost.spot.png`) still work and seed the settings when no JSON exists. Draft shape:

```json
{
  "version": 1,
  "name": "Cool Deck",
  "finish": { "kind": "holo", "mode": "frost", "spot": false },
  "emboss": { "strength": 4.0 },
  "shader": { "effect": "cycle", "speed": 0.25, "glow": 0.0 },
  "board": { "effect": "aura", "colour": "#00ffff" }
}
```

Wheels get the same treatment later (`Caveman Wheels`, `Name.caveman.json` with the wheel colour).

## Port order (each step verified before the next)

| Step | Rust module | Python reference | Verification |
|---|---|---|---|
| 1 | Oodle (the game's `oo2core_9_win64.dll`, loaded at run time) and CAS blocks | `fb.decode_cas`, `fbw.encode_cas` | decode every block of a few game archives; encode then decode round trips |
| 2 | Superbundle TOC read/write (Huffman names, perfect hash), bundle regions, binary manifests | `fb.py`, `fbw.py` | byte-identical on the game's TOCs and the installed mods (as the Python writers were: 106/107 TOCs, 74/74 manifests) |
| 3 | Native DB (chunk metadata, layout.toc) | `ndb.py` | byte-identical round trips |
| 4 | EBX read/write and editing (the largest part) | `ebx.py` | byte-identical round trip over all game EBX (60,000) and the mods' deck EBX |
| 5 | Bundle reference table, texture headers, BC7 and BC1 encoders | `brt.py`, `bc7.py`, `bc1.py` | BRT byte-identical; encoders: same blocks as Python for the same input (both deterministic) |
| 6 | Thumbnails (decks, metal, wheels) | `thumbs.py`, `newdeck.thumbnail_lighting` | pixel difference against Python's renders |
| 7 | The build: new decks, mixins, holo, board effects (aura, nyan), wheels | `newdeck.py`, `mixins.py`, `vfx.py`, `newwheel.py`, `caveman.py` | **Python's `verify_newdeck.py` / `verify_newwheel.py` must pass on Rust-built mods** (the strongest check: a separate implementation re-reads everything) |
| 8 | Shader patching through the game's `dxcompiler.dll`/`dxil.dll` (COM from Rust) | `dxc.py`, `shaderpatch.py`, `nyanpatch.py`, `shadermod.py` | the patched IR text identical to Python's; programs assemble, validate and sign |
| 9 | CLI and launcher on the Rust core | `caveman.py`, `launcher/` | same decks built from the same drop folder; then in game |

Random parts (GUIDs, resource ids) take a seed in tests so Rust and Python outputs can be compared exactly where it matters.

## The app (after step 7, in parallel with 8 and 9)

- **Library**: the drop folders (decks; wheels later), each deck with its thumbnail, effects and build state; add, rename, remove; subfolder prefixes as today.
- **Editor**: effect controls (finish and holo mode, spot, emboss strength, shader effect and speed, glow strength, board effect and colour, nyan steps), written to the sidecar JSON.
- **3D preview** (three.js): a deck mesh generated from Caveman's measured outline (`assets/deck_outline.json`) and the nose/tail curve fitted for thumbnails, with the art; approximations of gloss, metal, emboss (normal map), holo (iridescence by view angle), glow (emissive with bloom), cycle (animated hue), and a riding mode showing the aura light and the nyan trail. It approximates the game; the in-game look is the truth.
- **Bake and play**: build the mod (only with the game closed; the app checks), launch ReSkate (replacing the `Caveman.exe` wrapper), show the build log and the verifier's result.

## Status

- 2026-10-20: workspace skeleton in place (`Cargo.toml`, `crates/caveman-core` with one file per module, `crates/caveman-cli` stub); crates so far: `png`, `sha1_smol`. Three port agents running in parallel: io (steps 1-3), ebx (step 4), pixels (steps 5-6 pixel half), each with its own target dir under the session scratchpad.
- 2026-10-20 later: **steps 1-6 done and verified** against Python. io: 142/142 TOCs, 20,156/20,156 manifests, 20,134 chunk-metadata and 75 layout.toc round trips, Kraken identical. ebx: 115,371 of 117,809 game docs byte-identical (the rest are not identical in Python either, and Rust writes Python's bytes), 50,971/50,971 compact rewrites and 12/12 build edits identical. pixels: BC7/BC1 0 differing blocks, decoders = Pillow, Lanczos and thumbnails byte-identical; BC7 about 50 ms per deck (Python 1.5 s). **Step 7 (build) done**: Python's verifiers pass with 0 failures on Rust-built mods (drop folder 18,460 checks, calibration 18,685, wheels 17,271, decks + wheels together), same asset names and identical textures/thumbnails as Python's builds; 14 decks in 8.4 s (Python 78 s). **Step 9 done and INSTALLED (2026-10-20 21:21)**: `E:\Games\ReSkate\Caveman.exe` is now the Rust launcher (`launcher/`, built as `caveman-launcher.exe`, icon via `launcher/set_icon.py`): it builds `Caveman-Decks` and, when any deck uses cycle/glow/nyan, `Caveman-Shaders` in process (no Python), then starts ReSkateLauncher.exe. First build 13.8 s (14 decks + shaders); up-to-date launch skips. Verified after install: verify_newdeck 18,460 checks 0 failed; Caveman-Shaders' archive and TOC byte-identical to the Python-built test mod. Retired to the session scratchpad quarantine: the Python-calling Caveman.exe, `Caveman-Shaders-Test`. Sidecar JSON settings live (`docs/settings.md`). The CLI (`caveman.exe`) is a dev/app tool, not installed in the game folder. **The engine port is complete: next is the app** (tooling downloads to approve first).
- 2026-10-20 21:52: installed a new `Caveman.exe` with (1) **incremental builds** (`item_cache.rs`: per-deck/wheel cache under `%LOCALAPPDATA%\Caveman\cache\items`, keyed by the art bytes, effects, names, mod, Skate.exe build and the Caveman exe's own hash; deterministic ids per item; 200-deck library: one deck added 2.8 s vs 35 s clean; incremental output byte-identical to a clean build; verifiers 0 failed) and (2) the **flat nyan trail** (`shader/ribbon_vs.rs` + `tools/re/nyanvspatch.py`: the ribbon's vertex program `6cfc6613-...`, for nyan ribbons only: horizontal width, 4 cm pull toward the camera; Rust shader mod byte-identical to Python's). Awaiting Trevor's in-game check. **Step 8 (shader) done**: all 7 programs (disassembly, patched IR, signed containers) and the whole shader mod byte-identical to Python's (only the build timestamp differs, pinnable); `caveman_core::shader::build_shader_mod`.
- App tooling: sizes checked (Tauri CLI 16 MB, three.js 20 MB, Svelte/Kit/Vite/API 8 MB, their deps about 30 MB, Tauri crates 60 to 80 MB, build output 2 to 4 GB). **Trevor: wait for the engine port before downloading it and starting the app.** Later (2026-10-20): **download approved** ("go ahead and download the tooling needed"), after a round of feature research first (live control panel / music-reactive shaders / live shader editing; state-triggered board effects for grinds, manuals, tricks; a banner trail that rolls with the board), since live control would change the app's design (talking to the running game).

## Tooling to install (sizes to confirm before downloading)

- Tauri CLI (npm `@tauri-apps/cli`), the Tauri and three.js packages, and the Rust crates they pull in.
- Tauri on Windows normally builds with the MSVC toolchain (Visual Studio Build Tools, a multi-gigabyte download); this machine has the GNU toolchain (as the launcher uses) and the MSVC target without its linker. First try Tauri on GNU; fall back to Build Tools only if needed (ask first).

## Out of scope for v1

Pack mode (Thunderstore packs), the store, the installer, the Skate 3-style deck builder (layers, stickers, text). They build on the app later (see `backlog.md`).
