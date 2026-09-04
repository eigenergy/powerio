#!/usr/bin/env bash
# Release identity agreement: the versions a consumer can observe must state
# one story before anything publishes.
#
# - PIO_ABI_VERSION in the Rust source and the checked-in C header agree.
# - The generated PowerIO IR schema states the workspace version: since
#   0.11.0 the IR version is the `powerio` crate version, so the schema file,
#   its `$id`, and its header constants all name that version.
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
# The generated schema is what a consumer validates a document against, so it
# is the artifact that states the IR identity: CI regenerates it from the Rust
# constants, and this gate reads the identity back out of it.
schema_path="docs/schema/pio-module/$workspace_version/schema.json"
if [ ! -f "$schema_path" ]; then
    echo "$schema_path is not checked in" >&2
    exit 1
fi
schema_name=$(python3 - "$schema_path" "$workspace_version" <<'PY'
import json
import sys

path, version = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as handle:
    schema = json.load(handle)
header = schema["properties"]
expected_id = f"https://powerio.dev/schema/pio-module/{version}/schema.json"
problems = []
if schema.get("$id") != expected_id:
    problems.append(f"$id is {schema.get('$id')!r}, not {expected_id}")
if header["version"].get("const") != version:
    problems.append(f"the version constant is {header['version'].get('const')!r}, not {version!r}")
if problems:
    print(f"{path}: " + "; ".join(problems), file=sys.stderr)
    sys.exit(1)
print(header["schema"]["const"])
PY
) || { echo "the current PowerIO IR schema disagrees with workspace version $workspace_version" >&2; exit 1; }
if [ "$schema_name" != "powerio.module" ]; then
    echo "unexpected PowerIO IR schema name: $schema_name" >&2
    exit 1
fi

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
