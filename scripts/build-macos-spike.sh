#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="x86_64-apple-darwin"
plugin="$repo_root/target/$target/release/libets2_adaptive_eta_telemetry_spike.dylib"
build_kind="production"

if [[ "${1:-}" == "--developer-parity" ]]; then
  build_kind="developer-parity"
elif [[ $# -ne 0 ]]; then
  printf 'usage: %s [--developer-parity]\n' "$0" >&2
  exit 2
fi

if [[ "$build_kind" == "developer-parity" ]]; then
  cargo build --manifest-path "$repo_root/Cargo.toml" --package ets2-adaptive-eta-telemetry-spike --target "$target" --locked --release --features developer-parity
else
  cargo build --manifest-path "$repo_root/Cargo.toml" --package ets2-adaptive-eta-telemetry-spike --target "$target" --locked --release
fi
codesign --force --sign - "$plugin"
"$repo_root/scripts/verify-macos-spike.sh" "$plugin"

printf 'Built %s telemetry plugin: %s\n' "$build_kind" "$plugin"
