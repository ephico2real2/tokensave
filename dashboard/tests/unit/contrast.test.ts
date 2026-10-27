// @vitest-environment node
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

// Reads tokens.css and checks every colour pair the design names ("Theme and colours" in
// inkcast docs/tooling/tokensave-dashboard/design.md), in both themes: text 4.5:1, large numbers,
// chart marks and category colours 3:1, icon tiles as the design measured them.

const css = readFileSync(fileURLToPath(new URL('../../public/tokens.css', import.meta.url)), 'utf8');

type Theme = Record<string, string>;

function block(selectorStart: string, from = 0): { tokens: Theme; end: number } {
  const start = css.indexOf(selectorStart, from);
  if (start < 0) throw new Error(`tokens.css has no block starting ${selectorStart}`);
  const open = css.indexOf('{', start);
  const close = css.indexOf('}', open);
  const tokens: Theme = {};
  for (const m of css.slice(open + 1, close).matchAll(/--([\w-]+):\s*([^;]+);/g)) tokens[m[1]!] = m[2]!.trim();
  return { tokens, end: close };
}

const dark = block(':root,').tokens;
const light = block(":root[data-theme='light']").tokens;
const autoLight = block(":root[data-theme='auto']", css.indexOf('@media (prefers-color-scheme: light)')).tokens;

function rgb(hex: string): [number, number, number] {
  const m = /^#([0-9a-f]{6})$/i.exec(hex);
  if (!m) throw new Error(`not a #rrggbb colour: ${hex}`);
  const n = parseInt(m[1]!, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function luminance(hex: string): number {
  const [r, g, b] = rgb(hex).map((c) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG 2 contrast ratio. */
export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

/** The icon tile: the colour at --tint over --surface, mixed in sRGB as color-mix() does. */
function tile(t: Theme, color: string): string {
  const a = parseFloat(t.tint!) / 100;
  const s = rgb(t.surface!);
  return `#${rgb(color)
    .map((c, i) => Math.round(a * c + (1 - a) * s[i]!).toString(16).padStart(2, '0'))
    .join('')}`;
}

const CATS = ['emerald', 'sky', 'amber', 'violet', 'coral', 'pink', 'teal', 'slate'];

/** [foreground, background, minimum, what uses it] */
function pairs(): [string, string, number, string][] {
  const p: [string, string, number, string][] = [];
  for (const bg of ['bg', 'surface', 'raised']) {
    p.push(['text', bg, 4.5, `body text on ${bg}`], ['muted', bg, 4.5, `labels on ${bg}`]);
  }
  p.push(
    ['accent', 'surface', 4.5, 'eyebrows, the active tab line'],
    ['accent-fg', 'accent', 4.5, 'pressed buttons'],
    ['logo-fg', 'logo-bg', 3, 'the logo mark'],
    // Big numbers are at least 24 px (app.css .ts-tile-card__value): large text, so 3:1.
    ['saved', 'surface', 3, 'saved bars and the Tokens saved number'],
    ['spent', 'surface', 3, 'spent bars'],
    ['warn-fg', 'surface', 3, 'the Missed savings and Not indexed numbers'],
    ['warn-fg', 'raised', 4.5, 'missed-savings figures'],
    ['warn-fg', 'warn-bg', 4.5, 'the callout title'],
    ['stop-fg', 'surface', 4.5, 'error messages'],
  );
  for (const tone of ['ok', 'warn', 'info', 'stop']) p.push([`${tone}-fg`, `${tone}-bg`, 4.5, `${tone} badges`]);
  for (const k of ['0', '1', '2', '3', 'none']) p.push([`heat-${k}-fg`, `heat-${k}-bg`, 4.5, `heat cells, level ${k}`]);
  for (const c of CATS) p.push([`cat-${c}`, 'surface', 3, `${c} bars and dots`]);
  return p;
}

describe.each([
  ['light', light, 3.25],
  ['dark', dark, 5.1],
] as const)('%s theme', (_name, t, tileMin) => {
  it.each(pairs())('%s on %s ≥ %s:1 (%s)', (fg, bg, min) => {
    expect(t[fg], `--${fg}`).toBeDefined();
    expect(t[bg], `--${bg}`).toBeDefined();
    expect(contrast(t[fg]!, t[bg]!)).toBeGreaterThanOrEqual(min);
  });

  // The design quotes these minimums to one decimal (dark cat-pink measures 5.095:1), so the
  // ratio is compared at that precision.
  it.each(CATS)('cat-%s on its icon tile ≥ the design’s measured minimum', (c) => {
    const color = t[`cat-${c}`]!;
    expect(Math.round(contrast(color, tile(t, color)) * 10) / 10).toBeGreaterThanOrEqual(tileMin);
  });
});

describe('tokens.css', () => {
  it('defines the same tokens in both themes', () => {
    expect(Object.keys(light).sort()).toEqual(Object.keys(dark).sort());
  });

  it('Auto on a light system is exactly the light theme', () => {
    expect(autoLight).toEqual(light);
  });

  it('is the only place colours are written', () => {
    const root = fileURLToPath(new URL('../../', import.meta.url));
    const files = [
      'index.html',
      ...readdirSync(`${root}src`, { recursive: true, encoding: 'utf8' })
        .filter((f) => /\.(ts|tsx|css)$/.test(f))
        .map((f) => `src/${f}`),
    ];
    const colour = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(/i;
    const offenders = files.filter((f) => colour.test(readFileSync(`${root}${f}`, 'utf8')));
    expect(files.length).toBeGreaterThan(20);
    expect(offenders).toEqual([]);
  });
});
