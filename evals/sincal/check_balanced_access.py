"""Compare original CSIRO19 balanced Access snapshots with independent pandapower.

The reference uses only hash-pinned native acquired input, not Rust parameters
or stored SINCAL results. Actual Rust output is solved separately by nodal current
injection. This is explicitly balanced coverage, not an unbalanced acceptance.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess

import numpy as np
import pandapower as pp
from check_balanced_simbench import solve_mapped

RECORD = 'c762c848f4f239fa751b2f30cd4bfce10912afbcad64ae5fa4f5782965aea0fd'
SOURCE = '5540e956effe9a8e6e691f834e79f62c86d96924fdd38092e5fd5c6fa84789c4'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def reference(tables, hours):
    nodes = {r['Node_ID']: r for r in tables['Node']}
    elements = {r['Element_ID']: r for r in tables['Element']}
    assert len(nodes) == 26 and len(elements) == 33
    assert {r['Variant_ID'] for rows in tables.values() for r in rows if 'Variant_ID' in r} == {1}
    ports = {}
    for row in tables['Terminal']:
        assert row['Flag_Terminal'] == 7 and row['Flag_State'] == 1
        ports.setdefault(row['Element_ID'], {})[row['TerminalNo']] = row['Node_ID']
    settings, = tables['CalcParameter']
    levels = {r['VoltLevel_ID']: r for r in tables['VoltageLevel']}
    net = pp.create_empty_network(f_hz=settings['f'], sn_mva=100)
    buses = {i: pp.create_bus(net, vn_kv=levels[r['VoltLevel_ID']]['Un']) for i, r in nodes.items()}
    for row in tables['Line']:
        eid = row['Element_ID']
        assert row['Flag_LineTyp'] == row['Flag_ESB'] == 1 and not row['va']
        assert row['fr'] == 1 and row['ParSys'] == 1
        pp.create_line_from_parameters(net, buses[ports[eid][1]], buses[ports[eid][2]],
            length_km=row['l'], r_ohm_per_km=row['r'], x_ohm_per_km=row['x'],
            c_nf_per_km=row['c'], max_i_ka=row['Ith'], in_service=bool(elements[eid]['Flag_State']))
    source, = tables['Infeeder']
    assert source['Flag_Lf'] == 3 and source['xi'] == 0
    pp.create_ext_grid(net, buses[ports[source['Element_ID']][1]],
        vm_pu=source['u']/100, va_degree=source['delta'])
    series = {r['OpSer_ID']: r for r in tables['OpSer']}
    powers = {}
    for row in tables['Load']:
        eid = row['Element_ID']
        assert row['Flag_LoadType'] == 2 and row['Flag_Lf'] == 1
        profile = series[row['DayOpSer_ID']]
        assert profile['Flag_Ser'] == 1 and profile['Flag_Typ'] == 3 and profile['BaseT'] == 24
        assert all(profile[f] == 0 for f in ['Power_a1', 'Power_b1', 'Reduce_a2', 'Reduce_b2'])
        samples = sorted((r for r in tables['OpSerVal'] if r['OpSer_ID'] == profile['OpSer_ID']), key=lambda r:r['OpTime'])
        assert len(samples) == 48 and all(r['Flag_Curve'] == 1 for r in samples)
        t = [r['OpTime'] for r in samples]
        p = np.interp(hours % 24, t, [r['P'] for r in samples], period=24) * row['fP']/1000
        q = np.interp(hours % 24, t, [r['Q'] for r in samples], period=24) * row['fQ']/1000
        powers[eid] = (float(p), float(q))
        pp.create_load(net, buses[ports[eid][1]], p_mw=p, q_mvar=q, in_service=bool(elements[eid]['Flag_State']))
    pp.runpp(net, calculate_voltage_angles=True, tolerance_mva=1e-10, numba=False)
    assert net.converged
    expected = {i: net.res_bus.at[b, 'vm_pu'] * np.exp(1j*np.deg2rad(net.res_bus.at[b, 'va_degree'])) for i, b in buses.items()}
    return expected, powers, ports, elements


def check_model(model, expected, powers, ports, elements):
    native = {b['id']: int(b['uid'].split(':')[-1]) for b in model['buses'] if b['uid'].startswith('sincal:node:')}
    roots = dict(native)
    for switch in model['switches']:
        assert switch['closed']
        roots[switch['to']] = roots[switch['from']]
    assert set(native.values()) == set(expected)
    rows = model['loads'] + model['generators'] + model['branches']
    ids = [int(r['uid'].split(':')[-1]) for r in rows]
    assert len(ids) == len(set(ids)) == len(elements) == 33
    assert set(ids) == set(elements)
    for row in rows:
        eid = int(row['uid'].split(':')[-1])
        actual_ports = [roots[row['from']], roots[row['to']]] if 'from' in row else [roots[row['bus']]]
        assert actual_ports == [ports[eid][k] for k in sorted(ports[eid])]
        assert row['in_service'] == bool(elements[eid]['Flag_State'])
    power_error = 0.
    for row in model['loads']:
        eid = int(row['uid'].split(':')[-1])
        error = max(abs(row['p']-powers[eid][0]), abs(row['q']-powers[eid][1]))
        assert error < 1e-12
        power_error = max(power_error, error)
    voltage, iterations, mismatch = solve_mapped(model, power_tolerance=1e-12)
    solved = {b['id']: v for b, v in zip(model['buses'], voltage)}
    error = max(abs(solved[i]-expected[native_id]) for i, native_id in native.items())
    assert error < 2e-8, error
    return dict(maximum_complex_voltage_error_pu=float(error), maximum_power_error_mw_mvar=power_error,
                nodal_iterations=iterations, nodal_power_mismatch_pu=mismatch)


def check(inspector, source, records):
    assert digest(source) == SOURCE and digest(records) == RECORD
    doc = json.loads(records.read_text())
    assert doc['source']['sha256'] == SOURCE and doc['source']['bytes'] == source.stat().st_size
    tables = {t['name']: [dict(zip([c['name'] for c in t['columns']], r, strict=True)) for r in t['rows']]
              for t in doc['tables']}
    reports, controls = [], []
    for hours in [0, 0.25, 6, 12, 18, 23.75, 24]:
        run = subprocess.run([str(inspector), str(source), str(records), str(hours)],
                             check=True, capture_output=True, text=True, timeout=60)
        model = json.loads(run.stdout)
        assert model['public_facade_access_selection']
        assert model['source_echo_and_ir_checked']
        assert model['wrong_family_missing_time_variant_and_origin_rejected']
        assert model['profile'] == 'balanced' and model['schema'] == 11.5 and model['variant'] == 1
        expected, powers, ports, elements = reference(tables, hours)
        report = check_model(model, expected, powers, ports, elements)
        reports.append(dict(hours=hours, total_load_mw=sum(p[0] for p in powers.values()), **report))
        if hours == 12:
            for kind in ('power_units', 'impedance_base', 'charging', 'source_voltage'):
                changed = copy.deepcopy(model)
                if kind == 'power_units': changed['loads'][0]['p'] *= 1000
                elif kind == 'impedance_base': changed['branches'][0]['r'] *= 100
                elif kind == 'charging':
                    for branch in changed['branches']: branch['b'] = 0; branch['charging'] = None
                else:
                    for b in changed['buses']:
                        if b['kind'] == 'REF': b['vm'] *= 1.01
                try: check_model(changed, expected, powers, ports, elements)
                except AssertionError: controls.append(kind)
                else: raise AssertionError(f'undetected mutation {kind}')
    assert len(controls) == 4
    return dict(scope=__doc__, collection='https://doi.org/10.4225/08/5631B1DF6F1A0',
        attribution='Berry, Adam; Collins, Lyle; Oliver, Erin; Perfumo, Cristian (2015), Representative Australian Electricity Feeders with load and solar generation profiles, v1, CSIRO.',
        license='CC-BY-4.0', source_sha256=SOURCE, record_sha256=digest(records),
        reader_sha256=digest(inspector), profile='balanced', schema=11.5, variant=1,
        native_nodes=26, native_elements=33, lines=25, loads=7, sources=1,
        snapshots=reports, negative_controls=controls, pandapower=pp.__version__, numpy=np.__version__,
        native_sincal_execution=False, public_facade_access_selection=True, source_echo_and_ir_checked=True,
        wrong_family_missing_time_variant_and_origin_rejected=True,
        additional_unbalanced_cases=0, passed=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('inspector', 'source', 'records', 'report'): parser.add_argument(name, type=Path)
    args = parser.parse_args()
    result = check(args.inspector.resolve(), args.source, args.records)
    args.report.write_text(json.dumps(result, indent=2)+'\n')
    print('CSIRO19 balanced: 33 components, seven snapshots, four mutation controls passed.')
