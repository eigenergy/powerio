#!/usr/bin/env bash
# Regenerate powerio.h with cbindgen and diff it against the committed copy.
#
# This is the authoritative header check. capi-header-parity.sh compares symbol
# NAMES only, so it cannot see a changed argument order, a changed type, or a
# struct field — the class of defect that let pio_convert_file ship with two
# arguments reversed. Both run: the parity script is cheap enough for every
# feature job, this one needs cbindgen and runs once.
#
# cbindgen maps each optional feature to an #ifdef via cbindgen.toml [defines],
# so one generated header covers every feature configuration and this check does
# not need to be repeated per feature set.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cbindgen >/dev/null 2>&1; then
    echo "cbindgen not found. Install it with:" >&2
    echo "    cargo install cbindgen --locked" >&2
    exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cbindgen --config powerio-capi/cbindgen.toml \
         --crate powerio-capi \
         --output "$tmp/powerio.h" \
         --quiet

if ! diff -u powerio-capi/include/powerio.h "$tmp/powerio.h"; then
    echo >&2
    echo "powerio.h is out of date with the Rust source." >&2
    echo "Regenerate it (never hand-edit):" >&2
    echo "    cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi --output powerio-capi/include/powerio.h" >&2
    exit 1
fi

echo "powerio.h matches cbindgen output"
