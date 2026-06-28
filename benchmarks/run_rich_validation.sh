#!/usr/bin/env bash
# Rich data model validation tier.
#
# Strict committed fixtures fail this script. The PowerModels rich oracle needs
# Julia. Local corpora are opt in and reported under benchmarks/results without
# turning external data quirks into a release gate.
#
#   bash benchmarks/run_rich_validation.sh
#   bash benchmarks/run_rich_validation.sh --root /path/to/corpus --root /path/to/other/corpus

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
        echo "error: Python 3.11+ is required for the rich validation oracle stack" >&2
        echo "hint: python3.12 -m venv .venv && .venv/bin/python -m pip install -r benchmarks/requirements.txt" >&2
        exit 1
    fi
fi
JL=(julia --project=benchmarks)
OUT="${POWERIO_RICH_RESULTS_DIR:-benchmarks/results}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$OUT"

strict_fail=0
report_fail=0

run_strict() {
    echo "=== $1 ==="
    shift
    "$@" || strict_fail=$((strict_fail + 1))
}

run_report() {
    echo "=== $1 ==="
    shift
    "$@" || report_fail=$((report_fail + 1))
}

run_strict "powerio rich transmission tests" \
    cargo test -p powerio rich
run_strict "powerio-dist rich distribution tests" \
    cargo test -p powerio-dist rich
run_strict "powerio-matrix terminal admittance Ybus test" \
    cargo test -p powerio-matrix ybus_uses_asymmetric_terminal_admittance

cat >"$TMP/rich_powermodels.json" <<'JSON'
{
  "name": "rich_powermodels_oracle",
  "baseMVA": 100.0,
  "per_unit": true,
  "bus": {
    "1": {"index": 1, "bus_i": 1, "bus_type": 3, "vm": 1.0, "va": 0.0, "vmax": 1.1, "vmin": 0.9, "base_kv": 230.0},
    "2": {"index": 2, "bus_i": 2, "bus_type": 1, "vm": 1.0, "va": 0.0, "vmax": 1.1, "vmin": 0.9, "base_kv": 230.0}
  },
  "load": {
    "1": {"index": 1, "load_bus": 1, "pd": 0.10, "qd": 0.01, "status": 1},
    "2": {"index": 2, "load_bus": 1, "pd": 0.20, "qd": 0.02, "status": 1}
  },
  "branch": {
    "1": {
      "index": 1, "f_bus": 1, "t_bus": 2, "br_r": 0.01, "br_x": 0.10,
      "g_fr": 0.001, "b_fr": 0.02, "g_to": 0.003, "b_to": 0.04,
      "rate_a": 1.0, "rate_b": 1.1, "rate_c": 1.2,
      "c_rating_a": 500.0, "c_rating_b": 600.0, "c_rating_c": 700.0,
      "tap": 1.0, "shift": 0.0, "transformer": false, "br_status": 1,
      "angmin": -6.283185307179586, "angmax": 6.283185307179586,
      "pf": 0.125, "qf": 0.025, "pt": -0.120, "qt": -0.020
    }
  },
  "switch": {
    "1": {
      "index": 1, "f_bus": 1, "t_bus": 2, "state": 1,
      "thermal_rating": 0.75, "current_rating": 9.0,
      "pf": 0.01, "qf": 0.02, "pt": -0.01, "qt": -0.02
    }
  },
  "storage": {
    "1": {
      "index": 1, "storage_bus": 1, "ps": 0.5, "qs": 0.25,
      "energy": 1.0, "energy_rating": 6.0, "charge_rating": 3.0,
      "discharge_rating": 3.0, "charge_efficiency": 0.9,
      "discharge_efficiency": 0.91, "thermal_rating": 3.0,
      "current_rating": 4.2, "qmin": -1.0, "qmax": 1.0,
      "r": 0.0, "x": 0.0, "p_loss": 0.0, "q_loss": 0.0, "status": 1
    }
  },
  "dcline": {
    "1": {
      "index": 1, "f_bus": 1, "t_bus": 2, "br_status": 1,
      "pf": 0.30, "pt": -0.29, "qf": -0.03, "qt": 0.02,
      "vf": 1.0, "vt": 1.0, "pminf": 0.0, "pmaxf": 0.50,
      "pmint": -0.495, "pmaxt": 0.005, "mp_pmin": 0.0, "mp_pmax": 50.0,
      "qminf": -0.10, "qmaxf": 0.10, "qmint": -0.11, "qmaxt": 0.11,
      "loss0": 0.005, "loss1": 0.01,
      "model": 2, "startup": 0.0, "shutdown": 0.0, "ncost": 3,
      "cost": [200.0, 300.0, 10.0]
    }
  },
  "gen": {},
  "shunt": {}
}
JSON

if ! command -v julia >/dev/null 2>&1; then
    echo "error: julia is required for the PowerModels rich oracle" >&2
    echo "hint: julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'" >&2
    strict_fail=$((strict_fail + 1))
    printf 'rich_powermodels_oracle\tPMrich\tFAIL\n' >"$OUT/rich_oracle.tsv"
else
    echo "=== PowerModels rich oracle ==="
    : >"$TMP/results.tsv"
    if "${JL[@]}" benchmarks/validate_oracles.jl rich "$TMP" "$TMP/rich_powermodels.json"; then
        cp "$TMP/results.tsv" "$OUT/rich_oracle.tsv"
    else
        strict_fail=$((strict_fail + 1))
        cp "$TMP/results.tsv" "$OUT/rich_oracle.tsv" 2>/dev/null || true
    fi
fi

run_report "local rich corpus scan" \
    "$PY" benchmarks/rich_corpus.py --output-dir "$OUT" "$@"

if [ -n "${POWERIO_DIST_LOCAL_DSS_CORPUS:-}" ]; then
    echo "=== local distribution DSS corpus report ==="
    if cargo test -p powerio-dist local_dss_corpus_converts_to_valid_bmopf; then
        printf 'dist-local-dss\tbmopf\tok\n' >"$OUT/rich_dist_local.tsv"
    else
        printf 'dist-local-dss\tbmopf\tFAIL\n' >"$OUT/rich_dist_local.tsv"
        report_fail=$((report_fail + 1))
    fi
else
    printf 'dist-local-dss\tbmopf\tSKIP(POWERIO_DIST_LOCAL_DSS_CORPUS)\n' >"$OUT/rich_dist_local.tsv"
fi

echo
echo "=== rich validation summary ==="
echo "strict failures: $strict_fail"
echo "report-only failures: $report_fail"
echo "reports: $OUT/rich_oracle.tsv, $OUT/rich_corpus.tsv, $OUT/rich_corpus.json, $OUT/rich_dist_local.tsv"

if [ "$strict_fail" -eq 0 ]; then
    exit 0
fi
exit 1
