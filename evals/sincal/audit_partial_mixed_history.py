#!/usr/bin/env python3
"""Audit CSIRO06 partial YNd1 transformers against their stored terminal results.

This deliberately evaluates an UNACCEPTED independent-coil hypothesis: one
single-phase transformer per installed coil, rated at Sn/3, with SINCAL's nominal
pi excitation split between its ends. It does not encode the distinct native
zero-sequence impedance. It must not be promoted to reader support merely
because OpenDSS can represent the circuit. Stored results do not attest that
current inputs were used to calculate them. No fitting, repair or native run.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

from audit_distribution import digest
from check_shunt_history import key, selected_rows


def candidate(row):
    # The native coil connection is YNd1 W1: primary L1-earth, secondary L1-L2.
    # This candidate is physical independent-coil modelling, NOT native acceptance.
    high, low, kva = row['Un1'] / np.sqrt(3), row['Un2'], row['Sn'] * 1000 / 3
    r, x = row['ur'], np.sqrt(row['uk']**2 - row['ur']**2)
    dss(f'Clear\nNew Circuit.check basekv={row["Un1"]}\n'
        f'New Transformer.subject phases=1 windings=2 buses=[h.1.0 l.1.2] '
        f'conns=[wye wye] kvs=[{high:.17g} {low:.17g}] kvas=[{kva:.17g} {kva:.17g}] '
        f'%rs=[{r/2:.17g} {r/2:.17g}] xhl={x:.17g} %noloadloss=0 %imag=0 ppm_antifloat=0\n'
        '~ wdg=1 rneut=-1\n~ wdg=2 rneut=-1\nSolve')
    dss.Circuit.SetActiveElement('Transformer.subject')
    if dss.CktElement.NodeOrder() != [1, 0, 1, 2]:
        raise ValueError('unexpected OpenDSS candidate terminal order')
    raw = np.array(dss.CktElement.YPrim())
    y = (raw[::2] + 1j*raw[1::2]).reshape(4, 4, order='F')
    watts, va = row['Vfe']*1000, row['i0']/100*row['Sn']*1e6
    if watts > va + 8*np.finfo(float).eps*max(watts, va):
        raise ValueError('candidate input has inconsistent core nameplate')
    core = complex(watts, -np.sqrt(max(0., (va-watts)*(va+watts))))
    for side, kv in enumerate([high, low]):
        a = core / (6*(kv*1000)**2)
        y[2*side:2*side+2, 2*side:2*side+2] += a*np.array([[1, -1], [-1, 1]])
    return y


def volts(row):
    return np.array([row[f'U{i}']*1000*np.exp(1j*np.deg2rad(row[f'phi{i}'])) for i in [1, 2, 3]])


def unique(rows):
    indexed = {key(r):r for r in rows}
    if len(indexed) != len(rows):
        raise ValueError('duplicate historical result identity')
    return indexed


def audit(source, records):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identity = next(c for c in manifest['cases'] if c['case'] == 6)
    if digest(source) != identity['source_sha256'] or digest(records) != identity['record_sha256']:
        raise ValueError('native input identity mismatch')
    tables = {t['name']:[dict(zip([c['name'] for c in t['columns']], r, strict=True)) for r in t['rows']]
              for t in json.loads(records.read_text())['tables']}
    ports = {}
    for r in tables['Terminal']:
        if r['Variant_ID'] == 1:
            ports.setdefault(r['Element_ID'], []).append(r)
    selected = {r['Element_ID']:r for r in tables['TwoWindingTransformer'] if r['Variant_ID'] == 1
                and r['VecGrp'] == 14 and any(p['Flag_Terminal'] != 7 for p in ports[r['Element_ID']])}
    if len(selected) != 9:
        raise ValueError('native partial YNd1 inventory changed')
    node_cache = {}
    comparisons = []
    for eid, row in sorted(selected.items()):
        pair = sorted(ports[eid], key=lambda p:p['TerminalNo'])
        if (len(pair) != 2 or [p['TerminalNo'] for p in pair] != [1, 2]
                or any(p['Flag_Terminal'] not in [1, 2, 3] or p['Flag_State'] != 1 for p in pair)
                or pair[0]['Flag_Terminal'] != pair[1]['Flag_Terminal']
                or row['Flag_roh'] != 1 or row['roh'] != row['rohm'] or row['AddRotate'] != 0
                or row['Stp_ID1'] or row['Stp_ID2'] or row['Flag_Z0_Input'] != 2):
            raise ValueError('native circuit outside candidate audit contract')
        nodes, branches = [], []
        for p in pair:
            node = p['Node_ID']
            if node not in node_cache:
                node_cache[node] = unique(selected_rows(source, 'ULFNodeResult', 'Node_ID', node))
            nodes.append(node_cache[node])
            branches.append(unique(selected_rows(source, 'ULFBranchResult', 'Terminal1_ID', p['Terminal_ID'])))
        keys = set(nodes[0])
        if not keys or any(set(rows) != keys for rows in nodes + branches):
            raise ValueError('node and branch result snapshots do not align')
        winding = pair[0]['Flag_Terminal'] - 1
        second = (winding + 1) % 3
        y = candidate(row)
        power_errors, current_errors = [], []
        minimum_stored_current = float('inf')
        for k in sorted(keys):
            if k[0] != 1 or k[3] != 0:
                raise ValueError('unexpected historical variant/result mode')
            v1, v2 = [volts(rows[k]) for rows in nodes]
            vv = np.array([v1[winding], 0, v2[winding], v2[second]])
            currents = y@vv
            powers = -vv*np.conj(currents)
            for side, mapping in enumerate([[(winding+1, 0)], [(winding+1, 2), (second+1, 3)]]):
                branch = branches[side][k]
                if (branch['Terminal2_ID'] != pair[1-side]['Terminal_ID']
                        or branch['Flag_State'] not in [1, 2]
                        or any(branch[f'{f}{p}'] != 0 for p in set([1, 2, 3])-set(p for p, _ in mapping) for f in ['P', 'Q', 'I'])):
                    raise ValueError('unexpected historical terminal/state/phase declaration')
                for phase, index in mapping:
                    stored_power = complex(branch[f'P{phase}'], branch[f'Q{phase}'])*1e6
                    stored_current = branch[f'I{phase}']*1000
                    minimum_stored_current = min(minimum_stored_current, stored_current)
                    power_errors.append(abs(powers[index]-stored_power))
                    current_errors.append(abs(abs(currents[index])-stored_current))
        if not np.all(np.isfinite(power_errors + current_errors)):
            raise ValueError('nonfinite comparison')
        comparisons.append(dict(element=eid, snapshots=len(keys), active_windings=[winding+1],
            native_zero_sequence_ohm=[row['R0'], row['X0']],
            maximum_terminal_power_error_va=max(power_errors),
            maximum_terminal_current_magnitude_error_a=max(current_errors),
            minimum_stored_terminal_current_a=minimum_stored_current,
            candidate_matches_stored_results=bool(max(power_errors)<1e-3 and max(current_errors)<1e-6)))
    return dict(scope=__doc__, source_sha256=digest(source), record_sha256=digest(records),
        collection=manifest['collection'], attribution=manifest['attribution'], license=manifest['license'],
        engine=dss.Basic.Version(), numpy=np.__version__, comparisons=comparisons,
        compared_components=len(comparisons), compared_snapshots=sum(r['snapshots'] for r in comparisons),
        candidate_accepted=False, production_mapping_added=False,
        disposition='Independent-coil hypothesis does not establish native partial mixed-winding semantics; retain rejection and resolve distinct zero-sequence/core/rating treatment.',
        native_execution=False, parameters_fitted=False, source_data_changed=False,
        stored_results_attest_current_inputs=False, audit_complete=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'records', 'report']:
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    result = audit(args.source, args.records)
    args.report.write_text(json.dumps(result, indent=2)+'\n')
    print(f"Audited {result['compared_components']} partial transformers; candidate matches {sum(c['candidate_matches_stored_results'] for c in result['comparisons'])} stored cases. No production mapping accepted.")
