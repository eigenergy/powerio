#!/usr/bin/env python3
"""Independent constant-impedance OpenDSS circuits for rated SINCAL shunt banks.

Tests actual Rust conductor primitives, port identities and service states.
Original MDB files and acquired records remain external. No native execution.
"""
import argparse
import copy
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss

from audit_distribution import digest

PHASES = {1:['1'], 2:['2'], 3:['3'], 4:['1','2'], 5:['2','3'], 6:['3','1'], 7:['1','2','3']}


def reference(kind, selection, grounded, kva, loss_kw, kv):
    phases = PHASES[selection]
    n = len(phases)
    # Each OpenDSS branch is a separately parameterized constant-impedance
    # load. Negative kvar supplies capacitive power; positive kvar absorbs it.
    # Divide total nameplate power only when there are three installed branches.
    q = np.sqrt(kva*kva-loss_kw*loss_kw) * (1 if kind == 'ShuntReactor' else -1)
    pair = n == 2
    branches = 1 if pair else n
    size = 2 if pair else n+1
    y = np.zeros((size,size),complex)
    dss('Clear\nNew Circuit.check basekv=0.4 phases=3\nSet frequency=50')
    for i in range(branches):
        branch_kv = kv if pair else kv/np.sqrt(3)
        dss(f'New Load.b{i} phases=1 bus1=bank.{i+1}.{size} conn=wye model=2 '
            f'kv={branch_kv:.17g} kw={loss_kw/branches:.17g} kvar={q/branches:.17g} basefreq=50')
    dss('Solve')
    for i in range(branches):
        dss.Circuit.SetActiveElement(f'Load.b{i}')
        raw = np.asarray(dss.CktElement.YPrim())
        primitive = (raw[::2]+1j*raw[1::2]).reshape(2,2,order='F')
        # OpenDSS adds a tiny neutral-diagonal numerical regularizer. The
        # physical branch is determined by its first current row; reconstruct
        # the return row from KCL rather than treating regularization as earth.
        primitive[1] = -primitive[0]
        positions = [i,size-1]
        y[np.ix_(positions,positions)] += primitive
    if n == 3 and not grounded:
        y = y[:3,:3] - np.outer(y[:3,3],y[3,:3])/y[3,3]
    return y


def validate(c, *, kind, selection, grounded, kva, loss_kw, kv, bus_id, opened):
    phases = PHASES[selection]
    terminals = phases + (['earth'] if grounded else [])
    shunt,bus,switch = c['shunt'],c['bus'],c['switch']
    if (bus['terminals'] != terminals or bus['grounded'] != (['earth'] if grounded else [])
            or shunt['terminal_map'] != terminals or shunt['bus'] != bus['id']
            or switch['bus_from'] != str(bus_id) or switch['bus_to'] != bus['id']
            or switch['terminal_map_from'] != phases or switch['terminal_map_to'] != phases
            or switch['open'] != opened):
        raise ValueError('bank identity, phase, earth or service state mismatch')
    actual = np.asarray(shunt['g'])+1j*np.asarray(shunt['b'])
    expected = reference(kind,selection,grounded,kva,loss_kw,kv)
    if actual.shape != expected.shape or not np.all(np.isfinite(actual)):
        raise ValueError('invalid bank conductor primitive')
    error = float(np.max(abs(actual-expected))/np.max(abs(expected)))
    if error > 1e-10 or np.max(abs(actual-actual.T)) > 1e-12:
        raise ValueError(f'OpenDSS bank primitive mismatch: {error}')
    if np.max(abs(actual.sum(axis=1))) > 1e-12:
        raise ValueError('bank primitive violates conductor KCL')
    return error


def tables(record):
    return {t['name']: [dict(zip([c['name'] for c in t['columns']],row,strict=True))
                        for row in t['rows']] for t in record['tables']}


def check(export, source_dir, records_dir, original_records_dir):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identities = {c['case']:c for c in manifest['cases']}
    data = json.loads(export.read_text())
    expected_synthetic = [(k,s) for k in ('ShuntReactor','ShuntCondensator') for s in range(1,8)]
    if [(c['kind'],c['selection']) for c in data['synthetic']] != expected_synthetic:
        raise ValueError('incomplete original synthetic coverage')
    checks = []
    controls = 0
    for c in data['synthetic']:
        kwargs = dict(kind=c['kind'],selection=c['selection'],grounded=c['selection'] not in (4,5,6),
                      kva=60,loss_kw=.6,kv=.4,bus_id=30,opened=False)
        error = validate(c,**kwargs)
        checks.append(dict(kind=c['kind'],selection=c['selection'],relative_error=error))
        if c['selection'] == 1:
            for mutation in ('factor_three','reactive_sign','grounding','service_state'):
                bad = copy.deepcopy(c)
                if mutation == 'factor_three': bad['shunt']['b'] = (np.asarray(bad['shunt']['b'])/3).tolist()
                elif mutation == 'reactive_sign': bad['shunt']['b'] = (-np.asarray(bad['shunt']['b'])).tolist()
                elif mutation == 'grounding': bad['bus']['grounded'] = []
                else: bad['switch']['open'] = True
                try: validate(bad,**kwargs)
                except ValueError: controls += 1
                else: raise ValueError('corrupted bank mapping accepted')
    native = []
    expected_ids = set()
    for case in (3,12,16,17):
        identity = identities[case]
        source = source_dir/f'csiro-representative{case:02}.mdb'
        old_path = original_records_dir/f'representative{case:02}.json'
        path = records_dir/f'representative{case:02}.json'
        if digest(source) != identity['source_sha256'] or digest(old_path) != identity['record_sha256']:
            raise ValueError('native or original acquisition identity mismatch')
        old = json.loads(old_path.read_text()); record = json.loads(path.read_text())
        by_name = {t['name']:t for t in record['tables']}
        if record['source'] != old['source'] or any(by_name[t['name']] != t for t in old['tables']):
            raise ValueError('new acquisition altered existing tables or source identity')
        if set(by_name) != {t['name'] for t in old['tables']} | {'ShuntReactor','ShuntCondensator'}:
            raise ValueError('unexpected acquisition extension')
        ts = tables(record)
        elements = {r['Element_ID']:r for r in ts['Element'] if r['Variant_ID']==1}
        ports = {r['Element_ID']:r for r in ts['Terminal'] if r['Variant_ID']==1 and r['Element_ID'] in elements
                 and elements[r['Element_ID']]['Type'].strip() in ('ShuntReactor','ShuntCondensator')}
        for kind in ('ShuntReactor','ShuntCondensator'):
            for r in ts[kind]:
                if r['Variant_ID'] != 1: continue
                eid = r['Element_ID']; expected_ids.add((case,eid)); port=ports[eid]
                matches=[c for c in data['native'] if (c['case'],c['element'])==(case,eid)]
                if len(matches)!=1: raise ValueError('missing or duplicated native bank')
                c=matches[0]; selection=port['Flag_Terminal']
                loss=((r['Vcu'] or 0)+(r['Vfe'] or 0)) if kind=='ShuntReactor' else (r['Vdi'] or 0)
                if r['Flag_roh'] not in (None,0,1) or (r['Flag_roh']==1 and r['roh']!=r['rohm']):
                    raise ValueError('native bank is outside the selected nominal fixed profile')
                grounded=selection in (1,2,3) or (selection==7 and r['Flag_Z0']==1)
                if selection==7 and grounded and not (r['Flag_Z0_Input']==1 and r['Z0_Z1']==1 and r['R0_X0']==0 and loss==0):
                    raise ValueError('native grounded bank requires independent coupled-sequence check')
                opened=port['Flag_State']==0 or elements[eid]['Flag_State']==0
                error=validate(c,kind=kind,selection=selection,grounded=grounded,kva=r['Sn']*1000,
                               loss_kw=loss,kv=r['Un'],bus_id=port['Node_ID'],opened=opened)
                native.append(dict(case=case,element=eid,kind=kind,selection=selection,opened=opened,
                                   defaulted=c['defaulted'],relative_error=error,
                                   source_sha256=digest(source),record_sha256=digest(path)))
    if {(c['case'],c['element']) for c in data['native']} != expected_ids or len(data['native'])!=len(expected_ids):
        raise ValueError('unexpected native export coverage')
    return dict(scope=__doc__,synthetic=checks,native=native,negative_controls=controls,
                maximum_relative_primitive_error=max(c['relative_error'] for c in checks+native),tolerance=1e-10,
                export_sha256=digest(export),attribution=manifest['attribution'],collection=manifest['collection'],
                license=manifest['license'],engine=dss.Basic.Version(),numpy=np.__version__,
                native_execution=False,whole_feeder_validation=False,passed=True)


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('export','source_dir','records_dir','original_records_dir','report'):
        parser.add_argument(name,type=Path)
    a=parser.parse_args();r=check(a.export,a.source_dir,a.records_dir,a.original_records_dir)
    a.report.write_text(json.dumps(r,indent=2)+'\n')
    print(f"{len(r['synthetic'])} original and {len(r['native'])} native shunt banks passed")
