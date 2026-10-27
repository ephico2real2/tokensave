import { expect, test, type Page } from '@playwright/test';
import { openDashboard } from './dashboard';

/**
 * Presses Tab until focus leaves the page or comes back round, describing each stop as
 * tag:name (a select by its label, a button by its text, a region by its aria-label).
 */
async function tabStops(page: Page): Promise<string[]> {
  const stops: string[] = [];
  for (let i = 0; i < 100; i++) {
    await page.keyboard.press('Tab');
    const stop = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      if (!el || el === document.body || el.dataset.seen) return null;
      el.dataset.seen = '1';
      const label = el instanceof HTMLSelectElement ? el.labels?.[0]?.firstChild?.textContent : undefined;
      const name = el.getAttribute('aria-label') ?? label ?? el.innerText.split('\n')[0] ?? '';
      return `${el.tagName.toLowerCase()}:${name.trim()}`;
    });
    if (stop === null) break;
    stops.push(stop);
  }
  return stops;
}

test('every control on the Overview is reachable by Tab, in reading order', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openDashboard(page);
  const stops = await tabStops(page);
  expect(stops.slice(0, 20)).toEqual([
    'select:Workspace',
    'button:Dark',
    'button:Light',
    'button:Auto',
    'select:Repo',
    'select:Agent tool',
    'select:Model',
    'select:Effort',
    'button:7 days',
    'button:30 days',
    'button:45 days',
    'button:60 days',
    'button:90 days',
    'button:Custom',
    'button:Overview',
    'button:Agents',
    'button:Models and effort',
    'button:Repos',
    'button:Savings',
    'button:Sources',
  ]);
  expect(stops.slice(20)).toEqual(['button:Table', 'div:Daily chart, scrolls sideways']);
});

test('the keyboard drives the tabs, the range, the theme and the table view', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openDashboard(page);

  await page.getByRole('button', { name: 'Agents' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('button', { name: 'Agents' })).toHaveAttribute('aria-current', 'page');
  await expect(page).toHaveURL(/\?tab=agents$/);

  await page.getByRole('button', { name: '90 days' }).focus();
  await page.keyboard.press('Space');
  await expect(page.getByRole('button', { name: '90 days' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByText('Jul 29 – Oct 26, 2026 · 90 days')).toBeVisible();

  await page.getByRole('button', { name: 'Light' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

  await page.getByRole('button', { name: 'Overview' }).focus();
  await page.keyboard.press('Enter');
  await page.getByRole('button', { name: 'Table' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('table')).toBeVisible();
  await expect(page.getByRole('row')).toHaveCount(91);

  // A closed select takes typed letters: "h" picks "high".
  await page.getByRole('combobox', { name: /Effort/ }).focus();
  await page.keyboard.press('h');
  await expect(page).toHaveURL(/effort=high/);
});

test('the Filters sheet opens from the keyboard on a phone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openDashboard(page);
  const repo = page.getByRole('combobox', { name: /Repo/ });
  await expect(repo).toBeHidden();
  const button = page.getByRole('button', { name: /^Filters/ });
  await button.focus();
  await page.keyboard.press('Enter');
  await expect(button).toHaveAttribute('aria-expanded', 'true');
  await expect(repo).toBeVisible();
  await page.keyboard.press('Tab');
  await expect(repo).toBeFocused();
  const box = await repo.boundingBox();
  expect(box?.height).toBeGreaterThanOrEqual(44);
});

test('focus is visible on every control', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openDashboard(page, { theme: 'dark' });
  for (const name of ['Light', '30 days', 'Agents', 'Table']) {
    await page.getByRole('button', { name, exact: true }).focus();
    const outline = await page.evaluate(() => getComputedStyle(document.activeElement!).outlineStyle);
    expect(outline, name).toBe('solid');
  }
});
