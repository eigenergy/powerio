#!/usr/bin/env bash
# Release identity agreement: the versions a consumer can observe must state
# one story before anything publishes.
#
# - PIO_ABI_VERSION in the Rust source and the checked-in C header agree.
# - The stored module schema constants (name, version) agree with the served
#   schema directory and its $id.
# - Every publishable crate carries the one workspace version.
set -euo pipefail
cd "$(dirname "$0")/.."

rust_abi=$(grep -oE 'pub const PIO_ABI_VERSION: u32 = [0-9]+' powerio-capi/src/lib.rs | grep -oE '[0-9]+$')
header_abi=$(grep -oE '#define PIO_ABI_VERSION [0-9]+' powerio-capi/include/powerio.h | grep -oE '[0-9]+$')
if [ "$rust_abi" != "$header_abi" ]; then
    echo "ABI version disagreement: Rust says $rust_abi, powerio.h says $header_abi" >&2
    exit 1
fi

schema_version=$(grep -oE 'pub const SCHEMA_VERSION: u32 = [0-9]+' powerio/src/stored/dto.rs | grep -oE '[0-9]+$')
schema_name=$(grep -oE 'pub const SCHEMA_NAME: &str = "[^"]+"' powerio/src/stored/dto.rs | grep -oE '"[^"]+"$' | tr -d '"')
if [ "$schema_name" != "powerio.module" ]; then
    echo "unexpected stored schema name: $schema_name" >&2
    exit 1
fi
if [ ! -f "docs/schema/pio-module/$schema_version/schema.json" ]; then
    echo "docs/schema/pio-module/$schema_version/schema.json is not checked in" >&2
    exit 1
fi
grep -q "\"\$id\": \"https://powerio.dev/schema/pio-module/$schema_version/schema.json\"" \
    "docs/schema/pio-module/$schema_version/schema.json" \
    || { echo "the served module schema \$id disagrees with SCHEMA_VERSION $schema_version" >&2; exit 1; }

workspace_version=$(grep -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
for manifest in powerio/Cargo.toml powerio-core/Cargo.toml powerio-tx/Cargo.toml \
                powerio-dist/Cargo.toml powerio-matrix/Cargo.toml powerio-prob/Cargo.toml \
                powerio-cli/Cargo.toml; do
    grep -q '^version.workspace = true' "$manifest" \
        || { echo "$manifest does not take the workspace version" >&2; exit 1; }
done

echo "release identity OK: ABI $rust_abi, module schema $schema_name/$schema_version, workspace $workspace_version"
