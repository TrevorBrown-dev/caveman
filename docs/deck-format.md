# Deck format findings

Groundwork for **Caveman**, a tool that turns a folder of PNGs into ReSkate decks. Reverse-engineered on 2026-10-20 from the installed game (Steam build 25414733, ReSkate 2.1.1), the game's own deck data and 70 installed ReSkate Studio mods. Everything here was read from real files with the scripts in `tools/re/`; items marked **unverified** still need an in-game check.

## The deck texture

A deck's art (the "bottom art", the `_c` graphic colour texture) is one texture. The game ships 334 stock deck graphics and every one of them is identical in shape:

| Property | Value |
|---|---|
| Size | 512 x 2048 (1:4, long axis vertical) |
| Format | BC7, sRGB (Frostbite format ordinal 67) |
| Mips | 12 (512x2048 down to 1x1), 1,398,160 bytes in all |
| Alpha | not used (opaque everywhere) |
| Streaming | mips 0-2 stream from the TOC chunk; mips 3-11 (21,904 bytes, from logical offset 0x150000) stay resident in the bundle |

The 1,500+ community deck textures in the installed mods are also 512 x 2048 (BC7 sRGB, and a few BC3 sRGB). The only larger ones (1024 x 2048) are a different slot, the deck base (wood) texture, not the graphic.

Frostbite format ordinals seen in the game's own textures (inferred from which texture kinds use them):

| Ordinal | Format | Used for |
|---|---|---|
| 54 | BC1 linear | masks (`_msk`, `_osk`) |
| 55 | BC1 sRGB | colour |
| 60 | BC3 linear | masks |
| 61 | BC3 sRGB | colour (one community deck pack uses it for decks) |
| 63 | BC5 | normal maps (`_n`) |
| 66 | BC7 linear | normal maps (`_ny`), masks |
| 67 | BC7 sRGB | colour (`_c`), every stock deck graphic |

### Visible area

The deck is a pill shape inside the 512 x 2048 canvas, measured from where the 334 stock decks paint a flat "outside" colour:

- top of the deck at y = 79, bottom at y = 1970
- widest at mid-length: x = 12 to 500
- the ends are rounded over roughly the first and last 300 px

`assets/deck_template_512x2048.png` is that outline (white inside, transparent outside) and `assets/deck_outline.json` has it row by row. Art should still bleed past the line to the canvas edge: the outline is the artists' consensus, not the mesh's exact UV edge (**unverified** until tested in game).

**Orientation (verified in game, 2026-10-20):** the image maps onto the board upright and unmirrored as seen in the board customization view. The top of the image is the end at the top of that view, and left stays left. An image that looks right on screen looks right on the board. The board's edges in game fell close to the measured outline (the 128 and 1792 grid labels were just inside the ends; the L and R labels near the sides).

### Fidelity

- **Ship 512 x 2048 BC7.** It is what all the game's own decks use, and BC7 at this size is visually near-lossless.
- **Author bigger.** Work at 1024 x 4096 (or 2048 x 8192) and let the tool downscale, which gives sharper mips than painting at 512 wide.
- **Bigger in-game textures are untested.** The texture header allows up to 16384 on a side and nothing obviously caps it, but no installed mod ships a deck graphic above 512 x 2048. A 1024 x 4096 deck would cost 4x the memory (about 5.6 MB a deck) and only shows its extra detail in very close shots. Worth one in-game test before offering it as an option.

### Other per-deck texture slots

Community packs override up to three textures per deck, named `<deck>_role3_<slot guid>`:

| Slot GUID suffix | Format and size | Shader parameter (from the `_ap` preset) |
|---|---|---|
| `2259d419-387f-b1ba-30fc-c5140100b012` | BC7 sRGB, 512 x 2048 | `param_graphic_c`: the graphic colour (the art) |
| `c1b71b8a-395f-e8ce-30fc-c5140100a381` | BC7 linear, 512 x 2048 | `param_graphic_osk`: the graphic's osk mask (stock `deck_gen_graphic_default_osk`) |
| `0bb23445-4861-79a3-30fc-c5140100e899` | BC7 sRGB, 1024 x 2048 | `param_base_c`: the deck base (wood) colour (stock `deck_gen_popsicle_c`) |

The slot GUIDs are the `ParamKey`s of the appearance preset's parameters. A deck's `_ap` preset (`AppearanceShaderExpressionPreset`) has 12: the three above, `param_graphic_ny` (`830b93ff...`, stock `default_ny`), `param_base_ny` (`3f4cd4ca...`), `param_base_msk` (`00c854f4...`), `param_usegraphic` (`098279ad...`, true), `param_udim_graphica` (`53261100...`, 2), and four region and graphic enums.

The stock base is `deck_gen_popsicle_c` (BC7 sRGB 1024 x 2048), `_ny` (BC7 linear 1024 x 2048) and `_msk` (BC1 linear 1024 x 2048). The base texture's UV layout differs from the graphic's (two faces side by side, a ply strip at the top).

## Anatomy of a new deck (ReSkate Studio output)

Studio adds new decks into an existing stock deck's graphic bundle (for example `deck_gen_graphic_00010_ap_cas_main_bundlereftable`) and into the shared customization bundle (`characters/customization/configs/cas_main_sharedbundle`).

Per deck, in the graphic bundle:

- EBX `characters/customization/reskate/board_bottomart/<name>_ap` (about 3.5 KB)
- EBX `characters/customization/reskate/board_bottomart/<name>_role3_<slot guid>` (the texture asset, about 1 KB)
- the texture resource of the same name (180-byte header) and its chunk

Per deck, in the shared bundle:

- EBX `items/board_bottomart/own_<name>` (the item, about 12 KB)
- EBX `thumbnail/tool/own_<name>` and `..._lrg`, with their texture resources and chunks (256 and 768 px)
- EBX `thumbnail/cdn/img_own_<name>` and `..._lrg`

Once per pack, in the shared bundle:

- EBX `items/itemmastercollection` (the master item list; ReSkate merges each mod's copy)
- resource `characters/customization/configs/cas_main_bundlereftable` (type `0x428ec9d4`, the bundle reference table)

Per-deck EBX files differ only by name length from one deck to the next within a pack (for example 3,478 vs 3,476 bytes), which suggests a clean template-and-rename approach for adding decks.

## Registering a new deck: the bundle reference table

`characters/customization/configs/cas_main_bundlereftable` (resource type `0x428ec9d4`, in `cas_main_sharedbundle`) tells the game which bundle to load for an appearance preset (`..._ap`). `tools/re/brt.py` (a port of ReSkate's `bundle_ref_table.cpp`) reads and writes it: the game's table round-trips byte for byte, and 52 of the 57 installed mods' tables rebuild byte for byte from the game's plus their additions (the other 5 differ only in how path text is packed; their lookups are identical).

- Every bundle-list entry (2,633 of them, index 0 is a sentinel) is a stock preset path whose bundle is `win32/<path>_cas_main_bundlereftable` in `items.toc`. The bundle list is never changed.
- Each preset has two lookup rows, the 64-bit FNV-1 hash (lowercase) of its full path and of its leaf name, both pointing at its bundle's index.
- **Per new deck**: the preset path is the new `_ap` EBX's name, lowercased (for example `characters/customization/reskate/board_bottomart/supreme_shrek_deck_ap`), pointed at the stock deck bundle the deck's assets were added to (Supreme uses `deck_gen_graphic_00010`; Tearrist and LeClan use `deck_baker_graphic_00001`). `brt.add_presets(table, [(path, bundle name)])` writes the path text, both rows, the header offsets and the 16-byte resource meta (payload size, 20, zeros), exactly as ReSkate's merge does.
- So a new-deck mod ships `cas_main_sharedbundle` with this resource changed (and its manifest SHA-1 updated).

## Chunk metadata (new texture chunks)

A bundle manifest's chunk metadata (`tools/re/ndb.py`, a port of ReSkate's native DB reader and writer, round-trips byte for byte) is a list of records: one per chunk, `{h64, meta: {firstMip}}`, plus one container record (`meta: {container, metrics}`). Verified on the game's bundles (14 of 14 records):

- `h64` is the hash of the name of the resource the chunk belongs to: start at 5381, then `h = h * 33 ^ lowercase(char)` per character (64-bit). The container record's `h64` is the hash of the bundle's `..._win32_metricsasset` resource name.
- `firstMip` matches the texture header's first mip (3 for a stock 512 x 2048 deck, whose mips 0-2 stream).
- A mod's copy keeps the game's list and puts the record for each chunk it adds at that chunk's position in the manifest's chunk list.
- **Chunk GUIDs carry the encoding**: the streamer reads a chunk as compressed blocks only when the low bit of its GUID's last byte (in the little-endian form) is set. Every texture chunk has it set; a new chunk's GUID must too.
- **Keep cosmetic textures whole**: ReSkate's merge notes that Studio keeps cosmetics and UI textures unstreamed on purpose (a streamed costume texture once left a load waiting forever). New deck textures are written with first mip 0, fully resident in the bundle; the texture header holds the first mip at bytes 0x1F and 0x20.

## Crash: an equipped board's texture stored uncompressed (2026-10-20)

`Caveman-Test` (Natural Blank replaced, board and thumbnails) worked when the deck was equipped in a session, but **the game crashed about 2 s after the skater was created on every launch with the deck already equipped**. The dump: an access violation in `memcpy` (`VCRUNTIME140.dll+0x1bc5b`) called from the game's texture loading, copying 349,584 bytes (exactly mips 1-11 of a 512 x 2048 BC7 texture) from a buffer holding only 294,912. At spawn the game streams the board in at one mip below full detail; in the menu it loads it whole.

The mod's texture chunk was all **stored** (uncompressed) CAS blocks, so its encoded size exceeded its decoded size (8 header bytes per block). The game's own Natural Blank chunk is 22 Kraken blocks; Studio's chunks are Kraken with the odd stored block. The likely mechanism: the engine reads a streamed range's encoded bytes into the end of the destination buffer and decodes in place, which only works when the encoded data is smaller.

**Second test, Kraken blocks, same streaming layout: no crash, but a hang.** The menu and the new thumbnail worked, but with the deck equipped at launch the world stayed black, the skater never spawned and pause stopped working: the load waited forever, exactly what ReSkate's merge notes about cosmetic textures streamed from a mod. So the streaming itself is the problem, not just the block format. Third build: the same texture written **whole** (`Patch.make_texture_whole`). Before it was tested, the new-deck work found the real defect: **texture header bytes 0-7 are the encoded-chunk offsets of mips 1 and 2**, and all three builds kept the stock deck's values, pointing into the game's own compressed data. Fourth build (installed for testing): whole, Kraken, and correct mip offsets.

Rules adopted: **compress chunks with Oodle Kraken** from the game's own `oo2core` (`fbw.encode_cas`, level 4, stored only for blocks that do not shrink, as ReSkate's `encode_cas` and the game do), and **write new deck textures whole** (first mip 0, nothing streamed), as Studio does for cosmetics. A Kraken-compressed rebuild of the same replacement (same streaming layout as stock) is ready to confirm the cause in game.

## Container rules (all verified against real files)

- **`layout.toc`**: the game's `Data/layout.toc` with bytes 8 to 0x22C zeroed (its signature). This reproduces the cosmetic mods' file exactly.
- **`initfs_win32`**: a byte-for-byte copy of the game's `Data/initfs_Win32`. Optional: three installed mods leave it out.
- **`Win32/items.toc`**: a patch TOC (flags 3, uncompressed names, zeroed envelope) listing only the bundles the mod changes plus the chunks it adds. The Python port of ReSkate's writer (`tools/re/fbw.py`) reproduces 106 of 107 Studio TOCs byte for byte (the one mismatch is a map's level TOC).
- **The archive**: `Win32/configurations/layout/initialinstallpackage/cas_01.cas`, addressed as patch = 1, install chunk 3162564249, archive 1.
- **Bundle manifest**: stored uncompressed in the cas, as the first file of the bundle's region.
- **Manifest SHA-1s**: the SHA-1 of each payload's encoded cas bytes. For a chunk that is resident only from a logical offset, the SHA-1 of the whole TOC chunk instead.
- **Partially resident textures**: the bundle's file entry points into the TOC chunk's bytes, at the block that decodes to the logical offset. Writing 64 KiB blocks lines this up automatically (0x150000 is 21 blocks).
- **CAS blocks**: 8-byte header (decoded size, compression, encoded size), at most 64 KiB decoded each. Raw (0x00) works; the game itself uses Oodle Kraken (0x11) from its own `oo2core_9_win64.dll`.
- **`.reskate-studio-patch`**: `ReSkate Studio native Patch v1`, then `skate_sha256=<SHA-256 of Skate.exe>`, newline-terminated. ReSkate refuses a mod that ships game data without it. Whether a third-party tool may write it is an open question for the ReSkate devs.
- **`reskate-build.json`**: `{"schema":1,"tool":...,"version":...,"built":...}`; the tool name is display-only.

## Menu thumbnails

Each deck item has two menu thumbnails, `thumbnail/tool/own_<item id, lower case>` (256 x 256, the list tile) and `..._lrg` (768 x 768), both BC7 sRGB with one mip. They are pictures of the deck rendered from below, standing up, on a transparent background (RGB is black where alpha is 0); the menu draws them over its grey tile. Their texture resources sit in `cas_main_sharedbundle`, but their pixel chunks live only in the TOC's chunk table, not in the bundle: replacing a thumbnail is one TOC chunk entry, and the shared bundle does not need to ship. (There is also an older 256 px `<deck>_thumb` texture, a flat swatch of the art; it was left alone and its use is unknown.)

Caveman renders new thumbnails so they match the vanilla ones (`tools/re/thumbs.py`):

- **Where the art lands**: in the 768 px render the deck spans x 287 to 481 and y 26 to 750. Columns map linearly from art x 14 to 498. Rows follow a curve, because the nose and tail bend away from the camera: through the middle each thumbnail row shows about 2.5 art rows, more toward the ends. The curve was fitted by matching the rows of real thumbnails to their decks' art across five stock decks (median residual 0.4 px) and is stored in `assets/thumbnail_mapping.json`.
- **How it is lit**: per pixel and channel, `thumbnail = art x light + sheen`, fitted by least squares over 40 stock decks read from the local game at build time (nothing from the game is shipped). The light is smoothed away from the edge, because the game's renders carry blocky shading noise that would otherwise average into a faint grid; the sheen carries the gloss, the rim and the light scratches the vanilla renders show.
- **Accuracy**: on 129 stock decks left out of the fit, the rendered thumbnail matches the game's real one to a median error of 6.9 out of 255 (8.2 at the 90th percentile). Two stock decks (00041, 00042) do not match their own art at all and are excluded.

## The Caveman front end (`caveman.py`)

`python caveman.py [--game E:\Games\ReSkate] [--decks <folder>]` reads a folder of PNGs (by default `Caveman Decks` in the game folder) and prepares each as a deck. Building them into a mod comes with milestone 2. The rules:

- **One PNG is one deck, named after the file.** Names keep ASCII letters, digits, hyphens and spaces (the game's own keys use hyphens, as in `sci-fi`); accents are simplified (`Café` becomes `Cafe`) and other characters dropped (`Mr. Smith` becomes `Mr Smith`); `_` and `.` count as spaces. `--prefix "000 - "` puts text before every name (for testing: the decks list first).
- **Subfolders prefix their decks' names**: `000\Cool Deck.png` is "000 - Cool Deck", and nesting stacks (`000\Anime\Cool Deck.png` is "000 - Anime - Cool Deck"). The `previews` folder Caveman writes, and folders starting with `.`, are skipped. A folder like `000` is the easy way to sort decks to the top of the in-game list (Trevor's idea; verified on a three-deck build, 17,214 checks).
- **Effects by suffix**: `Cool Deck.gloss.png` is the "Cool Deck" art with the gloss preset, built as its own deck named "Cool Deck Gloss" (a variant needs its own item ID, or ReSkate drops one of the two). Only known effect names count as suffixes; none are implemented yet (`gloss`, `shimmer` and `foil` are reserved, and such files are skipped with a note).
- **Name clashes warn at build time**: two files that end up with the same name (the second is skipped), and a name already used by the game or another installed mod (ReSkate would load only one of them).
- **Any image size is fitted**: art within 3% of 1:4 is scaled; anything else is scaled to cover the deck and centred, with the overflow cropped (fill). Art drawn sideways (between 3:1 and 5:1, wider than tall) is turned clockwise, so its left edge becomes the top of the deck. Art under 256 px wide builds with a warning that it will look soft. Transparent parts show natural wood (the stock Natural Blank art).
- **A preview per deck** goes to `previews/` beside the PNGs: the fitted art with everything off the board darkened.

**Launching through `Caveman.exe`** (Trevor's personal setup, 2026-10-20): `Caveman/launcher/` is a small Rust program (no dependencies, about 850 KB) that sits beside `ReSkateLauncher.exe` in the game folder. It runs `caveman.py --if-changed` (the script's path from `Caveman.txt` beside the exe, else the project default), then starts ReSkate's launcher with the same arguments and exits. It skips the build while the game is running, and on any failure explains and waits for Enter before starting ReSkate anyway. `--if-changed` compares a fingerprint of every PNG (path, size, time), the options and Caveman's own code with `caveman-fingerprint.txt` in the built mod: an unchanged launch costs about half a second. The thumbnail lighting fit is cached in `%LOCALAPPDATA%\Caveman\cache`, keyed by `Skate.exe`, so a rebuild takes about 7 s instead of 12. With no decks left, the generated mod is removed (only a folder whose build stamp names Caveman). The wrapper needs Python, so it is for Trevor's machine; it cannot ship through Thunderstore (the launcher installs content only), though the decks it builds can.

The artist template, `assets/Caveman Deck Template 1024x4096.png` (from `tools/re/make_template.py`), is a guide layer at twice the game's size: the bleed darkened, the trim line in red, a safe zone 32 px inside it in cyan, and the truck baseplates and bolt holes in orange.

## Trucks

Read against the grid of the test pattern as it showed on the board in game (the grid is printed on the board's own surface, so it gives art coordinates directly), in the game's 512 x 2048 pixels:

| | Baseplate | Bolt holes |
|---|---|---|
| Front (top) truck | x 201-308, y 472-638 | x 213 and 297; y 501 and 625 |
| Rear (bottom) truck | x 202-309, y 1445-1613 | x 214 and 299; y 1458 and 1582 |

The trucks sit symmetrically about y = 1042, a little below the art's centre. The grip-tape thumbnails show the same bolt pattern centred on the same row (its spacing comes out about 4% tighter through the thumbnail mapping, which was fitted on the bottom view). The community deck tool's guides (truck centres at 22.5% and 77.5% of the height) are, by its own label, approximate. In game, Trevor's first real deck (Dan Da Dan) confirmed the trim line by eye.

## Display names

A deck's name in the game's list is its item ID (the `Key` field of the item EBX; `CustomizationItemId` holds a category code such as `custom_0015`), shown by ReSkate with the leading `Own_` removed and underscores turned into spaces (ReSkate's `Extension/Customization/local_cosmetic_catalog.cpp`). Studio's `Own_Supreme_Spaghetti_Deck` shows as "Supreme Spaghetti Deck". Only the game's own items get titles from the content cache (the stock `Own_DeckGraphic_Gen_Popsicle_00001` is "Natural Blank"); a player's profile can override a title, but a mod has no title field of its own.

So Caveman names each deck after its PNG: `Cool Deck.png` becomes the item `Own_Cool_Deck` and shows as "Cool Deck". Rules that follow from this:

- spaces become underscores in the ID; a literal underscore cannot be shown
- the ID travels into asset paths, so it is limited to ASCII letters, digits and spaces (other characters are dropped or transliterated)
- case is kept as typed
- two items with the same ID collide (ReSkate leaves one out: "another item has the same identity hash"), so names must be unique across every installed mod

## Milestone 1: replace a stock deck's art (works in game)

`tools/re/build_replace.py` builds a mod that replaces the art of one stock deck with a PNG, board and menu thumbnails both, generated entirely from the local game install in about 10 seconds:

1. Copies the stock deck's bundle region from the game's `items.toc`.
2. Encodes the PNG as BC7 sRGB with 12 mips, using `tools/re/bc7.py` (a small mode-6 encoder; Pillow cannot write BC7).
3. Writes the full chunk and a patched manifest (new SHA-1s) into `cas_01.cas`, and points the bundle's changed entries at them. The texture header stays the game's.
4. Renders the two menu thumbnails (see "Menu thumbnails") and points their TOC chunks at the new data.
5. Writes the patch TOC, `layout.toc`, `initfs_win32`, the build stamp, `manifest.json`, `README.md` and `icon.png`.

Reading the result back: the TOC parses, all 40 manifest hashes match their payloads, the resident tail equals the full chunk from 0x150000, and the decoded art matches the source PNG to a mean error of 0.24 out of 255.

**The first in-game test (BC3) loaded and showed the pattern, but washed out to pale cream.** That build wrote BC3 data and changed the header's format ordinal from 67 to 61. Decoding that BC3 data as BC7 reproduces the in-game look exactly, so **the game ignores the format ordinal in the 180-byte texture header** and takes the format from somewhere else (the texture asset EBX or the bundle's chunk metadata are the likely places). Rule: write exactly the format the game's texture already has, which for decks means BC7 sRGB. The rebuilt mod uses real BC7 and an untouched header.

BC7 vs BC3 on the same images (PSNR, higher is better): test pattern 41.0 vs 34.6 dB, stock flowers deck 44.5 vs 33.6 dB, Supreme photo deck 38.4 vs 28.2 dB. The encoder takes about 1.5 s for a 512 x 2048 deck.

The test build, `Caveman-Test`, replaces stock deck `deck_gen_graphic_00001` (item `Own_DeckGraphic_Gen_Popsicle_00001`, "Natural Blank", the plain natural-wood deck) with `assets/test_pattern.png`, board and thumbnails. It is installed at `E:\Games\ReSkate\Mods\Caveman-Test` (it replaced the earlier `DeckDrop-Test`); delete that folder to remove it.

## Next steps

1. **Confirm the BC7 rebuild in game**: the Natural Blank deck should now show the full-colour test pattern.
2. **A faster or better BC7 encoder** for the shipping tool (Rust has `intel_tex_2`), with `bc7.py` as the reference.
3. **Milestone 2, new decks instead of replacements**: an EBX reader and writer (port `Engine/Resource/ebx_document.cpp` and `ebx_writer.cpp`), then clone a deck's `_ap`, texture asset, item and thumbnail EBX under new names and GUIDs, and add them to `itemmastercollection` and `cas_main_bundlereftable` (ReSkate's `bundle_ref_table.cpp` reads and writes the latter).
4. **Thumbnails**: done for the replacement test (`Caveman-Test` replaces Natural Blank's two thumbnails); awaiting the in-game check. New decks will need their own thumbnail resources, cloned from a stock deck's.
5. **Later, effects as sidecar files** (Trevor's idea): next to `Cool Deck.png`, an optional `Cool Deck.gloss.png` or `Cool Deck.shimmer.png` (white where the effect applies) that Caveman turns into the deck's effect texture. The lead is the second per-deck texture slot (`role3_c1b71b8a...`, BC7 linear 512 x 2048), which mirrors the stock `_osk` mask the deck shader reads, likely occlusion, smoothness and a metal or foil channel. First step: decode the stock `_osk` and `_ny` textures and the community packs that ship that slot, and work out what each channel does. The community deck tool's effects (lenticular, depth, "Ghost Rare") go further, with custom shaders built through Studio.

## Tools (`tools/re/`, Python 3, Pillow, read-only except `build_replace.py`)

Run each with `python -I`. They are ports of ReSkate's GPL-3.0 code, so they are GPL-3.0 as well.

| Script | What it does |
|---|---|
| `fb.py` | readers: TOC (incl. Huffman names), bundle regions, binary bundle manifests, cas blocks (Oodle via the game's DLL), texture headers |
| `fbw.py` | writers: raw cas blocks, bundle regions, patch TOCs with perfect hashes |
| `bc7.py` | BC7 encoder (mode 6, numpy, RGB or RGBA), checked against Pillow's BC7 decoder |
| `gamedata.py` | the installed game's bundles and textures, decoded to images |
| `patch.py` | builds a mod: replaces texture pixels (bundle and TOC chunk entries, manifest hashes) and writes the mod folder |
| `thumbs.py` | renders menu thumbnails: maps the art onto the board and fits light and sheen from stock decks |
| `fit_thumb_trim.py`, `fit_thumb_rows.py` | the fits behind `assets/thumbnail_mapping.json` |
| `dump_mod.py` | lists every bundle and asset a mod ships, with locations and texture headers |
| `scan_decks.py` | deck texture formats and sizes across every installed mod |
| `base_textures.py` | texture headers in one bundle of the game or a mod |
| `extract_texture.py`, `stock_decks.py` | decode textures (one bundle, or all 334 stock deck graphics) to PNG |
| `make_pattern.py` | the outline, template and test pattern in `assets/` |
| `build_replace.py` | milestone 1: a stock-deck replacement mod from a PNG, board and thumbnails |
