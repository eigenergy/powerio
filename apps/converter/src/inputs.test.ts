import { describe, expect, it } from 'vitest';
import { BlobReader, BlobWriter, ZipWriter } from '@zip.js/zip.js';
import { groupInputs, safeName, unpack } from './inputs';
const entry = (path: string, content = '') => ({ path, file: new File([content], path.split('/').at(-1)!) });

describe('project discovery', () => {
  it('keeps transmission cases independent beside an OpenDSS project', async () => {
    const cases = await groupInputs([entry('case.m'), entry('case.raw'), entry('Master.dss'), entry('lines.dss')]);
    expect(cases).toHaveLength(3);
    expect(cases.find(project => project.primary === 'Master.dss')?.entries).toHaveLength(2);
  });
  it('keeps nested OpenDSS paths and requests an ambiguous entry point', async () => {
    const [project] = await groupInputs([entry('feeder/model/Master.dss'), entry('feeder/model/lines.dss'), entry('feeder/loads.csv')]);
    expect(project.primary).toBe('model/Master.dss');
    expect(project.entries.map(file => file.path)).toContain('loads.csv');
    expect((await groupInputs([entry('a.dss'), entry('b.dss')]))[0].needsPrimary).toBe(true);
  });
  it('groups PyPSA and CGMES by project', async () => {
    const cases = await groupInputs([entry('pypsa/network.csv'), entry('pypsa/buses.csv'), entry('cgmes/EQ.xml', '<rdf:RDF>'), entry('cgmes/TP.xml', '<rdf:RDF>'), entry('network.xiidm')]);
    expect(cases).toHaveLength(3);
    expect(cases[0].primary).toBeUndefined();
    expect(cases[0].entries.map(file => file.path)).toEqual(['network.csv', 'buses.csv']);
  });
  it('extracts a mixed batch ZIP with paths preserved', async () => {
    const zip = new ZipWriter(new BlobWriter(), { useWebWorkers: false });
    await zip.add('first.m', new BlobReader(new Blob(['mpc.version=2;'])));
    await zip.add('second.raw', new BlobReader(new Blob(['raw'])));
    const result = await unpack({ path: 'batch.zip', file: new File([await zip.close()], 'batch.zip') });
    expect(result.map(file => file.path)).toEqual(['batch/first.m', 'batch/second.raw']);
  });
  it('refuses traversal paths inside ZIP containers', async () => {
    const zip = new ZipWriter(new BlobWriter(), { useWebWorkers: false });
    await zip.add('../escape.m', new BlobReader(new Blob(['x'])));
    await expect(unpack({ path: 'bad.zip', file: new File([await zip.close()], 'bad.zip') })).rejects.toThrow();
  });
  it('produces portable output names', () => {
    expect(safeName('../x /y')).toBe('_x__y');
    expect(safeName('CON')).toBe('case-CON');
    expect(safeName('')).toBe('case');
  });
});
