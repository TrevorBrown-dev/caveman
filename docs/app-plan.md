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

- 2026-10-22: **the Live panel** (a Decks | Live switch in the header; `components/Live.svelte`, `lib/live.ts`, `src-tauri/src/live.rs`): The Caveman in the running game through `caveman-trainer.exe` run as a hidden child beside the app (or `Config.trainer`): inject and unload, ride the selected deck, finishes, the holo's pattern, colour and amount, the tint, the effects, and video from OBS's virtual camera or a file as a child kept until Stop (its lines as `live-video` events; a stop event the producer watches). `pnpm check` clean, built with `pnpm tauri build --no-bundle`; its first run in the app itself is pending. Details: `app/README.md`, the mechanisms `knowledge/live-board.md`.

### Live panel

- 2026-10-22, later: **the Live panel made the full GUI for The Caveman, and run in the app.** Added: any picture on the board (drop a PNG or JPG on the window through Tauri's own file drop, the webview's file picker, or a path; non-PNGs made PNGs in `%LOCALAPPDATA%\dev.caveman.app\live` first, the trainer reading PNG only); the library as step one ("Click a deck, Put it on the board", the selected deck's thumbnail in the panel, the deck on the board marked in the library); animated decks (GIF, video) played on the board as video, a GIF's first frame as a still; the board cam (`pip` on or off, size, corner, view, art, mirror, flip) and the bars (`bars` on or off, source, track, window, floor, release, rise, fall, gain, colour, count), both synced from the DLL's settings. Robustness: a result bar and a log with each action's command line, error toasts, controls locked while a command runs, a poll that never overlaps itself and reads The Caveman's 9 MB status only at open, after a change and every 30 s (the DLL builds it under the locks its render hooks take), a 90 s cap per command, the video producer in a job object with its ffmpeg (Stop: stop event, the job ended after 3 s, then `board video off`; also on app exit). Fixed: the Holo finish sent `board finish holo-pearl`, which the trainer rejects. Test hooks: `CAVEMAN_LIVE_DRY=1` (only read-only trainer commands run) and `CAVEMAN_DEVTOOLS_PORT` (WebView2's DevTools port; the main window is now made in `setup`).
- Verified: `pnpm check` 0 errors 0 warnings; `pnpm tauri build --no-bundle`; 5 back-end unit tests (`cargo test --release -p caveman-app --lib`, including the job object ending a child's child); the real app driven over DevTools against the running game: the read-only state right (DLL pid and phase, The Caveman ridden, `Biggie Cheese.png` on the board and marked in the library, the bars' and board cam's live settings), and in a dry run every write control produced the expected command line, a JPG and a picked PNG were staged and sent, simulated drops of a JPG and a video did the right thing. **Not tried: any write in game** (the owner was playing); real drags from Explorer.

## Tooling to install (sizes to confirm before downloading)

- Tauri CLI (npm `@tauri-apps/cli`), the Tauri and three.js packages, and the Rust crates they pull in.
- Tauri on Windows normally builds with the MSVC toolchain (Visual Studio Build Tools, a multi-gigabyte download); this machine has the GNU toolchain (as the launcher uses) and the MSVC target without its linker. First try Tauri on GNU; fall back to Build Tools only if needed (ask first).

- 2026-10-30: **the launcher (0.2.0)**: the app took ReSkate's launcher's shape at Trevor's ask (a front page with the status and what's new, Plugins, Studio, Play; a Console in the corner). Studio = the old three panes plus a switch per item (`.caveman-off.json` per drop folder, honoured by the CLI and `Caveman.exe`), the folder and template buttons, and Packs (`caveman pack`: a Thunderstore-ready zip and folder, "Generated with Caveman" in the README and the zip's comment). Plugins = the OBS link, the board cam window and the soundbars, each with a start-with-the-game switch; the FX DLL goes in by itself whenever the game runs while the app is open and the switch is on. Particle effects parked. Details: `app/README.md`.

## Out of scope for v1

The store, the installer, the Skate 3-style deck builder (layers, stickers, text). They build on the app later (see `backlog.md`). Pack mode arrived on 2026-10-30.
