#!/usr/bin/env bash
# One checked source of truth for the release feature set: every workflow and
# script that names the set must quote .github/release-features exactly, the
# capi manifest must declare each member, and no location may still name the
# retired pkg feature.
set -euo pipefail
cd "$(dirname "$0")/.."

expected=$(tr -d '[:space:]' < .github/release-features)
if [ -z "$expected" ]; then
    echo "empty .github/release-features" >&2
    exit 1
fi

# Every literal feature list in workflows and scripts must be the expected set.
lists=$(grep -rhoE -- '--features [a-z,]+' .github/workflows scripts \
    | sed 's/--features //' | grep ',' | sort -u || true)
for found in $lists; do
    if [ "$found" != "$expected" ]; then
        echo "a workflow or script names the feature set \"$found\"; .github/release-features says \"$expected\"" >&2
        exit 1
    fi
done

# The manifest must declare each member.
for feature in ${expected//,/ }; do
    if ! grep -qE "^$feature = " powerio-capi/Cargo.toml; then
        echo "powerio-capi/Cargo.toml does not declare feature $feature" >&2
        exit 1
    fi
done

# The retired pkg feature and its define must not survive anywhere a release
# path reads.
if grep -rn 'PIO_PKG' .github/workflows scripts | grep -v 'check-release-features'; then
    echo "a release path still names the retired pkg define" >&2
    exit 1
fi

echo "release feature set OK: $expected"
