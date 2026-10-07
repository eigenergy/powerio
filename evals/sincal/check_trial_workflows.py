#!/usr/bin/env python3
"""Exercise an installed wheel on externally held cases; never vendor model data.

Checks parse, source echo, IR, PF preparation, and ordinary emission separately.
Numerical oracle evidence lives in the case-specific checkers.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path

import powerio as pio


def check(source, records, family, hours=None, compatibility=False):
    raw = source.read_bytes()
    kwargs = {'format': 'sincal-' + family}
    if records:
        selection = dict(variant=1, snapshot_hours=hours, acquired_tables='records.json')
        if family == 'balanced':
            kwargs['sincal_balanced'] = pio.SincalBalancedReadOptions(**selection)
        else:
            kwargs['sincal_multiconductor'] = pio.dist.SincalReadOptions(**selection,
                assume_inactive_source_controls=compatibility)
        kwargs['named_buffers'] = {'records.json': records.read_bytes()}
    module = pio.parse(raw, name=source.name, **kwargs)
    expected = pio.BalancedNetwork if family == 'balanced' else pio.dist.MulticonductorNetwork
    assert isinstance(module.value, expected)
    assert pio.emit(module, 'sincal').artifacts[0].data == raw
    wire = json.loads(pio.serialize(module).text)
    restored = pio.deserialize(io.StringIO(json.dumps(wire)))
    assert wire['value'] == json.loads(pio.serialize(restored).text)['value']
    try:
        pio.emit(restored, 'sincal')
    except pio.PowerIOError:
        pass
    else:
        raise AssertionError('IR restoration unexpectedly recreated native bytes')
    try:
        instance = module.to_ac_pf_instance() if family == 'balanced' else module.to_mc_ac_pf_instance()
        pf = {'constructed': True, 'type': type(instance.value).__name__}
    except pio.PowerIOError as error:
        pf = {'constructed': False, 'code': error.code, 'message': str(error)}
    emissions = {}
    for target in (['matpower'] if family == 'balanced' else ['dss', 'pmd-json', 'bmopf-json']):
        try:
            output = pio.emit(module, target)
            # Reparse only where a single artifact fully represents the output.
            reparsed = pio.parse(output.artifacts[0].data, format=target)
            assert isinstance(reparsed.value, expected)
            emissions[target] = {'emitted_and_reparsed': True,
                'diagnostic_codes': [d['code'] for d in pio.diagnostic_records(output.diagnostics)]}
        except pio.PowerIOError as error:
            emissions[target] = {'emitted_and_reparsed': False, 'code': error.code, 'message': str(error)}
    return {'source_sha256': hashlib.sha256(raw).hexdigest(), 'source_bytes': len(raw),
        'family': family, 'snapshot_hours': hours, 'experimental_compatibility': compatibility,
        'buses_including_port_buses': module.value.n_buses, 'loads': len(module.value.loads),
        'source_echo': True, 'typed_ir': True, 'ir_cannot_echo_native': True,
        'pf_preparation': pf, 'ordinary_emission': emissions,
        'source_inventory_entries': sum('retention' in d.get('details', {}) for d in pio.diagnostic_records(module.diagnostics)),
        'numerical_validation_in_this_check': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['csiro19', 'records19', 'csiro09', 'records09', 'csiro12', 'records12', 'truong12', 'report']:
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    cases = {
        'CSIRO19': check(args.csiro19, args.records19, 'balanced', 12),
        'CSIRO09': check(args.csiro09, args.records09, 'multiconductor', 12),
        'CSIRO12': check(args.csiro12, args.records12, 'multiconductor', 12, True),
        'Truong12': check(args.truong12, None, 'multiconductor'),
    }
    assert cases['CSIRO19']['pf_preparation']['constructed']
    assert cases['CSIRO19']['ordinary_emission']['matpower']['emitted_and_reparsed']
    assert not cases['CSIRO09']['pf_preparation']['constructed']
    assert cases['CSIRO12']['pf_preparation']['constructed']
    assert cases['Truong12']['pf_preparation']['constructed']
    for case in ['CSIRO09', 'CSIRO12']:
        for emission in cases[case]['ordinary_emission'].values():
            assert not emission['emitted_and_reparsed']
            assert 'referenc' in emission['message'].lower()
    args.report.write_text(json.dumps({'powerio_version': pio.__version__, 'binding': 'installed Python wheel',
        'cases': cases, 'passed': True}, indent=2) + '\n')
    print(json.dumps({name: {'pf': c['pf_preparation']['constructed'],
        'emission': {k: v['emitted_and_reparsed'] for k, v in c['ordinary_emission'].items()}} for name, c in cases.items()}))


if __name__ == '__main__':
    main()
