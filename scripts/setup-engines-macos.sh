#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "macOS engine setup requires an Apple Silicon Mac." >&2
  exit 1
fi

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$workspace/engines/manifest.macos.json"
cache="$workspace/.cache/engines/macos"
destination="$workspace/engines/macos/arm64"

read_manifest() {
  node -e 'const m=require(process.argv[1]); let v=m; for (const key of process.argv[2].split(".")) v=v[key]; process.stdout.write(String(v));' "$manifest" "$1"
}

archive_name="$(read_manifest distribution.archiveName)"
archive="$cache/$archive_name"
checksum="$archive.sha256"

mkdir -p "$cache" "$(dirname "$destination")"
if [[ -n "${OOOSPLAT_MACOS_ENGINE_ARCHIVE:-}" ]]; then
  source_archive="$OOOSPLAT_MACOS_ENGINE_ARCHIVE"
  [[ -f "$source_archive" ]] || { echo "Missing local engine archive: $source_archive" >&2; exit 1; }
  cp "$source_archive" "$archive"
  if [[ -f "$source_archive.sha256" ]]; then
    cp "$source_archive.sha256" "$checksum"
  else
    (cd "$cache" && shasum -a 256 "$(basename "$archive")" > "$(basename "$checksum")")
  fi
else
  source_url="$(read_manifest distribution.sourceUrl)"
  checksum_url="$(read_manifest distribution.archiveSha256File)"
  curl --fail --location --retry 3 "$source_url" --output "$archive"
  curl --fail --location --retry 3 "$checksum_url" --output "$checksum"
fi

expected="$(awk 'NF { print tolower($1); exit }' "$checksum")"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || { echo "Invalid release checksum file." >&2; exit 1; }
actual="$(shasum -a 256 "$archive" | awk '{ print tolower($1) }')"
[[ "$actual" == "$expected" ]] || { echo "macOS engine archive SHA-256 mismatch." >&2; exit 1; }

if tar -tJf "$archive" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
  echo "Unsafe path found in macOS engine archive." >&2
  exit 1
fi

temporary="$(mktemp -d)"
trap 'rm -rf -- "$temporary"' EXIT
tar -xJf "$archive" -C "$temporary"
runtime_root="$temporary/ooosplat-engines-macos-arm64"
[[ -d "$runtime_root" ]] || { echo "Unexpected macOS engine archive layout." >&2; exit 1; }

staged="$temporary/runtime"
mv "$runtime_root" "$staged"
rm -rf -- "$destination"
mv "$staged" "$destination"

"$workspace/scripts/verify-engines-macos.sh"
