#!/usr/bin/env python3
"""Explicit, bounded MDB Tools acquisition into internal typed table records.

This is not a SINCAL SQLite export or an electrical reader. Original MDB files
are never edited. NULLs omitted by mdb-json are restored using mdb-schema;
missing columns and excluded tables remain distinguishable from NULL cells.
"""

import argparse
from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time


DEFAULT_TABLES = (
    'Version', 'Variant', 'Node', 'Element', 'Terminal', 'VoltageLevel',
    'CalcParameter', 'Line', 'LineSeg', 'Load', 'Infeeder', 'DCInfeeder',
    'TwoWindingTransformer', 'ThreeWindingTransformer', 'ShuntImpedance',
    'CouplingData', 'CoupledLine', 'NeutralPointImp', 'OpSer', 'OpSerVal',
    'TransformerTap',
)
IDENTIFIER = re.compile(r'[A-Za-z_][A-Za-z_0-9]{0,127}\Z')


@dataclass(frozen=True)
class Limits:
    source_bytes: int = 256 * 1024 * 1024
    output_bytes: int = 64 * 1024 * 1024
    field_bytes: int = 1024 * 1024
    table_rows: int = 100_000
    cells: int = 4_000_000
    command_seconds: float = 30.0
    total_seconds: float = 300.0


def identifier(name):
    if not isinstance(name, str) or not IDENTIFIER.fullmatch(name):
        raise ValueError(f'unsupported native identifier: {name!r}')
    return name


def catalog_name(name):
    # Non-input tables can have names such as CSIRO 11's "Paste Errors".
    # Preserve their names as excluded metadata; never execute them as SQL.
    if not name or len(name.encode('utf-8')) > 128 or not name.isprintable():
        raise ValueError(f'unsupported catalog name: {name!r}')
    return name


def run_bounded(arguments, *, byte_limit, seconds):
    """Drain to private files, not pipes or an unbounded in-memory capture."""
    if byte_limit < 1 or seconds <= 0:
        raise ValueError('invalid process budget')
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        try:
            child = subprocess.Popen(arguments, stdout=stdout, stderr=stderr,
                                     stdin=subprocess.DEVNULL, shell=False)
        except FileNotFoundError as error:
            raise ValueError(f'MDB Tools executable unavailable: {arguments[0]}') from error
        deadline = time.monotonic() + seconds
        try:
            while child.poll() is None:
                if stdout.tell() > byte_limit or stderr.tell() > 64 * 1024:
                    raise ValueError('MDB Tools output limit exceeded')
                if time.monotonic() >= deadline:
                    raise ValueError('MDB Tools command timed out')
                time.sleep(0.01)
            if stdout.tell() > byte_limit or stderr.tell() > 64 * 1024:
                raise ValueError('MDB Tools output limit exceeded')
            stderr.seek(0)
            detail = stderr.read(64 * 1024).decode('utf-8', errors='replace')
            if child.returncode:
                raise ValueError(f'MDB Tools exited {child.returncode}: {detail[:2000]}')
            # Do not accept a partial decode accompanied by an exporter warning.
            if detail.strip():
                raise ValueError(f'MDB Tools reported a diagnostic: {detail[:2000]}')
            stdout.seek(0)
            return stdout.read(byte_limit + 1).decode('utf-8')
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def columns_from_schema(text, table):
    """Read only the known mdb-schema sqlite declaration syntax; execute no SQL."""
    identifier(table)
    lines = [line.strip() for line in text.splitlines()
             if line.strip() and not line.lstrip().startswith('--')]
    if len(lines) < 4 or lines[:2] != [f'CREATE TABLE `{table}`', '('] or lines[-1] != ');':
        raise ValueError(f'unsupported MDB schema declaration for {table}')
    columns = []
    seen = set()
    for line in lines[2:-1]:
        match = re.fullmatch(r'`([^`]+)`\s+([A-Za-z][A-Za-z0-9 ()]*?)(?:,)?', line)
        if not match:
            raise ValueError(f'unsupported column declaration in {table}: {line!r}')
        name = identifier(match[1])
        native_type = match[2].strip()
        if not re.fullmatch(r'(?:INTEGER|REAL|varchar|TEXT|DateTime|BLOB|NUMERIC)(?: NOT NULL)?',
                            native_type, re.IGNORECASE):
            raise ValueError(f'unsupported column type in {table}: {native_type!r}')
        if name.casefold() in seen:
            raise ValueError(f'duplicate column in {table}: {name}')
        seen.add(name.casefold())
        columns.append({'name': name, 'native_type': native_type})
    if not columns or len(columns) > 2048:
        raise ValueError(f'invalid column count in {table}')
    return columns


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate JSON field {key}')
        result[key] = value
    return result


def table_rows(text, columns, limits):
    names = [column['name'] for column in columns]
    known = set(names)
    rows = []
    for line in text.splitlines():
        if not line.strip():
            continue
        if len(rows) >= limits.table_rows:
            raise ValueError('native table row limit exceeded')
        raw = json.loads(line, object_pairs_hook=unique_object)
        if not isinstance(raw, dict) or set(raw) - known:
            raise ValueError('MDB row does not match declared columns')
        row = []
        for name in names:
            value = raw.get(name)  # mdb-json omits SQL NULL, not numeric zero.
            if value is not None and type(value) not in (str, int, float):
                raise ValueError(f'unsupported MDB cell type for {name}')
            if isinstance(value, str) and len(value.encode('utf-8')) > limits.field_bytes:
                raise ValueError(f'field limit exceeded for {name}')
            if isinstance(value, float) and not math.isfinite(value):
                raise ValueError(f'nonfinite MDB number for {name}')
            if type(value) is int and not -(2**63) <= value < 2**63:
                raise ValueError(f'integer outside signed 64-bit range for {name}')
            row.append(value)
        rows.append(row)
    return rows


def acquire(source, *, tables=DEFAULT_TABLES, tool_directory=None, limits=Limits()):
    source = Path(source)
    requested = list(dict.fromkeys(identifier(name) for name in tables))
    if not requested or len(requested) > 512:
        raise ValueError('select between 1 and 512 tables')
    deadline = time.monotonic() + limits.total_seconds

    def command(tool, *arguments):
        binary = str(Path(tool_directory) / tool) if tool_directory else tool
        return run_bounded([binary, *map(str, arguments)], byte_limit=limits.output_bytes,
                           seconds=min(limits.command_seconds, deadline - time.monotonic()))

    with tempfile.TemporaryDirectory(prefix='powerio-mdb-') as directory:
        snapshot = Path(directory) / 'source.mdb'
        digest = hashlib.sha256()
        size = 0
        with source.open('rb') as original, snapshot.open('xb') as copy:
            while chunk := original.read(1024 * 1024):
                size += len(chunk)
                if size > limits.source_bytes:
                    raise ValueError('Access input exceeds source byte limit')
                digest.update(chunk)
                copy.write(chunk)
        versions = {tool: command(tool, '--version').strip()
                    for tool in ('mdb-tables', 'mdb-schema', 'mdb-json')}
        catalog = command('mdb-tables', '-1', snapshot).splitlines()
        if len(catalog) > 4096 or len({name.casefold() for name in catalog}) != len(catalog):
            raise ValueError('invalid native table catalog')
        catalog = sorted(catalog_name(name) for name in catalog)
        selected = [name for name in requested if name in catalog]
        exported = []
        cells = 0
        for name in selected:
            schema = command('mdb-schema', '-T', name, '--no-indexes', '--no-relations',
                             snapshot, 'sqlite')
            columns = columns_from_schema(schema, name)
            rows = table_rows(command('mdb-json', snapshot, name), columns, limits)
            cells += len(columns) * len(rows)
            if cells > limits.cells:
                raise ValueError('acquired cell limit exceeded')
            exported.append({'name': name, 'columns': columns, 'rows': rows})
        return {'format': 'powerio-sincal-tables', 'version': 1,
                'transport': 'access-mdbtools',
                'source': {'name': source.name, 'sha256': digest.hexdigest(), 'bytes': size},
                'tools': versions, 'tables': exported,
                'excluded_tables': sorted(set(catalog) - set(selected)),
                'absent_requested_tables': sorted(set(requested) - set(catalog))}


def write_records(record, destination, limits=Limits()):
    """Do not truncate an existing result or expose a partially written document."""
    destination = Path(destination)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as stream:
            temporary = Path(stream.name)
            for chunk in json.JSONEncoder(ensure_ascii=False, allow_nan=False,
                                          separators=(',', ':')).iterencode(record):
                stream.write(chunk.encode('utf-8'))
                if stream.tell() > limits.output_bytes:
                    raise ValueError('record document exceeds output byte limit')
        # An exclusive atomic link publishes only the complete file; an existing
        # destination is never replaced, even in a race with another importer.
        os.link(temporary, destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--table', action='append', help='select native table (repeatable)')
    parser.add_argument('--tool-directory', type=Path)
    arguments = parser.parse_args()
    try:
        record = acquire(arguments.source, tables=arguments.table or DEFAULT_TABLES,
                         tool_directory=arguments.tool_directory)
        write_records(record, arguments.output)
    except (OSError, ValueError) as error:
        parser.exit(1, f'Access acquisition failed: {error}\n')
    print(f"Acquired {len(record['tables'])} tables from {record['source']['sha256']}; "
          'electrical mapping not performed')


if __name__ == '__main__':
    main()
