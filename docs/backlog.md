# Caveman backlog

Ideas and open work, newest decisions first within each group. Detail on anything already built is in [deck-format.md](deck-format.md).

## Vision (Trevor, 2026-10-20)

Caveman becomes its own product, released on GitHub with an installer: own the shaders and effects for our boards 100%, a Caveman launcher app, a Skate 3-style custom deck builder, a GUI for viewing and managing boards, and a community store.

Proposed order (agreed in principle; the biggest unknown first):

1. **Shader spike**: prove we can ship our own shader program: one test deck whose colours shift over time. Work out the compiled-shader container, registering it in the game's shader database, and compiling with the game's own `dxcompiler.dll`. Success unlocks animated, glowing (neon art) and holographic decks.
2. **Tier 1 presets**: gloss, foil, emboss with the existing pipeline.
3. **Standalone core**: Caveman in Rust (no Python), `bc7.py` and the Python toolkit as the reference.
4. **Caveman app**: Svelte + Tauri (Trevor's go-to stack; Rust core): launcher wrapping ReSkate's, board manager, Skate 3-style deck builder (layers of images, stickers, text, colours on the template, live board preview).
5. **Store and installer**: a Caveman front end over Thunderstore (Caveman-built packs are ordinary mods), so no own hosting, accounts or moderation; plus the installer.

Started 2026-10-20 (Trevor: "lets try getting shaders working and the proven/likelies"):

- **Shader spike** (step 1), running as research: the compiled-shader container, the shader database, the stock deck shader's inputs and outputs (is there a clock for animation, an emissive output for glow?), and compiling with `dxcompiler.dll`. Findings go to `knowledge/shaders.md`.
- **Animated decks from a GIF or video** (low priority, Trevor 2026-10-20: a deck GIF is a complex asset to make just to test): Caveman recognises `.gif` (and later video: `.mp4`, `.webm`) as an animated deck, samples its frames into an atlas texture, and the deck's shader steps through them over time. Needs the shader spike. When it comes up, Caveman generates its own animated test deck (a moving test pattern), so testing needs no hand-made GIF.
- **In game, 2026-10-20 (Trevor's second test)**: foil, foil spot emboss, glitter, holo and holo vertical "all look great"; the shader effect `cycle` works (Caveman's own shader code, per deck); the clock runs (pulse diagnostic); glow does NOT show yet (research running: `knowledge/glow.md`). Ideas from it: a smooth "breathe" effect (the pulse diagnostic snaps dark and fades in), more effects on the same marker.
- **Mixin library** (cheap effects, step 2): BUILT 2026-10-20. Presets that need no new shader, chosen by file suffix (`tools/re/mixins.py`): finishes gloss, matte, foil, chrome, glitter, holo (Pearl), holo-cellular, holo-frost, holo-vertical, holo-glitter; modifiers spot (finish on the light parts only) and emboss. Holo uses the game's own holo foil (the community method). Calibration decks: `tools/re/build_finishes.py` (mod `Caveman-Finishes-Test`). Next: tune values from Trevor's look (gloss/matte levels, foil gloss, glitter flake size, spot threshold, emboss strength and direction, full vs spot holo as the default); holo menu thumbnails (now plain gloss); then candidates: stains or custom wood under the art (`param_base_c`), iridescence strength per holo preset.

**Glow and light trails** (researched 2026-10-20, `knowledge/glow.md`; Trevor: "something for later"):

- **Real light, trails and particles from the item, no shader and no Lua**: an item's `DelMarCustomizationItemData.SkaterVfx` can point at a VFX asset (always-on, board-attached effects whose blueprints hold `PbrSphereLightEntityData` lights and ribbon trail emitters, colour baked per blueprint). This is how Kichi Aura Wheels glow (confirmed in game: trails, light on the ground, particles; trails spawn by distance, so not in menus). Recipe for decks: clone the game's `ebp_chr_oni_sword_light` and `skfx_cst_oni2026vfx`, set the colour, point the deck item's `SkaterVfx` at it. Unconfirmed: whether deck items honour `SkaterVfx`. Suffix idea: `.glow-cyan`.
- **Emissive art** (the deck itself lights up): render target 3 is the emissive buffer; Caveman's glow only went to the float layout (A), and the deck is most likely drawn by layout B (half-float rgb plus a code in alpha). Fix in `shaderpatch.py`: write `legacyF32ToF16(glow)` into B's target 3 rgb.

Separate research items: **flames** as real particle effects around the board (fire *on* the deck is just an animated deck), and **neon light trails** behind the board (a world effect: the game's VFX system, or ReSkate's upcoming plugins, which can draw lines in the world). Give the ReSkate devs a heads-up before shipping a wrapper launcher.

## T-shirts: all-over print mode (backlogged, Trevor 2026-10-21)

The print layer (UV1, the stencil) cannot reach the tee's shoulder yoke, side strips, collar and cuffs: the mesh maps them onto one corner of the print sheet (knowledge/clothing.md). For full coverage, an "all-over" mode would paint the design into the shirt's fabric texture (UV0, `param_base_c`) instead, as the 617 Underground pack does, blending the fabric's folds and shading over the art; it needs its own stencil of the fabric layout. File name idea: `Name.allover.png`. Trevor wants to see what the print-layer method can do first, then decide.

## One Caveman folder (Trevor, 2026-10-21)

Move the drop folders out of the game root into one folder: `<game>\Caveman\Boards` (today `Caveman Decks`), `\Wheels` (`Caveman Wheels`), `\Tees` (new), `\Imports` (`Caveman Imports`), `\Archive` (`Caveman Archive`). Caveman moves the old folders on its first run (with a log line), so nothing breaks; item IDs do not change (a deck's name comes from its path inside the drop folder, not the root). Update Caveman.exe, the CLI defaults, the importer's default target, the app's settings and the docs. Do it once the agents editing the Caveman crates have landed; tees go straight into `Caveman\Tees`.

## Next up

- **Pack mode: a folder into a distributable mod** (Trevor, 2026-10-20). Compile a folder of PNGs into a genuine, standalone deck pack that anyone can install from Thunderstore without Caveman, the same kind of mod ReSkate Studio makes. Most of it exists: `Caveman-Decks` already is a standard mod. Still needed:
  - Thunderstore packaging: `manifest.json` with the real fields (`name` as `Owner-Name` rules require, `version_number`, `description`, `website_url`, `dependencies`), a 256 x 256 `icon.png` (a contact sheet of the decks' thumbnails), a `README.md` listing the decks with previews, and the zip layout Thunderstore expects.
  - A pack name and version per pack (from the folder name, or a small settings file in the folder), so several packs can coexist and update.
  - Unique deck IDs across creators (see "Deck IDs in multiplayer" below), and a check of names against the Thunderstore listing (`/c/reskate/api/v1/package/`), not just installed mods.
  - The build-stamp question for the ReSkate devs (below) must be settled before publishing anything.
- **Effects as suffix presets** (Trevor's design): `Cool Deck.foil.png` builds a variant with a pre-packaged effect; the art is the same image. Built as the mixin library (above); suffixes stack (`Cool Deck.holo-frost.spot.png`).

## Effects and shaders (exploring, 2026-10-20)

Two tiers (detail in `knowledge/cosmetics.md`, "Effects and custom shaders"):

- **Tier 1, no new shader** (built, see the mixin library): gloss/matte and metal/foil/glitter (`param_graphic_osk`), emboss (`param_graphic_ny`), holo foil (the deck shader's own iridescence: `param_graphic_rgb` + `param_foilmode`); not yet custom wood (`param_base_c`).
- **Tier 2, custom shaders** (Trevor has ideas): animated looks (GIF decks, colour cycling), glow, parallax, anything the deck shader cannot do. Proven by the community deck tool (pre-compiled programs + global shader database rows + a data-driven shared renderer + the Lua board compositor). Needs shader-capable Studio once, or reverse-engineering the bytecode container and compiling with the game's `dxcompiler.dll`. Must not collide with the deck tool's shared renderer.

## Import decks from installed mods (Trevor's idea, 2026-10-20)

Extract the art of the deck mods Trevor has installed into PNGs in the drop folder, to consolidate them under Caveman (and its effects). **Goal (Trevor): keep the deck collection organised: import every deck pack into folders, then uninstall the original deck mods** (one Caveman collection, sorted by folders; no duplicates; fewer mods to merge at launch). Feasible with what exists: read the mod layer, follow each deck item (`items/board_bottomart/*`, or the mod's own category path) -> its `AssetPaths[0].AssetName` preset -> the `param_graphic_c` texture -> decode (bcdec) -> PNG, at the texture's own size. Detect what carries over into a sidecar JSON: a holo map and foil mode (holo), a metal mask (foil/spot), a raised normal map; report what does not (custom board meshes from Studio's `reskateboard1_` loader, the deck tool's lenticular/animated renderer).

- Size: 238 deck-like items in 17 mods by name alone; packs that do not name items "deck" (Skate 3 Decks 155 items, the DGK packs) push it to several hundred.
- Plan: `caveman import <mod folder>` -> `Caveman Decks\Imported\<Pack>\<Deck>.png` (+ `.caveman.json`), read-only on the mods; later an app button. Never disable or change the original mods.
- Trade-offs to keep in mind: in multiplayer others see a deck only if they have the same item ID (the original packs are on Thunderstore; imported copies get Caveman IDs); importing while the original stays installed duplicates decks (the name-clash check skips clashing names: import, then disable the original); the art is the pack authors' (personal use; republishing needs permission).
- **Built 2026-10-21** (`caveman import`, `import.rs`): all 1,314 decks of Trevor's 50 packs into `E:\Games\ReSkate\Caveman Imports\` (not the drop folder yet: Trevor moves packs in). Whole-collection builds made cheap the same night: a clean build of all 1,314 peaks at 2.0 GB of RAM (was 12.1 GB: every deck's decoded art, layers and thumbnail were held until the end) and a rebuild with nothing changed takes 21 s, output byte-identical.
- **Limit: one archive holds 4 GiB** (32-bit locations), about 3,000 decks; the build now fails with a clear message past it instead of writing a broken mod. Beyond that: split the mod into several archives (cas_01, cas_02, ...; the game's own layout does this) or into several mods.

## Grind and trick effects beyond data (Trevor's idea, 2026-10-21)

Data alone reaches only states of the game configuration or the shared bundle (knowledge/state-effects-runtime.md); grinding, manuals and the trick enums live in the root level. **We own this** (Trevor, 2026-10-21): no asking ReSkate for changes; local-only effects are fine; the goal is a design that survives game and ReSkate updates. Routes:

- **A trainer in Caveman.exe / the app (Rust, no code injection)**, the first choice: (1) **patch the trigger once**: find our effect items in memory by a magic `InstanceGuidHash` we choose, write the loaded root-level state's address into `TriggerEffectsGameState.AssetRef` (item + 0x50), then refresh the skater's effects; or (2) **a state bridge**: our own states in the shared bundle (`caveman.grinding`, `caveman.fx1`, ...) that decks trigger on, switched by the trainer from anything it can read (grinds and tricks from game memory, music beats, the app's control panel). Open question for (2): whether the game gives a mod's own state a value slot.
- **Our own native DLL** (hooks): the most power (it could also null-check the trigger update and bind triggers late itself); more update-fragile. Engineer around ReSkate hooking the same functions (`build_effects`): chain, don't replace.
- **Surviving updates** (all routes): find things at run time instead of hard-coding them: byte-pattern scans for code (as ReSkate's tables carry a 16-byte prefix per function), the game's own reflection type info for field offsets (run-time offsets equal the EBX ones, read from the reflection table), GUIDs and magic values we put in our own data; key anything fixed to the exe hash; on any mismatch switch the feature off and never crash the game.

## Custom wheels (Trevor's idea, PARKED 2026-10-20 for glow first)

Design: a `Caveman Wheels` folder beside `Caveman Decks`; `Name.ff0000.png` = a square face graphic plus an optional hex wheel colour (default white); transparent parts show the wheel colour. Worked out so far (from the stock wheels, nothing built yet):

- Items: category `items/boardwheelcolors0/board_wheelcolor` holds both solid-colour wheels (`own_wheelcolor_*`, 115) and graphic wheels (`own_wheelgraphic_*`, 99); each points at an `_ap` like decks and uses the deck shader (`m_skateboard_deckbase`, parent `wheels_m_parent`). Studio's wheel mods name new items `items/board_wheelcolor/<name>` and assets `characters/customization/reskate/board_wheelcolor/<name>` (forestmouse Spitfire F4 Wheels).
- Host/template: the donut wheel `wheel_gen_graphic_00006` (`.../wheelgraphic/wheel_gen_graphic_00006/2024/`; item `items/boardwheelcolors0/own_wheelgraphic_gen_classic00006`; untouched by installed mods). Its preset: `param_graphic_c` (own art), `param_graphic_osk` (own mask), generic `wheel_gen_classic_c/ny/msk` base, `param_onebythreemode`, and **`param_region_color_0`** (ParamKey `4b18a9e2-cd35-8c46-59ff-5e880100548c`; boxed 16 bytes: three **linear** floats + u32 1) = the wheel colour. Items also carry `BaseColor` (Vec3).
- Textures: art `_c` 512 x 512 BC7 sRGB (format 67), 10 mips; mask `_osk` 512 x 512 **BC1** (format 54; needs a BC1 encoder). Layout: the outer face is the top half (512 x 256) as an ellipse; the bottom half is the inner face. Mask **R = where the graphic shows** (elsewhere the wheel colour), G/B the finish (copy the template's). Alpha unused.
- Thumbnails (face-on render, bearing hub in the middle, 768/256 px): disc centre (378.5, 383.5), radius 335.5 at 768; the face ellipse maps to it with no flip or offset at scale 1.22 (the outer 18% of the ellipse wraps over the wheel's rounded edge; correlation 0.76 over 65 stock wheels; `scratchpad/wheels/geometry.py`). Then fit light and sheen per pixel as for decks.
- To build: BC1 encoder, mip offsets per format and size (wheel mips differ from decks), a wheel path in `newdeck.build`, wheel thumbnails, the verifier, an artist template (visible face circle at 1/1.22, hub circle).

## Distribution and plug-and-play

- **GitHub-only release is fine** (Trevor, 2026-10-20): a simple installer (MSI or a single setup exe) that finds the ReSkate folder and drops the wrapped `Caveman.exe` beside the launcher. Needs the standalone, no-Python Caveman first.

- **Native PNG decks in ReSkate itself** (the real plug-and-play). Pitch to the devs (Discord or a GitHub issue), then build ReSkate from source (Visual Studio 2022 Build Tools with the C++ workload and CMake; confirm the download size first) and contribute it with a design document in their `contrib/` style. Caveman's Python is the reference implementation, including today's crash rules.
- **Ask the ReSkate devs**: may a third-party tool write `.reskate-studio-patch`? Would they accept native PNG decks, and where should it live?
- **Standalone Caveman** without Python (Rust, with `bc7.py` as the reference for the BC7 encoder), for other people.
- Rejected: a background folder watcher (Trevor, 2026-10-20). A DLL loaded beside the launcher (malware-like, fragile, not hostable).

## Deck IDs in multiplayer

Only item IDs travel between players; each game draws its own installed item for an ID. Two different "Cool Deck"s show each player their own art; an ID nobody else has shows the stock Natural Blank. Options: plain names (now), name plus a short tag from the image's pixels (same image, same ID for everyone; the tag shows in the name), or a creator prefix. Decide before pack mode publishes anything.

**Decided (Trevor, 2026-10-21): plain names.** Multiplayer is only expected to work for basic boards: the same deck name on two machines is the same deck, and having the same image is the players' unenforced side of the contract. Effects (auras, trails, state effects) are local only and that's fine. Consequences:

- **The name-to-ID rule is frozen**: `Own_` + the in-game name (folder prefix + file name + effect words) with spaces as underscores (`names::item_key`). Changing it would silently break matching between Caveman versions, so any change needs a versioned migration.
- Folder prefixes and effect suffixes are part of the name; capital letters probably are too (untested).
- Open (asked Trevor): should imported decks keep the original pack's item ID, so players with the original Thunderstore pack match them? That means the in-game name is the original one, without the folder prefix.

**Idea (Trevor, 2026-10-21, backlogged): IDs from the image, not the name.** `Own_cv_<hash>`, the hash of the decoded pixels (not the file bytes: a re-saved PNG still matches) plus the deck's effects (the same image can be a plain and a holo deck, and two items cannot share an ID). Same image + same effects = same deck for everyone, whatever the file is called; renaming or moving a deck keeps its ID (stays equipped, keeps matching).

- The catch: the game shows a mod item's ID as its name (ReSkate's `local_cosmetic_catalog.cpp`: `metadata.value("title", <key minus Own_>)`), and a mod cannot ship titles (they come from the game's installed content cache, `content_catalogs.cpp`, stock items only). ReSkate does keep **per-player title overrides** in the profile (`customization.catalog[<key>].title`, validated in `local_customization.cpp`; stored in `%LOCALAPPDATA%\ReSkate\profiles\offline\reskate.sqlite3`, table `catalog_entries(catalog, item_id, definition_json)`, which today holds only `challenge` rows). Caveman would write a title per deck there at build time (game closed), so the list still shows the file names and folder prefixes; on any format mismatch skip it (the names fall back to the hash).
- To test first: does a title written there show in the deck list; does ReSkate keep it when it saves the profile (`profile_update.cpp` preserves catalogs selectively).
- Consequences: imported decks would match other Caveman users who imported the same pack, not players of the original pack; existing name-based IDs change once (equipped decks reset).

## Smaller items

- **Real-deck photos as input**: shop listings and archives such as [Deckslog](https://www.deckslog.com/) show real decks' bottom graphics, often on a plain background with the deck's own outline. A real deck (8.0 x 31.75 in) is almost exactly our 1:4, so detecting the deck's outline in such a photo and cropping to it would map its nose and tail straight onto the trim line. Personal use only: those graphics are the brands' and artists' work, and bulk-downloading an archive or publishing them is a different matter.
- A Desktop shortcut for `Caveman.exe` (done: the icon, copied from ReSkate's launcher by `launcher/set_icon.py` for Trevor's own copy; a public build needs its own icon).
- Faster rebuilds: the remaining ~7 s is mostly scanning every installed mod for name clashes and BC7 encoding; both could be cached or parallelised.
- More cosmetic kinds with the same machinery: grip tape, wheels, then clothing.
- Spread large collections over several host bundles: every deck in one bundle loads together.
- Understand why "Dan Da Dan" did not show in the deck list without the `000 - ` prefix while "Caveman Test Pattern" did (all data checked out; likely list ordering).
