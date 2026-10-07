#!/usr/bin/env python3
"""Export and independently validate five native CSIRO09 snapshots and stress cases.
Original model and acquired records remain external. Requires the built Rust
sincal_multiconductor example and the oracle Python dependencies in README.md.
"""
import argparse
import copy
import hashlib
import json
import subprocess
import tempfile
from pathlib import Path

from check_csiro09_network import check


def validate(records, source, reader, public_reader=False):
    results = []
    controls = []
    consumers = []
    with tempfile.TemporaryDirectory(prefix='powerio-csiro09-') as directory:
        root = Path(directory)
        for hours in [0, 0.25, 12, 23.75, 24]:
            network = root / f'network-{hours}.json'
            with network.open('w') as output:
                command = ([str(reader), str(source), str(records), str(hours)] if public_reader else
                           [str(reader), 'records', 'read', str(records), '1', str(hours)])
                run = subprocess.run(command, stdout=output, stderr=subprocess.PIPE,
                                     text=True, check=True, timeout=120)
                if public_reader:
                    consumer = json.loads(run.stderr)
                    expected = {
                        'generic_matrix_diagnostics': 0,
                        'power_flow_instance': {
                            'constructed': False,
                            'diagnostics': [{
                                'code': 'BUILD.INSTANCE.SHAPE_MISMATCH',
                                'message': 'multiconductor island containing bus `2178` has no voltage source; explicitly resolve de-energized islands before constructing a calculation instance',
                            }],
                        },
                    }
                    if consumer != expected:
                        raise ValueError(f'unexpected generic consumer outcome: {consumer}')
                    consumers.append({'hours': hours, **consumer})
            for stress in [False, True]:
                results.append(check(records, source, network, hours, stress))
        original = json.loads((root / 'network-0.json').read_text())
        for name in ['wrong-profile-factor', 'missing-mutual-impedance', 'closed-native-open-connection', 'missing-load']:
            mutated = copy.deepcopy(original)
            if name == 'wrong-profile-factor':
                for load in mutated['loads']:
                    load['p_nom'] = [value / 1.14 for value in load['p_nom']]
            elif name == 'missing-mutual-impedance':
                for code in mutated['linecodes']:
                    for field in ['r_series', 'x_series']:
                        code[field] = [[value if i == j else 0 for j, value in enumerate(row)]
                                       for i, row in enumerate(code[field])]
            elif name == 'closed-native-open-connection':
                next(s for s in mutated['switches'] if s['name'] == '2737')['open'] = False
            else:
                mutated['loads'].pop()
            path = root / f'{name}.json'
            path.write_text(json.dumps(mutated))
            try:
                check(records, source, path, 0)
            except ValueError as error:
                controls.append({'mutation': name, 'rejected': True, 'reason': str(error)})
            else:
                raise ValueError(f'checker accepted {name}')
    return {'reader_api': 'public facade, explicit Access acquisition' if public_reader else 'private component mapper',
            'source_echo_and_ir_checked': public_reader, 'reader_sha256': hashlib.sha256(reader.read_bytes()).hexdigest(),
            'scope': 'One complete native conductor-resolved feeder at five snapshots; separately labelled synthetic unequal delta branches. No native SINCAL execution.',
            'generic_consumers': consumers,
            'cases': results, 'negative_controls': controls, 'passed': True}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['records', 'source', 'reader', 'report']:
        parser.add_argument(name, type=Path)
    parser.add_argument('--public-reader', action='store_true', help='use the sincal_public facade example, which also checks original-source echo and IR fidelity')
    args = parser.parse_args()
    report = validate(args.records, args.source, args.reader, args.public_reader)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'snapshots_and_stress_cases': len(report['cases']),
                      'negative_controls': len(report['negative_controls']),
                      'maximum_voltage_error_v': max(c['maximum_voltage_error_v'] for c in report['cases'])}))
