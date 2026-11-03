#!/usr/bin/env python3
"""Audit complete-row variant inheritance against native active-row selections.

Research only: this does not enable derived variants in either electrical reader.
The native Flag_Variant cache is an independent check for the stored active
variant, not a rule for selecting arbitrary variants or a deletion marker.
Deletion encoding remains unverified. Sources and acquired rows are never edited.
"""
import argparse
import copy
import json
from pathlib import Path

from audit_distribution import digest


KEYS = {
    'Node': 'Node_ID', 'Element': 'Element_ID', 'Terminal': 'Terminal_ID',
    'VoltageLevel': 'VoltLevel_ID', 'CalcParameter': 'CalcParameter_ID',
    'Line': 'Element_ID', 'Load': 'Element_ID', 'Infeeder': 'Element_ID',
    'DCInfeeder': 'Element_ID', 'TwoWindingTransformer': 'Element_ID',
    'ThreeWindingTransformer': 'Element_ID', 'ShuntImpedance': 'Element_ID',
    'ShuntReactor': 'Element_ID', 'ShuntCondensator': 'Element_ID',
    'OpSer': 'OpSer_ID', 'OpSerVal': 'OpSerVal_ID',
    'TransformerTap': 'TransformerTap_ID',
}
COUNTS = ('Node', 'Element', 'Terminal', 'Load', 'OpSer', 'OpSerVal')
SPECIFICATION = 'https://sincal.s3.amazonaws.com/6.0/ReleaseNotes-Eng.pdf'


def positive_id(value):
    if type(value) is not int or value <= 0:
        raise ValueError('expected positive native identity; deletion markers are not interpreted')
    return value


def ancestry(variants, selected):
    """Return root through selected child, rejecting unresolved/cyclic ancestry."""
    path, seen = [], set()
    current = positive_id(selected)
    while current:
        if current in seen:
            raise ValueError('cyclic variant ancestry')
        if current not in variants:
            raise ValueError('missing variant or parent')
        seen.add(current)
        path.append(current)
        parent = variants[current]['ParentVariant_ID']
        current = 0 if parent is None or (type(parent) is int and parent == 0) else positive_id(parent)
    return list(reversed(path))


def index_tables(tables):
    variants = {}
    for row in tables['Variant']:
        key = positive_id(row['Variant_ID'])
        if key in variants:
            raise ValueError('duplicate variant')
        variants[key] = row
    for selected in variants:
        ancestry(variants, selected)
    indexed = {}
    for name, rows in tables.items():
        if name in ('Variant', 'Version') or not rows:
            continue
        if name not in KEYS:
            raise ValueError(f'nonempty table outside structural audit: {name}')
        groups = {}
        for row in rows:
            variant = positive_id(row['Variant_ID'])
            if variant not in variants:
                raise ValueError(f'{name}: row references missing variant')
            key = positive_id(row[KEYS[name]])
            if type(row.get('Flag_Variant')) is not int or row['Flag_Variant'] not in (0, 1):
                raise ValueError(f'{name}: undocumented active-row flag')
            group = groups.setdefault(variant, {})
            if key in group:
                raise ValueError(f'{name}: duplicate identity within one variant')
            group[key] = row
        indexed[name] = groups
    return variants, indexed


def resolve(indexed, path):
    result = {}
    for name, groups in indexed.items():
        rows = {}
        for variant in path:
            # A child record is complete, including NULLs. Never merge fields.
            rows.update(groups.get(variant, {}))
        result[name] = rows
    return result


def active_rows(indexed):
    result = {}
    for name, groups in indexed.items():
        rows = {}
        for group in groups.values():
            for key, row in group.items():
                if row['Flag_Variant'] == 1:
                    if key in rows:
                        raise ValueError(f'{name}: ambiguous native active-row cache')
                    rows[key] = row
        result[name] = rows
    return result


def reference_errors(snapshot):
    errors = []
    for row in snapshot.get('Terminal', {}).values():
        for field, target in [('Element_ID', 'Element'), ('Node_ID', 'Node')]:
            if row[field] not in snapshot.get(target, {}):
                errors.append(f"Terminal {row['Terminal_ID']}: unresolved {field}")
    for row in snapshot.get('Load', {}).values():
        for field in ('DayOpSer_ID', 'WeekOpSer_ID', 'YearOpSer_ID'):
            if row.get(field) not in (None, 0) and row[field] not in snapshot.get('OpSer', {}):
                errors.append(f"Load {row['Element_ID']}: unresolved {field}")
    for row in snapshot.get('OpSerVal', {}).values():
        if row['OpSer_ID'] not in snapshot.get('OpSer', {}):
            errors.append(f"OpSerVal {row['OpSerVal_ID']}: unresolved OpSer_ID")
    return errors


def compare(snapshot, expected):
    return sum(snapshot.get(name, {}).get(key) != expected.get(name, {}).get(key)
               for name in set(snapshot) | set(expected)
               for key in set(snapshot.get(name, {})) | set(expected.get(name, {})))


def audit_case(case, source_directory, record_directory, identity):
    source = source_directory / f'csiro-representative{case:02}.mdb'
    records = record_directory / f'representative{case:02}.json'
    if digest(source) != identity['source_sha256'] or digest(records) != identity['record_sha256']:
        raise ValueError(f'case {case}: pinned native source or acquisition changed')
    document = json.loads(records.read_text())
    tables = {t['name']: [dict(zip([c['name'] for c in t['columns']], row, strict=True))
                          for row in t['rows']] for t in document['tables']}
    variants, indexed = index_tables(tables)
    active = [v for v, row in variants.items() if row['Flag_Variant'] == 1]
    if len(active) != 1:
        raise ValueError('expected exactly one recorded active variant')
    active = active[0]
    path = ancestry(variants, active)
    expected = active_rows(indexed)
    resolved = resolve(indexed, path)
    differences = compare(resolved, expected)
    if differences:
        raise ValueError(f'case {case}: {differences} differences from native active-row cache')
    controls = {}
    candidates = {
        'parent_overrides_child': resolve(indexed, list(reversed(path))),
        'child_rows_only': resolve(indexed, [active]),
        'numeric_union_of_all_variants': resolve(indexed, sorted(variants)),
    }
    # A union can coincidentally pick the stored active variant (10/11). Its
    # failure is required across the corpus, not invented for each individual case.
    for name, candidate in candidates.items():
        controls[name] = compare(candidate, expected)
    candidate = copy.deepcopy(resolved)
    row = next(iter(candidate['Load'].values()))
    row['DayOpSer_ID'] = max(candidate['OpSer']) + 1
    if not reference_errors(candidate):
        raise ValueError('dangling profile reference control was not detected')
    controls['dangling_profile_reference'] = len(reference_errors(candidate))
    inventory = []
    for selected in sorted(variants):
        selected_path = ancestry(variants, selected)
        snapshot = resolve(indexed, selected_path)
        errors = reference_errors(snapshot)
        if errors:
            raise ValueError(f'case {case}, variant {selected}: {errors[:3]}')
        inventory.append(dict(variant=selected, ancestry=selected_path,
            counts={name: len(snapshot.get(name, {})) for name in COUNTS},
            matched_native_active_cache=selected == active))
    return dict(case=case, source_sha256=identity['source_sha256'],
        record_sha256=identity['record_sha256'], active_variant=active,
        native_cache_comparison=[dict(table=name, rows=len(rows), differing_rows=0)
                                 for name, rows in sorted(expected.items())],
        active_rows_compared=sum(len(rows) for rows in expected.values()),
        mutation_disagreements=controls, variants=inventory)


def audit(source_directory, record_directory):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identities = {c['case']: c for c in manifest['cases']}
    cases = [audit_case(c, source_directory, record_directory, identities[c]) for c in (10, 11, 14)]
    for control in cases[0]['mutation_disagreements']:
        if not any(c['mutation_disagreements'][control] for c in cases):
            raise ValueError(f'corpus does not distinguish {control}')
    return dict(scope=__doc__, specification=dict(url=SPECIFICATION, section='1.3', printed_pages=[4, 5]),
        collection=manifest['collection'], attribution=manifest['attribution'], license=manifest['license'],
        cases=cases, reconstructed_variants=sum(len(c['variants']) for c in cases),
        variants_with_native_active_cache=len(cases),
        native_active_rows_compared=sum(c['active_rows_compared'] for c in cases),
        source_data_changed=False, deletion_encoding_verified=False,
        production_derived_variant_support=False, native_execution=False,
        disposition='Complete-row ancestry agrees with all acquired active native input rows in three projects. Other reconstructed variants have reference checks only. Deletion encoding and production materialization remain work.',
        passed=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('source_directory', 'record_directory', 'report'):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    result = audit(args.source_directory, args.record_directory)
    args.report.write_text(json.dumps(result, indent=2) + '\n')
    print(f"Reconstructed {result['reconstructed_variants']} variants; "
          f"matched {result['native_active_rows_compared']} native active rows in three projects. "
          'Structural evidence only; no additional electrical parse.')
