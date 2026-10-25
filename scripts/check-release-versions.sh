#!/usr/bin/env bash
# Release identity agreement: the versions a consumer can observe must state
# one story before anything publishes.
#
# - PIO_ABI_VERSION in the Rust source and the checked-in C header agree.
# - IR_VERSION, the release whose PowerIO IR schema this build writes, has its
#   own schema file, and that file and the facade constants agree on it. It is
#   at or after the workspace version during development, and `--release`
#   (passed when a tag is built or published) requires it to equal it.
# - Every publishable crate carries the one workspace version.
set -euo pipefail
cd "$(dirname "$0")/.."

release=false
case "${1:-}" in
    "") ;;
    --release) release=true ;;
    *) echo "usage: $0 [--release]" >&2; exit 2 ;;
esac

rust_abi=$(grep -oE 'pub const PIO_ABI_VERSION: u32 = [0-9]+' powerio-capi/src/lib.rs | grep -oE '[0-9]+$')
header_abi=$(grep -oE '#define PIO_ABI_VERSION [0-9]+' powerio-capi/include/powerio.h | grep -oE '[0-9]+$')
if [ "$rust_abi" != "$header_abi" ]; then
    echo "ABI version disagreement: Rust says $rust_abi, powerio.h says $header_abi" >&2
    exit 1
fi

workspace_version=$(grep -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
# The generated schema is what a consumer validates a document against, so it
# is the artifact that states the PowerIO IR identity: CI regenerates it from
# the Rust constants, and this gate reads the identity back out of it.
ir_version=$(python3 - "$workspace_version" "$release" <<'PY'
import hashlib
import json
import re
import sys
from pathlib import Path

workspace, release = sys.argv[1], sys.argv[2] == "true"
source = Path("powerio/src/lib.rs").read_text()


def constant(name):
    match = re.search(rf'pub const {name}: &str\s*=\s*"([^"]+)"', source)
    if match is None:
        raise SystemExit(f"{name} is not a string constant")
    return match.group(1)


def parse(text):
    if re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", text) is None:
        raise SystemExit(f"{text!r} is not a MAJOR.MINOR.PATCH release")
    return tuple(int(part) for part in text.split("."))


version = constant("IR_VERSION")
schema_id = constant("IR_SCHEMA_ID")
if parse(version) < parse(workspace):
    raise SystemExit(
        f"IR_VERSION {version} is behind the workspace version {workspace}; set it to "
        f"{workspace} and generate docs/schema/pio-ir/{workspace}/schema.json"
    )
if release and version != workspace:
    raise SystemExit(f"IR_VERSION {version} is not the release being published, {workspace}")
expected_id = f"https://powerio.dev/schema/pio-ir/{version}/schema.json"
if schema_id != expected_id:
    raise SystemExit(f"IR_SCHEMA_ID is {schema_id}, not {expected_id}")

published = Path("docs/schema/pio-ir/2/schema.json").read_bytes()
if hashlib.sha256(published).hexdigest() != "8700329706173edb90924460464673310dabb5f4c5264b96fc0ff03eea42c812":
    raise SystemExit("the published 0.11.0 schema changed")
path = Path(f"docs/schema/pio-ir/{version}/schema.json")
if not path.is_file():
    raise SystemExit(f"{path} is not checked in")
schema = json.loads(path.read_text(encoding="utf-8"))
header = schema["properties"]
problems = []
if schema.get("$id") != expected_id:
    problems.append(f"$id is {schema.get('$id')!r}, not {expected_id}")
if header["schema"].get("const") != "pio-ir":
    problems.append(f"the schema name is {header['schema'].get('const')!r}, not 'pio-ir'")
if header["version"].get("const") != version:
    problems.append(f"the version constant is {header['version'].get('const')!r}, not {version!r}")
if problems:
    raise SystemExit(f"{path}: " + "; ".join(problems))
print(version)
PY
) || { echo "IR_VERSION and its schema file disagree with the release" >&2; exit 1; }

for manifest in powerio/Cargo.toml powerio-core/Cargo.toml powerio-tx/Cargo.toml \
                powerio-dist/Cargo.toml powerio-tnd/Cargo.toml powerio-matrix/Cargo.toml powerio-prob/Cargo.toml \
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

# The changelog and release notes state the workspace version.
# Final release notes take precedence over a draft for the same version.
changelog_version=$(grep -m1 -oE '^## [0-9]+\.[0-9]+\.[0-9]+' CHANGELOG.md | grep -oE '[0-9.]+')
if [ "$changelog_version" != "$workspace_version" ]; then
    echo "CHANGELOG.md leads with $changelog_version, the workspace says $workspace_version" >&2
    exit 1
fi
release_notes="docs/release-notes/$workspace_version.md"
if [ ! -f "$release_notes" ]; then
    release_notes="docs/release-notes/$workspace_version-draft.md"
fi
if [ ! -f "$release_notes" ]; then
    echo "no final or draft release notes are checked in for $workspace_version" >&2
    exit 1
fi
grep -q "PowerIO $workspace_version release notes" "$release_notes" \
    || { echo "the release notes title does not state $workspace_version" >&2; exit 1; }

echo "release identity OK: ABI $rust_abi, PowerIO IR pio-ir/$ir_version, workspace $workspace_version, tag v$workspace_version"

# The Arrow payload goldens embed the producing build's powerio_version; a
# renumber that misses one fails the golden comparisons later. Stored
# document fixtures under other directories legitimately keep the versions
# and producers of the releases that wrote them, so only the live-build
# goldens sweep.
stale=$(grep -rL "\"powerio_version\": \"$workspace_version\"" \
    tests/data/capi_matrix 2>/dev/null || true)
if [ -n "$stale" ]; then
    echo "goldens embed a powerio_version other than $workspace_version:" >&2
    echo "$stale" >&2
    exit 1
fi

echo "release versions OK"
