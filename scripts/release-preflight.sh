#!/usr/bin/env bash
# The nonpublishing release preflight: exercise the decision logic the tag,
# artifact, and registration paths would run, on any ref, writing nothing.
# It never creates a tag, release, registry entry, or artifact update.
set -euo pipefail
cd "$(dirname "$0")/.."

run() { echo "=== $* ==="; "$@"; }

run bash scripts/check-release-versions.sh
run bash scripts/check-release-features.sh
run bash scripts/deprecated-inventory.sh --assert-empty

# The tag decision, against the tag the workspace version implies.
version=$(grep -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
run env TAG="v$version" bash scripts/release-tag-check.sh

# The five asset names the tag workflow builds and PowerIO.jl's artifact
# updater accepts, derived from the one triplet list. A drift in either place
# fails here instead of after a human publishes.
expected_triplets="aarch64-apple-darwin aarch64-linux-gnu x86_64-apple-darwin x86_64-linux-gnu x86_64-w64-mingw32"
workflow_triplets=$(grep -oE 'triplet: [a-z0-9_-]+' .github/workflows/release-binaries.yml \
    | sed 's/triplet: //' | sort | tr '\n' ' ' | sed 's/ $//')
if [ "$workflow_triplets" != "$expected_triplets" ]; then
    echo "release-binaries.yml builds \"$workflow_triplets\"; the release expects \"$expected_triplets\"" >&2
    exit 1
fi
for triplet in $expected_triplets; do
    echo "asset: libpowerio_capi.$triplet.tar.gz"
done

# The publishable crate set, in dependency order, with no retired member. The
# actual archive audit runs in ci-mirror's package step.
publishable=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; m=json.load(sys.stdin); print("\n".join(sorted(p["name"] for p in m["packages"] if p.get("publish") != [])))')
echo "publishable crates:"; printf '%s\n' "$publishable"
for retired in powerio-pkg powerio-diag; do
    if printf '%s\n' "$publishable" | grep -qx "$retired"; then
        echo "retired crate $retired is still publishable" >&2
        exit 1
    fi
done

echo "=== release preflight OK: v$version decision paths hold ==="
