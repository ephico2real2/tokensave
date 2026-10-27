import { expect, test } from '@playwright/test';
import { TAB_IDS, openDashboard } from './dashboard';

// Every tab, since each draws its own sections (and their inline sizes and colours).
for (const tab of TAB_IDS) {
  test(`${tab} runs under the server CSP (default-src self) with no console error`, async ({ page }) => {
    const problems: string[] = [];
    page.on('console', (m) => m.type() === 'error' && problems.push(m.text()));
    page.on('pageerror', (e) => problems.push(e.message));
    await openDashboard(page, { search: tab === 'overview' ? '' : `?tab=${tab}` });
    const response = await page.request.get('/');
    expect(response.headers()['content-security-policy']).toBe("default-src 'self'");
    expect(problems).toEqual([]);
  });
}

test('loads no remote resource: every request stays on the page origin', async ({ page }) => {
  const origins = new Set<string>();
  page.on('request', (r) => origins.add(new URL(r.url()).origin));
  await openDashboard(page);
  expect([...origins]).toEqual(['http://127.0.0.1:4173']);
});

test('Auto follows the operating system, live', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  await openDashboard(page, { theme: 'auto' });
  const bg = () => page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(await bg()).toBe('rgb(15, 18, 22)');
  await page.emulateMedia({ colorScheme: 'light' });
  expect(await bg()).toBe('rgb(246, 247, 249)');
});

test('a refresh keeps the tab, range and filters; the theme comes back from storage', async ({ page }) => {
  await openDashboard(page, { search: '?tab=repos&range=custom&from=2026-10-01&to=2026-10-20&tool=Codex' });
  await page.getByRole('button', { name: 'Light' }).click();
  await page.reload();
  await expect(page.getByRole('button', { name: 'Repos' })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByText('Oct 1 – Oct 20, 2026 · 20 days')).toBeVisible();
  await expect(page.getByRole('combobox', { name: /Agent tool/ })).toHaveValue('Codex');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
});

test('an invalid address falls back to the defaults', async ({ page }) => {
  const requests = await openDashboard(page, { search: '?tab=nope&range=365&effort=extreme' });
  await expect(page.getByRole('button', { name: 'Overview' })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('button', { name: '30 days' })).toHaveAttribute('aria-pressed', 'true');
  expect(requests).toContain('/api/summary?range=30');
  await expect(page).toHaveURL(/127\.0\.0\.1:4173\/$/);
});

test('a failing server shows each section’s error, and Retry recovers', async ({ page }) => {
  let failing = true;
  await page.clock.setFixedTime(new Date('2026-10-26T12:00:00Z'));
  await page.route('**/api/**', async (route) => {
    if (failing) return route.fulfill({ status: 500, body: 'database is locked' });
    const { fixtureFor } = await import('../fixtures/mockup');
    const url = new URL(route.request().url());
    return route.fulfill({ json: fixtureFor(url.pathname, url.searchParams) });
  });
  await page.goto('/?tab=agents');
  await expect(page.getByRole('alert')).toHaveCount(3);
  failing = false;
  await page.getByRole('alert').filter({ hasText: '/api/runs?' }).getByRole('button', { name: 'Retry' }).click();
  await expect(page.locator('[data-dataset="runs"] .ts-line')).toHaveCount(7);
});

test('the chart scrolls inside its card on a phone; the page does not', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openDashboard(page, { search: '?range=90' });
  const box = page.getByRole('region', { name: 'Daily chart, scrolls sideways' });
  const { client, scroll } = await box.evaluate((el) => ({ client: el.clientWidth, scroll: el.scrollWidth }));
  expect(scroll).toBeGreaterThan(client);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

test('on a phone, a link to the last tab shows that tab in the scrolled tab row', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openDashboard(page, { search: '?tab=sources' });
  const nav = await page.getByRole('navigation', { name: 'Dashboard sections' }).boundingBox();
  const tab = await page.getByRole('button', { name: 'Sources' }).boundingBox();
  expect(tab!.x).toBeGreaterThanOrEqual(nav!.x);
  expect(tab!.x + tab!.width).toBeLessThanOrEqual(nav!.x + nav!.width + 0.5);
});

test('the index command stays on one line (it scrolls in its box when it does not fit)', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openDashboard(page, { search: '?tab=repos' });
  const code = page.locator('[data-component="callout"] code');
  await expect(code).toHaveText('tokensave workspace init-all --dry-run');
  await page.setViewportSize({ width: 320, height: 700 });
  const lines = await code.evaluate((el) => {
    const range = document.createRange();
    range.selectNodeContents(el);
    return new Set([...range.getClientRects()].map((r) => Math.round(r.top))).size;
  });
  expect(lines).toBe(1);
});
