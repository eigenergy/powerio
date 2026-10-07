#!/usr/bin/env python3
"""Acquire supported shunt tables alongside the unchanged base CSIRO records."""
import argparse
import json
from pathlib import Path

from audit_distribution import digest, invoke
from import_access import acquire, write_records


def run(source_dir, original_records_dir, output_dir, reader):
    manifest = json.loads(Path(__file__).with_name('access-acquisition.json').read_text())
    output_dir.mkdir(parents=True, exist_ok=False)
    cases = []
    for n in (3,12,16,17):
        identity = next(c for c in manifest['cases'] if c['case']==n)
        source = source_dir/f'csiro-representative{n:02}.mdb'
        base_path = original_records_dir/f'representative{n:02}.json'
        if digest(source)!=identity['source_sha256'] or digest(base_path)!=identity['record_sha256']:
            raise ValueError('source or base acquisition identity mismatch')
        old = json.loads(base_path.read_text())
        records = acquire(source)
        tables = {t['name']:t for t in records['tables']}
        if records['source']!=old['source'] or any(tables[t['name']]!=t for t in old['tables']):
            raise ValueError('existing source or tables changed during shunt acquisition')
        path = output_dir/f'representative{n:02}.json'
        write_records(records,path)
        audit,context_error = invoke(reader,path,'audit',0.0)
        network,parse_error = invoke(reader,path,'read',0.0)
        row = dict(case=n,source_sha256=digest(source),record_sha256=digest(path),
                   source_bytes=source.stat().st_size,record_bytes=path.stat().st_size,
                   added_tables=['ShuntReactor','ShuntCondensator'],base_tables_unchanged=True,
                   context_error=context_error,complete_parse=network is not None,parse_error=parse_error)
        if audit is not None:
            row['component_count']=len(audit['components'])
            row['mapped']=sum(c['component_maps'] for c in audit['components'])
        cases.append(row)
    return dict(scope=__doc__,cases=cases,reader_sha256=digest(reader),
                collection=manifest['collection'],attribution=manifest['attribution'],license=manifest['license'],
                native_execution=False,whole_feeder_validation=False)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('source_dir','original_records_dir','output_dir','reader','report'):
        parser.add_argument(name,type=Path)
    a=parser.parse_args();r=run(a.source_dir,a.original_records_dir,a.output_dir,a.reader.resolve())
    a.report.write_text(json.dumps(r,indent=2)+'\n')
    print(f"Acquired and audited {len(r['cases'])} extended native records")
