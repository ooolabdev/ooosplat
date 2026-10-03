#!/usr/bin/env bash
set -euo pipefail

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
node "$workspace/scripts/setup-colmap-runtime.mjs" linux
node "$workspace/scripts/brush-runtime.mjs" prepare linux

"$workspace/scripts/verify-engines-linux.sh"
