#!/usr/bin/env bash
set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
managed_brush="$workspace/engines/linux/brush/brush_app"

for engine in ffmpeg ffprobe; do
  command -v "$engine" >/dev/null || {
    echo "Missing $engine. Install the Ubuntu packages listed in README.md." >&2
    exit 1
  }
done

brush="$managed_brush"
if [[ -z "$brush" || ! -x "$brush" ]]; then
  echo "Missing Brush. Run 'npm run setup:engines' first." >&2
  exit 1
fi
node "$workspace/scripts/brush-runtime.mjs" verify linux

colmap="$workspace/engines/linux/colmap/bin/colmap"
pin="$(node -p 'require(process.argv[1]).colmap.integritySha256' "$workspace/engines/manifest.linux.json")"
node "$workspace/scripts/colmap-runtime.mjs" "$workspace/engines/linux/colmap" linux "$pin" release
feature_help="$("$colmap" feature_extractor -h 2>&1)"
matching_help="$("$colmap" sequential_matcher -h 2>&1)"
"$colmap" mapper -h >/dev/null 2>&1
if grep -q -- '--FeatureExtraction.use_gpu' <<<"$feature_help"; then
  grep -q -- '--FeatureMatching.use_gpu' <<<"$matching_help"
elif grep -q -- '--SiftExtraction.use_gpu' <<<"$feature_help"; then
  grep -q -- '--SiftMatching.use_gpu' <<<"$matching_help"
else
  echo "Unsupported COLMAP CLI: no recognized CPU SIFT options." >&2
  exit 1
fi

brush_help="$("$brush" --help 2>&1)"
for flag in --total-train-iters --max-resolution --export-every --export-path --export-name; do
  grep -q -- "$flag" <<<"$brush_help" || { echo "Brush is missing $flag" >&2; exit 1; }
done

echo "Verified system FFmpeg/FFprobe, pinned bundled COLMAP 4.2.1 (Caspar + Ceres), and OOOBrush ooo-v1.0.0 CLI."
