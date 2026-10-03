#!/usr/bin/env bash
set -euo pipefail

[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || {
  echo 'COLMAP macOS builds require an Apple Silicon Mac.' >&2; exit 1;
}
for command_name in brew curl shasum tar cmake ninja clang file otool install_name_tool vtool codesign node; do
  command -v "$command_name" >/dev/null || { echo "Missing build command: $command_name" >&2; exit 1; }
done

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock="$workspace/engines/colmap-build.json"
read_lock() { node -p 'require(process.argv[1])[process.argv[2]]' "$lock" "$1"; }
# The shared compiler also serves the mixed builder; this entrypoint reads only
# COLMAP's source lock and never reads/downloads other engine source archives.
engine_field() { [[ "$1" == COLMAP ]]; read_lock "$2"; }
node "$workspace/scripts/verify-colmap-lock.mjs"

cache="$workspace/.cache/colmap-macos-build"
mkdir -p "$cache" "$workspace/dist-engines"
task_directory="$(mktemp -d "$cache/build-XXXXXX")"
build="$task_directory/build"
stage="$task_directory/ooosplat-colmap-macos-arm64"
mkdir -p "$build" "$stage/bin" "$stage/lib"
jobs="${OOOSPLAT_BUILD_JOBS:-$(sysctl -n hw.logicalcpu)}"
deployment_target="15.0"
dependency_origins="$build/dependency-origins.tsv"
: > "$dependency_origins"

commit="$(read_lock commit)"
colmap_archive="$cache/colmap-$commit.tar.gz"
if [[ ! -f "$colmap_archive" ]] || [[ "$(shasum -a 256 "$colmap_archive" | awk '{print toupper($1)}')" != "$(read_lock sourceSha256)" ]]; then
  curl --fail --location --retry 3 "$(read_lock sourceUrl)" --output "$colmap_archive"
fi
[[ "$(shasum -a 256 "$colmap_archive" | awk '{print toupper($1)}')" == "$(read_lock sourceSha256)" ]] || {
  echo 'COLMAP source SHA-256 mismatch.' >&2; exit 1;
}

source "$workspace/scripts/colmap-macos-common.sh"
build_macos_colmap
collect_macos_colmap_notices
bundle_macos_runtime
node "$workspace/scripts/package-colmap-macos-runtime.mjs" "$stage" "$build" "$colmap_removed_bytes"
echo "Build files retained for dependency/license review at $task_directory"
