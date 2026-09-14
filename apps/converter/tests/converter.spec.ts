import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { Uint8ArrayReader, Uint8ArrayWriter, ZipReader, ZipWriter } from '@zip.js/zip.js';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const case9 = await readFile(`${root}tests/data/case9.m`);
const feeder = Buffer.from('Clear\nNew Circuit.example basekv=12.47 bus1=source phases=3\nNew Line.l bus1=source bus2=load phases=3 r1=0.1 x1=0.2 r0=0.3 x0=0.4 length=1\nNew Load.ld bus1=load phases=3 conn=wye kv=12.47 kw=10 kvar=2\n');
async function ready(page: import('@playwright/test').Page) {
  await page.goto('./');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
}
async function downloadBytes(result: import('@playwright/test').Download) {
  return readFile((await result.path())!);
}
async function entries(bytes: Uint8Array) {
  const reader = new ZipReader(new Uint8ArrayReader(bytes), { useWebWorkers: false });
  const output: Record<string, Uint8Array> = {};
  for (const entry of await reader.getEntries()) if (!entry.directory) output[entry.filename] = await entry.getData!(new Uint8ArrayWriter());
  await reader.close(); return output;
}

async function disconnect(context: import('@playwright/test').BrowserContext, browserName: string) {
  if (browserName === 'webkit') {
    // WebKit's offline emulator rejects local Blob reads; HTTP routing preserves local file access.
    await context.route(/^https?:/, route => route.abort('internetdisconnected'));
    return () => context.unroute(/^https?:/);
  }
  await context.setOffline(true);
  return () => context.setOffline(false);
}

test('mixed batch converts locally and downloads original bytes and distribution output', async ({ page, context, browserName }) => {
  const external: string[] = [];
  page.on('request', request => { if (/^https?:/.test(request.url()) && !request.url().startsWith('http://127.0.0.1:4280/')) external.push(request.url()); });
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles([
    { name: 'private-client-case.m', mimeType: 'text/plain', buffer: case9 },
    { name: 'private-feeder.dss', mimeType: 'text/plain', buffer: feeder },
  ]);
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  const reconnect = await disconnect(context, browserName);
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
  const completed = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download all', exact: false }).click();
  const archive = await entries(await downloadBytes(await completed));
  const original = Object.entries(archive).find(([name]) => name.endsWith('.m'))![1];
  expect(Buffer.from(original)).toEqual(case9);
  const distribution = Object.entries(archive).find(([name]) => name.startsWith('pmd-json/') && name.endsWith('.json'))![1];
  expect(JSON.parse(new TextDecoder().decode(distribution)).data_model).toBe('ENGINEERING');
  expect(archive['conversion-report.json']).toBeTruthy();
  expect(external).toEqual([]);
  await reconnect();
});

test('valid cases survive a malformed neighbor and issue reports omit filenames and source content', async ({ page }) => {
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles([
    { name: 'valid.m', mimeType: 'text/plain', buffer: case9 },
    { name: 'private-utility-secret.json', mimeType: 'application/json', buffer: Buffer.from('{"privateBus": "secret-identifier", broken') },
  ]);
  await expect(page.getByText("Couldn't convert", { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
  await page.getByRole('button', { name: /Not the conversion|Get help|Report/ }).first().click();
  const report = await page.getByRole('dialog').getByRole('textbox').inputValue();
  expect(report).not.toContain('private-utility-secret');
  expect(report).not.toContain('secret-identifier');
  expect(report).toContain('Diagnostic codes');
});

test('every applicable transmission writer runs in browser WASM', async ({ page }) => {
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'case9.m', mimeType: 'text/plain', buffer: case9 });
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  const settings = page.getByRole('complementary', { name: 'Convert to' });
  const checkboxes = settings.getByRole('checkbox');
  for (const checkbox of await checkboxes.all()) {
    const label = await checkbox.evaluate(input => input.parentElement?.textContent ?? '');
    if (!label.includes('GO Challenge')) await checkbox.check();
  }
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByText('Conversion finished. Review any warnings, then download your files.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: /^Download case9.m as / })).toHaveCount(16);
  const finished = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download all', exact: false }).click();
  const archive = await entries(await downloadBytes(await finished));
  const zip = new ZipWriter(new Uint8ArrayWriter(), { useWebWorkers: false });
  for (const [path, bytes] of Object.entries(archive)) if (path !== 'conversion-report.json') await zip.add(path, new Uint8ArrayReader(bytes));
  await page.getByRole('button', { name: 'Clear all', exact: true }).click();
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'converted.zip', mimeType: 'application/zip', buffer: Buffer.from(await zip.close()) });
  await expect(page.getByRole('article')).toHaveCount(16);
  await expect(page.getByText('Ready to convert', { exact: true })).toHaveCount(16);
  await expect(page.getByText("Couldn't convert", { exact: true })).toHaveCount(0);
});

test('distribution outputs have explicit BMOPF profiles', async ({ page }) => {
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'feeder.dss', mimeType: 'text/plain', buffer: feeder });
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  const settings = page.getByRole('complementary', { name: 'Convert to' });
  for (const checkbox of await settings.getByRole('checkbox').all()) await checkbox.check();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByText('Conversion finished. Review any warnings, then download your files.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: /^Download feeder.dss as / })).toHaveCount(4);
  await expect(page.getByRole('link', { name: /BMOPF task force/ }).first()).toHaveAttribute('href', 'https://github.com/distribution-system-opt');
  for (const button of await page.getByRole('button', { name: /^Download feeder.dss as / }).all()) {
    const completed = page.waitForEvent('download');
    await button.click();
    const output = await completed;
    const bytes = await downloadBytes(output);
    const newPage = await page.context().newPage();
    await ready(newPage);
    await newPage.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: output.suggestedFilename(), mimeType: 'application/octet-stream', buffer: bytes });
    await expect(newPage.getByText('Ready to convert', { exact: true })).toHaveCount(1);
    await newPage.close();
  }
});

test('ZIP projects preserve nested OpenDSS references', async ({ page }) => {
  await ready(page);
  const zip = new ZipWriter(new Uint8ArrayWriter(), { useWebWorkers: false });
  await zip.add('model/Master.dss', new Uint8ArrayReader(Buffer.from('Clear\nNew Circuit.c basekv=12.47 bus1=source\nRedirect ../load.dss\n')));
  await zip.add('load.dss', new Uint8ArrayReader(Buffer.from('New Load.l bus1=source phases=3 kv=12.47 kw=10 kvar=2\n')));
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'project.zip', mimeType: 'application/zip', buffer: Buffer.from(await zip.close()) });
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  await page.getByRole('complementary', { name: 'Convert to' }).getByRole('checkbox', { name: 'OpenDSS', exact: true }).check();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
  const completed = page.waitForEvent('download');
  await page.getByRole('button', { name: /Download project as OpenDSS/ }).click();
  const output = await completed;
  const bytes = await downloadBytes(output);
  const files = await entries(bytes);
  expect(Object.keys(files).some(path => path.endsWith('/source/load.dss'))).toBe(true);
  const next = await page.context().newPage();
  await ready(next);
  await next.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: output.suggestedFilename(), mimeType: 'application/zip', buffer: bytes });
  await expect(next.getByText('Ready to convert', { exact: true })).toHaveCount(1);
  await next.close();
});

test('landing pages select their target and mobile layout fits the viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('./matpower-to-pandapower/');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await page.getByRole('button', { name: 'Transmission', exact: true }).click();
  await expect(page.getByRole('complementary', { name: 'Convert to' }).getByRole('checkbox', { name: 'pandapower JSON' })).toBeChecked();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(page.getByRole('link', { name: 'PowerIO home' })).toBeVisible();
});


test('read-only and problem readers identify their native types', async ({ page }) => {
  await ready(page);
  const paths = ['ieee-cdf/ieee14cdf.txt', 'powerworld/ACTIVSg200.pwb', 'opfdataset/example_0.json', 'goc3/goc3_small.json'];
  for (const path of paths) {
    await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: path.split('/').at(-1)!, mimeType: 'application/octet-stream', buffer: await readFile(`${root}tests/data/${path}`) });
  }
  await expect(page.getByRole('article')).toHaveCount(4);
  await expect(page.getByText('Ready to convert', { exact: true })).toHaveCount(4);
  await expect(page.getByRole('complementary', { name: 'Convert to' }).getByRole('checkbox', { name: /GO Challenge/ })).toBeDisabled();
});

test('missing dependencies can be added, and changing a format removes stale downloads', async ({ page }) => {
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'Master.dss', mimeType: 'text/plain', buffer: Buffer.from('Clear\nNew Circuit.c basekv=12.47 bus1=source\nRedirect load.dss\n') });
  await expect(page.getByText('Needs files', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Add missing files', exact: true }).click();
  await page.getByLabel('Add missing project files', { exact: true }).setInputFiles({ name: 'load.dss', mimeType: 'text/plain', buffer: Buffer.from('New Load.l bus1=source phases=3 kv=12.47 kw=10 kvar=2\n') });
  await expect(page.getByText('Ready to convert', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: /^Download Master.dss as / })).toBeVisible();
  await page.getByText('Case settings', { exact: true }).click();
  await page.getByRole('combobox', { name: /Input format/ }).selectOption('dss');
  await expect(page.getByRole('button', { name: /^Download Master.dss as / })).toHaveCount(0);
  await expect(page.getByText('Ready to convert', { exact: true })).toBeVisible();
});

test('settings links contain only allowed output preferences', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'share', { value: undefined, configurable: true });
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text: string) => { (window as any).copiedSettings = text; } }, configurable: true });
  });
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'private-network-name.m', mimeType: 'text/plain', buffer: case9 });
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: /Share.*settings/ }).click();
  const shared = await page.evaluate(() => (window as any).copiedSettings);
  expect(shared).toBe('https://powerio.dev/convert/#v=1&transmission=matpower&distribution=pmd-json');
  expect(shared).not.toContain('private-network');
});


test('cancelled work restarts offline and retains completed outputs', async ({ page, context, browserName }) => {
  await page.addInitScript(() => {
    const post = Worker.prototype.postMessage;
    Worker.prototype.postMessage = function (message: any, options?: any) {
      if ((window as any).pauseNextEmission && message.type === 'emit') {
        (window as any).pauseNextEmission = false;
        return;
      }
      return post.call(this, message, options);
    };
  });
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'case9.m', mimeType: 'text/plain', buffer: case9 });
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: /^Download case9.m as / })).toHaveCount(1);
  for (const checkbox of await page.getByRole('complementary', { name: 'Convert to' }).getByRole('checkbox').all()) {
    if (await checkbox.isEnabled()) await checkbox.check();
  }
  await page.evaluate(() => { (window as any).pauseNextEmission = true; });
  const reconnect = await disconnect(context, browserName);
  await page.getByRole('button', { name: 'Convert again', exact: true }).click();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByText('Cancelled. Completed results are still available.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: /Download case9.m as MATPOWER/ })).toBeVisible();
  await page.getByRole('button', { name: 'Retry failed', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Convert again', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Convert again', exact: true }).click();
  await expect(page.getByText('Conversion finished. Review any warnings, then download your files.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: /^Download case9.m as / })).toHaveCount(16);
  await reconnect();
});

test('unchanged CGMES archives download as the original ZIP', async ({ page }) => {
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'case9.m', mimeType: 'text/plain', buffer: case9 });
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  const targets = page.getByRole('complementary', { name: 'Convert to' });
  await targets.getByRole('checkbox', { name: 'MATPOWER', exact: true }).uncheck();
  await targets.getByRole('checkbox', { name: /CIM CGMES/ }).check();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  const first = page.waitForEvent('download');
  await page.getByRole('button', { name: /Download case9.m as CIM CGMES/ }).click();
  const archive = await downloadBytes(await first);
  await page.getByRole('button', { name: 'Clear all', exact: true }).click();
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'network.zip', mimeType: 'application/zip', buffer: archive });
  await expect(page.getByText('Ready to convert', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  const second = page.waitForEvent('download');
  await page.getByRole('button', { name: /Download network.zip as CIM CGMES/ }).click();
  const echoed = await second;
  expect(echoed.suggestedFilename()).toBe('network.zip');
  expect(await downloadBytes(echoed)).toEqual(archive);
});
