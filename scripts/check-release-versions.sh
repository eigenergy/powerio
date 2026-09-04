#!/usr/bin/env bash
# Release identity agreement: the versions a consumer can observe must state
# one story before anything publishes.
#
# - PIO_ABI_VERSION in the Rust source and the checked-in C header agree.
# - The generated PowerIO IR schema and the facade constants agree on the
#   independent IR identity and generation.
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
ir_version=$(grep -oE 'pub const IR_VERSION: u64 = [0-9]+' powerio/src/lib.rs | grep -oE '[0-9]+$')
ir_min_version=$(grep -oE 'pub const IR_MIN_VERSION: u64 = [0-9]+' powerio/src/lib.rs | grep -oE '[0-9]+$')
if [ "$ir_min_version" -gt "$ir_version" ]; then
    echo "IR_MIN_VERSION $ir_min_version is later than IR_VERSION $ir_version" >&2
    exit 1
fi
# Every generation the reader accepts has its schema archived.
for generation in $(seq "$ir_min_version" "$ir_version"); do
    if [ ! -f "docs/schema/pio-ir/$generation/schema.json" ]; then
        echo "docs/schema/pio-ir/$generation/schema.json is not checked in for a readable generation" >&2
        exit 1
    fi
done
# The generated schema is what a consumer validates a document against, so it
# is the artifact that states the IR identity: CI regenerates it from the Rust
# constants, and this gate reads the identity back out of it.
schema_path="docs/schema/pio-ir/$ir_version/schema.json"
if [ ! -f "$schema_path" ]; then
    echo "$schema_path is not checked in" >&2
    exit 1
fi
schema_name=$(python3 - "$schema_path" "$ir_version" <<'PY'
import json
import sys

path, version = sys.argv[1], int(sys.argv[2])
with open(path, encoding="utf-8") as handle:
    schema = json.load(handle)
header = schema["properties"]
expected_id = f"https://powerio.dev/schema/pio-ir/{version}/schema.json"
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
) || { echo "the current PowerIO IR schema disagrees with generation $ir_version" >&2; exit 1; }
if [ "$schema_name" != "pio-ir" ]; then
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

echo "release identity OK: ABI $rust_abi, PowerIO IR $schema_name/$ir_version (reads $ir_min_version through $ir_version), workspace $workspace_version, tag v$workspace_version"

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
