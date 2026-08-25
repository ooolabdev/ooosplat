#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "macOS engine builds require an Apple Silicon Mac." >&2
  exit 1
fi

for command_name in brew curl shasum tar cmake ninja make clang file otool install_name_tool vtool codesign node; do
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
deployment_target="11.0"

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
mkdir -p "$sources" "$build" "$stage/bin" "$stage/lib" "$output"
dependency_origins="$build/dependency-origins.tsv"
: > "$dependency_origins"

ffmpeg_archive="$sources/ffmpeg-8.1.2.tar.xz"
colmap_archive="$sources/colmap-4.0.4.tar.gz"
brush_archive="$sources/brush-app-aarch64-apple-darwin.tar.xz"
download_verified "$(engine_field 'FFmpeg / FFprobe' sourceUrl)" "$(engine_field 'FFmpeg / FFprobe' sourceSha256)" "$ffmpeg_archive"
download_verified "$(engine_field COLMAP sourceUrl)" "$(engine_field COLMAP sourceSha256)" "$colmap_archive"
download_verified "$(engine_field Brush sourceUrl)" "$(engine_field Brush sourceSha256)" "$brush_archive"

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
    --enable-ffmpeg \
    --enable-ffprobe \
    --extra-ldflags=-Wl,-headerpad_max_install_names \
    --install-name-dir=@rpath
  make -j"$jobs"
  make install
)
rm -rf -- "$stage/include" "$stage/share"
for ffmpeg_library in "$stage/lib"/*; do
  [[ -f "$ffmpeg_library" ]] || continue
  printf '%s\t%s\n' "$(basename "$ffmpeg_library")" "$ffmpeg_library" >> "$dependency_origins"
done

mkdir -p "$build/colmap-source" "$build/colmap"
tar -xzf "$colmap_archive" -C "$build/colmap-source" --strip-components=1
libomp_prefix="$(brew --prefix libomp)"
[[ -f "$libomp_prefix/lib/libomp.dylib" ]] || {
  echo "Homebrew libomp runtime was not found at $libomp_prefix/lib/libomp.dylib" >&2
  exit 1
}
cmake -S "$build/colmap-source" -B "$build/colmap" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$stage" \
  -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET="$deployment_target" \
  -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON \
  -DCMAKE_INSTALL_RPATH='@executable_path/../lib' \
  -DCMAKE_EXE_LINKER_FLAGS='-Wl,-headerpad_max_install_names' \
  -DCMAKE_SHARED_LINKER_FLAGS='-Wl,-headerpad_max_install_names' \
  -DOpenMP_C_FLAGS="-Xpreprocessor -fopenmp -I$libomp_prefix/include" \
  -DOpenMP_CXX_FLAGS="-Xpreprocessor -fopenmp -I$libomp_prefix/include" \
  -DOpenMP_C_LIB_NAMES=omp \
  -DOpenMP_CXX_LIB_NAMES=omp \
  -DOpenMP_omp_LIBRARY="$libomp_prefix/lib/libomp.dylib" \
  -DGUI_ENABLED=OFF \
  -DCUDA_ENABLED=OFF \
  -DONNX_ENABLED=OFF \
  -DOPENGL_ENABLED=OFF \
  -DCGAL_ENABLED=OFF \
  -DLSD_ENABLED=OFF \
  -DDOWNLOAD_ENABLED=OFF \
  -DTESTS_ENABLED=OFF
cmake --build "$build/colmap" --parallel "$jobs"
cmake --install "$build/colmap"
rm -rf -- "$stage/include" "$stage/share" "$stage/lib/cmake" "$stage/lib/pkgconfig"
find "$stage/lib" -type f -name '*.a' -delete

mkdir -p "$stage/licenses/colmap-thirdparty"
install -m 0644 "$workspace/licenses/FFmpeg-LGPL-2.1.txt" "$stage/licenses/FFmpeg-LGPL-2.1.txt"
install -m 0644 "$workspace/licenses/Brush-LICENSE.txt" "$stage/licenses/Brush-LICENSE.txt"
install -m 0644 "$build/colmap-source/COPYING.txt" "$stage/licenses/COLMAP-LICENSE.txt"
for component in PoissonRecon SiftGPU VLFeat; do
  install -m 0644 \
    "$build/colmap-source/src/thirdparty/$component/LICENSE" \
    "$stage/licenses/colmap-thirdparty/$component-LICENSE.txt"
done
for fetched in poselib faiss; do
  fetched_license="$(find "$build/colmap/_deps/${fetched}-src" -maxdepth 2 -type f \( -iname 'LICENSE*' -o -iname 'COPYING*' \) -print -quit)"
  [[ -n "$fetched_license" ]] || { echo "Missing $fetched license from COLMAP FetchContent." >&2; exit 1; }
  install -m 0644 "$fetched_license" "$stage/licenses/colmap-thirdparty/$fetched-LICENSE.txt"
done

brush_extract="$build/brush"
mkdir -p "$brush_extract"
tar -xJf "$brush_archive" -C "$brush_extract"
brush_binary="$(find "$brush_extract" -type f -name brush_app -print -quit)"
[[ -n "$brush_binary" ]] || { echo "Brush archive does not contain brush_app." >&2; exit 1; }
install -m 0755 "$brush_binary" "$stage/bin/brush_app"

# Keep only the CLI deliverables. COLMAP may install auxiliary files that the
# headless commands do not use; dynamic libraries are collected below.
find "$stage/bin" -maxdepth 1 -type f ! -name ffmpeg ! -name ffprobe ! -name colmap ! -name brush_app -delete

brew_root="$(brew --prefix 2>/dev/null || true)"
resolve_rpath_dependency() {
  local name="$1"
  [[ -n "$brew_root" ]] || return 1
  find -L "$brew_root/opt" -type f -name "$name" -print -quit 2>/dev/null
}

declare -a queue=("$stage/bin/ffmpeg" "$stage/bin/ffprobe" "$stage/bin/colmap" "$stage/bin/brush_app")
processed_list="$build/processed-mach-o.txt"
: > "$processed_list"
while ((${#queue[@]})); do
  target="${queue[0]}"
  queue=("${queue[@]:1}")
  [[ -f "$target" ]] || continue
  grep -Fqx "$target" "$processed_list" && continue
  printf '%s\n' "$target" >> "$processed_list"

  while IFS= read -r old_dependency; do
    [[ -n "$old_dependency" ]] || continue
    case "$old_dependency" in
      /System/Library/*|/usr/lib/*|@loader_path/*|@executable_path/*) continue ;;
      @rpath/*)
        base="${old_dependency#@rpath/}"
        source_dependency="$stage/lib/$base"
        if [[ ! -f "$source_dependency" ]]; then
          source_dependency="$(resolve_rpath_dependency "$base" || true)"
        fi
        ;;
      *)
        base="$(basename "$old_dependency")"
        source_dependency="$old_dependency"
        ;;
    esac
    [[ -f "$source_dependency" ]] || { echo "Cannot resolve $old_dependency required by $target" >&2; exit 1; }
    destination_dependency="$stage/lib/$base"
    existing_origin="$(awk -F '\t' -v name="$base" '$1 == name { print $2; exit }' "$dependency_origins")"
    if [[ ! -f "$destination_dependency" ]]; then
      cp -L "$source_dependency" "$destination_dependency"
      printf '%s\t%s\n' "$base" "$source_dependency" >> "$dependency_origins"
      chmod u+w "$destination_dependency"
      install_name_tool -id "@rpath/$base" "$destination_dependency" 2>/dev/null || true
      queue+=("$destination_dependency")
    elif [[ "$source_dependency" != "$destination_dependency" ]]; then
      if [[ -z "$existing_origin" ]]; then
        printf '%s\t%s\n' "$base" "$source_dependency" >> "$dependency_origins"
      elif [[ "$source_dependency" != "$existing_origin" ]] && ! cmp -s "$source_dependency" "$existing_origin"; then
        echo "Conflicting dylib basename $base while bundling $target" >&2
        exit 1
      fi
    fi
    install_name_tool -change "$old_dependency" "@rpath/$base" "$target"
  done < <(otool -L "$target" | tail -n +2 | awk '{print $1}')

  while IFS= read -r old_rpath; do
    case "$old_rpath" in
      /opt/homebrew*|/usr/local*|/Users/*|/private/tmp/*|/var/folders/*)
        install_name_tool -delete_rpath "$old_rpath" "$target"
        ;;
    esac
  done < <(otool -l "$target" | awk '$1 == "path" { print $2 }')

  if [[ "$target" == "$stage/bin/"* ]] && otool -L "$target" | grep -F '@rpath/' >/dev/null; then
    otool -l "$target" | grep -F '@executable_path/../lib' >/dev/null || install_name_tool -add_rpath '@executable_path/../lib' "$target"
  fi
done

components_tsv="$build/components.tsv"
: > "$components_tsv"
while IFS=$'\t' read -r library source_dependency; do
  [[ -n "$library" ]] || continue
  component=""
  case "$source_dependency" in
    "$stage/lib/"*) component="ffmpeg" ;;
    "$brew_root/Cellar/"*) remainder="${source_dependency#"$brew_root/Cellar/"}"; component="${remainder%%/*}" ;;
    "$brew_root/opt/"*) remainder="${source_dependency#"$brew_root/opt/"}"; component="${remainder%%/*}" ;;
  esac
  [[ -n "$component" ]] || { echo "Cannot map $source_dependency to a licensed component." >&2; exit 1; }
  if [[ "$component" == "ffmpeg" ]]; then
    license="LGPL-2.1-or-later"
    homepage="https://ffmpeg.org/"
  else
    info="$(brew info --json=v2 "$component")"
    license="$(node -e 'const i=JSON.parse(process.argv[1]).formulae[0]; process.stdout.write(i.license||"")' "$info")"
    homepage="$(node -e 'const i=JSON.parse(process.argv[1]).formulae[0]; process.stdout.write(i.homepage||"")' "$info")"
    [[ -n "$license" ]] || { echo "Homebrew formula $component has no license metadata." >&2; exit 1; }
  fi
  printf '%s\t%s\t%s\tlib/%s\n' "$component" "$license" "$homepage" "$library" >> "$components_tsv"
done < "$dependency_origins"

node -e '
const fs=require("fs");
const lines=fs.readFileSync(process.argv[1],"utf8").trim().split(/\n/).filter(Boolean);
const map=new Map();
for(const line of lines){const [name,license,homepage,file]=line.split("\t"); const item=map.get(name)||{name,license,homepage,files:[]}; item.files.push(file); map.set(name,item);}
const licenseRoot=process.argv[3];
const sourceLicenseFiles=[];
const walk=directory=>{for(const entry of fs.readdirSync(directory,{withFileTypes:true})){const full=`${directory}/${entry.name}`; if(entry.isDirectory()) walk(full); else sourceLicenseFiles.push(full.slice(licenseRoot.length+1));}};
walk(licenseRoot);
const output={schemaVersion:1,note:"Generated from the dylibs actually copied into the macOS runtime. Source and statically linked component notices are packaged under licenses/.",components:[...map.values()].map(v=>({...v,files:[...new Set(v.files)].sort()})).sort((a,b)=>a.name.localeCompare(b.name)),sourceLicenseFiles:sourceLicenseFiles.sort()};
fs.writeFileSync(process.argv[2],JSON.stringify(output,null,2)+"\n");
' "$components_tsv" "$stage/BUNDLED-COMPONENTS.json" "$stage/licenses"

while IFS= read -r macho; do
  codesign --force --sign - "$macho"
done < <(find "$stage/bin" "$stage/lib" -type f -print)

node -e '
const fs=require("fs");
const path=require("path");
const manifest=require(process.argv[1]);
const output={schemaVersion:1,platform:"macos",architecture:"arm64",minimumSystemVersion:manifest.minimumSystemVersion,generatedAt:new Date().toISOString(),sources:manifest.engines.map(({name,version,sourceUrl,sourceSha256,buildPolicy,license})=>({name,version,sourceUrl,sourceSha256,buildPolicy,license}))};
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

OOOSPLAT_MACOS_ENGINE_ARCHIVE="$archive" "$workspace/scripts/setup-engines-macos.sh"
echo "Created $archive"
