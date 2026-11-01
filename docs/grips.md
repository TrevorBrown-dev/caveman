# Grips and grip patterns

How the game's "Grip Color" and "Grip Pattern" items work, and how Caveman builds its own (2026-10-31, from the game's files; the item records, presets and textures were read with `crates/caveman-core/tests/grip_items.rs`, whose report and exported textures land in `target/grip/`).

## What the game has

**Grip colour** (category `boardtapecolors0`, 80 items, the menu's "Grip Color") is an appearance preset of the grip material, exactly as a deck graphic is a preset of the deck material. The item record (`items/boardtapecolors0/own_deckgripcolor_gen_popsicle_000NN`) names the preset `Deck_Gen_PopsicleGripColor_000NN_AP` (AssetTypeId 3), carries two thumbnails (`thumbnail/tool/own_deckgripcolor_gen_popsicle_000NN` and `_lrg`, 256 and 768 px BC7, the deck seen from above) and a `BaseColor` that is a plain grey (0.214) for every grip, so the menu goes by the thumbnail. The preset's bundle (`win32/.../gripcolor/presets/deck_gen_popsiclegripcolor_000NN_ap_cas_main_bundlereftable`, 28 EBX, 6 textures) holds the grip material `m_skateboard_griptape` under the parent `deck_griptape_m_parent`, the wood's three textures (`param_base_*`: the top face, where the bolt heads are painted) and the grip's three graphic textures (`param_graphic_c/_ny/_osk`, 512 x 2048, the top face filling them as deck art does). Two kinds of grip:

- **Plain colours** (00001 black, 00006 hot pink and most others) all point at the same template textures, `_template/2023/deck_gen_gripcolor_c/_ny/_osk` (a grey grit, mean 155; the normal map's grain; an `_osk` with R and B 255 and G about 99 for the smoothness), and differ only in the boxed parameter `param_graphic_tintcolor` (ParamKey `36e50d59-48af-873f-59ff-5e88010042d4`: three linear floats and a u32 1; black (0.048, 0.044, 0.054), hot pink (0.763, 0.0175, 0.301)). The tint multiplies the grit, which the menu thumbnail confirms (the pink thumbnail is the grey grit times the tint). Two more parameters set the sparkle, `sparklestrength` and `sparklescale` (0.1 and 1.0 in every grip looked at).
- **Printed grips** (00023, a neon skeleton) have colour, normal and `_osk` maps of their own in the preset's own folder, and no tint parameter at all.

**Grip pattern** (category `boardtapetextures`, 33 items, the menu's "Grip Pattern") is not a preset: the item record (`items/boardtapetextures/own_deckgripcutout_gen_popsicle_000NN`, `CustomizationItemId` empty) names a texture asset directly, `Deck_Gen_GripCutout_000NN_OSK` (AssetTypeId 6), and the game puts that texture in the grip material's `_osk` slot. Each cutout is a bundle of its own (`win32/.../griptape/deck_gen_gripcutout_000NN/<year>/deck_gen_gripcutout_000NN_osk_cas_main_bundlereftable`: the texture's EBX, the bundle's reference table and blueprint, a metrics asset, one resource, two chunks). The texture is 512 x 2048 BC1 (format 54) with 12 mips, the first two streamed: **R is 255 where the grip is and 0 where it is cut away** (the wood of the top face shows there; the bolt heads show through the grip anyway), **G about 99** (the grit's smoothness, the same as the template `_osk`) and **B 255**. The hearts (00018) are a full-frame tiled pattern with soft edges; 00001 has no holes; 00010 has stars. The pattern's frame is the deck art's: the top face fills the 512 x 2048, the nose at the top. Its thumbnails show black grip with the pattern's wood through it.

Both kinds are in the master item list (`items/itemmastercollection`) like every other item, and the bundle reference table maps the preset path (grip colours) or the texture path (cutouts) to the bundle that holds it, by the full path and the leaf name.

## What Caveman builds

Two more drop folders beside the game, in the one Caveman folder:

- **`Caveman\Grips`**: `Name.png` is a picture printed on the grip tape (fitted as deck art is: turned upright, cover-cropped to 1:4; its transparent parts show the grit); `Name.ff69b4.png` the same over grit in that colour; `Name.ff69b4.grip` (any contents, usually an empty file) plain grip in that colour. Subfolders prefix the name as for decks. The item key is `Own_Grip_<name>`.
- **`Caveman\Patterns`**: `Name.png` is a mask. Its transparency is the pattern when it has any (clear parts cut the grip away), else its lightness (dark parts cut it away). A picture of the deck's 1:4 shape (within 3 per cent) covers the deck; any other size is repeated as a tile at its own pixel size across the 512 x 2048, so a 128 x 128 motif tiles 4 by 16. The item key is `Own_GripPattern_<name>`.

The build (`caveman-core/src/build.rs`, `grip_product` and `cutout_product`; the CLI's `names::scan_grips` and `scan_patterns`; `Options.grips`/`patterns`, `--grips DIR`, `--patterns DIR`):

- A plain grip is the hot pink host preset (00006) cloned with the tint set to the colour (linear, `set_vec3_param`), no textures of its own: it shares the game's template grit, as the game's own colours do. Its `BaseColor` is the colour.
- A printed grip gets one new texture, the colour map (the picture over the grit, over grit in the colour when one is given), BC7 with 12 mips in the host's bundle, the preset's import of the template colour map pointed at it, and a white tint. The grit's normal map and `_osk` stay the template's.
- A pattern is one new texture in the host cutout's bundle (00018's): R the mask, G 99, B 255, BC1 with 12 mips, first mip 0 (whole, as every Caveman texture: `knowledge/testing-and-debugging.md`), named `characters/customization/reskate/board_griptape/<key>_osk`; the item names its leaf (`<key>_osk`) as the game names its own, and the bundle reference table gets the path.
- Thumbnails (`thumbs::render_top`): the stock thumbnail of the host supplies the deck's outline from above; inside it the grip's colour map (or the black grit for a pattern), the wood through the holes and the bolt heads from the wood's mask, mapped as the texture frame onto the outline's bounding box. The 768 px one is also the item's preview PNG in the folder's `previews`.
- Items go into the same mod as the decks and wheels (`Caveman-Decks`), with the master list, the reference table and the thumbnails' CDN records as for decks; the item cache keeps them like any item (kinds `grip` and `cutout`).

`crates/caveman-core/tests/grip_build.rs` (ignored) builds `target/mods/Caveman-Grip-Test`: a hot pink plain grip, the Biggie Cheese deck art as a printed grip (plain and over blue grit) and a polka-dot pattern, and writes their thumbnails into `target/grip/`.

## Open

- Not yet seen in the game's menu or on the board (built and installed with the game closed; the first look is Trevor's).
- The tint of a printed grip is set to white rather than removed as the game's printed grips have it; if the game treats an absent tint differently from a white one, the print will look off and the parameter should be dropped from the clone instead.
- The sparkle parameters are the host's (0.1 and 1.0); a `Name.caveman.json` could set them later, as a deck's settings file sets its finish.
- The Studio in the app does not list grips and patterns yet (the back end scans only decks and wheels into the library); the build picks them up regardless, so the folders work from the command line, from Caveman.exe and from the app's Play button.
- The board cam window builds its replica from the game's own item records, so a Caveman grip or pattern equipped in game falls back to the stock grip there until the model builder reads the mod's items too.
- Packs (`caveman pack`) take decks and wheels only.
