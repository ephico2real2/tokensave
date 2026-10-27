import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { SCREENS, TAB_IDS, openDashboard } from './dashboard';

// Every tab at the mockup's three widths (plus the design's 768 and 1024) in both themes:
// the page never scrolls sideways, its background fills the screen, and a full-page
// screenshot is kept (test-results/screens, a CI artifact; SCREENSHOT_DIR overrides it).
const SHOTS = process.env.SCREENSHOT_DIR ?? join('test-results', 'screens');
const WIDTHS = [...SCREENS, { width: 768, height: 1024 }, { width: 1024, height: 768 }];

for (const theme of ['light', 'dark'] as const) {
  for (const tab of TAB_IDS) {
    for (const { width, height } of WIDTHS) {
      test(`${tab} at ${width} px, ${theme}`, async ({ page }) => {
        await page.setViewportSize({ width, height });
        await openDashboard(page, { search: tab === 'overview' ? '' : `?tab=${tab}`, theme });

        const { scrollWidth, appHeight, innerHeight } = await page.evaluate(() => ({
          scrollWidth: document.documentElement.scrollWidth,
          appHeight: document.querySelector('.ts-app')?.getBoundingClientRect().height ?? 0,
          innerHeight: window.innerHeight,
        }));
        expect(scrollWidth, 'the page scrolls sideways').toBeLessThanOrEqual(width);
        expect(appHeight, 'the background stops short of the screen').toBeGreaterThanOrEqual(innerHeight);
        await expect(page.locator(`nav [aria-current="page"]`)).toHaveCount(1);
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme);

        mkdirSync(SHOTS, { recursive: true });
        await page.screenshot({ path: join(SHOTS, `${theme}-${tab}-${width}.png`), fullPage: true });
      });
    }
  }
}
