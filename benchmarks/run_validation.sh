#!/usr/bin/env bash
# Run every cross-tool correctness validator over every fixture and print a
# pass/fail matrix. Each MATPOWER fixture is checked against:
#
#   PMjson  — caseio's PowerModels JSON vs PowerModels.jl's own parse (field by
#             field, after make_per_unit!).        benchmarks/validate_powermodels.jl
#   PSSE    — caseio's PSS/E .raw vs PowerModels.jl (counts + demand/gen totals).
#                                                   benchmarks/validate_psse.jl
#   Exa     — caseio (via C ABI) vs ExaPowerIO.jl, value for value.
#                                                   benchmarks/validate_exapowerio.jl
#   pp      — caseio's parse + Y_bus vs pandapower (_m2ppc + makeYbus).
#                                                   benchmarks/validate_pandapower.py
#
# PSS/E .raw fixtures are checked on the read side only (caseio reads the .raw,
# emits PowerModels JSON, compared against PowerModels.jl reading the same .raw).
#
# Prereqs: `cargo build --release -p caseio-capi`, the casemat/caseio Python
# extensions built into .venv (`maturin develop --release`), and the Julia env
# instantiated (`julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'`).
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

MCASES=(
    tests/data/case9.m
    tests/data/case14.m
    tests/data/case30.m
    tests/data/case57.m
    tests/data/case118.m
    tests/data/t_case9_dcline.m
    tests/data/pglib/pglib_opf_case5_pjm.m
    tests/data/pglib/pglib_opf_case14_ieee.m
    tests/data/case2869pegase.m
)
RAWCASES=(tests/data/psse/case5.raw tests/data/psse/case14.raw)

fails=0
rows=()

# Convert a case to another format via the casemat Python package (no CLI build).
convert() { # <in> <to> <out>
    "$PY" - "$@" <<'EOF'
import sys, warnings
warnings.filterwarnings("ignore")
import casemat
inp, to, out = sys.argv[1], sys.argv[2], sys.argv[3]
open(out, "w").write(casemat.convert(inp, to).text)
EOF
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
echo "=== summary ==="
for row in "${rows[@]}"; do echo "$row"; done
echo
if [ "$fails" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) FAILED"
fi
exit "$((fails > 0 ? 1 : 0))"
