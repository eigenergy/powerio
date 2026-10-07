#!/usr/bin/env python3
"""Check actual mapped Rust source circuits against independent OpenDSS models.

Inputs are original synthetic schema-11.5 snapshots exported by the Rust tests.
The OpenDSS deck uses the native source/load inputs, not the mapped shunt. The
mapped value is solved independently with dense MNA using its typed components.
This verifies circuit construction, not historical input alignment or SINCAL
execution. OpenDSS requires a tiny nonzero positive/negative source impedance;
its explicitly recorded approximation is bounded by the voltage tolerance.
"""
import argparse
import cmath
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from opendssdirect import dss

EPS_OHM = 1e-8
VOLTAGE_TOLERANCE = 1e-5
CURRENT_TOLERANCE = 1e-6


def mapped_solve(net):
    if net['lines'] or net['transformers'] or net['generators'] or net['untyped']:
        raise ValueError('unexpected component in source oracle')
    ground = {(b['id'], t) for b in net['buses'] for t in b['grounded']}
    nodes = {(b['id'], t): i for i, (b, t) in enumerate(
        (b, t) for b in net['buses'] for t in b['terminals'] if (b['id'], t) not in ground)}

    def at(bus, terminal):
        return None if (bus, terminal) in ground else nodes[bus, terminal]

    constraints = []
    for switch in net['switches']:
        if not switch['open']:
            for a, b in zip(switch['terminal_map_from'], switch['terminal_map_to'], strict=True):
                constraints.append((at(switch['bus_from'], a), at(switch['bus_to'], b), 0j))
    for wrapped in net['sources']:
        if wrapped['type'] != 'powerio.ReferencedVoltageSource':
            raise ValueError('source lost its typed reference')
        source = wrapped['value']
        star = at(source['bus'], source['reference_terminal'])
        for t, v, angle in zip(source['terminal_map'], source['v_magnitude'], source['v_angle'], strict=True):
            constraints.append((at(source['bus'], t), star, cmath.rect(v, angle)))
    matrix = np.zeros((len(nodes)+len(constraints),)*2, dtype=complex)
    rhs = np.zeros(len(matrix), dtype=complex)
    for load in net['loads']:
        if load['configuration'] != 'wye' or load['voltage_model']['model'] != 'constant_impedance':
            raise ValueError('unexpected load model')
        star = at(load['bus'], load['terminal_map'][-1])
        for t, p, q, v in zip(load['terminal_map'][:-1], load['p_nom'], load['q_nom'],
                              load['voltage_model']['v_nom'], strict=True):
            y = complex(p, -q)/v**2
            a = np.zeros(len(matrix))
            a[at(load['bus'], t)] += 1
            if star is not None:
                a[star] -= 1
            matrix += y*np.outer(a, a)
    for shunt in net['shunts']:
        for i, a in enumerate(shunt['terminal_map']):
            for j, b in enumerate(shunt['terminal_map']):
                row, col = at(shunt['bus'], a), at(shunt['bus'], b)
                if row is not None and col is not None:
                    matrix[row, col] += complex(shunt['g'][i][j], shunt['b'][i][j])
    for k, (a, b, voltage) in enumerate(constraints, len(nodes)):
        for node, sign in ((a, 1), (b, -1)):
            if node is not None:
                matrix[k, node] += sign
                matrix[node, k] += sign
        rhs[k] = voltage
    solution = np.linalg.solve(matrix, rhs)
    if np.max(np.abs(matrix@solution-rhs)) > 1e-8:
        raise ValueError('mapped MNA residual exceeds tolerance')
    return np.array([solution[nodes['10', str(i)]] for i in (1, 2, 3)])


def check(path):
    data = json.loads(path.read_text())
    native = data['native_input']
    volts = mapped_solve(data['network'])
    vnom = native['voltage_ll_v']/math.sqrt(3)
    y = np.array([complex(p, -q)/vnom**2 for p, q in zip(native['phase_p_w'], native['phase_q_var'], strict=True)])
    current = y*volts
    z0 = complex(native['r0'], native['x0'])
    deck = ['Clear', 'Set DefaultBaseFrequency=50',
            f'New Circuit.source_test phases=3 bus1=bus.1.2.3 basekv={native["voltage_ll_v"]/1000:.17g} pu=1 angle=0 frequency=50',
            f'Edit Vsource.source r1={EPS_OHM} x1={EPS_OHM} r0={z0.real:.17g} x0={z0.imag:.17g}']
    for phase, (p, q) in enumerate(zip(native['phase_p_w'], native['phase_q_var'], strict=True), 1):
        deck.append(f'New Load.phase{phase} phases=1 bus1=bus.{phase}.0 conn=wye '
                    f'kv={vnom/1000:.17g} kw={p/1000:.17g} kvar={q/1000:.17g} model=2 status=fixed')
    deck += ['Set controlmode=off tolerance=1e-12 maxiterations=100', 'Solve']
    text = '\n'.join(deck)+'\n'
    dss(text)
    if not dss.Solution.Converged():
        raise ValueError('OpenDSS did not converge')
    dss.Circuit.SetActiveBus('bus')
    raw = dss.Bus.Voltages()
    ordered = dict(zip(dss.Bus.Nodes(), (complex(a,b) for a,b in zip(raw[::2], raw[1::2], strict=True)), strict=True))
    reference = np.array([ordered[i] for i in (1,2,3)])
    voltage_error = float(max(abs(volts-reference)))
    current_error = float(max(abs(current-y*reference)))
    sequence_residual = float(abs(sum(volts)/3+z0*sum(current)/3))
    return {'case': path.stem, 'mapped_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'deck_sha256': hashlib.sha256(text.encode()).hexdigest(),
            'max_voltage_error_v': voltage_error, 'max_current_error_a': current_error,
            'zero_sequence_residual_v': sequence_residual,
            'passed': voltage_error < VOLTAGE_TOLERANCE and current_error < CURRENT_TOLERANCE and sequence_residual < 1e-9}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    cases = [check(args.directory/f'{name}.json') for name in ('resistive', 'inductive', 'capacitive')]
    report = {'engine': dss.Basic.Version(), 'numpy': np.__version__,
              'positive_negative_source_impedance_ohms': {'r': EPS_OHM, 'x': EPS_OHM},
              'voltage_tolerance_v': VOLTAGE_TOLERANCE, 'current_tolerance_a': CURRENT_TOLERANCE,
              'native_execution': False, 'historical_results_used': False, 'cases': cases}
    args.report.write_text(json.dumps(report, indent=2, allow_nan=False)+'\n')
    print(json.dumps(report, indent=2, allow_nan=False))
    if not all(case['passed'] for case in cases):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
