# Lays out what a player unzips into the ReSkate folder, from the workspace's release builds:
#
#   Caveman.exe                    the launcher (caveman-launcher.exe renamed): builds, then starts ReSkate's launcher
#   Caveman\caveman-app.exe        the app (Studio, Packs, Plugins, Play)
#   Caveman\caveman.exe            the command line
#   Caveman\caveman-trainer.exe    puts the FX DLL into the game and drives it
#   Caveman\caveman_fx.dll         the FX DLL
#   Caveman\WebView2Loader.dll     only when the build made one (GNU toolchain builds load WebView2 through it)
#   README.txt                     what this is and where it goes, at the top of the zip
#
# The same script serves CI (uploaded as an artifact) and the release workflow (zipped). Run from the repository root.
param([string]$Out = "dist")

$ErrorActionPreference = "Stop"
$release = Join-Path (Get-Location) "target\release"
$inner = Join-Path $Out "Caveman"
New-Item -ItemType Directory -Force -Path $inner | Out-Null

Copy-Item (Join-Path $release "caveman-launcher.exe") (Join-Path $Out "Caveman.exe")
foreach ($name in "caveman-app.exe", "caveman.exe", "caveman-trainer.exe", "caveman_fx.dll") {
    Copy-Item (Join-Path $release $name) (Join-Path $inner $name)
}
$loader = Join-Path $release "WebView2Loader.dll"
if (Test-Path $loader) {
    Copy-Item $loader (Join-Path $inner "WebView2Loader.dll")
}

@(
    "Caveman for ReSkate",
    "===================",
    "",
    "Your own decks, wheels and grip tape in skate. through ReSkate: drop a PNG in a folder and ride it.",
    "Open source (GPL-3.0): https://github.com/TrevorBrown-dev/caveman",
    "",
    "INSTALL",
    "",
    "Unzip everything into your ReSkate folder, the one that holds ReSkateLauncher.exe, so that",
    "Caveman.exe sits beside ReSkateLauncher.exe and the Caveman folder sits next to Mods:",
    "",
    "  <your ReSkate folder>",
    "    ReSkateLauncher.exe          (ReSkate's own)",
    "    Caveman.exe                  builds your decks, then starts ReSkate's launcher",
    "    Caveman\",
    "      caveman-app.exe            the launcher app: Studio, Packs, Plugins, Play",
    "      caveman.exe                the same build from a command line",
    "      caveman-trainer.exe        puts the FX DLL into the running game, for the plugins",
    "      caveman_fx.dll             the FX DLL (optional: only the plugins use it)",
    "      Decks\, Wheels\, Grips\, Patterns\   your PNGs go here (made on the first run)",
    "",
    "Windows asks once about an unsigned program the first time you start it: More info, then Run anyway.",
    "",
    "USE",
    "",
    "Put a PNG in Caveman\Decks and start the game with Caveman.exe, or open Caveman\caveman-app.exe and",
    "press Play. Either builds your art into the mod Mods\Caveman-Decks (Mods\Caveman-Shaders too when a",
    "deck needs it) and starts ReSkate. The deck shows in the game's deck list under the file's name.",
    "A shortcut to caveman-app.exe on your desktop or Start Menu is handy; make one if you like.",
    "",
    "UPDATE",
    "",
    "Close Caveman and the game, unzip the new version over the old one.",
    "",
    "REMOVE",
    "",
    "Delete Caveman.exe, the Caveman folder (move your PNGs out first) and the Mods\Caveman-* folders.",
    "Caveman's settings are in %APPDATA%\dev.caveman.app and %LOCALAPPDATA%\Caveman.",
    "",
    "Questions, problems and the source: https://trevorbrown.dev/gh/caveman"
) | Set-Content -Encoding ascii (Join-Path $Out "README.txt")

Get-ChildItem -Recurse -File $Out | ForEach-Object { "{0,12:n0}  {1}" -f $_.Length, $_.FullName.Substring((Resolve-Path $Out).Path.Length + 1) }
