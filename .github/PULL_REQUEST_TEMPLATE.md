<!-- Thanks for the pull request. Short is fine; the checklist is what the review needs. -->

## What this changes

<!-- One or two sentences: what a player or a developer gets, and why. Link the issue if there is one. -->

## How it was checked

<!-- CI runs fmt, clippy and the tests. Say what you checked beyond that, in game if the change touches what the game loads or runs. -->

- [ ] `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` are clean
- [ ] `cargo test` passes (and `pnpm check` in `app/` if the front end changed)
- [ ] Checked in game with ReSkate version ___ (or: no game-facing change)

## Rules that apply

- [ ] Nothing derived from the game's files is committed (built mods, textures, previews, statistics; see `AGENTS.md`)
- [ ] If this touches the FX DLL: engine functions only on the engine thread, nothing ever blocks a game thread
- [ ] `CHANGELOG.md` has a line if a player would notice the change

## Written with

<!-- Optional. Agents and assistants are welcome here; whoever opens the PR stands behind every line and has run the checks above. -->
