# Caveman app

A desktop app for Caveman: browse the deck and wheel drop folders, set each deck's effects with live 3D preview, then build the mods and start ReSkate. Tauri 2 (Rust back end) with a Svelte 5 + TypeScript front end and three.js for the preview. First version, 2026-10-21.

| Folder | What it is |
|---|---|
| `src/` | The front end: `App.svelte`, `components/` (library, editor, preview, build log, settings), `lib/` (back-end calls, effect model, `preview/` three.js scene) |
| `src-tauri/` | The back end (`caveman-app` crate, a member of the Caveman workspace): commands in `src/main.rs`, the work in `src/{config,library,art,bake}.rs`, tests in `tests/backend.rs` |
| `mock/` | `library.json`, a stand-in back end for reviewing the UI in a plain browser (`pnpm dev:mock`) |
| `toolchain/` | What the GNU Rust toolchain needs to build Tauri: `fetch-toolchain.ps1` and the `gcc -E` stand-in (see `toolchain/README.md`) |
| `tools/` | `screenshot.mjs`: headless Chrome screenshots that wait until the 3D preview has drawn |

## Running it

Once per machine: `pnpm install` in `app/` (done: about 127 MB of `node_modules`), and the MinGW tools for the GNU toolchain (done: `%LOCALAPPDATA%\Caveman\toolchain`, or run `toolchain\fetch-toolchain.ps1`). Every app build needs those tools on PATH:

```
$env:PATH = "$env:LOCALAPPDATA\Caveman\toolchain\mingw64\bin;$env:PATH"
```

- **The app, for development:** `pnpm tauri dev` in `app/` (Vite on port 5173 plus a debug build of the back end, reloading on edits). Not run overnight (the mock server held the port); the debug exe it builds was run and works.
- **The app, standalone:** `pnpm tauri build --no-bundle` builds `target\release\caveman-app.exe` (22 MB) with the front end inside it; keep `WebView2Loader.dll` beside it (GNU builds load WebView2 through that DLL). Use the Tauri CLI rather than plain `cargo build --release`: without the CLI's `custom-protocol` feature the exe looks for the dev server instead of its own pages. There is no installer yet (`bundle.active` is false).
- **The UI in a browser, no back end:** `pnpm dev:mock`, then open `http://localhost:5173/`. It serves `mock/library.json` and the images it names. URL options for links and screenshots: `?deck=<part of a name>`, `?wheel=<part of a name>`, `?mode=ride`, `?log=1`.
- **Checks:** `pnpm check` (svelte-check and TypeScript, 0 errors and 0 warnings), `pnpm build` (the front end into `dist/`).
- **Back-end tests:** `cargo test --release -p caveman-app -- --ignored` from `Caveman/` (tools on PATH; with `--release` the clean build of 14 decks, a wheel and the shaders takes about 22 s, in a debug build about 6 minutes). They read the game at `E:\Games\ReSkate` and copy the real drop folder's `000` decks into a scratch folder (`CAVEMAN_APP_SCRATCH`, default `%TEMP%\caveman-app-tests`); nothing is written to the game folder. `dump_mock_fixture` rewrites `mock/library.json`.
- **Screenshots:** `node tools/screenshot.mjs <out dir> name=http://localhost:5173/?deck=holo ...` against `pnpm dev:mock`.

In the workspace, `app/src-tauri` is a member but not a default member, so a plain `cargo build` or `cargo test` in `Caveman/` leaves it out (it needs the MinGW tools and the Tauri crates); name it with `-p caveman-app`.

## What it does

- **Library** (left): the deck and wheel drop folders (by default `Caveman Decks` and `Caveman Wheels` in the game folder), in the build's own reading (`caveman_cli::names`), so names, prefixes and suffix effects match what the build makes. Cards show the build's preview thumbnail when there is one (else the PNG), effect chips and a JSON badge for a settings file; groups per subfolder, collapsible; search over names, folders and effects. PNGs the build would skip are listed with the reason.
- **Editor** (right): finish (print, gloss, matte, foil, chrome, glitter, holo with its five patterns, spot), raised print (emboss strength), shader effects (colour cycle with speed, glow with strength and its game factor), board effect (aura with the art's colour or a picked one, or the rainbow trail). **Save settings** writes the deck's `<name>.caveman.json` beside the PNG (validated by `caveman_core::settings` first); **Use suffixes** removes it so the file name's suffixes apply again; Revert drops unsaved changes (Ctrl+S saves). The settings' `name` and the import's `layers` object are kept when saving (neither is editable here yet). A deck named with suffixes opens with those effects shown. Wheels show their colour from the file name (no wheel settings yet).
- **3D preview** (middle), labelled approximate: the deck from `assets/deck_outline.json` with kicked nose and tail and concave, the build's fitted art (512 x 2048, cached per PNG version) on the bottom, grip on top, trucks and wheels (the selected wheel's colour, its PNG on the outer faces). Finishes as roughness and metalness maps, holo as iridescence by view angle under each pattern's mask, emboss as a normal map, glow as emissive light with bloom, cycle as the game's hue rotation (same maths as the shader patch) animated. **Showcase** stands the deck nose up with the bottom to you (the art upright and unmirrored, as the game's customization view shows it; checked with `assets/test_pattern.png`); **Ride** lays it on the ground and rolls it along, showing the aura's light and trails and the rainbow trail (six solid stepped bands, the shader patch's formula). Drag to orbit, scroll to zoom.
- **Build and Play** (header): Build runs `caveman_cli::run` in process, the same build as `Caveman.exe` (incremental, so unchanged decks are skipped), with progress and the log from its JSON events in the bar under the preview. It refuses while `Skate.exe` runs (the header shows the game's state, checked every 4 s). Play starts `ReSkateLauncher.exe` from the game folder. Settings sets the game folder and the drop folders (advanced: another mods folder, another cache).

Where the app keeps its own things: its settings in `%APPDATA%\dev.caveman.app\config.json` (only once saved; until then the defaults), the fitted-art cache in `%LOCALAPPDATA%\dev.caveman.app\art`, WebView2's data in `%LOCALAPPDATA%\dev.caveman.app\EBWebView`. Under the game folder it writes only what Caveman already writes: settings files beside deck PNGs (on Save) and the `Caveman-Decks` / `Caveman-Shaders` mods (on Build).

## Approximate, mock, and not yet

- **The preview approximates the game.** Lighting is a studio environment, not the game's; the holo patterns are drawn to resemble the game's, not ported from its shader; glow uses a gentler curve than the game's `2^(12 x glow) - 1` factor (the real factor would wash out a monitor), and in game the glow deck still showed no glow, so the preview is ahead of the game there; the aura is a point light plus a halo; the rainbow trail's step length (0.32 m) is a guess.
- **Wheels** are shown, not edited (the colour comes from the file name, and there is no wheel settings file yet).
- **Not in v1:** adding, renaming or removing PNGs from the app (use the folder; Refresh rescans), the nyan step count (kept as saved, not shown: in game the step sizes looked the same), running the Python verifiers after a build, an installer and a real icon (the current one is a placeholder).
- **The mock fixture** holds absolute paths from the machine that wrote it (a scratch copy of the drop folder); regenerate it with `dump_mock_fixture` if those folders are gone.
- **Hardening for later:** the webview may read any local file through the asset protocol (`assetProtocol.scope` is `**`, needed for drop folders anywhere) and there is no content security policy.
