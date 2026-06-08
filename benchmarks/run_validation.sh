#!/usr/bin/env bash
# Run every cross-tool correctness validator over every fixture and print a
# pass/fail matrix. Each MATPOWER fixture is checked against:
#
#   PMjson  — powerio's PowerModels JSON (writer) vs PowerModels.jl's own parse of
#             the .m, field by field.              benchmarks/validate_powermodels.jl
#   PMread  — powerio's PowerModels JSON reader: PowerModels exports the .m to JSON,
#             powerio reads it and re-emits, the two are compared.  benchmarks/pm_export.jl
#   PSSE    — powerio's PSS/E .raw vs PowerModels.jl (counts + demand/gen totals).
#                                                   benchmarks/validate_psse.jl
#   Exa     — powerio (via C ABI) vs ExaPowerIO.jl, value for value.
#                                                   benchmarks/validate_exapowerio.jl
#   pp      — powerio's parse + Y_bus vs pandapower (_m2ppc + makeYbus).
#                                                   benchmarks/validate_pandapower.py
#   Surge   — optional: powerio's Surge JSON writer vs Surge's own parser.
#             Set SURGE_BIN=/path/to/surge-solve or SURGE_CHECKOUT=/path/to/surge.
#                                                   benchmarks/validate_surge.py
#
# Then the read sides and the full conversion matrix:
#   PSSE-read   — powerio reads a real PSS/E .raw, emits PowerModels JSON, compared
#                 against PowerModels.jl reading the same .raw.
#   EGRET-read  — powerio reads a real EGRET .json (egret's own output), emits
#                 PowerModels JSON, checked against the matching MATPOWER case.
#   matrix(5x5) — every reader -> every writer over the fixtures covered by the
#                 independent PowerModels and egret oracles, each output's
#                 electrical core checked against the ground-truth MATPOWER case
#                 (PowerModels.jl for MATPOWER/PowerModels/PSS-E/PowerWorld, the
#                 egret package for EGRET), byte-exact on the diagonal.
#                                                   benchmarks/validate_matrix.py
#
# Prereqs: `cargo build --release -p powerio-capi`, the powerio Python extension
# built into .venv (`maturin develop --release`), the Julia env instantiated
# (`julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'`), and the Python
# oracle tools (`pip install -r benchmarks/requirements.txt`, for the pandapower
# and EGRET checks). Surge is optional through SURGE_BIN or SURGE_CHECKOUT. All
# oracle tools are benchmark-scoped, not powerio deps.
#
#   bash benchmarks/run_validation.sh
#
# Exits nonzero if any check fails.

set -uo pipefail
cd "$(dirname "$0")/.."

PY="${PYTHON:-.venv/bin/python}"
JL=(julia --project=benchmarks)
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# The full matrix needs the egret package (the EGRET oracle); it's a benchmark
# dependency, not a powerio one (see benchmarks/requirements.txt). Skip that one
# leg with a notice if it isn't installed, rather than failing the whole run.
HAVE_EGRET=1
"$PY" -c "import egret" 2>/dev/null || HAVE_EGRET=0

HAVE_SURGE=0
if [ -n "${SURGE_BIN:-}" ] || [ -n "${SURGE_CHECKOUT:-}" ]; then
    HAVE_SURGE=1
fi

MCASES=(
    tests/data/case9.m
    tests/data/case14.m
    tests/data/case30.m
    tests/data/case57.m
    tests/data/case118.m
    tests/data/t_case9_dcline.m
    tests/data/t_case9_oos.m
    tests/data/pglib/pglib_opf_case5_pjm.m
    tests/data/pglib/pglib_opf_case14_ieee.m
    tests/data/case2869pegase.m
)
RAWCASES=(tests/data/psse/case5.raw tests/data/psse/case14.raw)

fails=0
rows=()

# Convert a case to another format via the powerio Python package (no CLI build).
convert() { # <in> <to> <out>
    "$PY" - "$@" <<'EOF'
import sys, warnings
warnings.filterwarnings("ignore")
import powerio
inp, to, out = sys.argv[1], sys.argv[2], sys.argv[3]
open(out, "w").write(powerio.convert(inp, to).text)
EOF
    local rc=$?
    # Guard against a silent success that wrote nothing — validating an empty file
    # would otherwise look like a pass.
    if [ "$rc" -eq 0 ] && [ ! -s "$3" ]; then
        echo "convert: $1 → $2 produced empty output" >&2
        return 1
    fi
    return "$rc"
}

# Run a check, echo its output, return a one-word mark for the summary row.
mark() { # <command...> ; sets MARK
    if "$@"; then MARK="ok"; else MARK="FAIL"; fails=$((fails + 1)); fi
}

echo "=== MATPOWER fixtures ==="
for m in "${MCASES[@]}"; do
    base="$(basename "$m" .m)"
    echo "--- $base ---"
    row="$(printf '%-26s' "$base")"

    if convert "$m" powermodels-json "$TMP/$base.json" 2>"$TMP/err"; then
        mark "${JL[@]}" benchmarks/validate_powermodels.jl "$m" "$TMP/$base.json"
    else
        echo "  PMjson: convert failed"; cat "$TMP/err"; MARK="FAIL"; fails=$((fails + 1))
    fi
    row+="  PMjson:$MARK"

    # PowerModels JSON reader: PowerModels exports the .m to JSON, powerio reads that
    # and re-emits, and the two are compared — exercises powerio reading real
    # PowerModels output, not only its own writer.
    if "${JL[@]}" benchmarks/pm_export.jl "$m" "$TMP/$base.pmref.json" 2>"$TMP/err" \
        && convert "$TMP/$base.pmref.json" powermodels-json "$TMP/$base.reemit.json" 2>>"$TMP/err"; then
        mark "${JL[@]}" benchmarks/validate_powermodels.jl "$TMP/$base.pmref.json" "$TMP/$base.reemit.json"
    else
        echo "  PMread: setup failed"; cat "$TMP/err"; MARK="FAIL"; fails=$((fails + 1))
    fi
    row+="  PMread:$MARK"

    if convert "$m" psse "$TMP/$base.raw" 2>"$TMP/err"; then
        mark "${JL[@]}" benchmarks/validate_psse.jl "$m" "$TMP/$base.raw"
    else
        echo "  PSSE: convert failed"; cat "$TMP/err"; MARK="FAIL"; fails=$((fails + 1))
    fi
    row+="  PSSE:$MARK"

    mark "${JL[@]}" benchmarks/validate_exapowerio.jl "$m"
    row+="  Exa:$MARK"

    mark "$PY" benchmarks/validate_pandapower.py "$m"
    row+="  pp:$MARK"

    if [ "$HAVE_SURGE" -eq 0 ]; then
        MARK="SKIP"
    elif [ "$base" = "case2869pegase" ]; then
        MARK="SKIP(nonfinite)"
    elif convert "$m" surge-json "$TMP/$base.surge.json" 2>"$TMP/err"; then
        mark "$PY" benchmarks/validate_surge.py "$m" "$TMP/$base.surge.json"
    else
        echo "  Surge: convert failed"; cat "$TMP/err"; MARK="FAIL"; fails=$((fails + 1))
    fi
    row+="  Surge:$MARK"

    rows+=("$row")
done

echo
echo "=== PSS/E .raw fixtures (read side) ==="
for r in "${RAWCASES[@]}"; do
    base="$(basename "$r" .raw)"
    echo "--- psse/$base ---"
    row="$(printf '%-26s' "psse/$base")"
    if convert "$r" powermodels-json "$TMP/psse_$base.json" 2>"$TMP/err"; then
        mark "${JL[@]}" benchmarks/validate_psse.jl "$r" "$TMP/psse_$base.json"
    else
        echo "  PSSE-read: convert failed"; cat "$TMP/err"; MARK="FAIL"; fails=$((fails + 1))
    fi
    row+="  PSSE-read:$MARK"
    rows+=("$row")
done

echo
echo "=== EGRET .json fixtures (read side) ==="
# Real EGRET files (egret's own serializer output) read by powerio, re-emitted as
# PowerModels JSON, and checked against the matching MATPOWER case.
for base in case9 case14 case30; do
    echo "--- egret/$base ---"
    row="$(printf '%-26s' "egret/$base")"
    if convert "tests/data/egret/$base.json" powermodels-json "$TMP/egret_$base.json" 2>"$TMP/err"; then
        mark "${JL[@]}" benchmarks/validate_core.jl "tests/data/$base.m" "$TMP/egret_$base.json"
    else
        echo "  EGRET-read: convert failed"; cat "$TMP/err"; MARK="FAIL"; fails=$((fails + 1))
    fi
    row+="  EGRET-read:$MARK"
    rows+=("$row")
done

echo
echo "=== full reader x writer matrix (PowerModels + egret oracles) ==="
# Every source format -> every target, each output's core checked against the
# source's own core via an independent oracle; byte-exact on the diagonal. Real
# native files where they exist (PSS/E .raw, EGRET .json). See validate_matrix.py.
if [ "$HAVE_EGRET" -eq 0 ]; then
    echo "  skipped: egret not installed (pip install -r benchmarks/requirements.txt)"
    rows+=("$(printf '%-26s' 'matrix(5x5)')  all-cells:SKIP(egret)")
elif "$PY" benchmarks/validate_matrix.py; then
    rows+=("$(printf '%-26s' 'matrix(5x5)')  all-cells:ok")
else
    rows+=("$(printf '%-26s' 'matrix(5x5)')  all-cells:FAIL"); fails=$((fails + 1))
fi

echo
echo "=== summary ==="
for row in "${rows[@]}"; do echo "$row"; done
echo
if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) FAILED"
fi
exit "$((fails > 0 ? 1 : 0))"
