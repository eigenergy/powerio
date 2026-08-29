#!/usr/bin/env bash
# Diagnostic record parity: the field set and the severity ladder must agree
# across the core record, the stored version 1 DTO, the C row accessors, the
# Python class, and (when POWERIO_JL names a checkout) the Julia struct. The
# details map stays an open JSON object on every surface; everything else is
# a named field.
set -euo pipefail
cd "$(dirname "$0")/.."

fields="code severity message id target suggested_action spans related details"
severities="note remark warning error"

# The core record.
for field in $fields; do
    grep -q "    $field:" powerio-core/src/diagnostic.rs \
        || { echo "powerio-core Diagnostic has no field $field" >&2; exit 1; }
done
for severity in $severities; do
    grep -q "\"$severity\"" powerio-core/src/diagnostic.rs \
        || { echo "powerio-core severity ladder misses $severity" >&2; exit 1; }
done

# The stored DTO.
for field in $fields; do
    grep -q "pub $field:" powerio/src/stored/dto.rs \
        || { echo "stored DiagnosticV1 has no field $field" >&2; exit 1; }
done

# The C row accessors (details crosses as JSON by design).
for accessor in code severity message id target suggested_action details_json \
                n_spans span n_related related; do
    grep -q "pio_diagnostic_$accessor" powerio-capi/include/powerio.h \
        || { echo "powerio.h has no pio_diagnostic_$accessor" >&2; exit 1; }
done

# The Python class: defined in the native module's stub, re-exported by the
# package stub.
for field in code severity message id target suggested_action spans related details; do
    grep -q "$field" python/powerio/_powerio.pyi \
        || { echo "the Python stub does not name Diagnostic.$field" >&2; exit 1; }
done
grep -q "from ._powerio import Diagnostic" python/powerio/__init__.pyi \
    || { echo "the package stub does not re-export Diagnostic" >&2; exit 1; }

if [ -n "${POWERIO_JL:-}" ] && [ -f "$POWERIO_JL/src/diagnostics.jl" ]; then
    for field in $fields; do
        grep -q "    $field::" "$POWERIO_JL/src/diagnostics.jl" \
            || { echo "the Julia Diagnostic has no field $field" >&2; exit 1; }
    done
    echo "diagnostic parity OK across core, stored, C, Python, and Julia"
else
    echo "diagnostic parity OK across core, stored, C, and Python (POWERIO_JL unset: Julia skipped)"
fi
