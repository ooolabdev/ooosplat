#!/usr/bin/env bash
set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$workspace/src-tauri/target/release/ooo-splat"
bundled_brush="$workspace/src-tauri/target/release/engines/linux/brush/brush_app"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This launcher only supports Linux." >&2
  exit 1
fi

for command_name in node npm; do
  command -v "$command_name" >/dev/null || {
    echo "Missing $command_name. Install the Ubuntu dependencies listed in README.md." >&2
    exit 1
  }
done

cd "$workspace"
npm run verify:engines
npm run verify:licenses

needs_build=false
if [[ ! -x "$binary" || ! -x "$bundled_brush" ]]; then
  needs_build=true
else
  build_inputs=(
    "$workspace/src"
    "$workspace/assets"
    "$workspace/src-tauri/src"
    "$workspace/src-tauri/icons"
    "$workspace/src-tauri/Cargo.toml"
    "$workspace/src-tauri/Cargo.lock"
    "$workspace/src-tauri/tauri.conf.json"
    "$workspace/src-tauri/tauri.linux.conf.json"
    "$workspace/package.json"
    "$workspace/package-lock.json"
    "$workspace/index.html"
    "$workspace/vite.config.ts"
  )
  for input in "${build_inputs[@]}"; do
    if [[ -e "$input" ]] && [[ -n "$(find "$input" -newer "$binary" -print -quit)" ]]; then
      needs_build=true
      break
    fi
  done
fi

if [[ "$needs_build" == true ]]; then
  if [[ ! -d "$workspace/node_modules" ]]; then
    echo "Frontend dependencies are missing. Run 'npm ci' first." >&2
    exit 1
  fi
  if ! command -v cargo >/dev/null && [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
    export PATH="${HOME}/.cargo/bin:$PATH"
  fi
  command -v cargo >/dev/null || {
    echo "Missing Cargo. Install Rust stable as described in README.md." >&2
    exit 1
  }

  echo "Building the current OOOSplat release..."
  npm run tauri -- build --no-bundle
fi

echo "Starting OOOSplat..."
exec "$binary"
