#!/usr/bin/env python3
"""Check every CSIRO05 load at five times against publisher inputs and OpenDSS.

Manipulators edit stored input fields; they are not an extra runtime multiplier
(Siemens Release Notes 21.0, pp.4–6). The acquired electrical rows are the source
of truth. Conflicting native timestamps remain explicit rejections. This checks
components, not a complete feeder or native SINCAL execution.
"""
import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

TIMES = [0.0, 0.25, 0.5, 23.75, 24.0]
EVIDENCE = 'https://sincal.s3.amazonaws.com/21.0/ReleaseNotes-Eng.pdf'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def complex_vector(values):
    raw = np.array(values)
    return raw[::2] + 1j * raw[1::2]


def check(source, records, exported):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identity = next(c for c in manifest['cases'] if c['case'] == 5)
    if digest(source) != identity['source_sha256'] or digest(records) != identity['record_sha256']:
        raise ValueError('native source/acquisition identity mismatch')
    tables = {}
    for table in json.loads(records.read_text())['tables']:
        columns = [c['name'] for c in table['columns']]
        tables[table['name']] = [dict(zip(columns, r, strict=True)) for r in table['rows']]
    selected = {r['Element_ID']: r for r in tables['Load'] if r['Variant_ID'] == 1}
    ports = {r['Element_ID']: r for r in tables['Terminal'] if r['Variant_ID'] == 1 and r['TerminalNo'] == 1}
    profiles = {r['OpSer_ID']: r for r in tables['OpSer'] if r['Variant_ID'] == 1}
    points = {}
    for row in tables['OpSerVal']:
        if row['Variant_ID'] == 1:
            points.setdefault(row['OpSer_ID'], []).append(row)
    conflicts = {p for p, rows in points.items() if len({r['OpTime'] for r in rows}) != len(rows)}
    rejected = {e: r['DayOpSer_ID'] for e, r in selected.items() if r['DayOpSer_ID'] in conflicts}
    export = json.loads(exported.read_text())
    if (export['case'] != 5 or len(selected) != 458 or len(export['rejected']) != len(rejected)
            or {r['element']: r['profile'] for r in export['rejected']} != rejected
            or any('duplicate or unordered OpTime' not in r['error'] for r in export['rejected'])):
        raise ValueError('native inventory or rejection accounting mismatch')
    selected = {e: r for e, r in selected.items() if e not in rejected}
    required = {(e, t) for e in selected for t in TIMES}
    components = export['components']
    if len(components) != len(required) or {(c['element'], c['hours']) for c in components} != required:
        raise ValueError('missing or duplicated load snapshot')
    max_power = max_current = 0.0
    double_factor_counterexamples = zero_power_snapshots = 0
    for component in components:
        row = selected[component['element']]
        port = ports[component['element']]
        profile = profiles[row['DayOpSer_ID']]
        if (row['Flag_LoadType'] != 2 or row['Flag_Lf'] != 15 or row['Mpl_ID'] != 1
                or row['fP'] != 0.28 or row['fQ'] != 0.28 or row['WeekOpSer_ID'] or row['YearOpSer_ID']
                or row['Flag_LP'] != 3 or row['u'] != 100 or row['Ul'] != 22
                or port['Flag_Terminal'] not in [4, 7] or port['Flag_State'] != 1
                or profile['Flag_Typ'] != 3 or profile['Flag_Ser'] != 1 or profile['BaseT'] != 24
                or any(profile[k] != 0 for k in ['Power_a1', 'Power_b1', 'Reduce_a2', 'Reduce_b2'])):
            raise ValueError('native input outside independent oracle contract')
        samples = sorted(points[row['DayOpSer_ID']], key=lambda r: r['OpTime'])
        times = [r['OpTime'] for r in samples]
        if times[0] != 0 or times[-1] >= 24 or any(r['Flag_Curve'] != 1 for r in samples):
            raise ValueError('unsupported profile interpolation')
        t = component['hours'] % 24
        # Independent interpolation and one application of the materialized factors.
        pq = np.array([np.interp(t, times + [24], [r[k] for r in samples] + [samples[0][k]])
                       * row[f'f{k}'] for k in ['P', 'Q']])
        phases = ['1', '2'] if port['Flag_Terminal'] == 4 else ['1', '2', '3']
        count = 1 if len(phases) == 2 else 3
        load, bus, switch = [component[k] for k in ['load', 'bus', 'switch']]
        if (load['configuration'] != ('single_phase' if count == 1 else 'delta')
                or load['terminal_map'] != phases or bus['terminals'] != phases or bus['grounded']
                or load['bus'] != bus['id'] or load['name'] != str(component['element'])
                or switch['bus_from'] != str(port['Node_ID']) or switch['bus_to'] != bus['id']
                or switch['terminal_map_from'] != phases or switch['terminal_map_to'] != phases
                or switch['open'] or load['voltage_model']['model'] != 'constant_power'
                or load['voltage_model']['v_nom'] != [22000.0] * count
                or len(load['p_nom']) != count or len(load['q_nom']) != count):
            raise ValueError('mapped circuit changed phase connection, state or load model')
        extras = load['extras']
        if (extras['sincal_manipulation'] != {'id': 1, 'semantics': 'materialized_input'}
                or extras['sincal_profile'] != {'profile': row['DayOpSer_ID'],
                    'requested_hours': component['hours'], 'cyclic_hours': t, 'period_hours': 24,
                    'power_factors': [row['fP'], row['fQ']]}):
            raise ValueError('manipulator/profile provenance mismatch')
        actual_pq = np.array([load['p_nom'], load['q_nom']])
        expected_pq = np.repeat((pq * 1000 / count)[:, None], count, axis=1)
        if not np.all(np.isfinite(actual_pq)):
            raise ValueError('nonfinite mapped power')
        max_power = max(max_power, float(np.max(np.abs(actual_pq - expected_pq))))
        if np.max(np.abs(expected_pq * 0.28 - actual_pq)) > 1e-8:
            double_factor_counterexamples += 1
        elif np.all(expected_pq == 0) and np.all(actual_pq == 0):
            zero_power_snapshots += 1
        else:
            raise ValueError('nonzero load failed double-factor counterexample')
        dss('Clear\nNew Circuit.subject basekv=22 bus1=test phases=3 pu=1.03\n'
            f'New Load.subject phases={count} bus1=test.{".".join(phases)} conn=delta '
            f'kv=22 kw={pq[0]:.17g} kvar={pq[1]:.17g} model=1 status=fixed\n'
            'Set tolerance=1e-12\nSolve')
        if not dss.Solution.Converged():
            raise ValueError('OpenDSS component circuit did not converge')
        dss.Circuit.SetActiveElement('Load.subject')
        if dss.CktElement.NodeOrder() != [int(p) for p in phases]:
            raise ValueError('OpenDSS terminal order mismatch')
        voltage = complex_vector(dss.CktElement.Voltages())
        expected_current = complex_vector(dss.CktElement.Currents())
        incidence = np.array([[1., -1.]]) if count == 1 else np.array([[1., -1., 0.], [0., 1., -1.], [-1., 0., 1.]])
        power = actual_pq[0] + 1j * actual_pq[1]
        actual_current = incidence.T @ np.conj(power / (incidence @ voltage))
        if not np.all(np.isfinite(actual_current)) or not np.all(np.isfinite(expected_current)):
            raise ValueError('nonfinite terminal current')
        max_current = max(max_current, float(np.max(np.abs(actual_current - expected_current))))
    passed = (max_power < 1e-8 and max_current < 1e-8 and double_factor_counterexamples + zero_power_snapshots == len(components))
    return {'collection': manifest['collection'], 'attribution': manifest['attribution'], 'license': manifest['license'],
            'scope': 'CSIRO05 constant-power delta load components; no complete feeder or native execution.',
            'source_sha256': identity['source_sha256'], 'record_sha256': identity['record_sha256'],
            'export_sha256': digest(exported), 'semantics_source': EVIDENCE, 'semantics_pages': '4–6',
            'load_count': len(selected), 'three_phase_load_count': sum(ports[e]['Flag_Terminal'] == 7 for e in selected),
            'phase_pair_load_count': sum(ports[e]['Flag_Terminal'] == 4 for e in selected),
            'snapshot_count': len(components), 'hours': TIMES, 'rejected_conflicting_profiles': export['rejected'],
            'maximum_power_error_w_var': max_power, 'maximum_terminal_current_error_a': max_current,
            'double_factor_counterexamples': double_factor_counterexamples, 'zero_power_snapshots': zero_power_snapshots,
            'power_tolerance_w_var': 1e-8, 'current_tolerance_a': 1e-8,
            'engine': dss.Basic.Version(), 'numpy': np.__version__,
            'native_execution': False, 'historical_results_used': False, 'passed': passed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('records', type=Path)
    parser.add_argument('exported', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    report = check(args.source, args.records, args.exported)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    if not report['passed']:
        raise SystemExit('materialized-load validation failed')
    print(f"{report['load_count']} loads / {report['snapshot_count']} snapshots passed; {len(report['rejected_conflicting_profiles'])} conflicting profiles rejected")


if __name__ == '__main__':
    main()
