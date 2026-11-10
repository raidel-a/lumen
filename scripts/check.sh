#!/usr/bin/env bash
# Local pre-release check. Runs tests, builds and signs the bundle, and verifies the signature.
# Pass --smoke to also run the end-to-end dev smoke test.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "check: tests"
cargo test --bin lumen --manifest-path "$root/Cargo.toml"

echo "check: bundle"
"$root/scripts/bundle.sh" >/dev/null

echo "check: signature"
codesign --verify --strict --verbose=2 "$root/dist/Lumen.app"

if [[ "${1:-}" == "--smoke" ]]; then
  echo "check: smoke"
  "$root/scripts/dev/smoke.sh"
fi

echo "check: ok"
