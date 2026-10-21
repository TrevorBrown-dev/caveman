# Morning report (overnight work, 2026-10-20 to 21)

Trevor asked for independent work overnight: a first test of each new feature idea, plus the app. This file is updated as each thread finishes; the summary table at the top is the place to start.

**Everything is built, verified and installed** (finished about 01:00). Launch through Caveman.exe as usual; the first launch is slower because ReSkate re-merges the changed mods. Caveman mods installed now: Caveman-Decks (26 decks: your 14 Dan Da Dan variants + 12 imported test decks), Caveman-Shaders (now with the banner trail), Caveman-Finishes-Test, Caveman-State-Test, Caveman-Trick-Test. Suggested order: the banner trail on "000 - Dan Da Dan Nyan", then the State decks, then the Trick decks (switch Caveman-Trick-Test off first if anything crashes), then compare the imported DGK decks, then try the app.

## Summary

| Thread | Status | What to try |
|---|---|---|
| Live control (control panel, music, live shader editing) | Researched twice. True real-time control still needs ReSkate plugins + one dev API, but the second look found a **near-live path that works today**: rebuild, then `mods apply reload` in the ReSkate console (~20-30 s per change; deck data only, not shaders). Untested in game | See "Live control" |
| State effects (grind, manual, trick) | **Test installed**: `Mods\Caveman-State-Test` (6 decks, verified 0 failures) | Ride each "000 - State ..." deck (see "State effects") |
| Single-trick triggers ("only kickflips") | **Test installed**: `Mods\Caveman-Trick-Test` (7 decks, verified 0 failures). Fable cracked the game's compiled expression format, so "current trick == kickflip" can be written | Ride each "000 - Trick ..." deck (see "Single-trick triggers") |
| Banner trail (rainbow rolls with the board) | **Installed** (Caveman.exe of 00:50; Caveman-Shaders rebuilt and verified) | Ride any nyan deck through flips and rolls (see "Banner trail") |
| The app (Svelte/Tauri, 3D preview) | **v1 built**: library, deck editor (writes the `.caveman.json`), approximate 3D preview (showcase + ride with aura/rainbow), Build, Play. Back-end tests pass; no installer yet | Run `Caveman\target\release\caveman-app.exe` (see "The app") |
| Deck importer (installed packs into folders) | **Done**: all 1,314 decks from your 50 packs imported as PNGs into `E:\Games\ReSkate\Caveman Imports\<pack>\` (2.15 GB; not built at launch). One pack, DGK Foil Vault, staged in the drop folder as a test (verified 0 failures) | Compare "000 - Imported Test - DGK Foil Vault - ..." with the originals (see "Deck importer") |
| Flat nyan trail + incremental builds | Installed last night, before you left (the banner builds on it) | Ride "000 - Dan Da Dan Nyan": flat band, no ground clipping, not over the deck |
| Big-collection builds | **Installed** (Caveman.exe of 00:50): a first build of all 1,314 imported decks peaks at 2.0 GB of RAM (was 12.1 GB); nothing changed = 21 s; output byte-identical | Nothing to do; see "Deck importer" |

## Banner trail

The nyan rainbow now keeps the board's tilt from the moment each piece of it was laid, so flips and rolls twist the band behind the board like a banner. Riding straight it should look like the flat trail you saw last night. Details: `knowledge/banner-trail.md`.

- **Try**: "000 - Dan Da Dan Nyan" (always on) or "000 - State Air Nyan" (only in the air): kickflips, heelflips, shove-its, rolls. Powerslides and reverts: the band should narrow, not collapse to a line. The aura decks (and Kichi, if enabled) should look unchanged.
- **If the band runs along the board instead of across it**, tell me: it is a one-line switch (the board's other axis).
- **Under the hood**: Caveman-Shaders now also patches the shared customization bundle (`items.toc`: the trail's two compute programs). ReSkate merges that with the deck mods asset by asset, so they don't conflict. Rust and Python build it byte for byte the same.
- **Backup**: last night's Caveman.exe, Caveman-Decks and Caveman-Shaders are in the session scratchpad quarantine (`re\quarantine\2026-10-21-banner`; the 00:07 exe in `2026-10-21-lazy`).

## Deck importer

All your deck packs, as PNGs you can organise: `E:\Games\ReSkate\Caveman Imports\<pack>\<deck>.png`, each with a `.caveman.json` and, where the original had them, its own mask, normal map and holo map in `.layers\` (so foils, holo direction and bumps come over too). 1,314 of 1,314 decks from 50 packs, none skipped. The original mods were only read.

- **Test first**: one pack is in the drop folder, `Caveman Decks\000\Imported Test\DGK Foil Vault\` (12 decks with mask, normal map, holo and foil mode: the hardest case). In game they are "000 - Imported Test - DGK Foil Vault - DGK - ...". Compare a few with the originals from the DGK Foil Vault mod (still installed): art, foil, holo, bumps. Measured fidelity: art median 44.7 dB PSNR against the original texture (lowest 29.6 dB on high-contrast neon art: Caveman's BC7 encoder), masks and normal maps 49 dB+.
- **To move your collection over**: drag pack folders from `Caveman Imports` into `Caveman Decks` (rename and nest them as you like: folder names become name prefixes), then switch off or uninstall the original mods. That part is your call; I haven't touched them.
- **Moving all 1,314 at once is fine now**: I reworked the build overnight (installed in Caveman.exe at 00:50). A first build of the whole collection peaks at 2.0 GB of RAM (was 12.1 GB) and takes about 6 minutes once; after that a launch with nothing changed rebuilds it from the cache in about 21 s, and adding one pack only encodes that pack. The output is byte for byte what the old build wrote. Space: Caveman-Decks about 1.6 GB, the item cache about 2 GB on C:.
- **One limit**: a single Caveman-Decks holds up to about 3,000 decks (4 GiB, the archive format's limit). Past that the build stops with a message rather than writing a broken mod; splitting into several archives is on the backlog if you ever get there.
- **Left out**: 27 Epoch Neon Empire normal maps (they point at game textures outside the mod, so the game's default is used). 5 masks had metal cleared (their preset never switched metal on, so it never showed).
- **Command line**: `caveman import --mod <folder in Mods> | --all [--to DIR] [--force]`. Settings format: `docs/settings.md` ("layers").

## The app (v1)

Tauri 2 + Svelte 5 + three.js, in `Caveman\app` (details: `app\README.md`). Run `C:\Users\thetr\Projects\ReSkate\Caveman\target\release\caveman-app.exe` (keep `WebView2Loader.dll` beside it); I rebuilt it at 00:55 from the same engine as the installed Caveman.exe.

- **Library**: decks and wheels, grouped by subfolder, search, and the PNGs a build would skip.
- **Editor**: finish (print, gloss, matte, foil, chrome, glitter, holo with patterns and spot), emboss, colour cycle and glow, board effect (aura with a colour picker or the art's colour, rainbow trail). Save writes `<deck>.caveman.json` beside the PNG (the suffixes still work; the file wins).
- **3D preview, approximate**: Showcase (the deck standing up) and Ride (the board on asphalt with the aura's light or the rainbow trail). The holo patterns and glow are drawn to look like the game's, not copied from its shaders.
- **Build** (refuses while Skate.exe runs) and **Play** (starts ReSkateLauncher.exe). Building from the app and then launching through Caveman.exe rebuilds once more (each has its own cache key); harmless, about 20 s.
- **Not in v1**: adding or renaming PNGs from the app, an installer, a real icon (placeholder), the state and trick triggers, the importer. For later hardening: no content security policy yet.
- **Toolchain**: it builds with your GNU Rust toolchain plus three small MinGW tools in `%LOCALAPPDATA%\Caveman\toolchain` (MSYS2 binutils, about 8.6 MB; `app\toolchain\fetch-toolchain.ps1` repeats the setup with pinned checksums). No Visual Studio Build Tools needed.

## State effects (grinds, manuals, landings)

The game's effect system can fire an item's effects on a **game state** instead of always (`TriggerType` 6; the Dem Bones costume does it for hard wipeouts). About 2,100 states exist; the useful ones: grinding, locking onto a grind, manualing, in the air, landing a trick, powersliding, grabbing, and trick families (fingerflip, lateflip, underflip, boneless, nollie...). Details and recipes: `knowledge/state-effects.md`.

**Installed for you to try** (a separate test mod, `Mods\Caveman-State-Test`; each deck's art names its trigger):

| Deck | Should do |
|---|---|
| 000 - State Grind Nyan | the rainbow trail **only while grinding** |
| 000 - State Manual Aura | the cyan light + trails **only while manualing** |
| 000 - State Air Nyan | the rainbow **only in the air** |
| 000 - State Powerslide Aura | an orange light **only while powersliding** |
| 000 - State Land Burst | an impact burst **once each time you land a trick** |
| 000 - State Grind Burst | an impact burst **once each time you lock into a grind** |

Questions to answer: does each effect start and stop with its state? Do the "once" bursts fire once (not repeatedly)? Anything odd in the menu (states never fire there, so menu previews show nothing)?

If the game misbehaves with this mod (unlikely: it only adds item data, plus copies of the game's own state assets, as the game does for its costumes), switch `Caveman-State-Test` off in the ReSkate launcher. This is a prototype built with the Python reference toolkit; once confirmed it goes into the Rust engine and becomes suffixes/app settings (e.g. `Deck.nyan.grind.png`, "board effect: nyan, when: grinding").

## Single-trick triggers (only kickflips, only 50-50s)

The game's current flip trick is a number (None 0, Ollie 1, Kickflip 2, Heelflip 3, Pop Shuvit 4, ...; nollie variants from 16), and triggers take a true/false state. The Fable second look decoded the game's compiled expression format (the "kernels" that turn states into true/false) well enough to write new ones byte for byte as the game's own compiler does, e.g. "current trick is kickflip" or "current trick is any of the shuvits". Grinds work the same way (50-50 in all four stances). Details: `knowledge/trick-triggers.md`.

**Installed for you to try** (`Mods\Caveman-Trick-Test`, separate from the state test; nothing ran in the game yet):

| Deck | Should do | What it tells us |
|---|---|---|
| 000 - Trick Kickflip Nyan | rainbow on kickflips only (not heelflip, shuvit, ollie) | the written kernel works. Note when it starts (pop or catch) and stops (landing or the next trick) |
| 000 - Trick Heelflip Aura | aura on heelflips only | if Kickflip works but this never fires, one header field must be computed (open question) |
| 000 - Trick Shuvit Nyan | pop shuvit, FS pop shuvit, 360 shuvit; not kickflips | "any of these tricks" works |
| 000 - Trick 50-50 Aura | any 50-50; not boardslides or 5-0s | grind types work |
| 000 - Trick Kickflip Item | kickflips | a simpler route (the game's ready-made "state == value" asset) |
| 000 - Trick Any Flip Direct | probably every flip trick and ollie; not manuals | whether a number state can be used directly |
| 000 - Trick Kickflip Burst | one burst per kickflip (two kickflips a second apart give two) | "once" triggers on a trick |

**If the game crashes** at load or when you equip one of these: switch `Caveman-Trick-Test` off in the ReSkate launcher (it is the only mod with brand-new asset types) and tell me which deck. If nothing fires but the State-Test decks do, the trick-state packaging is the suspect, not the triggers.

## Live control

First pass (`knowledge/live-control.md`):

- **Live colour, speed, glow from a control panel**: needs ReSkate's plugin system (unreleased: the `plugins` branch) plus one plugin call the devs would add (set a colour parameter on the local board; ReSkate already does this every frame for developer boards). Design once that exists: the Caveman app writes a `live.json`, a Caveman plugin reads it each tick and sets the board colour; the patched deck shader can use that colour as a 3-value control bus (hue, glow, beat) for marked decks.
- **Music**: the same route; the app listens to the PC's audio (WASAPI loopback), detects beats and levels, and writes them to `live.json`. Plugins only see track metadata, not audio.
- **Live shader editing**: not feasible; shader programs load once at boot. Keep effects data-driven so iterating a look is a data change.
- **Bonus**: the plugin API already exposes grinding / air / riding / wipeout, trick names and the board's pose: the state effects and the banner trail could also be driven by a plugin later.
- Ask the ReSkate devs: (1) a plugin API to set colour/vector parameters on the local player's board materials; (2) whether a live mod apply works for a mod rebuilt after launch.

Second look (Fable 5.1, `knowledge/live-control-second-look.md`): real-time control from outside the game is still impossible in 2.1.1, but it found a near-live path the first pass missed, and corrected the plugin file channel.

- **Works today (verified in ReSkate's code, not yet tried in game): change a deck mid-session without restarting.** With the game running: change the deck's sidecar `.caveman.json` (speed, glow) or its PNG suffix, rebuild with the CLI (`cargo run --release -p caveman-cli -- --game E:\Games\ReSkate` from `Caveman\`; Caveman.exe refuses while Skate.exe runs), then type `mods apply reload` in the ReSkate console (the key left of 1) or press "Apply and reload level" on the MODS tab. ReSkate re-places any mod whose files changed and reloads the level; about 20-30 s per change. Expect in the log: "Caveman-Decks: its files changed since it was loaded, so it is placed again" and "Reloading levels/game/BAM_LevelRoot...". Effect code, speed and glow (the holo-map marker) go live this way; changes to Caveman-Shaders itself still need a restart.
- **When plugins ship, no dev ask needed**: the app can trigger that apply-and-reload itself (a plugin switching between two variants of the deck data); an in-game control panel (sliders, dropdowns); a music-reactive halo or ring drawn at the board as an overlay (not the deck itself). Correction: the plugin's live file goes in `Mods\.reskate_plugin_data\<id>\`, not in the plugin's own folder (ReSkate reloads a plugin whose folder changes).
- **The asks for the ReSkate devs, sharpened**: (1) a plugin call to colour the local board the way developer boards already are (or open that "MarkStyle" colouring to every player; the UI, networking and per-tick renderer path all exist, only a Steam ID list gates them); (2) `reskate.music.level()`: ReSkate already captures the music's audio and runs a 5-band EQ on it, so exposing levels is small; (3) optional: `reskate.mods.apply(reload)` for plugins.
- **Confirmed impossible**: shader hot-reload; a plugin swapping the deck; the developer-board path for normal players.
