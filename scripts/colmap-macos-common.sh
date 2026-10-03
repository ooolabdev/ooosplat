#!/usr/bin/env bash
# Shared by the COLMAP-only and mixed macOS runtime builders.
# Callers supply workspace, stage, build, jobs, deployment_target,
# colmap_archive, dependency_origins, and engine_field().
# Both entrypoints use set -euo pipefail before sourcing this file.
build_macos_colmap() {
mkdir -p "$build/colmap-source" "$build/colmap"
tar -xzf "$colmap_archive" -C "$build/colmap-source" --strip-components=1
libomp_prefix="$(brew --prefix libomp)"
[[ -f "$libomp_prefix/lib/libomp.dylib" ]] || {
  echo "Homebrew libomp runtime was not found at $libomp_prefix/lib/libomp.dylib" >&2
  exit 1
}
cmake -S "$build/colmap-source" -B "$build/colmap" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DGIT_COMMIT_ID="$(engine_field COLMAP commit)" \
  -DGIT_COMMIT_DATE=Unknown \
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
  -DCASPAR_ENABLED=OFF \
  -DCASPAR_USE_DOUBLE=OFF \
  -DMVS_ENABLED=OFF \
  -DONNX_ENABLED=OFF \
  -DOPENGL_ENABLED=OFF \
  -DCGAL_ENABLED=OFF \
  -DLSD_ENABLED=OFF \
  -DDOWNLOAD_ENABLED=OFF \
  -DTESTS_ENABLED=OFF
cmake --build "$build/colmap" --parallel "$jobs"
cmake --install "$build/colmap"
colmap_pre_trim_bytes="$(node -e 'const fs=require("fs"),p=require("path");const size=d=>fs.readdirSync(d,{withFileTypes:true}).reduce((s,e)=>{const f=p.join(d,e.name);return s+(e.isDirectory()?size(f):fs.statSync(f).size)},0);console.log(size(process.argv[1]));' "$stage")"
rm -rf -- "$stage/include" "$stage/share" "$stage/lib/cmake" "$stage/lib/pkgconfig"
find "$stage/lib" -type f -name '*.a' -delete
colmap_post_trim_bytes="$(node -e 'const fs=require("fs"),p=require("path");const size=d=>fs.readdirSync(d,{withFileTypes:true}).reduce((s,e)=>{const f=p.join(d,e.name);return s+(e.isDirectory()?size(f):fs.statSync(f).size)},0);console.log(size(process.argv[1]));' "$stage")"
colmap_removed_bytes="$((colmap_pre_trim_bytes - colmap_post_trim_bytes))"
}

collect_macos_colmap_notices() {
mkdir -p "$stage/licenses/colmap-thirdparty"
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
}

# A staged pathname does not establish provenance. Only the mixed builder's
# explicitly registered FFmpeg outputs may use its LGPL build notice. Homebrew
# FFmpeg (including transitive OpenImageIO dependencies) uses formula metadata.
classify_macos_component_origin() {
  local source_dependency="$1" origin_kind="${2:-}" remainder
  component=""
  component_source=""
  case "$source_dependency" in
    "$brew_root/Cellar/"*)
      remainder="${source_dependency#"$brew_root/Cellar/"}"
      component="${remainder%%/*}"
      component_source="homebrew"
      ;;
    "$brew_root/opt/"*)
      remainder="${source_dependency#"$brew_root/opt/"}"
      component="${remainder%%/*}"
      component_source="homebrew"
      ;;
    "$stage/lib/"*)
      [[ "$origin_kind" == built-ffmpeg ]] || {
        echo "Unregistered staged dependency origin: $source_dependency" >&2
        return 1
      }
      component="ffmpeg"
      component_source="built-ffmpeg"
      ;;
  esac
  [[ -n "$component" ]] || { echo "Cannot map $source_dependency to a licensed component." >&2; return 1; }
}

collect_macos_runtime_component_notices() {
components_tsv="$build/components.tsv"
: > "$components_tsv"
while IFS=$'\t' read -r library source_dependency origin_kind; do
  [[ -n "$library" ]] || continue
  classify_macos_component_origin "$source_dependency" "$origin_kind"
  if [[ "$component_source" == built-ffmpeg ]]; then
    [[ -f "$stage/licenses/FFmpeg-LGPL-2.1.txt" ]] || {
      echo "Missing notice for explicitly registered mixed-build FFmpeg." >&2
      exit 1
    }
    license="LGPL-2.1-or-later"
    homepage="https://ffmpeg.org/"
  else
    info="$(brew info --json=v2 "$component")"
    license="$(node -e 'const i=JSON.parse(process.argv[1]).formulae[0]; process.stdout.write(i.license||"")' "$info")"
    homepage="$(node -e 'const i=JSON.parse(process.argv[1]).formulae[0]; process.stdout.write(i.homepage||"")' "$info")"
    [[ -n "$license" ]] || { echo "Homebrew formula $component has no license metadata." >&2; exit 1; }
    if [[ ! -d "$stage/licenses/homebrew/$component" ]]; then
      info_file="$build/$component-formula.json"
      printf '%s\n' "$info" > "$info_file"
      node "$workspace/scripts/collect-macos-component-notices.mjs" \
        "$info_file" "$(brew --prefix "$component")" \
        "$stage/licenses/homebrew/$component" "$build/license-sources"
    fi
  fi
  printf '%s\t%s\t%s\t%s\tlib/%s\n' "$component" "$component_source" "$license" "$homepage" "$library" >> "$components_tsv"
done < "$dependency_origins"

node -e '
const fs=require("fs");
const lines=fs.readFileSync(process.argv[1],"utf8").trim().split(/\n/).filter(Boolean);
const map=new Map();
for(const line of lines){const [name,source,license,homepage,file]=line.split("\t"); const key=`${source}:${name}`; const item=map.get(key)||{name,source,license,homepage,files:[]}; item.files.push(file); map.set(key,item);}
const licenseRoot=process.argv[3];
const sourceLicenseFiles=[];
const walk=directory=>{for(const entry of fs.readdirSync(directory,{withFileTypes:true})){const full=`${directory}/${entry.name}`; if(entry.isDirectory()) walk(full); else sourceLicenseFiles.push(full.slice(licenseRoot.length+1));}};
walk(licenseRoot);
const output={schemaVersion:1,note:"Generated from the dylibs actually copied into the macOS runtime. Source and statically linked component notices are packaged under licenses/.",components:[...map.values()].map(v=>({...v,licenseFiles:v.source==="built-ffmpeg"?["FFmpeg-LGPL-2.1.txt"]:sourceLicenseFiles.filter(f=>f.startsWith(`homebrew/${v.name}/`)),files:[...new Set(v.files)].sort()})).sort((a,b)=>a.name.localeCompare(b.name)||a.source.localeCompare(b.source)),sourceLicenseFiles:sourceLicenseFiles.sort()};
fs.writeFileSync(process.argv[2],JSON.stringify(output,null,2)+"\n");
' "$components_tsv" "$stage/BUNDLED-COMPONENTS.json" "$stage/licenses"
}

bundle_macos_runtime() {
brew_root="$(brew --prefix 2>/dev/null || true)"
resolve_rpath_dependency() {
  local name="$1"
  [[ -n "$brew_root" ]] || return 1
  find -L "$brew_root/opt" -type f -name "$name" -print -quit 2>/dev/null
}

declare -a queue=("$stage/bin/"*)
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

collect_macos_runtime_component_notices

node "$workspace/scripts/materialize-runtime-links.mjs" "$stage"
while IFS= read -r macho; do
  codesign --force --sign - "$macho"
done < <(find "$stage/bin" "$stage/lib" -type f -print)
}
