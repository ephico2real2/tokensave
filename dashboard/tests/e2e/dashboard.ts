import type { Page } from '@playwright/test';
import { MOCKUP_NOW, emptyFor, fixtureFor } from '../fixtures/mockup';

export const TAB_IDS = ['overview', 'agents', 'models', 'repos', 'savings', 'sources'] as const;
export type TabId = (typeof TAB_IDS)[number];

/** The mockup checker's screens (inkcast scripts/mockups/check.py WIDTHS). */
export const SCREENS = [
  { width: 390, height: 844 },
  { width: 820, height: 1100 },
  { width: 1440, height: 900 },
] as const;

export type ApiMode = 'fixture' | 'empty' | 'error';

/**
 * Opens the dashboard with the mockup's clock (2026-10-26, noon UTC), the API answered from
 * the fixtures (or empty, or failing), and optionally a stored theme. Resolves once every
 * section has finished loading.
 */
export async function openDashboard(
  page: Page,
  { search = '', theme, api = 'fixture' }: { search?: string; theme?: 'dark' | 'light' | 'auto'; api?: ApiMode } = {},
): Promise<string[]> {
  const requests: string[] = [];
  await page.clock.setFixedTime(new Date(MOCKUP_NOW));
  if (theme) {
    await page.addInitScript((t) => window.localStorage.setItem('theme', t), theme);
  }
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    requests.push(`${url.pathname}${url.search}`);
    if (api === 'error') {
      await route.fulfill({ status: 500, contentType: 'text/plain', body: 'database is locked' });
      return;
    }
    const body = api === 'empty' ? emptyFor(url.pathname) : fixtureFor(url.pathname, url.searchParams);
    await route.fulfill({ json: body });
  });
  await page.goto(`/${search}`);
  await settled(page);
  return requests;
}

/** Waits until no section is loading. */
export async function settled(page: Page): Promise<void> {
  await page.locator('main').waitFor();
  await page.waitForFunction(() => document.querySelectorAll('[aria-busy="true"]').length === 0 && !document.body.textContent?.includes('connecting…'));
}
