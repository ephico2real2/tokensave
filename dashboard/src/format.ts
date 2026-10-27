// Text formatting, translated from the helpers in the mockup's renderVals(): the API returns
// raw numbers and ISO times, and these turn them into the text the page shows.

import type { Effort } from './types';

const DAY_MS = 86_400_000;

/** Tokens in millions, one decimal: 48.2M. */
export const M1 = (tokens: number): string => `${(tokens / 1e6).toFixed(1)}M`;

/** Tokens in millions, two decimals: 2.31M. */
export const M2 = (tokens: number): string => `${(tokens / 1e6).toFixed(2)}M`;

/** Tokens in thousands below a million, else M2: 412k, 1.81M; null is a dash. */
export const K = (tokens: number | null): string =>
  tokens === null ? '—' : tokens < 1e6 ? `${Math.round(tokens / 1e3)}k` : M2(tokens);

/** US dollars with cents; null (no price for the model) is a dash. */
export const usd = (cost: number | null): string => (cost === null ? '—' : `$${cost.toFixed(2)}`);

/** A 0..1 share as a whole percentage: 82%. */
export const pct = (share: number): string => `${Math.round(share * 100)}%`;

/** A 0..1 share with one decimal: 12.5%. */
export const pct1 = (share: number): string => `${(share * 100).toFixed(1)}%`;

/** A count with thousands separators; null is a dash. */
export const int = (value: number | null): string => (value === null ? '—' : value.toLocaleString('en-US'));

/** A model name without the vendor prefix the page repeats everywhere. */
export const shortModel = (model: string): string => model.replace(/^claude-/, '');

/** Model and effort as one label: opus-5-5 · xhigh. */
export const modelLabel = (model: string, effort: Effort | null): string =>
  effort ? `${shortModel(model)} · ${effort}` : shortModel(model);

/**
 * A cell of the model × effort grid. The mockup prints whole dollars ('$' + v); real costs
 * have cents, so values from $10 up are rounded to the dollar and smaller ones keep one decimal.
 */
/** A heat cell's amount: whole dollars from $10, one decimal below. */
export const heatAmount = (cost: number): number =>
  cost >= 10 ? Math.round(cost) : Math.round(cost * 10) / 10;

export const heatUsd = (cost: number): string => `$${heatAmount(cost)}`;

/** How long ago an ISO time was, in words; null is "never". */
export function ago(iso: string | null, now: number): string {
  if (iso === null) return 'never';
  const minutes = Math.max(0, Math.round((now - Date.parse(iso)) / 60_000));
  const days = Math.floor(minutes / 1440);
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)} h ago`;
  if (days === 1) return 'yesterday';
  if (days < 7) return `${days} days ago`;
  if (days < 14) return '1 week ago';
  return `${Math.floor(days / 7)} weeks ago`;
}

/** A YYYY-MM-DD date as "Oct 26". */
export const shortDate = (iso: string): string =>
  new Date(`${iso}T00:00:00Z`).toLocaleDateString('en-US', { month: 'short', day: 'numeric', timeZone: 'UTC' });

const year = (iso: string): string => iso.slice(0, 4);

/** The range in words for the header: "Sep 27 – Oct 26, 2026 · 30 days". */
export function rangeText(from: string, to: string): string {
  const days = Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / DAY_MS) + 1;
  const start = year(from) === year(to) ? shortDate(from) : `${shortDate(from)}, ${year(from)}`;
  return `${start} – ${shortDate(to)}, ${year(to)} · ${days} ${days === 1 ? 'day' : 'days'}`;
}

/** Five axis labels at 0, ¼, ½, ¾ and the end of a list of dates. */
export function ticks(dates: string[]): string[] {
  if (dates.length === 0) return [];
  const last = dates.length - 1;
  return [0, 0.25, 0.5, 0.75, 1].map((f) => shortDate(dates[Math.round(f * last)] ?? dates[last] ?? ''));
}
