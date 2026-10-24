#!/usr/bin/env python3
"""Check actual Rust mappings of CSIRO06 phase-pair loads against OpenDSS.

This is component evidence, not complete-network parsing or historical-result
agreement. Original hash-pinned MDB acquisition supplies the OpenDSS inputs.
No native results supply powers, voltages, connectivity or acceptance limits.
"""
import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def check(records, source, exported):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    case = next(c for c in manifest['cases'] if c['case'] == 6)
    if digest(records) != case['record_sha256'] or digest(source) != case['source_sha256']:
        raise ValueError('CSIRO06 original/acquisition hashes do not match the pinned corpus')
    document = json.loads(records.read_text())
    tables = {t['name']: [dict(zip([c['name'] for c in t['columns']], row, strict=True))
                          for row in t['rows']] for t in document['tables']}
    terminals = {r['Element_ID']: r for r in tables['Terminal'] if r['TerminalNo'] == 1 and r['Variant_ID'] == 1}
    pairs = {4: ['1','2'], 5: ['2','3'], 6: ['3','1']}
    selected = {r['Element_ID']: r for r in tables['Load'] if r['Variant_ID'] == 1
                and terminals[r['Element_ID']]['Flag_Terminal'] in pairs}
    components = json.loads(exported.read_text())['components']
    if len(components) != 18 or {c['element'] for c in components} != set(selected):
        raise ValueError('requires every one of the 18 authentic phase-pair load components')
    maximum = 0.0
    ground_counterexample = 0.0
    for component in components:
        row = selected[component['element']]
        port = terminals[component['element']]
        phases = pairs[port['Flag_Terminal']]
        load, bus, switch = (component[k] for k in ['load','bus','switch'])
        if row['Flag_Lf'] != 4 or row['Flag_LoadType'] != 1 or port['Flag_State'] != 1:
            raise ValueError('unexpected native input profile')
        if (load['configuration'] != 'single_phase' or load['terminal_map'] != phases
            or bus['grounded'] or bus['terminals'] != phases or switch['open']
            or switch['bus_from'] != str(port['Node_ID']) or switch['bus_to'] != bus['id']
            or switch['terminal_map_from'] != phases or switch['terminal_map_to'] != phases
            or load['bus'] != bus['id']
            or load['voltage_model']['model'] != 'constant_impedance'):
            raise ValueError('typed mapping changed conductor connectivity or voltage dependence')
        kva = row['S']*row['fS']*1000
        # OpenDSS derives Q itself from the publisher's apparent power and PF.
        dss('Clear\nNew Circuit.loadcheck basekv=0.5 phases=3 bus1=test\n'
            f'New Load.subject phases=1 bus1=test.{phases[0]}.{phases[1]} conn=delta '
            f'kv={row["Ul"]:.17g} kva={kva:.17g} pf={row["cosphi"]:.17g} model=2 status=fixed\nSolve')
        dss.Circuit.SetActiveElement('Load.subject')
        if dss.CktElement.NumConductors() != 2:
            raise ValueError('unexpected OpenDSS load primitive shape')
        raw = np.array(dss.CktElement.YPrim())
        expected = (raw[::2]+1j*raw[1::2]).reshape(2,2,order='F')
        if any(len(values) != 1 for values in [load['p_nom'], load['q_nom'], load['voltage_model']['v_nom']]):
            raise ValueError('phase-pair load must have exactly one branch')
        v = load['voltage_model']['v_nom'][0]
        if not np.isfinite(v) or v <= 0:
            raise ValueError('invalid nominal load voltage')
        y = complex(load['p_nom'][0],-load['q_nom'][0])/v**2
        actual = y*np.array([[1,-1],[-1,1]])
        if not np.all(np.isfinite(actual)) or not np.all(np.isfinite(expected)):
            raise ValueError('nonfinite load admittance')
        maximum = max(maximum,float(np.max(np.abs(actual-expected))))
        # A wrong grounded-Wye interpretation conducts common-mode current.
        ground_counterexample = max(ground_counterexample,abs(y*200))
        if np.max(np.abs(actual@np.ones(2))) > 1e-12:
            raise ValueError('phase-pair mapping introduces a ground path')
    return {'collection':manifest['collection'],'attribution':manifest['attribution'],
            'license':manifest['license'],'case':6,'component_count':len(components),
            'element_ids':sorted(selected),'source_sha256':case['source_sha256'],
            'record_sha256':case['record_sha256'],'export_sha256':digest(exported),
            'scope':'phase-pair load components only; not a complete-network parse',
            'engine':dss.Basic.Version(),'numpy':np.__version__,
            'maximum_admittance_error_s':maximum,'tolerance_s':1e-12,
            'wrong_grounding_common_mode_current_a':ground_counterexample,
            'native_execution':False,'historical_results_used':False,
            'passed': maximum < 1e-12 and ground_counterexample > 1}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('records',type=Path)
    parser.add_argument('source',type=Path)
    parser.add_argument('exported',type=Path)
    parser.add_argument('--report',type=Path,required=True)
    args=parser.parse_args()
    report=check(args.records,args.source,args.exported)
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    if not report['passed']:
        raise SystemExit('phase-pair load oracle failed')
    print(f"{report['component_count']} native load components passed; maximum admittance error {report['maximum_admittance_error_s']:.3e} S")


if __name__=='__main__':
    main()
