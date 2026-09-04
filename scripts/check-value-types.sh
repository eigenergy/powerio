#!/usr/bin/env bash
# Structural type parity: the PowerIO IR schema exposes the complete set of
# canonical type names used by the dynamic Rust boundary.
set -euo pipefail
cd "$(dirname "$0")/.."

expected=$(python3 - <<'PY'
names = [
    "powerio.AcOpfInstance",
    "powerio.AcOpfSolution",
    "powerio.AcPfInstance",
    "powerio.AcPfSolution",
    "powerio.AcScucInstance",
    "powerio.AcScucSolution",
    "powerio.BalancedNetwork",
    "powerio.DcOpfInstance",
    "powerio.DcOpfSolution",
    "powerio.DcPfInstance",
    "powerio.DcPfSolution",
    "powerio.GeoLayer",
    "powerio.McAcOpfInstance",
    "powerio.McAcOpfSolution",
    "powerio.McAcPfInstance",
    "powerio.McAcPfSolution",
    "powerio.MulticonductorNetwork",
    "powerio.OperatingPoint<powerio.BalancedNetwork>",
    "powerio.OperatingPoint<powerio.MulticonductorNetwork>",
    "powerio.ScenarioSet<powerio.BalancedNetwork>",
    "powerio.ScenarioSet<powerio.MulticonductorNetwork>",
    "powerio.ScenarioSet<powerio.OperatingPoint<powerio.BalancedNetwork>>",
    "powerio.ScenarioSet<powerio.OperatingPoint<powerio.MulticonductorNetwork>>",
    "powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>",
    "powerio.ScenarioSet<powerio.TimeSeries<powerio.MulticonductorNetwork>>",
    "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>>",
    "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>>",
    "powerio.SocwrOpfSolution",
    "powerio.TimeSeries<powerio.BalancedNetwork>",
    "powerio.TimeSeries<powerio.MulticonductorNetwork>",
    "powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>",
    "powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>",
]
print("\n".join(sorted(names)))
PY
)

workspace_version=$(grep -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
schema=$(python3 - "docs/schema/pio-module/$workspace_version/schema.json" <<'PY'
import json
import sys
with open(sys.argv[1], encoding='utf-8') as handle:
    document = json.load(handle)
branches = document['$defs']['StoredValue']['oneOf']
names = sorted(branch['properties']['type']['const'] for branch in branches)
print("\n".join(names))
PY
)

if [ "$schema" != "$expected" ]; then
    echo "the PowerIO IR schema's structural types disagree with the Rust boundary" >&2
    diff <(printf '%s\n' "$expected") <(printf '%s\n' "$schema") >&2 || true
    exit 1
fi

grep -q 'pub enum PioValue' powerio/src/value.rs
grep -q 'powerio.TimeSeries<{element_type}>' powerio/src/value.rs
grep -q 'powerio.ScenarioSet<{element_type}>' powerio/src/value.rs

echo "PowerIO IR structural types match the Rust boundary"
