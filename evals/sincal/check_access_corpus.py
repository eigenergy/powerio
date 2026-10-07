#!/usr/bin/env python3
"""Reproduce native MDB acquisition and Rust structural checks for all 19 CSIRO cases.

No downloads or electrical mapping. Model payloads and acquired records remain
in caller-selected external storage; the report contains derived metadata only.
"""

import argparse
import hashlib
import json
from pathlib import Path

from import_access import BASE_TABLES, acquire, run_bounded, write_records


def selected_count(table, variant):
    index = [column['name'] for column in table['columns']].index('Variant_ID')
    return sum(row[index] == variant for row in table['rows'])


def check(source_directory, record_directory, inspector, tool_directory):
    inventory = json.loads(Path(__file__).with_name('csiro-inventory.json').read_text())
    # A fresh directory prevents a failed run from silently reusing old records.
    record_directory.mkdir(parents=True, exist_ok=False)
    cases = []
    for case in inventory['databases']:
        number = case['representative']
        candidates = [source_directory / case['source_path'],
                      source_directory / f'csiro-representative{number:02d}.mdb']
        source = next((path for path in candidates if path.is_file()), None)
        if source is None:
            raise ValueError(f'missing required corpus case {number:02d}')
        # Keep the established hash-pinned 21-table corpus reproducible as the
        # general importer grows. Shunt extensions have their own identity report.
        records = acquire(source, tables=BASE_TABLES, tool_directory=tool_directory)
        if records['source']['sha256'] != case['sha256'] or records['source']['bytes'] != case['bytes']:
            raise ValueError(f'original source digest/size mismatch in case {number:02d}')
        tables = {table['name']: table for table in records['tables']}
        for name, expected in case['stored_row_counts'].items():
            if name in tables and len(tables[name]['rows']) != expected:
                raise ValueError(f'acquired row count mismatch in case {number:02d}: {name}')
        destination = record_directory / f'representative{number:02d}.json'
        write_records(records, destination)
        result = json.loads(run_bounded([str(inspector), str(destination), '1'],
                                       byte_limit=1024 * 1024, seconds=30))
        for table, field in [('Node', 'nodes'), ('Element', 'elements'), ('Terminal', 'terminals')]:
            if result[field] != selected_count(tables[table], 1):
                raise ValueError(f'Rust variant-local count mismatch in case {number:02d}: {field}')
        if (result['schema'] != 11.5 or result['variant'] != 1
                or result['source_sha256'] != case['sha256']
                or result['electrical_mapping_performed'] is not False):
            raise ValueError(f'Rust snapshot identity mismatch in case {number:02d}')
        cases.append({'case': number, 'source_sha256': case['sha256'],
                      'source_bytes': case['bytes'],
                      'record_sha256': hashlib.sha256(destination.read_bytes()).hexdigest(),
                      'record_bytes': destination.stat().st_size,
                      'acquired_rows': {name: len(table['rows']) for name, table in tables.items()},
                      'selected_variant': 1,
                      'structural_counts': {key: result[key] for key in ('nodes', 'elements', 'terminals')},
                      'tools': records['tools']})
    return {'collection': inventory['collection'], 'attribution': inventory['attribution'],
            'license': inventory['license'], 'license_url': inventory['license_url'],
            'scope': 'Tool-assisted Access acquisition and Rust base-variant structural checks only; no electrical mapping or native SQLite export.',
            'cases': cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source_directory', type=Path)
    parser.add_argument('record_directory', type=Path)
    parser.add_argument('report', type=Path)
    parser.add_argument('--inspector', type=Path, required=True)
    parser.add_argument('--tool-directory', type=Path)
    arguments = parser.parse_args()
    try:
        report = check(arguments.source_directory, arguments.record_directory,
                       arguments.inspector.resolve(), arguments.tool_directory)
        write_records(report, arguments.report)
    except (OSError, ValueError) as error:
        parser.exit(1, f'Access corpus verification failed: {error}\n')
    print(f"Verified acquisition and selected-variant structure for {len(report['cases'])} cases; "
          'electrical mapping remains separate')


if __name__ == '__main__':
    main()
