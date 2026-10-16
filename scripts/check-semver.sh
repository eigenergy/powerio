#!/usr/bin/env bash
# Public Rust API compatibility: compare every publishable crate against the
# newest stable release tag at or below the workspace version with
# cargo-semver-checks.
#
# A release line takes compatible fixes, performance work, and additive change;
# a break in the public Rust API goes into the next minor release
# (docs/src/scope-0.11.md). cargo-semver-checks reads the release type off the
# version difference, so a branch that keeps the workspace version must keep
# the API, and a branch that bumps it to the next minor may break it. Taking
# the newest tag at or below the workspace version keeps a maintenance branch
# on its own line after a later minor is tagged.
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

echo "comparing the publishable crates at $version against $baseline"
# cargo-semver-checks runs cargo update in synthetic consumers generated from
# the current checkout as well as the baseline. Since ZIP 8.6.0 was yanked,
# those consumers cannot resolve the current workspace's ^8.6 dependency.
# Apply the same narrow compatibility adjustment to temporary source archives
# on both sides of the comparison; neither the real checkout nor the published
# API is changed by this workaround.
baseline_root=$(mktemp -d)
current_root=$(mktemp -d)
trap 'rm -rf "$baseline_root" "$current_root"' EXIT
git archive "$baseline" | tar -x -C "$baseline_root"
git archive HEAD | tar -x -C "$current_root"
python3 - "$baseline_root" "$current_root" <<'PY'
import pathlib
import sys

for root in map(pathlib.Path, sys.argv[1:]):
    manifest = root / "Cargo.toml"
    text = manifest.read_text()
    old_zip = 'zip = { version = "8.6", default-features = false, features = ["deflate-flate2-zlib-rs"] }'
    new_zip = 'zip = { version = "9.0.3", default-features = false, features = ["deflate-flate2-zlib-rs"] }'
    if text.count(old_zip) != 1:
        raise SystemExit(f"expected exactly one ZIP 8.6 dependency in {root}")
    manifest.write_text(text.replace(old_zip, new_zip))

    source = root / "powerio-tx/src/format/cgmes/mod.rs"
    text = source.read_text()
    replacements = [
        (
            'let raw_name = file.name().to_string();',
            '''let raw_name = file
            .name()
            .map_err(|error| {
                format_error(format!(
                    "cannot decode entry {index} name in {archive_name}: {error}"
                ))
            })?
            .into_owned();''',
        ),
        (
            '''"archive {archive_name} is refused as malformed: entry {} lies outside the archive's entry data",
                    entry.name()''',
            '''"archive {archive_name} is refused as malformed: entry {index} lies outside the archive's entry data"''',
        ),
        (
            'let name = |index| archive.name_for_index(index).unwrap_or_default();',
            '''let name = |index| {
            archive
                .name_for_index(index)
                .and_then(std::result::Result::ok)
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default()
        };''',
        ),
    ]
    for old_anchor, new_anchor in replacements:
        if old_anchor not in text:
            # The current checkout may already carry the ZIP 9 compatibility
            # code. Only the baseline is expected to require these edits.
            continue
        if text.count(old_anchor) != 1:
            raise SystemExit(f"expected exactly one ZIP API compatibility anchor in {root}: {old_anchor!r}")
        text = text.replace(old_anchor, new_anchor)
    source.write_text(text)
PY

# Run SemVer against the isolated current checkout. cargo-semver-checks creates
# synthetic consumer packages from this workspace, so running it against the
# original checkout would still attempt to resolve the yanked ^8.6 dependency.
cd "$current_root"
cargo semver-checks check-release --workspace \
    --exclude powerio-py --exclude powerio-capi \
    --baseline-root "$baseline_root" "$@"
