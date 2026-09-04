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
# The tag workflow copies the CHANGELOG section headed exactly `## <version>`
# into the draft release body; a suffix such as "(unreleased)" leaves that
# body empty and fails the run after every platform has built.
if ! grep -qx "## $version" CHANGELOG.md; then
    echo "CHANGELOG.md needs a section headed exactly '## $version' for the release body" >&2
    exit 1
fi
echo "release tag OK: $TAG matches workspace $version"
