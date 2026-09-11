import { beforeEach, expect, test, vi } from 'vitest';

const fake = vi.hoisted(() => ({
  paths: [] as string[],
  crash: false, quota: false, closed: 0, removed: [] as string[], current: '',
  pending: undefined as undefined | { resolve: (value: unknown) => void; reject: (error: Error) => void },
  pause: false,
}));
vi.mock('./analytics', () => ({ analyticsEnabled: () => false, configureAnalytics: vi.fn(), track: vi.fn(), bucket: String }));
vi.mock('./worker-client', () => ({ WorkerClient: class {
  async initialize() { return { version: '0.11.1', formats: [
    { token: 'matpower', family: 'transmission', canRead: true, canEmit: true, extension: 'm' },
    { token: 'powermodels-json', family: 'transmission', canRead: true, canEmit: true, extension: 'json' },
  ] }; }
  async request(type: string, data: any) {
    if (type === 'inspect') { fake.current = data.name; return { ok: true, format: 'matpower', family: 'transmission', valueType: 'powerio.BalancedNetwork', diagnostics: [] }; }
    if (type === 'emit') {
      if (fake.crash && fake.current === 'broken.m') throw new Error('WASM stopped');
      if (fake.pause) return new Promise((resolve, reject) => { fake.pending = { resolve, reject }; });
      return { ok: true, fidelity: 'canonical', diagnostics: [], artifacts: [{ path: data.name, bytes: new Uint8Array([1, 2, 3]) }] };
    }
    return {};
  }
  close() { fake.closed++; fake.pending?.reject(new Error('Cancelled')); fake.pending = undefined; }
} }));
vi.mock('./artifacts', () => ({ download: vi.fn(), ArtifactStore: class {
  async initialize() {}
  async put() { if (fake.quota) throw new Error('Quota exceeded'); }
  async remove(key: string) { fake.removed.push(key); }
  async clear() {}
  async zip(entries: { path: string }[]) { fake.paths = entries.map(entry => entry.path); return new Blob(); }
  dispose() {}
} }));
import { ConverterController } from './controller';

beforeEach(() => {
  fake.paths = []; fake.crash = false; fake.quota = false; fake.pause = false; fake.closed = 0; fake.removed = []; fake.pending = undefined;
  vi.stubGlobal('document', { documentElement: { dataset: {} } });
  vi.stubGlobal('location', { hash: '' });
});
const file = (name: string) => new File(['mpc.version = 2;'], name);

test('a trapped writer leaves an error result and the next case still completes', async () => {
  const controller = new ConverterController();
  await controller.addFiles([file('broken.m'), file('working.m')]);
  fake.crash = true;
  await controller.convert();
  expect(controller.state.jobs[0].outputs[0].diagnostics[0].code).toBe('WEB.CONVERT.WORKER');
  expect(controller.state.jobs[1].outputs[0].status).toBe('converted');
  expect(fake.closed).toBeGreaterThan(0);
  controller.dispose();
});

test('changing interpretation invalidates stored artifacts before converting again', async () => {
  const controller = new ConverterController();
  await controller.addFiles([file('case.m')]);
  await controller.convert();
  const job = controller.state.jobs[0];
  const oldOutput = job.outputs[0].id;
  await controller.setFormat(job.id, 'matpower');
  expect(controller.state.jobs[0].outputs).toEqual([]);
  expect(fake.removed).toContain(`${oldOutput}-0`);
  await controller.convert();
  expect(controller.state.jobs[0].outputs[0].id).not.toBe(oldOutput);
  controller.dispose();
});

test('cancel interrupts an active worker and preserves previously completed outputs', async () => {
  const controller = new ConverterController();
  await controller.addFiles([file('case.m')]);
  await controller.convert();
  const original = controller.state.jobs[0].outputs[0].id;
  controller.setTargets('transmission', ['matpower', 'powermodels-json']);
  fake.pause = true;
  const active = controller.convert();
  await vi.waitFor(() => expect(fake.pending).toBeTruthy());
  controller.cancel();
  await active;
  expect(controller.state.phase).toBe('idle');
  expect(controller.state.jobs[0].outputs.map(output => output.id)).toEqual([original]);
  fake.pause = false;
  await controller.retry();
  await controller.convert();
  expect(controller.state.jobs[0].outputs).toHaveLength(2);
  controller.dispose();
});

test('storage exhaustion retains completed results and replaces a retried error', async () => {
  const controller = new ConverterController();
  await controller.addFiles([file('case.m')]);
  await controller.convert();
  const original = controller.state.jobs[0].outputs[0].id;
  controller.setTargets('transmission', ['matpower', 'powermodels-json']);
  fake.quota = true;
  await controller.convert();
  expect(controller.state.jobs[0].outputs[0].id).toBe(original);
  expect(controller.state.jobs[0].outputs[1].diagnostics[0].code).toBe('WEB.STORAGE.FULL');
  await controller.convert();
  expect(controller.state.jobs[0].outputs).toHaveLength(2);
  fake.quota = false;
  await controller.convert();
  expect(controller.state.jobs[0].outputs.every(output => output.status !== 'error')).toBe(true);
  controller.dispose();
});


test('batch archive paths remain unique for duplicate and suffixed case names', async () => {
  const controller = new ConverterController();
  await controller.addFiles([file('case.m'), file('case.m'), file('case-2.m'), file('CASE.m')]);
  await controller.convert();
  await controller.downloadAll();
  expect(fake.paths).toHaveLength(4);
  const directories = fake.paths.map(path => path.split('/')[1].toLowerCase());
  expect(new Set(directories).size).toBe(4);
  controller.dispose();
});
