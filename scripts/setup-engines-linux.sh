#!/usr/bin/env bash
set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
archive="$workspace/.cache/engines/brush-app-x86_64-unknown-linux-gnu.tar.xz"
destination="$workspace/engines/linux/brush"
source_url="https://github.com/ArthurBrussee/brush/releases/download/v0.3.0/brush-app-x86_64-unknown-linux-gnu.tar.xz"
archive_sha="4f0f9a8785d1951c62df26aae247c02c5bba32b00f40b06df4e1c9b867399e20"
binary_sha="13d28ee06a388bc4e987774e890b594d60a75bba26064e82b4ee338a78f158a4"

mkdir -p "$(dirname "$archive")" "$destination"
if [[ ! -f "$archive" ]] || [[ "$(sha256sum "$archive" | cut -d' ' -f1)" != "$archive_sha" ]]; then
  curl --fail --location --retry 3 "$source_url" --output "$archive"
fi
echo "$archive_sha  $archive" | sha256sum --check --status

temporary="$(mktemp -d)"
trap 'rm -rf -- "$temporary"' EXIT
tar -xJf "$archive" -C "$temporary"
brush_binary="$(find "$temporary" -type f -name brush_app -print -quit)"
if [[ -z "$brush_binary" ]]; then
  echo "Brush archive does not contain brush_app" >&2
  exit 1
fi
install -m 0755 "$brush_binary" "$destination/brush_app"
echo "$binary_sha  $destination/brush_app" | sha256sum --check --status

"$workspace/scripts/verify-engines-linux.sh"
