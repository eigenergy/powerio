#!/usr/bin/env python3
"""Check original unequal-phase relative-profile cases against OpenDSS.

NumPy samples the independently specified factor curve. Separate single-phase
OpenDSS loads preserve each Wye or delta branch, including capacitive demand.
The actual Rust output supplies the circuit being tested, never oracle inputs.
This is synthetic component validation, not additional native corpus coverage.
"""
import argparse
import copy
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

from audit_distribution import digest

TIMES = [0.0, 3.0, 6.0, 12.0, 18.0, 24.0, 30.0]
MODELS = {1: ('constant_impedance', 2, 2),
          2: ('constant_power', 1, 0),
          3: ('constant_current', 5, 1)}


def vector(raw):
    a = np.asarray(raw)
    return a[::2] + 1j*a[1::2]


def validate(case):
    mode, model, hours = (case[k] for k in ('mode','model','hours'))
    if mode not in (13,14) or model not in MODELS or hours not in TIMES:
        raise ValueError('unexpected original test case')
    wye = mode == 13
    phases = ['1','2','3','0'] if wye else ['1','2','3']
    load, bus = case['load'], case['bus']
    name, dss_model, exponent = MODELS[model]
    factor = float(np.interp(hours % 24, [0,12,24], [0,2,0]))
    nominal = 400/np.sqrt(3) if wye else 400
    expected_pq = np.array([[6000,12000,18000],[-3000,6000,9000]])*factor
    if (load['configuration'] != ('wye' if wye else 'delta')
            or load['terminal_map'] != phases or bus['terminals'] != phases
            or bus['grounded'] != (['0'] if wye else []) or load['bus'] != bus['id']
            or load['voltage_model']['model'] != name):
        raise ValueError('changed phase, grounding or voltage-model semantics')
    vm = np.asarray(load['voltage_model']['v_nom'])
    pq = np.asarray([load['p_nom'],load['q_nom']])
    if (vm.shape != (3,) or pq.shape != (2,3) or not np.all(np.isfinite(pq))
            or not np.all(np.isfinite(vm)) or np.max(abs(vm-nominal)) > 1e-10):
        raise ValueError('invalid nominal voltage or power shape')
    error_pq = float(np.max(abs(pq-expected_pq)))
    if error_pq > 1e-8:
        raise ValueError('relative scaling changed phase powers')
    if load['extras']['sincal_profile'] != dict(profile=7,requested_hours=hours,
            cyclic_hours=hours%24,period_hours=24,relative_factor=factor):
        raise ValueError('incorrect relative profile provenance')
    dss('Clear\nNew Circuit.relative basekv=0.4 bus1=test phases=3 pu=1.03')
    pairs = [(1,0),(2,0),(3,0)] if wye else [(1,2),(2,3),(3,1)]
    for k,(a,b) in enumerate(pairs):
        p,q = expected_pq[:,k]/1000
        dss(f'New Load.branch{k} phases=1 bus1=test.{a}.{b} '
            f'conn={"wye" if wye else "delta"} kv={nominal/1000:.17g} '
            f'kw={p:.17g} kvar={q:.17g} model={dss_model} status=fixed vminpu=0.5 vmaxpu=1.5')
    dss('Set tolerance=1e-12\nSolve')
    if not dss.Solution.Converged():
        raise ValueError('independent load circuit did not converge')
    maximum = 0.0
    for k,pair in enumerate(pairs):
        dss.Circuit.SetActiveElement(f'Load.branch{k}')
        if dss.CktElement.NodeOrder() != list(pair):
            raise ValueError('OpenDSS branch order mismatch')
        v = vector(dss.CktElement.Voltages())
        current = vector(dss.CktElement.Currents())
        voltage = v[0]-v[1]
        actual = np.conj(complex(*pq[:,k])/voltage)*(abs(voltage)/vm[k])**exponent
        error = float(np.max(abs(np.array([actual,-actual])-current)))
        if not np.isfinite(error) or error > 1e-8:
            raise ValueError(f'branch current mismatch {error}')
        maximum = max(maximum,error)
    return dict(mode=mode,model=model,hours=hours,relative_factor=factor,
                maximum_power_error_w_var=error_pq,maximum_terminal_current_error_a=maximum)


def check(export):
    cases = json.loads(export.read_text())
    required = {(mode,model,h) for mode in (13,14) for model in MODELS for h in TIMES}
    if len(cases) != len(required) or {(c['mode'],c['model'],c['hours']) for c in cases} != required:
        raise ValueError('incomplete or duplicated synthetic case inventory')
    results = [validate(c) for c in cases]
    negatives = 0
    for mode in (13,14):
        original = next(c for c in cases if (c['mode'],c['model'],c['hours']) == (mode,2,3))
        for mutation in ('double_factor','phase_average','grounding','voltage_model','provenance'):
            bad = copy.deepcopy(original)
            if mutation == 'double_factor': bad['load']['p_nom'] = [v*.5 for v in bad['load']['p_nom']]
            elif mutation == 'phase_average': bad['load']['p_nom'] = [6000]*3
            elif mutation == 'grounding': bad['bus']['grounded'] = [] if mode == 13 else ['0']
            elif mutation == 'voltage_model': bad['load']['voltage_model']['model'] = 'constant_current'
            else: bad['load']['extras']['sincal_profile']['relative_factor'] = 1
            try: validate(bad)
            except ValueError: negatives += 1
            else: raise ValueError(f'corruption not detected: {mutation}')
    return dict(scope=__doc__,cases=results,negative_controls=negatives,
        maximum_power_error_w_var=max(r['maximum_power_error_w_var'] for r in results),
        maximum_terminal_current_error_a=max(r['maximum_terminal_current_error_a'] for r in results),
        current_tolerance_a=1e-8,export_sha256=digest(export),engine=dss.Basic.Version(),
        numpy=np.__version__,native_execution=False,native_corpus_coverage_added=False,passed=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('export',type=Path);parser.add_argument('report',type=Path)
    args = parser.parse_args();report = check(args.export)
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(f"{len(report['cases'])} relative-profile snapshots and {report['negative_controls']} negative controls passed")
