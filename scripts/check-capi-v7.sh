#!/usr/bin/env bash
# PowerIO 1.0 ships exactly ABI 7. This gate checks the ABI number, rejects
# removed beta entry points, and checks a paired PowerIO.jl checkout when one
# is available.
set -euo pipefail
cd "$(dirname "$0")/.."

header=powerio-capi/include/powerio.h
source_file=powerio-capi/src/lib.rs

grep -qx '#define PIO_ABI_VERSION 7' <(grep '^#define PIO_ABI_VERSION ' "$header")
grep -q '^pub const PIO_ABI_VERSION: u32 = 7;$' "$source_file"

declared() {
    grep -oE 'pio_[a-z0-9_]+ *\(' "$header" | grep -oE 'pio_[a-z0-9_]+' | sort -u
}

removed='pio_parse_file
pio_parse_str
pio_parse_bytes
pio_write_file
pio_write_string
pio_module_kind
pio_module_read_json
pio_module_write_json
pio_module_try_into_typed
pio_list_states
pio_select_state
pio_export_state
pio_materialize_network
pio_dc_data
pio_dc_network_data'

present=$(comm -12 <(printf '%s\n' "$removed" | sort -u) <(declared))
if [ -n "$present" ]; then
    echo "error: removed beta C entry points are declared by ABI 7:" >&2
    printf '%s\n' "$present" >&2
    exit 1
fi

echo "ABI 7 number and removed entry point checks pass"

jl=${POWERIO_JL:-../PowerIO.jl}
if [ -d "$jl/src" ]; then
    # A Julia Symbol assembled as `:pio_diagnostic_$field` leaves the literal
    # prefix `pio_diagnostic_` in source. It is not an entry point name; the
    # diagnostic parity gate checks the completed names separately.
    named=$(grep -rhoE ':pio_[a-z0-9_]+' "$jl/src" \
        | grep -oE 'pio_[a-z0-9_]+' | grep -vE '_$' | sort -u)
    missing=$(comm -23 <(printf '%s\n' "$named") <(declared))
    if [ -n "$missing" ]; then
        echo "error: the PowerIO.jl checkout names entry points ABI 7 does not declare:" >&2
        printf '%s\n' "$missing" >&2
        exit 1
    fi
    echo "PowerIO.jl entry point coverage passes ($(printf '%s\n' "$named" | grep -c .) named)"
elif [ "${POWERIO_JL_OPTIONAL:-0}" = 1 ]; then
    echo "PowerIO.jl entry point coverage skipped: no checkout at $jl"
else
    echo "error: no PowerIO.jl checkout at $jl; set POWERIO_JL or POWERIO_JL_OPTIONAL=1" >&2
    exit 1
fi
