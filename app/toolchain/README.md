# Build tools for the app on the GNU toolchain

This machine builds Rust with the GNU toolchain (`x86_64-pc-windows-gnu`), not MSVC. Tauri builds on it without Visual Studio Build Tools, given three things on PATH that Rust's own MinGW kit lacks:

- `dlltool` and `as` (GNU binutils): the `windows` crates make import libraries with `dlltool`, which calls `as`.
- `windres` (GNU binutils): `tauri-build` compiles the app's Windows resources (icon, version info, manifest) with it.
- `gcc` for `windres` to preprocess with (`gcc -E`). There is no C compiler here, and Tauri's resource script needs no preprocessing, so `cpp_passthrough.rs` stands in: built as `gcc.exe`, it prints the input file unchanged and refuses anything but `-E`.

`fetch-toolchain.ps1` sets this up: it downloads binutils 2.47 and the five libraries it links against from the MSYS2 mirror (`https://mirror.msys2.org/mingw/mingw64/`, about 8.6 MB, each file pinned by SHA-256), unpacks them with Windows' own `tar`, and builds the stand-in with `rustc`. Default destination `%LOCALAPPDATA%\Caveman\toolchain` (installed there on 2026-10-20); then put `<destination>\mingw64\bin` on PATH for app builds.

```
powershell -ExecutionPolicy Bypass -File app\toolchain\fetch-toolchain.ps1 [-Dest <folder>]
```

It uses Windows' `curl` because the mirror answers PowerShell's own web client (whose user agent says Mozilla) with a browser check page.
