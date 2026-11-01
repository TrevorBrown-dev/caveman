# Working in Caveman

Guidance for coding agents and the people who run them. Everything here applies to human contributors too; `CONTRIBUTING.md` has the workflow, this file has the rules and the map. Read it before changing anything.

## What Caveman is

Caveman turns a folder of PNGs into mods for **ReSkate**, the fan-made mod platform for EA's *skate.*: decks, wheels, grip tape and grip patterns, built from the player's own game install into ordinary ReSkate mods. Around that build sit a launcher (`Caveman.exe`), a desktop app (Tauri 2, Rust, Svelte 5, three.js), a command line, and an optional **FX DLL** that `caveman-trainer` loads into the running game for live things: a picture or video on the ridden board, soundbars, and a see-through board cam window for streaming. Windows only, like the game.

## The map

| Path | What it is |
|---|---|
| `crates/caveman-core` | The engine: readers and writers for the game's formats (TOC, CAS, bundles, EBX, textures, meshes, the bundle reference table), the BC7 and BC1 encoders, the item build (`build.rs`), thumbnails, the board model, shader patching, imports |
| `crates/caveman-cli` | `caveman`: the drop folders, name rules, the incremental build and its fingerprint, packs for Thunderstore, the item selection file |
| `crates/caveman-fx` | `caveman_fx.dll`: everything that runs inside the game (hooks, the board texture writer, the video ring, the overlay, the pose reader's shared parts, the memory guard and staging ring) |
| `crates/caveman-trainer` | `caveman-trainer`: injects and drives the DLL over a named pipe, reads the game from outside (`pose`, `probe`), and the diagnostics |
| `launcher/` | `Caveman.exe`: builds what changed, starts ReSkate's launcher, attaches the DLL when asked |
| `app/` | The desktop app; `app/README.md` describes it. `app/src-tauri` is the back end crate (a workspace member, not a default member: it needs the Tauri toolchain). `app/src/lib/widgets.ts` and `app/src/components/widgets/` are the widget system (the overlay windows: board cam, speed meter), `docs/widgets.md` explains it and how to add one |
| `docs/` | Design notes and working records (`deck-format.md` is the format write-up; `board-cam.md`, `grips.md` and the rest are the stories of how each part was worked out); `docs/index.html` is the download page served by GitHub Pages |
| `assets/` | Templates and outlines Caveman ships (ours, not the game's) |
| `tools/re`, `caveman.py`, `prototypes/` | The frozen Python research toolkit from before the Rust port (2026-10-21). Kept for the record; nothing builds from it and nothing new goes into it |
| `CHANGELOG.md` | What a player notices, one line each; the app's front page shows the newest entry |

The reverse-engineering knowledge base this was written against (`knowledge/` in the maintainer's workspace) is not in this repository. Some docs mention its files; treat those mentions as citations, not links, and do not add new references to it.

## Hard rules (each one cost a crash, a hang or a legal line)

- **Never commit anything derived from the game's files.** Built mods, textures, previews, thumbnails and statistics rendered from game assets, `.cas`, `.toc`, `initfs`: all of it is regenerated from the player's own install at build time and none of it may be in the repository or a package. `.gitignore` lists the kinds; extend it when a new kind of generated file appears. The game's data belongs to EA; Caveman ships only its own code and its own art.
- **Never stream a cosmetic texture from a mod: write it whole** (first mip 0, the full chain in the bundle), as ReSkate Studio does. Streamed from a mod, a texture crashed the game at spawn or left the load waiting forever. Compress chunks with Oodle Kraken, as the game does.
- **New resource ids must be odd** (bit 0 set), like every one the game ships; an even one crashed the game at boot.
- **A re-encoded texture needs its header updated**: bytes 0-7 are the encoded offsets of mips 1 and 2; stale values crashed and hung the game.
- **Test the paths a player actually takes**: an item equipped at launch loads differently (streamed) from one equipped in the menu.
- **The FX DLL calls engine functions only on the engine thread** (through its client-tick relay, `caveman-fx/src/tick.rs`). A game function called from the DLL's own thread read a per-thread engine context it did not have and killed the game. GPU resources written on one queue and read on another need a fence wait across them.
- **Nothing of ours ever blocks a game thread**, not for a millisecond: no waits, sleeps, retries or file writes on a thread we hooked, and locks taken from a hook with `try_lock` (drop the record, count it). The game's watchdog kicks the player to the menu after stalls, and a guard that held the game at a commit wall once froze the PC.
- **Nothing slow on the thread that feeds a 60 Hz stream, and nothing blocking on the Tauri app's main thread.** A plain (non-`async`) Tauri command runs on the main thread, which also delivers every event to the windows; mark commands that spawn programs or touch files `#[tauri::command(async)]`.
- **Port from ReSkate's own source rather than guessing**, and verify every reader and writer by byte-identical round trips over real files. ReSkate is GPL-3.0; so is Caveman.

## Building and checking

- Rust stable, MSVC (`rust-toolchain.toml` picks the host's stable; the linker comes from Visual Studio Build Tools, which the maintainer's PC and CI have). The GNU toolchain also builds everything, with the MinGW tools from `app/toolchain/fetch-toolchain.ps1` on PATH for the app; keep it working, but MSVC is what ships.
- `cargo build --release` builds the command line, the trainer, the DLL and `Caveman.exe` (the default members). The app: `pnpm install` once, then `pnpm tauri build --no-bundle` in `app/` (never plain `cargo build` for the app: without the Tauri CLI's `custom-protocol` feature the exe looks for a dev server instead of its own pages). Releases are a zip of the built files with a README.txt (`.github/scripts/assemble.ps1`), unzipped into the ReSkate folder by hand; there is no installer on purpose (a standalone exe that drops a DLL beside a game is the wrong shape of thing to ask players to trust).
- Before every push, exactly what CI runs: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build -p caveman-fx` (the two D3D12 host tests load `target\debug\caveman_fx.dll`, which `cargo test` links against but does not write out; a stale one fails them with "unknown field" errors from the pipe), `cargo test --workspace`, and `pnpm check` in `app/` when the front end changed. The clippy lints allowed in `Cargo.toml` are the only ones to be silenced; fix the rest.
- Tests that need the game are `#[ignore]` and look for it at `E:\Games\ReSkate` (read only); run them with `cargo test -- --ignored` on a machine that has it. CI has no game and no GPU, so a test that needs either must be ignored or must skip itself cleanly.
- Installing a test mod: check the game is not running first, install as a new folder under `Mods`, and remove by moving the folder out rather than deleting it.

## Style

- Rust: `rustfmt.toml` (wide lines); comments say why, in sentences; one idea per line. Prefer the shape the surrounding file uses. Unsafe code carries a `// SAFETY:` line that says what makes it sound.
- TypeScript and Svelte: template literals, never string concatenation; `pnpm check` must report zero errors and zero warnings.
- Prose (docs, changelog, comments, commit messages): plain sentences; two hyphens `--` where a dash would go, never an em dash; Markdown paragraphs and list items on one line each, with a blank line before and after lists, tables and code blocks; a literal dollar sign in Markdown as `\$`.
- A change a player would notice gets a line in `CHANGELOG.md` under the current version.
- Commit messages: a short summary line, then the why. Agents add a `Co-Authored-By` line for themselves.

## Working with the game and the DLL

- Default game folder `E:\Games\ReSkate` (ReSkate 2.1.3, `Skate.exe` build 25414733). Every address, hook and stamp in `caveman-fx` is for that build; `game::verify_exe` refuses others. A game update means re-checking them (`docs/` records how each was found).
- The DLL goes into the game only through `caveman-trainer attach` or `inject`; it waits until the game is in a world with a camera before it hooks anything (`worker.rs`), because hooking during startup ended the game twice.
- `caveman-trainer unload` removes the DLL from a running game; no restart needed.
- Read-only looks at the running game (`probe`, `pose`, `status`) are safe while someone plays. Anything that writes into the game is not: say what you are about to do.
