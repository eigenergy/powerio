#!/usr/bin/env python3
"""Audit all hash-pinned CSIRO base variants with the actual Rust dist adapter.

This is a development coverage report, not a passing acceptance gate. Context
failures, rejected components, complete parsing and independent validation are
separate outcomes. Neither original models nor acquired records are vendored.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import re

from import_access import run_bounded


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def invoke(reader, records, mode, hours=None):
    try:
        command = [str(reader), 'records', mode, str(records), '1']
        if hours is not None:
            command.append(str(hours))
        return json.loads(run_bounded(command,
                                     byte_limit=32 * 1024 * 1024, seconds=30)), None
    except ValueError as error:
        message = str(error)
        # An intentional Rust electrical rejection is an audit disposition.
        # Missing tools, timeouts, malformed JSON and process failures are not.
        if 'exited 1: Error: FormatRead {' not in message:
            raise
        return None, message.split('Error: ', 1)[1].strip()


def audit(source_directory, record_directory, reader, hours=None):
    if hours is not None and (not math.isfinite(hours) or hours < 0):
        raise ValueError("snapshot hours must be finite and nonnegative")
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    cases = []
    for case in manifest['cases']:
        number = case['case']
        source = source_directory / f'csiro-representative{number:02d}.mdb'
        records = record_directory / f'representative{number:02d}.json'
        for path, prefix in [(source, 'source'), (records, 'record')]:
            if path.stat().st_size != case[f'{prefix}_bytes'] or digest(path) != case[f'{prefix}_sha256']:
                raise ValueError(f'case {number}: {prefix} identity mismatch')
        report, context_error = invoke(reader, records, 'audit', hours)
        network, parse_error = invoke(reader, records, 'read', hours)
        row = {key: case[key] for key in ('case', 'source_sha256', 'record_sha256')}
        row.update({'schema': 11.5, 'variant': 1, 'context_error': context_error,
                    'complete_parse': network is not None, 'parse_error': parse_error,
                    'independently_validated': False, 'native_execution': False})
        if hours is not None:
            row["snapshot_hours"] = hours
        if report is not None:
            components = report['components']
            if (report['schema_version'] != 11.5 or report['variant'] != 1
                    or report.get('snapshot_hours') != hours
                    or len(components) != case['structural_counts']['elements']
                    or len({c['element'] for c in components}) != len(components)):
                raise ValueError(f'case {number}: inconsistent Rust component accounting')
            row['component_count'] = len(components)
            row['mapped_by_type'] = dict(sorted(Counter(c['type'] for c in components if c['component_maps']).items()))
            failures = Counter((c['type'], re.sub(r'Element \d+ \([^)]*\): ', '', c['first_error']))
                               for c in components if not c['component_maps'])
            row['failures'] = [{'type': kind, 'count': count, 'reason': reason}
                               for (kind, reason), count in sorted(failures.items())]
        cases.append(row)
    return {'collection': manifest['collection'], 'attribution': manifest['attribution'],
            'license': manifest['license'],
            'scope': 'Conductor-resolved base-variant development audit; no family inference, independent solve or native execution.',
            'reader_sha256': digest(reader), 'cases': cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source_directory', type=Path)
    parser.add_argument('record_directory', type=Path)
    parser.add_argument('report', type=Path)
    parser.add_argument('--reader', type=Path, required=True)
    parser.add_argument('--snapshot-hours', type=float)
    args = parser.parse_args()
    report = audit(args.source_directory, args.record_directory, args.reader.resolve(), args.snapshot_hours)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    complete = sum(c['complete_parse'] for c in report['cases'])
    print(f"Audited {len(report['cases'])} cases; {complete} complete parses. See per-case limitations.")


if __name__ == '__main__':
    main()
