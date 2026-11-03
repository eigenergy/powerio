#!/usr/bin/env bash
# Leg D of the distribution CIM validation: schema conformance of PowerIO's
# cim-dist output and of the CIM sources, against the CIM-Graph profile
# classes. See README.md in this directory.
#
#   bash evals/validation/cim_dist/run_schema_check.sh [schema_check.py args]
#
# Environment:
#   POWERIO   the powerio CLI (default target/release/powerio, built if absent)
#   CIM_VENV  the validation virtualenv (default .venv-cim, created if absent)
#   CIM_DIST_CORPUS  the fetched corpus (default tests/data/large/cim_dist;
#             feeders not present are skipped)
#
# Writes evals/validation/cim_dist/RESULTS.md and exits nonzero when any case
# differs from schema_expected.json.

set -euo pipefail
cd "$(dirname "$0")/../../.."

venv="${CIM_VENV:-.venv-cim}"
if [ ! -d "$venv" ]; then
    python3 -m venv "$venv" 2>/dev/null || python -m venv "$venv"
fi
if [ -x "$venv/bin/python" ]; then
    py="$venv/bin/python"
else
    py="$venv/Scripts/python"
fi
"$py" -m pip install --quiet -r evals/validation/cim_dist/requirements-cim.txt

powerio="${POWERIO:-target/release/powerio}"
if [ -z "${POWERIO:-}" ] && [ ! -x "$powerio" ] && [ ! -x "$powerio.exe" ]; then
    cargo build --release -p powerio-cli
fi

"$py" evals/validation/cim_dist/schema_check.py \
    --powerio "$powerio" \
    --corpus "${CIM_DIST_CORPUS:-tests/data/large/cim_dist}" \
    --report evals/validation/cim_dist/RESULTS.md \
    "$@"
