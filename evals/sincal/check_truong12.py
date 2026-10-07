#!/usr/bin/env python3
"""Validate the externally held native 12-bus SINCAL case against OpenDSS.

The original database has no published redistribution license and is never a
repository fixture. Run the public Rust example first; supply its JSON output.
The typed circuit is solved independently, while OpenDSS uses only native inputs.
"""
import argparse
import hashlib
import cmath
import json
import sqlite3
from pathlib import Path
import numpy as np
from opendssdirect import dss
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('source', type=Path)
parser.add_argument('network', type=Path)
parser.add_argument('report', type=Path)
args = parser.parse_args()
source_hash = hashlib.sha256(args.source.read_bytes()).hexdigest()
if source_hash != 'fda71fb98e45f118dcea90b61c78db855c822f9b2b8d594bf05bf101ca9dff9a':
    raise ValueError('native source identity mismatch')
n = json.loads(args.network.read_text())
c = sqlite3.connect(args.source.resolve().as_uri() + '?mode=ro', uri=True)
c.row_factory = sqlite3.Row
t = {k: [dict(r) for r in c.execute('SELECT * FROM ' + k + ' WHERE Variant_ID=1')] for k in ['Node', 'Element', 'Terminal', 'Line', 'Load', 'Infeeder', 'VoltageLevel', 'ULFNodeResult']}
expected = {'Line': 11, 'Load': 33, 'Infeeder': 1}
for kind, key in [('Line', 'lines'), ('Load', 'loads'), ('Infeeder', 'sources')]:
    ids = {str(row['Element_ID']) for row in t['Element'] if row['Type'] == kind}
    if len(ids) != expected[kind] or ids != {row['name'] for row in n[key]}:
        raise ValueError(f'incomplete {kind} accounting')
if len(t['Node']) != 12 or len(t['ULFNodeResult']) != 12:
    raise ValueError('incomplete native nodes/results')
parents = {}

def find(x):
    parents.setdefault(x, x)
    if parents[x] != x:
        parents[x] = find(parents[x])
    return parents[x]
for s in n['switches']:
    if not s['open']:
        for a, b in zip(s['terminal_map_from'], s['terminal_map_to']):
            parents[find((s['bus_from'], a))] = find((s['bus_to'], b))
ground = {find((b['id'], t)) for b in n['buses'] for t in b['grounded']}
index = {}
for b in n['buses']:
    for p in b['terminals']:
        x = find((b['id'], p))
        if x not in ground:
            index.setdefault(x, len(index))

def at(b, p):
    return index.get(find((b, p)))
size = len(index)
y = np.zeros((size, size), complex)
code = {x['name']: x for x in n['linecodes']}
line_primitives = {}
for l in n['lines']:
    d = code[l['linecode']]
    z = (np.array(d['r_series']) + 1j * np.array(d['x_series'])) * l['length']
    v = np.linalg.inv(z)
    ports = [at(l['bus_' + end], p) for end in ['from', 'to'] for p in l['terminal_map_' + end]]
    primitive = np.block([[v, -v], [-v, v]])
    for end in range(2):
        side = ['from', 'to'][end]
        sh = (np.array(d['g_' + side]) + 1j * np.array(d['b_' + side])) * l['length']
        primitive[end * 3:end * 3 + 3, end * 3:end * 3 + 3] += sh
    line_primitives[l['name']] = (ports, primitive)
    for i, a in enumerate(ports):
        for j, b in enumerate(ports):
            if a is not None and b is not None:
                y[a, b] += primitive[i, j]
assert not n['shunts'] and (not n['transformers']) and (not n['generators']) and (not n['untyped'])
boundaries = []
for s in n['sources']:
    for p, v, a in zip(s['terminal_map'], s['v_magnitude'], s['v_angle']):
        boundaries.append((at(s['bus'], p), cmath.rect(v, a)))
C = np.zeros((len(boundaries), size), complex)
for i, (idx, v) in enumerate(boundaries):
    C[i, idx] = 1
mat = np.block([[y, C.T], [C, np.zeros((len(C), len(C)), complex)]])
rhs = np.r_[np.zeros(size, complex), [v for i, v in boundaries]]
loads = []
for l in n['loads']:
    assert l['configuration'] == 'single_phase' and l['voltage_model']['model'] == 'constant_power'
    a, b = (at(l['bus'], p) for p in l['terminal_map'])
    loads.append((a, b, complex(l['p_nom'][0], l['q_nom'][0])))

def calc(v):
    r = rhs.copy()
    for a, b, s in loads:
        u = (0 if a is None else v[a]) - (0 if b is None else v[b])
        i = (s / u).conjugate()
        if a is not None:
            r[a] -= i
        if b is not None:
            r[b] += i
    return r
volts = np.linalg.solve(mat, rhs)
for i in range(1000):
    new = np.linalg.solve(mat, calc(volts))
    err = max(abs(new[:size] - volts[:size]))
    volts = new
    if err < 1e-09:
        break
else:
    raise RuntimeError('no convergence')
residual = float(max(abs(mat @ volts - calc(volts))))
iterations = i + 1
if residual > 1e-06:
    raise ValueError('typed KCL residual exceeded')
ports = {}
for p in t['Terminal']:
    ports.setdefault(p['Element_ID'], []).append(p)
for ps in ports.values():
    ps.sort(key=lambda p: p['TerminalNo'])
s = t['Infeeder'][0]
nom = t['VoltageLevel'][0]['Un']
dss.Text.Command('clear')
dss.Text.Command('set defaultbasefrequency=50')
dss.Text.Command(f"new circuit.check phases=3 bus1=n1.1.2.3 basekv={nom} pu={s['u'] / 100} angle=0 frequency=50")
dss.Text.Command('edit vsource.source r1=1e-8 x1=1e-8 r0=1e-8 x0=1e-8')
for l in t['Line']:
    a, b = ports[l['Element_ID']]
    assert a['Flag_Terminal'] == b['Flag_Terminal'] == 7
    dss.Text.Command(f"new line.l{l['Element_ID']} bus1=n{a['Node_ID']}.1.2.3 bus2=n{b['Node_ID']}.1.2.3 phases=3 length={l['l']} units=km r1={l['r']} x1={l['x']} r0={l['r']} x0={l['x']} c1={l['c']} c0={l['c']}")
for l in t['Load']:
    p = ports[l['Element_ID']][0]
    assert p['Flag_Terminal'] in [1, 2, 3]
    dss.Text.Command(f"new load.l{l['Element_ID']} bus1=n{p['Node_ID']}.{p['Flag_Terminal']}.0 phases=1 conn=wye model=1 kv={nom / np.sqrt(3)} kw={l['P'] * l['fP'] * 1000} kvar={l['Q'] * l['fQ'] * 1000} vminpu=.1 vmaxpu=2")
dss.Text.Command('set maxiterations=1000 tolerance=1e-12 controlmode=off')
dss.Text.Command('solve')
assert dss.Solution.Converged()
errors = []
historical = []
for row in t['ULFNodeResult']:
    node = row['Node_ID']
    dss.Circuit.SetActiveBus(f'n{node}')
    vals = np.array(dss.Bus.Voltages())
    dd = dict(zip(dss.Bus.Nodes(), vals[::2] + 1j * vals[1::2]))
    for p in [1, 2, 3]:
        v = volts[at(str(node), str(p))]
        errors.append(abs(v - dd[p]))
        historical.append(abs(v - cmath.rect(row['U' + str(p)] * 1000, np.deg2rad(row['phi' + str(p)] + 30))))
if max(errors) > 0.001:
    raise ValueError(f'independent voltage disagreement: {max(errors)} V')
# Native VDN=.01% sets the historical voltage stopping criterion. This is
# secondary evidence: stored results do not attest the current input revision.
historical_tolerance = nom * 1000 / np.sqrt(3) * 0.0001
if max(historical) > historical_tolerance:
    raise ValueError('historical voltage disagreement')
current_errors = []
power_errors = []
for name, (line_ports, primitive) in line_primitives.items():
    terminal_voltage = np.array([0 if idx is None else volts[idx] for idx in line_ports])
    terminal_current = primitive @ terminal_voltage
    dss.Circuit.SetActiveElement("line.l" + name)
    raw_current = np.array(dss.CktElement.Currents())
    reference_current = raw_current[::2] + 1j * raw_current[1::2]
    raw_power = np.array(dss.CktElement.Powers())
    reference_power = (raw_power[::2] + 1j * raw_power[1::2]) * 1000
    current_errors.extend(abs(terminal_current - reference_current))
    power_errors.extend(abs(terminal_voltage * terminal_current.conjugate() - reference_power))
if max(current_errors) > 0.001 or max(power_errors) > 0.1:
    raise ValueError("independent terminal current/power disagreement")
phase_power = [sum(complex(l['P'] * l['fP'], l['Q'] * l['fQ']) * 1e6
                   for l in t['Load'] if ports[l['Element_ID']][0]['Flag_Terminal'] == p)
               for p in (1, 2, 3)]
if len(set(phase_power)) != 3:
    raise ValueError("native loads no longer have unequal phase totals")
report = {
    'source_repository': 'https://github.com/Truong812001/Unbalance-Power-Flow',
    'revision': '1459d2be39b3c00aac195b1d6c01bcde16ee357f',
    'source_path': 'Update_finalV1_database/12bus/12busbc_files/database.db',
    'source_sha256': source_hash,
    'source_bytes': args.source.stat().st_size,
    'license': 'No license found; external research only, no redistribution',
    'schema': 15.0,
    'variant': 1,
    'native_nodes': 12,
    'native_elements': 45,
    'single_phase_loads': 33,
    'lines': 11,
    'sources': 1,
    'compared_phase_voltages': len(errors),
    'maximum_opendss_voltage_error_v': float(max(errors)),
    'opendss_voltage_tolerance_v': 0.001,
    'maximum_historical_voltage_error_v': float(max(historical)),
    'historical_tolerance_v': float(historical_tolerance),
    'historical_angle_alignment': 'Fixed +30 degrees: stored phase-1 is -30 degrees while the typed source uses zero; all relative phase angles are retained.',
    'historical_results_attest_current_inputs': False,
    'typed_kcl_residual_a': residual,
    'typed_iterations': iterations,
    'opendss_source_sequence_r_x_ohm': 1e-08,
    'opendss_version': dss.Basic.Version(),
    'case_size_disposition': 'New authentic asymmetric success; smaller than requested larger feeders, not counted as completion of that gate.',
    'native_sincal_executed': False,
    'compared_line_terminal_currents': len(current_errors),
    'maximum_opendss_current_error_a': float(max(current_errors)),
    'opendss_current_tolerance_a': 0.001,
    'maximum_opendss_power_error_va': float(max(power_errors)),
    'opendss_power_tolerance_va': 0.1,
    'native_phase_load_totals_w_var': [[s.real, s.imag] for s in phase_power],
    'passed': True,
}

args.report.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
