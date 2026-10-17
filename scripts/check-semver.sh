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
# powerio-py and powerio-capi are publish = false; name them anyway so the
# crate set reads the same as the msrv and archive jobs.
excluded=(--exclude powerio-py --exclude powerio-capi)
# This crate is introduced by the SINCAL stack, so older release trees have
# no API for it to compare. Once a release contains its manifest, it joins
# the ordinary workspace comparison automatically.
if [ -f powerio-sincal/Cargo.toml ] \
    && ! git cat-file -e "$baseline:powerio-sincal/Cargo.toml" 2>/dev/null; then
    echo "powerio-sincal is new since $baseline; no previous API to compare"
    excluded+=(--exclude powerio-sincal)
fi
cargo semver-checks check-release --workspace \
    "${excluded[@]}" --baseline-rev "$baseline" "$@"
