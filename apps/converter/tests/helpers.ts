import { expect, type Page } from '@playwright/test';

export async function inspectQueued(page: Page): Promise<void> {
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
  await expect(page.getByRole('button', { name: 'Cancel', exact: true })).toBeHidden();
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  while (await page.getByRole('button', { name: 'Inspect case', exact: true }).count()) {
    await page.getByRole('button', { name: 'Inspect case', exact: true }).first().click();
    await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
  }
  await expect(page.getByRole('button', { name: 'Choose files', exact: false })).toBeEnabled();
}
