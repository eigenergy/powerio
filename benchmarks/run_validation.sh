#!/usr/bin/env bash
# Run every cross-tool correctness validator over every fixture and print a
# pass/fail matrix. Each MATPOWER fixture is checked against:
#
#   PMjson  — powerio's PowerModels JSON (writer) vs PowerModels.jl's own parse of
#             the .m, field by field.
#   PMread  — powerio's PowerModels JSON reader: PowerModels exports the .m to JSON,
#             powerio reads it and re-emits, the two are compared.
#   PSSE    — powerio's PSS/E .raw vs PowerModels.jl (counts + demand/gen totals).
#   Exa     — powerio (via C ABI) vs ExaPowerIO.jl, value for value.
#   pp      — powerio's parse + Y_bus vs pandapower (_m2ppc + makeYbus).
#   pp-json — powerio's pandapower JSON output imported by pandapower.
#   pypsa   — powerio's PyPSA CSV folder output imported by PyPSA.
#   OpenDSS — original micro distribution decks vs canonical DSS regeneration,
#             compared by solved node voltage magnitude.
#   BMOPF schema: distribution fixtures converted to canonical BMOPF JSON,
#                  then checked by Python jsonschema against the task force schema.
#
# Then the read sides and the full conversion matrix:
#   PSSE-read   — powerio reads a real PSS/E .raw, emits PowerModels JSON, compared
#                 against PowerModels.jl reading the same .raw.
#   egret-read  — powerio reads a real egret .json, emits PowerModels JSON, checked
#                 against the matching MATPOWER case.
#   matrix(5x5) — every legacy text reader -> every legacy text writer over the
#                 fixtures, each output's electrical core checked against the
#                 ground truth MATPOWER case.
#
# To keep the wall time down the work is staged so each heavy interpreter starts
# once, not once per case: PowerModels exports all references (one Julia process),
# powerio runs all conversions (one Python process), PowerModels + ExaPowerIO run
# every comparison (one Julia process), pandapower and PyPSA run their converter
# imports in batched Python processes, and the 5x5 matrix runs its own batched
# process. Each leg appends a `<case>\t<leg>\t<mark>` line to results.tsv,
# rendered into the matrix below.
#
# Prereqs: `cargo build --release -p powerio-capi`, the powerio Python extension
# built into the Python oracle venv (`.venv` by default), the Julia env
# instantiated (`julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'`),
# and the Python oracle tools (`.venv/bin/python -m pip install -r
# benchmarks/requirements.txt`). All oracle tools are benchmark scoped, not
# powerio deps.
#
#   bash benchmarks/run_validation.sh
#
# Exits nonzero if any check fails.

set -uo pipefail
cd "$(dirname "$0")/.."

export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$PWD/.cache}"
export MPLCONFIGDIR="${MPLCONFIGDIR:-$XDG_CACHE_HOME/matplotlib}"
mkdir -p "$MPLCONFIGDIR" "$XDG_CACHE_HOME/fontconfig"

if [ -n "${PYTHON:-}" ]; then
    PY="$PYTHON"
else
    PY=""
    for candidate in .venv/bin/python .venv-validate/bin/python .venv312/bin/python python3; do
        if command -v "$candidate" >/dev/null 2>&1 &&
            "$candidate" -c 'import sys; raise SystemExit(sys.version_info < (3, 11))' >/dev/null 2>&1; then
            PY="$candidate"
            break
        fi
    done
    if [ -z "$PY" ]; then
        echo "error: Python 3.11+ is required for the validation oracle stack" >&2
        echo "hint: python3.12 -m venv .venv && .venv/bin/python -m pip install -r benchmarks/requirements.txt" >&2
        exit 1
    fi
fi
JL=(julia --project=benchmarks)
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export PIO_RESULTS_TSV="$TMP/results.tsv"
: > "$PIO_RESULTS_TSV"

if ! "$PY" -c "import egret, jsonschema, opendssdirect, pandapower, pypsa" >/dev/null 2>&1; then
    echo "error: validation oracle imports failed for $PY" >&2
    echo "hint: .venv/bin/python -m pip install -r benchmarks/requirements.txt" >&2
    "$PY" -c "import egret, jsonschema, opendssdirect, pandapower, pypsa"
    exit 1
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
EGCASES=(tests/data/egret/case9.json tests/data/egret/case14.json tests/data/egret/case30.json)

phase_fail=0
run() { # <label> <command...>
    echo "=== $1 ==="
    shift
    "$@" || phase_fail=$((phase_fail + 1))
}

# 1. PowerModels exports each .m to a per_unit reference JSON (one Julia process).
#    Must precede the conversions: the PMread leg re-emits these references.
run "PowerModels reference export (PMread)" \
    "${JL[@]}" benchmarks/validate_oracles.jl export "$TMP" "${MCASES[@]}"

# 2. powerio runs every conversion the comparisons read (one Python process).
run "powerio conversions" \
    "$PY" benchmarks/convert_cases.py "$TMP" --m "${MCASES[@]}" --raw "${RAWCASES[@]}" --egret "${EGCASES[@]}"

# 3. PowerModels + ExaPowerIO run every comparison leg (one Julia process). A
#    nonzero exit here means a leg failed, which the results.tsv tally below counts;
#    it is not a phase crash, so don't double-count it.
echo "=== PowerModels + ExaPowerIO comparisons ==="
"${JL[@]}" benchmarks/validate_oracles.jl compare "$TMP" \
    --m "${MCASES[@]}" --raw "${RAWCASES[@]}" --egret "${EGCASES[@]}" || true

# 4. pandapower parse + Y_bus over every case (one Python process; n/a where its
#    reader can't parse the case). Nonzero exit == a real mismatch, counted below.
# 4b. pandapower JSON converter output loaded by pandapower itself.
echo "=== pandapower (parse + Y_bus) ==="
"$PY" benchmarks/validate_pandapower.py "${MCASES[@]}" || true
echo "=== pandapower JSON converter ==="
"$PY" benchmarks/validate_pandapower_converter.py "${MCASES[@]}" || true

# 4c. PyPSA CSV converter output loaded by PyPSA itself.
echo "=== PyPSA CSV converter ==="
"$PY" benchmarks/validate_pypsa.py "${MCASES[@]}" || true

# 4d. OpenDSS solves over the distribution micro fixtures.
echo "=== OpenDSS distribution solve oracle ==="
"$PY" benchmarks/validate_opendss.py || true

# 4e. External schema validation of emitted BMOPF JSON.
echo "=== BMOPF schema validation ==="
"$PY" benchmarks/validate_bmopf_schema.py || true

# 5. Full reader x writer matrix (its own batched process).
echo "=== full reader x writer matrix (PowerModels + egret oracles) ==="
if "$PY" benchmarks/validate_matrix.py; then
    printf 'matrix(5x5)\tall-cells\tok\n' >>"$PIO_RESULTS_TSV"
else
    printf 'matrix(5x5)\tall-cells\tFAIL\n' >>"$PIO_RESULTS_TSV"
fi

# --- summary --------------------------------------------------------------
echo
echo "=== summary ==="
awk -F'\t' '
    !($1 in seen) { order[++n] = $1; seen[$1] = 1 }
    { row[$1] = row[$1] sprintf("  %s:%s", $2, $3) }
    END { for (i = 1; i <= n; i++) printf "%-26s%s\n", order[i], row[order[i]] }
' "$PIO_RESULTS_TSV"

# A FAIL mark is a real discrepancy; n/a and SKIP are not. A short results file
# (fewer rows than legs run) means a phase crashed before recording — fail loudly.
mark_fails=$(awk -F'\t' '$3 == "FAIL" { c++ } END { print c + 0 }' "$PIO_RESULTS_TSV")
# 7 legs per .m case (PMjson, PMread, PSSE, Exa, pp, pp-json, pypsa)
# + 1 per raw + 1 per egret + every OpenDSS micro fixture
# + every emitted BMOPF schema fixture + 1 matrix.
opendss_expected=$("$PY" benchmarks/validate_opendss.py --count)
bmopf_schema_expected=$("$PY" benchmarks/validate_bmopf_schema.py --count)
expected=$((${#MCASES[@]} * 7 + ${#RAWCASES[@]} + ${#EGCASES[@]} + opendss_expected + bmopf_schema_expected + 1))
got=$(wc -l <"$PIO_RESULTS_TSV")
short=0
[ "$got" -lt "$expected" ] && short=1

echo
if [ "$mark_fails" -eq 0 ] && [ "$phase_fail" -eq 0 ] && [ "$short" -eq 0 ]; then
    echo "all checks passed"
    exit 0
fi
[ "$short" -eq 1 ] && echo "incomplete: recorded $got of $expected legs (a phase crashed)"
[ "$phase_fail" -ne 0 ] && echo "$phase_fail phase(s) failed to run"
[ "$mark_fails" -ne 0 ] && echo "$mark_fails check(s) FAILED"
exit 1
