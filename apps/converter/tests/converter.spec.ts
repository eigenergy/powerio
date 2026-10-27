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

test('a discovery read failure is retryable and does not prevent an independent case from converting', async ({ page }) => {
  await page.addInitScript(() => {
    const read = File.prototype.text;
    let failed = false;
    File.prototype.text = function () {
      if (this.name === 'unreadable.dss' && !failed) {
        failed = true;
        return Promise.reject(new DOMException('The selected file is temporarily unreadable.', 'NotReadableError'));
      }
      return read.call(this);
    };
  });
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles([
    { name: 'unreadable.dss', mimeType: 'text/plain', buffer: feeder },
    { name: 'working.m', mimeType: 'text/plain', buffer: case9 },
  ]);
  const broken = page.getByRole('article').filter({ hasText: 'unreadable.dss' });
  const working = page.getByRole('article').filter({ hasText: 'working.m' });
  await expect(page.getByRole('article')).toHaveCount(2);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(broken.getByText("Couldn't convert", { exact: true })).toBeVisible();
  await expect(working.getByText('Ready to convert', { exact: true })).toBeVisible();
  await broken.locator('details.diagnostics > summary').click();
  await expect(broken.getByText('WEB.INPUT.DISCOVERY', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  const downloaded = page.waitForEvent('download');
  await working.getByRole('button', { name: /^Download working.m as MATPOWER/ }).click();
  expect(await downloadBytes(await downloaded)).toEqual(case9);
  await broken.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(broken.getByText('Ready to convert', { exact: true })).toBeVisible();
  await expect(page.getByRole('article')).toHaveCount(2);
  await expect(working.getByRole('button', { name: /^Download working.m as MATPOWER/ })).toBeEnabled();
  await page.getByRole('button', { name: 'Convert again', exact: true }).click();
  await expect(broken.getByRole('button', { name: /^Download unreadable.dss as PMD/ })).toBeEnabled();
});

test('an oversized loose DSS file is rejected before discovery while a valid neighbor remains usable', async ({ page }) => {
  await page.addInitScript(() => {
    const size = Object.getOwnPropertyDescriptor(Blob.prototype, 'size')!.get!;
    Object.defineProperty(File.prototype, 'size', { configurable: true, get() {
      return this.name === 'oversized.dss' ? 64 * 1024 * 1024 + 1 : size.call(this);
    } });
    const read = File.prototype.text;
    (window as any).oversizedDiscoveryReads = 0;
    File.prototype.text = function () {
      if (this.name === 'oversized.dss') (window as any).oversizedDiscoveryReads++;
      return read.call(this);
    };
  });
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles([
    { name: 'oversized.dss', mimeType: 'text/plain', buffer: feeder },
    { name: 'working.m', mimeType: 'text/plain', buffer: case9 },
  ]);
  const large = page.getByRole('article').filter({ hasText: 'oversized.dss' });
  const working = page.getByRole('article').filter({ hasText: 'working.m' });
  await expect(large.getByText("Couldn't convert", { exact: true })).toBeVisible();
  await expect(working.getByText('Ready to convert', { exact: true })).toBeVisible();
  await large.locator('details.diagnostics > summary').click();
  await expect(large.getByText('WEB.INPUT.LIMIT', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).oversizedDiscoveryReads)).toBe(0);
  await large.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(large.getByText("Couldn't convert", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).oversizedDiscoveryReads)).toBe(0);
  await expect(page.getByRole('article')).toHaveCount(2);
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(working.getByRole('button', { name: /^Download working.m as MATPOWER/ })).toBeEnabled();
});

for (const mode of ['file', 'nested-file', 'nested-directory'] as const) {
  test(`dropped ${mode} acquisition failure preserves independent conversion and blocks partial projects`, async ({ page }) => {
    await ready(page);
    await page.evaluate(({ mode, network, feeder }) => {
      type Entry = import('../src/acquisition').DropEntry;
      const file = (name: string, content: string, failed = false): Entry => ({
        name, isFile: true,
        file: (success, failure) => failed ? failure(new DOMException('Unavailable', 'NotReadableError')) : success(new File([content], name)),
        createReader: () => { throw new Error('Not a directory'); },
      });
      const folder = (name: string, children: Entry[], failed = false): Entry => ({
        name, isFile: false, file: () => { throw new Error('Not a file'); },
        createReader() {
          let batch = 0;
          return { readEntries(success, failure) {
            if (!batch++) success(children);
            else if (failed) failure(new DOMException('Unavailable', 'NotReadableError'));
            else success([]);
          } };
        },
      });
      const broken = mode === 'file' ? file('unavailable.dss', '', true)
        : folder('batch', [folder('broken', [file('Master.dss', feeder), ...(mode === 'nested-file' ? [file('notes.asset', '', true)] : [])], mode === 'nested-directory')]);
      const event = new Event('drop', { bubbles: true, cancelable: true });
      Object.defineProperty(event, 'dataTransfer', { value: { types: ['Files'], files: [], items: [file('working.m', network), broken].map(entry => ({ webkitGetAsEntry: () => entry })) } });
      document.querySelector('.drop-zone')!.dispatchEvent(event);
    }, { mode, network: case9.toString(), feeder: feeder.toString() });
    await expect(page.getByRole('article')).toHaveCount(2);
    const working = page.getByRole('article', { name: 'working.m', exact: true });
    const broken = page.getByRole('article').filter({ hasText: /Couldn't convert/ });
    await expect(working.getByText('Ready to convert', { exact: true })).toBeVisible();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(broken.locator('.job-help')).toContainText('choose its complete files or folder again');
    await broken.locator('details.diagnostics > summary').click();
    await expect(broken.getByText('WEB.INPUT.ACQUISITION', { exact: true })).toBeVisible();
    await expect(broken.getByRole('button', { name: 'Retry', exact: true })).toHaveCount(0);
    await expect(broken.getByRole('button', { name: 'Add missing files', exact: true })).toBeHidden();
    await broken.getByText('Case settings', { exact: true }).click();
    await expect(broken.getByRole('combobox', { name: 'Input format' })).toHaveCount(0);
    await page.getByRole('button', { name: 'Convert', exact: true }).click();
    const downloaded = page.waitForEvent('download');
    await working.getByRole('button', { name: /^Download working.m as MATPOWER/ }).click();
    expect(await downloadBytes(await downloaded)).toEqual(case9);
    await expect(broken.getByRole('button', { name: /^Download / })).toHaveCount(0);
    await broken.getByRole('button', { name: /^Remove / }).click();
    await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'reselected.dss', mimeType: 'text/plain', buffer: feeder });
    await expect(page.getByRole('article', { name: 'reselected.dss', exact: true }).getByText('Ready to convert', { exact: true })).toBeVisible();
    await expect(working.getByRole('button', { name: /^Download working.m as MATPOWER/ })).toBeEnabled();
  });
}

test('cancel during initial drop acquisition stops stale cases before a new selection', async ({ page }) => {
  await ready(page);
  await page.evaluate(() => {
    const entry = { isFile: true, name: 'cancelled.m', file: (success: (file: File) => void) => { (window as any).finishAcquisition = () => success(new File([''], 'cancelled.m')); } };
    const event = new Event('drop', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'dataTransfer', { value: { types: ['Files'], files: [], items: [{ webkitGetAsEntry: () => entry }] } });
    document.querySelector('.drop-zone')!.dispatchEvent(event);
  });
  await expect(page.getByRole('button', { name: 'Cancel', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.evaluate(() => (window as any).finishAcquisition());
  await page.getByRole('button', { name: 'Transmission', exact: true }).click();
  await expect(page.getByRole('article', { name: 'case9.m', exact: true }).getByText('Ready to convert', { exact: true })).toBeVisible();
  await expect(page.getByRole('article')).toHaveCount(1);
});

test('privacy disclosure links to the committed analytics template', async ({ page }) => {
  await ready(page);
  await page.getByText('Your files stay here.', { exact: true }).click();
  await expect(page.getByRole('link', { name: 'Review the analytics policy', exact: true })).toHaveAttribute('href', 'https://github.com/eigenergy/powerio/blob/main/apps/converter/scripts/analytics-policy.template.js');
  await expect(page.locator('#privacy')).toContainText('The build supplies the one permitted PowerIO version.');
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
  await expect(page.getByRole('button', { name: /^Download feeder.dss as / })).toHaveCount(5);
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

test('CIM distribution and explicit SINCAL profiles reach ready jobs', async ({ page }) => {
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({
    name: 'feeder.xml', mimeType: 'application/xml', buffer: await readFile(`${root}tests/data/dist/cim/micro_feeder.xml`),
  });
  await page.getByText('Case settings', { exact: true }).click();
  await page.getByRole('combobox', { name: 'Input format' }).selectOption('cim-dist');
  await expect(page.getByText('Ready to convert', { exact: true })).toHaveCount(1);
  await page.getByRole('button', { name: 'Clear all', exact: true }).click();
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({
    name: 'case.sinx', mimeType: 'application/octet-stream', buffer: await readFile(`${root}tests/data/sincal/1-LV-rural1--0-sw.sinx`),
  });
  await page.getByText('Case settings', { exact: true }).click();
  await page.getByRole('combobox', { name: 'Input format' }).selectOption('sincal-balanced');
  await expect(page.getByText('Ready to convert', { exact: true })).toHaveCount(1);
  await expect(page.getByRole('complementary', { name: 'Convert to' }).getByRole('checkbox', { name: /SINCAL/ })).toHaveCount(0);
});

test('two DSS projects keep their contents and companions after batch download and re-import', async ({ page }) => {
  const source = new ZipWriter(new Uint8ArrayWriter(), { useWebWorkers: false });
  for (const [id, kw] of [['one', 10], ['two', 20]] as const) {
    await source.add(`${id}/Master.dss`, new Uint8ArrayReader(Buffer.from(`New Circuit.${id} basekv=12.47 bus1=source\nRedirect loads.inc\n`)));
    await source.add(`${id}/loads.inc`, new Uint8ArrayReader(Buffer.from(`New Load.${id} bus1=source phases=3 kv=12.47 kw=${kw} kvar=2\n`)));
    await source.add(`${id}/notes.asset`, new Uint8ArrayReader(Buffer.from(`companion ${id}`)));
  }
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'projects.zip', mimeType: 'application/zip', buffer: Buffer.from(await source.close()) });
  await expect(page.getByText('Ready to convert', { exact: true })).toHaveCount(2);
  const targets = page.getByRole('complementary', { name: 'Convert to' });
  for (const checkbox of await targets.getByRole('checkbox').all()) if (await checkbox.isEnabled()) await checkbox.uncheck();
  await targets.getByRole('checkbox', { name: 'OpenDSS', exact: true }).check();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
  const downloaded = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download all', exact: false }).click();
  const archive = await entries(await downloadBytes(await downloaded));
  for (const id of ['one', 'two']) {
    const asset = Object.entries(archive).find(([path]) => path.includes(`/${id}/`) && path.endsWith('notes.asset'));
    expect(asset).toBeTruthy();
    expect(Buffer.from(asset![1]).toString()).toBe(`companion ${id}`);
    const load = Object.entries(archive).find(([path]) => path.includes(`/${id}/`) && path.endsWith('loads.inc'));
    expect(Buffer.from(load![1]).toString()).toContain(`kw=${id === 'one' ? 10 : 20}`);
  }
  const grids = new ZipWriter(new Uint8ArrayWriter(), { useWebWorkers: false });
  for (const [path, bytes] of Object.entries(archive)) if (path !== 'conversion-report.json') await grids.add(path, new Uint8ArrayReader(bytes));
  await page.getByRole('button', { name: 'Clear all', exact: true }).click();
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'batch.zip', mimeType: 'application/zip', buffer: Buffer.from(await grids.close()) });
  await expect(page.getByRole('article')).toHaveCount(2);
  await expect(page.getByText('Ready to convert', { exact: true })).toHaveCount(2);
});


test('indexed MATLAB mutations stay visible through conversion, report and exact source download', async ({ page }) => {
  const mutation = "mpc.bus(:, [3 4]) ... ignored ) ' ; ,\n = mpc.bus(:, [3 4]) / 1000;";
  const source = Buffer.concat([case9, Buffer.from(`\nx = [1 2].';\n${mutation}\n`)]);
  await ready(page);
  await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'mutated.m', mimeType: 'text/plain', buffer: source });
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  const job = page.getByRole('article');
  await job.locator('details.diagnostics > summary').click();
  await expect(job.getByText('READ.MATPOWER.INDEXED_ASSIGNMENT', { exact: true })).toBeVisible();
  await expect(job.getByText(/indexed assignment to mpc.bus is not evaluated/)).toBeVisible();
  const targets = page.getByRole('complementary', { name: 'Convert to' });
  for (const checkbox of await targets.getByRole('checkbox').all()) if (await checkbox.isEnabled()) await checkbox.uncheck();
  await targets.getByRole('checkbox', { name: 'MATPOWER', exact: true }).check();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(job.getByText('Converted with warnings', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
  const downloaded = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download all', exact: false }).click();
  const archive = await entries(await downloadBytes(await downloaded));
  const original = Object.entries(archive).find(([path]) => path.endsWith('.m'));
  expect(original).toBeTruthy();
  expect(Buffer.from(original![1])).toEqual(source);
  const report = JSON.parse(new TextDecoder().decode(archive['conversion-report.json']));
  expect(report.cases).toHaveLength(1);
  const warning = report.cases[0].diagnostics.find((item: { code: string }) => item.code === 'READ.MATPOWER.INDEXED_ASSIGNMENT');
  expect(warning).toMatchObject({ severity: 'warning' });
  const start = source.indexOf(Buffer.from(mutation));
  expect(warning.spans[0]).toMatchObject({ start, end: start + Buffer.byteLength(mutation) });
});


for (const unsupported of [false, true]) {
  test(`MATPOWER case33bw ${unsupported ? 'unsupported postprocessing preserves literals and warnings' : 'supported conversions export interpreted values'} through real WASM`, async ({ page }) => {
    const original = await readFile(`${root}tests/data/case33bw.m`);
    const mutation = 'mpc.bus(:, 3) = unknown();';
    const source = unsupported ? Buffer.concat([original, Buffer.from(mutation)]) : original;
    await ready(page);
    await page.getByLabel('Choose power system files', { exact: true }).setInputFiles({ name: 'case33bw.m', mimeType: 'text/plain', buffer: source });
    await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
    const job = page.getByRole('article');
    if (unsupported) {
      await job.locator('details.diagnostics > summary').click();
      await expect(job.getByText('READ.MATPOWER.INDEXED_ASSIGNMENT', { exact: true })).toHaveCount(3);
      await expect(job.getByText('READ.MATPOWER.INDEXED_ASSIGNMENT', { exact: true }).first()).toBeVisible();
    } else {
      await expect(job.getByText('Ready to convert', { exact: true })).toBeVisible();
      await expect(job.getByText('READ.MATPOWER.INDEXED_ASSIGNMENT', { exact: true })).toHaveCount(0);
    }
    const targets = page.getByRole('complementary', { name: 'Convert to' });
    for (const checkbox of await targets.getByRole('checkbox').all()) if (await checkbox.isEnabled()) await checkbox.uncheck();
    await targets.getByRole('checkbox', { name: 'MATPOWER', exact: true }).check();
    await targets.getByRole('checkbox', { name: 'PowerModels JSON', exact: true }).check();
    await page.getByRole('button', { name: 'Convert', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
    const downloaded = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Download all', exact: false }).click();
    const archive = await entries(await downloadBytes(await downloaded));
    const echoed = Object.entries(archive).find(([path]) => path.endsWith('.m'));
    expect(echoed).toBeTruthy();
    expect(Buffer.from(echoed![1])).toEqual(source);
    const projected = Object.entries(archive).find(([path]) => path.startsWith('powermodels-json/') && path.endsWith('.json'));
    expect(projected).toBeTruthy();
    const network = JSON.parse(new TextDecoder().decode(projected![1]));
    expect(network.per_unit).toBe(true);
    const loads = Object.values(network.load) as { pd: number; qd: number }[];
    expect(loads.reduce((sum, load) => sum + load.pd, 0) * network.baseMVA).toBeCloseTo(unsupported ? 3715 : 3.715, 10);
    expect(loads.reduce((sum, load) => sum + load.qd, 0) * network.baseMVA).toBeCloseTo(unsupported ? 2300 : 2.3, 10);
    const firstBranch = Object.values(network.branch).find(branch => (branch as { f_bus: number; t_bus: number }).f_bus === 1 && (branch as { t_bus: number }).t_bus === 2) as { br_r: number; br_x: number };
    expect(firstBranch).toBeTruthy();
    const zbase = 12.66 ** 2 / 10;
    expect(firstBranch.br_r).toBeCloseTo(unsupported ? 0.0922 : 0.0922 / zbase, 12);
    expect(firstBranch.br_x).toBeCloseTo(unsupported ? 0.047 : 0.047 / zbase, 12);
    const report = JSON.parse(new TextDecoder().decode(archive['conversion-report.json']));
    expect(report.cases).toHaveLength(1);
    const warnings = report.cases[0].diagnostics.filter((item: { code: string }) => item.code === 'READ.MATPOWER.INDEXED_ASSIGNMENT');
    expect(warnings).toHaveLength(unsupported ? 3 : 0);
    if (unsupported) {
      for (const warning of warnings) expect(warning).toMatchObject({ severity: 'warning' });
      expect(warnings[2].spans[0]).toMatchObject({ start: original.length, end: source.length });
    }
  });
}
