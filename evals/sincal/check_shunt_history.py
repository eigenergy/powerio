#!/usr/bin/env python3
"""CSIRO03 local reactor check against aligned historical node/branch records.

Uses measured terminal voltages, not a feeder solve. No fitted parameters,
input repair, historical-result fixture or native SINCAL execution.
"""
import argparse
import json
import math
import subprocess
import threading
from pathlib import Path

from audit_distribution import digest


def selected_rows(source, table, field, identity):
    rows=[]
    with subprocess.Popen(['mdb-json',str(source),table],stdout=subprocess.PIPE,stderr=subprocess.PIPE) as process:
        timer=threading.Timer(30,process.kill);timer.start()
        try:
            size=0
            for raw in process.stdout:
                size+=len(raw)
                if len(raw)>1024*1024 or size>256*1024*1024:
                    process.kill();raise ValueError('native result extraction exceeded byte bound')
                row=json.loads(raw)
                if row.get(field)==identity:rows.append(row)
            if process.wait()!=0:
                raise ValueError('native result extraction failed or exceeded deadline')
        finally:
            timer.cancel()
    return rows


def key(row):
    # Result Flag_State is a limit-status flag (1 OK, 2 limit violation),
    # not the equipment service state. Node and branch limits can differ.
    return tuple(row.get(k) for k in ('Variant_ID','ResDate','ResTime','Flag_Result'))


def check(source, export):
    manifest=json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identity=next(c for c in manifest['cases'] if c['case']==3)
    if digest(source)!=identity['source_sha256']:
        raise ValueError('native source identity mismatch')
    c=next(c for c in json.loads(export.read_text())['native'] if c['case']==3 and c['element']==3766)
    if c['shunt']['terminal_map']!=['1','earth'] or c['bus']['grounded']!=['earth'] or c['switch']['open']:
        raise ValueError('reactor does not retain its phase-earth circuit')
    ports=selected_rows(source,'Terminal','Element_ID',3766)
    banks=selected_rows(source,'ShuntReactor','Element_ID',3766)
    if len(ports)!=1 or len(banks)!=1:
        raise ValueError('ambiguous native reactor records')
    port,bank=ports[0],banks[0]
    if (port['Flag_Terminal']!=1 or port['Flag_State']!=1 or bank['Flag_Z0']!=0
            or bank['Flag_roh']!=1 or bank['roh']!=bank['rohm'] or bank['Vcu']+bank['Vfe']!=0):
        raise ValueError('historical reactor input profile changed')
    nodes=selected_rows(source,'ULFNodeResult','Node_ID',port['Node_ID'])
    branches=selected_rows(source,'ULFBranchResult','Terminal1_ID',port['Terminal_ID'])
    by_node={key(r):r for r in nodes};by_branch={key(r):r for r in branches}
    if len(by_node)!=49 or len(by_node)!=len(nodes) or set(by_node)!=set(by_branch) or len(branches)!=49:
        raise ValueError('all49 historical snapshots must align uniquely')
    y=complex(c['shunt']['g'][0][0],c['shunt']['b'][0][0])
    nominal_y=-3j*bank['Sn']/bank['Un']**2
    if abs(y-nominal_y)>1e-14:
        raise ValueError('Rust primitive differs from independently rated reactor')
    errors=[];wrong=[]
    for k,node in by_node.items():
        branch=by_branch[k]
        if (k[0]!=1 or k[3]!=1 or node['Flag_State'] not in (1,2) or branch['Flag_State'] not in (1,2)
                or any(branch[f'{f}{p}']!=0 for p in (2,3) for f in ('P','Q','I'))):
            raise ValueError('unexpected historical phase or result state')
        v=node['U1']*1000*complex(math.cos(math.radians(node['phi1'])),math.sin(math.radians(node['phi1'])))
        current=y*v
        injection=-v*current.conjugate()/1e6
        actual=complex(branch['P1'],branch['Q1'])
        errors.append((abs(injection-actual)*1e6,abs(abs(current)-branch['I1']*1000)))
        wrong.append(abs(injection/3-actual)*1e6)
        if abs(branch['Ie']-branch['I1'])>1e-12:
            raise ValueError('historical earth return differs from phase current')
    power=max(e[0] for e in errors);current=max(e[1] for e in errors)
    if power>1e-6 or current>1e-9 or min(wrong)<1:
        raise ValueError('historical local equations or factor-three control failed')
    return dict(scope=__doc__,case=3,element=3766,snapshots=49,
                node_limit_violations=sum(n['Flag_State']==2 for n in nodes),
                maximum_power_error_va=power,maximum_current_error_a=current,
                minimum_wrong_factor_three_error_va=min(wrong),power_tolerance_va=1e-6,current_tolerance_a=1e-9,
                source_sha256=digest(source),export_sha256=digest(export),
                collection=manifest['collection'],attribution=manifest['attribution'],license=manifest['license'],
                native_execution=False,whole_feeder_validation=False,passed=True)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('source','export','report'):parser.add_argument(name,type=Path)
    a=parser.parse_args();r=check(a.source,a.export)
    a.report.write_text(json.dumps(r,indent=2)+'\n')
    print('49 aligned historical reactor snapshots passed without parameter fitting')
