// The page's shared state, as the mockup declares it (`// state` lines of Main.dc.html):
// tab, range, from, to and the filters live in the URL; the theme lives in browser storage.
// Every value read from outside is checked, and an invalid one falls back to its default.

import { EFFORTS, TOOLS, type Effort, type Tool } from './types';

export type Tab = 'overview' | 'agents' | 'models' | 'repos' | 'savings' | 'sources';
export type Range = '7' | '30' | '45' | '60' | '90' | 'custom';
export type Theme = 'dark' | 'light' | 'auto';

export const TABS: readonly { id: Tab; label: string }[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'agents', label: 'Agents' },
  { id: 'models', label: 'Models and effort' },
  { id: 'repos', label: 'Repos' },
  { id: 'savings', label: 'Savings' },
  { id: 'sources', label: 'Sources' },
];

export const RANGES: readonly { id: Range; label: string }[] = [
  { id: '7', label: '7 days' },
  { id: '30', label: '30 days' },
  { id: '45', label: '45 days' },
  { id: '60', label: '60 days' },
  { id: '90', label: '90 days' },
  { id: 'custom', label: 'Custom' },
];

export const THEMES: readonly { id: Theme; label: string }[] = [
  { id: 'dark', label: 'Dark' },
  { id: 'light', label: 'Light' },
  { id: 'auto', label: 'Auto' },
];

export const DEFAULT_TAB: Tab = 'overview';
export const DEFAULT_RANGE: Range = '30';
export const DEFAULT_THEME: Theme = 'dark';
export const THEME_STORAGE_KEY = 'theme';

/** Empty string means "all" for every filter. */
export type Filters = { workspace: string; repo: string; tool: Tool | ''; model: string; effort: Effort | '' };

export type UrlState = { tab: Tab; range: Range; from: string; to: string } & Filters;

const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;
const DAY_MS = 86_400_000;

/** Today in the viewer's time zone, as YYYY-MM-DD. */
export function localToday(now: Date): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, '0');
  const d = String(now.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

/** A real calendar date in YYYY-MM-DD form (rejects 2026-02-30). */
export function isIsoDate(value: string): boolean {
  if (!ISO_DATE.test(value)) return false;
  const d = new Date(`${value}T00:00:00Z`);
  return !Number.isNaN(d.getTime()) && d.toISOString().slice(0, 10) === value;
}

export function addDays(iso: string, days: number): string {
  return new Date(Date.parse(`${iso}T00:00:00Z`) + days * DAY_MS).toISOString().slice(0, 10);
}

/** Inclusive number of days from `from` to `to`. */
export function daysBetween(from: string, to: string): number {
  return Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / DAY_MS) + 1;
}

/** The custom range's defaults: the 30 days ending today (state from: default=today-29). */
export function defaultCustom(today: string): { from: string; to: string } {
  return { from: addDays(today, -29), to: today };
}

/** Keeps a custom range valid: To no later than today, From no later than To. */
export function clampCustom(from: string, to: string, today: string): { from: string; to: string } {
  const end = to > today ? today : to;
  return { from: from > end ? end : from, to: end };
}

function oneOf<T extends string>(value: string | null, allowed: readonly T[]): T | undefined {
  return value !== null && (allowed as readonly string[]).includes(value) ? (value as T) : undefined;
}

export function parseUrlState(search: string, today: string): UrlState {
  const q = new URLSearchParams(search);
  const tab = oneOf(q.get('tab'), TABS.map((t) => t.id)) ?? DEFAULT_TAB;
  const range = oneOf(q.get('range'), RANGES.map((r) => r.id)) ?? DEFAULT_RANGE;
  let { from, to } = defaultCustom(today);
  if (range === 'custom') {
    const qFrom = q.get('from') ?? '';
    const qTo = q.get('to') ?? '';
    if (isIsoDate(qFrom) && isIsoDate(qTo) && qFrom <= qTo && qTo <= today) {
      from = qFrom;
      to = qTo;
    }
  }
  return {
    tab,
    range,
    from,
    to,
    workspace: q.get('workspace') ?? '',
    repo: q.get('repo') ?? '',
    tool: oneOf(q.get('tool'), TOOLS) ?? '',
    model: q.get('model') ?? '',
    effort: oneOf(q.get('effort'), EFFORTS) ?? '',
  };
}

/** The query string for a state: defaults are left out, so a plain link opens the defaults. */
export function serializeUrlState(s: UrlState): string {
  const q = new URLSearchParams();
  if (s.tab !== DEFAULT_TAB) q.set('tab', s.tab);
  if (s.range !== DEFAULT_RANGE) q.set('range', s.range);
  if (s.range === 'custom') {
    q.set('from', s.from);
    q.set('to', s.to);
  }
  for (const key of ['workspace', 'repo', 'tool', 'model', 'effort'] as const) {
    if (s[key]) q.set(key, s[key]);
  }
  const text = q.toString();
  return text ? `?${text}` : '';
}

/**
 * The date-range part of every API query: a preset sends `range`, a custom range sends
 * `from` and `to` (the server validates them and answers 400 naming a bad parameter).
 */
export function rangeQuery(s: Pick<UrlState, 'range' | 'from' | 'to'>): Record<string, string> {
  return s.range === 'custom' ? { from: s.from, to: s.to } : { range: s.range };
}

export function readStoredTheme(): Theme | undefined {
  try {
    return oneOf(window.localStorage.getItem(THEME_STORAGE_KEY), THEMES.map((t) => t.id));
  } catch {
    return undefined;
  }
}

export function storeTheme(theme: Theme): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Storage can be off (private window, blocked site data): the choice then lasts for this visit.
  }
}
