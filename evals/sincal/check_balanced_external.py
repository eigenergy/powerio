"""Check the original external IEEE18/33 and student SQLite inputs end to end.

No external model files are redistributed. All three named, hash-pinned files
must be supplied explicitly. Build pandapower independently from native physical
inputs and solve the actual Rust-mapped network with separate nodal equations.
Stored native results are a separate, non-gating historical comparison; in the
student case static input powers are zero while stored results use profiles.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import sqlite3
import subprocess

import numpy as np
import pandapower as pp

from check_balanced_simbench import close, solve_mapped

# Research-only source identities, recorded in research-catalog.md. A license
# for this validation program does not grant rights to redistribute the models.
CASES = {
    'student': {'sha256': 'b7cadedb67548ff45587412cf895f7eeca00937f287ccdb0c3a39236dad7207e', 'nodes': 40, 'elements': 84},
    'IEEE18': {'sha256': '9978078e4c956cf6d2e740bf2411fd5076111c17bfbc564925b3f205e797e8e5', 'nodes': 18, 'elements': 44},
    'IEEE33': {'sha256': '3b392c49f28770c77aa73b0a9b6091afab8e43f2b9af8b37df919e09f60f8e12', 'nodes': 33, 'elements': 67},
}


def table(db, name, key):
    return {row[key]: dict(row) for row in db.execute(f'SELECT * FROM "{name}" WHERE Variant_ID=1')}


def reference(db):
    settings = next(iter(table(db, 'CalcParameter', 'CalcParameter_ID').values()))
    levels = table(db, 'VoltageLevel', 'VoltLevel_ID')
    nodes = table(db, 'Node', 'Node_ID')
    elements = table(db, 'Element', 'Element_ID')
    terminals = table(db, 'Terminal', 'Terminal_ID')
    ports = {}
    for row in terminals.values():
        assert row['Flag_Terminal'] == 7 and row['Flag_State'] == 1
        ports.setdefault(row['Element_ID'], {})[row['TerminalNo']] = row['Node_ID']
    net = pp.create_empty_network(f_hz=settings['f'], sn_mva=100.0)
    buses = {key: pp.create_bus(net, vn_kv=levels[r['VoltLevel_ID']]['Un']) for key, r in nodes.items()}
    def node(element, position=1):
        return buses[ports[element][position]]
    for key, row in table(db, 'Line', 'Element_ID').items():
        assert row['Flag_Lf'] == 1 and row['Flag_ESB'] == 1 and not row['va']
        assert not row['CoupData_ID']
        pp.create_line_from_parameters(net, node(key), node(key, 2),
            length_km=row['l'], r_ohm_per_km=row['r'] * row['fr'],
            x_ohm_per_km=row['x'], c_nf_per_km=row['c'],
            max_i_ka=row['Ith'], parallel=int(row['ParSys']),
            in_service=bool(elements[key]['Flag_State']))
        assert row['ParSys'] == int(row['ParSys'])
    for key, row in table(db, 'Load', 'Element_ID').items():
        assert row['Flag_Lf'] == 1 and row['Flag_LoadType'] == 2
        for flag, field in [('Flag_UseTimeSer', 'DayOpSer_ID'), ('Flag_UseTimeSer', 'YearOpSer_ID'),
                            ('Flag_UseOpSer', 'WeekOpSer_ID'), ('Flag_UseIncSer', 'IncrSer_ID')]:
            assert not settings[flag] or not row[field]
        pp.create_load(net, node(key), p_mw=row['P'] * row['fP'], q_mvar=row['Q'] * row['fQ'],
            in_service=bool(elements[key]['Flag_State']))
    for key, row in table(db, 'Infeeder', 'Element_ID').items():
        assert row['Flag_Lf'] in (3, 8) and not any(row[f] for f in ('Rlf', 'Xlf', 'xi'))
        pp.create_ext_grid(net, node(key), vm_pu=row['u'] / 100, va_degree=row['delta'],
            in_service=bool(elements[key]['Flag_State']))
    for key, row in table(db, 'ShuntCondensator', 'Element_ID').items():
        assert row['Flag_roh'] == 1 and row['Flag_Lf'] == 1
        s = row['Sn'] + row['deltaS'] * (row['roh'] - row['rohm'])
        p = row['Vdi'] / 1000 * s / row['Sn']
        q = -math.sqrt(s * s - p * p)
        pp.create_shunt(net, node(key), p_mw=p, q_mvar=q, vn_kv=row['Un'],
            in_service=bool(elements[key]['Flag_State']))
    assert {r['Type'].rstrip(' ') for r in elements.values()} <= {'Line', 'Load', 'Infeeder', 'ShuntCondensator'}
    pp.runpp(net, calculate_voltage_angles=True, tolerance_mva=1e-10, numba=False)
    assert net.converged
    return net, buses, elements, nodes, terminals


def check(name, path, inspector):
    identity = CASES[name]
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    assert digest == identity['sha256'], (name, 'unexpected source hash', digest)
    run = subprocess.run([str(inspector), str(path)], check=True, capture_output=True, text=True, timeout=60)
    model = json.loads(run.stdout)
    db = sqlite3.connect(f'{path.resolve().as_uri()}?mode=ro', uri=True)
    db.row_factory = sqlite3.Row
    with db:
        expected, buses, elements, nodes, terminals = reference(db)
        historical = [dict(r) for r in db.execute('SELECT * FROM LFNodeResult WHERE Variant_ID=1')]
        settings = next(iter(table(db, 'CalcParameter', 'CalcParameter_ID').values()))
        loads = table(db, 'Load', 'Element_ID')
    native_buses = {int(b['uid'].split(':')[-1]): b for b in model['buses'] if b['uid'].startswith('sincal:node:')}
    assert len(native_buses) == identity['nodes'] == len(nodes)
    mapped_elements = [r for family in ('branches', 'loads', 'generators', 'shunts') for r in model[family]]
    assert len(mapped_elements) == len(elements) == identity['elements']
    assert {r['uid'] for r in mapped_elements} == {f'sincal:element:{key}' for key in elements}
    # Validate stable identity, native topology after collapsing closed terminal
    # switches, and loads independently of the whole-network voltage check.
    root = {b['id']: b['id'] for b in model['buses']}
    for s in model['switches']:
        assert s['closed']
        root[s['to']] = s['from']
    for r in mapped_elements:
        key = int(r['uid'].split(':')[-1])
        ports = sorted((t['TerminalNo'], t['Node_ID']) for t in terminals.values() if t['Element_ID'] == key)
        actual = [r['from'], r['to']] if 'from' in r else [r['bus']]
        assert [root[n] for n in actual] == [n for _, n in ports]
        assert r['in_service'] == bool(elements[key]['Flag_State'])
    for r in model['loads']:
        native = loads[int(r['uid'].split(':')[-1])]
        close(r['p'], native['P'] * native['fP'], 'load P')
        close(r['q'], native['Q'] * native['fQ'], 'load Q')
    voltage, iterations, mismatch = solve_mapped(model, power_tolerance=1e-12)
    solved = {b['id']: v for b, v in zip(model['buses'], voltage)}
    fresh_error = max(abs(solved[key] - expected.res_bus.at[buses[key], 'vm_pu'] *
        np.exp(1j * np.deg2rad(expected.res_bus.at[buses[key], 'va_degree']))) for key in nodes)
    assert fresh_error < 2e-8, (name, fresh_error)
    # Never treat historical rows as inputs or tune the mapper to their values.
    historical_error = max((abs(abs(solved[r['Node_ID']]) - r['U_Un'] / 100)
        for r in historical if r['Node_ID'] in solved), default=None)
    return {
        'case': name, 'source_sha256': digest, 'source_bytes': path.stat().st_size,
        'schema': model['schema'], 'variant': model['variant'], 'profile': model['profile'],
        'native_nodes': len(nodes), 'mapped_buses_including_switch_ports': len(model['buses']),
        'mapped_elements': len(mapped_elements), 'mapped_switches': len(model['switches']),
        'mapping': 'complete static inputs', 'identity_topology_load_checks': 'passed',
        'fresh_pandapower_voltage_complex_error_pu': float(fresh_error), 'tolerance_pu': 2e-8,
        'nodal_power_tolerance_pu': 1e-12, 'nodal_iterations': iterations, 'nodal_power_mismatch_pu': mismatch,
        'active_load_mw': sum(r['p'] for r in model['loads'] if r['in_service']),
        'native_profile_enable_flags': {f: settings[f] for f in ('Flag_UseTimeSer', 'Flag_UseOpSer', 'Flag_UseIncSer')},
        'historical_rows': len(historical), 'historical_max_voltage_magnitude_difference_pu': historical_error,
        'historical_alignment': 'not established; not an acceptance check',
        'native_sincal_execution': False, 'redistribution': 'external-only; no model fixture',
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inspector', type=Path)
    parser.add_argument('directory', type=Path, help='contains IEEE18.db, IEEE33.db and student.db')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    result = {'pandapower_version': pp.__version__, 'numpy_version': np.__version__,
              'cases': [check(name, args.directory / f'{name}.db', args.inspector.resolve()) for name in CASES]}
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
