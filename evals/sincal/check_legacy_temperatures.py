#!/usr/bin/env python3
"""Check native lines with schema-11.5 temperature defaults against OpenDSS.

Only local line circuits are checked; no complete feeder or native execution.
The original MDB and its acquired NULL fields are never rewritten.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

from audit_distribution import digest
from check_csiro09_network import tables


def check(source_dir, record_dir, export):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    exported = json.loads(export.read_text())
    if [c['case'] for c in exported] != [3, 5, 12]:
        raise ValueError('expected three native cases exactly once')
    reports = []
    for case, expected_count in zip(exported, [25, 8, 153], strict=True):
        n = case['case']
        identity = next(c for c in manifest['cases'] if c['case'] == n)
        source = source_dir/f'csiro-representative{n:02}.mdb'
        records = record_dir/f'representative{n:02}.json'
        if digest(source) != identity['source_sha256'] or digest(records) != identity['record_sha256']:
            raise ValueError('native source/acquisition identity mismatch')
        t = tables(records)
        native = {r['Element_ID']: r for r in t['Line']}
        elements = {r['Element_ID']: r for r in t['Element']}
        levels = {r['VoltLevel_ID']: r for r in t['VoltageLevel']}
        ports = {}
        for p in t['Terminal']:
            ports.setdefault(p['Element_ID'], []).append(p)
        candidates = {i for i, r in native.items() if r['Flag_LineTyp'] in [1, 2]
                      and r['Flag_Ll'] == 0
                      and levels[elements[i]['VoltLevel_ID']][{1:'Temp_Cable', 2:'Temp_Line'}[r['Flag_LineTyp']]] is None}
        if len(case['lines']) != expected_count or {c['element'] for c in case['lines']} != candidates:
            raise ValueError('missing, extra or duplicate defaulted native line')
        maximum = 0.0
        wrong_temperature = []
        opened = 0
        single_phase = 0
        for c in case['lines']:
            i = c['element']; r = native[i]; line = c['line']; code = c['code']
            ps = sorted(ports[i], key=lambda p: p['TerminalNo'])
            if (len(ps) != 2 or [p['TerminalNo'] for p in ps] != [1, 2]
                    or r['Flag_Z0_Input'] != 2 or r['fn'] != 50 or r['ParSys'] != 1 or r['va'] != 0
                    or elements[i]['Flag_State'] != 1 or ps[0]['Flag_Terminal'] != ps[1]['Flag_Terminal']):
                raise ValueError('native profile changed')
            selection = ps[0]['Flag_Terminal']
            if selection not in [1, 7]:
                raise ValueError('unexpected native phase profile')
            phases = ['1'] if selection == 1 else ['1', '2', '3']
            size = len(phases)
            single_phase += int(size == 1)
            if size == 1 and any(r[a] != r[b] for a,b in [('r','r0'), ('x','x0'), ('c','c0')]):
                raise ValueError('single phase oracle needs independent phase parameters')
            expected_default = 'VoltageLevel.' + {1:'Temp_Cable', 2:'Temp_Line'}[r['Flag_LineTyp']]
            if (c['defaulted'] != [expected_default] or code['n_conductors'] != size
                    or line['terminal_map_from'] != phases or line['terminal_map_to'] != phases
                    or line['linecode'] != code['name']
                    or not np.isclose(line['length'], r['l']*1000, rtol=1e-14, atol=0)):
                raise ValueError('phase, length or default provenance mismatch')
            rating = None if r['Ith'] == 0 else [r['Ith']*1000*r['fr']]*size
            if code.get('i_max') != rating:
                raise ValueError('native rating changed')
            if len(c['switches']) != sum(p['Flag_State'] == 0 for p in ps) or len(c['auxiliary_buses']) != len(c['switches']):
                raise ValueError('open native terminal was not retained')
            for p, side in zip(ps, ['from', 'to'], strict=True):
                end = line['bus_'+side]
                if p['Flag_State'] == 1:
                    if end != str(p['Node_ID']): raise ValueError('native line endpoint changed')
                elif p['Flag_State'] == 0:
                    switch = next(s for s in c['switches'] if s['name'] == f"sincal:terminal:{p['Terminal_ID']}")
                    bus = next(b for b in c['auxiliary_buses'] if b['id'] == end)
                    if (not switch['open'] or switch['bus_from'] != str(p['Node_ID']) or switch['bus_to'] != end
                            or switch['terminal_map_from'] != phases or switch['terminal_map_to'] != phases
                            or bus['terminals'] != phases or bus['grounded']):
                        raise ValueError('open terminal connectivity changed')
                    opened += 1
                else: raise ValueError('unknown native terminal state')
            dss('Clear\nNew Circuit.check basekv=22 phases=3\n'
                f'New Line.subject bus1=a bus2=b phases={size} units=km length={r["l"]:.17g} '
                f'r1={r["r"]:.17g} x1={r["x"]:.17g} r0={r["r0"]:.17g} x0={r["x0"]:.17g} '
                f'c1={r["c"]:.17g} c0={r["c0"]:.17g} basefreq=50\nSet frequency=50\nSolve')
            dss.Circuit.SetActiveElement('Line.subject')
            raw = np.asarray(dss.CktElement.YPrim())
            expected = (raw[::2]+1j*raw[1::2]).reshape(2*size, 2*size, order='F')
            z = (np.asarray(code['r_series'])+1j*np.asarray(code['x_series']))*line['length']
            y = np.linalg.inv(z)
            sh = [(np.asarray(code['g_'+side])+1j*np.asarray(code['b_'+side]))*line['length'] for side in ['from','to']]
            actual = np.block([[y+sh[0],-y],[-y,y+sh[1]]])
            error = float(np.max(abs(actual-expected))/np.max(abs(expected)))
            if not np.isfinite(error) or error > 1e-10:
                raise ValueError(f'case {n} line {i}: independent primitive mismatch {error}')
            maximum = max(maximum, error)
            # A wrong 70 C default must change the electrical result measurably.
            bad_y = np.linalg.inv(z.real*(1+50*r['alpha'])+1j*z.imag)
            bad = np.block([[bad_y+sh[0],-bad_y],[-bad_y,bad_y+sh[1]]])
            wrong_temperature.append(float(np.max(abs(bad-expected))/np.max(abs(expected))))
        if min(wrong_temperature) < 1e-4:
            raise ValueError('wrong-temperature negative control did not detect a difference')
        reports.append(dict(case=n,lines=expected_count,single_phase_lines=single_phase,open_terminals=opened,
                            maximum_relative_primitive_error=maximum,
                            minimum_wrong_temperature_relative_error=min(wrong_temperature),
                            source_sha256=digest(source),record_sha256=digest(records)))
    return dict(scope=__doc__,cases=reports,relative_tolerance=1e-10,default_temperature_c=20,
                export_sha256=digest(export),collection=manifest['collection'],attribution=manifest['attribution'],
                license=manifest['license'],engine=dss.Basic.Version(),numpy=np.__version__,
                native_execution=False,whole_feeder_validation=False,passed=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source_dir','record_dir','export','report']: parser.add_argument(name,type=Path)
    a = parser.parse_args()
    result = check(a.source_dir,a.record_dir,a.export)
    a.report.write_text(json.dumps(result,indent=2)+'\n')
    print(f"{sum(c['lines'] for c in result['cases'])} native default-temperature line circuits passed")
