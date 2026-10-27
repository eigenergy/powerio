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
# Compare the new crate once the selected release baseline contains it.
excluded=(--exclude powerio-py --exclude powerio-capi)
if [ -f powerio-tnd/Cargo.toml ] \
    && ! git cat-file -e "$baseline:powerio-tnd/Cargo.toml" 2>/dev/null; then
    echo "powerio-tnd is new since $baseline; no previous API to compare"
    excluded+=(--exclude powerio-tnd)
fi
if [ -f powerio-synth/Cargo.toml ] \
    && ! git cat-file -e "$baseline:powerio-synth/Cargo.toml" 2>/dev/null; then
    echo "powerio-synth is new since $baseline; no previous API to compare"
    excluded+=(--exclude powerio-synth)
fi
baseline_root=$(mktemp -d)
current_root=$(mktemp -d)
trap 'rm -rf "$baseline_root" "$current_root"' EXIT
git archive "$baseline" | tar -x -C "$baseline_root"
git archive HEAD | tar -x -C "$current_root"
# Exercise the old-baseline/new-current dependency combination explicitly, then
# apply the same preparation to the isolated source archives.
python3 scripts/semver_zip_compat.py --self-test
python3 scripts/semver_zip_compat.py "$baseline_root" "$current_root"
# Rustdoc JSON cannot follow foreign re-exports. In the isolated archives,
# expose the moved implementation locally while retaining the actual export
# lists, so the full API checks still cover every moved item.
python3 scripts/semver_tnd_compat.py --self-test
python3 scripts/semver_tnd_compat.py "$baseline_root" "$current_root"
# Run SemVer against the isolated current checkout. cargo-semver-checks creates
# synthetic consumer packages from this workspace, so running it against the
# original checkout would still attempt to resolve the yanked ^8.6 dependency.
cd "$current_root"
cargo semver-checks check-release --workspace \
    "${excluded[@]}" \
    --baseline-root "$baseline_root" "$@"
