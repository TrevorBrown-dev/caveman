# Security

Caveman writes mods into your ReSkate folder, and its optional FX DLL is loaded into the running game by `caveman-trainer`. Both deserve care, and problems with either are taken seriously.

## Reporting

Please report anything that could harm a player's game, files or PC privately, through GitHub's private vulnerability reporting on this repository (**Security** tab, **Report a vulnerability**), rather than in a public issue. Include what you ran, what happened, and how to reproduce it. You will get an answer within a week, and a fix or a plan in the open once there is one.

Crashes, hangs and wrong output that do not cross a trust boundary are ordinary bugs: open an issue.

## What Caveman does, so you can judge a report

- The build reads the game's files and writes only under `Mods\Caveman-*`, the `Caveman` folder beside the game, `%APPDATA%\dev.caveman.app` and `%LOCALAPPDATA%\Caveman`. It never changes the game's own files.
- `caveman-trainer` and the FX DLL read and write the running game's memory and hook its renderer. They run only when you ask (the Plugins page, Play with the switch on, or the command line), only against `Skate.exe` with ReSkate loaded, and come back out with `caveman-trainer unload`.
- Releases are built by GitHub Actions from the tagged source; the workflow is `.github/workflows/release.yml`. The exes are not code-signed yet, so SmartScreen warns on first run.
- Nothing phones home. The app reads GitHub's public releases API only on the download page.
