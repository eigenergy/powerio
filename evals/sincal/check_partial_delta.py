#!/usr/bin/env python3
"""Check installed delta-delta coils against independent OpenDSS single-phase
transformers and symmetric core shunts. Native model files remain external.
This validates component mapping, not complete feeders or native SINCAL execution.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def primitive(raw, windings, clock):
    v1, v2, kva = raw['Un1'], raw['Un2'], raw['Sn'] * 1000
    r, x = raw['ur'], np.sqrt(raw['uk'] ** 2 - raw['ur'] ** 2)
    core = complex(raw['Vfe'] * 1000, -np.sqrt((raw['i0'] / 100 * kva * 1000) ** 2 - (raw['Vfe'] * 1000) ** 2))
    result = np.zeros((6, 6), dtype=complex)
    for w in windings:
        endpoints = [w, (w + 1) % 3]
        secondary = endpoints if clock == 0 else endpoints[::-1]
        dss(f'clear\nnew circuit.check basekv={v1}\n'
            f'new transformer.subject phases=1 windings=2 buses=[h.1.2 l.1.2] '
            f'conns=[wye wye] kvs=[{v1} {v2}] kvas=[{kva/3} {kva/3}] '
            f'%rs=[{r/2} {r/2}] xhl={x:.17g} %noloadloss=0 %imag=0 ppm_antifloat=0\n'
            '~ wdg=1 rneut=-1\n~ wdg=2 rneut=-1\nsolve')
        dss.Circuit.SetActiveElement('Transformer.subject')
        vector = np.asarray(dss.CktElement.YPrim())
        y = (vector[::2] + 1j * vector[1::2]).reshape(4, 4, order='F')
        indices = endpoints + [3 + p for p in secondary]
        result[np.ix_(indices, indices)] += y
        # SINCAL's documented nominal pi circuit divides excitation equally
        # across both ports; model those physical branches separately in DSS.
        for side, voltage in enumerate([v1, v2]):
            admittance = core / (6 * (voltage * 1000) ** 2)
            if admittance == 0:
                continue
            impedance = 1 / admittance
            dss(f'new reactor.core{side} phases=1 bus1=c{side}.1 bus2=c{side}.2 '
                f'r={impedance.real:.17g} x={impedance.imag:.17g}\nsolve')
            dss.Circuit.SetActiveElement(f'Reactor.core{side}')
            vector = np.asarray(dss.CktElement.YPrim())
            y = (vector[::2] + 1j * vector[1::2]).reshape(2, 2, order='F')
            indices = [side * 3 + p for p in endpoints]
            result[np.ix_(indices, indices)] += y
    return result


def validate_case(case, raw, windings, clock, ports=None):
    if case['windings'] != windings or case['clock'] != clock:
        raise ValueError('wrong native winding selection or polarity')
    actual = np.asarray(case['y_re']) + 1j * np.asarray(case['y_im'])
    expected = primitive(raw, windings, clock)
    if actual.shape != (6, 6) or not np.all(np.isfinite(actual)):
        raise ValueError('invalid primitive')
    error = float(np.max(abs(actual - expected)) / np.max(abs(expected)))
    if error >= 1e-11:
        raise ValueError(f'OpenDSS primitive mismatch: {error}')
    active = sorted({p for w in windings for p in (w, (w + 1) % 3)})
    indices = active + [3 + p for p in active]
    names = [f'{side}{p+1}' for side in ('p', 's') for p in active]
    shunt = case['shunt']
    if shunt['terminal_map'] != names:
        raise ValueError('wrong primitive conductor map')
    reduced = np.asarray(shunt['g']) + 1j * np.asarray(shunt['b'])
    if reduced.shape != (len(indices), len(indices)) or not np.allclose(reduced, actual[np.ix_(indices, indices)], rtol=0, atol=1e-12):
        raise ValueError('compact primitive differs')
    switches = case['switches']
    if len(switches) != 2:
        raise ValueError('two native ports required')
    for side, switch in enumerate(switches):
        if switch['terminal_map_from'] != [str(p+1) for p in active] or switch['terminal_map_to'] != names[side*len(active):(side+1)*len(active)] or switch['bus_to'] != shunt['bus']:
            raise ValueError('wrong coil endpoint switch mapping')
        if ports is not None and (switch['bus_from'] != str(ports[side]['Node_ID']) or switch['open'] != (ports[side]['Flag_State'] == 0)):
            raise ValueError('native port identity or state differs')
    return error


def check(export, records, source, case_number=6):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identity = next(c for c in manifest['cases'] if c['case'] == case_number)
    if digest(records) != identity['record_sha256'] or digest(source) != identity['source_sha256']:
        raise ValueError('native identity mismatch')
    tables = {t['name']: [dict(zip([c['name'] for c in t['columns']], r, strict=True)) for r in t['rows']] for t in json.loads(records.read_text())['tables']}
    selections = {1:[0], 2:[1], 3:[2], 4:[0,1], 5:[1,2], 6:[2,0]}
    rows = json.loads(export.read_text())
    original = dict(Un1=11, Un2=.4, Sn=.1, ur=3, uk=5, Vfe=1, i0=2)
    if len(rows['synthetic']) != 12:
        raise ValueError('all synthetic winding/polarity combinations required')
    errors = []
    for c, (clock, winding) in zip(rows['synthetic'], [(clock, winding) for clock in (0, 6) for winding in selections.values()], strict=True):
        errors.append(validate_case(c, original, winding, clock))
    native = {r['Element_ID']:r for r in tables['TwoWindingTransformer'] if r['Variant_ID'] == 1 and r['VecGrp'] in [1,35] and any(t['Element_ID'] == r['Element_ID'] and t['Variant_ID'] == 1 and t['Flag_Terminal'] != 7 for t in tables['Terminal'])}
    if {c['element'] for c in rows['native']} != set(native):
        raise ValueError('incomplete native partial delta accounting')
    passed, rejected = [], []
    for c in rows['native']:
        eid = c['element']; r = native[eid]
        if 'error' in c:
            if r['Vfe'] * 1000 <= r['i0'] / 100 * r['Sn'] * 1e6 or 'real component exceeds stated magnitude' not in c['error']:
                raise ValueError('unexpected native rejection')
            rejected.append(eid)
            continue
        ports = sorted([t for t in tables['Terminal'] if t['Variant_ID'] == 1 and t['Element_ID'] == eid], key=lambda p:p['TerminalNo'])
        if len(ports) != 2 or ports[0]['Flag_Terminal'] != ports[1]['Flag_Terminal']:
            raise ValueError('inconsistent native winding selection')
        errors.append(validate_case(c, r, selections[ports[0]['Flag_Terminal']], 0 if r['VecGrp'] == 1 else 6, ports))
        passed.append(eid)
    controls = 0
    c = rows['synthetic'][0]
    for change in ['scale', 'phase', 'ground', 'polarity']:
        bad = copy.deepcopy(c)
        if change == 'scale': bad['y_re'] = (np.asarray(bad['y_re']) * 3).tolist()
        elif change == 'phase': bad['switches'][0]['terminal_map_from'] = ['1', '3']
        elif change == 'ground': bad['y_re'][0][0] += 1
        else: bad['clock'] = 6
        try: validate_case(bad, original, [0], 0)
        except ValueError: controls += 1
        else: raise ValueError('corrupted mapping accepted')
    return dict(scope=__doc__, case=case_number, synthetic_cases=12, native_passed=passed, native_rejected_core_inputs=rejected,
                maximum_relative_primitive_error=max(errors), tolerance=1e-11, negative_controls=controls,
                source_sha256=digest(source), record_sha256=digest(records), export_sha256=digest(export),
                collection=manifest['collection'], attribution=manifest['attribution'], license=manifest['license'],
                engine=dss.Basic.Version(), numpy=np.__version__, native_execution=False, passed=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['export', 'records', 'source', 'report']:
        parser.add_argument(name, type=Path)
    parser.add_argument("--case", type=int, default=6)
    args = parser.parse_args()
    report = check(args.export, args.records, args.source, args.case)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
