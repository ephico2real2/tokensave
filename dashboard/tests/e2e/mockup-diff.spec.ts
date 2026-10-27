import { appendFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';
import { SCREENS, TAB_IDS, openDashboard } from './dashboard';

// Compares the page with the mockup, pixel by pixel, in the same browser. It needs the mockup
// rendered on this machine: MOCKUP_DIR is the --out folder of inkcast's
// scripts/mockups/check.py, which holds one static page per variant (<theme>-<tab>-<range>.html).
// Both are captured full page at the same viewport; the table (report.md, rows appended: remove
// the folder first) gives the share of differing pixels for the whole page and for <main> alone,
// so a header difference that moves everything below it is told apart from one in the content.
// Diff images per case (whole page, and <main>) go beside it. The test fails above MAX_DIFF; each larger difference is
// listed and justified in the PR.
const MOCKUP_DIR = process.env.MOCKUP_DIR;
const OUT = join('test-results', 'mockup-diff');
const REPORT = join(OUT, 'report.md');
const MAX_DIFF = Number(process.env.MOCKUP_MAX_DIFF ?? '1');

// In order, in one worker, so the table rows keep the loop's order.
test.describe.configure({ mode: 'default' });
test.skip(!MOCKUP_DIR, 'MOCKUP_DIR is not set: no mockup to compare with');

test.beforeAll(() => {
  mkdirSync(OUT, { recursive: true });
  if (!existsSync(REPORT)) {
    writeFileSync(
      REPORT,
      '| Tab | Width | Theme | Page px | Mockup px | Whole page | Main only |\n|---|---|---|---|---|---|---|\n',
    );
  }
});

type Box = { x: number; y: number; width: number; height: number };

/** A full-page capture and the box of its <main>. */
async function capture(page: Page): Promise<{ png: PNG; main: Box }> {
  const main = await page.evaluate(() => {
    const r = document.querySelector('main')?.getBoundingClientRect();
    if (!r) throw new Error('no <main>');
    return { x: Math.round(r.x), y: Math.round(r.y + window.scrollY), width: Math.round(r.width), height: Math.round(r.height) };
  });
  return { png: PNG.sync.read(await page.screenshot({ fullPage: true })), main };
}

function crop(img: PNG, box: Box): PNG {
  const width = Math.min(box.width, img.width - box.x);
  const height = Math.min(box.height, img.height - box.y);
  const out = new PNG({ width, height });
  PNG.bitblt(img, out, box.x, box.y, width, height, 0, 0);
  return out;
}

/** Share of differing pixels over the larger image; area only one image has counts as different. */
function compare(a: PNG, b: PNG, diffPath?: string): number {
  const width = Math.min(a.width, b.width);
  const height = Math.min(a.height, b.height);
  const box = { x: 0, y: 0, width, height };
  const diffImg = new PNG({ width, height });
  const common = pixelmatch(crop(a, box).data, crop(b, box).data, diffImg.data, width, height, { threshold: 0.1 });
  if (diffPath) writeFileSync(diffPath, PNG.sync.write(diffImg));
  const total = Math.max(a.width * a.height, b.width * b.height);
  return (common + total - width * height) / total;
}

const percent = (share: number) => `${(share * 100).toFixed(2)} %`;

for (const theme of ['light', 'dark'] as const) {
  for (const tab of TAB_IDS) {
    for (const { width, height } of SCREENS) {
      test(`${tab} at ${width} px, ${theme}, against the mockup`, async ({ page }) => {
        const mockupPage = resolve(MOCKUP_DIR ?? '', `${theme}-${tab}-30.html`);
        expect(existsSync(mockupPage), `${mockupPage} is missing`).toBe(true);
        await page.setViewportSize({ width, height });

        await page.goto(pathToFileURL(mockupPage).href);
        const mockup = await capture(page);
        await page.unrouteAll();
        await openDashboard(page, { search: tab === 'overview' ? '' : `?tab=${tab}`, theme });
        const ours = await capture(page);

        const whole = compare(ours.png, mockup.png, join(OUT, `${theme}-${tab}-${width}-diff.png`));
        const main = compare(
          crop(ours.png, ours.main),
          crop(mockup.png, mockup.main),
          join(OUT, `${theme}-${tab}-${width}-main-diff.png`),
        );
        appendFileSync(
          REPORT,
          `| ${tab} | ${width} | ${theme} | ${ours.png.width}×${ours.png.height} | ${mockup.png.width}×${mockup.png.height} | ${percent(whole)} | ${percent(main)} |\n`,
        );
        expect.soft(main * 100, `<main> differs from the mockup by more than ${MAX_DIFF} %`).toBeLessThanOrEqual(MAX_DIFF);
      });
    }
  }
}
