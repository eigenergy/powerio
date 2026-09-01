#!/usr/bin/env bash
# Check that every exported pio_* Rust symbol is declared in powerio.h.
#
# Symbol NAMES only. A reordered argument, a changed type, or a struct field all
# pass this check — capi-header-regen.sh is the one that catches those. This one
# is cheap enough to run in every feature job; that one needs cbindgen.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

{
    grep -ohE 'extern "C" fn pio_[a-z0-9_]+' powerio-capi/src/lib.rs \
        | grep -oE 'pio_[a-z0-9_]+'
    awk '
        /^(scuc_transformer_control_accessors|scuc_membership_accessors)!\(/ {
            generated = 1
            next
        }
        generated && /^    pio_[a-z0-9_]+,/ {
            name = $1
            sub(/,$/, "", name)
            print name
        }
        generated && /^\);/ { generated = 0 }
    ' powerio-capi/src/lib.rs
} | sort -u >"$tmp/rs_syms"

grep -oE 'pio_[a-z0-9_]+ *\(' powerio-capi/include/powerio.h \
    | grep -oE 'pio_[a-z0-9_]+' \
    | sort -u >"$tmp/h_syms"

if ! diff -u "$tmp/rs_syms" "$tmp/h_syms"; then
    echo "C ABI header symbol parity failed" >&2
    echo "Regenerate or edit powerio-capi/include/powerio.h after changing exported pio_* functions." >&2
    exit 1
fi

echo "C ABI header symbols match Rust exports"
