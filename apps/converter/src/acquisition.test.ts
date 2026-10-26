import { expect, test } from 'vitest';
import { acquire, type DropEntry } from './acquisition';
import { groupInputs } from './inputs';

const source = (name: string, text = '', fail = false): DropEntry => ({
  name, isFile: true,
  file: (success, failure) => fail ? failure(new DOMException('Unavailable', 'NotReadableError')) : success(new File([text], name)),
  createReader: () => { throw new Error('Not a directory'); },
});
const folder = (name: string, children: DropEntry[], failAfter = false): DropEntry => ({
  name, isFile: false, file: () => { throw new Error('Not a file'); },
  createReader() {
    let count = 0;
    return { readEntries(success, failure) {
      if (!count++) success(children);
      else if (failAfter) failure(new DOMException('Unavailable', 'NotReadableError'));
      else success([]);
    } };
  },
});

test('acquisition retains successful files and scopes unavailable files to their project', async () => {
  const groups = await groupInputs(await acquire([
    folder('batch', [
      folder('broken', [source('Master.dss', 'Redirect loads.inc'), source('loads.inc', '', true)]),
      folder('working', [source('Master.dss')]),
    ]), source('case.m'),
  ]));
  expect(groups).toHaveLength(3);
  const broken = groups.find(group => group.name === 'broken')!;
  expect(broken.acquisitionFailed).toBe(true);
  expect(broken.paths).toContain('loads.inc');
  expect(broken.entries).toHaveLength(1);
  expect(groups.filter(group => !group.acquisitionFailed)).toHaveLength(2);
});

test('a directory enumeration error blocks its acquired partial inventory and preserves a sibling', async () => {
  const groups = await groupInputs(await acquire([
    folder('batch', [folder('broken', [source('Master.dss')], true), folder('working', [source('Master.dss')])]),
  ]));
  expect(groups).toHaveLength(2);
  expect(groups.find(group => group.name === 'broken')?.acquisitionFailed).toBe(true);
  expect(groups.find(group => group.name === 'working')?.error).toBeUndefined();
});

test('a failed ancestor listing blocks all projects with potentially incomplete inventories', async () => {
  const groups = await groupInputs(await acquire([
    folder('batch', [folder('one', [source('Master.dss')]), folder('two', [source('Master.dss')])], true), source('case.m'),
  ]));
  expect(groups.filter(group => group.acquisitionFailed)).toHaveLength(2);
  expect(groups.find(group => group.name === 'case.m')?.error).toBeUndefined();
});

test('unavailable PyPSA and CGMES members prevent partial project inspection', async () => {
  const groups = await groupInputs(await acquire([
    folder('pypsa', [source('network.csv'), source('buses.csv', '', true)]),
    folder('cgmes', [source('EQ.xml', '<rdf:RDF>'), source('TP.xml', '', true)]), source('case.m'),
  ]));
  expect(groups).toHaveLength(3);
  expect(groups.filter(group => group.acquisitionFailed)).toHaveLength(2);
  expect(groups.find(group => group.name === 'case.m')?.error).toBeUndefined();
});

test('cancellation discards the pending acquisition and stops subsequent callbacks', async () => {
  let cancelled = false;
  let finish!: (file: File) => void;
  const pending = source('pending.m');
  pending.file = success => { finish = success; };
  const collecting = acquire([pending, source('later.m')], () => cancelled);
  cancelled = true; finish(new File([''], 'pending.m'));
  expect(await collecting).toEqual([]);
});

for (const directory of [false, true]) test(`an unavailable ${directory ? 'empty directory' : 'entry script'} still has its own rejected case`, async () => {
  const broken = directory ? folder('broken', []) : source('Master.dss', '', true);
  if (directory) broken.createReader = () => { throw new DOMException('Unavailable', 'NotReadableError'); };
  const groups = await groupInputs(await acquire([broken, source('working.m')]));
  expect(groups).toHaveLength(2);
  expect(groups.filter(group => group.acquisitionFailed)).toHaveLength(1);
  expect(groups.find(group => group.name === 'working.m')?.error).toBeUndefined();
});

test('an unavailable shared include blocks each dependent project without affecting an independent one', async () => {
  const groups = await groupInputs(await acquire([folder('batch', [
    folder('one', [source('Master.dss', 'Redirect ../shared/loads.inc')]),
    folder('two', [source('Master.dss', 'Redirect ../shared/loads.inc')]),
    folder('shared', [source('loads.inc', '', true)]),
    folder('working', [source('Master.dss')]),
  ])]));
  expect(groups).toHaveLength(3);
  expect(groups.filter(group => group.acquisitionFailed)).toHaveLength(2);
  expect(groups.find(group => group.name === 'working')?.error).toBeUndefined();
});

for (const family of ['pypsa', 'cgmes']) test(`a shared listing failure blocks every ${family} project`, async () => {
  const groups = await groupInputs(await acquire([folder('batch', ['one', 'two'].map(name => folder(name,
    family === 'pypsa' ? [source('network.csv'), source('buses.csv')]
      : [source('EQ.xml', '<rdf:RDF>'), source('TP.xml', '<rdf:RDF>')],
  )), true)]));
  expect(groups).toHaveLength(2);
  expect(groups.every(group => group.acquisitionFailed)).toBe(true);
});
