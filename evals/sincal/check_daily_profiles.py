#!/usr/bin/env python3
"""Validate actual Rust CSIRO daily single-phase and phase-pair load snapshots using OpenDSS.

All selected loads in cases 01/04/07 are accounted for: valid profiles at five
declared times, conflicting native timestamps as explicit rejections.
Publisher inputs supply powers and interpolation; no historical result is used.
This is component evidence, not a complete feeder solve or native acceptance.
"""
import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

TIMES = [0.0, 0.25, 0.5, 23.75, 24.0]
CONNECTIONS = {1: ['1', '0'], 2: ['2', '0'], 3: ['3', '0'], 4: ['1', '2'], 5: ['2', '3'], 6: ['3', '1']}


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def check(sources, records, exported):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    exports = json.loads(exported.read_text())['cases']
    if sorted(c['case'] for c in exports) != [1, 4, 7]:
        raise ValueError('requires exactly CSIRO cases 01, 04, 07')
    reports = []
    for export in exports:
        case = export['case']
        identity = next(c for c in manifest['cases'] if c['case'] == case)
        record = records / f'representative{case:02d}.json'
        source = sources / f'csiro-representative{case:02d}.mdb'
        if digest(record) != identity['record_sha256'] or digest(source) != identity['source_sha256']:
            raise ValueError(f'case {case}: source/acquisition identity mismatch')
        tables = {}
        for table in json.loads(record.read_text())['tables']:
            columns = [c['name'] for c in table['columns']]
            tables[table['name']] = [dict(zip(columns, row, strict=True)) for row in table['rows']]
        ports = {r['Element_ID']: r for r in tables['Terminal'] if r['Variant_ID'] == 1 and r['TerminalNo'] == 1}
        selected = {r['Element_ID']: r for r in tables['Load'] if r['Variant_ID'] == 1
                    and ports[r['Element_ID']]['Flag_Terminal'] in CONNECTIONS and r['DayOpSer_ID']}
        profiles = {r['OpSer_ID']: r for r in tables['OpSer'] if r['Variant_ID'] == 1}
        points = {}
        for row in tables['OpSerVal']:
            if row['Variant_ID'] == 1:
                points.setdefault(row['OpSer_ID'], []).append(row)
        conflicting = {profile for profile, rows in points.items()
                       if len({p['OpTime'] for p in rows}) != len(rows)}
        rejected = {element: row['DayOpSer_ID'] for element, row in selected.items()
                    if row['DayOpSer_ID'] in conflicting}
        observed_rejections = export['rejected']
        if (len(observed_rejections) != len(rejected)
                or {r['element']: r['profile'] for r in observed_rejections} != rejected
                or any('duplicate or unordered OpTime' not in r['error'] for r in observed_rejections)):
            raise ValueError('conflicting native sample rejections do not match independent inventory')
        selected = {element: row for element, row in selected.items() if element not in rejected}
        components = export['components']
        required = {(element, t) for element in selected for t in TIMES}
        if len(components) != len(required) or {(c['element'], c['hours']) for c in components} != required:
            raise ValueError('export does not cover every selected element and time exactly once')
        maximum = power_error = 0.0
        for component in components:
            row = selected[component['element']]
            profile = profiles[row['DayOpSer_ID']]
            if (profile['Flag_Typ'] != 3 or profile['Flag_Ser'] != 1
                    or row['WeekOpSer_ID'] or row['YearOpSer_ID']
                    or row['Flag_Lf'] != 4 or row['Flag_LoadType'] != 1
                    or row['fP'] != 1 or row['fQ'] != 1
                    or any(profile[k] != 0 for k in ['Power_a1', 'Power_b1', 'Reduce_a2', 'Reduce_b2'])):
                raise ValueError('native inputs outside independent profile contract')
            samples = sorted(points[row['DayOpSer_ID']], key=lambda p: p['OpTime'])
            period = profile['BaseT'] or 24.0
            times = [p['OpTime'] for p in samples]
            if not (times[0] == 0 and times[-1] < period and len(set(times)) == len(times)) or any(p['Flag_Curve'] != 1 for p in samples):
                raise ValueError('native profile is not the verified continuous cyclic series')
            # NumPy's independent interpolation includes the final-to-first segment.
            t = component['hours'] % period
            pq = [float(np.interp(t, times + [period], [p[k] for p in samples] + [samples[0][k]])) for k in ['P', 'Q']]
            port = ports[component['element']]
            phases = CONNECTIONS[port['Flag_Terminal']]
            grounded = port['Flag_Terminal'] in [1, 2, 3]
            switched = phases[:1] if grounded else phases
            kv = row['Ul'] / np.sqrt(3) if grounded else row['Ul']
            load, bus, switch = [component[k] for k in ['load', 'bus', 'switch']]
            if (load['configuration'] != 'single_phase' or load['terminal_map'] != phases
                    or bus['terminals'] != phases or bus['grounded'] != (['0'] if grounded else [])
                    or load['bus'] != bus['id']
                    or switch['bus_from'] != str(port['Node_ID']) or switch['bus_to'] != bus['id']
                    or switch['terminal_map_from'] != switched or switch['terminal_map_to'] != switched
                    or switch['open'] or port['Flag_State'] != 1
                    or load['voltage_model']['model'] != 'constant_impedance'
                    or any(len(a) != 1 for a in [load['p_nom'], load['q_nom'], load['voltage_model']['v_nom']])):
                raise ValueError('mapped load changed its electrical connection or voltage model')
            voltage = load['voltage_model']['v_nom'][0]
            actual_pq = np.array([load['p_nom'][0], load['q_nom'][0]])
            if not np.isfinite(voltage) or not np.isclose(voltage, kv * 1000, rtol=1e-14, atol=0) or not np.all(np.isfinite(actual_pq)):
                raise ValueError('invalid mapped voltage or powers')
            power_error = max(power_error, float(np.max(np.abs(actual_pq - np.array(pq) * 1000))))
            selection = load['extras']['sincal_profile']
            if selection != {'profile': row['DayOpSer_ID'], 'requested_hours': component['hours'],
                             'cyclic_hours': t, 'period_hours': period,
                             'power_factors': [row['fP'], row['fQ']]}:
                raise ValueError('snapshot selection provenance mismatch')
            dss('Clear\nNew Circuit.profiles basekv=0.5 phases=3 bus1=test\n'
                f'New Load.subject phases=1 bus1=test.{phases[0]}.{phases[1]} conn={"wye" if grounded else "delta"} '
                f'kv={kv:.17g} kw={pq[0]:.17g} kvar={pq[1]:.17g} model=2 status=fixed\nSolve')
            dss.Circuit.SetActiveElement('Load.subject')
            if dss.CktElement.NodeOrder() != [int(p) for p in phases]:
                raise ValueError('unexpected OpenDSS phase/earth terminal order')
            raw = np.array(dss.CktElement.YPrim())
            expected = (raw[::2] + 1j * raw[1::2]).reshape(2, 2, order='F')
            actual = complex(*[actual_pq[0], -actual_pq[1]]) / voltage**2 * np.array([[1, -1], [-1, 1]])
            if not np.all(np.isfinite(expected)) or not np.all(np.isfinite(actual)):
                raise ValueError('nonfinite primitive')
            # For phase-to-earth loads V_earth is constrained to zero. Check
            # both terminal-current rows against every unconstrained voltage
            # column. OpenDSS adds a numerical shunt only to its neutral
            # diagonal (Load.pas, CalcYPrimMatrix, DSS C-API 0.14.5); that
            # diagonal multiplies zero here. No fitted correction or looser
            # tolerance is applied to the electrical response.
            columns = [0] if grounded else [0, 1]
            maximum = max(maximum, float(np.max(np.abs(actual[:, columns] - expected[:, columns]))))
        reports.append({'case': case, 'source_sha256': identity['source_sha256'],
                        'record_sha256': identity['record_sha256'], 'load_count': len(selected),
                        'single_phase_load_count': sum(ports[e]['Flag_Terminal'] in [1, 2, 3] for e in selected),
                        'snapshot_count': len(components), 'rejected_conflicting_profiles': observed_rejections, 'maximum_power_error_w_var': power_error,
                        'maximum_admittance_error_s': maximum,
                        'passed': maximum < 1e-12 and power_error < 1e-8})
    return {'collection': manifest['collection'], 'attribution': manifest['attribution'],
            'license': manifest['license'], 'scope': 'Profiled single-phase and phase-pair load components only; no complete feeder parsing or solve.',
            'export_sha256': digest(exported), 'hours': TIMES, 'engine': dss.Basic.Version(),
            'numpy': np.__version__, 'admittance_tolerance_s': 1e-12,
            'grounded_comparison': 'Both current rows over unconstrained voltage columns; native earth and OpenDSS node 0 constrained to zero.',
            'oracle_source': 'https://github.com/dss-extensions/dss_capi/blob/0.14.5/src/PCElements/Load.pas', 'power_tolerance_w_var': 1e-8,
            'native_execution': False, 'historical_results_used': False, 'cases': reports,
            'passed': all(c['passed'] for c in reports)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sources', type=Path)
    parser.add_argument('records', type=Path)
    parser.add_argument('exported', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    report = check(args.sources, args.records, args.exported)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    if not report['passed']:
        raise SystemExit('native load-profile check failed')
    print(f"{sum(c['load_count'] for c in report['cases'])} native loads, {sum(c['snapshot_count'] for c in report['cases'])} snapshots passed")


if __name__ == '__main__':
    main()
