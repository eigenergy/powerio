#!/usr/bin/env bash
# Release identity agreement: the versions a consumer can observe must state
# one story before anything publishes.
#
# - PIO_ABI_VERSION in the Rust source and the checked-in C header agree.
# - The PowerIO IR schema name and version agree with the current schema file.
#   Since 0.11.0, the schema version is the `powerio` crate version.
# - Every publishable crate carries the one workspace version.
set -euo pipefail
cd "$(dirname "$0")/.."

rust_abi=$(grep -oE 'pub const PIO_ABI_VERSION: u32 = [0-9]+' powerio-capi/src/lib.rs | grep -oE '[0-9]+$')
header_abi=$(grep -oE '#define PIO_ABI_VERSION [0-9]+' powerio-capi/include/powerio.h | grep -oE '[0-9]+$')
if [ "$rust_abi" != "$header_abi" ]; then
    echo "ABI version disagreement: Rust says $rust_abi, powerio.h says $header_abi" >&2
    exit 1
fi

workspace_version=$(grep -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
schema_name=$(grep -oE 'pub const IR_SCHEMA_NAME: &str = "[^"]+"' powerio/src/lib.rs | grep -oE '"[^"]+"$' | tr -d '"')
if [ "$schema_name" != "powerio.module" ]; then
    echo "unexpected PowerIO IR schema name: $schema_name" >&2
    exit 1
fi
grep -q 'pub const IR_SCHEMA_VERSION: &str = VERSION;' powerio/src/lib.rs \
    || { echo "IR_SCHEMA_VERSION must track the powerio crate version" >&2; exit 1; }
schema_path="docs/schema/pio-module/$workspace_version/schema.json"
if [ ! -f "$schema_path" ]; then
    echo "$schema_path is not checked in" >&2
    exit 1
fi
grep -q "\"\$id\": \"https://powerio.dev/schema/pio-module/$workspace_version/schema.json\"" \
    "$schema_path" \
    || { echo "the current PowerIO IR schema \$id disagrees with $workspace_version" >&2; exit 1; }

for manifest in powerio/Cargo.toml powerio-core/Cargo.toml powerio-tx/Cargo.toml \
                powerio-dist/Cargo.toml powerio-matrix/Cargo.toml powerio-prob/Cargo.toml \
                powerio-cli/Cargo.toml; do
    grep -q '^version.workspace = true' "$manifest" \
        || { echo "$manifest does not take the workspace version" >&2; exit 1; }
done

# The internal dependency pins publish with the crates, so each must state the
# workspace version it resolves to.
while read -r pinned; do
    if [ "$pinned" != "$workspace_version" ]; then
        echo "an internal dependency pin says $pinned, the workspace says $workspace_version" >&2
        exit 1
    fi
done < <(grep -E '^powerio[a-z-]* = \{ path = "[^"]+", version = "[0-9.]+"' Cargo.toml \
    | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')

# The changelog's top section and the release notes draft state the workspace
# version; the eventual tag is v<workspace version>.
changelog_version=$(grep -m1 -oE '^## [0-9]+\.[0-9]+\.[0-9]+' CHANGELOG.md | grep -oE '[0-9.]+')
if [ "$changelog_version" != "$workspace_version" ]; then
    echo "CHANGELOG.md leads with $changelog_version, the workspace says $workspace_version" >&2
    exit 1
fi
if [ ! -f "docs/release-notes/$workspace_version-draft.md" ]; then
    echo "docs/release-notes/$workspace_version-draft.md is not checked in" >&2
    exit 1
fi
grep -q "PowerIO $workspace_version release notes" "docs/release-notes/$workspace_version-draft.md" \
    || { echo "the release notes draft title does not state $workspace_version" >&2; exit 1; }

echo "release identity OK: ABI $rust_abi, PowerIO IR $schema_name/$workspace_version, workspace $workspace_version, tag v$workspace_version"

# The Arrow payload goldens embed the producing build's powerio_version; a
# renumber that misses one fails the golden comparisons later. Stored
# document fixtures under other directories legitimately keep the versions
# of the generations they exercise, so only the live-build goldens sweep.
stale=$(grep -rL "\"powerio_version\": \"$workspace_version\"" \
    tests/data/capi_matrix 2>/dev/null || true)
if [ -n "$stale" ]; then
    echo "goldens embed a powerio_version other than $workspace_version:" >&2
    echo "$stale" >&2
    exit 1
fi

echo "release versions OK"
