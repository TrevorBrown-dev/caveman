# Deck settings files

A deck's effects can be set in a small JSON file beside its PNG instead of (or as well as) filename suffixes: `Cool Deck.caveman.json` beside `Cool Deck.png` (the PNG's file name with `.png` replaced by `.caveman.json`, so `Cool Deck.foil.png` pairs with `Cool Deck.foil.caveman.json`). The Caveman app writes these; they can also be written by hand. Read by the Rust engine (`caveman_core::settings`); the Python toolkit only knows suffixes.

- **When a settings file is there it decides the deck's effects**; the suffixes in the PNG's name are then only part of its file name (they are still peeled off the name).
- **When there is none**, the suffixes apply as before (`Cool Deck.holo-frost.spot.png`).
- A settings file that cannot be used (bad JSON, an unknown field, a value out of range, another version) leaves the deck out with a message saying why, rather than building it without the effects asked for.

## Schema (version 1)

Every part except `version` is optional; `{"version": 1}` is a plain deck.

```json
{
  "version": 1,
  "name": "Cool Deck",
  "finish": { "kind": "holo", "mode": "frost", "spot": false },
  "emboss": { "strength": 4.0 },
  "shader": { "effect": "cycle", "speed": 0.25, "glow": 0.5 },
  "board": { "effect": "aura", "colour": "#00ffff" }
}
```

(`layers`, a deck's own texture files, is described below; it replaces `finish` and `emboss`.)

| Field | Values | Default | What it does |
|---|---|---|---|
| `version` | `1` | (required) | The schema version. |
| `name` | text | the file name | The deck's name in game (a subfolder's prefix and `--prefix` still go first). With a name, the effects are not added to it; without one, the deck is named like a suffixed one ("Cool Deck Holo Frost"). |
| `finish.kind` | `gloss`, `matte`, `foil`, `chrome`, `glitter`, `holo` | (no finish) | The deck's finish (one per deck). |
| `finish.mode` | `pearl`, `cellular`, `frost`, `vertical`, `glitter` | `pearl` | The holo pattern (holo only). |
| `finish.spot` | `true`, `false` | `false` | The finish on the light parts of the art only. |
| `emboss.strength` | more than 0, at most 20 | 4 | Raised print: the light parts of the art stand up from the deck. |
| `shader.effect` | `none`, `cycle` | `none` | `cycle`: the colours cycle through the hues. Needs the Caveman-Shaders mod (built automatically). |
| `shader.speed` | 0 to 2 | 0.25 | Cycle speed in turns a second. |
| `shader.glow` | 0 to 1 | 0 | The print glows (0 off); `cycle` and `glow` suffixes give 0.25 turns a second and glow 128 / 255 (about 0.5). The glow grows fast: the emissive is the colour times 2 to the power of 12 x glow, minus 1. |
| `board.effect` | `none`, `aura`, `nyan` | `none` | An effect attached to the board: `aura` a light and trails, `nyan` the solid rainbow trail (needs Caveman-Shaders). One per deck. |
| `board.colour` | `"#rrggbb"` | the art's most vivid colour | The aura's colour (aura only). |
| `board.steps` | more than 0, at most 256 | 8 | The rainbow's step setting (nyan only; it has no visible effect yet, see `knowledge/trails.md`). |
| `layers.osk` | a PNG path | (made by the finish) | The deck's own mask (R occlusion, G gloss, B metal), used as it is. |
| `layers.ny` | a PNG path | (made by emboss) | The deck's own normal map. |
| `layers.rgb` | a PNG path | (made by holo) | The deck's own holo map (red where the deck is holo). Without `layers.osk`, the game's default mask goes with it. |
| `layers.foilmode` | `pearl`, `cellular`, `frost`, `vertical`, `glitter` | `pearl` | The holo pattern of `layers.rgb` (needs `layers.rgb`). |

## Animated decks

A `.gif` (or, with ffmpeg and ffprobe on PATH, a video: mp4, webm, mov, mkv, avi, m4v) in the drop folder is an animated deck: its frames go into one atlas texture in the holo map slot and Caveman's deck shader plays them from the game clock (`knowledge/animated-decks.md`). The first frame is the deck's art (thumbnails, and the still a game without Caveman-Shaders shows). Suffixes and the settings file work as for a PNG (`Cool.glow.gif`; `Cool.caveman.json` with the clip's extension replaced); `cycle` and a holo finish are left out with a note. The `animation` part sets the frames:

| Field | Values | Default | What it does |
|---|---|---|---|
| `animation.fps` | 2 to 60 | the clip's own rate | Frames a second (rounded to an even number). |
| `animation.frames` | 2 to 254 | 64 | The most frames in the atlas; a longer clip plays at a lower rate so the whole loop still fits. |
| `animation.size` | `auto`, `full`, `half`, `quarter` | `auto` | A frame's size in the atlas: 512 x 2048, 256 x 1024 or 128 x 512; `auto` is the largest whose grid fits the budget (16 frames full, 64 half, 254 quarter within 16 MiB). |
| `animation.budget_mib` | 1 to 32 | 16 | The atlas's first mip at most (one byte a texel; the mips add a third). |
| `animation.seconds` | 0.5 to 600 | 30 | How much of a video is read. |

`caveman testclip out.gif --frames 8 --fps 2 --size 512x2048` writes a test animation (frame numbers, a sweeping bar, a tick gauge) to drop into the folder.

## Layers: a deck's own textures

`layers` gives the deck's texture slots from files instead of from the finish and emboss: what `caveman import` writes for a deck taken from another mod, so it builds as it was. Paths are relative to the deck PNG's folder (no drive, no `..`); `caveman import` puts them in a `.layers` subfolder, which the drop-folder scan skips (the files are not decks) but `--if-changed` and the item cache watch (a changed file rebuilds the deck). Any size works (resized to 512 x 2048; a normal map is made unit length again). `layers` replaces `finish` and `emboss` (a file with both is refused); `shader` and `board` still apply on top (a shader marker goes into the given holo map). A deck with its own mask is built on the gold deck's preset, which switches the mask's metal channel (B) on; a deck with a holo map on the Primitive holo deck's.

```json
{
  "version": 1,
  "name": "DGK - Stevie High Gloss Holo",
  "layers": {
    "osk": ".layers/DGK - Stevie High Gloss Holo.mask.png",
    "ny": ".layers/DGK - Stevie High Gloss Holo.normal.png",
    "rgb": ".layers/DGK - Stevie High Gloss Holo.holo.png",
    "foilmode": "pearl"
  }
}
```

## Importing decks from installed mods

`caveman import --mod <folder> [--mod ...] | --all [--to DIR] [--force]` copies the decks of installed mods (the folders in `<game>\Mods`; the mods are only read) into `<decks>\Imported\<pack>\<deck>.png` (`--to` for another folder), one settings file each naming the deck as the game shows it, with its own mask, normal map and holo map (and foil mode) in `.layers\` when it has them. Left out: a mask or normal map that is a copy of the game's default (the deck then builds on the plain deck, as it looked); a texture in none of the mod's bundles (reported). A mask whose preset never switched metal on is written with its metal channel cleared (so it keeps the look on the gold deck's preset). Decks imported before are left as they are unless `--force`. Each pack folder is a name prefix in game ("Imported - Supreme Deck Pack - Supreme Spaghetti Deck"). Remove the original mods yourself once the imports look right: two mods with the same decks would just show each deck twice under different names.

## Suffixes and settings

Each suffix has a settings equivalent: `gloss`, `matte`, `foil`, `chrome`, `glitter` and `holo`, `holo-cellular`, `holo-frost`, `holo-vertical`, `holo-glitter` are `finish`; `spot` is `finish.spot`; `emboss` is `emboss`; `cycle` and `glow` are `shader`; `aura` (with a hex colour such as `.aura.00ffff`) and `nyan` are `board`. `caveman --json` reports every deck's effects in this schema (its `settings` field), so a settings file can be seeded from a suffixed deck.

## Wheels

Wheels have no settings file yet: `Name.ff0000.png` in `Caveman Wheels` is the wheel "Name" in that colour (white without one).
