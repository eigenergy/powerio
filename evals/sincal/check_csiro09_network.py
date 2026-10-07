#!/usr/bin/env python3
"""Compare complete CSIRO09 typed snapshots with independent native-input OpenDSS.
No historic results or electrical inputs are edited. The typed network is solved
with sparse MNA and constant-power current iteration, independent of PowerIO.
"""
import argparse
import cmath
import copy
import hashlib
import json
from pathlib import Path
import numpy as np
from scipy.sparse import coo_matrix, bmat, csc_matrix
from scipy.sparse.linalg import splu
from opendssdirect import dss

EPS = 1e-6
TOLERANCE_V = 1e-3

class Union:
    def __init__(self): self.parents = {}
    def root(self, x):
        self.parents.setdefault(x,x)
        if self.parents[x] != x: self.parents[x] = self.root(self.parents[x])
        return self.parents[x]
    def join(self,a,b): self.parents[self.root(a)] = self.root(b)


def solve_typed(net, source_impedance=0j):
    if any(net[k] for k in ['transformers','generators','untyped']):
        raise ValueError('unexpected component in case profile')
    u = Union()
    for switch in net['switches']:
        if not switch['open']:
            for a,b in zip(switch['terminal_map_from'],switch['terminal_map_to'],strict=True):
                u.join((switch['bus_from'],a),(switch['bus_to'],b))
    ground = {u.root((b['id'],t)) for b in net['buses'] for t in b['grounded']}
    index = {}
    def at(bus,t):
        key = u.root((bus,t))
        if key in ground: return None
        return index.setdefault(key,len(index))
    entries = []
    def stamp(ports,matrix):
        for i,a in enumerate(ports):
            for j,b in enumerate(ports):
                if a is not None and b is not None: entries.append((a,b,matrix[i,j]))
    codes = {c['name']:c for c in net['linecodes']}
    for line in net['lines']:
        c = codes[line['linecode']]
        z = (np.asarray(c['r_series'])+1j*np.asarray(c['x_series']))*line['length']
        y = np.linalg.inv(z)
        ports = [at(line['bus_'+side],t) for side in ['from','to'] for t in line['terminal_map_'+side]]
        sh = [(np.asarray(c['g_'+side])+1j*np.asarray(c['b_'+side]))*line['length'] for side in ['from','to']]
        stamp(ports,np.block([[y+sh[0],-y],[-y,y+sh[1]]]))
    for sh in net['shunts']:
        stamp([at(sh['bus'],t) for t in sh['terminal_map']],np.asarray(sh['g'])+1j*np.asarray(sh['b']))
    loads = []
    for load in net['loads']:
        if load['configuration'] != 'delta' or load['voltage_model']['model'] != 'constant_power':
            raise ValueError('unexpected load profile')
        t = load['terminal_map']
        if len(t)!=3 or len(load['p_nom'])!=3 or len(load['q_nom'])!=3:
            raise ValueError('delta branch dimension mismatch')
        for k in range(3):
            loads.append((at(load['bus'],t[k]),at(load['bus'],t[(k+1)%3]),complex(load['p_nom'][k],load['q_nom'][k])))
    constraints = []
    for wrapped in net['sources']:
        source = wrapped['value']
        star = at(source['bus'],source['reference_terminal'])
        for t,v,angle in zip(source['terminal_map'],source['v_magnitude'],source['v_angle'],strict=True):
            constraints.append((at(source['bus'],t),star,cmath.rect(v,angle)))
    n = len(index)
    rows,cols,data = zip(*entries,strict=True)
    y = coo_matrix((data,(rows,cols)),shape=(n,n)).tocsc()
    cr,cc,cv = [],[],[]
    for k,(a,b,_) in enumerate(constraints):
        for node,sign in [(a,1),(b,-1)]:
            if node is not None: cr.append(k);cc.append(node);cv.append(sign)
    c = coo_matrix((cv,(cr,cc)),shape=(len(constraints),n)).tocsc()
    if len(constraints)!=3: raise ValueError('expected one three-phase source')
    # Finite positive/negative-sequence boundary for quantifying the separate
    # OpenDSS ideal-source approximation; the zero-sequence star is unchanged.
    boundary = -source_impedance*(np.eye(3)-np.ones((3,3))/3)
    matrix = bmat([[y,c.T],[c,csc_matrix(boundary)]],format='csc')
    lu = splu(matrix)
    rhs = np.r_[np.zeros(n,dtype=complex),[c[2] for c in constraints]]
    solution = lu.solve(rhs)
    def current_rhs(volts):
        out = rhs.copy()
        for a,b,power in loads:
            v = volts[a]-volts[b]
            if abs(v)<1 or not np.isfinite(v): raise ValueError('invalid load branch voltage')
            current = np.conj(power/v)
            out[a] -= current;out[b] += current
        return out
    for iteration in range(1000):
        update = lu.solve(current_rhs(solution))
        error = np.max(abs(update[:n]-solution[:n]))
        solution = update
        if error < 1e-8: break
    else: raise ValueError('typed constant-power iteration failed')
    residual = float(np.max(abs(matrix@solution-current_rhs(solution))))
    if not np.isfinite(residual) or residual > 1e-4: raise ValueError(f'typed MNA residual {residual}')
    result = {(b['id'],t):solution[index[u.root((b['id'],t))]] for b in net['buses'] for t in b['terminals'] if u.root((b['id'],t)) in index}
    return result,residual,iteration+1


def tables(path):
    return {t['name']:[dict(zip([c['name'] for c in t['columns']],r,strict=True)) for r in t['rows'] if dict(zip([c['name'] for c in t['columns']],r)).get('Variant_ID',1)==1] for t in json.loads(path.read_text())['tables']}


def native_reference(t,hours,stress=False):
    if len(t['CalcParameter'])!=1 or any(t['CalcParameter'][0][k]!=v for k,v in [('Flag_ScType',1),('Flag_LFZ0',1),('f',50),('Temp_Cond',20)]):
        raise ValueError('unexpected native calculation context')
    if any(v['f']!=50 or v['Temp_Line']!=20 or v['Temp_Cable']!=20 for v in t['VoltageLevel']):
        raise ValueError('unapplied native frequency/temperature correction')
    if any(e['Flag_State']!=1 for e in t['Element']):
        raise ValueError('unexpected inactive native element')
    ports = {}
    for p in t['Terminal']: ports.setdefault(p['Element_ID'],[]).append(p)
    for ps in ports.values(): ps.sort(key=lambda p:p['TerminalNo'])
    elems = {r['Element_ID']:r for r in t['Element']}
    union = Union()
    def ideal(row):
        return row['Flag_LineTyp']==3 or (row['Flag_Z0_Input']==2 and all(row[k]==0 for k in ['r','x','r0','x0','c','c0','va']))
    for row in t['Line']:
        ps = ports[row['Element_ID']]
        if ideal(row) and elems[row['Element_ID']]['Flag_State']==1 and all(p['Flag_State']==1 for p in ps):
            union.join(ps[0]['Node_ID'],ps[1]['Node_ID'])
    def bus(node): return 'n'+str(union.root(node))
    source = t['Infeeder'][0]
    if (len(t['Infeeder'])!=1 or source['Flag_Typ']!=1 or source['Flag_Z0_Input']!=1
            or source['xi']!=0 or source['Flag_Lf']!=6 or source['Flag_Typ_ID']!=0):
        raise ValueError('unexpected native source profile')
    zabs = np.hypot(source['R'],source['X'])*source['Z0_Z1']
    angle = np.arctan2(1,source['R0_X0'])
    z0 = cmath.rect(zabs,angle)
    deck=['Clear','Set DefaultBaseFrequency=50',f'New Circuit.check phases=3 bus1={bus(ports[source["Element_ID"]][0]["Node_ID"])}.1.2.3 basekv={source["Ug"]:.17g} pu=1 angle={source["delta"]:.17g} frequency=50',
          f'Edit Vsource.source r1={EPS} x1={EPS} r0={z0.real:.17g} x0={z0.imag:.17g}']
    for row in t['Line']:
        if ideal(row):continue
        ps = ports[row['Element_ID']]
        if (row['ParSys']!=1 or row['fn']!=50 or row['Flag_Z0_Input']!=2 or row['va']!=0
                or any(p['Flag_Terminal']!=7 or p['Flag_State']!=1 for p in ps)):
            raise ValueError('unexpected native line profile')
        a,b=[bus(p['Node_ID']) for p in ps]
        deck.append(f'New Line.e{row["Element_ID"]} phases=3 bus1={a}.1.2.3 bus2={b}.1.2.3 units=km length={row["l"]:.17g} r1={row["r"]:.17g} x1={row["x"]:.17g} r0={row["r0"]:.17g} x0={row["x0"]:.17g} c1={row["c"]:.17g} c0={row["c0"]:.17g}')
    profiles={p['OpSer_ID']:p for p in t['OpSer']};points={}
    for p in t['OpSerVal']:points.setdefault(p['OpSer_ID'],[]).append(p)
    levels={v['VoltLevel_ID']:v for v in t['VoltageLevel']};nodes={n['Node_ID']:n for n in t['Node']}
    powers={}
    for row in t['Load']:
        p=ports[row['Element_ID']][0]
        if row['Flag_Lf']!=15 or row['Flag_LoadType']!=2 or p['Flag_Terminal']!=7 or p['Flag_State']!=1:
            raise ValueError('unexpected native load profile')
        if row['DayOpSer_ID']:
            profile=profiles[row['DayOpSer_ID']];samples=sorted(points[row['DayOpSer_ID']],key=lambda p:p['OpTime']);period=profile['BaseT'] or 24
            if profile['Flag_Typ']!=3 or profile['Flag_Ser']!=1 or any(profile[k]!=0 for k in ['Power_a1','Power_b1','Reduce_a2','Reduce_b2']) or any(p['Flag_Curve']!=1 for p in samples): raise ValueError('unexpected native daily profile')
            times=[p['OpTime'] for p in samples]
            if times[0]!=0 or len(set(times))!=len(times) or times[-1]>=period:raise ValueError('invalid profile timestamps')
            pq=[float(np.interp(hours%period,times+[period],[p[k] for p in samples]+[samples[0][k]]))*row[f]*1000 for k,f in [('P','fP'),('Q','fQ')]]
        else:pq=[row[k]*row[f]*1e6 for k,f in [('P','fP'),('Q','fQ')]]
        factors=[2.0,0.5,1.0] if stress else [1.0,1.0,1.0]
        powers[str(row['Element_ID'])]=[v*sum(factors)/3 for v in pq]
        kv=levels[nodes[p['Node_ID']]['VoltLevel_ID']]['Un']*row['u']/100
        if stress:
            for branch,(a,b) in enumerate([(1,2),(2,3),(3,1)]):
                deck.append(f'New Load.e{row["Element_ID"]}b{branch} phases=1 bus1={bus(p["Node_ID"])}.{a}.{b} conn=delta kv={kv:.17g} kw={pq[0]*factors[branch]/3000:.17g} kvar={pq[1]*factors[branch]/3000:.17g} model=1 vminpu=0.1 vmaxpu=2 status=fixed')
        else:
            deck.append(f'New Load.e{row["Element_ID"]} phases=3 bus1={bus(p["Node_ID"])}.1.2.3 conn=delta kv={kv:.17g} kw={pq[0]/1000:.17g} kvar={pq[1]/1000:.17g} model=1 vminpu=0.1 vmaxpu=2 status=fixed')
    deck+=['Set controlmode=off tolerance=1e-12 maxiterations=1000','Solve']
    text='\n'.join(deck)+'\n';dss(text)
    if not dss.Solution.Converged():raise ValueError('native-input OpenDSS did not converge')
    result={}
    for node in nodes:
        if dss.Circuit.SetActiveBus(bus(node))<0:continue
        raw=np.asarray(dss.Bus.Voltages());volts=raw[::2]+1j*raw[1::2]
        for phase,v in zip(dss.Bus.Nodes(),volts,strict=True):result[str(node),str(phase)]=v
    for row in t['Load']:
        port=ports[row['Element_ID']][0];node=str(port['Node_ID'])
        nominal=levels[nodes[port['Node_ID']]['VoltLevel_ID']]['Un']*row['u']*10
        for a,b in [('1','2'),('2','3'),('3','1')]:
            pu=abs(result[node,a]-result[node,b])/nominal
            if not 0.1<pu<2: raise ValueError('OpenDSS load outside constant-power voltage domain')
    return result,powers,hashlib.sha256(text.encode()).hexdigest()


def check(records,source,network,hours,stress=False):
    manifest=json.loads(Path(__file__).with_name('access-acquisition.json').read_text());identity=next(c for c in manifest['cases'] if c['case']==9)
    def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
    if digest(records)!=identity['record_sha256'] or digest(source)!=identity['source_sha256']:raise ValueError('source/acquisition identity mismatch')
    net=json.loads(network.read_text())
    native_tables=tables(records)
    native_lines={str(r['Element_ID']) for r in native_tables['Line']}
    typed_lines={r['name'] for r in net['lines']}|{r['name'] for r in net['switches'] if r['name'].isdigit()}
    if native_lines!=typed_lines or len(typed_lines)!=620:
        raise ValueError('native line/connection identities do not match')
    if {str(r['Element_ID']) for r in native_tables['Load']}!={r['name'] for r in net['loads']}:
        raise ValueError('native load identities do not match')
    if stress:
        net=copy.deepcopy(net)
        for load in net['loads']:
            for field in ['p_nom','q_nom']:
                load[field]=[v*f for v,f in zip(load[field],[2.0,0.5,1.0],strict=True)]
    typed,residual,iterations=solve_typed(net)
    native,powers,deck_digest=native_reference(tables(records),hours,stress)
    if len(net['loads'])!=67 or len(net['lines'])!=472 or len(net['switches'])!=216 or len(net['sources'])!=1:raise ValueError('missing native components')
    power_error=max(abs(sum(l[k])-powers[l['name']][i]) for l in net['loads'] for i,k in enumerate(['p_nom','q_nom']))
    keys=set(native)&set(typed)
    isolated={'2178','2468','2480','2482'}
    expected={(str(n['Node_ID']),p) for n in tables(records)['Node'] if str(n['Node_ID']) not in isolated for p in ['1','2','3']}
    if keys!=expected or any(k[0] in isolated for k in set(native)|set(typed)):
        raise ValueError('energized and isolated native node/phase identities differ')
    voltage_error=float(max(abs(typed[k]-native[k]) for k in keys))
    if voltage_error>=TOLERANCE_V or power_error>=1e-7:raise ValueError(f'network comparison failed: voltage {voltage_error}, power {power_error}')
    finite,finite_residual,_=solve_typed(net,complex(EPS,EPS))
    matched_error=float(max(abs(finite[k]-native[k]) for k in keys))
    boundary_error=float(max(abs(finite[k]-typed[k]) for k in keys))
    if matched_error>=1e-4: raise ValueError(f'matched finite-source comparison failed: {matched_error}')
    return {'case':9,'hours':hours,'synthetic_delta_branch_stress':stress,'source_sha256':identity['source_sha256'],'record_sha256':identity['record_sha256'],
            'network_sha256':digest(network),'deck_sha256':deck_digest,'native_nodes_compared':617,'isolated_native_nodes':sorted(isolated),'element_count':688,
            'maximum_voltage_error_v':voltage_error,'maximum_load_power_error_va':power_error,'typed_kcl_residual_a':residual,'typed_iterations':iterations,
            'matched_finite_source_error_v':matched_error,'source_approximation_error_v':boundary_error,'finite_source_kcl_residual_a':finite_residual,
            'voltage_tolerance_v':TOLERANCE_V,'opendss_source_r1_x1_ohm':EPS,'native_execution':False,'historical_results_used':False,'passed':True,
            'engine':dss.Basic.Version(),'numpy':np.__version__,'license':manifest['license'],'attribution':manifest['attribution']}

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ['records','source','network']:p.add_argument(name,type=Path)
    p.add_argument('--stress',action='store_true');p.add_argument('--hours',type=float,required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
    report=check(a.records,a.source,a.network,a.hours,a.stress);a.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
