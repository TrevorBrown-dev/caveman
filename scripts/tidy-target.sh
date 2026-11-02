#!/usr/bin/env bash
# Keeps Cargo's build cache (target/) from growing without end.
#
# Cargo never deletes old output: every compiler update, every dependency bump (Dependabot opens some each week) and
# every feature change leaves the previous copy of each crate behind, so target/ only grows (22 GB here on 2026-11-02,
# most of it copies from before the switch to the MSVC toolchain). Telling a live copy from a dead one would take
# last-access times, and on Windows merely listing a file under Git Bash counts as an access, so this goes by two
# signals that need none:
#
#   - the compiler changed (`rustc -vV` differs from the last run's): every compiled crate in the cache belongs to
#     the old compiler, so all of it is dead;
#   - the cache is bigger than the budget: old copies have piled up.
#
# Either one clears each profile's caches (deps/, build/, incremental/, .fingerprint/, examples/ under
# target/debug, target/release and target/<triple>/<profile>) and keeps the finished programs at each profile's top
# (an app started from target/release keeps working). The next build compiles everything again: a few minutes.
#
# usage: scripts/tidy-target.sh [--max-gb N] [--now] [--dry-run] [--quiet] [TARGET_DIR]
#   --max-gb N  the budget (default 12; fresh, the debug cache of clippy, the build and the tests is about 4.5 GB and
#               a release build adds about 2)
#   --now       clear regardless
#   --dry-run   say what it would do, delete nothing
#   --quiet     print only when it clears (or would)
#   TARGET_DIR  default: $CARGO_TARGET_DIR, else target/ in the repository
#
# It stops if a build is using this cache (other projects' builds do not matter). Files a running program holds stay
# until it closes and are counted.

set -euo pipefail

max_gb=12
now=0
dry=0
quiet=0
target=""
while [ $# -gt 0 ]; do
  case "$1" in
    --max-gb) max_gb="$2"; shift 2 ;;
    --max-gb=*) max_gb="${1#*=}"; shift ;;
    --now) now=1; shift ;;
    -n | --dry-run) dry=1; shift ;;
    -q | --quiet) quiet=1; shift ;;
    -h | --help) sed -n '2,/^$/s/^# \{0,1\}//p' "$0"; exit 0 ;;
    -*) echo "tidy-target: unknown option $1 (see --help)" >&2; exit 2 ;;
    *) target="$1"; shift ;;
  esac
done

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${target:-${CARGO_TARGET_DIR:-$repo/target}}"
[ -d "$target" ] || exit 0
marker="$target/.tidy-toolchain"

say() { [ "$quiet" = 1 ] || echo "$@"; }

# A build in progress holds the lock files of its profile folder (target/<profile>/.cargo-lock and its siblings) for
# its whole length; other projects' builds do not matter.
busy() {
  local locks=() f
  while IFS= read -r f; do locks+=("$f"); done < <(find "$target" -maxdepth 3 -type f -name '.cargo*lock' 2> /dev/null)
  [ ${#locks[@]} -gt 0 ] || return 1
  if command -v powershell.exe > /dev/null 2>&1 && command -v cygpath > /dev/null 2>&1; then
    # Windows: Cargo holds an exclusive byte-range lock (LockFileEx), so taking one byte of it fails while it builds
    TIDY_LOCKS="$(cygpath -w "${locks[@]}")" powershell.exe -NoProfile -NonInteractive -Command '
      foreach ($p in ($env:TIDY_LOCKS -split "`n")) {
        $p = $p.Trim(); if (-not $p) { continue }
        try { $f = [IO.File]::Open($p, "Open", "ReadWrite", "ReadWrite") } catch { exit 0 }
        try { $f.Lock(0, 1); $f.Unlock(0, 1) } catch { $f.Close(); exit 0 }
        $f.Close()
      }
      exit 1'
  else
    for f in "${locks[@]}"; do
      flock -n "$f" true 2> /dev/null || return 0
    done
    return 1
  fi
}
if busy; then
  echo "tidy-target: a build is using $target; run it again when it finishes" >&2
  exit 1
fi

gb() { awk -v k="$1" 'BEGIN { printf "%.1f GB", k / 1048576 }'; }

# the compiler the repository builds with (rust-toolchain.toml applies inside the repository)
toolchain="$(cd "$repo" && rustc -vV 2> /dev/null | tr -d '\r' || true)"
kb="$(du -sk "$target" 2> /dev/null | awk '{ print $1 }')"

reason=""
if [ "$now" = 1 ]; then
  reason="asked to"
elif [ -n "$toolchain" ] && [ -f "$marker" ] && [ "$(tr -d '\r' < "$marker")" != "$toolchain" ]; then
  reason="the compiler changed ($(sed -n 's/^release: //p' "$marker") to $(sed -n 's/^release: //p' <<< "$toolchain"))"
elif [ "$kb" -gt $((max_gb * 1048576)) ]; then
  reason="it holds $(gb "$kb"), over the $max_gb GB budget"
fi

if [ -z "$reason" ]; then
  [ "$dry" = 1 ] || { [ -z "$toolchain" ] || printf '%s\n' "$toolchain" > "$marker"; }
  say "tidy-target: $(gb "$kb") in $target, within the $max_gb GB budget; nothing to clear"
  exit 0
fi

caches=()
while IFS= read -r fingerprints; do
  dir="$(dirname "$fingerprints")"
  for sub in deps build incremental .fingerprint examples; do
    [ -e "$dir/$sub" ] && caches+=("$dir/$sub")
  done
done < <(find "$target" -maxdepth 3 -type d -name .fingerprint)

if [ "$dry" = 1 ]; then
  echo "tidy-target: would clear ${#caches[@]} cache folders in $target because $reason"
  exit 0
fi

held=0
for c in "${caches[@]}"; do
  rm -rf -- "$c" 2> /dev/null || held=$((held + 1))
done
[ -z "$toolchain" ] || printf '%s\n' "$toolchain" > "$marker"

after="$(du -sk "$target" 2> /dev/null | awk '{ print $1 }')"
echo "tidy-target: cleared the build caches because $reason: $(gb "$kb") to $(gb "$after"); the next build compiles everything again"
[ "$held" = 0 ] || echo "tidy-target: $held cache folder(s) kept files a running program holds; run it again once that program closes"
exit 0
