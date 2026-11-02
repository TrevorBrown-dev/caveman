# Contributing to Caveman

Thanks for being here. Caveman is one person's project made with a lot of help from coding agents, and it stays open to both: pull requests from people, from people working with agents, and from agents working for people are all welcome. What matters is the same in every case: the change works in the game, the checks pass, and whoever opens the pull request stands behind it.

## Before you start

- Read `AGENTS.md`. It is the map of the repository and the rules that keep the game from crashing and the repository legal. It is written for agents, and it is the shortest honest introduction for anyone.
- Look at the open issues. Ones labelled `good first issue` are scoped and explained; `help wanted` ones are bigger and open to a plan. For anything large, open an issue first and say what you intend, so the work is not done twice.
- You need Windows, Rust stable, and for the app pnpm and Node. To check a change in the game you need *skate.* with ReSkate installed (the tests that read the game look for it at `E:\Games\ReSkate`). Without the game you can still build, run the unit tests, and work on the app's front end against the mock (`pnpm dev:mock`).

## The loop

1. Fork, branch from `main`, make the change.
2. Run what CI runs: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build -p caveman-fx` (the host tests load that DLL), `cargo test --workspace`, and `pnpm check` in `app/` if you touched the front end.
3. If the change affects what the game loads or runs, try it in the game, the way a player would (an item equipped in the menu *and* one equipped at launch load differently). Install test mods as new folders under `Mods`, with the game closed, and remove them by moving them out.
4. Add a line to `CHANGELOG.md` if a player would notice.
5. Open the pull request. The template asks what changed, how it was checked, and which ReSkate version you checked it against. Small, focused pull requests get merged; a pull request that does three things gets three rounds of questions.

Cargo never cleans `target/` by itself, and it grows by a full copy with every Rust update and dependency bump. `bash scripts/tidy-target.sh` clears the caches when that happens or when the folder passes 12 GB; run it now and then.

Reviews are kind and specific. Expect questions about the game-facing parts; the history of this project is a list of crashes that each came from one plausible-looking shortcut.

## Working with agents

Using Claude Code, Codex, Copilot or anything else to write or review code here is fine and encouraged. A few things make it work:

- Point the agent at `AGENTS.md` (Claude Code picks it up through `CLAUDE.md` by itself). The rules there are the ones that were learned the hard way.
- You are the author of what you submit. Read the diff, run the checks, and try it in the game when it is game-facing. "The agent said it works" is not a test.
- Say so in the pull request if you like (the template has a line for it); it is not required, and it never counts against a change.
- Agents must not commit anything derived from the game's files, and must not touch a running game without saying so. Both rules are in `AGENTS.md`.

Fully automated pull requests (no human in the loop) are not accepted yet: open an issue and describe what the automation would do.

## What the repository will not take

- Game data of any kind, however small: textures, meshes, bundles, statistics computed from them, previews rendered from them. Caveman builds everything from the player's own install at run time.
- Anything that bypasses ReSkate's own mod loading or touches the game's files in place.
- Anything that only builds on one Windows toolchain: MSVC is what CI and the releases use, and the GNU toolchain (with the small MinGW kit in `app/toolchain`) has to keep working too.

## Releases

The maintainer tags releases from `main` (`v0.2.0`, `v0.3.0`, ...). GitHub Actions builds the zip and publishes the release; the download page picks it up by itself. Version numbers live in `Cargo.toml` (`workspace.package.version`) and `app/package.json`, and the changelog entry's heading is the version.

## Conduct

The Contributor Covenant (`CODE_OF_CONDUCT.md`) applies. Security problems go through the private report described in `SECURITY.md`.
