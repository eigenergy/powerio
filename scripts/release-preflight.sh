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

# crates.yml hardcodes the publishable set (a gate) and the publish order (a
# loop) as literal strings; both drift silently when a crate is added, so
# check each against the workspace directly rather than trusting the file.
gate_string=$(grep -oE 'actual" != "[^"]*"' .github/workflows/crates.yml \
    | sed -E 's/^actual" != "//; s/"$//')
gate_sorted=$(printf '%s\n' "$gate_string" | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//')
publishable_line=$(printf '%s\n' "$publishable" | tr '\n' ' ' | sed 's/ $//')
if [ "$gate_sorted" != "$publishable_line" ]; then
    echo "crates.yml's publish gate (\"$gate_string\") does not match the actual publishable set (\"$publishable_line\")" >&2
    exit 1
fi

loop_order=$(grep -oE 'for crate in [^;]+; do' .github/workflows/crates.yml \
    | sed -E 's/^for crate in //; s/; do$//')
PIO_LOOP_ORDER="$loop_order" python3 -c '
import json, os, subprocess, sys

order = os.environ["PIO_LOOP_ORDER"].split()
meta = json.loads(subprocess.run(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"],
    check=True, capture_output=True, text=True,
).stdout)
publishable = {p["name"] for p in meta["packages"] if p.get("publish") != []}
if set(order) != publishable:
    order_str = " ".join(order)
    pub_str = " ".join(sorted(publishable))
    sys.exit("crates.yml publish loop (" + order_str + ") does not name exactly the publishable set (" + pub_str + ")")
deps = {}
for pkg in meta["packages"]:
    if pkg["name"] not in publishable:
        continue
    deps[pkg["name"]] = {
        d["name"] for d in pkg["dependencies"]
        if d.get("kind") in (None, "build") and d["name"] in publishable
    }
position = {name: i for i, name in enumerate(order)}
for name, index in position.items():
    for dep in deps[name]:
        if position[dep] > index:
            sys.exit("crates.yml publishes " + name + " before its dependency " + dep + "; fix the loop order")
print("publish loop order:", " ".join(order))
'

echo "=== release preflight OK: v$version decision paths hold ==="
