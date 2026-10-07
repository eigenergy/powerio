"""Compare the external native IEEE18 reader output with a published paper.

Requires numpy, pandapower (for the existing solve_mapped helper's module), and
pdfplumber==0.11.9. Supply both original files externally; neither the paper nor
its tables/plot nor the unlicensed native model is redistributed here.

Milovanovic et al., IJEEC 2(1), DOI 10.7251/IJEEC1801011M:
Table A.I (printed p18), section IV.A (p13-14), Figure 8 (p15).
This is a balanced fundamental-frequency comparison, not harmonic validation.
"""
import argparse
from collections import defaultdict
import copy
import hashlib
import json
from pathlib import Path
import subprocess

import numpy as np
import pdfplumber

from check_balanced_simbench import solve_mapped

MODEL_SHA = '9978078e4c956cf6d2e740bf2411fd5076111c17bfbc564925b3f205e797e8e5'
PAPER_SHA = '5a7f8b5aae3b1bd6d2674a43c8ed90dc6189e298eccb2f0e82e1a3f2950f68f0'
PAPER_URL = 'https://doisrpska.nub.rs/index.php/IJEEC/article/download/5630/5453'
# Visually checked Figure 8 left-to-right labels. Its last bar aggregates the
# two parallel 25-26 lines; the input table lists both separately.
LABELS = '51-50 50-1 1-2 2-3 3-4 4-5 5-6 6-7 7-8 2-9 1-20 20-21 21-22 21-23 23-24 23-25 25-26'.split()
# Chosen from the ~0.2145 pt plot stroke on a 109.877 pt / 60 kW axis,
# with margin, not from the observed electrical residual. Figure precision
# does not justify treating digitized coordinates as exact tabulated values.
PLOT_TOLERANCE_KW = 0.15


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def edge(a, b):
    return tuple(sorted((a, b)))


def read_paper(path):
    require(hashlib.sha256(path.read_bytes()).hexdigest() == PAPER_SHA, 'unexpected paper PDF')
    with pdfplumber.open(path) as pdf:
        raw = pdf.pages[7].extract_tables()[0][2:]
        require(len(raw) == 18, 'Table A.I extraction changed')
        inputs = [(a, b, list(map(float, fields))) for a, b, *fields in raw]
        page = pdf.pages[4]
        bars = sorted((r for r in page.rects if r['fill']
            and r['non_stroking_color'] == (0.239258, 0.148926, 0.658203)
            and r['x0'] < 290 and 80 < r['top'] < 200), key=lambda r: r['x0'])
        require(len(bars) == len(LABELS), 'Figure 8 bar extraction changed')
        axes = [r for r in page.rects if r['fill'] and r['non_stroking_color'] == 1.0
                and abs(r['x0'] - 56.6273) < 0.001 and abs(r['top'] - 86.662) < 0.001]
        require(len(axes) == 1, 'Figure 8 axes extraction changed')
        axis = axes[0]
        require(all(abs(r['bottom'] - axis['bottom']) < 0.002 for r in bars), 'bar baseline changed')
        losses = {edge(*label.split('-')): r['height'] * 60 / axis['height']
                  for label, r in zip(LABELS, bars)}
    return inputs, losses


def bus_names(model):
    names = {b['id']: b['name'] for b in model['buses']}
    require(len(model['switches']) == 1 and model['switches'][0]['closed'], 'unexpected terminal switch')
    for switch in model['switches']:
        names[switch['to']] = names[switch['from']]
    return names


def check_inputs(model, inputs):
    names = bus_names(model)
    require(len(set(names.values())) == 18, 'unexpected bus labels')
    require(model['frequency'] == 50, 'expected fundamental-frequency model')
    require(all(b['base_kv'] == 12.5 for b in model['buses']), 'voltage base differs from paper')
    refs = [b for b in model['buses'] if b['kind'] == 'REF']
    require(len(refs) == 1 and names[refs[0]['id']] == '51'
            and refs[0]['vm'] == 1.05 and refs[0]['va'] == 0, 'slack differs from paper')
    require(len(model['generators']) == 1, 'unexpected additional generation')
    expected = defaultdict(list)
    loads = defaultdict(complex)
    shunts = defaultdict(float)
    for a, b, row in inputs:
        expected[edge(a, b)].append(row[:3])
        loads[b] += complex(row[3] + row[5], row[4] + row[6]) * 10
        shunts[b] += row[7] * 10
    require(len(model['branches']) == 18, 'line count differs from Table A.I')
    errors = []
    for branch in model['branches']:
        require(branch['in_service'] and not branch['shift'] and branch['tap'] in (0, 1)
                and branch['charging'] is None, 'unexpected line mode')
        key = edge(names[branch['from']], names[branch['to']])
        require(bool(expected[key]), f'unexpected line {key}')
        row = expected[key].pop()
        actual = [branch['r'] * 10 / model['base_mva'], branch['x'] * 10 / model['base_mva'],
                  branch['b'] * model['base_mva'] / 10]
        errors.extend(abs(a - b) for a, b in zip(actual, row))
    require(not any(expected.values()), 'unmatched paper lines')
    require(max(errors) < 1e-10, 'line inputs differ from paper')
    actual_loads = defaultdict(complex)
    for load in model['loads']:
        require(load['in_service'] and load['voltage_model'] is None, 'unexpected load mode')
        actual_loads[names[load['bus']]] += complex(load['p'], load['q'])
    load_error = max(abs(actual_loads[n] - loads[n]) for n in set(names.values()))
    require(load_error < 1e-10 and len(model['loads']) == 15, 'loads differ from paper')
    actual_shunts = defaultdict(float)
    for shunt in model['shunts']:
        require(shunt['in_service'] and shunt['g'] == 0, 'unexpected shunt mode')
        actual_shunts[names[shunt['bus']]] += shunt['b']
    shunt_error = max(abs(actual_shunts[n] - shunts[n]) for n in set(names.values()))
    require(shunt_error < 1e-10 and len(model['shunts']) == 10, 'shunts differ from paper')
    return {'lines': 18, 'loads': 15, 'shunts': 10, 'max_line_parameter_error_on_10_mva_base': max(errors),
            'max_bus_load_error_mva': load_error, 'max_bus_shunt_error_mvar': shunt_error}


def solve_losses(model):
    names = bus_names(model)
    voltages, iterations, mismatch = solve_mapped(model, power_tolerance=1e-12)
    voltage = {b['id']: v for b, v in zip(model['buses'], voltages)}
    losses = defaultdict(float)
    for b in model['branches']:
        current = (voltage[b['from']] - voltage[b['to']]) / complex(b['r'], b['x'])
        losses[edge(names[b['from']], names[b['to']])] += abs(current)**2 * b['r'] * model['base_mva'] * 1000
    return dict(losses), iterations, mismatch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('model', type=Path)
    parser.add_argument('paper', type=Path)
    parser.add_argument('inspector', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    require(hashlib.sha256(args.model.read_bytes()).hexdigest() == MODEL_SHA, 'unexpected native IEEE18 file')
    model = json.loads(subprocess.run([str(args.inspector.resolve()), str(args.model.resolve())],
        capture_output=True, text=True, check=True, timeout=60).stdout)
    inputs, published = read_paper(args.paper)
    input_report = check_inputs(model, inputs)
    actual, iterations, mismatch = solve_losses(model)
    require(actual.keys() == published.keys(), 'branch loss groups differ from Figure 8')
    error = max(abs(actual[k] - published[k]) for k in published)
    require(error < PLOT_TOLERANCE_KW, f'Figure 8 disagreement: {error} kW')
    perturbed = copy.deepcopy(model)
    for load in perturbed['loads']:
        load['p'] *= 1.1
        load['q'] *= 1.1
    changed, _, _ = solve_losses(perturbed)
    control_error = max(abs(changed[k] - published[k]) for k in published)
    require(control_error > PLOT_TOLERANCE_KW, '10% load perturbation was not detected')
    report = {'case': 'IEEE18', 'profile': 'balanced', 'source_sha256': MODEL_SHA,
        'paper_doi': '10.7251/IJEEC1801011M', 'paper_url': PAPER_URL, 'paper_sha256': PAPER_SHA,
        'reference': 'Table A.I, section IV.A, Figure 8; printed pages 13-15 and 18',
        'input_alignment': input_report, 'branch_loss_groups_checked': len(published),
        'max_plot_loss_difference_kw': error, 'plot_tolerance_kw': PLOT_TOLERANCE_KW,
        'computed_total_fundamental_line_loss_kw': sum(actual.values()),
        'nodal_iterations': iterations, 'nodal_power_mismatch_pu': mismatch,
        'negative_control_10_percent_load_change_max_error_kw': control_error,
        'numpy_version': np.__version__, 'pdfplumber_version': pdfplumber.__version__,
        'status': 'passed at figure precision', 'stored_native_result_tables_used': False,
        'native_sincal_executed': False, 'new_unbalanced_case': False,
        'limitations': ['Independent of native result dumps; solves the actual typed balanced reader output.',
            'Digitized vector bars are approximate published graphical results, not exact numeric oracles.',
            'No harmonic validation; the paper total including harmonics is not the quantity compared.',
            'Neither native model nor PDF nor extracted input table/plot is redistributed.']}
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
