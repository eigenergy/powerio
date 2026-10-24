#!/usr/bin/env python3
"""Inspect native SINCAL evidence without modifying/extracting the input.

This is a schema research tool, not PowerIO's electrical reader. Requires
Python 3.11+ with SQLite deserialize support. It intentionally reports raw
flags instead of assigning electrical meanings that have not been verified.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path
import sqlite3
import stat
import zipfile


MAX_BYTES = 64 << 20
MAX_ENTRIES = 4096
MAX_RATIO = 200
SQLITE_MAGIC = b"SQLite format 3\0"
REQUIRED = {
    "Version": {"Version_ID", "Version_No", "Calc_Type"},
    "Variant": {"Variant_ID", "ParentVariant_ID", "Flag_Variant"},
    "Node": {"Node_ID", "Variant_ID"},
    "Element": {"Element_ID", "Variant_ID", "Type"},
    "Terminal": {"Terminal_ID", "Variant_ID", "Element_ID", "Node_ID", "TerminalNo"},
}
ELECTRICAL = (
    "Version", "Variant", "VoltageLevel", "Node", "Element", "Terminal",
    "Line", "LineSeg", "CouplingData", "CoupledLine", "NeutralPointImp",
    "Load", "Infeeder", "SynchronousMachine", "DCInfeeder", "Breaker",
    "TwoWindingTransformer", "ThreeWindingTransformer", "TransformerTap",
    "TransformerTapValue", "TransformerCon", "TransformerConValue",
    "ShuntCondensator", "ShuntReactor", "ShuntImpedance", "CalcParameter",
)


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def bounded_read(stream, limit=MAX_BYTES):
    data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError(f"input exceeds {limit} byte limit")
    return data


def archive_database(data):
    """Select one native project database, never diagram/result databases."""
    inventory = []
    names = set()
    expanded = 0
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        if len(archive.infolist()) > MAX_ENTRIES:
            raise ValueError("archive entry limit exceeded")
        for entry in archive.infolist():
            name = entry.filename
            parts = name.rstrip('/').split('/')
            if (not name or name.startswith('/') or '\\' in name or ':' in name
                    or any(part in ('', '.', '..') for part in parts)):
                raise ValueError(f"unsafe archive path: {name!r}")
            if name.casefold() in names:
                raise ValueError(f"duplicate archive path: {name!r}")
            names.add(name.casefold())
            if stat.S_ISLNK(entry.external_attr >> 16):
                raise ValueError(f"archive symbolic link: {name!r}")
            if entry.flag_bits & 1:
                raise ValueError("encrypted archives are not supported")
            expanded += entry.file_size
            if expanded > MAX_BYTES:
                raise ValueError("archive expanded byte limit exceeded")
            if entry.file_size > MAX_RATIO * max(1, entry.compress_size):
                raise ValueError(f"archive compression ratio exceeded: {name!r}")
            inventory.append({"name": name, "bytes": entry.file_size})
        if 'SIArchive.ini' not in archive.namelist():
            raise ValueError("missing SINCAL archive marker SIArchive.ini")
        marker = archive.read('SIArchive.ini').decode('utf-8-sig')
        fields = dict(line.strip().split('=', 1) for line in marker.splitlines() if '=' in line)
        if fields.get('AppVersion') != 'PSS SINCAL' or fields.get('NetworkType') != 'Electro':
            raise ValueError("not a SINCAL electrical archive")
        candidates = [entry.filename for entry in archive.infolist()
                      if len(entry.filename.split('/')) == 2
                      and entry.filename.split('/')[0].endswith('_files')
                      and entry.filename.split('/')[1] == 'database.db']
        if len(candidates) != 1:
            raise ValueError(f"expected one native SQLite database; found {len(candidates)}")
        name = candidates[0]
        with archive.open(name) as stream:
            database = bounded_read(stream)
    return name, database, inventory


def read_native(path):
    with Path(path).open('rb') as stream:
        original = bounded_read(stream)
    if original.startswith(SQLITE_MAGIC):
        return original, Path(path).name, original, []
    name, database, inventory = archive_database(original)
    return original, name, database, inventory


def connect(database):
    if not database.startswith(SQLITE_MAGIC):
        raise ValueError("native payload is not SQLite")
    if len(database) < 100 or database[18:20] != b'\x01\x01':
        raise ValueError("database needs a self-contained rollback-journal snapshot, not WAL")
    connection = sqlite3.connect(':memory:')
    try:
        connection.deserialize(database)
        connection.execute('PRAGMA query_only=ON')
        connection.execute('PRAGMA trusted_schema=OFF')
        # Bound work even when inspecting malformed or unexpectedly large schemas.
        remaining = 10_000

        def budget():
            nonlocal remaining
            remaining -= 1
            return remaining <= 0

        connection.set_progress_handler(budget, 1000)
        connection.row_factory = sqlite3.Row
        return connection
    except Exception:
        connection.close()
        raise


def columns(connection, table):
    return [row['name'] for row in connection.execute(f'PRAGMA table_info({quote(table)})')]


def rows(connection, table, variant=None):
    query = f'SELECT * FROM {quote(table)}'
    params = ()
    if variant is not None and 'Variant_ID' in columns(connection, table):
        query += ' WHERE Variant_ID=?'
        params = (variant,)
    return [dict(row) for row in connection.execute(query, params)]


def unique(records, key, table):
    result = {}
    for row in records:
        value = row[key]
        if value is None or value in result:
            raise ValueError(f"missing or duplicate {table}.{key}: {value}")
        result[value] = row
    return result


def inspect_database(database, variant=None):
    connection = connect(database)
    try:
        tables = [row[0] for row in connection.execute(
            "SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")]
        for table, required in REQUIRED.items():
            if table not in tables or not required.issubset(columns(connection, table)):
                raise ValueError(f"missing SINCAL table/columns: {table}")
        versions = rows(connection, 'Version')
        if len(versions) != 1 or versions[0]['Calc_Type'] != 1:
            raise ValueError("expected one electrical Version record")
        variants = unique(rows(connection, 'Variant'), 'Variant_ID', 'Variant')
        if variant is None:
            if len(variants) != 1:
                raise ValueError("multiple variants: select --variant explicitly")
            variant = next(iter(variants))
        if variant not in variants:
            raise ValueError(f"unknown variant {variant}")
        if variants[variant]['ParentVariant_ID'] not in (None, 0):
            raise ValueError("derived variant requires documented inheritance semantics")
        nodes = unique(rows(connection, 'Node', variant), 'Node_ID', 'Node')
        elements = unique(rows(connection, 'Element', variant), 'Element_ID', 'Element')
        terminals = unique(rows(connection, 'Terminal', variant), 'Terminal_ID', 'Terminal')
        positions = set()
        for terminal in terminals.values():
            if terminal['Node_ID'] not in nodes or terminal['Element_ID'] not in elements:
                raise ValueError(f"unresolved Terminal {terminal['Terminal_ID']} in variant {variant}")
            position = (terminal['Element_ID'], terminal['TerminalNo'])
            if position in positions:
                raise ValueError(f"duplicate element terminal position: {position}")
            positions.add(position)
        counts = {}
        fields = {}
        flags = {}
        for table in tables:
            cols = columns(connection, table)
            condition = ' WHERE Variant_ID=?' if 'Variant_ID' in cols else ''
            params = (variant,) if condition else ()
            count = connection.execute(
                f'SELECT count(*) FROM {quote(table)}{condition}', params).fetchone()[0]
            if count:
                counts[table] = count
            if table not in ELECTRICAL:
                continue
            fields[table] = cols
            for col in cols:
                if not col.startswith('Flag_'):
                    continue
                values = connection.execute(
                    f'SELECT DISTINCT {quote(col)} FROM {quote(table)}{condition} '
                    f'ORDER BY {quote(col)} LIMIT 65', params).fetchall()
                if values:
                    flags[f'{table}.{col}'] = [row[0] for row in values]
        return {
            'version': versions[0], 'variant': variants[variant],
            'table_count': len(tables), 'nonempty_tables': counts,
            'electrical_columns': fields, 'raw_flags': flags,
            'topology': {'nodes': len(nodes), 'elements': len(elements), 'terminals': len(terminals)},
            'electrical_support_verified': False,
        }
    finally:
        connection.close()


def inspect(path, variant=None):
    original, name, database, inventory = read_native(path)
    return {
        'input_sha256': hashlib.sha256(original).hexdigest(),
        'input_bytes': len(original), 'database_name': name,
        'database_sha256': hashlib.sha256(database).hexdigest(),
        'database_bytes': len(database), 'archive_inventory': inventory,
        **inspect_database(database, variant),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('--variant', type=int)
    args = parser.parse_args()
    try:
        print(json.dumps(inspect(args.input, args.variant), indent=2, allow_nan=False))
    except (ValueError, OSError, sqlite3.Error, zipfile.BadZipFile) as error:
        parser.exit(1, f'SINCAL inspection failed: {error}\n')


if __name__ == '__main__':
    main()
