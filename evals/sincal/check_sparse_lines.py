#!/usr/bin/env python3
"""Verify sparse schema-11.5 line inputs against independent OpenDSS circuits.

Native NULLs are retained. Only enumerated manual defaults are interpreted.
Reduced-phase charging must be independent; for coupled series-only lines,
eliminate absent currents from OpenDSS's full three-phase admittance, fixing
one unused endpoint per absent conductor as an arbitrary zero-current gauge.
No native execution or complete feeder validation is claimed.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from opendssdirect import dss
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components

from audit_distribution import digest
from check_csiro09_network import tables


def expected_defaults(row, level=None):
    fields = ['Flag_Ll','Flag_Ground','Flag_Macro','ParSys','fr']
    if row['Flag_LineTyp'] != 3:
        fields += ['fn','va']
    out = [f for f in fields if row[f] is None]
    if level is not None and row['Flag_LineTyp'] != 3:
        field = {1:'Temp_Cable',2:'Temp_Line'}[row['Flag_LineTyp']]
        if level[field] is None: out.append('VoltageLevel.'+field)
        temperature = 20 if level[field] is None else level[field]
        if temperature != 20 and row['alpha'] is None: out.append('alpha')
    return sorted(out)


def oracle_primitive(row, selected, frequency):
    rated = 50 if row['fn'] is None else row['fn']
    parallel = 1 if row['ParSys'] is None else row['ParSys']
    if (rated != 50 or frequency != 50 or parallel != 1 or row['va'] is not None
            or row['Flag_Z0_Input'] != 2):
        raise ValueError('native electrical profile changed')
    dss('Clear\nNew Circuit.check basekv=22 phases=3\n'
        f'New Line.subject bus1=a bus2=b phases=3 units=km length={row["l"]:.17g} '
        f'r1={row["r"]:.17g} x1={row["x"]:.17g} r0={row["r0"]:.17g} x0={row["x0"]:.17g} '
        f'c1={row["c"]:.17g} c0={row["c0"]:.17g} basefreq={rated}\nSet frequency=50\nSolve')
    dss.Circuit.SetActiveElement('Line.subject')
    raw = np.asarray(dss.CktElement.YPrim())
    full = (raw[::2]+1j*raw[1::2]).reshape(6,6,order='F')
    keep = selected+[p+3 for p in selected]
    expected = full[np.ix_(keep,keep)]
    absent = [p+3 for p in range(3) if p not in selected]
    coupled_reduced = False
    if absent:
        if row['c'] != 0 or row['c0'] != 0:
            if any(row[a] != row[b] for a,b in [('r','r0'),('x','x0'),('c','c0')]):
                raise ValueError('coupled reduced-phase charging is outside this oracle')
        else:
            expected = expected-full[np.ix_(keep,absent)]@np.linalg.solve(full[np.ix_(absent,absent)],full[np.ix_(absent,keep)])
            coupled_reduced = row['r'] != row['r0'] or row['x'] != row['x0']
    wrong_projection = None
    if coupled_reduced:
        wrong_projection = float(np.max(abs(full[np.ix_(keep,keep)]-expected))/np.max(abs(expected)))
        if wrong_projection < 1e-8:
            raise ValueError('absent-current negative control did not detect a difference')
    return expected, coupled_reduced, wrong_projection


def connection_graph(case, native, ports, elements):
    native_edges = []
    typed_edges = []
    graph = case['graph']
    exported = case['connections']
    if len(graph['edges']) != len(exported): raise ValueError('connection graph count mismatch')
    by_id = {e['id']:e for e in graph['edges']}
    opened = 0
    for c in exported:
        i=c['element'];row=native[i];s=c['switch'];ps=sorted(ports[i],key=lambda p:p['TerminalNo'])
        if len(ps)!=2 or any(p['Flag_Terminal']!=7 for p in ps): raise ValueError('connection port profile changed')
        is_open = elements[i]['Flag_State']!=1 or any(p['Flag_State']!=1 for p in ps)
        rating = row['Ith']*1000*(1 if row['ParSys'] is None else row['ParSys'])*(1 if row['fr'] is None else row['fr'])
        if (s['bus_from']!=str(ps[0]['Node_ID']) or s['bus_to']!=str(ps[1]['Node_ID'])
                or s['terminal_map_from']!=['1','2','3'] or s['terminal_map_to']!=['1','2','3']
                or s['open']!=is_open or s['i_max']!=([rating]*3 if rating else None)
                or sorted(c['defaulted'])!=expected_defaults(row)):
            raise ValueError('connection endpoints, state, limits or provenance changed')
        edge=by_id[s['name']]
        if (edge['kind']!='switch' or edge['from']!=s['bus_from'] or edge['to']!=s['bus_to']
                or edge['closed']==is_open or edge['conductors']!=[['1','1'],['2','2'],['3','3']]):
            raise ValueError('typed graph differs from the native connection')
        opened += int(is_open)
        if not is_open:
            native_edges.append((str(ps[0]['Node_ID']),str(ps[1]['Node_ID'])))
            typed_edges.append((edge['from'],edge['to']))
    nodes={b['id'] for b in graph['buses']}
    if any(b['grounded'] for b in graph['buses']): raise ValueError('connection graph introduced grounding')
    parent={n:n for n in nodes}
    def root(n):
        while parent[n]!=n:
            parent[n]=parent[parent[n]];n=parent[n]
        return n
    for a,b in native_edges:parent[root(a)]=root(b)
    expected={}
    for n in nodes:expected.setdefault(root(n),set()).add(n)
    index={n:i for i,n in enumerate(sorted(nodes))}
    adjacency=coo_matrix(([1]*len(typed_edges),([index[a] for a,b in typed_edges],[index[b] for a,b in typed_edges])),shape=(len(nodes),len(nodes)))
    _,labels=connected_components(adjacency,directed=False)
    actual={}
    for n,label in zip(sorted(nodes),labels,strict=True):actual.setdefault(int(label),set()).add(n)
    if {frozenset(v) for v in expected.values()}!={frozenset(v) for v in actual.values()}:raise ValueError('connection partitions differ')
    return opened


def check(source_dir, record_dir, export):
    manifest=json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    cases=json.loads(export.read_text())
    expected_counts={3:(749,0),5:(827,0),14:(40,15),15:(71,18)}
    if [c['case'] for c in cases]!=list(expected_counts):raise ValueError('case inventory differs')
    reports=[]
    for case in cases:
        n=case['case'];identity=next(c for c in manifest['cases'] if c['case']==n)
        source=source_dir/f'csiro-representative{n:02}.mdb';records=record_dir/f'representative{n:02}.json'
        if digest(source)!=identity['source_sha256'] or digest(records)!=identity['record_sha256']:raise ValueError('native identity mismatch')
        t=tables(records);native={r['Element_ID']:r for r in t['Line']};elements={r['Element_ID']:r for r in t['Element']};levels={r['VoltLevel_ID']:r for r in t['VoltageLevel']}
        ports={}
        for p in t['Terminal']:ports.setdefault(p['Element_ID'],[]).append(p)
        line_ids={i for i,r in native.items() if r['Flag_Ll'] is None and r['Flag_LineTyp']!=3}
        connection_ids={i for i,r in native.items() if r['Flag_Ll'] is None and r['Flag_LineTyp']==3}
        if (len(case['lines']),len(case['connections']))!=expected_counts[n] or {c['element'] for c in case['lines']}!=line_ids or {c['element'] for c in case['connections']}!=connection_ids:raise ValueError('component coverage differs')
        maximum=0.0;single=0;coupled=0;open_ports=0;zero_lines=[];wrong_projections=[]
        for c in case['lines']:
            i=c['element'];row=native[i];line=c['line'];code=c['code'];ps=sorted(ports[i],key=lambda p:p['TerminalNo']);level=levels[elements[i]['VoltLevel_ID']]
            if len(ps)!=2 or [p['TerminalNo'] for p in ps]!=[1,2] or ps[0]['Flag_Terminal']!=ps[1]['Flag_Terminal'] or elements[i]['Flag_State']!=1:raise ValueError('native port/state profile differs')
            selection=ps[0]['Flag_Terminal'];selected={1:[0],2:[1],3:[2],7:[0,1,2]}[selection];phases=[str(p+1) for p in selected]
            temperature=level[{1:'Temp_Cable',2:'Temp_Line'}[row['Flag_LineTyp']]]
            if temperature not in [None,20]:raise ValueError('unexpected active temperature correction')
            if (line['terminal_map_from']!=phases or line['terminal_map_to']!=phases or code['n_conductors']!=len(phases)
                    or sorted(c['defaulted'])!=expected_defaults(row,level) or line['linecode']!=code['name']
                    or not np.isclose(line['length'],row['l']*1000,rtol=1e-14,atol=0)):raise ValueError('line phase, length or default provenance differs')
            rating=row['Ith']*1000*(1 if row['ParSys'] is None else row['ParSys'])*(1 if row['fr'] is None else row['fr'])
            if code.get('i_max')!=([rating]*len(phases) if rating else None):raise ValueError('line rating differs')
            if len(c['switches'])!=sum(p['Flag_State']==0 for p in ps) or len(c['auxiliary_buses'])!=len(c['switches']):raise ValueError('open port count differs')
            for p,side in zip(ps,['from','to'],strict=True):
                if p['Flag_State']==1:
                    if line['bus_'+side]!=str(p['Node_ID']):raise ValueError('line endpoint differs')
                elif p['Flag_State']==0:
                    s=next(s for s in c['switches'] if s['name']==f"sincal:terminal:{p['Terminal_ID']}")
                    b=next(b for b in c['auxiliary_buses'] if b['id']==line['bus_'+side])
                    if not s['open'] or s['bus_from']!=str(p['Node_ID']) or s['bus_to']!=b['id'] or s['terminal_map_from']!=phases or s['terminal_map_to']!=phases or b['grounded']:raise ValueError('open terminal mapping differs')
                    open_ports+=1
                else:raise ValueError('unknown native terminal state')
            single+=int(len(selected)==1)
            if all(row[k]==0 for k in ['r','x','r0','x0','c','c0']):
                switch=c['ideal_switch']
                if (switch is None or switch['name']!=str(i) or switch['bus_from']!=line['bus_from']
                        or switch['bus_to']!=line['bus_to'] or switch['terminal_map_from']!=phases
                        or switch['terminal_map_to']!=phases or switch['open'] or switch['i_max']!=code.get('i_max')
                        or any(v!=0 for k in ['r_series','x_series','g_from','g_to','b_from','b_to'] for r in code[k] for v in r)):
                    raise ValueError('exact-zero line must remain an exact typed connection')
                zero_lines.append(i)
                continue
            if c['ideal_switch'] is not None:raise ValueError('nonzero native line was fused')
            expected,is_coupled,wrong_projection=oracle_primitive(row,selected,level['f']);coupled+=int(is_coupled)
            if wrong_projection is not None:wrong_projections.append(wrong_projection)
            z=(np.asarray(code['r_series'])+1j*np.asarray(code['x_series']))*line['length'];y=np.linalg.inv(z)
            sh=[(np.asarray(code['g_'+s])+1j*np.asarray(code['b_'+s]))*line['length'] for s in ['from','to']]
            actual=np.block([[y+sh[0],-y],[-y,y+sh[1]]])
            error=float(np.max(abs(actual-expected))/np.max(abs(expected)))
            if not np.isfinite(error) or error>1e-10:raise ValueError(f'case {n} line {i}: primitive error {error}')
            maximum=max(maximum,error)
        if {b["id"] for b in case["graph"]["buses"]} != {str(r["Node_ID"]) for r in t["Node"]}:raise ValueError("graph node identities differ")
        open_connections=connection_graph(case,native,ports,elements)
        reports.append(dict(case=n,lines=len(case['lines']),connections=len(case['connections']),single_phase_lines=single,
                            coupled_reduced_series_lines=coupled,exact_zero_line_ids=zero_lines,finite_lines=len(case['lines'])-len(zero_lines),open_line_ports=open_ports,open_connections=open_connections,
                            maximum_relative_primitive_error=maximum,minimum_wrong_projection_relative_error=min(wrong_projections) if wrong_projections else None,source_sha256=digest(source),record_sha256=digest(records)))
    return dict(scope=__doc__,cases=reports,relative_tolerance=1e-10,export_sha256=digest(export),
                collection=manifest['collection'],attribution=manifest['attribution'],license=manifest['license'],
                engine=dss.Basic.Version(),numpy=np.__version__,native_execution=False,whole_feeder_validation=False,passed=True)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['source_dir','record_dir','export','report']:parser.add_argument(name,type=Path)
    a=parser.parse_args();r=check(a.source_dir,a.record_dir,a.export);a.report.write_text(json.dumps(r,indent=2)+'\n')
    print(f"{sum(c['finite_lines'] for c in r['cases'])} finite line primitives, {sum(len(c['exact_zero_line_ids']) for c in r['cases'])} exact-zero lines and {sum(c['connections'] for c in r['cases'])} declared connections passed")
