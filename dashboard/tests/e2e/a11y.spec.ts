import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import { TAB_IDS, openDashboard } from './dashboard';

// axe on every tab, both themes, desktop and phone: no serious or critical violation.
// The table view and the open Filters sheet are scanned too, since they only exist after a click.
for (const theme of ['light', 'dark'] as const) {
  for (const width of [390, 1440]) {
    for (const tab of TAB_IDS) {
      test(`axe: ${tab} at ${width} px, ${theme}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await openDashboard(page, { search: tab === 'overview' ? '' : `?tab=${tab}`, theme });
        if (tab === 'overview') await page.getByRole('button', { name: 'Table' }).click();
        if (width === 390) await page.getByRole('button', { name: /^Filters/ }).click();
        const results = await new AxeBuilder({ page }).analyze();
        const blocking = results.violations
          .filter((v) => v.impact === 'serious' || v.impact === 'critical')
          .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`);
        expect(blocking).toEqual([]);
      });
    }
  }
}

for (const api of ['empty', 'error'] as const) {
  test(`axe: every tab ${api}`, async ({ page }) => {
    for (const tab of TAB_IDS) {
      await openDashboard(page, { search: `?tab=${tab}`, api, theme: 'dark' });
      const results = await new AxeBuilder({ page }).analyze();
      const blocking = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
      expect(blocking.map((v) => `${tab}: ${v.id}`)).toEqual([]);
      await page.unrouteAll();
    }
  });
}
