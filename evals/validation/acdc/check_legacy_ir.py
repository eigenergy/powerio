"""Run with a frozen PowerIO wheel: prove capability refusal, including nesting."""
import copy
import io
import json
from pathlib import Path
import powerio

ROOT = Path(__file__).resolve().parents[3]
m = powerio.parse(ROOT / 'tests/data/case9.m')
doc = json.loads(powerio.serialize(m).text)
n = doc['value']['data']
t = [{'label': 't0'}]
ts = {'time_points': t, 'values': [n]}
def sc(value):
    return {'scenarios': [{'id': 's0', 'value': value}]}

records = {
    'powerio.BalancedNetwork': n,
    'powerio.OperatingPoint<powerio.BalancedNetwork>': {'network': n, 'quantities': {}},
    'powerio.TimeSeries<powerio.BalancedNetwork>': ts,
    'powerio.ScenarioSet<powerio.BalancedNetwork>': sc(n),
    'powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>': sc(ts),
    'powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>>': sc({'network': n, 'time_points': t, 'values': [{'quantities': {}}]}),
    'powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>': {'network': n, 'time_points': t, 'values': [{'quantities': {}}]},
    'powerio.ScenarioSet<powerio.OperatingPoint<powerio.BalancedNetwork>>': {'network': n, 'scenarios': [{'id': 's0', 'quantities': {}}]},
}
def inject(value):
    if isinstance(value, dict):
        if 'base_mva' in value and 'buses' in value:
            value['ac_dc_equivalent'] = {'probe': 'must not disappear'}
        else:
            for child in value.values():
                inject(child)
    elif isinstance(value, list):
        for child in value:
            inject(child)

for kind, data in records.items():
    ordinary = copy.deepcopy(doc)
    ordinary['value'] = {'type': kind, 'data': copy.deepcopy(data)}
    powerio.deserialize(io.StringIO(json.dumps(ordinary)))
    extended = copy.deepcopy(ordinary)
    inject(extended['value'])
    decoded = powerio.deserialize(io.StringIO(json.dumps(extended)))
    assert 'ac_dc_equivalent' not in powerio.serialize(decoded).text, kind
    extended['required_capabilities'] = ['balanced-acdc-equivalent-v1']
    try:
        powerio.deserialize(io.StringIO(json.dumps(extended)))
    except powerio.PowerIOError as error:
        assert 'required_capabilities' in str(error) and 'unknown field' in str(error), error
    else:
        raise AssertionError(f'{kind}: old reader accepted extended document')
print(f'{powerio.__version__}: {len(records)} ordinary containers accepted; unguarded field loss reproduced; all capability-guarded containers rejected')

# New coupled structural types must also refuse without the capability marker.
for kind, data in {
    'powerio.AcDcOpfInstance': {'network': n, 'profile': 'power_models_ac_dc0101_acp_vsc_v1'},
    'powerio.AcDcOpfSolution': {'instance': {'network': n, 'profile': 'power_models_ac_dc0101_acp_vsc_v1'}, 'point': [], 'objective': 0.0, 'termination': {'kind': 'converged'}, 'producer': None},
}.items():
    for guarded in [False, True]:
        candidate = copy.deepcopy(doc)
        candidate['value'] = {'type': kind, 'data': data}
        if guarded:
            candidate['required_capabilities'] = ['balanced-acdc-equivalent-v1']
        try:
            powerio.deserialize(io.StringIO(json.dumps(candidate)))
        except powerio.PowerIOError as error:
            assert 'unknown variant' in str(error) or (guarded and 'required_capabilities' in str(error)), error
        else:
            raise AssertionError(f'{kind}: frozen reader accepted coupled intent/result')
print('Coupled instance and result rejected with and without the capability marker')
