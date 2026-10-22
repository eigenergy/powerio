#!/usr/bin/env python3
"""Check all CSIRO09 native ideal connections against actual Rust switch/graph output.

The manual defines these as node fusion for steady-state calculations. Compare
native union-find partitions with SciPy graph components of the mapped switches,
including the exact phase maps and all open-port dispositions. No impedance is
fitted, no native result is read, and this is not a complete feeder validation.
"""
import argparse
import hashlib
import json
from pathlib import Path

import scipy
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def check(source, records, exported):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    identity = next(c for c in manifest['cases'] if c['case'] == 9)
    if digest(source) != identity['source_sha256'] or digest(records) != identity['record_sha256']:
        raise ValueError('original source/acquisition identity mismatch')
    tables = {}
    for table in json.loads(records.read_text())['tables']:
        columns = [c['name'] for c in table['columns']]
        tables[table['name']] = [dict(zip(columns, row, strict=True)) for row in table['rows']]
    elements = {r['Element_ID']: r for r in tables['Element'] if r['Variant_ID'] == 1}
    selected = {r['Element_ID']: r for r in tables['Line'] if r['Variant_ID'] == 1 and r['Flag_LineTyp'] == 3}
    ports = {}
    for row in tables['Terminal']:
        if row['Variant_ID'] == 1:
            ports.setdefault(row['Element_ID'], []).append(row)
    data = json.loads(exported.read_text())
    switches = data['switches']
    graph = data['graph']
    nodes = {str(r['Node_ID']) for r in tables['Node'] if r['Variant_ID'] == 1}
    if len(selected) != 144 or len(switches) != 144 or {s['name'] for s in switches} != {str(i) for i in selected}:
        raise ValueError('requires all 144 ideal connections exactly once')
    if len(graph['buses']) != len(nodes) or {b['id'] for b in graph['buses']} != nodes:
        raise ValueError('mapped graph changed native node identities')
    if any(b['grounded'] for b in graph['buses']):
        raise ValueError('ideal connection introduced a ground')
    native_edges = []
    typed_edges = []
    expected_open = set()
    for switch in switches:
        element = int(switch['name'])
        pair = sorted(ports[element], key=lambda p: p['TerminalNo'])
        if len(pair) != 2 or [p['TerminalNo'] for p in pair] != [1, 2] or any(p['Flag_Terminal'] != 7 for p in pair):
            raise ValueError('unsupported original port declaration')
        closed = [p['Flag_State'] == 1 for p in pair]
        service = elements[element]['Flag_State'] == 1
        is_open = not service or not all(closed)
        if is_open:
            expected_open.add(str(element))
        if (switch['bus_from'] != str(pair[0]['Node_ID']) or switch['bus_to'] != str(pair[1]['Node_ID'])
                or switch['terminal_map_from'] != ['1', '2', '3'] or switch['terminal_map_to'] != ['1', '2', '3']
                or switch['open'] != is_open or selected[element]['Ith'] != 0 or switch['i_max'] is not None):
            raise ValueError('mapped connection changed endpoints, phases, state or rating')
        evidence = switch['extras']['sincal_connection']
        if (evidence['element'] != element or evidence['terminal_ids'] != [p['Terminal_ID'] for p in pair]
                or evidence['terminal_closed'] != closed or evidence['in_service'] != service):
            raise ValueError('lost independent native service/terminal state')
        if not is_open:
            for phase in ['1', '2', '3']:
                native_edges.append(((str(pair[0]['Node_ID']), phase), (str(pair[1]['Node_ID']), phase)))
    if len(graph['edges']) != 144 or {e['id'] for e in graph['edges']} != {s['name'] for s in switches}:
        raise ValueError('graph omitted or duplicated a native connection')
    by_id = {s['name']: s for s in switches}
    for edge in graph['edges']:
        switch = by_id[edge['id']]
        if (edge['kind'] != 'switch' or edge['from'] != switch['bus_from'] or edge['to'] != switch['bus_to']
                or edge['conductors'] != [['1', '1'], ['2', '2'], ['3', '3']] or edge['closed'] == switch['open']):
            raise ValueError('graph changed the typed ideal connection')
        if edge['closed']:
            typed_edges.extend(((edge['from'], a), (edge['to'], b)) for a, b in edge['conductors'])
    coordinates = sorted((node, phase) for node in nodes for phase in ['1', '2', '3'])
    parent = {n: n for n in coordinates}
    def root(node):
        while parent[node] != node:
            parent[node] = parent[parent[node]]
            node = parent[node]
        return node
    for a, b in native_edges:
        parent[root(a)] = root(b)
    expected = {}
    for node in coordinates:
        expected.setdefault(root(node), set()).add(node)
    index = {node: i for i, node in enumerate(coordinates)}
    adjacency = coo_matrix(([1] * len(typed_edges),
                            ([index[a] for a, _ in typed_edges], [index[b] for _, b in typed_edges])),
                           shape=(len(index), len(index)))
    count, labels = connected_components(adjacency, directed=False)
    actual = {}
    for node, label in zip(coordinates, labels, strict=True):
        actual.setdefault(int(label), set()).add(node)
    if {frozenset(s) for s in expected.values()} != {frozenset(s) for s in actual.values()}:
        raise ValueError('mapped conductor connectivity differs from native node fusion')
    return {'collection': manifest['collection'], 'attribution': manifest['attribution'], 'license': manifest['license'],
            'case': 9, 'source_sha256': identity['source_sha256'], 'record_sha256': identity['record_sha256'],
            'export_sha256': digest(exported), 'scope': 'Ideal-connection components and graph only; not a complete feeder.',
            'connection_count': len(switches), 'closed_count': len(switches) - len(expected_open),
            'open_elements': sorted(expected_open, key=int), 'phase_constraint_count': len(native_edges),
            'conductor_component_count': int(count), 'scipy': scipy.__version__,
            'native_execution': False, 'historical_results_used': False, 'passed': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('records', type=Path)
    parser.add_argument('exported', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    report = check(args.source, args.records, args.exported)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{report['connection_count']} native ideal connections passed, {report['closed_count']} closed")


if __name__ == '__main__':
    main()
