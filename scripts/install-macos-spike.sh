#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_plugin="$repo_root/target/x86_64-apple-darwin/release/libets2_adaptive_eta_telemetry_spike.dylib"
steam_common="$HOME/Library/Application Support/Steam/steamapps/common"
game_macos="${ETS2_MACOS_DIR:-$steam_common/Euro Truck Simulator 2/Euro Truck Simulator 2.app/Contents/MacOS}"
plugin_dir="$game_macos/plugins"
destination="$plugin_dir/libets2_adaptive_eta_telemetry_spike.dylib"

if [[ ! -x "$game_macos/eurotrucks2" ]]; then
  printf 'ETS2 executable not found: %s\n' "$game_macos/eurotrucks2" >&2
  printf 'Set ETS2_MACOS_DIR to the game app Contents/MacOS directory.\n' >&2
  exit 1
fi

"$repo_root/scripts/verify-macos-spike.sh" "$source_plugin"

if [[ -e "$destination" ]]; then
  printf 'Refusing to overwrite existing file: %s\n' "$destination" >&2
  printf 'Remove that exact prior plugin artifact manually before reinstalling.\n' >&2
  exit 1
fi

mkdir -p "$plugin_dir"
cp "$source_plugin" "$destination"
chmod 755 "$destination"
xattr -d com.apple.quarantine "$destination" 2>/dev/null || true
codesign --force --sign - "$destination"
"$repo_root/scripts/verify-macos-spike.sh" "$destination"

printf 'Installed production telemetry plugin: %s\n' "$destination"
