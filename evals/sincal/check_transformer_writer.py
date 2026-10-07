#!/usr/bin/env python3
"""Compare actual fresh SINCAL transformer readback with OpenDSS YPrim.

Original synthetic typed winding data feeds OpenDSS directly. Compare every
entry of the six-phase primitive and solve an unequal-load circuit driven by
an asymmetric supply. No native rows or reader formulas construct the oracle.
The source is ideal in this algebraic circuit; there is no source-impedance
approximation. Native SINCAL execution remains a separate acceptance gate.
"""
import argparse
import hashlib
import itertools
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss


def oracle(transformer):
    windings = transformer['windings']
    buses = ['primary.1.2.3.0', 'secondary.1.2.3.0']
    def values(key, factor=1):
        return '('+' '.join(format(w[key]*factor, '.17g') for w in windings)+')'
    leadlag = transformer['extras'].get('leadlag', 'lag')
    deck = '\n'.join([
        'Clear', 'Set DefaultBaseFrequency=50',
        'New Circuit.oracle basekv=11 bus1=primary',
        'New Transformer.subject phases=3 windings=2 buses=('+ ' '.join(buses)+') '
        +'conns=('+' '.join(w['conn'] for w in windings)+') '
        +f'kvs={values("v_ref", .001)} kvas={values("s_rating", .001)} '
        +f'%rs={values("r_pct")} taps={values("tap")} '
        +f'xhl={transformer["xsc_pct"][0]:.17g} leadlag={leadlag} '
        +'%noloadloss=0 %imag=0 ppm_antifloat=0',
        'Set controlmode=off', 'Solve',
    ])+'\n'
    dss(deck)
    if not dss.Circuit.SetActiveElement('Transformer.subject'):
        raise ValueError('missing OpenDSS transformer')
    if dss.CktElement.NumConductors() != 4 or dss.CktElement.NumTerminals() != 2:
        raise ValueError('unexpected OpenDSS terminal layout')
    raw = np.array(dss.CktElement.YPrim())
    y = (raw[::2]+1j*raw[1::2]).reshape(8, 8, order='F')
    # Both Wye neutrals are explicitly grounded, and Delta's unused fourth
    # conductor is absent. Select the six active phase coordinates.
    phases = [0, 1, 2, 4, 5, 6]
    return y[np.ix_(phases, phases)], hashlib.sha256(deck.encode()).hexdigest()


def loaded_voltage(y, transformer):
    primary, secondary = transformer['windings']
    # Simultaneous positive/negative/zero sequences exercise grounding and
    # both rotation signs. Unequal constant-admittance loads ground the LV
    # common mode even for a delta secondary.
    p = np.exp(-2j*np.pi*np.arange(3)/3)
    supply = primary['v_ref']/np.sqrt(3)*(p + .07*p.conj() + .03)
    rating = secondary['v_ref']**2
    loads = np.diag(np.array([9000-1800j, 15000+3000j, 21000-6000j])/rating)
    return np.linalg.solve(y[3:, 3:]+loads, -y[3:, :3]@supply)


def check(path):
    doc = json.loads(path.read_text())
    t = doc['input']['transformers'][0]
    fresh = doc['fresh_readback']
    if len(fresh['shunts']) != 1 or fresh['transformers']:
        raise ValueError('expected one actual reader-produced transformer primitive')
    shunt = fresh['shunts'][0]
    if shunt['terminal_map'] != ['p1', 'p2', 'p3', 's1', 's2', 's3']:
        raise ValueError('unexpected phase coordinates')
    for side in range(2):
        node = str(doc['bus_ids'][t['windings'][side]['bus']])
        if not any(s['bus_from'] == node and s['bus_to'] == shunt['bus']
                   and not s['open'] and s['terminal_map_from'] == ['1','2','3']
                   and s['terminal_map_to'] == shunt['terminal_map'][3*side:3*side+3]
                   for s in fresh['switches']):
            raise ValueError('fresh transformer port does not preserve conductor connectivity')
    actual = np.array(shunt['g'])+1j*np.array(shunt['b'])
    expected, deck_hash = oracle(t)
    primitive_error = float(np.max(np.abs(actual-expected))/np.max(np.abs(expected)))
    volts = loaded_voltage(expected, t)
    voltage_error = float(np.max(np.abs(loaded_voltage(actual,t)-volts)))
    # Conjugating only the cross-port phase ordering reverses the vector
    # group while retaining positive impedance. This must fail mixed cases.
    mixed = t['windings'][0]['conn'] != t['windings'][1]['conn']
    wrong = actual.copy()
    wrong[:3, 3:] = wrong[:3, 3:].T.copy()
    wrong[3:, :3] = wrong[3:, :3].T.copy()
    counterexample = float(np.max(np.abs(loaded_voltage(wrong,t)-volts))) if mixed else None
    if mixed and counterexample < 1:
        raise ValueError('asymmetric circuit does not detect reversed winding rotation')
    return {'case': path.stem, 'export_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'opendss_deck_sha256': deck_hash,
            'primitive_relative_error': primitive_error,
            'unequal_load_voltage_error_v': voltage_error,
            'reversed_rotation_counterexample_v': counterexample,
            'passed': primitive_error < 1e-10 and voltage_error < 1e-7}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    cases = []
    for kind, up, lead, tap in itertools.product(range(3), [False,True], [False,True], [False,True]):
        name = f'transformer-{kind}-{str(up).lower()}-{str(lead).lower()}-{str(tap).lower()}.json'
        cases.append(check(args.directory/name))
    report = {'engine': dss.Basic.Version(), 'numpy': np.__version__,
              'native_sincal_execution': False, 'cases': cases}
    args.report.write_text(json.dumps(report, indent=2)+'\n')
    if not all(c['passed'] for c in cases):
        raise SystemExit('transformer writer validation failed')
    print(f'{len(cases)} transformer cases passed; maximum relative primitive error '
          f'{max(c["primitive_relative_error"] for c in cases):.3e}; maximum voltage error '
          f'{max(c["unequal_load_voltage_error_v"] for c in cases):.3e} V')


if __name__ == '__main__':
    main()
