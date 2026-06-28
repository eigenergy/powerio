#!/usr/bin/env bash
# Check that every exported pio_* Rust symbol is declared in powerio.h.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

grep -oE 'extern "C" fn pio_[a-z_]+' powerio-capi/src/lib.rs \
    | grep -oE 'pio_[a-z_]+' \
    | sort -u >"$tmp/rs_syms"

grep -oE 'pio_[a-z_]+ *\(' powerio-capi/include/powerio.h \
    | grep -oE 'pio_[a-z_]+' \
    | sort -u >"$tmp/h_syms"

if ! diff -u "$tmp/rs_syms" "$tmp/h_syms"; then
    echo "C ABI header symbol parity failed" >&2
    echo "Regenerate or edit powerio-capi/include/powerio.h after changing exported pio_* functions." >&2
    exit 1
fi

echo "C ABI header symbols match Rust exports"
