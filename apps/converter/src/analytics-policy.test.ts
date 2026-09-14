import { expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import '../public/analytics-policy.js';

const { normalize, safeCode, safeFormat, createBudget } = globalThis.powerioAnalyticsPolicy;

test('free text, numeric values, nested data, and electrical codes cannot enter telemetry', () => {
  const secret = 'PRIVATE.UTILITY.1920';
  expect(normalize('problem', {
    source: secret, target: secret, stage: 'parse', code: secret,
    message: secret, targetId: secret, spans: [{ source: secret }], voltage: 230,
  })).toEqual({ name: 'problem', data: { source: 'unknown', target: 'unknown', stage: 'parse', code: 'other' } });
  expect(safeCode('READ.CGMES.CONNECTIVITY_INSUFFICIENT')).toBe('other');
  expect(safeCode('EMIT.BMOPF.BUS_LOCATION_DROPPED')).toBe('other');
  expect(safeCode('READ.DSS.INCLUDE_LOAD_FAILED')).toBe('READ.DSS.INCLUDE_LOAD_FAILED');
  expect(safeFormat('dss')).toBe('dss');
  expect(normalize('batch_started', { count: '999', version: secret })).toEqual({ name: 'batch_started', data: {} });
  expect(normalize(secret, {})).toBeNull();
  expect(normalize('__proto__', {})).toBeNull();
  expect(normalize('problem', [secret])).toBeNull();
  expect(normalize('problem', null)).toBeNull();
  expect(normalize('parse_result', Object.create({ source: secret }))).toEqual({ name: 'parse_result', data: {} });
});

test('every collected parser code is declared in the source and engine version is explicit', () => {
  const policy = readFileSync(new URL('../public/analytics-policy.js', import.meta.url), 'utf8');
  const source = ['powerio-core/src/codes.rs', 'powerio-tx/src/diagnostics.rs', 'powerio-dist/src/diagnostics.rs',
    'powerio/src/codes.rs', 'powerio-wasm/src/lib.rs', 'apps/converter/src/controller.ts']
    .map(path => readFileSync(new URL(`../../../${path}`, import.meta.url), 'utf8')).join('\n');
  const codes = [...policy.matchAll(/'(?:PARSE|READ|REQUEST|EMIT|WEB)\.[A-Z_.]+'/g)].map(match => match[0].slice(1, -1));
  expect(codes.length).toBeGreaterThan(20);
  for (const code of codes) expect(source, code).toContain(code);
  const cargo = readFileSync(new URL('../../../Cargo.toml', import.meta.url), 'utf8');
  const version = cargo.match(/^version = "([^"]+)"/m)![1];
  expect(normalize('batch_started', { version })?.data.version).toBe(version);
});

test('problem events are deduplicated and capped independently of usage events', () => {
  const budget = createBudget();
  const problem = normalize('problem', { source: 'dss', stage: 'parse', code: 'READ.DSS.INCLUDE_LOAD_FAILED' })!;
  expect(budget(problem)).toBe(true);
  expect(budget(problem)).toBe(false);
  for (let index = 1; index < 20; index++) expect(budget({ name: 'problem', data: { code: String(index) } })).toBe(true);
  expect(budget({ name: 'problem', data: { code: 'overflow' } })).toBe(false);
  for (let index = 20; index < 100; index++) expect(budget(normalize('cli_opened')!)).toBe(true);
  expect(budget(normalize('cli_opened')!)).toBe(false);
});
