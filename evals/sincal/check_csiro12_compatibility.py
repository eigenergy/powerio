#!/usr/bin/env python3
"""Check original CSIRO12 through the public reader's explicit compatibility option.
External native files are never modified or vendored. OpenDSS is built directly
from acquired native inputs; the PowerIO output is solved independently by MNA.
This validates the assumed circuit, not undocumented native NULL semantics.
"""
import argparse
import copy
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from opendssdirect import dss
from check_csiro09_network import EPS, native_reference, solve_typed, tables

SOURCE_SHA256 = '7339548baf8bfcf5b891b9d3dbb6b616d9234b8f3df298d959904e150b8cfe91'
RECORD_SHA256 = '9861c2618515d42a972141097d8d177fa6364349e38a49a44af25d17af4f9f4e'
ASSUMED_FIELDS = ['Flag_LfLimit', 'Flag_LfCtrl', 'Flag_Qctrl', 'Flag_Macro', 'Kr']


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compare(net, native, hours, stress=False):
    net = copy.deepcopy(net)
    if stress:
        for load in net['loads']:
            for field in ['p_nom', 'q_nom']:
                load[field] = [v*f for v, f in zip(load[field], [2, .5, 1], strict=True)]
    expected_lines = {str(r['Element_ID']) for r in native['Line']}
    actual_lines = {r['name'] for r in net['lines']} | {
        r['name'] for r in net['switches'] if r['name'].isdigit()}
    if expected_lines != actual_lines or len(expected_lines) != 187:
        raise ValueError('native line identity coverage differs')
    if {r['name'] for r in net['loads']} != {str(r['Element_ID']) for r in native['Load']}:
        raise ValueError('native load identity coverage differs')
    if {r['value']['name'] for r in net['sources']} != {'181'}:
        raise ValueError('native source identity coverage differs')
    if {r['name'] for r in net['shunts'] if r['name'].isdigit()} != {'180'}:
        raise ValueError('native capacitor identity coverage differs')
    unused = {r['id'] for r in net['extras']['sincal_unconnected_nodes']}
    if unused != {'35', '153', '173'}:
        raise ValueError('unexpected unused native nodes')
    expected_nodes = {str(r['Node_ID']) for r in native['Node']} - unused
    if {b['id'] for b in net['buses'] if b['id'].isdigit()} != expected_nodes:
        raise ValueError('native electrical node identity coverage differs')
    assumptions = net['extras']['sincal_compatibility_assumptions']
    if assumptions['components'] != {'Infeeder.181': ASSUMED_FIELDS} or assumptions['assumed_value'] != 0:
        raise ValueError('compatibility assumptions lost')
    typed, residual, iterations = solve_typed(net)
    reference, powers, deck_hash = native_reference(native, hours, stress,
                                                   assume_inactive_source_controls=True)
    keys = {(n, phase) for n in expected_nodes for phase in ['1', '2', '3']}
    if set(reference) != keys or not keys <= set(typed):
        raise ValueError('native node/phase coverage differs')
    voltage_error = max(abs(typed[k] - reference[k]) for k in keys)
    power_error = max(abs(sum(load[field]) - powers[load['name']][i])
                      for load in net['loads'] for i, field in enumerate(['p_nom', 'q_nom']))
    # DSS needs a small positive-sequence source impedance. Quantify its effect
    # separately from the mapping, using the same boundary in the MNA solve.
    matched, matched_residual, _ = solve_typed(net, complex(EPS, EPS))
    matched_error = max(abs(matched[k] - reference[k]) for k in keys)
    current_error = terminal_power_error = 0.0
    codes = {c['name']: c for c in net['linecodes']}
    for line in net['lines']:
        code = codes[line['linecode']]
        y = np.linalg.inv((np.asarray(code['r_series']) + 1j*np.asarray(code['x_series']))*line['length'])
        vf, vt = [np.array([matched[line['bus_'+side], t] for t in line['terminal_map_'+side]])
                  for side in ['from', 'to']]
        sh = [(np.asarray(code['g_'+side]) + 1j*np.asarray(code['b_'+side]))*line['length']
              for side in ['from', 'to']]
        currents = np.r_[y@(vf-vt)+sh[0]@vf, y@(vt-vf)+sh[1]@vt]
        if dss.Circuit.SetActiveElement('Line.e'+line['name']) < 0:
            raise ValueError('missing DSS line')
        raw = np.asarray(dss.CktElement.Currents())
        expected_i = raw[::2]+1j*raw[1::2]
        raw_p = np.asarray(dss.CktElement.Powers())
        expected_s = (raw_p[::2]+1j*raw_p[1::2])*1000
        current_error = max(current_error, float(np.max(abs(currents-expected_i))))
        terminal_power_error = max(terminal_power_error, float(np.max(abs(np.r_[vf, vt]*np.conj(currents)-expected_s))))
    if voltage_error >= 1e-3 or matched_error >= 1e-4 or power_error >= 1e-7 or current_error >= 1e-4 or terminal_power_error >= 2:
        raise ValueError(f'comparison failed: volts={voltage_error}, matched={matched_error}, loadVA={power_error}, amps={current_error}, terminalVA={terminal_power_error}')
    return {'hours': hours, 'synthetic_unequal_delta_branch_stress': stress,
            'native_nodes_compared': 188, 'complex_voltages_compared': len(keys),
            'line_terminal_currents_and_powers_compared': len(net['lines'])*6,
            'maximum_voltage_error_v': float(voltage_error),
            'matched_source_voltage_error_v': float(matched_error),
            'maximum_line_terminal_current_error_a': current_error,
            'maximum_line_terminal_power_error_va': terminal_power_error,
            'maximum_load_power_error_va': power_error,
            'typed_kcl_residual_a': residual, 'matched_kcl_residual_a': matched_residual,
            'typed_iterations': iterations, 'deck_sha256': deck_hash}


def validate(records, source, reader):
    if digest(records) != RECORD_SHA256 or digest(source) != SOURCE_SHA256:
        raise ValueError('external source/acquisition identity differs')
    native = tables(records)
    results, consumers, controls = [], [], []
    strict = subprocess.run([str(reader), str(source), str(records), '12'],
                            capture_output=True, text=True, timeout=120)
    if strict.returncode == 0 or 'Flag_LfLimit' not in strict.stderr:
        raise ValueError('strict reader did not reject NULL source controls')
    for hours in [0, .25, 12, 23.75, 24]:
        run = subprocess.run([str(reader), str(source), str(records), str(hours),
                              '--assume-inactive-source-controls'],
                             capture_output=True, text=True, check=True, timeout=120)
        net = json.loads(run.stdout)
        consumer = json.loads(run.stderr)
        codes = {d['code'] for d in consumer['reader_diagnostics']}
        if consumer['generic_matrix_diagnostics'] != 0 or consumer['power_flow_instance'] != {'constructed': True}:
            raise ValueError('generic consumer failed')
        if not {'READ.DIST.SINCAL_ASSUMED_INACTIVE_SOURCE_CONTROLS', 'READ.DIST.SINCAL_UNCONNECTED_NODES'} <= codes:
            raise ValueError('public diagnostic missing')
        consumers.append({'hours': hours, **consumer})
        for stress in [False, True]:
            results.append(compare(net, native, hours, stress))
        if hours == 12:
            original = net
    for mutation in ['missing-load', 'missing-capacitor', 'missing-mutual-impedance', 'wrong-profile-factor']:
        bad = copy.deepcopy(original)
        if mutation == 'missing-load': bad['loads'].pop()
        elif mutation == 'missing-capacitor': bad['shunts'] = [s for s in bad['shunts'] if s['name'] != '180']
        elif mutation == 'wrong-profile-factor':
            for load in bad['loads']: load['p_nom'] = [p*.9 for p in load['p_nom']]
        else:
            for code in bad['linecodes']:
                for field in ['r_series', 'x_series']:
                    code[field] = [[v if i == j else 0 for j, v in enumerate(row)] for i, row in enumerate(code[field])]
        try: compare(bad, native, 12)
        except ValueError as error: controls.append({'mutation': mutation, 'rejected': True, 'reason': str(error)})
        else: raise ValueError(f'checker accepted {mutation}')
    return {'case': 'CSIRO12', 'status': 'passed with explicit compatibility assumptions',
            'source_sha256': SOURCE_SHA256, 'record_sha256': RECORD_SHA256,
            'reader_sha256': digest(reader), 'native_element_count': 215,
            'native_electrical_node_count': 188, 'unused_node_ids_preserved_in_extras': ['35', '153', '173'],
            'original_source_echo_and_ir_verified': True, 'native_sincal_executed': False,
            'native_null_control_semantics_verified': False,
            'native_loads_are_symmetric': True, 'synthetic_stress_is_not_a_published_unbalanced_case': True,
            'assumed_inactive_source_fields': ASSUMED_FIELDS,
            'tolerances': {'voltage_v': .001, 'matched_source_voltage_v': .0001, 'line_terminal_current_a': .0001, 'line_terminal_power_va': 2, 'load_power_va': 1e-7},
            'opendss_source_r1_x1_ohm': EPS,
            'engine': dss.Basic.Version(), 'numpy': np.__version__,
            'attribution': 'Berry, Collins, Oliver and Perfumo (2015), Representative Australian Electricity Feeders with load and solar generation profiles, v1, CSIRO.',
            'license': 'CC BY 4.0; external source models are not vendored',
            'cases': results, 'generic_consumers': consumers, 'negative_controls': controls}


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    for name in ['records', 'source', 'reader', 'report']: p.add_argument(name, type=Path)
    a = p.parse_args()
    report = validate(a.records, a.source, a.reader)
    a.report.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'status': report['status'], 'snapshots_and_stress_cases': len(report['cases']),
                      'max_voltage_error_v': max(r['maximum_voltage_error_v'] for r in report['cases'])}))
