#!/usr/bin/env python3
"""Check explicit CSIRO09 selections through the installed Python wheel and CLI.
This is binding/source-fidelity evidence, not another independent solver oracle.
Original MDB and acquired records remain external; supply an acquisition root.
"""
import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

import powerio
from powerio.dist import SincalReadOptions

SOURCE_SHA256 = 'd2c41fae2f6fc3b9cdea75c1bcbc50f083b104440148a25274ee6ed7da57a14b'
RECORD_SHA256 = 'fa661a3947bd171683d1b73a4e15c0d073c19c0df473f649fb32d4e8a2068592'


def validate(source, records, cli, acquisition_root):
    if hashlib.sha256(source.read_bytes()).hexdigest() != SOURCE_SHA256:
        raise ValueError('expected the original CSIRO09 MDB')
    if hashlib.sha256(records.read_bytes()).hexdigest() != RECORD_SHA256:
        raise ValueError('expected the verified CSIRO09 acquisition records')
    relative = os.path.relpath(records, source.parent)
    results = []
    powers = []
    for hours in [0, 6]:
        module = powerio.parse(source, format='sincal-multiconductor',
            sincal_multiconductor=SincalReadOptions(variant=1, snapshot_hours=hours, acquired_tables=relative),
            acquisition_root=acquisition_root)
        if not isinstance(module.value, powerio.dist.MulticonductorNetwork):
            raise ValueError('Python returned the wrong network family')
        expected = json.loads(powerio.serialize(module).text)['value']
        flags = ['--from', 'sincal-multiconductor', '--sincal-variant', '1',
                 '--sincal-snapshot-hours', str(hours), '--sincal-acquired-tables', relative,
                 '--acquisition-root', str(acquisition_root)]
        run = subprocess.run([str(cli), 'serialize', str(source), *flags],
                             capture_output=True, check=True, timeout=120)
        actual = json.loads(run.stdout)['value']
        if actual != expected:
            raise ValueError('CLI and Python typed values differ')
        if powerio.emit(module, 'sincal').artifacts[0].data != source.read_bytes():
            raise ValueError('Python did not echo the original MDB')
        with tempfile.TemporaryDirectory(prefix='powerio-sincal-echo-') as directory:
            output = Path(directory) / 'copy.mdb'
            subprocess.run([str(cli), 'convert', str(source), *flags, '--to',
                            'sincal-multiconductor', '-o', str(output)],
                           capture_output=True, check=True, timeout=120)
            if hashlib.sha256(output.read_bytes()).hexdigest() != SOURCE_SHA256:
                raise ValueError('CLI did not echo the original MDB')
        loads = module.value.loads
        powers.append([load['p_nom'] for load in loads])
        results.append({'hours': hours, 'loads': len(loads), 'typed_value_equal': True,
                        'python_original_mdb_echo': True, 'cli_original_mdb_echo': True})
    if powers[0] == powers[1]:
        raise ValueError('snapshot selections did not change the native load powers')
    return {'scope': 'Python/CLI binding selection and native source fidelity; not native SINCAL execution or independent solver evidence',
            'source_sha256': SOURCE_SHA256, 'record_sha256': RECORD_SHA256,
            'cli_sha256': hashlib.sha256(cli.read_bytes()).hexdigest(),
            'python_extension_sha256': hashlib.sha256(Path(powerio._powerio.__file__).read_bytes()).hexdigest(),
            'cases': results, 'snapshots_differ': True, 'passed': True,
            'license': 'CC-BY-4.0',
            'attribution': 'Berry, Adam; Collins, Lyle; Oliver, Erin; Perfumo, Cristian (2015), Representative Australian Electricity Feeders with load and solar generation profiles, v1, CSIRO.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'records', 'cli', 'report']:
        parser.add_argument(name, type=Path)
    parser.add_argument('--acquisition-root', type=Path, required=True)
    args = parser.parse_args()
    report = validate(args.source.resolve(), args.records.resolve(), args.cli.resolve(), args.acquisition_root.resolve())
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'snapshots': len(report['cases']), 'passed': report['passed']}))
