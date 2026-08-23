#!/usr/bin/env bash
set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
managed_brush="$workspace/engines/linux/brush/brush_app"
binary_sha="13d28ee06a388bc4e987774e890b594d60a75bba26064e82b4ee338a78f158a4"

for engine in ffmpeg ffprobe colmap; do
  command -v "$engine" >/dev/null || {
    echo "Missing $engine. Install the Ubuntu packages listed in README.md." >&2
    exit 1
  }
done

brush="$managed_brush"
if [[ ! -x "$brush" ]]; then
  brush="$(command -v brush_app || true)"
fi
if [[ -z "$brush" || ! -x "$brush" ]]; then
  echo "Missing Brush. Run 'npm run setup:engines' first." >&2
  exit 1
fi
if [[ "$brush" == "$managed_brush" ]]; then
  echo "$binary_sha  $brush" | sha256sum --check --status
fi

feature_help="$(colmap feature_extractor -h 2>&1)"
matching_help="$(colmap sequential_matcher -h 2>&1)"
colmap mapper -h >/dev/null 2>&1
if grep -q -- '--FeatureExtraction.use_gpu' <<<"$feature_help"; then
  grep -q -- '--FeatureMatching.use_gpu' <<<"$matching_help"
elif grep -q -- '--SiftExtraction.use_gpu' <<<"$feature_help"; then
  grep -q -- '--SiftMatching.use_gpu' <<<"$matching_help"
else
  echo "Unsupported COLMAP CLI: no recognized CPU SIFT options." >&2
  exit 1
fi

brush_help="$("$brush" --help 2>&1)"
for flag in --total-steps --max-resolution --export-every --export-path --export-name; do
  grep -q -- "$flag" <<<"$brush_help" || { echo "Brush is missing $flag" >&2; exit 1; }
done

echo "Verified system FFmpeg/FFprobe/COLMAP and Brush v0.3.0. Brush selects its graphics backend at runtime."
