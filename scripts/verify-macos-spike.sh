#!/usr/bin/env bash

set -euo pipefail

plugin="${1:?usage: verify-macos-spike.sh PATH_TO_DYLIB}"

if [[ ! -f "$plugin" ]]; then
  printf 'Plugin does not exist: %s\n' "$plugin" >&2
  exit 1
fi

description="$(file "$plugin")"
if [[ "$description" != *"Mach-O"* || "$description" != *"x86_64"* ]]; then
  printf 'Expected an x86-64 Mach-O library, got: %s\n' "$description" >&2
  exit 1
fi

codesign --verify --strict --verbose=2 "$plugin"

exports="$(nm -gjU "$plugin" | LC_ALL=C sort)"
expected=$'_scs_telemetry_init\n_scs_telemetry_shutdown'
if [[ "$exports" != "$expected" ]]; then
  printf 'Unexpected exported symbols:\n%s\n' "$exports" >&2
  exit 1
fi

printf 'Verified x86-64 Mach-O signature and SCS exports: %s\n' "$plugin"

