#!/usr/bin/env python3
"""Explain every remaining CSIRO06 rejection against hash-pinned native inputs.

This is a development blocker audit, not an electrical acceptance test. Decimal
nameplate arithmetic follows Siemens Input Data (April 2014), printed p.180.
It does not repair inputs, infer missing grounding, or execute SINCAL.
"""
import argparse
import copy
from decimal import Decimal
import json
from pathlib import Path

from audit_distribution import digest, invoke

CORE_ERROR = 'no-load core loss Vfe exceeds apparent power from i0 and Sn'
LOAD_ERROR = 'load requires zero-sequence circuit resolution'
WINDING_ERROR = 'partial transformer windings require verified delta-delta nominal circuits'


def decimal(row, field):
    value = row[field]
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f'{field}: explicit numeric input required')
    result = Decimal(str(value))
    if not result.is_finite():
        raise ValueError(f'{field}: finite input required')
    return result


def classify(tables, audit):
    if audit['schema_version'] != 11.5 or audit['variant'] != 1 or audit['snapshot_hours'] != 0:
        raise ValueError('wrong audit selection')
    elements = {r['Element_ID']: r for r in tables['Element'] if r['Variant_ID'] == 1}
    components = {c['element']: c for c in audit['components']}
    if len(components) != len(audit['components']) or set(components) != set(elements):
        raise ValueError('incomplete or duplicated component accounting')
    settings = [r for r in tables['CalcParameter'] if r['Variant_ID'] == 1]
    if len(settings) != 1 or settings[0]['Flag_LFZ0'] != 1:
        raise ValueError('native zero-sequence completion is not disabled')
    native_ports = {}
    for row in tables['Terminal']:
        if row['Variant_ID'] == 1:
            native_ports.setdefault(row['Element_ID'], []).append(row)
    core, partial, loads = [], [], []
    nameplates_checked = 0
    for row in tables['TwoWindingTransformer']:
        if row['Variant_ID'] != 1:
            continue
        nameplates_checked += 1
        eid = row['Element_ID']
        component = components[eid]
        error = component['first_error'] or ''
        uk, ur = decimal(row, 'uk'), decimal(row, 'ur')
        if not 0 <= ur <= uk:
            raise ValueError(f'{eid}: additional short-circuit nameplate conflict')
        watts = decimal(row, 'Vfe') * 1000
        va = decimal(row, 'i0') * decimal(row, 'Sn') * 10000
        if min(watts, va) < 0:
            raise ValueError(f'{eid}: negative excitation nameplate')
        if watts > va:
            if component['component_maps'] or CORE_ERROR not in error:
                raise ValueError(f'{eid}: core inconsistency not rejected explicitly')
            core.append(dict(element=eid, core_loss_w=float(watts), no_load_va=float(va),
                             excess_w=float(watts-va), excess_fraction=float((watts-va)/va)))
        elif not component['component_maps']:
            ports = sorted(native_ports[eid], key=lambda p: p['TerminalNo'])
            if (WINDING_ERROR not in error or row['VecGrp'] != 14 or len(ports) != 2
                    or [p['TerminalNo'] for p in ports] != [1, 2]
                    or ports[0]['Flag_Terminal'] != ports[1]['Flag_Terminal']
                    or ports[0]['Flag_Terminal'] not in range(1, 7)
                    or row['Flag_Z0_Input'] != 2 or not elements[eid]['Flag_Input'] & 4):
                raise ValueError(f'{eid}: unexpected transformer mapping blocker')
            # Direct ohms are referred to the grounded primary. Retain that
            # distinction; these numbers must not be reinterpreted as percentages.
            base = decimal(row, 'Un1') ** 2 / decimal(row, 'Sn')
            positive_magnitude = base * uk / 100
            zero_magnitude = (decimal(row, 'R0') ** 2 + decimal(row, 'X0') ** 2).sqrt()
            partial.append(dict(element=eid, vector_group='YNd1', winding_selection=ports[0]['Flag_Terminal'],
                                positive_primary_magnitude_ohm=float(positive_magnitude),
                                zero_primary_magnitude_ohm=float(zero_magnitude)))
    for row in tables['Load']:
        if row['Variant_ID'] != 1:
            continue
        eid = row['Element_ID']; component = components[eid]
        if component['component_maps']:
            continue
        ports = native_ports[eid]
        if (LOAD_ERROR not in (component['first_error'] or '') or len(ports) != 1
                or ports[0]['Flag_Terminal'] != 7 or row['Flag_Lf'] != 4
                or elements[eid]['Flag_Input'] & 4 or row['Stp_ID'] not in (None, 0)):
            raise ValueError(f'{eid}: unexpected load mapping blocker')
        loads.append(eid)
    failed = {eid for eid, c in components.items() if not c['component_maps']}
    explained = {r['element'] for r in core + partial} | set(loads)
    if failed != explained:
        raise ValueError('unexplained or incorrectly classified component failures')
    return dict(component_count=len(components), mapped=len(components)-len(failed),
                rejected=len(failed), transformer_nameplates_checked=nameplates_checked,
                core_nameplate_conflicts=core,
                partial_mixed_windings=partial, undeclared_wye_sequence_loads=loads,
                zero_sequence_completion='disabled by native Flag_LFZ0=1',
                classification='Core conflicts violate the published excitation formula. Partial mixed windings and Wye loads require native-semantic mapping; neither is classified as malformed solely because it is unsupported.')


def check(source, records, reader):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identity = next(c for c in manifest['cases'] if c['case'] == 6)
    for path, prefix in ((source, 'source'), (records, 'record')):
        if digest(path) != identity[f'{prefix}_sha256'] or path.stat().st_size != identity[f'{prefix}_bytes']:
            raise ValueError(f'{prefix} identity mismatch')
    raw = json.loads(records.read_text())
    tables = {t['name']: [dict(zip([c['name'] for c in t['columns']], r, strict=True))
                          for r in t['rows']] for t in raw['tables']}
    audit, error = invoke(reader, records, 'audit', 0.0)
    if error is not None:
        raise ValueError(f'audit context failed: {error}')
    result = classify(tables, audit)
    if (result['component_count'], result['mapped'], result['transformer_nameplates_checked'],
            len(result['core_nameplate_conflicts']), len(result['partial_mixed_windings']),
            len(result['undeclared_wye_sequence_loads'])) != (218, 163, 51, 11, 9, 35):
        raise ValueError('coverage changed; review this development audit before updating its evidence')
    controls = []
    for mutation in ('missing_component', 'accepted_bad_core', 'wrong_core_reason', 'ignored_load_failure'):
        wrong = copy.deepcopy(audit)
        if mutation == 'missing_component':
            wrong['components'].pop()
        else:
            target = (result['undeclared_wye_sequence_loads'][0] if mutation == 'ignored_load_failure'
                      else result['core_nameplate_conflicts'][0]['element'])
            c = next(c for c in wrong['components'] if c['element'] == target)
            if mutation in ('accepted_bad_core', 'ignored_load_failure'):
                c['component_maps'] = True
            else:
                c['first_error'] = WINDING_ERROR
        try:
            changed = classify(tables, wrong)
            if changed != result:
                raise ValueError('accounting changed')
        except ValueError:
            controls.append(mutation)
        else:
            raise ValueError(f'counterexample {mutation} was accepted')
    return dict(scope=__doc__, case=6, variant=1, snapshot_hours=0, **result,
                source_sha256=digest(source), record_sha256=digest(records), reader_sha256=digest(reader),
                attribution=manifest['attribution'], collection=manifest['collection'], license=manifest['license'],
                negative_controls=controls, native_execution=False, complete_parse=False,
                blocker_audit_passed=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('source', 'records', 'reader', 'report'):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    result = check(args.source, args.records, args.reader.resolve())
    args.report.write_text(json.dumps(result, indent=2) + '\n')
    print(f"{result['rejected']} rejections classified: 11 nameplate conflicts, 9 mixed windings, 35 Wye loads")
