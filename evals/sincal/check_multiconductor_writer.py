#!/usr/bin/env python3
"""Validate fresh multiconductor writer output using independent circuit solves.

The Rust exporter creates typed circuits from original synthetic parameters,
then writes fresh SQLite and reads it back. Dense MNA here stamps both typed
circuits independently of PowerIO's matrix code. A separate OpenDSS circuit is
built from the original physical parameters, never from fresh native records.
This checks electrical equivalence; it is not native SINCAL acceptance.
"""
import argparse
import cmath
import copy
import hashlib
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

EPS_OHM = 1e-6
VOLTAGE_TOLERANCE = 1e-3


def source_fields(source):
    if source.get('type') == 'powerio.ReferencedVoltageSource':
        return source['value']
    return source


def solve(net):
    ground = {(b['id'], t) for b in net['buses'] for t in b['grounded']}
    terminals = [(b['id'], t) for b in net['buses'] for t in b['terminals']
                 if (b['id'], t) not in ground]
    index = {terminal: i for i, terminal in enumerate(terminals)}
    n = len(index)
    y = np.zeros((n, n), dtype=complex)

    def incidence(a, b=None):
        row = np.zeros(n)
        if a not in ground:
            row[index[a]] += 1
        if b is not None and b not in ground:
            row[index[b]] -= 1
        return row

    codes = {c['name']: c for c in net['linecodes']}
    for line in net['lines']:
        c = codes[line['linecode']]
        z = (np.array(c['r_series']) + 1j*np.array(c['x_series']))*line['length']
        a = np.array([incidence((line['bus_from'], f), (line['bus_to'], t))
                      for f, t in zip(line['terminal_map_from'], line['terminal_map_to'], strict=True)])
        y += a.T @ np.linalg.inv(z) @ a
        for side, bus in [('from', line['bus_from']), ('to', line['bus_to'])]:
            a = np.array([incidence((bus, t)) for t in line[f'terminal_map_{side}']])
            shunt = (np.array(c[f'g_{side}']) + 1j*np.array(c[f'b_{side}']))*line['length']
            y += a.T @ shunt @ a
    for load in net['loads']:
        if load['voltage_model']['model'] != 'constant_impedance':
            raise ValueError('this linear oracle requires constant-impedance loads')
        t = load['terminal_map']
        if load['configuration'] == 'wye':
            pairs = [(phase, t[-1]) for phase in t[:-1]]
        elif len(load['p_nom']) == 1:
            pairs = [t]
        else:
            pairs = [(t[i], t[(i+1) % len(t)]) for i in range(len(t))]
        for pair, p, q, v in zip(pairs, load['p_nom'], load['q_nom'], load['voltage_model']['v_nom'], strict=True):
            a = incidence((load['bus'], pair[0]), (load['bus'], pair[1]))
            y += complex(p, -q)/v**2 * np.outer(a, a)
    constraints, values = [], []
    for switch in net['switches']:
        if not switch['open']:
            for f, t in zip(switch['terminal_map_from'], switch['terminal_map_to'], strict=True):
                constraints.append(incidence((switch['bus_from'], f), (switch['bus_to'], t)))
                values.append(0j)
    for wrapped in net['sources']:
        source = source_fields(wrapped)
        reference = source.get('reference_terminal')
        for terminal, voltage, angle in zip(source['terminal_map'], source['v_magnitude'], source['v_angle'], strict=True):
            constraints.append(incidence((source['bus'], terminal), None if reference is None else (source['bus'], reference)))
            values.append(cmath.rect(voltage, angle))
    if any(net.get(key) for key in ('transformers', 'shunts', 'generators', 'capacitors', 'ibrs', 'untyped')):
        raise ValueError('unexpected electrical components in writer oracle')
    c = np.array(constraints)
    matrix = np.block([[y, c.T], [c, np.zeros((len(c), len(c)))]])
    rhs = np.r_[np.zeros(n), values]
    solution = np.linalg.solve(matrix, rhs)
    residual = float(np.max(np.abs(matrix@solution-rhs)))
    if residual > 1e-8:
        raise ValueError(f'MNA residual {residual}')
    result = {key: solution[i] for key, i in index.items()}
    result.update({key: 0j for key in ground})
    return result, residual


def opendss_reference(floating):
    # All electrical parameters are the original synthetic construction inputs.
    # The tiny source impedance is an explicit engine approximation to the
    # typed ideal boundary; it does not replace any native source parameter.
    back = 'supply.4.4.4' if floating else 'supply.0.0.0'
    deck = ['Clear', 'Set DefaultBaseFrequency=50',
            f'New Circuit.writer phases=3 bus1=supply.1.2.3 bus2={back} basekv={230*np.sqrt(3)/1000:.17g} pu=1 angle=0 frequency=50',
            f'Edit Vsource.source r1={EPS_OHM} x1={EPS_OHM} r0={EPS_OHM} x0={EPS_OHM}',
            'New Linecode.cable nphases=3 units=m basefreq=50 '
            'rmatrix=[0.0004 | 0.0001 0.0004 | 0.0001 0.0001 0.0004] '
            'xmatrix=[0.0003 | 0.0001 0.0003 | 0.0001 0.0001 0.0003] '
            'cmatrix=[0 | 0 0 | 0 0 0]',
            'New Line.feeder bus1=supply.1.2.3 bus2=load.1.2.3 phases=3 linecode=cable length=120 units=m']
    for i, (p, q) in enumerate(zip([1, 2, 3], [.2, -.1, .7], strict=True), 1):
        deck.append(f'New Load.phase{i} phases=1 bus1=load.{i}.0 conn=wye kv=0.23 kw={p} kvar={q} model=2 status=fixed')
    deck += ['Set controlmode=off tolerance=1e-12 maxiterations=100', 'Solve']
    text = '\n'.join(deck)+'\n'
    dss(text)
    if not dss.Solution.Converged():
        raise ValueError('OpenDSS did not converge')
    result = {}
    for bus in ['supply', 'load']:
        dss.Circuit.SetActiveBus(bus)
        raw = dss.Bus.Voltages()
        for terminal, value in zip(dss.Bus.Nodes(), (complex(a,b) for a,b in zip(raw[::2], raw[1::2], strict=True)), strict=True):
            result[bus, str(terminal)] = value
    return result, hashlib.sha256(text.encode()).hexdigest()


def check(path):
    document = json.loads(path.read_text())
    original, original_residual = solve(document['input'])
    recovered, recovered_residual = solve(document['fresh_readback'])
    differences = []
    for bus, native in document['bus_ids'].items():
        for phase in ('1', '2', '3'):
            if (bus, phase) in original:
                differences.append(abs(original[bus, phase]-recovered[str(native), phase]))
    mapped_error = float(max(differences))
    reference, deck_hash = opendss_reference(path.stem == 'floating')
    engine_error = float(max(abs(original[bus, phase]-reference[bus, phase])
                            for bus in ('supply', 'load') for phase in ('1', '2', '3')))
    counterexample = None
    if path.stem == 'floating':
        wrong = copy.deepcopy(document['fresh_readback'])
        source = source_fields(wrong['sources'][0])
        bus = next(b for b in wrong['buses'] if b['id'] == source['bus'])
        bus['grounded'].append(source['reference_terminal'])
        wrong_volts, _ = solve(wrong)
        counterexample = float(max(abs(wrong_volts[key]-recovered[key])
                                   for key in recovered if key in wrong_volts))
        if counterexample < 1:
            raise ValueError('floating-source oracle does not detect an incorrect earth connection')
    return {'case': path.stem, 'grounded_reference_counterexample_error_v': counterexample,
            'export_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'opendss_deck_sha256': deck_hash, 'fresh_readback_voltage_error_v': mapped_error,
            'opendss_voltage_error_v': engine_error, 'original_mna_residual': original_residual,
            'fresh_mna_residual': recovered_residual,
            'passed': mapped_error < 1e-8 and engine_error < VOLTAGE_TOLERANCE}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--report', required=True, type=Path)
    args = parser.parse_args()
    cases = [check(args.directory/f'{name}.json') for name in ('grounded', 'floating')]
    report = {'engine': dss.Basic.Version(), 'numpy': np.__version__, 'native_sincal_execution': False,
              'ideal_source_engine_approximation_ohms': EPS_OHM, 'opendss_voltage_tolerance_v': VOLTAGE_TOLERANCE,
              'cases': cases}
    args.report.write_text(json.dumps(report, indent=2, allow_nan=False)+'\n')
    print(json.dumps(report, indent=2, allow_nan=False))
    if not all(case['passed'] for case in cases):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
