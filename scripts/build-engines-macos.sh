#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "macOS engine builds require an Apple Silicon Mac." >&2
  exit 1
fi

for command_name in brew curl shasum tar make clang file otool install_name_tool vtool codesign node; do
  command -v "$command_name" >/dev/null || { echo "Missing build command: $command_name" >&2; exit 1; }
done

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$workspace/engines/manifest.macos.json"
cache="$workspace/.cache/macos-engine-build"
sources="$cache/sources"
build="$cache/build"
stage="$cache/stage/ooosplat-engines-macos-arm64"
output="$workspace/dist-engines"
jobs="${OOOSPLAT_BUILD_JOBS:-$(sysctl -n hw.logicalcpu)}"
deployment_target="15.0"

read_manifest() {
  node -e 'const m=require(process.argv[1]); let v=m; for (const key of process.argv[2].split(".")) v=v[key]; process.stdout.write(String(v));' "$manifest" "$1"
}

engine_field() {
  node -e 'const m=require(process.argv[1]); const e=m.engines.find(e=>e.name===process.argv[2]); process.stdout.write(String(e[process.argv[3]]));' "$manifest" "$1" "$2"
}

download_verified() {
  local url="$1" sha="$2" destination="$3"
  if [[ ! -f "$destination" ]] || [[ "$(shasum -a 256 "$destination" | awk '{print toupper($1)}')" != "$sha" ]]; then
    curl --fail --location --retry 3 "$url" --output "$destination"
  fi
  [[ "$(shasum -a 256 "$destination" | awk '{print toupper($1)}')" == "$sha" ]] || {
    echo "Source SHA-256 mismatch: $destination" >&2
    exit 1
  }
}

rm -rf -- "$build" "$cache/stage"
mkdir -p "$sources" "$build" "$stage/bin" "$stage/lib" "$stage/licenses" "$output"
dependency_origins="$build/dependency-origins.tsv"
: > "$dependency_origins"

ffmpeg_archive="$sources/ffmpeg-8.1.2.tar.xz"
download_verified "$(engine_field 'FFmpeg / FFprobe' sourceUrl)" "$(engine_field 'FFmpeg / FFprobe' sourceSha256)" "$ffmpeg_archive"

mkdir -p "$build/ffmpeg-source"
tar -xJf "$ffmpeg_archive" -C "$build/ffmpeg-source" --strip-components=1
(
  cd "$build/ffmpeg-source"
  MACOSX_DEPLOYMENT_TARGET="$deployment_target" ./configure \
    --prefix="$stage" \
    --arch=arm64 \
    --target-os=darwin \
    --cc=clang \
    --enable-shared \
    --disable-static \
    --disable-gpl \
    --disable-nonfree \
    --disable-ffplay \
    --disable-doc \
    --disable-debug \
    --disable-autodetect \
    --enable-ffmpeg \
    --enable-ffprobe \
    --extra-cflags="-mmacosx-version-min=$deployment_target" \
    --extra-cxxflags="-mmacosx-version-min=$deployment_target" \
    --extra-ldflags="-mmacosx-version-min=$deployment_target -Wl,-headerpad_max_install_names" \
    --install-name-dir=@rpath
  make -j"$jobs"
  make install
)
rm -rf -- "$stage/include" "$stage/share" "$stage/lib/pkgconfig"
for ffmpeg_library in "$stage/lib"/*; do
  [[ -f "$ffmpeg_library" ]] || continue
  printf '%s\t%s\tbuilt-ffmpeg\n' "$(basename "$ffmpeg_library")" "$ffmpeg_library" >> "$dependency_origins"
done

source "$workspace/scripts/colmap-macos-common.sh"
install -m 0644 "$workspace/licenses/FFmpeg-LGPL-2.1.txt" "$stage/licenses/FFmpeg-LGPL-2.1.txt"
install -m 0644 "$workspace/licenses/Brush-LICENSE.txt" "$stage/licenses/Brush-LICENSE.txt"
node "$workspace/scripts/brush-runtime.mjs" prepare macos --destination "$stage" --cache "$sources/ooobrush"

# The mixed runtime owns only FFmpeg/FFprobe and Brush. COLMAP is installed
# separately from the locked ooosplat-colmap Release.
find "$stage/bin" -maxdepth 1 -type f ! -name ffmpeg ! -name ffprobe ! -name brush_app -delete

bundle_macos_runtime

node -e '
const fs=require("fs");
const path=require("path");
const manifest=require(process.argv[1]);
const output={schemaVersion:2,platform:"macos",architecture:"arm64",minimumSystemVersion:manifest.minimumSystemVersion,generatedAt:new Date().toISOString(),sources:manifest.engines.filter(engine=>engine.name!=="COLMAP").map(({name,version,commit,sourceUrl,sourceSha256,buildPolicy,license})=>({name,version,commit,sourceUrl,sourceSha256,buildPolicy,license}))};
fs.writeFileSync(path.join(process.argv[2],"BUILD-INFO.json"),JSON.stringify(output,null,2)+"\n");
' "$manifest" "$stage"

(
  cd "$stage"
  find bin lib licenses -type f -print | LC_ALL=C sort | while IFS= read -r relative; do shasum -a 256 "$relative"; done > SHA256SUMS
  shasum -a 256 BUILD-INFO.json BUNDLED-COMPONENTS.json >> SHA256SUMS
)

archive_name="$(read_manifest distribution.archiveName)"
archive="$output/$archive_name"
rm -f -- "$archive" "$archive.sha256"
tar -cJf "$archive" -C "$cache/stage" ooosplat-engines-macos-arm64
(
  cd "$output"
  shasum -a 256 "$archive_name" > "$archive_name.sha256"
)

OOOSPLAT_ENGINE_BUILD_VERIFY=1 OOOSPLAT_MACOS_ENGINE_ARCHIVE="$archive" bash "$workspace/scripts/setup-engines-macos.sh"
node "$workspace/scripts/smoke-colmap-image-io.mjs" "$workspace/engines/macos/arm64/colmap/bin/colmap"
node "$workspace/scripts/engine-archive-info.mjs" macos "$archive" "$stage"
echo "Created $archive"
