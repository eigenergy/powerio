import { test, expect, type BrowserContext } from '@playwright/test';
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

test('analytics is isolated and transmits only coarse allowlisted data', async ({ page, context }) => {
  const sent = await productionRoutes(context);
  await page.goto('https://powerio.dev/convert/?private=secret-query#private-case');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
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
    expect(Object.keys(event.payload.data ?? {}).every(key => ['kind', 'source', 'target', 'family', 'count', 'duration', 'outcome', 'code', 'version'].includes(key))).toBe(true);
  }
  expect(JSON.stringify(sent)).not.toMatch(/secret-query|private-case|case9|mpc\./);
  await page.getByText('Your files stay here.', { exact: true }).click();
  await page.getByRole('checkbox', { name: 'Allow anonymous usage analytics' }).uncheck();
  await expect(page.locator('iframe')).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await expect(page.locator('iframe')).toHaveCount(0);
});

test('blocked analytics never blocks conversion', async ({ page, context }) => {
  await productionRoutes(context, true);
  await page.goto('https://powerio.dev/convert/');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await page.getByRole('button', { name: 'Transmission', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Convert', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download all', exact: false })).toBeEnabled();
});

test('Do Not Track prevents analytics initialization', async ({ page, context }) => {
  const sent = await productionRoutes(context);
  await page.addInitScript(() => Object.defineProperty(navigator, 'doNotTrack', { value: '1', configurable: true }));
  await page.goto('https://powerio.dev/convert/');
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  await expect(page.locator('iframe')).toHaveCount(0);
  expect(sent).toEqual([]);
});
