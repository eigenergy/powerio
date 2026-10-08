#!/usr/bin/env python3
"""Compare public-reader geometry with native SQLite or independent MDB CSV rows.

Models stay external. The temporary baseline deletes only optional graphic tables;
all electrical input is identical. No SINCAL desktop or geographic CRS is attested.
"""
import argparse
import copy
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import zipfile

from import_access import GRAPHICS_TABLES, run_bounded


def native_rows(path):
    names = (*GRAPHICS_TABLES, 'Terminal', 'Node', 'Element')
    if path.suffix.lower() == '.mdb':
        return {name: list(csv.DictReader(io.StringIO(run_bounded(
            ['mdb-export', str(path), name], byte_limit=32 * 1024**2, seconds=30))))
                for name in names}
    with sqlite3.connect(':memory:') as connection:
        if path.suffix.lower() == '.sinx':
            with zipfile.ZipFile(path) as archive:
                entries = [n for n in archive.namelist() if n.endswith('/database.db')]
                assert len(entries) == 1
                data = archive.read(entries[0])
        else:
            data = path.read_bytes()
        connection.deserialize(data)
        connection.row_factory = sqlite3.Row
        return {name: [dict(row) for row in connection.execute(f'SELECT * FROM {name}')]
                for name in names}


def selected(rows):
    return [r for r in rows if int(r['Variant_ID']) == 1 and int(r.get('Flag_Variant', 1)) == 1]


def expected_geometry(tables):
    nodes = {int(r['Node_ID']): r for r in selected(tables['GraphicNode'])}
    assert len({r['GraphicArea_ID'] for r in nodes.values()}) == 1
    points = {}
    for node, r in nodes.items():
        start = [float(r['NodeStartX']), float(r['NodeStartY'])]
        end = [float(r['NodeEndX']), float(r['NodeEndY'])]
        points[node] = (start if start == end else [a/2+b/2 for a, b in zip(start, end)],
                        'source' if start == end else 'derived')
    terminals = {int(r['Terminal_ID']): r for r in selected(tables['Terminal'])}
    elements = {int(r['GraphicElement_ID']): int(r['Element_ID'])
                for r in selected(tables['GraphicElement'])}
    lines = {int(r['Element_ID']) for r in selected(tables['Element']) if r['Type'].strip() == 'Line'}
    bends = {}
    for r in selected(tables['GraphicBucklePoint']):
        key = int(r['GraphicTerminal_ID'])
        order = int(r['NoPoint'])
        assert order > 0 and order not in bends.setdefault(key, {})
        bends[key][order] = [float(r['PosX']), float(r['PosY'])]
    ports = {}
    for r in selected(tables['GraphicTerminal']):
        element = elements[int(r['GraphicElement_ID'])]
        if element not in lines:
            continue
        native = terminals[int(r['Terminal_ID'])]
        assert int(native['Element_ID']) == element
        port = int(native['TerminalNo'])
        assert port in (1, 2) and port not in ports.setdefault(element, {})
        chain = [[float(r['PosX']), float(r['PosY'])]]
        sequence = bends.get(int(r['GraphicTerminal_ID']), {})
        assert sorted(sequence) == list(range(1, len(sequence)+1))
        chain.extend(sequence[i] for i in sorted(sequence, reverse=True))
        ports[element][port] = chain
    routes = {}
    for element, ends in ports.items():
        assert set(ends) == {1, 2}
        chain = ends[1] + ends[2][::-1]
        route = [p for i, p in enumerate(chain) if i == 0 or p != chain[i-1]]
        if len(route) >= 2:
            routes[element] = route
    return points, routes


def close_points(actual, expected):
    # Independent MDB CSV vs JSON exporters may differ by a few ulps.
    return len(actual) == len(expected) and all(
        len(a) == len(b) and all(math.isclose(x, y, rel_tol=2e-15, abs_tol=1e-12)
                               for x, y in zip(a, b))
        for a, b in zip(actual, expected))


def verify(network, points, routes, balanced, space):
    mapped_points = derived = mapped_routes = bend_vertices = 0
    for bus in network['buses']:
        node = int(bus['uid'].split(':')[-1]) if balanced else int(bus['id']) if bus['id'].isdigit() else None
        location = bus.get('location')
        if node not in points:
            assert location is None, 'fabricated auxiliary location'
            continue
        expected, kind = points[node]
        assert location is not None and close_points([[location['x'], location['y']]], [expected]), 'point/axis/identity mismatch'
        assert location['kind'] == kind
        mapped_points += 1
        derived += kind == 'derived'
    for line in network['branches' if balanced else 'lines']:
        element = int(line['uid'].split(':')[-1]) if balanced else line['extras']['sincal']['element']
        route = line.get('route')
        if element not in routes:
            assert route is None
            continue
        assert route is not None and close_points([[p['x'], p['y']] for p in route], routes[element]), 'route/identity mismatch'
        mapped_routes += 1
        bend_vertices += max(0, len(route)-2)
    assert network['geo']['space'] == space
    return dict(native_graphic_nodes=len(points), mapped_points=mapped_points,
                native_nodes_without_typed_location=len(points)-mapped_points,
                derived_midpoints=derived, mapped_line_routes=mapped_routes,
                mapped_bend_vertices=bend_vertices)


def verify_view(actual, native):
    assert actual['coordinates_transformed'] is False
    for field in ['Flag','GraphicArea_ID','VectorX','VectorY','AreaWidth','AreaHeight','Scale1','Scale2','ScalePaper','ScaleReal']:
        if native.get(field) not in (None, ''):
            assert math.isclose(actual['native_fields'][field], float(native[field]), rel_tol=2e-15, abs_tol=1e-12), field


def electrical(network):
    result = copy.deepcopy(network)
    result.pop('name', None)  # Temporary baseline source path differs.
    result.pop('geo', None)
    for bus in result['buses']:
        bus.pop('location', None)
        bus.get('extras', {}).pop('sincal_geometry', None)
    for line in result.get('lines', result.get('branches', [])):
        line.pop('route', None)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--native', type=Path, required=True)
    parser.add_argument('--records', type=Path)
    parser.add_argument('--family', choices=['balanced', 'multiconductor'], required=True)
    parser.add_argument('--assume-inactive-source-controls', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--drawing-only', action='store_true', help='validate shared geometry only; no electrical parsing, echo or IR claims')
    args = parser.parse_args()
    def probe(path, records=None):
        command = [str(args.binary.resolve()), 'sincal-'+args.family, str(path)]
        if records:
            command.append(str(records))
        if args.assume_inactive_source_controls:
            command.append('--assume-inactive-source-controls')
        run = subprocess.run(command, capture_output=True, text=True, timeout=60, check=False)
        if run.returncode:
            raise RuntimeError(run.stderr[:4000])
        return json.loads(run.stdout)
    tables = native_rows(args.native)
    points, routes = expected_geometry(tables)
    view, = selected(tables['GraphicAreaTile'])
    space = 'diagram' if int(view['Flag']) == 2 else 'unknown'
    if args.drawing_only:
        assert args.records
        result = json.loads(subprocess.check_output([str(args.binary.resolve()), str(args.records)], text=True, timeout=60))
        assert result['source_sha256'] == hashlib.sha256(args.native.read_bytes()).hexdigest()
        assert result['electrical_mapping_performed'] is False
        g = result['geometry']
        assert set(map(int,g['routes'])) == set(routes), 'native route identities differ'
        for key, route in g['routes'].items():
            assert close_points(route, routes[int(key)]), 'native multi-bend routes differ'
        assert set(map(int, g['points'])) == set(points)
        for key, p in g['points'].items():
            start, end = p['start'], p['end']
            position = start if start == end else [a/2+b/2 for a,b in zip(start,end)]
            assert close_points([position], [points[int(key)][0]])
        assert g['schematic'] == (space == 'diagram')
        verify_view(g['view'], view)
        # Swapping the first port's documented order must change the oracle.
        bad = copy.deepcopy(tables)
        candidates = {}
        for r in selected(bad['GraphicBucklePoint']):
            candidates.setdefault(r['GraphicTerminal_ID'], []).append(r)
        sequence = next(rs for rs in candidates.values() if len(rs) >= 2 and len({(r['PosX'], r['PosY']) for r in rs}) > 1)
        sequence[0]['NoPoint'], sequence[1]['NoPoint'] = sequence[1]['NoPoint'], sequence[0]['NoPoint']
        assert expected_geometry(bad)[1] != routes
        report = dict(source_sha256=hashlib.sha256(args.native.read_bytes()).hexdigest(),
            source_bytes=args.native.stat().st_size, electrical_mapping_performed=False,
            native_graphic_nodes=len(points), mapped_line_routes=len(routes),
            mapped_bend_vertices=sum(max(0,len(r)-2) for r in routes.values()),
            maximum_route_vertices=max(map(len,routes.values())), space=space,
            view_metadata_verified=True, bend_order_negative_control_detected=True,
            coordinate_tolerance={'relative':2e-15,'absolute':1e-12},
            findings=g['findings'])
        args.output.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report))
        return
    actual = probe(args.native, args.records)
    verify_view(actual['extensions']['powerio.sincal.graphic_view'], view)
    counts = verify(actual['network'], points, routes, args.family == 'balanced', space)
    with tempfile.TemporaryDirectory() as directory:
        if args.records:
            records = json.loads(args.records.read_text())
            records['tables'] = [t for t in records['tables'] if t['name'] not in GRAPHICS_TABLES]
            records['excluded_tables'] = sorted(set(records['excluded_tables']) | set(GRAPHICS_TABLES))
            baseline = Path(directory)/'electrical.json'
            baseline.write_text(json.dumps(records))
            original = probe(args.native, baseline)
        else:
            if args.native.suffix.lower() == '.sinx':
                with zipfile.ZipFile(args.native) as archive:
                    data = archive.read(next(n for n in archive.namelist() if n.endswith('/database.db')))
            else:
                data = args.native.read_bytes()
            baseline = Path(directory)/'electrical.db'
            baseline.write_bytes(data)
            with sqlite3.connect(baseline) as db:
                for table in GRAPHICS_TABLES:
                    db.execute(f'DROP TABLE {table}')
            original = probe(baseline)
    assert electrical(actual['network']) == electrical(original['network']), 'electrical fields changed'
    negatives = []
    for mutation in ['swap_axes', 'wrong_node_identity', 'wrong_route_vertex']:
        bad = copy.deepcopy(actual['network'])
        located = [b for b in bad['buses'] if b.get('location')]
        if mutation == 'swap_axes':
            for bus in located:
                p = bus['location']; p['x'], p['y'] = p['y'], p['x']
        elif mutation == 'wrong_node_identity':
            located[0]['location'] = copy.deepcopy(located[1]['location'])
        else:
            line = next(l for l in bad.get('lines', bad.get('branches', [])) if l.get('route'))
            line['route'][1]['x'] += 1
        try:
            verify(bad, points, routes, args.family == 'balanced', space)
        except AssertionError:
            negatives.append(mutation)
        else:
            raise AssertionError('negative control not detected: '+mutation)
    report = dict(source_sha256=hashlib.sha256(args.native.read_bytes()).hexdigest(),
                  source_bytes=args.native.stat().st_size, family=args.family,
                  assume_inactive_source_controls=args.assume_inactive_source_controls,
                  **counts, space=space, view_metadata_verified=True, source_echo=actual['source_echo'], typed_ir=actual['typed_ir'],
                  geo_export=actual['geo_export'], electrical_fields_unchanged=True,
                  negative_controls_detected=negatives,
                  coordinate_tolerance={'relative':2e-15,'absolute':1e-12},
                  geometry_diagnostics=[d for d in actual['diagnostics'] if 'GEOMETRY' in d['code']])
    args.output.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({k:v for k,v in report.items() if k!='geometry_diagnostics'}))


if __name__ == '__main__':
    main()
