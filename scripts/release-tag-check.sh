#!/usr/bin/env bash
# The release tag must be v<workspace version>: the tarballs and the draft
# release are named for the tag and PowerIO.jl pins them by it. TAG comes from
# the environment; the tag workflow passes the pushed ref, the preflight
# passes the fixture tag the workspace version implies.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${TAG:?TAG must name the release tag under test}"
pkgid="$(cargo pkgid -p powerio)"
version="${pkgid##*#}"
version="${version##*@}"
if [ "v$version" != "$TAG" ]; then
    echo "tag $TAG does not match workspace version $version; bump Cargo.toml" >&2
    exit 1
fi
echo "release tag OK: $TAG matches workspace $version"
