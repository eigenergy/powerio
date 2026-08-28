#!/usr/bin/env bash
# Value kind parity: the twenty stable kind identifiers must agree, in exact
# membership and spelling, across the Rust registry, the served stored
# schema, the Python surface, the guide, and (when POWERIO_JL names a
# checkout) the Julia wrapper's kind table.
set -euo pipefail
cd "$(dirname "$0")/.."

expected="ac_opf_instance ac_opf_solution ac_pf_instance ac_pf_solution ac_scuc_instance ac_scuc_solution balanced_network balanced_network_scenario_set balanced_network_time_series balanced_operating_point_time_series dc_opf_instance dc_opf_solution dc_pf_instance dc_pf_solution mc_ac_opf_instance mc_ac_opf_solution mc_ac_pf_instance mc_ac_pf_solution multiconductor_network multiconductor_operating_point_time_series"

# The as_str arms wrap long identifiers across lines, so membership is
# checked per kind and the arm count pins the total.
for kind in $expected; do
    grep -q "\"$kind\"" powerio/src/value.rs \
        || { echo "powerio/src/value.rs does not spell kind $kind" >&2; exit 1; }
done
arms=$(sed -n '/pub const fn as_str/,/^    }/p' powerio/src/value.rs | grep -c 'Self::')
if [ "$arms" != "20" ]; then
    echo "PioValueKind::as_str has $arms arms; the registry has 20" >&2
    exit 1
fi

schema=$(python3 - <<'PY'
import json
doc = json.load(open('docs/schema/pio-module/1/schema.json'))
branches = doc['$defs']['StoredValueV1']['oneOf']
kinds = sorted(b['properties']['kind']['const'] for b in branches)
print(' '.join(kinds))
PY
)
if [ "$schema" != "$expected" ]; then
    echo "docs/schema/pio-module/1/schema.json kinds disagree with the registry:" >&2
    diff <(printf '%s\n' $expected) <(printf '%s\n' $schema) >&2 || true
    exit 1
fi

for kind in $expected; do
    grep -q "\"$kind\"" python/powerio/__init__.py \
        || { echo "python/powerio/__init__.py does not name kind $kind" >&2; exit 1; }
done

# The guide's concepts page names both endpoints of each family range.
for token in balanced_network multiconductor_network dc_pf_instance ac_scuc_instance dc_pf_solution ac_scuc_solution; do
    grep -rq "$token" docs/src/concepts.md \
        || { echo "docs/src/concepts.md does not name $token" >&2; exit 1; }
done

if [ -n "${POWERIO_JL:-}" ] && [ -f "$POWERIO_JL/src/module.jl" ]; then
    for kind in $expected; do
        grep -q "\"$kind\"" "$POWERIO_JL/src/module.jl" \
            || { echo "PowerIO.jl module.jl does not map kind $kind" >&2; exit 1; }
    done
    echo "value kinds OK across Rust, schema, Python, docs, and Julia"
else
    echo "value kinds OK across Rust, schema, Python, and docs (POWERIO_JL unset: Julia skipped)"
fi
