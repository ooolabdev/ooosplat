#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "macOS build dependency setup requires an Apple Silicon Mac." >&2
  exit 1
fi

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$workspace/engines/manifest.macos.json"
core_commit="$(node -e 'process.stdout.write(require(process.argv[1]).buildEnvironment.homebrewCoreCommit.toLowerCase())' "$manifest")"
build_tools=()
while IFS= read -r formula; do
  build_tools+=("$formula")
done < <(node -e 'for (const item of require(process.argv[1]).buildEnvironment.buildTools) console.log(item)' "$manifest")
runtime_formulae=()
while IFS= read -r formula; do
  runtime_formulae+=("$formula")
done < <(node -e 'for (const item of require(process.argv[1]).buildEnvironment.runtimeFormulae) console.log(item)' "$manifest")

export HOMEBREW_NO_AUTO_UPDATE=1
export HOMEBREW_NO_INSTALL_FROM_API=1
brew tap --force homebrew/core
core_repository="$(brew --repo homebrew/core)"
git -C "$core_repository" fetch origin "$core_commit" --depth=1
git -C "$core_repository" checkout --detach "$core_commit"
brew install "${build_tools[@]}"
MACOSX_DEPLOYMENT_TARGET=11.0 brew install --build-from-source "${runtime_formulae[@]}"
