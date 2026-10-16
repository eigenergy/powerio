#!/usr/bin/env bash
# Public Rust API compatibility: compare every publishable crate against the
# newest stable release tag at or below the workspace version with
# cargo-semver-checks.
#
# zip 8.6.0, used by the v0.11.x release line, has been yanked. The SemVer
# tool builds temporary consumer manifests and runs cargo update, which cannot
# resolve that yanked version. For the isolated comparison snapshots only,
# use zip 9.0.3 and adapt the three private archive-name calls whose return
# types changed in zip 9. This keeps the public API comparison enabled while
# avoiding a dependency-resolution failure unrelated to the public API.
# The repository manifests, source files, and lockfile are not rewritten.
#
# The tags must be present: a shallow clone fails here rather than comparing
# against nothing. Extra arguments go to cargo-semver-checks.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! cargo semver-checks --version >/dev/null 2>&1; then
    echo "cargo-semver-checks is not installed; run: cargo install cargo-semver-checks --locked" >&2
    exit 2
fi

version=$(grep -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
baseline=
for tag in $(git tag --list 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V); do
    if [ "$(printf '%s\n%s\n' "${tag#v}" "$version" | sort -V | tail -n1)" = "$version" ]; then
        baseline=$tag
    fi
done
if [ -z "$baseline" ]; then
    echo "no stable release tag at or below $version; fetch the tags (git fetch --tags)" >&2
    exit 1
fi

baseline_root=$(mktemp -d "${TMPDIR:-/tmp}/powerio-semver-baseline.XXXXXX")
current_root=$(mktemp -d "${TMPDIR:-/tmp}/powerio-semver-current.XXXXXX")
trap 'rm -rf "$baseline_root" "$current_root"' EXIT
git archive "$baseline" | tar -x -C "$baseline_root"
git archive HEAD | tar -x -C "$current_root"

python3 - "$baseline_root" "$current_root" <<'PY'
from pathlib import Path
import sys

for root_name in sys.argv[1:]:
    root = Path(root_name)
    manifest_path = root / "Cargo.toml"
    manifest = manifest_path.read_text()
    old_dependency = 'zip = { version = "8.6",'
    new_dependency = 'zip = { version = "9.0.3",'
    if old_dependency not in manifest:
        raise SystemExit(f"expected zip 8.6 workspace dependency in {manifest_path}")
    manifest_path.write_text(manifest.replace(old_dependency, new_dependency, 1))

    source_path = root / "powerio-tx/src/format/cgmes/mod.rs"
    source = source_path.read_text()
    replacements = (
        (
            'let raw_name = file.name().to_string();',
            'let raw_name = file.name().map_err(|error| format_error(format!("cannot read entry name from {archive_name}: {error}")))?.into_owned();',
        ),
        (
            '                    entry.name()',
            '                    entry.name().map(|name| name.into_owned()).unwrap_or_else(|_| format!("<entry {index}>"))',
        ),
        (
            '        let name = |index| archive.name_for_index(index).unwrap_or_default();',
            '        let name = |index| archive.name_for_index(index).and_then(|result| result.ok()).map(|name| name.into_owned()).unwrap_or_default();',
        ),
    )
    for old, new in replacements:
        count = source.count(old)
        if count != 1:
            raise SystemExit(f"expected exactly one occurrence of {old!r} in {source_path}; found {count}")
        source = source.replace(old, new, 1)
    source_path.write_text(source)

print("prepared isolated zip-9 snapshots with private API compatibility adjustments")
PY

echo "comparing publishable crates at $version against $baseline using isolated snapshots"
# powerio-py and powerio-capi are publish = false; exclude only those crates.
cd "$current_root"
cargo semver-checks check-release --workspace \
    --exclude powerio-py --exclude powerio-capi \
    --baseline-root "$baseline_root" "$@"
