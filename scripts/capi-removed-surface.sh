#!/usr/bin/env bash
# The abi-v6.md removed-surface enumeration must equal the exact set of C
# symbols declared in the baseline header and absent from the current one.
# Symbols are extracted from declarations, the same way capi-header-parity.sh
# reads them, so prose mentions elsewhere in the book cannot drift the gate.
set -euo pipefail
cd "$(dirname "$0")/.."

doc=docs/src/abi-v6.md
header=powerio-capi/include/powerio.h
baseline=${PIO_REMOVED_SURFACE_BASELINE:-v0.9.0}

declared() { grep -oE '\bpio_[a-z0-9_]+ *\(' | sed 's/ *(//' | sort -u; }

old=$(git show "$baseline:$header" | declared)
new=$(declared < "$header")
removed=$(comm -23 <(printf '%s\n' "$old") <(printf '%s\n' "$new"))

enumerated=$(sed -n '/removed-c-surface:begin/,/removed-c-surface:end/p' "$doc" \
    | grep -oE '\bpio_[a-z0-9_]+' | sort -u)

if [ -z "$enumerated" ]; then
    echo "no removed-c-surface enumeration found in $doc" >&2
    exit 1
fi
if [ "$removed" != "$enumerated" ]; then
    echo "$doc does not enumerate the $baseline -> current header difference:" >&2
    diff <(printf '%s\n' "$removed") <(printf '%s\n' "$enumerated") >&2 || true
    exit 1
fi
echo "removed surface enumeration matches ($(printf '%s\n' "$removed" | grep -c .) symbols)"
