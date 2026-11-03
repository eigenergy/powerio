#!/usr/bin/env python3
"""Independent checks of all European LV line/load components, not a feeder solve.

Native source/transformer zero-sequence data are undeclared. Original models
remain external because inherited redistribution rights are unresolved.
"""
import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

SOURCE_SHA = '7565a1ab584d44ba2305a40ba9925bd043231bcd05668d6b8a700a69bf1baa15'
RECORD_SHA = 'c0965a1e08f9bf578451776d91448f59ed1bb9396a7565b594b04a4d606dd1f8'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def complex_array(values):
    a = np.asarray(values)
    return a[::2] + 1j*a[1::2]


def check(records, source, exported):
    document = json.loads(records.read_text())
    if (digest(source) != SOURCE_SHA or document['source']['sha256'] != SOURCE_SHA
            or digest(records) != RECORD_SHA):
        raise ValueError('original source identity mismatch')
    tables = {t['name']: [dict(zip([c['name'] for c in t['columns']], row, strict=True))
                          for row in t['rows']] for t in document['tables']}
    if tables['Version'][0]['Version_No'] != 12.8:
        raise ValueError('native schema must not be relabeled')
    ports = {}
    for port in tables['Terminal']:
        if port['Variant_ID'] == 1:
            ports.setdefault(port['Element_ID'], []).append(port)
    for values in ports.values():
        values.sort(key=lambda p: p['TerminalNo'])
    lines = {r['Element_ID']: r for r in tables['Line'] if r['Variant_ID'] == 1}
    loads = {r['Element_ID']: r for r in tables['Load'] if r['Variant_ID'] == 1}
    elements = {r['Element_ID']: r for r in tables['Element'] if r['Variant_ID'] == 1}
    nodes = {r['Node_ID']: r for r in tables['Node'] if r['Variant_ID'] == 1}
    levels = {r['VoltLevel_ID']: r for r in tables['VoltageLevel'] if r['Variant_ID'] == 1}
    output = json.loads(exported.read_text())
    if (len(output['lines']) != len(lines) or len(lines) != 205 or
            {c['element'] for c in output['lines']} != set(lines) or
            len(output['loads']) != len(loads) or len(loads) != 55 or
            {c['element'] for c in output['loads']} != set(loads)):
        raise ValueError('every native line/load must be present exactly once')
    line_error = current_error = power_error = 0.0
    for c in output['lines']:
        row, ps = lines[c['element']], ports[c['element']]
        line, code = c['line'], c['code']
        if (len(ps) != 2 or any(p['Flag_Terminal'] != 7 or p['Flag_State'] != 1 for p in ps)
                or elements[c['element']]['Flag_State'] != 1 or row['ParSys'] != 1
                or row['Ith'] != 0 or row['va'] != 0 or row['fn'] != 50
                or row['Flag_Z0_Input'] != 2):
            raise ValueError('unexpected native line profile')
        if (line['bus_from'] != str(ps[0]['Node_ID']) or line['bus_to'] != str(ps[1]['Node_ID'])
                or line['terminal_map_from'] != ['1','2','3'] or line['terminal_map_to'] != ['1','2','3']
                or c['switches'] or c['auxiliary_buses'] or code.get('i_max') is not None
                or line.get('i_max') is not None or line.get('s_max') is not None
                or line['linecode'] != code['name'] or not np.isclose(line['length'],row['l']*1000,rtol=1e-13,atol=0)):
            raise ValueError('line connectivity, length or absent rating changed')
        dss('Clear\nNew Circuit.check basekv=0.4 phases=3\n'
            f'New Line.subject bus1=a.1.2.3 bus2=b.1.2.3 phases=3 units=km length={row["l"]:.17g} '
            f'r1={row["r"]:.17g} x1={row["x"]:.17g} r0={row["r0"]:.17g} x0={row["x0"]:.17g} '
            f'c1={row["c"]:.17g} c0={row["c0"]:.17g} basefreq=50\nSet frequency=50\nSolve')
        dss.Circuit.SetActiveElement('Line.subject')
        expected = complex_array(dss.CktElement.YPrim()).reshape(6,6,order='F')
        z = (np.asarray(code['r_series'])+1j*np.asarray(code['x_series']))*line['length']
        y = np.linalg.inv(z)
        shunts = [(np.asarray(code['g_'+end])+1j*np.asarray(code['b_'+end]))*line['length']
                  for end in ['from','to']]
        actual = np.block([[y+shunts[0],-y],[-y,y+shunts[1]]])
        error = float(np.max(abs(actual-expected))/np.max(abs(expected)))
        if not np.all(np.isfinite(actual)) or not error < 1e-11:
            raise ValueError('line primitive differs from independent OpenDSS circuit')
        line_error = max(line_error,error)
    for c in output['loads']:
        row, ps = loads[c['element']], ports[c['element']]
        if len(ps) != 1:
            raise ValueError('load must have one native port')
        port = ps[0]; phase = str(port['Flag_Terminal'])
        level = levels[nodes[port['Node_ID']]['VoltLevel_ID']]
        voltage = level['Un']*1000/np.sqrt(3)*row['u']/100
        load,bus,switch = c['load'],c['bus'],c['switch']
        if (phase not in ['1','2','3'] or port['Flag_State'] != 1 or row['Flag_Lf'] != 11
                or row['Flag_LoadType'] != 2 or elements[c['element']]['Flag_State'] != 1
                or load['configuration'] != 'single_phase' or load['terminal_map'] != [phase,'0']
                or bus['grounded'] != ['0'] or bus['terminals'] != [phase,'0']
                or switch['open'] or switch['bus_from'] != str(port['Node_ID'])
                or switch['bus_to'] != bus['id'] or load['bus'] != bus['id']
                or switch['terminal_map_from'] != [phase] or switch['terminal_map_to'] != [phase]
                or load['voltage_model']['model'] != 'constant_power'
                or len(load['p_nom']) != 1 or len(load['q_nom']) != 1):
            raise ValueError('single-phase load connectivity or voltage dependence changed')
        dss('Clear\nNew Circuit.check phases=1 bus1=test.1 basekv=0.23094010767585033 pu=1.02\n'
            f'New Load.subject phases=1 bus1=test.1.0 conn=wye model=1 '
            f'kv={voltage/1000:.17g} kw={row["P"]*row["fP"]*1000:.17g} pf={row["cosphi"]:.17g}\nSolve')
        dss.Circuit.SetActiveElement('Load.subject')
        native_power = complex(dss.Loads.kW(),dss.Loads.kvar())*1000
        typed_power = complex(load['p_nom'][0],load['q_nom'][0])
        power_error = max(power_error,abs(native_power-typed_power))
        volts = complex_array(dss.CktElement.Voltages())
        currents = complex_array(dss.CktElement.Currents())
        actual_current = np.conj(typed_power/(volts[0]-volts[1]))
        error = max(abs(actual_current-currents[0]),abs(-actual_current-currents[1]))
        if not np.isfinite(error) or error >= 1e-8 or abs(native_power-typed_power) >= 1e-8:
            raise ValueError('load current or power differs from independent OpenDSS circuit')
        current_error = max(current_error,float(error))
    return {'source':'https://github.com/lik1212/Matlab2Sincal_LPC-Tool',
            'license':'Inherited model redistribution rights unresolved; external research only',
            'scope':__doc__, 'source_sha256':SOURCE_SHA,'record_sha256':digest(records),
            'export_sha256':digest(exported),'schema':12.8,'variant':1,
            'line_count':len(lines),'load_count':len(loads),
            'maximum_relative_line_primitive_error':line_error,
            'maximum_load_power_error_va':float(power_error),'maximum_load_current_error_a':current_error,
            'relative_line_tolerance':1e-11,'load_tolerance':1e-8,
            'complete_network':False,'native_execution':False,'historical_results_used':False,
            'remaining':'Source and transformer lack declared zero-sequence input; native automatic completion is disabled.',
            'engine':dss.Basic.Version(),'numpy':np.__version__,'passed':True}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['records','source','exported']:
        parser.add_argument(name,type=Path)
    parser.add_argument('--report',type=Path,required=True)
    args = parser.parse_args()
    result = check(args.records,args.source,args.exported)
    args.report.write_text(json.dumps(result,indent=2)+'\n')
    print('205 line primitives and 55 load components passed; complete network remains unresolved')
