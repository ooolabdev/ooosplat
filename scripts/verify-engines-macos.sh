#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "macOS engine verification requires an Apple Silicon Mac." >&2
  exit 1
fi

for command_name in file shasum otool vtool node; do
  command -v "$command_name" >/dev/null || { echo "Missing verification command: $command_name" >&2; exit 1; }
done

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runtime="$workspace/engines/macos/arm64"
manifest="$workspace/engines/manifest.macos.json"

for relative in bin/ffmpeg bin/ffprobe bin/colmap bin/brush_app SHA256SUMS BUILD-INFO.json BUNDLED-COMPONENTS.json; do
  [[ -f "$runtime/$relative" ]] || { echo "Missing macOS runtime file: $relative" >&2; exit 1; }
done

for binary in ffmpeg ffprobe colmap brush_app; do
  [[ -x "$runtime/bin/$binary" ]] || { echo "$binary is not executable." >&2; exit 1; }
done

(cd "$runtime" && shasum -a 256 -c SHA256SUMS)

version_le() {
  local left_major="${1%%.*}" left_minor="${1#*.}" right_major="${2%%.*}" right_minor="${2#*.}"
  left_minor="${left_minor%%.*}"; right_minor="${right_minor%%.*}"
  (( left_major < right_major || (left_major == right_major && left_minor <= right_minor) ))
}

mach_o_validation_failed=0
while IFS= read -r file_path; do
  if ! file "$file_path" | grep -q 'arm64'; then
    echo "Non-arm64 Mach-O: $file_path" >&2
    mach_o_validation_failed=1
    continue
  fi
  # The first line is the inspected file's own absolute path. Only dependency
  # lines are relevant when checking whether the runtime is relocatable.
  if otool -L "$file_path" | tail -n +2 | grep -E '/opt/homebrew|/usr/local|/Users/|/private/tmp|/var/folders' >/dev/null; then
    echo "Non-relocatable dependency in $file_path" >&2
    otool -L "$file_path" >&2
    mach_o_validation_failed=1
  fi
  minos="$(vtool -show-build "$file_path" 2>/dev/null | awk '/minos/ { print $2; exit }')"
  if [[ -z "$minos" ]]; then
    echo "Cannot read deployment target from $file_path" >&2
    mach_o_validation_failed=1
  elif ! version_le "$minos" "11.0"; then
    echo "$file_path requires macOS $minos (maximum allowed is 11.0)." >&2
    mach_o_validation_failed=1
  fi
done < <(find "$runtime/bin" "$runtime/lib" -type f -print)
(( mach_o_validation_failed == 0 )) || exit 1

for target in "$runtime/bin"/* "$runtime/lib"/*; do
  [[ -f "$target" ]] || continue
  while IFS= read -r dependency; do
    case "$dependency" in
      @rpath/*)
        dylib="${dependency#@rpath/}"
        [[ -f "$runtime/lib/$dylib" ]] || { echo "Missing bundled dependency $dylib for $target" >&2; exit 1; }
        ;;
      /System/Library/*|/usr/lib/*|@loader_path/*|@executable_path/*) ;;
      *) echo "Unsupported dependency path $dependency in $target" >&2; exit 1 ;;
    esac
  done < <(otool -L "$target" | tail -n +2 | awk '{ print $1 }')
done

restricted_path="/usr/bin:/bin:/usr/sbin:/sbin"
PATH="$restricted_path" "$runtime/bin/ffmpeg" -version | grep -F 'ffmpeg version 8.1.2'
PATH="$restricted_path" "$runtime/bin/ffprobe" -version | grep -F 'ffprobe version 8.1.2'

feature_help="$(PATH="$restricted_path" "$runtime/bin/colmap" feature_extractor -h 2>&1)"
matching_help="$(PATH="$restricted_path" "$runtime/bin/colmap" sequential_matcher -h 2>&1)"
PATH="$restricted_path" "$runtime/bin/colmap" mapper -h >/dev/null 2>&1
if grep -q -- '--FeatureExtraction.use_gpu' <<<"$feature_help"; then
  grep -q -- '--FeatureMatching.use_gpu' <<<"$matching_help"
elif grep -q -- '--SiftExtraction.use_gpu' <<<"$feature_help"; then
  grep -q -- '--SiftMatching.use_gpu' <<<"$matching_help"
else
  echo "Unsupported COLMAP CLI: no recognized CPU SIFT options." >&2
  exit 1
fi

if find "$runtime" -type f -print | grep -Ei 'cuda|cudnn|cudart|curand' >/dev/null; then
  echo "CUDA files are forbidden in the macOS COLMAP runtime." >&2
  exit 1
fi

brush_help="$(PATH="$restricted_path" "$runtime/bin/brush_app" --help 2>&1)"
for flag in --total-steps --max-resolution --export-every --export-path --export-name; do
  grep -q -- "$flag" <<<"$brush_help" || { echo "Brush is missing $flag" >&2; exit 1; }
done

node -e '
const fs=require("fs"), path=require("path");
const m=require(process.argv[1]), b=require(process.argv[2]), c=require(process.argv[3]);
if (m.architecture!=="arm64" || m.minimumSystemVersion!=="11.0" || b.minimumSystemVersion!=="11.0" || !Array.isArray(c.components)) process.exit(1);
const covered=new Set(c.components.flatMap(component=>component.files));
for (const file of fs.readdirSync(path.join(process.argv[4],"lib"))) if (!covered.has(`lib/${file}`)) throw new Error(`Missing license inventory for lib/${file}`);
for (const license of c.sourceLicenseFiles||[]) if (!fs.existsSync(path.join(process.argv[4],license))) throw new Error(`Missing source license ${license}`);
if ((c.sourceLicenseFiles||[]).length < 6) throw new Error("Incomplete COLMAP source license inventory");
' "$manifest" "$runtime/BUILD-INFO.json" "$runtime/BUNDLED-COMPONENTS.json" "$runtime"
echo "Verified bundled Apple Silicon FFmpeg/FFprobe, CPU COLMAP, and Brush without PATH fallback."
