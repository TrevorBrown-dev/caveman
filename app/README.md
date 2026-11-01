# Caveman app

Caveman's desktop app, since 0.2.0 (2026-10-30) a launcher in the shape of ReSkate's own: a front page with the game's state and what's new, and three ways in: **Plugins** (the OBS link, the board cam window, the soundbars, through the FX DLL), **Studio** (the deck and wheel library with a switch per item, the editor, the 3D preview, the build, the templates, and **Packs**, the mod generator for creators) and **Play** (build what changed, start ReSkate's launcher, put the FX DLL in once the game is up). A **Console** for every `caveman-trainer` command sits in the corner. Tauri 2 (Rust back end) with a Svelte 5 + TypeScript front end and three.js for the preview. First version 2026-10-21; the launcher shape 2026-10-30.

| Folder | What it is |
|---|---|
| `src/` | The front end: `App.svelte` (the shell: the front page, the pages over it, the FX DLL's life), `components/` (`Home`, `Plugins`, `Studio`, `Packs`, `Console`, `Live` (the plugins' controls), `Library`, `Editor`, `PreviewPane`, `BuildLog`, `SettingsDialog`, `BoardCam` (the see-through window's page)), `lib/` (back-end calls, the effect model, the console's command tree, the changelog, `preview/` three.js scene) |
| `src-tauri/` | The back end (`caveman-app` crate, a member of the Caveman workspace): commands in `src/main.rs`, the work in `src/{config,library,art,bake,live}.rs`, tests in `tests/backend.rs` |
| `public/` | Static files; `background.jpg` is the front page's picture when there is one (see below) |
| `mock/` | `library.json`, a stand-in back end for reviewing the UI in a plain browser (`pnpm dev:mock`) |
| `toolchain/` | What the GNU Rust toolchain needs to build Tauri: `fetch-toolchain.ps1` and the `gcc -E` stand-in (see `toolchain/README.md`) |
| `tools/` | `screenshot.mjs`: headless Chrome screenshots that wait until the 3D preview has drawn |

## Running it

Once per machine: `pnpm install` in `app/` (about 127 MB of `node_modules`). The workspace's `rust-toolchain.toml` picks the host's stable toolchain, MSVC on most machines (Visual Studio Build Tools for the linker), which needs nothing else. On the GNU toolchain the app also needs the MinGW tools (`%LOCALAPPDATA%\Caveman\toolchain`, or run `toolchain\fetch-toolchain.ps1`) on PATH for every build:

```
$env:PATH = "$env:LOCALAPPDATA\Caveman\toolchain\mingw64\bin;$env:PATH"
```

- **The app, for development:** `pnpm tauri dev` in `app/` (Vite on port 5173 plus a debug build of the back end, reloading on edits).
- **The app, standalone:** `pnpm tauri build --no-bundle` builds `target\release\caveman-app.exe` with the front end inside it. A GNU build also needs `WebView2Loader.dll` beside it (it loads WebView2 through that DLL; MSVC builds do not). Use the Tauri CLI rather than plain `cargo build --release`: without the CLI's `custom-protocol` feature the exe looks for the dev server instead of its own pages. There is no installer yet (`bundle.active` is false). The exe cannot be rebuilt while it runs (the linker cannot overwrite it).
- **The UI in a browser, no back end:** `pnpm dev:mock`, then open `http://localhost:5173/`. It serves `mock/library.json` and the images it names; the plugins' controls run nothing there. URL options: `?page=plugins|studio|console` opens a page.
- **Checks:** `pnpm check` (svelte-check and TypeScript, 0 errors and 0 warnings), `pnpm build` (the front end into `dist/`).
- **Back-end tests:** `cargo test --release -p caveman-app --lib` (the Live back end's unit tests, the config's). `cargo test --release -p caveman-app -- --ignored` from `Caveman/` (tools on PATH) builds against the game at `E:\Games\ReSkate` into a scratch folder (`CAVEMAN_APP_SCRATCH`); nothing is written to the game folder. `dump_mock_fixture` rewrites `mock/library.json`.
- **Driving the real app from a script:** `CAVEMAN_DEVTOOLS_PORT=9333` opens WebView2's DevTools port for every window of the app (the board cam window too: WebView2 refuses a second window made with other browser arguments than the first, error 0x8007139F, so the port goes on all of them). `CAVEMAN_LIVE_DRY=1` makes the plugins a dry run: only the read-only trainer commands run, the others are only logged. Start the exe from bash with both set (`CAVEMAN_DEVTOOLS_PORT=9333 CAVEMAN_LIVE_DRY=1 ./caveman-app.exe`).

In the workspace, `app/src-tauri` is a member but not a default member, so a plain `cargo build` or `cargo test` in `Caveman/` leaves it out (it needs the MinGW tools and the Tauri crates); name it with `-p caveman-app`.

## The front page

Laid out like ReSkate's launcher (`components/Home.svelte`): the brand and the version top left, a **Status** tile (the game folder, the skate. build against the one Caveman knows, ReSkate's version from `ReSkateLauncher.exe`'s version resource, whether the mods on disk are what a build would write now, the FX DLL and trainer installed, how many decks and wheels are on), a **What's new** tile (the newest entry of `Caveman/CHANGELOG.md`, bundled at build time by `lib/changelog.ts`), **Changelogs** and **Console** buttons bottom left, and on the right **Settings**, **Plugins**, **Studio** and **Play**.

**The background picture:** put a JPEG at `app/public/background.jpg` and rebuild; until then a dark gradient shows. The window is 1440 x 840 (12:7, like ReSkate's launcher); the picture is drawn with `cover`, so make it **2880 x 1680** (12:7, twice the window for high-DPI screens) and keep the subject away from the left third and the bottom right, where the tiles sit. Any 16:9 picture also works (it is cropped a little at the sides).

**Play** builds what is switched on (incremental, as `Caveman.exe` does), starts `ReSkateLauncher.exe`, and, when the Plugins page's switch says so, starts `caveman-trainer attach` detached: it waits for the game with ReSkate loaded, puts the FX DLL in and switches the staging ring on (`%LOCALAPPDATA%\Caveman\fx\attach.log`). The same attach runs when the app sees the game running without the DLL, however the game was started, once per game session; and once the DLL answers, the plugins marked **with the game** are started (the bars switched on, the OBS link's video started, the board cam window opened), also once per session.

## Studio

The library on the left (decks and wheels in the drop folders, by default `Caveman\Decks` and `Caveman\Wheels` in the game folder (one `Caveman` folder beside the game holds everything of Caveman's but the exe: decks, wheels, imports, packs, templates, the archive, the FX DLL and the trainer; the folders from before 2026-10-30 move into it on the first run), in the build's own reading, with a **switch per item**: off takes it out of the build and the game at the next Play; the switches are kept in `.caveman-off.json` at the top of each drop folder, which `Caveman.exe` and the `caveman` command honour too), the 3D preview in the middle, the editor on the right (finish, raised print, shader effects, the board effect: aura or the rainbow trail; Save writes the deck's `<name>.caveman.json`), the build's log under the preview. The bar above: **Library | Packs**, buttons to open the deck and wheel folders in Explorer, **Get the templates** (writes the deck template at twice the game's size, the plain outline and the wheel template, with a README, into `<game>\Caveman Templates` and opens it), Refresh and **Build now** (refused while the game runs).

**Packs** (`components/Packs.svelte`, back end `pack_build` running `caveman_cli::pack`): choose decks and wheels, give the pack an owner (your Thunderstore team), a name, a version, a description, a website and an icon, and **Generate**: a mod folder `Owner-Name` and `Owner-Name-1.0.0.zip` in `<game>\Caveman Packs` (or the folder given) with `manifest.json`, `icon.png`, `README.md` (ending "Generated with Caveman") and `CHANGELOG.md` at the top and the mod's files beside them, as ReSkate's docs on Thunderstore packages describe. A pack needs nothing of Caveman to play and is apart from the personal `Caveman-Decks` (its decks get their own ids). Decks with shader effects (cycle, glow, the rainbow trail, animation) need the Caveman-Shaders mod, which a package cannot carry: they come out plain and the result names them. The form is remembered in the browser's storage.

## Plugins

`components/Plugins.svelte`: a column with the **FX DLL** block (in the game or not, the switch to put it in when the game starts, the staging ring's size, a button to put it in now) and a card per plugin with a **with the game** switch (start it by itself once the DLL answers); the chosen card's controls on the right (`components/Live.svelte` with `show`):

- **OBS link:** the library (pick a deck for the board), the picture on the board (a deck, any picture dropped on the window or chosen, a colour behind see-through parts), the finish, the holo pattern, colour and amount, the tint, and **Video**: OBS Studio's virtual camera or a file on the ridden board (`board video`, run as a child in a job object with its ffmpeg; Stop puts the kept art back). Equip The Caveman deck (in the Caveman-FX mod) in the game first.
- **Board cam:** the see-through, frameless, always-on-top window ("Caveman board cam", for OBS's window capture) with the board turning as it does under the skater, from the pose the trainer reads outside the game (`pose`). Size, the viewpoint (from below shows the art, from above the grip), zoom (1 fits the whole board whichever way it turns), turns with the camera or the world, the selected deck's art, mirror and nose. In the window itself: drag to turn the view, scroll to zoom (the sliders follow, through a `boardcam-view` event), move it by the strip at its top, Escape closes it. The view is kept in the plugins' settings.
- **Soundbars:** the equaliser bars on the deck (on or off, source, track, window, floor, release; under More rise, fall, gain, colour, count), read from and written to the DLL's own settings.

Particle effects are parked (2026-10-30): the Effects list and the FX test decks are out of the app and the default Caveman-FX build (`caveman fx --test-decks` brings the decks back) until their tooling is ready.

## Console

`components/Console.svelte` with the command tree in `lib/console.ts` (every command of `caveman-trainer`'s usage texts): Tab completes a word (the options show under the line with their hints), Enter runs the command through the back end (`live`: the trainer as a hidden child, cut off after 90 s), Up and Down recall the history (kept in the browser's storage), Ctrl+L clears. Long-running commands (watch, probe, pose, mem trace, bars meter) are marked; give them a time.

## Where the app keeps things

Its settings in `%APPDATA%\dev.caveman.app\config.json` (the game and drop folders, the author name for the built mods' manifest, the FX DLL's switch and the ring's size, the plugins' settings as a free-form object the front end owns); every save also writes `%LOCALAPPDATA%\Caveman\launcher.json` (`ring_mb`, `attach`, `author`) so `Caveman.exe` agrees. The fitted-art cache in `%LOCALAPPDATA%\dev.caveman.app\art`, pictures made PNG for the board in `%LOCALAPPDATA%\dev.caveman.app\live`, WebView2's data in `%LOCALAPPDATA%\dev.caveman.app\EBWebView`. Under the game folder it writes only what Caveman already writes: settings files beside deck PNGs and `.caveman-off.json` in the drop folders, the `Caveman-Decks` / `Caveman-Shaders` mods (on Build or Play), `Caveman Templates` and `Caveman Packs` when asked.

## Approximate, mock, and not yet

- **The preview approximates the game.** Lighting is a studio environment, not the game's; the holo patterns resemble the game's rather than being ported; glow uses a gentler curve than the game's; the aura is a point light plus a halo; the rainbow trail's step length is a guess.
- **Wheels** are shown, not edited (the colour comes from the file name; no wheel settings file yet).
- **The board cam window** does not yet remember its position, and takes the selected deck's art rather than the picture actually on the board; the wheels' colour is fixed.
- **Not yet:** a changelog link to GitHub, an installer and a real icon, click-through for the board cam window while streaming, adding or removing PNGs from the app (use the folders; Refresh rescans).
- **The mock fixture** holds absolute paths from the machine that wrote it; regenerate it with `dump_mock_fixture` if those folders are gone.
- **Hardening for later:** the webview may read any local file through the asset protocol (`assetProtocol.scope` is `**`, needed for drop folders anywhere) and there is no content security policy.
