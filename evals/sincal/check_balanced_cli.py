"""Check CSIRO19 balanced Access selection and original-source echo through the CLI.

Uses the existing independent native-input pandapower oracle. This adds interface
coverage for an already validated case, not another supported native case.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from check_balanced_access import SOURCE, RECORD, check_model, digest, reference


def check(cli, source, records, acquisition_root):
    assert digest(source) == SOURCE and digest(records) == RECORD
    doc = json.loads(records.read_text())
    tables = {t['name']: [dict(zip([c['name'] for c in t['columns']], r, strict=True))
                         for r in t['rows']] for t in doc['tables']}
    relative = os.path.relpath(records, source.parent)
    shared = ['--from', 'sincal-balanced', '--sincal-variant', '1',
              '--sincal-acquired-tables', relative, '--acquisition-root', str(acquisition_root)]

    def invoke(command, flags, *extra, success=True):
        result = subprocess.run([str(cli), command, str(source), *flags, *extra],
                                capture_output=True, text=True, timeout=60)
        assert (result.returncode == 0) == success, result.stderr
        return result

    results, powers = [], []
    with tempfile.TemporaryDirectory(prefix='powerio-sincal-balanced-cli-') as directory:
        for hours in [0, 6]:
            flags = [*shared, '--sincal-snapshot-hours', str(hours)]
            serialized = json.loads(invoke('serialize', flags).stdout)
            assert serialized['value']['type'] == 'powerio.BalancedNetwork'
            model = serialized['value']['data']
            result = check_model(model, *reference(tables, hours))
            output = Path(directory) / f'echo-{hours}.mdb'
            invoke('convert', flags, '--to', 'sincal-balanced', '-o', str(output))
            assert digest(output) == SOURCE
            powers.append([load['p'] for load in model['loads']])
            results.append(dict(hours=hours, original_mdb_echo=True, **result))
    assert powers[0] != powers[1]
    invoke('serialize', shared, success=False)  # No implicit midnight.
    invalid_variant = shared.copy()
    invalid_variant[3] = '999999'
    invoke('serialize', invalid_variant, '--sincal-snapshot-hours', '0', success=False)
    rejected = ['missing_snapshot', 'invalid_variant']
    if not records.is_relative_to(source.parent):
        invoke('serialize', shared[:-2], '--sincal-snapshot-hours', '0', success=False)
        rejected.append('missing_acquisition_root')
    return dict(scope=__doc__, source_sha256=SOURCE, record_sha256=RECORD,
                cli_sha256=hashlib.sha256(cli.read_bytes()).hexdigest(),
                snapshots=results, snapshots_differ=True,
                rejected=rejected,
                native_sincal_execution=False, additional_native_cases=0, passed=True,
                license='CC-BY-4.0',
                attribution='Berry, Adam; Collins, Lyle; Oliver, Erin; Perfumo, Cristian (2015), Representative Australian Electricity Feeders with load and solar generation profiles, v1, CSIRO.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['cli', 'source', 'records', 'acquisition_root', 'report']:
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    report = check(args.cli.resolve(), args.source.resolve(), args.records.resolve(), args.acquisition_root.resolve())
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print('CSIRO19 CLI: two snapshots, original MDB echo and invalid selection checks passed.')
