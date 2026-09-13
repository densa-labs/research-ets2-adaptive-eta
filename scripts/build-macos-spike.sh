#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="x86_64-apple-darwin"
plugin="$repo_root/target/$target/release/libets2_adaptive_eta_telemetry_spike.dylib"

cargo build --manifest-path "$repo_root/Cargo.toml" --target "$target" --locked --release
codesign --force --sign - "$plugin"
"$repo_root/scripts/verify-macos-spike.sh" "$plugin"

printf 'Built spike plugin: %s\n' "$plugin"

