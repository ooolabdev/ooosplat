#!/usr/bin/env bash
set -euo pipefail
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || { echo 'Linux x86_64 required' >&2; exit 1; }
workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock="$workspace/engines/colmap-build.json"
read_lock() { node -p 'require(process.argv[1])[process.argv[2]]' "$lock" "$1"; }
for command_name in curl git tar cmake ninja nvcc node ldd patchelf; do command -v "$command_name" >/dev/null; done
[[ "$(cmake --version | head -n1)" == "cmake version $(read_lock cmakeVersion)" ]]
[[ "$(ninja --version)" == "$(read_lock ninjaVersion)" ]]
cuda_root="${CUDA_PATH:-/usr/local/cuda-13.2}"
[[ "$(node -p 'require(process.argv[1]).cuda.version' "$cuda_root/version.json")" == "$(read_lock cudaVersion)" ]]
cache="$workspace/.cache/colmap-linux-build"
mkdir -p "$cache"
task_directory="$(mktemp -d "$cache/build-XXXXXX")"
commit="$(read_lock commit)"
archive="$cache/colmap-$commit.tar.gz"
if [[ ! -f "$archive" ]] || [[ "$(sha256sum "$archive" | awk '{print toupper($1)}')" != "$(read_lock sourceSha256)" ]]; then
  curl --fail --location --retry 3 "$(read_lock sourceUrl)" --output "$archive"
fi
echo "$(read_lock sourceSha256)  $archive" | sha256sum --check --status
tar -xzf "$archive" -C "$task_directory"
source="$task_directory/colmap-$commit"
vcpkg="$task_directory/vcpkg"
git clone --no-checkout https://github.com/microsoft/vcpkg.git "$vcpkg"
git -C "$vcpkg" checkout --detach "$(read_lock vcpkgCommit)"
bash "$vcpkg/bootstrap-vcpkg.sh" -disableMetrics
build="$task_directory/build"
stage="$task_directory/ooosplat-colmap-linux-x64"
options=()
while IFS= read -r feature; do options+=("-D${feature}_ENABLED=OFF"); done < <(node -e 'for(const f of require(process.argv[1]).disabledFeatures) console.log(f)' "$lock")
architectures="$(node -p 'require(process.argv[1]).cudaArchitectures.join(";")' "$lock")"
cmake -S "$source" -B "$build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$stage" \
  -DGIT_COMMIT_ID="$commit" -DGIT_COMMIT_DATE=Unknown \
  -DCMAKE_TOOLCHAIN_FILE="$vcpkg/scripts/buildsystems/vcpkg.cmake" \
  -DVCPKG_TARGET_TRIPLET=x64-linux-dynamic -DBUILD_SHARED_LIBS=OFF \
  -DCMAKE_INSTALL_RPATH='$ORIGIN/../lib' -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON \
  -DCUDA_ENABLED=ON -DCASPAR_ENABLED=ON -DCASPAR_USE_DOUBLE=OFF \
  -DCMAKE_CUDA_ARCHITECTURES="$architectures" -DCUDAToolkit_ROOT="$cuda_root" "${options[@]}"
cmake --build "$build" --parallel "${OOOSPLAT_BUILD_JOBS:-4}"
cmake --install "$build"
installed="$build/vcpkg_installed/x64-linux-dynamic"
mkdir -p "$stage/lib"
# Preserve all release shared libraries and runtime-loaded plugins from the
# pinned core dependency tree, then collect their transitive ELF dependencies.
while IFS= read -r library; do
  relative="${library#"$installed/lib/"}"
  mkdir -p "$(dirname "$stage/lib/$relative")"
  cp -L "$library" "$stage/lib/$relative"
done < <(find -L "$installed/lib" -type f -name '*.so*' -print)
node "$workspace/scripts/collect-linux-runtime.mjs" "$stage" "$installed" "$cuda_root"
while IFS= read -r library; do
  # NVIDIA redistributables must stay byte-for-byte unmodified.
  case "$(basename "$library")" in libcudart.so*|libcurand.so*) continue ;; esac
  relative_lib="$(realpath --relative-to="$(dirname "$library")" "$stage/lib")"
  patchelf --set-rpath "\$ORIGIN:\$ORIGIN/$relative_lib" "$library"
done < <(find "$stage/lib" -type f -name '*.so*')
patchelf --set-rpath '$ORIGIN/../lib' "$stage/bin/colmap"
curl --fail --location --retry 3 https://docs.nvidia.com/cuda/archive/13.2.0/eula/index.html --output "$stage/CUDA-EULA.html"
node "$workspace/scripts/package-colmap-runtime.mjs" linux "$stage" "$source" "$build" "$installed"
echo "Build retained for review: $task_directory"
