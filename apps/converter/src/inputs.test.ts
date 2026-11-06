import { describe, expect, it, test, vi } from 'vitest';
import { BlobReader, BlobWriter, Uint8ArrayReader, Uint8ArrayWriter, ZipWriter } from '@zip.js/zip.js';
import { groupInputs, safeName, unpack } from './inputs';
const entry = (path: string, content = '') => ({ path, file: new File([content], path.split('/').at(-1)!) });

test('XML-only ZIPs containing independent networks are expanded', async () => {
  const zip = new ZipWriter(new BlobWriter(), { useWebWorkers: false });
  for (const name of ['one.xml', 'two.xml']) await zip.add(name, new BlobReader(new Blob(['<?xml version="1.0"?><!-- not <rdf:RDF> --><network xmlns="http://www.powsybl.org/schema/iidm/1_0"/>'])));
  const result = await unpack({ path: 'networks.zip', file: new File([await zip.close()], 'networks.zip') });
  expect(result.map(f => f.path)).toEqual(['networks/one.xml', 'networks/two.xml']);
});

test('directory local headers are validated even though their payload is empty', async () => {
  const zip = new ZipWriter(new Uint8ArrayWriter(), { useWebWorkers: false });
  await zip.add('folder/', undefined, { directory: true });
  await zip.add('case.m', new Uint8ArrayReader(new Uint8Array([1])));
  const bytes = await zip.close();
  bytes[30] = 'x'.charCodeAt(0); // Local directory name disagrees with its index.
  await expect(unpack({ path: 'bad.zip', file: new File([bytes], 'bad.zip') })).rejects.toThrow();
});

test('valid directories and native RDF archives retain exact source ZIP bytes in their scope', async () => {
  const zip = new ZipWriter(new BlobWriter(), { useWebWorkers: false });
  await zip.add('profiles/', undefined, { directory: true });
  await zip.add('profiles/EQ.xml', new BlobReader(new Blob(['<?xml version="1.0"?><!--', 'description '.repeat(500), '--><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/>'])));
  const file = new File([await zip.close()], 'network.zip');
  const [result] = await unpack({ path: 'network.zip', file }, () => false, 'unique');
  expect(result.file).toBe(file);
  expect(result.path).toBe('unique/network.zip');
  expect(result.collectionRoot).toBe('unique');
});
const sized = (path: string, size: number, content = '') => {
  const result = entry(path, content);
  Object.defineProperty(result.file, 'size', { value: size });
  return result;
};

test('a project over the cumulative discovery budget fails before any command file is read', async () => {
  const master = sized('bad/Master.dss', 40 * 1024 * 1024, 'Redirect loads.inc');
  const read = vi.spyOn(master.file, 'text');
  const projects = await groupInputs([master, sized('bad/loads.inc', 30 * 1024 * 1024), entry('good/Master.dss')]);
  expect(read).not.toHaveBeenCalled();
  expect(projects).toHaveLength(2);
  expect(projects.find(project => project.name === 'bad')?.error?.code).toBe('WEB.INPUT.LIMIT');
  expect(projects.find(project => project.name === 'good')?.error).toBeUndefined();
});

test('the discovery file-count guard applies per project before reading it', async () => {
  const master = entry('bad/Master.dss');
  const read = vi.spyOn(master.file, 'text');
  const projects = await groupInputs([master, ...Array.from({ length: 4096 }, (_, index) => entry(`bad/${index}.asset`)), entry('good/Master.dss')]);
  expect(read).not.toHaveBeenCalled();
  expect(projects.find(project => project.name === 'bad')?.error?.code).toBe('WEB.INPUT.LIMIT');
  expect(projects.find(project => project.name === 'good')?.error).toBeUndefined();
});

test('independent projects may exceed the discovery budget in aggregate', async () => {
  const projects = await groupInputs([sized('one/Master.dss', 40 * 1024 * 1024), sized('two/Master.dss', 40 * 1024 * 1024)]);
  expect(projects).toHaveLength(2);
  expect(projects.every(project => !project.error)).toBe(true);
});

test('a newly discovered nested project is charged before its command files are read', async () => {
  const child = sized('bad/sub/Master.dss', 30 * 1024 * 1024);
  const read = vi.spyOn(child.file, 'text');
  const projects = await groupInputs([sized('bad/case.dss', 40 * 1024 * 1024, 'Redirect sub/Master.dss'), child, entry('good/Master.dss')]);
  expect(read).not.toHaveBeenCalled();
  expect(projects).toHaveLength(2);
  expect(projects.find(project => project.name === 'bad')?.error?.code).toBe('WEB.INPUT.LIMIT');
  expect(projects.find(project => project.name === 'good')?.error).toBeUndefined();
});

test('parent-relative includes with nonstandard suffixes are bounded before they are read', async () => {
  const child = sized('shared/loads.inc', 30 * 1024 * 1024);
  const read = vi.spyOn(child.file, 'text');
  const projects = await groupInputs([
    sized('bad/Master.dss', 40 * 1024 * 1024, 'Redirect ../shared/loads.inc'), child, entry('good/Master.dss'),
  ]);
  expect(read).not.toHaveBeenCalled();
  expect(projects).toHaveLength(2);
  expect(projects.find(project => project.primary === 'bad/Master.dss')?.error?.code).toBe('WEB.INPUT.LIMIT');
  expect(projects.find(project => project.name === 'good')?.error).toBeUndefined();
});

test('newly reached include counts are bounded before any of those includes is read', async () => {
  const includes = Array.from({ length: 4096 }, (_, index) => entry(`shared/${index}.inc`));
  const read = vi.spyOn(includes[0].file, 'text');
  const master = entry('bad/Master.dss', includes.map(file => `Redirect ../${file.path}`).join('\n'));
  const projects = await groupInputs([master, ...includes, entry('good/Master.dss')]);
  expect(read).not.toHaveBeenCalled();
  expect(projects).toHaveLength(2);
  expect(projects.find(project => project.primary === 'bad/Master.dss')?.error?.code).toBe('WEB.INPUT.LIMIT');
  expect(projects.find(project => project.name === 'good')?.error).toBeUndefined();
});

test('the exact discovery byte limit remains accepted', async () => {
  const master = sized('one/Master.dss', 64 * 1024 * 1024);
  const read = vi.spyOn(master.file, 'text');
  const [project] = await groupInputs([master]);
  expect(read).toHaveBeenCalledOnce();
  expect(project.error).toBeUndefined();
});

test('read failures are retained in wrapper-stripped project inventories', async () => {
  const master = entry('batch/one/model/Master.dss');
  vi.spyOn(master.file, 'text').mockRejectedValue(new DOMException('Unavailable', 'NotReadableError'));
  const [project] = await groupInputs([master, entry('batch/one/notes.asset')]);
  expect(project.error?.code).toBe('WEB.INPUT.DISCOVERY');
  expect(project.entries).toHaveLength(2);
});

test('a failed shared include affects its dependents while independent projects survive', async () => {
  const shared = entry('batch/shared/loads.inc');
  vi.spyOn(shared.file, 'text').mockRejectedValue(new DOMException('Unavailable', 'NotReadableError'));
  const projects = await groupInputs([
    entry('batch/one/Master.dss', 'Redirect ../shared/loads.inc'), shared,
    entry('batch/two/Master.dss', 'Redirect ../shared/loads.inc'), entry('batch/good/Master.dss'), entry('case.m'),
  ]);
  expect(projects).toHaveLength(4);
  expect(projects.filter(project => project.error?.code === 'WEB.INPUT.DISCOVERY')).toHaveLength(2);
  expect(projects.find(project => project.name === 'good')?.error).toBeUndefined();
  expect(projects.find(project => project.name === 'case.m')?.error).toBeUndefined();
});

test('an oversized neighbor cannot taint a small shared include used by a valid project', async () => {
  const projects = await groupInputs([
    entry('one/Master.dss', 'Redirect ../two/helper.dss'), entry('two/helper.dss'),
    sized('two/other.dss', 40 * 1024 * 1024), sized('two/opaque.asset', 30 * 1024 * 1024),
  ]);
  expect(projects).toHaveLength(2);
  expect(projects.find(project => project.primary === 'one/Master.dss')?.error).toBeUndefined();
  expect(projects.find(project => project.name === 'two')?.error?.code).toBe('WEB.INPUT.LIMIT');
});

test('a failed XML prefix read does not abort an independent case', async () => {
  const xml = entry('bad.xml');
  const prefix = new Blob();
  vi.spyOn(prefix, 'text').mockRejectedValue(new DOMException('Unavailable', 'NotReadableError'));
  vi.spyOn(xml.file, 'slice').mockReturnValue(prefix);
  const projects = await groupInputs([xml, entry('case.m')]);
  expect(projects).toHaveLength(2);
  expect(projects.find(project => project.name === 'bad.xml')?.error?.code).toBe('WEB.INPUT.DISCOVERY');
  expect(projects.find(project => project.name === 'case.m')?.error).toBeUndefined();
});

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


test('retains nonstandard DSS companions and opaque assets', async () => {
  const [project] = await groupInputs([
    entry('one/Master.dss', 'Redirect "loads.inc"'), entry('one/loads.inc'), entry('one/notes.asset'), entry('one/metadata.json'),
  ]);
  expect(project.primary).toBe('Master.dss');
  expect(project.entries.map(file => file.path)).toEqual(['Master.dss', 'loads.inc', 'notes.asset', 'metadata.json']);
});

test('separates DSS projects beneath a batch wrapper', async () => {
  const projects = await groupInputs(['one', 'two'].flatMap(id => [
    entry(`batch/dss/${id}/case.dss`, 'Redirect source/Master.dss'),
    entry(`batch/dss/${id}/source/Master.dss`, 'Redirect loads.inc'),
    entry(`batch/dss/${id}/source/loads.inc`), entry(`batch/dss/${id}/notes.asset`),
  ]));
  expect(projects).toHaveLength(2);
  for (const project of projects) {
    expect(project.primary).toBe('case.dss');
    expect(project.entries.map(file => file.path)).toEqual(['case.dss', 'source/Master.dss', 'source/loads.inc', 'notes.asset']);
  }
});

test('retains parent-relative references without absorbing another project', async () => {
  const projects = await groupInputs([
    entry('batch/one/Master.dss', 'Redirect ../shared/loads.inc'), entry('batch/one/note.asset'),
    entry('batch/shared/loads.inc'), entry('batch/two/Master.dss'), entry('batch/two/other.asset'),
  ]);
  expect(projects).toHaveLength(2);
  const one = projects.find(project => project.primary === 'one/Master.dss')!;
  expect(one.entries.map(file => file.path)).toEqual(['one/Master.dss', 'one/note.asset', 'shared/loads.inc']);
  expect(projects.find(project => project.primary === 'Master.dss')!.entries).toHaveLength(2);
});


test('shared discovery keeps overlapping masters and mixed inputs without consuming companions', async () => {
  const { groupCollectionInputs } = await import('./inputs');
  const files = ['masters/day.dss', 'masters/night.dss', 'shared/topology.inc', 'case.m'].map(path => ({ path, file: new File(['content'], path.split('/').at(-1)!) }));
  const discover = async () => ({ ok: true, candidates: ['masters/day.dss', 'masters/night.dss', 'case.m'].map(primary => ({ status: 'selectable', selection: { primary, root: null, format: primary.endsWith('.m') ? 'matpower' : 'dss', operation: 'parse' as const } })) });
  const cases = await groupCollectionInputs(files, discover, 'shared', () => false);
  expect(cases).toHaveLength(3);
  expect(cases[0].entries).toBe(cases[1].entries);
  expect(cases.map(c => c.primary)).toEqual(['masters/day.dss', 'masters/night.dss', 'case.m']);
  expect(cases.every(c => c.collectionKey === 'shared')).toBe(true);
});


test('ZIP selections cannot resolve into a sibling upload', async () => {
  const { groupCollectionInputs } = await import('./inputs');
  const files = [{ ...entry('first/master.dss'), collectionRoot: 'first' }, { ...entry('second/shared.inc'), collectionRoot: 'second' }];
  const discover = async () => ({ ok: true, candidates: [{ status: 'selectable', selection: { primary: 'first/master.dss', root: null, format: 'dss', operation: 'parse' as const } }] });
  const [selected] = await groupCollectionInputs(files, discover, 'key', () => false);
  expect(selected.selection).toMatchObject({ root: 'first', primary: 'master.dss' });
  expect(selected.paths).toEqual(['master.dss']);
  expect(selected.entries).toHaveLength(2);
});

test.each([['a/file.dss', 'A/other.dss'], ['file', 'file/child'], ['nul.txt'], ['bad./x']])('rejects nonportable ZIP namespace %j', async (...paths: string[]) => {
  const zip = new ZipWriter(new BlobWriter(), { useWebWorkers: false });
  for (const path of paths) await zip.add(path, new BlobReader(new Blob(['data'])));
  await expect(unpack({ path: 'bad.zip', file: new File([await zip.close()], 'bad.zip') })).rejects.toThrow();
});

test('rejects ZIP symlinks and observes cancellation before payload reads', async () => {
  const zip = new ZipWriter(new BlobWriter(), { useWebWorkers: false });
  await zip.add('link.dss', new BlobReader(new Blob(['target'])), { unixMode: 0o120777 });
  const archive = { path: 'bad.zip', file: new File([await zip.close()], 'bad.zip') };
  await expect(unpack(archive)).rejects.toThrow(/links|special/);
  await expect(unpack(archive, () => true)).rejects.toThrow(/cancelled/);
});
