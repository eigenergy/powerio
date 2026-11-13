#!/usr/bin/env bash
# Release identity agreement: the versions a consumer can observe must state
# one story before anything publishes.
#
# The workspace version is MAJOR.MINOR.PATCH with an optional prerelease
# suffix (1.0.0-rc.1). The suffix marks a prerelease of the core release;
# everything that states a release number states that core release.
#
# - PIO_VERSION_MAJOR, _MINOR, and _PATCH in the Rust source and the
#   checked-in C header state the core of the workspace version.
# - IR_VERSION, the release whose PowerIO IR schema this build writes, has its
#   own schema file, and that file and the facade constants agree on it. It is
#   at or after the core workspace version during development, and `--release`
#   (passed when a tag is built or published) requires it to equal it. In a
#   GitHub Actions run for a `v*` tag, `--release` also requires the tag to be
#   v<workspace version>.
# - The published 0.11 schema files keep their exact bytes.
# - Every publishable crate carries the one workspace version.
set -euo pipefail
cd "$(dirname "$0")/.."

release=false
case "${1:-}" in
    "") ;;
    --release) release=true ;;
    *) echo "usage: $0 [--release]" >&2; exit 2 ;;
esac

semver='[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?'
workspace_version=$(grep -m1 -oE "^version = \"$semver\"" Cargo.toml | grep -oE "$semver")
core_version=${workspace_version%%-*}
# The header states the release it was generated from, so a version bump
# updates the constants and regenerates powerio.h.
c_version() {
    local part
    for part in MAJOR MINOR PATCH; do
        grep -oE "$1$part$2[0-9]+" "$3" | grep -oE '[0-9]+$' || echo '?'
    done | paste -sd. -
}
rust_version=$(c_version 'pub const PIO_VERSION_' ': u32 = ' powerio-capi/src/lib.rs)
header_version=$(c_version '#define PIO_VERSION_' ' ' powerio-capi/include/powerio.h)
if [ "$rust_version" != "$core_version" ] || [ "$header_version" != "$core_version" ]; then
    echo "C header release disagreement: PIO_VERSION_* says $rust_version in powerio-capi/src/lib.rs and $header_version in powerio.h, the workspace says $core_version" >&2
    exit 1
fi
# The generated schema is what a consumer validates a document against, so it
# is the artifact that states the PowerIO IR identity: CI regenerates it from
# the Rust constants, and this gate reads the identity back out of it.
ir_version=$(python3 - "$core_version" "$release" <<'PY'
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

# Published schema files are served at their URLs forever; their bytes stay.
for snapshot, expected_hash in {
    "2/schema.json": "8700329706173edb90924460464673310dabb5f4c5264b96fc0ff03eea42c812",
    "2/0.11.7/schema.json": "b74ba4faff286efcb07981de515a9787a242280fd6edda966daea7de6ba16cde",
    "2/0.11.8/schema.json": "1f60c37db2acba917779af92fa21239067457ead8d4607529c70f7203382dd54",
}.items():
    published = Path("docs/schema/pio-ir", snapshot).read_bytes()
    if hashlib.sha256(published).hexdigest() != expected_hash:
        raise SystemExit(f"the published PowerIO IR schema {snapshot} changed")
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

if $release && [ "${GITHUB_REF_TYPE:-}" = tag ]; then
    case "${GITHUB_REF_NAME:-}" in
        v*)
            if [ "$GITHUB_REF_NAME" != "v$workspace_version" ]; then
                echo "the tag $GITHUB_REF_NAME is not v$workspace_version, the workspace version" >&2
                exit 1
            fi
            ;;
    esac
fi

for manifest in powerio/Cargo.toml powerio-core/Cargo.toml powerio-archive/Cargo.toml powerio-tx/Cargo.toml \
                powerio-dist/Cargo.toml powerio-tnd/Cargo.toml powerio-matrix/Cargo.toml powerio-prob/Cargo.toml \
                powerio-synth/Cargo.toml powerio-cli/Cargo.toml; do
    grep -q '^version.workspace = true' "$manifest" \
        || { echo "$manifest does not take the workspace version" >&2; exit 1; }
done

# The internal dependency requirements publish with the crates, so each must
# state the workspace version it resolves to. A release states it as an
# ordinary caret requirement ("0.12.0"); a prerelease pins it exactly
# ("=1.0.0-rc.1"), so the crates of one prerelease only resolve to each other.
case "$workspace_version" in
    *-*) expected_requirement="=$workspace_version" ;;
    *) expected_requirement="$workspace_version" ;;
esac
internal=$(grep -cE '^powerio[a-z-]* = \{ path = ' Cargo.toml || true)
checked=0
while read -r requirement; do
    checked=$((checked + 1))
    if [ "$requirement" != "$expected_requirement" ]; then
        echo "an internal dependency requires \"$requirement\", the workspace version $workspace_version needs \"$expected_requirement\"" >&2
        exit 1
    fi
done < <(grep -E "^powerio[a-z-]* = \{ path = \"[^\"]+\", version = \"=?$semver\"" Cargo.toml \
    | grep -oE "version = \"=?$semver\"" | cut -d'"' -f2)
if [ "$checked" -ne "$internal" ]; then
    echo "$((internal - checked)) internal dependencies in Cargo.toml state no version requirement this check reads" >&2
    exit 1
fi

# The changelog and release notes state the workspace version.
# Final release notes take precedence over a draft for the same version.
changelog_version=$(grep -m1 -oE "^## $semver\$" CHANGELOG.md | grep -oE "$semver")
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

echo "release identity OK: C header $header_version, PowerIO IR pio-ir/$ir_version, workspace $workspace_version, tag v$workspace_version"

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
