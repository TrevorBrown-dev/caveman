# Lays out what a player unzips into the ReSkate folder, from the workspace's release builds:
#
#   Caveman.exe                    the launcher (caveman-launcher.exe renamed): builds, then starts ReSkate's launcher
#   Caveman\caveman-app.exe        the app (Studio, Packs, Plugins, Play)
#   Caveman\caveman.exe            the command line
#   Caveman\caveman-trainer.exe    puts the FX DLL into the game and drives it
#   Caveman\caveman_fx.dll         the FX DLL
#   Caveman\WebView2Loader.dll     only when the build made one (GNU toolchain builds load WebView2 through it)
#   Caveman\README.txt             where things go
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
    "",
    "Unzip this into your ReSkate folder, the one that holds ReSkateLauncher.exe, so that Caveman.exe",
    "sits beside it and this Caveman folder is next to Mods.",
    "",
    "  Caveman.exe            builds your decks and starts ReSkate's launcher",
    "  Caveman\caveman-app.exe  the full launcher: Studio, Packs, Plugins, Play",
    "  Caveman\Decks           drop deck PNGs here (made on the first run)",
    "  Caveman\Wheels, Grips, Patterns   the same for wheels, grip tape and grip patterns",
    "",
    "Install steps, questions and the source: https://trevorbrown.dev/gh/caveman"
) | Set-Content -Encoding ascii (Join-Path $inner "README.txt")

Get-ChildItem -Recurse -File $Out | ForEach-Object { "{0,12:n0}  {1}" -f $_.Length, $_.FullName.Substring((Resolve-Path $Out).Path.Length + 1) }
