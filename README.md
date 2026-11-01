# Caveman

Your own decks, wheels and grip tape in *skate.*, through [ReSkate](https://reskate.gg): drop a PNG in a folder and ride it.

**Just want to install it?** Go to **[trevorbrown.dev/gh/caveman](https://trevorbrown.dev/gh/caveman)**: one download, three steps.

[![CI](https://github.com/TrevorBrown-dev/caveman/actions/workflows/ci.yml/badge.svg)](https://github.com/TrevorBrown-dev/caveman/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/TrevorBrown-dev/caveman?label=release)](https://github.com/TrevorBrown-dev/caveman/releases/latest) [![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE) [![Ko-fi](https://img.shields.io/badge/Ko--fi-support-e8a24a)](https://ko-fi.com/trevorbrowndev)

## What it does

- **Decks from PNGs.** `Caveman\Decks\Cool Deck.png` becomes the deck "Cool Deck" in the game's list, with a thumbnail. A suffix picks a finish (`.foil`, `.chrome`, `.holo-frost`, `.glitter`, ...), `.spot` and `.emboss` shape it, and a settings file beside the PNG does the rest. Subfolders prefix the name, so `000\` sorts yours to the top.
- **Wheels, grip tape, grip patterns.** The same folders for `Wheels`, `Grips` (a picture printed on the grip, or a plain colour) and `Patterns` (a cutout through which the wood shows).
- **A launcher and an app.** `Caveman.exe` builds what changed and starts ReSkate. The app is a launcher in the shape of ReSkate's own, with a **Studio** (the library with a switch per item, an editor, a 3D preview of the real board), **Packs** (a Thunderstore-ready package of your decks and wheels that needs nothing of Caveman to play), **Plugins** and **Play**.
- **Live things, through the optional FX DLL.** A picture or OBS's video on the board you ride, soundbars on the deck, and a see-through, always-on-top **board cam** window that follows the real board's pose for your stream.
- **Nothing of the game ships.** Caveman reads the player's own install and writes ordinary ReSkate mods next to the others; everything it makes is regenerated from that install.

## Install

Download `Caveman-win64.zip` from [trevorbrown.dev/gh/caveman](https://trevorbrown.dev/gh/caveman) and unzip it into your ReSkate folder (the one with `ReSkateLauncher.exe`). You get `Caveman.exe` beside the launcher, a `Caveman` folder with the app (`caveman-app.exe`), the command line, the trainer and the FX DLL, and a `README.txt` that says the same. Windows asks once about an unsigned program: *More info*, *Run anyway*. Requires Windows 10 or 11 and *skate.* on PC with ReSkate 2.1.3. There is no installer on purpose: what goes where is plain to see, and nothing runs that you did not start.

Everything Caveman writes stays in your ReSkate folder (`Mods\Caveman-*`, the `Caveman` folder) and in `%APPDATA%\dev.caveman.app`. To remove it, delete those.

## How it works, briefly

A build clones a host item from the game's own files (a deck, a wheel, a grip, a cutout), gives the clone your texture (BC7, written whole, every mip, Kraken-compressed) and new odd resource ids, appends it to the item lists and the bundle reference table, and writes the result as a mod that ReSkate merges at launch. The build is incremental: a fingerprint of the drop folders decides whether anything runs, and an item cache keyed by content keeps unchanged items byte for byte. `docs/deck-format.md` is the long version; the other files in `docs/` are the stories of how each part was worked out, crashes included.

The FX DLL hooks the game's renderer to put your picture on the ridden board's own texture, shares video frames through a ring in shared memory, and never calls an engine function off the engine thread or blocks a game thread (`AGENTS.md` has the rules).

## Building from source

Windows, Rust stable with the MSVC toolchain (`rust-toolchain.toml` picks the host's stable with rustfmt and clippy; Visual Studio Build Tools provide the linker), and for the app pnpm 11 with Node 24.

```
cargo build --release                      # caveman.exe, caveman-trainer.exe, caveman_fx.dll, caveman-launcher.exe (= Caveman.exe)
cd app && pnpm install && pnpm tauri build --no-bundle    # caveman-app.exe, with the front end inside it
```

The GNU toolchain (`x86_64-pc-windows-gnu`) works too, with a few MinGW tools on PATH for the app that `app/toolchain/fetch-toolchain.ps1` fetches (about 9 MB); point `rust-toolchain.toml` at it or use a `rustup override`. `app/README.md` has the development loop (`pnpm tauri dev`, the mock, the DevTools port).

Checks, the same ones CI runs:

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd app && pnpm check
```

Tests that read the game are `#[ignore]` and look for it at `E:\Games\ReSkate`; `cargo test -- --ignored` runs them on a machine that has it.

## The repository

| Path | What it is |
|---|---|
| `crates/caveman-core` | Readers and writers for the game's formats, the texture encoders, the item build, thumbnails, the board model, shader patching |
| `crates/caveman-cli` | The `caveman` command: drop folders, names, the incremental build, packs |
| `crates/caveman-fx` | The FX DLL |
| `crates/caveman-trainer` | `caveman-trainer`: injects and drives the DLL, reads the game from outside |
| `launcher/` | `Caveman.exe` |
| `app/` | The desktop app (Tauri 2, Svelte 5, three.js) |
| `docs/` | Design notes and working records; `docs/index.html` is the download page |
| `tools/re`, `caveman.py`, `prototypes/` | The frozen Python toolkit the formats were first worked out with; nothing builds from it |

## Contributing

Pull requests are welcome, from people and from people working with coding agents alike; `CONTRIBUTING.md` has the loop and `AGENTS.md` the rules and the map. Issues labelled `good first issue` are a place to start. Security problems go through `SECURITY.md`.

## License and credits

Caveman is free software under the [GPL-3.0](LICENSE). Its readers and writers for the game's formats were ported from [ReSkate](https://reskate.gg)'s own GPL-3.0 source, and ReSkate's mod loading is what makes any of this work. *skate.* is made by Full Circle and published by EA. Caveman is not affiliated with EA, Full Circle or the ReSkate team, and ships none of their data.

If Caveman made your board yours, [a coffee](https://ko-fi.com/trevorbrowndev) is always welcome.
