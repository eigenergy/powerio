"""Validate actual Rust balanced-reader output against the publisher's paired CSV.

Requires numpy and pandapower==3.2.2. Builds the reference network solely from
CSV input (not stored results or the Rust parameters), runs a fresh pandapower
pi-transformer calculation, and independently solves the Rust network's nodal
constant-power equations. Historical result agreement is reported separately.
"""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sqlite3
import tempfile

from inspect_native import read_native

import numpy as np
import pandapower as pp

FIXTURES = Path(__file__).resolve().parents[2] / 'tests/data/sincal'


def rows(name):
    with (FIXTURES / 'simbench-csv' / name).open(encoding='utf-8-sig', newline='') as f:
        return list(csv.DictReader(f, delimiter=';'))


def close(actual, expected, label, atol=1e-12):
    if not math.isclose(actual, float(expected), abs_tol=atol, rel_tol=1e-9):
        raise AssertionError(f'{label}: {actual} != {expected}')


def reference(tap_side=None, tap_position=None):
    net = pp.create_empty_network(f_hz=50.0, sn_mva=100.0)
    buses = {r['id']: pp.create_bus(net, vn_kv=float(r['vmR']), name=r['id'])
             for r in rows('Node.csv')}
    parent = {name: name for name in buses}

    def root(name):
        while parent[name] != name:
            name = parent[name]
        return name

    for r in rows('Switch.csv'):
        pp.create_switch(net, buses[r['nodeA']], buses[r['nodeB']], et='b', closed=r['cond'] == '1')
        if r['cond'] == '1':
            parent[root(r['nodeB'])] = root(r['nodeA'])
    nodes = {r['id']: r for r in rows('Node.csv')}
    for r in rows('ExternalNet.csv'):
        assert r['calc_type'] == 'vavm'
        node = nodes[r['node']]
        pp.create_ext_grid(net, buses[r['node']], vm_pu=float(node['vmSetp']), va_degree=float(node['vaSetp']))
    for r in rows('Load.csv'):
        pp.create_load(net, buses[r['node']], p_mw=float(r['pLoad']), q_mvar=float(r['qLoad']))
    for r in rows('RES.csv'):
        assert r['calc_type'] == 'pq'
        pp.create_sgen(net, buses[r['node']], p_mw=float(r['pRES']), q_mvar=float(r['qRES']))
    line_types = {r['id']: r for r in rows('LineType.csv')}
    for r in rows('Line.csv'):
        t = line_types[r['type']]
        pp.create_line_from_parameters(net, buses[r['nodeA']], buses[r['nodeB']],
            length_km=float(r['length']), r_ohm_per_km=float(t['r']), x_ohm_per_km=float(t['x']),
            c_nf_per_km=float(t['b']) * 1e3 / (2 * math.pi * 50), max_i_ka=float(t['iMax']) / 1000)
    tr_types = {r['id']: r for r in rows('TransformerType.csv')}
    for r in rows('Transformer.csv'):
        t = tr_types[r['type']]
        assert r['autoTap'] == '0'
        pp.create_transformer_from_parameters(net, buses[r['nodeHV']], buses[r['nodeLV']],
            sn_mva=float(t['sR']), vn_hv_kv=float(t['vmHV']), vn_lv_kv=float(t['vmLV']),
            vk_percent=float(t['vmImp']), vkr_percent=float(t['pCu']) / (float(t['sR']) * 10),
            pfe_kw=float(t['pFe']), i0_percent=float(t['iNoLoad']), shift_degree=float(t['va0']),
            tap_side=tap_side or t['tapside'].lower(), tap_neutral=float(t['tapNeutr']),
            tap_pos=float(r['tappos']) if tap_position is None else tap_position, tap_step_percent=float(t['dVm']),
            tap_step_degree=float(t['dVa']), tap_changer_type='Ratio')
    pp.runpp(net, trafo_model='pi', calculate_voltage_angles=True, tolerance_mva=1e-10, numba=False)
    assert net.converged
    return net, buses, root


def check_parameters(model, root):
    buses = {r['id']: r for r in model['buses']}
    nodes = {r['id']: r for r in rows('Node.csv')}
    for b in buses.values():
        close(b['base_kv'], nodes[b['name']]['vmR'], f"bus {b['id']} nominal kV")
    def terminal(bus, name):
        assert root(buses[bus]['name']) == root(name), (bus, name)
    for file, key, p, q in [('Load.csv', 'loads', 'pLoad', 'qLoad'), ('RES.csv', 'generators', 'pRES', 'qRES')]:
        actual = [r for r in model[key] if key == 'loads' or not r['voltage_regulation_on']]
        expected = rows(file)
        assert len(actual) == len(expected)
        # This case has one load and one converter at most at each node.
        expected = {root(r['node']): r for r in expected}
        for r in actual:
            e = expected.pop(root(buses[r['bus']]['name']))
            close(r['p' if key == 'loads' else 'pg'], e[p], f'{key} active power')
            close(r['q' if key == 'loads' else 'qg'], e[q], f'{key} reactive power')
        assert not expected
    lines = {r['id']: r for r in rows('Line.csv')}
    types = {r['id']: r for r in rows('LineType.csv')}
    transformers = {r['id']: r for r in rows('Transformer.csv')}
    tr_types = {r['id']: r for r in rows('TransformerType.csv')}
    for b in model['branches']:
        if b['name'] in lines:
            e = lines.pop(b['name'])
            t = types[e['type']]
            zbase = buses[b['from']]['base_kv'] ** 2 / model['base_mva']
            terminal(b['from'], e['nodeA']); terminal(b['to'], e['nodeB'])
            close(b['r'] * zbase, float(t['r']) * float(e['length']), 'line R')
            close(b['x'] * zbase, float(t['x']) * float(e['length']), 'line X')
            close(b['b'] / zbase, float(t['b']) * 1e-6 * float(e['length']), 'line charging')
            close(b['current_ratings']['c_rating_a'], t['iMax'], 'line current rating')
        else:
            e = transformers.pop(b['name']); t = tr_types[e['type']]
            terminal(b['from'], e['nodeHV']); terminal(b['to'], e['nodeLV'])
            scale = model['base_mva'] / float(t['sR'])
            close(math.hypot(b['r'], b['x']) / scale * 100, t['vmImp'], 'transformer uk')
            close(b['r'] / scale * float(t['sR']) * 1000, t['pCu'], 'transformer copper loss')
            close(2 * b['charging']['g_fr'] * model['base_mva'] * 1000, t['pFe'], 'transformer core loss')
            close(2 * math.hypot(b['charging']['g_fr'], b['charging']['b_fr']) * model['base_mva'] / float(t['sR']) * 100, t['iNoLoad'], 'transformer exciting current')
            close(b['shift'], t['va0'], 'transformer phase shift')
    assert not lines and not transformers
    assert len(model['buses']) == 15
    assert len(model['loads']) + len(model['generators']) + len(model['branches']) == 32


def solve_mapped(model):
    """Nodal current-injection iteration, independent of PowerIO matrix code."""
    index = {b['id']: i for i, b in enumerate(model['buses'])}
    n = len(index)
    ybus = np.zeros((n, n), dtype=complex)
    assert not model['switches']
    for b in model['branches']:
        if not b['in_service']:
            continue
        i, j = index[b['from']], index[b['to']]
        y = 1 / complex(b['r'], b['x'])
        t = (b['tap'] or 1.0) * np.exp(1j * np.deg2rad(b['shift']))
        c = b['charging'] or dict(g_fr=0, g_to=0, b_fr=b['b']/2, b_to=b['b']/2)
        ybus[i,i] += (y + complex(c['g_fr'],c['b_fr'])) / abs(t)**2
        ybus[j,j] += y + complex(c['g_to'],c['b_to'])
        ybus[i,j] -= y / t.conjugate()
        ybus[j,i] -= y / t
    s = np.zeros(n, dtype=complex)
    for load in model['loads']:
        assert load['voltage_model'] is None
        if load['in_service']:
            s[index[load['bus']]] -= complex(load['p'], load['q']) / model['base_mva']
    for gen in model['generators']:
        if gen['in_service'] and not gen['voltage_regulation_on']:
            s[index[gen['bus']]] += complex(gen['pg'], gen['qg']) / model['base_mva']
    slack = [i for i, b in enumerate(model['buses']) if b['kind'] == 'REF']
    assert len(slack) == 1, slack
    free = [i for i in range(n) if i not in slack]
    b = model['buses'][slack[0]]
    v = np.ones(n, dtype=complex)
    v[slack] = b['vm'] * np.exp(1j * np.deg2rad(b['va']))
    # The no-load network solution initializes large phase shifts correctly.
    yll, yls = ybus[np.ix_(free,free)], ybus[np.ix_(free,slack)]
    v[free] = np.linalg.solve(yll, -yls @ v[slack])
    for iteration in range(1000):
        v[free] = np.linalg.solve(yll, np.conj(s[free] / v[free]) - yls @ v[slack])
        mismatch = float(np.max(np.abs(v[free] * np.conj((ybus @ v)[free]) - s[free])))
        if mismatch < 1e-13:
            return v, iteration + 1, mismatch
    raise AssertionError(f'mapped load flow failed: mismatch={mismatch}')


def check(inspector):
    completed = subprocess.run([str(inspector), str(FIXTURES / '1-LV-rural1--0-sw.sinx')], check=True, capture_output=True, text=True)
    model = json.loads(completed.stdout)
    net, buses, root = reference()
    check_parameters(model, root)
    actual, iterations, mismatch = solve_mapped(model)
    expected = np.array([net.res_bus.at[buses[b['name']], 'vm_pu'] * np.exp(1j*np.deg2rad(net.res_bus.at[buses[b['name']], 'va_degree'])) for b in model['buses']])
    error = float(np.max(np.abs(actual - expected)))
    assert error < 2e-8, error
    tap_checks = []
    _, _, native_bytes, _ = read_native(FIXTURES / '1-LV-rural1--0-sw.sinx')
    for side, flag in [('hv', 1), ('lv', 2)]:
        with tempfile.TemporaryDirectory(prefix='sincal-balanced-tap-') as directory:
            path = Path(directory) / 'tap.db'
            path.write_bytes(native_bytes)
            with sqlite3.connect(path) as connection:
                connection.execute('UPDATE TwoWindingTransformer SET Flag_ConNode=?, roh=2', [flag])
            mapped = subprocess.run([str(inspector), str(path)], check=True, capture_output=True, text=True)
            tap_model = json.loads(mapped.stdout)
            tap_reference, tap_buses, _ = reference(tap_side=side, tap_position=2)
            tap_voltage, _, _ = solve_mapped(tap_model)
            expected_tap = np.array([tap_reference.res_bus.at[tap_buses[b['name']], 'vm_pu'] *
                np.exp(1j*np.deg2rad(tap_reference.res_bus.at[tap_buses[b['name']], 'va_degree']))
                for b in tap_model['buses']])
            tap_error = float(np.max(np.abs(tap_voltage - expected_tap)))
            assert tap_error < 2e-8, (side, tap_error)
            tap_checks.append({'side': side, 'position': 2, 'fresh_voltage_complex_error_pu': tap_error})
    historical = {r['node']: r for r in rows('NodePFResult.csv')}
    historical_vm = max(abs(abs(v) - float(historical[b['name']]['vm'])) for b, v in zip(model['buses'],actual))
    return {
        'case': '1-LV-rural1--0-sw',
        'source_sha256': hashlib.sha256((FIXTURES/'1-LV-rural1--0-sw.sinx').read_bytes()).hexdigest(),
        'profile': model['profile'], 'schema': model['schema'], 'variant': model['variant'],
        'mapped_nodes': 15, 'mapped_elements': 32, 'paired_csv_parameters': 'passed',
        'pandapower_version': pp.__version__, 'numpy_version': np.__version__,
        'fresh_voltage_complex_error_pu': error, 'tolerance_pu': 2e-8,
        'current_injection_iterations': iterations, 'power_mismatch_pu': mismatch,
        'historical_rounded_csv_voltage_magnitude_error_pu': historical_vm,
        'native_sincal_execution': False,
        'derived_fixed_tap_checks': tap_checks,
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inspector', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    report = check(args.inspector.resolve())
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
