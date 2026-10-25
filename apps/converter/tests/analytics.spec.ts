import { test, expect, type BrowserContext, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';

async function productionRoutes(context: BrowserContext, disabled = false) {
  const sent: any[] = [];
  await context.route('https://powerio.dev/**', async route => {
    const url = new URL(route.request().url());
    const response = await route.fetch({ url: `http://127.0.0.1:4280${url.pathname}`, headers: { ...route.request().headers(), host: '127.0.0.1:4280' } });
    await route.fulfill({ response });
  });
  await context.route('https://cloud.umami.is/script.js', async route => {
    if (disabled) return route.abort();
    const body = process.env.UMAMI_SCRIPT_PATH ? await readFile(process.env.UMAMI_SCRIPT_PATH, 'utf8') : `window.umami = { track: payload => fetch('https://gateway.umami.is/api/send', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ type: 'event', payload }) }) };`;
    await route.fulfill({ contentType: 'application/javascript', body });
  });
  await context.route('https://gateway.umami.is/**', async route => {
    if (route.request().method() === 'POST') sent.push(route.request().postDataJSON());
    await route.fulfill({ contentType: 'application/json', body: '{}', headers: { 'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Methods': 'POST, OPTIONS', 'Access-Control-Allow-Headers': '*' } });
  });
  return sent;
}

test('analytics needs consent, remains isolated, and sends only the closed vocabulary', async ({ page, context }) => {
  const sent = await productionRoutes(context);
  const remote: string[] = [];
  context.on('request', request => { if (/https:\/\/(cloud|gateway)\.umami\.is/.test(request.url())) remote.push(request.url()); });
  await page.goto('https://powerio.dev/convert/analytics.html');
  expect(remote).toEqual([]);
  await page.goto('https://powerio.dev/convert/?private=secret-query#private-case');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  expect(sent).toEqual([]);
  expect(remote).toEqual([]);
  await expect(page.locator('iframe')).toHaveCount(0);
  await consent(page);
  await expect.poll(() => sent.length).toBeGreaterThan(0);
  const frame = page.frames().find(frame => frame.url().endsWith('/analytics.html'))!;
  expect(await frame.evaluate(() => {
    try { return !!parent.document.querySelector('input[type=file]'); } catch { return false; }
  })).toBe(false);
  await page.getByRole('button', { name: 'Transmission', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
  await expect.poll(() => sent.some(event => event.payload.name === 'batch_completed')).toBe(true);
  for (const event of sent) {
    expect(Object.keys(event.payload).every(key => ['website', 'hostname', 'url', 'title', 'name', 'data'].includes(key))).toBe(true);
    expect(event.payload.url).toBe('/convert/');
    expect(Object.keys(event.payload.data ?? {}).every(key => ['kind', 'source', 'target', 'count', 'duration', 'outcome', 'stage', 'code', 'version'].includes(key))).toBe(true);
  }
  expect(JSON.stringify(sent)).not.toMatch(/secret-query|private-case|case9|mpc\./);
  await page.getByRole('checkbox', { name: 'Share limited usage and error statistics' }).uncheck();
  await expect(page.locator('iframe')).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await expect(page.locator('iframe')).toHaveCount(0);
});

for (const blocked of ['tracker', 'policy']) {
  test(`blocked analytics ${blocked} never blocks conversion`, async ({ page, context }) => {
    await productionRoutes(context, blocked === 'tracker');
    if (blocked === 'policy') await context.route('**/analytics-policy.js', route => route.abort());
    await page.goto('https://powerio.dev/convert/');
    await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
    if (blocked === 'tracker') await consent(page);
    await page.getByRole('button', { name: 'Transmission', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Convert', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
  });
}

for (const signal of ['doNotTrack', 'globalPrivacyControl']) {
  test(`${signal} overrides a saved analytics preference`, async ({ page, context }) => {
    const sent = await productionRoutes(context);
    await page.addInitScript(signal => {
      Object.defineProperty(navigator, signal, { value: signal === 'doNotTrack' ? '1' : true, configurable: true });
      try { localStorage.setItem('powerio-analytics', 'on'); } catch {}
    }, signal);
    await page.goto('https://powerio.dev/convert/');
    await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
    await page.getByText('Your files stay here.', { exact: true }).click();
    const checkbox = page.getByRole('checkbox', { name: 'Share limited usage and error statistics' });
    await checkbox.click();
    await expect(checkbox).not.toBeChecked();
    await expect(page.locator('iframe')).toHaveCount(0);
    expect(sent).toEqual([]);
  });
}

async function consent(page: Page) {
  await page.getByText('Your files stay here.', { exact: true }).click();
  await page.getByRole('checkbox', { name: 'Share limited usage and error statistics' }).check();
}

test('native parser problems report reviewed codes without messages, paths, or earlier activity', async ({ page, context }) => {
  const sent = await productionRoutes(context);
  await page.goto('https://powerio.dev/convert/');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({
    name: 'BEFORE.CONSENT.json', mimeType: 'application/json', buffer: Buffer.from('{private:'),
  });
  await expect(page.getByText("Couldn't convert", { exact: true })).toBeVisible();
  await consent(page);
  await expect.poll(() => sent.length).toBeGreaterThan(0);
  expect(sent.some(event => event.payload.name === 'problem')).toBe(false);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles([
    { name: 'PRIVATE.UTILITY.dss', mimeType: 'text/plain', buffer: Buffer.from('Clear\nNew Circuit.PRIVATEGRID bus1=SECRET_BUS basekv=12.47\nRedirect SECRET.DEPENDENCY.dss\n') },
    { name: 'PRIVATE.PARSE.m', mimeType: 'text/plain', buffer: Buffer.from('function mpc = PRIVATEGRID\nmpc.version = \'2\';\nmpc.bus = [ SECRET_BUS ];\n') },
  ]);
  await expect.poll(() => sent.some(event => event.payload.data?.code === 'READ.DSS.INCLUDE_LOAD_FAILED')).toBe(true);
  await expect.poll(() => sent.some(event => event.payload.name === 'parse_result' && event.payload.data?.outcome === 'error')).toBe(true);
  const missing = sent.find(event => event.payload.data?.code === 'READ.DSS.INCLUDE_LOAD_FAILED');
  expect(missing.payload.data).toMatchObject({ stage: 'parse', source: 'dss' });
  expect(JSON.stringify(sent)).not.toMatch(/PRIVATE|SECRET|12\.47|BEFORE|mpc|basekv|bus1/);
});

test('the frame rejects private values and duplicate problem events even when the parent sends them', async ({ page, context }) => {
  const sent = await productionRoutes(context);
  await page.goto('https://powerio.dev/convert/?SECRETQUERY');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await consent(page);
  await expect.poll(() => sent.length).toBeGreaterThan(0);
  await page.evaluate(() => {
    const frame = document.querySelector('iframe')!;
    const data = { source: 'PRIVATE.NAME', target: 'PRIVATE.PATH', code: 'READ.CGMES.CONNECTIVITY_INSUFFICIENT', stage: 'parse', message: 'PRIVATE.MESSAGE', count: 999, spans: ['PRIVATE.SPAN'] };
    for (let i = 0; i < 8; i++) frame.contentWindow!.postMessage({ type: 'powerio-usage', name: 'problem', data }, '*');
    frame.contentWindow!.postMessage({ type: 'powerio-usage', name: 'PRIVATE.EVENT', data }, '*');
    frame.contentWindow!.postMessage({ type: 'powerio-usage', name: 'cli_opened', data: {} }, '*');
  });
  await expect.poll(() => sent.some(event => event.payload.name === 'cli_opened')).toBe(true);
  const problems = sent.filter(event => event.payload.name === 'problem');
  expect(problems).toHaveLength(1);
  expect(problems[0].payload.data).toEqual({ source: 'unknown', target: 'unknown', stage: 'parse', code: 'other' });
  expect(JSON.stringify(sent)).not.toMatch(/PRIVATE|SECRETQUERY|CONNECTIVITY|999/);
});

test('opting out clears events queued while the analytics script is loading', async ({ page, context }) => {
  const sent = await productionRoutes(context);
  let requested = false;
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  await context.route('https://cloud.umami.is/script.js', async route => {
    requested = true;
    await pending;
    await route.abort().catch(() => {});
  });
  await page.goto('https://powerio.dev/convert/');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await consent(page);
  await expect.poll(() => requested).toBe(true);
  await page.getByRole('button', { name: 'Transmission', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  await page.getByRole('checkbox', { name: 'Share limited usage and error statistics' }).uncheck();
  release();
  await expect(page.locator('iframe')).toHaveCount(0);
  expect(sent).toEqual([]);
});
