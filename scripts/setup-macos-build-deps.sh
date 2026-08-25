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

# Keep the Homebrew executable and Formula DSL in sync before checking out the
# pinned homebrew/core revision. Runner images can ship an older Homebrew with
# newer or locally modified formula files, which makes valid formula DSL fail.
unset HOMEBREW_NO_AUTO_UPDATE HOMEBREW_NO_INSTALL_FROM_API
brew tap --force homebrew/core
core_repository="$(brew --repo homebrew/core)"
brew update-reset "$(brew --repository)" "$core_repository"
brew update --force

export HOMEBREW_NO_AUTO_UPDATE=1
export HOMEBREW_NO_INSTALL_FROM_API=1
export HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1
git -C "$core_repository" fetch origin "$core_commit" --depth=1
# GitHub's macOS runner image can contain tracked Homebrew formula changes
# (for example Formula/r/rustup.rb). The runner is disposable, so discard
# those image-local changes before pinning homebrew/core for reproducibility.
git -C "$core_repository" checkout --force --detach "$core_commit"
brew install "${build_tools[@]}"
ccache --set-config=max_size=2G

# `brew install --build-from-source` only applies to formulae explicitly named
# on the command line; dependencies still use host-version bottles. Build the
# complete required/recommended runtime dependency graph from source so every
# dylib can retain the declared macOS 11 deployment target.
source_formulae=()
append_unique_formula() {
  local candidate="$1" existing
  for existing in "${source_formulae[@]}"; do
    [[ "$existing" == "$candidate" ]] && return
  done
  source_formulae+=("$candidate")
}
while IFS= read -r formula; do
  [[ -n "$formula" ]] && append_unique_formula "$formula"
done < <(brew deps --union --topological --full-name "${runtime_formulae[@]}")
for formula in "${runtime_formulae[@]}"; do
  append_unique_formula "$formula"
done

for formula in "${source_formulae[@]}"; do
  echo "Building macOS 11 dependency from source: $formula"
  if brew list --versions "$formula" >/dev/null 2>&1; then
    MACOSX_DEPLOYMENT_TARGET=11.0 brew reinstall --build-from-source "$formula"
  else
    MACOSX_DEPLOYMENT_TARGET=11.0 brew install --build-from-source "$formula"
  fi
done
