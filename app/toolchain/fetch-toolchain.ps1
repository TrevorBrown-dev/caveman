# Fetches the MinGW tools the app needs on the GNU Rust toolchain (about 8.6 MB from the MSYS2 mirror, pinned by
# SHA-256), unpacks them, and builds the `gcc -E` stand-in. See README.md here for why.
# Usage: powershell -ExecutionPolicy Bypass -File app\toolchain\fetch-toolchain.ps1 [-Dest <folder>]
# Then put <Dest>\mingw64\bin on PATH for app builds.

param([string]$Dest = "$env:LOCALAPPDATA\Caveman\toolchain")

$ErrorActionPreference = 'Stop'
$mirror = 'https://mirror.msys2.org/mingw/mingw64'
$packages = [ordered]@{
    'mingw-w64-x86_64-binutils-2.47-4-any.pkg.tar.zst' = 'e36c52a7fe1d104c2efd97dc74162386eabf8323f6aeb9b83aeb574b291a7da0'
    'mingw-w64-x86_64-zlib-1.3.2-2-any.pkg.tar.zst' = '9e75842a070ba648e986e12424e1c92c9d7d77200e85f6a34eeb600819f2e694'
    'mingw-w64-x86_64-zstd-1.5.7-2-any.pkg.tar.zst' = '1add6705b344664f6aca108c85f79ab5bdd9e1162662bb06a4cf40a34f6e0907'
    'mingw-w64-x86_64-gettext-runtime-1.0-1-any.pkg.tar.zst' = 'be68d7f260633284b910c588c6d82ee304a81c8817a686d2cd9df83f872c27af'
    'mingw-w64-x86_64-libwinpthread-14.0.0.r495.g63ebe6cdb-2-any.pkg.tar.zst' = 'e657ade501916ef2f0ff8b5f5d93dc65d39f1a0ec66cfee8206026b240f60518'
    'mingw-w64-x86_64-libiconv-1.19-1-any.pkg.tar.zst' = '21e334d0911f25de75d3e18e0697648bcecfa9658256d600cad0827d719c2f35'
}

$pkgs = Join-Path $Dest 'pkgs'
New-Item -ItemType Directory -Force -Path $pkgs | Out-Null
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
foreach ($name in $packages.Keys) {
    $file = Join-Path $pkgs $name
    if (-not (Test-Path $file)) {
        Write-Host "downloading $name"
        # Windows' own curl: the mirror shows a browser check to PowerShell's web client (its user agent says Mozilla)
        & "$env:SystemRoot\System32\curl.exe" -fsSL -o $file "$mirror/$name"
        if ($LASTEXITCODE -ne 0) { throw "could not download $name" }
    }
    $hash = (Get-FileHash -Algorithm SHA256 -Path $file).Hash.ToLower()
    if ($hash -ne $packages[$name]) {
        Remove-Item $file
        throw "$name does not match its pinned SHA-256 (got $hash); removed it"
    }
    # Windows' own tar reads .tar.zst; the packages unpack under mingw64\
    & "$env:SystemRoot\System32\tar.exe" -xf $file -C $Dest mingw64
    if ($LASTEXITCODE -ne 0) { throw "could not unpack $name" }
}

$bin = Join-Path $Dest 'mingw64\bin'
Write-Host 'building the gcc -E stand-in'
& rustc -O --edition 2021 -o (Join-Path $bin 'gcc.exe') (Join-Path $PSScriptRoot 'cpp_passthrough.rs')
if ($LASTEXITCODE -ne 0) { throw 'rustc failed' }

foreach ($tool in 'as.exe', 'dlltool.exe', 'windres.exe', 'gcc.exe') {
    if (-not (Test-Path (Join-Path $bin $tool))) { throw "missing $tool" }
}
Write-Host "ready: put $bin on PATH for app builds"
