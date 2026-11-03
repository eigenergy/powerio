#!/usr/bin/env python3
"""Y0 component checks: OpenDSS rotating sequences, native longitudinal zero
sequence equation, and two authentic neutral-tap connections. No complete
native feeder or native SINCAL solver acceptance is claimed.
"""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from opendssdirect import dss


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def check(synthetic, records, source, native):
    rows = json.loads(synthetic.read_text())
    errors = []
    for c in rows:
        v1, v2 = c['kv']
        r, kva = c['ur_percent'], c['kva']
        x = np.sqrt(c['uk_percent']**2-r*r)
        dss(f'clear\nnew circuit.check basekv={v1}\n'
            f'new transformer.subject phases=3 windings=2 buses=[h.1.2.3.4 l.1.2.3.4] '
            f'conns=[wye wye] kvs=[{v1} {v2}] kvas=[{kva} {kva}] %rs=[{r/2} {r/2}] '
            f'xhl={x:.17g} %noloadloss=0 %imag=0 ppm_antifloat=0\n'
            '~ wdg=1 rneut=-1\n~ wdg=2 rneut=-1\nsolve')
        dss.Circuit.SetActiveElement('Transformer.subject')
        raw = np.asarray(dss.CktElement.YPrim())
        y = (raw[::2]+1j*raw[1::2]).reshape(8,8,order='F')
        external, neutral = [0,1,2,4,5,6], [3,7]
        # The pair of floating stars has one free common reference. Its
        # pseudoinverse eliminates only determined internal differences.
        a, b, e = y[np.ix_(external,external)], y[np.ix_(external,neutral)], y[np.ix_(neutral,neutral)]
        yy = a-b@np.linalg.pinv(e, rcond=1e-12)@b.T
        z0 = complex(r*c['r0_r1'],x*c['x0_x1'])/100*(v2*1000)**2/(kva*1000)
        # Siemens Input Data p189: I0=(U01-U02)/Z0. This is a separate
        # manual-equation check, not an OpenDSS native AutoTrans comparison.
        incidence = np.array([1,1,1,-1,-1,-1])
        longitudinal = np.outer(incidence,incidence)/(3*z0)
        expected = yy+longitudinal
        actual = np.asarray(c['y_re'])+1j*np.asarray(c['y_im'])
        if not np.all(np.isfinite(actual)) or actual.shape != (6,6):
            raise ValueError('invalid primitive')
        error = float(np.max(abs(actual-expected))/np.max(abs(expected)))
        if error >= 1e-11:
            raise ValueError('Y0 primitive disagrees with rotating/zero sequence checks')
        errors.append(error)
    if len(rows) != 2 or sorted(c['kv'] for c in rows) != [[0.4,11.0],[11.0,0.4]]:
        raise ValueError('both voltage directions required')
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identity = next(c for c in manifest['cases'] if c['case']==1)
    if digest(records) != identity['record_sha256'] or digest(source) != identity['source_sha256']:
        raise ValueError('native source/acquisition identity mismatch')
    tables={t['name']:[dict(zip([c['name'] for c in t['columns']],r,strict=True)) for r in t['rows']] for t in json.loads(records.read_text())['tables']}
    selected={r['Element_ID']:r for r in tables['TwoWindingTransformer'] if r['Variant_ID']==1 and r['VecGrp']==71 and r['Un1']==r['Un2'] and r['roh']==r['rohm']}
    switches=json.loads(native.read_text())
    if len(switches)!=2 or {int(s['name']) for s in switches}!=set(selected) or set(selected)!={2451,2453}:
        raise ValueError('all native neutral-tap devices required')
    for s in switches:
        eid=int(s['name']); row=selected[eid]
        ports=sorted([p for p in tables['Terminal'] if p['Variant_ID']==1 and p['Element_ID']==eid],key=lambda p:p['TerminalNo'])
        if len(ports)!=2 or any(p['Flag_Terminal'] not in [1,2,3] for p in ports):
            raise ValueError('unexpected native profile')
        phase=[str(ports[0]['Flag_Terminal'])]
        if (s['bus_from']!=str(ports[0]['Node_ID']) or s['bus_to']!=str(ports[1]['Node_ID']) or s['terminal_map_from']!=phase or s['terminal_map_to']!=phase or s['open'] or any(p['Flag_State']!=1 for p in ports) or row['Vfe'] or row['i0']):
            raise ValueError('native ideal connection mismatch')
    return {'scope':__doc__, 'synthetic_count':len(rows), 'maximum_relative_primitive_error':max(errors),
            'relative_tolerance':1e-11, 'native_ideal_elements':sorted(selected),
            'source_sha256':digest(source),'record_sha256':digest(records),
            'synthetic_export_sha256':digest(synthetic),'native_export_sha256':digest(native),
            'collection':manifest['collection'],'license':manifest['license'],'attribution':manifest['attribution'],
            'engine':dss.Basic.Version(),'numpy':np.__version__,'native_execution':False,'passed':True}


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for arg in ['synthetic','records','source','native']:p.add_argument(arg,type=Path)
    p.add_argument('--report',type=Path,required=True)
    a=p.parse_args();r=check(a.synthetic,a.records,a.source,a.native)
    a.report.write_text(json.dumps(r,indent=2)+'\n')
    print(f"{r['synthetic_count']} finite Y0 primitives and {len(r['native_ideal_elements'])} native ideal connections passed")
