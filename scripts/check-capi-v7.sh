#!/usr/bin/env bash
# PowerIO 0.11 ships exactly ABI 7. This gate checks the ABI number, rejects
# removed beta entry points, and, when a paired PowerIO.jl checkout is
# available, checks the entry point names both ways: every name the binding
# calls is declared by ABI 7, and every name ABI 7 declares is either called by
# the binding or exempted in its gen/unbound_entry_points.txt.
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
    # The other direction: every entry point ABI 7 declares is either called by
    # the PowerIO.jl sources or listed in gen/unbound_entry_points.txt with a
    # reason. A new entry point that reaches main with neither fails here
    # rather than shipping a binding that cannot call it.
    exempt_file=$jl/gen/unbound_entry_points.txt
    if [ ! -f "$exempt_file" ]; then
        echo "error: no $exempt_file in the PowerIO.jl checkout, which predates the entry point coverage rule; update the checkout" >&2
        exit 1
    fi
    # `name<TAB>reason` per line. Comment and blank lines name nothing.
    exempt=$(grep -v -e '^#' -e '^[[:space:]]*$' "$exempt_file" | cut -f1 | sort -u)

    unbound=$(comm -23 <(comm -23 <(declared) <(printf '%s\n' "$named")) \
        <(printf '%s\n' "$exempt"))
    if [ -n "$unbound" ]; then
        echo "error: ABI 7 declares entry points the PowerIO.jl checkout neither calls nor exempts:" >&2
        printf '%s\n' "$unbound" >&2
        echo "bind each one in PowerIO.jl on a companion branch of the same name as this branch, or list it in gen/unbound_entry_points.txt with a reason" >&2
        exit 1
    fi

    # An exemption for a name ABI 7 no longer declares, left by a rename or a
    # removal that nobody carried into the exemption file.
    unknown=$(comm -23 <(printf '%s\n' "$exempt") <(declared))
    if [ -n "$unknown" ]; then
        echo "error: gen/unbound_entry_points.txt exempts entry points ABI 7 does not declare:" >&2
        printf '%s\n' "$unknown" >&2
        echo "drop or rename each stale exemption" >&2
        exit 1
    fi

    # An exemption for a name the sources now call.
    bound=$(comm -12 <(printf '%s\n' "$exempt") <(printf '%s\n' "$named"))
    if [ -n "$bound" ]; then
        echo "error: gen/unbound_entry_points.txt exempts entry points the PowerIO.jl sources call:" >&2
        printf '%s\n' "$bound" >&2
        echo "drop the exemption line for each" >&2
        exit 1
    fi

    echo "PowerIO.jl entry point coverage passes ($(declared | grep -c .) declared, $(printf '%s\n' "$named" | grep -c .) bound, $(printf '%s\n' "$exempt" | grep -c .) exempt)"
elif [ "${POWERIO_JL_OPTIONAL:-0}" = 1 ]; then
    echo "PowerIO.jl entry point coverage skipped: no checkout at $jl"
else
    echo "error: no PowerIO.jl checkout at $jl; set POWERIO_JL or POWERIO_JL_OPTIONAL=1" >&2
    exit 1
fi
